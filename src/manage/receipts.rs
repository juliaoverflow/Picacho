//! Receipts in mail: what a message says was paid, found without contacting
//! anyone, for each Summary's Receipts section.
//!
//! A message is a receipt when its sender's schema.org `Order` or `Invoice`
//! markup says so (`from_markup`), or when its subject says a purchase was
//! made or money given back (`PURCHASE`, `REFUND`: "Your receipt from…",
//! "Order confirmation", "Payment received", "Refund issued",
//! "Rechnung"…). Either way not when its subject says it is something
//! else (`NOT_RECEIPT`: a reminder of money owed, an order about to ship,
//! an offer, a reply), nor when a bank or card sent it or was paid (a
//! card's bill paid is no purchase: what it paid for was). Its total is the markup's price, or else
//! the amount after the strongest "total" label in its text ("Order total:
//! $54.20" over "Total: $54.20" over "Amount: $54.20"), with its currency
//! read from a symbol or code beside it.
//!
//! `summarize` gathers what every message says into one receipt per order:
//! a confirmation and the receipt sent after it are one purchase.
use crate::manage::parcels::Sighting;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::LazyLock;

/// An amount of money, in hundredths of its currency's unit (also for
/// currencies without a minor unit, such as yen, which `Amount::show`
/// writes without decimals).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Amount {
    pub cents: i64,
    /// Its ISO 4217 code ("USD").
    pub currency: String,
}

impl Amount {
    /// As the Summary writes it: "$54.20", "€1,234.00", "¥1,200", "CHF 12.50".
    pub fn show(&self) -> String {
        let negative = self.cents < 0;
        let cents = self.cents.unsigned_abs();
        let whole = group_thousands(cents / 100);
        let number = if no_minor_unit(&self.currency) {
            whole
        } else {
            format!("{whole}.{:02}", cents % 100)
        };
        let sign = if negative { "−" } else { "" };
        match symbol_of(&self.currency) {
            Some(symbol) => format!("{sign}{symbol}{number}"),
            None => format!("{sign}{} {number}", self.currency),
        }
    }
}

/// What a receipt is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Kind {
    /// A purchase, order or payment made.
    Purchase,
    /// Money given back: counted against what was spent.
    Refund,
}

/// Where a receipt was found.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Source {
    /// The sender's schema.org `Order` or `Invoice` markup.
    Markup,
    /// The message's subject and text.
    Text,
}

/// A receipt a message is.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Receipt {
    /// Who was paid, when the markup names them; else the sender is.
    pub merchant: Option<String>,
    /// What was paid in all, when the message says.
    pub total: Option<Amount>,
    /// The order or invoice it is for, when the message gives its number.
    pub order: Option<String>,
    pub kind: Kind,
    pub source: Source,
}

/// A receipt as each Summary lists it: one purchase, and every email about
/// it, newest first.
#[derive(Clone, Debug, PartialEq)]
pub struct Listed {
    pub receipt: Receipt,
    /// Who was paid: the markup's merchant, or the sender's name.
    pub merchant: String,
    /// When it was paid: its first email's date (Unix seconds).
    pub date: i64,
    pub emails: Vec<Sighting>,
    /// Hidden by hand (see `apply_hidden`).
    pub hidden: bool,
}

impl Listed {
    /// What it is known by when hidden by hand: its order and merchant, or
    /// else its first email.
    pub fn key(&self) -> String {
        match &self.receipt.order {
            Some(order) => format!("order:{}:{order}", self.merchant.to_lowercase()),
            None => format!(
                "mail:{}",
                self.emails.last().map_or("", |s| s.message_id.as_str())
            ),
        }
    }
    /// The email it was found in first (its emails are newest first).
    pub fn first(&self) -> &Sighting {
        self.emails.last().expect("a receipt has an email")
    }
}

/// The preference holding the receipts hidden by hand, by key (a JSON
/// list): hidden from the Summary, and not found by `has:receipt`.
pub const HIDDEN_PREFERENCE: &str = "hidden_receipts";

/// Marks `listed` hidden by their keys.
pub fn apply_hidden(listed: &mut [Listed], hidden: &[String]) {
    for l in listed {
        l.hidden = hidden.contains(&l.key());
    }
}

/// What was spent in each currency, refunds taken off, among `listed`;
/// in the order the currencies first come.
pub fn totals<'a>(listed: impl IntoIterator<Item = &'a Listed>) -> Vec<Amount> {
    let mut out: Vec<Amount> = Vec::new();
    for l in listed {
        let Some(total) = &l.receipt.total else {
            continue;
        };
        let cents = match l.receipt.kind {
            Kind::Purchase => total.cents,
            Kind::Refund => -total.cents,
        };
        match out.iter_mut().find(|a| a.currency == total.currency) {
            Some(a) => a.cents += cents,
            None => out.push(Amount {
                cents,
                currency: total.currency.clone(),
            }),
        }
    }
    out
}

/// A message's text as `find` reads it.
pub struct MailText<'a> {
    pub subject: &'a str,
    /// Its plain text, or the text of its HTML, or its preview.
    pub text: &'a str,
    pub html: Option<&'a str>,
    /// Its sender, as "Name <address>".
    pub from: &'a str,
}

/// The receipt a message is, if it is one.
pub fn find(mail: &MailText<'_>) -> Option<Receipt> {
    let subject = mail.subject;
    if rejected(subject, mail.from) {
        return None;
    }
    let marked = mail.html.and_then(from_markup);
    let kind = subject_kind(subject);
    if marked.is_none() && kind.is_none() {
        return None;
    }
    let mut receipt = marked.unwrap_or(Receipt {
        merchant: None,
        total: None,
        order: None,
        kind: Kind::Purchase,
        source: Source::Text,
    });
    if kind == Some(Kind::Refund) {
        receipt.kind = Kind::Refund;
    }
    if receipt.merchant.is_none() {
        receipt.merchant = payee_in(subject);
    }
    // Paying a card's bill isn't spending: what it paid for was.
    if receipt.merchant.as_deref().is_some_and(card_issuer) {
        return None;
    }
    if receipt.total.is_none() {
        receipt.total = total_in(subject, mail.text);
    }
    // A free game's or trial's "receipt" spent nothing.
    if receipt.total.as_ref().is_some_and(|t| t.cents == 0) {
        return None;
    }
    if receipt.order.is_none() {
        receipt.order = crate::manage::parcels::order_numbers(subject, mail.text)
            .into_iter()
            .next();
    }
    Some(receipt)
}

/// Whether a message may be a receipt by what is known before it is read
/// whole: its subject reads like one, or its sender sends them.
pub fn worth_reading(subject: &str, from: &str) -> bool {
    if rejected(subject, from) {
        return false;
    }
    if subject_kind(subject).is_some() {
        return true;
    }
    let from = from.to_lowercase();
    RECEIPT_SENDERS.iter().any(|s| from.contains(s))
}

/// Whether mail known only by its headers is worth downloading to read
/// its total: its subject says it is a receipt.
pub fn worth_downloading(subject: &str, from: &str) -> bool {
    !rejected(subject, from) && subject_kind(subject).is_some()
}

/// Senders whose mail is often a receipt, its markup saying so though its
/// subject doesn't.
const RECEIPT_SENDERS: &[&str] = &[
    "receipt",
    "invoice",
    "billing@",
    "payments@",
    "orders@",
    "order@",
    "order-confirm",
    "auto-confirm@amazon",
];

/// Subjects that make a message a purchase's receipt, lowercase: phrases
/// said only of a purchase made, each with word boundaries ("receipt" but
/// not "PleaseConfirmReceipt"), in several languages.
static PURCHASE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?ix)
        \breceipts?\b
        | \binvoice\b
        | ^ordered:
        | \border\s+confirm(?:ation|ed)\b
        | \bconfirm(?:ation)?\s+of\s+(?:your\s+)?order\b
        | \border\b[^.!?]{0,40}\b(?:is|has\s+been|was)\s+(?:confirmed|received|placed|processed)\b
        | \border\s+(?:received|placed)\b
        | \border\s+(?:no\.?\s*|number\s*)?\#?\s*[a-z0-9][a-z0-9-]{3,30}:?\s+(?:is\s+|has\s+been\s+)?(?:confirmed|received|placed)\b
        | \b(?:we|we've|we\s+have|we\s+just)\s+(?:received|got)\s+your\s+order\b
        | \bthanks?\s+(?:you\s+)?for\s+(?:your|the)\s+(?:\w+\s+)?(?:order|purchase)\b
        | \bpurchase\s+confirm
        | \bpre-?\s?order\s+(?:purchase\s+)?confirm
        | \bprocessing\s+your\s+order\b
        | \bpayment\s+(?:received|confirm(?:ation|ed)?|successful|complete[d]?|processed)\b
        | \byour\s+payment\b[^.!?]{0,40}\b(?:is|was|has\s+been)\s+(?:complete|successful|received|processed)\b
        | \byou\s+(?:paid|sent\s+a\s+payment)\b
        | \bbooking\s+confirm
        | \bquittung\b | \brechnung\b | \bbestellbestätigung\b | \bzahlungsbestätigung\b
        | \brecibo\b | \bfactura\b | \bconfirmación\s+de\s+(?:pedido|compra)\b
        | \breçu\b | \bfacture\b | \bconfirmation\s+de\s+commande\b
        | \bricevuta\b | \bfattura\b | \bconferma\s+d'ordine\b
        | \bfatura\b | \bbevestiging\s+van\s+(?:je|uw)\s+bestelling\b
        ",
    )
    .expect("pattern")
});

/// Subjects that make a message a refund's receipt: money given back, not
/// asked for ("refund request") nor offered ("your tax refund").
static REFUND: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?ix)
        \brefund\s+(?:issued|confirm(?:ation|ed)?|notification|successful|processed|complete[d]?|on\s+order|for\b|from\b)
        | \brefund\s+(?:has|was|is)\s+(?:been\s+)?(?:issued|processed|complete|sent)
        | \b(?:a|your)\s+(?:\S+\s+)?refund\s+(?:has|was|is)\s+(?:been\s+)?(?:issued|processed|complete|sent|on\s+its\s+way)
        | \byou\s+have\s+a\s+refund\b
        | \bsending\s+you\s+a\s+refund\b
        | \b(?:has\s+been|was|been)\s+refunded\b
        | \berstattung\b | \breembolso\b | \bremboursement\b | \brimborso\b
        ",
    )
    .expect("pattern")
});

/// Words in a subject that make a message no receipt, though it reads like
/// one: money still owed or not taken, a reminder, an order still to be
/// made or on its way (the Packages section's), an offer, a reward, a
/// card's bill, spam.
static NOT_RECEIPT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?ix)
        ^\s*(?:re|fwd?|aw|wg|tr|sv|vs)\s*(?:\[\d+\])?\s*:
        | \b(?:read|return|delivery)\s+receipt\b | \bconfirm\s+receipt\b | \breceipt\s+confirmation\b
        | \bplease\s+confirm\b | \blesebestätigung\b
        | \boverdue\b | \bpast\s+due\b | \bdue\s+(?:soon|today|tomorrow|on|by)\b | \bis\s+due\b
        | \bpayment\s+due\b | \bupcoming\b | \bcoming\s+up\b | \bscheduled\b | \bstatement\b
        | \?
        | \breceipt\s+(?:is|was|has\s+been)\s+(?:received|processed|submitted)\b
        | \bfail(?:ed|ure)?\b | \bdeclined\b | \bunsuccessful\b | \bunable\b | \bcouldn't\b | \bcould\s+not\b
        | \breminder\b | \berinnerung\b | \bmahnung\b | \baction\s+required\b | \bneeds\s+attention\b
        | \bupdate\s+your\s+payment\b | \bverify\s+your\b | \baccount\s+recovery\b
        | \bcancel
        | \b(?:ready|about|preparing)\s+to\s+ship\b | \bwill\s+(?:be\s+)?ship | \bship(?:s|ping)?\s+soon\b
        | \b(?:being|been|was)\s+prepared\b | \bshipping\s+update\b | \breturn\s+ship
        | \bcomplete\s+your\b | \bbefore\s+you\s+forget\b | \bmodified\s+your\s+order\b
        | \bimportant\s+information\b | \bwhat's\s+next\b
        | \brefund\s+request\b | \btax\s+refund\b
        | \b(?:\d+%|[$€£]\d+)\s+off\b | \bsave\s+\d | \bdeals?\b | \bsale\b
        | \bwon\b | \bwinner\b | \bwin\b | \battempt\b | \bcongratulations\b | \brewards?\b | \bprize\b
        | \bwithdrawal\b | \batm\b | \bfunds?\s+added\b
        | \bsent\s+your\s+invoice\b | \byour\s+invoice\s+to\b | \bto\s+pay\s+your\s+invoice\b | \bpaid\s+your\s+invoice\b
        ",
    )
    .expect("pattern")
});

/// Whether a message is no receipt whatever it says: its subject says so
/// (`NOT_RECEIPT`), it is news of a parcel on its way, or its sender is a
/// bank or card (a card's bill paid is no purchase).
fn rejected(subject: &str, from: &str) -> bool {
    if NOT_RECEIPT.is_match(subject) {
        return true;
    }
    // "Your refund from … is on the way" is money, not a parcel.
    if !REFUND.is_match(subject)
        && crate::manage::parcels::status_in(subject, "")
            .is_some_and(|st| st != crate::manage::parcels::Status::Processing)
    {
        return true;
    }
    card_issuer(from) || card_issuer(subject) || free_mail(from) || mixed_scripts(subject)
}

/// Mailboxes anyone can open: shops don't send receipts from them, but
/// the spam passing for receipts ("Your Order#ABC-1234" from a stranger's
/// iCloud address) does.
const FREE_MAIL: &[&str] = &[
    "gmail.com",
    "googlemail.com",
    "icloud.com",
    "me.com",
    "mac.com",
    "outlook.com",
    "hotmail.com",
    "live.com",
    "msn.com",
    "yahoo.com",
    "ymail.com",
    "aol.com",
    "gmx.com",
    "gmx.net",
    "gmx.de",
    "mail.com",
    "proton.me",
    "protonmail.com",
    "pm.me",
    "yandex.com",
    "yandex.ru",
    "mail.ru",
    "qq.com",
    "163.com",
];

/// Whether a sender ("Name <address>", or the address) writes from a free
/// mailbox.
fn free_mail(from: &str) -> bool {
    let address = match (from.rfind('<'), from.rfind('>')) {
        (Some(a), Some(b)) if a < b => &from[a + 1..b],
        _ => from,
    };
    let Some((_, domain)) = address.trim().rsplit_once('@') else {
        return false;
    };
    let domain = domain.to_lowercase();
    FREE_MAIL.contains(&domain.as_str())
}

/// Whether a word of a subject mixes Latin letters with Cyrillic or Greek
/// ones that look alike ("Confirmаtion" with a Cyrillic а): spam does it
/// to get past filters; real mail never does.
fn mixed_scripts(subject: &str) -> bool {
    subject.split(|c: char| !c.is_alphabetic()).any(|word| {
        let latin = word.chars().any(|c| c.is_ascii_alphabetic());
        let other = word
            .chars()
            .any(|c| matches!(c, '\u{0370}'..='\u{03FF}' | '\u{0400}'..='\u{04FF}'));
        latin && other
    })
}

/// What a subject says the message is a receipt of, if it says.
fn subject_kind(subject: &str) -> Option<Kind> {
    if REFUND.is_match(subject) {
        Some(Kind::Refund)
    } else if PURCHASE.is_match(subject) {
        Some(Kind::Purchase)
    } else {
        None
    }
}

/// Banks and cards, by words in their names: mail from them, or a payment
/// to them, pays a bill for what was bought elsewhere.
const CARD_ISSUERS: &[&str] = &[
    "bank",
    "mastercard",
    "visa",
    "credit card",
    "capital one",
    "synchrony",
    "chase",
    "discover card",
    "american express",
    "amex",
    "citi card",
    "citibank",
    "credit union",
    "barclays",
    "wells fargo",
    "card services",
    "secured card",
    "credit karma",
    "affirm",
    "klarna",
    "afterpay",
    "pay in 4",
];

/// Whether a name or sender is a bank's or a card's.
fn card_issuer(name: &str) -> bool {
    let name = name.to_lowercase();
    CARD_ISSUERS.iter().any(|w| {
        name.match_indices(w).any(|(i, _)| {
            let before = name[..i].chars().next_back();
            let after = name[i + w.len()..].chars().next();
            !before.is_some_and(char::is_alphanumeric) && !after.is_some_and(char::is_alphanumeric)
        })
    })
}

/// Who a subject says was paid: "Receipt for your payment to Example Games Inc.",
/// "Receipt: you paid Acme Inc", "You have a refund from Example Home", "Your
/// receipt from Example Store.", "Invoice #C1234 from ACME".
static PAYEE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(?:payment\s+to|you\s+paid|refund\s+from|receipt\s+from|invoice\s+(?:\S+\s+)?from|order\s+from|purchase\s+from)\s+(?P<name>[^#|()\[\]]{2,60}?)\s*(?:[#|(\[]|[.!]?\s*$|\s+-\s|\s+(?:is|has|was|are|have)\s)",
    )
    .expect("pattern")
});

/// Who was paid, as the subject names them.
fn payee_in(subject: &str) -> Option<String> {
    let name = PAYEE.captures(subject)?["name"]
        .trim()
        .trim_end_matches(['.', ','])
        .to_owned();
    // Not a date ("Order Receipt from Mar 3, 2025") nor an amount.
    let date = MONTHS.iter().any(|m| name.to_lowercase().starts_with(m));
    let named = name.chars().next().is_some_and(char::is_alphabetic);
    (named && !date && !name.contains('$')).then_some(name)
}

/// Months as they begin a date in a subject.
const MONTHS: [&str; 12] = [
    "jan", "feb", "mar", "apr", "may ", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
];

/// Currency signs and codes before or after an amount, longest first, with
/// the code each stands for.
const CURRENCIES: &[(&str, &str)] = &[
    ("US$", "USD"),
    ("CA$", "CAD"),
    ("AU$", "AUD"),
    ("NZ$", "NZD"),
    ("HK$", "HKD"),
    ("R$", "BRL"),
    ("C$", "CAD"),
    ("A$", "AUD"),
    ("$", "USD"),
    ("€", "EUR"),
    ("£", "GBP"),
    ("¥", "JPY"),
    ("₹", "INR"),
    ("₩", "KRW"),
    ("zł", "PLN"),
];
/// Codes taken as they are, beside an amount.
const CODES: &[&str] = &[
    "USD", "EUR", "GBP", "CAD", "AUD", "NZD", "JPY", "CHF", "INR", "SEK", "NOK", "DKK", "PLN",
    "BRL", "MXN", "HKD", "SGD", "KRW", "CNY", "CZK", "HUF", "ZAR", "TRY",
];

/// An amount with its currency on either side: "$1,234.56", "12,50 €",
/// "EUR 12.50", "USD $12.50", "-$5.00".
static MONEY: LazyLock<Regex> = LazyLock::new(|| {
    let signs = r"US\$|CA\$|AU\$|NZ\$|HK\$|R\$|C\$|A\$|\$|€|£|¥|₹|₩|zł";
    let codes = CODES.join("|");
    Regex::new(&format!(
        r"(?i)^[\s:\-–=]*(?:(?P<pre>(?:{codes})\s?(?:{signs})?|{signs})\s?)?(?P<neg>[-−])?\s?(?:(?P<pre2>{signs})\s?)?(?P<num>\d{{1,3}}(?:[,.' \u{{a0}}]\d{{3}})+(?:[.,]\d{{1,2}})?|\d+(?:[.,]\d{{1,2}})?)(?:\s?(?P<post>{signs}|\b(?:{codes})\b))?"
    ))
    .expect("pattern")
});

/// Labels before a total, strongest first: a message giving "Total" for
/// each item and "Order total" once means the latter.
const TOTAL_LABELS: &[&[&str]] = &[
    &[
        "grand total",
        "order total",
        "total charged",
        "amount charged",
        "amount billed",
        "total billed",
        "total paid",
        "amount paid",
        "total amount",
        "payment amount",
        "refund total",
        "total refund",
        "refund amount",
        "amount refunded",
        "you paid",
        "you sent",
        "gesamtbetrag",
        "rechnungsbetrag",
        "importe total",
        "total ttc",
        "montant total",
        "importo totale",
        "valor total",
        "totaalbedrag",
    ],
    &["total", "gesamt", "summe", "totale", "totaal", "montant"],
    &["amount", "betrag", "importe", "importo", "valor"],
];

/// A label, then up to a few words or a parenthesis before the amount
/// ("Total (2 items):", "Order total incl. VAT:").
static LABEL: LazyLock<Regex> = LazyLock::new(|| {
    let labels: Vec<String> = TOTAL_LABELS
        .iter()
        .flat_map(|l| l.iter())
        .map(|l| regex::escape(l))
        .collect();
    Regex::new(&format!(
        r"(?i)\b(?P<label>{})\b\*?(?:\s*\([^)\n]{{0,30}}\))?(?:[ \t]+(?:incl\.?|including|inkl\.?|with|after)[^:\n\d$€£¥]{{0,24}})?[ \t]*:?",
        labels.join("|")
    ))
    .expect("pattern")
});

/// How much of a message's text is read for its total: receipts give it
/// near the top; their footers' small print is noise.
const TOTAL_TEXT: usize = 20_000;

/// The total a receipt's subject or text gives: the amount after its
/// strongest label (the last of those, as items come before their sum).
fn total_in(subject: &str, text: &str) -> Option<Amount> {
    let head: String = text.chars().take(TOTAL_TEXT).collect();
    let mut best: Option<(usize, Amount)> = None;
    for s in [head.as_str(), subject] {
        for caps in LABEL.captures_iter(s) {
            let label = caps["label"].to_lowercase();
            let Some(rank) = TOTAL_LABELS
                .iter()
                .position(|l| l.contains(&label.as_str()))
            else {
                continue;
            };
            let start = caps.get(0).map_or(0, |m| m.start());
            let end = caps.get(0).map_or(0, |m| m.end());
            let Some(amount) = column_amount(s, start, end).or_else(|| amount_after(&s[end..]))
            else {
                continue;
            };
            if best.as_ref().is_none_or(|(r, _)| rank <= *r) {
                best = Some((rank, amount));
            }
        }
        if best.is_some() {
            break;
        }
    }
    best.map(|(_, a)| a)
}

/// Words that may stand between a total's label and its amount: "Total /
/// includes all fees, taxes and discounts $86.12".
static FILLER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^\s*(?:incl\.?|includ\w*|inkl\.?|with|after|before)\b[^\n\d$€£¥:]{0,60}:?")
        .expect("pattern")
});

/// The amount just after a label (maybe on the next line, a table's
/// cell), or after a few words saying what it includes.
fn amount_after(rest: &str) -> Option<Amount> {
    let near: String = rest.chars().take(40).collect();
    money_at(&near).or_else(|| {
        let skipped = FILLER.find(rest)?.end();
        let near: String = rest[skipped..].chars().take(40).collect();
        money_at(&near)
    })
}

/// A label in a column of labels whose amounts follow in a column of their
/// own: "Production / Shipping / Sub-total / Sales tax / Total" then "USD
/// 31.40 / USD 6.50 / USD 37.90 / USD 0.00 / USD 37.90". The label's
/// amount is the one at its place in the column. None when the label isn't
/// in such a column (it isn't alone on its line, no label comes before it,
/// or no amount after it).
fn column_amount(s: &str, start: usize, end: usize) -> Option<Amount> {
    let line_start = s[..start].rfind('\n').map_or(0, |i| i + 1);
    let line_end = s[end..].find('\n').map_or(s.len(), |i| end + i);
    // The label is its line, alone.
    if !s[line_start..start].trim().is_empty() || !s[end..line_end].trim().is_empty() {
        return None;
    }
    // The labels above it, up to an amount, a long line or the top; blank
    // lines between them are left out.
    let place = s[..line_start]
        .lines()
        .rev()
        .filter(|l| !l.trim().is_empty())
        .take_while(|l| column_label(l))
        .take(12)
        .count();
    if place == 0 {
        return None;
    }
    // The amounts below it, each its own line ("Free" in an amount's place).
    let values: Vec<Option<Amount>> = s[line_end..]
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map_while(|l| {
            let l = l.trim();
            if FREE.is_match(l) {
                Some(None)
            } else {
                money_at(l).filter(|_| l.chars().count() <= 24).map(Some)
            }
        })
        .take(13)
        .collect();
    if values.is_empty() {
        return None;
    }
    // More labels than amounts: the first are headings ("Summary") over
    // the column, so the two columns are lined up from the bottom.
    let headings = (place + 1).saturating_sub(values.len());
    values[place - headings].clone()
}

/// What stands in an amount's place in a column: nothing charged.
static FREE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(?:free|included|n/a|—|-|gratis|kostenlos|gratuit)$").expect("pattern")
});

/// Whether a line is a label in a column of them: a few words, no amount.
fn column_label(line: &str) -> bool {
    let line = line.trim();
    line.chars().count() <= 32
        && line.chars().any(char::is_alphabetic)
        && !FREE.is_match(line)
        && !MONEY_ANYWHERE.is_match(line)
}

/// An amount anywhere in a line.
static MONEY_ANYWHERE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:[$€£¥₹₩]|\b(?:usd|eur|gbp|cad|aud|jpy|chf)\b)\s?-?\d|\d\s?(?:€|\b(?:usd|eur|gbp)\b)")
        .expect("pattern")
});

/// The amount, with a currency, at the start of `s`.
fn money_at(s: &str) -> Option<Amount> {
    let caps = MONEY.captures(s)?;
    let sign = caps
        .name("pre")
        .or_else(|| caps.name("pre2"))
        .or_else(|| caps.name("post"))?
        .as_str();
    let currency = currency_of(sign)?;
    // A refund's "-$5.00" is still five back: the sign is left out.
    let cents = parse_number(&caps["num"])?;
    Some(Amount { cents, currency })
}

/// The code a sign or code beside an amount stands for: "USD $" is USD.
fn currency_of(sign: &str) -> Option<String> {
    let sign = sign.trim();
    let upper = sign.to_uppercase();
    if let Some(code) = CODES.iter().find(|c| upper.starts_with(*c)) {
        return Some((*code).to_owned());
    }
    CURRENCIES
        .iter()
        .find(|(s, _)| sign.eq_ignore_ascii_case(s))
        .map(|(_, code)| (*code).to_owned())
}

/// A number as receipts write it, in hundredths: "1,234.56", "1.234,56",
/// "1 234,56", "12,5", "1200".
fn parse_number(raw: &str) -> Option<i64> {
    let raw = raw.trim();
    // The last separator is the decimal one when one or two digits follow.
    let decimal = raw
        .rfind(['.', ','])
        .filter(|&i| (1..=2).contains(&(raw.len() - i - 1)));
    let (whole, fraction) = match decimal {
        Some(i) => (&raw[..i], &raw[i + 1..]),
        None => (raw, ""),
    };
    let whole: String = whole.chars().filter(char::is_ascii_digit).collect();
    let whole: i64 = if whole.is_empty() {
        0
    } else {
        whole.parse().ok()?
    };
    let fraction: i64 = match fraction.len() {
        0 => 0,
        1 => fraction.parse::<i64>().ok()? * 10,
        _ => fraction.parse().ok()?,
    };
    whole.checked_mul(100)?.checked_add(fraction)
}

/// Whether a currency is written without decimals.
fn no_minor_unit(code: &str) -> bool {
    matches!(code, "JPY" | "KRW" | "HUF")
}

/// The sign the Summary writes a currency with, when it has a clear one.
fn symbol_of(code: &str) -> Option<&'static str> {
    Some(match code {
        "USD" => "$",
        "EUR" => "€",
        "GBP" => "£",
        "JPY" => "¥",
        "INR" => "₹",
        "KRW" => "₩",
        "CAD" => "CA$",
        "AUD" => "A$",
        "BRL" => "R$",
        _ => return None,
    })
}

/// "1234567" as "1,234,567".
fn group_thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, d) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(d);
    }
    out
}

/// JSON-LD blocks in HTML.
static JSON_LD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?is)<script\b[^>]*\btype\s*=\s*["']?application/ld\+json["']?[^>]*>(.*?)</script\s*>"#,
    )
    .expect("pattern")
});
/// How deep the markup is followed.
const MAX_DEPTH: usize = 8;

/// The receipt a message's schema.org `Order` or paid `Invoice` markup
/// tells of. An order whose status says it is on its way or delivered is
/// shipping news, not a receipt.
pub fn from_markup(html: &str) -> Option<Receipt> {
    for caps in JSON_LD.captures_iter(html) {
        let raw = caps[1]
            .trim()
            .trim_start_matches("<!--")
            .trim_end_matches("-->")
            .trim_start_matches("<![CDATA[")
            .trim_end_matches("]]>")
            .trim();
        let Ok(value) = serde_json::from_str::<Value>(raw) else {
            continue;
        };
        if let Some(r) = walk(&value, 0) {
            return Some(r);
        }
    }
    None
}

/// The first `Order` or paid `Invoice` in `value`.
fn walk(value: &Value, depth: usize) -> Option<Receipt> {
    if depth > MAX_DEPTH {
        return None;
    }
    match value {
        Value::Array(items) => items.iter().find_map(|v| walk(v, depth + 1)),
        Value::Object(map) => {
            if is_type(value, "Order") && order_is_receipt(value) {
                return Some(Receipt {
                    merchant: map
                        .get("seller")
                        .or_else(|| map.get("merchant"))
                        .and_then(name_of),
                    total: price_of(value)
                        .or_else(|| map.get("priceSpecification").and_then(price_of))
                        .or_else(|| map.get("totalPrice").and_then(price_of)),
                    order: map.get("orderNumber").and_then(text_of),
                    kind: Kind::Purchase,
                    source: Source::Markup,
                });
            }
            if is_type(value, "Invoice") && invoice_is_paid(value) {
                return Some(Receipt {
                    merchant: map
                        .get("provider")
                        .or_else(|| map.get("broker"))
                        .and_then(name_of),
                    total: map
                        .get("totalPaymentDue")
                        .or_else(|| map.get("minimumPaymentDue"))
                        .and_then(price_of),
                    order: map
                        .get("confirmationNumber")
                        .or_else(|| map.get("accountId"))
                        .and_then(text_of),
                    kind: Kind::Purchase,
                    source: Source::Markup,
                });
            }
            map.values().find_map(|v| walk(v, depth + 1))
        }
        _ => None,
    }
}

/// Whether `value`'s `@type` is (or lists) `name`.
fn is_type(value: &Value, name: &str) -> bool {
    let matches = |t: &Value| {
        t.as_str()
            .is_some_and(|t| t.rsplit(['/', ':']).next() == Some(name))
    };
    match value.get("@type") {
        Some(Value::Array(types)) => types.iter().any(matches),
        Some(t) => matches(t),
        None => false,
    }
}

/// Whether an `Order` is a purchase made: not one on its way, delivered,
/// returned or cancelled.
fn order_is_receipt(value: &Value) -> bool {
    let status = value
        .get("orderStatus")
        .and_then(text_of)
        .unwrap_or_default();
    let status = status.rsplit('/').next().unwrap_or_default();
    matches!(
        status,
        "" | "OrderProcessing" | "OrderPaymentDue" | "OrderPickupAvailable"
    ) || status.eq_ignore_ascii_case("processing")
}

/// Whether an `Invoice` was paid.
fn invoice_is_paid(value: &Value) -> bool {
    value
        .get("paymentStatus")
        .and_then(text_of)
        .is_some_and(|s| {
            let s = s.rsplit('/').next().unwrap_or_default();
            s == "PaymentComplete" || s == "PaymentAutomaticallyApplied"
        })
}

/// A string, or a number written as one.
fn text_of(value: &Value) -> Option<String> {
    let s = match value {
        Value::String(s) => s.trim().to_owned(),
        Value::Number(n) => n.to_string(),
        _ => return None,
    };
    (!s.is_empty()).then_some(s)
}

/// An organization's name, or the string it is given as.
fn name_of(value: &Value) -> Option<String> {
    match value {
        Value::Object(map) => map.get("name").and_then(text_of),
        other => text_of(other),
    }
}

/// A `price` and `priceCurrency` (on an object, or a PriceSpecification).
fn price_of(value: &Value) -> Option<Amount> {
    let map = value.as_object()?;
    let price = map.get("price").or_else(|| map.get("value"))?;
    let currency = map
        .get("priceCurrency")
        .or_else(|| map.get("currency"))
        .and_then(text_of)?
        .to_uppercase();
    let raw = text_of(price)?;
    // Markup writes "1234.5", with a point.
    let cents = match raw.split_once('.') {
        Some((w, f)) => {
            let f = format!("{f:0<2}");
            w.parse::<i64>().ok()? * 100 + f.get(..2)?.parse::<i64>().ok()?
        }
        None => raw.parse::<i64>().ok()? * 100,
    };
    (currency.len() == 3).then_some(Amount { cents, currency })
}

/// A sender as a receipt names it: its name, or else its address's domain
/// without "mail." and the like.
pub fn sender_name(sender: &str) -> String {
    let name = match sender.find('<') {
        Some(i) => sender[..i].trim(),
        None if sender.contains('@') => "",
        None => sender.trim(),
    }
    .trim_matches('"')
    .trim();
    if !name.is_empty() {
        return name.to_owned();
    }
    let address = sender
        .rsplit('<')
        .next()
        .unwrap_or(sender)
        .trim_end_matches('>');
    let domain = address.rsplit('@').next().unwrap_or(address);
    let parts: Vec<&str> = domain.split('.').collect();
    // "email.shop.example.com" → "shop.example.com"; "shop.com" stays.
    let skip = parts
        .iter()
        .take_while(|p| {
            matches!(
                **p,
                "mail" | "email" | "e" | "em" | "info" | "news" | "m" | "send" | "notify"
            )
        })
        .count()
        .min(parts.len().saturating_sub(2));
    parts[skip..].join(".")
}

/// The receipts in `found`, newest first: one per order and merchant (a
/// confirmation and the receipt after it are one purchase), else one per
/// email; the same message in two accounts once.
pub fn summarize(found: Vec<(Receipt, Sighting)>) -> Vec<Listed> {
    let mut out: Vec<Listed> = Vec::new();
    let mut by_order: HashMap<String, usize> = HashMap::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut found = found;
    // Oldest first, so each purchase is dated by its first email.
    found.sort_by_key(|(_, s)| s.sent);
    for (receipt, sighting) in found {
        if !sighting.message_id.is_empty() && !seen.insert(sighting.message_id.clone()) {
            continue;
        }
        let merchant = receipt
            .merchant
            .clone()
            .unwrap_or_else(|| sender_name(&sighting.sender));
        let key = receipt
            .order
            .as_ref()
            .map(|o| format!("{}\n{o}\n{:?}", merchant.to_lowercase(), receipt.kind));
        if let Some(i) = key.as_ref().and_then(|k| by_order.get(k)) {
            let l = &mut out[*i];
            // A later email's total and merchant fill in what was missing;
            // its total replaces an earlier one (an order's final receipt).
            if receipt.total.is_some() {
                l.receipt.total = receipt.total;
            }
            if l.receipt.merchant.is_none() {
                l.receipt.merchant = receipt.merchant;
            }
            if receipt.source == Source::Markup {
                l.receipt.source = Source::Markup;
            }
            l.emails.insert(0, sighting);
            continue;
        }
        if let Some(k) = key {
            by_order.insert(k, out.len());
        }
        out.push(Listed {
            receipt,
            merchant,
            date: sighting.sent,
            emails: vec![sighting],
            hidden: false,
        });
    }
    out.sort_by(|a, b| {
        b.date
            .cmp(&a.date)
            .then_with(|| a.merchant.cmp(&b.merchant))
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(subject: &str, body: &str) -> Option<Receipt> {
        find(&MailText {
            subject,
            text: body,
            html: None,
            from: "Shop <orders@shop.example>",
        })
    }

    fn usd(cents: i64) -> Option<Amount> {
        Some(Amount {
            cents,
            currency: "USD".into(),
        })
    }

    #[test]
    fn receipts_by_their_subject_with_the_strongest_total() {
        let r = text(
            "Your receipt from Corner Café",
            "Latte  Total: $4.50\nCroissant  Total: $3.25\nSubtotal: $7.75\nTax: $0.62\nOrder total: $8.37",
        )
        .unwrap();
        assert_eq!(r.total, usd(837));
        assert_eq!(r.kind, Kind::Purchase);
        let r = text(
            "Thanks for your order #A-12345",
            "Total (2 items):\n  $1,234.56",
        )
        .unwrap();
        assert_eq!(r.total, usd(123_456));
        assert_eq!(r.order.as_deref(), Some("A-12345"));
        let r = text("Ihre Rechnung", "Gesamtbetrag: 1.234,50 €").unwrap();
        assert_eq!(
            r.total,
            Some(Amount {
                cents: 123_450,
                currency: "EUR".into()
            })
        );
        let r = text("Payment received", "Amount paid: USD 12.5").unwrap();
        assert_eq!(r.total, usd(1250));
        let r = text("Your refund from Shop", "Refund total: -$5.00").unwrap();
        assert_eq!((r.kind, r.total), (Kind::Refund, usd(500)));
        // A table's labels, then its amounts in the same order.
        let r = text(
            "Invoice for your order #507112233445",
            "Production\nShipping\nSub-total\nSales tax (0%)\nTotal\nUSD 31.40\nUSD 6.50\nUSD 37.90\nUSD 0.00\nUSD 37.90",
        )
        .unwrap();
        assert_eq!(r.total, usd(3790));
        // With a heading over the labels.
        let r = text(
            "Your receipt",
            "Summary\nProduction\nShipping\nSub-total\nSales tax\nTotal\nUSD 31.40\nUSD 6.50\nUSD 37.90\nUSD 0.00\nUSD 37.90\nExample Prints Inc.",
        )
        .unwrap();
        assert_eq!(r.total, usd(3790));
        // Labels and amounts taking turns are read as they come.
        let r = text(
            "Thanks for your order.",
            "Subtotal\n$1,299.99\nShipping\nFree\nEstimated Sales Tax\n$104.00\nTotal\n$1,403.99",
        )
        .unwrap();
        assert_eq!(r.total, usd(140_399));
        let r = text(
            "Order #58201946 confirmed",
            "Subtotal\n$42.50\nShipping\n$6.00\nTaxes\n$3.83\nTotal\n$52.33 USD\nYou saved $9.00",
        )
        .unwrap();
        assert_eq!(r.total, usd(5233));
        // A few words between the label and its amount.
        let r = text(
            "Thanks for your delivery order, Sam",
            "Total\nIncludes all fees, taxes and discounts $86.12\nPayment method",
        )
        .unwrap();
        assert_eq!(r.total, usd(8612));
        // Nothing spent is no receipt; one that gives no total still is.
        assert!(text("Your Example Games Receipt", "Total: $0.00 USD").is_none());
        assert_eq!(
            text("Thanks for your order", "We got it.").unwrap().total,
            None
        );
    }

    #[test]
    fn not_receipts() {
        assert!(text("Weekly newsletter", "Total: $5.00").is_none());
        assert!(text("Your invoice is overdue", "Amount due: $50.00").is_none());
        assert!(text("Payment failed for your subscription", "").is_none());
        assert!(text("Read receipt: Lunch?", "").is_none());
        assert!(text("Your order has shipped", "Order total: $20.00").is_none());
        assert!(text("Your order was delivered", "").is_none());
        assert!(text("Your order is ready to ship", "").is_none());
        assert!(text("Your order 4410 from Mesa Print Works is about to ship", "").is_none());
        assert!(text("Your order #100234 has been prepared!", "").is_none());
        assert!(text("Put your tax refund toward something new.", "").is_none());
        assert!(text("Your refund might be bigger this year", "").is_none());
        assert!(text("Readers wanted refunds for the event", "").is_none());
        assert!(text("$10 off your next order", "").is_none());
        assert!(text("Before you forget, complete your order", "").is_none());
        assert!(text("Security check: verify your order", "").is_none());
        assert!(text("Alex, your payment history is impressive", "").is_none());
        assert!(text("Your payment of $120.00 is scheduled for Mar. 3", "").is_none());
        assert!(text("You paid $2,500?!", "").is_none());
        assert!(text("Re: Your receipt from Example Store.", "").is_none());
        assert!(text("An update on your refund request", "").is_none());
        assert!(text("Northwind ApS used MASTERCARD to pay your invoice", "").is_none());
        assert!(text("We sent your invoice (0007) for $410.00 USD", "").is_none());
        assert!(text("Final notice: Confirmаtion Receipt#Q", "").is_none());
        assert!(text("Your rebate receipt was received", "").is_none());
        // Paying a card's bill.
        assert!(text("Receipt for your payment to Example Bank", "").is_none());
        let card = find(&MailText {
            subject: "We've received your payment",
            text: "",
            html: None,
            from: "Capital One <alerts@bank.example>",
        });
        assert!(card.is_none());
        // Spam from a stranger's mailbox.
        let spam = find(&MailText {
            subject: "INVOICE#QK48ZT",
            text: "",
            html: None,
            from: "Pat Doe <pat.doe.1987@icloud.com>",
        });
        assert!(spam.is_none());
    }

    #[test]
    fn receipts_name_who_was_paid() {
        let payee = |s: &str| text(s, "").and_then(|r| r.merchant);
        assert_eq!(
            payee("Receipt for Your Payment to Example Games Inc.").as_deref(),
            Some("Example Games Inc")
        );
        assert_eq!(
            payee("Receipt: you paid Acme Inc").as_deref(),
            Some("Acme Inc")
        );
        assert_eq!(
            payee("Your refund from Example Store Co., Inc. is on the way").as_deref(),
            Some("Example Store Co., Inc")
        );
        assert_eq!(
            payee("Your receipt from X #1000-2000-3000").as_deref(),
            Some("X")
        );
        assert_eq!(
            payee("Your Example Order Receipt from Mar 3, 2025"),
            None
        );
        let r = text("Your refund from Example Commerce Inc. is on the way", "").unwrap();
        assert_eq!(r.kind, Kind::Refund);
    }

    #[test]
    fn receipts_by_their_markup() {
        let html = r#"<script type="application/ld+json">{"@context":"http://schema.org","@type":"Order","merchant":{"@type":"Organization","name":"Gadget Store"},"orderNumber":"GS-99812","priceCurrency":"EUR","price":"89.9","orderStatus":"http://schema.org/OrderProcessing"}</script>"#;
        let r = find(&MailText {
            subject: "We've got it",
            text: "",
            html: Some(html),
            from: "noreply@gadget.example",
        })
        .unwrap();
        assert_eq!(r.merchant.as_deref(), Some("Gadget Store"));
        assert_eq!(r.order.as_deref(), Some("GS-99812"));
        assert_eq!(
            r.total,
            Some(Amount {
                cents: 8990,
                currency: "EUR".into()
            })
        );
        assert_eq!(r.source, Source::Markup);
        // An order on its way is the Packages section's.
        let shipped = html.replace("OrderProcessing", "OrderInTransit");
        assert!(from_markup(&shipped).is_none());
        let invoice = r#"<script type="application/ld+json">{"@type":"Invoice","provider":{"name":"Power Co"},"paymentStatus":"PaymentComplete","totalPaymentDue":{"@type":"PriceSpecification","price":70,"priceCurrency":"USD"}}</script>"#;
        assert_eq!(from_markup(invoice).unwrap().total, usd(7000));
        assert!(from_markup(&invoice.replace("PaymentComplete", "PaymentDue")).is_none());
    }

    #[test]
    fn amounts_are_written_with_their_currency() {
        assert_eq!(usd(123_456).unwrap().show(), "$1,234.56");
        assert_eq!(
            Amount {
                cents: 120_000,
                currency: "JPY".into()
            }
            .show(),
            "¥1,200"
        );
        assert_eq!(
            Amount {
                cents: -1250,
                currency: "CHF".into()
            }
            .show(),
            "−CHF 12.50"
        );
        assert_eq!(parse_number("1 234,5"), Some(123_450));
        assert_eq!(parse_number("1200"), Some(120_000));
    }

    fn seen(id: &str, sender: &str, sent: i64) -> Sighting {
        Sighting {
            account: "a".into(),
            mail_id: id.into(),
            message_id: format!("<{id}@x>"),
            mailbox: crate::manage::model::Mailbox::Standard(crate::manage::model::Folder::Inbox),
            subject: String::new(),
            sender: sender.into(),
            sent,
        }
    }

    #[test]
    fn one_receipt_per_order_and_totals_by_currency() {
        let r = |order: Option<&str>, total: Option<Amount>, kind: Kind| Receipt {
            merchant: None,
            total,
            order: order.map(Into::into),
            kind,
            source: Source::Text,
        };
        let listed = summarize(vec![
            (
                r(Some("1"), None, Kind::Purchase),
                seen("c", "Shop <o@shop.example>", 10),
            ),
            (
                r(Some("1"), usd(2000), Kind::Purchase),
                seen("r", "Shop <o@shop.example>", 20),
            ),
            (
                r(None, usd(500), Kind::Refund),
                seen("f", "billing@mail.other.example", 30),
            ),
            (
                r(None, usd(100), Kind::Purchase),
                seen("f", "billing@mail.other.example", 30),
            ),
        ]);
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].merchant, "other.example");
        assert_eq!(listed[1].merchant, "Shop");
        assert_eq!(listed[1].date, 10);
        assert_eq!(listed[1].receipt.total, usd(2000));
        assert_eq!(listed[1].emails.len(), 2);
        assert_eq!(totals(&listed), [usd(1500).unwrap()]);
    }
}

#[cfg(test)]
mod store_tests {
    use crate::manage::model::{Folder, Mail};

    fn temp_store() -> (crate::manage::store::Store, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("receipts-test-{}", uuid::Uuid::new_v4()));
        (crate::manage::store::Store::open(&dir).unwrap(), dir)
    }

    /// Receipts known only by their headers are read by their subject, and
    /// listed to download (some mail was cached with an empty HTML and
    /// nothing else: that is no text either); downloaded, their totals are
    /// read.
    #[test]
    fn receipts_in_the_cache_and_those_to_download() {
        let (store, dir) = temp_store();
        let now = chrono::Utc::now().timestamp();
        let mail = |id: &str, subject: &str, body: Option<&str>, html: Option<&str>| Mail {
            id: id.into(),
            account: "a".into(),
            folder: Folder::Inbox,
            remote_folder: "INBOX".into(),
            from: "receipts@shop.example".into(),
            sender: "Shop".into(),
            subject: subject.into(),
            date: now,
            body: body.map(Into::into),
            html: html.map(Into::into),
            message_id: format!("<{id}@shop.example>"),
            ..Mail::default()
        };
        store
            .save_messages(&[
                mail(
                    "read",
                    "Your receipt from Shop",
                    Some("Order total: $12.00"),
                    None,
                ),
                mail("headers", "Your receipt from Shop #2", None, None),
                mail("empty", "Thanks for your order", None, Some("")),
                mail("other", "Weekly news", None, None),
            ])
            .unwrap();
        let scan = store.receipts(&["a".into()]).unwrap();
        assert_eq!(scan.scanned, 4);
        assert_eq!(scan.listed.len(), 3);
        let read = scan
            .listed
            .iter()
            .find(|l| l.emails[0].mail_id == "read")
            .unwrap();
        assert_eq!(
            read.receipt.total.as_ref().map(|t| t.show()).as_deref(),
            Some("$12.00")
        );
        let mut waiting: Vec<&str> = scan.to_download.iter().map(|(_, id)| id.as_str()).collect();
        waiting.sort();
        assert_eq!(waiting, ["empty", "headers"]);
        std::fs::remove_dir_all(dir).ok();
    }
}

/// Looks through a copy of a real mail cache for receipts and prints only
/// counts (no mail). It clears the copy's saved findings, so never point
/// it at the live data folder: `PICACHO_DIAG_DIR=<dir with mail.sqlite3>
/// cargo test receipts_in_a_real_cache -- --ignored --nocapture`.
#[cfg(test)]
#[test]
#[ignore]
fn receipts_in_a_real_cache() {
    let Some(dir) = std::env::var_os("PICACHO_DIAG_DIR") else {
        return;
    };
    let path = std::path::Path::new(&dir);
    let store = crate::manage::store::Store::open(path).unwrap();
    let accounts: Vec<String> = store
        .accounts()
        .unwrap()
        .into_iter()
        .map(|a| a.id)
        .collect();
    rusqlite::Connection::open(path.join("mail.sqlite3"))
        .unwrap()
        .execute("DELETE FROM receipt_scan", [])
        .ok();
    let started = std::time::Instant::now();
    let scan = store.receipts(&accounts).unwrap();
    let with_total = scan
        .listed
        .iter()
        .filter(|l| l.receipt.total.is_some())
        .count();
    let refunds = scan
        .listed
        .iter()
        .filter(|l| l.receipt.kind == Kind::Refund)
        .count();
    let markup = scan
        .listed
        .iter()
        .filter(|l| l.receipt.source == Source::Markup)
        .count();
    println!(
        "{} receipts ({with_total} with a total, {refunds} refunds, {markup} from markup) in {} emails, \
         {} to download, in {:?}",
        scan.listed.len(),
        scan.scanned,
        scan.to_download.len(),
        started.elapsed()
    );
    for a in store.accounts().unwrap() {
        let n = scan
            .to_download
            .iter()
            .filter(|(id, _)| *id == a.id)
            .count();
        if n > 0 {
            println!("  {:?} account {}: {n} to download", a.provider, &a.id[..8]);
        }
    }
}

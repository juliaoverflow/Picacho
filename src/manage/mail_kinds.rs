//! Kinds of mail the search can ask for (`has:otp`, `has:package-update`,
//! `has:receipt`): mail with a sign-in or verification code, mail telling
//! where a package is, and receipts.
//!
//! Which messages are which is worked out in the background, once per
//! change to the mail (`Kinds`): package updates and receipts are the
//! messages the Summary's looks through mail found a parcel or a receipt in
//! (`Store::parcels`, `Store::receipts`, which read their text), and code
//! mail is what the subject and preview suggest (`might_be_code`),
//! confirmed by the local sign-in code model when it is on, or else by a
//! stricter rule (`code_by_rule`). Until then, and for mail not looked at
//! yet, the subject, preview and sender alone decide (`Kinds::code`,
//! `Kinds::package`, `Kinds::receipt`).
use super::local_ai::otp;
use super::model::{Folder, Mail};
use std::collections::HashSet;

/// Which messages are code mail, package updates and receipts, by message
/// id.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Kinds {
    pub codes: HashSet<String>,
    pub packages: HashSet<String>,
    pub receipts: HashSet<String>,
    /// Every message looked at: one not among them is judged by its
    /// subject and preview.
    pub checked: HashSet<String>,
}

impl Kinds {
    /// Whether a message has a sign-in or verification code.
    pub fn code(kinds: Option<&Self>, m: &Mail) -> bool {
        match kinds.filter(|k| k.checked.contains(&m.id)) {
            Some(k) => k.codes.contains(&m.id),
            None => code_by_rule(&m.subject, &m.snippet),
        }
    }
    /// Whether a message tells where a package is.
    pub fn package(kinds: Option<&Self>, m: &Mail) -> bool {
        match kinds.filter(|k| k.checked.contains(&m.id)) {
            Some(k) => k.packages.contains(&m.id),
            None => package_by_rule(&m.subject, &m.snippet, &m.from),
        }
    }
    /// Whether a message is a receipt (or a refund's). Mail sent or
    /// drafted never is: an invoice sent is another's receipt.
    pub fn receipt(kinds: Option<&Self>, m: &Mail) -> bool {
        if matches!(m.folder, Folder::Sent | Folder::Drafts) {
            return false;
        }
        match kinds.filter(|k| k.checked.contains(&m.id)) {
            Some(k) => k.receipts.contains(&m.id),
            None => receipt_by_rule(&m.subject, &m.snippet, &m.sender, &m.from),
        }
    }
}

/// The concepts (`otp::concepts`) a message's subject and text name. The
/// lexicon takes "use" for a code used once ("for one-time use"), but
/// alone it says nothing: "Use code SAVE20".
fn concepts(subject: &str, text: &str) -> Vec<&'static str> {
    let head: String = text.chars().take(600).collect();
    let mut words = otp::words(&format!("{subject}\n{head}"), false);
    words.retain(|w| w != "use");
    otp::concepts(&words)
}

/// Whether a subject and preview name a code at all ("code", "código",
/// "one-time"…): the mail worth a closer look for one.
pub fn might_be_code(subject: &str, preview: &str) -> bool {
    let head: String = preview.chars().take(600).collect();
    let mut words = otp::words(&format!("{subject}\n{head}"), false);
    words.retain(|w| w != "use");
    // Only the two concepts asked about: every message is looked at.
    otp::names(&words, "code") || otp::names(&words, "once")
}

/// Codes that aren't for signing in: a place's, a shop's, a program's.
const OTHER_CODES: &[&str] = &[
    "zip code", "postal code", "post code", "postcode", "area code", "country code", "promo code",
    "promotional code", "discount code", "coupon code", "gift code", "offer code", "voucher code",
    "referral code", "source code", "dress code", "error code", "status code", "tracking code",
    "product code", "qr code", "bar code", "barcode",
];

/// Any of `OTHER_CODES`, however written.
static OTHER_CODE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    let words: Vec<String> = OTHER_CODES.iter().map(|w| regex::escape(w)).collect();
    regex::Regex::new(&format!("(?i){}", words.join("|"))).expect("pattern")
});

/// How much of a message's text is looked through for its code: a code
/// is given at the top, not in a newsletter's tail.
const CODE_HEAD: usize = 3000;

/// Whether a sentence (or line) of a message's subject or the top of its
/// text names a code and holds something shaped like one ("Your code is
/// 482913."), leaving out codes of other kinds ("zip code 85140").
pub fn code_in_a_sentence(subject: &str, text: &str) -> bool {
    let head: String = text.chars().take(CODE_HEAD).collect();
    // Blanked, so the code shaped text after them is left without its word.
    let all = OTHER_CODE
        .replace_all(&format!("{subject}\n{head}"), " ")
        .into_owned();
    all.split(['\n', '.', '!', '?', '|', '\u{3002}'])
        .filter(|s| s.chars().count() <= 300)
        .any(|sentence| {
            let found = concepts(sentence, "");
            (found.contains(&"code") || found.contains(&"once")) && otp::has_candidate(sentence, "")
        })
}

/// Whether a message has a sign-in or verification code, without the
/// model: a sentence gives a code (`code_in_a_sentence`), and it is about
/// signing in, verifying, or a code used once or expiring ("expires in
/// 10 minutes"). A promotion's code ("Use code SAVE20") isn't one.
pub fn code_by_rule(subject: &str, text: &str) -> bool {
    let head: String = text.chars().take(CODE_HEAD).collect();
    let found = concepts(subject, &head);
    // A promotion's offer expires too: it needs signing in or verifying.
    let purposes: &[&str] = if found.contains(&"promo") {
        &["verify", "signin"]
    } else {
        &["verify", "signin", "once", "expire"]
    };
    purposes.iter().any(|c| found.contains(c)) && code_in_a_sentence(subject, text)
}

/// Whether a message tells where a package is, by its subject, preview and
/// sender: what the Summary finds in mail not downloaded.
pub fn package_by_rule(subject: &str, preview: &str, from: &str) -> bool {
    super::parcels::looks_like_shipping(subject, preview, from)
        && super::parcels::status_in(subject, preview).is_some()
}

/// Whether a message is a receipt by its subject, preview and sender: what
/// the Summary finds in mail not downloaded.
pub fn receipt_by_rule(subject: &str, preview: &str, sender: &str, from: &str) -> bool {
    super::receipts::find(&super::receipts::MailText {
        subject,
        text: preview,
        html: None,
        from: &format!("{sender} <{from}>"),
    })
    .is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_by_rule() {
        assert!(code_by_rule("Your verification code", "Your code is 482913. It expires in 10 minutes."));
        assert!(code_by_rule("Sign in to Example", "Your sign-in code is 731 204."));
        assert!(code_by_rule("Tu código de verificación", "Código: 552190"));
        // A promotion's code, and a code with nothing shaped like one.
        assert!(!code_by_rule("20% off this weekend", "Use code SAVE20 at checkout"));
        assert!(!code_by_rule("Your discount code", "Code SAVE2024 - offer expires Sunday"));
        // A code word and a number, but not one code: a job alert.
        assert!(!code_by_rule("12 new jobs near you", "Sign in to see jobs in zip code 85140.\nApply by Nov 30"));
        assert!(!code_by_rule("Security alert", "Someone signed in to your account.\nIf this was you, there's nothing to do. Reference 4821 9930."));
        assert!(!code_by_rule("Verify your email", "Click the link to verify your address."));
        assert!(might_be_code("Your one-time passcode", ""));
        assert!(!might_be_code("Weekly report", "Numbers for the week"));
    }

    #[test]
    fn package_updates_by_rule() {
        assert!(package_by_rule("Your package has been delivered", "", "shop@example.com"));
        assert!(package_by_rule("Shipped: \"Kettle\"", "Your package is on the way", "ship-confirm@amazon.com"));
        assert!(!package_by_rule("Free shipping this weekend", "Shop now", "news@shop.example"));
    }

    #[test]
    fn receipts_by_rule() {
        assert!(receipt_by_rule("Your receipt from Example Store.", "", "Example Store", "receipts@store.example"));
        assert!(receipt_by_rule("Order #58201946 confirmed", "", "Shop", "orders@shop.example"));
        assert!(!receipt_by_rule("Your order is ready to ship", "", "Shop", "orders@shop.example"));
        assert!(!receipt_by_rule(
            "We've received your payment",
            "",
            "Capital One",
            "alerts@bank.example"
        ));
    }

    #[test]
    fn what_was_looked_at_decides() {
        let m = Mail {
            id: "1".into(),
            subject: "Your verification code".into(),
            snippet: "Your code is 482913.".into(),
            ..Mail::default()
        };
        // Not looked at yet: by its subject and preview.
        assert!(Kinds::code(None, &m));
        let mut kinds = Kinds::default();
        assert!(Kinds::code(Some(&kinds), &m));
        // Looked at, and the model said no.
        kinds.checked.insert("1".into());
        assert!(!Kinds::code(Some(&kinds), &m));
        kinds.codes.insert("1".into());
        assert!(Kinds::code(Some(&kinds), &m));
    }
}

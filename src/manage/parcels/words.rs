//! What a message's words say about a delivery: where it is ("Shipped",
//! "Out for delivery", "Zugestellt"…), the order it belongs to, and whether
//! it is shipping mail at all (to download it for the Summary).
use super::Status;
use super::text::{PARCEL_WORDS, parcel_concepts};
use regex::Regex;
use std::sync::LazyLock;

/// Phrases naming each status, lowercase, in the order a subject or text is
/// read for them: an ending (delivered, returned, cancelled) before the
/// stages before it, so "Delivered: your package shipped Monday" is
/// delivered.
const PHRASES: &[(Status, &[&str])] = &[
    (
        Status::Cancelled,
        &[
            "order cancelled",
            "order canceled",
            "has been cancelled",
            "has been canceled",
            "was cancelled",
            "was canceled",
            "cancellation confirm",
            "storniert",
            "cancelado",
            "annulée",
            "annulé",
            "annullato",
            "geannuleerd",
            "anulowan",
            "キャンセル",
            "已取消",
            "주문취소",
            "отменён",
            "отменен",
            "iptal edildi",
        ],
    ),
    (
        Status::Returned,
        &[
            "returned to sender",
            "returns label",
            "return label",
            "return to sender",
            "return received",
            "we received your return",
            "your return",
            "rücksendung",
            "retoure",
            "devolución",
            "devuelto",
            "retourné",
            "reso ricevuto",
            "devolvido",
            "retour ontvangen",
            "zwrot",
            "returnerad",
            "返品",
            "退货",
            "반품",
            "возврат",
            "iade",
        ],
    ),
    (
        Status::Delivered,
        &[
            "delivered",
            "has arrived",
            "have arrived",
            "package arrived",
            "parcel arrived",
            "order arrived",
            "shipment arrived",
            "delivery complete",
            "delivery confirmation",
            "was left at",
            "zugestellt",
            "geliefert",
            "entregado",
            "entregada",
            "livré",
            "livrée",
            "consegnato",
            "consegnata",
            "entregue",
            "bezorgd",
            "afgeleverd",
            "dostarczon",
            "doręczon",
            "levererad",
            "levereret",
            "leveret",
            "toimitettu",
            "配達完了",
            "お届け完了",
            "配達しました",
            "お届けしました",
            "已签收",
            "已送达",
            "妥投",
            "배송완료",
            "배송 완료",
            "доставлен",
            "вручен",
            "доставлено",
            "teslim edildi",
        ],
    ),
    (
        Status::PickupAvailable,
        &[
            "ready for pickup",
            "ready for pick up",
            "ready for pick-up",
            "ready to pick up",
            "ready to pick-up",
            "ready to be picked up",
            "ready for in-store pickup",
            "ready for store pickup",
            "ready for curbside pickup",
            "available to pick up",
            "available for pick up",
            "available for pick-up",
            "awaiting pickup",
            "waiting for pickup",
            "waiting to be picked up",
            "pickup is ready",
            "ready for collection",
            "available for pickup",
            "awaiting collection",
            "ready to collect",
            "abholbereit",
            "zur abholung",
            "listo para recoger",
            "disponible en point",
            "disponible au retrait",
            "pronto per il ritiro",
            "disponível para retirada",
            "ligt klaar",
            "do odbioru",
            "kan hämtas",
            "受取可能",
            "待取件",
            "ожидает получения",
            "teslim almaya hazır",
        ],
    ),
    (
        Status::OutForDelivery,
        &[
            "out for delivery",
            "arriving today",
            "arrives today",
            "delivered today",
            "heute zugestellt",
            "in zustellung",
            "zustellung heute",
            "en reparto",
            "en cours de livraison",
            "livré aujourd'hui",
            "in consegna",
            "saiu para entrega",
            "vandaag bezorgd",
            "w doręczeniu",
            "ute för leverans",
            "配達中",
            "本日お届け",
            "派送中",
            "正在派送",
            "배송출발",
            "배송 출발",
            "передан курьеру",
            "dağıtıma çıktı",
        ],
    ),
    (
        Status::Problem,
        &[
            "delivery exception",
            "delayed",
            "is delayed",
            "running late",
            "failed delivery",
            "delivery attempt",
            "attempted delivery",
            "couldn't deliver",
            "could not deliver",
            "unable to deliver",
            "missed delivery",
            "verspätet",
            "verzögert",
            "zustellversuch",
            "nicht zugestellt",
            "retrasado",
            "retraso",
            "en retard",
            "in ritardo",
            "atrasad",
            "vertraagd",
            "opóźni",
            "försenad",
            "遅延",
            "延误",
            "지연",
            "задерж",
            "gecik",
        ],
    ),
    (
        Status::InTransit,
        &[
            "in transit",
            "arrived at",
            "on its way",
            "on the way",
            "is coming",
            "en route",
            "on the move",
            "unterwegs",
            "en camino",
            "en tránsito",
            "en cours d'acheminement",
            "en transit",
            "in transito",
            "em trânsito",
            "a caminho",
            "onderweg",
            "w drodze",
            "på väg",
            "輸送中",
            "运输中",
            "배송중",
            "배송 중",
            "в пути",
            "yolda",
        ],
    ),
    (
        Status::Shipped,
        &[
            "shipped",
            "dispatched",
            "shipping confirmation",
            "shipment confirmation",
            "label created",
            "shipping label",
            "picked up",
            "handed over to",
            "ship notification",
            "shipment notification",
            "shipping notification",
            "versandt",
            "verschickt",
            "versendet",
            "versandbestätigung",
            "enviado",
            "enviada",
            "confirmación de envío",
            "expédié",
            "expédiée",
            "spedito",
            "spedita",
            "postado",
            "postada",
            "verzonden",
            "wysłan",
            "nadan",
            "skickat",
            "skickad",
            "afsendt",
            "lähetetty",
            "発送",
            "出荷",
            "已发货",
            "已发出",
            "발송",
            "отправлен",
            "kargoya verildi",
            "kargoya teslim",
        ],
    ),
    (
        Status::Processing,
        &[
            "ordered",
            "about to ship",
            "ready to ship",
            "preparing to ship",
            "preparing your shipment",
            "order placed",
            "order has been placed",
            "order is confirmed",
            "order is being processed",
            "processing your order",
            "preparing your order",
            "order confirmation",
            "order confirmed",
            "order received",
            "thanks for your order",
            "thank you for your order",
            "we received your order",
            "we've received your order",
            "bestellbestätigung",
            "bestellung eingegangen",
            "pedido confirmado",
            "confirmación de pedido",
            "commande confirmée",
            "confirmation de commande",
            "conferma d'ordine",
            "ordine confermato",
            "confirmação de pedido",
            "orderbevestiging",
            "potwierdzenie zamówienia",
            "orderbekräftelse",
            "ご注文の確認",
            "注文確認",
            "订单确认",
            "주문 확인",
            "подтверждение заказа",
            "sipariş onayı",
            "siparişiniz alındı",
        ],
    ),
    // When it should arrive: carriers' updates say so, but so does an
    // order confirmation ("Thanks for your order. Arriving Friday"), so
    // these count only when nothing above was said.
    (
        Status::InTransit,
        &[
            "expected delivery",
            "scheduled delivery",
            "scheduled for delivery",
            "estimated delivery",
            "new delivery date",
            "arriving",
            "will arrive",
            "zustellung voraussichtlich",
            "entrega prevista",
            "livraison prévue",
        ],
    ),
];

/// Words before a phrase that make it a promise rather than news: "we'll
/// email you when it has shipped", "expected to be delivered Friday".
const NOT_YET: &[&str] = &[
    "will ",
    "'ll ",
    "to be ",
    "once ",
    "when ",
    "as soon as",
    "after ",
    "until ",
    "expected",
    "estimated",
    "scheduled",
    "should ",
    "if ",
    "before ",
    "not been",
    "not yet",
    "sera ",
    "wird ",
    "será ",
    "sarà ",
    "wordt ",
    "будет",
    "予定",
    "将",
    "sobald",
    "wenn ",
    "cuando",
    "lorsque",
    "quand ",
    "quando",
    "zodra",
    "gdy ",
    "när ",
];
/// How far before a phrase those words are looked for, in characters.
const NOT_YET_WINDOW: usize = 32;
/// How much of a message's text is read for its status after its subject:
/// where shipping mail says what happened, before order lines and footers.
const STATUS_TEXT: usize = 1000;

/// Where a delivery is, as a subject or text says, or None.
fn status_of(text: &str) -> Option<Status> {
    status_among(text, PHRASES, true)
}

/// The first of `table`'s statuses `text` names; promises ("will ship")
/// count only without `news_only`.
fn status_among(text: &str, table: &[(Status, &[&str])], news_only: bool) -> Option<Status> {
    let lower = text.to_lowercase();
    for (status, phrases) in table {
        for phrase in *phrases {
            for (at, _) in lower.match_indices(phrase) {
                // Phrases written in Latin letters are whole words.
                let before = lower[..at].chars().next_back();
                let after = lower[at + phrase.len()..].chars().next();
                let latin = phrase
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_alphabetic());
                if latin
                    && (before.is_some_and(char::is_alphanumeric)
                        || (!phrase.ends_with(' ') && after.is_some_and(|c| c.is_alphabetic())))
                {
                    continue;
                }
                // "Has arrived at the sorting facility" is a carrier's scan
                // on the way, not the delivery.
                if phrase.contains("arrived") {
                    let rest = lower[at + phrase.len()..].trim_start();
                    if ["at ", "in ", "to "].iter().any(|w| rest.starts_with(w)) {
                        continue;
                    }
                }
                if news_only
                    && !matches!(
                    status,
                    Status::Processing | Status::Problem | Status::OutForDelivery
                ) {
                    let start = lower[..at]
                        .char_indices()
                        .rev()
                        .nth(NOT_YET_WINDOW - 1)
                        .map_or(0, |(i, _)| i);
                    // Only the phrase's own sentence and line: "If you have
                    // questions, reply.\nDelivered" is news.
                    let window = &lower[start..at];
                    let window = window
                        .rfind(['\n', '.', '!', '?', '|', '•'])
                        .map_or(window, |i| &window[i + 1..]);
                    if NOT_YET.iter().any(|w| window.contains(w)) {
                        continue;
                    }
                }
                return Some(*status);
            }
        }
    }
    None
}

/// Where the delivery a message tells of is: as its subject says, or else
/// its first lines.
pub fn status_in(subject: &str, text: &str) -> Option<Status> {
    status_of(subject).or_else(|| {
        let head: String = text.chars().take(STATUS_TEXT).collect();
        status_of(&TRACKER.replace_all(&head, " "))
    })
}

/// A progress bar's stage names in a row ("Ordered Shipped Out for
/// delivery Delivered"), as shops put at the top of every shipping email:
/// they name every stage, not the one the parcel is at.
static TRACKER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?is)\bordered\W{1,12}shipped\b.{0,60}?\bdelivered\b").expect("pattern")
});

/// Whether a subject is about an order or a parcel ("An update on your
/// order", "About your package"), so that what its text says of a delivery
/// is taken even without an order confirmation's or a tracking number.
pub(super) fn shipping_subject(subject: &str) -> bool {
    // Not "shipping" or "delivery" alone: "Free shipping this weekend".
    parcel_concepts(subject)
        .iter()
        .any(|c| matches!(*c, "order" | "parcel" | "track"))
}

/// Whether a subject says a delivery moved (shipped, out for delivery,
/// delivered…): the mail a delivery known only by its order is taken from.
pub(super) fn delivery_subject(subject: &str) -> bool {
    status_of(subject).is_some_and(|s| s != Status::Processing && s != Status::Cancelled)
}

/// Whether a message says when its order should arrive ("Arriving
/// Friday", "Estimated delivery: …"): an order confirmation that does is
/// for something shipped, not a meal or a subscription.
pub(super) fn mentions_arrival(subject: &str, text: &str) -> bool {
    let arrival = &PHRASES[PHRASES.len() - 1..];
    let head: String = text.chars().take(4000).collect();
    status_among(subject, arrival, false).is_some() || status_among(&head, arrival, false).is_some()
}

/// Whether a subject says an order was placed ("Ordered: …", "Order
/// confirmation").
pub(super) fn order_subject(subject: &str) -> bool {
    status_of(subject) == Some(Status::Processing)
}

/// Amazon's order numbers ("113-1234567-1234567"), and order numbers after
/// a word for an order in many languages.
static ORDER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(\d{3}-\d{7}-\d{7})\b|(?:\border|bestellung|bestellnummer|auftrag|pedido|commande|ordine|encomenda|bestelling|bestelnummer|zamówieni[ae]|beställning|ordre|tilaus|注文番号|订单号|訂單編號|주문번호|заказ|sipariş)\s*(?:number|no\.?|nr\.?|n\.?[º°o]\.?|id|num(?:éro|ero|ber)?|numer|numarası)?\s*[:#：]?\s*#?\s*([A-Z0-9][A-Z0-9\-]{3,30})",
    )
    .expect("pattern")
});

/// Orders a message names at most.
const MAX_ORDERS: usize = 4;

/// The orders a message tells of, its subject's first, each once: one
/// shipment may carry several orders' items ("Delivered 2 items" of two
/// Amazon orders).
pub(crate) fn order_numbers(subject: &str, text: &str) -> Vec<String> {
    let head: String = text.chars().take(4000).collect();
    let mut out: Vec<String> = Vec::new();
    for s in [subject, head.as_str()] {
        let mut from = 0;
        while let Some(c) = ORDER.captures_at(s, from) {
            let Some(m) = c.get(1).or_else(|| c.get(2)) else {
                break;
            };
            let n = m.as_str().trim_end_matches('-');
            if n.bytes().filter(u8::is_ascii_digit).count() < 3 {
                // Not a number ("Review your Order\nOrder number: 5259…"
                // took the second "Order" as it): looked for again from the
                // word it took, which may itself begin the order's number.
                from = m.start();
                continue;
            }
            from = c.get(0).map_or(m.end(), |all| all.end());
            let n = n.to_ascii_uppercase();
            if !out.contains(&n) {
                out.push(n);
            }
            if out.len() == MAX_ORDERS {
                return out;
            }
        }
    }
    out
}

/// Whether a message speaks of a parcel, shipping or delivery at all, in
/// its subject or first lines.
pub(super) fn about_parcels(subject: &str, text: &str) -> bool {
    let head: String = text.chars().take(4000).collect();
    [subject, head.as_str()].iter().any(|s| {
        parcel_concepts(s)
            .iter()
            .any(|c| PARCEL_WORDS.contains(c))
    })
}

/// Whether a subject says a delivery moved: such mail is downloaded for
/// the Summary however old it is (see `Store::parcels`).
pub fn delivery_news(subject: &str) -> bool {
    delivery_subject(subject)
}

/// A subject's stage before what is in the shipment: "Shipped:",
/// "Delivered 2 items:", "Out for delivery:", "Arriving tomorrow:".
static STAGE_PREFIX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^\s*(?:ordered|shipped|delivered|out for delivery|arriving(?: today| tomorrow| \w+day)?|now arriving(?: \w+)?|in transit|delayed)\s*:?\s*(.{3,})$",
    )
    .expect("pattern")
});

/// What a subject says is in the shipment after its stage, normalized:
/// "Shipped: 1 Luggage item" → "1 luggage item", "Delivered 2 items:
/// Video Games, Light Bulbs" → "2 items: video games, light bulbs".
pub(super) fn contents(subject: &str) -> Option<String> {
    let rest = STAGE_PREFIX.captures(subject)?.get(1)?.as_str();
    let rest = normalize_name(rest);
    (rest.chars().count() >= 3).then_some(rest)
}

/// A product or shipment's name as it is compared: lowercase, without
/// quotes, direction marks, a trailing "..." or "and 1 more item".
fn normalize_name(name: &str) -> String {
    let name: String = name
        .chars()
        .filter(|c| !matches!(c, '\u{2066}'..='\u{2069}' | '\u{200e}' | '\u{200f}' | '"' | '“' | '”' | '«' | '»'))
        .collect();
    let lower = name.to_lowercase();
    let lower = match lower.find(" and ") {
        Some(i) if lower[i..].contains(" more item") || lower[i..].contains(" other item") => {
            lower[..i].to_owned()
        }
        _ => lower,
    };
    lower
        .trim_end_matches(['.', '…', ' '])
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Where a message's own items end and suggestions begin.
const SUGGESTIONS: &[&str] = &[
    "you may also like",
    "you might also like",
    "you might like",
    "recommended for you",
    "customers also",
    "related items",
    "more to explore",
    "inspired by",
    "deals for you",
    "view more",
];
/// Products read per message at most.
const MAX_PRODUCTS: usize = 4;

/// The products a message lists by shortened name ("Hard Shell Travel Case
/// for Mous...", or a subject's "Shipped: \"Acme 8 oz Glass Spray...\""),
/// before any suggestions, normalized.
pub(super) fn products(subject: &str, text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let head: String = text.chars().take(6000).collect();
    let lower = head.to_lowercase();
    let end = SUGGESTIONS
        .iter()
        .filter_map(|s| lower.find(s))
        .min()
        .unwrap_or(head.len());
    let own = head.get(..end).unwrap_or(&head);
    let subject_item = STAGE_PREFIX
        .captures(subject)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_owned());
    for line in subject_item.iter().map(String::as_str).chain(own.lines()) {
        let line = line.trim();
        let shortened = line.ends_with("...") || line.ends_with('…');
        // Shortened at a quote: Amazon's subject quotes the first item.
        let quoted = line.contains("...\"") || line.contains("...”") || line.contains("…\"");
        if !(shortened || quoted) {
            continue;
        }
        let name = normalize_name(line.split(['"', '“']).find(|p| p.contains("...") || p.contains('…')).unwrap_or(line));
        let letters = name.chars().filter(|c| c.is_alphabetic()).count();
        if letters < 8 || name.chars().count() > 120 || out.contains(&name) {
            continue;
        }
        out.push(name);
        if out.len() == MAX_PRODUCTS {
            break;
        }
    }
    out
}

/// Carriers and shops' shipping senders, as their address's domain ends.
pub(super) const SHIPPING_SENDERS: &[&str] = &[
    "ups.com",
    "fedex.com",
    "usps.com",
    "usps.gov",
    "dhl.com",
    "dhl.de",
    "dhl.co.uk",
    "dpd.com",
    "dpd.co.uk",
    "dpd.de",
    "gls-group.eu",
    "gls-group.com",
    "hermesworld.com",
    "myhermes.de",
    "evri.com",
    "royalmail.com",
    "parcelforce.com",
    "canadapost.ca",
    "postescanada-canadapost.ca",
    "purolator.com",
    "ontrac.com",
    "lasership.com",
    "laposte.fr",
    "colissimo.fr",
    "chronopost.fr",
    "correos.es",
    "seur.com",
    "poste.it",
    "brt.it",
    "ctt.pt",
    "correios.com.br",
    "postnl.nl",
    "bpost.be",
    "inpost.pl",
    "postnord.com",
    "posti.fi",
    "auspost.com.au",
    "nzpost.co.nz",
    "japanpost.jp",
    "kuronekoyamato.co.jp",
    "sagawa-exp.co.jp",
    "sf-express.com",
    "cainiao.com",
    "17track.net",
    "aftership.com",
    "narvar.com",
    "route.com",
    "shipstation.com",
    "shopify.com",
    "shipment-tracking@amazon",
    "order-update@amazon",
    "shipping@",
    "tracking@",
    "delivery@",
    "shipment@",
    "versand@",
];

/// Whether a downloaded message is worth reading whole for parcels, by its
/// subject, preview and sender: shipping mail (`looks_like_shipping`), or
/// any naming an order, parcel, shipping or delivery. Reading every
/// message's source takes minutes in a large mailbox.
pub fn worth_reading(subject: &str, preview: &str, from: &str) -> bool {
    looks_like_shipping(subject, preview, from)
        || about_parcels(subject, preview)
        || parcel_concepts(subject).contains(&"order")
}

/// Whether a message looks like shipping mail by what is known before it
/// is downloaded: a subject saying a delivery moved, naming tracking or a
/// parcel (or shipping or delivery of an order), a carrier's or a shipping
/// sender's address, or a preview giving a tracking number. A subject
/// naming delivery alone ("Free delivery this weekend") isn't enough.
pub fn looks_like_shipping(subject: &str, preview: &str, from: &str) -> bool {
    if delivery_subject(subject) {
        return true;
    }
    let concepts = parcel_concepts(subject);
    if concepts.iter().any(|c| matches!(*c, "track" | "parcel"))
        || (concepts.contains(&"order") && concepts.iter().any(|c| PARCEL_WORDS.contains(c)))
    {
        return true;
    }
    let from = from.to_ascii_lowercase();
    if SHIPPING_SENDERS.iter().any(|s| from.contains(s)) {
        return true;
    }
    !super::from_text(subject, preview, None).is_empty()
}

//! Tracking numbers in the addresses of a message's links: a "Track
//! package" button usually points at the carrier's (or the shop's) tracking
//! page with the number in it, even when the text doesn't give it.
use super::{Parcel, Source, known_url, web_url};
use regex::Regex;
use std::sync::LazyLock;

/// A link's address and its content.
static ANCHOR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?is)<a\b[^>]*?\bhref\s*=\s*(?:"([^"]*)"|'([^']*)')[^>]*>(.*?)</a\s*>"#)
        .expect("pattern")
});
/// Runs of letters and digits in an address.
static TOKEN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[A-Za-z0-9]{10,40}").expect("pattern"));
/// Links looked at per message.
const MAX_LINKS: usize = 300;
/// Words in a link's address or text saying it is about tracking a parcel.
const TRACKING_WORDS: &[&str] = &[
    "track",
    "trace",
    "trk",
    "sendung",
    "suivi",
    "seguimiento",
    "rastre",
    "tracciamento",
    "spedizion",
    "parcel",
    "package",
    "shipment",
    "awb",
    "waybill",
    "inquiry",
    "colis",
    "paket",
    "przesyl",
    "zending",
    "追跡",
    "物流",
    "배송",
    "отслеж",
];
/// Carriers' sites: a number with a carrier's check digit in a link to one
/// of them is its tracking number whatever the link's words.
const CARRIER_SITES: &[&str] = &[
    "ups.com",
    "fedex.com",
    "usps.com",
    "dhl.",
    "dpd.",
    "gls-",
    "hermes",
    "evri.com",
    "royalmail.com",
    "parcelforce.com",
    "canadapost",
    "purolator.com",
    "ontrac.com",
    "lasership.com",
    "laposte.fr",
    "colissimo",
    "chronopost",
    "correos",
    "seur.com",
    "poste.it",
    "brt.it",
    "ctt.pt",
    "correios",
    "postnl",
    "bpost",
    "inpost",
    "postnord",
    "posti.fi",
    "auspost",
    "nzpost",
    "japanpost",
    "kuronekoyamato",
    "sagawa",
    "sf-express",
    "17track",
    "aftership",
    "narvar",
    "parcelsapp",
    "parcelsapp.com",
    "route.com",
];

/// A link's address, with its HTML character references read.
fn address(raw: &str) -> String {
    raw.trim()
        .replace("&amp;", "&")
        .replace("&#38;", "&")
        .replace("&#x26;", "&")
        .replace("&#61;", "=")
}

/// The address a click-tracking wrapper leads to, where url-sanitize knows
/// the wrapper; the address as it is otherwise.
fn unwrapped(link: &str) -> String {
    crate::manage::tracking::clean_url(link).unwrap_or_else(|| link.to_owned())
}

/// The links of HTML that may be about tracking: (address, text), web
/// links only. A link elsewhere than a carrier's site is unwrapped
/// (`unwrapped`) first; one on a carrier's site isn't, as `places` reads
/// all of it, and unwrapping takes the sanitizer's whole rule set (a
/// newsletter, or a USPS Informed Delivery digest, has a hundred links:
/// seconds of work each message, in a scan of every message).
fn anchors(html: &str) -> Vec<(String, String)> {
    ANCHOR
        .captures_iter(html)
        .take(MAX_LINKS)
        .filter_map(|c| {
            let href = address(c.get(1).or_else(|| c.get(2))?.as_str());
            let text = crate::sync::mime::html_to_text(c.get(3).map_or("", |m| m.as_str()));
            let text = text.trim().to_owned();
            if !may_be_tracking(&href, &text) {
                return None;
            }
            let href = if on_carrier_site(&href) { href } else { unwrapped(&href) };
            web_url(&href)?;
            Some((href, text))
        })
        .collect()
}

/// Whether a link is to a carrier's site.
fn on_carrier_site(href: &str) -> bool {
    url::Url::parse(href).is_ok_and(|url| {
        let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
        CARRIER_SITES.iter().any(|s| host.contains(s))
    })
}

/// A tracking link's address as it is kept: without its tracking.
fn kept(href: &str) -> String {
    crate::manage::tracking::clean_url(href).unwrap_or_else(|| href.to_owned())
}

/// Whether a link's address (perhaps wrapped for click-tracking) or text
/// has any word about tracking, or a carrier's site, in it: as
/// `about_tracking` reads, the words in the address past its host (a
/// click-tracking host, "trk.email.example.com", says nothing). A wrapper
/// the sanitizer can unwrap holds the address it leads to, so its words
/// too.
fn may_be_tracking(href: &str, text: &str) -> bool {
    let href = href.to_lowercase();
    let after_scheme = href.split_once("://").map_or(href.as_str(), |(_, rest)| rest);
    let (host, rest) = after_scheme
        .find(['/', '?', '#'])
        .map_or((after_scheme, ""), |i| after_scheme.split_at(i));
    let text = text.to_lowercase();
    TRACKING_WORDS
        .iter()
        .any(|w| rest.contains(w) || text.contains(w))
        || CARRIER_SITES.iter().any(|s| host.contains(s) || rest.contains(s))
}

/// Whether a link is about tracking: by its address's words or site, or
/// its text ("Track package", "Sendung verfolgen").
fn about_tracking(href: &str, text: &str) -> bool {
    let Ok(url) = url::Url::parse(href) else {
        return false;
    };
    let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
    // A click-tracking host ("click.track.example.com") says nothing.
    let rest = format!("{}?{}", url.path(), url.query().unwrap_or_default()).to_lowercase();
    let text = text.to_lowercase();
    TRACKING_WORDS
        .iter()
        .any(|w| rest.contains(w) || text.contains(w))
        || CARRIER_SITES.iter().any(|s| host.contains(s))
}

/// Words in a query parameter's name, or the path segment before a value,
/// saying the value is a tracking number.
const NUMBER_KEYS: &[&str] = &[
    "track",
    "trk",
    "num",
    "awb",
    "waybill",
    "parcel",
    "shipment",
    "sendung",
    "colis",
    "consignment",
    "package",
    "barcode",
    "inquiry",
    "label",
    "trace",
    "piece",
    "match",
];

/// The places in an address a tracking number may be: on a carrier's site,
/// its whole path and query; elsewhere, the values of query parameters
/// named for one (`?tracking_number=`, `?trackNums=`), and the path segment
/// after one about tracking (`/tracking/1Z…`). An address inside a query
/// value (a redirect's `?url=`) is read the same way.
fn places(href: &str, depth: usize) -> Vec<String> {
    let Ok(url) = url::Url::parse(href) else {
        return Vec::new();
    };
    let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
    let carrier = CARRIER_SITES.iter().any(|s| host.contains(s));
    let keyed = |key: &str| {
        let key = key.to_ascii_lowercase();
        NUMBER_KEYS.iter().any(|k| key.contains(k))
    };
    let mut out = Vec::new();
    let segments: Vec<&str> = url.path().split('/').collect();
    for (i, segment) in segments.iter().enumerate() {
        if carrier || (i > 0 && keyed(segments[i - 1])) {
            out.push((*segment).to_owned());
        }
    }
    for (key, value) in url.query_pairs() {
        if carrier || keyed(&key) {
            out.extend(value.split([',', ' ', ';', '|']).map(str::to_owned));
        }
        if depth == 0 && value.starts_with("http") {
            out.extend(places(&value, 1));
        }
    }
    out
}

/// The tracking numbers in the addresses of tracking links. Only runs of
/// uppercase letters and digits are read: the mixed-case runs of
/// click-tracking addresses are identifiers, not tracking numbers.
pub(super) fn from_links(html: &str) -> Vec<Parcel> {
    let mut out: Vec<Parcel> = Vec::new();
    for (href, text) in anchors(html) {
        if !about_tracking(&href, &text) {
            continue;
        }
        for place in places(&href, 0) {
            for token in TOKEN.find_iter(&place) {
                let number = token.as_str();
                if number.bytes().any(|b| b.is_ascii_lowercase())
                    || !number.bytes().any(|b| b.is_ascii_digit())
                    || out.iter().any(|p| p.tracking_number == number)
                {
                    continue;
                }
                // The whole value, not a part of a longer run.
                if token.start() > 0 || token.end() < place.len() {
                    let whole = place
                        .trim_matches(|c: char| !c.is_ascii_alphanumeric())
                        .len();
                    if whole != number.len() {
                        continue;
                    }
                }
                let Some(known) = tracking_numbers::track(number)
                    .filter(|k| !super::text::product_code(number, k))
                else {
                    continue;
                };
                out.push(Parcel {
                    tracking_url: Some(kept(&href)).or_else(|| known_url(&known)),
                    tracking_number: number.to_owned(),
                    order: None,
                    carrier: Some(known.courier),
                    service: Some(known.service),
                    expected_arrival: None,
                    status: None,
                    updated: None,
                    orders: Vec::new(),
                    contents: None,
                    products: Vec::new(),
                    source: Source::Link,
                });
            }
        }
    }
    out
}

/// The page a message's "Track package" link opens, for a delivery known
/// only by its order.
pub(super) fn tracking_page(html: &str) -> Option<String> {
    anchors(html)
        .into_iter()
        .find(|(href, text)| {
            let text = text.to_lowercase();
            TRACKING_WORDS.iter().any(|w| text.contains(w))
                || (text.is_empty() && about_tracking(href, text.as_str()))
        })
        .map(|(href, _)| kept(&href))
}

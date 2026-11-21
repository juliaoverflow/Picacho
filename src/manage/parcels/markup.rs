//! schema.org `ParcelDelivery` markup (JSON-LD) in mail.
use super::{Parcel, Source, Status, dates, known_url, web_url};
use regex::Regex;
use serde_json::Value;
use std::sync::LazyLock;

/// The JSON-LD blocks in HTML.
static JSON_LD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?is)<script\b[^>]*\btype\s*=\s*["']?application/ld\+json["']?[^>]*>(.*?)</script\s*>"#,
    )
    .expect("pattern")
});
/// How deep the markup is followed: Order → ParcelDelivery is two levels;
/// mail never needs many more.
const MAX_DEPTH: usize = 12;
/// Parcels taken from one message's markup.
const MAX_PARCELS: usize = 20;

/// The parcels in a message's schema.org `ParcelDelivery` markup.
pub fn from_markup(html: &str) -> Vec<Parcel> {
    let mut out = Vec::new();
    for caps in JSON_LD.captures_iter(html) {
        let raw = caps[1].trim();
        // Some senders wrap it as for old browsers.
        let raw = raw
            .trim_start_matches("<!--")
            .trim_end_matches("-->")
            .trim_start_matches("<![CDATA[")
            .trim_end_matches("]]>")
            .trim();
        let Ok(value) = serde_json::from_str::<Value>(raw) else {
            continue;
        };
        walk(&value, None, 0, &mut out);
    }
    out
}

/// Collects the `ParcelDelivery` objects in `value`, with the status of the
/// order they are in (`orderDelivery`) when they don't give their own.
fn walk(value: &Value, order_status: Option<Status>, depth: usize, out: &mut Vec<Parcel>) {
    if depth > MAX_DEPTH || out.len() >= MAX_PARCELS {
        return;
    }
    match value {
        Value::Array(items) => {
            for item in items {
                walk(item, order_status, depth + 1, out);
            }
        }
        Value::Object(map) => {
            let status = map.get("orderStatus").and_then(status_of).or(order_status);
            if is_type(value, "ParcelDelivery")
                && let Some(parcel) = parcel_of(value, status)
                && !out
                    .iter()
                    .any(|p| p.tracking_number == parcel.tracking_number)
            {
                out.push(parcel);
            }
            for child in map.values() {
                walk(child, status, depth + 1, out);
            }
        }
        _ => {}
    }
}

/// Whether the object's `@type` (a name, a schema.org address, or a list of
/// them) is `name`.
fn is_type(value: &Value, name: &str) -> bool {
    let named = |t: &Value| {
        t.as_str()
            .is_some_and(|t| t.rsplit(['/', '#', ':']).next() == Some(name))
    };
    match value.get("@type") {
        Some(Value::Array(types)) => types.iter().any(named),
        Some(t) => named(t),
        None => false,
    }
}

/// A text value, or the `name` of an object.
fn text_of(value: &Value) -> Option<String> {
    let s = match value {
        Value::String(s) => s.as_str(),
        Value::Object(_) => value.get("name")?.as_str()?,
        Value::Array(items) => return items.iter().find_map(text_of),
        _ => return None,
    };
    let s = s.trim();
    (!s.is_empty()).then(|| s.to_owned())
}

/// A status given as a name or address, or as an object (a DeliveryEvent)
/// naming one.
fn status_of(value: &Value) -> Option<Status> {
    match value {
        Value::String(s) => Status::parse(s),
        Value::Object(map) => ["@id", "name", "@type"]
            .iter()
            .filter_map(|k| map.get(*k)?.as_str())
            .find_map(Status::parse),
        _ => None,
    }
}

/// A link that opens in a browser, and nothing else (no `javascript:`).
fn web_link(value: &Value) -> Option<String> {
    web_url(&text_of(value)?)
}

fn parcel_of(value: &Value, order_status: Option<Status>) -> Option<Parcel> {
    let tracking_number: String = text_of(value.get("trackingNumber")?)?
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    // A tracking number is short, of letters and digits (and the odd
    // hyphen); anything else isn't one.
    if !(4..=64).contains(&tracking_number.len())
        || !tracking_number
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-')
    {
        return None;
    }
    let stated = ["carrier", "provider"]
        .iter()
        .find_map(|k| value.get(*k).and_then(text_of));
    // The catalog fills in what the markup leaves out, unless it takes the
    // number for another carrier's than the one stated.
    let known = tracking_numbers::track(&tracking_number.to_ascii_uppercase()).filter(|k| {
        stated
            .as_deref()
            .is_none_or(|s| same_carrier(s, &k.courier))
    });
    let carrier = stated.or_else(|| known.as_ref().map(|k| k.courier.clone()));
    let tracking_url = value
        .get("trackingUrl")
        .and_then(web_link)
        .or_else(|| known.as_ref().and_then(known_url));
    let expected_arrival = ["expectedArrivalUntil", "expectedArrivalFrom"]
        .iter()
        .find_map(|k| value.get(*k).and_then(text_of));
    let updated = dates::markup_updated(value);
    let status = value
        .get("deliveryStatus")
        .and_then(status_of)
        .or_else(|| {
            value
                .get("partOfOrder")
                .and_then(|o| o.get("orderStatus"))
                .and_then(status_of)
        })
        .or(order_status);
    let order = value
        .get("partOfOrder")
        .and_then(|o| o.get("orderNumber"))
        .and_then(text_of);
    Some(Parcel {
        tracking_number,
        order,
        carrier,
        service: known.map(|k| k.service),
        tracking_url,
        expected_arrival,
        status,
        updated,
        orders: Vec::new(),
        contents: None,
        products: Vec::new(),
        source: Source::Markup,
    })
}

/// Whether a carrier named in markup ("USPS", "FedEx Ground") is the
/// catalog's ("United States Postal Service", "FedEx").
fn same_carrier(stated: &str, catalog: &str) -> bool {
    let (s, c) = (stated.to_lowercase(), catalog.to_lowercase());
    let initials: String = catalog
        .split_whitespace()
        .filter_map(|w| w.chars().next())
        .collect::<String>()
        .to_lowercase();
    s.contains(&c) || c.contains(&s) || s == initials
}

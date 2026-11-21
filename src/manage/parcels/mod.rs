//! Parcels in mail: what a message says about a delivery, found without
//! contacting anyone.
//!
//! Where a message's parcels come from, in order of trust:
//!
//! 1. **Markup.** Shops and carriers put schema.org `ParcelDelivery` JSON-LD
//!    (`<script type="application/ld+json">`) in their mail for Gmail's
//!    delivery cards: the tracking number, carrier, tracking page, status
//!    and expected arrival, as the sender states them (`markup`).
//! 2. **Links.** A "Track package" button's address usually holds the
//!    tracking number: `ups.com/track?tracknum=1Z…`, a shop's
//!    `…/tracking?number=…`. A number there with a carrier's format and
//!    check digit, in a link about tracking, is taken (`links`).
//! 3. **Text.** Runs of uppercase letters and digits (possibly split into
//!    groups by spaces, as "1Z 999 AA1 01 2345 6784") that match a carrier's
//!    format and check digit in the tracking_number_data catalog, through the
//!    `tracking-numbers` crate. About one random 10- or 12-digit number in
//!    ten passes some carrier's check, so each candidate must be confirmed:
//!    by the local model when it is installed (`local_ai::parcel`), or else
//!    by a stricter rule (`text`).
//! 4. **Orders.** Many shops (Amazon among them) send "Shipped" and
//!    "Delivered" mail with no tracking number at all. A message whose
//!    subject says where a delivery is, and that gives an order number, is
//!    a delivery of that order (`words`).
//!
//! Each message's own stage (shipped, out for delivery, delivered…) is read
//! from its markup or, failing that, its subject and first lines (`words`),
//! and the update time from the markup or an "Updated:" line (`dates`).
//! `summary` gathers what every message says of each parcel into one entry
//! with its whole history, as each Summary lists them.
//!
//! Nothing here looks a parcel up: the tracking page is only a link.
mod dates;
mod links;
mod markup;
mod summary;
#[cfg(test)]
mod tests;
mod text;
mod words;

use super::local_ai::parcel::Confirmer;
use serde::{Deserialize, Serialize};

pub use markup::from_markup;
pub use summary::{Event, Override, STAGES, Sighting, Tracked, apply_overrides, summarize};
#[cfg(test)]
pub(crate) use summary::summarize_at;
#[cfg(test)]
pub(crate) use text::candidates;
pub use text::from_text;
pub use words::{delivery_news, looks_like_shipping, status_in, worth_reading};
pub(crate) use words::order_numbers;

/// A parcel a message tells of.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Parcel {
    /// As the carrier writes it, without spaces; empty for a delivery known
    /// only by its order (`order`).
    pub tracking_number: String,
    /// The order it belongs to, when the message gives its number.
    #[serde(default)]
    pub order: Option<String>,
    /// The carrier's name ("UPS", "United States Postal Service"...).
    pub carrier: Option<String>,
    /// The carrier's service the number's format belongs to ("USPS 22").
    pub service: Option<String>,
    /// The carrier's (or shop's) tracking page (http or https only).
    pub tracking_url: Option<String>,
    /// When it is expected to arrive by (ISO 8601), as the markup gives it
    /// or the message says ("Expected Delivery on Friday, October 30").
    pub expected_arrival: Option<String>,
    /// Where it is, as this message says.
    pub status: Option<Status>,
    /// When the message says the parcel was last updated (Unix seconds), if
    /// it says: a delivery event's time in the markup, or an "Updated:"
    /// date in the text.
    pub updated: Option<i64>,
    /// Every order the message names (`order` is the first): one shipment
    /// may carry several orders' items.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub orders: Vec<String>,
    /// What the subject says is in it, after its stage ("Shipped: 1
    /// Luggage item" → "1 luggage item"), normalized: shops that send no
    /// tracking number tell an order's shipments apart by it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contents: Option<String>,
    /// The products the message lists, as shortened names ("Hard Shell
    /// Travel Case for Mous..."), normalized: a shop's "shipped" and "delivered"
    /// emails that share no number share these.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub products: Vec<String>,
    pub source: Source,
}

impl Parcel {
    /// What the Summary gathers its messages by: the tracking number, or
    /// the order for a delivery without one.
    pub fn key(&self) -> String {
        if self.tracking_number.is_empty() {
            format!("order:{}", self.order.as_deref().unwrap_or_default())
        } else {
            self.tracking_number.clone()
        }
    }
    /// How the Summary names it: its tracking number, or its order.
    pub fn title(&self) -> String {
        match (&self.tracking_number, &self.order) {
            (n, _) if !n.is_empty() => n.clone(),
            (_, Some(order)) => format!("Order {order}"),
            _ => String::new(),
        }
    }
}

/// Where a parcel was found.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Source {
    /// The sender's schema.org markup.
    Markup,
    /// A tracking link's address.
    Link,
    /// The message's text, confirmed by the local model.
    Model,
    /// The message's text, confirmed by the rule used without the model.
    Rule,
    /// A delivery message giving only an order number.
    Order,
}

/// Where a parcel is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Status {
    /// Ordered; not yet handed to a carrier.
    Processing,
    /// Handed to the carrier, or its label made.
    Shipped,
    InTransit,
    OutForDelivery,
    PickupAvailable,
    Delivered,
    /// Delayed, a failed delivery attempt, an exception.
    Problem,
    Returned,
    Cancelled,
}

impl Status {
    /// The status a schema.org value names: "OrderInTransit",
    /// "http://schema.org/OrderDelivered", or a DeliveryEvent's name.
    fn parse(value: &str) -> Option<Self> {
        let name = value.rsplit(['/', '#', ':']).next().unwrap_or(value);
        let name = name.strip_prefix("Order").unwrap_or(name);
        let lower = name.to_ascii_lowercase();
        Some(match lower.as_str() {
            "processing" | "paymentdue" | "paymentcomplete" => Self::Processing,
            "shipped" => Self::Shipped,
            "intransit" => Self::InTransit,
            "outfordelivery" => Self::OutForDelivery,
            "pickupavailable" => Self::PickupAvailable,
            "delivered" => Self::Delivered,
            "problem" => Self::Problem,
            "returned" => Self::Returned,
            "cancelled" | "canceled" => Self::Cancelled,
            _ => return None,
        })
    }

    /// Every status, in the order they are offered to set by hand.
    pub const ALL: [Status; 9] = [
        Self::Processing,
        Self::Shipped,
        Self::InTransit,
        Self::OutForDelivery,
        Self::PickupAvailable,
        Self::Delivered,
        Self::Problem,
        Self::Returned,
        Self::Cancelled,
    ];

    /// How the Summary names it.
    pub fn label(self) -> &'static str {
        match self {
            Self::Processing => "Ordered",
            Self::Shipped => "Shipped",
            Self::InTransit => "In transit",
            Self::OutForDelivery => "Out for delivery",
            Self::PickupAvailable => "Ready for pickup",
            Self::Delivered => "Delivered",
            Self::Problem => "Delayed or a problem",
            Self::Returned => "Returned",
            Self::Cancelled => "Cancelled",
        }
    }

    /// Its place among `STAGES`: ordered, shipped, in transit, out for
    /// delivery (or ready for pickup), delivered. A problem, a return or a
    /// cancellation is no stage of the way.
    pub fn stage(self) -> Option<usize> {
        match self {
            Self::Processing => Some(0),
            Self::Shipped => Some(1),
            Self::InTransit => Some(2),
            Self::OutForDelivery | Self::PickupAvailable => Some(3),
            Self::Delivered => Some(4),
            Self::Problem | Self::Returned | Self::Cancelled => None,
        }
    }
}

/// A message's text as `find_in_mail` reads it.
pub struct MailText<'a> {
    pub subject: &'a str,
    /// Its plain text, or the text of its HTML, or (when neither was
    /// downloaded) its preview.
    pub text: &'a str,
    pub html: Option<&'a str>,
    /// When it was sent (Unix seconds).
    pub sent: i64,
}

/// The parcels in a message: in its markup, its tracking links, then its
/// text (confirmed by the model, or without it the rule); each tracking
/// number once. Without any, a delivery message giving an order number is
/// that order's delivery. Each gets the message's stage and update date
/// where its markup gives none.
pub fn find_in_mail(mail: &MailText<'_>, model: Option<&Confirmer>) -> Vec<Parcel> {
    let mut parcels = mail.html.map(from_markup).unwrap_or_default();
    let add = |found: Vec<Parcel>, parcels: &mut Vec<Parcel>| {
        for p in found {
            if !parcels
                .iter()
                .any(|q| q.tracking_number == p.tracking_number)
            {
                parcels.push(p);
            }
        }
    };
    if let Some(html) = mail.html {
        add(links::from_links(html), &mut parcels);
    }
    add(from_text(mail.subject, mail.text, model), &mut parcels);
    let status = status_in(mail.subject, mail.text);
    let orders = words::order_numbers(mail.subject, mail.text);
    let order = orders.first().cloned();
    // A shop's own package number ("Package UUSC000012345678 has been
    // delivered"), in no carrier's catalog, where a delivery is news.
    if parcels.is_empty()
        && status.is_some_and(|s| s != Status::Processing)
        && let Some(number) = text::package_number(mail.subject, mail.text)
    {
        parcels.push(Parcel {
            tracking_number: number,
            order: None,
            carrier: None,
            service: None,
            tracking_url: mail.html.and_then(links::tracking_page),
            expected_arrival: None,
            status: None,
            updated: None,
            orders: Vec::new(),
            contents: None,
            products: Vec::new(),
            source: Source::Rule,
        });
    }
    // An order confirmation is a delivery when it says when it arrives
    // ("Ordered: …", "Arriving Friday"); without that it may be for a meal
    // or a subscription.
    let ordered = status == Some(Status::Processing)
        && words::order_subject(mail.subject)
        && words::mentions_arrival(mail.subject, mail.text);
    if parcels.is_empty()
        && let Some(order) = &order
        && (ordered
            // About a parcel at all: not a "Rewards order number" for a
            // statement credit "on the way".
            || (words::about_parcels(mail.subject, mail.text)
                && ((status.is_some_and(|s| s != Status::Processing)
                    && words::delivery_subject(mail.subject))
                    // "An update on your order" whose text says it was
                    // delivered.
                    || (status.is_some_and(|s| {
                        !matches!(s, Status::Processing | Status::Cancelled | Status::Problem)
                    }) && words::shipping_subject(mail.subject)))))
    {
        parcels.push(Parcel {
            tracking_number: String::new(),
            order: Some(order.clone()),
            carrier: None,
            service: None,
            tracking_url: mail.html.and_then(links::tracking_page),
            expected_arrival: None,
            status: None,
            updated: None,
            orders: Vec::new(),
            contents: None,
            products: Vec::new(),
            source: Source::Order,
        });
    }
    let said = dates::update_date(mail.text, mail.sent);
    let expected = dates::expected_date(mail.subject, mail.text, mail.sent);
    let contents = words::contents(mail.subject);
    let products = words::products(mail.subject, mail.text);
    for p in &mut parcels {
        p.status = p.status.or(status);
        p.order = p.order.take().or_else(|| order.clone());
        p.orders = orders.clone();
        if let Some(own) = &p.order
            && !p.orders.contains(own)
        {
            p.orders.insert(0, own.clone());
        }
        p.contents = contents.clone();
        p.products = products.clone();
        p.expected_arrival = p.expected_arrival.take().or_else(|| expected.clone());
        p.updated = p
            .updated
            .or(said)
            .filter(|&t| dates::plausible(t, mail.sent));
    }
    parcels
}

/// The parcels in a message's subject, text and HTML, as the reader shows
/// them (see `find_in_mail`).
pub fn find(
    subject: &str,
    body: &str,
    html: Option<&str>,
    sent: i64,
    model: Option<&Confirmer>,
) -> Vec<Parcel> {
    let converted;
    let text = if body.is_empty() {
        converted = html
            .map(crate::sync::mime::html_to_text)
            .unwrap_or_default();
        converted.as_str()
    } else {
        body
    };
    find_in_mail(
        &MailText {
            subject,
            text,
            html,
            sent,
        },
        model,
    )
}

/// The catalog's tracking page for a number, if it has one.
fn known_url(known: &tracking_numbers::TrackingResult) -> Option<String> {
    web_url(&known.tracking_url)
}

/// An address that opens in a browser, and nothing else (no `javascript:`).
fn web_url(link: &str) -> Option<String> {
    let url = url::Url::parse(link.trim()).ok()?;
    matches!(url.scheme(), "http" | "https").then(|| url.to_string())
}

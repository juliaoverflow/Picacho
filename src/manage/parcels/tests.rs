//! Finding parcels in mail and gathering them.
use super::dates::{local, update_date};
use super::text::candidates;
use super::*;
use crate::manage::local_ai::otp::Text;
use chrono::NaiveDate;

fn numbers(parcels: &[Parcel]) -> Vec<&str> {
    parcels.iter().map(|p| p.tracking_number.as_str()).collect()
}

#[test]
fn reads_parcel_delivery_markup() {
    let html = r#"<html><head>
<script type="application/ld+json">
{
  "@context": "http://schema.org",
  "@type": "ParcelDelivery",
  "carrier": {"@type": "Organization", "name": "FedEx"},
  "trackingNumber": "3453291231",
  "trackingUrl": "http://fedex.com/Tracking?tracknumbers=3453291231",
  "expectedArrivalUntil": "2027-03-12T12:00:00-08:00",
  "partOfOrder": {"@type": "Order", "orderNumber": "176057",
                  "orderStatus": "http://schema.org/OrderInTransit"}
}
</script></head><body>Your order shipped.</body></html>"#;
    let parcels = from_markup(html);
    assert_eq!(
        parcels,
        [Parcel {
            tracking_number: "3453291231".into(),
            order: Some("176057".into()),
            carrier: Some("FedEx".into()),
            service: None,
            tracking_url: Some("http://fedex.com/Tracking?tracknumbers=3453291231".into()),
            expected_arrival: Some("2027-03-12T12:00:00-08:00".into()),
            status: Some(Status::InTransit),
            updated: None,
            orders: Vec::new(),
            contents: None,
            products: Vec::new(),
            source: Source::Markup,
        }]
    );
}

#[test]
fn reads_parcels_inside_orders_and_graphs() {
    let html = r#"<script type='application/ld+json'><!--
{"@graph": [
  {"@type": "Order", "orderStatus": "OrderDelivered",
   "orderDelivery": {"@type": "http://schema.org/ParcelDelivery",
                     "trackingNumber": "1Z 5R8 939 0357 5671 27",
                     "trackingUrl": "javascript:alert(1)"}},
  {"@type": ["ParcelDelivery"], "trackingNumber": "RB123456785GB",
   "carrier": "Royal Mail", "deliveryStatus": {"@type": "DeliveryEvent", "name": "OutForDelivery"}},
  {"@type": "ParcelDelivery", "trackingNumber": "RB123456785GB"},
  {"@type": "ParcelDelivery", "trackingNumber": "<b>no</b> such thing"}
]}
--></script>"#;
    let parcels = from_markup(html);
    assert_eq!(numbers(&parcels), ["1Z5R89390357567127", "RB123456785GB"]);
    let ups = &parcels[0];
    assert_eq!(ups.carrier.as_deref(), Some("UPS"));
    assert_eq!(ups.status, Some(Status::Delivered));
    // Not the markup's script link: the catalog's page.
    let url = ups.tracking_url.as_deref().unwrap();
    assert!(
        url.starts_with("http") && url.contains("1Z5R89390357567127"),
        "{url}"
    );
    assert_eq!(parcels[1].carrier.as_deref(), Some("Royal Mail"));
    assert_eq!(parcels[1].status, Some(Status::OutForDelivery));
    assert!(from_markup("<script type=application/ld+json>{not json</script>").is_empty());
}

#[test]
fn finds_numbers_with_a_carriers_format_and_check_digit() {
    let text = Text::new(
        "Your order has shipped",
        "UPS: 1Z5R89390357567127\nUSPS 9400 1112 0620 6406 2607 87 and 1Z5R89390357567128.\n追跡番号RB123456785GB",
    );
    let found: Vec<(String, String)> = candidates(&text)
        .into_iter()
        .map(|c| (text.slice(c.start, c.end), c.known.courier))
        .collect();
    assert_eq!(
        found,
        [
            ("1Z5R89390357567127".into(), "UPS".into()),
            (
                "9400 1112 0620 6406 2607 87".into(),
                "United States Postal Service".into()
            ),
            ("RB123456785GB".into(), "S10 International Standard".into()),
        ]
    );
    // Part of a date or phone number, or touching a letter.
    let text = Text::new(
        "",
        "Call +9400111206206406260787, on 2026-9400111206206406260787, X1Z5R89390357567127",
    );
    assert!(candidates(&text).is_empty());
}

#[test]
fn without_the_model_the_rule_wants_a_shipping_word_and_no_short_bare_number() {
    let found = from_text(
        "Your order",
        "Tracking number: 1Z5R89390357567127\nOrder 986578788855\nCall 3318810025",
        None,
    );
    assert_eq!(numbers(&found), ["1Z5R89390357567127"]);
    assert_eq!(found[0].source, Source::Rule);
    assert_eq!(found[0].carrier.as_deref(), Some("UPS"));
    // A short bare number isn't taken even beside a shipping word.
    assert!(from_text("Shipped", "Tracking: 3318810025", None).is_empty());
}

#[test]
fn markup_comes_first_and_numbers_are_not_repeated() {
    let html = r#"<script type="application/ld+json">{"@type":"ParcelDelivery","trackingNumber":"1Z5R89390357567127","carrier":"UPS"}</script>"#;
    let found = find(
        "Shipped",
        "Tracking 1Z5R89390357567127, also tracking RB123456785GB",
        Some(html),
        0,
        None,
    );
    assert_eq!(numbers(&found), ["1Z5R89390357567127", "RB123456785GB"]);
    assert_eq!(found[0].source, Source::Markup);
    assert_eq!(found[1].source, Source::Rule);
}

#[test]
fn reads_update_dates_from_markup_and_text() {
    let at = |y, m, d, h, min| {
        local(
            NaiveDate::from_ymd_opt(y, m, d)
                .unwrap()
                .and_hms_opt(h, min, 0)
                .unwrap(),
        )
        .unwrap()
    };
    let sent = at(2026, 3, 5, 9, 0);
    assert_eq!(
        update_date("Last updated: 2026-03-04 10:22", sent),
        Some(at(2026, 3, 4, 10, 22))
    );
    assert_eq!(
        update_date("Updated on March 4, 2026 at 10:22 AM", sent),
        Some(at(2026, 3, 4, 10, 22))
    );
    assert_eq!(
        update_date("Status as of 4 March 2026", sent),
        Some(at(2026, 3, 4, 12, 0))
    );
    assert_eq!(
        update_date("Aktualisiert am 04.03.2026, 18:05", sent),
        Some(at(2026, 3, 4, 18, 5))
    );
    // Well after the message was sent, or long before: not an update.
    assert_eq!(update_date("Updated: 2026-04-20", sent), None);
    assert_eq!(update_date("Updated: 2025-01-01", sent), None);
    let html = r#"<script type="application/ld+json">{"@type":"ParcelDelivery","trackingNumber":"1Z5R89390357567127","deliveryStatus":{"@type":"DeliveryEvent","name":"OrderDelivered","dateTime":"2026-03-04T15:30:00Z"}}</script>"#;
    let found = find("Delivered", "Updated: 2026-03-01", Some(html), sent, None);
    assert_eq!(found[0].updated, Some(1_772_638_200));
    assert_eq!(found[0].status, Some(Status::Delivered));
    let found = find(
        "Shipped",
        "Tracking 1Z879E930346834440\nLast update: 2026-03-03 08:00",
        None,
        sent,
        None,
    );
    assert_eq!(found[0].updated, Some(at(2026, 3, 3, 8, 0)));
}

#[test]
fn the_summary_lists_each_parcel_once_by_its_last_update() {
    let sighting = |id: &str, sent: i64| Sighting {
        account: "a".into(),
        mail_id: id.into(),
        message_id: format!("<{id}@shop>"),
        mailbox: crate::manage::model::Folder::Inbox.into(),
        subject: format!("Message {id}"),
        sender: "Shop".into(),
        sent,
    };
    let parcel = |number: &str, carrier: Option<&str>, updated: Option<i64>| Parcel {
        tracking_number: number.into(),
        order: None,
        carrier: carrier.map(Into::into),
        service: None,
        tracking_url: None,
        expected_arrival: None,
        status: None,
        updated,
        orders: Vec::new(),
        contents: None,
        products: Vec::new(),
        source: Source::Rule,
    };
    let tracked = summarize(vec![
        (parcel("A", Some("UPS"), None), sighting("a1", 100)),
        (parcel("B", None, None), sighting("b1", 300)),
        (parcel("A", None, Some(400)), sighting("a2", 200)),
        // A copy of a2 in another mailbox.
        (parcel("A", None, Some(400)), sighting("a2", 200)),
    ]);
    let order: Vec<(&str, i64, usize)> = tracked
        .iter()
        .map(|t| {
            (
                t.parcel.tracking_number.as_str(),
                t.updated,
                t.history.len(),
            )
        })
        .collect();
    assert_eq!(order, [("A", 400, 2), ("B", 300, 1)]);
    assert_eq!(tracked[0].latest().mail_id, "a2");
    // The carrier comes from the older message.
    assert_eq!(tracked[0].parcel.carrier.as_deref(), Some("UPS"));
}

/// The model takes tracking numbers given as such, and not the same
/// kind of number given as an order, invoice or phone number.
#[test]
fn the_model_confirms_tracking_numbers_by_their_context() {
    let model = crate::manage::local_ai::parcel::tests::confirmer();
    let cases = [
        (
            "Your package is on its way",
            "Good news! Your order has shipped.\nTracking number: 986578788855\nOrder number: 477179081230",
            vec!["986578788855"],
        ),
        (
            "Ihre Bestellung wurde versandt",
            "Sendungsnummer: 1Z879E930346834440\nBestellnummer 3318810025",
            vec!["1Z879E930346834440"],
        ),
        (
            "Invoice 3318810025",
            "Thanks for your payment. Invoice number 3318810025, account 986578788855.",
            vec![],
        ),
    ];
    for (subject, body, want) in cases {
        let found = from_text(subject, body, Some(&model));
        assert_eq!(numbers(&found), want, "{subject}");
        assert!(
            found
                .iter()
                .all(|p| matches!(p.source, Source::Model | Source::Rule))
        );
    }
}

#[test]
fn reads_where_a_delivery_is_from_the_subject_and_first_lines() {
    let cases = [
        ("Delivered: \"Kettle\"", "", Some(Status::Delivered)),
        (
            "Your package has been delivered",
            "",
            Some(Status::Delivered),
        ),
        ("Out for delivery today", "", Some(Status::OutForDelivery)),
        (
            "Ihr Paket wird heute zugestellt",
            "",
            Some(Status::OutForDelivery),
        ),
        ("Ihre Sendung wurde zugestellt", "", Some(Status::Delivered)),
        ("Tu pedido ha sido enviado", "", Some(Status::Shipped)),
        ("Votre colis est en route", "", Some(Status::InTransit)),
        (
            "USPS® Expected Delivery on Monday",
            "",
            Some(Status::InTransit),
        ),
        (
            "UPS Ship Notification, Tracking Number 1Z5R89390357567127",
            "",
            Some(Status::Shipped),
        ),
        ("Your package is delayed", "", Some(Status::Problem)),
        (
            "Ready for pickup at your Post Office",
            "",
            Some(Status::PickupAvailable),
        ),
        ("ご注文の商品を発送しました", "", Some(Status::Shipped)),
        ("包裹已签收", "", Some(Status::Delivered)),
        ("Thanks for your order", "", Some(Status::Processing)),
        // Promises aren't news.
        (
            "Order confirmation",
            "We'll let you know when it has shipped.",
            Some(Status::Processing),
        ),
        ("Hello", "Your order will be delivered on Friday.", None),
        // Membership isn't shipping.
        ("Your membership renewal", "", None),
        // The subject first; the text only without it.
        (
            "An update on your order",
            "Good news: your order has shipped.",
            Some(Status::Shipped),
        ),
    ];
    for (subject, text, want) in cases {
        assert_eq!(status_in(subject, text), want, "{subject:?} / {text:?}");
    }
}

#[test]
fn finds_tracking_numbers_in_tracking_links_only() {
    let html = r#"
<a href="https://www.ups.com/track?loc=en_US&amp;tracknum=1Z5R89390357567127">Track</a>
<a href="https://shop.example.com/orders/tracking?tracking_number=RB123456785GB&amp;carrier=royal">Where's my order?</a>
<a href="https://click.track.example.com/ls/click?upn=1Z879E930346834440">Unsubscribe</a>
<a href="https://shop.example.com/product/986578788855">A kettle</a>
<a href="https://click.example.com/abc?url=https%3A%2F%2Fwww.fedex.com%2Ffedextrack%2F%3Ftrknbr%3D986578788855">Track your FedEx package</a>"#;
    let found = find("Your order", "", Some(html), 0, None);
    let numbers: Vec<(&str, Source)> = found
        .iter()
        .map(|p| (p.tracking_number.as_str(), p.source))
        .collect();
    assert_eq!(
        numbers,
        [
            ("1Z5R89390357567127", Source::Link),
            ("RB123456785GB", Source::Link),
            ("986578788855", Source::Link),
        ]
    );
    assert!(
        found[0]
            .tracking_url
            .as_deref()
            .unwrap()
            .contains("ups.com")
    );
}

#[test]
fn a_delivery_without_a_tracking_number_is_known_by_its_order() {
    let html = r#"<p>Order #113-1234567-1234567</p><a href="https://www.amazon.com/gp/your-account/ship-track?orderId=113-1234567-1234567">Track package</a>"#;
    let found = find("Delivered: \"Kettle\"", "", Some(html), 0, None);
    assert_eq!(found.len(), 1);
    let p = &found[0];
    assert_eq!(
        (p.title().as_str(), p.source, p.status),
        (
            "Order 113-1234567-1234567",
            Source::Order,
            Some(Status::Delivered)
        )
    );
    assert!(p.tracking_url.as_deref().unwrap().contains("ship-track"));
    // An order confirmation, or an order number in a newsletter, is none.
    assert!(
        find(
            "Thanks for your order",
            "Order 113-1234567-1234567",
            None,
            0,
            None
        )
        .is_empty()
    );
    assert!(find("Weekly deals", "Order 113-1234567-1234567", None, 0, None).is_empty());
}

#[test]
fn a_parcels_history_and_stage_come_from_all_its_emails() {
    let sighting = |id: &str, account: &str, sent: i64| Sighting {
        account: account.into(),
        mail_id: id.into(),
        message_id: format!("<{id}@shop>"),
        mailbox: crate::manage::model::Folder::Inbox.into(),
        subject: format!("Message {id}"),
        sender: "Shop".into(),
        sent,
    };
    let parcel = |number: &str, order: Option<&str>, status: Option<Status>| Parcel {
        tracking_number: number.into(),
        order: order.map(Into::into),
        carrier: None,
        service: None,
        tracking_url: None,
        expected_arrival: None,
        status,
        updated: None,
        orders: Vec::new(),
        contents: None,
        products: Vec::new(),
        source: if number.is_empty() {
            Source::Order
        } else {
            Source::Rule
        },
    };
    let tracked = summarize(vec![
        (
            parcel("", Some("A-1001"), Some(Status::Shipped)),
            sighting("o1", "a", 100),
        ),
        (
            parcel(
                "1Z5R89390357567127",
                Some("A-1001"),
                Some(Status::InTransit),
            ),
            sighting("t1", "a", 200),
        ),
        (
            parcel("", Some("A-1001"), Some(Status::Delivered)),
            sighting("o2", "a", 400),
        ),
        // A reminder after delivery is left out.
        (
            parcel("1Z5R89390357567127", None, Some(Status::Shipped)),
            sighting("t2", "a", 500),
        ),
        // The same order number at another account's shop is another order.
        (
            parcel("", Some("A-1001"), Some(Status::Problem)),
            sighting("x1", "b", 300),
        ),
    ]);
    assert_eq!(tracked.len(), 2);
    let ups = &tracked[0];
    assert_eq!(ups.parcel.title(), "1Z5R89390357567127");
    assert_eq!(ups.parcel.order.as_deref(), Some("A-1001"));
    let ids: Vec<&str> = ups
        .history
        .iter()
        .map(|e| e.sighting.mail_id.as_str())
        .collect();
    // Only the first "Delivered" email counts: the reminder after it is
    // left out.
    assert_eq!(ids, ["o2", "t1", "o1"]);
    assert_eq!((ups.stage, ups.delivered(), ups.trouble), (4, true, None));
    let other = &tracked[1];
    assert_eq!(other.parcel.title(), "Order A-1001");
    assert_eq!(other.trouble, Some(Status::Problem));
    assert_eq!(other.status_label(), "Delayed or a problem");
    assert_eq!(other.stage, 0);
}

/// Looks through a copy of a real mail cache for parcels and prints only
/// counts (no mail). It clears the copy's saved findings, so never point
/// it at the live data folder: `PICACHO_DIAG_DIR=<dir with mail.sqlite3>
/// cargo test parcels_in_a_real_cache -- --ignored --nocapture`.
/// `PICACHO_DIAG_LIST=all` (or `stuck`, for those not delivered) also
/// lists each parcel with its emails' senders and subjects, and
/// `PICACHO_DIAG_MISSED` and `PICACHO_DIAG_CODES` the subjects of mail
/// passed over and of code mail: that output is your mail, not to share.
#[test]
#[ignore]
fn parcels_in_a_real_cache() {
    let Some(dir) = std::env::var_os("PICACHO_DIAG_DIR") else {
        return;
    };
    let store = crate::manage::store::Store::open(std::path::Path::new(&dir)).unwrap();
    let accounts: Vec<String> = store
        .accounts()
        .unwrap()
        .into_iter()
        .map(|a| a.id)
        .collect();
    // Found afresh, as this build finds them.
    db_for(&dir).execute("DELETE FROM parcel_scan", []).ok();
    for pass in 0..2 {
        let started = std::time::Instant::now();
        // The second pass with the model, as Find tracking numbers reads.
        let model = (pass == 1).then(crate::manage::local_ai::parcel::tests::confirmer);
        let scan = store.parcels(&accounts, model.as_ref()).unwrap();
        let mut sources: std::collections::BTreeMap<String, usize> = Default::default();
        let mut stages = [0usize; 5];
        let mut carriers: std::collections::BTreeMap<String, usize> = Default::default();
        let mut trouble = 0;
        for t in &scan.tracked {
            *sources.entry(format!("{:?}", t.parcel.source)).or_default() += 1;
            stages[t.stage] += 1;
            *carriers
                .entry(t.parcel.carrier.clone().unwrap_or("-".into()))
                .or_default() += 1;
            trouble += usize::from(t.trouble.is_some());
        }
        let emails: usize = scan.tracked.iter().map(|t| t.history.len()).sum();
        println!(
            "pass {pass}: {:?}, scanned {}, parcels {}, emails {}, to download {}",
            started.elapsed(),
            scan.scanned,
            scan.tracked.len(),
            emails,
            scan.to_download.len()
        );
        println!(
            "  sources {sources:?}\n  stages {stages:?} trouble {trouble}\n  carriers {carriers:?}"
        );
        let mut by: std::collections::BTreeMap<String, (usize, usize)> = Default::default();
        for t in &scan.tracked {
            let e = by
                .entry(format!(
                    "{:?} {}",
                    t.parcel.source,
                    t.parcel.service.clone().unwrap_or_default()
                ))
                .or_default();
            e.0 += 1;
            e.1 += usize::from(t.history.iter().any(|e| e.status.is_some()));
        }
        println!("  (parcels, with a status) by source and service: {by:?}");
        let mut q2 = db_for(&dir);
        let mut statusless = (0, 0);
        for t in scan.tracked.iter().filter(|t| {
            t.parcel.source == Source::Rule && t.history.iter().all(|e| e.status.is_none())
        }) {
            statusless.0 += 1;
            let from: String = q2
                .query_row(
                    "SELECT coalesce(json_extract(data,'$.from'),'') FROM messages WHERE id=?1",
                    [&t.latest().mail_id],
                    |r| r.get(0),
                )
                .unwrap();
            let from = from.to_ascii_lowercase();
            if super::words::SHIPPING_SENDERS
                .iter()
                .any(|s| from.contains(s))
            {
                statusless.1 += 1
            }
        }
        let _ = &mut q2;
        println!(
            "  rule parcels without a status: {}, from a shipping sender: {}",
            statusless.0, statusless.1
        );
        // Why mail is to be downloaded.
        let mut why = [0usize; 4];
        let mut senders: std::collections::BTreeMap<&str, usize> = Default::default();
        let db =
            rusqlite::Connection::open(std::path::Path::new(&dir).join("mail.sqlite3")).unwrap();
        let mut q = db.prepare("SELECT coalesce(json_extract(data,'$.subject'),''), coalesce(json_extract(data,'$.snippet'),''), coalesce(json_extract(data,'$.from'),'') FROM messages WHERE id=?1").unwrap();
        for (_, id) in &scan.to_download {
            let (subject, snippet, from): (String, String, String) = q
                .query_row([id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                .unwrap();
            if super::words::delivery_subject(&subject) {
                why[0] += 1
            } else if super::text::parcel_concepts(&subject)
                .iter()
                .any(|c| super::text::PARCEL_WORDS.contains(c))
            {
                why[1] += 1;
                if pass == 0 && why[1] <= 40 {
                    println!("    concept: {:?}", super::text::parcel_concepts(&subject));
                }
            } else if !super::from_text(&subject, &snippet, None).is_empty() {
                why[3] += 1
            } else {
                why[2] += 1;
                let from = from.to_ascii_lowercase();
                if let Some(s) = super::words::SHIPPING_SENDERS
                    .iter()
                    .find(|s| from.contains(*s))
                {
                    *senders.entry(*s).or_default() += 1;
                }
            }
        }
        println!("  senders {senders:?}");
        println!(
            "  to download because: delivery subject {}, parcel words {}, sender {}, preview number {}",
            why[0], why[1], why[2], why[3]
        );
    }
}

fn db_for(dir: &std::ffi::OsStr) -> rusqlite::Connection {
    rusqlite::Connection::open(std::path::Path::new(dir).join("mail.sqlite3")).unwrap()
}

#[test]
fn product_codes_are_not_tracking_numbers() {
    // DHL E-Commerce's format has no check digit and takes a GPU's model.
    assert!(tracking_numbers::track("RX9070XT16GB").is_some());
    assert!(
        from_text(
            "Free shipping on graphics cards",
            "Shop the RX9070XT16GB now",
            None
        )
        .is_empty()
    );
    let html = r#"<a href="https://www.dhl.com/track?AWB=RX9070XT16GB">Track</a>"#;
    assert!(find("Deals", "", Some(html), 0, None).is_empty());
    // Real numbers with letters still are.
    assert_eq!(
        numbers(&from_text(
            "Shipped",
            "Tracking: 1Z5R89390357567127, RB123456785GB",
            None
        )),
        ["1Z5R89390357567127", "RB123456785GB"]
    );
}

#[test]
fn orders_and_pickups_are_not_in_transit() {
    let cases = [
        (
            "Ordered: \"Graphics card\"",
            "Arriving Friday",
            Some(Status::Processing),
        ),
        (
            "Thanks for your order",
            "Estimated delivery: Friday, March 6",
            Some(Status::Processing),
        ),
        (
            "Your order is ready to pick up",
            "",
            Some(Status::PickupAvailable),
        ),
        (
            "An update on your order",
            "Your package is ready to be picked up at the store.",
            Some(Status::PickupAvailable),
        ),
        // A carrier's estimate alone is still on its way.
        ("Scheduled delivery: Tuesday", "", Some(Status::InTransit)),
    ];
    for (subject, text, want) in cases {
        assert_eq!(status_in(subject, text), want, "{subject:?} / {text:?}");
    }
    // An order placed for something shipped is listed as ordered.
    let found = find(
        "Ordered: \"Graphics card\"",
        "Order #113-1234567-1234567\nArriving Friday",
        None,
        0,
        None,
    );
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].status, Some(Status::Processing));
    // Ready for pickup is named so, not "Out for delivery".
    let mut ready = found[0].clone();
    ready.status = Some(Status::PickupAvailable);
    let tracked = summarize_at(vec![(
        ready,
        Sighting {
            account: "a".into(),
            mail_id: "m".into(),
            message_id: "<m@shop>".into(),
            mailbox: crate::manage::model::Folder::Inbox.into(),
            subject: "Ready".into(),
            sender: "Shop".into(),
            sent: 0,
        },
    )], 0);
    assert_eq!(tracked[0].status_label(), "Ready for pickup");
}

#[test]
fn deliveries_are_found_however_they_are_said() {
    let cases = [
        // A promise in another sentence or line doesn't hide the news.
        (
            "An update on your order",
            "Questions? We will help.\nDelivered: your package was left at the front door.",
            Some(Status::Delivered),
        ),
        (
            "Your order",
            "Estimated delivery Friday | Delivered today at 2:14 PM",
            Some(Status::Delivered),
        ),
        ("Your package has arrived", "", Some(Status::Delivered)),
        // A carrier's scan on the way is not the delivery.
        (
            "Your package has arrived at the sorting facility",
            "",
            Some(Status::InTransit),
        ),
        // A progress bar names every stage; the words beside it say where.
        (
            "An update on your order",
            "Ordered  Shipped  Out for delivery  Delivered\nYour package is on its way.",
            Some(Status::InTransit),
        ),
    ];
    for (subject, text, want) in cases {
        assert_eq!(status_in(subject, text), want, "{subject:?} / {text:?}");
    }
    // An order update whose text says it was delivered is that order's
    // delivery, though its subject doesn't say so.
    let found = find(
        "An update on your order",
        "Your package was delivered.\nOrder #113-1234567-1234567",
        None,
        0,
        None,
    );
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].status, Some(Status::Delivered));
    // A promotion saying "free shipping" with an order number is none.
    assert!(
        find(
            "Free shipping this weekend",
            "Delivered to your door. Order 113-1234567-1234567",
            None,
            0,
            None
        )
        .is_empty()
    );
}

/// Lists what a copy of a real cache's Summary shows, for diagnosis:
/// `PICACHO_DIAG_DIR=<copy> cargo test parcels_report -- --ignored --nocapture`.
/// Keeps what was found, so a second run times the cached look.
#[test]
#[ignore]
fn parcels_report() {
    let Some(dir) = std::env::var_os("PICACHO_DIAG_DIR") else {
        return;
    };
    let store = crate::manage::store::Store::open(std::path::Path::new(&dir)).unwrap();
    let accounts: Vec<String> = store.accounts().unwrap().into_iter().map(|a| a.id).collect();
    let started = std::time::Instant::now();
    let scan = store.parcels(&accounts, None).unwrap();
    println!("look: {:?}, scanned {}, parcels {}", started.elapsed(), scan.scanned, scan.tracked.len());
    let started = std::time::Instant::now();
    let _ = store.parcels(&accounts, None).unwrap();
    println!("again (cached): {:?}", started.elapsed());
    // What the Packages search reads to search the emails' text.
    let ids: Vec<String> = scan
        .tracked
        .iter()
        .flat_map(|t| t.history.iter().map(|e| e.sighting.mail_id.clone()))
        .collect();
    let started = std::time::Instant::now();
    let texts = store.parcel_search_texts(&ids).unwrap();
    let size: usize = texts.values().map(String::len).sum();
    println!("search texts: {:?}, {} emails, {} KB", started.elapsed(), texts.len(), size / 1024);
    // What the mail search's has:otp, has:package-update and has:receipt
    // find, with and without the sign-in code model; code mail's subjects,
    // digits hidden.
    for with_model in [false, true] {
        let detector = with_model
            .then(|| crate::manage::local_ai::otp_detector(&store).ok())
            .flatten();
        if with_model && detector.is_none() {
            println!("mail kinds with the code model: not installed");
            continue;
        }
        let started = std::time::Instant::now();
        let kinds = store.mail_kinds(&accounts, None, detector.as_ref(), None).unwrap();
        println!(
            "mail kinds (code model {with_model}): {:?}, checked {}, codes {}, package updates {}, receipts {}",
            started.elapsed(),
            kinds.checked.len(),
            kinds.codes.len(),
            kinds.packages.len(),
            kinds.receipts.len()
        );
        if std::env::var("PICACHO_DIAG_CODES").is_ok() {
            let db = db_for(&dir);
            for id in kinds.codes.iter().take(60) {
                let subject: String = db
                    .query_row(
                        "SELECT coalesce(json_extract(data,'$.subject'),'') FROM messages WHERE id=?1",
                        [id],
                        |r| r.get(0),
                    )
                    .unwrap_or_default();
                let hidden: String = subject
                    .chars()
                    .take(70)
                    .map(|c| if c.is_ascii_digit() { '#' } else { c })
                    .collect();
                println!("   code: {hidden}");
            }
        }
    }
    let now = chrono::Utc::now().timestamp();
    let mut labels: std::collections::BTreeMap<&str, usize> = Default::default();
    for t in &scan.tracked {
        *labels.entry(t.status_label()).or_default() += 1;
    }
    println!("by status: {labels:?}, to download {}", scan.to_download.len());
    // Which parcels' messages a reading of headers and previews alone
    // would pass over.
    let db = db_for(&dir);
    let mut missed = 0;
    for t in &scan.tracked {
        let mut any = false;
        for e in &t.history {
            let (subject, snippet, from): (String, String, String) = db
                .query_row(
                    "SELECT coalesce(json_extract(data,'$.subject'),''), coalesce(json_extract(data,'$.snippet'),''), coalesce(json_extract(data,'$.from'),'') FROM messages WHERE id=?1",
                    [&e.sighting.mail_id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .unwrap();
            if super::worth_reading(&subject, &snippet, &from) {
                any = true;
            } else if std::env::var("PICACHO_DIAG_MISSED").is_ok() {
                println!("  passed over: {} | {}", e.sighting.sender, subject);
            }
        }
        missed += usize::from(!any);
    }
    println!("parcels with no message worth reading: {missed}");
    // Each parcel with its emails' senders and subjects only when asked
    // for: the rest prints counts alone, safe to share.
    let Ok(filter) = std::env::var("PICACHO_DIAG_LIST") else {
        return;
    };
    for t in &scan.tracked {
        if filter == "stuck" && t.done() { continue; }
        println!(
            "\n[{}] {} {:?} {:?} stage {} {} days ago",
            t.status_label(),
            t.parcel.title(),
            t.parcel.source,
            t.parcel.carrier,
            t.stage,
            (now - t.updated) / 86_400
        );
        for e in &t.history {
            println!(
                "   {:>16} {} | {} | {}",
                format!("{:?}", e.status),
                chrono::DateTime::from_timestamp(e.at, 0).unwrap().format("%Y-%m-%d"),
                e.sighting.sender.chars().take(30).collect::<String>(),
                e.sighting.subject.chars().take(90).collect::<String>()
            );
        }
    }
}

/// Prints the text of the newest message in a copy of a real cache whose
/// subject contains PICACHO_DIAG_SUBJECT, and what is found in it: the
/// output is that email, not to share.
#[test]
#[ignore]
fn parcels_show() {
    let (Some(dir), Ok(want)) = (
        std::env::var_os("PICACHO_DIAG_DIR"),
        std::env::var("PICACHO_DIAG_SUBJECT"),
    ) else {
        return;
    };
    let store = crate::manage::store::Store::open(std::path::Path::new(&dir)).unwrap();
    let id: String = db_for(&dir)
        .query_row(
            "SELECT id FROM messages WHERE json_extract(data,'$.subject') LIKE ?1 AND (raw IS NOT NULL OR source IS NOT NULL) ORDER BY date DESC LIMIT 1",
            [format!("%{want}%")],
            |r| r.get(0),
        )
        .unwrap();
    let m = store.message(&id).unwrap().unwrap();
    let raw = store.raw(&id).unwrap().unwrap();
    let a = crate::manage::model::Account::new(crate::manage::model::Provider::Imap);
    let parsed =
        crate::sync::mime::parse_message(&raw, &a, &m.id, m.folder, &m.remote_folder, m.uid)
            .unwrap();
    let text = parsed
        .body
        .clone()
        .unwrap_or_else(|| crate::sync::mime::html_to_text(parsed.html.as_deref().unwrap_or("")));
    println!("SUBJECT {}\n---\n{}\n---", m.subject, text.chars().take(2500).collect::<String>());
    println!(
        "{:#?}",
        find(&m.subject, &text, parsed.html.as_deref(), m.date, None)
    );
}

/// A sighting and a parcel for the gathering tests.
fn seen(id: &str, sent: i64) -> Sighting {
    Sighting {
        account: "a".into(),
        mail_id: id.into(),
        message_id: format!("<{id}@shop>"),
        mailbox: crate::manage::model::Folder::Inbox.into(),
        subject: format!("Message {id}"),
        sender: "Shop".into(),
        sent,
    }
}
fn told(number: &str, orders: &[&str], status: Status) -> Parcel {
    Parcel {
        tracking_number: number.into(),
        order: orders.first().map(|o| (*o).into()),
        carrier: None,
        service: None,
        tracking_url: None,
        expected_arrival: None,
        status: Some(status),
        updated: None,
        orders: orders.iter().map(|o| (*o).into()).collect(),
        contents: None,
        products: Vec::new(),
        source: if number.is_empty() {
            Source::Order
        } else {
            Source::Rule
        },
    }
}
fn ids(t: &Tracked) -> Vec<&str> {
    t.history.iter().map(|e| e.sighting.mail_id.as_str()).collect()
}
const DAY: i64 = 86_400;

#[test]
fn an_orders_shipments_are_told_apart_by_what_is_in_them() {
    let with = |mut p: Parcel, contents: &str| {
        p.contents = Some(contents.into());
        p
    };
    let tracked = summarize_at(
        vec![
            (told("", &["114-1"], Status::Processing), seen("ordered", 0)),
            (with(told("", &["114-1"], Status::Shipped), "1 luggage item"), seen("s1", DAY)),
            (with(told("", &["114-1"], Status::Shipped), "1 device accessories item"), seen("s2", DAY)),
            (with(told("", &["114-1"], Status::Delivered), "1 device accessories item"), seen("d2", 2 * DAY)),
            (with(told("", &["114-1"], Status::Delivered), "1 luggage item"), seen("d1", 3 * DAY)),
            // One shipment of two orders' items, named by the other order.
            (with(told("", &["114-2"], Status::Shipped), "2 items: video games, light bulbs"), seen("s3", DAY)),
            (with(told("", &["114-3", "114-2"], Status::Delivered), "2 items: video games, light bulbs"), seen("d3", 2 * DAY)),
        ],
        10 * DAY,
    );
    let mut got: Vec<Vec<&str>> = tracked.iter().map(ids).collect();
    got.sort();
    assert_eq!(
        got,
        [
            vec!["d1", "s1", "ordered"],
            vec!["d2", "s2", "ordered"],
            vec!["d3", "s3"],
        ]
    );
    assert!(tracked.iter().all(Tracked::delivered));
}

#[test]
fn a_shop_and_its_carrier_are_joined_by_product_or_by_dates() {
    let with = |mut p: Parcel, product: &str| {
        p.products = vec![product.into()];
        p
    };
    let tracked = summarize_at(
        vec![
            // The shop's "order shipped", and its package (under another
            // number on the way) delivered, sharing only a product cut at
            // different lengths.
            (with(told("", &["7301"], Status::Shipped), "hard shell travel case for mous"), seen("shop", 0)),
            (with(told("AP00123456789012", &[], Status::InTransit), "hard shell travel case for mouse m7/m"), seen("ap", DAY)),
            (with(told("SPXABC012345678901", &[], Status::Delivered), "hard shell travel case for mouse m7/m"), seen("spx", 6 * DAY)),
            // Another order of the same thing, both only shipped: two.
            (with(told("", &["9999"], Status::Shipped), "hard shell travel case for mous"), seen("other", 40 * DAY)),
            (with(told("", &["9998"], Status::Shipped), "hard shell travel case for mous"), seen("third", 40 * DAY)),
            // A carrier's parcel without an order, and a shop's delivery of
            // an order without a number: the only ones that fit.
            (told("799531274483", &[], Status::InTransit), seen("fedex", 100 * DAY)),
            (told("", &["40517"], Status::Delivered), seen("p2r", 102 * DAY)),
        ],
        200 * DAY,
    );
    let mut got: Vec<Vec<&str>> = tracked.iter().map(ids).collect();
    got.sort();
    assert_eq!(
        got,
        [
            vec!["other"],
            vec!["p2r", "fedex"],
            vec!["spx", "ap", "shop"],
            vec!["third"],
        ]
    );
}

#[test]
fn an_order_of_several_parcels_tells_each_and_old_news_is_probably_delivered() {
    let mut expected = told("9261292700768711948021", &["US-1"], Status::InTransit);
    expected.expected_arrival = Some("1970-01-05".into());
    let tracked = summarize_at(
        vec![
            (expected, seen("usps1", 0)),
            (told("9261290983497923666238", &["US-1"], Status::InTransit), seen("usps2", 0)),
            (told("", &["US-1"], Status::Delivered), seen("shop", 5 * DAY)),
            (told("1Z879E930346834440", &[], Status::Shipped), seen("ups", 0)),
            (told("1Z410E7W0392751591", &[], Status::OutForDelivery), seen("today", 0)),
        ],
        20 * DAY,
    );
    let by = |n: &str| tracked.iter().find(|t| t.parcel.tracking_number == n).unwrap();
    // The shop's "delivered" for the order is each parcel's.
    assert!(by("9261292700768711948021").delivered());
    assert!(by("9261290983497923666238").delivered());
    // Out for delivery weeks ago: probably delivered; shipped with no day
    // given: not known.
    let today = by("1Z410E7W0392751591");
    assert!(today.assumed && today.done() && !today.delivered());
    assert_eq!(today.status_label(), "Probably delivered");
    assert!(!by("1Z879E930346834440").done());
}

#[test]
fn reads_what_a_shipment_holds_and_when_it_is_expected() {
    assert_eq!(
        super::words::contents("Shipped: \u{2066}1\u{2069} Garden item").as_deref(),
        Some("1 garden item")
    );
    assert_eq!(
        super::words::contents("Delivered 2 items: Kitchen, Books").as_deref(),
        Some("2 items: kitchen, books")
    );
    assert_eq!(super::words::contents("Your order has shipped"), None);
    assert_eq!(
        super::words::products(
            "Shipped: \"Acme 8 oz Glass Spray...\" and \u{2066}1\u{2069} more item",
            "Track order\nHard Shell Travel Case for Mous...\nx1\nYou may also like\nSome Other Product Name...",
        ),
        ["acme 8 oz glass spray", "hard shell travel case for mous"]
    );
    let sent = local(
        NaiveDate::from_ymd_opt(2025, 3, 5)
            .unwrap()
            .and_hms_opt(9, 0, 0)
            .unwrap(),
    )
    .unwrap();
    let expected = |subject: &str| super::dates::expected_date(subject, "", sent);
    assert_eq!(
        expected("USPS® Expected Delivery on Friday, March 7, 2025 arriving by 9:00pm").as_deref(),
        Some("2025-03-07")
    );
    assert_eq!(expected("Estimated delivery: Mar 10").as_deref(), Some("2025-03-10"));
    assert_eq!(
        expected("Your shipment is scheduled for delivery tomorrow").as_deref(),
        Some("2025-03-06")
    );
    assert_eq!(expected("Your package is arriving today").as_deref(), Some("2025-03-05"));
    assert_eq!(expected("Your order has shipped"), None);
}

#[test]
fn finds_shops_own_package_numbers_and_not_rewards_orders() {
    let found = find(
        "Package UUSC000012345678 has been delivered",
        "Your package UUSC000012345678 has been successfully delivered!",
        None,
        0,
        None,
    );
    assert_eq!(numbers(&found), ["UUSC000012345678"]);
    assert_eq!(found[0].status, Some(Status::Delivered));
    // A rewards "order number" for a credit "on the way" is no parcel.
    assert!(
        find(
            "Your $8.40 statement credit is on the way!",
            "Rewards order number 1QX-7K2D\nAccount credit $8.40",
            None,
            0,
            None
        )
        .is_empty()
    );
    assert_eq!(status_in("Your Returns Label Is Ready", ""), Some(Status::Returned));
    assert_eq!(
        status_in("Your order 507112233445 is about to ship", ""),
        Some(Status::Processing)
    );
}

#[test]
fn picked_up_after_ready_for_pickup_is_delivered() {
    let tracked = summarize_at(
        vec![
            (told("", &["BBY01-800000123456"], Status::PickupAvailable), seen("ready", 0)),
            (told("", &["BBY01-800000123456"], Status::Shipped), seen("picked", DAY)),
        ],
        2 * DAY,
    );
    assert_eq!(tracked.len(), 1);
    assert!(tracked[0].delivered());
}

#[test]
fn a_status_set_by_hand_holds_as_the_parcel_grows_and_can_be_taken_back() {
    let first = vec![(told("", &["A-1"], Status::Shipped), seen("o1", 0))];
    let mut tracked = summarize_at(first.clone(), DAY);
    let set = Override {
        ids: tracked[0].ids.clone(),
        status: Some(Status::Delivered),
        hidden: true,
        name: None,
        sender: None,
        set: DAY,
    };
    apply_overrides(&mut tracked, std::slice::from_ref(&set));
    assert!(tracked[0].delivered() && tracked[0].hidden);
    assert_eq!(tracked[0].status_label(), "Delivered");
    // Its tracking number arrives later: still found by its order.
    let mut grown = first;
    grown.push((told("1Z5R89390357567127", &["A-1"], Status::InTransit), seen("t1", 2 * DAY)));
    let mut tracked = summarize_at(grown, 3 * DAY);
    assert_eq!(tracked.len(), 1);
    apply_overrides(&mut tracked, std::slice::from_ref(&set));
    assert!(tracked[0].delivered() && tracked[0].hidden);
    // A trouble set by hand shows though it had been delivered.
    tracked[0].set_manual(Some(Status::Returned), false);
    assert_eq!(tracked[0].trouble, Some(Status::Returned));
    assert_eq!(tracked[0].status_label(), "Returned");
    // Taken back: what its emails say.
    apply_overrides(&mut tracked, &[]);
    assert_eq!(
        (tracked[0].stage, tracked[0].manual, tracked[0].hidden, tracked[0].trouble),
        (2, None, false, None)
    );
}

#[test]
fn a_delivery_whose_order_follows_a_heading_naming_the_order_is_found() {
    // A print shop's: "Order" as a heading, then "Order number: …" under it.
    let found = find(
        "Your Example Prints order has been delivered. Let us know what you think!",
        "Review your Example Prints Order\nOrder number: 507112233445\nby Mesa Print Works\n\
         We are happy to see that your order\narrived safe and sound.",
        None,
        0,
        None,
    );
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].order.as_deref(), Some("507112233445"));
    assert_eq!(found[0].status, Some(Status::Delivered));
    // It joins the parcel the order's other emails gave a number.
    let mut out_for_delivery = told("420950549261290336128704133837", &["507112233445"], Status::OutForDelivery);
    out_for_delivery.source = Source::Link;
    let tracked = summarize_at(
        vec![
            (out_for_delivery, seen("out", 0)),
            (found[0].clone(), seen("delivered", DAY / 3)),
        ],
        DAY,
    );
    assert_eq!(tracked.len(), 1);
    assert!(tracked[0].delivered());
    assert_eq!(ids(&tracked[0]), ["delivered", "out"]);
}

#[test]
fn the_same_product_twice_goes_to_the_parcel_expected_that_day() {
    let with = |mut p: Parcel, expected: Option<&str>| {
        p.products = vec!["usb-c charging cable 2 pack".into()];
        p.expected_arrival = expected.map(Into::into);
        p
    };
    let tracked = summarize_at(
        vec![
            // Two orders of one thing, the second expected first.
            (with(told("", &["A-1"], Status::Shipped), Some("1970-01-11")), seen("s1", DAY)),
            (with(told("", &["A-2"], Status::Shipped), Some("1970-01-06")), seen("s2", 2 * DAY)),
            // Deliveries naming no order: the first on the second's day.
            (with(told("", &[], Status::Delivered), None), seen("d-early", 5 * DAY)),
            (with(told("", &[], Status::Delivered), None), seen("d-late", 10 * DAY)),
        ],
        12 * DAY,
    );
    let mut got: Vec<Vec<&str>> = tracked.iter().map(ids).collect();
    got.sort();
    assert_eq!(got, [vec!["d-early", "s2"], vec!["d-late", "s1"]]);
}

#[test]
fn a_return_naming_one_shipment_of_an_order_is_about_that_one() {
    let with = |mut p: Parcel, contents: &str| {
        p.contents = Some(contents.into());
        p
    };
    let titled = |id: &str, sent: i64, subject: &str| {
        let mut s = seen(id, sent);
        s.subject = subject.into();
        s
    };
    let tracked = summarize_at(
        vec![
            (told("", &["112-8"], Status::Processing), titled("ordered", 0, "Ordered: \"Acme Z790 Board...\" and 4 more items")),
            (with(told("", &["112-8"], Status::Shipped), "acme z790 board pro wifi"), seen("s1", DAY)),
            (with(told("", &["112-8"], Status::Shipped), "acme nvme ssd 2tb"), seen("s2", DAY)),
            (
                told("", &["112-8"], Status::Returned),
                titled("return", 10 * DAY, "Your return of Acme Z790 Board Pro WiFi7..."),
            ),
        ],
        12 * DAY,
    );
    let mut got: Vec<Vec<&str>> = tracked.iter().map(ids).collect();
    got.sort();
    assert_eq!(got, [vec!["return", "s1", "ordered"], vec!["s2", "ordered"]]);
    let returned = tracked.iter().find(|t| ids(t).contains(&"return")).unwrap();
    assert!(returned.finished() && !returned.done());
}

#[test]
fn a_parcel_on_its_way_with_no_news_for_weeks_is_probably_delivered() {
    let tracked = summarize_at(
        vec![
            (told("9261290983497923666238", &[], Status::Shipped), seen("usps", 0)),
            (told("", &["K-1"], Status::Processing), seen("ordered", 0)),
        ],
        30 * DAY,
    );
    let by = |id: &str| tracked.iter().find(|t| ids(t) == [id]).unwrap();
    assert!(by("usps").assumed);
    assert_eq!(by("usps").status_label(), "Probably delivered");
    // Not shipped yet: nothing is known of its delivery.
    assert!(!by("ordered").done());
}

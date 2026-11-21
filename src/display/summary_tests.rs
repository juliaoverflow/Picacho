use super::tests::{act, frame, two_accounts};
use super::*;
use std::prelude::v1::test;

/// A received message telling of a parcel.
fn shipping(account: &str, id: &str, date: i64, body: &str, html: Option<&str>) -> Mail {
    Mail {
        id: format!("{account}:{id}"),
        account: account.into(),
        folder: Folder::Inbox,
        remote_folder: "INBOX".into(),
        from: "shop@example.com".into(),
        sender: "Example Shop".into(),
        subject: format!("Your order has shipped ({id})"),
        snippet: body.chars().take(80).collect(),
        date,
        body: Some(body.into()),
        html: html.map(Into::into),
        message_id: format!("<{id}@{account}.example.com>"),
        ..Mail::default()
    }
}

fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

#[gpui::test]
fn the_summary_lists_packages_by_last_update_with_their_stage_and_history(cx: &mut TestAppContext) {
    let (view, cx, store) = two_accounts(cx);
    let (first, second) = view.read_with(cx, |this, _| {
        (this.accounts[0].id.clone(), this.accounts[1].id.clone())
    });
    let now = chrono::Utc::now().timestamp();
    let mut delivered = shipping(
        &first,
        "s10-delivered",
        now - 86_400,
        "Your parcel RB123456785GB was delivered to your mailbox.",
        None,
    );
    delivered.subject = "Delivered: your parcel".into();
    let mut confirmation = shipping(&first, "order", now, "Order 986578788855 received.", None);
    confirmation.subject = "Thanks for your order".into();
    let mut amazon_shipped = shipping(
        &second,
        "amazon",
        now - 3 * 86_400,
        "Order # 113-1234567-1234567. Your package is on the way.",
        Some(
            r#"<p>Order # 113-1234567-1234567</p><a href="https://www.amazon.com/progress-tracker/package?orderId=113-1234567-1234567">Track package</a>"#,
        ),
    );
    amazon_shipped.subject = "Shipped: \"Kettle\"".into();
    store
        .save_messages(&[
            // Sent first, but its markup says it was updated last.
            shipping(
                &first,
                "ups",
                now - 2 * 86_400 - 3600,
                "Your order is on its way.",
                Some(&format!(
                    r#"<script type="application/ld+json">{{"@type":"ParcelDelivery","trackingNumber":"1Z5R89390357567127","carrier":{{"name":"UPS"}},"deliveryStatus":{{"@type":"DeliveryEvent","name":"OrderInTransit","dateTime":"{}"}}}}</script><p>Your order is on its way.</p>"#,
                    chrono::DateTime::from_timestamp(now - 86_400 - 7200, 0)
                        .unwrap()
                        .to_rfc3339()
                )),
            ),
            shipping(
                &first,
                "s10",
                now - 2 * 86_400,
                "Good news! Tracking number: RB123456785GB",
                None,
            ),
            delivered,
            // Not a parcel: an order confirmation's order number.
            confirmation,
            amazon_shipped,
        ])
        .unwrap();
    frame(cx);
    assert!(
        cx.debug_bounds(leak(format!("nav-{first}-summary")))
            .is_some(),
        "Summary is under the account's Inbox"
    );
    act(&view, Action::OpenSummary(first.clone()), cx);
    frame(cx);
    view.read_with(cx, |this, _| {
        assert!(this.summary_open);
        assert!(this.selected.is_none());
        let listed: Vec<(String, &str, usize)> = this
            .summary
            .iter()
            .map(|t| (t.parcel.title(), t.status_label(), t.history.len()))
            .collect();
        assert_eq!(
            listed,
            [
                ("RB123456785GB".into(), "Delivered", 2),
                ("1Z5R89390357567127".into(), "In transit", 1),
            ]
        );
        assert_eq!(this.summary[0].stage, 4);
        assert_eq!(this.summary[1].stage, 2);
        assert_eq!(this.summary[1].updated, now - 86_400 - 7200);
    });
    assert!(cx.debug_bounds("summary-page").is_some());
    // The Summary shows a Packages widget with the first few, each with
    // its stage; View more… shows them all on a page of their own.
    assert!(cx.debug_bounds("summary-packages-widget").is_some());
    assert!(cx.debug_bounds("summary-widget-stepper-0").is_some());
    assert!(cx.debug_bounds("summary-widget-package-1").is_some());
    assert!(cx.debug_bounds("summary-package-0").is_none());
    // Downloading shipping mail is said within the widget, its Stop link
    // and words inside it however many there are.
    view.update(cx, |this, cx| {
        this.summary_downloads = Some((0, 55_100));
        cx.notify();
    });
    frame(cx);
    let widget = cx.debug_bounds("summary-packages-widget").unwrap();
    for part in ["summary-downloads-stop", "summary-downloads-text"] {
        let b = cx.debug_bounds(part).unwrap();
        assert!(
            b.origin.x >= widget.origin.x
                && b.origin.x + b.size.width <= widget.origin.x + widget.size.width,
            "{part} {b:?} runs out of the widget {widget:?}"
        );
    }
    view.update(cx, |this, cx| {
        this.summary_downloads = None;
        cx.notify();
    });
    frame(cx);
    let more = cx.debug_bounds("summary-view-more").unwrap().center();
    cx.simulate_click(more, Modifiers::none());
    frame(cx);
    assert!(cx.debug_bounds("summary-packages-page").is_some());
    assert!(cx.debug_bounds("summary-packages-widget").is_none());
    // Back returns to the widgets, and View more… comes back here.
    let back = cx.debug_bounds("summary-back").unwrap().center();
    cx.simulate_click(back, Modifiers::none());
    frame(cx);
    assert!(cx.debug_bounds("summary-packages-widget").is_some());
    act(&view, Action::SummaryPackages(true), cx);
    frame(cx);
    assert!(
        cx.debug_bounds("summary-stepper-0").is_some(),
        "each shows its stage"
    );
    // Clicking a parcel shows its whole history; again hides it.
    assert!(cx.debug_bounds("summary-history-0").is_none());
    let row = cx.debug_bounds("summary-package-0").unwrap().center();
    cx.simulate_click(row, Modifiers::none());
    frame(cx);
    assert!(cx.debug_bounds("summary-history-0").is_some());
    assert!(cx.debug_bounds("summary-stages-0").is_some());
    assert!(cx.debug_bounds("summary-event-0-0").is_some());
    assert!(cx.debug_bounds("summary-event-0-1").is_some());
    // Open Email on the older step opens that message in its mailbox.
    let open = cx.debug_bounds("summary-open-0-1").unwrap().center();
    cx.simulate_click(open, Modifiers::none());
    cx.run_until_parked();
    frame(cx);
    view.read_with(cx, |this, _| {
        assert!(!this.summary_open);
        assert_eq!(this.folder, Folder::Inbox);
        assert_eq!(
            this.selected.as_deref(),
            Some(format!("{first}:s10").as_str())
        );
    });
    // The reader shows the open message's tracking number.
    assert!(cx.debug_bounds("parcel-banner-0").is_some());

    // All Accounts' Summary has every account's parcels, the shop's
    // delivery known only by its order among them.
    assert!(
        cx.debug_bounds(leak(format!("nav-{}-summary", fwd::ALL_MAIL)))
            .is_some(),
        "Summary is under All Accounts' Inbox"
    );
    act(&view, Action::OpenSummary(fwd::ALL_MAIL.into()), cx);
    frame(cx);
    view.read_with(cx, |this, _| {
        let listed: Vec<(String, String)> = this
            .summary
            .iter()
            .map(|t| (t.parcel.title(), t.latest().account.clone()))
            .collect();
        assert_eq!(
            listed,
            [
                ("RB123456785GB".into(), first.clone()),
                ("1Z5R89390357567127".into(), first.clone()),
                ("Order 113-1234567-1234567".into(), second.clone()),
            ]
        );
        let order = &this.summary[2];
        assert_eq!(order.status_label(), "Shipped");
        assert!(
            order
                .parcel
                .tracking_url
                .as_deref()
                .is_some_and(|u| u.contains("progress-tracker"))
        );
    });
    // A status set by hand holds, and is saved; hiding tidies a parcel
    // away until it is asked for.
    let key = view.read_with(cx, |this, _| this.summary[1].parcel.key());
    act(
        &view,
        Action::SetParcelStatus(key.clone(), Some(crate::manage::parcels::Status::Delivered)),
        cx,
    );
    act(&view, Action::HideParcel(key.clone(), true), cx);
    // Looked at again from the cache, as after new mail.
    act(&view, Action::OpenSummary(fwd::ALL_MAIL.into()), cx);
    frame(cx);
    view.read_with(cx, |this, _| {
        let t = this.summary.iter().find(|t| t.parcel.key() == key).unwrap();
        assert_eq!(t.status_label(), "Delivered");
        assert!(t.delivered() && t.hidden);
        assert_eq!(this.listed_parcels(false).count(), 2);
    });
    assert!(cx.debug_bounds("summary-widget-package-2").is_none());
    act(&view, Action::SummaryPackages(true), cx);
    frame(cx);
    assert!(cx.debug_bounds("summary-show-hidden").is_some());
    act(&view, Action::ShowHiddenParcels(true), cx);
    frame(cx);
    assert!(cx.debug_bounds("summary-package-2").is_some());
    // Given back to its emails, and shown again.
    act(&view, Action::SetParcelStatus(key.clone(), None), cx);
    act(&view, Action::HideParcel(key.clone(), false), cx);
    view.read_with(cx, |this, _| {
        let t = this.summary.iter().find(|t| t.parcel.key() == key).unwrap();
        assert_eq!(t.status_label(), "In transit");
        assert!(t.manual.is_none() && !t.hidden);
        assert!(this.parcel_overrides().is_empty());
    });
    // The search button finds packages by any word of their number, order,
    // emails or sender, however written; Escape lists them all again.
    frame(cx);
    assert!(cx.debug_bounds("summary-search").is_none());
    let search = cx.debug_bounds("summary-search-button").unwrap().center();
    cx.simulate_click(search, Modifiers::none());
    frame(cx);
    assert!(cx.debug_bounds("summary-search").is_some());
    view.update_in(cx, |this, w, cx| MailApp::set(&this.parcel_search, "113-1234567", w, cx));
    frame(cx);
    assert!(cx.debug_bounds("summary-package-0").is_some());
    assert!(cx.debug_bounds("summary-package-1").is_none());
    view.update_in(cx, |this, w, cx| MailApp::set(&this.parcel_search, "rb123456785GB", w, cx));
    frame(cx);
    assert!(cx.debug_bounds("summary-package-0").is_some());
    assert!(cx.debug_bounds("summary-package-1").is_none());
    assert!(cx.debug_bounds("summary-in-email-0").is_none(), "its number matched");
    // A word only an email's text has: found once the emails are read,
    // and said to be.
    cx.run_until_parked();
    view.update_in(cx, |this, w, cx| MailApp::set(&this.parcel_search, "MAILBOX", w, cx));
    frame(cx);
    view.read_with(cx, |this, _| assert!(!this.parcel_texts_loading && !this.parcel_texts.is_empty()));
    assert!(cx.debug_bounds("summary-package-0").is_some());
    assert!(cx.debug_bounds("summary-package-1").is_none());
    assert!(cx.debug_bounds("summary-in-email-0").is_some());
    view.update_in(cx, |this, w, cx| MailApp::set(&this.parcel_search, "nothing like it", w, cx));
    frame(cx);
    assert!(cx.debug_bounds("summary-packages-empty").is_some());
    // Escape typed in the field itself.
    cx.simulate_keystrokes("escape");
    frame(cx);
    assert!(cx.debug_bounds("summary-search").is_none());
    assert!(cx.debug_bounds("summary-packages-page").is_some(), "Escape closes the search first");
    assert!(cx.debug_bounds("summary-package-2").is_some());
    view.read_with(cx, |this, cx| assert_eq!(MailApp::value(&this.parcel_search, cx), ""));
    // A name given by hand shows above the parcel's number; a sender given
    // by hand in place of its emails'. Enter saves, Escape keeps.
    act(&view, Action::EditParcel(key.clone(), ParcelField::Name), cx);
    frame(cx);
    assert!(cx.debug_bounds("summary-name-input-1").is_some());
    view.update_in(cx, |this, w, cx| {
        MailApp::set(&this.parcel_input, "  Graphics card ", w, cx);
        this.commit_parcel_edit(w, cx);
    });
    act(&view, Action::EditParcel(key.clone(), ParcelField::Sender), cx);
    frame(cx);
    view.read_with(cx, |this, cx| {
        assert_eq!(MailApp::value(&this.parcel_input, cx), "Example Shop");
    });
    view.update_in(cx, |this, w, cx| {
        MailApp::set(&this.parcel_input, "Newegg", w, cx);
        this.commit_parcel_edit(w, cx);
    });
    act(&view, Action::EditParcel(key.clone(), ParcelField::Name), cx);
    view.update_in(cx, |this, w, cx| {
        MailApp::set(&this.parcel_input, "Not saved", w, cx)
    });
    // Escape typed in the field itself.
    frame(cx);
    cx.simulate_keystrokes("escape");
    // Kept as new mail is looked at.
    act(&view, Action::OpenSummary(fwd::ALL_MAIL.into()), cx);
    act(&view, Action::SummaryPackages(true), cx);
    frame(cx);
    view.read_with(cx, |this, _| {
        assert!(this.parcel_editing.is_none());
        let t = this.summary.iter().find(|t| t.parcel.key() == key).unwrap();
        assert_eq!(t.name.as_deref(), Some("Graphics card"));
        assert_eq!(t.from(), "Newegg");
        assert_eq!(t.parcel.title(), "1Z5R89390357567127");
    });
    assert!(cx.debug_bounds("summary-name-1").is_some());
    // Emptied, it goes back to the emails'.
    act(&view, Action::EditParcel(key.clone(), ParcelField::Name), cx);
    view.update_in(cx, |this, w, cx| {
        MailApp::set(&this.parcel_input, "", w, cx);
        this.commit_parcel_edit(w, cx);
    });
    act(&view, Action::EditParcel(key.clone(), ParcelField::Sender), cx);
    view.update_in(cx, |this, w, cx| {
        MailApp::set(&this.parcel_input, "", w, cx);
        this.commit_parcel_edit(w, cx);
    });
    view.read_with(cx, |this, _| {
        let t = this.summary.iter().find(|t| t.parcel.key() == key).unwrap();
        assert!(t.name.is_none() && t.sender.is_none());
        assert!(this.parcel_overrides().is_empty());
    });
    // Right-clicking a parcel offers its statuses, its emails' checked.
    frame(cx);
    let at = cx.debug_bounds("summary-package-0").unwrap().center();
    cx.simulate_mouse_move(at, None, Modifiers::none());
    cx.simulate_event(MouseDownEvent {
        button: MouseButton::Right,
        position: at,
        modifiers: Modifiers::none(),
        click_count: 1,
        first_mouse: false,
    });
    cx.simulate_event(MouseUpEvent {
        button: MouseButton::Right,
        position: at,
        modifiers: Modifiers::none(),
        click_count: 1,
    });
    frame(cx);
    view.read_with(cx, |this, _| {
        let menu = this.menu.as_ref().expect("right-click offers statuses");
        let labels: Vec<(String, bool)> = menu
            .entries
            .iter()
            .filter_map(|e| match e {
                Entry::Item { label, checked, .. } => Some((label.to_string(), *checked)),
                Entry::Separator => None,
            })
            .collect();
        assert_eq!(labels[0], ("Automatic (from emails)".into(), true));
        assert!(labels.iter().any(|(l, _)| l == "Delivered"));
        assert!(labels.iter().any(|(l, _)| l == "Cancelled"));
        assert!(labels.iter().any(|(l, _)| l == "Rename Package…"));
        assert!(labels.iter().any(|(l, _)| l == "Edit Sender…"));
        assert_eq!(labels.last().unwrap().0, "Hide Package");
    });
    // Opening a mailbox leaves the Summary.
    act(
        &view,
        Action::OpenMailbox(first.clone(), Folder::Archive.into()),
        cx,
    );
    view.read_with(cx, |this, _| assert!(!this.summary_open));
    // The Inbox too, though the Summary opened over it.
    act(&view, Action::OpenSummary(first.clone()), cx);
    frame(cx);
    let inbox = cx.debug_bounds(leak(format!("nav-{first}-inbox"))).unwrap().center();
    cx.simulate_click(inbox, Modifiers::none());
    frame(cx);
    view.read_with(cx, |this, _| {
        assert!(!this.summary_open);
        assert_eq!(this.mailbox(), Folder::Inbox.into());
    });
    assert!(cx.debug_bounds("summary-page").is_none());
    // And All Accounts' row from its Summary.
    act(&view, Action::OpenSummary(fwd::ALL_MAIL.into()), cx);
    act(&view, Action::RefreshAccount(fwd::ALL_MAIL.into()), cx);
    view.read_with(cx, |this, _| {
        assert!(!this.summary_open);
        assert_eq!(this.account.as_deref(), Some(fwd::ALL_MAIL));
    });
}

/// The search's `has:otp`, `has:package-update` and `has:receipt` find mail
/// by what its text says, once it is looked at; before, by its subject and
/// preview.
#[gpui::test]
fn the_search_finds_code_mail_and_package_updates(cx: &mut TestAppContext) {
    let (view, cx, store) = two_accounts(cx);
    let first = view.read_with(cx, |this, _| this.accounts[0].id.clone());
    let now = chrono::Utc::now().timestamp();
    // The code and the tracking number are only in the text.
    let mut code = shipping(&first, "code", now, "Your verification code is 482913. It expires in 10 minutes.", None);
    code.subject = "Your verification code".into();
    code.from = "no-reply@accounts.example".into();
    code.snippet = "Hi there,".into();
    let mut parcel = shipping(
        &first,
        "parcel",
        now - 3600,
        "Good news! Your package 1Z5R89390357567127 was delivered today.",
        None,
    );
    // Its subject names an order, so its text is read; alone it doesn't
    // say where a package is.
    parcel.subject = "An update on your order".into();
    parcel.snippet = "Hello from Example Shop".into();
    // A receipt, and an ad that only reads like one ("$25 off your order").
    let mut receipt = shipping(&first, "receipt", now - 7200, "Order total: $54.20", None);
    receipt.subject = "Your receipt from Corner Café".into();
    let mut ad = shipping(&first, "ad", now - 9000, "Total: $5.00", None);
    ad.subject = "$25 off your order this weekend".into();
    store.save_messages(&[code, parcel, receipt, ad]).unwrap();
    // Listed afresh, with them.
    act(&view, Action::OpenMailbox(first.clone(), Folder::Archive.into()), cx);
    act(&view, Action::OpenMailbox(first.clone(), Folder::Inbox.into()), cx);
    cx.run_until_parked();
    frame(cx);
    let listed = |cx: &mut VisualTestContext| {
        view.read_with(cx, |this, _| {
            this.visible
                .iter()
                .map(|&i| this.messages[i].id.clone())
                .collect::<Vec<_>>()
        })
    };
    for (query, want) in [
        ("has:otp", format!("{first}:code")),
        ("has:package-update", format!("{first}:parcel")),
        ("has:receipt", format!("{first}:receipt")),
    ] {
        view.update_in(cx, |this, w, cx| MailApp::set(&this.search, query, w, cx));
        cx.run_until_parked();
        frame(cx);
        view.read_with(cx, |this, _| assert!(this.mail_kinds.is_some() && !this.mail_kinds_loading));
        assert_eq!(listed(cx), [want], "{query}");
    }
    // A receipt hidden in the Summary (taken for one wrongly) is no longer
    // found.
    act(&view, Action::OpenSummary(first.clone()), cx);
    cx.run_until_parked();
    let key = view.read_with(cx, |this, _| {
        let l = this.receipts.iter().find(|l| l.emails[0].mail_id == format!("{first}:receipt"));
        l.expect("the Summary lists the receipt").key()
    });
    act(&view, Action::HideReceipt(key.clone(), true), cx);
    act(&view, Action::OpenMailbox(first.clone(), Folder::Inbox.into()), cx);
    cx.run_until_parked();
    view.update_in(cx, |this, w, cx| MailApp::set(&this.search, "has:receipt", w, cx));
    cx.run_until_parked();
    frame(cx);
    assert!(listed(cx).is_empty(), "hidden receipts aren't found");
    act(&view, Action::OpenSummary(first.clone()), cx);
    cx.run_until_parked();
    act(&view, Action::HideReceipt(key, false), cx);
    act(&view, Action::OpenMailbox(first.clone(), Folder::Inbox.into()), cx);
    cx.run_until_parked();
    view.update_in(cx, |this, w, cx| MailApp::set(&this.search, "has:receipt", w, cx));
    cx.run_until_parked();
    frame(cx);
    assert_eq!(listed(cx), [format!("{first}:receipt")]);
    // Left out, with the rest listed.
    view.update_in(cx, |this, w, cx| MailApp::set(&this.search, "-has:otp", w, cx));
    frame(cx);
    let rest = listed(cx);
    assert!(!rest.contains(&format!("{first}:code")) && rest.contains(&format!("{first}:parcel")));
    // Offered among the search's hints.
    view.update_in(cx, |this, w, cx| MailApp::set(&this.search, "", w, cx));
    view.read_with(cx, |_, _| {
        assert!(crate::manage::search::OPERATORS.iter().any(|(op, _)| *op == "has:otp"));
        assert!(crate::manage::search::OPERATORS.iter().any(|(op, _)| *op == "has:receipt"));
    });
}

/// The Summary's Receipts widget lists the newest receipts with what was
/// spent this month; its page lists them all by month, each opening its
/// email, and one hidden stays hidden.
#[gpui::test]
fn the_summary_lists_receipts_with_what_was_spent(cx: &mut TestAppContext) {
    let (view, cx, store) = two_accounts(cx);
    let first = view.read_with(cx, |this, _| this.accounts[0].id.clone());
    let now = chrono::Utc::now().timestamp();
    let receipt = |id: &str, date: i64, subject: &str, sender: &str, body: &str| {
        let mut m = shipping(&first, id, date, body, None);
        m.subject = subject.into();
        m.sender = sender.into();
        m
    };
    store
        .save_messages(&[
            receipt("cafe", now - 60, "Your receipt from Corner Café", "Corner Café", "Latte\nTotal: $4.50"),
            receipt(
                "order",
                now - 120,
                "Thanks for your order #A-12345",
                "Gadget Store",
                "Order total: $120.00",
            ),
            // The same order's receipt, later: one purchase.
            receipt(
                "order-receipt",
                now - 90,
                "Receipt for order #A-12345",
                "Gadget Store",
                "Amount paid: $120.00",
            ),
            receipt("refund", now - 30, "Your refund from Corner Café", "Corner Café", "Refund total: $4.50"),
            receipt("news", now, "Weekly deals", "Gadget Store", "Total: $5.00"),
            receipt("due", now, "Your invoice is overdue", "Power Co", "Amount due: $50.00"),
        ])
        .unwrap();
    act(&view, Action::OpenSummary(first.clone()), cx);
    cx.run_until_parked();
    frame(cx);
    view.read_with(cx, |this, _| {
        let listed: Vec<(&str, Option<String>, usize)> = this
            .receipts
            .iter()
            .map(|l| {
                (
                    l.merchant.as_str(),
                    l.receipt.total.as_ref().map(|t| t.show()),
                    l.emails.len(),
                )
            })
            .collect();
        assert_eq!(
            listed,
            [
                ("Corner Café", Some("$4.50".into()), 1),
                ("Corner Café", Some("$4.50".into()), 1),
                ("Gadget Store", Some("$120.00".into()), 2),
            ]
        );
        assert_eq!(this.receipts[0].receipt.kind, crate::manage::receipts::Kind::Refund);
    });
    assert!(cx.debug_bounds("summary-receipts-widget").is_some());
    assert!(cx.debug_bounds("summary-widget-receipt-2").is_some());
    assert!(cx.debug_bounds("summary-receipts-count").is_some());
    // View more… shows every one by month; Back returns.
    let more = cx.debug_bounds("summary-receipts-more").unwrap().center();
    cx.simulate_click(more, Modifiers::none());
    frame(cx);
    assert!(cx.debug_bounds("summary-receipts-page").is_some());
    assert!(cx.debug_bounds("summary-receipts-widget").is_none());
    assert!(cx.debug_bounds("summary-receipts-month-0").is_some());
    assert!(cx.debug_bounds("summary-receipts-month-total-0").is_some());
    assert!(cx.debug_bounds("summary-receipt-2").is_some());
    let back = cx.debug_bounds("summary-back").unwrap().center();
    cx.simulate_click(back, Modifiers::none());
    frame(cx);
    assert!(cx.debug_bounds("summary-receipts-widget").is_some());
    // Hidden, it is left out, and stays so when looked at again.
    let key = view.read_with(cx, |this, _| this.receipts[1].key());
    act(&view, Action::HideReceipt(key.clone(), true), cx);
    act(&view, Action::OpenSummary(first.clone()), cx);
    cx.run_until_parked();
    act(&view, Action::SummaryReceipts(true), cx);
    frame(cx);
    view.read_with(cx, |this, _| assert_eq!(this.listed_receipts(false).count(), 2));
    assert!(cx.debug_bounds("summary-receipt-2").is_none());
    act(&view, Action::ShowHiddenReceipts(true), cx);
    frame(cx);
    assert!(cx.debug_bounds("summary-receipt-2").is_some());
    act(&view, Action::HideReceipt(key, false), cx);
    view.read_with(cx, |this, _| assert_eq!(this.listed_receipts(false).count(), 3));
    // The search finds receipts by who was paid, order or amount; by what
    // their emails say once read; Escape lists them all again.
    frame(cx);
    assert!(cx.debug_bounds("summary-receipts-search").is_none());
    let search = cx.debug_bounds("summary-receipts-search-button").unwrap().center();
    cx.simulate_click(search, Modifiers::none());
    frame(cx);
    assert!(cx.debug_bounds("summary-receipts-search").is_some());
    view.update_in(cx, |this, w, cx| MailApp::set(&this.receipt_search, "gadget", w, cx));
    frame(cx);
    assert!(cx.debug_bounds("summary-receipt-0").is_some());
    assert!(cx.debug_bounds("summary-receipt-1").is_none());
    view.update_in(cx, |this, w, cx| MailApp::set(&this.receipt_search, "A-12345", w, cx));
    frame(cx);
    assert!(cx.debug_bounds("summary-receipt-0").is_some());
    assert!(cx.debug_bounds("summary-receipt-1").is_none());
    view.update_in(cx, |this, w, cx| MailApp::set(&this.receipt_search, "4.50", w, cx));
    frame(cx);
    assert!(cx.debug_bounds("summary-receipt-1").is_some());
    assert!(cx.debug_bounds("summary-receipt-2").is_none());
    assert!(cx.debug_bounds("summary-receipt-in-email-0").is_none());
    cx.run_until_parked();
    view.update_in(cx, |this, w, cx| MailApp::set(&this.receipt_search, "latte", w, cx));
    frame(cx);
    view.read_with(cx, |this, _| assert!(!this.parcel_texts_loading && !this.parcel_texts.is_empty()));
    assert!(cx.debug_bounds("summary-receipt-0").is_some());
    assert!(cx.debug_bounds("summary-receipt-1").is_none());
    assert!(cx.debug_bounds("summary-receipt-in-email-0").is_some());
    view.update_in(cx, |this, w, cx| MailApp::set(&this.receipt_search, "nothing like it", w, cx));
    frame(cx);
    assert!(cx.debug_bounds("summary-receipts-empty").is_some());
    cx.simulate_keystrokes("escape");
    frame(cx);
    assert!(cx.debug_bounds("summary-receipts-search").is_none());
    assert!(cx.debug_bounds("summary-receipts-page").is_some(), "Escape closes the search first");
    assert!(cx.debug_bounds("summary-receipt-2").is_some());
    view.read_with(cx, |this, cx| assert_eq!(MailApp::value(&this.receipt_search, cx), ""));
    // Clicking one opens its email.
    frame(cx);
    let row = cx.debug_bounds("summary-receipt-0").unwrap().center();
    cx.simulate_click(row, Modifiers::none());
    cx.run_until_parked();
    frame(cx);
    view.read_with(cx, |this, _| {
        assert!(!this.summary_open);
        assert_eq!(this.selected.as_deref(), Some(format!("{first}:refund").as_str()));
    });
}

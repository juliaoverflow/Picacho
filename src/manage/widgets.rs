//! Home-screen widgets: builds a compact, privacy-aware snapshot of unread
//! counts, recent inbox messages, upcoming calendar events, and Picacho's
//! theme accent color for Android AppWidgets and iOS WidgetKit, and parses
//! `picacho://widget/...` deep links back into actions the window opens.
//!
//! Snapshots are written atomically to `widget-snapshot.json` inside the mail
//! store directory (and, on iOS, copied to the App Group container) so the
//! system's widget hosts can render immediately without opening SQLite or
//! spinning up the full Rust runtime. Redundant publishes with unchanged
//! content are skipped to conserve battery and respect WidgetKit's daily
//! reload budget.

use crate::manage::{
    local_ai::{self, otp},
    model::{Account, Folder, Mail},
    parcels::{self, Override as ParcelOverride},
    receipts::{self, Kind as ReceiptKind},
    store::Store,
};
use crate::sync::{caldav, ical};
use chrono::{Datelike, Local, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

/// File name written inside the data directory (and iOS App Group container).
pub const SNAPSHOT_FILE: &str = "widget-snapshot.json";
/// Current snapshot schema version.
/// - v1: initial Inbox / Unread / Calendar snapshot.
/// - v2: per-account Packages & Receipts (up to 6 per account), receipt
///   `updated` timestamps, and `account_label` metadata.
pub const SCHEMA_VERSION: u32 = 2;
/// Maximum inbox messages pulled per account so per-account widgets always
/// have plenty of rows even when another account is busy.
const PER_ACCOUNT: usize = 25;
/// Maximum total inbox messages kept in the snapshot.
const MAX_MESSAGES: usize = 40;
/// Maximum upcoming calendar occurrences kept in the snapshot.
const MAX_EVENTS: usize = 30;
/// Recent packages and receipts kept per account. Widgets list as many as
/// fit their size (a tall one on a large screen fits a few dozen) and link to
/// the Summary for the rest, so both "All Accounts" and a single account's
/// widget have plenty, while the snapshot stays small.
const PER_ACCOUNT_SUMMARY: usize = 50;
/// Cap on packages / receipts kept in all, across many accounts.
const MAX_PACKAGES: usize = 200;
const MAX_RECEIPTS: usize = 200;
/// Preference key holding manual parcel overrides (`display::summary::OVERRIDES`).
const PARCEL_OVERRIDES: &str = "parcel_statuses";
/// Calendar agenda lookahead window (days from start of today).
const AGENDA_DAYS: i64 = 14;
/// Maximum length of subject / snippet fields kept in a widget snapshot.
const MAX_SUBJECT_CHARS: usize = 160;
const MAX_SNIPPET_CHARS: usize = 200;

/// A snapshot of Picacho's state rendered by home-screen widgets.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    pub version: u32,
    pub generated_at: i64,
    pub accent_key: String,
    pub accent_light: u32,
    pub accent_dark: u32,
    pub accent_soft_light: u32,
    pub accent_soft_dark: u32,
    pub total_unread: usize,
    pub accounts: Vec<AccountSummary>,
    pub messages: Vec<WidgetMessage>,
    pub calendar_connected: bool,
    pub events: Vec<WidgetEvent>,
    #[serde(default)]
    pub packages: Vec<WidgetPackage>,
    #[serde(default)]
    pub receipts: Vec<WidgetReceipt>,
    /// The language Picacho shows (BCP 47): widgets write their own text in
    /// it, even where it isn't the system's.
    #[serde(default)]
    pub language: String,
    /// The widgets' own words in that language, for widget hosts without
    /// translations of their own (KDE Plasma's), by name (`labels`).
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
}

impl Snapshot {
    /// A snapshot with nothing in it yet, in the accent `accent_key`: what
    /// widgets show before Picacho has written one.
    #[cfg_attr(any(target_os = "android", target_os = "ios"), allow(dead_code))]
    pub fn empty(accent_key: &str, now: i64) -> Self {
        let (accent_key, accent_light, accent_dark, accent_soft_light, accent_soft_dark) =
            accent_palette(accent_key);
        Self {
            version: SCHEMA_VERSION,
            generated_at: now,
            accent_key: accent_key.to_owned(),
            accent_light,
            accent_dark,
            accent_soft_light,
            accent_soft_dark,
            total_unread: 0,
            accounts: Vec::new(),
            messages: Vec::new(),
            calendar_connected: false,
            events: Vec::new(),
            packages: Vec::new(),
            receipts: Vec::new(),
            language: String::new(),
            labels: BTreeMap::new(),
        }
        .worded()
    }

    /// With the language Picacho shows, and the widgets' words in it.
    fn worded(mut self) -> Self {
        self.language = crate::i18n::language().to_owned();
        self.labels = labels(&self);
        self
    }

    /// How many packages the Summary lists for one account, or for All
    /// Accounts when `None` (the "View all N" in widgets). Never fewer than
    /// the snapshot itself holds.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn package_total(&self, account_id: Option<&str>) -> usize {
        let counted = match account_id.filter(|id| !id.is_empty()) {
            Some(id) => self.accounts.iter().find(|a| a.id == id).map_or(0, |a| a.packages),
            None => self.accounts.iter().map(|a| a.packages).sum(),
        };
        counted.max(filtered_packages(self, account_id, usize::MAX).len())
    }

    /// How many receipts the Summary lists for one account, or for All
    /// Accounts when `None`.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn receipt_total(&self, account_id: Option<&str>) -> usize {
        let counted = match account_id.filter(|id| !id.is_empty()) {
            Some(id) => self.accounts.iter().find(|a| a.id == id).map_or(0, |a| a.receipts),
            None => self.accounts.iter().map(|a| a.receipts).sum(),
        };
        counted.max(filtered_receipts(self, account_id, usize::MAX).len())
    }
}

/// An account listed in widget configuration and unread summaries.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountSummary {
    pub id: String,
    pub name: String,
    pub email: String,
    pub unread: usize,
    /// Packages the Summary lists for this account (not hidden), of which
    /// the snapshot holds the most recent.
    #[serde(default)]
    pub packages: usize,
    /// Receipts the Summary lists for this account (not hidden).
    #[serde(default)]
    pub receipts: usize,
}

/// An inbox message row shown in the Inbox and Unread widgets.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WidgetMessage {
    pub id: String,
    pub account_id: String,
    pub account_label: String,
    pub sender: String,
    pub subject: String,
    pub snippet: String,
    pub date: i64,
    pub unread: bool,
    pub starred: bool,
    pub has_attachments: bool,
}

/// An upcoming calendar event occurrence shown in the Calendar widget.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WidgetEvent {
    pub uid: String,
    pub title: String,
    pub location: String,
    pub start: i64,
    pub end: i64,
    pub all_day: bool,
    pub calendar_name: String,
    pub calendar_color: String,
}

/// A recently updated package shown in the Packages widget.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WidgetPackage {
    pub key: String,
    pub title: String,
    pub number: String,
    pub carrier: String,
    pub status: String,
    /// Progress stage 0..=4 (Ordered, Shipped, In transit, Out for delivery, Delivered).
    pub stage: usize,
    pub delivered: bool,
    pub expected: String,
    pub updated: i64,
    pub account_id: String,
    #[serde(default)]
    pub account_label: String,
    pub mail_id: String,
}

/// A recent receipt shown in the Receipts widget.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WidgetReceipt {
    pub key: String,
    pub merchant: String,
    pub amount: String,
    pub refund: bool,
    pub order: String,
    /// First email date for this purchase/refund.
    pub date: i64,
    /// Most recently updated email timestamp for this receipt (used for sorting).
    #[serde(default)]
    pub updated: i64,
    pub account_id: String,
    #[serde(default)]
    pub account_label: String,
    pub mail_id: String,
}

/// What tapping a home-screen widget opens in Picacho.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WidgetAction {
    /// Open Inbox for one account, or All Accounts / the first account when `None`.
    Inbox { account: Option<String> },
    /// Open a specific message in its account.
    Message { account: String, id: String },
    /// Start composing a new message (optionally from a chosen account).
    Compose { account: Option<String> },
    /// Open the Calendar view.
    Calendar,
    /// Open Summary → Packages (optionally for a specific account).
    Packages { account: Option<String> },
    /// Open Summary → Receipts (optionally for a specific account).
    Receipts { account: Option<String> },
    /// Open or focus the built-in desktop widget window (macOS, Linux, Windows).
    Panel,
}

impl WidgetAction {
    /// Encodes the action as a `picacho://widget/...` deep link.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn to_uri(&self) -> String {
        let mut url = match self {
            Self::Inbox { .. } => url::Url::parse("picacho://widget/inbox").unwrap(),
            Self::Message { .. } => url::Url::parse("picacho://widget/message").unwrap(),
            Self::Compose { .. } => url::Url::parse("picacho://widget/compose").unwrap(),
            Self::Calendar => url::Url::parse("picacho://widget/calendar").unwrap(),
            Self::Packages { .. } => url::Url::parse("picacho://widget/packages").unwrap(),
            Self::Receipts { .. } => url::Url::parse("picacho://widget/receipts").unwrap(),
            Self::Panel => url::Url::parse("picacho://widget/panel").unwrap(),
        };
        match self {
            Self::Inbox { account: Some(acc) }
            | Self::Compose { account: Some(acc) }
            | Self::Packages { account: Some(acc) }
            | Self::Receipts { account: Some(acc) }
                if !acc.is_empty() =>
            {
                url.query_pairs_mut().append_pair("account", acc);
            }
            Self::Message { account, id } => {
                url.query_pairs_mut()
                    .append_pair("account", account)
                    .append_pair("id", id);
            }
            _ => {}
        }
        url.into()
    }

    /// Parses a `picacho://widget/...` deep link. Returns `None` for any other
    /// URL (including OAuth callbacks) or malformed widget targets.
    pub fn parse(input: &str) -> Option<Self> {
        if input.contains(['\n', '\r']) {
            return None;
        }
        let trimmed = input.trim();
        if trimmed.is_empty() || trimmed.len() > 4096 {
            return None;
        }
        let url = url::Url::parse(trimmed).ok()?;
        if !url.scheme().eq_ignore_ascii_case("picacho") {
            return None;
        }
        if !url.host_str().is_some_and(|h| h.eq_ignore_ascii_case("widget")) {
            return None;
        }
        let path = url.path().trim_matches('/');
        let param = |key: &str| -> Option<String> {
            url.query_pairs()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.into_owned())
                .filter(|v| !v.is_empty())
        };
        match path {
            "inbox" => Some(Self::Inbox {
                account: param("account"),
            }),
            "message" => Some(Self::Message {
                account: param("account")?,
                id: param("id")?,
            }),
            "compose" => Some(Self::Compose {
                account: param("account"),
            }),
            "calendar" => Some(Self::Calendar),
            "packages" => Some(Self::Packages {
                account: param("account"),
            }),
            "receipts" => Some(Self::Receipts {
                account: param("account"),
            }),
            "panel" => Some(Self::Panel),
            _ => None,
        }
    }
}

/// Picacho's accent palette (`display::ui::Accent`), mirrored here so widgets
/// match the user's chosen theme color in both light and dark modes without
/// depending on GPUI.
fn accent_palette(key: &str) -> (&'static str, u32, u32, u32, u32) {
    match key {
        "teal" => ("teal", 0x0b6b70, 0x7fd0d0, 0xd6ecec, 0x2b4647),
        "blue" => ("blue", 0x2459a8, 0x93b8f0, 0xdde7f6, 0x2e3a52),
        "purple" => ("purple", 0x6741a8, 0xbea5ef, 0xe8e0f5, 0x3e3557),
        "pink" => ("pink", 0xa8336b, 0xf0a3c6, 0xf5dfe9, 0x53334a),
        "orange" => ("orange", 0xa64a12, 0xf2b07f, 0xf6e3d6, 0x553c2c),
        "graphite" => ("graphite", 0x444c59, 0xc2c9d4, 0xe3e6eb, 0x3a404b),
        _ => ("green", 0x246846, 0x86cfaa, 0xdcebe2, 0x30463b),
    }
}

/// Builds a widget snapshot from `store` as of Unix timestamp `now`.
pub fn snapshot(store: &Store, now: i64) -> Snapshot {
    let accent_pref = store.preference("accent").unwrap_or_default();
    let (accent_key, accent_light, accent_dark, accent_soft_light, accent_soft_dark) =
        accent_palette(&accent_pref);

    let accounts = store.accounts().unwrap_or_default();
    let unread_map = store.unread_counts().unwrap_or_default();
    let inbox_key = Folder::Inbox.key();

    let mut total_unread = 0usize;
    let mut account_summaries = Vec::with_capacity(accounts.len());
    let mut raw_messages: Vec<(Mail, String)> = Vec::new();

    for a in &accounts {
        let unread = unread_map
            .get(&(a.id.clone(), inbox_key.to_owned()))
            .copied()
            .unwrap_or(0);
        total_unread = total_unread.saturating_add(unread);
        let label = account_label(a);
        account_summaries.push(AccountSummary {
            id: a.id.clone(),
            name: label.clone(),
            email: a.email.clone(),
            unread,
            packages: 0,
            receipts: 0,
        });
        if let Ok(list) = store.newest(&a.id, Folder::Inbox, PER_ACCOUNT) {
            for m in list {
                if !m.pending {
                    raw_messages.push((m, label.clone()));
                }
            }
        }
    }

    raw_messages.sort_by(|(a, _), (b, _)| b.date.cmp(&a.date).then_with(|| a.id.cmp(&b.id)));
    raw_messages.truncate(MAX_MESSAGES);

    let hide_codes = local_ai::otp_enabled(store);
    let detector = hide_codes.then(|| local_ai::otp_detector(store).ok());

    let messages = raw_messages
        .into_iter()
        .map(|(m, account_label)| {
            let (subject, snippet) = sanitize_preview(
                &m.subject,
                &m.snippet,
                hide_codes,
                detector.as_ref().and_then(|d| d.as_ref()),
            );
            let sender = if m.sender.trim().is_empty() {
                m.from.trim().to_owned()
            } else {
                m.sender.trim().to_owned()
            };
            let has_attachments = m.has_attachments();
            WidgetMessage {
                id: m.id,
                account_id: m.account,
                account_label,
                sender,
                subject: truncate_chars(&subject, MAX_SUBJECT_CHARS),
                snippet: truncate_chars(&snippet, MAX_SNIPPET_CHARS),
                date: m.date,
                unread: m.unread,
                starred: m.starred,
                has_attachments,
            }
        })
        .collect();

    let account_ids: Vec<String> = accounts.iter().map(|a| a.id.clone()).collect();
    let account_labels: HashMap<&str, &str> = account_summaries
        .iter()
        .map(|a| (a.id.as_str(), a.name.as_str()))
        .collect();
    let (calendar_connected, events) = calendar_agenda(store, now);
    let (packages, package_counts) = recent_packages(store, &account_ids, &account_labels);
    let (receipts, receipt_counts) = recent_receipts(store, &account_ids, &account_labels);
    for a in &mut account_summaries {
        a.packages = package_counts.get(&a.id).copied().unwrap_or(0);
        a.receipts = receipt_counts.get(&a.id).copied().unwrap_or(0);
    }

    let mut snap = Snapshot {
        version: SCHEMA_VERSION,
        generated_at: now,
        accent_key: accent_key.to_owned(),
        accent_light,
        accent_dark,
        accent_soft_light,
        accent_soft_dark,
        total_unread,
        accounts: account_summaries,
        messages,
        calendar_connected,
        events,
        packages,
        receipts,
        language: String::new(),
        labels: BTreeMap::new(),
    }
    .worded();
    if crate::manage::applock::hides_contents(store) {
        redact(&mut snap);
    }
    snap
}

/// The widgets' own words, in the language shown, for widget hosts with
/// no translations of their own (KDE Plasma's). `%1` and `%2` stand for
/// what the widget fills in. Words that depend on a number are given for
/// each number the widgets may show, as `name:number`.
fn labels(snap: &Snapshot) -> BTreeMap<String, String> {
    let mut labels: BTreeMap<String, String> = [
        ("inbox", t!("widgets-inbox").to_owned()),
        ("all-inboxes", t!("widgets-all-inboxes").to_owned()),
        ("all-accounts", t!("widgets-all-accounts").to_owned()),
        ("account", t!("widgets-account").to_owned()),
        (
            "account-name",
            t!("widgets-account-name", name = "%1", address = "%2"),
        ),
        (
            "show-unread-only",
            t!("widgets-show-unread-messages-only").to_owned(),
        ),
        ("title-unread", t!("widgets-title-unread", title = "%1")),
        ("refresh", t!("widgets-refresh").to_owned()),
        ("compose", t!("widgets-compose").to_owned()),
        ("unknown-sender", t!("widgets-unknown-sender").to_owned()),
        ("no-subject", t!("manage-widgets-no-subject").to_owned()),
        ("no-title", t!("manage-widgets-no-title").to_owned()),
        (
            "open-to-add-account",
            t!("widgets-open-picacho-to-add-an-account").to_owned(),
        ),
        ("all-caught-up", t!("widgets-all-caught-up").to_owned()),
        (
            "no-messages-in-inbox",
            t!("widgets-no-messages-in-inbox").to_owned(),
        ),
        ("today", t!("widgets-today").to_owned()),
        ("tomorrow", t!("widgets-tomorrow").to_owned()),
        ("all-day", t!("widgets-all-day").to_owned()),
        ("calendar", t!("widgets-calendar").to_owned()),
        (
            "no-upcoming-events",
            t!("widgets-no-upcoming-events").to_owned(),
        ),
        (
            "connect-calendar",
            t!("widgets-connect-a-calendar-in-settings").to_owned(),
        ),
        ("packages", t!("widgets-packages").to_owned()),
        ("packages-of", t!("widgets-packages-of", account = "%1")),
        ("package", t!("manage-widgets-package").to_owned()),
        ("expected", t!("widgets-expected", date = "%1")),
        (
            "no-recent-packages",
            t!("widgets-no-recent-packages").to_owned(),
        ),
        (
            "open-in-summary",
            t!("widgets-open-them-in-the-summary").to_owned(),
        ),
        ("receipts", t!("widgets-receipts").to_owned()),
        ("receipts-of", t!("widgets-receipts-of", account = "%1")),
        ("receipt", t!("widgets-receipt").to_owned()),
        ("order", t!("widgets-order", number = "%1")),
        ("refund", t!("manage-widgets-refund").to_owned()),
        (
            "no-recent-receipts",
            t!("widgets-no-recent-receipts").to_owned(),
        ),
    ]
    .into_iter()
    .map(|(name, text)| (name.to_owned(), text))
    .collect();
    let choices = snap
        .accounts
        .iter()
        .map(|a| Some(a.id.as_str()))
        .chain([None]);
    for account in choices {
        for count in [snap.package_total(account), snap.receipt_total(account)] {
            labels.insert(
                format!("view-all:{count}"),
                t!("widgets-view-all", count = count),
            );
        }
        let unread = match account {
            Some(id) => snap
                .accounts
                .iter()
                .find(|a| a.id == id)
                .map_or(0, |a| a.unread),
            None => snap.total_unread,
        };
        labels.insert(
            format!("unread:{unread}"),
            t!("widgets-unread", count = unread),
        );
    }
    labels
}

/// Settings → Privacy → Hide mail outside Picacho: the snapshot keeps its
/// rows (and counts, dates, accounts and what tapping opens), but not who
/// wrote, what about, events' names and places, or what was bought where.
/// The snapshot is a file outside the (perhaps encrypted) database that the
/// system's widget hosts read.
pub fn redact(snap: &mut Snapshot) {
    for m in &mut snap.messages {
        m.sender = m.account_label.clone();
        m.subject = t!("manage-widgets-details-hidden").to_owned();
        m.snippet.clear();
    }
    for e in &mut snap.events {
        e.title = t!("manage-widgets-event").to_owned();
        e.location.clear();
    }
    for p in &mut snap.packages {
        p.title = t!("manage-widgets-package").to_owned();
        p.number.clear();
    }
    for r in &mut snap.receipts {
        r.merchant = if r.refund {
            t!("manage-widgets-refund")
        } else {
            t!("manage-widgets-purchase")
        }
        .to_owned();
        r.amount.clear();
        r.order.clear();
    }
}

/// Recent packages (up to `PER_ACCOUNT_SUMMARY` per account, sorted newest-updated
/// first), respecting manual parcel overrides, Local AI confirmation, and hidden
/// flags saved by the Summary; and how many the Summary lists per account.
fn recent_packages(
    store: &Store,
    account_ids: &[String],
    account_labels: &HashMap<&str, &str>,
) -> (Vec<WidgetPackage>, HashMap<String, usize>) {
    if account_ids.is_empty() {
        return Default::default();
    }
    let model = local_ai::parcel_enabled(store)
        .then(|| local_ai::parcel_confirmer(store).ok())
        .flatten();
    let mut scan = match store.parcels(account_ids, model.as_ref()) {
        Ok(scan) => scan,
        Err(e) => {
            eprintln!("Picacho: could not scan packages for widgets: {e:#}");
            return Default::default();
        }
    };
    let overrides: Vec<ParcelOverride> = store
        .preference(PARCEL_OVERRIDES)
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_default();
    parcels::apply_overrides(&mut scan.tracked, &overrides);
    scan.tracked.sort_by(|a, b| {
        b.updated
            .cmp(&a.updated)
            .then_with(|| a.parcel.key().cmp(&b.parcel.key()))
    });

    let mut per_account: HashMap<String, usize> = HashMap::new();
    let mut out = Vec::new();
    for t in scan.tracked.into_iter().filter(|t| !t.hidden) {
        let latest = t.latest();
        let count = per_account.entry(latest.account.clone()).or_insert(0);
        *count += 1;
        if *count > PER_ACCOUNT_SUMMARY || out.len() >= MAX_PACKAGES {
            continue;
        }

        let p = &t.parcel;
        let delivered = t.done();
        let raw_title = p.title();
        let (title, number) = match &t.name {
            Some(name) if !name.trim().is_empty() => (name.trim().to_owned(), raw_title),
            _ => (raw_title, String::new()),
        };
        let carrier = t
            .sender
            .clone()
            .or_else(|| p.carrier.clone())
            .or_else(|| clean_sender_name(&latest.sender))
            .unwrap_or_default();
        let expected = if !delivered {
            p.expected_arrival
                .as_deref()
                .and_then(format_expected_date)
                .unwrap_or_default()
        } else {
            String::new()
        };
        let account_label = account_labels
            .get(latest.account.as_str())
            .copied()
            .unwrap_or_default()
            .to_owned();
        out.push(WidgetPackage {
            key: p.key(),
            title: truncate_chars(&collapse_whitespace(&title), MAX_SUBJECT_CHARS),
            number: truncate_chars(&collapse_whitespace(&number), 80),
            carrier: truncate_chars(&collapse_whitespace(&carrier), 80),
            status: t.status_label().to_owned(),
            stage: t.stage.min(4),
            delivered,
            expected,
            updated: t.updated,
            account_id: latest.account.clone(),
            account_label,
            mail_id: latest.mail_id.clone(),
        });
    }
    (out, per_account)
}

/// Recent receipts (up to `PER_ACCOUNT_SUMMARY` per account, sorted by most
/// recently updated email first), respecting receipts hidden in the Summary;
/// and how many the Summary lists per account.
fn recent_receipts(
    store: &Store,
    account_ids: &[String],
    account_labels: &HashMap<&str, &str>,
) -> (Vec<WidgetReceipt>, HashMap<String, usize>) {
    if account_ids.is_empty() {
        return Default::default();
    }
    let mut scan = match store.receipts(account_ids) {
        Ok(scan) => scan,
        Err(e) => {
            eprintln!("Picacho: could not scan receipts for widgets: {e:#}");
            return Default::default();
        }
    };
    let hidden: Vec<String> = store
        .preference(receipts::HIDDEN_PREFERENCE)
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_default();
    receipts::apply_hidden(&mut scan.listed, &hidden);

    let mut listed: Vec<(i64, receipts::Listed)> = scan
        .listed
        .into_iter()
        .filter(|l| !l.hidden && !l.emails.is_empty())
        .map(|l| {
            let updated = l.emails.first().map_or(l.date, |s| s.sent).max(l.date);
            (updated, l)
        })
        .collect();
    listed.sort_by(|(au, a), (bu, b)| {
        bu.cmp(au)
            .then_with(|| b.date.cmp(&a.date))
            .then_with(|| a.merchant.cmp(&b.merchant))
    });

    let mut per_account: HashMap<String, usize> = HashMap::new();
    let mut out = Vec::new();
    for (updated, l) in listed {
        let newest = &l.emails[0];
        let count = per_account.entry(newest.account.clone()).or_insert(0);
        *count += 1;
        if *count > PER_ACCOUNT_SUMMARY || out.len() >= MAX_RECEIPTS {
            continue;
        }

        let refund = l.receipt.kind == ReceiptKind::Refund;
        let amount = match &l.receipt.total {
            Some(total) if refund => format!("+{}", total.show()),
            Some(total) => total.show(),
            None => "—".to_owned(),
        };
        let account_label = account_labels
            .get(newest.account.as_str())
            .copied()
            .unwrap_or_default()
            .to_owned();
        out.push(WidgetReceipt {
            key: l.key(),
            merchant: truncate_chars(&collapse_whitespace(&l.merchant), 80),
            amount,
            refund,
            order: truncate_chars(
                &collapse_whitespace(l.receipt.order.as_deref().unwrap_or_default()),
                60,
            ),
            date: l.date,
            updated,
            account_id: newest.account.clone(),
            account_label,
            mail_id: newest.mail_id.clone(),
        });
    }
    (out, per_account)
}

fn clean_sender_name(sender: &str) -> Option<String> {
    let name = match sender.find('<') {
        Some(i) => sender[..i].trim(),
        None => sender.trim(),
    }
    .trim_matches('"')
    .trim();
    (!name.is_empty()).then(|| name.to_owned())
}

/// Formats an ISO-8601 expected arrival date (e.g. "2026-11-26..." -> "Nov 26",
/// in the language shown).
fn format_expected_date(raw: &str) -> Option<String> {
    let date_part = raw.trim().get(..10)?;
    let d = chrono::NaiveDate::parse_from_str(date_part, "%Y-%m-%d").ok()?;
    Some(crate::i18n::dates::format(&d, t!("dates-month-day")))
}

/// Filters `snapshot`'s messages by optional account and unread-only toggle.
#[cfg_attr(not(test), allow(dead_code))]
pub fn filtered_messages<'a>(
    snapshot: &'a Snapshot,
    account_id: Option<&str>,
    unread_only: bool,
    limit: usize,
) -> Vec<&'a WidgetMessage> {
    let account_filter = account_id.filter(|id| !id.is_empty());
    snapshot
        .messages
        .iter()
        .filter(|m| {
            account_filter.is_none_or(|id| m.account_id == id) && (!unread_only || m.unread)
        })
        .take(limit)
        .collect()
}

/// Filters `snapshot`'s packages by optional account (`None` / `""` for All Accounts).
#[cfg_attr(not(test), allow(dead_code))]
pub fn filtered_packages<'a>(
    snapshot: &'a Snapshot,
    account_id: Option<&str>,
    limit: usize,
) -> Vec<&'a WidgetPackage> {
    let account_filter = account_id.filter(|id| !id.is_empty());
    snapshot
        .packages
        .iter()
        .filter(|p| account_filter.is_none_or(|id| p.account_id == id))
        .take(limit)
        .collect()
}

/// Filters `snapshot`'s receipts by optional account (`None` / `""` for All Accounts).
#[cfg_attr(not(test), allow(dead_code))]
pub fn filtered_receipts<'a>(
    snapshot: &'a Snapshot,
    account_id: Option<&str>,
    limit: usize,
) -> Vec<&'a WidgetReceipt> {
    let account_filter = account_id.filter(|id| !id.is_empty());
    snapshot
        .receipts
        .iter()
        .filter(|r| account_filter.is_none_or(|id| r.account_id == id))
        .take(limit)
        .collect()
}

/// Reads an existing cached snapshot from `store.path` if it exists and matches
/// the current `SCHEMA_VERSION`.
#[cfg_attr(any(target_os = "android", target_os = "ios"), allow(dead_code))]
pub fn read_cached(dir: &Path) -> Option<Snapshot> {
    let bytes = std::fs::read(dir.join(SNAPSHOT_FILE)).ok()?;
    let snap: Snapshot = serde_json::from_slice(&bytes).ok()?;
    (snap.version >= SCHEMA_VERSION).then_some(snap)
}

/// Returns the unread count for one account, or across all accounts when `None`.
#[cfg_attr(not(test), allow(dead_code))]
pub fn unread_for(snapshot: &Snapshot, account_id: Option<&str>) -> usize {
    match account_id.filter(|id| !id.is_empty()) {
        Some(id) => snapshot
            .accounts
            .iter()
            .find(|a| a.id == id)
            .map_or(0, |a| a.unread),
        None => snapshot.total_unread,
    }
}

fn account_label(a: &Account) -> String {
    let label = a.label().trim();
    if label.is_empty() {
        a.email.clone()
    } else {
        label.to_owned()
    }
}

/// Masks sign-in codes in subject and snippet when Local AI's Hide sign-in
/// codes setting is enabled, matching `manage::background::Codes::hide`.
fn sanitize_preview(
    subject: &str,
    snippet: &str,
    hide_codes: bool,
    detector: Option<&otp::Detector>,
) -> (String, String) {
    let clean_subject = collapse_whitespace(subject);
    let clean_snippet = collapse_whitespace(snippet);
    if !hide_codes {
        return (clean_subject, clean_snippet);
    }
    match detector {
        Some(det) => {
            let sub = otp::mask(&clean_subject, &det.codes(&clean_subject, "")).into_owned();
            let snip = otp::mask(&clean_snippet, &det.codes("", &clean_snippet)).into_owned();
            (sub, snip)
        }
        // Without the model a code cannot be distinguished; stay safe rather than
        // leaking a sign-in code onto the home screen.
        None => (t!("manage-widgets-new-message").to_owned(), String::new()),
    }
}

fn collapse_whitespace(s: &str) -> String {
    let mut out = String::with_capacity(s.len().min(256));
    let mut last_space = true;
    for ch in s.chars() {
        if ch.is_whitespace() {
            if !last_space {
                out.push(' ');
                last_space = true;
            }
        } else {
            out.push(ch);
            last_space = false;
        }
    }
    if out.ends_with(' ') {
        out.pop();
    }
    out
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_owned();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Reads cached calendar events from `store` and expands upcoming occurrences
/// from the start of today (local time) through `AGENDA_DAYS` ahead.
fn calendar_agenda(store: &Store, now: i64) -> (bool, Vec<WidgetEvent>) {
    let server: Option<caldav::Server> = store
        .preference("calendar_server")
        .and_then(|j| serde_json::from_str(&j).ok())
        .filter(|s: &caldav::Server| !s.url.is_empty());
    let calendars: Vec<caldav::Calendar> = store
        .preference("calendars")
        .and_then(|j| serde_json::from_str(&j).ok())
        .unwrap_or_default();
    let stored: Vec<caldav::Stored> = store
        .preference("calendar_events")
        .and_then(|j| serde_json::from_str(&j).ok())
        .unwrap_or_default();

    let connected = server.is_some() || !calendars.is_empty() || !stored.is_empty();
    if stored.is_empty() {
        return (connected, Vec::new());
    }

    let cal_info: HashMap<&str, (&str, &str)> = calendars
        .iter()
        .map(|c| (c.href.as_str(), (c.name.as_str(), c.color.as_str())))
        .collect();

    // Start of local today (for all-day events) and UTC midnight of local today
    // (since all-day events store midnight UTC of their date).
    let local_now = Local
        .timestamp_opt(now, 0)
        .single()
        .unwrap_or_else(Local::now);
    let today = local_now.date_naive();
    let today_utc_start = Utc
        .with_ymd_and_hms(today.year(), today.month(), today.day(), 0, 0, 0)
        .single()
        .map_or(now - 86_400, |t| t.timestamp());
    let local_today_start = today
        .and_hms_opt(0, 0, 0)
        .and_then(|dt| Local.from_local_datetime(&dt).earliest())
        .map_or(now - 86_400, |t| t.timestamp());
    let from = local_today_start.min(today_utc_start);
    let to = from + AGENDA_DAYS * 86_400;

    let mut out = Vec::new();
    for item in &stored {
        let (cal_name, cal_color) = cal_info
            .get(item.calendar.as_str())
            .copied()
            .unwrap_or(("", ""));
        for occ in ical::occurrences(&item.event, &item.overrides, from, to) {
            if occ.cancelled {
                continue;
            }
            if occ.all_day {
                // Keep all-day events that cover today or a future day.
                let effective_end = if occ.end > occ.start {
                    occ.end
                } else {
                    occ.start + 86_400
                };
                if effective_end <= today_utc_start {
                    continue;
                }
            } else {
                // Keep timed events that haven't finished yet (or started right now).
                let effective_end = occ.end.max(occ.start + 60);
                if effective_end <= now {
                    continue;
                }
            }
            let title = collapse_whitespace(&occ.summary);
            out.push(WidgetEvent {
                uid: occ.uid,
                title: if title.is_empty() {
                    t!("manage-widgets-no-title").to_owned()
                } else {
                    truncate_chars(&title, MAX_SUBJECT_CHARS)
                },
                location: truncate_chars(&collapse_whitespace(&occ.location), MAX_SUBJECT_CHARS),
                start: occ.start,
                end: occ.end,
                all_day: occ.all_day,
                calendar_name: cal_name.to_owned(),
                calendar_color: cal_color.to_owned(),
            });
        }
    }

    out.sort_by(|a, b| {
        a.start
            .cmp(&b.start)
            .then_with(|| b.all_day.cmp(&a.all_day))
            .then_with(|| a.title.cmp(&b.title))
    });
    out.truncate(MAX_EVENTS);
    (connected, out)
}

/// Content digest used to suppress redundant writes and OS widget reloads
/// when nothing visible has changed. Includes the local calendar day so
/// midnight rollovers always refresh relative dates and today's agenda.
fn content_digest(snapshot: &Snapshot, local_day: i32) -> [u8; 32] {
    let mut normalized = snapshot.clone();
    normalized.generated_at = 0;
    let mut hasher = Sha256::new();
    hasher.update(local_day.to_le_bytes());
    if let Ok(bytes) = serde_json::to_vec(&normalized) {
        hasher.update(&bytes);
    }
    hasher.finalize().into()
}

/// Last published digest per store path, so unrelated test stores don't
/// interfere with each other and unchanged refreshes stay cheap.
static LAST_PUBLISHED: Mutex<Option<HashMap<PathBuf, [u8; 32]>>> = Mutex::new(None);
/// Live desktop widget windows subscribed to snapshot updates.
#[cfg(not(any(target_os = "android", target_os = "ios")))]
static SUBSCRIBERS: Mutex<Vec<futures::channel::mpsc::UnboundedSender<Snapshot>>> =
    Mutex::new(Vec::new());

/// Subscribes to newly published widget snapshots (used by the desktop widget
/// window to refresh live).
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub fn subscribe() -> futures::channel::mpsc::UnboundedReceiver<Snapshot> {
    let (tx, rx) = futures::channel::mpsc::unbounded();
    SUBSCRIBERS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(tx);
    rx
}

/// Formats `snap` as a Waybar / Polybar custom-module JSON line (`text`,
/// `alt`, `tooltip`, `class`, `percentage`) for Linux status bars.
#[cfg_attr(any(target_os = "android", target_os = "ios"), allow(dead_code))]
pub fn waybar_json(snap: &Snapshot) -> String {
    let text = if snap.total_unread == 0 {
        "0".to_owned()
    } else if snap.total_unread > 999 {
        "999+".to_owned()
    } else {
        snap.total_unread.to_string()
    };
    let mut lines = Vec::new();
    if snap.accounts.is_empty() {
        lines.push(t!("manage-widgets-picacho-open-to-sign-in").to_owned());
    } else if snap.total_unread == 0 {
        lines.push(t!("manage-widgets-picacho-all-caught-up").to_owned());
    } else {
        lines.push(t!(
            "manage-widgets-picacho-unread-messages",
            count = snap.total_unread
        ));
    }
    let unread_msgs: Vec<&WidgetMessage> = snap.messages.iter().filter(|m| m.unread).take(5).collect();
    let shown = if unread_msgs.is_empty() {
        snap.messages.iter().take(3).collect::<Vec<_>>()
    } else {
        unread_msgs
    };
    for m in shown {
        let sender = if m.sender.is_empty() {
            t!("manage-widgets-unknown")
        } else {
            &m.sender
        };
        let subject = if m.subject.is_empty() {
            t!("manage-widgets-no-subject")
        } else {
            &m.subject
        };
        let mark = if m.unread { "• " } else { "  " };
        lines.push(format!("{mark}{sender}: {subject}"));
    }
    if !snap.events.is_empty() {
        lines.push(String::new());
        lines.push(t!("manage-widgets-upcoming").to_owned());
        for ev in snap.events.iter().take(3) {
            lines.push(format!("• {}", ev.title));
        }
    }
    let class = if snap.total_unread > 0 { "unread" } else { "empty" };
    let payload = serde_json::json!({
        "text": text,
        "alt": class,
        "tooltip": lines.join("\n"),
        "class": class,
        "percentage": snap.total_unread.min(100),
    });
    payload.to_string()
}

/// Copies `widget-snapshot.json` into the macOS App Group container (for the
/// WidgetKit extension `PicachoWidgets.appex`) and reloads the widgets'
/// timelines. The group is the one `scripts/bundle.sh` names in the app's
/// Info.plist (`PicachoAppGroup`, "<TEAM ID>.tech.pinal.picacho"); a build
/// without it (ad-hoc signed, no widgets) has nothing to do here.
#[cfg(target_os = "macos")]
fn publish_macos(json: &[u8]) {
    use objc2_foundation::{NSBundle, NSFileManager, NSString};
    let Some(group_id) = NSBundle::mainBundle()
        .objectForInfoDictionaryKey(&NSString::from_str("PicachoAppGroup"))
        .and_then(|value| value.downcast::<NSString>().ok())
        .map(|value| value.to_string())
        .filter(|value| !value.trim().is_empty())
    else {
        return;
    };
    let container = NSFileManager::defaultManager()
        .containerURLForSecurityApplicationGroupIdentifier(&NSString::from_str(&group_id))
        .and_then(|url| url.path())
        .map(|p| PathBuf::from(p.to_string()));
    let Some(dir) = container else { return };
    if write_snapshot_file(&dir, json).is_err() {
        return;
    }
    reload_macos_widgets();
}

/// Asks WidgetKit to reload Picacho's widgets, through the small Swift
/// library bundled beside the app (`Contents/Frameworks`): WidgetKit is a
/// Swift API, and reloads the calling app's widgets, so it is called here,
/// in Picacho's process. Loaded once and kept.
#[cfg(target_os = "macos")]
fn reload_macos_widgets() {
    static RELOAD: std::sync::OnceLock<Option<(libloading::Library, unsafe extern "C" fn())>> =
        std::sync::OnceLock::new();
    let loaded = RELOAD.get_or_init(|| {
        let exe = std::env::current_exe().ok()?;
        let lib = exe
            .parent()?
            .parent()?
            .join("Frameworks/libpicacho_widget_reload.dylib");
        if !lib.is_file() {
            return None;
        }
        // SAFETY: Picacho's own signed library, whose one export takes no
        // arguments and returns nothing.
        unsafe {
            let lib = libloading::Library::new(&lib).ok()?;
            let reload = *lib.get::<unsafe extern "C" fn()>(b"picacho_widgets_reload\0").ok()?;
            Some((lib, reload))
        }
    });
    if let Some((_, reload)) = loaded {
        // SAFETY: see above.
        unsafe { reload() };
    }
}

/// Atomically writes `json` to `dir/widget-snapshot.json` (`0600` on Unix).
pub fn write_snapshot_file(dir: &Path, json: &[u8]) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let target = dir.join(SNAPSHOT_FILE);
    let partial = dir.join(format!("{SNAPSHOT_FILE}.partial"));
    {
        use std::io::Write as _;
        let mut opts = std::fs::OpenOptions::new();
        opts.create(true).write(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            opts.mode(0o600);
        }
        let mut file = opts.open(&partial)?;
        file.write_all(json)?;
        file.sync_all()?;
    }
    std::fs::rename(&partial, &target)?;
    Ok(target)
}

/// Builds and publishes a widget snapshot if its visible content changed
/// since the last publish for `store.path`. Returns `true` when published.
pub fn publish(store: &Store) -> bool {
    publish_inner(store, false)
}

/// Builds and publishes a widget snapshot unconditionally (used on explicit
/// refresh requests from a widget or on initial launch).
#[cfg_attr(target_os = "ios", allow(dead_code))]
pub fn publish_force(store: &Store) -> bool {
    publish_inner(store, true)
}

fn publish_inner(store: &Store, force: bool) -> bool {
    let now = Utc::now().timestamp();
    let snap = snapshot(store, now);
    let local_day = Local
        .timestamp_opt(now, 0)
        .single()
        .unwrap_or_else(Local::now)
        .num_days_from_ce();
    let digest = content_digest(&snap, local_day);

    if !force {
        let map = LAST_PUBLISHED.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(prev) = map.as_ref().and_then(|m| m.get(&store.path))
            && *prev == digest
        {
            return false;
        }
    }

    let Ok(json) = serde_json::to_string(&snap) else {
        return false;
    };
    if let Err(e) = write_snapshot_file(&store.path, json.as_bytes()) {
        eprintln!("Picacho: could not write widget snapshot: {e}");
        return false;
    }

    {
        let mut guard = LAST_PUBLISHED.lock().unwrap_or_else(|e| e.into_inner());
        guard
            .get_or_insert_with(HashMap::new)
            .insert(store.path.clone(), digest);
    }

    #[cfg(target_os = "android")]
    crate::android::publish_widgets(&json);

    #[cfg(target_os = "ios")]
    crate::ios::publish_widgets(&json);

    #[cfg(target_os = "macos")]
    publish_macos(json.as_bytes());

    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        let mut subs = SUBSCRIBERS.lock().unwrap_or_else(|e| e.into_inner());
        subs.retain(|tx| tx.unbounded_send(snap.clone()).is_ok());
    }

    true
}

/// How long `publish_soon` waits for more changes before building a snapshot:
/// a sync, a batch of moves or a Summary look ask many times in a row, and a
/// snapshot reads every account's packages, receipts and calendar.
const PUBLISH_DELAY: std::time::Duration = std::time::Duration::from_millis(1500);

/// Stores with a `publish_soon` waiting or running, and whether another
/// change came in meanwhile (it is published once more, after).
static SCHEDULED: Mutex<Option<HashMap<PathBuf, bool>>> = Mutex::new(None);

/// Publishes updated widget data in the background, soon: UI actions and
/// sync hooks never wait on it, and changes close together are published
/// once, by one thread per store.
pub fn publish_soon(store: Arc<Store>) {
    {
        let mut guard = SCHEDULED.lock().unwrap_or_else(|e| e.into_inner());
        let map = guard.get_or_insert_with(HashMap::new);
        if let Some(again) = map.get_mut(&store.path) {
            *again = true;
            return;
        }
        map.insert(store.path.clone(), false);
    }
    let path = store.path.clone();
    let spawned = std::thread::Builder::new()
        .name("picacho-widgets".into())
        .spawn(move || {
            loop {
                std::thread::sleep(PUBLISH_DELAY);
                publish(&store);
                let mut guard = SCHEDULED.lock().unwrap_or_else(|e| e.into_inner());
                let map = guard.get_or_insert_with(HashMap::new);
                match map.get_mut(&store.path) {
                    Some(again) if *again => *again = false,
                    _ => {
                        map.remove(&store.path);
                        break;
                    }
                }
            }
        });
    if spawned.is_err()
        && let Some(map) = SCHEDULED.lock().unwrap_or_else(|e| e.into_inner()).as_mut()
    {
        map.remove(&path);
    }
}

#[cfg(test)]
mod redact_tests {
    use super::*;

    #[test]
    fn hiding_mail_leaves_widgets_counts_but_not_contents() {
        let mut snap = Snapshot::empty("green", 0);
        snap.total_unread = 3;
        snap.messages.push(WidgetMessage {
            id: "1".into(),
            account_id: "a".into(),
            account_label: "Work".into(),
            sender: "Ana".into(),
            subject: "Salary review".into(),
            snippet: "Your new salary".into(),
            date: 5,
            unread: true,
            starred: false,
            has_attachments: false,
        });
        snap.events.push(WidgetEvent {
            uid: "e".into(),
            title: "Doctor".into(),
            location: "Clinic".into(),
            start: 1,
            end: 2,
            all_day: false,
            calendar_name: "Home".into(),
            calendar_color: String::new(),
        });
        redact(&mut snap);
        let json = serde_json::to_string(&snap).unwrap();
        for secret in ["Ana", "Salary", "salary", "Doctor", "Clinic"] {
            assert!(!json.contains(secret), "{secret} in {json}");
        }
        assert_eq!(snap.total_unread, 3);
        assert_eq!(snap.messages[0].sender, "Work");
        assert_eq!((snap.messages[0].date, snap.messages[0].unread), (5, true));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manage::model::{Provider, demo_mail};

    fn temp_store() -> (Arc<Store>, PathBuf) {
        let dir = std::env::temp_dir().join(format!("picacho-widgets-{}", uuid::Uuid::new_v4()));
        let store = Arc::new(Store::open(&dir).unwrap());
        (store, dir)
    }

    fn account(id: &str, email: &str, name: &str) -> Account {
        let mut a = Account::new(Provider::Imap);
        a.id = id.into();
        a.email = email.into();
        a.name = name.into();
        a
    }

    #[test]
    fn widget_action_uri_round_trips_and_rejects_non_widget_urls() {
        let cases = [
            WidgetAction::Inbox { account: None },
            WidgetAction::Inbox {
                account: Some("acc-1".into()),
            },
            WidgetAction::Message {
                account: "acc:work/1".into(),
                id: "acc:work/1:inbox:42 99".into(),
            },
            WidgetAction::Compose { account: None },
            WidgetAction::Compose {
                account: Some("acc-2".into()),
            },
            WidgetAction::Calendar,
            WidgetAction::Packages { account: None },
            WidgetAction::Packages {
                account: Some("acc-1".into()),
            },
            WidgetAction::Receipts { account: None },
            WidgetAction::Receipts {
                account: Some("acc-2".into()),
            },
            WidgetAction::Panel,
        ];
        for action in cases {
            let uri = action.to_uri();
            assert_eq!(WidgetAction::parse(&uri), Some(action), "uri={uri}");
        }
        // OAuth callbacks and unrelated URLs must never match as widget actions.
        assert_eq!(WidgetAction::parse("picacho://callback?code=123"), None);
        assert_eq!(WidgetAction::parse("picacho://widget/unknown"), None);
        assert_eq!(WidgetAction::parse("picacho://widget/message?account=a"), None);
        assert_eq!(WidgetAction::parse("mailto:ana@example.com"), None);
        assert_eq!(WidgetAction::parse("https://example.com/widget/inbox"), None);
        assert_eq!(WidgetAction::parse("picacho://widget/inbox\n"), None);
    }

    #[test]
    fn snapshot_summarizes_accounts_unread_counts_and_inbox_order() {
        let (store, dir) = temp_store();
        let work = account("work", "work@example.com", "Work");
        let home = account("home", "home@example.com", "Personal");
        store.save_account(&work).unwrap();
        store.save_account(&home).unwrap();
        store.set_preference("accent", "blue").unwrap();

        let template = demo_mail().remove(0);
        let m1 = Mail {
            id: "work:inbox:1".into(),
            account: "work".into(),
            folder: Folder::Inbox,
            sender: "Ana".into(),
            subject: "Quarterly   plan\nupdate".into(),
            snippet: "Attached   is\tthe deck".into(),
            date: 1_700_000_100,
            unread: true,
            starred: true,
            has_attachments: true,
            ..template.clone()
        };
        let m2 = Mail {
            id: "home:inbox:2".into(),
            account: "home".into(),
            folder: Folder::Inbox,
            sender: "Bo".into(),
            subject: "Dinner tonight".into(),
            snippet: "See you at 7".into(),
            date: 1_700_000_300,
            unread: false,
            starred: false,
            has_attachments: false,
            ..template.clone()
        };
        // Non-inbox messages must not appear in inbox widgets or unread counts.
        let sent = Mail {
            id: "work:sent:3".into(),
            account: "work".into(),
            folder: Folder::Sent,
            sender: "Me".into(),
            subject: "Re: plan".into(),
            date: 1_700_000_500,
            unread: true,
            ..template
        };
        store.save_messages(&[m1, m2, sent]).unwrap();

        let snap = snapshot(&store, 1_700_000_600);
        assert_eq!(snap.version, SCHEMA_VERSION);
        assert_eq!(snap.accent_key, "blue");
        assert_eq!(snap.accent_light, 0x2459a8);
        assert_eq!(snap.accent_dark, 0x93b8f0);
        assert_eq!(snap.total_unread, 1);
        assert_eq!(unread_for(&snap, None), 1);
        assert_eq!(unread_for(&snap, Some("work")), 1);
        assert_eq!(unread_for(&snap, Some("home")), 0);

        // Sorted newest first across accounts.
        assert_eq!(snap.messages.len(), 2);
        assert_eq!(snap.messages[0].id, "home:inbox:2");
        assert_eq!(snap.messages[1].id, "work:inbox:1");
        assert_eq!(snap.messages[1].subject, "Quarterly plan update");
        assert_eq!(snap.messages[1].snippet, "Attached is the deck");
        assert!(snap.messages[1].starred);
        assert!(snap.messages[1].has_attachments);

        let work_only = filtered_messages(&snap, Some("work"), false, 10);
        assert_eq!(work_only.len(), 1);
        assert_eq!(work_only[0].id, "work:inbox:1");

        let unread_only = filtered_messages(&snap, None, true, 10);
        assert_eq!(unread_only.len(), 1);
        assert_eq!(unread_only[0].id, "work:inbox:1");

        drop(store);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn snapshot_expands_upcoming_calendar_events_and_filters_past_or_cancelled() {
        let (store, dir) = temp_store();
        let now = Utc
            .with_ymd_and_hms(2026, 11, 24, 15, 0, 0)
            .unwrap()
            .timestamp();
        let cal = caldav::Calendar {
            href: "/cal/work/".into(),
            name: "Work".into(),
            writable: true,
            color: "#18B965".into(),
            ..Default::default()
        };
        store
            .set_preference("calendars", &serde_json::to_string(&vec![cal]).unwrap())
            .unwrap();

        let past = caldav::Stored {
            calendar: "/cal/work/".into(),
            event: ical::Event {
                uid: "past-1".into(),
                summary: "Morning standup".into(),
                start: now - 7200,
                end: now - 3600,
                utc: true,
                ..Default::default()
            },
            ..Default::default()
        };
        let upcoming = caldav::Stored {
            calendar: "/cal/work/".into(),
            event: ical::Event {
                uid: "repeat-1".into(),
                summary: "Design review".into(),
                location: "Room 4B".into(),
                start: now + 3600,
                end: now + 7200,
                recurrence: "FREQ=DAILY;COUNT=3".into(),
                utc: true,
                ..Default::default()
            },
            ..Default::default()
        };
        let cancelled = caldav::Stored {
            calendar: "/cal/work/".into(),
            event: ical::Event {
                uid: "cancelled-1".into(),
                summary: "Cancelled sync".into(),
                start: now + 1800,
                end: now + 3600,
                cancelled: true,
                utc: true,
                ..Default::default()
            },
            ..Default::default()
        };
        store
            .set_preference(
                "calendar_events",
                &serde_json::to_string(&vec![past, upcoming, cancelled]).unwrap(),
            )
            .unwrap();

        let snap = snapshot(&store, now);
        assert!(snap.calendar_connected);
        assert_eq!(snap.events.len(), 3);
        assert_eq!(snap.events[0].title, "Design review");
        assert_eq!(snap.events[0].location, "Room 4B");
        assert_eq!(snap.events[0].calendar_name, "Work");
        assert_eq!(snap.events[0].calendar_color, "#18B965");
        assert_eq!(snap.events[0].start, now + 3600);
        assert_eq!(snap.events[1].start, now + 3600 + 86_400);

        drop(store);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn publish_writes_snapshot_file_and_deduplicates_unchanged_publishes() {
        let (store, dir) = temp_store();
        assert!(publish(&store), "first publish writes snapshot");
        let file = dir.join(SNAPSHOT_FILE);
        assert!(file.is_file());
        let parsed: Snapshot =
            serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        assert_eq!(parsed.version, SCHEMA_VERSION);
        assert!(!publish(&store), "unchanged content skips redundant publish");
        assert!(publish_force(&store), "forced publish always writes");

        store.set_preference("accent", "purple").unwrap();
        assert!(publish(&store), "accent change triggers publish");
        let updated: Snapshot =
            serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        assert_eq!(updated.accent_key, "purple");

        drop(store);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn waybar_json_reports_unread_count_messages_and_upcoming_events() {
        let snap = Snapshot {
            version: SCHEMA_VERSION,
            generated_at: 1_700_000_000,
            accent_key: "green".into(),
            accent_light: 0x246846,
            accent_dark: 0x86cfaa,
            accent_soft_light: 0xdcebe2,
            accent_soft_dark: 0x30463b,
            total_unread: 2,
            accounts: vec![AccountSummary {
                id: "a1".into(),
                name: "Work".into(),
                email: "me@example.com".into(),
                unread: 2,
                packages: 0,
                receipts: 0,
            }],
            language: String::new(),
            labels: BTreeMap::new(),
            messages: vec![WidgetMessage {
                id: "m1".into(),
                account_id: "a1".into(),
                account_label: "Work".into(),
                sender: "Ana".into(),
                subject: "Launch checklist".into(),
                snippet: "Ready to ship".into(),
                date: 1_700_000_000,
                unread: true,
                starred: false,
                has_attachments: false,
            }],
            calendar_connected: true,
            events: vec![WidgetEvent {
                uid: "e1".into(),
                title: "Standup".into(),
                location: String::new(),
                start: 1_700_003_600,
                end: 1_700_005_400,
                all_day: false,
                calendar_name: "Work".into(),
                calendar_color: "#246846".into(),
            }],
            packages: Vec::new(),
            receipts: Vec::new(),
        };
        let raw = waybar_json(&snap);
        let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(parsed["text"], "2");
        assert_eq!(parsed["class"], "unread");
        let tip = parsed["tooltip"].as_str().unwrap();
        assert!(tip.contains("Ana: Launch checklist"));
        assert!(tip.contains("Standup"));
    }

    #[test]
    fn snapshot_includes_recent_packages_and_receipts_with_totals_respecting_hidden() {
        let (store, dir) = temp_store();
        let a = account("work", "me@example.com", "Work");
        store.save_account(&a).unwrap();

        let template = demo_mail().remove(0);
        let mut messages = Vec::new();
        // 8 shipping orders + 8 receipts: all kept (widgets show as many as
        // fit), newest first, with the Summary's totals.
        for i in 1..=8 {
            let mut ship = template.clone();
            ship.id = format!("work:inbox:ship-{i}");
            ship.message_id = format!("<ship-{i}@shop.example>");
            ship.account = "work".into();
            ship.folder = Folder::Inbox;
            ship.sender = format!("Shop {i} <orders@shop{i}.example>");
            ship.subject = format!("Your order #ORD-100{i} has shipped");
            ship.body = Some(format!("Your package for order #ORD-100{i} is on the way via UPS."));
            ship.date = 1_700_000_000 + i * 3600;
            messages.push(ship);

            let mut rcpt = template.clone();
            rcpt.id = format!("work:inbox:rcpt-{i}");
            rcpt.message_id = format!("<rcpt-{i}@store.example>");
            rcpt.account = "work".into();
            rcpt.folder = Folder::Inbox;
            rcpt.sender = format!("Coffee {i} <billing@coffee{i}.example>");
            rcpt.subject = if i == 8 {
                format!("Your refund from Coffee {i}")
            } else {
                format!("Your receipt from Coffee {i}")
            };
            rcpt.body = Some(format!("Order total: ${i}.50\nThank you for your visit."));
            rcpt.date = 1_700_000_000 + i * 3600;
            messages.push(rcpt);
        }
        store.save_messages(&messages).unwrap();

        let snap = snapshot(&store, 1_700_100_000);
        assert_eq!(snap.packages.len(), 8);
        assert_eq!(snap.receipts.len(), 8);
        assert_eq!((snap.accounts[0].packages, snap.accounts[0].receipts), (8, 8));
        assert_eq!(snap.package_total(None), 8);
        assert_eq!(snap.receipt_total(Some("work")), 8);
        // Newest first (8 down to 1).
        assert!(snap.packages[0].title.contains("ORD-1008"));
        assert_eq!(snap.receipts[0].merchant, "Coffee 8");
        assert!(snap.receipts[0].refund);
        assert_eq!(snap.receipts[0].amount, "+$8.50");
        assert_eq!(snap.receipts[1].merchant, "Coffee 7");
        assert!(!snap.receipts[1].refund);
        assert_eq!(snap.receipts[1].amount, "$7.50");

        // Hiding the newest receipt leaves it out, and out of the total.
        store
            .set_preference(
                receipts::HIDDEN_PREFERENCE,
                &serde_json::to_string(&vec![snap.receipts[0].key.clone()]).unwrap(),
            )
            .unwrap();
        let snap2 = snapshot(&store, 1_700_100_000);
        assert_eq!(snap2.receipts.len(), 7);
        assert_eq!(snap2.receipt_total(None), 7);
        assert_eq!(snap2.receipts[0].merchant, "Coffee 7");

        drop(store);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn packages_and_receipts_filter_by_account_and_sort_by_most_recently_updated() {
        let (store, dir) = temp_store();
        let work = account("work", "work@example.com", "Work");
        let home = account("home", "home@example.com", "Personal");
        store.save_account(&work).unwrap();
        store.save_account(&home).unwrap();

        let template = demo_mail().remove(0);
        let mut messages = Vec::new();
        // 7 recent items on Work + 7 older items on Personal.
        for i in 1..=7 {
            let mut wship = template.clone();
            wship.id = format!("work:inbox:ship-{i}");
            wship.message_id = format!("<wship-{i}@shop.example>");
            wship.account = "work".into();
            wship.folder = Folder::Inbox;
            wship.sender = format!("WorkShop {i} <orders@workshop{i}.example>");
            wship.subject = format!("Your order #W-100{i} has shipped");
            wship.body = Some(format!("Your package for order #W-100{i} is on the way via UPS."));
            wship.date = 1_700_100_000 + i * 3600;
            messages.push(wship);

            let mut hship = template.clone();
            hship.id = format!("home:inbox:ship-{i}");
            hship.message_id = format!("<hship-{i}@shop.example>");
            hship.account = "home".into();
            hship.folder = Folder::Inbox;
            hship.sender = format!("HomeShop {i} <orders@homeshop{i}.example>");
            hship.subject = format!("Your order #H-200{i} has shipped");
            hship.body = Some(format!("Your package for order #H-200{i} is on the way via FedEx."));
            hship.date = 1_700_000_000 + i * 3600;
            messages.push(hship);

            let mut wrcpt = template.clone();
            wrcpt.id = format!("work:inbox:rcpt-{i}");
            wrcpt.message_id = format!("<wrcpt-{i}@store.example>");
            wrcpt.account = "work".into();
            wrcpt.folder = Folder::Inbox;
            wrcpt.sender = format!("WorkCafe {i} <billing@workcafe{i}.example>");
            wrcpt.subject = format!("Your receipt from WorkCafe {i} (Order #WR-{i})");
            wrcpt.body = Some(format!("Order #WR-{i}\nOrder total: ${i}.00"));
            wrcpt.date = 1_700_100_000 + i * 3600;
            messages.push(wrcpt);

            let mut hrcpt = template.clone();
            hrcpt.id = format!("home:inbox:rcpt-{i}");
            hrcpt.message_id = format!("<hrcpt-{i}@store.example>");
            hrcpt.account = "home".into();
            hrcpt.folder = Folder::Inbox;
            hrcpt.sender = format!("HomeCafe {i} <billing@homecafe{i}.example>");
            hrcpt.subject = format!("Your receipt from HomeCafe {i} (Order #HR-{i})");
            hrcpt.body = Some(format!("Order #HR-{i}\nOrder total: ${i}.25"));
            hrcpt.date = 1_700_000_000 + i * 3600;
            messages.push(hrcpt);
        }
        // A follow-up receipt email updating the oldest Personal receipt (HR-1)
        // to the newest timestamp overall: it must rise to #1 in both All Accounts
        // and Personal!
        let mut followup = template;
        followup.id = "home:inbox:rcpt-1-updated".into();
        followup.message_id = "<hrcpt-1-upd@store.example>".into();
        followup.account = "home".into();
        followup.folder = Folder::Inbox;
        followup.sender = "HomeCafe 1 <billing@homecafe1.example>".into();
        followup.subject = "Your receipt from HomeCafe 1 (Order #HR-1)".into();
        followup.body = Some("Order #HR-1\nUpdated Order total: $19.99".into());
        followup.date = 1_700_999_999;
        messages.push(followup);

        store.save_messages(&messages).unwrap();

        let snap = snapshot(&store, 1_701_000_000);
        // Every account's recent items are kept, so single-account widgets
        // fill up too (the follow-up's new total is a receipt of its own).
        assert_eq!(snap.packages.len(), 14);
        assert_eq!(snap.receipts.len(), 15);
        assert_eq!(snap.package_total(None), 14);
        assert_eq!(snap.package_total(Some("home")), 7);
        assert_eq!(snap.receipt_total(Some("work")), 7);

        // All Accounts packages -> 6 most recently updated (Work #7..#2).
        let all_pkgs = filtered_packages(&snap, None, 6);
        assert_eq!(all_pkgs.len(), 6);
        assert!(all_pkgs[0].title.contains("W-1007"));
        assert_eq!(all_pkgs[0].account_label, "Work");

        // Personal account packages -> Personal's 6 most recently updated (H-2007..H-2002).
        let home_pkgs = filtered_packages(&snap, Some("home"), 6);
        assert_eq!(home_pkgs.len(), 6);
        assert!(home_pkgs[0].title.contains("H-2007"));
        assert_eq!(home_pkgs[0].account_id, "home");
        assert_eq!(home_pkgs[0].account_label, "Personal");

        // All Accounts receipts -> HR-1 rose to #1 because its follow-up email is newest!
        let all_rcpts = filtered_receipts(&snap, None, 6);
        assert_eq!(all_rcpts.len(), 6);
        assert_eq!(all_rcpts[0].merchant, "HomeCafe 1");
        assert_eq!(all_rcpts[0].updated, 1_700_999_999);
        assert_eq!(all_rcpts[0].amount, "$19.99");
        assert_eq!(all_rcpts[1].merchant, "WorkCafe 7");

        // Work account receipts -> Work's 6 most recently updated (WorkCafe 7..2).
        let work_rcpts = filtered_receipts(&snap, Some("work"), 6);
        assert_eq!(work_rcpts.len(), 6);
        assert_eq!(work_rcpts[0].merchant, "WorkCafe 7");
        assert!(work_rcpts.iter().all(|r| r.account_id == "work"));

        // Stale v1 cached files must be ignored by read_cached.
        let v1 = serde_json::json!({
            "version": 1,
            "generated_at": 1,
            "accent_key": "green",
            "accent_light": 0,
            "accent_dark": 0,
            "accent_soft_light": 0,
            "accent_soft_dark": 0,
            "total_unread": 0,
            "accounts": [],
            "messages": [],
            "calendar_connected": false,
            "events": []
        });
        write_snapshot_file(&dir, v1.to_string().as_bytes()).unwrap();
        assert!(read_cached(&dir).is_none());

        assert!(publish_force(&store));
        let cached = read_cached(&dir).expect("v2 snapshot should be read");
        assert_eq!(cached.version, SCHEMA_VERSION);
        assert_eq!(cached.packages.len(), 14);

        drop(store);
        let _ = std::fs::remove_dir_all(dir);
    }
}

//! Inbox control: who gets through, and when Picacho speaks up.
//!
//! - **VIPs**: senders (an address, or `@domain` for a whole domain) whose
//!   mail is marked in the list, gathered in a VIPs view of the inbox, and
//!   announced whatever else the notification settings say.
//! - **The Screener**: with it on, mail from someone new (not in the
//!   address book, never written to, not one of your own addresses, not a
//!   VIP, not sorted into a tab by you, not approved before) waits in the
//!   Screener rather than in the inbox, and isn't announced. Approving the
//!   sender lets their mail (and what follows) into the inbox; blocking
//!   them sends it to Trash (`Organizing::blocked`); routing them to a
//!   category approves them and sorts them there (`Categorizing::senders`).
//!   Nothing is moved on the server: the Screener is a view of the inbox,
//!   so turning it off brings everything back. Only mail that arrived after
//!   it was turned on (`Screener::since`) is screened.
//! - **Notifications**: everything, only Primary's and VIPs', or only
//!   VIPs'; categories kept silent; quiet hours (scheduled Do Not Disturb
//!   windows, and a pause until a given time) that VIPs may break through.
//!
//! Every place new mail is announced (the window, the background monitor,
//! Proton's pushes) asks `InboxControl::announces`. Saved as one Store
//! preference.
use super::categories::{Categorizing, Category, Known};
use super::model::{Folder, Mail};
use super::store::Store;
use chrono::{DateTime, Datelike, Local, TimeZone, Timelike};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const PREFERENCE: &str = "inbox-control";

/// Whether `address` is `entry`: the same address, or within `entry`'s
/// `@domain` (or a subdomain of it). Both lowercase.
fn covers(entry: &str, address: &str) -> bool {
    if entry.is_empty() || address.is_empty() {
        return false;
    }
    match entry.strip_prefix('@') {
        Some(domain) => address
            .rsplit_once('@')
            .is_some_and(|(_, d)| d == domain || d.ends_with(&format!(".{domain}"))),
        None => entry == address,
    }
}

/// An address or `@domain` as kept: trimmed and lowercase; `None` when it
/// is neither.
pub fn normalized(text: &str) -> Option<String> {
    let text = text
        .trim()
        .trim_matches(|c| c == '<' || c == '>')
        .to_lowercase();
    let ok = match text.strip_prefix('@') {
        Some(domain) => {
            domain.contains('.') && !domain.contains('@') && !domain.contains(char::is_whitespace)
        }
        None => {
            text.split_once('@')
                .is_some_and(|(l, d)| !l.is_empty() && d.contains('.'))
                && !text.contains(char::is_whitespace)
        }
    };
    ok.then_some(text)
}

/// First-time senders' mail held back for a decision.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Screener {
    pub enabled: bool,
    /// When it was turned on (Unix seconds): older mail isn't screened.
    pub since: i64,
    /// Senders let in: addresses, or `@domain`.
    pub approved: BTreeSet<String>,
}

/// Which new mail is announced, outside quiet hours.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotifyMode {
    /// All of it, but the categories kept silent.
    #[default]
    Everything,
    /// Mail sorted into Primary, and VIPs'.
    PrimaryAndVips,
    /// Only VIPs'.
    VipsOnly,
}
impl NotifyMode {
    pub const ALL: [Self; 3] = [Self::Everything, Self::PrimaryAndVips, Self::VipsOnly];
    pub fn label(self) -> &'static str {
        match self {
            Self::Everything => t!("inbox-control-notify-everything"),
            Self::PrimaryAndVips => t!("inbox-control-notify-primary-and-vips"),
            Self::VipsOnly => t!("inbox-control-notify-vips-only"),
        }
    }
}

/// A scheduled quiet time: from `start` to `end` (minutes after midnight)
/// on each of `days`. One ending earlier than it starts runs past
/// midnight, into the next day; one ending as it starts lasts all day.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuietWindow {
    /// The days it starts on: bit 0 for Monday … bit 6 for Sunday.
    pub days: u8,
    pub start: u16,
    pub end: u16,
}
impl Default for QuietWindow {
    /// Nights: 22:00 to 07:00, every day.
    fn default() -> Self {
        Self {
            days: EVERY_DAY,
            start: 22 * 60,
            end: 7 * 60,
        }
    }
}
/// Every day of the week, as `QuietWindow::days`.
pub const EVERY_DAY: u8 = 0b111_1111;
/// Saturday and Sunday.
pub const WEEKEND: u8 = 0b110_0000;
/// Monday to Friday.
pub const WEEKDAYS: u8 = 0b001_1111;

impl QuietWindow {
    /// Whether `day` (0 = Monday) is one it starts on.
    pub fn on(&self, day: u32) -> bool {
        day < 7 && self.days & (1 << day) != 0
    }
    /// Whether it covers `minute` (after midnight) of `day` (0 = Monday).
    pub fn covers(&self, day: u32, minute: u16) -> bool {
        let yesterday = (day + 6) % 7;
        let (start, end) = (self.start.min(MINUTES - 1), self.end.min(MINUTES));
        if start == end {
            return self.on(day);
        }
        if start < end {
            self.on(day) && minute >= start && minute < end
        } else {
            (self.on(day) && minute >= start) || (self.on(yesterday) && minute < end)
        }
    }
    /// `minutes` after midnight as `HH:MM`.
    pub fn clock(minutes: u16) -> String {
        let m = minutes.min(MINUTES);
        format!("{:02}:{:02}", m / 60 % 24, m % 60)
    }
    /// A time typed as `22:30`, `7`, `0730` or `7:30pm`, in minutes after
    /// midnight.
    pub fn parse_clock(text: &str) -> Option<u16> {
        let text = text.trim().to_ascii_lowercase();
        let (text, pm, am) = match (text.strip_suffix("pm"), text.strip_suffix("am")) {
            (Some(t), _) => (t.trim().to_owned(), true, false),
            (_, Some(t)) => (t.trim().to_owned(), false, true),
            _ => (text, false, false),
        };
        let (h, m): (u16, u16) = match text.split_once([':', '.', 'h']) {
            Some((h, m)) => (
                h.trim().parse().ok()?,
                if m.trim().is_empty() {
                    0
                } else {
                    m.trim().parse().ok()?
                },
            ),
            None if text.len() >= 3 && text.bytes().all(|b| b.is_ascii_digit()) => {
                let (h, m) = text.split_at(text.len() - 2);
                (h.parse().ok()?, m.parse().ok()?)
            }
            None => (text.parse().ok()?, 0),
        };
        if m >= 60 {
            return None;
        }
        let h = match (pm, am) {
            _ if (pm || am) && !(1..=12).contains(&h) => return None,
            (true, _) => h % 12 + 12,
            (_, true) => h % 12,
            _ if h > 24 || (h == 24 && m > 0) => return None,
            _ => h,
        };
        Some(h * 60 + m)
    }
}
const MINUTES: u16 = 24 * 60;

/// Scheduled Do Not Disturb.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct QuietHours {
    /// The schedule applies.
    pub enabled: bool,
    pub windows: Vec<QuietWindow>,
    /// Mail from VIPs is announced even during quiet hours.
    pub vips_break_through: bool,
    /// Nothing is announced until then (Unix seconds), whatever the
    /// schedule: "Pause notifications for an hour". 0 for no pause.
    pub paused_until: i64,
}
impl Default for QuietHours {
    fn default() -> Self {
        Self {
            enabled: false,
            windows: vec![QuietWindow::default()],
            vips_break_through: true,
            paused_until: 0,
        }
    }
}
impl QuietHours {
    /// Whether it is quiet at `now`.
    pub fn quiet_at<Tz: TimeZone>(&self, now: &DateTime<Tz>) -> bool {
        if self.paused_until > now.timestamp() {
            return true;
        }
        if !self.enabled {
            return false;
        }
        let day = now.weekday().num_days_from_monday();
        let minute = (now.hour() * 60 + now.minute()) as u16;
        self.windows.iter().any(|w| w.covers(day, minute))
    }
    /// Whether notifications are paused now, by hand.
    pub fn paused(&self, now: i64) -> bool {
        self.paused_until > now
    }
}

/// How new mail is announced.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Notifications {
    pub mode: NotifyMode,
    /// Categories whose mail is never announced (with `Everything`), but
    /// VIPs'.
    pub silent: BTreeSet<Category>,
    pub quiet: QuietHours,
}

/// VIPs, the Screener and notification policy, kept together.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct InboxControl {
    /// VIP senders: lowercase addresses, or `@domain`.
    pub vips: Vec<String>,
    pub screener: Screener,
    pub notifications: Notifications,
}

impl InboxControl {
    /// As saved; the first time, with the inbox categories' older "notify
    /// only for Primary" carried over.
    pub fn load(store: &Store) -> Self {
        match store.preference(PREFERENCE) {
            Some(json) => serde_json::from_str(&json).unwrap_or_default(),
            None => Self::from_categories(&Categorizing::load(store)),
        }
    }
    /// The settings before any were saved: as the categories' older
    /// switch had them.
    pub fn from_categories(categories: &Categorizing) -> Self {
        let mut control = Self::default();
        if categories.notify_primary_only {
            control.notifications.mode = NotifyMode::PrimaryAndVips;
        }
        control
    }
    pub fn save(&self, store: &Store) {
        if let Ok(json) = serde_json::to_string(self) {
            store.set_preference_soon(PREFERENCE, &json);
        }
    }

    // --- VIPs ---

    /// Whether mail from `address` is a VIP's.
    pub fn is_vip(&self, address: &str) -> bool {
        if self.vips.is_empty() {
            return false;
        }
        let address = address.trim().to_lowercase();
        self.vips.iter().any(|v| covers(v, &address))
    }
    /// Makes `address` (or `@domain`) a VIP, or no longer one when it is.
    /// Returns whether it is one now; `None` for text that is neither.
    pub fn toggle_vip(&mut self, address: &str) -> Option<bool> {
        let address = normalized(address)?;
        let before = self.vips.len();
        self.vips.retain(|v| *v != address);
        if self.vips.len() < before {
            return Some(false);
        }
        self.vips.push(address.clone());
        // A VIP needs no screening.
        self.screener.approved.insert(address);
        Some(true)
    }

    // --- The Screener ---

    /// Turns the Screener on (from now) or off.
    pub fn set_screening(&mut self, on: bool, now: i64) {
        if on && !self.screener.enabled {
            self.screener.since = now;
        }
        self.screener.enabled = on;
    }
    /// Whether `address` was approved, by itself or by its domain.
    pub fn approved(&self, address: &str) -> bool {
        let address = address.trim().to_lowercase();
        self.screener.approved.iter().any(|a| covers(a, &address))
    }
    /// Lets mail from `address` (or `@domain`) into the inbox.
    pub fn approve(&mut self, address: &str) -> bool {
        // A sender as the mail names it, even one `normalized` wouldn't
        // take (`root@localhost`): it must be let in all the same.
        let address = normalized(address).unwrap_or_else(|| address.trim().to_lowercase());
        !address.is_empty() && self.screener.approved.insert(address)
    }
    /// Forgets that `address` was approved: their next mail is screened.
    pub fn forget_approval(&mut self, address: &str) {
        let address = address.trim().to_lowercase();
        self.screener.approved.remove(&address);
    }
    /// Whether `m` waits in the Screener: from someone new, arrived in the
    /// inbox since the Screener was turned on. `known` must be loaded
    /// (with an empty one, everyone is new).
    pub fn screens(&self, m: &Mail, known: &Known, sorted: &Categorizing) -> bool {
        if !self.screener.enabled
            || m.folder != Folder::Inbox
            || m.date < self.screener.since
            || m.answered
        {
            return false;
        }
        let from = m.from.trim().to_lowercase();
        if from.is_empty() || known.own.contains(&from) || known.people.contains(&from) {
            return false;
        }
        !self.is_vip(&from) && !self.approved(&from) && sorted.sender_choice(&from).is_none()
    }

    // --- Notifications ---

    /// Whether new mail `m` is announced at `now`. `category` is the tab it
    /// shows under (asked only when it matters), `screened` whether it
    /// waits in the Screener.
    pub fn announces<Tz: TimeZone>(
        &self,
        m: &Mail,
        screened: bool,
        category: impl FnOnce() -> Category,
        now: &DateTime<Tz>,
    ) -> bool {
        if screened {
            return false;
        }
        let n = &self.notifications;
        let vip = self.is_vip(&m.from);
        if n.quiet.quiet_at(now)
            && !(vip && n.quiet.vips_break_through && !n.quiet.paused(now.timestamp()))
        {
            return false;
        }
        if vip {
            return true;
        }
        match n.mode {
            NotifyMode::VipsOnly => false,
            NotifyMode::PrimaryAndVips => category() == Category::Primary,
            NotifyMode::Everything => n.silent.is_empty() || !n.silent.contains(&category()),
        }
    }
    /// Whether deciding what's announced needs the mail's category.
    pub fn wants_category(&self) -> bool {
        match self.notifications.mode {
            NotifyMode::VipsOnly => false,
            NotifyMode::PrimaryAndVips => true,
            NotifyMode::Everything => !self.notifications.silent.is_empty(),
        }
    }
    /// Turns a category's notifications off, or on again.
    pub fn toggle_silent(&mut self, category: Category) {
        if !self.notifications.silent.remove(&category) {
            self.notifications.silent.insert(category);
        }
    }
}

/// What deciding about new mail away from the window needs: who is known
/// (loaded only when the Screener or the categories ask), and the
/// categories' settings.
pub struct Gate {
    pub control: InboxControl,
    pub categories: Categorizing,
    pub known: Option<Known>,
}
impl Gate {
    /// The settings in `store`; who `accounts` know is read only when
    /// the Screener is on, or categories decide what's announced.
    pub fn load(backend: &super::backend::Backend, accounts: &[super::model::Account]) -> Self {
        let store = &backend.store;
        let control = InboxControl::load(store);
        let categories = Categorizing::load(store);
        let wants = control.screener.enabled || (control.wants_category() && categories.sorting());
        let known = wants.then(|| Known::load(backend, accounts));
        Self {
            control,
            categories,
            known,
        }
    }
    /// Whether `m` waits in the Screener.
    pub fn screens(&self, m: &Mail) -> bool {
        self.known
            .as_ref()
            .is_some_and(|k| self.control.screens(m, k, &self.categories))
    }
    /// Whether new `m` is announced now, its category decided by `content`
    /// (the window's look at it, or a fresh one).
    pub fn announces(
        &self,
        m: &Mail,
        content: impl FnOnce() -> super::categories::Content,
    ) -> bool {
        let screened = self.screens(m);
        let categories = &self.categories;
        let known = &self.known;
        self.control.announces(
            m,
            screened,
            || {
                if !categories.sorting() {
                    return Category::Primary;
                }
                let empty = Known::default();
                let known = known.as_ref().unwrap_or(&empty);
                categories.fold(super::categories::decide(m, content(), categories, known))
            },
            &Local::now(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn at(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<chrono::Utc> {
        NaiveDate::from_ymd_opt(y, mo, d)
            .unwrap()
            .and_hms_opt(h, mi, 0)
            .unwrap()
            .and_utc()
    }
    fn mail(from: &str) -> Mail {
        Mail {
            id: "1".into(),
            account: "a".into(),
            from: from.into(),
            folder: Folder::Inbox,
            date: 100,
            ..Mail::default()
        }
    }

    #[test]
    fn vips_by_address_and_domain() {
        let mut c = InboxControl::default();
        assert_eq!(c.toggle_vip("Boss@Work.example"), Some(true));
        assert_eq!(c.toggle_vip("@family.example"), Some(true));
        assert_eq!(c.toggle_vip("not an address"), None);
        assert!(c.is_vip("boss@work.example"));
        assert!(c.is_vip("mum@mail.family.example"));
        assert!(!c.is_vip("x@notfamily.example"));
        assert!(
            c.approved("boss@work.example"),
            "VIPs are let through the Screener"
        );
        assert_eq!(c.toggle_vip("boss@work.example"), Some(false));
        assert!(!c.is_vip("boss@work.example"));
    }

    #[test]
    fn the_screener_holds_only_new_senders_since_it_was_on() {
        let mut c = InboxControl::default();
        let known = Known {
            people: ["friend@x.example".to_owned()].into(),
            own: ["me@x.example".to_owned()].into(),
        };
        let mut sorted = Categorizing::default();
        let stranger = mail("new@shop.example");
        assert!(!c.screens(&stranger, &known, &sorted), "off");
        c.set_screening(true, 50);
        assert!(c.screens(&stranger, &known, &sorted));
        assert!(!c.screens(&mail("friend@x.example"), &known, &sorted));
        assert!(!c.screens(&mail("me@x.example"), &known, &sorted));
        let old = Mail {
            date: 10,
            ..mail("new@shop.example")
        };
        assert!(!c.screens(&old, &known, &sorted), "older than the Screener");
        let elsewhere = Mail {
            folder: Folder::Archive,
            ..mail("new@shop.example")
        };
        assert!(!c.screens(&elsewhere, &known, &sorted));
        // Turning it on again keeps when it started.
        c.set_screening(true, 500);
        assert_eq!(c.screener.since, 50);
        // Routed to a category: let in.
        sorted.set_sender("new@shop.example", Some(Category::Promotions));
        assert!(!c.screens(&stranger, &known, &sorted));
        sorted.set_sender("new@shop.example", None);
        assert!(c.approve("@shop.example"));
        assert!(!c.screens(&stranger, &known, &sorted));
        c.forget_approval("@shop.example");
        assert!(c.screens(&stranger, &known, &sorted));
        let local = mail("root@localhost");
        assert!(c.screens(&local, &known, &sorted));
        assert!(c.approve("root@localhost"));
        assert!(!c.screens(&local, &known, &sorted));
        assert!(!c.announces(
            &stranger,
            true,
            || Category::Primary,
            &at(2026, 11, 27, 12, 0)
        ));
    }

    #[test]
    fn quiet_windows_run_past_midnight() {
        let night = QuietWindow {
            days: 1 << 4,
            start: 22 * 60,
            end: 7 * 60,
        }; // Fridays
        assert!(night.covers(4, 23 * 60));
        assert!(night.covers(5, 6 * 60), "into Saturday morning");
        assert!(!night.covers(5, 23 * 60), "but not Saturday night");
        assert!(!night.covers(4, 12 * 60));
        let lunch = QuietWindow {
            days: WEEKDAYS,
            start: 12 * 60,
            end: 13 * 60,
        };
        assert!(lunch.covers(0, 12 * 60 + 30));
        assert!(!lunch.covers(6, 12 * 60 + 30));
        let all_day = QuietWindow {
            days: WEEKEND,
            start: 0,
            end: 0,
        };
        assert!(all_day.covers(6, 3));
        assert!(!all_day.covers(0, 3));
    }

    #[test]
    fn clocks_read_and_written() {
        assert_eq!(QuietWindow::parse_clock("22:30"), Some(22 * 60 + 30));
        assert_eq!(QuietWindow::parse_clock("7"), Some(7 * 60));
        assert_eq!(QuietWindow::parse_clock("0730"), Some(7 * 60 + 30));
        assert_eq!(QuietWindow::parse_clock("7:15pm"), Some(19 * 60 + 15));
        assert_eq!(QuietWindow::parse_clock("12am"), Some(0));
        assert_eq!(QuietWindow::parse_clock("25:00"), None);
        assert_eq!(QuietWindow::parse_clock("9:75"), None);
        assert_eq!(QuietWindow::clock(7 * 60 + 5), "07:05");
    }

    #[test]
    fn notification_policy() {
        let mut c = InboxControl::default();
        c.toggle_vip("boss@work.example");
        let boss = mail("boss@work.example");
        let shop = mail("deals@shop.example");
        let noon = at(2026, 11, 27, 12, 0); // a Friday
        let promo = || Category::Promotions;
        assert!(c.announces(&shop, false, promo, &noon));
        c.toggle_silent(Category::Promotions);
        assert!(!c.announces(&shop, false, promo, &noon));
        assert!(c.announces(&shop, false, || Category::Primary, &noon));
        assert!(
            c.announces(&boss, false, promo, &noon),
            "VIPs override silent categories"
        );
        c.notifications.mode = NotifyMode::PrimaryAndVips;
        assert!(!c.announces(&shop, false, || Category::Updates, &noon));
        assert!(c.announces(&shop, false, || Category::Primary, &noon));
        c.notifications.mode = NotifyMode::VipsOnly;
        assert!(!c.announces(&shop, false, || Category::Primary, &noon));
        assert!(c.announces(&boss, false, promo, &noon));
        assert!(!c.wants_category());
        // Quiet hours: VIPs get through only where allowed.
        c.notifications.mode = NotifyMode::Everything;
        c.notifications.quiet.enabled = true;
        let late = at(2026, 11, 27, 23, 0);
        assert!(!c.announces(&shop, false, || Category::Primary, &late));
        assert!(c.announces(&boss, false, promo, &late));
        c.notifications.quiet.vips_break_through = false;
        assert!(!c.announces(&boss, false, promo, &late));
        // Paused by hand: silence, VIPs too.
        c.notifications.quiet.enabled = false;
        c.notifications.quiet.vips_break_through = true;
        c.notifications.quiet.paused_until = noon.timestamp() + 3600;
        assert!(!c.announces(&boss, false, promo, &noon));
        assert!(c.announces(&boss, false, promo, &at(2026, 11, 27, 14, 0)));
    }

    #[test]
    fn primary_only_carries_over_once() {
        let categories = Categorizing {
            notify_primary_only: true,
            ..Categorizing::default()
        };
        assert_eq!(
            InboxControl::from_categories(&categories)
                .notifications
                .mode,
            NotifyMode::PrimaryAndVips
        );
        let json = serde_json::to_string(&InboxControl::default()).unwrap();
        let back: InboxControl = serde_json::from_str(&json).unwrap();
        assert_eq!(back, InboxControl::default());
        // Older settings without the newer fields still read.
        let partial: InboxControl = serde_json::from_str(r#"{"vips":["a@b.example"]}"#).unwrap();
        assert!(partial.is_vip("a@b.example"));
        assert!(partial.notifications.quiet.vips_break_through);
    }
}

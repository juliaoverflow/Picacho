//! Workspaces: named sets of accounts and calendars shown together, such as
//! Work and Personal, switched between from the sidebar.
//!
//! - **Accounts**: the sidebar lists only the workspace's accounts (and
//!   aliases shown as accounts, by the account they're delivered to); All
//!   Accounts and its unread counts take in only those. Mail of the others
//!   keeps arriving and syncing, unseen.
//! - **Calendars**: the calendar's grids, its coming events and its tasks
//!   show only the workspace's calendars.
//! - **Address books**: the Contacts page and the composer's suggestions
//!   show only the workspace's contacts sources.
//! - **Counts**: home-screen and desktop widgets, Waybar and the tray
//!   count and list only the workspace's accounts for All Accounts
//!   (`widgets::snapshot`); a widget set to one account keeps showing it.
//! - **Notifications**: only the workspace's accounts' new mail and its
//!   calendars' reminders (the default), everything, or nothing at all (a
//!   focus workspace). Every place new mail is announced asks
//!   `Workspaces::announces_account` (through `inbox_control::Gate` away
//!   from the window), so it holds with the window closed too.
//! - **Schedules**: a workspace may turn on by itself at set times (Work on
//!   weekdays from 9 to 5), in the same windows quiet hours use. A
//!   workspace chosen by hand holds until a schedule next starts or ends.
//! - **Writing from**: the account new messages are written from where the
//!   open view doesn't decide (All Accounts).
//!
//! Each kind is every one (`None`, taking in those set up later too) or a
//! chosen set, so a new workspace hides nothing until it's narrowed. Saved
//! as one Store preference.
use super::inbox_control::QuietWindow;
use super::store::Store;
use chrono::{DateTime, Datelike, Days, Local, NaiveDate, TimeZone};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const PREFERENCE: &str = "workspaces";
/// How many colors workspaces are told apart by.
pub const COLORS: u8 = 8;
const MINUTES: u16 = 24 * 60;

/// Which notifications a workspace lets through while it's on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Alerts {
    /// Its own accounts' new mail and its calendars' reminders.
    #[default]
    Workspace,
    /// Every account's and calendar's, as without workspaces.
    Everything,
    /// None at all.
    Nothing,
}
impl Alerts {
    pub const ALL: [Self; 3] = [Self::Workspace, Self::Everything, Self::Nothing];
    pub fn label(self) -> &'static str {
        match self {
            Self::Workspace => t!("workspaces-alerts-workspace"),
            Self::Everything => t!("workspaces-alerts-everything"),
            Self::Nothing => t!("workspaces-alerts-nothing"),
        }
    }
}

/// Which of a kind (accounts, calendars, address books) a workspace shows:
/// every one (`None`, also those set up later), or those chosen.
pub type Pick = Option<BTreeSet<String>>;

/// Whether `pick` shows `item`.
fn picks(pick: &Pick, item: &str) -> bool {
    pick.as_ref().is_none_or(|set| set.contains(item))
}

/// Reads a pick; an empty list, as workspaces were first kept, is every one.
fn read_pick<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Pick, D::Error> {
    Ok(Pick::deserialize(d)?.filter(|set| !set.is_empty()))
}

/// A named set of accounts and calendars.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Workspace {
    pub id: String,
    pub name: String,
    /// Which of `COLORS` marks it.
    pub color: u8,
    /// The accounts it shows (ids). Never an empty set: every one instead.
    #[serde(deserialize_with = "read_pick")]
    pub accounts: Pick,
    /// The calendars it shows (hrefs); an empty set shows none. Kept
    /// under a name of its own: the first saved workspaces' empty list of
    /// calendars meant every one.
    #[serde(rename = "shown_calendars")]
    pub calendars: Pick,
    /// The address books it shows (contacts sources' ids, `""` for the
    /// first); an empty set shows none.
    pub contacts: Pick,
    pub alerts: Alerts,
    /// When it turns on by itself.
    pub schedule: Vec<QuietWindow>,
    /// The account new messages are written from where the open view
    /// doesn't say which (All Accounts).
    pub compose_from: Option<String>,
}

impl Workspace {
    pub fn new(name: &str, color: u8) -> Self {
        Self {
            id: format!("ws-{}", &uuid::Uuid::new_v4().simple().to_string()[..10]),
            name: name.trim().to_owned(),
            color: color % COLORS,
            ..Self::default()
        }
    }
    /// Whether account `id` is shown in it.
    pub fn shows_account(&self, id: &str) -> bool {
        picks(&self.accounts, id)
    }
    /// Whether account `id` is shown, among the accounts set up (`all`):
    /// where none of those it chose is set up any longer, every one is.
    pub fn shows_account_among(&self, id: &str, all: &[String]) -> bool {
        match &self.accounts {
            None => true,
            Some(set) => set.contains(id) || !all.iter().any(|a| set.contains(a)),
        }
    }
    /// Whether calendar `href` is shown in it.
    pub fn shows_calendar(&self, href: &str) -> bool {
        picks(&self.calendars, href)
    }
    /// Whether address book (contacts source) `id` is shown in it.
    pub fn shows_contacts(&self, id: &str) -> bool {
        picks(&self.contacts, id)
    }
    /// Whether new mail to account `id` is announced while it's on.
    pub fn announces_account(&self, id: &str) -> bool {
        match self.alerts {
            Alerts::Workspace => self.shows_account(id),
            Alerts::Everything => true,
            Alerts::Nothing => false,
        }
    }
    /// Whether a reminder from `calendar` (`None`: one not known) is shown
    /// while it's on.
    pub fn reminds(&self, calendar: Option<&str>) -> bool {
        match self.alerts {
            Alerts::Workspace => calendar.is_none_or(|c| self.shows_calendar(c)),
            Alerts::Everything => true,
            Alerts::Nothing => false,
        }
    }
    /// Shows account `id`, or no longer. The last one stays: a workspace
    /// shows at least one account. False when nothing changed.
    pub fn toggle_account(&mut self, id: &str) -> bool {
        let Some(set) = &mut self.accounts else {
            return false;
        };
        if set.contains(id) {
            if set.len() == 1 {
                return false;
            }
            set.remove(id);
        } else {
            set.insert(id.to_owned());
        }
        if self
            .compose_from
            .as_deref()
            .is_some_and(|a| !picks(&self.accounts, a))
        {
            self.compose_from = None;
        }
        true
    }
    /// Shows calendar `href`, or no longer (in a chosen set).
    pub fn toggle_calendar(&mut self, href: &str) {
        toggle(&mut self.calendars, href);
    }
    /// Shows address book `id`, or no longer (in a chosen set).
    pub fn toggle_contacts(&mut self, id: &str) {
        toggle(&mut self.contacts, id);
    }
    /// Shows every account, or (`every` false) those of `all` chosen: all
    /// of them to start with, to be narrowed.
    pub fn every_account(&mut self, every: bool, all: &[String]) {
        self.accounts = (!every && !all.is_empty()).then(|| all.iter().cloned().collect());
    }
    /// As `every_account`, for calendars.
    pub fn every_calendar(&mut self, every: bool, all: &[String]) {
        self.calendars = (!every).then(|| all.iter().cloned().collect());
    }
    /// As `every_account`, for address books.
    pub fn every_contacts(&mut self, every: bool, all: &[String]) {
        self.contacts = (!every).then(|| all.iter().cloned().collect());
    }
    /// Whether its schedule covers `minute` (after midnight) of `day`
    /// (0 = Monday).
    fn scheduled(&self, day: u32, minute: u16) -> bool {
        self.schedule.iter().any(|w| w.covers(day, minute))
    }
}

/// Adds `item` to a chosen set, or takes it out; every one stays every one.
fn toggle(pick: &mut Pick, item: &str) {
    if let Some(set) = pick
        && !set.remove(item)
    {
        set.insert(item.to_owned());
    }
}

/// Every workspace, and which is on.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Workspaces {
    pub list: Vec<Workspace>,
    /// The workspace chosen by hand; `None` for everything.
    pub chosen: Option<String>,
    /// When it was chosen (Unix seconds): a schedule starting or ending
    /// after it takes over.
    pub chosen_at: i64,
    /// Workspaces turn on and off by their schedules.
    pub follow_schedule: bool,
}
impl Default for Workspaces {
    fn default() -> Self {
        Self {
            list: Vec::new(),
            chosen: None,
            chosen_at: 0,
            follow_schedule: true,
        }
    }
}

impl Workspaces {
    pub fn load(store: &Store) -> Self {
        store
            .preference(PREFERENCE)
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_default()
    }
    pub fn save(&self, store: &Store) {
        if let Ok(json) = serde_json::to_string(self) {
            store.set_preference_soon(PREFERENCE, &json);
        }
    }
    /// Turns workspace `id` on (`None`: everything) away from the window
    /// (the tray of the background process), and has widgets and the tray
    /// count again.
    #[cfg_attr(any(target_os = "android", target_os = "ios"), allow(dead_code))]
    pub fn switch_in(store: &std::sync::Arc<Store>, id: Option<String>) {
        let mut workspaces = Self::load(store);
        workspaces.choose(id, chrono::Utc::now().timestamp());
        workspaces.save(store);
        super::widgets::publish_soon(store.clone());
    }

    pub fn get(&self, id: &str) -> Option<&Workspace> {
        self.list.iter().find(|w| w.id == id)
    }
    pub fn get_mut(&mut self, id: &str) -> Option<&mut Workspace> {
        self.list.iter_mut().find(|w| w.id == id)
    }

    /// Adds a workspace named `name` (in the next color), returning its id.
    pub fn add(&mut self, name: &str) -> String {
        let used: BTreeSet<u8> = self.list.iter().map(|w| w.color).collect();
        let color = (0..COLORS)
            .find(|c| !used.contains(c))
            .unwrap_or(self.list.len() as u8 % COLORS);
        let w = Workspace::new(name, color);
        let id = w.id.clone();
        self.list.push(w);
        id
    }
    /// Removes workspace `id`; everything shows again where it was on.
    pub fn remove(&mut self, id: &str) {
        self.list.retain(|w| w.id != id);
        if self.chosen.as_deref() == Some(id) {
            self.chosen = None;
        }
    }
    /// Moves workspace `id` up (negative) or down the list. The first
    /// whose schedule covers a time wins it, so order matters.
    pub fn shift(&mut self, id: &str, by: i32) -> bool {
        let Some(i) = self.list.iter().position(|w| w.id == id) else {
            return false;
        };
        let to = (i as i64 + by as i64).clamp(0, self.list.len() as i64 - 1) as usize;
        if to == i {
            return false;
        }
        let w = self.list.remove(i);
        self.list.insert(to, w);
        true
    }
    /// Chooses workspace `id` (`None`: everything) by hand at `now`.
    pub fn choose(&mut self, id: Option<String>, now: i64) {
        self.chosen = id.filter(|id| self.get(id).is_some());
        self.chosen_at = now;
    }
    /// Forgets accounts no longer set up: kept nowhere, written from by
    /// none; a workspace left with none of its accounts shows every one.
    /// True when anything changed.
    pub fn reconcile(&mut self, accounts: &[String]) -> bool {
        let before = self.list.clone();
        for w in &mut self.list {
            if let Some(set) = &mut w.accounts {
                set.retain(|a| accounts.contains(a));
                if set.is_empty() {
                    w.accounts = None;
                }
            }
            if w.compose_from
                .as_ref()
                .is_some_and(|a| !accounts.contains(a))
            {
                w.compose_from = None;
            }
        }
        self.list != before
    }
    /// Whether any workspace has a schedule that applies.
    pub fn scheduling(&self) -> bool {
        self.follow_schedule && self.list.iter().any(|w| !w.schedule.is_empty())
    }
    /// The workspace whose schedule covers `now`: the first listed.
    pub fn scheduled_at<Tz: TimeZone>(&self, now: &DateTime<Tz>) -> Option<&Workspace> {
        let day = now.weekday().num_days_from_monday();
        let minute = (chrono::Timelike::hour(now) * 60 + chrono::Timelike::minute(now)) as u16;
        self.list.iter().find(|w| w.scheduled(day, minute))
    }
    /// Whether a schedule started or ended after `from` (Unix seconds),
    /// up to `now`.
    fn schedule_turned<Tz: TimeZone>(&self, from: i64, now: &DateTime<Tz>) -> bool {
        let to = now.timestamp();
        if from >= to {
            return false;
        }
        let windows: Vec<&QuietWindow> = self.list.iter().flat_map(|w| &w.schedule).collect();
        if windows.is_empty() {
            return false;
        }
        let tz = now.timezone();
        let Some(start) = DateTime::from_timestamp(from, 0) else {
            return true;
        };
        // From the day before (a window running past midnight) to today.
        let first = start.with_timezone(&tz).date_naive() - Days::new(1);
        let last = now.date_naive();
        // Over a week: every window that has days started since.
        if (last - first).num_days() > 8 {
            return windows.iter().any(|w| w.days != 0);
        }
        let at = |day: NaiveDate, minute: u16| -> Option<i64> {
            let (day, minute) = if minute >= MINUTES {
                (day + Days::new(1), minute - MINUTES)
            } else {
                (day, minute)
            };
            let time = day.and_hms_opt(u32::from(minute / 60), u32::from(minute % 60), 0)?;
            Some(tz.from_local_datetime(&time).earliest()?.timestamp())
        };
        let mut day = first;
        while day <= last {
            let weekday = day.weekday().num_days_from_monday();
            for w in windows.iter().filter(|w| w.on(weekday)) {
                let (s, e) = (w.start.min(MINUTES - 1), w.end.min(MINUTES));
                // All day (`QuietWindow::covers`): midnight to midnight.
                let (s, e) = if s == e {
                    (0, MINUTES)
                } else if e < s {
                    (s, e + MINUTES)
                } else {
                    (s, e)
                };
                if [at(day, s), at(day, e)]
                    .into_iter()
                    .flatten()
                    .any(|t| t > from && t <= to)
                {
                    return true;
                }
            }
            day = day + Days::new(1);
        }
        false
    }
    /// The workspace on at `now`: the one chosen by hand, unless a
    /// schedule started or ended since, then the one scheduled (none when
    /// none is).
    pub fn current_at<Tz: TimeZone>(&self, now: &DateTime<Tz>) -> Option<&Workspace> {
        let chosen = self.chosen.as_deref().and_then(|id| self.get(id));
        if self.scheduling() && self.schedule_turned(self.chosen_at, now) {
            return self.scheduled_at(now);
        }
        chosen
    }
    /// The workspace on now.
    pub fn current(&self) -> Option<&Workspace> {
        self.current_at(&Local::now())
    }
    /// Whether new mail to account `id` is announced at `now`.
    pub fn announces_account<Tz: TimeZone>(&self, id: &str, now: &DateTime<Tz>) -> bool {
        self.current_at(now).is_none_or(|w| w.announces_account(id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manage::inbox_control::WEEKDAYS;

    fn at(d: u32, h: u32, mi: u32) -> DateTime<chrono::Utc> {
        // November 2026: the 23rd is a Monday.
        NaiveDate::from_ymd_opt(2026, 11, d)
            .unwrap()
            .and_hms_opt(h, mi, 0)
            .unwrap()
            .and_utc()
    }
    fn set(items: &[&str]) -> Pick {
        Some(items.iter().map(|i| (*i).to_owned()).collect())
    }
    fn work_and_home() -> (Workspaces, String, String) {
        let mut ws = Workspaces::default();
        let work = ws.add("Work");
        let home = ws.add("Home");
        let w = ws.get_mut(&work).unwrap();
        w.accounts = set(&["work"]);
        w.calendars = set(&["/cal/work/"]);
        w.schedule = vec![QuietWindow {
            days: WEEKDAYS,
            start: 9 * 60,
            end: 17 * 60,
        }];
        ws.get_mut(&home).unwrap().accounts = set(&["home"]);
        (ws, work, home)
    }

    #[test]
    fn every_one_unless_chosen() {
        let w = Workspace::new(" Everything ", 9);
        assert_eq!(w.name, "Everything");
        assert_eq!(w.color, 1, "colors wrap");
        assert!(w.shows_account("any") && w.shows_calendar("any") && w.shows_contacts(""));
        assert!(w.announces_account("any") && w.reminds(None));
    }

    #[test]
    fn choosing_accounts_calendars_and_address_books() {
        let all = ["a".to_owned(), "b".to_owned(), "c".to_owned()];
        let mut w = Workspace::new("W", 0);
        assert!(!w.toggle_account("b"), "every account: nothing to take out");
        w.every_account(false, &all);
        assert_eq!(w.accounts, set(&["a", "b", "c"]), "all chosen, to narrow");
        w.compose_from = Some("b".into());
        assert!(w.toggle_account("b"));
        assert!(w.shows_account("a") && !w.shows_account("b"));
        assert!(
            w.compose_from.is_none(),
            "not written from a hidden account"
        );
        assert!(w.toggle_account("a"));
        assert!(!w.toggle_account("c"), "the last account stays");
        assert_eq!(w.accounts, set(&["c"]));
        // An account set up later isn't shown, unless every one is.
        assert!(!w.shows_account("d"));
        // Among accounts set up: none of its own left, every one shows.
        assert!(!w.shows_account_among("a", &all));
        assert!(w.shows_account_among("a", &["a".to_owned()]));
        w.every_account(true, &all);
        assert!(w.shows_account("d"));
        // Calendars and address books may be emptied: a workspace of none.
        w.every_calendar(false, &["/x/".to_owned()]);
        w.toggle_calendar("/x/");
        assert_eq!(w.calendars, Some(BTreeSet::new()));
        assert!(!w.shows_calendar("/x/"));
        w.every_contacts(false, &["".to_owned(), "s1".to_owned()]);
        w.toggle_contacts("");
        assert!(!w.shows_contacts("") && w.shows_contacts("s1"));
    }

    #[test]
    fn alerts_follow_the_workspace() {
        let (mut ws, work, _) = work_and_home();
        let noon = at(28, 12, 0); // a Saturday: no schedule
        ws.choose(Some(work.clone()), noon.timestamp() - 60);
        assert!(ws.announces_account("work", &noon));
        assert!(!ws.announces_account("home", &noon));
        let w = ws.get_mut(&work).unwrap();
        assert!(w.reminds(Some("/cal/work/")) && !w.reminds(Some("/cal/home/")));
        assert!(w.reminds(None), "a reminder from no known calendar shows");
        w.alerts = Alerts::Everything;
        assert!(ws.announces_account("home", &noon));
        ws.get_mut(&work).unwrap().alerts = Alerts::Nothing;
        assert!(!ws.announces_account("work", &noon));
        ws.choose(None, noon.timestamp());
        assert!(
            ws.announces_account("home", &noon),
            "everything: all announced"
        );
    }

    #[test]
    fn schedules_switch_and_hand_choices_hold_until_the_next_change() {
        let (mut ws, work, home) = work_and_home();
        // Never chosen: the schedule decides.
        assert_eq!(ws.current_at(&at(23, 10, 0)).map(|w| &w.id), Some(&work));
        assert_eq!(ws.current_at(&at(23, 18, 0)).map(|w| &w.id), None);
        // Home chosen at 10:00 on Monday holds through the workday…
        ws.choose(Some(home.clone()), at(23, 10, 0).timestamp());
        assert_eq!(ws.current_at(&at(23, 16, 59)).map(|w| &w.id), Some(&home));
        // …until the schedule ends at 17:00: nothing is scheduled then.
        assert_eq!(ws.current_at(&at(23, 17, 0)).map(|w| &w.id), None);
        // Chosen after work, it holds overnight, until work starts.
        ws.choose(Some(home.clone()), at(23, 18, 0).timestamp());
        assert_eq!(ws.current_at(&at(24, 8, 59)).map(|w| &w.id), Some(&home));
        assert_eq!(ws.current_at(&at(24, 9, 0)).map(|w| &w.id), Some(&work));
        // Chosen on Friday evening, it holds over the weekend.
        ws.choose(Some(home.clone()), at(27, 20, 0).timestamp());
        assert_eq!(ws.current_at(&at(29, 23, 0)).map(|w| &w.id), Some(&home));
        assert_eq!(ws.current_at(&at(30, 9, 30)).map(|w| &w.id), Some(&work));
        // Chosen long ago: the schedule has turned since.
        ws.choose(Some(home.clone()), at(1, 12, 0).timestamp());
        assert_eq!(ws.current_at(&at(28, 12, 0)).map(|w| &w.id), None);
        // Not following schedules: the choice holds.
        ws.follow_schedule = false;
        assert_eq!(ws.current_at(&at(30, 10, 0)).map(|w| &w.id), Some(&home));
    }

    #[test]
    fn overnight_and_all_day_windows_turn_at_their_ends() {
        let mut ws = Workspaces::default();
        let night = ws.add("Night");
        ws.get_mut(&night).unwrap().schedule = vec![QuietWindow {
            days: 1 << 0, // Monday
            start: 22 * 60,
            end: 6 * 60,
        }];
        ws.choose(None, at(23, 21, 0).timestamp());
        assert!(ws.current_at(&at(23, 21, 59)).is_none());
        assert_eq!(ws.current_at(&at(23, 22, 0)).map(|w| &w.id), Some(&night));
        ws.choose(None, at(23, 23, 0).timestamp());
        assert!(ws.current_at(&at(24, 5, 0)).is_none(), "chosen by hand");
        ws.choose(Some(night.clone()), at(24, 5, 0).timestamp());
        assert!(ws.current_at(&at(24, 6, 0)).is_none(), "ended at 06:00");

        let mut ws = Workspaces::default();
        let sunday = ws.add("Sunday");
        ws.get_mut(&sunday).unwrap().schedule = vec![QuietWindow {
            days: 1 << 6,
            start: 0,
            end: 0,
        }];
        ws.choose(None, at(28, 12, 0).timestamp());
        assert_eq!(ws.current_at(&at(29, 0, 0)).map(|w| &w.id), Some(&sunday));
        assert!(ws.current_at(&at(30, 0, 1)).is_none());
    }

    #[test]
    fn removing_ordering_and_forgetting_accounts() {
        let (mut ws, work, home) = work_and_home();
        assert_ne!(ws.get(&work).unwrap().color, ws.get(&home).unwrap().color);
        assert!(ws.shift(&home, -1));
        assert_eq!(ws.list[0].id, home);
        assert!(!ws.shift(&home, -1), "already first");
        ws.get_mut(&work).unwrap().compose_from = Some("work".into());
        assert!(ws.reconcile(&["home".to_owned()]));
        let w = ws.get(&work).unwrap();
        assert!(w.accounts.is_none(), "none of its accounts left: every one");
        assert!(w.compose_from.is_none());
        assert_eq!(ws.get(&home).unwrap().accounts, set(&["home"]));
        assert!(!ws.reconcile(&["home".to_owned()]));
        ws.choose(Some(work.clone()), 0);
        ws.remove(&work);
        assert!(ws.chosen.is_none() && ws.get(&work).is_none());
        ws.choose(Some("gone".into()), 0);
        assert!(ws.chosen.is_none(), "only a workspace there is");
    }

    #[test]
    fn saved_settings_read_back() {
        let (ws, ..) = work_and_home();
        let json = serde_json::to_string(&ws).unwrap();
        assert_eq!(serde_json::from_str::<Workspaces>(&json).unwrap(), ws);
        let old: Workspaces = serde_json::from_str(r#"{"list":[{"id":"x","name":"X"}]}"#).unwrap();
        assert!(old.follow_schedule && old.list[0].accounts.is_none());
        // An empty list of accounts, as first kept, is every account.
        let first: Workspaces =
            serde_json::from_str(r#"{"list":[{"id":"x","accounts":[],"calendars":[]}]}"#).unwrap();
        assert!(first.list[0].accounts.is_none() && first.list[0].calendars.is_none());
        // A workspace of no calendars reads back as one.
        let mut none = Workspaces::default();
        let id = none.add("None");
        none.get_mut(&id).unwrap().calendars = Some(BTreeSet::new());
        let back: Workspaces =
            serde_json::from_str(&serde_json::to_string(&none).unwrap()).unwrap();
        assert_eq!(back, none);
    }
}

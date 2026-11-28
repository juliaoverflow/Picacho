//! Linux: GNOME Shell's search (its Activities overview) and KDE Plasma's
//! KRunner (and the launcher's search, which uses it) ask apps for results
//! over D-Bus as one types. Picacho answers both from its own full-text
//! index (`Store::search_texts`), while it runs (with a window or in the
//! background), on one bus name:
//!
//! - `org.gnome.Shell.SearchProvider2` at `GNOME_PATH`, which GNOME finds
//!   through `packaging/linux/tech.pinal.picacho.search-provider.ini`
//!   (installed with the Flatpak or a system package: GNOME reads only the
//!   system's data folders);
//! - `org.kde.krunner1` at `KRUNNER_PATH`, which KRunner finds through
//!   `packaging/linux/tech.pinal.picacho.krunner.desktop` (the Flatpak's,
//!   or the one `display::desktop` installs for the user).
//!
//! Nothing is copied anywhere: the searches get the subjects, senders and
//! dates of what matches, and only while mail in the system's search is on
//! and allowed (`super::enabled`, `super::allowed`). Each result is the
//! message's `picacho://widget/message` link; choosing one opens it.
use super::{allowed, enabled, link, listed_mailbox, person};
use crate::manage::store::{Store, SystemSearchMessage};
use crate::manage::widgets::WidgetAction;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use zbus::zvariant::{OwnedValue, Value};

/// The name both searches call Picacho by (one of the app ID's, which a
/// Flatpak may own).
pub const BUS_NAME: &str = "tech.pinal.picacho.SearchProvider";
pub const GNOME_PATH: &str = "/tech/pinal/picacho/SearchProvider";
pub const KRUNNER_PATH: &str = "/tech/pinal/picacho/KRunner";
/// The most results given at a time.
const MOST: usize = 12;
/// Fewer letters than this find nothing: every message has an "a".
const FEWEST_LETTERS: usize = 3;
/// The icon results show (the app's).
const ICON: &str = "tech.pinal.picacho";
/// KRunner's `QueryMatch::PossibleMatch`.
const POSSIBLE_MATCH: i32 = 30;

/// What both searches share: the store, and what opens a result.
struct Search {
    store: Arc<Store>,
    open: fn(String),
}

impl Search {
    fn on(&self) -> bool {
        enabled(&self.store) && allowed(&self.store)
    }

    /// The messages matching every one of `words`, best first, but junk
    /// and trash: their links and summaries.
    fn find(&self, words: &[String]) -> Vec<(String, SystemSearchMessage)> {
        let words: Vec<String> = words.iter().map(|w| w.trim().to_owned()).filter(|w| !w.is_empty()).collect();
        let letters: usize = words.iter().map(|w| w.chars().filter(|c| c.is_alphanumeric()).count()).sum();
        if letters < FEWEST_LETTERS || !self.on() {
            return Vec::new();
        }
        let ids = self.store.search_texts(&words, &[], MOST * 3).unwrap_or_default();
        self.store
            .system_search_messages(&ids, 200)
            .unwrap_or_default()
            .into_iter()
            .filter(|(_, m)| listed_mailbox(&m.mailbox))
            .take(MOST)
            .map(|(id, m)| (link(&m.account, &id), m))
            .collect()
    }

    /// The message a result names, if it's one here that may be shown.
    fn summary(&self, result: &str) -> Option<SystemSearchMessage> {
        let Some(WidgetAction::Message { account, id }) = WidgetAction::parse(result) else {
            return None;
        };
        let (_, m) = self.store.system_search_messages(&[id], 200).ok()?.pop()?;
        (m.account == account && listed_mailbox(&m.mailbox)).then_some(m)
    }

    /// Opens a result chosen: a message's link, and only while on.
    fn activate(&self, result: &str) {
        if self.on() && matches!(WidgetAction::parse(result), Some(WidgetAction::Message { .. })) {
            (self.open)(result.to_owned());
        }
    }
}

/// A result's title: its subject.
fn title(m: &SystemSearchMessage) -> String {
    let subject = m.subject.trim();
    if subject.is_empty() { t!("manage-system-search-no-subject").to_owned() } else { subject.replace(['\n', '\r'], " ") }
}

/// A result's line below: who sent it, and when.
fn subtitle(m: &SystemSearchMessage) -> String {
    let from = person(&m.sender, &m.from);
    let from = match m.sender.trim() {
        name if !name.is_empty() && name != m.from.trim() => name.to_owned(),
        _ => from,
    };
    let date = chrono::DateTime::from_timestamp(m.date, 0)
        .map(|d| crate::i18n::dates::format(&d.with_timezone(&chrono::Local), t!("dates-month-day-year")))
        .unwrap_or_default();
    if date.is_empty() { from } else { t!("manage-system-search-from-on", from = from, date = date) }
}

fn value(text: String) -> OwnedValue {
    OwnedValue::try_from(Value::from(text)).expect("a string has no file descriptors")
}

/// GNOME Shell's search provider (version 2).
struct Gnome(Arc<Search>);

#[zbus::interface(name = "org.gnome.Shell.SearchProvider2")]
impl Gnome {
    fn get_initial_result_set(&self, terms: Vec<String>) -> Vec<String> {
        self.0.find(&terms).into_iter().map(|(link, _)| link).collect()
    }

    fn get_subsearch_result_set(&self, _previous_results: Vec<String>, terms: Vec<String>) -> Vec<String> {
        self.0.find(&terms).into_iter().map(|(link, _)| link).collect()
    }

    fn get_result_metas(&self, identifiers: Vec<String>) -> Vec<HashMap<String, OwnedValue>> {
        if !self.0.on() {
            return Vec::new();
        }
        identifiers
            .into_iter()
            .filter_map(|id| {
                let m = self.0.summary(&id)?;
                Some(HashMap::from([
                    ("id".to_owned(), value(id)),
                    ("name".to_owned(), value(title(&m))),
                    ("description".to_owned(), value(subtitle(&m))),
                    ("gicon".to_owned(), value(ICON.to_owned())),
                ]))
            })
            .collect()
    }

    fn activate_result(&self, identifier: String, _terms: Vec<String>, _timestamp: u32) {
        self.0.activate(&identifier);
    }

    /// "Search in Picacho": opens Picacho on its inbox.
    fn launch_search(&self, _terms: Vec<String>, _timestamp: u32) {
        if self.0.on() {
            (self.0.open)(WidgetAction::Inbox { account: None }.to_uri());
        }
    }
}

/// One KRunner result: id, text, icon, kind, relevance, properties.
type Match = (String, String, String, i32, f64, HashMap<String, OwnedValue>);

/// KDE's KRunner (its D-Bus runner API, version 1).
struct KRunner(Arc<Search>);

#[zbus::interface(name = "org.kde.krunner1")]
impl KRunner {
    /// Results have no buttons of their own: choosing one opens it.
    fn actions(&self) -> Vec<(String, String, String)> {
        Vec::new()
    }

    #[zbus(name = "Match")]
    fn find(&self, query: String) -> Vec<Match> {
        let words: Vec<String> = query.split_whitespace().map(str::to_owned).collect();
        self.0
            .find(&words)
            .into_iter()
            .enumerate()
            .map(|(i, (link, m))| {
                let properties = HashMap::from([("subtext".to_owned(), value(subtitle(&m)))]);
                let relevance = (0.7 - i as f64 * 0.04).max(0.1);
                (link, title(&m), ICON.to_owned(), POSSIBLE_MATCH, relevance, properties)
            })
            .collect()
    }

    fn run(&self, match_id: String, _action_id: String) {
        self.0.activate(&match_id);
    }

    fn teardown(&self) {}
}

/// Serves both searches for `store` on the session bus, once per process,
/// opening results chosen with `open`. Off the caller's thread: connecting
/// waits on the bus. Where there's no session bus, Settings says so.
pub fn serve(store: Arc<Store>, open: fn(String)) {
    static SERVING: OnceLock<zbus::blocking::Connection> = OnceLock::new();
    static STARTED: std::sync::Once = std::sync::Once::new();
    STARTED.call_once(move || {
        let started = std::thread::Builder::new().name("desktop-search".into()).spawn(move || {
            let search = Arc::new(Search { store, open });
            let connected = (|| -> zbus::Result<zbus::blocking::Connection> {
                let connection = zbus::blocking::connection::Builder::session()?
                    .serve_at(GNOME_PATH, Gnome(search.clone()))?
                    .serve_at(KRUNNER_PATH, KRunner(search))?
                    .build()?;
                // The window's process and the background one hand over to
                // each other: the one starting takes the name.
                use zbus::fdo::RequestNameFlags;
                connection.request_name_with_flags(
                    BUS_NAME,
                    RequestNameFlags::ReplaceExisting | RequestNameFlags::AllowReplacement,
                )?;
                Ok(connection)
            })();
            match connected {
                Ok(connection) => {
                    let _ = SERVING.set(connection);
                }
                Err(e) => {
                    eprintln!("Picacho: the desktop's search can't ask Picacho: {e}");
                    super::set_status(|s| s.error = Some(t!("manage-system-search-no-session-bus", e = e.to_string())));
                }
            }
        });
        if let Err(e) = started {
            eprintln!("Picacho: couldn't start answering the desktop's search: {e}");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manage::model::{Folder, Mail};

    fn store() -> (Arc<Store>, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("picacho-desktop-search-{}", uuid::Uuid::new_v4()));
        let store = Arc::new(Store::open(&dir).unwrap());
        let mail = |id: &str, folder: Folder, subject: &str| Mail {
            id: id.into(),
            account: "a".into(),
            folder,
            subject: subject.into(),
            from: "ana@example.org".into(),
            sender: "Ana Núñez".into(),
            body: Some(if subject == "Lunch" { "See you at noon" } else { "The quarterly invoice is attached" }.into()),
            date: 1_700_000_000,
            ..Mail::default()
        };
        store
            .save_messages(&[
                mail("m1", Folder::Inbox, "Invoice for March"),
                mail("m2", Folder::Trash, "Invoice, thrown away"),
                mail("m3", Folder::Archive, "Lunch"),
            ])
            .unwrap();
        (store, dir)
    }

    static OPENED: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

    /// Over the session bus, as GNOME Shell and KRunner call it: their
    /// methods, by name, with their signatures. Needs a session bus (and
    /// takes the name a running Picacho would have): run by hand,
    /// `cargo test -- --ignored answers_over_the_session_bus`.
    #[test]
    #[ignore = "needs a session bus"]
    fn answers_over_the_session_bus() -> zbus::Result<()> {
        let (store, dir) = store();
        super::super::set_enabled(&store, true).unwrap();
        serve(store, |link| OPENED.lock().unwrap().push(link));
        let client = zbus::blocking::Connection::session()?;
        let dbus = zbus::blocking::fdo::DBusProxy::new(&client)?;
        let name = zbus::names::BusName::try_from(BUS_NAME)?;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !dbus.name_has_owner(name.clone())? {
            assert!(std::time::Instant::now() < deadline, "the name wasn't taken");
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let reply = client.call_method(Some(BUS_NAME), KRUNNER_PATH, Some("org.kde.krunner1"), "Match", &("invoice",))?;
        assert_eq!(reply.body().signature().to_string(), "a(sssida{sv})");
        let matches: Vec<Match> = reply.body().deserialize()?;
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].1, "Invoice for March");
        let actions = client.call_method(Some(BUS_NAME), KRUNNER_PATH, Some("org.kde.krunner1"), "Actions", &())?;
        assert_eq!(actions.body().signature().to_string(), "a(sss)");
        client.call_method(Some(BUS_NAME), KRUNNER_PATH, Some("org.kde.krunner1"), "Run", &(matches[0].0.as_str(), ""))?;
        let gnome = Some("org.gnome.Shell.SearchProvider2");
        let reply = client.call_method(Some(BUS_NAME), GNOME_PATH, gnome, "GetInitialResultSet", &(vec!["invoice"],))?;
        let ids: Vec<String> = reply.body().deserialize()?;
        assert_eq!(ids, [matches[0].0.clone()]);
        let reply = client.call_method(Some(BUS_NAME), GNOME_PATH, gnome, "GetSubsearchResultSet", &(ids.clone(), vec!["invoice", "march"]))?;
        assert_eq!(reply.body().deserialize::<Vec<String>>()?, ids);
        let reply = client.call_method(Some(BUS_NAME), GNOME_PATH, gnome, "GetResultMetas", &(ids.clone(),))?;
        assert_eq!(reply.body().signature().to_string(), "aa{sv}");
        let metas: Vec<HashMap<String, OwnedValue>> = reply.body().deserialize()?;
        assert_eq!(String::try_from(metas[0]["name"].try_clone().unwrap()).unwrap(), "Invoice for March");
        client.call_method(Some(BUS_NAME), GNOME_PATH, gnome, "ActivateResult", &(ids[0].as_str(), vec!["invoice"], 0u32))?;
        assert_eq!(*OPENED.lock().unwrap(), [ids[0].clone(), ids[0].clone()]);
        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    }

    /// Off, nothing is said; on, what matches (but trash), with titles and
    /// senders; a result chosen opens its message.
    #[test]
    fn searches_answer_only_while_on() {
        let (store, dir) = store();
        let search = Arc::new(Search {
            store: store.clone(),
            open: |link| OPENED.lock().unwrap().push(link),
        });
        let gnome = Gnome(search.clone());
        let krunner = KRunner(search.clone());
        assert!(gnome.get_initial_result_set(vec!["invoice".into()]).is_empty(), "off");
        super::super::set_enabled(&store, true).unwrap();
        let found = gnome.get_initial_result_set(vec!["invo".into()]);
        assert_eq!(found, [link("a", "m1")], "trash stays out; the last word's start is enough");
        assert!(gnome.get_initial_result_set(vec!["in".into()]).is_empty(), "too short");
        let metas = gnome.get_result_metas(found.clone());
        assert_eq!(metas.len(), 1);
        let text = |key: &str| String::try_from(metas[0][key].try_clone().unwrap()).unwrap();
        assert_eq!(text("name"), "Invoice for March");
        assert!(text("description").starts_with("Ana Núñez"), "{}", text("description"));
        // Only messages' links are opened, never other `picacho:` links.
        gnome.activate_result(found[0].clone(), vec![], 0);
        gnome.activate_result("picacho://widget/compose".into(), vec![], 0);
        let matches = krunner.find("quarterly invoice".into());
        assert_eq!(matches.iter().map(|m| m.0.as_str()).collect::<Vec<_>>(), [link("a", "m1").as_str()]);
        assert_eq!(matches[0].1, "Invoice for March");
        krunner.run(matches[0].0.clone(), String::new());
        assert_eq!(*OPENED.lock().unwrap(), [link("a", "m1"), link("a", "m1")]);
        // Hidden outside Picacho: nothing again.
        crate::manage::applock::set_hides_contents(&store, true).unwrap();
        assert!(krunner.find("invoice".into()).is_empty());
        assert!(gnome.get_result_metas(found).is_empty());
        crate::manage::applock::set_hides_contents(&store, false).unwrap();
        // Nothing is queued for copies on Linux.
        assert_eq!(store.system_search_waiting().unwrap(), 0);
        drop((gnome, krunner, search, store));
        let _ = std::fs::remove_dir_all(&dir);
    }
}

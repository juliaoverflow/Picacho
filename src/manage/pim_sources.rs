//! Several calendar and address-book sources side by side: a work account
//! (Exchange through Microsoft Graph or EWS) beside a personal one
//! (Fastmail's JMAP, iCloud, Nextcloud, Google, Proton), and calendar
//! feeds subscribed to, all shown together.
//!
//! Each source keeps what one source always kept (its server, its
//! calendars or cards, its sync state), under keys of its own: the first
//! source under the keys there have always been (so what was set up before
//! keeps working as it was), each other one under `key@id`. The other
//! sources' ids are listed under `calendar_sources` and `contacts_sources`.
//!
//! Work on one source runs *within* it (`within`): every preference read
//! or written there is that source's. Outside any source, reading puts
//! every source's together (all calendars, every event, every card), and
//! anything that changes one thing (an event, a card, a calendar) first
//! finds the source that has it and goes on within that one. That is how
//! `calendar.rs` and `contacts.rs` are written: each changing function
//! starts by finding its source when called from outside one.
use super::*;
use std::cell::RefCell;

/// Which kind of source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pim {
    Calendar = 0,
    Contacts = 1,
}

impl Pim {
    /// Where the other sources' ids are listed.
    fn list_key(self) -> &'static str {
        match self {
            Pim::Calendar => "calendar_sources",
            Pim::Contacts => "contacts_sources",
        }
    }
}

thread_local! {
    /// The source this thread works within, for each kind: `None` outside
    /// any, `Some("")` the first source.
    static WITHIN: RefCell<[Option<String>; 2]> = const { RefCell::new([None, None]) };
}

/// The source this thread works within, if any.
pub(crate) fn current(kind: Pim) -> Option<String> {
    WITHIN.with(|w| w.borrow()[kind as usize].clone())
}
/// Whether this thread works within a source of `kind`.
pub(crate) fn inside(kind: Pim) -> bool {
    WITHIN.with(|w| w.borrow()[kind as usize].is_some())
}

/// `base`'s key in source `id`: `base` itself for the first, `base@id`
/// for the others.
pub fn key_in(base: &str, id: &str) -> String {
    if id.is_empty() { base.to_owned() } else { format!("{base}@{id}") }
}
/// `base`'s key in the source this thread works within (the first
/// source's outside any).
pub(crate) fn key(kind: Pim, base: &str) -> String {
    key_in(base, current(kind).as_deref().unwrap_or_default())
}

/// Puts back the source worked within before, also when work panics.
struct Restore {
    kind: Pim,
    before: Option<String>,
}
impl Drop for Restore {
    fn drop(&mut self) {
        let before = self.before.take();
        WITHIN.with(|w| w.borrow_mut()[self.kind as usize] = before);
    }
}

/// Runs `work` within source `id` of `kind`.
pub(crate) fn within<T>(kind: Pim, id: &str, work: impl FnOnce() -> T) -> T {
    let before = WITHIN.with(|w| w.borrow_mut()[kind as usize].replace(id.to_owned()));
    let _restore = Restore { kind, before };
    work()
}

/// Runs `work` outside any source of `kind` (from within one: as if from
/// outside, where reading puts every source's together).
pub(crate) fn outside<T>(kind: Pim, work: impl FnOnce() -> T) -> T {
    let before = WITHIN.with(|w| w.borrow_mut()[kind as usize].take());
    let _restore = Restore { kind, before };
    work()
}

/// The other sources' ids, in the order they were added.
pub fn extra_sources(store: &Store, kind: Pim) -> Vec<String> {
    store
        .preference(kind.list_key())
        .and_then(|j| serde_json::from_str(&j).ok())
        .unwrap_or_default()
}
/// Every source's id: the first (""), then the others.
pub fn source_ids(store: &Store, kind: Pim) -> Vec<String> {
    let mut ids = vec![String::new()];
    ids.extend(extra_sources(store, kind));
    ids
}

/// Changes to the lists of sources, one at a time.
static LISTS: std::sync::Mutex<()> = std::sync::Mutex::new(());

impl Backend {
    /// Adds a source of `kind`, setting it up by running `work` within it;
    /// a source whose setting up fails isn't kept.
    pub(crate) fn add_source<T>(&self, kind: Pim, work: impl FnOnce() -> Result<T>) -> Result<T> {
        let id = format!("s{}", &uuid::Uuid::new_v4().simple().to_string()[..10]);
        {
            let _lists = LISTS.lock().unwrap_or_else(|e| e.into_inner());
            let mut ids = extra_sources(&self.store, kind);
            ids.push(id.clone());
            self.store.set_preference(kind.list_key(), &serde_json::to_string(&ids)?)?;
        }
        match within(kind, &id, work) {
            Ok(done) => Ok(done),
            Err(e) => {
                let _ = self.forget_source(kind, &id);
                Err(e)
            }
        }
    }
    /// Takes source `id` off the list (its own keys already emptied).
    pub(crate) fn forget_source(&self, kind: Pim, id: &str) -> Result<()> {
        if id.is_empty() {
            return Ok(());
        }
        let _lists = LISTS.lock().unwrap_or_else(|e| e.into_inner());
        let mut ids = extra_sources(&self.store, kind);
        ids.retain(|i| i != id);
        self.store.set_preference(kind.list_key(), &serde_json::to_string(&ids)?)
    }
    /// Empties source `id`'s `keys` (removed for the other sources, as
    /// nothing reads them again; emptied for the first).
    pub(crate) fn clear_source_keys(&self, id: &str, keys: &[&str]) -> Result<()> {
        for base in keys {
            if id.is_empty() {
                self.store.set_preference(base, "")?;
            } else {
                self.store.remove_preference(&key_in(base, id))?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The first source keeps the keys it always had; work within another
    /// uses that one's own, and the one before comes back after, even when
    /// work within another source is nested in it.
    #[test]
    fn work_within_a_source_uses_its_own_keys() {
        assert!(!inside(Pim::Calendar));
        assert_eq!(key(Pim::Calendar, "calendars"), "calendars");
        within(Pim::Calendar, "s1", || {
            assert_eq!(key(Pim::Calendar, "calendars"), "calendars@s1");
            // Contacts aren't worked on within a calendar source.
            assert_eq!(key(Pim::Contacts, "contacts"), "contacts");
            within(Pim::Calendar, "", || assert_eq!(key(Pim::Calendar, "calendars"), "calendars"));
            assert_eq!(current(Pim::Calendar).as_deref(), Some("s1"));
        });
        let _ = std::panic::catch_unwind(|| within(Pim::Calendar, "s2", || panic!("work failed")));
        assert!(!inside(Pim::Calendar), "a panic doesn't leave the source behind");
    }

    #[test]
    fn sources_that_fail_to_set_up_arent_kept() -> Result<()> {
        let path = std::env::temp_dir().join(format!("picacho-sources-{}", uuid::Uuid::new_v4()));
        let b = Backend::new(Arc::new(Store::open(&path)?));
        let kept = b.add_source(Pim::Contacts, || Ok(current(Pim::Contacts).unwrap()))?;
        assert!(b.add_source::<()>(Pim::Contacts, || bail!("refused")).is_err());
        assert_eq!(extra_sources(&b.store, Pim::Contacts), std::slice::from_ref(&kept));
        assert_eq!(source_ids(&b.store, Pim::Contacts), ["".to_owned(), kept]);
        drop(b);
        std::fs::remove_dir_all(path)?;
        Ok(())
    }
}

//! Lists Picacho shares with itself on the account's other computers
//! through the account's own server: what follow-up reminders wait on, and
//! what is snoozed. Each is kept as a private annotation on the inbox where
//! an IMAP server has METADATA (RFC 5464), and otherwise as a note in
//! Picacho's folder (`notes`): the Gmail and Microsoft APIs, Exchange,
//! JMAP, Proton, and IMAP without METADATA.
//!
//! Each entry carries when it last changed (`Stampable::changed`), and an
//! entry removed (a wait stopped, a message back from snooze) leaves a note
//! of when it went (`Gone`), so two computers' lists merge entry by entry:
//! the newer change wins, and an entry removed on one isn't brought back by
//! another's older copy.
//!
//! A list changed here is shared soon; one with something in it (here or
//! on the server) every `SYNC_EVERY`; an empty one once a day, only to
//! notice another computer starting to keep one, and not at all where a
//! note would be needed and there is no Picacho folder.
use super::*;

/// An entry of a shared list.
pub(crate) trait Stampable: Clone + PartialEq + serde::Serialize + serde::de::DeserializeOwned {
    /// What it is about: a message's Message-ID.
    fn key(&self) -> &str;
    /// When it last changed, on any computer (milliseconds since the
    /// epoch); 0 in entries from before changes were dated.
    fn changed(&self) -> i64;
}

/// An entry removed (a wait stopped or answered, a message back from
/// snooze), and when (milliseconds since the epoch): kept a while so
/// another computer's older copy of it doesn't bring it back.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Gone {
    pub message_id: String,
    pub at: i64,
}

/// A shared list: its entries, and those removed lately.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(bound(serialize = "T: serde::Serialize", deserialize = "T: serde::de::DeserializeOwned"))]
pub(crate) struct Stamped<T> {
    #[serde(default = "Vec::new", alias = "waiting")]
    pub entries: Vec<T>,
    #[serde(default)]
    pub gone: Vec<Gone>,
}
impl<T> Default for Stamped<T> {
    fn default() -> Self {
        Self { entries: Vec::new(), gone: Vec::new() }
    }
}

/// How often a list is shared with the server when nothing changed here:
/// another computer may have changed it.
pub(crate) const SYNC_EVERY: i64 = 10 * 60;
/// How soon a list changed here is shared again after a try that failed.
pub(crate) const RETRY_SHARING: i64 = 60;
/// How often an account with an empty list, here and on its server, is
/// looked at for another computer starting to keep one.
pub(crate) const DISCOVER_EVERY: i64 = 86_400;
/// How long a removed entry is remembered.
pub(crate) const GONE_FOR_MS: i64 = 60 * 86_400 * 1000;

pub(crate) fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

impl<T: Stampable> Stamped<T> {
    /// Two computers' lists as one: for each key, whichever change is newer,
    /// the entry or its removal (a removal wins a tie). Removals older than
    /// `GONE_FOR_MS` are forgotten.
    pub(crate) fn merge(self, theirs: Self, now: i64) -> Self {
        let mut entries: HashMap<String, T> = HashMap::new();
        for e in self.entries.into_iter().chain(theirs.entries) {
            match entries.get(e.key()) {
                Some(x) if x.changed() >= e.changed() => {}
                _ => {
                    entries.insert(e.key().to_owned(), e);
                }
            }
        }
        let mut gone: HashMap<String, i64> = HashMap::new();
        for g in self.gone.into_iter().chain(theirs.gone) {
            let at = gone.entry(g.message_id).or_insert(g.at);
            *at = (*at).max(g.at);
        }
        entries.retain(|id, e| gone.get(id).is_none_or(|at| *at < e.changed()));
        gone.retain(|id, at| *at > now - GONE_FOR_MS && !entries.contains_key(id));
        let mut out = Self {
            entries: entries.into_values().collect(),
            gone: gone.into_iter().map(|(message_id, at)| Gone { message_id, at }).collect(),
        };
        out.normalize();
        out
    }
    /// In a set order, so equal lists compare equal.
    pub(crate) fn normalize(&mut self) {
        self.entries.sort_by(|a, b| a.key().cmp(b.key()));
        self.gone.sort_by(|a, b| a.message_id.cmp(&b.message_id));
    }
    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty() && self.gone.is_empty()
    }
    /// A fingerprint, to tell whether it changed since it was shared.
    pub(crate) fn fingerprint(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        serde_json::to_string(self).unwrap_or_default().hash(&mut h);
        h.finish()
    }
}

/// When this computer last shared a list with the server: a fingerprint of
/// the list as it was then (changed here since, it's shared again; not
/// times, which another computer's clock may set ahead), when it last
/// tried (seconds since the epoch), and whether the server held a list.
#[derive(Clone, Copy, Debug, Default, serde::Serialize, serde::Deserialize)]
pub(crate) struct SyncState {
    #[serde(default)]
    pub shared: u64,
    pub at: i64,
    #[serde(default)]
    pub remote: bool,
}

/// A kind of shared list: where it is kept, and how it is read and written
/// here.
pub(crate) struct ListKind<T: 'static> {
    /// The kind of note it's kept in without METADATA.
    pub note: &'static str,
    /// The METADATA entry it's kept in.
    pub entry: &'static str,
    /// Where earlier versions kept the entries alone, as an array, under
    /// `entry`: the removals go under this entry, so they still read it.
    pub gone_entry: Option<&'static str>,
    /// The preference its `SyncState` is kept in, for an account.
    pub state_key: fn(&str) -> String,
    /// The list here, and writing it here (merged).
    pub load: fn(&Backend, &str) -> Stamped<T>,
    pub store: fn(&Backend, &str, &Stamped<T>) -> Result<()>,
    /// Held while the list here is read, merged and written back.
    pub guard: &'static Mutex<()>,
    /// Whether sharing takes the account's lock: always, or where reaching
    /// the account does (one IMAP connection or Proton session at a time).
    pub always_lock: bool,
}

impl Backend {
    /// Whether an account can share lists: any but POP3 (an IMAP one, once
    /// its server is set up).
    pub(crate) fn shares_lists(a: &Account) -> bool {
        a.provider != Provider::Pop3 && (!a.imap() || !a.incoming_host.is_empty())
    }
    /// Whether the account has a Picacho folder, as last listed here: where
    /// notes would be.
    pub(crate) fn has_picacho_folder(&self, account: &str) -> bool {
        let parent = super::snooze::PARENT;
        self.store.folders(account).iter().any(|f| {
            f.label.eq_ignore_ascii_case(parent)
                && (f.depth == 0 || f.name.eq_ignore_ascii_case(&format!("Folders/{parent}")))
        })
    }
    fn list_state<T>(&self, account: &str, kind: &ListKind<T>) -> SyncState {
        self.store
            .preference(&(kind.state_key)(account))
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }
    fn save_list_state<T>(&self, account: &str, kind: &ListKind<T>, state: SyncState) -> Result<()> {
        self.store.set_preference(&(kind.state_key)(account), &serde_json::to_string(&state)?)
    }
    /// How long until a list is shared again (see the module's notes).
    pub(crate) fn list_sync_wait<T: Stampable>(&self, account: &str, kind: &ListKind<T>, state: &SyncState) -> i64 {
        let ours = (kind.load)(self, account);
        if ours.fingerprint() != state.shared {
            RETRY_SHARING
        } else if !ours.is_empty() || state.remote {
            SYNC_EVERY
        } else {
            DISCOVER_EVERY
        }
    }
    /// Shares a list with the account's server, merging in what other
    /// computers changed there: when it's time (`list_sync_wait`), or now
    /// with `force`. Returns whether the list here changed. Skipped while
    /// the account is busy (a long download holds it), rather than holding
    /// up the caller.
    pub(crate) fn sync_list<T: Stampable>(&self, a: &Account, kind: &ListKind<T>, force: bool) -> Result<bool> {
        if !Self::shares_lists(a) {
            return Ok(false);
        }
        let state = self.list_state(&a.id, kind);
        let now = chrono::Utc::now().timestamp();
        if !force && now - state.at < self.list_sync_wait(&a.id, kind, &state) {
            return Ok(false);
        }
        let lock = self.account_lock(&a.id);
        let _operation = if kind.always_lock || a.imap() || a.provider == Provider::Proton {
            match lock.try_lock() {
                Ok(guard) => Some(guard),
                Err(_) => return Ok(false),
            }
        } else {
            None
        };
        // Tried now, whatever happens: a server that's away isn't asked
        // again at every look.
        self.save_list_state(&a.id, kind, SyncState { at: now, ..state })?;
        let metadata = a.imap() && self.with_imap(a, |c| Ok(c.extensions()?.metadata))?;
        let (mut theirs, notes) = if metadata {
            let theirs = self.with_imap(a, |c| read_metadata(c, kind))?;
            (theirs, Vec::new())
        } else if !force && (kind.load)(self, &a.id).is_empty() && !state.remote && !self.has_picacho_folder(&a.id) {
            // Nothing here, and no Picacho folder a note could be in.
            let shared = (kind.load)(self, &a.id).fingerprint();
            self.save_list_state(&a.id, kind, SyncState { shared, at: now, remote: false })?;
            return Ok(false);
        } else {
            // Every note found, merged (two computers may each have left one).
            let mut theirs = Stamped::default();
            let mut ids = Vec::new();
            for (id, data) in self.read_notes(a, kind.note)?.unwrap_or_default() {
                if let Ok(note) = serde_json::from_str::<Stamped<T>>(&data) {
                    theirs = theirs.merge(note, now_ms());
                }
                ids.push(id);
            }
            (theirs, ids)
        };
        theirs.normalize();
        let (changed, merged) = {
            let _held = kind.guard.lock().unwrap_or_else(|e| e.into_inner());
            let mut ours = (kind.load)(self, &a.id);
            ours.normalize();
            let merged = ours.clone().merge(theirs.clone(), now_ms());
            let changed = merged != ours;
            if changed {
                (kind.store)(self, &a.id, &merged)?;
            }
            (changed, merged)
        };
        if metadata {
            if merged != theirs {
                self.with_imap(a, |c| write_metadata(c, kind, &merged))?;
            }
        } else if (merged != theirs || notes.len() > 1) && !(merged.is_empty() && notes.is_empty()) {
            // A new note, then the ones it replaces; no folder is made
            // while there's nothing to keep.
            self.write_note(a, kind.note, &serde_json::to_string(&merged)?, &notes)?;
        }
        self.save_list_state(
            &a.id,
            kind,
            SyncState { shared: merged.fingerprint(), at: now, remote: !merged.is_empty() },
        )?;
        Ok(changed)
    }
}

/// A list as a server with METADATA keeps it.
pub(super) fn read_metadata<T: Stampable>(c: &mut Connection, kind: &ListKind<T>) -> Result<Stamped<T>> {
    let entries: Vec<&str> = std::iter::once(kind.entry).chain(kind.gone_entry).collect();
    let values = get_metadata(&mut c.session, "INBOX", &entries)?;
    let value = |entry: &str| {
        values
            .iter()
            .find(|(e, _)| e.eq_ignore_ascii_case(entry))
            .and_then(|(_, v)| v.clone())
    };
    Ok(match kind.gone_entry {
        // Entries as an array, as earlier versions wrote them; removals apart.
        Some(gone) => Stamped {
            entries: value(kind.entry).and_then(|v| serde_json::from_str(&v).ok()).unwrap_or_default(),
            gone: value(gone).and_then(|v| serde_json::from_str(&v).ok()).unwrap_or_default(),
        },
        None => value(kind.entry).and_then(|v| serde_json::from_str(&v).ok()).unwrap_or_default(),
    })
}

/// Writes a list as a server with METADATA keeps it (removing what's empty).
pub(super) fn write_metadata<T: Stampable>(c: &mut Connection, kind: &ListKind<T>, list: &Stamped<T>) -> Result<()> {
    match kind.gone_entry {
        Some(gone) => {
            let entries = serde_json::to_string(&list.entries)?;
            let removed = serde_json::to_string(&list.gone)?;
            set_metadata(&mut c.session, "INBOX", kind.entry, (!list.entries.is_empty()).then_some(entries.as_str()))?;
            set_metadata(&mut c.session, "INBOX", gone, (!list.gone.is_empty()).then_some(removed.as_str()))?;
        }
        None => {
            let json = serde_json::to_string(list)?;
            set_metadata(&mut c.session, "INBOX", kind.entry, (!list.is_empty()).then_some(json.as_str()))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    struct E {
        id: String,
        changed: i64,
    }
    impl Stampable for E {
        fn key(&self) -> &str {
            &self.id
        }
        fn changed(&self) -> i64 {
            self.changed
        }
    }
    fn list(entries: &[(&str, i64)], gone: &[(&str, i64)]) -> Stamped<E> {
        Stamped {
            entries: entries.iter().map(|(id, changed)| E { id: (*id).into(), changed: *changed }).collect(),
            gone: gone.iter().map(|(id, at)| Gone { message_id: (*id).into(), at: *at }).collect(),
        }
    }

    #[test]
    fn two_computers_lists_merge_by_the_newer_change() {
        let now = 1_800_000_000_000;
        let ours = list(&[("<a>", now - 50), ("<b>", now - 50), ("<c>", now - 10)], &[("<d>", now - 5)]);
        let theirs = list(
            &[("<a>", now - 20), ("<d>", now - 30), ("<e>", now - 1)],
            // Removed there after it was set here; and one removed long ago.
            &[("<b>", now - 40), ("<c>", now - 60), ("<old>", now - GONE_FOR_MS - 1)],
        );
        let merged = ours.clone().merge(theirs.clone(), now);
        let entries: Vec<(&str, i64)> = merged.entries.iter().map(|e| (e.id.as_str(), e.changed)).collect();
        // <a>: their newer change; <b>: removed there; <c>: set again here
        // after it was removed there; <d>: removed here after it was set there.
        assert_eq!(entries, [("<a>", now - 20), ("<c>", now - 10), ("<e>", now - 1)]);
        let gone: Vec<&str> = merged.gone.iter().map(|g| g.message_id.as_str()).collect();
        assert_eq!(gone, ["<b>", "<d>"]);
        // Either way round, the same; and merging again changes nothing.
        assert_eq!(theirs.merge(ours, now), merged);
        assert_eq!(merged.clone().merge(merged.clone(), now), merged);
        assert_ne!(merged.fingerprint(), Stamped::<E>::default().fingerprint());
        // Lists written before the entries were called that read too.
        let old: Stamped<E> = serde_json::from_str(r#"{"waiting":[{"id":"<a>","changed":1}]}"#).unwrap();
        assert_eq!(old.entries.len(), 1);
    }
}

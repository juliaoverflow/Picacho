//! Mail in the system's search: Spotlight on macOS and iOS, Windows Search
//! on Windows, GNOME's and KDE Plasma's searches on Linux (Settings →
//! Privacy, off until turned on).
//!
//! Linux's searches ask apps as one types (`desktop_search`): Picacho
//! answers from its own full-text index, while it runs, and gives nothing
//! away ahead. Spotlight and Windows Search keep indexes of their own,
//! which are given copies:
//!
//! The store's triggers queue a message whenever it is added, removed,
//! moved, or its words change, while this is on (`Store::system_search_items`).
//! One worker thread per process gives the system's search what is queued, a
//! batch at a time, and records what it was given (`system_search` table),
//! so each message is handed over once, and again only when it changes.
//!
//! - macOS and iOS (`spotlight`): Core Spotlight items in Picacho's own
//!   index, each named by the message's `picacho://widget/message` link.
//!   Choosing one in Spotlight opens Picacho on it (on macOS
//!   `spotlight::handle_activities`; on iOS the scene, packaging/ios/main.m).
//! - Windows (`windows_search`): one small `.eml` file per message (its
//!   headers and text, no attachments) under the local, not roaming,
//!   application data folder, which is added to Windows Search's indexed
//!   locations. Windows indexes `.eml` files with its own MIME filter.
//!   Opened with Picacho, such a file opens the message it stands for
//!   (`link_in_file`).
//!
//! Mail never goes to the system's search while it is hidden outside
//! Picacho, nor copies of it while the database is encrypted: turning
//! either on takes it back out (`allowed`, `set_enabled`).
use super::store::{Store, SystemSearchMessage, SYSTEM_SEARCH_PREFERENCE, SYSTEM_SEARCH_PUSH_PREFERENCE};
use anyhow::Result;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
pub mod desktop_search;
#[cfg(any(target_os = "macos", target_os = "ios"))]
pub mod spotlight;
#[cfg(windows)]
pub mod windows_search;

/// Whether this system has a search Picacho can show mail in.
pub const AVAILABLE: bool = cfg!(any(
    target_os = "macos",
    target_os = "ios",
    windows,
    target_os = "linux",
    target_os = "freebsd"
));
/// Whether that search is given copies of messages to index (Spotlight,
/// Windows Search), rather than asking Picacho (Linux).
pub const PUSHES: bool = cfg!(any(target_os = "macos", target_os = "ios", windows));

/// Messages handed over at a time.
const BATCH: usize = 100;
/// How often the worker looks for queued messages without being told.
const LOOK_EVERY: Duration = Duration::from_secs(30);
/// The header of the `.eml` files Windows Search indexes that names the
/// message each stands for.
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
pub const LINK_HEADER: &str = "X-Picacho-Link";

/// Whether mail goes to the system's search (as set).
pub fn enabled(store: &Store) -> bool {
    AVAILABLE && store.preference(SYSTEM_SEARCH_PREFERENCE).as_deref() == Some("true")
}

/// Whether it may be turned on: not while mail is hidden outside Picacho
/// (notifications and widgets say only how much came), nor, where the
/// system keeps copies in an index of its own, while the database is
/// encrypted.
pub fn allowed(store: &Store) -> bool {
    AVAILABLE && !crate::manage::applock::hides_contents(store) && !(PUSHES && store.encrypted())
}

/// Turns mail in the system's search on (where it is given copies, every
/// message is queued) or off (the worker takes everything back out).
pub fn set_enabled(store: &Store, on: bool) -> Result<()> {
    if on {
        if !allowed(store) {
            anyhow::bail!(t!("manage-system-search-isnt-allowed"));
        }
        store.set_preference(SYSTEM_SEARCH_PREFERENCE, "true")?;
        if PUSHES {
            store.set_preference(SYSTEM_SEARCH_PUSH_PREFERENCE, "true")?;
            store.system_search_queue_all()?;
        }
    } else {
        store.remove_preference(SYSTEM_SEARCH_PREFERENCE)?;
        store.remove_preference(SYSTEM_SEARCH_PUSH_PREFERENCE)?;
    }
    set_status(|s| s.error = None);
    poke();
    Ok(())
}

/// What the worker last did, for Settings.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Status {
    /// Messages still to hand over.
    pub waiting: usize,
    /// Why the last batch couldn't be handed over.
    pub error: Option<String>,
}

static STATUS: Mutex<Status> = Mutex::new(Status { waiting: 0, error: None });

/// What the worker last did.
pub fn status() -> Status {
    STATUS.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

fn set_status(change: impl FnOnce(&mut Status)) {
    change(&mut STATUS.lock().unwrap_or_else(|e| e.into_inner()));
}

/// Wakes the worker: set when there is something for it.
static WAKE: (Mutex<bool>, Condvar) = (Mutex::new(false), Condvar::new());

/// Tells the worker to look at the queue now.
pub fn poke() {
    let (lock, wake) = &WAKE;
    *lock.lock().unwrap_or_else(|e| e.into_inner()) = true;
    wake.notify_all();
}

/// Starts what keeps the system's search up to date for `store`, once per
/// process (later calls do nothing): the worker giving it copies, or on
/// Linux the D-Bus service it asks, which opens a result chosen with `open`
/// (the message's link).
#[cfg_attr(not(any(target_os = "linux", target_os = "freebsd")), allow(unused_variables))]
pub fn start(store: Arc<Store>, open: fn(String)) {
    if !AVAILABLE {
        return;
    }
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    desktop_search::serve(store.clone(), open);
    if !PUSHES {
        return;
    }
    static STARTED: std::sync::Once = std::sync::Once::new();
    STARTED.call_once(move || {
        #[cfg(windows)]
        windows_search::init(&store.path);
        let started = std::thread::Builder::new()
            .name("system-search".into())
            .spawn(move || work(&store));
        if let Err(e) = started {
            eprintln!("Picacho: couldn't start adding mail to the system's search: {e}");
        }
    });
}

fn work(store: &Store) {
    #[cfg(windows)]
    let _apartment = windows_search::Apartment::new();
    loop {
        // A panic (a system API misbehaving) ends this round, not the app.
        let round = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run_round(store)));
        match round {
            Ok(Ok(())) => set_status(|s| s.error = platform::warning()),
            Ok(Err(e)) => {
                eprintln!("Picacho: the system's search wasn't brought up to date: {e:#}");
                set_status(|s| s.error = Some(format!("{e:#}")));
            }
            Err(_) => set_status(|s| s.error = Some(t!("manage-system-search-stopped").to_owned())),
        }
        let (lock, wake) = &WAKE;
        let mut poked = lock.lock().unwrap_or_else(|e| e.into_inner());
        if !*poked {
            poked = wake
                .wait_timeout(poked, LOOK_EVERY)
                .map(|(g, _)| g)
                .unwrap_or_else(|e| e.into_inner().0);
        }
        *poked = false;
    }
}

/// Brings the system's search up to date with the queue, or empties it
/// when turned off.
fn run_round(store: &Store) -> Result<()> {
    if !enabled(store) || !allowed(store) {
        // Turned off, or no longer allowed (mail hidden outside Picacho,
        // the database encrypted): everything comes back out, and stays
        // out until turned on again.
        if enabled(store) {
            store.remove_preference(SYSTEM_SEARCH_PREFERENCE)?;
        }
        if store.preference(SYSTEM_SEARCH_PUSH_PREFERENCE).is_some() {
            store.remove_preference(SYSTEM_SEARCH_PUSH_PREFERENCE)?;
        }
        let listed = store.system_search_listed()?;
        if !listed.is_empty() || store.system_search_waiting()? > 0 || platform::has_leftovers() {
            platform::clear()?;
            store.system_search_forget()?;
        }
        set_status(|s| s.waiting = 0);
        return Ok(());
    }
    // Turned on before messages were queued for it (an earlier build): every
    // message is, now.
    if store.preference(SYSTEM_SEARCH_PUSH_PREFERENCE).as_deref() != Some("true") {
        store.set_preference(SYSTEM_SEARCH_PUSH_PREFERENCE, "true")?;
        store.system_search_queue_all()?;
    }
    // Each round, also with nothing queued: the folder may not be in
    // Windows Search's indexed locations yet (and Spotlight may be
    // unavailable), which Settings should say.
    platform::prepare()?;
    let waiting = store.system_search_waiting()?;
    set_status(|s| s.waiting = waiting);
    if waiting == 0 {
        return Ok(());
    }
    loop {
        // Turned off meanwhile: the next round takes everything out.
        if !enabled(store) || !allowed(store) {
            return Ok(());
        }
        let items = store.system_search_items(BATCH)?;
        if items.is_empty() {
            break;
        }
        let changes: Vec<Change> = items
            .into_iter()
            .map(|item| {
                let entry = item.now.filter(|m| listed_mailbox(&m.mailbox)).map(|m| Entry {
                    link: link(&m.account, &item.id),
                    id: item.id.clone(),
                    message: m,
                });
                Change {
                    id: item.id,
                    queued: item.queued,
                    entry,
                    listed: item.listed,
                }
            })
            .collect();
        let names = platform::apply(&changes)?;
        let done: Vec<(String, i64, Option<(String, String)>)> = changes
            .into_iter()
            .zip(names)
            .map(|(c, name)| {
                let listed = match (c.entry, name) {
                    (Some(e), Some(name)) => Some((e.message.account, name)),
                    _ => None,
                };
                (c.id, c.queued, listed)
            })
            .collect();
        store.system_search_done(&done)?;
        let waiting = store.system_search_waiting()?;
        set_status(|s| s.waiting = waiting);
        // The cache and the system's indexer get room in between.
        std::thread::sleep(Duration::from_millis(50));
    }
    Ok(())
}

/// Whether mail in this mailbox (its key) goes to the system's search: not
/// junk, and not what was thrown away.
pub fn listed_mailbox(key: &str) -> bool {
    use crate::manage::model::Folder;
    key != Folder::Junk.key() && key != Folder::Trash.key()
}

/// The link that opens message `id` of `account` in Picacho.
pub fn link(account: &str, id: &str) -> String {
    crate::manage::widgets::WidgetAction::Message {
        account: account.to_owned(),
        id: id.to_owned(),
    }
    .to_uri()
}

/// A message to put in the system's search.
#[cfg_attr(not(any(target_os = "macos", target_os = "ios", windows)), allow(dead_code))]
pub struct Entry {
    /// Its `picacho:` link (`link`).
    pub link: String,
    pub id: String,
    pub message: SystemSearchMessage,
}

/// What the system's search should have of one queued message.
#[cfg_attr(not(any(target_os = "macos", target_os = "ios", windows)), allow(dead_code))]
pub struct Change {
    pub id: String,
    queued: i64,
    /// The message to list; `None` to take it out.
    pub entry: Option<Entry>,
    /// What it was listed as before: its account and name.
    pub listed: Option<(String, String)>,
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
use spotlight as platform;
#[cfg(windows)]
use windows_search as platform;

/// Where the system isn't given copies, nothing is ever listed.
#[cfg(not(any(target_os = "macos", target_os = "ios", windows)))]
mod platform {
    use super::Change;
    use anyhow::Result;
    pub fn prepare() -> Result<()> {
        Ok(())
    }
    pub fn apply(changes: &[Change]) -> Result<Vec<Option<String>>> {
        Ok(vec![None; changes.len()])
    }
    pub fn clear() -> Result<()> {
        Ok(())
    }
    pub fn has_leftovers() -> bool {
        false
    }
    pub fn warning() -> Option<String> {
        None
    }
}

/// The message an email file stands for, when it is one Picacho wrote for
/// Windows Search (in its folder, `windows_search::folder`): its link.
/// Elsewhere, and for any other file, `None`: the file opens as itself.
pub fn link_for_file(path: &std::path::Path) -> Option<String> {
    #[cfg(windows)]
    {
        use std::io::Read as _;
        let folder = std::fs::canonicalize(windows_search::folder()?).ok()?;
        let path = std::fs::canonicalize(path).ok()?;
        if !path.starts_with(&folder) {
            return None;
        }
        let mut head = Vec::new();
        std::fs::File::open(&path).ok()?.take(16 * 1024).read_to_end(&mut head).ok()?;
        link_in(&head)
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        None
    }
}

/// The name a sender goes by in the system's search: "Ana <ana@x>", or
/// the address alone.
#[cfg_attr(not(any(target_os = "macos", target_os = "ios", target_os = "linux", target_os = "freebsd", test)), allow(dead_code))]
fn person(name: &str, address: &str) -> String {
    let name = name.trim();
    let address = address.trim();
    if name.is_empty() || name == address {
        address.to_owned()
    } else if address.is_empty() {
        name.to_owned()
    } else {
        format!("{name} <{address}>")
    }
}

/// Separates a header's people ("Ana <ana@x>, bo@y") into names and addresses.
#[cfg_attr(not(any(target_os = "macos", target_os = "ios", test)), allow(dead_code))]
fn people(header: &str) -> (Vec<String>, Vec<String>) {
    let mut names = Vec::new();
    let mut addresses = Vec::new();
    for part in header.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        match part.rsplit_once('<') {
            Some((name, address)) => {
                let name = name.trim().trim_matches('"').trim();
                let address = address.trim_end_matches('>').trim();
                if !name.is_empty() {
                    names.push(name.to_owned());
                }
                if !address.is_empty() {
                    addresses.push(address.to_owned());
                }
            }
            None if part.contains('@') => addresses.push(part.to_owned()),
            None => names.push(part.to_owned()),
        }
    }
    (names, addresses)
}

/// A message as a small email file (RFC 5322): its headers, a link back
/// to it in Picacho (`LINK_HEADER`), and its text as UTF-8 plain text. No
/// attachments, no HTML: what Windows Search's MIME filter indexes.
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
pub fn eml(entry: &Entry) -> Vec<u8> {
    use crate::sync::mime::encoded_words;
    let m = &entry.message;
    let mut out = String::with_capacity(m.text.len() + 512);
    let mut header = |name: &str, value: &str| {
        let value = value.replace(['\r', '\n'], " ");
        if !value.trim().is_empty() {
            out.push_str(name);
            out.push_str(": ");
            out.push_str(&value);
            out.push_str("\r\n");
        }
    };
    let from = match (m.sender.trim(), m.from.trim()) {
        (name, address) if !name.is_empty() && name != address => {
            format!("{} <{address}>", encoded_words(name).replace(['"', '\\'], ""))
        }
        (_, address) => address.to_owned(),
    };
    header("From", &from);
    header("To", &encoded_words(&m.to));
    header("Cc", &encoded_words(&m.cc));
    header("Subject", &encoded_words(&m.subject));
    if let Some(date) = chrono::DateTime::from_timestamp(m.date, 0) {
        header("Date", &date.to_rfc2822());
    }
    // Only a Message-ID in its brackets, without spaces: anything else
    // would be read as more headers or garbled.
    if m.message_id.starts_with('<') && m.message_id.ends_with('>') && !m.message_id.contains(char::is_whitespace) {
        header("Message-ID", &m.message_id);
    }
    header(LINK_HEADER, &entry.link);
    header("MIME-Version", "1.0");
    header("Content-Type", "text/plain; charset=utf-8");
    header("Content-Transfer-Encoding", "8bit");
    out.push_str("\r\n");
    // Lines end in CRLF, and none is "." alone or overly long for MIME
    // readers that count (998 bytes): wrapped between characters.
    for line in m.text.lines() {
        let mut rest = line;
        loop {
            let mut cut = rest.len().min(900);
            while !rest.is_char_boundary(cut) {
                cut -= 1;
            }
            out.push_str(&rest[..cut]);
            out.push_str("\r\n");
            rest = &rest[cut..];
            if rest.is_empty() {
                break;
            }
        }
    }
    out.into_bytes()
}

/// The link back to a message in an email file Picacho wrote for Windows
/// Search (`eml`): its `LINK_HEADER`, when it is a link to a message.
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
pub fn link_in(bytes: &[u8]) -> Option<String> {
    let head = bytes.split(|&b| b == b'\n').take_while(|l| !l.iter().all(|b| b.is_ascii_whitespace()));
    for line in head.take(64) {
        let line = std::str::from_utf8(line).ok()?.trim_end();
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if name.eq_ignore_ascii_case(LINK_HEADER) {
            let value = value.trim();
            return matches!(
                crate::manage::widgets::WidgetAction::parse(value),
                Some(crate::manage::widgets::WidgetAction::Message { .. })
            )
            .then(|| value.to_owned());
        }
    }
    None
}

/// The file name a message is given in Windows Search's folder: its
/// subject (what search results show), made safe, and a short tag of its
/// id, so two messages of one subject differ.
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
pub fn file_name(entry: &Entry) -> String {
    use sha2::Digest as _;
    let digest = sha2::Sha256::digest(entry.id.as_bytes());
    let tag: String = digest[..5].iter().map(|b| format!("{b:02x}")).collect();
    let subject = entry.message.subject.trim();
    let subject = if subject.is_empty() { t!("manage-system-search-no-subject") } else { subject };
    // Short enough for a path well under Windows' 260 characters.
    // Folders' separators first: `safe_file_name` would keep only what
    // follows the last.
    let short: String = subject.chars().take(80).map(|c| if matches!(c, '/' | '\\' | '%') { '_' } else { c }).collect();
    let safe = crate::manage::backend::opened::safe_file_name(&short);
    format!("{safe} ({tag}).eml")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry() -> Entry {
        Entry {
            link: link("acct-1", "acct-1:INBOX:7"),
            id: "acct-1:INBOX:7".into(),
            message: SystemSearchMessage {
                account: "acct-1".into(),
                mailbox: "inbox".into(),
                date: 1_700_000_000,
                subject: "Plans for Friday / Café?".into(),
                from: "ana@example.org".into(),
                sender: "Ana Núñez".into(),
                to: "bo@example.org".into(),
                cc: String::new(),
                text: "Lunch?\nSee you.\n.\nend".into(),
                message_id: "<m1@example.org>".into(),
            },
        }
    }

    #[test]
    fn email_files_carry_headers_text_and_the_link_back() {
        let e = entry();
        let bytes = eml(&e);
        let text = String::from_utf8(bytes.clone()).unwrap();
        assert!(text.starts_with("From: =?UTF-8?B?"), "{text}");
        assert!(text.contains("<ana@example.org>\r\n"));
        assert!(text.contains("To: bo@example.org\r\n"));
        assert!(!text.contains("Cc:"), "empty headers are left out");
        assert!(text.contains("Subject: =?UTF-8?B?"));
        assert!(text.contains("Message-ID: <m1@example.org>\r\n"));
        assert!(text.contains("Content-Type: text/plain; charset=utf-8\r\n"));
        assert!(text.ends_with("\r\n\r\nLunch?\r\nSee you.\r\n.\r\nend\r\n"));
        // The file reads back as the message it was made from.
        use mailparse::MailHeaderMap as _;
        let parsed = mailparse::parse_mail(&bytes).unwrap();
        assert_eq!(parsed.headers.get_first_value("Subject").as_deref(), Some("Plans for Friday / Café?"));
        assert_eq!(parsed.headers.get_first_value("From").as_deref(), Some("Ana Núñez <ana@example.org>"));
        assert_eq!(parsed.get_body().unwrap().replace("\r\n", "\n").trim_end(), "Lunch?\nSee you.\n.\nend");
        assert_eq!(link_in(&bytes).as_deref(), Some(e.link.as_str()));
    }

    #[test]
    fn headers_cant_be_added_through_a_message() {
        let mut e = entry();
        e.message.subject = "Hi\r\nX-Picacho-Link: picacho://widget/compose".into();
        e.message.message_id = "<a@b>\r\nBcc: x".into();
        let bytes = eml(&e);
        let text = String::from_utf8(bytes.clone()).unwrap();
        let head = text.split("\r\n\r\n").next().unwrap();
        assert!(!head.lines().any(|l| l.starts_with("Bcc")), "{head}");
        assert_eq!(head.lines().filter(|l| l.starts_with("X-Picacho-Link:")).count(), 1, "{head}");
        assert_eq!(link_in(&bytes).as_deref(), Some(e.link.as_str()));
        // Only a message's link is taken from a file.
        assert_eq!(link_in(b"X-Picacho-Link: picacho://widget/compose\r\n\r\n"), None);
        assert_eq!(link_in(b"Subject: x\r\n\r\nX-Picacho-Link: picacho://widget/message?account=a&id=b\r\n"), None);
    }

    #[test]
    fn long_lines_are_wrapped_between_characters() {
        let mut e = entry();
        e.message.text = "é".repeat(1000);
        let text = String::from_utf8(eml(&e)).unwrap();
        let body = text.split("\r\n\r\n").nth(1).unwrap();
        assert!(body.split("\r\n").all(|l| l.len() <= 900));
        assert_eq!(body.replace("\r\n", ""), "é".repeat(1000));
    }

    #[test]
    fn file_names_are_safe_and_differ_by_message() {
        let mut e = entry();
        let a = file_name(&e);
        assert!(a.starts_with("Plans for Friday _ Café_ ("), "{a}");
        assert!(a.ends_with(").eml"));
        e.id = "acct-1:INBOX:8".into();
        assert_ne!(file_name(&e), a);
        e.message.subject = "  ".into();
        assert!(file_name(&e).ends_with(").eml"));
        e.message.subject = "CON".into();
        assert!(!file_name(&e).starts_with("CON "));
        e.message.subject = "x".repeat(500);
        assert!(file_name(&e).chars().count() < 100);
    }

    /// Windows Search's files: written, renamed with their subject, read back
    /// as links when opened, removed, and the folder cleared. Where Windows
    /// Search can't take the folder (no service, as under Wine), that's a
    /// warning, and the files are kept all the same.
    #[cfg(windows)]
    #[test]
    fn windows_search_files_follow_their_messages() -> Result<()> {
        let store_dir = std::env::temp_dir().join(format!("picacho-ws-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&store_dir)?;
        windows_search::init(&store_dir);
        let folder = windows_search::folder().unwrap().to_owned();
        assert!(folder.starts_with(directories::ProjectDirs::from("tech", "pinal", "picacho").unwrap().data_local_dir()));
        let _apartment = windows_search::Apartment::new();
        windows_search::prepare()?;
        assert!(folder.is_dir());
        let put = |e: Entry, listed: Option<(String, String)>| Change { id: e.id.clone(), queued: 1, entry: Some(e), listed };
        let names = windows_search::apply(&[put(entry(), None)])?;
        let first = names[0].clone().unwrap();
        let path = folder.join(&first);
        assert!(path.is_file());
        assert_eq!(link_for_file(&path).as_deref(), Some(entry().link.as_str()));
        // Dated as the message, for search results' order.
        let modified = std::fs::metadata(&path)?.modified()?;
        assert_eq!(modified.duration_since(std::time::UNIX_EPOCH)?.as_secs(), 1_700_000_000);
        // An email file elsewhere opens as itself.
        let elsewhere = store_dir.join("note.eml");
        std::fs::copy(&path, &elsewhere)?;
        assert_eq!(link_for_file(&elsewhere), None);
        // A new subject is a new name; the old file goes.
        let mut renamed = entry();
        renamed.message.subject = "Moved on".into();
        let names = windows_search::apply(&[put(renamed, Some(("acct-1".into(), first.clone())))])?;
        let second = names[0].clone().unwrap();
        assert_ne!(second, first);
        assert!(!path.exists() && folder.join(&second).is_file());
        // Gone (or moved to the trash): no file.
        let gone = Change { id: entry().id, queued: 2, entry: None, listed: Some(("acct-1".into(), second.clone())) };
        assert_eq!(windows_search::apply(&[gone])?, [None]);
        assert!(!folder.join(&second).exists());
        let _ = windows_search::warning();
        windows_search::clear()?;
        assert!(!folder.exists() && !windows_search::has_leftovers());
        let _ = std::fs::remove_dir_all(&store_dir);
        Ok(())
    }

    #[test]
    fn junk_and_trash_stay_out() {
        assert!(listed_mailbox("inbox"));
        assert!(listed_mailbox("archive"));
        assert!(!listed_mailbox("junk"));
        assert!(!listed_mailbox("trash"));
        assert_eq!(person("Ana", "ana@x"), "Ana <ana@x>");
        assert_eq!(person("", "ana@x"), "ana@x");
        assert_eq!(person("ana@x", "ana@x"), "ana@x");
        let (names, addresses) = people("\"Ana N\" <ana@x.org>, bo@y.org, Team");
        assert_eq!(names, ["Ana N", "Team"]);
        assert_eq!(addresses, ["ana@x.org", "bo@y.org"]);
    }

    /// The store's triggers queue what changes, only while this is on, and
    /// what was handed over is recorded once.
    #[test]
    fn the_store_queues_changes_while_on() -> Result<()> {
        use crate::manage::model::{Folder, Mail};
        let dir = std::env::temp_dir().join(format!("picacho-system-search-{}", uuid::Uuid::new_v4()));
        let store = Store::open(&dir)?;
        let mail = |id: &str, subject: &str| Mail {
            id: id.into(),
            account: "a".into(),
            folder: Folder::Inbox,
            subject: subject.into(),
            from: "ana@example.org".into(),
            body: Some("Hello there".into()),
            date: 1_700_000_000,
            ..Mail::default()
        };
        store.save_messages(&[mail("m1", "First")])?;
        assert_eq!(store.system_search_waiting()?, 0, "nothing is queued while off");
        store.set_preference(SYSTEM_SEARCH_PUSH_PREFERENCE, "true")?;
        store.system_search_queue_all()?;
        let items = store.system_search_items(10)?;
        assert_eq!(items.len(), 1);
        let now = items[0].now.clone().unwrap();
        assert_eq!((now.subject.as_str(), now.text.as_str(), now.mailbox.as_str()), ("First", "Hello there", "inbox"));
        store.system_search_done(&[("m1".into(), items[0].queued, Some(("a".into(), "First.eml".into())))])?;
        assert_eq!(store.system_search_waiting()?, 0);
        // Flags change nothing it lists; words do.
        let mut read = mail("m1", "First");
        read.unread = true;
        store.save_messages(&[read])?;
        assert_eq!(store.system_search_waiting()?, 0, "a flag isn't a change for it");
        store.save_messages(&[mail("m1", "First, again"), mail("m2", "Second")])?;
        let items = store.system_search_items(10)?;
        assert_eq!(items.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(), ["m1", "m2"]);
        assert_eq!(items[0].listed, Some(("a".into(), "First.eml".into())));
        // Queued again while being handed over: stays queued.
        store.save_messages(&[mail("m2", "Second, edited")])?;
        store.system_search_done(&items.iter().map(|i| (i.id.clone(), i.queued, None)).collect::<Vec<_>>())?;
        let left = store.system_search_items(10)?;
        assert_eq!(left.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(), ["m2"]);
        assert!(store.system_search_listed()?.is_empty());
        store.system_search_forget()?;
        // Off: nothing more is queued.
        store.remove_preference(SYSTEM_SEARCH_PUSH_PREFERENCE)?;
        store.save_messages(&[mail("m3", "Third")])?;
        assert_eq!(store.system_search_waiting()?, 0);
        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    }
}

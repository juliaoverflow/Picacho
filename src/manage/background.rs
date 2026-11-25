//! Checking for new mail while Picacho has no window: on a desktop once the
//! last window closes (when Picacho keeps running in the background), and on
//! Android once the activity is gone while the mail service keeps the app's
//! process alive. iOS doesn't let apps run on: it wakes Picacho now and then
//! for a moment (background app refresh), which does one pass of the same
//! work (`check_once`).
//!
//! The window does this itself while it is open (`MailApp`); the monitor is
//! the same work without any interface: it keeps an IDLE (or Proton events)
//! watch on each inbox, checks inboxes on their schedule, and announces new
//! unread mail. It also sends scheduled mail (Send Later) once due and
//! brings snoozed messages back to the inbox, announcing both. It runs on a
//! thread of its own, so it needs no GPUI, and shares the app's `Backend`
//! (its account locks, connections and sessions).
//!
//! Only one of the window and the monitor watches at a time: starting either
//! ends the other's watches (`Backend::stop_all_watching`). Dropping the
//! monitor ends its watches at once and lets its thread finish what it was
//! doing (one check or send at most) without waiting for it, so opening the
//! window never stalls on the network. A scheduled message the window finds
//! due while the monitor still sends it isn't sent twice
//! (`Backend::send_scheduled`). The thread then ends, dropping its
//! share of the backend.
use super::{
    backend::Backend,
    local_ai::{self, otp},
    model::*,
    status::AccessFailure,
};
use std::{
    collections::{HashMap, HashSet},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

/// The preference that keeps Picacho running in the background once its
/// windows close (on Android: the mail service). On unless turned off.
pub const PREFERENCE: &str = "background";
/// The preference that shows the tray icon while a window is open too. While
/// Picacho runs in the background the icon always shows.
pub const TRAY_PREFERENCE: &str = "tray_icon";

pub fn enabled(store: &super::store::Store) -> bool {
    store.preference(PREFERENCE).as_deref() != Some("false")
}

pub fn tray_while_open(store: &super::store::Store) -> bool {
    store.preference(TRAY_PREFERENCE).as_deref() == Some("true")
}

/// How often the monitor looks for checks that are due, as the window does.
const TICK: Duration = Duration::from_secs(20);
/// How many of the newest inbox messages each check lists, as the window's.
const CHECK_LIMIT: usize = 100;
/// New mail older than this isn't announced: it arrived long before, while
/// nothing was checking, and was likely read elsewhere.
const RECENT_SECONDS: i64 = 2 * 86400;

/// After a send or a wake that failed in a way that may pass (offline, the
/// server away), the next try waits this long, as the window's does.
const RETRY: Duration = Duration::from_secs(5 * 60);

/// What the monitor tells the person, by system notification.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Announcement<'a> {
    /// New unread mail: the latest message's sender and subject (as it may
    /// be shown), and how many arrived.
    /// `account_id` and `message` (the latest's id) say where tapping the
    /// notification goes.
    NewMail {
        account: &'a str,
        account_id: &'a str,
        message: &'a str,
        /// Every new message's id, for the notification's buttons.
        messages: &'a [&'a str],
        sender: &'a str,
        subject: &'a str,
        count: usize,
    },
    /// Snoozed messages came back to the inbox: the latest's sender,
    /// subject and Message-ID, and how many.
    BackFromSnooze {
        account: &'a str,
        account_id: &'a str,
        message_id: &'a str,
        sender: &'a str,
        subject: &'a str,
        count: usize,
    },
    /// A scheduled message went.
    ScheduledSent {
        account: &'a str,
        account_id: &'a str,
        to: &'a str,
        subject: &'a str,
    },
    /// A scheduled message couldn't be sent; it stays in Drafts, saying why.
    /// `account_id` is `None` when its account was removed.
    ScheduledFailed {
        account: &'a str,
        account_id: Option<&'a str>,
        draft: &'a str,
        subject: &'a str,
        error: &'a str,
    },
}

/// Shows an announcement (`display::notify::announce`).
pub type Announce = Box<dyn Fn(Announcement<'_>) + Send>;

/// The running monitor; dropping it stops it.
pub struct Monitor {
    shared: Arc<Shared>,
    wake: mpsc::Sender<Wake>,
    /// Disconnects once the thread has ended (its sender drops with it).
    done: Option<mpsc::Receiver<()>>,
}

/// The longest `stop_and_wait` waits for a scheduled message to finish
/// sending.
const SEND_WAIT: Duration = Duration::from_secs(180);

struct Shared {
    backend: Arc<Backend>,
    /// Set once the monitor is dropped. Held while the thread starts
    /// watches, so none starts after `Drop` ended them.
    stopped: Mutex<bool>,
    /// Set when the system ends a single pass's time (`check_once`).
    cancel: Arc<AtomicBool>,
}

enum Wake {
    /// A watch heard of a change in this account's inbox.
    Pushed(String),
    Stop,
}

impl Monitor {
    /// Starts watching and checking every account's inbox, announcing new
    /// mail with `announce`. Watches someone else started end first.
    pub fn start(backend: Arc<Backend>, announce: Announce) -> std::io::Result<Self> {
        backend.stop_all_watching();
        let (wake, woken) = mpsc::channel();
        let shared = Arc::new(Shared {
            backend,
            stopped: Mutex::new(false),
            cancel: Arc::default(),
        });
        let thread_shared = shared.clone();
        let thread_wake = wake.clone();
        let (finished, done) = mpsc::channel::<()>();
        std::thread::Builder::new()
            .name("picacho-background".into())
            .spawn(move || {
                let _finished = finished;
                run(&thread_shared, thread_wake, woken, announce)
            })?;
        Ok(Self {
            shared,
            wake,
            done: Some(done),
        })
    }

    /// Stops the monitor and waits for its thread to finish what it was
    /// doing: up to `limit` for a check, and for as long as a scheduled
    /// message takes to send (up to `SEND_WAIT`), so a process handing over
    /// to another never cuts a send short and has it sent twice.
    pub fn stop_and_wait(mut self, limit: Duration) {
        let done = self.done.take();
        let backend = self.shared.backend.clone();
        drop(self);
        let Some(done) = done else {
            return;
        };
        let start = Instant::now();
        loop {
            if !matches!(
                done.recv_timeout(Duration::from_millis(50)),
                Err(mpsc::RecvTimeoutError::Timeout)
            ) {
                return;
            }
            let waited = start.elapsed();
            let sending = backend.sending_any_scheduled();
            if waited >= limit && (!sending || waited >= SEND_WAIT) {
                return;
            }
        }
    }
}

impl Drop for Monitor {
    fn drop(&mut self) {
        let mut stopped = self
            .shared
            .stopped
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        *stopped = true;
        // Their callbacks hold the thread's channel; ending them drops those.
        self.shared.backend.stop_all_watching();
        drop(stopped);
        let _ = self.wake.send(Wake::Stop);
    }
}

impl Shared {
    fn stopped(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
            || *self.stopped.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// One pass of the monitor's work, for a system that wakes Picacho for a
/// moment rather than letting it run (iOS's background app refresh): sends
/// scheduled mail that's due, brings back snoozed mail, checks each inbox
/// that's checked on a schedule once, and announces mail that arrived since
/// Picacho last looked. Watches nothing. Stops between steps once `cancel`
/// is set (the system's time is up). Returns whether every check went.
#[cfg_attr(not(any(target_os = "ios", test)), allow(dead_code))]
pub fn check_once(backend: Arc<Backend>, announce: &Announce, cancel: Arc<AtomicBool>) -> bool {
    let shared = Shared {
        backend,
        stopped: Mutex::new(false),
        cancel,
    };
    let backend = &shared.backend;
    let store = &backend.store;
    let accounts = store.accounts().unwrap_or_default();
    // Unread inbox mail already in the cache was seen in the window (or
    // announced) before; only what arrives now is new.
    let mut known: HashMap<String, HashSet<String>> = accounts
        .iter()
        .map(|a| {
            let unread = store.unread_ids(&a.id, Folder::Inbox).unwrap_or_default();
            (a.id.clone(), unread.into_iter().collect())
        })
        .collect();
    let mut refused = HashSet::new();
    let mut later = Later::default();
    let mut codes = Codes::default();
    if send_due(&shared, &accounts, &refused, &mut later, announce).is_err()
        || wake_snoozed(
            &shared,
            &accounts,
            &mut refused,
            &mut later,
            &mut codes,
            announce,
        )
        .is_err()
    {
        return false;
    }
    let mut all = later.sends.is_empty() && later.wakes.is_empty();
    let mut checked = false;
    for a in &accounts {
        let returning = later.returning.remove(&a.id);
        if refused.contains(&a.id)
            || backend.throttled_until(&a.id).is_some()
            || (a.sync.refresh_minutes == 0 && returning.is_none())
        {
            continue;
        }
        if shared.stopped() {
            return false;
        }
        checked = true;
        match backend.sync(a, Folder::Inbox, CHECK_LIMIT) {
            Ok(messages) => {
                let mut fresh = fresh_mail(known.entry(a.id.clone()).or_default(), &messages);
                without_returning(&mut fresh, returning);
                // Nothing keeps Proton's pushes coming while iOS holds
                // Picacho: what they'd announce is announced here.
                arrived(&shared, a, fresh, false, &mut codes, announce);
            }
            Err(_) => all = false,
        }
    }
    if checked {
        super::widgets::publish(store);
        super::memory::release_unused();
    }
    all && !shared.stopped()
}

/// Applies blocked senders, muted conversations and rules to the new mail
/// of a check of `a`'s inbox, as the window does, and announces what they
/// leave (`pushed`: unless Proton's pushes announce the account's mail).
fn arrived(
    shared: &Shared,
    a: &Account,
    mut fresh: Vec<&Mail>,
    pushed: bool,
    codes: &mut Codes,
    announce: &Announce,
) {
    let backend = &shared.backend;
    let store = &backend.store;
    if shared.stopped() {
        return;
    }
    // What the rules file away or mark read isn't announced.
    let org = super::organize::Organizing::load(store);
    if !fresh.is_empty() && org.applies_to_arrivals() {
        let arrived: Vec<Mail> = fresh.iter().map(|m| (*m).clone()).collect();
        if let Err(e) = super::organize::apply_to_arrivals(backend, a, &arrived) {
            eprintln!("Picacho: a rule couldn't be applied to {}: {e:#}", a.email);
        }
        fresh.retain(|m| !org.silenced(m));
    }
    if !a.sync.notify || (pushed && a.pushes_new_mail()) || shared.stopped() {
        return;
    }
    if let Some(latest) = fresh.iter().max_by_key(|m| m.date) {
        let subject = codes.hide(store, &latest.subject);
        let ids: Vec<&str> = fresh.iter().map(|m| m.id.as_str()).collect();
        announce(Announcement::NewMail {
            account: &a.email,
            account_id: &a.id,
            message: &latest.id,
            messages: &ids,
            sender: &latest.sender,
            subject: &subject,
            count: fresh.len(),
        });
    }
}

fn run(shared: &Shared, wake: mpsc::Sender<Wake>, woken: mpsc::Receiver<Wake>, announce: Announce) {
    let backend = &shared.backend;
    let store = &backend.store;
    // Unread inbox mail already in the cache was seen in the window (or
    // announced) before; only what arrives after it is new.
    let mut known: HashMap<String, HashSet<String>> = store
        .accounts()
        .unwrap_or_default()
        .into_iter()
        .map(|a| {
            // By id from the index: the inbox's messages aren't read.
            let unread = store
                .unread_ids(&a.id, Folder::Inbox)
                .unwrap_or_default()
                .into_iter()
                .collect();
            (a.id, unread)
        })
        .collect();
    let mut last_check: HashMap<String, Instant> = HashMap::new();
    let mut pushed: HashSet<String> = HashSet::new();
    // Accounts that couldn't sign in: retrying in the background could
    // get them locked out. The window reports them once it opens.
    let mut refused: HashSet<String> = HashSet::new();
    let mut codes = Codes::default();
    let mut later = Later::default();
    loop {
        let accounts = store.accounts().unwrap_or_default();
        let present = |id: &String| accounts.iter().any(|a| &a.id == id);
        known.retain(|id, _| present(id));
        last_check.retain(|id, _| present(id));
        refused.retain(present);
        later.returning.retain(|id, _| present(id));
        later.wakes.retain(|id, _| present(id));
        {
            let stopped = shared.stopped.lock().unwrap_or_else(|e| e.into_inner());
            if *stopped {
                break;
            }
            watch(backend, &accounts, &refused, &wake);
        }
        if send_due(shared, &accounts, &refused, &mut later, &announce).is_err()
            || wake_snoozed(
                shared,
                &accounts,
                &mut refused,
                &mut later,
                &mut codes,
                &announce,
            )
            .is_err()
        {
            return;
        }
        // Accounts with mail back from snooze are checked now, so it lists.
        pushed.extend(later.returning.keys().cloned());
        let now = Instant::now();
        let mut checked = false;
        for a in &accounts {
            if refused.contains(&a.id) || backend.throttled_until(&a.id).is_some() {
                continue;
            }
            let every = Duration::from_secs(u64::from(a.sync.refresh_minutes) * 60);
            let due = pushed.remove(&a.id)
                || (a.sync.refresh_minutes > 0
                    && last_check
                        .get(&a.id)
                        .is_none_or(|t| now.duration_since(*t) >= every));
            if !due {
                continue;
            }
            if shared.stopped() {
                return;
            }
            last_check.insert(a.id.clone(), Instant::now());
            checked = true;
            match backend.sync(a, Folder::Inbox, CHECK_LIMIT) {
                Ok(messages) => {
                    let first = !known.contains_key(&a.id);
                    let mut fresh = fresh_mail(known.entry(a.id.clone()).or_default(), &messages);
                    without_returning(&mut fresh, later.returning.remove(&a.id));
                    // An account added since the monitor started: what it
                    // has now was there before, like the window's first check.
                    if first {
                        continue;
                    }
                    // Proton's pushes announce mail for accounts that take them.
                    arrived(shared, a, fresh, true, &mut codes, &announce);
                }
                Err(e) if e.downcast_ref::<AccessFailure>().is_some() => {
                    refused.insert(a.id.clone());
                    let stopped = shared.stopped.lock().unwrap_or_else(|e| e.into_inner());
                    if !*stopped {
                        backend.stop_watching(&a.id);
                    }
                }
                Err(_) => {}
            }
        }
        // A check parses and stores mail, then frees it all again; give that
        // back to the system before waiting.
        if checked {
            super::widgets::publish(store);
            super::memory::release_unused();
        }
        match woken.recv_timeout(TICK) {
            Ok(Wake::Pushed(id)) => {
                pushed.insert(id);
                // A burst of changes makes one check.
                while let Ok(next) = woken.try_recv() {
                    match next {
                        Wake::Pushed(id) => {
                            pushed.insert(id);
                        }
                        Wake::Stop => return,
                    }
                }
            }
            Ok(Wake::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => return,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
    }
}

/// The monitor was stopped mid-way.
struct Stopped;

/// Scheduled sends and snooze wakes waiting to be tried again, and
/// messages back from snooze not yet seen by an inbox check.
#[derive(Default)]
struct Later {
    /// Scheduled drafts by id.
    sends: HashMap<String, Instant>,
    /// Accounts whose snoozed mail couldn't be brought back.
    wakes: HashMap<String, Instant>,
    /// Message-IDs back from snooze, by account.
    returning: HashMap<String, HashSet<String>>,
}

/// Sends the scheduled drafts whose time has come (`MailApp::send_due`).
fn send_due(
    shared: &Shared,
    accounts: &[Account],
    refused: &HashSet<String>,
    later: &mut Later,
    announce: &Announce,
) -> Result<(), Stopped> {
    let backend = &shared.backend;
    let Ok(drafts) = backend.store.scheduled_drafts() else {
        return Ok(());
    };
    // Drafts no longer scheduled (sent, deleted, unscheduled) aren't kept.
    later
        .sends
        .retain(|id, _| drafts.iter().any(|d| &d.id == id));
    let now = chrono::Utc::now().timestamp();
    for d in drafts {
        let due = d.send_at.is_some_and(|t| t <= now) && d.send_error.is_empty();
        if !due || later.sends.get(&d.id).is_some_and(|t| *t > Instant::now()) {
            continue;
        }
        if shared.stopped() {
            return Err(Stopped);
        }
        let Some(a) = accounts.iter().find(|a| a.id == d.account) else {
            let error = t!("manage-background-its-account-was-removed");
            backend.schedule_failed(&d.id, error);
            announce(Announcement::ScheduledFailed {
                account: &d.account,
                account_id: None,
                draft: &d.id,
                subject: &d.subject,
                error,
            });
            continue;
        };
        // It waits, unchanged, until the account can sign in again.
        if refused.contains(&a.id) {
            continue;
        }
        match backend.send_scheduled(a, &d.id) {
            Ok(Some((sent, _notes))) => {
                later.sends.remove(&d.id);
                // Its copy in the server's Drafts goes too.
                if a.provider != Provider::Pop3 {
                    let _ = backend.sync_drafts(a);
                }
                announce(Announcement::ScheduledSent {
                    account: &a.email,
                    account_id: &a.id,
                    to: &sent.to,
                    subject: &sent.subject,
                });
            }
            // The window sends it, or did; or it is no longer due.
            Ok(None) => {}
            // Offline, or the server's away: nothing went.
            Err(e) if crate::sync::retry::later(&e) => {
                later.sends.insert(d.id.clone(), Instant::now() + RETRY);
            }
            Err(e) => {
                let error = format!("{e:#}");
                backend.schedule_failed(&d.id, &error);
                announce(Announcement::ScheduledFailed {
                    account: &a.email,
                    account_id: Some(&a.id),
                    draft: &d.id,
                    subject: &d.subject,
                    error: &error,
                });
            }
        }
    }
    Ok(())
}

/// Brings back snoozed messages whose time has come
/// (`MailApp::wake_snoozed`), noting them for the next inbox check.
fn wake_snoozed(
    shared: &Shared,
    accounts: &[Account],
    refused: &mut HashSet<String>,
    later: &mut Later,
    codes: &mut Codes,
    announce: &Announce,
) -> Result<(), Stopped> {
    let backend = &shared.backend;
    let now = chrono::Utc::now().timestamp();
    later.wakes.retain(|_, t| *t > Instant::now());
    for a in accounts {
        if a.provider == Provider::Pop3
            || refused.contains(&a.id)
            || later.wakes.contains_key(&a.id)
            || !backend.snoozed(&a.id).iter().any(|s| s.until <= now)
        {
            continue;
        }
        if shared.stopped() {
            return Err(Stopped);
        }
        match backend.wake_due(a) {
            Ok(woken) => {
                let Some(latest) = woken.iter().max_by_key(|s| s.until) else {
                    continue;
                };
                let subject = codes.hide(&backend.store, &latest.subject);
                announce(Announcement::BackFromSnooze {
                    account: &a.email,
                    account_id: &a.id,
                    message_id: &latest.message_id,
                    sender: &latest.sender,
                    subject: &subject,
                    count: woken.len(),
                });
                later
                    .returning
                    .entry(a.id.clone())
                    .or_default()
                    .extend(woken.into_iter().map(|s| s.message_id));
            }
            Err(e) if e.downcast_ref::<AccessFailure>().is_some() => {
                refused.insert(a.id.clone());
            }
            // Not asked again at every tick: the server is spared, and the
            // window reports a lasting failure once it opens.
            Err(_) => {
                later.wakes.insert(a.id.clone(), Instant::now() + RETRY);
            }
        }
    }
    Ok(())
}

/// Keeps an inbox watch on each account that can have one, as the window
/// does (`MailApp::watch_inboxes`); ends watches of accounts that are gone
/// or can't sign in.
fn watch(
    backend: &Arc<Backend>,
    accounts: &[Account],
    refused: &HashSet<String>,
    wake: &mpsc::Sender<Wake>,
) {
    let watching = backend.watching();
    for id in &watching {
        if !accounts.iter().any(|a| &a.id == id) || refused.contains(id) {
            backend.stop_watching(id);
        }
    }
    for a in accounts {
        if a.provider == Provider::Pop3
            || a.incoming_host.is_empty()
            || watching.contains(&a.id)
            || refused.contains(&a.id)
        {
            continue;
        }
        let (wake, id) = (wake.clone(), a.id.clone());
        backend.watch_inbox(a, move |mailbox| {
            if mailbox == Mailbox::Standard(Folder::Inbox) {
                let _ = wake.send(Wake::Pushed(id.clone()));
            }
        });
    }
}

/// Leaves out of `fresh` the mail back from snooze (by Message-ID): it
/// arrives unread and recent, but was announced as back from snooze.
fn without_returning(fresh: &mut Vec<&Mail>, returning: Option<HashSet<String>>) {
    if let Some(returning) = returning {
        fresh.retain(|m| !returning.contains(m.message_id.trim()));
    }
}

/// Runs the monitor's snooze step once (tests): the accounts whose wake
/// waits to be tried again, those refused, and what was announced.
#[cfg(test)]
pub(super) fn wake_once(
    backend: Arc<Backend>,
    accounts: &[Account],
) -> (Vec<String>, HashSet<String>, Vec<String>) {
    let shared = Shared {
        backend,
        stopped: Mutex::new(false),
        cancel: Arc::default(),
    };
    let (mut refused, mut later, mut codes) = (HashSet::new(), Later::default(), Codes::default());
    let said = Arc::new(Mutex::new(Vec::new()));
    let heard = said.clone();
    let announce: Announce = Box::new(move |a| heard.lock().unwrap().push(format!("{a:?}")));
    let _ = wake_snoozed(
        &shared,
        accounts,
        &mut refused,
        &mut later,
        &mut codes,
        &announce,
    );
    let said = said.lock().unwrap().clone();
    (later.wakes.into_keys().collect(), refused, said)
}

/// Notes the unread messages of a check in `known` and returns those that
/// are new since the last check (`MailApp::note_inbox`'s rule).
fn fresh_mail<'a>(known: &mut HashSet<String>, messages: &'a [Mail]) -> Vec<&'a Mail> {
    let recent = chrono::Utc::now().timestamp() - RECENT_SECONDS;
    let fresh: Vec<&Mail> = messages
        .iter()
        .filter(|m| m.unread && m.date > recent && !known.contains(&m.id))
        .collect();
    let present: HashSet<&String> = messages.iter().map(|m| &m.id).collect();
    known.retain(|m| present.contains(m));
    known.extend(messages.iter().filter(|m| m.unread).map(|m| m.id.clone()));
    fresh
}

/// Sign-in codes kept out of announcements while Local AI hides them, as
/// the window keeps them off the lock screen. The model loads on the first
/// announcement that needs it and is freed with the monitor.
#[derive(Default)]
struct Codes {
    detector: Option<Result<otp::Detector, ()>>,
}

impl Codes {
    fn hide(&mut self, store: &super::store::Store, subject: &str) -> String {
        if !local_ai::otp_enabled(store) {
            self.detector = None;
            return subject.to_owned();
        }
        let detector = self
            .detector
            .get_or_insert_with(|| local_ai::otp_detector(store).map_err(|_| ()));
        match detector {
            Ok(detector) => otp::mask(subject, &detector.codes(subject, "")).into_owned(),
            // Without the model a code can't be told apart; leave the
            // subject out rather than show one.
            Err(()) => t!("manage-background-new-message").to_owned(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mail(id: &str, unread: bool, age: i64) -> Mail {
        Mail {
            id: id.into(),
            unread,
            date: chrono::Utc::now().timestamp() - age,
            ..demo_mail().remove(0)
        }
    }

    #[test]
    fn only_unread_mail_not_seen_before_is_fresh() {
        let mut known: HashSet<String> = ["a".to_owned()].into();
        let messages = [
            mail("a", true, 60),
            mail("b", true, 60),
            mail("c", false, 60),
            // Arrived long ago, while nothing was checking.
            mail("d", true, 3 * 86400),
        ];
        let fresh: Vec<&str> = fresh_mail(&mut known, &messages)
            .iter()
            .map(|m| m.id.as_str())
            .collect();
        assert_eq!(fresh, ["b"]);
        assert!(known.contains("b") && known.contains("d") && !known.contains("c"));
        // The next check finds nothing new.
        assert!(fresh_mail(&mut known, &messages).is_empty());
    }

    #[test]
    fn dropping_the_monitor_ends_its_thread_and_releases_the_backend() {
        let dir = std::env::temp_dir().join(format!("picacho-monitor-{}", uuid::Uuid::new_v4()));
        let store = Arc::new(super::super::store::Store::open(&dir).unwrap());
        let backend = Arc::new(Backend::new(store));
        for _ in 0..3 {
            let monitor = Monitor::start(backend.clone(), Box::new(|_| {})).unwrap();
            assert!(Arc::strong_count(&backend) > 1);
            drop(monitor);
            // The thread wakes at once (not at its next tick) and ends,
            // dropping its share of the backend.
            let deadline = Instant::now() + Duration::from_secs(5);
            while Arc::strong_count(&backend) > 1 {
                assert!(
                    Instant::now() < deadline,
                    "the monitor's thread kept running"
                );
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        assert!(backend.watching().is_empty());
        drop(backend);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A store with no accounts, the monitor's state over it, and what it
    /// announces.
    fn quiet() -> (
        Shared,
        Announce,
        Arc<Mutex<Vec<String>>>,
        std::path::PathBuf,
    ) {
        let dir = std::env::temp_dir().join(format!("picacho-monitor-{}", uuid::Uuid::new_v4()));
        let store = Arc::new(super::super::store::Store::open(&dir).unwrap());
        let shared = Shared {
            backend: Arc::new(Backend::new(store)),
            stopped: Mutex::new(false),
            cancel: Arc::default(),
        };
        let said = Arc::new(Mutex::new(Vec::new()));
        let heard = said.clone();
        let announce: Announce = Box::new(move |a| heard.lock().unwrap().push(format!("{a:?}")));
        (shared, announce, said, dir)
    }

    fn scheduled(id: &str, account: &str, at: i64) -> Draft {
        Draft {
            id: id.into(),
            account: account.into(),
            to: "ana@example.com".into(),
            subject: "Tomorrow's plans".into(),
            send_at: Some(at),
            ..Default::default()
        }
    }

    #[test]
    fn scheduled_mail_waits_until_due() {
        let (shared, announce, said, dir) = quiet();
        let store = &shared.backend.store;
        let later_on = chrono::Utc::now().timestamp() + 3600;
        store
            .save_draft(&scheduled("d1", "gone", later_on))
            .unwrap();
        let mut later = Later::default();
        assert!(send_due(&shared, &[], &HashSet::new(), &mut later, &announce).is_ok());
        let kept = store.draft("d1").unwrap().unwrap();
        assert!(kept.send_error.is_empty() && kept.send_at == Some(later_on));
        assert!(said.lock().unwrap().is_empty());
        drop(shared);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn scheduled_mail_of_a_removed_account_stays_in_drafts_saying_why() {
        let (shared, announce, said, dir) = quiet();
        let store = &shared.backend.store;
        let past = chrono::Utc::now().timestamp() - 5;
        store.save_draft(&scheduled("d1", "gone", past)).unwrap();
        let mut later = Later::default();
        assert!(send_due(&shared, &[], &HashSet::new(), &mut later, &announce).is_ok());
        let kept = store.draft("d1").unwrap().expect("kept in Drafts");
        assert_eq!(kept.send_error, "Its account was removed.");
        let said = said.lock().unwrap().clone();
        assert_eq!(said.len(), 1);
        assert!(said[0].starts_with("ScheduledFailed"), "{said:?}");
        // Failed, it isn't tried (or announced) again.
        let again = Arc::new(Mutex::new(0));
        let counted = again.clone();
        let announce: Announce = Box::new(move |_| *counted.lock().unwrap() += 1);
        assert!(send_due(&shared, &[], &HashSet::new(), &mut later, &announce).is_ok());
        assert_eq!(*again.lock().unwrap(), 0);
        drop(shared);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_stopped_monitor_sends_nothing() {
        let (shared, announce, said, dir) = quiet();
        let past = chrono::Utc::now().timestamp() - 5;
        shared
            .backend
            .store
            .save_draft(&scheduled("d1", "gone", past))
            .unwrap();
        *shared.stopped.lock().unwrap() = true;
        let mut later = Later::default();
        assert!(send_due(&shared, &[], &HashSet::new(), &mut later, &announce).is_err());
        assert!(
            shared
                .backend
                .store
                .draft("d1")
                .unwrap()
                .unwrap()
                .send_error
                .is_empty()
        );
        assert!(said.lock().unwrap().is_empty());
        drop(shared);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// iOS's background refresh: a pass whose time is up sends nothing,
    /// and a draft whose account is gone is said to have failed.
    #[test]
    fn one_pass_stops_when_its_time_is_up() {
        let (shared, announce, said, dir) = quiet();
        let past = chrono::Utc::now().timestamp() - 5;
        let store = &shared.backend.store;
        store.save_draft(&scheduled("d1", "gone", past)).unwrap();
        let cancel = Arc::new(AtomicBool::new(true));
        assert!(!check_once(shared.backend.clone(), &announce, cancel));
        assert!(store.draft("d1").unwrap().unwrap().send_error.is_empty());
        assert!(said.lock().unwrap().is_empty());
        // With time to run, the orphaned draft is reported, and an account
        // checked only by hand isn't checked.
        let mut manual = Account::new(Provider::Imap);
        manual.email = "manual@example.com".into();
        manual.sync.refresh_minutes = 0;
        store.save_account(&manual).unwrap();
        assert!(check_once(
            shared.backend.clone(),
            &announce,
            Arc::default()
        ));
        let said = said.lock().unwrap().clone();
        assert_eq!(said.len(), 1, "{said:?}");
        assert!(said[0].starts_with("ScheduledFailed"), "{said:?}");
        drop(shared);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn mail_back_from_snooze_is_not_announced_again_as_new() {
        let mut back = mail("back", true, 60);
        back.message_id = "<back@example.com>".into();
        let mut new = mail("new", true, 60);
        new.message_id = "<new@example.com>".into();
        let messages = [back, new];
        let mut known = HashSet::new();
        let mut fresh = fresh_mail(&mut known, &messages);
        without_returning(&mut fresh, Some(["<back@example.com>".to_owned()].into()));
        let fresh: Vec<&str> = fresh.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(fresh, ["new"]);
    }

    #[test]
    fn mail_gone_from_the_inbox_is_forgotten() {
        let mut known: HashSet<String> = ["gone".to_owned(), "a".to_owned()].into();
        fresh_mail(&mut known, &[mail("a", true, 60)]);
        assert_eq!(known, ["a".to_owned()].into());
    }
}

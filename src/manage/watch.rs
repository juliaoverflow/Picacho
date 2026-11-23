//! Push for new mail: extra connections wait in IDLE (RFC 2177) and say
//! when the server reports a change.
//!
//! Each IMAP account has one watcher on its inbox. Where the server has
//! NOTIFY (RFC 5465), that connection also hears about every other
//! personal mailbox (as STATUS lines), so one connection covers them all.
//! Elsewhere the mailbox open in the window gets a watcher of its own (see
//! `watch_folder`), so it is pushed too; other folders are checked on
//! their schedule.
//!
//! Watchers only notice; the regular mailbox check then syncs, under the
//! account lock, so IDLE never changes the cache itself. Polling stays as
//! the fallback for servers without IDLE and while a watcher reconnects.
use super::*;
use std::{
    net::TcpStream,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

/// Servers may end an IDLE that lasts too long (RFC 2177 allows 30
/// minutes; some drop it sooner). Renewing it is one short exchange.
const RENEW_IDLE: Duration = Duration::from_secs(9 * 60);
/// Waits after a failure, doubling up to the longest.
const FIRST_RETRY: Duration = Duration::from_secs(30);
const LONGEST_RETRY: Duration = Duration::from_secs(15 * 60);

pub(super) struct Watcher {
    stop: Arc<AtomicBool>,
    /// The waiting connection's socket, to end its read at once.
    socket: Arc<Mutex<Option<TcpStream>>>,
    /// The mailbox it waits in.
    mailbox: Mailbox,
    /// Its server agreed to NOTIFY: it hears about every mailbox.
    notifying: Arc<AtomicBool>,
    /// Its thread, woken from `pause` when stopped.
    thread: Option<std::thread::Thread>,
}
impl Watcher {
    fn on(mut self, thread: &std::thread::Thread) -> Self {
        self.thread = Some(thread.clone());
        self
    }
    fn stop(&self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = &self.thread {
            thread.unpark();
        }
        if let Some(socket) = self.socket.lock().unwrap().take() {
            let _ = socket.shutdown(std::net::Shutdown::Both);
        }
    }
}
/// Waits `wait` on a watcher's own thread, or less once `stop` is set
/// (`Watcher::stop` wakes it). Asleep meanwhile, rather than waking every
/// moment to look. Returns whether it was stopped.
fn pause(stop: &AtomicBool, wait: Duration) -> bool {
    let until = std::time::Instant::now() + wait;
    loop {
        if stop.load(Ordering::Acquire) {
            return true;
        }
        let now = std::time::Instant::now();
        if now >= until {
            return false;
        }
        // Wakes early on `unpark` (or spuriously): the loop looks again.
        std::thread::park_timeout(until - now);
    }
}
/// Why a watch ended.
enum Ended {
    Stopped,
    /// The server has no IDLE; polling alone checks this account.
    Unsupported,
}
/// The callback watchers call with the mailbox that changed.
type Changed = Arc<dyn Fn(Mailbox) + Send + Sync>;
/// Hears a Proton account notice: the account, its title and its body.
pub type ProtonNotice = Arc<dyn Fn(&Account, &str, &str) + Send + Sync>;

impl Backend {
    /// Who hears the notices Proton's event stream carries (its
    /// `Notifications`): messages Proton shows its apps' users.
    pub fn on_proton_notice(&self, hear: impl Fn(&Account, &str, &str) + Send + Sync + 'static) {
        *self.proton_notice.lock().unwrap() = Some(Arc::new(hear));
    }
    /// Watches the account's inbox for new mail with IDLE (and, with
    /// NOTIFY, every other mailbox), calling `changed` with the mailbox
    /// whenever the server reports a change, until `stop_watching`. A
    /// watch already running is replaced. Returns at once.
    pub fn watch_inbox(
        self: &Arc<Self>,
        a: &Account,
        changed: impl Fn(Mailbox) + Send + Sync + 'static,
    ) {
        if a.provider == Provider::Proton {
            self.stop_watching(&a.id);
            // Asking Proton for changes is turned off: the regular check
            // finds new mail.
            if a.sync.proton_events_seconds == 0 {
                return;
            }
            if let Some(w) = self.spawn_proton_watch(a, Arc::new(changed)) {
                self.keep_watcher(&self.watchers, &a.id, w);
            }
            return;
        }
        if a.provider == Provider::Jmap {
            self.stop_watching(&a.id);
            if let Some(w) = self.spawn_jmap_watch(a, Arc::new(changed)) {
                self.keep_watcher(&self.watchers, &a.id, w);
            }
            return;
        }
        // POP3 has no IDLE: its mail is checked on a timer.
        if !a.provider.imap() {
            return;
        }
        self.stop_watching(&a.id);
        let watcher = self.spawn_watch(a, Mailbox::Standard(Folder::Inbox), Arc::new(changed));
        if let Some(w) = watcher {
            self.keep_watcher(&self.watchers, &a.id, w);
        }
    }
    /// Also watches `mailbox` (the one open in the window) with IDLE, when
    /// the inbox's watcher doesn't already hear about it through NOTIFY.
    /// Replaces the account's earlier folder watch; the inbox and virtual
    /// mailboxes (Starred, All Mail) need none, and `None` just ends it.
    pub fn watch_folder(
        self: &Arc<Self>,
        a: &Account,
        mailbox: Option<Mailbox>,
        changed: impl Fn(Mailbox) + Send + Sync + 'static,
    ) {
        let wanted = mailbox.filter(|m| {
            !m.folder().is_virtual()
                && !matches!(m, Mailbox::Standard(Folder::Inbox | Folder::Drafts))
                && a.provider.imap()
        });
        let notifying = self
            .watchers
            .lock()
            .unwrap()
            .get(&a.id)
            .is_some_and(|w| w.notifying.load(Ordering::Acquire));
        let mut folders = self.folder_watchers.lock().unwrap();
        if let Some(current) = folders.get(&a.id)
            && Some(&current.mailbox) == wanted.as_ref()
            && !notifying
        {
            return;
        }
        if let Some(old) = folders.remove(&a.id) {
            old.stop();
        }
        drop(folders);
        let Some(mailbox) = wanted.filter(|_| !notifying) else {
            return;
        };
        if let Some(w) = self.spawn_watch(a, mailbox, Arc::new(changed)) {
            self.keep_watcher(&self.folder_watchers, &a.id, w);
        }
    }
    /// Proton: its event stream stands in for IDLE, as it does inside
    /// Proton Mail Bridge. One watcher per account hears about every
    /// mailbox (like NOTIFY), so no folder needs one of its own.
    fn spawn_proton_watch(self: &Arc<Self>, a: &Account, changed: Changed) -> Option<Watcher> {
        let watcher = Watcher {
            stop: Arc::new(AtomicBool::new(false)),
            socket: Arc::new(Mutex::new(None)),
            mailbox: Mailbox::Standard(Folder::Inbox),
            notifying: Arc::new(AtomicBool::new(true)),
            thread: None,
        };
        let stop = watcher.stop.clone();
        let backend = self.clone();
        let a = a.clone();
        let spawned = std::thread::Builder::new()
            .name(format!("proton-events-{}", a.id))
            .spawn(move || {
                let mut retry = FIRST_RETRY;
                while !stop.load(Ordering::Acquire) {
                    let mut watched = false;
                    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        backend.proton_events(&a, &stop, &*changed, &mut watched)
                    }))
                    .unwrap_or_else(|_| Err(anyhow::anyhow!("Proton's events failed")));
                    let Err(error) = outcome else {
                        break;
                    };
                    if stop.load(Ordering::Acquire) {
                        break;
                    }
                    if watched {
                        retry = FIRST_RETRY;
                    }
                    let wait = if error.downcast_ref::<AccessFailure>().is_some() {
                        LONGEST_RETRY
                    } else {
                        retry
                    };
                    retry = (retry * 2).min(LONGEST_RETRY);
                    pause(&stop, wait);
                    // Mail may have arrived while the watch was down.
                    if !stop.load(Ordering::Acquire) {
                        changed(Mailbox::Standard(Folder::Inbox));
                    }
                }
            });
        spawned.ok().map(|handle| watcher.on(handle.thread()))
    }
    /// Follows Proton's events until `stop`, saying which mailboxes
    /// changed. Like IDLE, it only notices: the regular check syncs.
    fn proton_events(
        &self,
        a: &Account,
        stop: &AtomicBool,
        changed: &(dyn Fn(Mailbox) + Send + Sync),
        watched: &mut bool,
    ) -> Result<()> {
        let mut since = self.with_proton(a, |s| s.latest_event())?;
        *watched = true;
        // Notices already told, should a later event repeat one.
        let mut told = std::collections::HashSet::<String>::new();
        loop {
            // As often as the account says, and up to a quarter more, so
            // the requests don't settle into lockstep with anything else.
            let every = Duration::from_secs(u64::from(a.sync.proton_events_seconds.max(1)));
            let wait = every + every.mul_f64(rand::random::<f64>() / 4.0);
            if pause(stop, wait) {
                return Ok(());
            }
            let (news, folders) =
                self.with_proton(a, |s| Ok((s.events_since(&since)?, s.folders())))?;
            let crate::sync::proton::Events {
                latest,
                messages: events,
                refresh,
                notices,
            } = news;
            since = latest;
            let hear = self.proton_notice.lock().unwrap().clone();
            for n in notices {
                let key = if n.id.is_empty() {
                    format!("{}\n{}\n{}", n.time, n.payload.title, n.payload.body)
                } else {
                    n.id.clone()
                };
                if !told.insert(key) {
                    continue;
                }
                if let Some(hear) = &hear {
                    let title = if n.payload.title.is_empty() {
                        "Proton"
                    } else {
                        &n.payload.title
                    };
                    let body = [n.payload.subtitle.as_str(), n.payload.body.as_str()]
                        .into_iter()
                        .filter(|t| !t.is_empty())
                        .collect::<Vec<_>>()
                        .join("\n");
                    hear(a, title, &body);
                }
            }
            let mut boxes: Vec<Mailbox> = Vec::new();
            if refresh {
                boxes.push(Mailbox::Standard(Folder::Inbox));
            }
            for e in events {
                let mut affected = Vec::new();
                if let Some(m) = &e.message {
                    let (folder, remote) = super::proton::location(&m.label_ids, &folders);
                    affected.push(if folder == Folder::Other {
                        Mailbox::Remote(remote)
                    } else {
                        Mailbox::Standard(folder)
                    });
                    if m.is_starred() {
                        affected.push(Mailbox::Standard(Folder::Starred));
                    }
                }
                // Where it was before: a deleted or moved message.
                if let Ok(Some(old)) = self.store.message(&super::proton::row_id(a, &e.id)) {
                    affected.push(old.mailbox());
                }
                for b in affected {
                    if !boxes.contains(&b) {
                        boxes.push(b);
                    }
                }
            }
            for b in boxes {
                changed(b);
            }
        }
    }
    /// JMAP: the server's event source stands in for IDLE (or, where it has
    /// none, asking for changes every minute). One watcher per account
    /// hears about every mailbox, so no folder needs one of its own.
    fn spawn_jmap_watch(self: &Arc<Self>, a: &Account, changed: Changed) -> Option<Watcher> {
        let watcher = Watcher {
            stop: Arc::new(AtomicBool::new(false)),
            socket: Arc::new(Mutex::new(None)),
            mailbox: Mailbox::Standard(Folder::Inbox),
            notifying: Arc::new(AtomicBool::new(true)),
            thread: None,
        };
        let stop = watcher.stop.clone();
        let backend = self.clone();
        let a = a.clone();
        let spawned = std::thread::Builder::new()
            .name(format!("jmap-events-{}", a.id))
            .spawn(move || {
                let mut retry = FIRST_RETRY;
                while !stop.load(Ordering::Acquire) {
                    let mut watched = false;
                    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        backend.jmap_events(&a, &stop, &*changed, &mut watched)
                    }))
                    .unwrap_or_else(|_| Err(anyhow::anyhow!("JMAP's events failed")));
                    let Err(error) = outcome else {
                        break;
                    };
                    if stop.load(Ordering::Acquire) {
                        break;
                    }
                    if watched {
                        retry = FIRST_RETRY;
                    }
                    let wait = if error.downcast_ref::<AccessFailure>().is_some() {
                        LONGEST_RETRY
                    } else {
                        retry
                    };
                    retry = (retry * 2).min(LONGEST_RETRY);
                    pause(&stop, wait);
                    // Mail may have arrived while the watch was down.
                    if !stop.load(Ordering::Acquire) {
                        changed(Mailbox::Standard(Folder::Inbox));
                    }
                }
            });
        spawned.ok().map(|handle| watcher.on(handle.thread()))
    }
    /// Follows a JMAP account's changes until `stop`, saying which
    /// mailboxes changed. Like IDLE, it only notices: the regular check
    /// syncs. Returns only with an error (or once stopped).
    fn jmap_events(
        &self,
        a: &Account,
        stop: &AtomicBool,
        changed: &(dyn Fn(Mailbox) + Send + Sync),
        watched: &mut bool,
    ) -> Result<()> {
        let mut since = self.with_jmap(a, |s| s.state())?;
        *watched = true;
        let tell = |since: &mut String| -> Result<()> {
            let (boxes, next) = self.with_jmap(a, |s| self.jmap_changed_mailboxes(a, s, since))?;
            if !next.is_empty() {
                *since = next;
            }
            if !stop.load(Ordering::Acquire) {
                for b in boxes {
                    changed(b);
                }
            }
            Ok(())
        };
        loop {
            if stop.load(Ordering::Acquire) {
                return Ok(());
            }
            let pushes = self.with_jmap(a, |s| Ok(s.pushes()))?;
            let wait = if pushes {
                let started = std::time::Instant::now();
                self.with_jmap(a, |s| s.listen(stop, &mut || tell(&mut since)))?;
                // A stream the server keeps ending at once isn't reopened
                // in a hurry.
                if started.elapsed() < Duration::from_secs(10) {
                    Duration::from_secs(30)
                } else {
                    Duration::ZERO
                }
            } else {
                Duration::from_secs(60)
            };
            if pause(stop, wait) {
                return Ok(());
            }
            if stop.load(Ordering::Acquire) {
                return Ok(());
            }
            // The stream ended, or a minute passed: something may have
            // changed meanwhile.
            tell(&mut since)?;
        }
    }
    fn spawn_watch(
        self: &Arc<Self>,
        a: &Account,
        mailbox: Mailbox,
        changed: Changed,
    ) -> Option<Watcher> {
        let watcher = Watcher {
            stop: Arc::new(AtomicBool::new(false)),
            socket: Arc::new(Mutex::new(None)),
            mailbox: mailbox.clone(),
            notifying: Arc::new(AtomicBool::new(false)),
            thread: None,
        };
        let (stop, socket, notifying) = (
            watcher.stop.clone(),
            watcher.socket.clone(),
            watcher.notifying.clone(),
        );
        let backend = self.clone();
        let a = a.clone();
        let spawned = std::thread::Builder::new()
            .name(format!("idle-{}-{}", a.id, mailbox.key()))
            .spawn(move || {
                let mut retry = FIRST_RETRY;
                while !stop.load(Ordering::Acquire) {
                    // Whether this attempt got as far as waiting in IDLE.
                    let mut watched = false;
                    // A misbehaving server must not take the app down with it.
                    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        backend.idle(
                            &a,
                            &mailbox,
                            &stop,
                            &socket,
                            &notifying,
                            &*changed,
                            &mut watched,
                        )
                    }))
                    .unwrap_or_else(|_| Err(anyhow::anyhow!("IDLE failed")));
                    match outcome {
                        Ok(Ended::Unsupported) => break,
                        Ok(Ended::Stopped) => break,
                        Err(error) => {
                            if stop.load(Ordering::Acquire) {
                                break;
                            }
                            notifying.store(false, Ordering::Release);
                            // A watch that worked and then dropped (the
                            // network went away) starts backing off afresh.
                            if watched {
                                retry = FIRST_RETRY;
                            }
                            // Wrong credentials won't improve by retrying soon;
                            // the mailbox checks report them.
                            let wait = if error.downcast_ref::<AccessFailure>().is_some()
                                || crate::sync::transport::throttled(&error)
                            {
                                LONGEST_RETRY
                            } else {
                                retry
                            };
                            retry = (retry * 2).min(LONGEST_RETRY);
                            pause(&stop, wait);
                            // Mail may have arrived while the watch was down.
                            if !stop.load(Ordering::Acquire) {
                                changed(mailbox.clone());
                            }
                        }
                    }
                }
            });
        // Without a thread, polling checks this mailbox.
        spawned.ok().map(|handle| watcher.on(handle.thread()))
    }
    /// Ends the account's watches, if any; their connections close.
    /// Keeps `w` as `id`'s watcher in `watchers`, stopping the one it
    /// replaces: two watches started at once each stopped the old one and
    /// then put theirs in, the first one's thread and connection left
    /// running with nothing to stop them.
    fn keep_watcher(&self, watchers: &Mutex<HashMap<String, Watcher>>, id: &str, w: Watcher) {
        if let Some(old) = watchers.lock().unwrap().insert(id.to_owned(), w) {
            old.stop();
        }
    }
    pub fn stop_watching(&self, id: &str) {
        if let Some(w) = self.watchers.lock().unwrap().remove(id) {
            w.stop();
        }
        if let Some(w) = self.folder_watchers.lock().unwrap().remove(id) {
            w.stop();
        }
    }
    /// Ends every account's watches. Whoever watches next (the window or
    /// the background monitor, see `background.rs`) starts its own, with its
    /// own callback: a watch outliving its owner would keep its connection
    /// and thread, and report to nobody.
    pub fn stop_all_watching(&self) {
        let inboxes: Vec<Watcher> = self
            .watchers
            .lock()
            .unwrap()
            .drain()
            .map(|(_, w)| w)
            .collect();
        let folders: Vec<Watcher> = self
            .folder_watchers
            .lock()
            .unwrap()
            .drain()
            .map(|(_, w)| w)
            .collect();
        for w in inboxes.iter().chain(&folders) {
            w.stop();
        }
    }
    /// Signs out of the account's connections (pooled and watching), so the
    /// next ones use changed sign-in settings.
    pub fn reconnect_later(&self, id: &str) {
        self.disconnect(id);
        self.stop_watching(id);
    }
    /// Accounts whose inbox is being watched.
    pub fn watching(&self) -> Vec<String> {
        self.watchers.lock().unwrap().keys().cloned().collect()
    }
    /// One IDLE connection's life: signs in, opens the mailbox read-only
    /// and waits, renewing the IDLE now and then, until stopped or it fails.
    #[allow(clippy::too_many_arguments)]
    fn idle(
        &self,
        a: &Account,
        mailbox: &Mailbox,
        stop: &AtomicBool,
        socket: &Mutex<Option<TcpStream>>,
        notifying: &AtomicBool,
        changed: &(dyn Fn(Mailbox) + Send + Sync),
        watched: &mut bool,
    ) -> Result<Ended> {
        let secret = self.credentials(a)?;
        let (session, interrupter, tap) =
            imap_login_interruptible(a, &secret, &self.tls_connector()?)?;
        *socket.lock().unwrap() = Some(interrupter);
        let mut c = Connection::open(session)?;
        if stop.load(Ordering::Acquire) {
            return Ok(Ended::Stopped);
        }
        let ext = c.extensions()?;
        if !ext.idle {
            let _ = c.session.logout();
            return Ok(Ended::Unsupported);
        }
        let names = c.names(false)?.to_vec();
        let remote = remote_name(&names, a, mailbox)?;
        let wire = c.wire(&remote);
        c.session.examine(&wire)?;
        // The inbox's watcher hears about every mailbox where it can.
        let inbox = *mailbox == Mailbox::Standard(Folder::Inbox);
        if inbox && ext.notify && notify_personal(&mut c.session, ext.condstore)? {
            notifying.store(true, Ordering::Release);
        }
        let utf8 = c.enabled.utf8;
        *watched = true;
        while !stop.load(Ordering::Acquire) {
            tap.start();
            let outcome = {
                let handle = c.session.idle()?;
                handle.wait_with_timeout(RENEW_IDLE)?
                // Dropping the handle ends the IDLE (DONE).
            };
            let heard = tap.take();
            // What the server said is acted on by the mailbox checks.
            while c.session.unsolicited_responses.try_recv().is_ok() {}
            if stop.load(Ordering::Acquire) {
                break;
            }
            if outcome != imap::extensions::idle::WaitOutcome::MailboxChanged {
                continue;
            }
            // NOTIFY's STATUS lines name other mailboxes that changed.
            // After an overflow the server has stopped notifying: every
            // mailbox may have changed, and NOTIFY is set again.
            if let Some(heard) = &heard
                && notification_overflow(heard)
            {
                for n in &names {
                    if !n.has("Noselect") && !n.has("NonExistent") {
                        changed(mailbox_of(&names, a.provider, &n.name));
                    }
                }
                let _ = notify_personal(&mut c.session, ext.condstore);
            }
            if let Some(heard) = &heard {
                for name in status_mailboxes(heard, utf8) {
                    if name != remote {
                        changed(mailbox_of(&names, a.provider, &name));
                    }
                }
            }
            // Any line ends the wait, keepalives too ("* OK still here"
            // in each server's words); only mailbox data is a change.
            if heard.as_deref().is_none_or(reports_change) {
                changed(mailbox.clone());
            }
        }
        let _ = c.session.logout();
        Ok(Ended::Stopped)
    }
}

/// The mailbox view a server mailbox is: one of the standard mailboxes, or
/// one of the account's own folders.
fn mailbox_of(names: &[Name], provider: Provider, remote: &str) -> Mailbox {
    if remote.eq_ignore_ascii_case("INBOX") {
        return Mailbox::Standard(Folder::Inbox);
    }
    for folder in [
        Folder::Sent,
        Folder::Drafts,
        Folder::Archive,
        Folder::Junk,
        Folder::Trash,
    ] {
        if resolve_in(names, provider, folder).is_ok_and(|r| r == remote) {
            return Mailbox::Standard(folder);
        }
    }
    Mailbox::Remote(remote.to_owned())
}

/// Whether what a server sent during IDLE reports a change in the watched
/// mailbox: untagged mailbox data (`* 3 EXISTS`, `* 2 EXPUNGE`, `* 5
/// FETCH`…), not status lines (`* OK`, `* NO`, `* BYE`), NOTIFY's STATUS
/// about other mailboxes, the `+ idling` go-ahead, or the tagged reply that
/// ends the IDLE. A copy cut off at its limit is taken to.
fn reports_change(heard: &[u8]) -> bool {
    if heard.len() >= crate::sync::stream::Tap::LIMIT {
        return true;
    }
    String::from_utf8_lossy(heard).lines().any(|line| {
        let mut words = line.split_ascii_whitespace();
        words.next() == Some("*")
            && words.next().is_some_and(|w| {
                !["OK", "NO", "BAD", "BYE", "PREAUTH", "STATUS"]
                    .iter()
                    .any(|s| s.eq_ignore_ascii_case(w))
            })
    })
}

#[cfg(test)]
mod tests {
    use super::reports_change;

    #[test]
    fn keepalives_are_not_new_mail() {
        let quiet = b"+ idling\r\n* OK Still here\r\n* OK NOOP\r\na7 OK IDLE terminated\r\n";
        assert!(!reports_change(quiet));
        assert!(!reports_change(b"+ idling\r\n* BYE going down\r\n"));
        assert!(!reports_change(b"* STATUS Work (MESSAGES 4)\r\n"));
        assert!(reports_change(b"+ idling\r\n* 4 EXISTS\r\na7 OK done\r\n"));
        assert!(reports_change(b"* 2 EXPUNGE\r\n"));
        assert!(reports_change(b"* VANISHED 4\r\n"));
        assert!(reports_change(b"* 1 FETCH (FLAGS (\\Seen))\r\n"));
        assert!(reports_change(&vec![b'x'; crate::sync::stream::Tap::LIMIT]));
    }
}

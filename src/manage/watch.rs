//! Push for new mail: one extra connection per account waits in IDLE
//! (RFC 2177) on the inbox, and says when the server reports a change.
//!
//! The watcher only notices; the regular inbox check then syncs, under the
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
}
impl Watcher {
    fn stop(&self) {
        self.stop.store(true, Ordering::Release);
        if let Some(socket) = self.socket.lock().unwrap().take() {
            let _ = socket.shutdown(std::net::Shutdown::Both);
        }
    }
}
/// Why a watch ended.
enum Ended {
    Stopped,
    /// The server has no IDLE; polling alone checks this account.
    Unsupported,
}

impl Backend {
    /// Watches the account's inbox for new mail with IDLE, calling
    /// `changed` whenever the server reports a change, until
    /// `stop_watching`. A watch already running is replaced. Returns at once.
    pub fn watch_inbox(self: &Arc<Self>, a: &Account, changed: impl Fn() + Send + 'static) {
        if a.provider == Provider::Pop3 {
            return;
        }
        self.stop_watching(&a.id);
        let watcher = Watcher {
            stop: Arc::new(AtomicBool::new(false)),
            socket: Arc::new(Mutex::new(None)),
        };
        let (stop, socket) = (watcher.stop.clone(), watcher.socket.clone());
        self.watchers.lock().unwrap().insert(a.id.clone(), watcher);
        let backend = self.clone();
        let id = a.id.clone();
        let a = a.clone();
        let spawned = std::thread::Builder::new()
            .name(format!("idle-{}", a.id))
            .spawn(move || {
                let mut retry = FIRST_RETRY;
                while !stop.load(Ordering::Acquire) {
                    // Whether this attempt got as far as waiting in IDLE.
                    let mut watched = false;
                    // A misbehaving server must not take the app down with it.
                    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        backend.idle(&a, &stop, &socket, &changed, &mut watched)
                    }))
                    .unwrap_or_else(|_| Err(anyhow::anyhow!("IDLE failed")));
                    match outcome {
                        Ok(Ended::Unsupported) => break,
                        Ok(Ended::Stopped) => break,
                        Err(error) => {
                            if stop.load(Ordering::Acquire) {
                                break;
                            }
                            // A watch that worked and then dropped (the
                            // network went away) starts backing off afresh.
                            if watched {
                                retry = FIRST_RETRY;
                            }
                            // Wrong credentials won't improve by retrying soon;
                            // the inbox checks report them.
                            let wait = if error.downcast_ref::<AccessFailure>().is_some()
                                || crate::sync::transport::throttled(&error)
                            {
                                LONGEST_RETRY
                            } else {
                                retry
                            };
                            retry = (retry * 2).min(LONGEST_RETRY);
                            let until = std::time::Instant::now() + wait;
                            while std::time::Instant::now() < until && !stop.load(Ordering::Acquire)
                            {
                                std::thread::sleep(Duration::from_millis(250));
                            }
                            // Mail may have arrived while the watch was down.
                            if !stop.load(Ordering::Acquire) {
                                changed();
                            }
                        }
                    }
                }
            });
        if spawned.is_err() {
            // Without a thread, polling checks this account.
            self.stop_watching(&id);
        }
    }
    /// Ends the account's watch, if any; its connection closes.
    pub fn stop_watching(&self, id: &str) {
        if let Some(w) = self.watchers.lock().unwrap().remove(id) {
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
    /// One IDLE connection's life: signs in, opens the inbox read-only and
    /// waits, renewing the IDLE now and then, until stopped or it fails.
    fn idle(
        &self,
        a: &Account,
        stop: &AtomicBool,
        socket: &Mutex<Option<TcpStream>>,
        changed: &dyn Fn(),
        watched: &mut bool,
    ) -> Result<Ended> {
        let secret = self.credentials(a)?;
        let (session, interrupter, tap) =
            imap_login_interruptible(a, &secret, &self.tls_connector()?)?;
        let mut c = Connection::new(session);
        *socket.lock().unwrap() = Some(interrupter);
        if stop.load(Ordering::Acquire) {
            return Ok(Ended::Stopped);
        }
        if !c.extensions()?.idle {
            let _ = c.session.logout();
            return Ok(Ended::Unsupported);
        }
        c.session.examine("INBOX")?;
        *watched = true;
        while !stop.load(Ordering::Acquire) {
            tap.start();
            let outcome = {
                let handle = c.session.idle()?;
                handle.wait_with_timeout(RENEW_IDLE)?
                // Dropping the handle ends the IDLE (DONE).
            };
            let heard = tap.take();
            // What the server said is acted on by the inbox check.
            while c.session.unsolicited_responses.try_recv().is_ok() {}
            if stop.load(Ordering::Acquire) {
                break;
            }
            // Any line ends the wait, keepalives too ("* OK still here"
            // in each server's words); only mailbox data is a change.
            if outcome == imap::extensions::idle::WaitOutcome::MailboxChanged
                && heard.as_deref().is_none_or(reports_change)
            {
                changed();
            }
        }
        let _ = c.session.logout();
        Ok(Ended::Stopped)
    }
}

/// Whether what a server sent during IDLE reports a change: untagged
/// mailbox data (`* 3 EXISTS`, `* 2 EXPUNGE`, `* 5 FETCH`…), not status
/// lines (`* OK`, `* NO`, `* BYE`), the `+ idling` go-ahead, or the tagged
/// reply that ends the IDLE. A copy cut off at its limit is taken to.
fn reports_change(heard: &[u8]) -> bool {
    if heard.len() >= crate::sync::stream::Tap::LIMIT {
        return true;
    }
    String::from_utf8_lossy(heard).lines().any(|line| {
        let mut words = line.split_ascii_whitespace();
        words.next() == Some("*")
            && words.next().is_some_and(|w| {
                !["OK", "NO", "BAD", "BYE", "PREAUTH"]
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
        assert!(reports_change(b"+ idling\r\n* 4 EXISTS\r\na7 OK done\r\n"));
        assert!(reports_change(b"* 2 EXPUNGE\r\n"));
        assert!(reports_change(b"* 1 FETCH (FLAGS (\\Seen))\r\n"));
        assert!(reports_change(&vec![b'x'; crate::sync::stream::Tap::LIMIT]));
    }
}

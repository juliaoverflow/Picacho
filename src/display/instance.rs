//! One Picacho per data directory. Picacho may be running with no window
//! (the background process, see `resident`), so launching it again must
//! not start a second app on the same mail cache: the new process asks the
//! running one to show its window, then exits.
//!
//! The running app listens on a socket in its data directory: a Unix socket,
//! or on Windows a loopback TCP port written to a file there, with a random
//! key so other local programs can't drive it.
//!
//! Becoming the running one takes a lock on a file beside the socket, held
//! for as long as the process runs (the system lets go of it if the process
//! dies): two launches at once can't both replace the socket and run on the
//! same mail cache.
use std::{
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    time::Duration,
};

/// What a second launch sends.
const SHOW: &str = "show";
/// What the background process sends to learn whether a Picacho is running,
/// without asking it to show anything.
const PING: &str = "ping";
/// Before a request a launch passes on (`launch::request`): a `mailto:`
/// address or an email file to open.
const OPEN: &str = "open ";

/// The outcome of [`claim`].
pub enum Claim {
    /// This is the only Picacho: it listens for later launches.
    Primary(Listener),
    /// Another Picacho is running and was asked to show its window.
    Forwarded,
    /// Another Picacho holds the lock but didn't answer: this one must not
    /// open the same mail cache.
    Busy,
}

/// The most a launch's message may be: a key and a word, or a request to
/// open. Anything longer is read no further.
const MAX_LINE: u64 = 128 + super::launch::MAX_REQUEST as u64;
/// How long a launch has to say it, in all.
const SAY_WITHIN: Duration = Duration::from_secs(2);

/// Listens for later launches; see [`Listener::serve`].
pub struct Listener {
    #[cfg(unix)]
    listener: std::os::unix::net::UnixListener,
    #[cfg(windows)]
    listener: std::net::TcpListener,
    #[cfg(windows)]
    key: String,
    /// Removed when the app quits (see [`release`]).
    path: PathBuf,
    /// The instance lock, held while this listens (`None` where the file
    /// system can't lock).
    _lock: Option<std::fs::File>,
}

/// How long a launch waits for a Picacho that holds the lock, but isn't
/// listening yet, to start listening.
const STARTING: Duration = Duration::from_secs(3);

fn lock_path(dir: &Path) -> PathBuf {
    dir.join("picacho.lock")
}

/// Takes the instance lock: `Ok(Some)` when this process now holds it,
/// `Ok(None)` when another does.
fn lock(dir: &Path) -> std::io::Result<Option<std::fs::File>> {
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(lock_path(dir))?;
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(std::fs::TryLockError::WouldBlock) => Ok(None),
        Err(std::fs::TryLockError::Error(e)) => Err(e),
    }
}

#[cfg(unix)]
fn socket_path(dir: &Path) -> PathBuf {
    dir.join("picacho.sock")
}
#[cfg(windows)]
fn socket_path(dir: &Path) -> PathBuf {
    dir.join("picacho.instance")
}

/// Hands over to a Picacho already using `dir`, or becomes the one others
/// hand over to. `None` when neither works (an unusable data directory, a
/// path too long for a socket); Picacho then just runs.
/// Makes the data directory readable by this user alone from the start:
/// made with the usual permissions, it was open to others until the store
/// restricted it, after the lock and socket were already made inside it.
fn private_dir(dir: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)
    }
    #[cfg(not(unix))]
    std::fs::create_dir_all(dir)
}

pub fn claim(dir: &Path, requests: &[String]) -> Option<Claim> {
    let path = socket_path(dir);
    // What this launch was to open goes to the running Picacho first.
    let pass_on = |path: &Path| -> std::io::Result<()> {
        for r in requests {
            forward(path, &format!("{OPEN}{r}"))?;
        }
        forward(path, SHOW)
    };
    if pass_on(&path).is_ok() {
        return Some(Claim::Forwarded);
    }
    private_dir(dir).ok()?;
    match lock(dir) {
        Ok(Some(held)) => listen(path, Some(held)).ok().map(Claim::Primary),
        // Another launch got here first and is starting: show its window
        // once it listens.
        // Or one is ending: the background process that started this one
        // (to show its window) lets go of the lock as it exits. The lock is
        // asked for again meanwhile; asked for only once, this launch said
        // Picacho wasn't responding after the moment it took to exit.
        Ok(None) => {
            let start = std::time::Instant::now();
            while start.elapsed() < STARTING {
                std::thread::sleep(Duration::from_millis(100));
                if pass_on(&path).is_ok() {
                    return Some(Claim::Forwarded);
                }
                if let Ok(Some(held)) = lock(dir) {
                    return listen(path, Some(held)).ok().map(Claim::Primary);
                }
            }
            Some(Claim::Busy)
        }
        // No locking here: as before, the socket alone.
        Err(_) => listen(path, None).ok().map(Claim::Primary),
    }
}

/// For the background process (see `resident`): listens for launches
/// unless a Picacho already runs on `dir`, which is left alone (`None`, as
/// when the socket can't be used).
pub fn claim_resident(dir: &Path) -> Option<Listener> {
    let path = socket_path(dir);
    if forward(&path, PING).is_ok() {
        return None;
    }
    private_dir(dir).ok()?;
    match lock(dir) {
        Ok(Some(held)) => listen(path, Some(held)).ok(),
        // A Picacho is starting on `dir`: it is left to run.
        Ok(None) => None,
        Err(_) => listen(path, None).ok(),
    }
}

#[cfg(unix)]
fn forward(path: &Path, message: &str) -> std::io::Result<()> {
    let mut stream = std::os::unix::net::UnixStream::connect(path)?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    writeln!(stream, "{message}")
}

#[cfg(unix)]
fn listen(path: PathBuf, lock: Option<std::fs::File>) -> std::io::Result<Listener> {
    use std::os::unix::fs::PermissionsExt;
    // Nobody answered: the file is left from a Picacho that didn't quit
    // cleanly.
    let _ = std::fs::remove_file(&path);
    let listener = std::os::unix::net::UnixListener::bind(&path)?;
    let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    Ok(Listener {
        listener,
        path,
        _lock: lock,
    })
}

#[cfg(windows)]
fn forward(path: &Path, message: &str) -> std::io::Result<()> {
    let saved = std::fs::read_to_string(path)?;
    let (port, key) = saved
        .trim()
        .split_once(' ')
        .ok_or(std::io::ErrorKind::InvalidData)?;
    let port: u16 = port.parse().map_err(|_| std::io::ErrorKind::InvalidData)?;
    let address = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let mut stream = std::net::TcpStream::connect_timeout(&address, Duration::from_secs(1))?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    writeln!(stream, "{key} {message}")
}

/// Windows has no Unix sockets for every version Picacho runs on: a port on
/// the loopback address, and a key only this user can read (the file is in
/// the data directory, limited to this user first), so other accounts on
/// the computer can't open Picacho's window.
#[cfg(windows)]
fn listen(path: PathBuf, lock: Option<std::fs::File>) -> std::io::Result<Listener> {
    // Best effort, as in `Store::open`: FAT and some network drives keep no
    // access lists, and Picacho still runs there.
    if let Some(dir) = path.parent() {
        let _ = super::windows::restrict_to_user(dir);
    }
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0))?;
    let port = listener.local_addr()?.port();
    let key = uuid::Uuid::new_v4().simple().to_string();
    std::fs::write(&path, format!("{port} {key}"))?;
    Ok(Listener {
        listener,
        key,
        path,
        _lock: lock,
    })
}

impl Listener {
    /// Answers later launches on a thread of its own for the rest of the
    /// process, calling `show` for each; it returns false once the app is
    /// gone. Returns the path to [`release`] when quitting.
    pub fn serve(self, mut show: impl FnMut() -> bool + Send + 'static) -> PathBuf {
        let path = self.path.clone();
        let spawned = std::thread::Builder::new()
            .name("picacho-instance".into())
            .spawn(move || {
                for stream in self.listener.incoming() {
                    let Ok(stream) = stream else {
                        continue;
                    };
                    // A launch that connects and says nothing, or a great
                    // deal slowly, can't hold the next one up for long: a
                    // few bytes, within a deadline for them all.
                    let Some(line) = read_message(stream) else {
                        continue;
                    };
                    #[cfg(windows)]
                    let line = match line.trim().split_once(' ') {
                        Some((key, rest)) if key == self.key => rest.to_owned(),
                        _ => continue,
                    };
                    if let Some(request) = line.strip_prefix(OPEN) {
                        // Checked again: anyone who can reach the socket
                        // could send anything.
                        if let Some(request) = super::launch::request(request) {
                            super::launch::push(request);
                        }
                        continue;
                    }
                    if line.trim() == SHOW && !show() {
                        // The app is gone.
                        break;
                    }
                }
            });
        if spawned.is_err() {
            eprintln!("Picacho: later launches can't reach this one");
        }
        path
    }
}

/// The line a launch sends, read within `SAY_WITHIN` and `MAX_LINE`.
fn read_message<S: Read + Timed>(stream: S) -> Option<String> {
    let deadline = std::time::Instant::now() + SAY_WITHIN;
    let mut reader = BufReader::new(stream.take(MAX_LINE));
    let mut line = Vec::new();
    loop {
        let left = deadline.checked_duration_since(std::time::Instant::now())?;
        if left.is_zero() {
            return None;
        }
        reader.get_ref().get_ref().timeout(left).ok()?;
        let buffer = reader.fill_buf().ok()?;
        if buffer.is_empty() {
            // Closed, or `MAX_LINE` reached.
            break;
        }
        if let Some(end) = buffer.iter().position(|&b| b == b'\n') {
            line.extend_from_slice(&buffer[..end]);
            break;
        }
        let n = buffer.len();
        line.extend_from_slice(buffer);
        reader.consume(n);
    }
    String::from_utf8(line).ok()
}

/// A stream whose reads can be given a timeout.
trait Timed {
    fn timeout(&self, left: Duration) -> std::io::Result<()>;
}
#[cfg(unix)]
impl Timed for std::os::unix::net::UnixStream {
    fn timeout(&self, left: Duration) -> std::io::Result<()> {
        self.set_read_timeout(Some(left))
    }
}
#[cfg(windows)]
impl Timed for std::net::TcpStream {
    fn timeout(&self, left: Duration) -> std::io::Result<()> {
        self.set_read_timeout(Some(left))
    }
}

impl Listener {
    /// Stops listening without serving, removing the socket.
    pub fn abandon(self) {
        release(&self.path);
    }
}

/// Removes the socket (or port file) as the app quits, so the next launch
/// starts afresh rather than trying to reach this one.
pub fn release(path: &Path) {
    let _ = std::fs::remove_file(path);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A launch's message is read to its line's end, and no further than
    /// a few bytes however much is sent.
    #[cfg(unix)]
    #[test]
    fn messages_are_short() {
        let (mut a, b) = std::os::unix::net::UnixStream::pair().unwrap();
        a.write_all(b"show\nmore").unwrap();
        assert_eq!(read_message(b).as_deref(), Some("show"));
        let (mut a, b) = std::os::unix::net::UnixStream::pair().unwrap();
        // Written from another thread: more than a socket's buffer may hold.
        let writer = std::thread::spawn(move || {
            let _ = a.write_all(&vec![b'x'; 2 * MAX_LINE as usize]);
        });
        assert_eq!(read_message(b).map(|l| l.len()), Some(MAX_LINE as usize));
        let _ = writer.join();
        // Saying nothing: given up on after the deadline.
        let (_a, b) = std::os::unix::net::UnixStream::pair().unwrap();
        let start = std::time::Instant::now();
        assert_eq!(read_message(b), None);
        assert!(start.elapsed() < SAY_WITHIN + Duration::from_secs(1));
    }

    #[test]
    fn one_process_holds_the_lock() {
        let dir = std::env::temp_dir().join(format!("picacho-instance-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let held = lock(&dir).unwrap();
        assert!(held.is_some());
        // Another holder (here, another handle) is refused while it's held.
        assert!(lock(&dir).unwrap().is_none());
        drop(held);
        assert!(lock(&dir).unwrap().is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Exercises the full background-process handover path:
    /// 1. Background process claims the resident listener and serves it.
    /// 2. A second launch forwards `mailto:`, `.eml`, `.msg`, and `notification:`
    ///    requests to the background process over the real IPC socket.
    /// 3. The background process wakes, saves pending requests to the private
    ///    handover file, and releases the socket + lock.
    /// 4. The newly started main window claims primary and consumes the handed-over
    ///    requests cleanly without needing them on the command line.
    #[test]
    fn background_handover_passes_requests_to_main_window() {
        let dir = std::env::temp_dir().join(format!("picacho-handover-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let eml = dir.join("note.eml");
        let msg = dir.join("outlook.msg");
        std::fs::write(&eml, "Subject: Hi\r\n\r\nHello").unwrap();
        std::fs::write(&msg, b"\xD0\xCF\x11\xE0\xA1\xB1\x1A\xE1").unwrap();

        // Drain any stale global requests from earlier tests.
        let _ = super::super::launch::take();

        let listener = claim_resident(&dir).expect("resident should claim fresh dir");
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        let socket = listener.serve(move || {
            let _ = tx.send(());
            // Stop serving after waking to hand over (as resident does on exit).
            false
        });

        let notif = format!(
            "{}{}",
            super::super::launch::NOTIFICATION,
            serde_json::to_string(&super::super::notify::Target {
                account: "acc1".into(),
                mailbox: crate::manage::model::Folder::Inbox.into(),
                message: Some("msg42".into()),
                messages: vec!["msg42".into()],
                action: Some("reply".into()),
            })
            .unwrap()
        );
        let forwarded_requests = vec![
            "mailto:alice@example.com?subject=Hello".to_owned(),
            super::super::launch::request(eml.to_str().unwrap()).unwrap(),
            super::super::launch::request(msg.to_str().unwrap()).unwrap(),
            notif.clone(),
        ];

        let outcome = claim(&dir, &forwarded_requests);
        assert!(matches!(outcome, Some(Claim::Forwarded)));

        // Resident received SHOW after all OPEN requests.
        rx.recv_timeout(Duration::from_secs(2))
            .expect("resident should wake on SHOW");

        // Resident persists queued requests into the private handover file and releases.
        let queued = super::super::launch::take();
        assert_eq!(queued.len(), 4);
        super::super::launch::save_pending(&dir, &queued).unwrap();
        release(&socket);

        // Give the listener thread a moment to exit and drop its lock file handle.
        let main_claim = claim(&dir, &[]);
        assert!(
            matches!(main_claim, Some(Claim::Primary(_))),
            "main window should claim primary after resident releases"
        );
        if let Some(Claim::Primary(l)) = main_claim {
            l.abandon();
        }

        // Main window loads and clears the handover file.
        let loaded = super::super::launch::load_pending(&dir);
        assert_eq!(loaded.len(), 4);
        assert_eq!(loaded[0], "mailto:alice@example.com?subject=Hello");
        assert!(loaded[1].starts_with("file:") && loaded[1].ends_with("note.eml"));
        assert!(loaded[2].starts_with("file:") && loaded[2].ends_with("outlook.msg"));
        assert_eq!(loaded[3], notif);
        // Second load must be empty (file removed on consume).
        assert!(super::super::launch::load_pending(&dir).is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }
}

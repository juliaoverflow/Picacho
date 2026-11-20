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
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    time::Duration,
};

/// What a second launch sends.
const SHOW: &str = "show";
/// What the background process sends to learn whether a Picacho is running,
/// without asking it to show anything.
const PING: &str = "ping";

/// The outcome of [`claim`].
pub enum Claim {
    /// This is the only Picacho: it listens for later launches.
    Primary(Listener),
    /// Another Picacho is running and was asked to show its window.
    Forwarded,
}

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
pub fn claim(dir: &Path) -> Option<Claim> {
    let path = socket_path(dir);
    if forward(&path, SHOW).is_ok() {
        return Some(Claim::Forwarded);
    }
    std::fs::create_dir_all(dir).ok()?;
    match lock(dir) {
        Ok(Some(held)) => listen(path, Some(held)).ok().map(Claim::Primary),
        // Another launch got here first and is starting: show its window
        // once it listens.
        Ok(None) => {
            let start = std::time::Instant::now();
            while start.elapsed() < STARTING {
                std::thread::sleep(Duration::from_millis(100));
                if forward(&path, SHOW).is_ok() {
                    return Some(Claim::Forwarded);
                }
            }
            None
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
    std::fs::create_dir_all(dir).ok()?;
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

#[cfg(windows)]
fn listen(path: PathBuf, lock: Option<std::fs::File>) -> std::io::Result<Listener> {
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
                    // A launch that connects and says nothing can't hold
                    // the next one up for long.
                    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                    let mut line = String::new();
                    if BufReader::new(stream).read_line(&mut line).is_err() {
                        continue;
                    }
                    #[cfg(windows)]
                    let line = match line.trim().split_once(' ') {
                        Some((key, rest)) if key == self.key => rest.to_owned(),
                        _ => continue,
                    };
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
}

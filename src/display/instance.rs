//! One Picacho per data directory. Picacho may be running with no window
//! (the background process, see `resident`), so launching it again must
//! not start a second app on the same mail cache: the new process asks the
//! running one to show its window, then exits.
//!
//! The running app listens on a socket in its data directory: a Unix socket,
//! or on Windows a loopback TCP port written to a file there, with a random
//! key so other local programs can't drive it.
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
    listen(path).ok().map(Claim::Primary)
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
    listen(path).ok()
}

#[cfg(unix)]
fn forward(path: &Path, message: &str) -> std::io::Result<()> {
    let mut stream = std::os::unix::net::UnixStream::connect(path)?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    writeln!(stream, "{message}")
}

#[cfg(unix)]
fn listen(path: PathBuf) -> std::io::Result<Listener> {
    use std::os::unix::fs::PermissionsExt;
    // Nobody answered: the file is left from a Picacho that didn't quit
    // cleanly.
    let _ = std::fs::remove_file(&path);
    let listener = std::os::unix::net::UnixListener::bind(&path)?;
    let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    Ok(Listener { listener, path })
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
fn listen(path: PathBuf) -> std::io::Result<Listener> {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0))?;
    let port = listener.local_addr()?.port();
    let key = uuid::Uuid::new_v4().simple().to_string();
    std::fs::write(&path, format!("{port} {key}"))?;
    Ok(Listener {
        listener,
        key,
        path,
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

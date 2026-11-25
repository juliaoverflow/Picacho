//! What Picacho was opened to do: write to a `mailto:` address (Picacho as
//! the default email app), show an email file (.eml or Outlook .msg), or start
//! a message with files attached (`attach:` and a path: a phone's Share sheet).
//! These come from the command line, from a later launch (passed on by
//! `instance`), from the background process handover file (`pending-launch.json`),
//! or from the system (macOS's open-URL and open-file events), and wait here
//! until the main window takes them.
use std::path::{Path, PathBuf};
use std::sync::Mutex;

static REQUESTS: Mutex<Vec<String>> = Mutex::new(Vec::new());
/// Where a new request is told: the window's, which takes it.
static WAKE: Mutex<Option<futures::channel::mpsc::UnboundedSender<()>>> = Mutex::new(None);

/// The longest request kept: a `mailto:` with a body, or a long path.
pub const MAX_REQUEST: usize = 8 * 1024;
/// Before a notification's target (`notify::Target`'s JSON): one tapped,
/// or its Reply pressed, while Picacho ran in the background, which
/// starts the window with it.
pub const NOTIFICATION: &str = "notification:";
/// Private handover file inside the data directory used when the background
/// process (`resident`) hands queued requests to the newly started main
/// window without exposing addresses, bodies or notification targets in
/// command-line arguments.
#[cfg(not(any(target_os = "android", target_os = "ios")))]
const PENDING_FILE: &str = "pending-launch.json";

/// Whether `arg` is something Picacho opens: a `mailto:` address, an
/// email file's path (.eml or .msg), or a `file:` URL of one. Returns it as kept.
pub fn request(arg: &str) -> Option<String> {
    let arg = arg.trim();
    if arg.is_empty() || arg.len() > MAX_REQUEST || arg.contains(['\n', '\r']) {
        return None;
    }
    if arg.get(..7).is_some_and(|p| p.eq_ignore_ascii_case("mailto:")) {
        return Some(arg.to_owned());
    }
    if crate::manage::widgets::WidgetAction::parse(arg).is_some() {
        return Some(arg.to_owned());
    }
    if let Some(target) = arg.strip_prefix(NOTIFICATION) {
        // Opens a message, or starts a reply to it, at most.
        return serde_json::from_str::<super::notify::Target>(target)
            .is_ok()
            .then(|| arg.to_owned());
    }
    if let Some(path) = arg.strip_prefix("attach:") {
        let path = Path::new(path);
        return path.is_file().then(|| format!("attach:{}", path.display()));
    }
    let path = match arg.get(..7) {
        Some(p) if p.eq_ignore_ascii_case("file://") => url::Url::parse(arg).ok()?.to_file_path().ok()?,
        _ => match arg.strip_prefix("file:") {
            Some(p) => PathBuf::from(p),
            None => PathBuf::from(arg),
        },
    };
    let email_file = path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("eml") || e.eq_ignore_ascii_case("msg"));
    (email_file && path.is_file()).then(|| {
        let path = std::fs::canonicalize(&path).unwrap_or(path);
        format!("file:{}", path.display())
    })
}

/// Parses OS command-line arguments (after argv[0]), extracting an optional
/// `--data-dir <dir>` without feeding the flag or directory path into
/// [`request`], and safely skipping non-UTF-8 arguments instead of panicking.
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub fn parse_args(
    args: impl IntoIterator<Item = std::ffi::OsString>,
) -> (Option<PathBuf>, Vec<String>) {
    let mut data_dir = None;
    let mut requests = Vec::new();
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        if arg == std::ffi::OsStr::new(super::resident::DATA_DIR_FLAG) {
            if let Some(dir) = iter.next() {
                data_dir = Some(PathBuf::from(dir));
            }
            continue;
        }
        if arg == std::ffi::OsStr::new("--widget") {
            requests.push("picacho://widget/panel".to_owned());
            continue;
        }
        if let Some(s) = arg.to_str()
            && let Some(req) = request(s)
        {
            requests.push(req);
        }
    }
    (data_dir, requests)
}

/// Persists queued launch requests into a private file inside `dir`
/// (`0600` on Unix, atomic rename) so the background process can hand them
/// over to the newly spawned window process without putting private URLs or
/// notification payloads in process command-line arguments.
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub fn save_pending(dir: &Path, requests: &[String]) -> std::io::Result<()> {
    if requests.is_empty() {
        return Ok(());
    }
    let path = dir.join(PENDING_FILE);
    let mut all: Vec<String> = std::fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    for r in requests {
        if let Some(valid) = request(r.strip_prefix("file:").unwrap_or(r)) {
            all.push(valid);
        }
    }
    if all.is_empty() {
        return Ok(());
    }
    let json = serde_json::to_vec(&all)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let partial = dir.join(format!("{PENDING_FILE}.partial"));
    {
        use std::io::Write as _;
        let mut opts = std::fs::OpenOptions::new();
        opts.create(true).write(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            opts.mode(0o600);
        }
        let mut file = opts.open(&partial)?;
        file.write_all(&json)?;
        file.sync_all()?;
    }
    std::fs::rename(&partial, &path)
}

/// Reads and immediately removes any requests handed over by the background
/// process via [`save_pending`], re-validating every entry.
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub fn load_pending(dir: &Path) -> Vec<String> {
    let path = dir.join(PENDING_FILE);
    let Ok(bytes) = std::fs::read(&path) else {
        return Vec::new();
    };
    let _ = std::fs::remove_file(&path);
    serde_json::from_slice::<Vec<String>>(&bytes)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|r| request(r.strip_prefix("file:").unwrap_or(&r)))
        .collect()
}

/// Keeps a request for the main window.
pub fn push(request: String) {
    REQUESTS.lock().unwrap_or_else(|e| e.into_inner()).push(request);
    if let Some(wake) = WAKE.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        let _ = wake.unbounded_send(());
    }
}

/// New requests from now on, for the window to `take`. A window opened
/// later takes them over.
pub fn wakes() -> futures::channel::mpsc::UnboundedReceiver<()> {
    let (sender, receiver) = futures::channel::mpsc::unbounded();
    *WAKE.lock().unwrap_or_else(|e| e.into_inner()) = Some(sender);
    receiver
}

/// The requests waiting, oldest first.
pub fn take() -> Vec<String> {
    std::mem::take(&mut *REQUESTS.lock().unwrap_or_else(|e| e.into_inner()))
}

/// macOS's open-URL events: `mailto:` and email files are kept here,
/// sign-in callbacks go to the browser sign-in.
pub fn opened(urls: Vec<String>) {
    let (mine, others): (Vec<String>, Vec<String>) =
        urls.into_iter().partition(|u| request(u).is_some());
    for u in mine {
        if let Some(r) = request(&u) {
            push(r);
        }
    }
    crate::sync::oauth::opened(others);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mailto_and_email_files_are_requests() {
        assert_eq!(request("MAILTO:a@example.com?subject=Hi").as_deref(), Some("MAILTO:a@example.com?subject=Hi"));
        assert_eq!(request("picacho://widget/inbox").as_deref(), Some("picacho://widget/inbox"));
        assert_eq!(request("picacho://widget/calendar").as_deref(), Some("picacho://widget/calendar"));
        assert_eq!(request("picacho://widget/compose?account=a1").as_deref(), Some("picacho://widget/compose?account=a1"));
        assert_eq!(request("picacho://callback?code=xyz"), None);
        assert_eq!(request("https://example.com"), None);
        assert_eq!(request("mailto:a@b\nc"), None);
        assert!(request(r#"notification:{"account":"a","mailbox":"x"}"#).is_none(), "not a target");
        let dir = std::env::temp_dir().join(format!("picacho-launch-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let eml = dir.join("note.eml");
        let msg = dir.join("outlook.msg");
        std::fs::write(&eml, "Subject: Hi\r\n\r\nHello").unwrap();
        std::fs::write(&msg, b"\xD0\xCF\x11\xE0\xA1\xB1\x1A\xE1").unwrap();
        let eml_req = request(eml.to_str().unwrap()).unwrap();
        assert!(eml_req.starts_with("file:"));
        // Idempotent when forwarded over IPC (`open file:/path/to/note.eml`).
        assert_eq!(request(&eml_req).as_deref(), Some(eml_req.as_str()));
        let msg_req = request(msg.to_str().unwrap()).unwrap();
        assert!(msg_req.starts_with("file:"));
        assert_eq!(request(&msg_req).as_deref(), Some(msg_req.as_str()));
        assert_eq!(request(dir.join("missing.eml").to_str().unwrap()), None);
        assert_eq!(request(dir.join("missing.msg").to_str().unwrap()), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    #[test]
    fn parse_args_extracts_data_dir_and_requests() {
        let dir = std::env::temp_dir().join(format!("picacho-args-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let msg = dir.join("sample.msg");
        std::fs::write(&msg, b"\xD0\xCF\x11\xE0\xA1\xB1\x1A\xE1").unwrap();
        let args = vec![
            std::ffi::OsString::from(super::super::resident::DATA_DIR_FLAG),
            dir.as_os_str().to_owned(),
            std::ffi::OsString::from("mailto:bo@example.com"),
            msg.as_os_str().to_owned(),
            std::ffi::OsString::from("--widget"),
            std::ffi::OsString::from("--ignored"),
        ];
        let (chosen, reqs) = parse_args(args);
        assert_eq!(chosen.as_deref(), Some(dir.as_path()));
        assert_eq!(reqs.len(), 3);
        assert_eq!(reqs[0], "mailto:bo@example.com");
        assert!(reqs[1].starts_with("file:") && reqs[1].ends_with("sample.msg"));
        assert_eq!(reqs[2], "picacho://widget/panel");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    #[test]
    fn pending_handover_round_trips_and_clears_file() {
        let dir = std::env::temp_dir().join(format!("picacho-pending-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let eml = dir.join("a.eml");
        std::fs::write(&eml, "Subject: A\r\n\r\nBody").unwrap();
        let file_req = request(eml.to_str().unwrap()).unwrap();
        save_pending(&dir, &["mailto:one@example.com".into(), file_req.clone()]).unwrap();
        save_pending(&dir, &["mailto:two@example.com".into()]).unwrap();
        let loaded = load_pending(&dir);
        assert_eq!(
            loaded,
            vec![
                "mailto:one@example.com".to_owned(),
                file_req,
                "mailto:two@example.com".to_owned()
            ]
        );
        assert!(!dir.join(PENDING_FILE).exists());
        assert!(load_pending(&dir).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

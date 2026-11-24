//! What Picacho was opened to do: write to a `mailto:` address (Picacho as
//! the default email app), show an email file (.eml), or start a message
//! with files attached (`attach:` and a path: a phone's Share sheet). These come from
//! the command line, from a later launch (passed on by `instance`), or from
//! the system (macOS's open-URL and open-file events), and wait here until
//! the main window takes them.
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

/// Whether `arg` is something Picacho opens: a `mailto:` address, an
/// email file's path, or a `file:` URL of one. Returns it as kept.
pub fn request(arg: &str) -> Option<String> {
    let arg = arg.trim();
    if arg.is_empty() || arg.len() > MAX_REQUEST || arg.contains(['\n', '\r']) {
        return None;
    }
    if arg.get(..7).is_some_and(|p| p.eq_ignore_ascii_case("mailto:")) {
        return Some(arg.to_owned());
    }
    if let Some(target) = arg.strip_prefix(NOTIFICATION) {
        // Opens a message, or starts a reply to it, at most.
        return serde_json::from_str::<super::notify::Target>(target)
            .is_ok()
            .then(|| arg.to_owned());
    }
    if let Some(path) = arg.strip_prefix("attach:") {
        let path = std::path::Path::new(path);
        return path.is_file().then(|| format!("attach:{}", path.display()));
    }
    let path = match arg.get(..7) {
        Some(p) if p.eq_ignore_ascii_case("file://") => url::Url::parse(arg).ok()?.to_file_path().ok()?,
        _ => std::path::PathBuf::from(arg),
    };
    let email_file = path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("eml"));
    (email_file && path.is_file()).then(|| {
        let path = std::fs::canonicalize(&path).unwrap_or(path);
        format!("file:{}", path.display())
    })
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
        assert_eq!(request("https://example.com"), None);
        assert_eq!(request("mailto:a@b\nc"), None);
        assert!(request(r#"notification:{"account":"a","mailbox":"x"}"#).is_none(), "not a target");
        let dir = std::env::temp_dir().join(format!("picacho-launch-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("note.eml");
        std::fs::write(&file, "Subject: Hi\r\n\r\nHello").unwrap();
        assert!(request(file.to_str().unwrap()).is_some_and(|r| r.starts_with("file:")));
        assert_eq!(request(dir.join("missing.eml").to_str().unwrap()), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

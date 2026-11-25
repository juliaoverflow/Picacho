//! Where attachments open from. Another app needs a file to open, so an
//! attachment opens from the file it was saved as (Save…), while that file
//! is still what was saved; otherwise from Picacho's own copy in the
//! `attachments` folder beside its cache, written the first time the
//! attachment opens and opened again from there after, without being
//! downloaded or written again. Copies nobody opened for 30 days are
//! removed (`Backend::tidy_files`), as are a message's when it is deleted
//! for good from this computer's cache.
use super::*;
use sha2::Digest as _;
use std::path::{Path, PathBuf};

/// The preference remembering where attachments were saved.
const SAVED: &str = "saved_attachments";
/// Copies unopened this long are removed.
const KEEP: std::time::Duration = std::time::Duration::from_secs(30 * 86_400);

/// A file an attachment was saved as, and what it held then.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct Saved {
    path: PathBuf,
    size: u64,
    sha256: String,
}

fn digest(bytes: &[u8]) -> String {
    sha2::Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The key of message `id`'s attachment `part`.
fn key(id: &str, part: usize) -> String {
    format!("{id}#{part}")
}

impl Backend {
    /// The folder of Picacho's own copies of opened attachments.
    pub fn opened_attachments(&self) -> PathBuf {
        self.store.path.join("attachments")
    }

    /// A file holding `m`'s attachment `part`, to hand to another app: the
    /// file it was saved as while that's unchanged, else Picacho's copy,
    /// written now if it isn't there yet.
    pub fn attachment_file(&self, a: &Account, m: &Mail, part: usize, name: &str) -> Result<PathBuf> {
        if let Some(saved) = self.saved_attachment(&m.id, part) {
            return Ok(saved);
        }
        let dir = self
            .opened_attachments()
            .join(&digest(key(&m.id, part).as_bytes())[..24]);
        let path = dir.join(safe_file_name(name));
        let expected = m
            .attachments()
            .iter()
            .find(|x| x.part == part)
            .map(|x| x.size as u64);
        if let Ok(meta) = std::fs::metadata(&path)
            && meta.is_file()
            && expected.is_none_or(|size| size == 0 || size == meta.len())
        {
            // Opened again: kept another 30 days from now.
            let _ = std::fs::File::options()
                .append(true)
                .open(&path)
                .and_then(|f| f.set_modified(std::time::SystemTime::now()));
            return Ok(path);
        }
        let bytes = self.attachment(a, m, part)?;
        create_private_dir(&dir)?;
        // Written whole, then named, so a half-written copy is never opened.
        let partial = dir.join(format!(".{}.partial", uuid::Uuid::new_v4().simple()));
        std::fs::write(&partial, &bytes)?;
        std::fs::rename(&partial, &path)?;
        Ok(path)
    }

    /// Remembers that `m`'s attachment `part` was saved as `path`, holding
    /// `bytes`, so opening it opens that file.
    pub fn note_saved_attachment(&self, m: &Mail, part: usize, path: &Path, bytes: &[u8]) {
        let mut saved = self.saved_attachments();
        saved.insert(
            key(&m.id, part),
            Saved {
                path: path.to_path_buf(),
                size: bytes.len() as u64,
                sha256: digest(bytes),
            },
        );
        // Only the latest few hundred are remembered.
        while saved.len() > 500 {
            let Some(first) = saved.keys().next().cloned() else { break };
            saved.remove(&first);
        }
        if let Ok(json) = serde_json::to_string(&saved) {
            let _ = self.store.set_preference(SAVED, &json);
        }
    }

    fn saved_attachments(&self) -> std::collections::BTreeMap<String, Saved> {
        self.store
            .preference(SAVED)
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_default()
    }

    /// The file `id`'s attachment `part` was saved as, while it still holds
    /// what was saved.
    fn saved_attachment(&self, id: &str, part: usize) -> Option<PathBuf> {
        let saved = self.saved_attachments().remove(&key(id, part))?;
        let meta = std::fs::metadata(&saved.path).ok()?;
        if !meta.is_file() || meta.len() != saved.size {
            return None;
        }
        let bytes = std::fs::read(&saved.path).ok()?;
        (digest(&bytes) == saved.sha256).then_some(saved.path)
    }

    /// Removes Picacho's copies of opened attachments for `mails` (when
    /// deleted for good from this computer's cache).
    pub(super) fn remove_opened_attachments(&self, mails: &[Mail]) {
        for m in mails {
            for a in m.attachments() {
                let dir = self
                    .opened_attachments()
                    .join(&digest(key(&m.id, a.part).as_bytes())[..24]);
                let _ = std::fs::remove_dir_all(dir);
            }
        }
    }

    /// Removes copies of opened attachments nobody opened for 30 days.
    pub(super) fn tidy_opened_attachments(&self) {
        let Ok(dirs) = std::fs::read_dir(self.opened_attachments()) else {
            return;
        };
        for dir in dirs.flatten() {
            let path = dir.path();
            large::remove_older_than(&path, KEEP, |_| true);
            // Empty now: gone too.
            let _ = std::fs::remove_dir(&path);
        }
    }
}

/// An attachment's name (a sender's choice) made safe to suggest for a
/// file: no folders (`/` or `\\`, on every system), no control or
/// direction-changing characters (U+202E shows "doc\u{202E}fdp.exe" as
/// "docexe.pdf"), and not hidden by a leading dot.
pub fn safe_file_name(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or_default();
    let cleaned: String = base
        .chars()
        .filter(|c| {
            !c.is_control()
                && !matches!(c, '\u{200E}' | '\u{200F}' | '\u{061C}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
        })
        .map(|c| if matches!(c, ':' | '*' | '?' | '"' | '<' | '>' | '|') { '_' } else { c })
        .collect();
    let cleaned = cleaned.trim().trim_start_matches('.').trim();
    if cleaned.is_empty() { "attachment".into() } else { cleaned.to_owned() }
}

/// A folder readable by you alone, where the system allows.
fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)
    }
    #[cfg(not(unix))]
    std::fs::create_dir_all(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attachments_open_from_their_saved_file_or_one_copy() -> Result<()> {
        let path = std::env::temp_dir().join(format!("picacho-opened-{}", uuid::Uuid::new_v4()));
        let b = Backend::new(Arc::new(crate::manage::store::Store::open(&path)?));
        let a = Account::new(Provider::Pop3);
        let raw = b"From: a@example.org\r\nSubject: File\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=b\r\n\r\n--b\r\nContent-Type: text/plain\r\n\r\nSee attached\r\n--b\r\nContent-Type: text/plain; name=notes.txt\r\nContent-Disposition: attachment; filename=notes.txt\r\n\r\nThe notes\r\n--b--\r\n";
        let id = format!("{}:pop:1", a.id);
        let m = parse_message(raw, &a, &id, Folder::Inbox, "", 0)?;
        b.store.save_messages(std::slice::from_ref(&m))?;
        b.store.save_raw(&id, raw)?;
        let part = m.attachments()[0].part;

        let first = b.attachment_file(&a, &m, part, "notes.txt")?;
        assert!(first.starts_with(b.opened_attachments()));
        assert_eq!(first.file_name().unwrap(), "notes.txt");
        let bytes = std::fs::read(&first)?;
        assert_eq!(bytes, b"The notes");
        // Opened again: the same file, not written again.
        std::fs::write(&first, b"The notes")?;
        let when = std::fs::metadata(&first)?.len();
        assert_eq!(b.attachment_file(&a, &m, part, "notes.txt")?, first);
        assert_eq!(std::fs::metadata(&first)?.len(), when);

        // Saved: opens from there, until the file there changes.
        let saved = path.join("Saved notes.txt");
        std::fs::write(&saved, &bytes)?;
        b.note_saved_attachment(&m, part, &saved, &bytes);
        assert_eq!(b.attachment_file(&a, &m, part, "notes.txt")?, saved);
        std::fs::write(&saved, b"Changed")?;
        assert_eq!(b.attachment_file(&a, &m, part, "notes.txt")?, first);
        drop(b);
        std::fs::remove_dir_all(path)?;
        Ok(())
    }
}

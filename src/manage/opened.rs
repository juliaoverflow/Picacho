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
/// Copies of attachments of encrypted mail, unopened this long: decrypted,
/// they lie outside the (perhaps encrypted) database, readable on disk.
const KEEP_SEALED: std::time::Duration = std::time::Duration::from_secs(86_400);
/// Marks a copy's folder as one of encrypted mail (`KEEP_SEALED`).
const SEALED_MARK: &str = ".sealed";

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

/// A file being written, hashed (SHA-256) as it is.
struct Hashing {
    file: std::io::BufWriter<std::fs::File>,
    hasher: sha2::Sha256,
}
impl std::io::Write for Hashing {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let n = self.file.write(bytes)?;
        self.hasher.update(&bytes[..n]);
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}

/// A file an attachment was written to: its size and SHA-256 (hex).
pub struct Written {
    pub size: u64,
    pub sha256: String,
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
            // A copy made before copies were marked is marked now.
            mark_from_internet(&path);
            return Ok(path);
        }
        create_private_dir(&dir)?;
        // Of encrypted mail: kept a day, not a month (`tidy_opened_attachments`).
        let sealed = self
            .store
            .raw(&m.id)
            .ok()
            .flatten()
            .is_some_and(|raw| crate::manage::crypto::is_encrypted(&raw));
        if sealed {
            let _ = std::fs::write(dir.join(SEALED_MARK), b"");
        }
        // Written whole, then named, so a half-written copy is never opened.
        let partial = dir.join(format!(".{}.partial", uuid::Uuid::new_v4().simple()));
        self.attachment_to(a, m, part, &partial)?;
        std::fs::rename(&partial, &path)?;
        mark_from_internet(&path);
        Ok(path)
    }

    /// Writes `m`'s attachment `part` to the file `path`. One of a message
    /// too large to keep, from an IMAP server, goes to the file as it
    /// downloads and is never held whole in memory; others as `attachment`
    /// gives them. A file left half-written by a failure is removed.
    pub fn attachment_to(&self, a: &Account, m: &Mail, part: usize, path: &Path) -> Result<Written> {
        use std::io::Write as _;
        let mut out = Hashing {
            file: std::io::BufWriter::new(std::fs::File::create(path)?),
            hasher: sha2::Sha256::new(),
        };
        let mut write = || -> Result<u64> {
            // Proton's come whole from its API, to be decrypted, and are
            // at most 25 MB: as `attachment` gives them.
            if let Some(attachment) = m.attachments().iter().find(|x| x.part == part && !x.section.is_empty())
                && a.provider != Provider::Proton
                && !self.store.has_raw(&m.id)?
            {
                let lock = self.account_lock(&a.id);
                let _operation = lock.lock().unwrap();
                let attachment = attachment.clone();
                if a.provider == Provider::Jmap {
                    return self.jmap_attachment_into(a, &attachment, &mut out);
                }
                return self.with_imap(a, |c| {
                    c.select_for(m)?;
                    self.fetch_attachment_into(c, m, &attachment, &mut out)
                });
            }
            let bytes = self.attachment(a, m, part)?;
            out.write_all(&bytes)?;
            Ok(bytes.len() as u64)
        };
        let written = write().and_then(|size| {
            out.flush()?;
            Ok(size)
        });
        let size = match written {
            Ok(size) => size,
            Err(e) => {
                drop(out);
                let _ = std::fs::remove_file(path);
                return Err(e);
            }
        };
        let sha256 = out.hasher.finalize().iter().map(|b| format!("{b:02x}")).collect();
        Ok(Written { size, sha256 })
    }

    /// Remembers that `m`'s attachment `part` was saved as `path`, holding
    /// what `written` says, so opening it opens that file.
    pub fn note_saved_attachment(&self, m: &Mail, part: usize, path: &Path, written: &Written) {
        let mut saved = self.saved_attachments();
        saved.insert(
            key(&m.id, part),
            Saved {
                path: path.to_path_buf(),
                size: written.size,
                sha256: written.sha256.clone(),
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
        // Hashed as it's read, not read whole first (it may be large).
        let mut hasher = sha2::Sha256::new();
        let mut file = std::fs::File::open(&saved.path).ok()?;
        let mut buffer = vec![0u8; 256 * 1024];
        loop {
            use std::io::Read as _;
            let n = file.read(&mut buffer).ok()?;
            if n == 0 {
                break;
            }
            hasher.update(&buffer[..n]);
        }
        let hash: String = hasher.finalize().iter().map(|b| format!("{b:02x}")).collect();
        (hash == saved.sha256).then_some(saved.path)
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

    /// Removes every copy of an opened attachment (they're written again
    /// when opened): once the database is encrypted, decrypted copies of
    /// attachments outside it would still lie readable on disk.
    pub fn forget_opened_attachments(&self) {
        let _ = std::fs::remove_dir_all(self.opened_attachments());
    }

    /// Removes copies of opened attachments nobody opened for 30 days (of
    /// encrypted mail, for a day).
    pub(super) fn tidy_opened_attachments(&self) {
        let Ok(dirs) = std::fs::read_dir(self.opened_attachments()) else {
            return;
        };
        for dir in dirs.flatten() {
            let path = dir.path();
            let keep = if path.join(SEALED_MARK).exists() { KEEP_SEALED } else { KEEP };
            large::remove_older_than(&path, keep, |file| file.file_name() != Some(std::ffi::OsStr::new(SEALED_MARK)));
            // Only the mark left: the folder goes too.
            if std::fs::read_dir(&path).is_ok_and(|mut d| {
                d.all(|e| e.is_ok_and(|e| e.file_name() == SEALED_MARK))
            }) {
                let _ = std::fs::remove_file(path.join(SEALED_MARK));
            }
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
    // Not ending in dots or spaces either: Windows drops them, so
    // "invoice.exe." would be saved, and run, as "invoice.exe".
    let cleaned = cleaned.trim().trim_start_matches('.').trim_end_matches(['.', ' ']).trim();
    if cleaned.is_empty() {
        return "attachment".into();
    }
    // Nor a Windows device name, with any extension ("CON", "nul.txt",
    // "COM1.png"): writing to one writes to the device, not a file.
    let stem = cleaned.split('.').next().unwrap_or_default().trim_end().to_ascii_uppercase();
    let device = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$")
        || (stem.chars().count() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && matches!(stem.chars().nth(3), Some('0'..='9' | '¹' | '²' | '³')));
    if device { format!("_{cleaned}") } else { cleaned.to_owned() }
}

/// Marks `path` (an attachment written from an email) as having come from
/// the internet, as browsers mark their downloads: Windows' Zone.Identifier
/// stream (Mark of the Web, which SmartScreen and Office's Protected View
/// go by) and macOS's quarantine attribute (which Gatekeeper goes by).
/// Unmarked, a program or script attached to an email ran without either
/// asking. Best effort: a disk without streams or attributes keeps none.
pub fn mark_from_internet(path: &Path) {
    #[cfg(windows)]
    {
        let mut stream = path.as_os_str().to_owned();
        stream.push(":Zone.Identifier");
        let _ = std::fs::write(PathBuf::from(stream), "[ZoneTransfer]\r\nZoneId=3\r\n");
    }
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::ffi::OsStrExt as _;
        unsafe extern "C" {
            fn setxattr(
                path: *const std::ffi::c_char,
                name: *const std::ffi::c_char,
                value: *const std::ffi::c_void,
                size: usize,
                position: u32,
                options: std::ffi::c_int,
            ) -> std::ffi::c_int;
        }
        let Ok(c_path) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
            return;
        };
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        // Flags (downloaded, not yet approved), time, the app that wrote it.
        let value = format!("0081;{now:08x};Picacho;");
        // SAFETY: both strings are NUL-terminated and `value` outlives the
        // call, which reads `value.len()` bytes of it.
        unsafe {
            setxattr(
                c_path.as_ptr(),
                c"com.apple.quarantine".as_ptr(),
                value.as_ptr().cast(),
                value.len(),
                0,
                0,
            );
        }
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    let _ = path;
}

/// Whether a file named `name` is a program, script, shortcut or disk image
/// that opening would run (or mount) rather than show: such an attachment is
/// shown in its folder instead of opened with one click.
pub fn runs_code(name: &str) -> bool {
    const KINDS: &[&str] = &[
        // Windows: programs, installers, scripts, shortcuts, disk images.
        "exe", "com", "scr", "pif", "bat", "cmd", "msi", "msp", "mst", "msix", "msixbundle", "appx",
        "appxbundle", "appinstaller", "application", "lnk", "url", "hta", "js", "jse", "vbs", "vbe",
        "wsf", "wsh", "ws", "wsc", "ps1", "psm1", "psd1", "ps1xml", "reg", "cpl", "inf", "scf", "chm",
        "jar", "jnlp", "gadget", "iso", "img", "vhd", "vhdx", "settingcontent-ms", "library-ms",
        "search-ms", "xll", "xbap", "msc", "diagcab",
        // macOS.
        "app", "command", "tool", "terminal", "pkg", "mpkg", "workflow", "scpt", "applescript",
        "fileloc", "inetloc", "webloc", "dmg", "sparseimage", "sparsebundle",
        // Linux and other Unix.
        "desktop", "sh", "bash", "zsh", "csh", "run", "bin", "appimage", "deb", "rpm",
        "flatpakref", "flatpak", "snap",
        // Android.
        "apk", "apks", "xapk",
    ];
    let name = name.trim_end_matches(['.', ' ']);
    name.rsplit_once('.')
        .is_some_and(|(_, ext)| KINDS.iter().any(|k| ext.eq_ignore_ascii_case(k)))
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

    /// Programs and scripts are told by their last extension, whatever its
    /// case and however Windows would trim it; documents aren't.
    #[test]
    fn programs_are_told_from_documents() {
        for name in ["invoice.exe", "INVOICE.EXE", "a.pdf.lnk", "x.js", "run.command", "y.exe. ", "z.desktop"] {
            assert!(runs_code(name), "{name}");
        }
        for name in ["report.pdf", "photo.jpg", "notes.txt", "exe", "archive.zip", "sheet.xlsx"] {
            assert!(!runs_code(name), "{name}");
        }
        assert_eq!(safe_file_name("invoice.exe. . "), "invoice.exe");
    }

    /// A name from mail is a file in the folder it's saved to: no path out
    /// of it, no Windows stream, no device.
    #[test]
    fn file_names_stay_in_their_folder() {
        assert_eq!(safe_file_name("..\\..\\Startup\\run.bat"), "run.bat");
        assert_eq!(safe_file_name("../../.bashrc"), "bashrc");
        assert_eq!(safe_file_name("x.txt:Zone.Identifier"), "x.txt_Zone.Identifier");
        assert_eq!(safe_file_name("CON"), "_CON");
        assert_eq!(safe_file_name("nul.txt"), "_nul.txt");
        assert_eq!(safe_file_name("com1.png"), "_com1.png");
        assert_eq!(safe_file_name("console.txt"), "console.txt");
        assert_eq!(safe_file_name(".."), "attachment");
        assert_eq!(crate::manage::backend::picture_extension("image/..\\..\\x"), "x");
        assert_eq!(crate::manage::backend::picture_extension("image/svg+xml"), "svg");
        assert_eq!(crate::manage::backend::picture_extension("image/"), "png");
    }

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
        let written = b.attachment_to(&a, &m, part, &saved)?;
        assert_eq!((written.size, written.sha256.as_str()), (9, digest(b"The notes").as_str()));
        assert_eq!(std::fs::read(&saved)?, bytes);
        b.note_saved_attachment(&m, part, &saved, &written);
        assert_eq!(b.attachment_file(&a, &m, part, "notes.txt")?, saved);
        std::fs::write(&saved, b"Changed")?;
        assert_eq!(b.attachment_file(&a, &m, part, "notes.txt")?, first);
        drop(b);
        std::fs::remove_dir_all(path)?;
        Ok(())
    }
}

//! Files beside the database that hold mail content, kept encrypted while
//! the database is: the attachments of drafts found on the server (among
//! them drafts sealed with your OpenPGP key or S/MIME certificate, which
//! are decrypted to be edited) lie in `draft-files`, outside the database,
//! and were readable on disk whatever its encryption.
//!
//! A sealed file is `MAGIC`, a 12-byte nonce, and the contents encrypted
//! with AES-256-GCM under a key derived (HKDF-SHA256) from the database's
//! key, so it lasts exactly as long as the database's encryption. Its name
//! stays the attachment's, which the composer shows and the sent message
//! names. Everything that reads a draft's files reads them through `read`,
//! which opens a sealed file and passes a plain one through. Turning the
//! database's encryption on seals the files there; turning it off opens
//! them again before the key is forgotten (`Store::set_encrypted`).
use aes_gcm::{
    Aes256Gcm, KeyInit, Nonce,
    aead::{Aead, Payload},
};
use anyhow::{Context, Result, ensure};
use std::path::{Path, PathBuf};
use zeroize::Zeroizing;

/// What a sealed file starts with.
const MAGIC: &[u8; 16] = b"PICACHO-SEALED1\n";
const NONCE: usize = 12;
const TAG: usize = 16;
/// The key's label in its derivation from the database's key.
pub(crate) const KEY_INFO: &str = "picacho draft files v1";

type Key = Zeroizing<[u8; 32]>;

/// The file key of each open store's data directory, while its database is
/// encrypted. By directory, so stores in other directories (tests open
/// many at once) each have their own.
static KEYS: std::sync::LazyLock<std::sync::Mutex<Vec<(PathBuf, Key)>>> = std::sync::LazyLock::new(Default::default);

/// Notes the file key of the store in `root` (None: its database isn't
/// encrypted, and new files are written plain).
pub(crate) fn set_key(root: &Path, key: Option<Key>) {
    let mut keys = KEYS.lock().unwrap_or_else(|e| e.into_inner());
    keys.retain(|(r, _)| r != root);
    if let Some(key) = key {
        keys.push((root.to_path_buf(), key));
    }
}

/// The file key for `path`: that of the store whose directory holds it.
fn key_for(path: &Path) -> Option<Key> {
    let keys = KEYS.lock().unwrap_or_else(|e| e.into_inner());
    keys.iter().filter(|(root, _)| path.starts_with(root)).max_by_key(|(root, _)| root.as_os_str().len()).map(|(_, k)| k.clone())
}

fn seal(key: &[u8; 32], plain: &[u8]) -> Result<Vec<u8>> {
    let nonce: [u8; NONCE] = rand::random();
    let cipher = Aes256Gcm::new_from_slice(key).expect("32-byte key");
    let sealed = cipher
        .encrypt(&Nonce::try_from(&nonce[..]).expect("12 bytes"), Payload { msg: plain, aad: MAGIC })
        .map_err(|_| anyhow::anyhow!(t!("manage-sealed-files-couldnt-encrypt-a-file")))?;
    let mut out = Vec::with_capacity(MAGIC.len() + NONCE + sealed.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&sealed);
    Ok(out)
}

fn open(key: &[u8; 32], data: &[u8]) -> Result<Vec<u8>> {
    ensure!(data.len() >= MAGIC.len() + NONCE + TAG, t!("manage-sealed-files-an-encrypted-file-is-damaged"));
    let (nonce, sealed) = data[MAGIC.len()..].split_at(NONCE);
    let cipher = Aes256Gcm::new_from_slice(key).expect("32-byte key");
    cipher
        .decrypt(&Nonce::try_from(nonce).expect("12 bytes"), Payload { msg: sealed, aad: MAGIC })
        .map_err(|_| anyhow::anyhow!(t!("manage-sealed-files-an-encrypted-file-is-damaged")))
}

fn is_sealed(data: &[u8]) -> bool {
    data.starts_with(MAGIC)
}

/// Writes `bytes` to `path`: sealed while the database of the store whose
/// directory holds it is encrypted, else as they are.
pub fn write(path: &Path, bytes: &[u8]) -> Result<()> {
    match key_for(path) {
        Some(key) => std::fs::write(path, seal(&key, bytes)?)?,
        None => std::fs::write(path, bytes)?,
    }
    Ok(())
}

/// The contents of the file at `path`, opened where it is sealed.
pub fn read(path: &Path) -> Result<Vec<u8>> {
    let data = std::fs::read(path)?;
    if !is_sealed(&data) {
        return Ok(data);
    }
    let key = key_for(path).with_context(|| t!("manage-sealed-files-this-file-is-encrypted-with-a-key"))?;
    open(&key, &data)
}

/// The size of the contents of the file at `path` (a sealed file's without
/// what sealing adds), reading only its start.
pub fn len(path: &Path) -> std::io::Result<u64> {
    use std::io::Read as _;
    let size = std::fs::metadata(path)?.len();
    let mut head = [0u8; MAGIC.len()];
    let sealed = std::fs::File::open(path).and_then(|mut f| f.read_exact(&mut head)).is_ok() && is_sealed(&head);
    Ok(if sealed { size.saturating_sub((MAGIC.len() + NONCE + TAG) as u64) } else { size })
}

/// Rewrites every file under `dir`: opened with `from` where sealed, then
/// sealed with `to` (or left plain without one). Each is written beside
/// itself and then put in its place, so a file is never half-written. A
/// file that can't be opened (sealed with another key) is left as it is.
/// Returns how many files couldn't be rewritten.
pub(crate) fn reseal_dir(dir: &Path, from: Option<&[u8; 32]>, to: Option<&[u8; 32]>) -> usize {
    let mut failed = 0;
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else { continue };
        if kind.is_dir() {
            failed += reseal_dir(&path, from, to);
            continue;
        }
        if !kind.is_file() {
            continue;
        }
        let rewritten = (|| -> Result<()> {
            let data = std::fs::read(&path)?;
            let sealed = is_sealed(&data);
            // Already as wanted.
            if sealed == to.is_some() && (!sealed || from.is_none_or(|f| Some(f) == to)) {
                return Ok(());
            }
            let plain = if sealed {
                open(from.context("no key")?, &data)?
            } else {
                data
            };
            let out = match to {
                Some(key) => seal(key, &plain)?,
                None => plain,
            };
            let mut name = path.clone().into_os_string();
            name.push(".resealing");
            let beside = PathBuf::from(name);
            std::fs::write(&beside, &out)?;
            std::fs::rename(&beside, &path)?;
            Ok(())
        })();
        if rewritten.is_err() {
            failed += 1;
        }
    }
    failed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_are_sealed_while_their_store_has_a_key() -> Result<()> {
        let root = std::env::temp_dir().join(format!("picacho-sealed-{}", uuid::Uuid::new_v4()));
        let dir = root.join("draft-files").join("d");
        std::fs::create_dir_all(&dir)?;
        let file = dir.join("notes.txt");
        // No key: written and read as they are.
        write(&file, b"The notes")?;
        assert_eq!(std::fs::read(&file)?, b"The notes");
        assert_eq!(read(&file)?, b"The notes");
        // A key: sealed on disk, read back whole, its size as the contents'.
        let key = Zeroizing::new([7u8; 32]);
        set_key(&root, Some(key.clone()));
        write(&file, b"The notes")?;
        let on_disk = std::fs::read(&file)?;
        assert!(is_sealed(&on_disk) && !on_disk.windows(9).any(|w| w == b"The notes"));
        assert_eq!(read(&file)?, b"The notes");
        assert_eq!(len(&file)?, 9);
        // A file in another directory isn't this store's.
        let elsewhere = std::env::temp_dir().join(format!("picacho-plain-{}.txt", uuid::Uuid::new_v4()));
        write(&elsewhere, b"x")?;
        assert_eq!(std::fs::read(&elsewhere)?, b"x");
        std::fs::remove_file(&elsewhere)?;
        // Changed on disk: refused, not read as something else.
        let mut damaged = on_disk.clone();
        *damaged.last_mut().unwrap() ^= 1;
        std::fs::write(dir.join("damaged.txt"), &damaged)?;
        assert!(read(&dir.join("damaged.txt")).is_err());
        std::fs::remove_file(dir.join("damaged.txt"))?;
        // Encryption off: opened again before the key goes.
        assert_eq!(reseal_dir(&root.join("draft-files"), Some(&key), None), 0);
        set_key(&root, None);
        assert_eq!(std::fs::read(&file)?, b"The notes");
        // On again: sealed in place.
        let next = Zeroizing::new([9u8; 32]);
        assert_eq!(reseal_dir(&root.join("draft-files"), None, Some(&next)), 0);
        assert!(is_sealed(&std::fs::read(&file)?));
        // Without its key, a sealed file can't be read.
        assert!(read(&file).is_err());
        set_key(&root, Some(next));
        assert_eq!(read(&file)?, b"The notes");
        set_key(&root, None);
        std::fs::remove_dir_all(root)?;
        Ok(())
    }
}

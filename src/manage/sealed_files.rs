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
//! which opens a sealed file and passes a plain one through.
//!
//! Turning the database's encryption on or off converts the files too, as
//! a journaled migration (`begin`, then `resume`): the journal is written
//! before the database changes, and an old key the files may still be
//! sealed with is kept under a keychain entry of its own. Until every file
//! is converted and read back as wanted, the journal and that key stay,
//! and `resume` (on every switch, retry and opening of the store) carries
//! on, whatever state the database is in by then. Only a migration found
//! complete forgets the old key.
use crate::manage::vault::{self, DatabaseKey, KeyStore};
use aes_gcm::{
    Aes256Gcm, KeyInit, Nonce,
    aead::{Aead, Payload},
};
use anyhow::{Context, Result, bail, ensure};
use std::path::{Path, PathBuf};
use zeroize::Zeroizing;

/// What a sealed file starts with.
const MAGIC: &[u8; 16] = b"PICACHO-SEALED1\n";
const NONCE: usize = 12;
const TAG: usize = 16;
/// The key's label in its derivation from the database's key.
pub(crate) const KEY_INFO: &str = "picacho draft files v1";
/// The drafts' files, inside the data directory.
pub(crate) const DIR: &str = "draft-files";
/// Beside the database while a migration is pending: its first line
/// `JOURNAL_HEADER`, then the keychain entries (`vault::retain_key`) of
/// old keys files may still be sealed with, one a line.
pub(crate) const JOURNAL: &str = "draft-files.migration";
const JOURNAL_HEADER: &str = "picacho draft files migration 1";
/// Where converted files are written before they replace the originals:
/// in the data directory (the same disk, so the replacement is a rename),
/// but outside `DIR`, so a file a crash left there is never taken for an
/// attachment, nor the name of one ever taken.
const SCRATCH: &str = "draft-files.resealing";

type Key = Zeroizing<[u8; 32]>;

/// The keys of one data directory's files.
#[derive(Default)]
struct RootKeys {
    /// The database's file key while it is encrypted: new files are sealed
    /// with it.
    current: Option<Key>,
    /// Old keys files may still be sealed with while a migration is
    /// pending: read with, never written with.
    retained: Vec<Key>,
}

/// The file keys of each open store's data directory. By directory, so
/// stores in other directories (tests open many at once) each have their
/// own.
static KEYS: std::sync::LazyLock<std::sync::Mutex<Vec<(PathBuf, RootKeys)>>> =
    std::sync::LazyLock::new(Default::default);

fn update(root: &Path, change: impl FnOnce(&mut RootKeys)) {
    let mut keys = KEYS.lock().unwrap_or_else(|e| e.into_inner());
    let at = match keys.iter().position(|(r, _)| r == root) {
        Some(at) => at,
        None => {
            keys.push((root.to_path_buf(), RootKeys::default()));
            keys.len() - 1
        }
    };
    change(&mut keys[at].1);
    if keys[at].1.current.is_none() && keys[at].1.retained.is_empty() {
        keys.remove(at);
    }
}

/// Notes the file key of the store in `root` (None: its database isn't
/// encrypted, and new files are written plain).
pub(crate) fn set_key(root: &Path, key: Option<Key>) {
    update(root, |k| k.current = key);
}

/// Notes the old keys files in `root` may still be sealed with.
fn set_retained(root: &Path, retained: Vec<Key>) {
    update(root, |k| k.retained = retained);
}

/// Runs `f` with the keys of the store whose directory holds `path`.
fn with_keys<T>(path: &Path, f: impl FnOnce(Option<&RootKeys>) -> T) -> T {
    let keys = KEYS.lock().unwrap_or_else(|e| e.into_inner());
    f(keys
        .iter()
        .filter(|(root, _)| path.starts_with(root))
        .max_by_key(|(root, _)| root.as_os_str().len())
        .map(|(_, k)| k))
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
    let key = with_keys(path, |k| k.and_then(|k| k.current.clone()));
    match key {
        Some(key) => std::fs::write(path, seal(&key, bytes)?)?,
        None => std::fs::write(path, bytes)?,
    }
    Ok(())
}

/// The contents of the file at `path`, opened where it is sealed: with the
/// database's key, or an old one a pending migration still keeps.
pub fn read(path: &Path) -> Result<Vec<u8>> {
    let data = std::fs::read(path)?;
    if !is_sealed(&data) {
        return Ok(data);
    }
    with_keys(path, |keys| {
        let keys = keys.with_context(|| t!("manage-sealed-files-this-file-is-encrypted-with-a-key"))?;
        let mut tried = false;
        for key in keys.current.iter().chain(&keys.retained) {
            tried = true;
            if let Ok(plain) = open(key, &data) {
                return Ok(plain);
            }
        }
        if tried {
            bail!(t!("manage-sealed-files-an-encrypted-file-is-damaged"))
        }
        bail!(t!("manage-sealed-files-this-file-is-encrypted-with-a-key"))
    })
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

/// The current file key of the store in `root` (its database's, while it
/// is encrypted).
pub(crate) fn current_key(root: &Path) -> Option<Key> {
    with_keys(root, |k| k.and_then(|k| k.current.clone()))
}

/// The folder of partial downloads in the data directory (`large.rs`).
pub(crate) const PARTIALS: &str = "partial-downloads";
/// What a sealed partial download starts with, before its salt.
const PARTIAL_MAGIC: &[u8; 16] = b"PICACHO-PARTIAL1";
const PARTIAL_SALT: usize = 32;
const PARTIAL_HEADER: u64 = (PARTIAL_MAGIC.len() + PARTIAL_SALT) as u64;
/// The label of a partial download's key, derived from the file key and
/// its salt.
const PARTIAL_INFO: &str = "picacho partial downloads v1";

/// Whether the file at `path` is a sealed partial download.
pub(crate) fn is_sealed_partial(path: &Path) -> bool {
    use std::io::Read as _;
    let mut head = [0u8; PARTIAL_MAGIC.len()];
    std::fs::File::open(path).and_then(|mut f| f.read_exact(&mut head)).is_ok() && &head == PARTIAL_MAGIC
}

/// A download kept in pieces while the database is encrypted, so it can
/// resume without leaving raw mail readable on disk (`large.rs`).
///
/// The file is `PARTIAL_MAGIC` and a random salt, then a record per piece:
/// its sealed length (4 bytes, little-endian), a 12-byte nonce, and the
/// piece sealed with AES-256-GCM under a key derived from the database's
/// file key and the salt. Each record's additional data names the file
/// (its salt), the message (a digest of its id), and the record's number
/// and offset, so records can't be moved, dropped from the middle or taken
/// from another download. On opening, records are read up to the first
/// that doesn't check out (one a crash cut short, one sealed with an older
/// key), and the file is cut there.
pub(crate) struct SealedPartial {
    file: std::fs::File,
    key: Key,
    salt: [u8; PARTIAL_SALT],
    id: [u8; 32],
    records: u64,
    len: u64,
}

impl SealedPartial {
    /// Opens the download kept at `path` for message `id`, sealed with
    /// keys from `file_key`, or starts one. Pieces are at most `max_piece`
    /// bytes.
    pub(crate) fn open(path: &Path, file_key: &[u8; 32], id: &str, max_piece: usize) -> Result<Self> {
        use std::io::{Read as _, Seek as _, Write as _};
        let mut options = std::fs::OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut file = options.open(path)?;
        let mut header = [0u8; PARTIAL_HEADER as usize];
        let kept = file.read_exact(&mut header).is_ok() && header.starts_with(PARTIAL_MAGIC);
        let salt: [u8; PARTIAL_SALT] = if kept {
            header[PARTIAL_MAGIC.len()..].try_into().expect("salt")
        } else {
            let salt: [u8; PARTIAL_SALT] = rand::random();
            file.set_len(0)?;
            file.seek(std::io::SeekFrom::Start(0))?;
            file.write_all(PARTIAL_MAGIC)?;
            file.write_all(&salt)?;
            salt
        };
        let mut ikm = Zeroizing::new(Vec::with_capacity(32 + PARTIAL_SALT));
        ikm.extend_from_slice(file_key);
        ikm.extend_from_slice(&salt);
        let key = crate::manage::crypto::hkdf(&ikm, PARTIAL_INFO);
        let id = sha2::Digest::finalize(<sha2::Sha256 as sha2::Digest>::new_with_prefix(id.as_bytes())).into();
        let mut partial = Self { file, key, salt, id, records: 0, len: 0 };
        // Every record that checks out, then cut off whatever follows.
        let mut end = PARTIAL_HEADER;
        if kept {
            partial.file.seek(std::io::SeekFrom::Start(PARTIAL_HEADER))?;
            let mut reader = std::io::BufReader::new(&partial.file);
            while let Some(piece) = partial.next_record(&mut reader, max_piece) {
                end += (4 + NONCE + piece.len() + TAG) as u64;
                partial.records += 1;
                partial.len += piece.len() as u64;
            }
        }
        partial.file.set_len(end)?;
        partial.file.seek(std::io::SeekFrom::Start(end))?;
        Ok(partial)
    }
    fn aad(&self, record: u64, offset: u64) -> Vec<u8> {
        let mut aad = Vec::with_capacity(PARTIAL_MAGIC.len() + PARTIAL_SALT + 32 + 16);
        aad.extend_from_slice(PARTIAL_MAGIC);
        aad.extend_from_slice(&self.salt);
        aad.extend_from_slice(&self.id);
        aad.extend_from_slice(&record.to_le_bytes());
        aad.extend_from_slice(&offset.to_le_bytes());
        aad
    }
    /// The next record from `reader`, opened, if it is whole and checks out
    /// as record `self.records` at offset `self.len`.
    fn next_record(&self, reader: &mut impl std::io::Read, max_piece: usize) -> Option<Vec<u8>> {
        let mut size = [0u8; 4];
        reader.read_exact(&mut size).ok()?;
        let size = u32::from_le_bytes(size) as usize;
        if !(TAG..=max_piece + TAG).contains(&size) {
            return None;
        }
        let mut nonce = [0u8; NONCE];
        reader.read_exact(&mut nonce).ok()?;
        let mut sealed = vec![0u8; size];
        reader.read_exact(&mut sealed).ok()?;
        let cipher = Aes256Gcm::new_from_slice(&*self.key).expect("32-byte key");
        cipher
            .decrypt(
                &Nonce::try_from(&nonce[..]).expect("12 bytes"),
                Payload { msg: &sealed, aad: &self.aad(self.records, self.len) },
            )
            .ok()
    }
    /// How much of the message has arrived.
    pub(crate) fn len(&self) -> u64 {
        self.len
    }
    /// Keeps the next piece.
    pub(crate) fn append(&mut self, piece: &[u8]) -> Result<()> {
        use std::io::Write as _;
        if piece.is_empty() {
            return Ok(());
        }
        let nonce: [u8; NONCE] = rand::random();
        let cipher = Aes256Gcm::new_from_slice(&*self.key).expect("32-byte key");
        let sealed = cipher
            .encrypt(
                &Nonce::try_from(&nonce[..]).expect("12 bytes"),
                Payload { msg: piece, aad: &self.aad(self.records, self.len) },
            )
            .map_err(|_| anyhow::anyhow!(t!("manage-sealed-files-couldnt-encrypt-a-file")))?;
        let size = u32::try_from(sealed.len()).context("piece too large")?;
        let mut record = Vec::with_capacity(4 + NONCE + sealed.len());
        record.extend_from_slice(&size.to_le_bytes());
        record.extend_from_slice(&nonce);
        record.extend_from_slice(&sealed);
        self.file.write_all(&record)?;
        self.file.flush()?;
        self.records += 1;
        self.len += piece.len() as u64;
        Ok(())
    }
    /// The whole download, every record opened again.
    pub(crate) fn read_all(mut self, max_piece: usize) -> Result<Vec<u8>> {
        use std::io::Seek as _;
        self.file.seek(std::io::SeekFrom::Start(PARTIAL_HEADER))?;
        let (records, len) = (self.records, self.len);
        self.records = 0;
        self.len = 0;
        let mut whole = Vec::with_capacity(len as usize);
        let file = self.file.try_clone()?;
        let mut reader = std::io::BufReader::new(file);
        while self.records < records {
            let piece = self
                .next_record(&mut reader, max_piece)
                .with_context(|| t!("manage-sealed-files-an-encrypted-file-is-damaged"))?;
            self.records += 1;
            self.len += piece.len() as u64;
            whole.extend_from_slice(&piece);
        }
        ensure!(self.len == len, t!("manage-sealed-files-an-encrypted-file-is-damaged"));
        Ok(whole)
    }
}

/// Whether a migration of the files in `root` is pending.
#[cfg(test)]
pub(crate) fn pending(root: &Path) -> bool {
    root.join(JOURNAL).exists()
}

fn read_journal(root: &Path) -> Result<Option<Vec<String>>> {
    match std::fs::read_to_string(root.join(JOURNAL)) {
        Ok(text) => Ok(Some(
            text.lines()
                .map(str::trim)
                .filter(|l| !l.is_empty() && *l != JOURNAL_HEADER)
                .map(str::to_owned)
                .collect(),
        )),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| t!("manage-sealed-files-couldnt-keep-track")),
    }
}

/// Writes the journal whole (beside it, then renamed), on disk before this
/// returns.
fn write_journal(root: &Path, names: &[String]) -> Result<()> {
    let tmp = root.join(format!("{JOURNAL}.tmp"));
    let mut text = format!("{JOURNAL_HEADER}\n");
    for name in names {
        text.push_str(name);
        text.push('\n');
    }
    let written = (|| -> std::io::Result<()> {
        use std::io::Write as _;
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&tmp, root.join(JOURNAL))?;
        sync_dir(root);
        Ok(())
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written.with_context(|| t!("manage-sealed-files-couldnt-keep-track"))
}

/// Makes a rename in `dir` last a crash, where the system allows.
fn sync_dir(dir: &Path) {
    #[cfg(unix)]
    if let Ok(d) = std::fs::File::open(dir) {
        let _ = d.sync_all();
    }
    #[cfg(not(unix))]
    let _ = dir;
}

/// Before the database in `root` changes its encryption: notes that its
/// files must follow (the journal), keeping `old`, the key the database
/// has now if any, under a keychain entry of its own until they all have.
/// Nothing changes when this fails.
pub(crate) fn begin(root: &Path, keys: &dyn KeyStore, old: Option<&DatabaseKey>) -> Result<()> {
    let mut names = read_journal(root)?.unwrap_or_default();
    let added = old.map(|k| vault::retain_key(keys, k)).transpose()?;
    names.extend(added.clone());
    if let Err(e) = write_journal(root, &names) {
        if let Some(name) = &added {
            let _ = keys.delete(name);
        }
        return Err(e);
    }
    if let Some(old) = old {
        // Readable meanwhile, whatever the database's key becomes.
        let file_key = old.file_key();
        update(root, |k| {
            if !k.retained.iter().any(|r| **r == *file_key) {
                k.retained.push(file_key);
            }
        });
    }
    Ok(())
}

/// Brings the files in `root` to the database's encryption, if a migration
/// is pending (see `begin`; one a crash or a failure cut short is too):
/// each of the drafts' files sealed with `current`'s file key, or plain
/// without one, written beside itself, read back and only then put in its
/// place; and, while the database is encrypted, partial downloads (raw
/// mail, `large.rs`) that aren't sealed deleted. Only once every file is found as wanted are
/// the old keys forgotten and the journal removed; otherwise an error says
/// how many files are left, and the next call tries again. Nothing to do
/// when no migration is pending.
pub(crate) fn resume(root: &Path, keys: &dyn KeyStore, current: Option<&DatabaseKey>) -> Result<()> {
    let Some(names) = read_journal(root)? else {
        return Ok(());
    };
    // A keychain that refuses now may give the keys later: stop, keeping
    // everything as it is.
    let mut retained = Vec::new();
    for name in &names {
        if let Some(key) = vault::retained_key(keys, name)? {
            retained.push(key.file_key());
        }
    }
    set_retained(root, retained.clone());
    let to = current.map(DatabaseKey::file_key);
    let open_with: Vec<&[u8; 32]> = to.iter().chain(&retained).map(|k| &**k).collect();
    let scratch = root.join(SCRATCH);
    remove_stale(&scratch);
    let mut failed = migrate(&root.join(DIR), &scratch, &open_with, to.as_deref());
    if current.is_some() && discard_plain_partials(root).is_err() {
        failed += 1;
    }
    if failed > 0 {
        bail!(t!("manage-sealed-files-some-files-couldnt-follow", count = failed));
    }
    // Every file is as the database is: the old keys open nothing now.
    let mut kept = Vec::new();
    for name in names {
        if let Err(e) = keys.delete(&name) {
            eprintln!("Picacho: couldn't remove an old draft files key from the keychain: {e:#}");
            kept.push(name);
        }
    }
    if kept.is_empty() {
        if let Err(e) = std::fs::remove_file(root.join(JOURNAL)) {
            eprintln!("Picacho: couldn't remove the draft files' migration journal: {e}");
        }
    } else {
        // Removed the next time; the files need them no longer.
        write_journal(root, &kept)?;
    }
    set_retained(root, Vec::new());
    let _ = std::fs::remove_dir(&scratch);
    Ok(())
}

/// Deletes the partial downloads in `root` that aren't sealed
/// (`SealedPartial`): raw mail, which an encrypted database's directory
/// mustn't hold in the clear. Sealed ones stay, to resume.
pub(crate) fn discard_plain_partials(root: &Path) -> std::io::Result<()> {
    let entries = match std::fs::read_dir(root.join(PARTIALS)) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    for entry in entries {
        let path = entry?.path();
        if path.is_file() && !is_sealed_partial(&path) {
            std::fs::remove_file(&path)?;
        }
    }
    Ok(())
}

/// Removes what a conversion cut short left in `scratch` a while ago (not
/// what another process is writing now).
fn remove_stale(scratch: &Path) {
    let Ok(entries) = std::fs::read_dir(scratch) else { return };
    for entry in entries.flatten() {
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > std::time::Duration::from_secs(600));
        if old {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// Converts every file under `dir` (see `convert`). Returns how many
/// couldn't be, a folder that couldn't be listed counting as one.
fn migrate(dir: &Path, scratch: &Path, open_with: &[&[u8; 32]], to: Option<&[u8; 32]>) -> usize {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return 0,
        Err(_) => return 1,
    };
    let mut failed = 0;
    for entry in entries {
        let Ok(entry) = entry else {
            failed += 1;
            continue;
        };
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            failed += 1;
            continue;
        };
        if kind.is_dir() {
            failed += migrate(&path, scratch, open_with, to);
        } else if kind.is_file() && convert(&path, scratch, open_with, to).is_err() {
            failed += 1;
        }
    }
    failed
}

/// Makes the file at `path` sealed with `to`, or plain without it: opened
/// with any of `open_with` where it is sealed otherwise. The new contents
/// go to a file of their own in `scratch` (made anew, never one that was
/// there), on disk and read back as wanted before replacing the original.
fn convert(path: &Path, scratch: &Path, open_with: &[&[u8; 32]], to: Option<&[u8; 32]>) -> Result<()> {
    use std::io::Write as _;
    let data = std::fs::read(path)?;
    let plain = if is_sealed(&data) {
        if let Some(to) = to
            && open(to, &data).is_ok()
        {
            return Ok(());
        }
        open_with
            .iter()
            .find_map(|key| open(key, &data).ok())
            .with_context(|| t!("manage-sealed-files-this-file-is-encrypted-with-a-key"))?
    } else if to.is_none() {
        return Ok(());
    } else {
        data
    };
    let out = match to {
        Some(key) => seal(key, &plain)?,
        None => plain.clone(),
    };
    std::fs::create_dir_all(scratch)?;
    let (tmp, mut file) = loop {
        let tmp = scratch.join(format!("{}.tmp", uuid::Uuid::new_v4()));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        match options.open(&tmp) {
            Ok(file) => break (tmp, file),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.into()),
        }
    };
    let written = (|| -> Result<()> {
        file.write_all(&out)?;
        file.sync_all()?;
        drop(file);
        let back = std::fs::read(&tmp)?;
        let as_wanted = match to {
            Some(key) => open(key, &back).is_ok_and(|p| p == plain),
            None => back == plain,
        };
        ensure!(as_wanted, t!("manage-sealed-files-an-encrypted-file-is-damaged"));
        std::fs::rename(&tmp, path)?;
        if let Some(parent) = path.parent() {
            sync_dir(parent);
        }
        Ok(())
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("picacho-{name}-{}", uuid::Uuid::new_v4()))
    }

    #[test]
    fn files_are_sealed_while_their_store_has_a_key() -> Result<()> {
        let root = temp_root("sealed");
        let dir = root.join(DIR).join("d");
        std::fs::create_dir_all(&dir)?;
        let file = dir.join("notes.txt");
        // No key: written and read as they are.
        write(&file, b"The notes")?;
        assert_eq!(std::fs::read(&file)?, b"The notes");
        assert_eq!(read(&file)?, b"The notes");
        // A key: sealed on disk, read back whole, its size as the contents'.
        let db_key = DatabaseKey::random();
        let key = db_key.file_key();
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
        // Encryption off: the old key kept apart until the files are open.
        let keys = vault::MemoryKeys::default();
        begin(&root, &keys, Some(&db_key))?;
        assert!(pending(&root));
        set_key(&root, None);
        // Readable meanwhile, by the key kept.
        assert_eq!(read(&file)?, b"The notes");
        resume(&root, &keys, None)?;
        assert!(!pending(&root));
        assert_eq!(std::fs::read(&file)?, b"The notes");
        // On again: sealed in place.
        let next = DatabaseKey::random();
        begin(&root, &keys, None)?;
        set_key(&root, Some(next.file_key()));
        resume(&root, &keys, Some(&next))?;
        assert!(is_sealed(&std::fs::read(&file)?));
        assert_eq!(read(&file)?, b"The notes");
        // Without its key, a sealed file can't be read.
        set_key(&root, None);
        assert!(read(&file).is_err());
        std::fs::remove_dir_all(root)?;
        Ok(())
    }

    /// A file that can't be converted, or a folder that can't be listed,
    /// leaves the migration pending with the old key kept; once it can be,
    /// the next try finishes and only then forgets the key.
    #[test]
    fn a_migration_finishes_only_when_every_file_has_followed() -> Result<()> {
        let root = temp_root("sealed-retry");
        let dir = root.join(DIR).join("d");
        std::fs::create_dir_all(&dir)?;
        let keys = vault::MemoryKeys::default();
        let old = DatabaseKey::random();
        set_key(&root, Some(old.file_key()));
        write(&dir.join("a.txt"), b"alpha")?;
        write(&dir.join("b.txt"), b"bravo")?;
        // A stranger's sealed file: no key here opens it.
        let stranger = seal(&DatabaseKey::random().file_key(), b"other")?;
        std::fs::write(dir.join("c.txt"), &stranger)?;
        // Decrypting.
        begin(&root, &keys, Some(&old))?;
        set_key(&root, None);
        assert!(resume(&root, &keys, None).is_err());
        assert!(pending(&root), "still pending");
        let names = read_journal(&root)?.unwrap();
        assert_eq!(names.len(), 1);
        assert!(keys.load(&names[0])?.is_some(), "the old key is kept");
        // What could be opened is.
        assert_eq!(std::fs::read(dir.join("a.txt"))?, b"alpha");
        assert_eq!(std::fs::read(dir.join("b.txt"))?, b"bravo");
        assert_eq!(std::fs::read(dir.join("c.txt"))?, stranger);
        // Retrying (as opening the store does) fails the same way.
        assert!(resume(&root, &keys, None).is_err());
        // The file dealt with: the next try finishes, and forgets the key.
        std::fs::remove_file(dir.join("c.txt"))?;
        resume(&root, &keys, None)?;
        assert!(!pending(&root));
        assert!(keys.load(&names[0])?.is_none(), "forgotten once every file is open");
        // A folder that can't be listed isn't taken for an empty one.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let new = DatabaseKey::random();
            begin(&root, &keys, None)?;
            set_key(&root, Some(new.file_key()));
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o000))?;
            let listable = std::fs::read_dir(&dir).is_ok();
            if !listable {
                assert!(resume(&root, &keys, Some(&new)).is_err());
                assert!(pending(&root));
            }
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
            resume(&root, &keys, Some(&new))?;
            assert!(!pending(&root));
            assert!(is_sealed(&std::fs::read(dir.join("a.txt"))?));
            set_key(&root, None);
        }
        std::fs::remove_dir_all(root)?;
        Ok(())
    }

    /// A sealed partial download resumes with what arrived, holds no raw
    /// mail, and drops what a crash cut short, records tampered with, and
    /// all of it under another key or for another message.
    #[test]
    fn sealed_partial_downloads_resume_and_check_every_piece() -> Result<()> {
        let root = temp_root("sealed-partial");
        std::fs::create_dir_all(&root)?;
        let path = root.join("p");
        let key = DatabaseKey::random().file_key();
        let mut p = SealedPartial::open(&path, &key, "m1", 16)?;
        p.append(b"Subject: secret ")?;
        p.append(b"plans for monday")?;
        assert_eq!(p.len(), 32);
        drop(p);
        let on_disk = std::fs::read(&path)?;
        assert!(is_sealed_partial(&path));
        assert!(!on_disk.windows(6).any(|w| w == b"secret"), "nothing in the clear");
        // Resumed: both pieces, and more after them.
        let mut p = SealedPartial::open(&path, &key, "m1", 16)?;
        assert_eq!(p.len(), 32);
        p.append(b"!")?;
        assert_eq!(p.read_all(16)?, b"Subject: secret plans for monday!");
        // A crash mid-record: the record is dropped, the rest kept.
        let whole = std::fs::read(&path)?;
        std::fs::write(&path, &whole[..whole.len() - 3])?;
        let p = SealedPartial::open(&path, &key, "m1", 16)?;
        assert_eq!(p.len(), 32);
        drop(p);
        // A record changed: it and what follows are dropped.
        let mut changed = std::fs::read(&path)?;
        let second = PARTIAL_HEADER as usize + 4 + NONCE + 16 + TAG + 4 + NONCE;
        changed[second] ^= 1;
        std::fs::write(&path, &changed)?;
        assert_eq!(SealedPartial::open(&path, &key, "m1", 16)?.len(), 16);
        // Another message's id, or another key: nothing of it is used.
        assert_eq!(SealedPartial::open(&path, &key, "m2", 16)?.len(), 0);
        let mut p = SealedPartial::open(&path, &key, "m1", 16)?;
        assert_eq!(p.len(), 0, "cut back when another message's download was opened");
        p.append(b"again")?;
        drop(p);
        assert_eq!(SealedPartial::open(&path, &DatabaseKey::random().file_key(), "m1", 16)?.len(), 0);
        // A plain file isn't one: started over; discarding plain ones keeps it.
        std::fs::write(&path, b"Subject: plain")?;
        assert!(!is_sealed_partial(&path));
        let partials = root.join(PARTIALS);
        std::fs::create_dir_all(&partials)?;
        std::fs::write(partials.join("plain"), b"Subject: raw")?;
        SealedPartial::open(&partials.join("sealed"), &key, "m1", 16)?.append(b"x")?;
        discard_plain_partials(&root)?;
        assert!(!partials.join("plain").exists());
        assert!(partials.join("sealed").exists());
        std::fs::remove_dir_all(root)?;
        Ok(())
    }

    /// Converted files are written to new files of their own outside the
    /// drafts' folders: an attachment named like the old temporary file
    /// (`name.resealing`) is left as it is, and nothing is left over.
    #[test]
    fn conversion_never_overwrites_a_sibling() -> Result<()> {
        let root = temp_root("sealed-sibling");
        let dir = root.join(DIR).join("d");
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join("report.pdf"), b"the report")?;
        std::fs::write(dir.join("report.pdf.resealing"), b"a file of its own")?;
        let keys = vault::MemoryKeys::default();
        let key = DatabaseKey::random();
        begin(&root, &keys, None)?;
        set_key(&root, Some(key.file_key()));
        resume(&root, &keys, Some(&key))?;
        assert_eq!(read(&dir.join("report.pdf"))?, b"the report");
        assert_eq!(read(&dir.join("report.pdf.resealing"))?, b"a file of its own");
        let mut names: Vec<_> = std::fs::read_dir(&dir)?.flatten().map(|e| e.file_name()).collect();
        names.sort();
        assert_eq!(names, ["report.pdf", "report.pdf.resealing"]);
        assert!(!root.join(SCRATCH).exists());
        set_key(&root, None);
        std::fs::remove_dir_all(root)?;
        Ok(())
    }
}

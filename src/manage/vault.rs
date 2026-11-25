//! The encrypted local database (Settings → Privacy): its key, where the key
//! is kept, and how to tell an encrypted database file from a plain one.
//!
//! The database is SQLCipher's (AES-256 pages with an HMAC each, the WAL
//! included). Its key is 32 random bytes, used as a raw key (no passphrase
//! derivation, so each of the store's connections opens at once), kept in
//! the system keychain as credentials are: the macOS or iOS Keychain,
//! Windows' Credential Manager, the Secret Service, or Android's
//! SharedPreferences under an Android Keystore key. The background checks
//! (the desktop's background process, Android's service) open it the same
//! way, without anyone at the screen; the app lock (`applock`) is separate.
//!
//! The keychain entry's name is kept beside the database
//! (`KEY_NAME_FILE`), so each data directory has its own key and a moved
//! directory still finds it. Whether the database is encrypted is read from
//! the file itself: a plain SQLite file starts with its format's name, an
//! encrypted one with random bytes.
use anyhow::{Context, Result, bail};
use std::{collections::HashMap, io::Read, path::Path, sync::Mutex};
use zeroize::Zeroizing;

/// The database file inside the data directory.
pub const DATABASE_FILE: &str = "mail.sqlite3";
/// Beside the database: the name of the keychain entry holding its key
/// (never the key itself).
pub const KEY_NAME_FILE: &str = "mail.sqlite3.keychain";
/// Where a database being encrypted or decrypted is written, until it
/// replaces the database. Left over after a crash, it is removed.
pub const CONVERTING_FILE: &str = "mail.sqlite3.converting";

/// What every plain SQLite database file begins with.
const SQLITE_HEADER: &[u8; 16] = b"SQLite format 3\0";

/// Why an encrypted database can't be opened, when no retry would help:
/// its key is gone, or isn't its key. Starting over (`move_aside`) is then
/// the way on. (A keychain that is locked or refuses is another error;
/// trying again may work.)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyProblem {
    Missing,
    Wrong,
}

impl std::fmt::Display for KeyProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Missing => t!("manage-vault-picachos-local-database-is-encrypted-but"),
            Self::Wrong => t!("manage-vault-picachos-local-database-couldnt-be-decrypted"),
        })
    }
}

impl std::error::Error for KeyProblem {}

/// The `KeyProblem` behind `error`, if that's why opening failed.
pub fn key_problem(error: &anyhow::Error) -> Option<KeyProblem> {
    // A context (`.context(KeyProblem::Wrong)`) is found by the error's
    // own downcast; a cause, along its chain.
    error.downcast_ref::<KeyProblem>().copied().or_else(|| {
        error
            .chain()
            .find_map(|e| e.downcast_ref::<KeyProblem>().copied())
    })
}

/// Moves the database in `dir` that can't be read (and the name of its key)
/// aside, as `mail.sqlite3.unreadable-<time>`, so a new, empty one is made
/// in its place. Nothing is deleted: with its key back in the keychain, the
/// file could be put back. Returns where it went.
pub fn move_aside(dir: &Path) -> Result<std::path::PathBuf> {
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let aside = dir.join(format!("{DATABASE_FILE}.unreadable-{stamp}"));
    std::fs::rename(dir.join(DATABASE_FILE), &aside)
        .with_context(|| t!("manage-vault-couldnt-move-the-unreadable-database-aside"))?;
    for suffix in ["-wal", "-shm"] {
        let from = dir.join(format!("{DATABASE_FILE}{suffix}"));
        if from.exists() {
            let mut to = aside.clone().into_os_string();
            to.push(suffix);
            std::fs::rename(&from, std::path::PathBuf::from(to))?;
        }
    }
    let name = dir.join(KEY_NAME_FILE);
    if name.exists() {
        let mut to = aside.clone().into_os_string();
        to.push(".keychain");
        std::fs::rename(&name, std::path::PathBuf::from(to))?;
    }
    Ok(aside)
}

/// A database key: 32 random bytes.
#[derive(Clone)]
pub struct DatabaseKey(Zeroizing<[u8; 32]>);

impl DatabaseKey {
    pub fn random() -> Self {
        Self(Zeroizing::new(rand::random::<[u8; 32]>()))
    }

    /// The key as kept in the keychain: 64 hexadecimal digits.
    fn to_hex(&self) -> Zeroizing<String> {
        let mut hex = Zeroizing::new(String::with_capacity(64));
        for b in self.0.iter() {
            hex.push(char::from_digit(u32::from(b >> 4), 16).unwrap_or('0'));
            hex.push(char::from_digit(u32::from(b & 15), 16).unwrap_or('0'));
        }
        hex
    }

    fn from_hex(hex: &str) -> Result<Self> {
        let hex = hex.trim();
        if hex.len() != 64 || !hex.is_ascii() {
            return Err(KeyProblem::Wrong.into());
        }
        let mut key = Zeroizing::new([0u8; 32]);
        for (i, byte) in key.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16)
                .map_err(|_| anyhow::Error::new(KeyProblem::Wrong))?;
        }
        Ok(Self(key))
    }

    /// The key as SQLCipher takes it, raw rather than a passphrase:
    /// `x'…'`, for `PRAGMA key` and `ATTACH … KEY`.
    pub fn sql_literal(&self) -> Zeroizing<String> {
        Zeroizing::new(format!("x'{}'", self.to_hex().as_str()))
    }
}

impl std::fmt::Debug for DatabaseKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("DatabaseKey(…)")
    }
}

/// Where database keys are kept: the system keychain, or memory in tests.
pub trait KeyStore: Send + Sync {
    fn load(&self, name: &str) -> Result<Option<Zeroizing<String>>>;
    fn save(&self, name: &str, value: &str) -> Result<()>;
    fn delete(&self, name: &str) -> Result<()>;
}

/// The system keychain, where credentials are kept (`store::secret_entry`).
pub struct Keychain;

impl KeyStore for Keychain {
    fn load(&self, name: &str) -> Result<Option<Zeroizing<String>>> {
        Ok(super::store::read_secret_text(name)?.map(Zeroizing::new))
    }
    fn save(&self, name: &str, value: &str) -> Result<()> {
        super::store::write_secret_text(name, value)
    }
    fn delete(&self, name: &str) -> Result<()> {
        super::store::delete_secret(name)
    }
}

/// Keys kept in memory, for tests and tools that must not touch the
/// person's keychain.
#[derive(Default)]
#[cfg_attr(not(test), allow(dead_code))]
pub struct MemoryKeys(Mutex<HashMap<String, Zeroizing<String>>>);

impl KeyStore for MemoryKeys {
    fn load(&self, name: &str) -> Result<Option<Zeroizing<String>>> {
        Ok(self
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(name)
            .cloned())
    }
    fn save(&self, name: &str, value: &str) -> Result<()> {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(name.to_owned(), Zeroizing::new(value.to_owned()));
        Ok(())
    }
    fn delete(&self, name: &str) -> Result<()> {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(name);
        Ok(())
    }
}

/// Whether `file` is an encrypted database. A missing or empty file is a
/// new, plain one.
pub fn is_encrypted(file: &Path) -> Result<bool> {
    let mut header = [0u8; 16];
    let mut f = match std::fs::File::open(file) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e).with_context(|| t!("manage-vault-couldnt-read-the-local-database")),
    };
    let mut read = 0;
    while read < header.len() {
        match f.read(&mut header[read..])? {
            0 => break,
            n => read += n,
        }
    }
    Ok(read > 0 && header[..read] != SQLITE_HEADER[..read])
}

/// The name of the keychain entry with the key of the database in `dir`,
/// if one was made.
fn key_name(dir: &Path) -> Result<Option<String>> {
    match std::fs::read_to_string(dir.join(KEY_NAME_FILE)) {
        Ok(name) if !name.trim().is_empty() => Ok(Some(name.trim().to_owned())),
        Ok(_) => Ok(None),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| t!("manage-vault-couldnt-read-which-keychain-entry-has")),
    }
}

/// The name of the keychain entry for the database in `dir`, made (and
/// written down) the first time.
pub fn key_name_or_new(dir: &Path) -> Result<String> {
    if let Some(name) = key_name(dir)? {
        return Ok(name);
    }
    let name = format!("database-key:{}", uuid::Uuid::new_v4());
    let tmp = dir.join(format!("{KEY_NAME_FILE}.tmp"));
    std::fs::write(&tmp, &name)?;
    std::fs::rename(&tmp, dir.join(KEY_NAME_FILE))?;
    Ok(name)
}

/// The key of the encrypted database in `dir`.
pub fn load_key(dir: &Path, keys: &dyn KeyStore) -> Result<DatabaseKey> {
    let name = key_name(dir)?.ok_or(KeyProblem::Missing)?;
    let hex = keys
        .load(&name)
        .with_context(|| t!("manage-vault-picachos-local-database-is-encrypted-and"))?
        .ok_or(KeyProblem::Missing)?;
    DatabaseKey::from_hex(&hex)
}

/// Saves `key` as the key of the database in `dir`.
pub fn save_key(dir: &Path, keys: &dyn KeyStore, key: &DatabaseKey) -> Result<()> {
    let name = key_name_or_new(dir)?;
    keys.save(&name, &key.to_hex())
        .with_context(|| t!("manage-vault-couldnt-save-the-database-key-in"))?;
    // Read back: a keychain that drops what it's given (a locked Secret
    // Service collection, say) would leave a database nobody can open.
    let back = keys.load(&name)?;
    if back.as_deref().map(|s| s.as_str()) != Some(key.to_hex().as_str()) {
        bail!(
            t!("manage-vault-the-system-keychain-didnt-keep-the")
        );
    }
    Ok(())
}

/// Forgets the key of the database in `dir` (once it is no longer encrypted).
pub fn forget_key(dir: &Path, keys: &dyn KeyStore) -> Result<()> {
    if let Some(name) = key_name(dir)? {
        keys.delete(&name)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_go_to_hex_and_back() {
        let key = DatabaseKey::random();
        let back = DatabaseKey::from_hex(&key.to_hex()).unwrap();
        assert_eq!(*key.0, *back.0);
        assert!(key.sql_literal().starts_with("x'") && key.sql_literal().len() == 67);
        assert!(DatabaseKey::from_hex("abc").is_err());
        assert!(DatabaseKey::from_hex(&"zz".repeat(32)).is_err());
        assert_eq!(format!("{key:?}"), "DatabaseKey(…)");
    }

    #[test]
    fn a_plain_database_is_told_from_an_encrypted_one() {
        let dir = std::env::temp_dir().join(format!("picacho-vault-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("x");
        assert!(!is_encrypted(&file).unwrap(), "missing");
        std::fs::write(&file, b"").unwrap();
        assert!(!is_encrypted(&file).unwrap(), "empty");
        std::fs::write(&file, b"SQLite format 3\0and more").unwrap();
        assert!(!is_encrypted(&file).unwrap());
        std::fs::write(&file, [7u8; 64]).unwrap();
        assert!(is_encrypted(&file).unwrap());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_key_is_kept_under_a_name_written_beside_the_database() {
        let dir = std::env::temp_dir().join(format!("picacho-vault-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let keys = MemoryKeys::default();
        assert_eq!(
            key_problem(&load_key(&dir, &keys).unwrap_err()),
            Some(KeyProblem::Missing)
        );
        let key = DatabaseKey::random();
        save_key(&dir, &keys, &key).unwrap();
        let name = std::fs::read_to_string(dir.join(KEY_NAME_FILE)).unwrap();
        assert!(name.starts_with("database-key:"));
        assert_eq!(*load_key(&dir, &keys).unwrap().0, *key.0);
        // The same name the next time.
        assert_eq!(key_name_or_new(&dir).unwrap(), name);
        forget_key(&dir, &keys).unwrap();
        assert_eq!(
            key_problem(&load_key(&dir, &keys).unwrap_err()),
            Some(KeyProblem::Missing)
        );
        keys.save(&name, "not a key").unwrap();
        assert_eq!(
            key_problem(&load_key(&dir, &keys).unwrap_err()),
            Some(KeyProblem::Wrong)
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}

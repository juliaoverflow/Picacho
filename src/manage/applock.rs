//! The app lock (Settings → Privacy): a passcode, and optionally Touch ID,
//! Face ID or the phone's fingerprint or face unlock, asked for when
//! Picacho opens and when it comes back after being away for a while.
//!
//! It guards the window, not the data: mail is still checked, sent and
//! announced while locked, and the background checks run without it. The
//! database is protected at rest by encrypting it (`vault`). The passcode is
//! kept as an Argon2id hash among the preferences, which are inside that
//! database; tries are slowed after a few wrong ones, across restarts.
use super::store::Store;
use anyhow::{Result, bail};
use base64::{Engine, engine::general_purpose::STANDARD as B64};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use unicode_normalization::UnicodeNormalization;
use zeroize::Zeroizing;

/// The preference with the lock's settings (`LockSettings`'s JSON).
pub const PREFERENCE: &str = "app_lock";
/// Wrong passcodes in a row, and when the last was entered.
const FAILURES: &str = "app_lock_failures";
/// The preference that keeps mail out of notifications and widgets
/// ("true" when on).
const HIDE_CONTENTS: &str = "hide_contents";
/// That preference, as last loaded or set, for notifications, which are
/// shown without a store at hand.
static HIDING: AtomicBool = AtomicBool::new(false);

/// Whether notifications and widgets leave out senders, subjects and
/// previews (Settings → Privacy → Hide mail outside Picacho).
pub fn hides_contents(store: &Store) -> bool {
    store.preference(HIDE_CONTENTS).as_deref() == Some("true")
}

/// Turns hiding mail outside Picacho on or off.
pub fn set_hides_contents(store: &Store, on: bool) -> Result<()> {
    if on {
        store.set_preference(HIDE_CONTENTS, "true")?;
    } else {
        store.remove_preference(HIDE_CONTENTS)?;
    }
    HIDING.store(on, Ordering::Relaxed);
    Ok(())
}

/// Loads the setting for `hiding`, once a store is open.
pub fn load_hiding(store: &Store) {
    HIDING.store(hides_contents(store), Ordering::Relaxed);
}

/// Whether notifications leave out what mail says, as last loaded.
pub fn hiding() -> bool {
    HIDING.load(Ordering::Relaxed)
}

/// The shortest passcode.
pub const MIN_PASSCODE: usize = 4;
/// Wrong passcodes entered without waiting; each after these doubles the wait.
const FREE_TRIES: u32 = 5;
const FIRST_WAIT: i64 = 30;
const LONGEST_WAIT: i64 = 60 * 60;

/// How long Picacho may be away (in the background, or another app in front)
/// before it locks again: each choice in seconds, with its label.
pub const AFTER_CHOICES: [(u64, &str); 5] = [
    (0, "Immediately"),
    (60, "1 minute"),
    (5 * 60, "5 minutes"),
    (15 * 60, "15 minutes"),
    (60 * 60, "1 hour"),
];

/// The lock's settings.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LockSettings {
    /// The passcode's hash; the lock is off without one.
    #[serde(default)]
    pub passcode: Option<PasscodeHash>,
    /// Touch ID, Face ID or the phone's biometrics unlock too.
    #[serde(default)]
    pub biometrics: bool,
    /// Seconds away before locking again; 0 locks as soon as Picacho is left.
    #[serde(default = "default_after")]
    pub after: u64,
}

fn default_after() -> u64 {
    60
}

impl LockSettings {
    pub fn load(store: &Store) -> Self {
        store
            .preference(PREFERENCE)
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_else(|| Self {
                after: default_after(),
                ..Self::default()
            })
    }

    pub fn save(&self, store: &Store) -> Result<()> {
        if self.passcode.is_none() {
            store.remove_preference(PREFERENCE)?;
            store.remove_preference(FAILURES)?;
            return Ok(());
        }
        store.set_preference(PREFERENCE, &serde_json::to_string(self)?)
    }

    pub fn enabled(&self) -> bool {
        self.passcode.is_some()
    }

    /// Whether being away for `away` locks Picacho again.
    pub fn locks_after(&self, away: Duration) -> bool {
        self.enabled() && away.as_secs() >= self.after
    }

    /// Whether `passcode` is the lock's passcode.
    pub fn verify(&self, passcode: &str) -> bool {
        self.passcode.as_ref().is_some_and(|h| h.verify(passcode))
    }
}

/// A passcode's Argon2id hash, with its salt and settings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PasscodeHash {
    salt: String,
    hash: String,
    memory_kib: u32,
    iterations: u32,
    parallelism: u32,
}

impl PasscodeHash {
    /// OWASP's suggestion for Argon2id: 19 MiB, two passes. Tenths of a
    /// second on a phone, run off the window's thread.
    const MEMORY_KIB: u32 = 19 * 1024;
    const ITERATIONS: u32 = 2;
    const PARALLELISM: u32 = 1;

    pub fn new(passcode: &str) -> Result<Self> {
        check_passcode(passcode)?;
        let salt = rand::random::<[u8; 16]>();
        let (m, t, p) = (Self::MEMORY_KIB, Self::ITERATIONS, Self::PARALLELISM);
        let hash = derive(passcode, &salt, m, t, p)?;
        Ok(Self {
            salt: B64.encode(salt),
            hash: B64.encode(hash.as_ref()),
            memory_kib: m,
            iterations: t,
            parallelism: p,
        })
    }

    pub fn verify(&self, passcode: &str) -> bool {
        let (Ok(salt), Ok(expected)) = (B64.decode(&self.salt), B64.decode(&self.hash)) else {
            return false;
        };
        // Settings within reason only: a changed preference mustn't make
        // each try take minutes or all memory.
        if !(8 * 1024..=256 * 1024).contains(&self.memory_kib)
            || !(1..=10).contains(&self.iterations)
            || !(1..=4).contains(&self.parallelism)
        {
            return false;
        }
        let Ok(hash) = derive(
            passcode,
            &salt,
            self.memory_kib,
            self.iterations,
            self.parallelism,
        ) else {
            return false;
        };
        constant_time_eq(hash.as_ref(), &expected)
    }
}

/// Why `passcode` can't be a passcode, if it can't.
pub fn check_passcode(passcode: &str) -> Result<()> {
    if passcode.chars().count() < MIN_PASSCODE {
        bail!("A passcode needs at least {MIN_PASSCODE} characters.");
    }
    if passcode.trim() != passcode {
        bail!("A passcode can't begin or end with a space.");
    }
    Ok(())
}

fn derive(passcode: &str, salt: &[u8], m: u32, t: u32, p: u32) -> Result<Zeroizing<[u8; 32]>> {
    let params = argon2::Params::new(m, t, p, Some(32)).map_err(|e| anyhow::anyhow!("{e}"))?;
    let hasher = argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);
    // The same passcode typed on another keyboard (composed or not) matches.
    let normalized = Zeroizing::new(passcode.nfc().collect::<String>());
    let mut out = Zeroizing::new([0u8; 32]);
    hasher
        .hash_password_into(normalized.as_bytes(), salt, out.as_mut())
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(out)
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Wrong passcodes in a row.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Failures {
    pub count: u32,
    /// When the last was entered (Unix seconds).
    pub last: i64,
}

impl Failures {
    pub fn load(store: &Store) -> Self {
        store
            .preference(FAILURES)
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    /// One more wrong passcode, at `now`; saved.
    pub fn record(store: &Store, now: i64) -> Self {
        let mut f = Self::load(store);
        f.count = f.count.saturating_add(1);
        f.last = now;
        if let Ok(json) = serde_json::to_string(&f) {
            let _ = store.set_preference(FAILURES, &json);
        }
        f
    }

    pub fn clear(store: &Store) {
        if store.preference(FAILURES).is_some() {
            let _ = store.remove_preference(FAILURES);
        }
    }

    /// Until when (Unix seconds) another try must wait, if it must.
    pub fn wait_until(&self) -> Option<i64> {
        if self.count < FREE_TRIES {
            return None;
        }
        let doublings = (self.count - FREE_TRIES).min(16);
        let wait = (FIRST_WAIT << doublings).min(LONGEST_WAIT);
        Some(self.last + wait)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_passcode_matches_only_itself() {
        let hash = PasscodeHash::new("2468").unwrap();
        assert!(hash.verify("2468"));
        assert!(!hash.verify("2469"));
        assert!(!hash.verify(""));
        // Composed or decomposed, the same.
        let hash = PasscodeHash::new("caf\u{e9}1").unwrap();
        assert!(hash.verify("cafe\u{301}1"));
        assert!(PasscodeHash::new("123").is_err());
        assert!(PasscodeHash::new(" 1234").is_err());
    }

    #[test]
    fn unreasonable_hash_settings_never_match() {
        let mut hash = PasscodeHash::new("2468").unwrap();
        hash.memory_kib = 4 * 1024 * 1024;
        assert!(!hash.verify("2468"));
    }

    #[test]
    fn tries_slow_down_after_a_few_wrong_ones() {
        let f = |count| Failures { count, last: 1000 };
        assert_eq!(f(4).wait_until(), None);
        assert_eq!(f(5).wait_until(), Some(1030));
        assert_eq!(f(6).wait_until(), Some(1060));
        assert_eq!(f(40).wait_until(), Some(1000 + LONGEST_WAIT));
    }

    #[test]
    fn settings_and_failures_are_kept() -> Result<()> {
        let dir = std::env::temp_dir().join(format!("picacho-lock-{}", uuid::Uuid::new_v4()));
        let store = Store::open(&dir)?;
        assert!(!LockSettings::load(&store).enabled());
        assert_eq!(LockSettings::load(&store).after, 60);
        let settings = LockSettings {
            passcode: Some(PasscodeHash::new("1357")?),
            biometrics: true,
            after: 0,
        };
        settings.save(&store)?;
        let loaded = LockSettings::load(&store);
        assert_eq!(loaded, settings);
        assert!(loaded.verify("1357"));
        assert!(loaded.locks_after(Duration::ZERO));
        for _ in 0..5 {
            Failures::record(&store, 50);
        }
        assert_eq!(Failures::load(&store).wait_until(), Some(80));
        Failures::clear(&store);
        assert_eq!(Failures::load(&store), Failures::default());
        assert!(!hides_contents(&store));
        set_hides_contents(&store, true)?;
        assert!(hides_contents(&store) && hiding());
        set_hides_contents(&store, false)?;
        assert!(!hides_contents(&store) && !hiding());
        // Turned off: nothing kept.
        Failures::record(&store, 50);
        LockSettings::default().save(&store)?;
        assert!(!LockSettings::load(&store).enabled());
        assert_eq!(Failures::load(&store), Failures::default());
        drop(store);
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }
}

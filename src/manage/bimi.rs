//! Brand logos (BIMI, `sync/bimi.rs`): checked with the rest of the sender
//! (Check sender, or on opening a message when Settings → Privacy says so),
//! and kept: each domain's verified logo as a small PNG in the data
//! directory, and which messages showed it. A message row or header shows
//! a logo only for a message that was itself checked and passed DMARC for
//! that domain, never for another message merely claiming the domain.
use super::backend::Backend;
use crate::sync::bimi::{self, MarkKind, Missing};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// Preference: check each message's sender (DKIM, SPF, DMARC and its logo)
/// as it opens, rather than only when asked ("true"/"false"; off).
pub const AUTO_PREFERENCE: &str = "bimi_auto";
/// Preference: each domain's verified logo (`Logo`), by domain.
const LOGOS: &str = "bimi_logos";
/// Preference: the domain whose logo each message showed, by message id.
const MARKS: &str = "bimi_marks";
/// Messages remembered; past this the oldest half are forgotten.
const MAX_MARKS: usize = 5000;
/// How long a verified logo is used before its certificate is checked
/// again (its revocation, and that the record still names it).
const RECHECK: i64 = 7 * 24 * 3600;

/// A domain's verified logo.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Logo {
    /// The PNG's file name in the `bimi` folder.
    pub file: String,
    pub kind: MarkKind,
    /// Who the certificate was issued to.
    pub holder: String,
    /// When the certificate expires, and when it was last checked (Unix seconds).
    pub until: i64,
    pub checked: i64,
}

/// What a sender check found of the sender's logo, for the details.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    Shown { domain: String, logo: Logo, path: PathBuf },
    Missing(Missing),
}
impl Verdict {
    /// The line the message details show, and whether it is good news.
    pub fn line(&self) -> (String, bool) {
        match self {
            Verdict::Shown { domain, logo, .. } => (
                match logo.kind {
                    MarkKind::Verified => t!("bimi-verified-mark", holder = logo.holder.clone(), domain = domain.clone()),
                    MarkKind::Common => t!("bimi-common-mark", holder = logo.holder.clone(), domain = domain.clone()),
                },
                true,
            ),
            Verdict::Missing(Missing::NotEligible) => (t!("bimi-no-logo-dmarc").into(), false),
            Verdict::Missing(Missing::NoRecord) => (t!("bimi-no-logo-published").into(), false),
            Verdict::Missing(Missing::SelfAsserted) => (t!("bimi-no-logo-self-asserted").into(), false),
            Verdict::Missing(Missing::Invalid(why)) => (t!("bimi-no-logo-invalid", why = why.clone()), false),
        }
    }
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

impl Backend {
    /// Whether senders are checked as each message opens.
    pub fn brand_auto(&self) -> bool {
        self.store.preference(AUTO_PREFERENCE).as_deref() == Some("true")
    }
    pub fn set_brand_auto(&self, on: bool) -> anyhow::Result<()> {
        self.store.set_preference(AUTO_PREFERENCE, if on { "true" } else { "false" })
    }
    fn brand_folder(&self) -> PathBuf {
        self.store.path.join("bimi")
    }
    fn brand_logos(&self) -> HashMap<String, Logo> {
        self.store
            .preference(LOGOS)
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }
    fn brand_mark_domains(&self) -> HashMap<String, String> {
        self.store
            .preference(MARKS)
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }
    /// Notes (or, with `None`, forgets) the logo a message showed.
    fn remember_mark(&self, message: &str, domain: Option<&str>) {
        static SAVING: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _saving = SAVING.lock().unwrap_or_else(|e| e.into_inner());
        let mut marks = self.brand_mark_domains();
        let changed = match domain {
            Some(d) => marks.insert(message.to_owned(), d.to_owned()).as_deref() != Some(d),
            None => marks.remove(message).is_some(),
        };
        if marks.len() > MAX_MARKS {
            let keep: Vec<String> = marks.keys().skip(MAX_MARKS / 2).cloned().collect();
            marks.retain(|k, _| keep.contains(k) || k == message);
        }
        if changed && let Ok(json) = serde_json::to_string(&marks) {
            let _ = self.store.set_preference(MARKS, &json);
        }
    }
    /// Each message that showed a logo still current, and the logo's file.
    pub fn brand_marks(&self) -> HashMap<String, PathBuf> {
        let logos = self.brand_logos();
        let folder = self.brand_folder();
        let now = now();
        self.brand_mark_domains()
            .into_iter()
            .filter_map(|(message, domain)| {
                let logo = logos.get(&domain).filter(|l| l.until > now)?;
                let path = folder.join(&logo.file);
                path.is_file().then_some((message, path))
            })
            .collect()
    }

    /// The logo of a checked message's sender (`details` filled by
    /// `auth::check`), verified now or recently, and remembered for the
    /// message. `None` when the sender wasn't checked.
    pub fn check_brand(&self, message: &str, details: &crate::sync::mime::Details) -> Option<Verdict> {
        if !details.checked {
            return None;
        }
        let domain = bimi::author_domain(&details.from);
        let Some(domain) = domain.filter(|_| bimi::eligible(details.dmarc.as_deref(), details.dmarc_policy.as_deref()))
        else {
            self.remember_mark(message, None);
            return Some(Verdict::Missing(Missing::NotEligible));
        };
        let verdict = self.domain_logo(&domain);
        self.remember_mark(message, matches!(verdict, Verdict::Shown { .. }).then_some(domain.as_str()));
        Some(verdict)
    }

    /// `domain`'s logo: kept, if checked lately; else looked up, verified
    /// and drawn now.
    fn domain_logo(&self, domain: &str) -> Verdict {
        let now = now();
        let folder = self.brand_folder();
        let mut logos = self.brand_logos();
        if let Some(logo) = logos.get(domain)
            && logo.until > now
            && now - logo.checked < RECHECK
            && folder.join(&logo.file).is_file()
        {
            return Verdict::Shown { domain: domain.to_owned(), path: folder.join(&logo.file), logo: logo.clone() };
        }
        let fresh = self.verify_logo(domain, now);
        // What was kept for the domain gives way to what was found now.
        if let Some(old) = logos.remove(domain)
            && !matches!(&fresh, Ok(l) if l.file == old.file)
        {
            let _ = std::fs::remove_file(folder.join(&old.file));
        }
        let verdict = match fresh {
            Ok(logo) => {
                logos.insert(domain.to_owned(), logo.clone());
                Verdict::Shown { domain: domain.to_owned(), path: folder.join(&logo.file), logo }
            }
            Err(missing) => Verdict::Missing(missing),
        };
        if let Ok(json) = serde_json::to_string(&logos) {
            let _ = self.store.set_preference(LOGOS, &json);
        }
        verdict
    }

    fn verify_logo(&self, domain: &str, now: i64) -> Result<Logo, Missing> {
        let invalid = |e: anyhow::Error| Missing::Invalid(format!("{e:#}"));
        let (record, at) = bimi::record(domain).map_err(invalid)?.ok_or(Missing::NoRecord)?;
        if record.authority.is_empty() {
            return Err(Missing::SelfAsserted);
        }
        let pem = bimi::fetch_certificate(&record.authority).map_err(invalid)?;
        let mark = bimi::verify(&pem, &[domain, at.as_str()], now).map_err(invalid)?;
        // RFC 5280 §6.3, RFC 6960: a revoked certificate vouches for
        // nothing. One whose status can't be had is used, as browsers do.
        if let Ok(fetcher) = crate::manage::smime::revocation::HttpFetcher::new() {
            use crate::manage::smime::revocation::{CrlCache, Status, status};
            let cache = CrlCache { folder: self.store.path.join("crls-fetched") };
            if status(&mark.leaf, &mark.issuer, &cache, Some(&fetcher), now) == Status::Revoked {
                return Err(Missing::Invalid(t!("bimi-the-certificate-was-revoked").into()));
            }
        }
        let png = bimi::rasterize(&mark.svg).map_err(invalid)?;
        let folder = self.brand_folder();
        std::fs::create_dir_all(&folder).map_err(|e| invalid(e.into()))?;
        let file = format!("{}.png", uuid::Uuid::new_v4().simple());
        std::fs::write(folder.join(&file), png).map_err(|e| invalid(e.into()))?;
        Ok(Logo {
            file,
            kind: mark.kind,
            holder: mark.holder,
            until: mark.until,
            checked: now,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manage::store::Store;

    fn backend() -> (tempdir::Dir, Backend) {
        let dir = tempdir::Dir::new();
        let keys: std::sync::Arc<crate::manage::vault::MemoryKeys> = std::sync::Arc::default();
        let store = Store::open_with_keys(&dir.0, keys).unwrap();
        (dir, Backend::new(std::sync::Arc::new(store)))
    }

    mod tempdir {
        pub struct Dir(pub std::path::PathBuf);
        impl Dir {
            pub fn new() -> Self {
                let p = std::env::temp_dir().join(format!("picacho-bimi-{}", uuid::Uuid::new_v4().simple()));
                std::fs::create_dir_all(&p).unwrap();
                Self(p)
            }
        }
        impl Drop for Dir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
    }

    #[test]
    fn logos_show_only_for_messages_that_passed_dmarc_for_their_domain() {
        let (_dir, b) = backend();
        // A logo kept for the domain, checked just now.
        std::fs::create_dir_all(b.brand_folder()).unwrap();
        std::fs::write(b.brand_folder().join("logo.png"), b"\x89PNG").unwrap();
        let logo = Logo { file: "logo.png".into(), kind: MarkKind::Verified, holder: "Shop".into(), until: now() + 3600, checked: now() };
        b.store
            .set_preference(LOGOS, &serde_json::to_string(&HashMap::from([("shop.example".to_owned(), logo.clone())])).unwrap())
            .unwrap();
        let mut details = crate::sync::mime::Details { from: "Shop <news@shop.example>".into(), ..Default::default() };
        // Not checked: nothing said, nothing kept.
        assert_eq!(b.check_brand("m1", &details), None);
        details.checked = true;
        details.dmarc = Some("pass".into());
        details.dmarc_policy = Some("none".into());
        assert_eq!(b.check_brand("m1", &details), Some(Verdict::Missing(Missing::NotEligible)));
        assert!(b.brand_marks().is_empty());
        details.dmarc_policy = Some("reject".into());
        let shown = b.check_brand("m1", &details).unwrap();
        assert!(matches!(&shown, Verdict::Shown { domain, .. } if domain == "shop.example"), "{shown:?}");
        assert!(shown.line().1);
        assert_eq!(b.brand_marks().keys().collect::<Vec<_>>(), ["m1"]);
        // Another message claiming the domain, failing DMARC, shows none,
        // and the first keeps its own.
        details.dmarc = Some("fail".into());
        assert_eq!(b.check_brand("m2", &details), Some(Verdict::Missing(Missing::NotEligible)));
        assert_eq!(b.brand_marks().len(), 1);
        // Checked again and failing now, the first is forgotten too.
        b.check_brand("m1", &details);
        assert!(b.brand_marks().is_empty());
    }

    #[test]
    fn expired_logos_are_not_shown() {
        let (_dir, b) = backend();
        std::fs::create_dir_all(b.brand_folder()).unwrap();
        std::fs::write(b.brand_folder().join("old.png"), b"\x89PNG").unwrap();
        let logo = Logo { file: "old.png".into(), kind: MarkKind::Common, holder: "Old".into(), until: now() - 1, checked: now() };
        b.store
            .set_preference(LOGOS, &serde_json::to_string(&HashMap::from([("old.example".to_owned(), logo)])).unwrap())
            .unwrap();
        b.remember_mark("m1", Some("old.example"));
        assert!(b.brand_marks().is_empty());
    }

    #[test]
    fn checking_automatically_is_off_until_turned_on() {
        let (_dir, b) = backend();
        assert!(!b.brand_auto());
        b.set_brand_auto(true).unwrap();
        assert!(b.brand_auto());
    }
}

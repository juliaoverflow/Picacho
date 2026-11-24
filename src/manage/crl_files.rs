//! Certificate revocation lists you add yourself (RFC 5280 §5), from
//! Settings → Encryption. S/MIME signatures are then checked against them
//! (`smime::open_checking_revocation`).
//!
//! Each list is copied into the app's data directory, as PEM, named by the
//! SHA-256 of its DER: the original file can move or change without
//! changing what was checked, and the same list added twice is kept once.
//! Nothing here goes on the network; fetching lists is separate.
use super::*;
use openssl::x509::X509Crl;

/// The folder, in the app's data directory, holding the lists.
const FOLDER: &str = "crls";

/// One list, as Settings shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CrlInfo {
    /// The copy's file stem: the SHA-256 of the list, in hex.
    pub id: String,
    /// Who issued it: its issuer's common name, or organization.
    pub issuer: String,
    /// RFC 5280 §5.1.2.5: when the issuer publishes its next list, as
    /// seconds since 1970; past it, the list may be missing revocations.
    pub next_update: Option<i64>,
    /// How many certificates it revokes.
    pub revoked: usize,
}

/// The lists in `data` (PEM, one or several, or DER).
fn parse(data: &[u8]) -> Result<Vec<X509Crl>> {
    const BEGIN: &[u8] = b"-----BEGIN X509 CRL-----";
    if memchr::memmem::find(data, BEGIN).is_none() {
        return Ok(vec![X509Crl::from_der(data)?]);
    }
    let mut out = Vec::new();
    let mut rest = data;
    while let Some(at) = memchr::memmem::find(rest, BEGIN) {
        let crl = X509Crl::from_pem(&rest[at..])?;
        out.push(crl);
        rest = &rest[at + BEGIN.len()..];
    }
    Ok(out)
}

/// Seconds since 1970 of an ASN.1 time.
fn unix(time: &openssl::asn1::Asn1TimeRef) -> Option<i64> {
    let epoch = openssl::asn1::Asn1Time::from_unix(0).ok()?;
    let d = epoch.diff(time).ok()?;
    Some(i64::from(d.days) * 86_400 + i64::from(d.secs))
}

fn issuer(crl: &X509Crl) -> String {
    use openssl::nid::Nid;
    [Nid::COMMONNAME, Nid::ORGANIZATIONNAME]
        .into_iter()
        .find_map(|nid| crl.issuer_name().entries_by_nid(nid).next()?.data().to_string().ok())
        .unwrap_or_default()
}

impl Backend {
    fn crl_folder(&self) -> std::path::PathBuf {
        self.store.path.join(FOLDER)
    }
    /// The revocation lists you added, as PEM files, for
    /// `smime::CrlCheck`. Empty when there are none.
    pub fn crl_files(&self) -> Vec<std::path::PathBuf> {
        let Ok(entries) = std::fs::read_dir(self.crl_folder()) else {
            return Vec::new();
        };
        let mut files: Vec<_> = entries
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|e| e == "pem"))
            .collect();
        files.sort();
        files
    }
    /// The check S/MIME reading makes with the lists you added, or `None`
    /// when there are none.
    pub(super) fn smime_crl_check(&self) -> Option<crate::manage::smime::CrlCheck> {
        let files = self.crl_files();
        (!files.is_empty()).then_some(crate::manage::smime::CrlCheck { files })
    }
    /// The lists you added, for Settings.
    pub fn crl_list(&self) -> Vec<CrlInfo> {
        self.crl_files()
            .iter()
            .filter_map(|path| {
                let crl = X509Crl::from_pem(&std::fs::read(path).ok()?).ok()?;
                Some(CrlInfo {
                    id: path.file_stem()?.to_string_lossy().into_owned(),
                    issuer: issuer(&crl),
                    next_update: crl.next_update().and_then(unix),
                    revoked: crl.get_revoked().map_or(0, |r| r.len()),
                })
            })
            .collect()
    }
    /// Adds the revocation lists in a file (PEM or DER). Returns how many
    /// it held; one already added counts but is kept once.
    pub fn add_crl_file(&self, data: &[u8]) -> Result<usize> {
        let crls = parse(data).context("That isn't a certificate revocation list (.crl or .pem).")?;
        ensure!(!crls.is_empty(), "That isn't a certificate revocation list (.crl or .pem).");
        let folder = self.crl_folder();
        std::fs::create_dir_all(&folder)?;
        for crl in &crls {
            let digest = openssl::sha::sha256(&crl.to_der()?);
            let name: String = digest.iter().map(|b| format!("{b:02x}")).collect();
            std::fs::write(folder.join(format!("{name}.pem")), crl.to_pem()?)?;
        }
        // Read again with the lists: what was read without them may change.
        self.smime_verdicts.lock().unwrap().clear();
        Ok(crls.len())
    }
    /// RFC 5280 §6.3 and RFC 6960, when the user asks (fetching tells a
    /// third party the message was opened): each S/MIME signer of `id`
    /// whose signature checked out, looked up by OCSP or its issuer's CRL
    /// through `fetcher` (tests give a fake). Lists fetched are kept, and
    /// a revoked signer marks the message's verdict Revoked. A line for
    /// the details, and whether every signer is known good; `None` when
    /// the message isn't S/MIME-signed.
    pub fn check_revocation(
        &self,
        id: &str,
        fetcher: &dyn crate::manage::smime::revocation::RevocationFetcher,
    ) -> Option<(String, bool)> {
        use crate::manage::smime::revocation::{CrlCache, Status, status};
        let verdict = self.smime_verdict(id)?;
        if verdict.checked.is_empty() {
            return None;
        }
        let carried: Vec<openssl::x509::X509> =
            verdict.carried.iter().filter_map(|d| openssl::x509::X509::from_der(d).ok()).collect();
        let known = self.certificates().known();
        let cache = CrlCache { folder: self.store.path.join("crls-fetched") };
        let now = chrono::Utc::now().timestamp();
        let (mut revoked, mut unknown) = (Vec::new(), Vec::new());
        for der in &verdict.checked {
            let Ok(cert) = openssl::x509::X509::from_der(der) else { continue };
            let who = crate::manage::smime::emails(&cert).into_iter().next().unwrap_or_default();
            // Its issuer: among what the message carried, or what's known.
            let issuer = carried.iter().chain(known.iter()).find(|i| {
                i.issued(&cert) == openssl::x509::X509VerifyResult::OK && i.to_der().ok().as_deref() != Some(der)
            });
            let Some(issuer) = issuer else {
                unknown.push(format!("{who}: its issuer's certificate isn't here"));
                continue;
            };
            match status(&cert, issuer, &cache, Some(fetcher), now) {
                Status::Good => {}
                Status::Revoked => revoked.push(who),
                Status::Unknown(why) => unknown.push(format!("{who}: {why}")),
            }
        }
        if let Some(first) = revoked.first() {
            if let Some(v) = self.smime_verdicts.lock().unwrap().get_mut(id) {
                v.signature = crate::manage::smime::Signature::Revoked(first.clone());
            }
            return Some((format!("Revoked: {}'s certificate has been revoked.", revoked.join(", ")), false));
        }
        if !unknown.is_empty() {
            return Some((format!("Revocation couldn't be checked ({}).", unknown.join("; ")), false));
        }
        Some(("Not revoked: checked with the certificate's issuer.".into(), true))
    }
    /// Removes the list with this id (see `CrlInfo::id`).
    pub fn remove_crl_file(&self, id: &str) -> Result<()> {
        ensure!(
            !id.is_empty() && id.chars().all(|c| c.is_ascii_hexdigit()),
            "There's no such revocation list."
        );
        let path = self.crl_folder().join(format!("{id}.pem"));
        if path.exists() {
            std::fs::remove_file(path)?;
        }
        self.smime_verdicts.lock().unwrap().clear();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> Vec<u8> {
        let path = format!("{}/tests/fixtures/smime/{name}", env!("CARGO_MANIFEST_DIR"));
        std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
    }

    fn backend() -> (Backend, std::path::PathBuf) {
        let path = std::env::temp_dir().join(format!("picacho-crls-{}", uuid::Uuid::new_v4()));
        (Backend::new(Arc::new(Store::open(&path).unwrap())), path)
    }

    /// RFC 5280 §5: lists are added from PEM or DER, kept once each as PEM
    /// copies in the data directory, listed with their issuer, and removed.
    #[test]
    fn crl_files_are_copied_listed_and_removed() {
        let (b, path) = backend();
        assert!(b.crl_files().is_empty() && b.crl_list().is_empty());
        let revoking = fixture("crl-bo-revoked.pem");
        assert_eq!(b.add_crl_file(&revoking).unwrap(), 1);
        // The same list as DER: the same copy.
        let der = X509Crl::from_pem(&revoking).unwrap().to_der().unwrap();
        assert_eq!(b.add_crl_file(&der).unwrap(), 1);
        assert_eq!(b.crl_files().len(), 1);
        // Two lists in one PEM file.
        let mut both = fixture("crl-empty.pem");
        both.extend_from_slice(&revoking);
        assert_eq!(b.add_crl_file(&both).unwrap(), 2);
        let files = b.crl_files();
        assert_eq!(files.len(), 2);
        assert!(files.iter().all(|f| f.starts_with(path.join(FOLDER))));
        let mut listed = b.crl_list();
        listed.sort_by_key(|l| l.revoked);
        assert_eq!(listed.iter().map(|l| l.revoked).collect::<Vec<_>>(), [0, 1]);
        assert!(listed.iter().all(|l| l.issuer == "Picacho Test CA" && l.next_update.is_some()));

        assert!(b.add_crl_file(&fixture("ana.pem")).is_err(), "a certificate isn't a list");
        assert!(b.add_crl_file(b"nothing").is_err());
        assert!(b.remove_crl_file("../secret").is_err());
        b.remove_crl_file(&listed[1].id).unwrap();
        assert_eq!(b.crl_list(), [listed[0].clone()]);
        std::fs::remove_dir_all(path).unwrap();
    }

    /// With a list added, S/MIME mail is read with it (whose effect on a
    /// signer `smime`'s own tests show), and read again once it changes.
    #[test]
    fn smime_mail_is_read_with_the_added_lists() {
        let (b, path) = backend();
        let raw = fixture("two-signers.eml");
        assert!(b.smime_crl_check().is_none());
        b.open_smime(Some("m"), &raw).unwrap();
        let before = b.smime_verdict("m").unwrap();

        b.add_crl_file(&fixture("crl-bo-revoked.pem")).unwrap();
        assert_eq!(b.smime_crl_check().unwrap().files, b.crl_files());
        assert!(b.smime_verdict("m").is_none(), "read again with the list");
        b.open_smime(Some("m"), &raw).unwrap();
        // The fixtures' authority isn't one the system trusts, so the list
        // (checked with the system's authorities) can't speak for it.
        assert_eq!(b.smime_verdict("m").unwrap(), before);
        let id = b.crl_list()[0].id.clone();
        b.remove_crl_file(&id).unwrap();
        assert!(b.smime_crl_check().is_none() && b.smime_verdict("m").is_none());
        std::fs::remove_dir_all(path).unwrap();
    }

    /// Check sender's revocation step (RFC 5280 §6.3, RFC 6960): with
    /// nothing to ask (the fixtures name no responder or list), every
    /// signer is unknown, never revoked, and the verdict is untouched;
    /// mail that isn't S/MIME-signed has nothing to check.
    #[test]
    fn revocation_is_checked_per_signer_and_unknown_is_never_revoked() {
        let (b, path) = backend();
        b.open_smime(Some("m"), &fixture("two-signers.eml")).unwrap();
        let before = b.smime_verdict("m").unwrap();
        assert_eq!(before.checked.len(), 2, "both signers' certificates are kept for it");
        let fetcher = crate::manage::smime::revocation::FakeFetcher::default();
        let (line, good) = b.check_revocation("m", &fetcher).unwrap();
        assert!(!good && line.starts_with("Revocation couldn't be checked"), "{line}");
        assert!(line.contains("ana@example.org") && line.contains("bo@example.se"), "{line}");
        assert_eq!(b.smime_verdict("m").unwrap(), before, "unknown changes nothing");
        assert!(fetcher.asked.lock().unwrap().is_empty(), "nothing named, nothing asked");
        assert!(b.check_revocation("other", &fetcher).is_none());
        std::fs::remove_dir_all(path).unwrap();
    }
}

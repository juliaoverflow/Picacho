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
}

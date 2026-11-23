//! Keys by where they came from: which found keys count as checked, and
//! (with Autocrypt 1.1 §2.3) what Picacho remembers of each person whose
//! mail carried an Autocrypt header.
use super::{Keyring, SignedPublicKey, fingerprint, has_address};
use anyhow::Result;
use pgp::composed::Deserializable as _;

impl Keyring {
    /// Whether the found key with this fingerprint needs no comparing with
    /// its owner: you marked it as checked, or its domain published it in
    /// DNSSEC-validated DNS (RFC 7929 §7).
    pub(super) fn checked(&self, fp: &str) -> bool {
        self.verified.iter().any(|f| f == fp) || self.dnssec.iter().any(|f| f == fp)
    }
    /// As `add_found`, for a key from a lookup: one from DNS that DNSSEC
    /// validated is the domain's own word (RFC 7929 §7) and is kept as
    /// such; every other source's key is unverified until compared.
    pub fn add_found_from(
        &mut self,
        email: &str,
        data: &[u8],
        source: crate::sync::keys::Source,
    ) -> Result<bool> {
        let added = self.add_found(email, data)?;
        if added
            && let Some(fp) = self.found_fingerprint(email)
        {
            self.dnssec.retain(|f| f != &fp);
            if source.domain_vouched() {
                self.dnssec.push(fp);
            }
        }
        Ok(added)
    }
    /// The fingerprint of the found key kept for `email`, if there is one.
    pub(super) fn found_fingerprint(&self, email: &str) -> Option<String> {
        self.public
            .iter()
            .filter_map(|k| SignedPublicKey::from_string(k).ok().map(|(k, _)| k))
            .filter(|k| has_address(k, email))
            .map(|k| fingerprint(&k))
            .find(|f| self.found.contains(f))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::keys::Source;

    fn public_bytes(uid: &str) -> Vec<u8> {
        use pgp::ser::Serialize as _;
        SignedPublicKey::from(super::super::test_key(uid, ""))
            .to_bytes()
            .unwrap()
    }

    /// RFC 7929 §7: a key from DNS is checked only when DNSSEC validated
    /// the answer; from insecure DNS, WKD or a keyserver it is unverified.
    #[test]
    fn dns_keys_count_as_checked_only_when_dnssec_validated() {
        let ana = public_bytes("Ana <ana@example.org>");
        for (source, checked) in [
            (Source::Dane { dnssec_validated: true }, true),
            (Source::Dane { dnssec_validated: false }, false),
            (Source::Wkd, false),
            (Source::KeysOpenpgpOrg, false),
        ] {
            let mut ring = Keyring::default();
            assert!(ring.add_found_from("ana@example.org", &ana, source).unwrap());
            assert_eq!(ring.list()[0].verified, checked, "{source:?}");
            assert_eq!(
                ring.unverified(&["ana@example.org".into()]).is_empty(),
                checked,
                "{source:?}"
            );
        }
        // A validated DNS key isn't replaced by a key someone else sends,
        // as a key you checked isn't.
        let mut ring = Keyring::default();
        ring.add_found_from("ana@example.org", &ana, Source::Dane { dnssec_validated: true })
            .unwrap();
        let other = public_bytes("Ana <ana@example.org>");
        assert!(!ring.add_found("ana@example.org", &other).unwrap());
        // A newer unverified key for the address doesn't inherit the mark.
        let mut ring = Keyring::default();
        ring.add_found_from("ana@example.org", &ana, Source::Dane { dnssec_validated: false })
            .unwrap();
        assert!(ring.add_found_from("ana@example.org", &other, Source::Wkd).unwrap());
        assert!(ring.dnssec.is_empty());
        // Removing the key forgets the mark.
        let mut ring = Keyring::default();
        ring.add_found_from("ana@example.org", &ana, Source::Dane { dnssec_validated: true })
            .unwrap();
        let fp = ring.list()[0].fingerprint.clone();
        ring.remove(&fp);
        assert!(ring.dnssec.is_empty());
    }
}

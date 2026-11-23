//! Keys by where they came from: which found keys count as checked, and
//! (with Autocrypt 1.1 §2.3) what Picacho remembers of each person whose
//! mail carried an Autocrypt header.
use super::{Keyring, SignedPublicKey, fingerprint, has_address};
use anyhow::Result;
use pgp::composed::Deserializable as _;
use serde::{Deserialize, Serialize};

/// What Picacho remembers of a person from their mail (Autocrypt 1.1
/// §2.3, "Communication Peers"): `peers[addr]`. Kept as the preference
/// `peer_preference_key(addr)`; every field defaults to empty, so state
/// written by an older Picacho (or a missing field) still reads.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Peer {
    /// The newest effective date of any message from them, Unix seconds.
    #[serde(default)]
    pub last_seen: i64,
    /// The newest effective date of a message from them with a valid
    /// Autocrypt header, Unix seconds.
    #[serde(default)]
    pub autocrypt_timestamp: i64,
    /// That header's `prefer-encrypt=mutual`; `nopreference` otherwise.
    #[serde(default)]
    pub mutual: bool,
    /// The fingerprint of that header's key (`public_key`); empty in state
    /// from before it was kept.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub fingerprint: String,
}

/// The preference holding `email`'s peer state.
pub fn peer_preference_key(email: &str) -> String {
    format!("autocrypt_seen:{}", email.trim().to_lowercase())
}

impl Peer {
    /// Peer state as kept: JSON, or the older `"<date> <mutual|nopreference>"`
    /// (the date of the newest Autocrypt header taken, which stood for
    /// both timestamps). Anything unreadable is no state.
    pub fn from_preference(value: &str) -> Self {
        if let Ok(peer) = serde_json::from_str(value) {
            return peer;
        }
        let mut words = value.split_whitespace();
        let Some(date) = words.next().and_then(|d| d.parse::<i64>().ok()) else {
            return Self::default();
        };
        Self {
            last_seen: date,
            autocrypt_timestamp: date,
            mutual: words.next() == Some("mutual"),
            fingerprint: String::new(),
        }
    }
    pub fn to_preference(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }
}

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

    /// Peer state and keyrings written before peer state had its fields
    /// load, with the new fields empty.
    #[test]
    fn old_format_peer_state_and_keyring_load() {
        assert_eq!(
            Peer::from_preference("1700000000 mutual"),
            Peer {
                last_seen: 1_700_000_000,
                autocrypt_timestamp: 1_700_000_000,
                mutual: true,
                fingerprint: String::new(),
            }
        );
        assert!(!Peer::from_preference("1700000000 nopreference").mutual);
        assert_eq!(Peer::from_preference(""), Peer::default());
        assert_eq!(Peer::from_preference("{}"), Peer::default());
        let peer = Peer {
            last_seen: 2,
            autocrypt_timestamp: 1,
            mutual: true,
            fingerprint: "AB".into(),
        };
        assert_eq!(Peer::from_preference(&peer.to_preference()), peer);
        assert_eq!(peer_preference_key(" Ana@Example.ORG"), "autocrypt_seen:ana@example.org");
        // A keyring as an older Picacho saved it: no `dnssec` or
        // `autocrypt` fields.
        let old = r#"{"secret":[],"public":[],"found":["AB"],"verified":["AB"]}"#;
        let ring: Keyring = serde_json::from_str(old).unwrap();
        assert_eq!(ring.found, ["AB"]);
        assert_eq!(ring.verified, ["AB"]);
        assert!(ring.dnssec.is_empty() && ring.autocrypt.is_empty());
        // And saved again, the empty new fields aren't written.
        assert_eq!(serde_json::to_string(&ring).unwrap(), old);
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

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
    /// Takes in a message from the peer with effective date `date`, and
    /// its valid Autocrypt header's `prefer-encrypt=mutual` and key
    /// fingerprint if it had one (Autocrypt 1.1 §2.3, "Updating Autocrypt
    /// Peer State"). Timestamps only move forward. Returns whether the
    /// header is now the youngest seen, so its key is the peer's.
    pub fn observe(&mut self, date: i64, header: Option<(bool, String)>) -> bool {
        // §2.3: a message older than the youngest Autocrypt header changes
        // nothing.
        if date < self.autocrypt_timestamp {
            return false;
        }
        self.last_seen = self.last_seen.max(date);
        let Some((mutual, fingerprint)) = header else {
            return false;
        };
        self.autocrypt_timestamp = date;
        self.mutual = mutual;
        self.fingerprint = fingerprint;
        true
    }
}

/// How much older than last_seen the youngest Autocrypt header may be
/// before its key is stale (Autocrypt 1.1 §2.3, "Preliminary
/// Recommendation"): 35 days, in seconds.
const STALE_AFTER: i64 = 35 * 24 * 60 * 60;

impl Peer {
    /// Whether the peer has since sent mail without Autocrypt for more than
    /// 35 days (their app may no longer read encrypted mail), so encrypting
    /// to them is discouraged (Autocrypt 1.1 §2.3).
    pub fn stale(&self) -> bool {
        self.last_seen - self.autocrypt_timestamp > STALE_AFTER
    }
}

/// What Autocrypt suggests for encrypting a message (Autocrypt 1.1 §2.3,
/// "Recommendation structure"). Only a suggestion: it never turns
/// Encrypt on in the composer by itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Recommendation {
    /// Encryption isn't possible: a recipient has no usable key.
    Disable,
    /// Possible, but a recipient may not read it (a stale or gossiped key).
    Discourage,
    /// Possible; not the default.
    Available,
    /// Possible, and the default: a reply to encrypted mail, or everyone
    /// prefers it (prefer-encrypt=mutual on both sides).
    Encrypt,
}

impl Keyring {
    /// Autocrypt's recommendation for a message to `recipients`, each with
    /// their peer state if any (Autocrypt 1.1 §2.3): `own_mutual` is the
    /// sending account's prefer-encrypt=mutual, and `reply_to_encrypted`
    /// whether the message replies to an encrypted one.
    ///
    /// A recipient's key is the one Picacho would encrypt to (`key_for`),
    /// treated as absent if it has no usable encryption subkey. A key
    /// from another source with no peer state counts as available, as §2.3
    /// allows ("keys from other sources (e.g., a local keyring)"). A key
    /// known only from Autocrypt-Gossip (Autocrypt's, but not the one their
    /// own header gave) is discouraged, as §2.3 does for `gossip_key`.
    // Backend-only until the UI calls it: see `Backend::autocrypt_recommendation`.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn recommend(
        &self,
        recipients: &[(String, Option<Peer>)],
        own_mutual: bool,
        reply_to_encrypted: bool,
    ) -> Recommendation {
        if recipients.is_empty() {
            return Recommendation::Disable;
        }
        let each: Vec<Recommendation> = recipients
            .iter()
            .map(|(email, peer)| self.recommend_one(email, peer.as_ref(), own_mutual, reply_to_encrypted))
            .collect();
        // §2.3, "Recommendations for messages to multiple addresses": the
        // earliest rule that matches.
        if each.contains(&Recommendation::Disable) {
            Recommendation::Disable
        } else if each.iter().all(|r| *r == Recommendation::Encrypt) {
            Recommendation::Encrypt
        } else if each.contains(&Recommendation::Discourage) {
            Recommendation::Discourage
        } else {
            Recommendation::Available
        }
    }
    /// §2.3, "Recommendations for single-recipient messages".
    #[cfg_attr(not(test), allow(dead_code))]
    fn recommend_one(
        &self,
        email: &str,
        peer: Option<&Peer>,
        own_mutual: bool,
        reply_to_encrypted: bool,
    ) -> Recommendation {
        let Some(key) = self.key_for(email).filter(|k| super::encryption_subkey(k).is_some()) else {
            return Recommendation::Disable;
        };
        let fp = fingerprint(&key);
        let gossip_only =
            self.autocrypt.contains(&fp) && peer.is_none_or(|p| p.fingerprint != fp);
        let preliminary = if gossip_only || peer.is_some_and(Peer::stale) {
            Recommendation::Discourage
        } else {
            Recommendation::Available
        };
        let mutual = !gossip_only && peer.is_some_and(|p| p.mutual) && own_mutual;
        if reply_to_encrypted || (preliminary == Recommendation::Available && mutual) {
            Recommendation::Encrypt
        } else {
            preliminary
        }
    }
}

/// The fingerprint of the first key in `data` (binary or armored).
pub fn key_fingerprint(data: &[u8]) -> Option<String> {
    let (mut keys, _) = SignedPublicKey::from_reader_many(data).ok()?;
    keys.next()?.ok().map(|k| fingerprint(&k))
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
            // Found by a lookup too: no longer only Autocrypt's.
            self.autocrypt.retain(|f| f != &fp);
            if source.domain_vouched() {
                self.dnssec.push(fp);
            }
        }
        Ok(added)
    }
    /// As `add_found`, for a key from an Autocrypt or Autocrypt-Gossip
    /// header (Autocrypt 1.1 §2.3, §2.4): marked as Autocrypt's, so
    /// `reset_peer` can forget it, unless it was already kept from a lookup.
    pub fn add_autocrypt(&mut self, email: &str, data: &[u8]) -> Result<bool> {
        let before = self.found_fingerprint(email);
        let added = self.add_found(email, data)?;
        if added
            && let Some(fp) = self.found_fingerprint(email)
        {
            // The same key kept from a lookup is only refreshed, unmarked.
            if before.as_deref() != Some(fp.as_str()) && !self.autocrypt.contains(&fp) {
                self.autocrypt.push(fp);
            }
        }
        Ok(added)
    }
    /// Forgets the key Autocrypt gave for `email` (Autocrypt 1.1 §2.3 peer
    /// state is the caller's): only a key known solely from Autocrypt
    /// headers or Gossip, never one imported by hand, found by WKD, DNS or
    /// a keyserver, or compared with its owner. Returns whether one was.
    pub fn reset_peer(&mut self, email: &str) -> bool {
        let Some(fp) = self.found_fingerprint(email) else {
            return false;
        };
        if !self.autocrypt.contains(&fp) || self.checked(&fp) {
            return false;
        }
        self.remove(&fp);
        true
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

    /// Autocrypt 1.1 §2.3: last_seen moves with every newer message;
    /// autocrypt_timestamp, the key and prefer-encrypt only with a newer
    /// valid header; nothing moves for a message older than that header.
    #[test]
    fn peer_state_updates_only_forward() {
        let mut peer = Peer::default();
        assert!(peer.observe(100, Some((true, "A".into()))));
        assert_eq!((peer.last_seen, peer.autocrypt_timestamp, peer.mutual), (100, 100, true));
        // Newer mail without a header: only last_seen.
        assert!(!peer.observe(200, None));
        assert_eq!((peer.last_seen, peer.autocrypt_timestamp), (200, 100));
        // Older than the header: nothing, with or without a header.
        assert!(!peer.observe(50, Some((false, "B".into()))));
        assert!(!peer.observe(50, None));
        assert_eq!(peer.fingerprint, "A");
        // Between the header and last_seen, with a header: the key and
        // preference change; last_seen stays the newest.
        assert!(peer.observe(150, Some((false, "B".into()))));
        assert_eq!(
            peer,
            Peer { last_seen: 200, autocrypt_timestamp: 150, mutual: false, fingerprint: "B".into() }
        );
    }

    /// reset_peer forgets only a key Autocrypt alone gave: never one
    /// imported by hand, found by WKD, DNS or a keyserver, or checked.
    #[test]
    fn reset_peer_never_touches_manual_wkd_dns_or_verified_keys() {
        let ana = public_bytes("Ana <ana@example.org>");
        // Autocrypt's own: forgotten, and only Ana's.
        let bo = public_bytes("Bo <bo@example.se>");
        let mut ring = Keyring::default();
        ring.add_autocrypt("ana@example.org", &ana).unwrap();
        ring.add_autocrypt("bo@example.se", &bo).unwrap();
        assert!(ring.reset_peer("Ana@Example.org"));
        assert!(ring.key_for("ana@example.org").is_none());
        assert!(ring.key_for("bo@example.se").is_some());
        assert!(!ring.autocrypt.contains(&key_fingerprint(&ana).unwrap()));
        assert!(!ring.reset_peer("ana@example.org"));
        // Imported by hand, even after Autocrypt sent the same key.
        let mut ring = Keyring::default();
        let armored = SignedPublicKey::from_bytes(&ana[..])
            .unwrap()
            .to_armored_string(Default::default())
            .unwrap();
        ring.import(&armored, "").unwrap();
        ring.add_autocrypt("ana@example.org", &ana).unwrap();
        let before = ring.clone();
        assert!(!ring.reset_peer("ana@example.org"));
        assert_eq!(ring, before);
        // Found by WKD, by DNS (validated or not) or a keyserver, even
        // when Autocrypt sent the same key after.
        for source in [
            Source::Wkd,
            Source::Dane { dnssec_validated: true },
            Source::Dane { dnssec_validated: false },
            Source::KeysOpenpgpOrg,
        ] {
            let mut ring = Keyring::default();
            ring.add_found_from("ana@example.org", &ana, source).unwrap();
            ring.add_autocrypt("ana@example.org", &ana).unwrap();
            let before = ring.clone();
            assert!(!ring.reset_peer("ana@example.org"), "{source:?}");
            assert_eq!(ring, before, "{source:?}");
        }
        // From Autocrypt, but compared with its owner.
        let mut ring = Keyring::default();
        ring.add_autocrypt("ana@example.org", &ana).unwrap();
        ring.verify(&key_fingerprint(&ana).unwrap());
        let before = ring.clone();
        assert!(!ring.reset_peer("ana@example.org"));
        assert_eq!(ring, before);
    }

    /// Autocrypt 1.1 §2.3's recommendation, for one recipient and for
    /// several.
    #[test]
    fn recommendations_follow_autocrypt() {
        use Recommendation::*;
        let day = 24 * 60 * 60;
        let ana = public_bytes("Ana <ana@example.org>");
        let bo = public_bytes("Bo <bo@example.se>");
        let mut ring = Keyring::default();
        ring.add_autocrypt("ana@example.org", &ana).unwrap();
        let peer = |mutual, header_age: i64, key: &[u8]| Peer {
            last_seen: 100 * day,
            autocrypt_timestamp: 100 * day - header_age,
            mutual,
            fingerprint: key_fingerprint(key).unwrap(),
        };
        let to = |peer: Option<Peer>| vec![("ana@example.org".to_owned(), peer)];
        // No key: disable, even replying to encrypted mail.
        assert_eq!(ring.recommend(&[("cy@example.net".into(), None)], true, true), Disable);
        assert_eq!(ring.recommend(&[], false, false), Disable);
        // Fresh key: available; encrypt when both prefer it or replying.
        assert_eq!(ring.recommend(&to(Some(peer(false, 0, &ana))), true, false), Available);
        assert_eq!(ring.recommend(&to(Some(peer(true, 0, &ana))), false, false), Available);
        assert_eq!(ring.recommend(&to(Some(peer(true, 0, &ana))), true, false), Encrypt);
        assert_eq!(ring.recommend(&to(Some(peer(false, 0, &ana))), false, true), Encrypt);
        // Stale (header over 35 days older than last_seen): discourage,
        // even with mutual on both sides; a reply still encrypts.
        assert_eq!(ring.recommend(&to(Some(peer(true, 36 * day, &ana))), true, false), Discourage);
        assert_eq!(ring.recommend(&to(Some(peer(true, 35 * day, &ana))), true, false), Encrypt);
        assert_eq!(ring.recommend(&to(Some(peer(false, 36 * day, &ana))), false, true), Encrypt);
        // Known only from Gossip (no header of theirs gave it): discourage.
        assert_eq!(ring.recommend(&to(None), true, false), Discourage);
        assert_eq!(ring.recommend(&to(Some(peer(true, 0, &bo))), true, false), Discourage);
        // A key from elsewhere, without peer state: available.
        let mut found = ring.clone();
        found.add_found_from("bo@example.se", &bo, Source::Wkd).unwrap();
        assert_eq!(found.recommend(&[("bo@example.se".into(), None)], true, false), Available);
        // Several: disable beats all, encrypt needs everyone, then discourage.
        let both = |a: Option<Peer>| {
            vec![("ana@example.org".to_owned(), a), ("bo@example.se".to_owned(), None)]
        };
        assert_eq!(ring.recommend(&both(Some(peer(true, 0, &ana))), true, true), Disable);
        assert_eq!(found.recommend(&both(Some(peer(true, 0, &ana))), true, true), Encrypt);
        assert_eq!(found.recommend(&both(Some(peer(true, 0, &ana))), true, false), Available);
        assert_eq!(found.recommend(&both(Some(peer(true, 36 * day, &ana))), true, false), Discourage);
    }

    /// Autocrypt 1.1 §2.3: stale only when the header is more than 35
    /// days older than last_seen.
    #[test]
    fn peer_keys_go_stale_after_35_days() {
        let day = 24 * 60 * 60;
        let peer = |ago: i64| Peer { last_seen: 100 * day, autocrypt_timestamp: 100 * day - ago, ..Peer::default() };
        assert!(!peer(0).stale());
        assert!(!peer(35 * day).stale());
        assert!(peer(35 * day + 1).stale());
    }

    /// Keys from Autocrypt are marked as such; one a lookup also found
    /// isn't, either way round.
    #[test]
    fn autocrypt_keys_are_marked_by_source() {
        let ana = public_bytes("Ana <ana@example.org>");
        let fp = key_fingerprint(&ana).unwrap();
        let mut ring = Keyring::default();
        assert!(ring.add_autocrypt("ana@example.org", &ana).unwrap());
        assert_eq!(ring.autocrypt, [fp.clone()]);
        assert!(ring.add_found_from("ana@example.org", &ana, Source::Wkd).unwrap());
        assert!(ring.autocrypt.is_empty());
        let mut ring = Keyring::default();
        ring.add_found_from("ana@example.org", &ana, Source::KeysOpenpgpOrg).unwrap();
        assert!(ring.add_autocrypt("ana@example.org", &ana).unwrap());
        assert!(ring.autocrypt.is_empty());
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

//! Checks who sent a message: its DKIM signatures, SPF for the server that
//! handed it to the mailbox's provider, and the From domain's DMARC policy.
//!
//! The answers are worked out here, from the message and DNS, rather than
//! read from `Authentication-Results` or `DKIM-Signature` headers, which a
//! sender can write themselves. They reflect DNS now, not at delivery: a
//! signing key since retired can't verify an old signature, so for older
//! mail that is reported as "couldn't verify", not as a failure.
//!
//! Lookups go through the system's DNS resolver, and only when the person
//! asks for them (Check sender in a message's details, or unsubscribing).
//! The sender's own DNS servers see each lookup for its domain, at the
//! time it is made: a sender that gives every recipient their own DKIM
//! selector learns when that recipient's message was checked, much as a
//! remote image would tell it the message was opened.
use super::mime::{Arrival, Details};
use mail_auth::{
    AuthenticatedMessage, DkimOutput, DkimResult, DmarcResult, DnsCache, MessageAuthenticator,
    Mx, Negative, Parameters, RecordSet, ResolverCache, SpfOutput, SpfResult, TxtRecord,
    dmarc::verify::DmarcParameters, spf::verify::SpfParameters,
};
use std::{
    borrow::Borrow,
    collections::HashMap,
    hash::Hash,
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

/// The longest all of one message's checks may take.
const TIMEOUT: Duration = Duration::from_secs(15);
/// Answers kept per record type before the oldest are dropped.
const CACHE_ENTRIES: usize = 2000;
/// How old a message must be before a signature that no longer verifies
/// against today's key is put down to the key having changed.
const KEY_ROTATION_AGE: i64 = 7 * 24 * 3600;

/// DNS answers, kept until they expire, so opening several messages from
/// one sender looks its records up once.
pub(crate) struct MapCache<K, V>(Mutex<HashMap<K, (V, Instant)>>);
impl<K, V> Default for MapCache<K, V> {
    fn default() -> Self {
        Self(Mutex::new(HashMap::new()))
    }
}
impl<K, V> MapCache<K, V> {
    /// The answers, also after a check panicked while holding them.
    fn map(&self) -> std::sync::MutexGuard<'_, HashMap<K, (V, Instant)>> {
        self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}
impl<K: Hash + Eq, V: Clone> ResolverCache<K, V> for MapCache<K, V> {
    fn get<Q>(&self, name: &Q) -> Option<V>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        let map = self.map();
        let (value, expires) = map.get(name)?;
        (*expires > Instant::now()).then(|| value.clone())
    }
    fn remove<Q>(&self, name: &Q) -> Option<V>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        self.map().remove(name).map(|(value, _)| value)
    }
    fn insert(&self, key: K, value: V, valid_until: Instant) {
        let mut map = self.map();
        if map.len() >= CACHE_ENTRIES {
            let now = Instant::now();
            map.retain(|_, (_, expires)| *expires > now);
            if map.len() >= CACHE_ENTRIES {
                map.clear();
            }
        }
        map.insert(key, (value, valid_until));
    }
}
type Answer<T> = Result<RecordSet<T>, Negative>;
#[derive(Default)]
pub(crate) struct Cache {
    pub(crate) txt: MapCache<Box<str>, TxtRecord>,
    mx: MapCache<Box<str>, Answer<Mx>>,
    ipv4: MapCache<Box<str>, Answer<Ipv4Addr>>,
    ipv6: MapCache<Box<str>, Answer<Ipv6Addr>>,
    ptr: MapCache<IpAddr, Answer<Box<str>>>,
}
impl DnsCache for Cache {
    type Txt = MapCache<Box<str>, TxtRecord>;
    type Mx = MapCache<Box<str>, Answer<Mx>>;
    type Ipv4 = MapCache<Box<str>, Answer<Ipv4Addr>>;
    type Ipv6 = MapCache<Box<str>, Answer<Ipv6Addr>>;
    type Ptr = MapCache<IpAddr, Answer<Box<str>>>;
    fn txt(&self) -> Option<&Self::Txt> {
        Some(&self.txt)
    }
    fn mx(&self) -> Option<&Self::Mx> {
        Some(&self.mx)
    }
    fn ipv4(&self) -> Option<&Self::Ipv4> {
        Some(&self.ipv4)
    }
    fn ipv6(&self) -> Option<&Self::Ipv6> {
        Some(&self.ipv6)
    }
    fn ptr(&self) -> Option<&Self::Ptr> {
        Some(&self.ptr)
    }
}

struct Checker {
    runtime: tokio::runtime::Runtime,
    authenticator: MessageAuthenticator,
    cache: Cache,
}
/// The resolver and its runtime; None when the system's DNS settings can't
/// be read.
fn checker() -> Option<&'static Checker> {
    static CHECKER: OnceLock<Option<Checker>> = OnceLock::new();
    CHECKER
        .get_or_init(|| {
            // Each check blocks its own caller's thread; the workers drive
            // DNS, so several messages' checks run at once rather than
            // queueing behind one another's timeouts.
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .thread_name("picacho-dns")
                .enable_all()
                .build()
                .ok()?;
            let authenticator = runtime.block_on(async {
                let (config, mut options) =
                    mail_auth::hickory_resolver::system_conf::read_system_conf().ok()?;
                // A slow server delays only the details panel, briefly.
                options.timeout = Duration::from_secs(4);
                options.attempts = 2;
                MessageAuthenticator::new(config, options).ok()
            })?;
            Some(Checker {
                runtime,
                authenticator,
                cache: Cache::default(),
            })
        })
        .as_ref()
}

/// RFC 8601's name for a DKIM result.
fn dkim_name(result: &DkimResult) -> &'static str {
    match result {
        DkimResult::Pass => "pass",
        DkimResult::Fail(_) => "fail",
        DkimResult::Neutral(_) => "neutral",
        DkimResult::TempError(_) => "temperror",
        DkimResult::PermError(_) => "permerror",
        DkimResult::None => "none",
    }
}
fn dmarc_name(result: &DmarcResult) -> &'static str {
    match result {
        DmarcResult::Pass => "pass",
        DmarcResult::Fail(_) => "fail",
        DmarcResult::TempError(_) => "temperror",
        DmarcResult::PermError(_) => "permerror",
        DmarcResult::None => "none",
    }
}
/// The domains whose signatures verified, each once.
fn verified_domains(outputs: &[DkimOutput]) -> Vec<String> {
    let mut domains: Vec<String> = Vec::new();
    for o in outputs.iter().filter(|o| *o.result() == DkimResult::Pass) {
        if let Some(d) = o.signature().map(|s| s.d.to_ascii_lowercase())
            && !domains.contains(&d)
        {
            domains.push(d);
        }
    }
    domains
}

/// What could be checked of one message.
#[derive(Debug, Default, PartialEq)]
struct Verdicts {
    signed_by: Vec<String>,
    dkim: Option<String>,
    spf: Option<String>,
    mailed_by: Option<String>,
    dmarc: Option<String>,
}
/// Whether a signature's result could come from its signing key having
/// been replaced or withdrawn since: it no longer verifies, or its key
/// record is gone or revoked. A changed body, by contrast, is a failure
/// whatever the key.
fn key_may_have_changed(result: &DkimResult) -> bool {
    use mail_auth::{Error, dkim::DkimError};
    match result {
        DkimResult::Fail(e) | DkimResult::PermError(e) => matches!(
            e,
            Error::Crypto(_) | Error::Dns(_) | Error::Dkim(DkimError::PublicKeyRevoked)
        ),
        _ => false,
    }
}
/// The verdict for DKIM, or DMARC, that could not be confirmed either way.
pub const UNVERIFIABLE: &str = "unverifiable";

async fn verify(
    authenticator: &MessageAuthenticator,
    cache: &Cache,
    message: &AuthenticatedMessage<'_>,
    arrival: Option<&Arrival>,
    return_path: &str,
    old: bool,
) -> Verdicts {
    let dkim = authenticator
        .verify_dkim(Parameters::new(message).with_cache(cache))
        .await;
    // SPF needs the address the provider received the message from.
    let spf = match arrival.and_then(|a| a.ip.map(|ip| (a, ip))) {
        Some((a, ip)) => {
            let helo = if a.helo.is_empty() { "unknown" } else { a.helo.as_str() };
            Some(
                authenticator
                    .verify_spf(
                        Parameters::new(SpfParameters::mail_from(ip, helo, &a.by, return_path))
                            .with_cache(cache),
                    )
                    .await,
            )
        }
        None => None,
    };
    let mail_from = spf
        .as_ref()
        .map(|s| s.domain().to_owned())
        .unwrap_or_default();
    let unchecked = SpfOutput::new(mail_from.clone()).with_result(SpfResult::None);
    let dmarc = authenticator
        .verify_dmarc(
            Parameters::new(DmarcParameters::new(
                message,
                &dkim,
                &mail_from,
                spf.as_ref().unwrap_or(&unchecked),
            ))
            .with_cache(cache),
        )
        .await;
    let signed_by = verified_domains(&dkim);
    // An older message whose signatures no longer verify against today's
    // keys can't be told from a forgery: neither is called a failure.
    let unverifiable =
        signed_by.is_empty() && old && dkim.iter().any(|o| key_may_have_changed(o.result()));
    // One verdict for all signatures: a pass if any verified.
    let dkim_verdict = if !signed_by.is_empty() {
        Some("pass".into())
    } else if unverifiable {
        Some(UNVERIFIABLE.into())
    } else {
        dkim.first().map(|o| dkim_name(o.result()).to_owned())
    };
    let dmarc_verdict = match dmarc.result() {
        // DMARC failed only for want of the signature that couldn't be checked.
        DmarcResult::Fail(_) if unverifiable => UNVERIFIABLE,
        result => dmarc_name(&result),
    };
    Verdicts {
        signed_by,
        dkim: dkim_verdict,
        mailed_by: spf
            .as_ref()
            .filter(|s| s.result() == SpfResult::Pass)
            .map(|s| s.domain().to_ascii_lowercase()),
        spf: spf.map(|s| s.result().to_string()),
        dmarc: Some(dmarc_verdict.into()),
    }
}

/// Fills `details` with what could be checked: the domains whose DKIM
/// signatures verified, the envelope sender's domain when SPF passed, and
/// the SPF, DKIM and DMARC results. When nothing can be checked (no DNS, or
/// no time), they stay empty and read "not recorded", never a pass.
///
/// Called from any thread outside Tokio. The checks parse what a sender
/// wrote, so a panic in them leaves the results empty rather than stopping
/// the details from showing.
pub fn check(raw: &[u8], details: &mut Details) {
    details.checked = true;
    let arrival = details.arrival.clone();
    let return_path = details.return_path.clone().unwrap_or_default();
    let old = age(raw).is_some_and(|age| age > KEY_ROTATION_AGE);
    let verdicts = std::panic::catch_unwind(|| {
        let checker = checker()?;
        let message = AuthenticatedMessage::parse(raw)?;
        // The timer is made inside the runtime: Tokio's timers need one.
        checker
            .runtime
            .block_on(async {
                tokio::time::timeout(
                    TIMEOUT,
                    verify(
                        &checker.authenticator,
                        &checker.cache,
                        &message,
                        arrival.as_ref(),
                        &return_path,
                        old,
                    ),
                )
                .await
            })
            .ok()
    });
    let Ok(Some(v)) = verdicts else {
        return;
    };
    details.signed_by = v.signed_by;
    details.dkim = v.dkim;
    details.spf = v.spf;
    details.mailed_by = v.mailed_by;
    details.dmarc = v.dmarc;
}

/// How long ago, in seconds, a message says it was written.
fn age(raw: &[u8]) -> Option<i64> {
    use mailparse::MailHeaderMap;
    let (headers, _) = mailparse::parse_headers(raw).ok()?;
    let date = mailparse::dateparse(&headers.get_first_value("Date")?).ok()?;
    Some(chrono::Utc::now().timestamp() - date)
}

/// A message's List-Unsubscribe header, as its sender signed it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignedUnsubscribe {
    pub header: String,
    /// Whether one-click (RFC 8058) is offered, with List-Unsubscribe-Post
    /// signed too.
    pub one_click: bool,
}

/// The List-Unsubscribe header of `raw`, when a DKIM signature that
/// verifies, from the From address's domain (or a parent of it), covers it
/// (RFC 8058, Section 4). Anything else in a mailbox could have been written
/// by anyone using the sender's address. None too when DNS can't be reached.
///
/// Called from any thread outside Tokio.
pub fn signed_unsubscribe(raw: &[u8]) -> Option<SignedUnsubscribe> {
    std::panic::catch_unwind(|| {
        let checker = checker()?;
        let message = AuthenticatedMessage::parse(raw)?;
        checker
            .runtime
            .block_on(async {
                tokio::time::timeout(
                    TIMEOUT,
                    unsubscribe_signature(&checker.authenticator, &checker.cache, &message, raw),
                )
                .await
            })
            .ok()
            .flatten()
    })
    .ok()
    .flatten()
}
async fn unsubscribe_signature(
    authenticator: &MessageAuthenticator,
    cache: &Cache,
    message: &AuthenticatedMessage<'_>,
    raw: &[u8],
) -> Option<SignedUnsubscribe> {
    use mailparse::MailHeaderMap;
    let (headers, _) = mailparse::parse_headers(raw).ok()?;
    // RFC 8058: with more than one, which applies is unclear.
    let [header] = &headers.get_all_values("List-Unsubscribe")[..] else {
        return None;
    };
    let from = mailparse::addrparse_header(headers.get_first_header("From")?).ok()?;
    let [mailparse::MailAddr::Single(from)] = &from[..] else {
        return None;
    };
    let from_domain = from.addr.rsplit_once('@')?.1.trim().to_ascii_lowercase();
    let dkim = authenticator
        .verify_dkim(Parameters::new(message).with_cache(cache))
        .await;
    let covers = |h: &[String], name: &str| h.iter().any(|h| h.trim().eq_ignore_ascii_case(name));
    let signature = dkim
        .iter()
        .filter(|o| *o.result() == DkimResult::Pass)
        .filter_map(|o| o.signature())
        .filter(|s| {
            let d = s.d.to_ascii_lowercase();
            from_domain == d || from_domain.ends_with(&format!(".{d}"))
        })
        .find(|s| covers(&s.h, "List-Unsubscribe"))?;
    let posts = headers.get_all_values("List-Unsubscribe-Post");
    let one_click = covers(&signature.h, "List-Unsubscribe-Post")
        && posts.len() == 1
        && posts[0]
            .to_ascii_lowercase()
            .replace(' ', "")
            .contains("list-unsubscribe=one-click");
    Some(SignedUnsubscribe {
        header: header.trim().to_owned(),
        one_click,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine as _;
    use mail_auth::{
        DnsError, dkim::DomainKey, dmarc::DmarcRecord, dns::TxtRecordParser,
        hickory_resolver::proto::op::ResponseCode,
    };
    use sha2::Digest;

    /// RFC 8463's example Ed25519 key, published for `bank.example`, and a
    /// DMARC policy, so nothing here reaches the network.
    fn bank_dns() -> Cache {
        let cache = Cache::default();
        let later = Instant::now() + Duration::from_secs(3600);
        let txt = |name: &str, record: TxtRecord| {
            cache.txt.insert(name.into(), record, later);
        };
        txt(
            "s1._domainkey.bank.example.",
            DomainKey::parse(b"v=DKIM1; k=ed25519; p=11qYAYKxCrfVS/7TyWQHOg7hcvPapiMlrwIaaPcHURo=")
                .unwrap()
                .into(),
        );
        txt(
            "_dmarc.bank.example.",
            DmarcRecord::parse(b"v=DMARC1; p=reject").unwrap().into(),
        );
        txt(
            "_dmarc.example.",
            TxtRecord::Error(mail_auth::Error::Dns(DnsError::RecordNotFound(
                ResponseCode::NXDomain,
            ))),
        );
        cache
    }

    #[test]
    fn forged_or_broken_signatures_vouch_for_nobody() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        // No resolver is reached: every answer is in the cache.
        let authenticator = MessageAuthenticator::new(
            mail_auth::hickory_resolver::config::ResolverConfig::udp_and_tcp(
                &mail_auth::hickory_resolver::config::CLOUDFLARE,
            ),
            Default::default(),
        )
        .unwrap();
        let cache = bank_dns();
        let body = "Pay now.\r\n";
        let bh = base64::engine::general_purpose::STANDARD.encode(sha2::Sha256::digest(body));
        let bad = base64::engine::general_purpose::STANDARD.encode([7u8; 64]);
        for signature in [
            // Not even a signature.
            "d=bank.example; b=garbage".to_owned(),
            // Well formed, for a published key, but not signed by it.
            format!(
                "v=1; a=ed25519-sha256; c=relaxed/relaxed; d=bank.example; s=s1; h=from:subject; bh={bh}; b={bad}"
            ),
        ] {
            let raw = format!(
                "DKIM-Signature: {signature}\r\nFrom: Bank <alerts@bank.example>\r\nSubject: Urgent\r\n\r\n{body}"
            );
            let message = AuthenticatedMessage::parse(raw.as_bytes()).unwrap();
            let v = runtime.block_on(verify(&authenticator, &cache, &message, None, "", false));
            assert!(v.signed_by.is_empty(), "{signature}: {v:?}");
            assert_ne!(v.dkim.as_deref(), Some("pass"), "{signature}");
            assert_eq!(v.dmarc.as_deref(), Some("fail"), "{signature}");
            assert_eq!((v.spf, v.mailed_by), (None, None), "no recorded arrival");
        }
    }

    fn offline() -> (tokio::runtime::Runtime, MessageAuthenticator) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        // No resolver is reached: every answer is in the cache.
        let authenticator = MessageAuthenticator::new(
            mail_auth::hickory_resolver::config::ResolverConfig::udp_and_tcp(
                &mail_auth::hickory_resolver::config::CLOUDFLARE,
            ),
            Default::default(),
        )
        .unwrap();
        (runtime, authenticator)
    }
    /// Signs `message` as bank.example with RFC 8463's example key.
    fn sign(message: &str, headers: &[&str]) -> String {
        use mail_auth::{crypto::Ed25519Key, dkim::DkimSigner, headers::HeaderWriter};
        let b64 = |s: &str| base64::engine::general_purpose::STANDARD.decode(s).unwrap();
        let key = Ed25519Key::from_seed_and_public_key(
            &b64("nWGxne/9WmC6hEr0kuwsxERJxWl7MmkZcDusAxyuf2A="),
            &b64("11qYAYKxCrfVS/7TyWQHOg7hcvPapiMlrwIaaPcHURo="),
        )
        .unwrap();
        let signature = DkimSigner::from_key(key)
            .domain("bank.example")
            .selector("s1")
            .headers(headers.iter().copied())
            .sign(message.as_bytes())
            .unwrap();
        format!("{}{message}", signature.to_header())
    }

    #[test]
    fn only_a_signed_list_unsubscribe_is_trusted() {
        let (runtime, authenticator) = offline();
        let cache = bank_dns();
        let check = |raw: &str| {
            let message = AuthenticatedMessage::parse(raw.as_bytes()).unwrap();
            runtime.block_on(unsubscribe_signature(&authenticator, &cache, &message, raw.as_bytes()))
        };
        let message = "From: Bank News <news@mail.bank.example>\r\nSubject: News\r\nList-Unsubscribe: <https://bank.example/u/1>\r\nList-Unsubscribe-Post: List-Unsubscribe=One-Click\r\n\r\nHello.\r\n";
        let all = ["From", "Subject", "List-Unsubscribe", "List-Unsubscribe-Post"];
        assert_eq!(
            check(&sign(message, &all)),
            Some(SignedUnsubscribe { header: "<https://bank.example/u/1>".into(), one_click: true })
        );
        // The header signed, but not the request for one click.
        assert_eq!(
            check(&sign(message, &["From", "List-Unsubscribe"])).map(|s| s.one_click),
            Some(false)
        );
        // Signed, but the header isn't among what was signed.
        assert_eq!(check(&sign(message, &["From", "Subject"])), None);
        // A header changed after signing (as a forger replaying it would).
        let replaced = sign(message, &all).replace("https://bank.example/u/1", "https://evil.example/u");
        assert_eq!(check(&replaced), None);
        // A valid signature from a domain other than the From address's.
        let other = message.replace("news@mail.bank.example", "news@news.example");
        assert_eq!(check(&sign(&other, &all)), None);
        // Unsigned.
        assert_eq!(check(message), None);
    }

    #[test]
    fn old_mail_whose_key_changed_is_unverifiable_not_failed() {
        let (runtime, authenticator) = offline();
        let message = "From: Bank <alerts@bank.example>\r\nSubject: Statement\r\n\r\nHello.\r\n";
        let signed = sign(message, &["From", "Subject"]);
        // The key since replaced with another.
        let cache = Cache::default();
        let later = Instant::now() + Duration::from_secs(3600);
        cache.txt.insert(
            "s1._domainkey.bank.example.".into(),
            DomainKey::parse(b"v=DKIM1; k=ed25519; p=GBEUmNb5Xm9Jrl/C8pBMjqTkGbYNK5Ou1uxW0EEnfMk=")
                .unwrap()
                .into(),
            later,
        );
        cache.txt.insert(
            "_dmarc.bank.example.".into(),
            DmarcRecord::parse(b"v=DMARC1; p=reject").unwrap().into(),
            later,
        );
        let parsed = AuthenticatedMessage::parse(signed.as_bytes()).unwrap();
        let recent = runtime.block_on(verify(&authenticator, &cache, &parsed, None, "", false));
        assert_eq!(recent.dkim.as_deref(), Some("fail"));
        assert_eq!(recent.dmarc.as_deref(), Some("fail"));
        let old = runtime.block_on(verify(&authenticator, &cache, &parsed, None, "", true));
        assert_eq!(old.dkim.as_deref(), Some(UNVERIFIABLE));
        assert_eq!(old.dmarc.as_deref(), Some(UNVERIFIABLE));
        // A body changed after signing fails however old the message.
        let tampered = signed.replace("Hello.", "Pay now.");
        let parsed = AuthenticatedMessage::parse(tampered.as_bytes()).unwrap();
        let v = runtime.block_on(verify(&authenticator, &bank_dns(), &parsed, None, "", true));
        assert_ne!(v.dkim.as_deref(), Some(UNVERIFIABLE));
        assert_ne!(v.dkim.as_deref(), Some("pass"));
    }

    #[test]
    fn checks_run_from_threads_outside_tokio() {
        // As the reader does: from a plain worker thread, more than once.
        // Nothing here needs DNS: no signature, no recorded arrival, and
        // no From domain for DMARC.
        let raw = b"From: someone\r\nSubject: Hi\r\n\r\nHello\r\n";
        let checked = std::thread::spawn(move || {
            (0..2)
                .map(|_| {
                    let mut details = Details::default();
                    check(raw, &mut details);
                    details
                })
                .collect::<Vec<_>>()
        })
        .join()
        .expect("checking from outside Tokio must not panic");
        if checker().is_some() {
            for details in checked {
                // Filled in, not left empty by a swallowed failure.
                assert_eq!(details.dmarc.as_deref(), Some("none"));
                assert_eq!(details.dkim, None, "no signatures");
                assert_eq!(details.spf, None, "no recorded arrival");
            }
        }
    }

    #[test]
    fn verdicts_name_results_as_rfc_8601_does() {
        assert_eq!(dkim_name(&DkimResult::Pass), "pass");
        assert_eq!(
            dkim_name(&DkimResult::PermError(mail_auth::Error::Parse)),
            "permerror"
        );
        assert_eq!(dmarc_name(&DmarcResult::None), "none");
        assert_eq!(SpfResult::SoftFail.to_string(), "softfail");
    }
}

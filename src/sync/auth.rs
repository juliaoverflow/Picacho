//! Checks who sent a message: its DKIM signatures, SPF for the server that
//! handed it to the mailbox's provider, and the From domain's DMARC policy.
//!
//! The answers are worked out here, from the message and DNS, rather than
//! read from `Authentication-Results` or `DKIM-Signature` headers, which a
//! sender can write themselves. They reflect DNS now, not at delivery: a
//! signing key since retired makes an old signature fail.
//!
//! Lookups go through the system's DNS resolver, and only when a message's
//! details are opened. The sender's own DNS servers see the lookups for its
//! domain, as the receiving server's did on delivery.
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
            let runtime = tokio::runtime::Builder::new_current_thread()
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
async fn verify(
    authenticator: &MessageAuthenticator,
    cache: &Cache,
    message: &AuthenticatedMessage<'_>,
    arrival: Option<&Arrival>,
    return_path: &str,
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
    // One verdict for all signatures: a pass if any verified.
    let dkim_verdict = if signed_by.is_empty() {
        dkim.first().map(|o| dkim_name(o.result()).to_owned())
    } else {
        Some("pass".into())
    };
    Verdicts {
        signed_by,
        dkim: dkim_verdict,
        mailed_by: spf
            .as_ref()
            .filter(|s| s.result() == SpfResult::Pass)
            .map(|s| s.domain().to_ascii_lowercase()),
        spf: spf.map(|s| s.result().to_string()),
        dmarc: Some(dmarc_name(&dmarc.result()).into()),
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
    let arrival = details.arrival.clone();
    let return_path = details.return_path.clone().unwrap_or_default();
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
            let v = runtime.block_on(verify(&authenticator, &cache, &message, None, ""));
            assert!(v.signed_by.is_empty(), "{signature}: {v:?}");
            assert_ne!(v.dkim.as_deref(), Some("pass"), "{signature}");
            assert_eq!(v.dmarc.as_deref(), Some("fail"), "{signature}");
            assert_eq!((v.spf, v.mailed_by), (None, None), "no recorded arrival");
        }
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

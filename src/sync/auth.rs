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
/// The system's DNS configuration (its name servers and options), read
/// once. On Android, hickory reads it through the app's context
/// (ndk-context), which Picacho sets only while it reads this and opens the
/// keychain, before anything else runs (`android::attach`): read later,
/// from a thread while the context is being set or released, would be a
/// data race. Every resolver here starts from this.
pub(crate) fn system_conf() -> Option<(
    mail_auth::hickory_resolver::config::ResolverConfig,
    mail_auth::hickory_resolver::config::ResolverOpts,
)> {
    type Conf = (
        mail_auth::hickory_resolver::config::ResolverConfig,
        mail_auth::hickory_resolver::config::ResolverOpts,
    );
    static CONF: OnceLock<Option<Conf>> = OnceLock::new();
    CONF.get_or_init(|| mail_auth::hickory_resolver::system_conf::read_system_conf().ok())
        .clone()
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
                let (config, mut options) = system_conf()?;
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
    dmarc_policy: Option<String>,
    arc: Option<String>,
    partly_signed: bool,
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
    // Forwarders' seals (ARC, RFC 8617): a list or forwarder that changed
    // the message can still say what it got.
    let arc = authenticator
        .verify_arc(Parameters::new(message).with_cache(cache))
        .await;
    let arc_verdict = match arc.result() {
        DkimResult::None => None,
        result => Some(dkim_name(result).to_owned()),
    };
    // A passing signature with a body length (`l=`) covers only part.
    let partly_signed = dkim.iter().any(|o| {
        *o.result() == DkimResult::Pass && o.signature().is_some_and(|s| s.l > 0)
    });
    // SPF needs the address the provider received the message from.
    let spf = match arrival.and_then(|a| a.ip.map(|ip| (a, ip))) {
        Some((a, ip)) => {
            let helo = if a.helo.is_empty() { "unknown" } else { a.helo.as_str() };
            Some(
                authenticator
                    // HELO checked too (RFC 7208 §2.3): its failure counts,
                    // and with a null sender it is the identity.
                    .verify_spf(
                        Parameters::new(SpfParameters::helo_and_mail_from(ip, helo, &a.by, return_path))
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
        // None verified: the worst result says most (one forged and one
        // broken signature read as a failure, not "neutral").
        dkim.iter()
            .map(|o| o.result())
            .max_by_key(|r| match r {
                DkimResult::Fail(_) => 5,
                DkimResult::PermError(_) => 4,
                DkimResult::TempError(_) => 3,
                DkimResult::Neutral(_) => 2,
                DkimResult::None => 1,
                DkimResult::Pass => 0,
            })
            .map(|r| dkim_name(r).to_owned())
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
        dmarc_policy: (dmarc.result() != DmarcResult::None).then(|| dmarc.policy().to_string()),
        arc: arc_verdict,
        partly_signed,
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
    details.dmarc_policy = v.dmarc_policy;
    details.arc = v.arc;
    details.partly_signed = v.partly_signed;
}

/// The id of a domain's MTA-STS record (`_mta-sts` TXT, RFC 8461): `None`
/// when it publishes none, an error when DNS couldn't say.
pub(crate) fn mta_sts_id(domain: &str) -> anyhow::Result<Option<String>> {
    use anyhow::Context as _;
    use mail_auth::{DnsError, Error};
    let checker = checker().with_context(|| t!("sync-auth-dns-isnt-available"))?;
    let name = format!("_mta-sts.{domain}");
    let answer = checker.runtime.block_on(async {
        tokio::time::timeout(
            TIMEOUT,
            checker
                .authenticator
                .txt_lookup::<mail_auth::mta_sts::MtaStsRecord>(
                    name.as_str(),
                    Some(&checker.cache.txt),
                ),
        )
        .await
    });
    match answer {
        Err(_) => anyhow::bail!(t!("sync-auth-dns-didnt-answer-in-time")),
        Ok(Ok(record)) => Ok(Some(record.id.clone())),
        Ok(Err(Error::Dns(DnsError::RecordNotFound(_) | DnsError::InvalidRecordType))) => Ok(None),
        Ok(Err(e)) => Err(anyhow::anyhow!("{e}")),
    }
}

/// A domain's mail servers (MX), most preferred first; none where it has
/// no MX records.
pub(crate) fn mx_hosts(domain: &str) -> anyhow::Result<Vec<String>> {
    use anyhow::Context as _;
    use mail_auth::{DnsError, Error};
    let checker = checker().with_context(|| t!("sync-auth-dns-isnt-available"))?;
    checker.runtime.block_on(async {
        tokio::time::timeout(TIMEOUT, async {
            match checker
                .authenticator
                .mx_lookup(domain, Some(&checker.cache.mx))
                .await
            {
                Ok(mx) => Ok(mx
                    .records
                    .iter()
                    .flat_map(|r| r.exchanges.iter())
                    .map(|e| e.trim_end_matches('.').to_ascii_lowercase())
                    .collect()),
                Err(Error::Dns(DnsError::RecordNotFound(_))) => Ok(Vec::new()),
                Err(e) => anyhow::bail!("{e}"),
            }
        })
        .await
        .map_err(|_| anyhow::anyhow!(t!("sync-auth-dns-didnt-answer-in-time")))?
    })
}

/// What DNSSEC says of an answer (RFC 4035 §4.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Validation {
    /// Signed, with a chain of trust from the root: the answer is the
    /// zone's own.
    Secure,
    /// From an unsigned zone (or no answer could be had about it): as good
    /// as any DNS answer, which anyone on the path can forge.
    Insecure,
    /// Signed, but the signatures don't check out: forged or broken.
    Bogus,
}
impl Validation {
    /// The validation of an RRset from its records' proofs: secure only if
    /// every record is; bogus if any is.
    fn of(proofs: impl IntoIterator<Item = mail_auth::hickory_resolver::proto::dnssec::Proof>) -> Self {
        use mail_auth::hickory_resolver::proto::dnssec::Proof;
        let mut all_secure = true;
        let mut any = false;
        for proof in proofs {
            any = true;
            match proof {
                Proof::Bogus => return Self::Bogus,
                Proof::Secure => {}
                _ => all_secure = false,
            }
        }
        if any && all_secure { Self::Secure } else { Self::Insecure }
    }
}

/// The resolver for records whose worth is their DNSSEC signature (TLSA,
/// OPENPGPKEY, SMIMEA): the system's own name servers, asked with DNSSEC
/// OK (RFC 4035 §3.2.1), each answer validated here up to the root's
/// trust anchors (hickory's built-in IANA keys), so a resolver on the path
/// (or the network) can't vouch for it. Separate from `checker`'s, which
/// mail-auth's DKIM, SPF, DMARC and ARC checks keep using unchanged. On the
/// same runtime.
fn validating() -> Option<&'static mail_auth::hickory_resolver::TokioResolver> {
    use mail_auth::hickory_resolver::{TokioResolver, net::runtime::TokioRuntimeProvider};
    static RESOLVER: OnceLock<Option<TokioResolver>> = OnceLock::new();
    RESOLVER
        .get_or_init(|| {
            let checker = checker()?;
            let _entered = checker.runtime.enter();
            let (config, mut options) = system_conf()?;
            options.timeout = Duration::from_secs(4);
            options.attempts = 2;
            options.validate = true;
            let mut builder = TokioResolver::builder_with_config(config, TokioRuntimeProvider::default());
            *builder.options_mut() = options;
            builder.build().ok()
        })
        .as_ref()
}
/// The records of `kind` at `name`, validated, through `validating`. None
/// found, in a zone signed or not, is an empty answer; one DNSSEC finds
/// bogus is `Bogus` with no records (they mustn't be used).
async fn validated_lookup(
    name: &str,
    kind: mail_auth::hickory_resolver::proto::rr::RecordType,
) -> anyhow::Result<(Vec<mail_auth::hickory_resolver::proto::rr::RData>, Validation)> {
    use mail_auth::hickory_resolver::net::{DnsError, NetError};
    let resolver = validating().ok_or_else(|| anyhow::anyhow!(t!("sync-auth-dns-isnt-available")))?;
    match resolver.lookup(name, kind).await {
        Ok(found) => {
            let records: Vec<_> = found
                .answers()
                .iter()
                .filter(|r| r.record_type() == kind)
                .collect();
            let validation = Validation::of(records.iter().map(|r| r.proof));
            if validation == Validation::Bogus {
                return Ok((Vec::new(), Validation::Bogus));
            }
            Ok((records.into_iter().map(|r| r.data.clone()).collect(), validation))
        }
        Err(NetError::Dns(DnsError::DnssecBogus)) => Ok((Vec::new(), Validation::Bogus)),
        Err(NetError::Dns(DnsError::Nsec { proof, .. })) => {
            // A proof that there are none: secure or not, as proven.
            Ok((Vec::new(), Validation::of([proof])))
        }
        Err(e) if e.is_no_records_found() || e.is_nx_domain() => Ok((Vec::new(), Validation::Insecure)),
        Err(e) => anyhow::bail!("{e}"),
    }
}

/// A TLSA record's fields (RFC 6698 §2.1), as numbers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Tlsa {
    pub(crate) usage: u8,
    pub(crate) selector: u8,
    pub(crate) matching: u8,
}
impl Tlsa {
    /// Whether an SMTP client can use it (RFC 7672 §3.1): DANE-TA(2) or
    /// DANE-EE(3), as PKIX-TA(0) and PKIX-EE(1) are not for SMTP (§3.1.3);
    /// a whole certificate or its public key (selector 0, 1); and the data
    /// itself or its SHA-256 or SHA-512 hash (matching 0, 1, 2), of the
    /// length that hash has (RFC 6698 §2.1.3, RFC 7671 §10.1.2).
    pub(crate) fn usable(self, data_len: usize) -> bool {
        matches!(self.usage, 2 | 3)
            && matches!(self.selector, 0 | 1)
            && match self.matching {
                0 => data_len > 0,
                1 => data_len == 32,
                2 => data_len == 64,
                _ => false,
            }
    }
}
fn tlsa_of(data: &mail_auth::hickory_resolver::proto::rr::RData) -> Option<(Tlsa, usize)> {
    use mail_auth::hickory_resolver::proto::rr::RData;
    let RData::TLSA(t) = data else { return None };
    Some((
        Tlsa {
            usage: t.cert_usage.into(),
            selector: t.selector.into(),
            matching: t.matching.into(),
        },
        t.cert_data.len(),
    ))
}

/// What DANE (RFC 7672) says of a mail server, from its TLSA RRset.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DaneState {
    /// Secure, with records an SMTP client can use: servers must use TLS
    /// and check the certificate against them.
    Usable,
    /// No secure, usable records (none at all, an unsigned zone, or only
    /// usages SMTP ignores): as if there were none (§2.2).
    None,
    /// The records are bogus: a sending server mustn't deliver (§2.1.1).
    Bogus,
}
/// DANE's verdict on one TLSA answer.
pub(crate) fn dane_state(records: &[(Tlsa, usize)], validation: Validation) -> DaneState {
    match validation {
        Validation::Bogus => DaneState::Bogus,
        Validation::Insecure => DaneState::None,
        Validation::Secure if records.iter().any(|(t, len)| t.usable(*len)) => DaneState::Usable,
        Validation::Secure => DaneState::None,
    }
}
/// The servers RFC 7672 §2.2.1 looks at for a domain: those at the lowest
/// preference among its MX records, each one; with no MX records, the
/// domain itself (the implicit MX, RFC 5321 §5.1). `None` where the domain
/// takes no mail (a null MX, RFC 7505).
pub(crate) fn dane_hosts(domain: &str, mx: &[(u16, Vec<String>)]) -> Option<Vec<String>> {
    let Some(lowest) = mx.iter().map(|(p, _)| *p).min() else {
        return Some(vec![domain.to_owned()]);
    };
    let hosts: Vec<String> = mx
        .iter()
        .filter(|(p, _)| *p == lowest)
        .flat_map(|(_, hosts)| hosts.iter())
        .map(|h| h.trim_end_matches('.').to_ascii_lowercase())
        .collect();
    if hosts.iter().all(|h| h.is_empty()) {
        return None;
    }
    Some(hosts.into_iter().filter(|h| !h.is_empty()).collect())
}

/// Whether a domain's mail servers are protected by DANE (RFC 7672): each
/// server at its lowest MX preference (or the domain itself, with no MX)
/// has secure TLSA records at `_25._tcp.<server>` that SMTP can use, so
/// servers sending to it must use TLS and check the certificate. Bogus
/// records are an error: mail there can't be delivered (§2.1.1).
pub(crate) fn dane(domain: &str) -> anyhow::Result<bool> {
    use anyhow::Context as _;
    use mail_auth::hickory_resolver::proto::rr::{RData, RecordType};
    let checker = checker().with_context(|| t!("sync-auth-dns-isnt-available"))?;
    checker.runtime.block_on(async {
        tokio::time::timeout(TIMEOUT, async {
            // The MX records themselves must be secure for DANE to apply
            // to the servers they name (§2.2.1).
            let (mx, validation) = validated_lookup(domain, RecordType::MX).await?;
            anyhow::ensure!(
                validation != Validation::Bogus,
                t!("sync-auth-s-mail-servers-dns-records-fail", domain = domain)
            );
            if validation == Validation::Insecure && !mx.is_empty() {
                return Ok(false);
            }
            let mut preferences: Vec<(u16, Vec<String>)> = Vec::new();
            for record in &mx {
                if let RData::MX(m) = record {
                    preferences.push((m.preference, vec![m.exchange.to_ascii()]));
                }
            }
            let Some(hosts) = dane_hosts(domain, &preferences) else {
                return Ok(false);
            };
            let mut any = false;
            for host in hosts {
                let name = format!("_25._tcp.{}.", host.trim_end_matches('.'));
                let (found, validation) = validated_lookup(&name, RecordType::TLSA).await?;
                let records: Vec<_> = found.iter().filter_map(tlsa_of).collect();
                match dane_state(&records, validation) {
                    DaneState::Usable => any = true,
                    DaneState::Bogus => anyhow::bail!(
                        t!("sync-auth-s-dane-records-fail-dnssec-validation", host = host, domain = domain)
                    ),
                    DaneState::None => {}
                }
            }
            Ok(any)
        })
        .await
        .map_err(|_| anyhow::anyhow!(t!("sync-auth-dns-didnt-answer-in-time")))?
    })
}

/// A service's SRV records (RFC 2782), such as `_jmap._tcp.<domain>`:
/// (target, port), the most preferred first. Empty when there are none or
/// the record says the service isn't offered (a target of ".").
/// The strings of a name's TXT records, each record's pieces joined by a
/// space: RFC 6763 §6 key/value pairs are separate character-strings, and
/// joined without one they would run together.
/// Keys a domain publishes in DNS, and whether DNSSEC vouched for them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct DnsKeys {
    pub(crate) keys: Vec<Vec<u8>>,
    /// The answer was signed and validated up to the root (see
    /// `validating`): the domain itself published these keys. Without it
    /// they are only as good as a key server's (RFC 7929 §7).
    pub(crate) dnssec_validated: bool,
}
/// The keys of a validated answer: none where DNSSEC found it bogus (forged
/// or broken; not offered even as unverified), else all, marked as
/// validated only when secure.
pub(crate) fn dns_keys(keys: Vec<Vec<u8>>, validation: Validation) -> DnsKeys {
    match validation {
        Validation::Bogus => DnsKeys::default(),
        v => DnsKeys {
            keys,
            dnssec_validated: v == Validation::Secure,
        },
    }
}
/// The owner name for an address's DNS-published key (RFC 7929 §3,
/// RFC 8162 §3.1): the first 28 bytes of the SHA-256 of its local part, in
/// hex, under `_<label>.<domain>`.
fn hashed_name(email: &str, label: &str) -> anyhow::Result<String> {
    use anyhow::Context as _;
    use sha2::Digest as _;
    let (local, domain) = email.trim().rsplit_once('@').with_context(|| t!("sync-auth-not-an-address"))?;
    let hash = sha2::Sha256::digest(local.as_bytes());
    let hex: String = hash[..28].iter().map(|b| format!("{b:02x}")).collect();
    Ok(format!("{hex}.{label}.{}.", domain.trim_end_matches('.')))
}
/// The OpenPGP keys a domain publishes for an address in DNS
/// (OPENPGPKEY, RFC 7929), DNSSEC-validated where the zone is signed.
pub(crate) fn openpgpkey(email: &str) -> anyhow::Result<DnsKeys> {
    use anyhow::Context as _;
    use mail_auth::hickory_resolver::proto::rr::{RData, RecordType};
    let name = hashed_name(email, "_openpgpkey")?;
    let checker = checker().with_context(|| t!("sync-auth-dns-isnt-available"))?;
    checker.runtime.block_on(async {
        tokio::time::timeout(TIMEOUT, async {
            let (found, validation) = validated_lookup(&name, RecordType::OPENPGPKEY).await?;
            let keys = found
                .iter()
                .filter_map(|r| match r {
                    RData::OPENPGPKEY(k) => Some(k.public_key.clone()),
                    _ => None,
                })
                .collect();
            Ok(dns_keys(keys, validation))
        })
        .await
        .map_err(|_| anyhow::anyhow!(t!("sync-auth-dns-didnt-answer-in-time")))?
    })
}
/// The S/MIME certificates a domain publishes for an address in DNS
/// (SMIMEA, RFC 8162), as DER: records with a full certificate (selector
/// 0, matching 0), DNSSEC-validated where the zone is signed.
#[allow(dead_code)]
pub(crate) fn smimea(email: &str) -> anyhow::Result<DnsKeys> {
    use anyhow::Context as _;
    use mail_auth::hickory_resolver::proto::rr::{RData, RecordType};
    let name = hashed_name(email, "_smimecert")?;
    let checker = checker().with_context(|| t!("sync-auth-dns-isnt-available"))?;
    checker.runtime.block_on(async {
        tokio::time::timeout(TIMEOUT, async {
            let (found, validation) = validated_lookup(&name, RecordType::SMIMEA).await?;
            let keys = found
                .iter()
                .filter_map(|r| match r {
                    RData::SMIMEA(t) if u8::from(t.selector) == 0 && u8::from(t.matching) == 0 => {
                        Some(t.cert_data.clone())
                    }
                    _ => None,
                })
                .collect();
            Ok(dns_keys(keys, validation))
        })
        .await
        .map_err(|_| anyhow::anyhow!(t!("sync-auth-dns-didnt-answer-in-time")))?
    })
}

pub(crate) fn txt(name: &str) -> anyhow::Result<Vec<String>> {
    use anyhow::Context as _;
    use mail_auth::hickory_resolver::proto::rr::{RData, RecordType};
    let checker = checker().with_context(|| t!("sync-auth-dns-isnt-available"))?;
    checker.runtime.block_on(async {
        tokio::time::timeout(TIMEOUT, async {
            let found = match checker
                .authenticator
                .resolver()
                .lookup(name, RecordType::TXT)
                .await
            {
                Ok(found) => found,
                Err(e) if e.is_no_records_found() || e.is_nx_domain() => return Ok(vec![]),
                Err(e) => anyhow::bail!("{e}"),
            };
            Ok(found
                .answers()
                .iter()
                .filter_map(|r| match &r.data {
                    RData::TXT(t) => Some(
                        t.txt_data
                            .iter()
                            .map(|p| String::from_utf8_lossy(p).into_owned())
                            .collect::<Vec<String>>()
                            .join(" "),
                    ),
                    _ => None,
                })
                .collect())
        })
        .await
        .map_err(|_| anyhow::anyhow!(t!("sync-auth-dns-didnt-answer-in-time")))?
    })
}

pub(crate) fn srv(name: &str) -> anyhow::Result<Vec<(String, u16)>> {
    use anyhow::Context as _;
    use mail_auth::hickory_resolver::proto::rr::{RData, RecordType};
    let checker = checker().with_context(|| t!("sync-auth-dns-isnt-available"))?;
    checker.runtime.block_on(async {
        tokio::time::timeout(TIMEOUT, async {
            let found = match checker
                .authenticator
                .resolver()
                .lookup(name, RecordType::SRV)
                .await
            {
                Ok(found) => found,
                Err(e) if e.is_no_records_found() || e.is_nx_domain() => return Ok(vec![]),
                Err(e) => anyhow::bail!("{e}"),
            };
            let records: Vec<(u16, u16, String, u16)> = found
                .answers()
                .iter()
                .filter_map(|r| match &r.data {
                    RData::SRV(s) => Some((
                        s.priority,
                        s.weight,
                        s.target
                            .to_ascii()
                            .trim_end_matches('.')
                            .to_ascii_lowercase(),
                        s.port,
                    )),
                    _ => None,
                })
                .collect();
            // A single record with target "." says the service isn't
            // offered at all (RFC 2782).
            if records.len() == 1 && records[0].2.is_empty() {
                return Ok(vec![]);
            }
            let records: Vec<_> = records
                .into_iter()
                .filter(|(.., target, port)| !target.is_empty() && *port > 0)
                .collect();
            Ok(srv_order(records, &mut rand::rng()))
        })
        .await
        .map_err(|_| anyhow::anyhow!(t!("sync-auth-dns-didnt-answer-in-time")))?
    })
}

/// SRV records in the order to try them (RFC 2782): by priority, and
/// within one priority by weighted random choice, so the weights share the
/// load. Each target is kept, as a fallback.
fn srv_order(
    mut records: Vec<(u16, u16, String, u16)>,
    rng: &mut impl rand::RngExt,
) -> Vec<(String, u16)> {
    records.sort_by_key(|r| r.0);
    let mut out = Vec::with_capacity(records.len());
    while !records.is_empty() {
        let priority = records[0].0;
        let end = records.iter().position(|r| r.0 != priority).unwrap_or(records.len());
        let mut group: Vec<_> = records.drain(..end).collect();
        // Zero weights first, so they have a small chance (RFC 2782).
        group.sort_by_key(|r| r.1 != 0);
        while !group.is_empty() {
            let total: u32 = group.iter().map(|r| u32::from(r.1)).sum();
            let pick = rng.random_range(0..=total);
            let mut running = 0;
            let at = group
                .iter()
                .position(|r| {
                    running += u32::from(r.1);
                    running >= pick
                })
                .unwrap_or(0);
            let (_, _, target, port) = group.remove(at);
            out.push((target, port));
        }
    }
    out
}

/// How long ago, in seconds, the mailbox's provider received a message: the
/// date its topmost `Received` header ends with, which the provider writes.
/// Not the `Date` header, which the sender writes: a forgery dated years
/// back, with a signature that can't be checked, was called unverifiable
/// rather than failed. `None` (not old) without one.
fn age(raw: &[u8]) -> Option<i64> {
    use mailparse::MailHeaderMap;
    let (headers, _) = mailparse::parse_headers(raw).ok()?;
    let received = headers.get_first_value("Received")?;
    let (_, when) = received.rsplit_once(';')?;
    let date = mailparse::dateparse(when.trim()).ok()?;
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
    // One From only (as DMARC asks, RFC 7489 §6.6.1): with two, the one
    // signed and the one shown could differ.
    if headers.get_all_headers("From").len() != 1 {
        return None;
    }
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
    use mail_auth::hickory_resolver::proto::dnssec::Proof;

    /// RFC 4035 §4.3: an RRset is secure only if each record is, bogus if
    /// any is, else insecure (indeterminate counts as insecure).
    #[test]
    fn dnssec_proofs_map_to_secure_insecure_and_bogus() {
        assert_eq!(Validation::of([Proof::Secure, Proof::Secure]), Validation::Secure);
        assert_eq!(Validation::of([Proof::Secure, Proof::Insecure]), Validation::Insecure);
        assert_eq!(Validation::of([Proof::Indeterminate]), Validation::Insecure);
        assert_eq!(Validation::of([Proof::Secure, Proof::Bogus]), Validation::Bogus);
        assert_eq!(Validation::of([]), Validation::Insecure);
        // DNS-published keys: marked validated only when secure, and none
        // at all from a bogus answer.
        let key = vec![vec![1u8, 2, 3]];
        assert_eq!(dns_keys(key.clone(), Validation::Secure), DnsKeys { keys: key.clone(), dnssec_validated: true });
        assert_eq!(dns_keys(key.clone(), Validation::Insecure), DnsKeys { keys: key.clone(), dnssec_validated: false });
        assert_eq!(dns_keys(key, Validation::Bogus), DnsKeys::default());
        // RFC 7929 §3 and RFC 8162 §3.1's owner names.
        assert_eq!(
            hashed_name("hugh@example.com", "_openpgpkey").unwrap(),
            "c93f1e400f26708f98cb19d936620da35eec8f72e57f9eec01c1afd6._openpgpkey.example.com."
        );
        assert!(hashed_name("hugh@example.com", "_smimecert").unwrap().ends_with("._smimecert.example.com."));
    }

    /// RFC 7672 §3.1 and RFC 6698 §2.1: only usages SMTP uses, known
    /// selectors and matching types of the right length; insecure answers
    /// count as none (§2.2), bogus ones refuse delivery (§2.1.1).
    #[test]
    fn tlsa_records_are_checked_for_smtp() {
        let t = |usage, selector, matching| Tlsa { usage, selector, matching };
        assert!(t(3, 1, 1).usable(32));
        assert!(t(2, 0, 2).usable(64));
        assert!(t(3, 0, 0).usable(800));
        // PKIX usages aren't SMTP's.
        assert!(!t(0, 1, 1).usable(32));
        assert!(!t(1, 1, 1).usable(32));
        // Unknown selector or matching type, or a hash of the wrong length.
        assert!(!t(3, 2, 1).usable(32));
        assert!(!t(3, 1, 3).usable(32));
        assert!(!t(3, 1, 1).usable(20));
        assert!(!t(3, 1, 2).usable(32));
        assert!(!t(3, 1, 0).usable(0));
        let good = [(t(3, 1, 1), 32)];
        let pkix = [(t(1, 1, 1), 32)];
        assert_eq!(dane_state(&good, Validation::Secure), DaneState::Usable);
        assert_eq!(dane_state(&pkix, Validation::Secure), DaneState::None);
        assert_eq!(dane_state(&good, Validation::Insecure), DaneState::None);
        assert_eq!(dane_state(&good, Validation::Bogus), DaneState::Bogus);
        assert_eq!(dane_state(&[], Validation::Bogus), DaneState::Bogus);
    }

    /// RFC 7672 §2.2.1: every server at the lowest preference; RFC 5321
    /// §5.1: with no MX, the domain itself; RFC 7505: a null MX takes none.
    #[test]
    fn dane_looks_at_peer_mxs_and_the_implicit_mx() {
        let mx = vec![
            (20, vec!["backup.example.com.".to_owned()]),
            (10, vec!["mx1.example.com.".to_owned()]),
            (10, vec!["MX2.example.com".to_owned()]),
        ];
        assert_eq!(dane_hosts("example.com", &mx), Some(vec!["mx1.example.com".into(), "mx2.example.com".into()]));
        assert_eq!(dane_hosts("example.com", &[]), Some(vec!["example.com".into()]));
        assert_eq!(dane_hosts("example.com", &[(0, vec![".".into()])]), None);
    }

    /// mail-auth's resolver (DKIM, SPF, DMARC, ARC) is configured as
    /// before: hickory's DNSSEC feature, now built, leaves validation off
    /// unless asked, as only `validating` does.
    #[test]
    fn mail_auth_resolver_does_not_validate() {
        let options = mail_auth::hickory_resolver::config::ResolverOpts::default();
        assert!(!options.validate);
        if let Ok((_, system)) = mail_auth::hickory_resolver::system_conf::read_system_conf() {
            assert!(!system.validate, "the system configuration turns validation on");
        }
    }

    /// Against the real DNS: `cargo test -- --ignored dane_live`. A
    /// signed domain with DANE, a signed one without, an unsigned one, and
    /// the deliberately broken dnssec-failed.org.
    #[test]
    #[ignore = "needs the network and a DNS resolver that passes DNSSEC records"]
    fn dane_live() {
        assert_eq!(dane("ietf.org").ok(), Some(true), "ietf.org publishes DANE");
        assert_eq!(dane("gmail.com").ok(), Some(false));
        // Bogus: refused, whether found so here or by a validating resolver
        // upstream (which answers SERVFAIL).
        assert!(dane("dnssec-failed.org").is_err(), "bogus records refuse delivery");
    }

    #[test]
    fn srv_records_go_by_priority_then_weight() {
        let mut rng = rand::rng();
        for _ in 0..50 {
            let order = srv_order(
                vec![
                    (20, 0, "c".into(), 1),
                    (10, 5, "a".into(), 1),
                    (10, 60, "b".into(), 1),
                ],
                &mut rng,
            );
            // Every target is kept; the higher priority's come first.
            assert_eq!(order.len(), 3);
            assert_eq!(order[2].0, "c");
        }
    }
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

    /// A message's age is when its provider received it, not the date its
    /// sender wrote: a forgery can't make itself old.
    #[test]
    fn age_comes_from_the_provider_not_the_sender() {
        let now = chrono::Utc::now();
        let received = (now - chrono::Duration::hours(1)).to_rfc2822();
        let backdated = format!(
            "Received: from mx.example by mx.provider.example; {received}\r\nDate: Mon, 1 Jan 2001 00:00:00 +0000\r\nFrom: a@bank.example\r\n\r\nHi\r\n"
        );
        let a = age(backdated.as_bytes()).unwrap();
        assert!((3500..3700).contains(&a), "{a}");
        // Only a Date: nothing says how old it is.
        assert_eq!(age(b"Date: Mon, 1 Jan 2001 00:00:00 +0000\r\nFrom: a@b.example\r\n\r\nHi\r\n"), None);
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

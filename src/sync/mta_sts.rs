//! Whether mail to a domain can be required to travel encrypted: its
//! MTA-STS policy (RFC 8461) or DANE records (RFC 7672). A message sent
//! with REQUIRETLS (RFC 8689) is only delivered over TLS to servers one of
//! these vouches for, and bounces otherwise, so before such a message goes
//! Picacho looks at its recipients' domains and says which can't take it.
//!
//! Picacho hands mail to its own outgoing server, which does the delivery
//! and enforces the policies; this is only for telling the person what to
//! expect. The lookups name the recipients' domains to the DNS resolver
//! and to each domain's policy host.
use anyhow::{Context, Result, bail, ensure};
use std::collections::HashMap;
use std::io::Read;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Policies larger than this are refused (RFC 8461 suggests 64 KB).
const POLICY_LIMIT: u64 = 64 * 1024;
/// Policies are kept at most this long, whatever their max_age.
const LONGEST_KEPT: Duration = Duration::from_secs(24 * 3600);

/// What a domain's policy says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    Enforce,
    Testing,
    None,
}

/// An MTA-STS policy (RFC 8461, section 3.2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Policy {
    pub(crate) mode: Mode,
    pub(crate) mx: Vec<String>,
    pub(crate) max_age: u64,
}

/// How mail to a domain can be kept encrypted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Protection {
    /// An MTA-STS policy in enforce or testing mode (REQUIRETLS treats
    /// testing as enforce).
    MtaSts,
    /// DANE records for its mail servers.
    Dane,
    /// Neither: mail requiring TLS bounces there.
    None,
    /// It couldn't be found out.
    Unknown(String),
}

/// The policy in a policy file.
pub(crate) fn parse(text: &str) -> Result<Policy> {
    let mut version = None;
    let mut mode = None;
    let mut mx = Vec::new();
    let mut max_age = None;
    for line in text.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            "version" => version = Some(value.to_owned()),
            "mode" => {
                mode = Some(match value {
                    "enforce" => Mode::Enforce,
                    "testing" => Mode::Testing,
                    "none" => Mode::None,
                    other => bail!("unknown policy mode {other:?}"),
                })
            }
            "mx" => mx.push(value.to_ascii_lowercase()),
            "max_age" => max_age = Some(value.parse::<u64>().context("max_age isn't a number")?),
            _ => {}
        }
    }
    ensure!(version.as_deref() == Some("STSv1"), "not an STSv1 policy");
    let mode = mode.context("the policy has no mode")?;
    ensure!(
        mode == Mode::None || !mx.is_empty(),
        "the policy names no mail servers"
    );
    Ok(Policy {
        mode,
        mx,
        max_age: max_age.context("the policy has no max_age")?,
    })
}

/// A policy fetched for a domain, under the record id it was fetched for.
struct Kept {
    id: String,
    policy: Policy,
    until: Instant,
}
static KEPT: Mutex<Option<HashMap<String, Kept>>> = Mutex::new(None);

/// The domain's policy file, over verified HTTPS without redirects, as RFC
/// 8461 requires.
fn fetch(domain: &str) -> Result<Policy> {
    // A host name only: a domain from an address could otherwise carry a
    // path, port or another host into the URL.
    ensure!(
        is_domain(domain),
        "{domain:?} isn't a domain name that can publish a policy"
    );
    let client = reqwest::blocking::Client::builder()
        .min_tls_version(reqwest::tls::Version::TLS_1_2)
        .timeout(Duration::from_secs(10))
        .https_only(true)
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(concat!("Picacho/", env!("CARGO_PKG_VERSION")))
        .build()?;
    let url = format!("https://mta-sts.{domain}/.well-known/mta-sts.txt");
    let response = client
        .get(&url)
        .send()
        .with_context(|| format!("couldn't fetch {url}"))?;
    ensure!(
        response.status().is_success(),
        "{url} answered {}",
        response.status()
    );
    let kind = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_ascii_lowercase();
    ensure!(kind.starts_with("text/plain"), "{url} isn't plain text");
    let mut text = String::new();
    response
        .take(POLICY_LIMIT + 1)
        .read_to_string(&mut text)
        .with_context(|| format!("couldn't read {url}"))?;
    ensure!(text.len() as u64 <= POLICY_LIMIT, "{url} is too large");
    parse(&text)
}

/// Whether `domain` is a plain DNS name (letters, digits and hyphens, in
/// dot-separated labels); internationalized names in their ASCII form.
pub(crate) fn is_domain(domain: &str) -> bool {
    domain.len() <= 253
        && domain.contains('.')
        && domain.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
}

/// The domain's MTA-STS policy, if it publishes one: fetched again when
/// its record's id changes or it is older than its max_age.
pub(crate) fn policy(domain: &str) -> Result<Option<Policy>> {
    let Some(id) = crate::sync::auth::mta_sts_id(domain)? else {
        return Ok(None);
    };
    {
        let kept = KEPT.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(k) = kept.as_ref().and_then(|m| m.get(domain))
            && k.id == id
            && k.until > Instant::now()
        {
            return Ok(Some(k.policy.clone()));
        }
    }
    let policy = fetch(domain)?;
    let age = Duration::from_secs(policy.max_age).min(LONGEST_KEPT);
    let mut kept = KEPT.lock().unwrap_or_else(|e| e.into_inner());
    let kept = kept.get_or_insert_with(HashMap::new);
    // Expired policies go, so the cache holds only the domains of recent
    // mail rather than every domain ever written to.
    let now = Instant::now();
    kept.retain(|_, k| k.until > now);
    kept.insert(
        domain.to_owned(),
        Kept {
            id,
            policy: policy.clone(),
            until: Instant::now() + age,
        },
    );
    Ok(Some(policy))
}

/// How mail to `domain` can be kept encrypted all the way.
pub(crate) fn protection(domain: &str) -> Protection {
    let domain = domain.trim().trim_end_matches('.').to_ascii_lowercase();
    // Internationalized names are looked up in their ASCII (punycode) form.
    let domain = match url::Host::parse(&domain) {
        Ok(url::Host::Domain(ascii)) if is_domain(&ascii) => ascii,
        _ => return Protection::Unknown(format!("{domain:?} isn't a domain name")),
    };
    let mut doubt = None;
    match policy(&domain) {
        Ok(Some(p)) if p.mode != Mode::None => return Protection::MtaSts,
        Ok(_) => {}
        Err(e) => doubt = Some(format!("{e:#}")),
    }
    match crate::sync::auth::dane(&domain) {
        Ok(true) => Protection::Dane,
        Ok(false) => match doubt {
            Some(why) => Protection::Unknown(why),
            None => Protection::None,
        },
        Err(e) => Protection::Unknown(doubt.unwrap_or_else(|| format!("{e:#}"))),
    }
}

/// The domains in recipient fields, lowercase, each once.
pub(crate) fn domains(fields: &[&str]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for field in fields {
        for (_, address) in crate::sync::smtp::recipients(field).unwrap_or_default() {
            if let Some((_, domain)) = address.rsplit_once('@') {
                let domain = domain.trim().to_ascii_lowercase();
                if !domain.is_empty() && !out.contains(&domain) {
                    out.push(domain);
                }
            }
        }
    }
    out
}

/// Of these domains, those where mail requiring TLS would bounce, checked
/// at once rather than one after another.
pub(crate) fn unprotected(domains: &[String]) -> Vec<String> {
    std::thread::scope(|scope| {
        let checks: Vec<_> = domains
            .iter()
            .map(|d| (d, scope.spawn(move || protection(d))))
            .collect();
        checks
            .into_iter()
            .filter_map(|(d, check)| {
                matches!(check.join(), Ok(Protection::None)).then(|| d.clone())
            })
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_domain_names_are_looked_up() {
        assert!(is_domain("example.com") && is_domain("xn--mnchen-3ya.de"));
        for bad in [
            "example.com/x",
            "a@b.com",
            "example.com:8443",
            "[127.0.0.1]",
            "-a.com",
            "com",
            "a..com",
        ] {
            assert!(!is_domain(bad), "{bad}");
            assert!(fetch(bad).is_err(), "{bad}");
        }
        assert!(matches!(protection("[127.0.0.1]"), Protection::Unknown(_)));
        assert!(matches!(
            protection("example.com#x"),
            Protection::Unknown(_)
        ));
    }

    #[test]
    fn policies_are_parsed_as_rfc_8461_writes_them() -> Result<()> {
        let p = parse(
            "version: STSv1\r\nmode: enforce\r\nmx: mail.example.com\r\nmx: *.example.net\r\nmax_age: 604800\r\n",
        )?;
        assert_eq!(p.mode, Mode::Enforce);
        assert_eq!(p.mx, ["mail.example.com", "*.example.net"]);
        assert_eq!(p.max_age, 604800);
        assert_eq!(
            parse("version: STSv1\nmode: testing\nmx: a.example\nmax_age: 86400\n")?.mode,
            Mode::Testing
        );
        assert_eq!(
            parse("version: STSv1\nmode: none\nmax_age: 86400")?.mode,
            Mode::None
        );
        assert!(parse("version: STSv2\nmode: enforce\nmx: a\nmax_age: 1").is_err());
        assert!(parse("version: STSv1\nmode: enforce\nmax_age: 1").is_err());
        assert!(parse("version: STSv1\nmode: strict\nmx: a\nmax_age: 1").is_err());
        assert!(parse("version: STSv1\nmode: enforce\nmx: a").is_err());
        Ok(())
    }

    /// Against live DNS and HTTPS: `cargo test live_protection -- --ignored`.
    #[test]
    #[ignore]
    fn live_protection() {
        assert_eq!(protection("gmail.com"), Protection::MtaSts);
        assert_eq!(protection("example.com"), Protection::None);
    }

    #[test]
    fn recipient_domains_are_listed_once() {
        assert_eq!(
            domains(&[
                "Ana <ana@Example.com>, bo@example.com",
                "",
                "cy@mail.example.org"
            ]),
            ["example.com", "mail.example.org"]
        );
    }
}

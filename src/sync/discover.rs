//! Finds server settings from an email address, following the order used by
//! Thunderbird: the provider's own autoconfig file, the public ISPDB, the
//! domain's MX records, and finally well-known hostnames.
//!
//! JMAP (RFC 8620 §2.2) comes first where the domain names a JMAP server
//! itself, with an `_jmap._tcp` SRV record whose target is in the domain; a
//! target elsewhere, or a domain that only answers at `/.well-known/jmap`,
//! is used when nothing else is found. DNS answers aren't authenticated, so
//! a spoofed SRV record could otherwise send the password to any host with
//! a certificate of its own; a target in the domain must still show the
//! domain's certificate.
use crate::manage::model::{Account, Provider};
use std::{
    io::Read,
    net::{TcpStream, ToSocketAddrs},
    time::Duration,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    pub provider: Provider,
    pub incoming_host: String,
    pub incoming_port: u16,
    pub smtp_host: String,
    pub smtp_port: u16,
    /// The outgoing server takes TLS from the first byte (true) or
    /// STARTTLS (false), where the configuration says; `None` goes by the
    /// port (`model::smtp_implicit_tls`).
    pub smtp_tls: Option<bool>,
    /// The incoming server starts plain and is upgraded with STARTTLS
    /// (IMAP on 143, POP3's STLS on 110) rather than TLS from the start.
    pub incoming_starttls: bool,
    /// The organization's single sign-on, where the domain's configuration
    /// says its servers take OAuth2 tokens (Thunderbird's `<oAuth2>`).
    pub sso: Option<crate::manage::model::Sso>,
}
impl Settings {
    /// A JMAP server, which sends mail too.
    fn jmap(host: &str, port: u16) -> Self {
        Self {
            provider: Provider::Jmap,
            incoming_host: host.to_owned(),
            incoming_port: port,
            smtp_host: host.to_owned(),
            smtp_port: port,
            smtp_tls: None,
            incoming_starttls: false,
            sso: None,
        }
    }
    fn hosted(provider: Provider) -> Self {
        let a = Account::new(provider);
        Self {
            provider,
            incoming_host: a.incoming_host,
            incoming_port: a.incoming_port,
            smtp_host: a.smtp_host,
            smtp_port: a.smtp_port,
            smtp_tls: a.smtp_tls,
            incoming_starttls: a.incoming_starttls,
            sso: None,
        }
    }
}

pub fn domain(email: &str) -> Option<String> {
    let (local, domain) = email.trim().rsplit_once('@')?;
    let domain = domain.trim_end_matches('.').to_ascii_lowercase();
    (!local.is_empty()
        && domain.contains('.')
        && domain
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.'))
    .then_some(domain)
}

pub fn discover(email: &str) -> Option<Settings> {
    let domain = domain(email)?;
    if let Some(found) = known(&domain) {
        return Some(found);
    }
    let domain = domain.as_str();
    std::thread::scope(|s| {
        let srv = s.spawn(|| jmap_srv(domain));
        let well_known = s.spawn(|| jmap_well_known(domain));
        let direct = [
            format!("https://autoconfig.{domain}/mail/config-v1.1.xml"),
            format!("https://{domain}/.well-known/autoconfig/mail/config-v1.1.xml"),
            ispdb(domain),
        ]
        .map(|url| s.spawn(move || fetch_config(&url, domain)));
        let mx = s.spawn(|| mx_host(domain));
        // The domain names its JMAP server, in the domain: that is what it
        // wants used. One elsewhere waits until the HTTPS sources are asked.
        let srv = srv.join().ok().flatten();
        if let Some(found) = srv
            .as_ref()
            .filter(|f| in_domain(&f.incoming_host, domain))
        {
            return Some(found.clone());
        }
        for job in direct {
            if let Ok(Some(found)) = job.join() {
                return Some(found);
            }
        }
        if let Ok(Some(mx)) = mx.join() {
            if let Some(found) = known_mx(&mx) {
                return Some(found);
            }
            for base in base_domains(&mx) {
                if base != domain
                    && let Some(found) = fetch_config(&ispdb(&base), domain)
                {
                    return Some(found);
                }
            }
        }
        if let Some(found) = srv {
            return Some(found);
        }
        if let Ok(Some(found)) = well_known.join() {
            return Some(found);
        }
        probe(domain)
    })
}

/// Whether `host` is `domain` or a host under it.
fn in_domain(host: &str, domain: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    host == domain || host.ends_with(&format!(".{domain}"))
}

/// The JMAP server a domain's `_jmap._tcp` SRV record names.
fn jmap_srv(domain: &str) -> Option<Settings> {
    let (host, port) = crate::sync::auth::srv(&format!("_jmap._tcp.{domain}"))
        .ok()?
        .into_iter()
        .next()?;
    Some(Settings::jmap(&host, port))
}

/// Whether the domain's web server answers as a JMAP server would, before
/// signing in: a session resource that wants credentials (401 with a Basic
/// or Bearer challenge), or a redirect to one on another host (Fastmail's
/// sends `fastmail.com` on to `api.fastmail.com`).
fn jmap_well_known(domain: &str) -> Option<Settings> {
    let client = reqwest::blocking::Client::builder()
        .min_tls_version(reqwest::tls::Version::TLS_1_2)
        .timeout(Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .ok()?;
    let response = client
        .get(format!("https://{domain}/.well-known/jmap"))
        .send()
        .ok()?;
    jmap_answer(
        domain,
        response.status().as_u16(),
        response
            .headers()
            .get(reqwest::header::WWW_AUTHENTICATE)
            .and_then(|v| v.to_str().ok()),
        response
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|v| v.to_str().ok()),
    )
}
/// The JMAP server an unauthenticated `/.well-known/jmap` answer shows.
fn jmap_answer(
    domain: &str,
    status: u16,
    challenge: Option<&str>,
    location: Option<&str>,
) -> Option<Settings> {
    match status {
        401 => {
            let challenge = challenge?.to_ascii_lowercase();
            (challenge.starts_with("basic") || challenge.starts_with("bearer"))
                .then(|| Settings::jmap(domain, 443))
        }
        301 | 302 | 307 | 308 => {
            let base = url::Url::parse(&format!("https://{domain}/.well-known/jmap")).ok()?;
            let next = base.join(location?).ok()?;
            (next.scheme() == "https" && next.path().contains("jmap")).then(|| {
                Settings::jmap(
                    next.host_str().unwrap_or(domain),
                    next.port().unwrap_or(443),
                )
            })
        }
        _ => None,
    }
}

fn ispdb(domain: &str) -> String {
    format!("https://autoconfig.thunderbird.net/v1.1/{domain}")
}

/// Whether `domain` is `name` under a top-level domain, directly
/// (`fastmail.fm`) or under a country's second level (`hotmail.co.uk`), and
/// not a name under someone else's domain (`fastmail.example.com`).
fn brand(domain: &str, name: &str) -> bool {
    let Some(rest) = domain.strip_prefix(name).and_then(|r| r.strip_prefix('.')) else {
        return false;
    };
    match rest.split_once('.') {
        None => !rest.is_empty(),
        Some((second, tld)) => {
            matches!(second, "co" | "com" | "net" | "org" | "ne" | "or")
                && !tld.is_empty()
                && !tld.contains('.')
        }
    }
}

fn known(domain: &str) -> Option<Settings> {
    let microsoft = ["hotmail", "outlook", "live", "windowslive"]
        .iter()
        .any(|p| brand(domain, p))
        || domain == "msn.com";
    let fastmail = brand(domain, "fastmail") || domain == "messagingengine.com";
    if matches!(domain, "gmail.com" | "googlemail.com") {
        Some(Settings::hosted(Provider::Gmail))
    } else if microsoft {
        Some(Settings::hosted(Provider::Outlook))
    } else if fastmail {
        Some(Settings::jmap(crate::sync::oauth::FASTMAIL_JMAP, 443))
    } else {
        None
    }
}

/// Custom domains hosted by Google Workspace or Microsoft 365.
fn known_mx(mx: &str) -> Option<Settings> {
    if mx.ends_with(".google.com") || mx.ends_with(".googlemail.com") {
        Some(Settings::hosted(Provider::Gmail))
    } else if mx.ends_with(".outlook.com") {
        Some(Settings::hosted(Provider::Outlook))
    } else if mx.ends_with(".messagingengine.com") {
        // A custom domain at Fastmail.
        Some(Settings::jmap(crate::sync::oauth::FASTMAIL_JMAP, 443))
    } else {
        None
    }
}

fn base_domains(host: &str) -> Vec<String> {
    let labels: Vec<_> = host.split('.').filter(|l| !l.is_empty()).collect();
    let mut out = Vec::new();
    for n in [2, 3] {
        if labels.len() >= n {
            out.push(labels[labels.len() - n..].join("."));
        }
    }
    out
}

fn fetch_config(url: &str, domain: &str) -> Option<Settings> {
    let client = reqwest::blocking::Client::builder()
        .min_tls_version(reqwest::tls::Version::TLS_1_2)
        .timeout(Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::limited(3))
        // A redirect to plain http would let anyone on the network choose
        // the servers (and the single sign-on) the account is set up with.
        .https_only(true)
        .build()
        .ok()?;
    let response = client.get(url).send().ok()?;
    if !response.status().is_success() {
        return None;
    }
    let mut body = String::new();
    response.take(256 * 1024).read_to_string(&mut body).ok()?;
    parse_config(&body, domain)
}

/// Parses a Mozilla autoconfig document. Only transports this client
/// supports are accepted: TLS from the start or STARTTLS, never plain.
/// Incoming servers with TLS from the start are preferred, then IMAP over
/// POP3.
pub fn parse_config(xml: &str, domain: &str) -> Option<Settings> {
    // (kind, host, port, STARTTLS)
    let mut incoming: Option<(Provider, String, u16, bool)> = None;
    // The incoming server signs in with tokens only (single sign-on).
    let mut oauth_only = false;
    for (attrs, inner) in blocks(xml, "incomingServer") {
        let kind = match attr(attrs, "type") {
            Some("imap") => Provider::Imap,
            Some("pop3") => Provider::Pop3,
            _ => continue,
        };
        let starttls = match tag(inner, "socketType") {
            Some("SSL") => false,
            Some("STARTTLS") => true,
            _ => continue,
        };
        let (Some(host), Some(port)) = (tag(inner, "hostname"), port(inner)) else {
            continue;
        };
        // Rank: TLS from the start first, then IMAP.
        let rank = |kind: Provider, starttls: bool| (!starttls, kind == Provider::Imap);
        let better = match &incoming {
            None => true,
            Some((existing, _, _, tls)) => rank(kind, starttls) > rank(*existing, *tls),
        };
        if better {
            incoming = Some((kind, substitute(host, domain), port, starttls));
            oauth_only = authentications(inner).iter().all(|a| a.eq_ignore_ascii_case("OAuth2"))
                && !authentications(inner).is_empty();
        }
    }
    let mut smtp = None;
    for (attrs, inner) in blocks(xml, "outgoingServer") {
        if attr(attrs, "type") != Some("smtp") {
            continue;
        }
        let (Some(host), Some(port)) = (tag(inner, "hostname"), port(inner)) else {
            continue;
        };
        let tls = match tag(inner, "socketType") {
            Some("STARTTLS") => false,
            Some("SSL") => true,
            _ => continue,
        };
        smtp = Some((substitute(host, domain), port, tls));
        break;
    }
    let ((kind, incoming_host, incoming_port, incoming_starttls), (smtp_host, smtp_port, smtp_tls)) =
        (incoming?, smtp?);
    let provider = if incoming_host.ends_with(".gmail.com") {
        Provider::Gmail
    } else if incoming_host.ends_with(".office365.com") || incoming_host.ends_with(".outlook.com") {
        Provider::Outlook
    } else {
        kind
    };
    // Gmail and Outlook have their own sign-in; others' OAuth2 is the
    // organization's.
    let sso = if matches!(provider, Provider::Imap | Provider::Pop3) && oauth_only {
        sso_of(xml)
    } else {
        None
    };
    Some(Settings {
        provider,
        incoming_host,
        incoming_port,
        smtp_host,
        smtp_port,
        smtp_tls: Some(smtp_tls),
        incoming_starttls,
        sso,
    })
}

/// A server's `<authentication>` methods.
fn authentications(server: &str) -> Vec<&str> {
    blocks(server, "authentication")
        .into_iter()
        .map(|(_, inner)| inner.trim())
        .collect()
}
/// The configuration's `<oAuth2>` block (Thunderbird's extension): the
/// identity provider's issuer and the scopes the mail servers want. Only an
/// https issuer is taken; its endpoints come from its own metadata, not from
/// this file, which could have come over a redirect. Thunderbird's files
/// name the issuer's host only, with `authURL` under it (Keycloak's
/// `/realms/<name>/protocol/openid-connect/auth`): the issuer is the
/// authURL's path up to its protocol part, where it has one.
fn sso_of(xml: &str) -> Option<crate::manage::model::Sso> {
    let (_, inner) = blocks(xml, "oAuth2").into_iter().next()?;
    let issuer = tag(inner, "issuer")?.trim();
    let auth = tag(inner, "authURL").and_then(|u| url::Url::parse(u.trim()).ok());
    let issuer = if issuer.contains("://") {
        issuer.to_owned()
    } else if let Some(auth) = auth.filter(|a| a.host_str() == Some(issuer)) {
        let path = auth.path();
        let base = ["/protocol/openid-connect/", "/oauth2/", "/oauth/", "/connect/", "/application/o/"]
            .iter()
            .find_map(|marker| path.find(marker).map(|i| &path[..i]))
            .unwrap_or("");
        format!("https://{issuer}{base}")
    } else {
        format!("https://{issuer}")
    };
    let parsed = url::Url::parse(&issuer).ok()?;
    (parsed.scheme() == "https" && parsed.host_str().is_some()).then(|| crate::manage::model::Sso {
        issuer: issuer.trim_end_matches('/').to_owned(),
        scope: tag(inner, "scope").unwrap_or_default().split_whitespace().collect::<Vec<_>>().join(" "),
    })
}
fn blocks<'a>(xml: &'a str, name: &str) -> Vec<(&'a str, &'a str)> {
    let open = format!("<{name}");
    let close = format!("</{name}>");
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find(&open) {
        rest = &rest[start + open.len()..];
        let Some(end_attrs) = rest.find('>') else {
            break;
        };
        let attrs = &rest[..end_attrs];
        rest = &rest[end_attrs + 1..];
        let Some(end) = rest.find(&close) else {
            break;
        };
        out.push((attrs, &rest[..end]));
        rest = &rest[end + close.len()..];
    }
    out
}
fn attr<'a>(attrs: &'a str, name: &str) -> Option<&'a str> {
    let rest = &attrs[attrs.find(&format!("{name}="))? + name.len() + 1..];
    let quote = rest.chars().next().filter(|c| *c == '"' || *c == '\'')?;
    let rest = &rest[1..];
    Some(&rest[..rest.find(quote)?])
}
fn tag<'a>(xml: &'a str, name: &str) -> Option<&'a str> {
    let start = xml.find(&format!("<{name}>"))? + name.len() + 2;
    let end = xml[start..].find(&format!("</{name}>"))?;
    Some(xml[start..start + end].trim())
}
fn port(xml: &str) -> Option<u16> {
    tag(xml, "port")?.parse().ok().filter(|p| *p > 0)
}
fn substitute(host: &str, domain: &str) -> String {
    host.replace("%EMAILDOMAIN%", domain).to_ascii_lowercase()
}

/// Uses the system resolver through `dig`, which ships with macOS, so no
/// third-party DNS service learns the address being configured.
fn mx_host(domain: &str) -> Option<String> {
    // A plain DNS name only (international ones in their ASCII form): one
    // starting with `-` would be read as one of dig's options.
    let domain = match url::Host::parse(domain).ok()? {
        url::Host::Domain(d) if super::mta_sts::is_domain(&d) => d,
        _ => return None,
    };
    let output = std::process::Command::new("/usr/bin/dig")
        .args(["+short", "+time=3", "+tries=1", "MX", &domain])
        .output()
        .ok()?;
    parse_mx(&String::from_utf8_lossy(&output.stdout))
}
fn parse_mx(output: &str) -> Option<String> {
    output
        .lines()
        .filter_map(|line| {
            let (priority, host) = line.trim().split_once(' ')?;
            Some((
                priority.parse::<u32>().ok()?,
                host.trim().trim_end_matches('.'),
            ))
        })
        .filter(|(_, host)| !host.is_empty())
        .min_by_key(|(priority, _)| *priority)
        .map(|(_, host)| host.to_ascii_lowercase())
}

fn open(host: &str, port: u16) -> bool {
    let Ok(addrs) = (host, port).to_socket_addrs() else {
        return false;
    };
    addrs
        .take(2)
        .any(|addr| TcpStream::connect_timeout(&addr, Duration::from_secs(3)).is_ok())
}

fn probe(domain: &str) -> Option<Settings> {
    let first_open = |candidates: Vec<(String, u16)>| {
        std::thread::scope(|s| {
            let jobs: Vec<_> = candidates
                .into_iter()
                .map(|(host, port)| s.spawn(move || open(&host, port).then_some((host, port))))
                .collect();
            jobs.into_iter().find_map(|j| j.join().ok().flatten())
        })
    };
    let (incoming, smtp) = std::thread::scope(|s| {
        let incoming = s.spawn(|| {
            first_open(vec![
                (format!("imap.{domain}"), 993),
                (format!("mail.{domain}"), 993),
                (format!("pop.{domain}"), 995),
                (format!("pop3.{domain}"), 995),
            ])
            .or_else(|| {
                // Only STARTTLS: IMAP on 143, POP3 on 110 (STLS).
                first_open(vec![
                    (format!("imap.{domain}"), 143),
                    (format!("mail.{domain}"), 143),
                    (format!("pop.{domain}"), 110),
                    (format!("pop3.{domain}"), 110),
                ])
            })
        });
        let smtp = s.spawn(|| {
            first_open(vec![
                (format!("smtp.{domain}"), 587),
                (format!("mail.{domain}"), 587),
                (format!("smtp.{domain}"), 465),
                (format!("mail.{domain}"), 465),
            ])
        });
        (incoming.join().ok().flatten(), smtp.join().ok().flatten())
    });
    let ((incoming_host, incoming_port), (smtp_host, smtp_port)) = (incoming?, smtp?);
    Some(Settings {
        provider: if matches!(incoming_port, 995 | 110) {
            Provider::Pop3
        } else {
            Provider::Imap
        },
        incoming_host,
        incoming_port,
        smtp_host,
        smtp_port,
        smtp_tls: None,
        incoming_starttls: matches!(incoming_port, 143 | 110),
        sso: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    const FASTMAIL: &str = r#"<?xml version="1.0"?>
<clientConfig version="1.1">
  <emailProvider id="fastmail.fm">
    <incomingServer type="pop3">
      <hostname>pop.fastmail.com</hostname><port>995</port><socketType>SSL</socketType>
    </incomingServer>
    <incomingServer type="imap">
      <hostname>imap.fastmail.com</hostname><port>143</port><socketType>STARTTLS</socketType>
    </incomingServer>
    <incomingServer type='imap'>
      <hostname> imap.%EMAILDOMAIN% </hostname>
      <port>993</port>
      <socketType>SSL</socketType>
    </incomingServer>
    <outgoingServer type="smtp">
      <hostname>smtp.fastmail.com</hostname><port>25</port><socketType>plain</socketType>
    </outgoingServer>
    <outgoingServer type="smtp">
      <hostname>smtp.fastmail.com</hostname><port>465</port><socketType>SSL</socketType>
    </outgoingServer>
  </emailProvider>
</clientConfig>"#;

    #[test]
    fn parses_autoconfig_and_prefers_tls_imap() {
        assert_eq!(
            parse_config(FASTMAIL, "fastmail.com"),
            Some(Settings {
                provider: Provider::Imap,
                incoming_host: "imap.fastmail.com".into(),
                incoming_port: 993,
                smtp_host: "smtp.fastmail.com".into(),
                smtp_port: 465,
                smtp_tls: Some(true),
                incoming_starttls: false,
                sso: None,
            })
        );
        assert_eq!(parse_config("<html>not found</html>", "x.com"), None);
        // Only STARTTLS offered: taken, and marked so.
        let only = FASTMAIL
            .replace("<port>993</port>\n      <socketType>SSL</socketType>", "<port>143</port><socketType>STARTTLS</socketType>")
            .replace("<port>995</port><socketType>SSL</socketType>", "<port>110</port><socketType>plain</socketType>");
        let found = parse_config(&only, "fastmail.com").unwrap();
        assert_eq!((found.provider, found.incoming_port, found.incoming_starttls), (Provider::Imap, 143, true));
    }

    #[test]
    fn finds_an_organizations_single_sign_on() {
        let config = r#"<clientConfig><emailProvider id="example.edu">
    <incomingServer type="imap">
      <hostname>imap.example.edu</hostname><port>993</port><socketType>SSL</socketType>
      <authentication>OAuth2</authentication>
    </incomingServer>
    <outgoingServer type="smtp">
      <hostname>smtp.example.edu</hostname><port>587</port><socketType>STARTTLS</socketType>
      <authentication>OAuth2</authentication>
    </outgoingServer>
  </emailProvider>
  <oAuth2>
    <issuer>login.example.edu</issuer>
    <scope>openid email offline_access mail</scope>
    <authURL>https://login.example.edu/realms/staff/protocol/openid-connect/auth</authURL>
    <tokenURL>https://login.example.edu/realms/staff/protocol/openid-connect/token</tokenURL>
  </oAuth2></clientConfig>"#;
        let found = parse_config(config, "example.edu").unwrap();
        assert_eq!(found.provider, Provider::Imap);
        let sso = found.sso.unwrap();
        assert_eq!(sso.issuer, "https://login.example.edu/realms/staff");
        assert_eq!(sso.scope, "openid email offline_access mail");
        // A server that also takes passwords isn't set to single sign-on.
        let both = config.replace(
            "<authentication>OAuth2</authentication>\n    </incomingServer>",
            "<authentication>OAuth2</authentication><authentication>password-cleartext</authentication></incomingServer>",
        );
        assert_ne!(both, config);
        assert!(parse_config(&both, "example.edu").unwrap().sso.is_none());
        // An issuer over plain http is ignored.
        let plain = config.replace("<issuer>login.example.edu</issuer>", "<issuer>http://login.example.edu</issuer>");
        assert!(parse_config(&plain, "example.edu").unwrap().sso.is_none());
        // Gmail's own sign-in isn't an organization's.
        let gmail = config.replace("imap.example.edu", "imap.gmail.com");
        assert!(parse_config(&gmail, "example.edu").unwrap().sso.is_none());
    }

    #[test]
    fn recognizes_hosted_providers_and_mx() {
        assert_eq!(domain(" Me@Example.COM "), Some("example.com".into()));
        assert_eq!(domain("nobody"), None);
        assert_eq!(known("gmail.com").unwrap().provider, Provider::Gmail);
        assert_eq!(known("hotmail.co.uk").unwrap().provider, Provider::Outlook);
        assert!(known("example.com").is_none());
        let mx = parse_mx("10 alt1.aspmx.l.google.com.\n1 aspmx.l.google.com.\n").unwrap();
        assert_eq!(mx, "aspmx.l.google.com");
        assert_eq!(known_mx(&mx).unwrap().provider, Provider::Gmail);
        assert_eq!(
            known_mx("contoso-com.mail.protection.outlook.com")
                .unwrap()
                .provider,
            Provider::Outlook
        );
        assert_eq!(
            base_domains("mx1.mail.example.co.uk"),
            ["co.uk", "example.co.uk"]
        );
    }

    #[test]
    fn finds_fastmail_and_jmap_servers() {
        let fastmail = Settings::jmap("api.fastmail.com", 443);
        assert_eq!(known("fastmail.com"), Some(fastmail.clone()));
        assert_eq!(known("fastmail.fm"), Some(fastmail.clone()));
        assert_eq!(
            known_mx("in1-smtp.messagingengine.com"),
            Some(fastmail.clone())
        );
        assert!(known("notfastmail.com").is_none());
        // A host under someone else's domain isn't the provider.
        assert!(known("fastmail.example.com").is_none());
        assert!(known("outlook.example.com").is_none());
        assert!(known("hotmail.example.co").is_none());
        assert_eq!(known("outlook.com").unwrap().provider, Provider::Outlook);
        assert_eq!(known("live.co.uk").unwrap().provider, Provider::Outlook);
        // An SRV target counts as the domain's own only within it.
        assert!(in_domain("jmap.example.com.", "example.com"));
        assert!(in_domain("example.com", "example.com"));
        assert!(!in_domain("example.com.evil.net", "example.com"));
        assert!(!in_domain("notexample.com", "example.com"));
        // A session that wants credentials, or a redirect to one.
        assert_eq!(
            jmap_answer("mail.example.com", 401, Some("Basic realm=\"jmap\""), None),
            Some(Settings::jmap("mail.example.com", 443))
        );
        assert_eq!(jmap_answer("example.com", 401, None, None), None);
        assert_eq!(
            jmap_answer(
                "fastmail.com",
                307,
                None,
                Some("https://api.fastmail.com/jmap/session")
            ),
            Some(fastmail)
        );
        // A web site's ordinary redirect isn't a JMAP server.
        assert_eq!(
            jmap_answer("example.com", 301, None, Some("https://www.example.com/")),
            None
        );
        assert_eq!(jmap_answer("example.com", 404, None, None), None);
    }
}

#[cfg(test)]
mod live {
    /// Contacts public autoconfig servers: `cargo test -- --ignored live`.
    #[test]
    #[ignore]
    fn discovers_real_providers() {
        for email in [
            "a@fastmail.com",
            "a@icloud.com",
            "a@yahoo.com",
            "a@gmx.de",
            "a@rust-lang.org",
            "a@zed.dev",
        ] {
            let start = std::time::Instant::now();
            println!(
                "{email}: {:?} in {:?}",
                super::discover(email),
                start.elapsed()
            );
        }
    }
}

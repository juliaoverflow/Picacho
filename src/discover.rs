//! Finds server settings from an email address, following the order used by
//! Thunderbird: the provider's own autoconfig file, the public ISPDB, the
//! domain's MX records, and finally well-known hostnames.
use crate::model::{Account, Provider};
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
}
impl Settings {
    fn hosted(provider: Provider) -> Self {
        let a = Account::new(provider);
        Self {
            provider,
            incoming_host: a.incoming_host,
            incoming_port: a.incoming_port,
            smtp_host: a.smtp_host,
            smtp_port: a.smtp_port,
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
        let direct = [
            format!("https://autoconfig.{domain}/mail/config-v1.1.xml"),
            format!("https://{domain}/.well-known/autoconfig/mail/config-v1.1.xml"),
            ispdb(domain),
        ]
        .map(|url| s.spawn(move || fetch_config(&url, domain)));
        let mx = s.spawn(|| mx_host(domain));
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
        probe(domain)
    })
}

fn ispdb(domain: &str) -> String {
    format!("https://autoconfig.thunderbird.net/v1.1/{domain}")
}

fn known(domain: &str) -> Option<Settings> {
    let microsoft = ["hotmail.", "outlook.", "live.", "windowslive."]
        .iter()
        .any(|p| domain.starts_with(p) && domain.matches('.').count() <= 2)
        || domain == "msn.com";
    if matches!(domain, "gmail.com" | "googlemail.com") {
        Some(Settings::hosted(Provider::Gmail))
    } else if microsoft {
        Some(Settings::hosted(Provider::Outlook))
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
        .timeout(Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::limited(3))
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
/// supports are accepted: implicit TLS for incoming mail, and STARTTLS or
/// implicit TLS on port 465 for SMTP.
pub fn parse_config(xml: &str, domain: &str) -> Option<Settings> {
    let mut incoming: Option<(Provider, String, u16)> = None;
    for (attrs, inner) in blocks(xml, "incomingServer") {
        let kind = match attr(attrs, "type") {
            Some("imap") => Provider::Imap,
            Some("pop3") => Provider::Pop3,
            _ => continue,
        };
        if tag(inner, "socketType") != Some("SSL") {
            continue;
        }
        let (Some(host), Some(port)) = (tag(inner, "hostname"), port(inner)) else {
            continue;
        };
        let better = match &incoming {
            None => true,
            Some((existing, ..)) => *existing == Provider::Pop3 && kind == Provider::Imap,
        };
        if better {
            incoming = Some((kind, substitute(host, domain), port));
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
        let usable = match tag(inner, "socketType") {
            Some("STARTTLS") => port != 465,
            Some("SSL") => port == 465,
            _ => false,
        };
        if usable {
            smtp = Some((substitute(host, domain), port));
            break;
        }
    }
    let ((kind, incoming_host, incoming_port), (smtp_host, smtp_port)) = (incoming?, smtp?);
    let provider = if incoming_host.ends_with(".gmail.com") {
        Provider::Gmail
    } else if incoming_host.ends_with(".office365.com") || incoming_host.ends_with(".outlook.com") {
        Provider::Outlook
    } else {
        kind
    };
    Some(Settings {
        provider,
        incoming_host,
        incoming_port,
        smtp_host,
        smtp_port,
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
    let output = std::process::Command::new("/usr/bin/dig")
        .args(["+short", "+time=3", "+tries=1", "MX", domain])
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
        provider: if incoming_port == 995 {
            Provider::Pop3
        } else {
            Provider::Imap
        },
        incoming_host,
        incoming_port,
        smtp_host,
        smtp_port,
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
            })
        );
        assert_eq!(parse_config("<html>not found</html>", "x.com"), None);
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

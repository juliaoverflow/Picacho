//! Addresses on the internet, as opposed to this computer or its network:
//! for requests to an address a sender chose (remote images, one-click
//! unsubscribing), which mustn't reach inside.

/// Whether `ip` is on the internet: not this computer, a private or
/// link-local network, carrier-grade NAT, or a reserved range. IPv6 forms
/// that carry an IPv4 address (mapped, NAT64, 6to4) are judged by it.
pub(crate) fn public_ip(ip: std::net::IpAddr) -> bool {
    use std::net::{IpAddr, Ipv4Addr};
    match ip {
        IpAddr::V4(v4) => {
            let [a, b, ..] = v4.octets();
            !(v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_documentation()
                || v4.is_multicast()
                || a == 0
                // 100.64.0.0/10, shared address space (carrier-grade NAT).
                || (a == 100 && (64..128).contains(&b))
                // 198.18.0.0/15, benchmarking; 240.0.0.0/4, reserved.
                || (a == 198 && (18..20).contains(&b))
                || a >= 240)
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return public_ip(IpAddr::V4(v4));
            }
            let s = v6.segments();
            let o = v6.octets();
            let embedded = |at: usize| Ipv4Addr::new(o[at], o[at + 1], o[at + 2], o[at + 3]);
            // 64:ff9b::/96, NAT64: the IPv4 address it reaches.
            if s[..6] == [0x64, 0xff9b, 0, 0, 0, 0] {
                return public_ip(IpAddr::V4(embedded(12)));
            }
            // 2002::/16, 6to4: the IPv4 address of its relay.
            if s[0] == 0x2002 {
                return public_ip(IpAddr::V4(embedded(2)));
            }
            !(v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                // ::/96, IPv4-compatible (deprecated): reaches IPv4 inside.
                || s[..6] == [0; 6]
                // 64:ff9b:1::/48, NAT64 for local use.
                || s[..3] == [0x64, 0xff9b, 1]
                // 2001:db8::/32, documentation.
                || s[..2] == [0x2001, 0xdb8]
                // fc00::/7, unique local; fe80::/10, link-local; fec0::/10,
                // site-local (deprecated, still routed inside some networks).
                || (s[0] & 0xfe00) == 0xfc00
                || (s[0] & 0xffc0) == 0xfe80
                || (s[0] & 0xffc0) == 0xfec0)
        }
    }
}

/// The proxies requests go through, as reqwest finds them (`HTTP_PROXY`
/// and the like, or the system's settings on macOS and Windows), and the
/// proxies' own names: a corporate proxy is on the local network, and is
/// looked up and reached there.
struct Proxies {
    matcher: hyper_util::client::proxy::matcher::Matcher,
    hosts: std::collections::HashSet<String>,
}
fn proxies() -> &'static Proxies {
    static PROXIES: std::sync::OnceLock<Proxies> = std::sync::OnceLock::new();
    PROXIES.get_or_init(|| {
        let matcher = hyper_util::client::proxy::matcher::Matcher::from_system();
        let hosts = ["http://example.com/", "https://example.com/"]
            .into_iter()
            .filter_map(|u| matcher.intercept(&http::Uri::from_static(u)))
            .filter_map(|i| i.uri().host().map(|h| h.trim_matches(['[', ']']).to_ascii_lowercase()))
            .collect();
        Proxies { matcher, hosts }
    })
}

/// The environment variable by which whoever runs Picacho says that the
/// proxies it is set to use refuse, themselves, requests to their own
/// computer and network (an egress policy enforced at the proxy). Only
/// then, or with the same said in Settings → Reading (`PROXY_PREFERENCE`),
/// do requests to addresses strangers chose go through a proxy.
pub(crate) const PROXY_EGRESS_FILTERED: &str = "PICACHO_PROXY_BLOCKS_LOCAL_NETWORK";

/// The preference by which the person says the same as
/// `PROXY_EGRESS_FILTERED` ("true"; anything else, or none: not).
pub const PROXY_PREFERENCE: &str = "proxy_blocks_local_network";

/// The setting, as last loaded or changed (`load_proxy_setting`,
/// `set_proxy_filters_egress`): every process that opens the store loads it.
static PROXY_SETTING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Whether `PROXY_EGRESS_FILTERED` is set (`1`, `true` or `yes`), read once.
fn proxy_filters_egress_by_environment() -> bool {
    static FILTERED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *FILTERED.get_or_init(|| {
        std::env::var(PROXY_EGRESS_FILTERED)
            .is_ok_and(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes"))
    })
}

/// Whether the proxies are said to refuse requests to the local network
/// themselves: by the environment (which Settings can't turn off), or by
/// the setting.
pub(crate) fn proxy_filters_egress() -> bool {
    proxy_filters_egress_by_environment() || PROXY_SETTING.load(std::sync::atomic::Ordering::Relaxed)
}

/// Whether the environment says so (Settings shows the setting as on, and
/// fixed).
pub fn proxy_filters_egress_fixed() -> bool {
    proxy_filters_egress_by_environment()
}

/// The setting as saved in `store` (see `PROXY_PREFERENCE`).
pub fn proxy_setting(store: &crate::manage::store::Store) -> bool {
    store.preference(PROXY_PREFERENCE).as_deref() == Some("true")
}

/// Loads the setting from `store`, for this process's requests.
pub fn load_proxy_setting(store: &crate::manage::store::Store) {
    set_proxy_filters_egress(proxy_setting(store));
}

/// Applies the setting to this process's requests (Settings saves it).
pub fn set_proxy_filters_egress(on: bool) {
    PROXY_SETTING.store(on, std::sync::atomic::Ordering::Relaxed);
}

/// Whether a proxy is set for the system at all (Settings offers the
/// setting only then).
pub fn proxy_configured() -> bool {
    !proxies().hosts.is_empty()
}

/// Whether a request to `name` through a proxy may reach this computer or
/// its network. The proxy looks the name up itself, with its own resolver,
/// and connects to whatever it finds: no lookup here can say where that
/// is (a name that fails here, or answers here with a public address, may
/// answer the proxy with a private one). So unless the proxy enforces the
/// restriction itself (`filtered`, see `PROXY_EGRESS_FILTERED`), any such
/// request may reach inside. Even then, a name that leads inside here is
/// refused, before the proxy is asked.
pub(crate) fn proxied_reaches_inside(name: &str, filtered: bool) -> bool {
    if !filtered {
        return true;
    }
    use std::net::ToSocketAddrs as _;
    // Every address: with a public one and a private one, the proxy may
    // well connect to the private one.
    (name, 0)
        .to_socket_addrs()
        .is_ok_and(|mut found| found.any(|a| !public_ip(a.ip())))
}

/// Whether a request to `url` may reach this computer or its network: its
/// host is such an address (`names_local_address`), or the request goes
/// through a proxy (see `proxied_reaches_inside`: unless the proxy is
/// declared to refuse such requests itself, it may). Requests that go
/// straight to their server are checked as they connect (`PublicOnly`).
pub(crate) fn reaches_inside(url: &url::Url) -> bool {
    if names_local_address(url) {
        return true;
    }
    if cfg!(test) {
        return false;
    }
    let Some(url::Host::Domain(name)) = url.host() else {
        return false;
    };
    let proxies = proxies();
    // The proxy itself, at any port: `PublicOnly` lets its name reach the
    // local network, to connect through it.
    if proxies.hosts.contains(&name.to_ascii_lowercase()) {
        return true;
    }
    let proxied = url
        .as_str()
        .parse::<http::Uri>()
        .is_ok_and(|uri| proxies.matcher.intercept(&uri).is_some());
    if !proxied {
        // Straight there: `PublicOnly` checks the name as it connects.
        return false;
    }
    proxied_reaches_inside(name, proxy_filters_egress())
}

/// `reaches_inside` for an address a service already signed in to gave
/// (Microsoft's upload address), not a stranger: a request through a
/// proxy is let through unless its name leads inside here. A best effort
/// that a proxy's own lookup can get past; never for addresses strangers
/// chose.
pub(crate) fn vouched_reaches_inside(url: &url::Url) -> bool {
    if names_local_address(url) {
        return true;
    }
    if cfg!(test) {
        return false;
    }
    let Some(url::Host::Domain(name)) = url.host() else {
        return false;
    };
    let proxies = proxies();
    if proxies.hosts.contains(&name.to_ascii_lowercase()) {
        return true;
    }
    let proxied = url
        .as_str()
        .parse::<http::Uri>()
        .is_ok_and(|uri| proxies.matcher.intercept(&uri).is_some());
    proxied && proxied_reaches_inside(name, true)
}

/// Redirects for fetches from addresses strangers chose (an email's domain,
/// a policy's host): at most `max`, each to https (any scheme in tests,
/// whose servers are plain http) and not to this computer or its network
/// (`reaches_inside`). With `PublicOnly` as the client's resolver.
pub(crate) fn public_redirects(max: usize) -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(move |attempt| {
        let url = attempt.url();
        if attempt.previous().len() >= max || (url.scheme() != "https" && !cfg!(test)) || reaches_inside(url) {
            attempt.stop()
        } else {
            attempt.follow()
        }
    })
}

/// Looks names up as the system does, keeping only addresses on the
/// internet (`public_ip`; any address in tests, whose servers are on this
/// computer): a name that leads to this computer or its network fails as if
/// it didn't exist. Checked as reqwest connects, so a name can't answer
/// with a public address when checked and a private one when used. The
/// proxies' own names are looked up as they are; what would go through a
/// proxy is refused before it's sent unless the proxy itself refuses the
/// local network (`reaches_inside`).
pub(crate) struct PublicOnly;
impl reqwest::dns::Resolve for PublicOnly {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let host = name.as_str().to_owned();
        let proxy = proxies().hosts.contains(&host.to_ascii_lowercase());
        Box::pin(async move {
            use std::net::ToSocketAddrs as _;
            let found = tokio::task::spawn_blocking(move || {
                (host.as_str(), 0).to_socket_addrs().map(Iterator::collect::<Vec<_>>)
            })
            .await??;
            let public: Vec<std::net::SocketAddr> = found
                .into_iter()
                .filter(|a| cfg!(test) || proxy || public_ip(a.ip()))
                .collect();
            if public.is_empty() {
                return Err(t!("sync-public-net-the-address-leads-to-a-local").into());
            }
            Ok(Box::new(public.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

/// Whether `url` names its host by an address that isn't on the internet
/// (`https://10.0.0.1/`, `https://[::1]/`): those skip the name lookup, so
/// `PublicOnly` never sees them.
pub(crate) fn names_local_address(url: &url::Url) -> bool {
    !cfg!(test)
        && match url.host() {
            Some(url::Host::Ipv4(ip)) => !public_ip(ip.into()),
            Some(url::Host::Ipv6(ip)) => !public_ip(ip.into()),
            Some(url::Host::Domain(_)) => false,
            None => true,
        }
}

/// A response's body as text, read up to `limit` bytes: a server could
/// otherwise answer without end, all of it held in memory. Past the limit,
/// an error. Not UTF-8 is read as `text()` does, with replacement marks.
pub(crate) fn bounded_text(response: reqwest::blocking::Response, limit: u64) -> std::io::Result<String> {
    use std::io::Read as _;
    let mut bytes = Vec::new();
    response.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(std::io::Error::other(t!("sync-public-net-the-servers-answer-is-too-large")));
    }
    Ok(match String::from_utf8(bytes) {
        Ok(text) => text,
        Err(e) => String::from_utf8_lossy(e.as_bytes()).into_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_and_reserved_addresses_are_not_public() {
        for local in [
            "127.0.0.1", "10.1.2.3", "192.168.0.1", "169.254.169.254", "100.64.0.1", "0.0.0.0",
            "::1", "::", "fc00::1", "fe80::1", "fec0::1", "::ffff:127.0.0.1", "::127.0.0.1",
            "64:ff9b::7f00:1", "64:ff9b::a00:1", "64:ff9b:1::1", "2002:7f00:1::", "2002:a00:1::",
            "2001:db8::1",
        ] {
            assert!(!public_ip(local.parse().unwrap()), "{local}");
        }
        for public in ["93.184.216.34", "2606:4700::1111", "64:ff9b::5db8:d822", "2002:5db8:d822::"] {
            assert!(public_ip(public.parse().unwrap()), "{public}");
        }
    }

    /// Through a proxy, which looks names up itself, a lookup here can't
    /// say where a request goes: unless the proxy is declared to refuse the
    /// local network itself, every name may reach inside, even one that
    /// fails to resolve here. With such a proxy, names that lead inside
    /// here are still refused.
    #[test]
    fn proxied_requests_are_refused_unless_the_proxy_filters() {
        assert!(proxied_reaches_inside("does-not-resolve.invalid", false));
        assert!(proxied_reaches_inside("localhost", false));
        assert!(proxied_reaches_inside("localhost", true));
        assert!(!proxied_reaches_inside("does-not-resolve.invalid", true));
    }
}

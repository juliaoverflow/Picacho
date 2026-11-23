//! Every subresource, including CSS imports/fonts/backgrounds, crosses this boundary.
//! No cookie jar, mailbox credentials, referrer, filesystem access or script runtime.
use crate::manage::model::InlineParts;
use blitz_traits::{
    navigation::{NavigationOptions, NavigationProvider},
    net::{Bytes, NetHandler, NetProvider, Request},
};
use std::{
    io::Read,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        mpsc,
    },
    time::Duration,
};

/// The most one resource may be.
const MAX_BYTES: u64 = 8 * 1024 * 1024;
/// The most an email may fetch: requests (a long newsletter has a hundred
/// images or more), and bytes in all.
const MAX_REQUESTS: usize = 400;
const MAX_TOTAL_BYTES: u64 = 64 * 1024 * 1024;
/// Resources fetched at once, as browsers fetch about six from a server:
/// one at a time, a newsletter's images came in one by one, each slow
/// server holding up all the rest.
const FETCHERS: usize = 6;
/// What the fetches say they are and accept. A generic name: the app, not
/// the person reading.
const USER_AGENT: &str = concat!("Mozilla/5.0 (compatible; Picacho/", env!("CARGO_PKG_VERSION"), ")");
const ACCEPT: &str = "image/webp,image/png,image/jpeg,image/gif,image/svg+xml,text/css,font/woff2,font/woff,*/*;q=0.5";
/// The most pixels one image may have once decoded, and all of an email's
/// together. Images are decoded whole (4 bytes a pixel) and kept while the
/// email is open; a small, very compressible file (a PNG of one color) can
/// stand for a gigabyte of them, so a sender, with inline images even while
/// remote content is blocked, could otherwise run the app out of memory.
const MAX_IMAGE_PIXELS: u64 = 40_000_000;
const MAX_TOTAL_PIXELS: u64 = 160_000_000;
type Fetch = (Request, Box<dyn NetHandler>);

/// Whether `bytes` may be handed over: anything not a raster image (CSS,
/// fonts, SVG), or one within the limits above, its pixels then counted
/// in `used`. Only the image's header is read.
fn admit(bytes: &[u8], used: &AtomicU64) -> bool {
    let Some((width, height)) = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()
        .filter(|r| r.format().is_some())
        .and_then(|r| r.into_dimensions().ok())
    else {
        return true;
    };
    let pixels = u64::from(width) * u64::from(height);
    if pixels > MAX_IMAGE_PIXELS {
        return false;
    }
    let before = used.fetch_add(pixels, Ordering::AcqRel);
    if before + pixels > MAX_TOTAL_PIXELS {
        used.fetch_sub(pixels, Ordering::AcqRel);
        return false;
    }
    true
}

pub struct Resources {
    blocked: bool,
    /// The message's own parts, for `cid:` addresses. Local, so shown
    /// even while remote content is blocked.
    inline: Arc<InlineParts>,
    changed: Arc<AtomicBool>,
    cancelled: Arc<AtomicBool>,
    count: AtomicUsize,
    /// Remote fetches asked for and not yet handed over. While none are,
    /// nothing changes the document but its own thread, which can then
    /// sleep until told to do something (see `worker::run`).
    pending: Arc<AtomicUsize>,
    /// The fetcher threads' queue, started by the email's first remote
    /// request. Most emails make none (remote content is blocked by
    /// default), and starting them costs threads and an HTTP client (some
    /// 6 ms, and a runtime thread of the client's own) for every email opened.
    fetcher: OnceLock<mpsc::Sender<Fetch>>,
    /// Bytes fetched so far (see `MAX_TOTAL_BYTES`).
    fetched: Arc<AtomicU64>,
    /// Hashes of the local (`data:`, `cid:`) addresses served, so that one
    /// asking for a relayout (see `local_served`) does so once: a broken one,
    /// never cached, is asked for again by every layout. With whether it
    /// was let through (see `admit`), so asking again neither counts its
    /// pixels twice nor lets through one refused.
    served: Mutex<std::collections::HashMap<u64, bool>>,
    /// Decoded image pixels handed over so far (see `admit`).
    pixels: Arc<AtomicU64>,
}
impl Resources {
    pub fn new(
        blocked: bool,
        inline: Arc<InlineParts>,
        changed: Arc<AtomicBool>,
        cancelled: Arc<AtomicBool>,
        pending: Arc<AtomicUsize>,
    ) -> Self {
        Self {
            blocked,
            inline,
            changed,
            cancelled,
            count: AtomicUsize::new(0),
            pending,
            fetcher: OnceLock::new(),
            fetched: Arc::default(),
            served: Mutex::default(),
            pixels: Arc::default(),
        }
    }
    /// A local resource's bytes as handed over: as they are, or none when
    /// `admit` refuses them. Requested during layout (a CSS background), it
    /// arrives as a message that only the next layout takes in: ask for
    /// one, the first time.
    fn local_served(&self, url: &str, bytes: &[u8]) -> Vec<u8> {
        use std::hash::{BuildHasher, BuildHasherDefault, DefaultHasher};
        let hash = BuildHasherDefault::<DefaultHasher>::default().hash_one(url);
        let mut served = self.served.lock().unwrap_or_else(|e| e.into_inner());
        let admitted = match served.get(&hash) {
            Some(&admitted) => admitted,
            None => {
                let admitted = admit(bytes, &self.pixels);
                served.insert(hash, admitted);
                self.changed.store(true, Ordering::Release);
                admitted
            }
        };
        if admitted { bytes.to_vec() } else { Vec::new() }
    }
    /// The fetchers' queue, started now if it isn't yet: `FETCHERS` threads
    /// per document sharing one HTTP client, bounded concurrency, no work on
    /// the UI thread.
    fn fetcher(&self) -> &mpsc::Sender<Fetch> {
        self.fetcher.get_or_init(|| {
            let (tx, rx) = mpsc::channel::<Fetch>();
            let rx = Arc::new(Mutex::new(rx));
            let (proxies, client) = shared_client();
            for _ in 0..FETCHERS {
                let (rx, client, proxies) = (rx.clone(), client.clone(), proxies.clone());
                let shared = Shared {
                    stopped: self.cancelled.clone(),
                    changed: self.changed.clone(),
                    fetched: self.fetched.clone(),
                    pending: self.pending.clone(),
                    pixels: self.pixels.clone(),
                };
                let _ = std::thread::Builder::new()
                    .name("picacho-fetch".into())
                    .spawn(move || fetch_all(&rx, client.as_ref(), &proxies, &shared));
            }
            tx
        })
    }
}
/// The HTTP client and proxies every email's fetches share. Each made its
/// own before: a blocking client is a runtime thread and connection pool of
/// its own, one more for each email opened with remote content allowed.
/// Each email still keeps to its own limits (`Shared`).
fn shared_client() -> (Arc<Proxies>, Option<reqwest::blocking::Client>) {
    static SHARED: std::sync::OnceLock<(Arc<Proxies>, Option<reqwest::blocking::Client>)> =
        std::sync::OnceLock::new();
    SHARED
        .get_or_init(|| {
            let proxies = Arc::new(Proxies::system());
            let client = client(&proxies);
            (proxies, client)
        })
        .clone()
}
fn client(proxies: &Arc<Proxies>) -> Option<reqwest::blocking::Client> {
    let redirects = proxies.clone();
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(reqwest::header::ACCEPT, reqwest::header::HeaderValue::from_static(ACCEPT));
    reqwest::blocking::Client::builder()
        .min_tls_version(reqwest::tls::Version::TLS_1_2)
        .referer(false)
        .user_agent(USER_AGENT)
        .default_headers(headers)
        .timeout(Duration::from_secs(8))
        // Names that lead to this computer or its network aren't fetched.
        .dns_resolver(Arc::new(PublicOnly(proxies.clone())))
        .redirect(reqwest::redirect::Policy::custom(move |attempt| {
            if attempt.previous().len() >= 5 || !redirects.admits(attempt.url()) {
                attempt.stop()
            } else {
                attempt.follow()
            }
        }))
        .build()
        .ok()
}
/// What the fetcher threads share with their document.
struct Shared {
    stopped: Arc<AtomicBool>,
    changed: Arc<AtomicBool>,
    fetched: Arc<AtomicU64>,
    pending: Arc<AtomicUsize>,
    pixels: Arc<AtomicU64>,
}
/// Fetches what the document asks for until it is dropped or cancelled.
fn fetch_all(
    rx: &Mutex<mpsc::Receiver<Fetch>>,
    client: Option<&reqwest::blocking::Client>,
    proxies: &Proxies,
    shared: &Shared,
) {
    let Shared {
        stopped,
        changed,
        fetched,
        pending,
        pixels,
    } = shared;
    loop {
        // One waits on the queue at a time; the others are fetching.
        let next = rx.lock().unwrap_or_else(|e| e.into_inner()).recv();
        let Ok((request, handler)) = next else {
            break;
        };
        if stopped.load(Ordering::Relaxed) {
            break;
        }
        let result = client.filter(|_| proxies.admits(&request.url)).and_then(|client| {
            // This fetch's share of the email's budget, taken before it
            // starts so fetches at once can't spend the same bytes; what it
            // doesn't use is given back.
            let limit = reserve(fetched)?;
            let mut used = 0;
            let result = (|| {
                let mut response = client
                    .get(request.url.clone())
                    .send()
                    .ok()?
                    .error_for_status()
                    .ok()?;
                if response.content_length().is_some_and(|n| n > limit) {
                    return None;
                }
                let resolved = response.url().to_string();
                let mut bytes = Vec::new();
                response
                    .by_ref()
                    .take(limit + 1)
                    .read_to_end(&mut bytes)
                    .ok()?;
                used = (bytes.len() as u64).min(limit);
                (bytes.len() as u64 <= limit && admit(&bytes, pixels)).then_some((resolved, bytes))
            })();
            fetched.fetch_sub(limit - used, Ordering::AcqRel);
            result
        });
        if stopped.load(Ordering::Relaxed) {
            break;
        }
        let (url, bytes) = result.unwrap_or_else(|| (request.url.to_string(), Vec::new()));
        handler.bytes(url, Bytes::from(bytes));
        changed.store(true, Ordering::Release);
        // After `changed`: a document thread that sees none pending also
        // sees the change.
        pending.fetch_sub(1, Ordering::AcqRel);
    }
}
/// Takes up to `MAX_BYTES` of what is left of `MAX_TOTAL_BYTES`; `None`
/// when nothing is.
fn reserve(fetched: &AtomicU64) -> Option<u64> {
    let mut limit = 0;
    fetched
        .try_update(Ordering::AcqRel, Ordering::Acquire, |spent| {
            limit = MAX_BYTES.min(MAX_TOTAL_BYTES.saturating_sub(spent));
            (limit > 0).then_some(spent + limit)
        })
        .ok()?;
    Some(limit)
}
pub fn remote_url(url: &url::Url) -> bool {
    matches!(url.scheme(), "http" | "https")
        && url.username().is_empty()
        && url.password().is_none()
}

/// Whether an email may load `url`: a remote address that isn't an IP
/// address of this computer or its local network. An email's images are
/// requests the reader never chose; pointed at a router's or a local
/// service's address, they could reach it (and change its settings by a
/// GET) from inside the network. Names are checked as they are looked up
/// (`PublicOnly`).
fn fetchable(url: &url::Url) -> bool {
    remote_url(url)
        && match url.host() {
            Some(url::Host::Ipv4(ip)) => allowed_ip(ip.into()),
            Some(url::Host::Ipv6(ip)) => allowed_ip(ip.into()),
            Some(url::Host::Domain(_)) => true,
            None => false,
        }
}

/// See `public_ip`; tests fetch from servers of their own on this computer.
fn allowed_ip(ip: std::net::IpAddr) -> bool {
    cfg!(test) || public_ip(ip)
}

use crate::sync::public_net::public_ip;

/// The proxies the fetches go through, as reqwest finds them (`HTTP_PROXY`
/// and the like, or the system's settings on macOS and Windows).
struct Proxies {
    matcher: hyper_util::client::proxy::matcher::Matcher,
    /// The proxies' own names: a corporate proxy is on the local network,
    /// and is looked up there.
    hosts: std::collections::HashSet<String>,
}
impl Proxies {
    fn system() -> Self {
        Self::with(hyper_util::client::proxy::matcher::Matcher::from_system())
    }
    fn with(matcher: hyper_util::client::proxy::matcher::Matcher) -> Self {
        let hosts = ["http://example.com/", "https://example.com/"]
            .into_iter()
            .filter_map(|u| matcher.intercept(&http::Uri::from_static(u)))
            .filter_map(|i| i.uri().host().map(|h| h.trim_matches(['[', ']']).to_ascii_lowercase()))
            .collect();
        Self { matcher, hosts }
    }
    /// Whether `url` goes through a proxy rather than straight to its server.
    fn routes(&self, url: &url::Url) -> bool {
        url.as_str()
            .parse::<http::Uri>()
            .is_ok_and(|uri| self.matcher.intercept(&uri).is_some())
    }
    /// Whether an email may load `url` (see `fetchable`). Through a proxy,
    /// which looks its name up itself, the name is also looked up here
    /// first: one that leads only to this computer or its network is
    /// refused, so the proxy isn't asked to reach inside. One that can't be
    /// looked up here (only the proxy can) goes on.
    fn admits(&self, url: &url::Url) -> bool {
        if !fetchable(url) {
            return false;
        }
        let Some(url::Host::Domain(name)) = url.host() else {
            return true;
        };
        // The proxy itself, at any port: `PublicOnly` lets its name reach
        // the local network (to connect through it), and a proxy on this
        // computer or the network is usually left out of the proxying.
        if self.hosts.contains(&name.to_ascii_lowercase()) {
            return false;
        }
        if !self.routes(url) {
            // Straight there: `PublicOnly` checks the name as it connects.
            return true;
        }
        use std::net::ToSocketAddrs as _;
        match (name, 0).to_socket_addrs() {
            Ok(found) => {
                let found: Vec<_> = found.collect();
                // Every address: with a public one and a private one, the
                // proxy may well connect to the private one.
                found.is_empty() || found.iter().all(|a| allowed_ip(a.ip()))
            }
            Err(_) => true,
        }
    }
}

/// Looks names up as the system does, keeping only addresses on the
/// internet (`allowed_ip`): a name of the sender's that leads to this
/// computer or its network fails as if it didn't exist. The proxies' own
/// names (`Proxies::hosts`) are looked up as they are.
struct PublicOnly(Arc<Proxies>);
impl reqwest::dns::Resolve for PublicOnly {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let host = name.as_str().to_owned();
        let proxy = self.0.hosts.contains(&host.to_ascii_lowercase());
        Box::pin(async move {
            use std::net::ToSocketAddrs as _;
            let found = tokio::task::spawn_blocking(move || {
                (host.as_str(), 0).to_socket_addrs().map(Iterator::collect::<Vec<_>>)
            })
            .await??;
            let public: Vec<std::net::SocketAddr> =
                found.into_iter().filter(|a| proxy || allowed_ip(a.ip())).collect();
            if public.is_empty() {
                return Err("The address leads to a local network.".into());
            }
            Ok(Box::new(public.into_iter()) as reqwest::dns::Addrs)
        })
    }
}
impl NetProvider for Resources {
    fn fetch(&self, _: usize, request: Request, handler: Box<dyn NetHandler>) {
        if self.cancelled.load(Ordering::Relaxed) {
            return;
        }
        let url = request.url.to_string();
        if url.starts_with("data:") && url.len() as u64 <= MAX_BYTES {
            let bytes = data_url::DataUrl::process(&url)
                .ok()
                .and_then(|data| data.decode_to_vec().ok())
                .map(|(bytes, _)| bytes)
                .unwrap_or_default();
            let bytes = self.local_served(&url, &bytes);
            handler.bytes(url, Bytes::from(bytes));
        } else if let Some(part) = self.inline.at_location(&url) {
            // A part sent with the message at that address (MHTML's
            // Content-Location): its own copy, nothing fetched.
            let bytes = self.local_served(&url, &part);
            handler.bytes(url, Bytes::from(bytes));
        } else if request.url.scheme() == "cid" {
            let part = self.inline.get(&url);
            let bytes = self.local_served(&url, part.as_deref().map_or(&[][..], |b| &b[..]));
            handler.bytes(url, Bytes::from(bytes));
        } else if !self.blocked
            && fetchable(&request.url)
            && request.method == blitz_traits::net::Method::GET
            && self.count.fetch_add(1, Ordering::Relaxed) < MAX_REQUESTS
        {
            self.pending.fetch_add(1, Ordering::AcqRel);
            if self.fetcher().send((request, handler)).is_err() {
                self.pending.fetch_sub(1, Ordering::AcqRel);
            }
        } else {
            // Complete refused resources, so Blitz never waits forever on a stylesheet.
            handler.bytes(url, Bytes::new());
        }
    }
}

/// Only a deliberate link activation can leave the reader. Forms cannot submit.
/// Links clicked in the email. Nothing is opened here: the addresses are
/// kept for the view (`Output::Open`), whose owner decides (it may ask
/// first, or open it in the system's browser).
#[derive(Default)]
pub struct Links {
    pub clicked: Arc<std::sync::Mutex<Vec<String>>>,
}
impl NavigationProvider for Links {
    fn navigate_to(&self, options: NavigationOptions) {
        if options.method == blitz_traits::net::Method::GET
            && (remote_url(&options.url) || options.url.scheme() == "mailto")
        {
            self.clicked
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(options.url.into());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use blitz_traits::net::{NetHandler, Request};
    use std::prelude::v1::test;

    /// Fetches at once take their shares of an email's byte budget before
    /// they start, so together they can't spend more than it.
    #[test]
    fn fetches_reserve_the_byte_budget() {
        let fetched = AtomicU64::new(0);
        let mut reserved = 0;
        while let Some(limit) = reserve(&fetched) {
            assert!(limit <= MAX_BYTES);
            reserved += limit;
        }
        assert_eq!(reserved, MAX_TOTAL_BYTES);
        // What a fetch doesn't use is given back.
        fetched.fetch_sub(1000, Ordering::AcqRel);
        assert_eq!(reserve(&fetched), Some(1000));
    }

    struct Ignore;
    impl NetHandler for Ignore {
        fn bytes(self: Box<Self>, _: String, _: Bytes) {}
    }

    /// An image that decodes to too many pixels isn't handed over, nor are
    /// more pixels than an email may have in all; anything else is.
    #[test]
    fn image_pixels_are_limited() {
        let png = |w: u32, h: u32| {
            let mut out = Vec::new();
            image::RgbaImage::new(w, h)
                .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
                .unwrap();
            out
        };
        let used = AtomicU64::new(0);
        assert!(admit(b"body { color: red }", &used));
        assert!(admit(&png(600, 400), &used));
        assert_eq!(used.load(Ordering::Relaxed), 240_000);
        // 50 million pixels: 200 MB decoded, from a file of some kilobytes.
        assert!(!admit(&png(10_000, 5_000), &used));
        used.store(MAX_TOTAL_PIXELS - 100, Ordering::Relaxed);
        assert!(!admit(&png(20, 20), &used));
        assert_eq!(used.load(Ordering::Relaxed), MAX_TOTAL_PIXELS - 100);
    }

    /// Names are looked up through `PublicOnly`, from the client's runtime.
    #[test]
    fn names_resolve_through_the_filter() {
        let server = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = server.local_addr().unwrap().port();
        std::thread::spawn(move || {
            use std::io::Write as _;
            if let Ok((mut s, _)) = server.accept() {
                let mut buf = [0u8; 1024];
                let _ = std::io::Read::read(&mut s, &mut buf);
                let _ = s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok");
            }
        });
        let proxies = Arc::new(Proxies::with(
            hyper_util::client::proxy::matcher::Matcher::builder().build(),
        ));
        let response =
            client(&proxies).unwrap().get(format!("http://localhost:{port}/")).send().unwrap();
        assert_eq!(response.text().unwrap(), "ok");
    }

    /// A corporate proxy on the local network is used; names it is asked
    /// for that lead inside the network are not.
    #[test]
    fn proxies_are_known_and_kept_from_the_inside() {
        use hyper_util::client::proxy::matcher::Matcher;
        let proxies = Proxies::with(
            Matcher::builder().all("http://Proxy.Corp.Example:3128").no("direct.example").build(),
        );
        assert!(proxies.hosts.contains("proxy.corp.example"));
        let url = |u: &str| url::Url::parse(u).unwrap();
        assert!(proxies.routes(&url("https://images.example/a.png")));
        assert!(!proxies.routes(&url("https://direct.example/a.png")));
        // The proxy itself isn't a place an email may load from, at any port.
        assert!(!proxies.admits(&url("http://proxy.corp.example:8080/admin")));
        assert!(!proxies.admits(&url("https://PROXY.corp.example/")));
        // `localhost` is this computer however it is reached.
        assert!(!public_ip("127.0.0.1".parse().unwrap()));
        assert!(Proxies::with(Matcher::builder().build()).hosts.is_empty());
    }

    #[test]
    fn local_network_addresses_are_not_public() {
        for ip in [
            "127.0.0.1", "10.1.2.3", "172.16.0.1", "192.168.1.1", "169.254.169.254", "0.0.0.0",
            "100.64.0.1", "255.255.255.255", "::1", "fd00::1", "fe80::1", "::ffff:192.168.1.1",
        ] {
            assert!(!public_ip(ip.parse().unwrap()), "{ip}");
        }
        for ip in ["93.184.215.14", "1.1.1.1", "2606:4700::1111", "::ffff:8.8.8.8"] {
            assert!(public_ip(ip.parse().unwrap()), "{ip}");
        }
    }

    /// An email whose remote content is blocked never starts the fetcher
    /// (its thread and HTTP client); one that isn't, on its first request.
    #[test]
    fn the_fetcher_starts_only_for_remote_content() {
        let make = |blocked| Resources::new(blocked, Default::default(), Arc::default(), Arc::default(), Arc::default());
        let get = |url: &str| Request::get(url::Url::parse(url).unwrap());
        let blocked = make(true);
        for url in ["https://example.com/a.png", "data:image/png;base64,AAAA", "cid:part@x"] {
            blocked.fetch(0, get(url), Box::new(Ignore));
        }
        assert!(blocked.fetcher.get().is_none());
        let open = make(false);
        open.fetch(0, get("cid:part@x"), Box::new(Ignore));
        assert!(open.fetcher.get().is_none(), "local parts need no fetcher");
        // Cancelled first, so nothing reaches the network.
        open.cancelled.store(true, Ordering::Relaxed);
        open.fetch(0, get("https://example.com/a.png"), Box::new(Ignore));
        assert!(open.fetcher.get().is_none(), "nothing is fetched once cancelled");
        // A server of the test's own, so nothing leaves the machine.
        let server = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/a.png", server.local_addr().unwrap());
        let open = make(false);
        open.fetch(0, get(&url), Box::new(Ignore));
        assert!(open.fetcher.get().is_some());
        // The request reached it; then it stops.
        let _ = server.accept().unwrap();
        open.cancelled.store(true, Ordering::Relaxed);
    }
}

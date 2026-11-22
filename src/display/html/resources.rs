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
type Fetch = (Request, Box<dyn NetHandler>);

pub struct Resources {
    blocked: bool,
    /// The message's own parts, for `cid:` addresses. Local, so shown
    /// even while remote content is blocked.
    inline: Arc<InlineParts>,
    changed: Arc<AtomicBool>,
    cancelled: Arc<AtomicBool>,
    count: AtomicUsize,
    /// The fetcher threads' queue, started by the email's first remote
    /// request. Most emails make none (remote content is blocked by
    /// default), and starting them costs threads and an HTTP client (some
    /// 6 ms, and a runtime thread of the client's own) for every email opened.
    fetcher: OnceLock<mpsc::Sender<Fetch>>,
    /// Bytes fetched so far (see `MAX_TOTAL_BYTES`).
    fetched: Arc<AtomicU64>,
    /// Hashes of the local (`data:`, `cid:`) addresses served, so that one
    /// asking for a relayout (see `local_served`) does so once: a broken one,
    /// never cached, is asked for again by every layout.
    served: Mutex<std::collections::HashSet<u64>>,
}
impl Resources {
    pub fn new(
        blocked: bool,
        inline: Arc<InlineParts>,
        changed: Arc<AtomicBool>,
        cancelled: Arc<AtomicBool>,
    ) -> Self {
        Self {
            blocked,
            inline,
            changed,
            cancelled,
            count: AtomicUsize::new(0),
            fetcher: OnceLock::new(),
            fetched: Arc::default(),
            served: Mutex::default(),
        }
    }
    /// Notes that a local resource was handed over. Requested during layout
    /// (a CSS background), it arrives as a message that only the next layout
    /// takes in: ask for one, the first time.
    fn local_served(&self, url: &str) {
        use std::hash::{BuildHasher, BuildHasherDefault, DefaultHasher};
        let hash = BuildHasherDefault::<DefaultHasher>::default().hash_one(url);
        if self.served.lock().unwrap_or_else(|e| e.into_inner()).insert(hash) {
            self.changed.store(true, Ordering::Release);
        }
    }
    /// The fetchers' queue, started now if it isn't yet: `FETCHERS` threads
    /// per document sharing one HTTP client, bounded concurrency, no work on
    /// the UI thread.
    fn fetcher(&self) -> &mpsc::Sender<Fetch> {
        self.fetcher.get_or_init(|| {
            let (tx, rx) = mpsc::channel::<Fetch>();
            let rx = Arc::new(Mutex::new(rx));
            let client = client();
            for _ in 0..FETCHERS {
                let (rx, client) = (rx.clone(), client.clone());
                let (stopped, changed, fetched) =
                    (self.cancelled.clone(), self.changed.clone(), self.fetched.clone());
                let _ = std::thread::Builder::new()
                    .name("picacho-fetch".into())
                    .spawn(move || fetch_all(&rx, client.as_ref(), &stopped, &changed, &fetched));
            }
            tx
        })
    }
}
fn client() -> Option<reqwest::blocking::Client> {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(reqwest::header::ACCEPT, reqwest::header::HeaderValue::from_static(ACCEPT));
    reqwest::blocking::Client::builder()
        .referer(false)
        .user_agent(USER_AGENT)
        .default_headers(headers)
        .timeout(Duration::from_secs(8))
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() >= 5 || !remote_url(attempt.url()) {
                attempt.stop()
            } else {
                attempt.follow()
            }
        }))
        .build()
        .ok()
}
/// Fetches what the document asks for until it is dropped or cancelled.
fn fetch_all(
    rx: &Mutex<mpsc::Receiver<Fetch>>,
    client: Option<&reqwest::blocking::Client>,
    stopped: &AtomicBool,
    changed: &AtomicBool,
    fetched: &AtomicU64,
) {
    loop {
        // One waits on the queue at a time; the others are fetching.
        let next = rx.lock().unwrap_or_else(|e| e.into_inner()).recv();
        let Ok((request, handler)) = next else {
            break;
        };
        if stopped.load(Ordering::Relaxed) {
            break;
        }
        let result = client.and_then(|client| {
            let left = MAX_TOTAL_BYTES.saturating_sub(fetched.load(Ordering::Relaxed));
            let limit = MAX_BYTES.min(left);
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
            fetched.fetch_add(bytes.len() as u64, Ordering::Relaxed);
            (bytes.len() as u64 <= limit).then_some((resolved, bytes))
        });
        if stopped.load(Ordering::Relaxed) {
            break;
        }
        let (url, bytes) = result.unwrap_or_else(|| (request.url.to_string(), Vec::new()));
        handler.bytes(url, Bytes::from(bytes));
        changed.store(true, Ordering::Release);
    }
}
pub fn remote_url(url: &url::Url) -> bool {
    matches!(url.scheme(), "http" | "https")
        && url.username().is_empty()
        && url.password().is_none()
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
            self.local_served(&url);
            handler.bytes(url, Bytes::from(bytes));
        } else if request.url.scheme() == "cid" {
            let bytes = self.inline.get(&url).map(|b| b.to_vec()).unwrap_or_default();
            self.local_served(&url);
            handler.bytes(url, Bytes::from(bytes));
        } else if !self.blocked
            && remote_url(&request.url)
            && request.method == blitz_traits::net::Method::GET
            && self.count.fetch_add(1, Ordering::Relaxed) < MAX_REQUESTS
        {
            let _ = self.fetcher().send((request, handler));
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

    struct Ignore;
    impl NetHandler for Ignore {
        fn bytes(self: Box<Self>, _: String, _: Bytes) {}
    }

    /// An email whose remote content is blocked never starts the fetcher
    /// (its thread and HTTP client); one that isn't, on its first request.
    #[test]
    fn the_fetcher_starts_only_for_remote_content() {
        let make = |blocked| Resources::new(blocked, Default::default(), Arc::default(), Arc::default());
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

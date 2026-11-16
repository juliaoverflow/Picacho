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
        Arc, OnceLock,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    },
    time::Duration,
};

const MAX_BYTES: u64 = 8 * 1024 * 1024;
const MAX_REQUESTS: usize = 64;
type Fetch = (Request, Box<dyn NetHandler>);

pub struct Resources {
    blocked: bool,
    /// The message's own parts, for `cid:` addresses. Local, so shown
    /// even while remote content is blocked.
    inline: Arc<InlineParts>,
    changed: Arc<AtomicBool>,
    cancelled: Arc<AtomicBool>,
    count: AtomicUsize,
    /// The fetcher thread, started by the email's first remote request.
    /// Most emails make none (remote content is blocked by default), and
    /// starting it costs a thread and an HTTP client (some 6 ms, and a
    /// runtime thread of the client's own) for every email opened.
    fetcher: OnceLock<mpsc::Sender<Fetch>>,
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
        }
    }
    /// The fetcher, started now if it isn't yet. One per document: bounded
    /// concurrency, no work on the UI thread.
    fn fetcher(&self) -> &mpsc::Sender<Fetch> {
        self.fetcher.get_or_init(|| {
            let (tx, rx) = mpsc::channel::<Fetch>();
            let (stopped, changed) = (self.cancelled.clone(), self.changed.clone());
            std::thread::spawn(move || fetch_all(rx, &stopped, &changed));
            tx
        })
    }
}
/// Fetches what the document asks for until it is dropped or cancelled.
fn fetch_all(rx: mpsc::Receiver<Fetch>, stopped: &AtomicBool, changed: &AtomicBool) {
    let client = reqwest::blocking::Client::builder()
        .referer(false)
        .timeout(Duration::from_secs(8))
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() >= 5 || !remote_url(attempt.url()) {
                attempt.stop()
            } else {
                attempt.follow()
            }
        }))
        .build();
    while let Ok((request, handler)) = rx.recv() {
        if stopped.load(Ordering::Relaxed) {
            break;
        }
        let result = client.as_ref().ok().and_then(|client| {
            let mut response = client
                .get(request.url.clone())
                .send()
                .ok()?
                .error_for_status()
                .ok()?;
            if response.content_length().is_some_and(|n| n > MAX_BYTES) {
                return None;
            }
            let resolved = response.url().to_string();
            let mut bytes = Vec::new();
            response
                .by_ref()
                .take(MAX_BYTES + 1)
                .read_to_end(&mut bytes)
                .ok()?;
            (bytes.len() as u64 <= MAX_BYTES).then_some((resolved, bytes))
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
            handler.bytes(url, Bytes::from(bytes));
        } else if request.url.scheme() == "cid" {
            let bytes = self.inline.get(&url).map(|b| b.to_vec()).unwrap_or_default();
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
pub struct Links;
impl NavigationProvider for Links {
    fn navigate_to(&self, options: NavigationOptions) {
        if options.method == blitz_traits::net::Method::GET
            && (remote_url(&options.url) || options.url.scheme() == "mailto")
        {
            let _ = open::that_detached(options.url.as_str());
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

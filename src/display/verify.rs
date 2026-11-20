//! Proton's human verification (its CAPTCHA, or a code by email or text)
//! in the system's own webview: WKWebView on macOS, WebView2 on Windows,
//! WebKitGTK on Linux.
//!
//! It runs as a second Picacho process (`picacho --proton-verify <url>`),
//! started by `sync::proton` when Proton asks for a check during sign-in.
//! GPUI owns the app's event loop, and macOS allows one per process, so the
//! webview gets a process and a window of its own. When Proton's page says
//! the check passed, the result is printed to standard output as one JSON
//! line (`{"token":…,"type":…}`) and the process ends; closing the window
//! ends it without one.
use std::io::Write;
use tao::{
    dpi::LogicalSize,
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoop},
    window::WindowBuilder,
};
use wry::{NewWindowResponse, WebViewBuilder};

use crate::sync::proton::verification_result as result;

/// Hands the page's messages to Picacho. Proton's verification page reports
/// its result with `postMessage` (to its parent when framed, otherwise to
/// itself) and, in Proton's mobile apps, through a native bridge; every one
/// of those reaches `window.ipc`, and Rust keeps only the result. Posted
/// messages count only from Proton's own pages: the CAPTCHA frames it embeds
/// (hCaptcha, Google's) can post to it too.
const BRIDGE: &str = r#"(() => {
  const send = (m) => {
    try { window.ipc.postMessage(typeof m === "string" ? m : JSON.stringify(m)); } catch (_) {}
  };
  const proton = (origin) => {
    try {
      const u = new URL(origin);
      return u.protocol === "https:" && (u.hostname === "proton.me" || u.hostname.endsWith(".proton.me"));
    } catch (_) { return false; }
  };
  window.addEventListener("message", (e) => { if (proton(e.origin)) send(e.data); });
  window.AndroidInterface = { dispatch: send };
  try {
    const handlers = window.webkit && window.webkit.messageHandlers;
    if (handlers && !handlers.iOS) {
      Object.defineProperty(handlers, "iOS", { value: { postMessage: send } });
    }
  } catch (_) {}
})();"#;

/// The page may only go to Proton and the CAPTCHA services it embeds.
fn allowed(url: &str) -> bool {
    if url == "about:blank" {
        return true;
    }
    let Ok(url) = url::Url::parse(url) else {
        return false;
    };
    if url.scheme() != "https" {
        return false;
    }
    let host = url.host_str().unwrap_or_default();
    let under = |domain: &str| host == domain || host.ends_with(&format!(".{domain}"));
    [
        "proton.me",
        "protonmail.com",
        "protonmail.ch",
        "hcaptcha.com",
        "recaptcha.net",
    ]
    .into_iter()
    .any(under)
        || (host == "www.google.com" && url.path().starts_with("/recaptcha/"))
        || host == "www.gstatic.com"
}

fn finish(found: Option<(String, String)>) -> ! {
    let code = match found {
        Some((token, kind)) => {
            let line = serde_json::json!({ "token": token, "type": kind });
            let mut out = std::io::stdout().lock();
            let _ = writeln!(out, "{line}");
            let _ = out.flush();
            0
        }
        None => 2,
    };
    std::process::exit(code)
}

/// Shows Proton's verification page at `url` until it passes or the
/// window is closed. Never returns.
pub fn run(url: &str) -> ! {
    if !url.starts_with("https://verify.proton.me/") {
        eprintln!("picacho: refusing to open {url}: not Proton's verification page");
        std::process::exit(64);
    }
    if let Err(e) = show(url) {
        eprintln!("picacho: couldn't open the verification window: {e}");
        std::process::exit(70);
    }
    finish(None)
}

fn show(url: &str) -> anyhow::Result<()> {
    let event_loop = EventLoop::new();
    let window = WindowBuilder::new()
        .with_title("Proton: verify that it's you")
        .with_inner_size(LogicalSize::new(480.0, 720.0))
        .with_min_inner_size(LogicalSize::new(360.0, 480.0))
        .with_focused(true)
        .build(&event_loop)?;
    let builder = WebViewBuilder::new()
        .with_url(url)
        // Nothing from this page stays: no cookies or storage on disk.
        .with_incognito(true)
        .with_initialization_script(BRIDGE)
        .with_navigation_handler(|url| allowed(&url))
        .with_new_window_req_handler(|url, _| {
            // Links (Proton's terms, help) open in the browser instead;
            // only Proton's own, so the page can't open other sites.
            if url::Url::parse(&url).is_ok_and(|u| {
                u.scheme() == "https"
                    && u.host_str().is_some_and(|h| {
                        ["proton.me", "protonmail.com"]
                            .iter()
                            .any(|d| h == *d || h.ends_with(&format!(".{d}")))
                    })
            }) {
                let _ = open::that_detached(&url);
            }
            NewWindowResponse::Deny
        })
        .with_ipc_handler(|request: wry::http::Request<String>| {
            // Only Proton's own page may report a result.
            let from_proton = request
                .uri()
                .host()
                .is_some_and(|h| h == "proton.me" || h.ends_with(".proton.me"));
            if !from_proton {
                return;
            }
            match result(request.body()) {
                Ok(Some(found)) => finish(Some(found)),
                Ok(None) => {}
                Err(()) => finish(None),
            }
        });
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    let _webview = builder.build(&window)?;
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let _webview = {
        use tao::platform::unix::WindowExtUnix;
        use wry::WebViewBuilderExtUnix;
        let vbox = window
            .default_vbox()
            .ok_or_else(|| anyhow::anyhow!("the window has no GTK container"))?;
        builder.build_gtk(vbox)?
    };
    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        if let Event::WindowEvent {
            event: WindowEvent::CloseRequested,
            ..
        } = event
        {
            finish(None);
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_result_messages() {
        let ok = r#"{"type":"HUMAN_VERIFICATION_SUCCESS","payload":{"token":"abc:def","type":"captcha"}}"#;
        assert_eq!(result(ok), Ok(Some(("abc:def".into(), "captcha".into()))));
        let old = r#"{"type":"pm_captcha","token":"xyz"}"#;
        assert_eq!(result(old), Ok(Some(("xyz".into(), "captcha".into()))));
        assert_eq!(result(r#"{"type":"CLOSE"}"#), Err(()));
        // Other messages (resizing, hCaptcha's own) are passed over.
        assert_eq!(
            result(r#"{"type":"RESIZE","payload":{"height":500}}"#),
            Ok(None)
        );
        assert_eq!(result("not json"), Ok(None));
        // A token with spaces or control characters isn't a token.
        let bad =
            r#"{"type":"HUMAN_VERIFICATION_SUCCESS","payload":{"token":"a b","type":"captcha"}}"#;
        assert_eq!(result(bad), Ok(None));
    }

    #[test]
    fn stays_with_proton_and_its_captcha() {
        assert!(allowed("https://verify.proton.me/?methods=captcha&token=t"));
        assert!(allowed("https://newassets.hcaptcha.com/captcha/v1/x"));
        assert!(allowed("https://www.google.com/recaptcha/api2/anchor"));
        assert!(!allowed("https://www.google.com/search?q=x"));
        assert!(!allowed("http://verify.proton.me/"));
        assert!(!allowed("https://proton.me.evil.example/"));
        assert!(!allowed("file:///etc/passwd"));
    }
}

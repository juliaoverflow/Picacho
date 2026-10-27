use super::{document::EmailDocument, resources};
use blitz_dom::Document;
use blitz_traits::events::*;
use std::prelude::v1::test;
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::atomic::AtomicUsize,
};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

fn mouse(x: f32, y: f32, pressed: bool) -> BlitzPointerEvent {
    BlitzPointerEvent {
        id: BlitzPointerId::Mouse,
        is_primary: true,
        coords: PointerCoords {
            page_x: x,
            page_y: y,
            client_x: x,
            client_y: y,
            screen_x: x,
            screen_y: y,
        },
        button: MouseEventButton::Main,
        buttons: if pressed {
            MouseEventButtons::Primary
        } else {
            MouseEventButtons::None
        },
        mods: Default::default(),
        details: Default::default(),
        element: Default::default(),
        active_pointers: Default::default(),
    }
}
#[test]
fn blitz_paints_html_and_details_expand_with_real_pointer_events() {
    let mut email = EmailDocument::new(
        "<div style='width:100px;height:30px;background:#ff0000'></div><details id='d'><summary id='s'>More information</summary><p style='height:100px'>Previously hidden content</p></details>",
        true,
    );
    let before = email.layout(500);
    let pixels = email.pixels(500, 300, 1., 0);
    assert_eq!(
        &pixels[(20 * 500 + 20) * 4..][..4],
        &[0, 0, 255, 255],
        "Blitz paints the author's CSS background"
    );
    let node = email
        .doc
        .get_node(email.doc.get_element_by_id("s").unwrap())
        .unwrap();
    let pos = node.absolute_position(0., 0.);
    let x = pos.x + 25.;
    let y = pos.y + node.final_layout().size.height / 2.;
    email
        .doc
        .handle_ui_event(UiEvent::PointerMove(mouse(x, y, false)));
    email
        .doc
        .handle_ui_event(UiEvent::PointerDown(mouse(x, y, true)));
    email
        .doc
        .handle_ui_event(UiEvent::PointerUp(mouse(x, y, false)));
    let after = email.layout(500);
    assert!(email.doc.query_selector("details[open]").unwrap().is_some());
    assert!(
        after > before + 80.,
        "interactive expansion increases the native scroll height: {before} -> {after}"
    );
}

#[test]
fn resource_policy_blocks_every_network_path_and_enabling_it_loads_css_and_images() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let requests = Arc::new(AtomicUsize::new(0));
    let seen = requests.clone();
    let stop = Arc::new(AtomicBool::new(false));
    let stopped = stop.clone();
    let server = std::thread::spawn(move || {
        while !stopped.load(Ordering::Relaxed) {
            if let Ok((mut socket, _)) = listener.accept() {
                socket
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut request = [0; 4096];
                let len = socket.read(&mut request).unwrap();
                let request = String::from_utf8_lossy(&request[..len]);
                assert!(!request.to_ascii_lowercase().contains("cookie:"));
                assert!(!request.to_ascii_lowercase().contains("referer:"));
                if request.contains("/redirect.css ") {
                    seen.fetch_add(1, Ordering::Relaxed);
                    let _ = write!(
                        socket,
                        "HTTP/1.1 302 Found\r\nLocation: /sheet.css\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    );
                    continue;
                }
                let body = if request.contains("/sheet.css ") {
                    "@import '/nested.css'; #box {height:123px;background:#00ff00}"
                } else if request.contains("/nested.css ") {
                    "#box {width:100px}"
                } else {
                    ""
                };
                seen.fetch_add(1, Ordering::Relaxed);
                let _ = write!(
                    socket,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: text/css\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            } else {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    });
    let html = format!(
        "<link rel='stylesheet' href='{origin}/redirect.css'><style>@font-face {{font-family:test;src:url({origin}/font.woff)}} body{{font-family:test}} #bg{{background-image:url({origin}/bg.png)}}</style><div id='box'>Hello</div><div id='bg'>Background</div><img src='{origin}/image.png'>"
    );
    {
        let mut blocked = EmailDocument::new(&html, true);
        blocked.layout(400);
        blocked.layout(400);
        assert_eq!(requests.load(Ordering::Relaxed), 0);
        assert!(!blocked.doc.has_pending_critical_resources());
    }
    let mut allowed = EmailDocument::new(&html, false);
    let until = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        allowed.layout(400);
        let box_node = allowed
            .doc
            .get_node(allowed.doc.get_element_by_id("box").unwrap())
            .unwrap();
        if box_node.final_layout().size.height == 123. && requests.load(Ordering::Relaxed) >= 4 {
            break;
        }
        assert!(
            std::time::Instant::now() < until,
            "allowed resources did not load; requests={}",
            requests.load(Ordering::Relaxed)
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let pixels = allowed.pixels(400, 300, 1., 0);
    assert_eq!(&pixels[(70 * 400 + 30) * 4..][..4], &[0, 255, 0, 255]);
    drop(allowed);
    stop.store(true, Ordering::Relaxed);
    server.join().unwrap();
}

#[test]
fn unsafe_resource_schemes_and_credentials_are_refused() {
    for url in [
        "file:///etc/passwd",
        "ftp://example.com/a",
        "javascript:alert(1)",
        "http://user:secret@example.com/a",
        "about:blank",
    ] {
        assert!(!resources::remote_url(&url::Url::parse(url).unwrap()));
    }
    assert!(resources::remote_url(
        &url::Url::parse("https://example.com/a").unwrap()
    ));
    let cancelled = Arc::new(AtomicBool::new(false));
    let resources =
        resources::Resources::new(false, Arc::new(AtomicBool::new(false)), cancelled.clone());
    struct Capture(std::sync::mpsc::Sender<blitz_traits::net::Bytes>);
    impl blitz_traits::net::NetHandler for Capture {
        fn bytes(self: Box<Self>, _: String, bytes: blitz_traits::net::Bytes) {
            self.0.send(bytes).unwrap();
        }
    }
    use blitz_traits::net::NetProvider;
    let (tx, rx) = std::sync::mpsc::channel();
    resources.fetch(
        0,
        blitz_traits::net::Request::get(url::Url::parse("file:///etc/passwd").unwrap()),
        Box::new(Capture(tx.clone())),
    );
    assert!(rx.recv_timeout(Duration::from_secs(1)).unwrap().is_empty());
    resources.fetch(
        0,
        blitz_traits::net::Request::get(
            url::Url::parse("data:text/plain;base64,aGVsbG8=").unwrap(),
        ),
        Box::new(Capture(tx)),
    );
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(1)).unwrap().as_ref(),
        b"hello"
    );
    cancelled.store(true, Ordering::Relaxed);
}

#[test]
fn long_email_painting_uses_visible_offset_and_hidpi_scale() {
    let mut email = EmailDocument::new(
        "<body style='margin:0'><div style='height:1000px;background:red'></div><div style='height:1000px;background:blue'></div></body>",
        true,
    );
    assert!(email.layout(400) >= 2000.);
    let pixels = email.pixels(800, 200, 2., 2400);
    assert_eq!(pixels.len(), 800 * 200 * 4);
    assert_eq!(
        &pixels[(50 * 800 + 50) * 4..][..4],
        &[255, 0, 0, 255],
        "a scrolled HiDPI tile shows the blue section"
    );
}

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
fn only_web_and_email_links_are_opened() {
    for url in [
        "file:///C:/Windows/System32/calc.exe",
        "file://attacker.example/share/payload.exe",
        "smb://attacker.example/share/payload.exe",
        "\\\\attacker.example\\share\\payload.exe",
        "javascript:alert(1)",
        "ms-msdt:/id PCWDiagnostic",
        "search-ms:query=x&crumb=location:\\\\attacker.example\\share",
        "http://user:secret@example.com/",
        "/relative/image.png",
    ] {
        assert!(super::openable_url(url).is_none(), "{url}");
    }
    assert!(super::openable_url("https://example.com/a.png").is_some());
    assert!(super::openable_url("mailto:ana@example.com").is_some());
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
        resources::Resources::new(
            false,
            Default::default(),
            Arc::new(AtomicBool::new(false)),
            cancelled.clone(),
        );
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
fn context_menus_never_offer_local_or_network_share_images() {
    let target = |src: &str| {
        let mut email = EmailDocument::new(
            &format!(
                "<body style='margin:0'><a href='file:///C:/x.exe'><img src='{src}' style='display:block;width:100px;height:100px'></a></body>"
            ),
            true,
        );
        email.layout(400);
        email.context_at(50., 50.)
    };
    for src in [
        "file://attacker.example/share/payload.exe",
        "smb://attacker.example/share/payload.exe",
        "\\\\attacker.example\\share\\payload.exe",
        // Unresolvable against the email's base; these used to stop it rendering.
        "logo.png",
        "http://[",
    ] {
        let hit = target(src);
        assert_eq!(hit.image, None, "{src}");
        assert_eq!(hit.link, None, "a file: link is not offered either");
    }
    assert_eq!(
        target("https://example.com/a.png").image.as_deref(),
        Some("https://example.com/a.png")
    );
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

#[test]
fn dark_reading_darkens_pages_lightens_text_and_keeps_brand_colors() {
    use super::document::{Role, dark_color};
    let lightness = |c: [f32; 4]| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
    let white = [1., 1., 1., 1.];
    let black = [0., 0., 0., 1.];
    let blue = [0.1, 0.45, 0.91, 1.];
    assert!(
        lightness(dark_color(white, Role::Surface)) < 0.2,
        "white page turns dark"
    );
    assert!(
        lightness(dark_color(black, Role::Text)) > 0.75,
        "black text turns light"
    );
    assert!(
        lightness(dark_color(white, Role::Text)) > 0.98,
        "white text on a banner stays"
    );
    let kept = dark_color(blue, Role::Surface);
    assert!(
        (kept[2] - blue[2]).abs() < 0.15 && kept[2] > kept[0],
        "a blue button stays blue"
    );
    let sky = dark_color([0.24, 0.71, 1., 1.], Role::Surface);
    assert!(
        sky[2] > 0.8,
        "a light but saturated brand color is not flipped"
    );
    let page = dark_color(white, Role::Surface);
    assert!(
        (page[0] - page[2]).abs() < 0.01,
        "neutral stays neutral: {page:?}"
    );
}

#[test]
fn images_sent_with_the_message_show_by_content_id_even_with_remote_content_blocked() {
    use crate::manage::model::{Account, Folder, Provider};
    use base64::Engine as _;
    // USPS Informed Delivery: the mailpiece scans are attached to the
    // message and the HTML shows them as `cid:` images.
    let html = include_str!("../../../tests/fixtures/usps-informed-delivery.html");
    let png = |rgb: [u8; 3]| {
        let image = image::RgbImage::from_pixel(300, 120, image::Rgb(rgb));
        let mut bytes = std::io::Cursor::new(Vec::new());
        image.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
        base64::engine::general_purpose::STANDARD.encode(bytes.into_inner())
    };
    let mut raw = String::from(
        "From: USPS Informed Delivery <USPSInformeddelivery@email.informeddelivery.usps.com>\r\n\
         Subject: Your Daily Digest\r\n\
         Content-Type: multipart/related; boundary=\"rel\"\r\n\r\n\
         --rel\r\nContent-Type: text/html; charset=utf-8\r\nContent-Transfer-Encoding: base64\r\n\r\n",
    );
    raw += &base64::engine::general_purpose::STANDARD.encode(html);
    // Some senders percent-encode or change the case of the id in the URL.
    for (id, rgb) in [
        ("2484554579-510.jpg", [255, 0, 0]),
        ("Mailer-13667.JPG", [0, 0, 255]),
    ] {
        raw += &format!(
            "\r\n--rel\r\nContent-Type: image/png; name=\"{id}\"\r\n\
             Content-Disposition: inline; filename=\"{id}\"\r\n\
             Content-ID: <{id}>\r\nContent-Transfer-Encoding: base64\r\n\r\n{}",
            png(rgb)
        );
    }
    raw += "\r\n--rel--\r\n";
    let mail = crate::sync::mime::parse_message(
        raw.as_bytes(),
        &Account::new(Provider::Imap),
        "usps",
        Folder::Inbox,
        "INBOX",
        1,
    )
    .unwrap();
    assert!(!mail.lacks_inline_parts(), "{:?}", mail.inline);

    let mut email = EmailDocument::with_parts(
        mail.html.as_deref().unwrap(),
        mail.inline.clone(),
        true,
        super::document::Tone::Light,
    );
    let color_at = |email: &mut EmailDocument, id: &str| {
        let node = email
            .doc
            .get_node(email.doc.get_element_by_id(id).unwrap())
            .unwrap();
        let size = node.final_layout().size;
        let pos = node.absolute_position(0., 0.);
        let (x, y) = (pos.x + size.width / 2., pos.y + size.height / 2.);
        let pixels = email.pixels_at(1, 1, 1., x, y);
        (size, [pixels[2], pixels[1], pixels[0]])
    };
    email.layout(640);
    // The images load on the document's thread; give them a moment.
    for _ in 0..50 {
        if color_at(&mut email, "mailpiece-image-src-id").1 == [255, 0, 0] {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
        email.layout(640);
    }
    let (size, color) = color_at(&mut email, "mailpiece-image-src-id");
    assert!(size.width > 100. && size.height > 50., "the scan has its size: {size:?}");
    assert_eq!(color, [255, 0, 0], "the scan is painted");
    let (_, color) = color_at(&mut email, "campaign-representative-image-src-id");
    assert_eq!(color, [0, 0, 255], "an id in another case still matches");
}

/// A development aid, not a check: renders the HTML file `SNAP_HTML` at
/// `SNAP_WIDTH` (700) pixels to `SNAP_DIR/usps-{light,dark}-{width}.png`,
/// with a stand-in picture for each image sent with the message (`cid:`).
/// `cargo test render_to_png -- --ignored`.
#[test]
#[ignore]
fn render_to_png() {
    use super::document::Tone;
    let html = std::fs::read_to_string(std::env::var("SNAP_HTML").unwrap()).unwrap();
    let width: u32 = std::env::var("SNAP_WIDTH").ok().and_then(|w| w.parse().ok()).unwrap_or(700);
    // Stand-ins for the images sent with the message.
    let mut inline = crate::manage::model::InlineParts::default();
    for part in html.split("cid:").skip(1) {
        let id: String = part.chars().take_while(|c| !matches!(c, '"' | '\'' | ')')).collect();
        let image = image::RgbImage::from_fn(510, 300, |x, y| {
            image::Rgb(if (x / 30 + y / 30) % 2 == 0 { [90, 120, 160] } else { [200, 210, 225] })
        });
        let mut png = std::io::Cursor::new(Vec::new());
        image.write_to(&mut png, image::ImageFormat::Png).unwrap();
        inline.insert(&id, png.into_inner());
    }
    let inline = std::sync::Arc::new(inline);
    for (tone, name) in [(Tone::Light, "light"), (Tone::NativeDark, "dark")] {
        let mut email = EmailDocument::with_parts(&html, inline.clone(), true, tone);
        email.layout(width);
        // The images arrive on the document's thread.
        std::thread::sleep(Duration::from_millis(300));
        let height = email.layout(width);
        let h = (height.ceil() as u32).min(6000);
        let mut px = email.pixels(width, h, 1., 0);
        for p in px.chunks_mut(4) {
            p.swap(0, 2);
        }
        let img = image::RgbaImage::from_raw(width, h, px).unwrap();
        img.save(format!("{}/usps-{name}-{width}.png", std::env::var("SNAP_DIR").unwrap())).unwrap();
        eprintln!("{name}: height {height}");
    }
}

/// The color painted at a point of a document laid out `width` wide.
fn color_at(email: &mut EmailDocument, x: f32, y: f32) -> [u8; 3] {
    let pixels = email.pixels_at(1, 1, 1., x, y);
    [pixels[2], pixels[1], pixels[0]]
}
fn element_box(email: &EmailDocument, id: &str) -> (f32, f32, f32, f32) {
    let node = email
        .doc
        .get_node(email.doc.get_element_by_id(id).unwrap())
        .unwrap();
    let pos = node.absolute_position(0., 0.);
    let size = node.final_layout().size;
    (pos.x, pos.y, size.width, size.height)
}

#[test]
fn tables_restacked_for_narrow_screens_keep_their_rows() {
    // Emails restack a layout by making a table (or its cells) `display:
    // block` in a narrow-screen media query.
    let mut email = EmailDocument::new(
        "<body style='margin:0'>\
         <table style='display:block;width:100%' cellspacing='0' cellpadding='0'><tbody>\
         <tr><td id='a' style='background:#ff0000;width:50%;height:40px'>One</td></tr>\
         <tr><td id='b' style='background:#0000ff;height:40px'>Two</td></tr>\
         </tbody></table>\
         <table width='100%' cellspacing='0' cellpadding='0'><tbody>\
         <tr><td id='c' style='background:#00ff00;height:40px'>Three</td></tr>\
         <tr><td id='d' style='display:block;background:#ff00ff;height:40px'>Four</td></tr>\
         </tbody></table></body>",
        true,
    );
    email.layout(400);
    let (_, ay, _, ah) = element_box(&email, "a");
    let (_, by, bw, bh) = element_box(&email, "b");
    assert!(by >= ay + ah - 1., "rows stack: a at {ay}+{ah}, b at {by}");
    assert!(bw > 0. && bh > 0.);
    // A block in a row stands in for its cell rather than vanishing.
    let (dx, dy, dw, dh) = element_box(&email, "d");
    assert!(dw > 100. && dh >= 40., "the block cell is laid out: {dw}x{dh}");
    assert_eq!(color_at(&mut email, dx + dw / 2., dy + dh / 2.), [255, 0, 255]);
}

#[test]
fn cellspacing_and_cellpadding_attributes_apply() {
    let mut email = EmailDocument::new(
        "<body style='margin:0;background:#ffffff'>\
         <table width='200' cellspacing='0' cellpadding='0'><tr>\
         <td id='a' style='background:#ff0000;height:30px'></td>\
         <td id='b' style='background:#0000ff;height:30px'></td></tr></table>\
         <table width='200' cellspacing='0' cellpadding='12'><tr>\
         <td id='p'><div id='inner' style='height:10px'></div></td></tr></table></body>",
        true,
    );
    email.layout(400);
    let (ax, ay, aw, ah) = element_box(&email, "a");
    let (bx, ..) = element_box(&email, "b");
    assert_eq!(bx, ax + aw, "no spacing between the cells");
    // No white seam where they meet, nor above them.
    assert_ne!(color_at(&mut email, ax + aw - 0.5, ay + ah / 2.), [255, 255, 255]);
    assert_eq!(ay, 0., "no spacing around the cells");
    let (px, py, ..) = element_box(&email, "p");
    let (ix, iy, ..) = element_box(&email, "inner");
    assert_eq!((ix - px, iy - py), (12., 12.), "cellpadding pads the cell");
}

#[test]
fn inline_gradient_backgrounds_paint_over_a_light_color() {
    // The dark-mode trick: a later light background-color, under a gradient.
    let mut email = EmailDocument::new(
        "<style>.dark{background:linear-gradient(#202020,#202020)}.dark{background-color:#f7f7f7}</style>\
         <body style='margin:0;font-size:40px'><p style='margin:0'>\
         <span id='s' class='dark' style='color:#ff0000'>MMMM</span></p></body>",
        true,
    );
    email.layout(400);
    // Inline boxes have no layout box of their own; sample the line.
    let (_, py, _, ph) = {
        let p = email.doc.query_selector("p").unwrap().unwrap();
        let node = email.doc.get_node(p).unwrap();
        let pos = node.absolute_position(0., 0.);
        let size = node.final_layout().size;
        (pos.x, pos.y, size.width, size.height)
    };
    let line: Vec<[u8; 3]> = (0..60)
        .map(|x| color_at(&mut email, x as f32 * 2. + 0.5, py + ph / 2.))
        .collect();
    // Red text on the dark gradient keeps green low; the light color under
    // it (#f7f7f7) would not.
    assert!(
        line.iter().all(|[_, g, _]| *g < 60),
        "only the gradient shows behind the text: {line:?}"
    );
    assert!(
        line.iter().filter(|c| **c == [32, 32, 32]).count() > 5,
        "between the letters it is the gradient's color: {line:?}"
    );
}

#[test]
fn clicking_an_emails_links_opens_them() {
    use blitz_dom::DocumentConfig;
    use blitz_traits::navigation::{NavigationOptions, NavigationProvider};
    struct Record(std::sync::Mutex<Vec<String>>);
    impl NavigationProvider for Record {
        fn navigate_to(&self, options: NavigationOptions) {
            self.0.lock().unwrap().push(options.url.to_string());
        }
    }
    let record = Arc::new(Record(Default::default()));
    let html = include_str!("../../../tests/fixtures/usps-informed-delivery.html")
        .replace(
            "id=\"mailbox-url-link\" class=\"view-dashboard-a\" href=\"hiddenforprivacy\"",
            "id=\"mailbox-url-link\" class=\"view-dashboard-a\" href=\"https://example.com/dashboard\"",
        )
        .replace(
            "id=\"unsubscribe-url-link\" href=\"hiddenforprivacy\"",
            "id=\"unsubscribe-url-link\" href=\"https://example.com/unsubscribe\"",
        );
    let mut doc = blitz_html::HtmlDocument::from_html(
        &super::document::as_html(&html),
        DocumentConfig {
            base_url: Some("about:blank".into()),
            navigation_provider: Some(record.clone()),
            ..Default::default()
        },
    );
    doc.set_viewport(blitz_traits::shell::Viewport::new(
        900,
        600,
        1.,
        blitz_traits::shell::ColorScheme::Light,
    ));
    doc.resolve(0.);
    let mut click = |id: &str| {
        // A link is an inline box, with no layout box of its own: find a
        // point on its line that hits it.
        let link = doc.get_element_by_id(id).unwrap();
        let mut block = doc.get_node(link).unwrap();
        while block.final_layout().size.height == 0. {
            block = doc.get_node(block.parent.unwrap()).unwrap();
        }
        let pos = block.absolute_position(0., 0.);
        let size = block.final_layout().size;
        let within = |doc: &blitz_html::HtmlDocument, x: f32, y: f32| {
            let mut node = doc.hit(x, y).map(|h| h.node_id);
            while let Some(id) = node {
                if id == link {
                    return true;
                }
                node = doc.get_node(id).and_then(|n| n.parent);
            }
            false
        };
        let (x, y) = (0..400)
            .flat_map(|i| (0..20).map(move |j| (i, j)))
            .map(|(i, j)| {
                (
                    pos.x + size.width * i as f32 / 400.,
                    pos.y + size.height * j as f32 / 20.,
                )
            })
            .find(|&(x, y)| within(&doc, x, y))
            .unwrap_or_else(|| panic!("{id} can be hit"));
        doc.handle_ui_event(UiEvent::PointerMove(mouse(x, y, false)));
        doc.handle_ui_event(UiEvent::PointerDown(mouse(x, y, true)));
        doc.handle_ui_event(UiEvent::PointerUp(mouse(x, y, false)));
    };
    // A button made of a span in a link, and a link that opens a new tab.
    click("mailbox-url-link");
    click("unsubscribe-url-link");
    assert_eq!(
        *record.0.lock().unwrap(),
        ["https://example.com/dashboard", "https://example.com/unsubscribe"]
    );
}


#[test]
fn emails_declaring_xhtml_are_parsed_as_html() {
    // Countless emails begin with an XHTML DOCTYPE, yet are sent as
    // text/html. Parsed as XML, the unclosed <meta> swallowed the email and
    // the `<` in its CSS began elements: it showed nothing at all.
    let html = include_str!("../../../tests/fixtures/usps-informed-delivery.html");
    assert!(html.starts_with("<!DOCTYPE html PUBLIC \"-//W3C//DTD XHTML 1.0"));
    assert!(html.contains("<=430px"));
    let mut email = EmailDocument::new(html, true);
    let height = email.layout(640);
    assert!(height > 1500., "the email is laid out: {height}");
    let style = email.doc.query_selector("head style").unwrap();
    assert!(style.is_some(), "<style> is in <head>, not swallowed by a <meta>");
    let tracking = email
        .doc
        .get_node(email.doc.get_element_by_id("pra-shipper-name-id").unwrap())
        .unwrap();
    assert_eq!(tracking.text_content(), "Example Shop");
    // Same with the namespace on <html> and no DOCTYPE, and after an XML
    // declaration.
    for start in [
        "<html xmlns=\"http://www.w3.org/1999/xhtml\"><head><meta charset=utf-8>",
        "<?xml version=\"1.0\"?><html><head><meta charset=utf-8>",
    ] {
        let mut email = EmailDocument::new(
            &format!("{start}<style>/* <=430px */ p {{ height: 40px }}</style></head><body><p>Hi</p></body></html>"),
            true,
        );
        assert!(email.layout(400) >= 40., "{start}");
    }
}

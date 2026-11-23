//! How emails are laid out and painted: browser behavior that email HTML
//! relies on, dark reading, and the document thread's input and output.
use super::document::{Compat, EmailDocument, Tone, compat_mode, page_color};
use super::worker::{self, Command, Output, Update};
use blitz_dom::Document;
use blitz_traits::events::*;
use std::prelude::v1::test;
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::{Arc, atomic::Ordering},
    time::{Duration, Instant},
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
/// The color painted at a point, in document coordinates.
fn rgb(email: &mut EmailDocument, x: f32, y: f32) -> [u8; 3] {
    let p = email.pixels_at(1, 1, 1., x, y);
    [p[2], p[1], p[0]]
}
/// An element's box: (x, y, width, height).
fn bx(email: &EmailDocument, id: &str) -> (f32, f32, f32, f32) {
    let node = email
        .doc
        .get_node(
            email
                .doc
                .get_element_by_id(id)
                .unwrap_or_else(|| panic!("#{id}")),
        )
        .unwrap();
    let pos = node.absolute_position(0., 0.);
    let s = node.final_layout().size;
    (pos.x, pos.y, s.width, s.height)
}
/// The colors painted over an element's box, on a grid.
fn colors_in(email: &mut EmailDocument, id: &str) -> Vec<[u8; 3]> {
    let (x, y, w, h) = bx(email, id);
    let mut out = Vec::new();
    for i in 0..30 {
        for j in 0..12 {
            out.push(rgb(
                email,
                x + (i as f32 + 0.5) * w / 30.,
                y + (j as f32 + 0.5) * h / 12.,
            ));
        }
    }
    out
}
fn luma([r, g, b]: [u8; 3]) -> f32 {
    0.2126 * r as f32 + 0.7152 * g as f32 + 0.0722 * b as f32
}
fn png(w: u32, h: u32, c: [u8; 3]) -> String {
    use base64::Engine as _;
    let img = image::RgbImage::from_pixel(w, h, image::Rgb(c));
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).unwrap();
    format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(out.into_inner())
    )
}

#[test]
fn an_email_inside_a_form_is_shown() {
    let mut email = EmailDocument::new(
        "<body style='margin:0'><form action='https://example.com'><div style='height:300px'>The newsletter</div><input value=x></form>",
        true,
    );
    assert!(email.layout(400) >= 300.);
}

#[test]
fn animations_and_transitions_settle_at_their_end() {
    let mut email = EmailDocument::new(
        "<style>@keyframes fade{from{opacity:0}to{opacity:1}} #b{animation:fade 1s;width:100px;height:40px;background:#00ff00}\
         #a{display:block;width:100px;height:40px;background:#0000ff;transition:background-color .3s}#a:hover{background:#ff0000}</style>\
         <body style='margin:0'><a id='a' href='https://example.com'></a><div id='b'></div>",
        true,
    );
    email.layout(400);
    assert_eq!(
        rgb(&mut email, 50., 60.),
        [0, 255, 0],
        "a fade-in shows its content"
    );
    email
        .doc
        .handle_ui_event(UiEvent::PointerMove(mouse(50., 20., false)));
    email.resolve();
    assert_eq!(
        rgb(&mut email, 50., 20.),
        [255, 0, 0],
        "a hover transition ends"
    );
}

#[test]
fn only_emails_whose_dark_styles_set_backgrounds_are_read_in_them() {
    let dark = |html: &str| Tone::for_email(html, true);
    assert_eq!(
        dark("<style>@media (prefers-color-scheme: dark) { .logo-dark { display:block } }</style>"),
        Tone::InvertedDark,
        "only a logo swapped"
    );
    assert_eq!(
        dark("<style>@media (prefers-color-scheme: light) {} p { color: darkgrey }</style>"),
        Tone::InvertedDark
    );
    assert_eq!(
        dark(
            "<style>@media (PREFERS-COLOR-SCHEME : DARK) { body { background: #111 !important } }</style>"
        ),
        Tone::NativeDark
    );
    assert_eq!(
        dark(
            "<style>@media screen and (prefers-color-scheme:dark){.a{color:#fff}.b{background-color:#000}}</style>"
        ),
        Tone::NativeDark
    );
    assert_eq!(
        Tone::for_email(
            "<style>@media (prefers-color-scheme: dark){body{background:#000}}</style>",
            false
        ),
        Tone::Light
    );
}

/// Waits for an output the predicate picks (and returns what it makes of it).
fn wait_for<T>(
    rx: &mut futures::channel::mpsc::UnboundedReceiver<Update>,
    mut pick: impl FnMut(Output) -> Option<T>,
) -> Option<T> {
    let until = Instant::now() + Duration::from_secs(10);
    while Instant::now() < until {
        match rx.try_recv() {
            Ok(update) => {
                if let Some(found) = pick(update.output) {
                    return Some(found);
                }
            }
            Err(futures::channel::mpsc::TryRecvError::Closed) => return None,
            Err(_) => std::thread::sleep(Duration::from_millis(5)),
        }
    }
    None
}
fn spawn(
    html: &str,
    tone: Tone,
) -> (
    std::sync::mpsc::Sender<Command>,
    futures::channel::mpsc::UnboundedReceiver<Update>,
) {
    let (tx, rx) = futures::channel::mpsc::unbounded();
    let commands = worker::spawn(html.into(), Default::default(), true, tone, 1, tx);
    commands
        .send(Command::Layout {
            width: 400,
            height: 800,
            scale: 1.,
        })
        .unwrap();
    (commands, rx)
}
fn page_of(output: Output) -> Option<u32> {
    match output {
        Output::State { page, .. } => Some(page),
        _ => None,
    }
}

#[test]
fn dark_styles_that_leave_the_email_light_are_replaced_by_inverted_colors() {
    // A dark rule for a detail, and the page left white.
    let (_commands, mut rx) = spawn(
        "<style>@media (prefers-color-scheme: dark){.badge{background:#000}}</style>\
         <table width='100%' bgcolor='#ffffff' style='height:600px'><tr><td style='color:#000'>Your order</td></tr></table>",
        Tone::NativeDark,
    );
    assert_eq!(
        wait_for(&mut rx, page_of),
        Some(page_color(Tone::InvertedDark))
    );
    // Dark styles that darken the email are kept.
    let (_commands, mut rx) = spawn(
        "<style>@media (prefers-color-scheme: dark){body{background:#000000 !important}}</style><p>Your order</p>",
        Tone::NativeDark,
    );
    assert_eq!(wait_for(&mut rx, page_of), Some(0x000000));
}

#[test]
fn dark_text_on_a_pastel_brand_color_stays_dark() {
    let mut email = EmailDocument::with_tone(
        "<body style='margin:0'><table cellpadding=0 cellspacing=0><tr><td id='t' bgcolor='#aaccff' style='padding:10px;font-size:40px;color:#000'>WWWW</td></tr></table>",
        true,
        Tone::InvertedDark,
    );
    email.layout(400);
    let colors = colors_in(&mut email, "t");
    let darkest = colors.iter().map(|c| luma(*c)).fold(255., f32::min);
    let surface = rgb(&mut email, 3., 3.);
    assert!(
        surface[2] > surface[0] + 40,
        "the brand color stays: {surface:?}"
    );
    assert!(darkest < 80., "the text keeps its contrast: {darkest}");
}

#[test]
fn dark_borders_and_rules_turn_light_in_inverted_colors() {
    let mut email = EmailDocument::with_tone(
        "<body style='margin:0'><div style='height:20px;border-bottom:4px solid #000'></div>\
         <div style='height:1px;background:#000'></div><div style='height:20px'></div>\
         <p id='u' style='margin:0;font-size:30px;line-height:50px;color:#000;text-decoration:underline;text-decoration-thickness:4px'>________</p>",
        true,
        Tone::InvertedDark,
    );
    email.layout(400);
    assert!(
        luma(rgb(&mut email, 50., 22.)) > 170.,
        "a black border turns light"
    );
    assert!(
        luma(rgb(&mut email, 50., 24.)) > 170.,
        "a black rule turns light"
    );
    assert!(luma(rgb(&mut email, 50., 10.)) < 60., "the page turns dark");
}

#[test]
fn svg_images_keep_their_colors_in_inverted_dark() {
    let html = format!(
        "<body style='margin:0'><img id='s' width=50 height=50 src=\"data:image/svg+xml,<svg xmlns='http://www.w3.org/2000/svg' width='50' height='50'><rect width='50' height='50' fill='%23888888'/></svg>\">\
         <img id='p' width=50 height=50 src='{}'>",
        png(4, 4, [0x88, 0x88, 0x88])
    );
    let mut email = EmailDocument::with_tone(&html, true, Tone::InvertedDark);
    email.layout(400);
    email.layout(400);
    let (x, y, _, _) = bx(&email, "s");
    let svg = rgb(&mut email, x + 25., y + 25.);
    let (x, y, _, _) = bx(&email, "p");
    assert_eq!(rgb(&mut email, x + 25., y + 25.), [0x88; 3]);
    assert!(
        svg.iter().all(|c| c.abs_diff(0x88) <= 2),
        "the SVG is untouched: {svg:?}"
    );
}

#[test]
fn local_css_backgrounds_load_in_the_next_layout() {
    let html = format!(
        "<body style='margin:0'><div style=\"width:100px;height:100px;background-image:url('{}')\"></div>",
        png(4, 4, [255, 0, 0])
    );
    let mut email = EmailDocument::new(&html, true);
    email.layout(400);
    assert!(
        email.changed.swap(false, Ordering::Acquire),
        "the document thread is asked to lay out again"
    );
    email.layout(400);
    assert_eq!(rgb(&mut email, 50., 50.), [255, 0, 0]);
    assert!(!email.changed.load(Ordering::Acquire), "and only once");
}

#[test]
fn content_wider_than_the_reader_is_scaled_to_fit() {
    let mut email = EmailDocument::new(
        "<body style='margin:0'><div style='background:#eee'><table id='t' width='700' cellpadding=0 cellspacing=0><tr><td height=100>x</td></tr></table></div>",
        true,
    );
    email.layout(360);
    assert_eq!(bx(&email, "t").2, 700.);
    assert!(
        (email.zoom() - 360. / 700.).abs() < 0.01,
        "{}",
        email.zoom()
    );
    // Content clipped by its box doesn't count.
    let mut email = EmailDocument::new(
        "<body style='margin:0'><div style='width:300px;overflow:hidden'><div style='width:3000px;height:20px'></div></div>",
        true,
    );
    email.layout(360);
    assert_eq!(email.zoom(), 1.);
}

#[test]
fn absolutely_positioned_content_is_in_the_height() {
    let mut email = EmailDocument::new(
        "<body style='margin:0'><div style='height:100px'></div><div style='position:absolute;top:900px;height:50px;width:10px;background:red'></div>",
        true,
    );
    assert!(email.layout(400) >= 950.);
}

#[test]
fn a_word_in_a_narrow_cell_is_not_broken_into_letters() {
    let mut email = EmailDocument::new(
        "<body style='margin:0'><table width=300 cellpadding=0 cellspacing=0><tr><td id='a'>Unsubscribe</td><td><div style='width:280px'>x</div></td></tr></table>",
        true,
    );
    email.layout(600);
    assert!(bx(&email, "a").2 > 60., "{:?}", bx(&email, "a"));
    // A long address still wraps rather than widening the email.
    let mut email = EmailDocument::new(
        "<p id='p'>https://example.com/a/very/long/address/that/goes/on/and/on/and/on/for/ever/and/ever</p>",
        true,
    );
    email.layout(200);
    assert!(bx(&email, "p").3 > 30.);
}

#[test]
fn the_body_background_fills_the_page() {
    for html in [
        "<body style='background:#336699'><p>Hi</p>",
        "<body bgcolor='#336699'><p>Hi</p>",
    ] {
        let mut email = EmailDocument::new(html, true);
        email.layout(400);
        assert_eq!(rgb(&mut email, 2., 2.), [0x33, 0x66, 0x99], "{html}");
        assert_eq!(email.page_color(), 0x336699);
    }
    let mut email = EmailDocument::new("<p>Hi</p>", true);
    email.layout(400);
    assert_eq!(rgb(&mut email, 2., 2.), [255; 3]);
    assert_eq!(email.page_color(), 0xffffff);
}

#[test]
fn doctypes_choose_the_mode_that_spaces_images() {
    assert_eq!(compat_mode("<html><body>"), Compat::Quirks);
    // Not a DOCTYPE, with a character across its ninth byte.
    assert_eq!(compat_mode("<!DOCTYé html>"), Compat::Quirks);
    assert_eq!(compat_mode("<!-- x --> <!DOCTYPE html>"), Compat::Standards);
    assert_eq!(
        compat_mode(
            "<!DOCTYPE html PUBLIC \"-//W3C//DTD XHTML 1.0 Transitional//EN\" \"http://www.w3.org/TR/xhtml1/DTD/xhtml1-transitional.dtd\">"
        ),
        Compat::LimitedQuirks
    );
    assert_eq!(
        compat_mode("<!DOCTYPE HTML PUBLIC \"-//W3C//DTD HTML 4.01 Transitional//EN\">"),
        Compat::Quirks
    );
    assert_eq!(
        compat_mode("<!DOCTYPE html PUBLIC \"-//W3C//DTD XHTML 1.0 Strict//EN\" \"x\">"),
        Compat::Standards
    );
    let image = png(4, 4, [255, 0, 0]);
    let cell = |doctype: &str| {
        let mut email = EmailDocument::new(
            &format!(
                "{doctype}<body style='margin:0'><table cellpadding=0 cellspacing=0 border=0><tr><td id='c'><a href='https://example.com'><img src='{image}' width=100 height=50></a></td></tr><tr><td id='s' height=1><img src='{image}' width=1 height=1></td></tr></table>"
            ),
            true,
        );
        email.layout(400);
        (bx(&email, "c").3, bx(&email, "s").3)
    };
    assert_eq!(cell(""), (50., 1.), "no DOCTYPE: no gaps");
    assert_eq!(
        cell(
            "<!DOCTYPE html PUBLIC \"-//W3C//DTD XHTML 1.0 Transitional//EN\" \"http://www.w3.org/TR/xhtml1/DTD/xhtml1-transitional.dtd\">"
        ),
        (50., 1.),
        "almost standards: none in table cells"
    );
    assert!(
        cell("<!DOCTYPE html>").0 > 50.,
        "standards: the line's descent shows"
    );
}

#[test]
fn legacy_presentation_attributes_apply() {
    let bg = png(4, 4, [0, 0, 255]);
    let html = format!(
        "<body style='margin:0'>\
         <table id='t1' width='300' align='center' cellpadding=0 cellspacing=0><tr><td id='c1' height=40>a</td></tr></table>\
         <table width='100%' cellpadding=0 cellspacing=0><tr><td id='c2' height=40 background='{bg}'>&nbsp;</td></tr></table>\
         <div style='width:400px'><img id='fl' align='RIGHT' width=50 height=50 src='{bg}'>text</div>\
         <table width=400 cellpadding=0 cellspacing=0><tr><td id='n1' bgcolor='red' height=10></td><td id='n2' bgcolor='00ff00' height=10></td></tr></table>\
         <p style='margin:0'><font id='f' color='#ff0000' size='7' face='Arial, \"Helvetica Neue\"'>Big</font> <span id='ref'>Base</span></p>"
    );
    let mut email = EmailDocument::new(&html, true);
    email.layout(800);
    email.layout(800);
    assert_eq!(bx(&email, "t1").0, 250., "table align=center is centered");
    assert_eq!(
        email
            .doc
            .get_node(email.doc.get_element_by_id("c1").unwrap())
            .unwrap()
            .primary_styles()
            .unwrap()
            .clone_text_align(),
        style::computed_values::text_align::T::Start,
        "and its cells' text is not"
    );
    let (x, y, _, _) = bx(&email, "c2");
    assert_eq!(
        rgb(&mut email, x + 300., y + 20.),
        [0, 0, 255],
        "td background"
    );
    assert_eq!(bx(&email, "fl").0, 350., "img align=right floats right");
    let (x, y, _, _) = bx(&email, "n1");
    assert_eq!(
        rgb(&mut email, x + 5., y + 5.),
        [255, 0, 0],
        "bgcolor names a color"
    );
    let (x, y, _, _) = bx(&email, "n2");
    assert_eq!(
        rgb(&mut email, x + 5., y + 5.),
        [0, 255, 0],
        "bgcolor without #"
    );
    let font = email
        .doc
        .get_node(email.doc.get_element_by_id("f").unwrap())
        .unwrap();
    let styles = font.primary_styles().unwrap();
    assert_eq!(
        styles.get_font().clone_font_size().computed_size().px(),
        48.,
        "size=7 is xxx-large"
    );
    let color = styles
        .clone_color()
        .to_color_space(style::color::ColorSpace::Srgb);
    assert_eq!(color.raw_components()[..3], [1., 0., 0.]);
}

#[test]
fn blocked_images_show_their_alt_text_and_tracking_pixels_stay_hidden() {
    let mut email = EmailDocument::new(
        "<body style='margin:0'><a href='https://example.com/sale'><img id='hero' class='hero' src='https://example.com/a.png' alt='Shop the sale' width=200 height=50></a>\
         <img id='t' src='https://t.example.com/o.gif' style='width:1px;height:1px;border:0' alt='tracker'>\
         <img id='n' src='https://example.com/b.png'></body>",
        true,
    );
    email.layout(400);
    assert!(email.doc.query_selector("img#hero").unwrap().is_none());
    let (x, y, w, h) = bx(&email, "hero");
    assert_eq!((w, h), (200., 50.), "the alt text keeps the image's size");
    email.select_all();
    assert!(
        email
            .doc
            .get_selected_text()
            .unwrap()
            .contains("Shop the sale")
    );
    let target = email.context_at(x + 100., y + 25.);
    assert_eq!(target.image.as_deref(), Some("https://example.com/a.png"));
    assert_eq!(target.link.as_deref(), Some("https://example.com/sale"));
    assert_eq!(bx(&email, "t").2, 1., "a pixel sized by CSS stays a pixel");
    assert_eq!(
        bx(&email, "n").2,
        16.,
        "an image of no size gets a small box"
    );
}

#[test]
fn fixed_boxes_are_painted_once() {
    let mut email = EmailDocument::new(
        "<body style='margin:0'><div style='position:fixed;top:0;left:0;width:50px;height:20px;background:#f00'></div><div style='height:3000px'></div>",
        true,
    );
    email.layout(400);
    assert_eq!(rgb(&mut email, 10., 5.), [255, 0, 0]);
    assert_eq!(rgb(&mut email, 10., 1505.), [255; 3]);
}

#[test]
fn viewport_units_and_resolution_queries_see_the_window() {
    let mut email = EmailDocument::new(
        "<style>#m{width:10px}@media (min-resolution: 2dppx){#m{width:20px}}</style><div id='v' style='height:100vh'></div><div id='m'>m</div>",
        true,
    );
    email.set_screen(900, 2.);
    email.layout(400);
    assert_eq!(bx(&email, "v").3, 900.);
    assert_eq!(bx(&email, "m").2, 20., "an email's retina rules apply");
    assert_eq!(bx(&email, "v").2, 376., "layout stays in CSS pixels");
}

#[test]
fn links_are_light_in_native_dark() {
    let mut email = EmailDocument::with_tone(
        "<body style='margin:0'><p id='p' style='margin:0;font-size:40px'><a href='https://example.com'>WWWW</a></p>",
        true,
        Tone::NativeDark,
    );
    email.layout(400);
    let lightest = colors_in(&mut email, "p")
        .into_iter()
        .map(luma)
        .fold(0., f32::max);
    assert!(lightest > 150., "{lightest}");
}

#[test]
fn words_of_unspaced_scripts_are_selected_and_wrapped() {
    let mut email = EmailDocument::new(
        "<body style='margin:0'><p id='p' style='margin:0;font-size:20px'>こんにちは世界</p>\
         <p id='th' style='margin:0;width:120px;font-size:16px'>ภาษาไทยเขียนติดกันโดยไม่มีช่องว่างระหว่างคำเลยครับ</p>",
        true,
    );
    email.layout(400);
    email.select_word_at(5., 10.);
    let word = email.doc.get_selected_text().unwrap();
    assert!(word.chars().count() < 7, "a word, not the line: {word}");
    assert!(
        bx(&email, "th").3 > 30.,
        "Thai wraps: {:?}",
        bx(&email, "th")
    );
}

#[test]
fn the_emails_own_scrollable_boxes_scroll() {
    let mut email = EmailDocument::new(
        "<body style='margin:0'><div id='s' style='height:100px;overflow:auto'><div style='height:500px'>Long</div></div>",
        true,
    );
    email.layout(400);
    let scrollers = email.scrollers();
    assert_eq!(scrollers.len(), 1);
    assert!(scrollers[0].down && !scrollers[0].up);
    email
        .doc
        .handle_ui_event(UiEvent::PointerMove(mouse(50., 50., false)));
    email.doc.handle_ui_event(UiEvent::Wheel(BlitzWheelEvent {
        delta: BlitzWheelDelta::Pixels(0., -60.),
        coords: mouse(50., 50., false).coords,
        buttons: MouseEventButtons::None,
        mods: Default::default(),
        element: Default::default(),
    }));
    email.resolve();
    let node = email
        .doc
        .get_node(email.doc.get_element_by_id("s").unwrap())
        .unwrap();
    assert!(node.scroll_offset().y > 0., "{:?}", node.scroll_offset());
    assert!(email.scrollers()[0].up);
}

#[test]
fn select_all_and_shift_click_select_text() {
    let mut email = EmailDocument::new(
        "<body style='margin:0;font-size:20px'><p id='a' style='margin:0'>First paragraph</p><p style='margin:0'>Middle</p><p id='z' style='margin:0'>Last words</p>",
        true,
    );
    email.layout(400);
    email.select_all();
    let all = email.doc.get_selected_text().unwrap();
    assert!(
        all.contains("First paragraph") && all.contains("Last words"),
        "{all}"
    );
    email.select_word_at(10., 10.);
    assert!(email.extend_selection_to(60., 50.));
    let extended = email.doc.get_selected_text().unwrap();
    assert!(
        extended.starts_with("First") && extended.contains("Middle"),
        "{extended}"
    );
}

#[test]
fn a_hash_link_and_tab_bring_their_targets_into_view() {
    let (commands, mut rx) = spawn(
        "<body style='margin:0'><a id='a' href='#sec' style='display:block;height:40px'>Jump</a><div style='height:2000px'></div><h2 id='sec' style='margin:0'>Section</h2><a href='https://example.com/end'>End</a>",
        Tone::Light,
    );
    wait_for(&mut rx, page_of).unwrap();
    for event in [
        UiEvent::PointerMove(mouse(10., 20., false)),
        UiEvent::PointerDown(mouse(10., 20., true)),
        UiEvent::PointerUp(mouse(10., 20., false)),
    ] {
        commands.send(Command::Pointer(event)).unwrap();
    }
    let reveal = wait_for(&mut rx, |o| match o {
        Output::Reveal { top, start, .. } => Some((top, start)),
        Output::Open(url) => panic!("a # link opened {url}"),
        _ => None,
    });
    assert_eq!(reveal, Some((2040., true)));
    let tab = BlitzKeyEvent {
        key: keyboard_types::Key::Named(keyboard_types::NamedKey::Tab),
        code: keyboard_types::Code::Tab,
        modifiers: keyboard_types::Modifiers::empty(),
        location: keyboard_types::Location::Standard,
        is_auto_repeating: false,
        is_composing: false,
        state: KeyState::Pressed,
        text: None,
    };
    commands.send(Command::Key(tab.clone())).unwrap();
    commands.send(Command::Key(tab)).unwrap();
    let reveal = wait_for(&mut rx, |o| match o {
        Output::Reveal { top, start, .. } if top > 2000. => Some(start),
        _ => None,
    });
    assert_eq!(
        reveal,
        Some(false),
        "Tab reaches the last link and shows it"
    );
}

#[test]
fn a_wide_email_is_painted_scaled_down() {
    let (commands, mut rx) = spawn(
        "<body style='margin:0'><table width=800 cellpadding=0 cellspacing=0><tr><td height=400 bgcolor='#ff0000'></td></tr></table>",
        Tone::Light,
    );
    let height = wait_for(&mut rx, |o| match o {
        Output::State { height, .. } => Some(height),
        _ => None,
    })
    .unwrap();
    assert_eq!(height, 200., "800 pixels shown in 400");
    commands
        .send(Command::Paint {
            band: 0,
            width: 400,
            scale: 1.,
        })
        .unwrap();
    let pixels = wait_for(&mut rx, |o| match o {
        Output::Tile { size, pixels, .. } => Some((size, pixels)),
        _ => None,
    })
    .unwrap();
    let ((pw, _), pixels) = pixels;
    let at = |x: usize, y: usize| &pixels[(y * pw as usize + x) * 4..][..3];
    assert_eq!(
        at(390, 190),
        [0, 0, 255],
        "the table's right end is shown (BGRA)"
    );
}

#[test]
fn remote_resources_load_side_by_side_and_say_what_they_accept() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let headers = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let seen = headers.clone();
    std::thread::spawn(move || {
        for socket in listener.incoming() {
            let Ok(mut socket) = socket else { break };
            let seen = seen.clone();
            std::thread::spawn(move || {
                let mut request = [0; 4096];
                let len = socket.read(&mut request).unwrap_or(0);
                seen.lock()
                    .unwrap()
                    .push(String::from_utf8_lossy(&request[..len]).into_owned());
                std::thread::sleep(Duration::from_millis(300));
                let _ = write!(
                    socket,
                    "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok"
                );
            });
        }
    });
    let resources =
        super::resources::Resources::new(false, Default::default(), Arc::default(), Arc::default(), Arc::default());
    struct Done(std::sync::mpsc::Sender<usize>);
    impl blitz_traits::net::NetHandler for Done {
        fn bytes(self: Box<Self>, _: String, bytes: blitz_traits::net::Bytes) {
            let _ = self.0.send(bytes.len());
        }
    }
    use blitz_traits::net::NetProvider;
    let (tx, rx) = std::sync::mpsc::channel();
    let started = Instant::now();
    // More than the old limit of 64 requests.
    for i in 0..72 {
        let url = url::Url::parse(&format!("{origin}/{i}.png")).unwrap();
        resources.fetch(
            0,
            blitz_traits::net::Request::get(url),
            Box::new(Done(tx.clone())),
        );
    }
    let sizes: Vec<usize> = (0..72)
        .map(|_| rx.recv_timeout(Duration::from_secs(20)).unwrap())
        .collect();
    assert!(sizes.iter().all(|&n| n == 2), "every image loads");
    assert!(
        started.elapsed() < Duration::from_secs(8),
        "{:?}",
        started.elapsed()
    );
    let request = headers.lock().unwrap()[0].to_ascii_lowercase();
    assert!(
        request.contains("user-agent: mozilla/5.0 (compatible; picacho/"),
        "{request}"
    );
    assert!(request.contains("accept: image/webp"), "{request}");
}

/// Text zoom: the worker takes and gives the view's pixels and works in the
/// document's, so painting, link clicks, selection, focus, scrollable boxes
/// and the email's height all move together, by the zoom applied once.
/// A zoom applied twice or not at all puts each probe on another element.
#[test]
fn text_zoom_maps_paint_hits_selection_and_extents_through_one_factor() {
    // In CSS pixels: a 100px red block with three words, a 40px green link
    // below it, then a 100px box that scrolls 200px.
    let html = "<body style='margin:0'>\
        <div id='red' style='height:100px;background:#ff0000'>\
        <span id='words' style='display:inline-block;font:20px/30px monospace'>alpha beta gamma</span></div>\
        <a id='link' href='https://example.com/zoomed' style='display:block;height:40px;background:#00ff00'>Link</a>\
        <div id='scroll' style='height:100px;overflow:auto'><div style='height:300px'></div></div>";
    for zoom in [1.5f32, 2.] {
        // The document alone: laid out narrower by the zoom, then scaled up.
        let mut email = EmailDocument::new(html, true);
        email.set_text_zoom(zoom);
        let css_height = email.layout(600);
        assert_eq!(css_height, 240., "{zoom}");
        assert_eq!(email.zoom(), zoom, "shown at the text zoom");
        assert_eq!(bx(&email, "red").2, 600. / zoom, "laid out 600 / zoom wide");
        let char_width = bx(&email, "words").2 / 16.;
        assert!(char_width > 5., "the words are laid out: {char_width}");

        let (tx, mut rx) = futures::channel::mpsc::unbounded();
        let commands = worker::spawn(html.into(), Default::default(), true, Tone::Light, 1, tx);
        commands.send(Command::TextZoom(zoom)).unwrap();
        commands
            .send(Command::Layout {
                width: 600,
                height: 800,
                scale: 2.,
            })
            .unwrap();
        // The email's height and its scrollable box, in the view's pixels.
        let (width, height, scrollers) = wait_for(&mut rx, |o| match o {
            Output::State {
                width,
                height,
                scrollers,
                ..
            } => Some((width, height, scrollers)),
            _ => None,
        })
        .unwrap();
        assert_eq!(width, 600, "{zoom}");
        assert_eq!(
            height,
            240. * zoom,
            "{zoom}: the reader scrolls the zoomed height"
        );
        assert_eq!(scrollers.len(), 1, "{zoom}");
        let s = &scrollers[0];
        assert_eq!(
            (s.x, s.y, s.width, s.height),
            (0., 140. * zoom, 600., 100. * zoom),
            "{zoom}: the scrollable box where it is painted"
        );

        // Painting: at window scale 2, the red-to-green edge at 100 CSS
        // pixels is at 100 * zoom view pixels, 200 * zoom device pixels.
        commands
            .send(Command::Paint {
                band: 0,
                width: 600,
                scale: 2.,
            })
            .unwrap();
        let ((pw, _), pixels) = wait_for(&mut rx, |o| match o {
            Output::Tile { size, pixels, .. } => Some((size, pixels)),
            _ => None,
        })
        .unwrap();
        let bgr = |x: f32, y: f32| {
            let at = (y.round() as usize * pw as usize + x.round() as usize) * 4;
            [pixels[at], pixels[at + 1], pixels[at + 2]]
        };
        let edge = 2. * 100. * zoom;
        assert_eq!(
            bgr(1100., edge - 3.),
            [0, 0, 255],
            "{zoom}: red above the edge (BGRA)"
        );
        assert_eq!(bgr(1100., edge + 3.), [0, 255, 0], "{zoom}: green below it");

        // A click 20 CSS pixels into the link, in view pixels, opens it.
        let (x, y) = (300., (100. + 20.) * zoom);
        for event in [
            UiEvent::PointerMove(mouse(x, y, false)),
            UiEvent::PointerDown(mouse(x, y, true)),
            UiEvent::PointerUp(mouse(x, y, false)),
        ] {
            commands.send(Command::Pointer(event)).unwrap();
        }
        let opened = wait_for(&mut rx, |o| match o {
            Output::Open(url) => Some(url),
            _ => None,
        });
        assert_eq!(
            opened.as_deref(),
            Some("https://example.com/zoomed"),
            "{zoom}"
        );

        // A long press on "beta" (characters 6 to 9) selects it.
        commands
            .send(Command::SelectWord {
                x: 7.5 * char_width * zoom,
                y: 15. * zoom,
            })
            .unwrap();
        let selected = wait_for(&mut rx, |o| match o {
            Output::State {
                selection: Some(s), ..
            } => Some(s),
            _ => None,
        });
        assert_eq!(selected.as_deref(), Some("beta"), "{zoom}");

        // Tab to the link: it is brought into view where it is painted.
        commands
            .send(Command::Key(BlitzKeyEvent {
                key: keyboard_types::Key::Named(keyboard_types::NamedKey::Tab),
                code: keyboard_types::Code::Tab,
                modifiers: keyboard_types::Modifiers::empty(),
                location: keyboard_types::Location::Standard,
                is_auto_repeating: false,
                is_composing: false,
                state: KeyState::Pressed,
                text: None,
            }))
            .unwrap();
        let reveal = wait_for(&mut rx, |o| match o {
            Output::Reveal { top, bottom, .. } => Some((top, bottom)),
            _ => None,
        });
        assert_eq!(reveal, Some((100. * zoom, 140. * zoom)), "{zoom}");

        // The wheel over the box scrolls it by view pixels: 190 of its 200
        // CSS pixels leave it short of its end, and 10 more reach it.
        let (x, y) = (300., 190. * zoom);
        commands
            .send(Command::Pointer(UiEvent::PointerMove(mouse(x, y, false))))
            .unwrap();
        let down_after = |rx: &mut _, dy: f32| {
            commands.send(Command::Wheel { x, y, dx: 0., dy }).unwrap();
            wait_for(rx, |o| match o {
                Output::State { scrollers, .. } => scrollers.first().map(|s| (s.up, s.down)),
                _ => None,
            })
            .unwrap()
        };
        assert_eq!(down_after(&mut rx, -190. * zoom), (true, true), "{zoom}");
        assert_eq!(down_after(&mut rx, -10. * zoom), (true, false), "{zoom}");
    }
}

/// Interactive ("kinetic") email: hidden checkboxes and radio buttons toggled
/// by their labels, with `:checked` rules showing what was chosen: tabs,
/// carousels, accordions and reveals. The inputs themselves stay hidden
/// (`READER_CSS`), but Blitz keeps their state, follows a label to its input
/// (HTML §4.10.4) and matches `:checked` (Selectors 4 §14.3.1).
#[test]
fn labels_toggle_hidden_inputs_and_checked_styles_follow() {
    let html = "<style>\
        .panel { display: none; height: 40px }\
        #a:checked ~ #pa, #b:checked ~ #pb { display: block }\
        #pa { background: #ff0000 } #pb { background: #0000ff }\
        #more + #extra { height: 0; overflow: hidden }\
        #more:checked + #extra { height: 50px; background: #00ff00 }\
        </style><body style='margin:0'>\
        <input type='radio' name='tabs' id='a' checked><input type='radio' name='tabs' id='b'>\
        <label for='a' style='display:block;height:30px'>A</label>\
        <label for='b' style='display:block;height:30px'>B</label>\
        <div class='panel' id='pa'></div><div class='panel' id='pb'></div>\
        <label style='display:block'><span style='display:block;height:30px'>More</span>\
        <input type='checkbox' id='more'><div id='extra'></div></label>";
    let (commands, mut rx) = spawn(html, Tone::Light);
    let height = |rx: &mut _| {
        wait_for(rx, |o| match o {
            Output::State { height, .. } => Some(height),
            _ => None,
        })
        .unwrap()
    };
    // Tab A's panel shows; the extra box is closed.
    let closed = height(&mut rx);
    assert_eq!(closed, 130.);
    let click = |y: f32| {
        for event in [
            UiEvent::PointerMove(mouse(10., y, false)),
            UiEvent::PointerDown(mouse(10., y, true)),
            UiEvent::PointerUp(mouse(10., y, false)),
        ] {
            commands.send(Command::Pointer(event)).unwrap();
        }
    };
    let paint = |rx: &mut _, y: usize| {
        commands
            .send(Command::Paint {
                band: 0,
                width: 400,
                scale: 1.,
            })
            .unwrap();
        let ((pw, _), pixels) = wait_for(rx, |o| match o {
            Output::Tile { size, pixels, .. } => Some((size, pixels)),
            _ => None,
        })
        .unwrap();
        let at = (y * pw as usize + 10) * 4;
        [pixels[at + 2], pixels[at + 1], pixels[at]]
    };
    assert_eq!(paint(&mut rx, 70), [255, 0, 0], "tab A's panel");
    // A label shows it can be clicked.
    commands
        .send(Command::Pointer(UiEvent::PointerMove(mouse(10., 45., false))))
        .unwrap();
    let cursor = wait_for(&mut rx, |o| match o {
        Output::State { cursor, .. } => Some(cursor),
        _ => None,
    });
    assert_eq!(cursor, Some(gpui::CursorStyle::PointingHand));
    // Tab B's label: its radio is checked, A's is not, B's panel shows.
    click(45.);
    let mut tab_b = None;
    for _ in 0..3 {
        if paint(&mut rx, 70) == [0, 0, 255] {
            tab_b = Some(());
            break;
        }
    }
    assert!(tab_b.is_some(), "tab B's panel replaces A's");
    // The label around a checkbox opens the box after it.
    click(115.);
    let opened = (0..6).any(|_| {
        wait_for(&mut rx, |o| match o {
            Output::State { height, .. } => Some(height),
            _ => None,
        }) == Some(closed + 50.)
    });
    assert!(opened, "the checkbox's label opens the box");
    assert_eq!(paint(&mut rx, 150), [0, 255, 0]);
}

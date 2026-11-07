//! Development-only driver for visual checks (debug macOS builds).
//!
//! Set `PICACHO_DEV_DIR` to a folder, then append lines to `<dir>/cmd`:
//! `snap <name> <window-number>` saves `<dir>/<name>.png` of this window,
//! `size <w> <h>`, `syncing` (toggles the sync spinner), `action <name>`, `select <index>`, `scroll <index>`,
//! `drag <sidebar|reader> <from-x> <to-x>`, `log <name>`,
//! `rscroll <y|end>`, `mouse <move|down|up|click|dclick|rclick|drag> <x> <y>` (window coordinates)
//! and `eml <path>` (opens a message file at the top of the list). End a line with
//! ` &` to run the next one without waiting. Capturing its own window needs no
//! screen-recording permission. Release builds do not include this module.
use super::*;

pub(super) fn start(window: &mut Window, cx: &mut Context<MailApp>) {
    let Some(dir) = std::env::var_os("PICACHO_DEV_DIR").map(std::path::PathBuf::from) else {
        return;
    };
    cx.spawn_in(window, async move |this, cx| {
        loop {
            Timer::after(Duration::from_millis(150)).await;
            let path = dir.join("cmd");
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let _ = std::fs::remove_file(&path);
            for line in text.lines() {
                let words: Vec<&str> = line.split_whitespace().filter(|w| *w != "&").collect();
                let dir = dir.clone();
                let result = if words.first() == Some(&"mouse") {
                    // AppKit delivers the event synchronously; GPUI ignores
                    // input that arrives while the app is being updated.
                    this.update_in(cx, |_, w, _| f32::from(w.viewport_size().height))
                        .map(|height| mouse(&words, height))
                } else {
                    this.update_in(cx, |this, w, cx| run(this, &words, &dir, w, cx))
                };
                if result.is_err() {
                    return;
                }
                // A trailing `&` runs the next line right away (mid-animation).
                if !line.trim_end().ends_with('&') {
                    Timer::after(Duration::from_millis(400)).await;
                } else {
                    Timer::after(Duration::from_millis(90)).await;
                }
            }
        }
    })
    .detach();
}

fn run(
    this: &mut MailApp,
    words: &[&str],
    dir: &std::path::Path,
    w: &mut Window,
    cx: &mut Context<MailApp>,
) {
    let number = |i: usize| {
        words
            .get(i)
            .and_then(|s| s.parse::<f32>().ok())
            .unwrap_or(0.)
    };
    match words.first().copied() {
        Some("snap") => {
            let name = words.get(1).copied().unwrap_or("snap");
            let id = match number(2) as u32 {
                0 => key_window(),
                id => id,
            };
            let result = capture(id, &dir.join(format!("{name}.png")));
            let _ = std::fs::write(dir.join(format!("{name}.log")), format!("{result:?}"));
        }
        Some("log") => {
            let name = words.get(1).copied().unwrap_or("log");
            let offset = this.list_scroll.0.borrow().base_handle.offset();
            let _ = std::fs::write(
                dir.join(format!("{name}.txt")),
                format!("offset={offset:?} selected={:?}", this.selected),
            );
        }
        // Shows the list's sync spinner, as during a long sync.
        Some("syncing") => this.syncing = !this.syncing,
        // Writes this app's window numbers, for `snap <name> <number>`.
        Some("windows") => {
            let _ = std::fs::write(dir.join("windows.txt"), window_numbers());
        }
        Some("size") => w.resize(size(px(number(1)), px(number(2)))),
        Some("select") => {
            if let Some(i) = this.visible.get(number(1) as usize) {
                let id = this.messages[*i].id.clone();
                this.handle(Action::Select(id), w, cx);
            }
        }
        Some("scroll") => this
            .list_scroll
            .scroll_to_item(number(1) as usize, ScrollStrategy::Top),
        Some("rscroll") => {
            // Scrolls the reader; `end` goes to the bottom.
            if words.get(1) == Some(&"end") {
                this.reader_scroll.scroll_to_bottom();
            } else {
                this.reader_scroll.set_offset(point(px(0.), px(-number(1))));
            }
        }
        Some("drag") => {
            let pane = if words.get(1) == Some(&"reader") {
                Pane::Reader
            } else {
                Pane::Sidebar
            };
            this.begin_resize(pane, number(2), cx);
            this.resize_to(number(3), cx);
            this.end_resize(cx);
        }
        Some("action") => {
            let action = match words.get(1).copied() {
                Some("sidebar") => Action::ToggleSidebar,
                Some("reader") => Action::ToggleReader,
                Some("settings") => Action::Settings,
                Some("account") => match this.accounts.first() {
                    Some(a) => Action::AccountSettings(a.id.clone()),
                    None => return,
                },
                Some("close") => Action::Close,
                Some("compose") => Action::Compose,
                Some("reply") => Action::Reply(false),
                Some("replyall") => Action::Reply(true),
                Some("forward") => Action::Forward,
                Some("composing") => Action::SettingsPage(Page::Composing),
                Some("theme") => Action::Theme,
                Some("details") => Action::ToggleDetails,
                Some("original") => Action::ShowOriginal,
                Some("remote") => Action::ToggleRemote,
                Some("emailtheme") => Action::ToggleEmailTheme,
                Some("plain") => Action::ShowHtml(false),
                Some("formatted") => Action::ShowHtml(true),
                #[cfg(feature = "sample-data")]
                Some("sample") => Action::Sample,
                _ => return,
            };
            this.handle(action, w, cx);
        }
        Some("eml") => {
            let Some(raw) = words.get(1).and_then(|p| std::fs::read(p).ok()) else {
                return;
            };
            let mut account = crate::manage::model::Account::new(Provider::Imap);
            account.id = "demo".into();
            let id = format!("eml-{}", this.messages.len());
            if let Ok(mut mail) =
                crate::sync::mime::parse_message(&raw, &account, &id, Folder::Inbox, "INBOX", 0)
            {
                mail.date = chrono::Utc::now().timestamp();
                let details = crate::sync::mime::details(&raw);
                this.details = Some(MessageInfo {
                    id: id.clone(),
                    result: Some(Ok(Rc::new(Original::new(details, &raw)))),
                });
                this.messages.insert(0, mail.clone());
                this.demo.insert(0, mail);
                this.filter(cx);
            }
        }
        _ => {}
    }
    cx.notify();
}

fn capture(window: u32, path: &std::path::Path) -> anyhow::Result<()> {
    use core_graphics::geometry::CGRect;
    use core_graphics::geometry::{CGPoint, CGSize};
    use core_graphics::window::{
        create_image, kCGWindowImageBoundsIgnoreFraming, kCGWindowListOptionIncludingWindow,
    };
    // CGRectNull: the window's own bounds.
    let null = CGRect::new(
        &CGPoint::new(f64::INFINITY, f64::INFINITY),
        &CGSize::new(0., 0.),
    );
    let image = create_image(
        null,
        kCGWindowListOptionIncludingWindow,
        window,
        kCGWindowImageBoundsIgnoreFraming,
    )
    .ok_or_else(|| anyhow::anyhow!("no image"))?;
    let (width, height, row) = (image.width(), image.height(), image.bytes_per_row());
    let data = image.data();
    let bytes = data.bytes();
    let mut rgba = Vec::with_capacity(width * height * 4);
    for y in 0..height {
        for x in 0..width {
            let p = &bytes[y * row + x * 4..y * row + x * 4 + 4];
            // BGRA, premultiplied.
            rgba.extend_from_slice(&[p[2], p[1], p[0], p[3]]);
        }
    }
    image::RgbaImage::from_raw(width as u32, height as u32, rgba)
        .ok_or_else(|| anyhow::anyhow!("bad image"))?
        .save(path)?;
    Ok(())
}

fn mouse(words: &[&str], height: f32) {
    let number = |i: usize| {
        words
            .get(i)
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(0.)
    };
    let (x, y) = (number(2), height as f64 - number(3));
    let kinds: &[(u64, i64)] = match words.get(1).copied() {
        Some("down") => &[(1, 1)],
        Some("up") => &[(2, 1)],
        Some("click") => &[(1, 1), (2, 1)],
        Some("dclick") => &[(1, 1), (2, 1), (1, 2), (2, 2)],
        Some("rclick") => &[(3, 1), (4, 1)],
        // Moves with the left button held, as while dragging.
        Some("drag") => {
            send_mouse(6, 1, x, y);
            return;
        }
        _ => &[],
    };
    // Move first so hover state is current, like a real pointer.
    send_mouse(5, 0, x, y);
    for (kind, count) in kinds {
        send_mouse(*kind, *count, x, y);
    }
}

/// Sends an Objective-C message (without objc's macros, which check a cfg
/// this crate does not declare).
unsafe fn send<A: objc::MessageArguments, R: std::any::Any>(
    target: *mut objc::runtime::Object,
    selector: &str,
    args: A,
) -> R {
    use objc::Message;
    unsafe {
        (*target)
            .send_message(objc::runtime::Sel::register(selector), args)
            .expect(selector)
    }
}
fn class(name: &str) -> *mut objc::runtime::Object {
    objc::runtime::Class::get(name).expect(name) as *const _ as *mut _
}

/// Every window of this app, frontmost first, one number per line.
fn window_numbers() -> String {
    let mut out = String::new();
    unsafe {
        let app: *mut objc::runtime::Object = send(class("NSApplication"), "sharedApplication", ());
        let windows: *mut objc::runtime::Object = send(app, "orderedWindows", ());
        let count: usize = send(windows, "count", ());
        for i in 0..count {
            let window: *mut objc::runtime::Object = send(windows, "objectAtIndex:", (i,));
            let number: isize = send(window, "windowNumber", ());
            out.push_str(&format!("{number}\n"));
        }
    }
    out
}

/// The frontmost window of this app.
fn key_window() -> u32 {
    unsafe {
        let app: *mut objc::runtime::Object = send(class("NSApplication"), "sharedApplication", ());
        let windows: *mut objc::runtime::Object = send(app, "windows", ());
        let window: *mut objc::runtime::Object = send(windows, "firstObject", ());
        if window.is_null() {
            return 0;
        }
        let number: isize = send(window, "windowNumber", ());
        number as u32
    }
}

/// Delivers a mouse event to this app's window through AppKit, so it takes
/// the same path as a real pointer. `y` counts up from the window's bottom.
fn send_mouse(kind: u64, clicks: i64, x: f64, y: f64) {
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct NSPoint {
        x: f64,
        y: f64,
    }
    unsafe impl objc::Encode for NSPoint {
        fn encode() -> objc::Encoding {
            unsafe { objc::Encoding::from_str("{CGPoint=dd}") }
        }
    }
    let window = key_window() as isize;
    unsafe {
        let null: *mut objc::runtime::Object = std::ptr::null_mut();
        let event: *mut objc::runtime::Object = send(
            class("NSEvent"),
            "mouseEventWithType:location:modifierFlags:timestamp:windowNumber:context:eventNumber:clickCount:pressure:",
            (
                kind,
                NSPoint { x, y },
                0u64,
                0f64,
                window,
                null,
                0isize,
                clicks as isize,
                if kind == 5 { 0f32 } else { 1f32 },
            ),
        );
        if event.is_null() {
            return;
        }
        // AppKit leaves the button number unset on synthesized secondary
        // clicks; GPUI reads it, so set it on the underlying CGEvent.
        let event = if matches!(kind, 3 | 4) {
            unsafe extern "C" {
                fn CGEventSetIntegerValueField(
                    event: *mut std::ffi::c_void,
                    field: u32,
                    value: i64,
                );
            }
            let cg: *mut std::ffi::c_void = send(event, "CGEvent", ());
            CGEventSetIntegerValueField(cg, 3, 1); // kCGMouseEventButtonNumber
            let copy: *mut objc::runtime::Object =
                send(class("NSEvent"), "eventWithCGEvent:", (cg,));
            if copy.is_null() { event } else { copy }
        } else {
            event
        };
        let app: *mut objc::runtime::Object = send(class("NSApplication"), "sharedApplication", ());
        let window: *mut objc::runtime::Object = send(app, "windowWithWindowNumber:", (window,));
        if !window.is_null() {
            // The window's own dispatch skips AppKit's inactive-app filtering.
            let _: () = send(window, "sendEvent:", (event,));
        }
    }
}

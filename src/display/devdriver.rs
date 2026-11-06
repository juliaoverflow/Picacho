//! Development-only driver for visual checks (debug macOS builds).
//!
//! Set `PICACHO_DEV_DIR` to a folder, then append lines to `<dir>/cmd`:
//! `snap <name> <window-number>` saves `<dir>/<name>.png` of this window,
//! `size <w> <h>`, `action <name>`, `select <index>`, `scroll <index>`,
//! `drag <sidebar|reader> <from-x> <to-x>` and `catalog`. Capturing its own window needs no
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
                let words: Vec<&str> = line.split_whitespace().collect();
                let dir = dir.clone();
                let result = this.update_in(cx, |this, w, cx| run(this, &words, &dir, w, cx));
                if result.is_err() {
                    return;
                }
                Timer::after(Duration::from_millis(400)).await;
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
            let id = number(2) as u32;
            let result = capture(id, &dir.join(format!("{name}.png")));
            let _ = std::fs::write(dir.join(format!("{name}.log")), format!("{result:?}"));
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
                Some("theme") => Action::Theme,
                _ => return,
            };
            this.handle(action, w, cx);
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

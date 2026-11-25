//! What can be done with an open message beyond reading it: finding text
//! in it (⌘F / Ctrl+F), printing it or saving it as a vector PDF, and opening
//! its attachments (from their saved file or one cached copy, with the system
//! app, Quick Look on macOS, Open With… on Windows/macOS/Linux, and image
//! previews in the reader).
use super::*;
use crate::display::open_with::OpenWithApp;

#[derive(Clone, Debug, PartialEq)]
pub(super) enum ToolAction {
    /// Prints a message (its id) as a vector PDF through the system's native
    /// print dialog.
    Print(String),
    /// Saves a message (its id) as a vector PDF with its formatting, images
    /// and fonts.
    SavePdf(String),
    /// The open message's attachment, by index: opened with the system's
    /// app for its type; shown with Quick Look (macOS); offered to the
    /// apps that open it (Open With…); shown in its folder.
    OpenAttachment(usize),
    QuickLook(usize),
    OpenWith(usize),
    /// Opens an attachment file in a specific app chosen from Open With….
    OpenAttachmentIn(std::path::PathBuf, OpenWithApp),
    RevealAttachment(usize),
    /// Shows an attached image in the reader, or hides it again.
    PreviewImage(usize),
    FindOpen,
    FindNext,
    FindPrevious,
    FindClose,
    /// Publishes your OpenPGP public keys on keys.openpgp.org, which
    /// emails each address a link to confirm it.
    PublishKeys,
    /// Publishes your OpenPGP public keys on the HKP keyserver typed in
    /// Settings → Encryption (keyserver.ubuntu.com when none is), once the
    /// person confirms: such servers list every address at once, for good.
    PublishKeysCustomHkp,
    /// Exports your OpenPGP public keys into a Web Key Directory (WKD) folder
    /// tree ready to host on your own domain.
    ExportWkd,
    /// Makes Picacho the default email app (or opens Windows Default apps
    /// directly on Picacho's page).
    SetDefaultApp,
}

/// The HKP keyserver keys are published on when none is typed.
const DEFAULT_HKP_SERVER: &str = "keyserver.ubuntu.com";

/// Find in Message (and menu anchor for Open With…, plus Settings helpers).
pub(super) struct Find {
    pub(super) open: bool,
    pub(super) input: Entity<InputState>,
    /// The match shown as current, from 0.
    pub(super) current: usize,
    /// Matches in the open message, counted as it was last drawn.
    pub(super) count: Cell<usize>,
    /// The current match should be scrolled to.
    pub(super) reveal: Cell<bool>,
    /// Where the last attachment menu opened, so an Open With… picker can
    /// open at the same spot.
    pub(super) menu_pos: Option<Point<Pixels>>,
    /// Custom HKP keyserver host/URL in Settings → Encryption.
    pub(super) hkp_server: Entity<InputState>,
    /// Cached result of checking whether Picacho is the OS default email app.
    pub(super) default_mail_app: Option<bool>,
}
impl Find {
    pub(super) fn new(window: &mut Window, cx: &mut App) -> Self {
        Self {
            open: false,
            input: input(t!("tools-find-in-message"), false, window, cx),
            current: 0,
            count: Cell::new(0),
            reveal: Cell::new(false),
            menu_pos: None,
            hkp_server: input(DEFAULT_HKP_SERVER, false, window, cx),
            default_mail_app: None,
        }
    }
}

/// The id of the `n`th match's mark.
fn mark_id(n: usize) -> String {
    format!("picacho-find-{n}")
}

/// Whether `tag` (a lowercase name) holds text that isn't shown.
fn hidden_tag(tag: &str) -> bool {
    matches!(tag, "script" | "style" | "title" | "head" | "template" | "noscript")
}

/// Positions of `query` (lowercase) in `text`, ignoring case where that
/// keeps byte offsets.
fn find_in(text: &str, query: &str) -> Vec<usize> {
    if query.is_empty() {
        return Vec::new();
    }
    let lower = text.to_lowercase();
    let hay = if lower.len() == text.len() { lower.as_str() } else { text };
    let mut found = Vec::new();
    let mut from = 0;
    while let Some(i) = hay[from..].find(query) {
        let at = from + i;
        // On character boundaries of the original.
        if text.is_char_boundary(at) && text.is_char_boundary(at + query.len()) {
            found.push(at);
        }
        from = at + query.len().max(1);
        while from < hay.len() && !hay.is_char_boundary(from) {
            from += 1;
        }
    }
    found
}

/// `html` with each match of `query` (lowercase) in its text marked, the
/// `current`th apart, and how many there are.
pub(super) fn mark_html(html: &str, query: &str, current: usize) -> (String, usize) {
    let mut out = String::with_capacity(html.len() + 256);
    let mut n = 0;
    let mut rest = html;
    let mut hidden: Option<String> = None;
    while !rest.is_empty() {
        if let Some(stripped) = rest.strip_prefix('<') {
            // A comment, a tag: copied whole.
            let end = if stripped.starts_with("!--") {
                rest.find("-->").map(|e| e + 3)
            } else {
                rest.find('>').map(|e| e + 1)
            }
            .unwrap_or(rest.len());
            let tag = &rest[..end];
            let name: String = tag
                .trim_start_matches('<')
                .trim_start_matches('/')
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric())
                .collect::<String>()
                .to_ascii_lowercase();
            if tag.starts_with("</") {
                if hidden.as_deref() == Some(name.as_str()) {
                    hidden = None;
                }
            } else if hidden.is_none() && hidden_tag(&name) && !tag.ends_with("/>") {
                hidden = Some(name);
            }
            out.push_str(tag);
            rest = &rest[end..];
            continue;
        }
        let end = rest.find('<').unwrap_or(rest.len());
        let text = &rest[..end];
        if hidden.is_some() {
            out.push_str(text);
        } else {
            let mut last = 0;
            for at in find_in(text, query) {
                out.push_str(&text[last..at]);
                let color = if n == current { "#ff9632" } else { "#ffe066" };
                out.push_str(&format!(
                    "<mark id=\"{}\" style=\"background:{color}!important;color:#000!important;border-radius:2px\">",
                    mark_id(n)
                ));
                out.push_str(&text[at..at + query.len()]);
                out.push_str("</mark>");
                last = at + query.len();
                n += 1;
            }
            out.push_str(&text[last..]);
        }
        rest = &rest[end..];
    }
    (out, n)
}

pub(super) fn escape_html(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 16);
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(ch),
        }
    }
    out
}

/// Plain text as HTML that keeps its lines, for Find in Message.
pub(super) fn plain_as_html(text: &str) -> String {
    format!(
        "<div style=\"white-space:pre-wrap;font-family:system-ui,-apple-system,'Segoe UI',sans-serif;font-size:14px;line-height:1.7\">{}</div>",
        escape_html(text)
    )
}

/// Maximum width/height when rasterizing SVG or decoding non-native images
/// for inline attachment previews.
const MAX_PREVIEW_DIM: u32 = 8192;
/// Maximum byte allocation when decoding a non-native image preview.
const MAX_PREVIEW_ALLOC: u64 = 64 * 1024 * 1024;
/// Maximum pixel edge when rasterizing an SVG preview.
const MAX_SVG_DIM: f32 = 1600.;

fn is_svg(mime: &str, name: &str, bytes: Option<&[u8]>) -> bool {
    let mime = mime.to_ascii_lowercase();
    let name = name.to_ascii_lowercase();
    if mime == "image/svg+xml" || name.ends_with(".svg") {
        return true;
    }
    bytes.is_some_and(|b| {
        let head = std::str::from_utf8(&b[..b.len().min(512)]).unwrap_or("");
        let lower = head.trim_start().to_ascii_lowercase();
        lower.starts_with("<svg") || (lower.starts_with("<?xml") && lower.contains("<svg"))
    })
}

/// Whether an attachment is an image the reader can preview inline.
pub(super) fn is_image(a: &Attachment) -> bool {
    let mime = a.mime.to_ascii_lowercase();
    let name = a.name.to_ascii_lowercase();
    let base = matches!(
        mime.as_str(),
        "image/png"
            | "image/jpeg"
            | "image/jpg"
            | "image/gif"
            | "image/webp"
            | "image/svg+xml"
            | "image/bmp"
            | "image/x-ms-bmp"
            | "image/x-icon"
            | "image/vnd.microsoft.icon"
            | "image/tiff"
            | "image/x-tga"
            | "image/tga"
            | "image/x-portable-anymap"
            | "image/x-portable-bitmap"
            | "image/x-portable-graymap"
            | "image/x-portable-pixmap"
            | "image/qoi"
            | "image/vnd.radiance"
    ) || [
        ".png", ".jpg", ".jpeg", ".gif", ".webp", ".svg", ".bmp", ".ico", ".tif", ".tiff",
        ".tga", ".pbm", ".pgm", ".ppm", ".pnm", ".pam", ".qoi", ".hdr",
    ]
    .iter()
    .any(|e| name.ends_with(e));
    #[cfg(target_os = "macos")]
    {
        base || matches!(mime.as_str(), "image/heic" | "image/heif" | "image/avif")
            || [".heic", ".heif", ".avif"].iter().any(|e| name.ends_with(e))
    }
    #[cfg(not(target_os = "macos"))]
    {
        base
    }
}

/// Decodes any supported image attachment into a GPUI-renderable `(ImageFormat, Vec<u8>)`.
///
/// PNG, JPEG, GIF and WebP pass straight through (preserving animation);
/// SVG is rasterized offline with `resvg` (no external fetches); BMP, ICO,
/// TIFF, TGA, PNM, QOI and HDR are decoded with strict dimension/memory
/// limits and encoded as PNG.
fn decode_image_preview(bytes: Vec<u8>, mime: &str, name: &str) -> anyhow::Result<(ImageFormat, Vec<u8>)> {
    if is_svg(mime, name, Some(&bytes)) {
        // An SVG from mail loads nothing it names: no file (usvg's default
        // reads any path an `<image>` names, even with no resources folder)
        // and nothing remote; only images inside it (data: URLs). Its text
        // is set in the system's fonts, read once.
        static FONTS: std::sync::OnceLock<std::sync::Arc<resvg::usvg::fontdb::Database>> =
            std::sync::OnceLock::new();
        let fontdb = FONTS
            .get_or_init(|| {
                let mut db = resvg::usvg::fontdb::Database::new();
                db.load_system_fonts();
                std::sync::Arc::new(db)
            })
            .clone();
        let opt = resvg::usvg::Options {
            resources_dir: None,
            fontdb,
            image_href_resolver: resvg::usvg::ImageHrefResolver {
                resolve_data: resvg::usvg::ImageHrefResolver::default_data_resolver(),
                resolve_string: Box::new(|_, _| None),
            },
            ..Default::default()
        };
        let tree = resvg::usvg::Tree::from_data(&bytes, &opt)
            .map_err(|e| anyhow::anyhow!(t!("tools-couldnt-read-svg-image", error = e.to_string())))?;
        let size = tree.size();
        let (w, h) = (size.width().max(1.), size.height().max(1.));
        let scale = (MAX_SVG_DIM / w.max(h)).clamp(0.01, 2.);
        let pw = (w * scale).ceil().clamp(1., MAX_SVG_DIM) as u32;
        let ph = (h * scale).ceil().clamp(1., MAX_SVG_DIM) as u32;
        let mut pixmap = resvg::tiny_skia::Pixmap::new(pw, ph)
            .ok_or_else(|| anyhow::anyhow!(t!("tools-svg-image-size-is-invalid")))?;
        let tx = resvg::tiny_skia::Transform::from_scale(pw as f32 / w, ph as f32 / h);
        resvg::render(&tree, tx, &mut pixmap.as_mut());
        let png = pixmap
            .encode_png()
            .map_err(|e| anyhow::anyhow!(t!("tools-couldnt-encode-svg-preview", error = e.to_string())))?;
        return Ok((ImageFormat::Png, png));
    }

    if let Ok(fmt) = image::guess_format(&bytes) {
        match fmt {
            image::ImageFormat::Png => return Ok((ImageFormat::Png, bytes)),
            image::ImageFormat::Gif => return Ok((ImageFormat::Gif, bytes)),
            image::ImageFormat::WebP => return Ok((ImageFormat::Webp, bytes)),
            image::ImageFormat::Jpeg => return Ok((ImageFormat::Jpeg, bytes)),
            _ => {}
        }
    }

    // Try decoding via `image` (BMP, ICO, TIFF, TGA, PNM, QOI, HDR, …)
    // using format detection from content, falling back to extension/MIME.
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_PREVIEW_DIM);
    limits.max_image_height = Some(MAX_PREVIEW_DIM);
    limits.max_alloc = Some(MAX_PREVIEW_ALLOC);

    let reader = image::ImageReader::new(std::io::Cursor::new(&bytes))
        .with_guessed_format()?;
    let mut reader = if reader.format().is_some() {
        reader
    } else if let Some(fmt) = image::ImageFormat::from_path(name)
        .ok()
        .or_else(|| image::ImageFormat::from_mime_type(mime))
    {
        let mut r = image::ImageReader::new(std::io::Cursor::new(&bytes));
        r.set_format(fmt);
        r
    } else {
        reader
    };
    reader.limits(limits);

    match reader.decode() {
        Ok(decoded) => {
            use image::ImageEncoder as _;
            let rgba = decoded.to_rgba8();
            let mut png = Vec::new();
            image::codecs::png::PngEncoder::new(&mut png).write_image(
                rgba.as_raw(),
                rgba.width(),
                rgba.height(),
                image::ExtendedColorType::Rgba8,
            )?;
            Ok((ImageFormat::Png, png))
        }
        Err(err) => {
            #[cfg(target_os = "macos")]
            if let Ok(png) = convert_with_sips(&bytes, name) {
                return Ok((ImageFormat::Png, png));
            }
            Err(anyhow::anyhow!(t!("tools-cant-preview-this-image", error = err.to_string())))
        }
    }
}

/// macOS fallback for Apple-native formats (HEIC, HEIF, AVIF) via `sips`.
#[cfg(target_os = "macos")]
fn convert_with_sips(bytes: &[u8], name: &str) -> anyhow::Result<Vec<u8>> {
    // Readable by this user alone: it holds an attachment of their mail.
    let dir = std::env::temp_dir().join(format!("picacho-sips-{}", uuid::Uuid::new_v4().simple()));
    {
        use std::os::unix::fs::DirBuilderExt as _;
        std::fs::DirBuilder::new().mode(0o700).create(&dir)?;
    }
    let src = dir.join(safe_file_name(name));
    let dst = dir.join("preview.png");
    std::fs::write(&src, bytes)?;
    let status = std::process::Command::new("/usr/bin/sips")
        .args(["-s", "format", "png"])
        .arg(&src)
        .arg("--out")
        .arg(&dst)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    let png = if status.is_ok_and(|s| s.success()) {
        std::fs::read(&dst).ok()
    } else {
        None
    };
    let _ = std::fs::remove_dir_all(&dir);
    png.ok_or_else(|| anyhow::anyhow!(t!("tools-sips-couldnt-convert-image")))
}

impl MailApp {
    /// What the find field holds, lowercase, while Find in Message is open.
    pub(super) fn find_query(&self, cx: &App) -> Option<String> {
        if !self.find.open {
            return None;
        }
        let q = Self::value(&self.find.input, cx).trim().to_lowercase();
        (!q.is_empty()).then_some(q)
    }

    /// The open message's HTML with the find matches marked, for the
    /// reader to show in place of `shown` (its HTML, or its text when it
    /// has none). Scrolls to the current match when asked.
    pub(super) fn find_marked(&self, shown: &str, plain: bool, cx: &App) -> Option<String> {
        let q = self.find_query(cx)?;
        let html = if plain { plain_as_html(shown) } else { shown.to_owned() };
        let (marked, n) = mark_html(&html, &escape_html(&q), self.find.current);
        self.find.count.set(n);
        Some(marked)
    }

    /// Scrolls to the current match if asked, once the email shows it.
    pub(super) fn reveal_find(&self, cx: &mut Context<Self>) {
        if self.find.reveal.get() && self.find.count.get() > 0 {
            self.find.reveal.set(false);
            let id = mark_id(self.find.current.min(self.find.count.get() - 1));
            self.html_view.update(cx, |v, _| v.reveal_element(id));
        }
    }

    /// The bar above the message while Find in Message is open.
    pub(super) fn find_bar(&self, cx: &Context<Self>) -> AnyElement {
        let c = self.colors();
        let count = self.find.count.get();
        let has_query = !Self::value(&self.find.input, cx).trim().is_empty();
        let status = if !has_query {
            String::new()
        } else if count == 0 {
            t!("tools-no-matches").into()
        } else {
            t!("tools-of", current = self.find.current.min(count - 1) + 1, count = count)
        };
        div()
            .id("find-bar")
            .debug_selector(|| "find-bar".into())
            .mb_3()
            .p_2()
            .flex()
            .items_center()
            .gap_2()
            .rounded_lg()
            .border_1()
            .border_color(rgb(c.border))
            .bg(rgb(c.raised))
            .child(icon_el("search", 14., c.muted))
            .child(div().flex_1().min_w_0().child(field(Input::new(&self.find.input), c, false)))
            .child(
                div()
                    .min_w(px(72.))
                    .text_size(px(12.))
                    .text_color(rgb(if has_query && count == 0 { c.danger } else { c.muted }))
                    .child(status),
            )
            .child(self.button(
                "find-previous",
                t!("tools-previous-match-enter"),
                "chevron-up",
                Action::Tools(ToolAction::FindPrevious),
                false,
                cx,
            ))
            .child(self.button(
                "find-next",
                t!("tools-next-match-enter"),
                "chevron-down",
                Action::Tools(ToolAction::FindNext),
                false,
                cx,
            ))
            .child(self.button(
                "find-close",
                t!("tools-close-esc"),
                "close",
                Action::Tools(ToolAction::FindClose),
                false,
                cx,
            ))
            .into_any_element()
    }

    /// A message by id, read whole (its HTML, text and inline images).
    fn with_full_message(
        &mut self,
        id: String,
        cx: &mut Context<Self>,
        then: impl FnOnce(&mut Self, Mail, &mut Context<Self>) + 'static,
    ) {
        let open = self
            .selected_mail()
            .filter(|m| m.id == id && (m.body.is_some() || m.html.is_some()));
        if let Some(m) = open {
            then(self, m, cx);
            return;
        }
        let Some(m) = self.messages.iter().find(|m| m.id == id).cloned() else {
            return;
        };
        let Some(a) = self.account_of(&m) else {
            then(self, m, cx);
            return;
        };
        self.task(
            cx,
            move |b| b.read(&a, &m),
            move |this, result: anyhow::Result<Mail>, cx| match result {
                Ok(m) => then(this, m, cx),
                Err(e) => this.notice(format!("{e:#}"), true, cx),
            },
        );
    }

    pub(super) fn tool_action(&mut self, action: ToolAction, w: &mut Window, cx: &mut Context<Self>) {
        match action {
            ToolAction::Print(id) => self.with_full_message(id, cx, |this, m, cx| {
                let remote = !this.reading_options(&m).block_remote;
                let from = if m.sender.is_empty() || m.sender == m.from {
                    m.from.clone()
                } else {
                    format!("{} <{}>", m.sender, m.from)
                };
                let date = date_full(m.date).to_string();
                let title = m.subject.clone();
                #[cfg(target_os = "macos")]
                let title_mac = title.clone();
                this.notice(t!("tools-preparing-to-print"), false, cx);
                this.task(
                    cx,
                    move |_| {
                        let pdf = crate::display::html::print::message_pdf(&m, &from, &date, remote)?;
                        // macOS shows its print dialog on the main thread, below.
                        #[cfg(target_os = "macos")]
                        return Ok((pdf, None));
                        #[cfg(not(target_os = "macos"))]
                        {
                            let how = crate::display::html::print::print_pdf(&pdf, &title)?;
                            Ok((Vec::new(), Some(how)))
                        }
                    },
                    move |this, result: anyhow::Result<(Vec<u8>, Option<crate::display::html::print::Printed>)>, cx| {
                        use crate::display::html::print::Printed;
                        match result {
                            #[cfg(target_os = "macos")]
                            Ok((pdf, None)) => {
                                if let Err(e) = crate::display::html::print::print_pdf_macos(&pdf, &title_mac) {
                                    this.notice(t!("tools-couldnt-print-the-message", e = format!("{:#}", e)), true, cx);
                                }
                            }
                            Ok((_, Some(Printed::Viewer))) => this.notice(
                                t!("tools-opened-the-message-as-a-pdf"),
                                false,
                                cx,
                            ),
                            Ok(_) => {}
                            Err(e) => this.notice(t!("tools-couldnt-print-the-message", e = format!("{:#}", e)), true, cx),
                        }
                    },
                );
            }),
            ToolAction::SavePdf(id) => self.with_full_message(id, cx, |this, m, cx| {
                let remote = !this.reading_options(&m).block_remote;
                let from = if m.sender.is_empty() || m.sender == m.from {
                    m.from.clone()
                } else {
                    format!("{} <{}>", m.sender, m.from)
                };
                let date = date_full(m.date).to_string();
                let name = safe_file_name(&format!(
                    "{}.pdf",
                    if m.subject.is_empty() { t!("tools-message") } else { &m.subject }
                ));
                let start = this.save_start();
                let picker = cx.prompt_for_new_path(&start, Some(&name));
                cx.spawn(async move |this, cx| {
                    if let Ok(Ok(Some(path))) = picker.await {
                        let _ = this.update(cx, |this, cx| {
                            this.notice(t!("tools-saving-pdf"), false, cx);
                            this.task(
                                cx,
                                move |_| {
                                    let pdf = crate::display::html::print::message_pdf(&m, &from, &date, remote)?;
                                    std::fs::write(&path, pdf)?;
                                    Ok(())
                                },
                                |this, result: anyhow::Result<()>, cx| match result {
                                    Ok(()) => this.notice(t!("tools-saved-as-pdf"), false, cx),
                                    Err(e) => this.notice(t!("tools-couldnt-save-the-pdf", e = format!("{:#}", e)), true, cx),
                                },
                            );
                        });
                    }
                })
                .detach();
            }),
            ToolAction::OpenAttachment(i)
            | ToolAction::QuickLook(i)
            | ToolAction::RevealAttachment(i) => {
                let how = action.clone();
                let Some(m) = self.selected_mail() else { return };
                let Some(a) = m.attachments().get(i).cloned() else { return };
                let Some(account) = self.account_of(&m) else { return };
                self.task(
                    cx,
                    move |b| b.attachment_file(&account, &m, a.part, &a.name),
                    move |this, result: anyhow::Result<std::path::PathBuf>, cx| match result {
                        Ok(path) => match how {
                            ToolAction::QuickLook(_) => quick_look(&path),
                            ToolAction::RevealAttachment(_) => cx.reveal_path(&path),
                            _ => cx.open_with_system(&path),
                        },
                        Err(e) => this.notice(format!("{e:#}"), true, cx),
                    },
                );
            }
            ToolAction::OpenWith(i) => {
                let Some(m) = self.selected_mail() else { return };
                let Some(a) = m.attachments().get(i).cloned() else { return };
                let Some(account) = self.account_of(&m) else { return };
                let mime = if !a.mime.is_empty() && a.mime != "application/octet-stream" {
                    a.mime.clone()
                } else {
                    mime_guess::from_path(&a.name)
                        .first_or_octet_stream()
                        .essence_str()
                        .to_owned()
                };
                self.task(
                    cx,
                    move |b| -> anyhow::Result<(std::path::PathBuf, Option<Vec<OpenWithApp>>)> {
                        let path = b.attachment_file(&account, &m, a.part, &a.name)?;
                        #[cfg(windows)]
                        {
                            // Windows' own Open With dialog lists the apps.
                            let _ = mime;
                            open_with_windows(&path);
                            Ok((path, None))
                        }
                        #[cfg(all(unix, not(any(target_os = "macos", target_os = "android", target_os = "ios"))))]
                        {
                            if futures::executor::block_on(
                                crate::display::open_with::portal_open_with(path.clone()),
                            )
                            .is_ok()
                            {
                                return Ok((path, None));
                            }
                            let apps = crate::display::open_with::apps_for(&path, &mime);
                            Ok((path, Some(apps)))
                        }
                        #[cfg(target_os = "macos")]
                        {
                            let apps = crate::display::open_with::apps_for(&path, &mime);
                            Ok((path, Some(apps)))
                        }
                        #[cfg(any(target_os = "android", target_os = "ios"))]
                        {
                            let _ = mime;
                            Ok((path, None))
                        }
                    },
                    move |this, result, cx| match result {
                        Ok((path, Some(apps))) => {
                            if apps.is_empty() {
                                cx.reveal_path(&path);
                                return;
                            }
                            let mut entries: Vec<Entry> = apps
                                .into_iter()
                                .map(|app| {
                                    let label = app.name.clone();
                                    entry(
                                        label,
                                        "external",
                                        Action::Tools(ToolAction::OpenAttachmentIn(path.clone(), app)),
                                    )
                                })
                                .collect();
                            entries.push(Entry::Separator);
                            entries.push(entry(
                                if cfg!(target_os = "macos") { t!("tools-show-in-finder") } else { t!("tools-show-in-folder") },
                                "download",
                                Action::Tools(ToolAction::RevealAttachment(i)),
                            ));
                            let pos = this.find.menu_pos.unwrap_or_else(|| point(px(360.), px(260.)));
                            this.open_menu(pos, entries, cx);
                        }
                        Ok((_, None)) => {}
                        Err(e) => this.notice(format!("{e:#}"), true, cx),
                    },
                );
            }
            ToolAction::OpenAttachmentIn(path, app) => {
                if let Err(e) = crate::display::open_with::open_in(&app, &path) {
                    self.notice(t!("tools-couldnt-open-in", name = app.name, e = format!("{:#}", e)), true, cx);
                }
            }
            ToolAction::PreviewImage(i) => {
                let Some(m) = self.selected_mail() else { return };
                let key = (m.id.clone(), i);
                if self.image_previews.remove(&key).is_some() {
                    cx.notify();
                    return;
                }
                let Some(a) = m.attachments().get(i).cloned() else { return };
                let Some(account) = self.account_of(&m) else { return };
                self.task(
                    cx,
                    move |b| {
                        let bytes = b.attachment(&account, &m, a.part)?;
                        decode_image_preview(bytes, &a.mime, &a.name)
                    },
                    move |this, result: anyhow::Result<(ImageFormat, Vec<u8>)>, cx| match result {
                        Ok((format, bytes)) => {
                            this.image_previews
                                .insert(key.clone(), Arc::new(gpui::Image::from_bytes(format, bytes)));
                            cx.notify();
                        }
                        Err(e) => this.notice(format!("{e:#}"), true, cx),
                    },
                );
            }
            ToolAction::FindOpen => {
                if self.selected.is_none() {
                    self.handle(Action::OpenSearch, w, cx);
                    return;
                }
                self.find.open = true;
                self.find.reveal.set(true);
                self.find.input.update(cx, |s, cx| s.focus(w, cx));
            }
            ToolAction::FindNext | ToolAction::FindPrevious => {
                let n = self.find.count.get();
                if n == 0 {
                    return;
                }
                let cur = self.find.current.min(n - 1);
                self.find.current = if action == ToolAction::FindNext {
                    (cur + 1) % n
                } else {
                    (cur + n - 1) % n
                };
                self.find.reveal.set(true);
            }
            ToolAction::FindClose => {
                self.find.open = false;
                self.find.current = 0;
                self.find.count.set(0);
                w.focus(&self.focus, cx);
            }
            ToolAction::PublishKeys => {
                let addresses: Vec<String> = self.accounts.iter().map(|a| a.email.clone()).collect();
                self.notice(t!("tools-publishing-your-public-key-on-keys"), false, cx);
                self.task(
                    cx,
                    move |b| {
                        let ring = b.keyring();
                        let keys = ring.secret_keys();
                        anyhow::ensure!(!keys.is_empty(), t!("tools-you-have-no-openpgp-key-of"));
                        let mut done = crate::sync::keys::Published::default();
                        for key in keys {
                            let armored = pgp::composed::SignedPublicKey::from(key)
                                .to_armored_string(Default::default())?;
                            let one = crate::sync::keys::publish(&armored, &addresses, b.tls_connector_pub()?)?;
                            done.listed.extend(one.listed);
                            done.confirming.extend(one.confirming);
                        }
                        Ok(done)
                    },
                    |this, result: anyhow::Result<crate::sync::keys::Published>, cx| match result {
                        Ok(p) if !p.confirming.is_empty() => this.notice(
                            t!("tools-keys-openpgp-org-emailed-a-link", addresses = p.confirming.join(", ")),
                            false,
                            cx,
                        ),
                        Ok(p) if !p.listed.is_empty() => this.notice(
                            t!("tools-your-key-is-listed-on-keys", addresses = p.listed.join(", ")),
                            false,
                            cx,
                        ),
                        Ok(_) => this.notice(t!("tools-uploaded-to-keys-openpgp-org"), false, cx),
                        Err(e) => this.notice(format!("{e:#}"), true, cx),
                    },
                );
            }
            ToolAction::PublishKeysCustomHkp => {
                let typed = Self::value(&self.find.hkp_server, cx);
                let server = match typed.trim() {
                    "" => DEFAULT_HKP_SERVER.to_owned(),
                    s => s.to_owned(),
                };
                // Checked before asking, so a mistyped address is said at once.
                if let Err(e) = crate::sync::keys::hkp_add_url(&server) {
                    self.notice(format!("{e:#}"), true, cx);
                    return;
                }
                // HKP keyservers list every address on the key at once and
                // never take a key down again: asked first.
                let detail = t!("tools-will-list-your-public-key-and", server = server);
                let answer = w.prompt(
                    PromptLevel::Warning,
                    t!("tools-publish-your-public-key-for-good"),
                    Some(&detail),
                    &[t!("tools-publish"), t!("tools-cancel")],
                    cx,
                );
                cx.spawn(async move |this, cx| {
                    if matches!(answer.await, Ok(0)) {
                        let _ = this.update(cx, |this, cx| this.publish_keys_hkp(server, cx));
                    }
                })
                .detach();
            }
            ToolAction::ExportWkd => {
                let picker = cx.prompt_for_paths(PathPromptOptions {
                    files: false,
                    directories: true,
                    multiple: false,
                    prompt: Some(t!("tools-choose-folder-for-wkd-export").into()),
                });
                cx.spawn(async move |this, cx| {
                    if let Ok(Ok(Some(paths))) = picker.await
                        && let Some(dir) = paths.into_iter().next()
                    {
                        let _ = this.update(cx, |this, cx| {
                            this.task(
                                cx,
                                move |b| {
                                    use pgp::ser::Serialize as _;
                                    let ring = b.keyring();
                                    let keys = ring.secret_keys();
                                    anyhow::ensure!(
                                        !keys.is_empty(),
                                        t!("tools-you-have-no-openpgp-key-of-2")
                                    );
                                    // Each address's file holds the key with that
                                    // address's user ID alone, so a domain's
                                    // directory doesn't list the person's others.
                                    let mut pairs = Vec::new();
                                    for key in keys {
                                        let public = pgp::composed::SignedPublicKey::from(key);
                                        let emails: Vec<String> = public
                                            .details
                                            .users
                                            .iter()
                                            .filter_map(|u| extract_uid_email(&String::from_utf8_lossy(u.id.id())))
                                            .collect();
                                        for email in emails {
                                            let mut only = public.clone();
                                            only.details.users.retain(|u| {
                                                extract_uid_email(&String::from_utf8_lossy(u.id.id()))
                                                    .is_some_and(|e| e.eq_ignore_ascii_case(&email))
                                            });
                                            pairs.push((email, only.to_bytes()?));
                                        }
                                    }
                                    let domains = crate::sync::keys::export_wkd(&pairs, &dir)?;
                                    Ok((pairs.len(), domains))
                                },
                                move |this, result: anyhow::Result<(usize, Vec<std::path::PathBuf>)>, cx| match result {
                                    Ok((n, domains)) => {
                                        let names: Vec<String> = domains
                                            .iter()
                                            .filter_map(|d| d.file_name().map(|n| n.to_string_lossy().into_owned()))
                                            .collect();
                                        this.notice(
                                            t!("tools-exported-wkd-files-for-address-put", n = n, domains = names.join(", ")),
                                            false,
                                            cx,
                                        );
                                        if let Some(first) = domains.first() {
                                            cx.reveal_path(first);
                                        }
                                    }
                                    Err(e) => this.notice(format!("{e:#}"), true, cx),
                                },
                            );
                        });
                    }
                })
                .detach();
            }
            ToolAction::SetDefaultApp => {
                self.task(
                    cx,
                    |_| set_default_mail_app(),
                    |this, result: anyhow::Result<(String, Option<bool>)>, cx| match result {
                        Ok((msg, status)) => {
                            if let Some(is_default) = status {
                                this.find.default_mail_app = Some(is_default);
                            }
                            this.notice(msg, false, cx);
                        }
                        Err(e) => this.notice(format!("{e:#}"), true, cx),
                    },
                );
            }
        }
        cx.notify();
    }

    /// Refreshes the cached OS default-email-app status on a background thread.
    pub(super) fn refresh_default_app(&mut self, cx: &mut Context<Self>) {
        if self.mobile.on {
            return;
        }
        self.task(
            cx,
            |_| Ok(is_default_mail_app()),
            |this, result: anyhow::Result<Option<bool>>, cx| {
                if let Ok(status) = result {
                    this.find.default_mail_app = status;
                    cx.notify();
                }
            },
        );
    }

    fn publish_keys_hkp(&mut self, server: String, cx: &mut Context<Self>) {
        self.notice(t!("tools-publishing-your-public-key-on", server = server), false, cx);
        self.task(
            cx,
            move |b| {
                let ring = b.keyring();
                let keys = ring.secret_keys();
                anyhow::ensure!(
                    !keys.is_empty(),
                    t!("tools-you-have-no-openpgp-key-of")
                );
                let mut host = server.clone();
                for key in keys {
                    let armored = pgp::composed::SignedPublicKey::from(key)
                        .to_armored_string(Default::default())?;
                    host = crate::sync::keys::publish_hkp(&server, &armored, b.tls_connector_pub()?)?;
                }
                Ok(host)
            },
            |this, result: anyhow::Result<String>, cx| match result {
                Ok(host) => this.notice(
                    t!("tools-published-your-public-key-on-it", host = host),
                    false,
                    cx,
                ),
                Err(e) => this.notice(format!("{e:#}"), true, cx),
            },
        );
    }

    /// Acts on what Picacho was opened with (`launch`): writes to `mailto:`
    /// addresses, shows email files, and attaches shared files to a new
    /// message (the one a `mailto:` just started, if any).
    pub(super) fn take_launch_requests(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        let requests = crate::display::launch::take();
        if requests.is_empty() {
            return;
        }
        let mut started: Option<Entity<Composer>> = None;
        let mut files = Vec::new();
        for request in requests {
            if let Some(action) = crate::manage::widgets::WidgetAction::parse(&request) {
                self.open_widget_action(action, w, cx);
            } else if let Some(target) = request.strip_prefix(crate::display::launch::NOTIFICATION) {
                // A notification tapped (or Reply pressed) while Picacho ran
                // in the background, which started this window for it.
                if let Ok(target) = serde_json::from_str(target) {
                    self.open_notified(target, w, cx);
                }
            } else if let Some(path) = request.strip_prefix("attach:") {
                files.push(std::path::PathBuf::from(path));
            } else if let Some(path) = request.strip_prefix("file:") {
                self.open_eml_file(std::path::Path::new(path), w, cx);
            } else {
                self.compose_mailto(&request, w, cx);
                started = self.composers.last().cloned();
            }
        }
        if !files.is_empty() {
            let composer = match started {
                Some(c) => Some(c),
                None => {
                    self.compose_with(0, None, None, None, None, w, cx);
                    self.composers.last().cloned()
                }
            };
            if let Some(composer) = composer {
                composer.update(cx, |c, cx| c.add_files(files, cx));
            }
        }
        w.activate_window();
    }

    /// Opens what a home-screen widget asked for: an inbox (one account or
    /// All Accounts), a specific inbox message, a new composer, or Calendar.
    pub(super) fn open_widget_action(
        &mut self,
        action: crate::manage::widgets::WidgetAction,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use crate::manage::widgets::WidgetAction;
        if self.onboarding || self.retired {
            return;
        }
        self.menu = None;
        self.presets_for = None;
        if self.modal == Some(Modal::Settings) {
            self.modal = None;
        }
        match action {
            WidgetAction::Inbox { account } => {
                let target_account = match account {
                    Some(id) if self.accounts.iter().any(|a| a.id == id) => Some(id),
                    _ if self.shows_all_mail() => Some(ALL_MAIL.to_owned()),
                    _ => self.accounts.first().map(|a| a.id.clone()),
                };
                if target_account.is_some() {
                    self.open_mailbox(target_account, Folder::Inbox.into(), w, cx);
                    if self.mobile.on {
                        self.mobile.mailboxes = false;
                        self.mobile_pushed(cx);
                    }
                }
            }
            WidgetAction::Message { account, id } => {
                // Messages linked from Packages/Receipts widgets may live in
                // Archive or another folder rather than Inbox. A link names
                // a message kept here, of that account, or opens its inbox:
                // `picacho:` links can come from anywhere (a web page), and
                // they only ever show what is here.
                let Some(mailbox) = self
                    .backend
                    .store
                    .message(&id)
                    .ok()
                    .flatten()
                    .filter(|m| m.account == account)
                    .map(|m| m.mailbox())
                else {
                    let account = Some(account).filter(|a| self.accounts.iter().any(|x| &x.id == a));
                    self.open_widget_action(WidgetAction::Inbox { account }, w, cx);
                    return;
                };
                self.open_notified(
                    crate::display::notify::Target {
                        account,
                        mailbox,
                        message: Some(id),
                        messages: Vec::new(),
                        action: None,
                    },
                    w,
                    cx,
                );
            }
            WidgetAction::Compose { account } => {
                if let Some(id) = account
                    && self.accounts.iter().any(|a| a.id == id)
                    && self.account.as_deref() != Some(id.as_str())
                {
                    self.open_mailbox(Some(id), Folder::Inbox.into(), w, cx);
                }
                self.compose_with(0, None, None, None, None, w, cx);
            }
            WidgetAction::Calendar => {
                self.cal_action(
                    calendar_views::CalAction::Open(calendar_views::AppView::Calendar),
                    w,
                    cx,
                );
                if self.mobile.on {
                    self.mobile.mailboxes = false;
                    self.mobile_pushed(cx);
                }
            }
            WidgetAction::Packages { account } => {
                let target_account = match account {
                    Some(id) if self.accounts.iter().any(|a| a.id == id) => Some(id),
                    _ if self.shows_all_mail() => Some(ALL_MAIL.to_owned()),
                    _ => self.accounts.first().map(|a| a.id.clone()),
                };
                if let Some(id) = target_account {
                    self.open_summary(id, w, cx);
                    self.summary_packages = true;
                    self.summary_receipts = false;
                    if self.mobile.on {
                        self.mobile.mailboxes = false;
                        self.mobile_pushed(cx);
                    }
                }
            }
            WidgetAction::Receipts { account } => {
                let target_account = match account {
                    Some(id) if self.accounts.iter().any(|a| a.id == id) => Some(id),
                    _ if self.shows_all_mail() => Some(ALL_MAIL.to_owned()),
                    _ => self.accounts.first().map(|a| a.id.clone()),
                };
                if let Some(id) = target_account {
                    self.open_summary(id, w, cx);
                    self.summary_receipts = true;
                    self.summary_packages = false;
                    if self.mobile.on {
                        self.mobile.mailboxes = false;
                        self.mobile_pushed(cx);
                    }
                }
            }
            WidgetAction::Panel => {
                #[cfg(not(any(target_os = "android", target_os = "ios")))]
                self.open_widget_window(w, cx);
            }
        }
        cx.notify();
    }

    /// A button on a new-mail notification: Mark as Read, Archive, Delete
    /// (each for every message it announced), or Reply (which brings the
    /// window forward with a reply to the latest).
    pub(super) fn quick_action(
        &mut self,
        target: crate::display::notify::Target,
        action: &str,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.accounts.iter().any(|a| a.id == target.account) {
            return;
        }
        if action == "reply" {
            let Some(id) = target.message.clone() else { return };
            let store = &self.backend.store;
            let found = store
                .message(&id)
                .ok()
                .flatten()
                .or_else(|| store.by_message_id(&id).ok().flatten());
            let Some(m) = found else { return };
            w.activate_window();
            cx.activate(true);
            let m = self.backend.with_readable(vec![m]).remove(0);
            self.compose_reply(1, Some(m), None, w, cx);
            return;
        }
        let Some(change) = crate::display::notify::quick_change(action) else {
            return;
        };
        // Listed here: changed at once, as from the list.
        let ids = target.message_ids();
        let listed: Vec<String> = self
            .messages
            .iter()
            .filter(|x| ids.contains(&x.id) || (!x.message_id.is_empty() && ids.contains(&x.message_id)))
            .map(|x| x.id.clone())
            .collect();
        if !listed.is_empty() && listed.len() == ids.len() {
            self.apply(listed, change, cx);
            return;
        }
        self.task(
            cx,
            move |b| crate::display::notify::act(&b, &target, change),
            |this, result: anyhow::Result<()>, cx| {
                if let Err(e) = result {
                    this.notice(format!("{e:#}"), true, cx);
                }
                this.reload_cached(cx);
                this.refresh_counts(cx);
            },
        );
    }

    /// The buttons beside an attachment in the reader.
    pub(super) fn attachment_buttons(&self, i: usize, a: &Attachment, cx: &Context<Self>) -> Vec<AnyElement> {
        let mut buttons = Vec::new();
        let m_id = self.selected.clone().unwrap_or_default();
        if is_image(a) {
            let shown = self.image_previews.contains_key(&(m_id, i));
            buttons.push(
                self.button(
                    format!("attachment-image-{i}"),
                    if shown { t!("tools-hide") } else { t!("tools-preview") },
                    "image",
                    Action::Tools(ToolAction::PreviewImage(i)),
                    false,
                    cx,
                )
                .into_any_element(),
            );
        }
        buttons.push(
            self.button(
                format!("attachment-open-{i}"),
                t!("tools-open"),
                "external",
                Action::Tools(ToolAction::OpenAttachment(i)),
                false,
                cx,
            )
            .into_any_element(),
        );
        let more: Vec<Entry> = [
            cfg!(target_os = "macos")
                .then(|| entry(t!("tools-quick-look"), "preview", Action::Tools(ToolAction::QuickLook(i)))),
            cfg!(not(any(target_os = "android", target_os = "ios")))
                .then(|| entry(t!("tools-open-with"), "external", Action::Tools(ToolAction::OpenWith(i)))),
            Some(entry(
                if cfg!(target_os = "macos") { t!("tools-show-in-finder") } else { t!("tools-show-in-folder") },
                "download",
                Action::Tools(ToolAction::RevealAttachment(i)),
            )),
            Some(entry(t!("tools-save"), "download", Action::SaveAttachment(i))),
        ]
        .into_iter()
        .flatten()
        .collect();
        let c = self.colors();
        buttons.push(
            div()
                .id(SharedString::from(format!("attachment-more-{i}")))
                .size(px(34.))
                .rounded(px(9.))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .hover(move |s| s.bg(rgb(c.raised)))
                .child(icon_el("ellipsis", 16., c.muted))
                .tooltip(|w, cx| Tooltip::new(t!("tools-more")).build(w, cx))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        this.find.menu_pos = Some(e.position);
                        this.open_menu(e.position, more.clone(), cx)
                    }),
                )
                .into_any_element(),
        );
        buttons
    }

    /// An attached image shown under its row, once loaded.
    pub(super) fn image_preview(&self, i: usize) -> Option<AnyElement> {
        let m_id = self.selected.clone()?;
        let image = self.image_previews.get(&(m_id, i))?.clone();
        Some(
            div()
                .mt_1()
                .max_h(px(480.))
                .flex()
                .justify_center()
                .child(img(image).max_w_full().max_h(px(480.)).object_fit(ObjectFit::Contain))
                .into_any_element(),
        )
    }
}

/// Extracts an email address from an OpenPGP User ID (`Name <addr@domain>` or `addr@domain`).
fn extract_uid_email(uid: &str) -> Option<String> {
    let addr = match (uid.rfind('<'), uid.rfind('>')) {
        (Some(a), Some(b)) if a < b => &uid[a + 1..b],
        _ => uid,
    }
    .trim();
    addr.contains('@').then(|| addr.to_owned())
}

/// Whether Picacho is currently the OS default handler for `mailto:` links.
pub(super) fn is_default_mail_app() -> Option<bool> {
    #[cfg(windows)]
    {
        Some(crate::display::windows::is_default_mail_app())
    }
    #[cfg(all(unix, not(any(target_os = "macos", target_os = "android", target_os = "ios"))))]
    {
        let out = std::process::Command::new("xdg-settings")
            .args(["get", "default-url-scheme-handler", "mailto"])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .or_else(|| {
                std::process::Command::new("xdg-mime")
                    .args(["query", "default", "x-scheme-handler/mailto"])
                    .output()
                    .ok()
                    .filter(|o| o.status.success())
            })?;
        let text = String::from_utf8_lossy(&out.stdout);
        Some(text.trim().eq_ignore_ascii_case("tech.pinal.picacho.desktop"))
    }
    #[cfg(target_os = "macos")]
    {
        use objc2::rc::autoreleasepool;
        use objc2_app_kit::NSWorkspace;
        use objc2_foundation::{NSString, NSURL};
        autoreleasepool(|_| {
            let url = NSURL::URLWithString(&NSString::from_str("mailto:test@example.com"))?;
            let app = NSWorkspace::sharedWorkspace().URLForApplicationToOpenURL(&url)?;
            let path = app.path()?.to_string();
            Some(path.to_ascii_lowercase().contains("picacho"))
        })
    }
    #[cfg(any(target_os = "android", target_os = "ios"))]
    {
        None
    }
}

/// Sets Picacho as the default email app where the OS allows it, or opens the
/// OS Default apps page directly on Picacho's entry.
fn set_default_mail_app() -> anyhow::Result<(String, Option<bool>)> {
    #[cfg(windows)]
    {
        crate::display::windows::open_default_apps()?;
        Ok((
            t!("tools-opened-windows-default-apps-to-picacho").into(),
            Some(crate::display::windows::is_default_mail_app()),
        ))
    }
    #[cfg(all(unix, not(any(target_os = "macos", target_os = "android", target_os = "ios"))))]
    {
        crate::display::desktop::register_sync();
        let desktop = format!("{}.desktop", crate::display::desktop::APP_ID);
        let mime = std::process::Command::new("xdg-mime")
            .args([
                "default",
                &desktop,
                "x-scheme-handler/mailto",
                "message/rfc822",
                "application/vnd.ms-outlook",
            ])
            .status();
        let settings = std::process::Command::new("xdg-settings")
            .args(["set", "default-url-scheme-handler", "mailto", &desktop])
            .status();
        let ok = mime.as_ref().is_ok_and(|s| s.success())
            || settings.as_ref().is_ok_and(|s| s.success());
        anyhow::ensure!(
            ok,
            t!("tools-could-not-set-default-email-app")
        );
        Ok((
            t!("tools-picacho-is-now-your-default-email").into(),
            Some(true),
        ))
    }
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("/usr/bin/open")
            .args(["-b", "com.apple.mail"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
        Ok((
            t!("tools-in-apple-mail-choose-settings-general").into(),
            is_default_mail_app(),
        ))
    }
    #[cfg(any(target_os = "android", target_os = "ios"))]
    {
        anyhow::bail!(t!("tools-choose-your-default-email-app-in"))
    }
}

/// Quick Look (macOS): a preview window of the file, as Finder's space bar.
fn quick_look(path: &std::path::Path) {
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("qlmanage")
            .arg("-p")
            .arg(path)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
    }
    #[cfg(not(target_os = "macos"))]
    let _ = path;
}

/// Windows' Open With dialog for the file.
#[cfg(windows)]
fn open_with_windows(path: &std::path::Path) {
    let _ = std::process::Command::new("rundll32.exe")
        .arg("shell32.dll,OpenAs_RunDLL")
        .arg(path)
        .spawn();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::prelude::v1::test;

    #[test]
    fn marks_text_only_outside_tags_and_styles() {
        let html = "<html><head><style>.cat{}</style></head><body><p class=\"cat\">A Cat and a cat.</p></body></html>";
        let (marked, n) = mark_html(html, "cat", 1);
        assert_eq!(n, 2);
        assert!(marked.contains("<style>.cat{}</style>"));
        assert!(marked.contains("class=\"cat\""));
        assert!(marked.contains("id=\"picacho-find-0\""));
        assert!(marked.contains(">Cat</mark>"));
        assert!(marked.contains("#ff9632!important;color:#000!important;border-radius:2px\">cat</mark>"));
    }

    #[test]
    fn plain_text_is_escaped_for_finding() {
        let (marked, n) = mark_html(&plain_as_html("a < b & c"), &escape_html("< b"), 0);
        assert_eq!(n, 1);
        assert!(marked.contains("a <mark"));
    }

    /// An SVG from mail can't show files from this computer.
    #[test]
    fn svg_previews_load_no_files() {
        let dir = std::env::temp_dir().join(format!("picacho-svg-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let png = dir.join("secret.png");
        image::RgbaImage::from_pixel(8, 8, image::Rgba([255, 0, 0, 255]))
            .save(&png)
            .unwrap();
        let svg = format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" xmlns:xlink=\"http://www.w3.org/1999/xlink\" width=\"8\" height=\"8\">\
             <image width=\"8\" height=\"8\" xlink:href=\"{}\"/></svg>",
            png.display()
        );
        let (_, out) = decode_image_preview(svg.into_bytes(), "image/svg+xml", "x.svg").unwrap();
        let shown = image::load_from_memory(&out).unwrap().to_rgba8();
        assert!(shown.pixels().all(|p| p[3] == 0), "the file's pixels must not be drawn");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn previews_decode_svg_bmp_tiff_qoi_and_pnm() {
        // SVG
        let svg = b"<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"24\" height=\"24\"><circle cx=\"12\" cy=\"12\" r=\"10\" fill=\"#2a7\"/></svg>";
        let (fmt, png) = decode_image_preview(svg.to_vec(), "image/svg+xml", "logo.svg").unwrap();
        assert_eq!(fmt, ImageFormat::Png);
        assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));

        // BMP, TIFF, QOI, PNM via image crate
        let img = image::RgbaImage::from_pixel(4, 4, image::Rgba([20, 120, 220, 255]));
        for (image_fmt, mime, name) in [
            (image::ImageFormat::Bmp, "image/bmp", "pic.bmp"),
            (image::ImageFormat::Tiff, "image/tiff", "scan.tiff"),
            (image::ImageFormat::Qoi, "image/qoi", "shot.qoi"),
            (image::ImageFormat::Pnm, "image/x-portable-pixmap", "img.ppm"),
            (image::ImageFormat::Ico, "image/x-icon", "fav.ico"),
            (image::ImageFormat::Tga, "image/x-tga", "tex.tga"),
        ] {
            let mut buf = std::io::Cursor::new(Vec::new());
            if image_fmt == image::ImageFormat::Pnm {
                image::DynamicImage::ImageRgba8(img.clone())
                    .to_rgb8()
                    .write_to(&mut buf, image_fmt)
                    .unwrap();
            } else {
                img.write_to(&mut buf, image_fmt).unwrap();
            }
            let att = Attachment {
                name: name.into(),
                mime: mime.into(),
                size: buf.get_ref().len(),
                part: 0,
                section: String::new(),
                encoding: String::new(),
            };
            assert!(is_image(&att), "{name} should be previewable");
            let (fmt, out) = decode_image_preview(buf.into_inner(), mime, name).unwrap();
            assert_eq!(fmt, ImageFormat::Png, "{name} should transcode to PNG");
            assert!(out.starts_with(b"\x89PNG\r\n\x1a\n"));
        }
    }
}

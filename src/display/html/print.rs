//! Vector PDF export and native system printing of messages.
//!
//! Both Print and Save as PDF lay out the message with Blitz (the same
//! HTML/CSS engine and system fonts the reader uses) and paint it into a
//! multi-page PDF through `PdfScene` (krilla): every script the system has
//! fonts for is embedded (subset, copyable and searchable), formatting and
//! inline/allowed images are kept, and pages break between lines of text
//! where possible.
use super::{
    document::{EmailDocument, Tone},
    pdf_scene::{PdfScene, Resources},
};
use crate::manage::model::Mail;
use anyrender::recording::RenderCommand;
#[cfg_attr(any(target_os = "android", target_os = "ios", windows), allow(unused_imports))]
use anyhow::{Context as _, Result};
use krilla::{
    Document,
    geom::{PathBuilder, Transform},
    metadata::Metadata,
    page::PageSettings,
    paint::FillRule,
};
use std::time::Duration;

/// 1 CSS pixel at 96 DPI in PDF points (72 pt/in).
const CSS_TO_PT: f32 = 0.75;
/// Page margin on every side, in PDF points (0.5 in).
const MARGIN: f32 = 36.;
/// Safety cap on pages for degenerate/malformed emails.
const MAX_PAGES: usize = 200;

/// Standard paper size in PDF points: US Letter for North America, A4 elsewhere.
fn page_size() -> (f32, f32) {
    let letter = sys_locale::get_locale().is_some_and(|loc| {
        let upper = loc.to_ascii_uppercase();
        upper.ends_with("-US")
            || upper.ends_with("_US")
            || upper.ends_with("-CA")
            || upper.ends_with("_CA")
            || upper.ends_with("-MX")
            || upper.ends_with("_MX")
    });
    if letter {
        (612., 792.)
    } else {
        (595.28, 841.89)
    }
}

fn escape_html(text: &str) -> String {
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

/// The message's header block rendered as its own clean HTML document so the
/// email's CSS never leaks into or hides the headers.
fn header_html(m: &Mail, from: &str, date: &str) -> String {
    let subject = if m.subject.trim().is_empty() {
        t!("html-print-no-subject")
    } else {
        m.subject.trim()
    };
    let mut rows = String::new();
    let mut add = |label: &str, value: &str| {
        if !value.trim().is_empty() {
            rows.push_str(&format!(
                "<div class=\"row\"><span class=\"k\">{label}:</span> <span class=\"v\">{}</span></div>",
                escape_html(value)
            ));
        }
    };
    add(t!("html-print-from"), from);
    add(t!("html-print-to"), &m.to);
    add(t!("html-print-cc"), m.cc());
    add(t!("html-print-date"), date);
    let attachments: Vec<String> = m
        .attachments()
        .iter()
        .map(|a| a.name.clone())
        .filter(|n| !n.is_empty())
        .collect();
    if !attachments.is_empty() {
        add(t!("html-print-attachments"), &attachments.join(", "));
    }
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><style>\
html,body{{margin:0;padding:0;background:#fff;color:#111;font-family:system-ui,-apple-system,'Segoe UI',sans-serif}}\
.hdr{{padding:0 0 12px 0;border-bottom:1px solid #d0d4da}}\
h1{{margin:0 0 8px 0;font-size:18px;line-height:1.3;font-weight:600;color:#111}}\
.row{{font-size:12px;line-height:1.5;color:#222}}\
.k{{font-weight:600;color:#555}}\
</style></head><body><div class=\"hdr\"><h1>{}</h1>{rows}</div></body></html>",
        escape_html(subject)
    )
}

/// Plain-text email wrapped in HTML so Blitz lays it out with system font
/// fallback across every script.
fn plain_body_html(text: &str) -> String {
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><style>\
html,body{{margin:0;padding:0;background:#fff;color:#111;\
font-family:system-ui,-apple-system,'Segoe UI',sans-serif;font-size:13px;line-height:1.55}}\
.plain{{white-space:pre-wrap;overflow-wrap:break-word}}\
</style></head><body><div class=\"plain\">{}</div></body></html>",
        escape_html(text)
    )
}

/// Rectangular clip path for a PDF surface region.
fn clip_rect(x: f32, y: f32, w: f32, h: f32) -> Option<krilla::geom::Path> {
    let mut b = PathBuilder::new();
    b.move_to(x, y);
    b.line_to(x + w, y);
    b.line_to(x + w, y + h);
    b.line_to(x, y + h);
    b.close();
    b.finish()
}

/// Finds a clean vertical break point at or slightly above `max_slice_h` (in
/// document CSS pixels, relative to the slice's top) so lines of text are not
/// sliced horizontally across two pages.
fn choose_break(scene: &anyrender::Scene, max_slice_h: f32) -> f32 {
    let min_slice = (max_slice_h * 0.65).max(1.);
    let mut cut = max_slice_h;
    for cmd in &scene.commands {
        let RenderCommand::GlyphRun(run) = cmd else {
            continue;
        };
        if run.glyphs.is_empty() || run.font_size <= 0. {
            continue;
        }
        let mut min_y = f64::INFINITY;
        let mut max_y = f64::NEG_INFINITY;
        for g in &run.glyphs {
            let p = run.transform * kurbo::Point::new(g.x as f64, g.y as f64);
            min_y = min_y.min(p.y);
            max_y = max_y.max(p.y);
        }
        // Approximate ascent/descent around the baseline.
        let top = (min_y as f32) - run.font_size * 0.9;
        let bottom = (max_y as f32) + run.font_size * 0.3;
        if top < cut && bottom > cut && top >= min_slice {
            cut = (top - 2.).max(min_slice);
        }
    }
    cut.max(1.)
}

/// Renders `m` (its headers, HTML or plain-text body, inline images, and
/// remote images when `remote` is true) into a multi-page vector PDF, on
/// this locale's paper (US Letter or A4).
pub fn message_pdf(m: &Mail, from: &str, date: &str, remote: bool) -> Result<Vec<u8>> {
    message_pdf_on(m, from, date, remote, page_size())
}

/// The smallest and largest paper laid out on, in PDF points (2 in, 200 in).
const PAPER_RANGE: std::ops::RangeInclusive<f32> = 144.0..=14_400.0;

/// `message_pdf` on paper of `paper` (width, height) in PDF points, as a
/// printer has it: this locale's paper when that size can't be laid out on.
pub fn message_pdf_on(m: &Mail, from: &str, date: &str, remote: bool, paper: (f32, f32)) -> Result<Vec<u8>> {
    let (page_w, page_h) = if PAPER_RANGE.contains(&paper.0) && PAPER_RANGE.contains(&paper.1) {
        paper
    } else {
        page_size()
    };
    let print_w = page_w - 2. * MARGIN;
    let print_h = page_h - 2. * MARGIN;
    let css_width = (print_w / CSS_TO_PT).round().max(320.) as u32;

    // Lay out the header block independently so the email's styles can't affect it.
    let mut header_doc = EmailDocument::with_parts(
        &header_html(m, from, date),
        Default::default(),
        true,
        Tone::Light,
    );
    header_doc.set_screen((print_h / CSS_TO_PT).round() as u32, 1.);
    let header_css_h = header_doc.layout(css_width);
    let header_scale = (print_w / header_doc.laid_width() as f32).min(CSS_TO_PT);
    let header_pt_h = (header_css_h * header_scale).min(print_h * 0.45);
    let header_gap = 12.0_f32;

    // Lay out the message body.
    let (body_html, inline) = match (&m.html, &m.body) {
        (Some(html), _) if !html.trim().is_empty() => (html.clone(), m.inline.clone()),
        (_, Some(text)) => (plain_body_html(text), Default::default()),
        _ => (plain_body_html(&m.snippet), Default::default()),
    };
    let mut body_doc = EmailDocument::with_parts(&body_html, inline, !remote, Tone::Light);
    body_doc.set_screen((print_h / CSS_TO_PT).round() as u32, 1.);
    body_doc.layout(css_width);
    // Let inline images (cid:/data:) decode, and remote images arrive when allowed.
    let wait = if remote {
        Duration::from_secs(5)
    } else {
        Duration::from_millis(500)
    };
    body_doc.wait_settled(css_width, wait);
    let total_body_h = body_doc.layout(css_width).max(1.);
    let body_laid_w = body_doc.laid_width();
    let body_scale = (print_w / body_laid_w as f32).min(CSS_TO_PT);

    let mut pdf = Document::new();
    let metadata = Metadata::new()
        .title(if m.subject.trim().is_empty() {
            t!("html-print-message").to_owned()
        } else {
            m.subject.clone()
        })
        .creator("Picacho".to_owned());
    pdf.set_metadata(metadata);

    let page_settings = PageSettings::from_wh(page_w, page_h)
        .ok_or_else(|| anyhow::anyhow!(t!("html-print-invalid-pdf-page-size")))?;
    let mut resources = Resources::default();

    let mut scroll_y = 0.0_f32;
    let mut page_index = 0usize;
    while page_index < MAX_PAGES && (page_index == 0 || scroll_y < total_body_h - 0.5) {
        let mut page = pdf.start_page_with(page_settings.clone());
        let mut surface = page.surface();

        let body_top_offset = if page_index == 0 {
            if let Some(clip) = clip_rect(MARGIN, MARGIN, print_w, header_pt_h) {
                surface.push_clip_path(&clip, &FillRule::NonZero);
                surface.push_transform(&Transform::from_translate(MARGIN, MARGIN));
                surface.push_transform(&Transform::from_scale(header_scale, header_scale));
                let mut scene = PdfScene::new(&mut surface, &mut resources);
                header_doc.paint_into(
                    &mut scene,
                    header_doc.laid_width(),
                    header_css_h.ceil().max(1.) as u32,
                    0.,
                );
                scene.finish();
                surface.pop();
                surface.pop();
                surface.pop();
            }
            header_pt_h + header_gap
        } else {
            0.
        };

        let avail_pt_h = (print_h - body_top_offset).max(1.);
        let max_slice_css_h = avail_pt_h / body_scale;
        let remaining_css_h = (total_body_h - scroll_y).max(0.);

        if remaining_css_h > 0.5 {
            let slice_h = if remaining_css_h > max_slice_css_h + 1. {
                let mut probe = anyrender::Scene::new();
                body_doc.paint_into(
                    &mut probe,
                    body_laid_w,
                    max_slice_css_h.ceil().max(1.) as u32,
                    scroll_y,
                );
                choose_break(&probe, max_slice_css_h)
            } else {
                remaining_css_h
            };

            let clip_h = (slice_h * body_scale).min(avail_pt_h);
            if let Some(clip) = clip_rect(MARGIN, MARGIN + body_top_offset, print_w, clip_h) {
                surface.push_clip_path(&clip, &FillRule::NonZero);
                surface.push_transform(&Transform::from_translate(
                    MARGIN,
                    MARGIN + body_top_offset,
                ));
                surface.push_transform(&Transform::from_scale(body_scale, body_scale));
                let mut scene = PdfScene::new(&mut surface, &mut resources);
                body_doc.paint_into(
                    &mut scene,
                    body_laid_w,
                    slice_h.ceil().max(1.) as u32,
                    scroll_y,
                );
                scene.finish();
                surface.pop();
                surface.pop();
                surface.pop();
            }
            scroll_y += slice_h;
        } else {
            scroll_y = total_body_h;
        }

        surface.finish();
        page.finish();
        page_index += 1;
    }

    pdf.finish()
        .map_err(|e| anyhow::anyhow!(t!("html-print-couldnt-build-pdf", e = format!("{:?}", e))))
}

/// How long a PDF written for printing is kept: long enough for a print
/// dialog or viewer to read it, then it is removed with the next print.
#[cfg_attr(any(target_os = "android", target_os = "ios", windows), allow(dead_code))]
const PRINT_FILES_KEPT: Duration = Duration::from_secs(60 * 60);

/// Writes `pdf_bytes` to a private file under the user's temporary folder for
/// handing to a system print API, first removing ones older than
/// `PRINT_FILES_KEPT` (they hold the text of mail). Windows prints from
/// memory, and writes none.
#[cfg_attr(any(target_os = "android", target_os = "ios", windows), allow(dead_code))]
fn temp_pdf(title: &str, pdf_bytes: &[u8]) -> Result<std::path::PathBuf> {
    let user: String = std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "user".into())
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    let parent = std::env::temp_dir().join(format!("picacho-{user}"));
    #[cfg_attr(not(unix), allow(unused_mut))]
    let mut dir = parent.join("print");
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&dir)?;
        // A folder that was already there keeps whoever made it and its
        // permissions: in a temporary folder all users share, another user
        // could have made it first, open to them. Then a fresh folder of a
        // name nobody could know ahead is used (made new, or not at all).
        if !private_dir(&parent) || !private_dir(&dir) {
            dir = std::env::temp_dir().join(format!("picacho-{user}-{}", uuid::Uuid::new_v4().simple()));
            std::fs::DirBuilder::new().mode(0o700).create(&dir)?;
        }
    }
    #[cfg(not(unix))]
    std::fs::create_dir_all(&dir)?;
    if let Ok(old) = std::fs::read_dir(&dir) {
        for entry in old.flatten() {
            let stale = entry
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|age| age > PRINT_FILES_KEPT);
            if stale {
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
    }
    let name = crate::manage::backend::opened::safe_file_name(&format!(
        "{}.pdf",
        if title.trim().is_empty() {
            t!("html-print-message")
        } else {
            title.trim()
        }
    ));
    let sub = dir.join(uuid::Uuid::new_v4().simple().to_string());
    std::fs::create_dir_all(&sub)?;
    let path = sub.join(name);
    std::fs::write(&path, pdf_bytes)?;
    Ok(path)
}

/// Whether `dir` is a folder (not a link to one) of this user's that no
/// other user can read, write or enter.
#[cfg(unix)]
#[cfg_attr(any(target_os = "android", target_os = "ios"), allow(dead_code))]
fn private_dir(dir: &std::path::Path) -> bool {
    use std::os::unix::fs::MetadataExt as _;
    unsafe extern "C" {
        fn geteuid() -> u32;
    }
    // SAFETY: geteuid takes nothing and can't fail.
    let me = unsafe { geteuid() };
    std::fs::symlink_metadata(dir).is_ok_and(|m| m.is_dir() && m.uid() == me && m.mode() & 0o077 == 0)
}

/// How a message was handed over for printing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(any(target_os = "android", target_os = "ios", windows), allow(dead_code))]
pub enum Printed {
    /// The system's print dialog was shown (and printed, or was cancelled).
    Dialog,
    /// No print dialog could be shown here: the PDF was opened in the
    /// system's PDF viewer, to print from there.
    Viewer,
}

/// Prints a generated PDF through the operating system's print dialog
/// (never via a web browser, and never straight to a printer unasked).
///
/// - Linux: xdg-desktop-portal's Print dialog (`ashpd::desktop::print`);
///   with no portal running, the PDF opens in the system's PDF viewer.
/// - Windows: `super::print_windows`, which shows Windows' own print
///   dialog and prints through Windows' print system, with no other app.
/// - macOS: `print_pdf_macos`, on the main thread, shows AppKit's dialog;
///   this function only opens the PDF there.
#[allow(unused_variables)]
#[cfg_attr(windows, allow(dead_code))]
pub fn print_pdf(pdf_bytes: &[u8], title: &str) -> Result<Printed> {
    #[cfg(all(unix, not(any(target_os = "android", target_os = "ios"))))]
    {
        let path = temp_pdf(title, pdf_bytes)?;
        #[cfg(not(target_os = "macos"))]
        if futures::executor::block_on(linux_portal_print(&path, title)).is_ok() {
            return Ok(Printed::Dialog);
        }
        open::that(&path).with_context(|| t!("html-print-couldnt-open-the-pdf-to-print"))?;
        Ok(Printed::Viewer)
    }
    #[cfg(windows)]
    {
        anyhow::bail!(t!("html-print-printing-isnt-available-on-this-device"))
    }
    #[cfg(any(target_os = "android", target_os = "ios"))]
    {
        anyhow::bail!(t!("html-print-printing-isnt-available-on-this-device"))
    }
}

/// macOS: shows the native macOS Print dialog for `pdf_bytes` on the main
/// thread using PDFKit's `PDFDocument` and `NSPrintOperation`. Falls back to
/// opening the PDF in Preview if PDFKit isn't available.
#[cfg(target_os = "macos")]
pub fn print_pdf_macos(pdf_bytes: &[u8], title: &str) -> Result<()> {
    use objc2::rc::{Retained, autoreleasepool};
    use objc2::runtime::{AnyClass, AnyObject};
    use objc2::{msg_send, MainThreadMarker};
    use objc2_app_kit::{NSPrintInfo, NSPrintOperation};
    use objc2_foundation::NSData;

    let ran = autoreleasepool(|_| -> Option<bool> {
        let mtm = MainThreadMarker::new()?;
        // PDFKit.framework loaded (once, and kept loaded) so `PDFDocument`
        // is registered.
        static PDFKIT: std::sync::OnceLock<Option<libloading::Library>> = std::sync::OnceLock::new();
        PDFKIT.get_or_init(|| unsafe {
            libloading::Library::new("/System/Library/Frameworks/PDFKit.framework/PDFKit").ok()
        });
        let cls = AnyClass::get(c"PDFDocument")?;
        let data = NSData::with_bytes(pdf_bytes);
        let doc: Option<Retained<AnyObject>> = unsafe {
            let alloc: *mut AnyObject = msg_send![cls, alloc];
            msg_send![alloc, initWithData: &*data]
        };
        let doc = doc?;
        let info = NSPrintInfo::sharedPrintInfo();
        // PDFPrintScalingMode::kPDFPrintPageScaleDownToFit = 1
        let op: Option<Retained<NSPrintOperation>> = unsafe {
            msg_send![
                &*doc,
                printOperationForPrintInfo: &*info,
                scalingMode: 1isize,
                autoRotate: true
            ]
        };
        let op = op?;
        op.setShowsPrintPanel(true);
        op.setShowsProgressPanel(true);
        let _ = mtm;
        Some(op.runOperation())
    });
    if ran.is_some() {
        return Ok(());
    }
    let path = temp_pdf(title, pdf_bytes)?;
    std::process::Command::new("/usr/bin/open")
        .arg("-a")
        .arg("Preview")
        .arg(&path)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .with_context(|| t!("html-print-couldnt-open-the-pdf-for-printing"))?;
    Ok(())
}

#[cfg(all(unix, not(any(target_os = "macos", target_os = "android", target_os = "ios"))))]
async fn linux_portal_print(path: &std::path::Path, title: &str) -> Result<()> {
    use ashpd::desktop::print::{PreparePrintOptions, PrintOptions, PrintProxy};
    use std::os::fd::AsFd as _;

    let file = std::fs::File::open(path)?;
    let title = if title.trim().is_empty() {
        t!("html-print-print-message")
    } else {
        title
    };
    let proxy = PrintProxy::new().await?;
    let pre = match proxy
        .prepare_print(
            None,
            title,
            Default::default(),
            Default::default(),
            PreparePrintOptions::default().set_modal(true),
        )
        .await?
        .response()
    {
        Ok(p) => p,
        Err(ashpd::Error::Response(ashpd::desktop::ResponseError::Cancelled)) => return Ok(()),
        Err(e) => return Err(e.into()),
    };
    match proxy
        .print(
            None,
            title,
            &file.as_fd(),
            PrintOptions::default()
                .set_token(pre.token)
                .set_modal(true),
        )
        .await?
        .response()
    {
        Ok(()) | Err(ashpd::Error::Response(ashpd::desktop::ResponseError::Cancelled)) => Ok(()),
        Err(e) => Err(e.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manage::model::InlineParts;
    use std::sync::Arc;

    /// Only a folder of this user's, closed to others and not a link, is
    /// taken for printed mail.
    #[cfg(unix)]
    #[test]
    fn print_folders_must_be_private() {
        use std::os::unix::fs::PermissionsExt as _;
        let base = std::env::temp_dir().join(format!("picacho-print-test-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir(&base).unwrap();
        std::fs::set_permissions(&base, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(private_dir(&base));
        std::fs::set_permissions(&base, std::fs::Permissions::from_mode(0o777)).unwrap();
        assert!(!private_dir(&base));
        std::fs::set_permissions(&base, std::fs::Permissions::from_mode(0o700)).unwrap();
        let link = base.with_extension("link");
        std::os::unix::fs::symlink(&base, &link).unwrap();
        assert!(!private_dir(&link));
        let _ = std::fs::remove_file(&link);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn html_with_unicode_and_inline_image_makes_valid_vector_pdf() -> Result<()> {
        // Minimal 1x1 red PNG as a cid: inline image.
        use image::ImageEncoder as _;
        let mut png = Vec::new();
        image::codecs::png::PngEncoder::new(&mut png).write_image(
            &[220, 40, 40, 255],
            1,
            1,
            image::ExtendedColorType::Rgba8,
        )?;
        let mut inline = InlineParts::default();
        inline.insert("dot@test", png);
        let m = Mail {
            subject: "Привет · こんにちは · مرحبا — ✓".into(),
            from: "ana@example.org".into(),
            to: "bo@example.org".into(),
            html: Some(
                "<html><body><h2 style=\"color:#1a5fb4\">Formatted heading</h2>\
                 <p>日本語テスト · Кириллица · Ελληνικά · العربية</p>\
                 <img src=\"cid:dot@test\" width=\"24\" height=\"24\"></body></html>"
                    .into(),
            ),
            inline: Arc::new(inline),
            ..Mail::default()
        };
        let pdf = message_pdf(&m, "Ana <ana@example.org>", "Nov 24, 2026", false)?;
        assert!(pdf.starts_with(b"%PDF-"));
        assert!(pdf.windows(5).any(|w| w == b"%%EOF"));
        // Embedded font subset and image XObject are present (not plain Type1 Helvetica).
        assert!(
            pdf.windows(b"/FontFile2".len()).any(|w| w == b"/FontFile2")
                || pdf.windows(b"/FontFile3".len()).any(|w| w == b"/FontFile3")
        );
        assert!(
            pdf.windows(b"/Subtype/Image".len())
                .any(|w| w == b"/Subtype/Image")
                || pdf
                    .windows(b"/Subtype /Image".len())
                    .any(|w| w == b"/Subtype /Image")
        );
        Ok(())
    }

    /// Laid out on a printer's paper (here A4, turned), pages are that size;
    /// a size no paper has falls back to the locale's.
    #[test]
    fn pages_take_the_printers_paper() -> Result<()> {
        let m = Mail {
            subject: "Wide".into(),
            body: Some("Hello".into()),
            ..Mail::default()
        };
        let media_box = |pdf: &[u8]| -> Vec<f32> {
            let text = String::from_utf8_lossy(pdf);
            let at = text.find("/MediaBox").expect("a MediaBox");
            let rest = &text[at + "/MediaBox".len()..];
            let inside = &rest[rest.find('[').unwrap() + 1..rest.find(']').unwrap()];
            inside.split_whitespace().map(|n| n.parse().unwrap()).collect()
        };
        let pdf = message_pdf_on(&m, "ana@example.org", "Nov 24, 2026", false, (841.89, 595.28))?;
        let b = media_box(&pdf);
        assert!((b[2] - b[0] - 841.89).abs() < 0.5 && (b[3] - b[1] - 595.28).abs() < 0.5, "{b:?}");
        let pdf = message_pdf_on(&m, "ana@example.org", "Nov 24, 2026", false, (0., f32::NAN))?;
        let b = media_box(&pdf);
        let (w, h) = page_size();
        assert!((b[2] - b[0] - w).abs() < 0.5 && (b[3] - b[1] - h).abs() < 0.5, "{b:?}");
        Ok(())
    }

    #[test]
    fn long_message_paginates_across_multiple_pages() -> Result<()> {
        let mut body = String::new();
        for i in 1..=140 {
            body.push_str(&format!("Line {i}: The quick brown fox jumps over the lazy dog.\n"));
        }
        let m = Mail {
            subject: "Long report".into(),
            from: "ana@example.org".into(),
            body: Some(body),
            ..Mail::default()
        };
        let pdf = message_pdf(&m, "ana@example.org", "Nov 24, 2026", false)?;
        assert!(pdf.starts_with(b"%PDF-"));
        let count_pages = |needle: &[u8]| {
            pdf.windows(needle.len() + 1)
                .filter(|w| w.starts_with(needle) && w[needle.len()] != b's')
                .count()
        };
        let pages = count_pages(b"/Type/Page") + count_pages(b"/Type /Page");
        assert!(pages >= 2, "expected multiple pages, got {pages}");
        Ok(())
    }
}

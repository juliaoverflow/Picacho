//! Pasting with or without formatting. GPUI's clipboard holds only text, so
//! the HTML a browser or word processor copies alongside it is read from the
//! system here and turned into the composer's Markdown.

/// What a paste into the composer can insert: the clipboard's plain text, and
/// its formatting as Markdown when that differs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Pasted {
    pub(super) plain: String,
    pub(super) formatted: String,
}

impl Pasted {
    /// The two versions of what is on the clipboard, or `None` when it has
    /// no formatting to choose about (the text pastes as usual).
    pub(super) fn from_clipboard(plain: String) -> Option<Self> {
        let html = clipboard_html()?;
        Self::from_parts(plain, &html)
    }

    pub(super) fn from_parts(plain: String, html: &str) -> Option<Self> {
        // Converted as it is pasted, on the interface's thread: a page copied
        // whole (megabytes of markup) pastes as plain text rather than
        // stalling the window while it is converted.
        if html.len() > 2 * 1024 * 1024 {
            return None;
        }
        let formatted = html_to_markdown(html)?;
        let same = |a: &str, b: &str| a.split_whitespace().eq(b.split_whitespace());
        (!formatted.is_empty() && !same(&formatted, &plain)).then_some(Self { plain, formatted })
    }
}

/// Copied HTML as Markdown the composer writes: dashes for bullets, no
/// styles, scripts or page furniture.
pub(super) fn html_to_markdown(html: &str) -> Option<String> {
    use htmd::options::{BrStyle, BulletListMarker, HrStyle, Options};
    // Windows' CF_HTML gives the fragment's byte offsets in its header
    // (StartFragment:/EndFragment:); those are what count, and the
    // comment markers where the header's are missing or out of range.
    let offset = |name: &str| {
        let at = html.find(name)? + name.len();
        html[at..]
            .split(['\r', '\n'])
            .next()?
            .trim()
            .parse::<usize>()
            .ok()
    };
    let header = match (offset("StartFragment:"), offset("EndFragment:")) {
        (Some(start), Some(end))
            if start < end
                && end <= html.len()
                && html.is_char_boundary(start)
                && html.is_char_boundary(end) =>
        {
            Some(&html[start..end])
        }
        _ => None,
    };
    let html = match (
        header,
        html.find("<!--StartFragment-->"),
        html.find("<!--EndFragment-->"),
    ) {
        (Some(fragment), ..) => fragment,
        (None, Some(start), Some(end)) if start < end => &html[start + 20..end],
        _ => html,
    };
    let converter = htmd::HtmlToMarkdown::builder()
        .options(Options {
            bullet_list_marker: BulletListMarker::Dash,
            ul_bullet_spacing: 1,
            ol_number_spacing: 1,
            br_style: BrStyle::Backslash,
            hr_style: HrStyle::Dashes,
            ..Options::default()
        })
        // Remote images and embeds too: as Markdown images they'd be sent
        // as <img> (a tracking pixel copied from a page or an email would
        // report each recipient opening the reply). Pictures to send go in
        // as attachments.
        .skip_tags(vec![
            "head", "script", "style", "title", "meta", "link", "noscript", "img", "picture",
            "svg", "iframe", "object", "embed", "video", "audio",
        ])
        // The composer's strikethrough; htmd keeps only the text.
        .add_handler(vec!["s", "del", "strike"], strikethrough)
        .build();
    let markdown = converter.convert(html).ok()?;
    let markdown = markdown.trim_matches('\n').to_string();
    (!markdown.trim().is_empty()).then_some(markdown)
}

/// The system clipboard's HTML, when it has some.
#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn clipboard_html() -> Option<String> {
    arboard::Clipboard::new().ok()?.get().html().ok()
}

#[cfg(target_os = "android")]
fn clipboard_html() -> Option<String> {
    crate::android::clipboard_html()
}

fn strikethrough(
    handlers: &dyn htmd::element_handler::Handlers,
    element: htmd::Element,
) -> Option<htmd::element_handler::HandlerResult> {
    let content = handlers.walk_children(element.node).content;
    let inner = content.trim();
    if inner.is_empty() {
        return None;
    }
    let lead = &content[..content.len() - content.trim_start().len()];
    let trail = &content[content.trim_end().len()..];
    Some(format!("{lead}~~{inner}~~{trail}").into())
}

/// iOS: the pasteboard's HTML, or its web archive or RTF turned into HTML.
#[cfg(target_os = "ios")]
fn clipboard_html() -> Option<String> {
    ios::clipboard_html()
}

#[cfg(target_os = "ios")]
#[path = "paste_ios.rs"]
mod ios;

/// A stretch of text with one style, from rich text that isn't HTML
/// (the RTF that iOS apps such as Notes copy).
#[cfg(any(target_os = "ios", test))]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Run {
    pub(super) text: String,
    pub(super) bold: bool,
    pub(super) italic: bool,
    pub(super) strike: bool,
    pub(super) code: bool,
    pub(super) link: Option<String>,
}

/// Styled runs as simple HTML for `html_to_markdown`: lines in paragraphs
/// (a blank line between them), and lines starting with a bullet or a
/// number and a tab, as RTF writes lists, as list items.
#[cfg(any(target_os = "ios", test))]
pub(super) fn runs_to_html(runs: &[Run]) -> String {
    let mut lines: Vec<Vec<Run>> = vec![vec![]];
    for run in runs {
        let text = run
            .text
            .replace("\r\n", "\n")
            .replace(['\r', '\u{2028}', '\u{2029}'], "\n");
        for (i, part) in text.split('\n').enumerate() {
            if i > 0 {
                lines.push(vec![]);
            }
            if !part.is_empty() {
                let line = lines.last_mut().expect("a line");
                line.push(Run {
                    text: part.to_string(),
                    ..run.clone()
                });
            }
        }
    }
    fn close(html: &mut String, block: &mut Option<&'static str>) {
        if let Some(tag) = block.take() {
            html.push_str(&format!("</{tag}>"));
        }
    }
    let mut html = String::new();
    let mut block: Option<&'static str> = None;
    for mut line in lines {
        let list = strip_list_marker(&mut line);
        let inline: String = line.iter().map(run_html).collect();
        match list {
            Some(tag) => {
                if block != Some(tag) {
                    close(&mut html, &mut block);
                    html.push_str(&format!("<{tag}>"));
                    block = Some(tag);
                }
                html.push_str(&format!("<li>{inline}</li>"));
            }
            None if line.iter().all(|r| r.text.trim().is_empty()) => close(&mut html, &mut block),
            None => {
                if block == Some("p") {
                    html.push_str("<br>");
                } else {
                    close(&mut html, &mut block);
                    html.push_str("<p>");
                    block = Some("p");
                }
                html.push_str(&inline);
            }
        }
    }
    close(&mut html, &mut block);
    html
}

/// Takes a list marker ("•\t", "1.\t") off the start of a line, and says
/// which list it starts.
#[cfg(any(target_os = "ios", test))]
fn strip_list_marker(line: &mut Vec<Run>) -> Option<&'static str> {
    let plain: String = line.iter().map(|r| r.text.as_str()).collect();
    let rest = plain.trim_start_matches([' ', '\t']);
    let (tag, after) = if let Some(after) = rest.strip_prefix(['•', '◦', '▪', '‣', '⁃', '–', '-'])
    {
        ("ul", after)
    } else {
        let digits = rest.len() - rest.trim_start_matches(|c: char| c.is_ascii_digit()).len();
        let after = rest[digits..]
            .strip_prefix(['.', ')'])
            .filter(|_| digits > 0)?;
        ("ol", after)
    };
    let after = after.strip_prefix('\t').or_else(|| {
        (tag == "ul" && !rest.starts_with(['-', '–']))
            .then(|| after.strip_prefix(' '))
            .flatten()
    })?;
    // Drop the marker's bytes from the front of the runs.
    let mut drop = plain.len() - after.len();
    while drop > 0 && !line.is_empty() {
        let n = line[0].text.len().min(drop);
        line[0].text.drain(..n);
        drop -= n;
        if line[0].text.is_empty() {
            line.remove(0);
        }
    }
    Some(tag)
}

#[cfg(any(target_os = "ios", test))]
fn run_html(run: &Run) -> String {
    let mut s = run
        .text
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('\t', " ");
    let wrap = |s: String, tag: &str| format!("<{tag}>{s}</{tag}>");
    if run.code {
        s = wrap(s, "code");
    }
    if run.bold {
        s = wrap(s, "b");
    }
    if run.italic {
        s = wrap(s, "i");
    }
    if run.strike {
        s = wrap(s, "s");
    }
    if let Some(link) = &run.link {
        let href = link.replace('&', "&amp;").replace('"', "&quot;");
        s = format!("<a href=\"{href}\">{s}</a>");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copied_html_becomes_markdown() {
        let html = "<html><head><style>p{}</style></head><body>\
            <p>Hello <b>there</b>, see <a href=\"https://example.com\">this</a>.</p>\
            <ul><li>one</li><li>two</li></ul></body></html>";
        assert_eq!(
            html_to_markdown(html).unwrap(),
            "Hello **there**, see [this](https://example.com).\n\n- one\n- two"
        );
    }

    /// A tracking pixel (or any remote image) copied with the text isn't
    /// pasted: it would go out as an image recipients load.
    #[test]
    fn remote_images_are_left_out() {
        let html = "<p>Thanks!<img src=\"https://t.example/p.gif\" width=1 height=1></p>";
        assert_eq!(html_to_markdown(html).unwrap(), "Thanks!");
    }

    #[test]
    fn only_the_fragment_is_pasted() {
        let html = "Version:0.9\r\n<html><body>junk<!--StartFragment--><i>hi</i><!--EndFragment--></body></html>";
        assert_eq!(html_to_markdown(html).unwrap(), "*hi*");
    }

    #[test]
    fn strikethrough_is_kept() {
        assert_eq!(
            html_to_markdown("<p>a <s>gone</s> b</p>").unwrap(),
            "a ~~gone~~ b"
        );
    }

    #[test]
    fn rich_text_runs_become_markdown() {
        let run = |text: &str| Run {
            text: text.into(),
            ..Run::default()
        };
        let runs = vec![
            run("Hi "),
            Run {
                bold: true,
                ..run("there")
            },
            run("\nsecond line\n\n\t•\tone\n\t•\t"),
            Run {
                link: Some("https://example.com/?a=1&b=2".into()),
                ..run("two")
            },
            run("\n1.\tfirst\n\nx < y"),
        ];
        assert_eq!(
            html_to_markdown(&runs_to_html(&runs)).unwrap(),
            "Hi **there**\\\nsecond line\n\n- one\n- [two](https://example.com/?a=1&b=2)\n\n1. first\n\nx < y"
        );
    }

    #[test]
    fn unformatted_html_offers_no_choice() {
        assert_eq!(
            Pasted::from_parts("just text".into(), "<p>just  text</p>"),
            None
        );
        let pasted = Pasted::from_parts("bold".into(), "<b>bold</b>").unwrap();
        assert_eq!(pasted.formatted, "**bold**");
    }
}

//! What can be done with an open message beyond reading it: finding text
//! in it (⌘F / Ctrl+F), printing it or saving it as a PDF, and opening its
//! attachments (with the system's app, Quick Look on macOS, Open With… on
//! Windows, and images previewed in the reader).
use super::*;

#[derive(Clone, Debug, PartialEq)]
pub(super) enum ToolAction {
    /// Prints a message (its id): the system's browser shows it with its
    /// print dialog, which also saves PDFs.
    Print(String),
    /// Saves a message (its id) as a PDF of its headers and text.
    SavePdf(String),
    /// The open message's attachment, by index: opened with the system's
    /// app for its type; shown with Quick Look (macOS); offered to the
    /// apps that open it (Windows' Open With); shown in its folder.
    OpenAttachment(usize),
    QuickLook(usize),
    OpenWith(usize),
    RevealAttachment(usize),
    /// Shows an attached image in the reader, or hides it again.
    PreviewImage(usize),
    FindOpen,
    FindNext,
    FindPrevious,
    FindClose,
    /// Opens the keyboard shortcuts file in the system's text editor.
    OpenShortcuts,
    /// Publishes your OpenPGP public keys on keys.openpgp.org, which
    /// emails each address a link to confirm it.
    PublishKeys,
}

/// Find in Message.
pub(super) struct Find {
    pub(super) open: bool,
    pub(super) input: Entity<InputState>,
    /// The match shown as current, from 0.
    pub(super) current: usize,
    /// Matches in the open message, counted as it was last drawn.
    pub(super) count: Cell<usize>,
    /// The current match should be scrolled to.
    pub(super) reveal: Cell<bool>,
}
impl Find {
    pub(super) fn new(window: &mut Window, cx: &mut App) -> Self {
        Self {
            open: false,
            input: input("Find in message", false, window, cx),
            current: 0,
            count: Cell::new(0),
            reveal: Cell::new(false),
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

/// `html` with its `cid:` images as data: URLs from `inline`, for pages
/// opened outside Picacho (printing).
fn inline_images(html: &str, inline: &InlineParts) -> String {
    use base64::Engine as _;
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(i) = rest.to_ascii_lowercase().find("cid:") {
        out.push_str(&rest[..i]);
        let after = &rest[i..];
        let end = after
            .find(|c: char| c == '"' || c == '\'' || c == ')' || c.is_whitespace() || c == '>')
            .unwrap_or(after.len());
        let url = &after[..end];
        match inline.get(url) {
            Some(bytes) => {
                let mime = match image::guess_format(&bytes) {
                    Ok(image::ImageFormat::Png) => "image/png",
                    Ok(image::ImageFormat::Gif) => "image/gif",
                    Ok(image::ImageFormat::WebP) => "image/webp",
                    _ => "image/jpeg",
                };
                out.push_str(&format!(
                    "data:{mime};base64,{}",
                    base64::engine::general_purpose::STANDARD.encode(&*bytes)
                ));
            }
            None => out.push_str(url),
        }
        rest = &after[end..];
    }
    out.push_str(rest);
    out
}

/// A page for printing `m`: its headers, then its HTML (or text), with
/// nothing run or fetched but the print dialog's script, unless `remote`
/// allows the email's remote images.
fn print_page(m: &Mail, remote: bool) -> String {
    let nonce = uuid::Uuid::new_v4().simple().to_string();
    let images = if remote { "data: https: http:" } else { "data:" };
    let mut headers = String::new();
    let mut header = |name: &str, value: &str| {
        if !value.trim().is_empty() {
            headers.push_str(&format!(
                "<div><b>{name}:</b> {}</div>",
                escape_html(value)
            ));
        }
    };
    let from = if m.sender.is_empty() || m.sender == m.from {
        m.from.clone()
    } else {
        format!("{} <{}>", m.sender, m.from)
    };
    header("From", &from);
    header("To", &m.to);
    header("Cc", m.cc());
    header("Date", &date_full(m.date));
    let body = match (&m.html, &m.body) {
        (Some(html), _) => inline_images(html, &m.inline),
        (None, Some(text)) => format!("<pre>{}</pre>", escape_html(text)),
        (None, None) => format!("<pre>{}</pre>", escape_html(&m.snippet)),
    };
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\">\
<meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; style-src 'unsafe-inline'; img-src {images}; font-src data:; script-src 'nonce-{nonce}'\">\
<title>{title}</title>\
<style>body{{font-family:system-ui,-apple-system,'Segoe UI',sans-serif;margin:24px;color:#111}}\
.picacho-headers{{font-size:13px;line-height:1.6;border-bottom:1px solid #ccc;padding-bottom:12px;margin-bottom:18px}}\
.picacho-headers h1{{font-size:20px;margin:0 0 8px}} pre{{white-space:pre-wrap;font-family:inherit;font-size:14px}}</style>\
<script nonce=\"{nonce}\">addEventListener('load',function(){{setTimeout(function(){{print()}},400)}})</script>\
</head><body><div class=\"picacho-headers\"><h1>{title}</h1>{headers}</div><div class=\"picacho-body\">{body}</div></body></html>",
        title = escape_html(&m.subject),
    )
}

/// A file in a folder of Picacho's own under the system's temporary
/// folder, readable only by this user.
fn temp_file(name: &str) -> std::io::Result<std::path::PathBuf> {
    let dir = std::env::temp_dir().join(format!("picacho-{}", whoami()));
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&dir)?;
    }
    #[cfg(not(unix))]
    std::fs::create_dir_all(&dir)?;
    // One folder per file keeps its own name, so the app opening it shows it.
    let sub = dir.join(uuid::Uuid::new_v4().simple().to_string());
    std::fs::create_dir_all(&sub)?;
    Ok(sub.join(safe_file_name(name)))
}
fn whoami() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "user".into())
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect()
}

/// Whether an attachment is an image the reader can show.
pub(super) fn is_image(a: &Attachment) -> bool {
    let mime = a.mime.to_ascii_lowercase();
    let name = a.name.to_ascii_lowercase();
    matches!(
        mime.as_str(),
        "image/png" | "image/jpeg" | "image/jpg" | "image/gif" | "image/webp"
    ) || [".png", ".jpg", ".jpeg", ".gif", ".webp"]
        .iter()
        .any(|e| name.ends_with(e))
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
            "No matches".into()
        } else {
            format!("{} of {}", self.find.current.min(count - 1) + 1, count)
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
                "@Previous match · ⇧Enter",
                "chevron-up",
                Action::Tools(ToolAction::FindPrevious),
                false,
                cx,
            ))
            .child(self.button(
                "find-next",
                "@Next match · Enter",
                "chevron-down",
                Action::Tools(ToolAction::FindNext),
                false,
                cx,
            ))
            .child(self.button(
                "find-close",
                "@Close · Esc",
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
                let page = print_page(&m, remote);
                let name = format!("{}.html", if m.subject.is_empty() { "Message" } else { &m.subject });
                match temp_file(&name).and_then(|p| std::fs::write(&p, page).map(|_| p)) {
                    Ok(path) => {
                        cx.open_with_system(&path);
                        this.notice(
                            "Opened for printing in your browser. Choose Save as PDF in its print dialog for a PDF.",
                            false,
                            cx,
                        );
                    }
                    Err(e) => this.notice(format!("Couldn't prepare the message for printing: {e}"), true, cx),
                }
            }),
            ToolAction::SavePdf(id) => self.with_full_message(id, cx, |this, m, cx| {
                let body = match (&m.body, &m.html) {
                    (Some(b), _) if !b.trim().is_empty() => b.clone(),
                    (_, Some(h)) => crate::sync::mime::html_to_text(h),
                    _ => m.snippet.clone(),
                };
                let from = if m.sender.is_empty() || m.sender == m.from {
                    m.from.clone()
                } else {
                    format!("{} <{}>", m.sender, m.from)
                };
                let headers = [
                    ("From", from),
                    ("To", m.to.clone()),
                    ("Cc", m.cc().to_owned()),
                    ("Date", date_full(m.date).to_string()),
                ];
                let pdf = crate::manage::pdf::message_pdf(&m.subject, &headers, &body);
                let name = safe_file_name(&format!(
                    "{}.pdf",
                    if m.subject.is_empty() { "Message" } else { &m.subject }
                ));
                let start = this.save_start();
                let picker = cx.prompt_for_new_path(&start, Some(&name));
                cx.spawn(async move |this, cx| {
                    if let Ok(Ok(Some(path))) = picker.await {
                        let result = std::fs::write(&path, pdf);
                        let _ = this.update(cx, |this, cx| match result {
                            Ok(()) => this.notice("Saved as PDF", false, cx),
                            Err(e) => this.notice(format!("Couldn't save the PDF: {e}"), true, cx),
                        });
                    }
                })
                .detach();
            }),
            ToolAction::OpenAttachment(i)
            | ToolAction::QuickLook(i)
            | ToolAction::OpenWith(i)
            | ToolAction::RevealAttachment(i) => {
                let how = action.clone();
                let Some(m) = self.selected_mail() else { return };
                let Some(a) = m.attachments().get(i).cloned() else { return };
                let Some(account) = self.account_of(&m) else { return };
                self.task(
                    cx,
                    move |b| {
                        let bytes = b.attachment(&account, &m, a.part)?;
                        let path = temp_file(&a.name)?;
                        std::fs::write(&path, bytes)?;
                        Ok(path)
                    },
                    move |this, result: anyhow::Result<std::path::PathBuf>, cx| match result {
                        Ok(path) => match how {
                            ToolAction::QuickLook(_) => quick_look(&path),
                            ToolAction::OpenWith(_) => open_with(&path, cx),
                            ToolAction::RevealAttachment(_) => cx.reveal_path(&path),
                            _ => cx.open_with_system(&path),
                        },
                        Err(e) => this.notice(format!("{e:#}"), true, cx),
                    },
                );
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
                        let format = match image::guess_format(&bytes)? {
                            image::ImageFormat::Png => ImageFormat::Png,
                            image::ImageFormat::Gif => ImageFormat::Gif,
                            image::ImageFormat::WebP => ImageFormat::Webp,
                            image::ImageFormat::Jpeg => ImageFormat::Jpeg,
                            other => anyhow::bail!("Can't show {other:?} images"),
                        };
                        Ok((format, bytes))
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
                self.notice("Publishing your public key on keys.openpgp.org…", false, cx);
                self.task(
                    cx,
                    move |b| {
                        let ring = b.keyring();
                        let keys = ring.secret_keys();
                        anyhow::ensure!(!keys.is_empty(), "You have no OpenPGP key of your own to publish.");
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
                            format!(
                                "keys.openpgp.org emailed {} a link: follow it to list your key under that address.",
                                p.confirming.join(", ")
                            ),
                            false,
                            cx,
                        ),
                        Ok(p) if !p.listed.is_empty() => this.notice(
                            format!("Your key is listed on keys.openpgp.org for {}", p.listed.join(", ")),
                            false,
                            cx,
                        ),
                        Ok(_) => this.notice("Uploaded to keys.openpgp.org", false, cx),
                        Err(e) => this.notice(format!("{e:#}"), true, cx),
                    },
                );
            }
            ToolAction::OpenShortcuts => match super::shortcuts::ensure_file() {
                Ok(path) => cx.open_with_system(&path),
                Err(e) => self.notice(format!("Couldn't write the shortcuts file: {e}"), true, cx),
            },
        }
        cx.notify();
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
            if let Some(target) = request.strip_prefix(crate::display::launch::NOTIFICATION) {
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
                    if shown { "Hide" } else { "Preview" },
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
                "Open",
                "external",
                Action::Tools(ToolAction::OpenAttachment(i)),
                false,
                cx,
            )
            .into_any_element(),
        );
        let more: Vec<Entry> = [
            cfg!(target_os = "macos")
                .then(|| entry("Quick Look", "preview", Action::Tools(ToolAction::QuickLook(i)))),
            cfg!(windows).then(|| entry("Open With…", "external", Action::Tools(ToolAction::OpenWith(i)))),
            Some(entry(
                if cfg!(target_os = "macos") { "Show in Finder" } else { "Show in Folder" },
                "download",
                Action::Tools(ToolAction::RevealAttachment(i)),
            )),
            Some(entry("Save…", "download", Action::SaveAttachment(i))),
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
                .tooltip(|w, cx| Tooltip::new("More").build(w, cx))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
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

/// Windows' Open With dialog for the file; elsewhere, its folder, where
/// the file manager offers the apps that open it.
fn open_with(path: &std::path::Path, cx: &mut App) {
    #[cfg(windows)]
    {
        let _ = cx;
        let _ = std::process::Command::new("rundll32.exe")
            .arg("shell32.dll,OpenAs_RunDLL")
            .arg(path)
            .spawn();
    }
    #[cfg(not(windows))]
    cx.reveal_path(path);
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

    #[test]
    fn print_pages_run_only_their_own_script() {
        let m = Mail {
            subject: "Hi <there>".into(),
            from: "a@example.com".into(),
            html: Some("<p onclick=\"x()\">Hello</p><script>alert(1)</script>".into()),
            ..Mail::default()
        };
        let page = print_page(&m, false);
        assert!(page.contains("script-src 'nonce-"));
        assert!(page.contains("img-src data:;"));
        assert!(page.contains("Hi &lt;there&gt;"));
    }
}

//! The message editor. One `Composer` entity is shown floating over the main
//! window (or minimized to a bar, or maximized to fill it), below a message
//! in the reader as an inline reply, or in a window of its own. The app
//! owns sending; the composer edits, formats and saves its draft.
use super::*;
use super::rich_editor::{Look, Mark, RichEditor, RichEvent};
use crate::manage::composing::{ComposeOptions, mentions_attachment};
use crate::manage::richtext::{self as rt, ListKind, html as rhtml};
use gpui_component::input::RopeExt as _;

actions!(
    composer,
    [
        Bold,
        Italic,
        Underline,
        Strike,
        InlineCode,
        InsertLink,
        Bullets,
        Numbers,
        QuoteLines,
        Heading,
        SaveDraft,
        SendDraft,
        TogglePreview
    ]
);

/// Where a composer is shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Placement {
    Floating,
    Maximized,
    /// Below the message it answers, in the reader.
    Inline,
    /// A window of its own.
    Window,
}

pub(super) enum ComposerEvent {
    /// The draft passed its checks and should be sent.
    Send,
    /// Close, keeping the draft (when anything was written).
    Close,
    /// Close and delete the draft.
    Discard,
    /// Move to another placement (pop out, dock, new window).
    Place(Placement),
    /// The draft was saved; lists of drafts may refresh.
    Saved,
    /// Open Forwarding settings to make an alias at a service for a
    /// mailbox: (service, mailbox).
    NewAlias(String, String),
    /// The draft passed its checks and should be sent at this time
    /// (seconds since the epoch): Send Later.
    Schedule(i64),
    /// Keep this as a template (Settings → Composing lists them).
    SaveTemplate(crate::manage::composing::Template),
}
impl EventEmitter<ComposerEvent> for Composer {}

/// A header being dragged to move a floating composer.
#[derive(Clone)]
pub(super) struct ComposerDrag(pub(super) EntityId);
pub(super) struct NoPreview;
impl Render for NoPreview {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

/// Aliases past which the From menu offers a search field.
const FROM_FILTER_AT: usize = 6;

/// A small label naming the forwarding service a message is sent through.
fn service_badge(service: fwd::ForwardingService, c: Colors) -> Div {
    div()
        .flex_shrink_0()
        .px(px(5.))
        .py(px(1.))
        .rounded(px(4.))
        .bg(rgb(c.soft))
        .text_size(px(10.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(rgb(c.muted))
        .child(service.label())
}

/// Just after pasting copied rich text: what went in, so the composer can
/// offer the other version (with or without its formatting) in its place.
/// A paste into the formatted editor and its other version, offered until
/// the text changes.
struct RichPaste {
    plain: String,
    /// The clipboard's HTML.
    html: String,
    /// The version that went in was the formatted one.
    formatted_inserted: bool,
    /// The draft was plain before the paste made it formatted, as it was.
    was_plain: Option<String>,
    /// The editor's revision just after the paste.
    revision: u64,
}

struct PasteOffer {
    /// Byte offset of the pasted text in the body.
    start: usize,
    inserted: String,
    other: String,
    inserted_formatted: bool,
    /// The body's length after pasting; any other edit lets the offer go.
    body_len: usize,
}

/// Recipient fields, in order.
const TO: usize = 0;
const CC: usize = 1;
const BCC: usize = 2;
/// Where replies go, for this message.
const REPLY_TO: usize = 3;

pub(super) struct Composer {
    pub(super) draft: Draft,
    pub(super) placement: Placement,
    /// Shown full screen on a phone or tablet, with touch-sized controls
    /// (display-mobile).
    pub(super) mobile: bool,
    pub(super) minimized: bool,
    /// Top-left inside the main window once moved; until then it sits at the
    /// lower right.
    pub(super) origin: Option<Point<Pixels>>,
    /// Where the pointer held the header when a move began.
    pub(super) grab: Point<Pixels>,
    /// The message an inline reply answers.
    pub(super) answers: Option<String>,
    /// The separate window showing it, for `Placement::Window`.
    pub(super) window: Option<AnyWindowHandle>,
    /// Sending is running or waiting out its undo delay.
    pub(super) sending: bool,
    pub(super) error: Option<String>,
    recipients: [Entity<InputState>; 4],
    subject: Entity<InputState>,
    pub(super) body: Entity<TextareaState>,
    /// The formatted editor: the body while the draft is formatted.
    pub(super) rich: Entity<RichEditor>,
    /// What switching to plain text took off: the text as it left it and
    /// the formatting, put back if switched again with the text unchanged.
    unformatted: Option<(String, String)>,
    /// The address being written for a link (⌘K) in the formatted editor.
    link_input: Option<Entity<InputState>>,
    rich_paste: Option<RichPaste>,
    show_cc: bool,
    show_bcc: bool,
    show_reply_to: bool,
    preview: bool,
    /// The last paste's other version, offered until the body changes.
    paste: Option<PasteOffer>,
    from_open: bool,
    /// When a press outside closed the From menu: the same press on its
    /// button must not open it again.
    from_closed: Option<std::time::Instant>,
    /// Narrows a long From menu.
    from_filter: Entity<InputState>,
    /// Send Later's menu is open.
    later_open: bool,
    /// The templates menu is open.
    templates_open: bool,
    /// The "Remind me if no reply" menu is open.
    follow_up_open: bool,
    /// Send Later's date and time picker, while its menu is open.
    later_picker: Option<Entity<DateTimePicker>>,
    later_subscription: Option<Subscription>,
    /// Reads a time typed into Send Later (Local AI), when it is on.
    pub(super) when: Option<Rc<crate::manage::local_ai::when::Parser>>,
    accounts: Vec<Account>,
    /// Aliases at forwarding services that can send.
    aliases: Vec<fwd::SendAlias>,
    /// Services that can make aliases to send as: (service, the mailbox it
    /// forwards to, or `None` for any).
    alias_services: Vec<(String, fwd::ForwardingService, Option<String>)>,
    /// Accounts that send only as an alias (Forwarding settings).
    alias_only: HashSet<String>,
    /// Known people (name, address) offered while typing a recipient.
    contacts: Rc<Vec<(String, String)>>,
    suggestions: Vec<(String, String)>,
    suggest_field: usize,
    pub(super) options: ComposeOptions,
    pub(super) colors: Colors,
    backend: Arc<Backend>,
    revision: u64,
    saved: bool,
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl Composer {
    #[allow(clippy::too_many_arguments)]
    /// Shows the draft's fields again after they were changed in code.
    pub(super) fn reload_fields(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let values = [
            self.draft.to.clone(),
            self.draft.cc.clone(),
            self.draft.bcc.clone(),
            self.draft.reply_to.clone(),
        ];
        for (state, value) in self.recipients.iter().zip(values) {
            state.update(cx, |s, cx| s.set_text(value, window, cx));
        }
        let subject = self.draft.subject.clone();
        self.subject.update(cx, |s, cx| s.set_text(subject, window, cx));
        let body = self.draft.body.clone();
        self.body.update(cx, |s, cx| s.set_text(body, window, cx));
        if self.draft.formatted {
            let doc = rich_doc(&mut self.draft);
            self.rich.update(cx, |r, cx| r.set_doc(doc, cx));
        }
    }
    pub(super) fn new(
        mut draft: Draft,
        placement: Placement,
        accounts: Vec<Account>,
        aliases: Vec<fwd::SendAlias>,
        contacts: Rc<Vec<(String, String)>>,
        options: ComposeOptions,
        colors: Colors,
        backend: Arc<Backend>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // Software keyboards (phones) take their hints from these: Next
        // between the address fields, corrections and capitals only where
        // people write sentences.
        let field = |placeholder: &str, window: &mut Window, cx: &mut Context<Self>| {
            cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(placeholder.to_owned())
                    .input_action(TextInputAction::Next)
            })
        };
        let recipients = [
            field(t!("composer-recipients"), window, cx),
            field(t!("composer-cc"), window, cx),
            field(t!("composer-bcc"), window, cx),
            field(t!("composer-where-replies-go"), window, cx),
        ];
        let subject = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t!("composer-subject"))
                .input_action(TextInputAction::Next)
                .autocorrect(true)
                .autocapitalize(Autocapitalize::Sentences)
        });
        let from_filter = field(t!("composer-find-an-address"), window, cx);
        let body = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder(t!("composer-write-your-message"))
                .autocorrect(true)
                .autocapitalize(Autocapitalize::Sentences)
        });
        let values = [&draft.to, &draft.cc, &draft.bcc, &draft.reply_to];
        for (state, value) in recipients.iter().zip(values) {
            let value = value.clone();
            state.update(cx, |s, cx| s.set_text(value, window, cx));
        }
        subject.update(cx, |s, cx| s.set_text(draft.subject.clone(), window, cx));
        body.update(cx, |s, cx| s.set_text(draft.body.clone(), window, cx));
        let doc = if draft.formatted { rich_doc(&mut draft) } else { rt::Doc::empty() };
        crate::manage::spelling::spelling().use_personal(backend.store.path.join("dictionary.txt"));
        let rich = cx.new(|cx| RichEditor::new(doc, t!("composer-write-your-message"), look_of(colors, false), cx));
        let mut subscriptions = vec![];
        subscriptions.push(cx.subscribe(&rich, |this, rich, _: &RichEvent, cx| {
            // Another edit after a paste lets its offer go.
            let revision = rich.read(cx).revision;
            if this.rich_paste.as_ref().is_some_and(|p| p.revision != revision) {
                this.rich_paste = None;
            }
            this.preview = false;
            this.changed(cx);
        }));
        for (i, state) in recipients.iter().enumerate() {
            subscriptions.push(cx.subscribe_in(state, window, move |this, _, e, w, cx| {
                match e {
                    InputEvent::Change => {
                        this.suggest(i, cx);
                        this.changed(cx);
                    }
                    InputEvent::PressEnter { .. } => this.accept_suggestion(0, w, cx),
                    InputEvent::Blur => {
                        // Leave time for a click on a suggestion.
                        cx.spawn(async move |this, cx| {
                            cx.background_executor().timer(Duration::from_millis(150)).await;
                            let _ = this.update(cx, |this, cx| {
                                if this.suggest_field == i {
                                    this.suggestions.clear();
                                    cx.notify();
                                }
                            });
                        })
                        .detach();
                    }
                    _ => {}
                }
            }));
        }
        subscriptions.push(cx.subscribe(&from_filter, |_, _, e, cx| {
            if matches!(e, InputEvent::Change) {
                cx.notify();
            }
        }));
        subscriptions.push(cx.subscribe(&subject, |this, _, e, cx| {
            if matches!(e, InputEvent::Change) {
                this.changed(cx);
            }
        }));
        subscriptions.push(cx.subscribe(&body, |this, body, e, cx| {
            if matches!(e, InputEvent::Change) {
                // Typing or another edit after a paste lets its offer go.
                let len = body.read(cx).value().len();
                if this.paste.as_ref().is_some_and(|p| p.body_len != len) {
                    this.paste = None;
                }
                this.changed(cx);
            }
        }));
        let show_cc = !draft.cc.is_empty();
        let show_bcc = !draft.bcc.is_empty();
        let show_reply_to = !draft.reply_to.is_empty();
        Self {
            draft,
            placement,
            mobile: false,
            minimized: false,
            origin: None,
            grab: Point::default(),
            answers: None,
            window: None,
            sending: false,
            error: None,
            recipients,
            subject,
            body,
            rich,
            unformatted: None,
            link_input: None,
            rich_paste: None,
            show_cc,
            show_bcc,
            show_reply_to,
            preview: false,
            paste: None,
            from_open: false,
            from_closed: None,
            from_filter,
            later_open: false,
            templates_open: false,
            follow_up_open: false,
            later_picker: None,
            later_subscription: None,
            when: None,
            accounts,
            aliases,
            alias_services: vec![],
            alias_only: HashSet::new(),
            contacts,
            suggestions: vec![],
            suggest_field: TO,
            options,
            colors,
            backend,
            revision: 0,
            saved: true,
            focus: cx.focus_handle(),
            _subscriptions: subscriptions,
        }
    }

    /// Focuses the first empty field: recipients, then subject, then body.
    pub(super) fn focus_first(&self, window: &mut Window, cx: &mut App) {
        let target = if self.recipients[TO].read(cx).value().trim().is_empty()
            && self.placement != Placement::Inline
        {
            Some(&self.recipients[TO])
        } else if self.subject.read(cx).value().trim().is_empty()
            && self.placement != Placement::Inline
        {
            Some(&self.subject)
        } else {
            None
        };
        match target {
            Some(target) => target.update(cx, |s, cx| s.focus(window, cx)),
            None if self.draft.formatted => self.rich.update(cx, |r, cx| r.put_caret_at_start(window, cx)),
            None => self.body.update(cx, |s, cx| {
                s.focus(window, cx);
                // Replies start writing above the quoted message.
                s.set_cursor_position(gpui_component::input::Position::new(0, 0), window, cx);
            }),
        }
    }

    /// Shows the formatted message as it will look, or the editor again.
    pub(super) fn toggle_preview(&mut self, cx: &mut Context<Self>) {
        if self.draft.formatted {
            self.preview = !self.preview;
            cx.notify();
        }
    }

    pub(super) fn previewing(&self) -> bool {
        self.preview && self.draft.formatted
    }

    /// The text fields a phone's keyboard serves, with the keyboard each
    /// wants: addresses get the email layout.
    pub(super) fn keyboard_fields(&self, cx: &App) -> Vec<(FocusHandle, super::mobile::Keys)> {
        let mut out: Vec<_> = self
            .recipients
            .iter()
            .chain([&self.from_filter])
            .map(|s| (s.read(cx).focus_handle(cx), super::mobile::Keys::Email))
            .collect();
        out.push((self.subject.read(cx).focus_handle(cx), super::mobile::Keys::Text));
        out.push((self.body.read(cx).focus_handle(cx), super::mobile::Keys::Text));
        out.push((self.rich.read(cx).focus_handle_ref(), super::mobile::Keys::Text));
        if let Some(focus) = self.later_picker.as_ref().and_then(|p| p.read(cx).typed_focus(cx)) {
            out.push((focus, super::mobile::Keys::Text));
        }
        out
    }

    /// The draft as currently written.
    pub(super) fn snapshot(&mut self, cx: &App) -> Draft {
        let value = |s: &Entity<InputState>| s.read(cx).value().to_string();
        self.draft.to = value(&self.recipients[TO]);
        self.draft.cc = value(&self.recipients[CC]);
        self.draft.bcc = value(&self.recipients[BCC]);
        self.draft.reply_to = value(&self.recipients[REPLY_TO]).trim().to_owned();
        self.draft.subject = value(&self.subject);
        if self.draft.formatted {
            // The HTML is the message; its text goes with it.
            let rich = self.rich.read(cx);
            self.draft.html = rich.html();
            self.draft.body = rich.plain();
        } else {
            self.draft.body = self.body.read(cx).value().to_string();
            self.draft.html.clear();
        }
        self.draft.clone()
    }

    /// The window and tab title: the subject, or what kind of message it is.
    pub(super) fn title(&self, cx: &App) -> String {
        let subject = self.subject.read(cx).value().trim().to_string();
        if self.placement == Placement::Inline {
            return if self.draft.subject.to_lowercase().starts_with("fwd:") {
                t!("composer-forward").into()
            } else {
                t!("composer-reply").into()
            };
        }
        if subject.is_empty() {
            t!("composer-new-message").into()
        } else {
            subject
        }
    }

    /// Whether you have a key or certificate to sign or encrypt with.
    fn can_protect(&self) -> bool {
        !self.backend.keyring().secret.is_empty() || !self.backend.certificates().own.is_empty()
    }
    /// Whether you have an OpenPGP key of your own.
    fn can_protect_openpgp(&self) -> bool {
        !self.backend.keyring().secret.is_empty()
    }
    /// Whether it goes as inline PGP: asked for, and protected with
    /// OpenPGP (not S/MIME, which is used too when there's no OpenPGP key).
    fn sends_inline_pgp(&self) -> bool {
        self.draft.pgp_inline && !self.draft.smime && self.can_protect_openpgp()
    }
    /// Whether you have both an OpenPGP key and an S/MIME certificate.
    fn has_both_kinds(&self) -> bool {
        !self.backend.keyring().secret.is_empty() && !self.backend.certificates().own.is_empty()
    }
    fn changed(&mut self, cx: &mut Context<Self>) {
        self.saved = false;
        self.error = None;
        self.revision += 1;
        let revision = self.revision;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(800)).await;
            let _ = this.update(cx, |this, cx| {
                if this.revision == revision {
                    this.save(cx);
                }
            });
        })
        .detach();
        cx.notify();
    }

    /// Saves the draft off the main thread. An untouched draft is not kept.
    pub(super) fn save(&mut self, cx: &mut Context<Self>) {
        if self.sending {
            return;
        }
        let draft = self.snapshot(cx);
        if draft.is_empty() {
            return;
        }
        let store = self.backend.store.clone();
        // In order with the other draft writes (`queue_draft_write`).
        let saved = queue_draft_write(move || store.save_draft(&draft));
        cx.spawn(async move |this, cx| {
            let result = saved
                .await
                .unwrap_or_else(|_| Err(anyhow::anyhow!(t!("ui-something-went-wrong-while-doing-this"))));
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(()) => {
                        this.saved = true;
                        cx.emit(ComposerEvent::Saved);
                    }
                    Err(e) => this.error = Some(format!("{e:#}")),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Saves the draft as it is now, for closing, sending and quitting:
    /// what is written is taken here, and written off this thread in order
    /// with the other draft writes (`queue_draft_write`). Quitting waits for
    /// it (`MailApp::persist_draft`); what reads drafts waits for it too.
    pub(super) fn save_now(&mut self, cx: &App) {
        if self.sending {
            return;
        }
        let draft = self.snapshot(cx);
        if !draft.is_empty() {
            let store = self.backend.store.clone();
            drop(queue_draft_write(move || store.save_draft(&draft)));
        }
    }

    // Recipient suggestions.

    fn suggest(&mut self, field: usize, cx: &App) {
        let value = self.recipients[field].read(cx).value().to_string();
        let token = value.rsplit(',').next().unwrap_or("").trim().to_lowercase();
        self.suggest_field = field;
        self.suggestions = if token.len() < 2 {
            vec![]
        } else {
            let chosen = value.to_lowercase();
            self.contacts
                .iter()
                .filter(|(name, email)| {
                    (name.to_lowercase().contains(&token) || email.to_lowercase().contains(&token))
                        && !chosen.contains(&email.to_lowercase())
                })
                .take(6)
                .cloned()
                .collect()
        };
    }

    fn accept_suggestion(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some((name, email)) = self.suggestions.get(index).cloned() else {
            return;
        };
        let field = self.suggest_field;
        let value = self.recipients[field].read(cx).value().to_string();
        let kept: Vec<&str> = value
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect();
        let mut kept = kept[..kept.len().saturating_sub(1)].to_vec();
        let entry = if name.is_empty() || name == email {
            email
        } else if name.contains([',', '"', '<']) {
            format!("\"{}\" <{email}>", name.replace('"', ""))
        } else {
            format!("{name} <{email}>")
        };
        kept.push(&entry);
        let next = format!("{}, ", kept.join(", "));
        self.suggestions.clear();
        self.recipients[field].update(cx, |s, cx| {
            s.set_text(next.clone(), window, cx);
            let end = s.text().offset_to_position(next.len());
            s.set_cursor_position(end, window, cx);
        });
        cx.notify();
    }

    // Formatting. A formatted draft is edited as it looks (`rich`); a
    // plain one as text, which formatting turns into a formatted one.

    /// The selection as a byte range of the body's text.
    fn selection(&self, window: &mut Window, cx: &mut App) -> (String, std::ops::Range<usize>) {
        self.body.update(cx, |s, cx| {
            let text = s.value().to_string();
            let utf16 = s
                .selected_text_range(true, window, cx)
                .map(|r| r.range)
                .unwrap_or(0..0);
            let to_byte = |u: usize| {
                let mut count = 0;
                for (i, ch) in text.char_indices() {
                    if count >= u {
                        return i;
                    }
                    count += ch.len_utf16();
                }
                text.len()
            };
            let range = to_byte(utf16.start)..to_byte(utf16.end);
            (text, range)
        })
    }

    /// Replaces a byte range of the body and puts the cursor at `cursor`
    /// (a byte offset in the new text).
    fn edit(
        &mut self,
        range: std::ops::Range<usize>,
        with: &str,
        cursor: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.body.update(cx, |s, cx| {
            let text = s.value().to_string();
            let utf16 = |b: usize| text[..b].encode_utf16().count();
            let range16 = utf16(range.start)..utf16(range.end);
            s.replace_text_in_range(Some(range16), with, window, cx);
            let position = s.text().offset_to_position(cursor);
            s.set_cursor_position(position, window, cx);
        });
        self.preview = false;
        self.changed(cx);
    }

    /// Writes a suggested reply at the top of the body, above any signature
    /// or quote, with the cursor after it to go on writing.
    pub(super) fn insert_reply_text(
        &mut self,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.draft.formatted {
            let fragment = rt::Doc::from_text(text);
            self.rich.update(cx, |r, cx| r.insert_at_start(&fragment, Some(window), cx));
            self.rich.update(cx, |r, cx| r.focus(window, cx));
            return;
        }
        let body = self.body.read(cx).value().to_string();
        // What was written already stays; the reply goes before it.
        let with = if body.trim().is_empty() || body.starts_with('\n') {
            text.to_owned()
        } else {
            format!("{text}\n\n")
        };
        self.edit(0..0, &with, text.len(), window, cx);
        self.put_cursor(text.len(), window, cx);
    }

    /// Focuses the body with the cursor at byte `offset` of its text.
    pub(super) fn put_cursor(&self, offset: usize, window: &mut Window, cx: &mut App) {
        if self.draft.formatted {
            self.rich.update(cx, |r, cx| r.put_caret_near(offset, window, cx));
            return;
        }
        self.body.update(cx, |s, cx| {
            s.focus(window, cx);
            let position = s.text().offset_to_position(offset);
            s.set_cursor_position(position, window, cx);
        });
    }

    /// Turns a format on or off for the selection (or what's typed next),
    /// making the draft formatted first if it isn't.
    pub(super) fn format(&mut self, mark: Mark, window: &mut Window, cx: &mut Context<Self>) {
        if !self.draft.formatted {
            self.set_formatted(true, window, cx);
        }
        self.preview = false;
        self.rich.update(cx, |r, cx| {
            r.toggle_mark(mark, Some(window), cx);
            r.focus(window, cx);
        });
    }

    pub(super) fn prefix_lines(&mut self, kind: Lines, window: &mut Window, cx: &mut Context<Self>) {
        if !self.draft.formatted {
            self.set_formatted(true, window, cx);
        }
        self.preview = false;
        self.rich.update(cx, |r, cx| match kind {
            Lines::Bullets => r.toggle_list(ListKind::Bullets, Some(window), cx),
            Lines::Numbers => r.toggle_list(ListKind::Numbers, Some(window), cx),
            Lines::Quote => r.toggle_quote(Some(window), cx),
            Lines::Heading => r.toggle_heading(Some(window), cx),
        });
        self.rich.update(cx, |r, cx| r.focus(window, cx));
    }

    fn insert_link(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.draft.formatted {
            self.set_formatted(true, window, cx);
        }
        self.preview = false;
        let (selected, existing) = {
            let r = self.rich.read(cx);
            (r.selected_text().trim().to_owned(), r.link())
        };
        // A selected address links to itself.
        let url_like = !selected.contains(char::is_whitespace)
            && (selected.starts_with("http://") || selected.starts_with("https://") || selected.starts_with("www."));
        if url_like && existing.is_none() {
            let url = if selected.starts_with("www.") { format!("https://{selected}") } else { selected };
            self.rich.update(cx, |r, cx| r.set_link(Some(url), Some(window), cx));
            return;
        }
        // Otherwise its address is asked for, above the text.
        let initial = existing.unwrap_or_else(|| "https://".into());
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t!("composer-link-address"))
                .input_action(TextInputAction::Done)
        });
        input.update(cx, |s, cx| {
            s.set_text(initial.clone(), window, cx);
            let end = s.text().offset_to_position(initial.len());
            s.set_cursor_position(end, window, cx);
            s.focus(window, cx);
        });
        self._subscriptions.push(cx.subscribe_in(&input, window, |this, input, e, w, cx| match e {
            InputEvent::PressEnter { .. } => {
                let url = input.read(cx).value().to_string();
                let url = url.trim();
                let url = (url != "https://" && url != "http://").then(|| url.to_owned());
                this.link_input = None;
                this.rich.update(cx, |r, cx| {
                    r.set_link(url, Some(w), cx);
                    r.focus(w, cx);
                });
                cx.notify();
            }
            InputEvent::Blur => {
                this.link_input = None;
                cx.notify();
            }
            _ => {}
        }));
        self.link_input = Some(input);
        cx.notify();
    }

    /// Pastes copied rich text as plain text or with its formatting, per
    /// Settings, and offers the other. Returns false when the clipboard has
    /// no formatting, leaving the paste to the text field.
    fn paste(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        self.paste = None;
        self.rich_paste = None;
        // A copied picture or files: attached.
        if let Some(item) = cx.read_from_clipboard() {
            let mut files = Vec::new();
            // Pictures from the clipboard, as against copied files.
            let mut pictures = Vec::new();
            for entry in item.entries() {
                match entry {
                    ClipboardEntry::Image(image) if !image.bytes.is_empty() => {
                        let extension = image.format.mime_type().rsplit('/').next().unwrap_or("png");
                        let extension = extension.split('+').next().unwrap_or(extension);
                        let name = t!("composer-pasted-image", extension = extension);
                        match self.backend.save_pasted(&self.draft.id, &name, &image.bytes) {
                            Ok(path) => {
                                pictures.push(path.clone());
                                files.push(path)
                            }
                            Err(e) => eprintln!("A pasted picture couldn't be kept: {e:#}"),
                        }
                    }
                    ClipboardEntry::ExternalPaths(paths) => {
                        files.extend(paths.paths().iter().cloned())
                    }
                    _ => {}
                }
            }
            if !files.is_empty() && item.text().is_none_or(|t| t.trim().is_empty()) {
                self.add_files(files, cx);
                self.place_pictures(&pictures, window, cx);
                return true;
            }
        }
        let Some(plain) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return false;
        };
        // Copied from a formatted draft: as it was.
        if self.draft.formatted
            && let Some(fragment) = RichEditor::copied_fragment(&plain)
        {
            self.rich.update(cx, |r, cx| r.insert_doc(&fragment, Some(window), cx));
            return true;
        }
        let Some(pasted) = super::paste::Pasted::from_clipboard(plain) else {
            return false;
        };
        self.insert_pasted(pasted, window, cx);
        true
    }

    /// A formatted draft shows pasted pictures where they were pasted
    /// (sent inline, RFC 2387). A plain draft only attaches them.
    pub(super) fn place_pictures(
        &mut self,
        pictures: &[std::path::PathBuf],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.draft.formatted || pictures.is_empty() {
            return;
        }
        self.preview = false;
        self.rich.update(cx, |r, cx| r.insert_pictures(pictures, Some(window), cx));
    }

    /// Pasted HTML as the formatted editor inserts it. Pictures the page
    /// itself carried (`data:`) become the draft's attachments; others
    /// (remote pictures, which could tell the sender each recipient read
    /// it) are left out.
    fn pasted_doc(&mut self, html: &str, cx: &mut Context<Self>) -> rt::Doc {
        // Without what would load from the page's server (`parse_pasted`).
        let mut doc = rhtml::parse_pasted(html);
        let mut saved = Vec::new();
        for i in 0..doc.blocks.len() {
            if !doc.blocks[i].runs.iter().any(|r| r.image.is_some()) {
                continue;
            }
            let b = doc.block_mut(i);
            let mut runs = Vec::with_capacity(b.runs.len());
            for run in b.runs.drain(..) {
                let Some(image) = run.image.clone() else {
                    runs.push(run);
                    continue;
                };
                let data = image.src.strip_prefix("data:").and_then(|d| d.split_once(";base64,"));
                let bytes = data.and_then(|(_, payload)| {
                    use base64::Engine as _;
                    base64::engine::general_purpose::STANDARD.decode(payload.trim()).ok()
                });
                let (Some((meta, _)), Some(bytes)) = (data, bytes) else {
                    continue;
                };
                // Letters and digits only: the type is the pasted page's.
                let extension = crate::manage::backend::picture_extension(meta);
                let name = t!("composer-pasted-image", extension = extension);
                match self.backend.save_pasted(&self.draft.id, &name, &bytes) {
                    Ok(path) => {
                        let file = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                        saved.push(path);
                        runs.push(rt::Run {
                            image: Some(Rc::new(rt::Image { src: format!("file:{file}"), ..(*image).clone() })),
                            ..run
                        });
                    }
                    Err(e) => eprintln!("A pasted picture couldn't be kept: {e:#}"),
                }
            }
            b.runs = runs;
            b.normalize();
        }
        if !saved.is_empty() {
            self.add_files(saved, cx);
        }
        doc
    }

    pub(super) fn insert_pasted(
        &mut self,
        pasted: super::paste::Pasted,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let formatted = self.options.paste_formatted;
        // A plain draft keeps a plain paste in its text.
        if !self.draft.formatted && !formatted {
            let (_, range) = self.selection(window, cx);
            let start = range.start;
            self.edit(range, &pasted.plain, start + pasted.plain.len(), window, cx);
            self.paste = Some(PasteOffer {
                start,
                inserted: pasted.plain,
                other: pasted.html,
                inserted_formatted: false,
                body_len: self.body.read(cx).value().len(),
            });
            cx.notify();
            return;
        }
        let was_plain = (!self.draft.formatted).then(|| self.body.read(cx).value().to_string());
        if !self.draft.formatted {
            self.set_formatted(true, window, cx);
        }
        if formatted {
            let doc = self.pasted_doc(&pasted.html, cx);
            self.rich.update(cx, |r, cx| r.insert_doc(&doc, Some(window), cx));
        } else {
            self.rich.update(cx, |r, cx| r.insert_plain(&pasted.plain, Some(window), cx));
        }
        self.rich_paste = Some(RichPaste {
            plain: pasted.plain,
            html: pasted.html,
            formatted_inserted: formatted,
            was_plain,
            revision: self.rich.read(cx).revision,
        });
        cx.notify();
    }

    /// Puts the last paste's other version in its place.
    pub(super) fn swap_paste(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // A plain paste in a plain draft: formatted, the draft becomes so.
        if let Some(offer) = self.paste.take() {
            let end = offer.start + offer.inserted.len();
            let text = self.body.read(cx).value().to_string();
            if text.get(offer.start..end) != Some(offer.inserted.as_str()) {
                cx.notify();
                return;
            }
            self.edit(offer.start..end, "", offer.start, window, cx);
            let before = self.body.read(cx).value().to_string();
            self.set_formatted(true, window, cx);
            let doc = self.pasted_doc(&offer.other, cx);
            self.rich.update(cx, |r, cx| {
                r.insert_doc(&doc, Some(window), cx);
                r.focus(window, cx);
            });
            self.rich_paste = Some(RichPaste {
                plain: offer.inserted,
                html: offer.other,
                formatted_inserted: true,
                was_plain: Some(before),
                revision: self.rich.read(cx).revision,
            });
            cx.notify();
            return;
        }
        let Some(offer) = self.rich_paste.take() else {
            return;
        };
        if self.rich.read(cx).revision != offer.revision || !self.draft.formatted {
            cx.notify();
            return;
        }
        self.rich.update(cx, |r, cx| r.undo(Some(window), cx));
        if offer.formatted_inserted {
            // The paste made the draft formatted: plain again, as it was,
            // with the text pasted where the paste went.
            if let Some(before) = offer.was_plain {
                let at = self.rich.read(cx).sel.start();
                let offset: usize = before
                    .split('\n')
                    .take(at.block)
                    .map(|l| l.len() + 1)
                    .sum::<usize>()
                    + at.offset;
                self.draft.formatted = false;
                self.unformatted = None;
                self.body.update(cx, |s, cx| s.set_text(before.clone(), window, cx));
                let offset = offset.min(before.len());
                self.edit(offset..offset, &offer.plain, offset + offer.plain.len(), window, cx);
                self.body.update(cx, |s, cx| s.focus(window, cx));
                self.paste = Some(PasteOffer {
                    start: offset,
                    inserted: offer.plain,
                    other: offer.html,
                    inserted_formatted: false,
                    body_len: self.body.read(cx).value().len(),
                });
                self.changed(cx);
                return;
            }
            self.rich.update(cx, |r, cx| r.insert_plain(&offer.plain, Some(window), cx));
        } else {
            let doc = self.pasted_doc(&offer.html, cx);
            self.rich.update(cx, |r, cx| r.insert_doc(&doc, Some(window), cx));
        }
        self.rich.update(cx, |r, cx| r.focus(window, cx));
        self.rich_paste = Some(RichPaste {
            formatted_inserted: !offer.formatted_inserted,
            revision: self.rich.read(cx).revision,
            ..offer
        });
        cx.notify();
    }

    pub(super) fn paste_offer_label(&self) -> Option<&'static str> {
        let formatted = match (&self.rich_paste, &self.paste) {
            (Some(p), _) => p.formatted_inserted,
            (None, Some(p)) => p.inserted_formatted,
            (None, None) => return None,
        };
        Some(if formatted {
            t!("composer-paste-without-formatting")
        } else {
            t!("composer-paste-with-formatting")
        })
    }

    /// The button offering the last paste's other version.
    fn paste_offer(&self, cx: &mut Context<Self>) -> Option<Stateful<Div>> {
        let label = self.paste_offer_label()?;
        let c = self.colors;
        Some(
            div()
                .id("compose-paste-offer")
                .debug_selector(|| "compose-paste-offer".into())
                .absolute()
                .bottom(px(8.))
                .right(px(12.))
                .h(px(if self.mobile { 36. } else { 28. }))
                .px_3()
                .flex()
                .items_center()
                .gap_2()
                .rounded(px(8.))
                .border_1()
                .border_color(rgb(c.border))
                .bg(rgb(c.surface))
                .shadow_md()
                .text_size(px(if self.mobile { 15. } else { 12. }))
                .font_weight(FontWeight::MEDIUM)
                .text_color(rgb(c.accent))
                .cursor_pointer()
                .hover(|s| s.bg(rgb(c.soft)))
                .child(label)
                .on_mouse_down(MouseButton::Left, |_, window, cx| {
                    // Keep the body focused (and its keyboard up).
                    window.prevent_default();
                    cx.stop_propagation();
                })
                .on_click(cx.listener(|this, _, window, cx| this.swap_paste(window, cx))),
        )
    }

    /// Makes the draft formatted (its text, or the formatting it had when
    /// the text is as switching to plain left it) or plain (its text).
    pub(super) fn set_formatted(&mut self, on: bool, window: &mut Window, cx: &mut Context<Self>) {
        if on == self.draft.formatted {
            return;
        }
        if on {
            let text = self.body.read(cx).value().to_string();
            let focused = self.body.read(cx).focus_handle(cx).is_focused(window);
            let (_, range) = self.selection(window, cx);
            let doc = match self.unformatted.take() {
                Some((plain, html)) if plain == text => rhtml::parse(&html, true),
                _ => rt::Doc::from_text(&text),
            };
            // The caret stays where it was: a line of text is a block.
            let at = |byte: usize| {
                let before = &text[..byte.min(text.len())];
                let block = before.matches('\n').count();
                let offset = before.rfind('\n').map_or(before.len(), |i| before.len() - i - 1);
                rt::Pos::new(block, offset)
            };
            let (start, end) = (at(range.start), at(range.end));
            self.rich.update(cx, |r, cx| {
                r.set_doc(doc, cx);
                r.sel.anchor = r.doc.clamp(start);
                r.sel.head = r.doc.clamp(end);
                if focused {
                    r.focus(window, cx);
                }
            });
        } else {
            let (plain, html, focused) = {
                let r = self.rich.read(cx);
                (r.plain(), r.html(), r.is_focused(window))
            };
            self.unformatted = Some((plain.clone(), html));
            self.body.update(cx, |s, cx| {
                s.set_text(plain, window, cx);
                if focused {
                    s.focus(window, cx);
                }
            });
            self.preview = false;
            self.link_input = None;
        }
        self.draft.formatted = on;
        self.changed(cx);
    }

    // Sending.

    /// Checks the draft and asks the app to send it. Missing recipients stop
    /// here; a missing subject or attachment asks first.
    pub(super) fn request_send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.request_send_at(None, window, cx)
    }

    /// Like `request_send`, for sending at `at` (Send Later) when given.
    pub(super) fn request_send_at(
        &mut self,
        at: Option<i64>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.sending {
            return;
        }
        self.close_later();
        if at.is_some_and(|t| t <= chrono::Utc::now().timestamp() + 30) {
            self.error = Some(t!("composer-choose-a-time-in-the-future").into());
            cx.notify();
            return;
        }
        let event = move || match at {
            Some(t) => ComposerEvent::Schedule(t),
            None => ComposerEvent::Send,
        };
        if self.needs_alias() {
            let email = self
                .accounts
                .iter()
                .find(|a| a.id == self.draft.account)
                .map_or(String::new(), |a| a.email.clone());
            self.error = Some(t!("composer-sends-only-as-its-aliases-choose", email = email));
            self.from_open = true;
            cx.notify();
            return;
        }
        let d = self.snapshot(cx);
        let any = [&d.to, &d.cc, &d.bcc].iter().any(|f| !f.trim().is_empty());
        if !any {
            self.error = Some(t!("composer-add-at-least-one-recipient").into());
            self.recipients[TO].update(cx, |s, cx| s.focus(window, cx));
            cx.notify();
            return;
        }
        if let Err(e) = crate::sync::smtp::recipients(&d.reply_to) {
            self.error = Some(t!("composer-reply-to", e = format!("{:#}", e)));
            cx.notify();
            return;
        }
        for list in [&d.to, &d.cc, &d.bcc] {
            if let Err(e) = crate::sync::smtp::recipients(list) {
                self.error = Some(format!("{e:#}"));
                cx.notify();
                return;
            }
        }
        // RFC 9788 §6.1: an answer to encrypted mail, about to go
        // unencrypted, mustn't carry the original's text or hidden subject.
        // Asked first; sent, those are left out (see `send_in_clear`).
        if !d.encrypt && self.answers_encrypted(&d) {
            let answer = window.prompt(
                PromptLevel::Warning,
                t!("composer-send-unencrypted"),
                Some(crate::manage::protected_reply::WARNING),
                &[t!("composer-send-without-the-quote"), t!("composer-keep-editing")],
                cx,
            );
            cx.spawn_in(window, async move |this, cx| {
                if let Ok(0) = answer.await {
                    let _ = this.update_in(cx, |this, window, cx| {
                        this.send_in_clear(window, cx);
                        cx.emit(event());
                    });
                }
            })
            .detach();
            return;
        }
        let mut doubts = vec![];
        if self.options.warn_empty_subject && d.subject.trim().is_empty() {
            doubts.push(t!("composer-this-message-has-no-subject"));
        }
        if self.options.attachment_reminder
            && d.attachments.is_empty()
            && mentions_attachment(&d.body)
        {
            doubts.push(t!("composer-the-message-mentions-an-attachment-but"));
        }
        if doubts.is_empty() {
            cx.emit(event());
            return;
        }
        let answer = window.prompt(
            PromptLevel::Warning,
            if at.is_some() {
                t!("composer-schedule-anyway")
            } else {
                t!("composer-send-anyway")
            },
            Some(&doubts.join("\n")),
            &[
                if at.is_some() { t!("composer-schedule") } else { t!("composer-send") },
                t!("composer-keep-editing"),
            ],
            cx,
        );
        cx.spawn(async move |this, cx| {
            if let Ok(0) = answer.await {
                let _ = this.update(cx, |_, cx| cx.emit(event()));
            }
        })
        .detach();
    }

    /// Whether `d` answers a message that came encrypted (RFC 9788 §6.1).
    fn answers_encrypted(&self, d: &Draft) -> bool {
        !d.original.is_empty()
            && self
                .backend
                .store
                .message(&d.original)
                .ok()
                .flatten()
                .is_some_and(|m| self.backend.answering(&m).encrypted)
    }

    /// RFC 9788 §6.1, for an answer to encrypted mail sent in the clear:
    /// the original's hidden subject becomes "...", and the quote of it is
    /// left out. Shown in the fields too, so what goes is what is seen.
    pub(super) fn send_in_clear(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let d = self.snapshot(cx);
        let Some(original) = self.backend.store.message(&d.original).ok().flatten() else {
            return;
        };
        let original = self.backend.answering(&original);
        let clear = crate::manage::protected_reply::cleartext(&original, &d.subject, &d.body);
        self.draft.subject = clear.subject;
        self.draft.encrypt = false;
        self.draft.encrypt_subject = false;
        if self.draft.formatted {
            // The quote's blocks go: from its introduction, or the first
            // quoted line.
            let doc = &self.rich.read(cx).doc;
            let from = doc
                .quote_start()
                .or_else(|| doc.blocks.iter().position(|b| b.path.iter().any(|e| e.is("blockquote"))));
            if let Some(from) = from.filter(|_| clear.body != d.body) {
                self.rich.update(cx, |r, cx| r.truncate_blocks(from, cx));
            }
            let subject = self.draft.subject.clone();
            self.subject.update(cx, |s, cx| s.set_text(subject, window, cx));
            self.snapshot(cx);
        } else {
            self.draft.body = clear.body;
            self.reload_fields(window, cx);
        }
        self.changed(cx);
    }

    /// Opens Send Later's picker, at the time already chosen if any.
    fn open_later(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let presets = crate::manage::composing::later_choices(chrono::Local::now());
        let colors = self.colors;
        let at = self.draft.send_at;
        let parser = self.when.clone();
        let mobile = self.mobile;
        // On a phone the page covers the fields: let go of the one being
        // typed in, and with it the keyboard.
        if mobile {
            self.focus.focus(window, cx);
        }
        let picker = cx.new(|cx| {
            let picker = DateTimePicker::new("later", t!("composer-send"), presets, colors).phone(mobile);
            let picker = match at {
                Some(at) => picker.at(at),
                None => picker,
            };
            picker.typed(parser, window, cx)
        });
        self.later_subscription = Some(cx.subscribe_in(&picker, window, |this, _, e, w, cx| {
            match e {
                PickerEvent::Picked(at) => this.request_send_at(Some(*at), w, cx),
                PickerEvent::Cancel => {
                    this.close_later();
                    cx.notify();
                }
            }
        }));
        self.later_picker = Some(picker);
        self.later_open = true;
    }
    fn close_later(&mut self) {
        self.later_open = false;
        self.later_picker = None;
        self.later_subscription = None;
    }

    /// Sends the draft when it was going to, no more: it stays a draft.
    fn unschedule(&mut self, cx: &mut Context<Self>) {
        self.draft.send_at = None;
        self.draft.send_error.clear();
        self.changed(cx);
    }

    /// When a scheduled draft is to go, and why it didn't if it couldn't.
    fn schedule_banner(&self, cx: &Context<Self>) -> Option<Div> {
        let at = self.draft.send_at?;
        let c = self.colors;
        let when = crate::manage::composing::describe_later(at, chrono::Local::now());
        let failed = !self.draft.send_error.is_empty();
        let text = if failed {
            t!("composer-not-sent", when = when, send_error = self.draft.send_error)
        } else {
            t!("composer-scheduled-to-send-it-waits-in", when = when)
        };
        Some(
            div()
                .flex_shrink_0()
                .px_4()
                .py_2()
                .flex()
                .items_center()
                .gap_2()
                .border_b_1()
                .border_color(rgb(c.border))
                .bg(rgb(c.soft))
                .text_size(px(12.))
                .child(icon_el(
                    if failed { "warning" } else { "clock" },
                    14.,
                    if failed { c.danger } else { c.accent },
                ))
                .child(div().flex_1().min_w_0().child(text))
                .child(
                    div()
                        .id("compose-unschedule")
                        .debug_selector(|| "compose-unschedule".into())
                        .px_2()
                        .py(px(3.))
                        .rounded(px(5.))
                        .flex_shrink_0()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(rgb(c.accent))
                        .cursor_pointer()
                        .hover(move |s| s.bg(rgb(c.border)))
                        .child(t!("composer-dont-send-later"))
                        .on_click(cx.listener(|this, _, _, cx| this.unschedule(cx))),
                ),
        )
    }

    /// A phone's formatting switch: says what it is and whether it is on,
    /// where the desktop has an icon with a tooltip.
    fn rich_text_toggle(&self, cx: &Context<Self>) -> Stateful<Div> {
        let c = self.colors;
        let on = self.draft.formatted;
        div()
            .id("compose-format")
            .debug_selector(|| "compose-format".into())
            .h(px(super::mobile::TOUCH))
            .pl_3()
            .pr(px(6.))
            .flex()
            .items_center()
            .gap_2()
            .rounded_full()
            .border_1()
            .border_color(rgb(if on { c.accent } else { c.border }))
            .when(on, |d| d.bg(rgb(c.soft)))
            .cursor_pointer()
            .active(|s| s.opacity(0.6))
            .text_size(px(14.))
            .text_color(rgb(if on { c.accent } else { c.muted }))
            .child(t!("composer-rich-text"))
            // A small switch, so on and off read at a glance.
            .child(
                div()
                    .w(px(30.))
                    .h(px(18.))
                    .p(px(2.))
                    .rounded_full()
                    .bg(rgb(if on { c.accent } else { c.border }))
                    .flex()
                    .when(on, |d| d.justify_end())
                    .child(div().size(px(14.)).rounded_full().bg(rgb(0xffffff))),
            )
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(|t, _, w, cx| {
                cx.stop_propagation();
                let on = !t.draft.formatted;
                t.set_formatted(on, w, cx)
            }))
    }

    /// Whether Send Later's page is open.
    pub(super) fn later_open(&self) -> bool {
        self.later_open
    }

    /// Send Later on a phone: a page covering the message, with its own bar.
    fn later_page(&self, cx: &Context<Self>) -> Stateful<Div> {
        let c = self.colors;
        div()
            .id("later-menu")
            .debug_selector(|| "later-menu".into())
            .absolute()
            .inset_0()
            .occlude()
            .bg(rgb(c.surface))
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(52.))
                    .flex_shrink_0()
                    .px_1()
                    .flex()
                    .items_center()
                    .gap_1()
                    .border_b_1()
                    .border_color(rgb(c.border))
                    .child(
                        div()
                            .id("later-back")
                            .size(px(super::mobile::TOUCH))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .active(|s| s.opacity(0.5))
                            .child(icon_el("chevron-left", 26., c.accent))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.close_later();
                                cx.notify();
                            })),
                    )
                    .child(
                        div()
                            .text_size(px(20.))
                            .font_weight(FontWeight::BOLD)
                            .child(t!("composer-send-later")),
                    ),
            )
            // The picker scrolls its middle and keeps its buttons in view.
            .child(
                div()
                    .id("later-scroll")
                    .flex_1()
                    .min_h_0()
                    .p(px(super::mobile::GUTTER))
                    .flex()
                    .flex_col()
                    .children(self.later_picker.clone()),
            )
    }

    /// Send Later's menu: quick choices, and a calendar and time of day.
    fn later_menu(&self, cx: &Context<Self>) -> Stateful<Div> {
        let c = self.colors;
        div()
            .id("later-menu")
            .debug_selector(|| "later-menu".into())
            .absolute()
            .bottom(px(48.))
            .left(px(12.))
            .w(px(520.))
            .p_3()
            .rounded(px(12.))
            .border_1()
            .border_color(rgb(c.border))
            .bg(rgb(c.raised))
            .shadow_lg()
            .occlude()
            .flex()
            .flex_col()
            .gap_2()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.close_later();
                cx.notify();
            }))
            .child(
                div()
                    .text_size(px(11.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(c.muted))
                    .child(t!("composer-send-later-2")),
            )
            .children(self.later_picker.clone())
    }

    /// Sets how long after sending to remind if nobody replies (`None`: not at all).
    fn set_follow_up(&mut self, wait: Option<i64>, cx: &mut Context<Self>) {
        self.follow_up_open = false;
        self.draft.follow_up = wait;
        self.changed(cx);
        cx.notify();
    }

    /// "Remind me if no reply"'s menu: how long to wait, or not to.
    fn follow_up_menu(&self, cx: &Context<Self>) -> Stateful<Div> {
        let c = self.colors;
        let chosen = self.draft.follow_up;
        let row = |id: String, label: String, on: bool| {
            div()
                .id(SharedString::from(id.clone()))
                .debug_selector(move || id.clone())
                .h(px(if self.mobile { super::mobile::TOUCH } else { 30. }))
                .px_2()
                .flex()
                .items_center()
                .gap_2()
                .rounded(px(6.))
                .cursor_pointer()
                .text_size(px(13.))
                .when(on, |d| d.text_color(rgb(c.accent)).font_weight(FontWeight::SEMIBOLD))
                .hover(move |s| s.bg(rgb(c.soft)))
                .child(div().flex_1().min_w_0().truncate().child(label))
        };
        let mut menu = div()
            .id("follow-up-menu")
            .debug_selector(|| "follow-up-menu".into())
            .absolute()
            .bottom(px(48.))
            .left(px(12.))
            .w(px(300.))
            .p_2()
            .rounded(px(12.))
            .border_1()
            .border_color(rgb(c.border))
            .bg(rgb(c.raised))
            .shadow_lg()
            .occlude()
            .flex()
            .flex_col()
            .gap(px(2.))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.follow_up_open = false;
                cx.notify();
            }))
            .child(
                div()
                    .px_2()
                    .pb_1()
                    .text_size(px(11.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(c.muted))
                    .child(t!("composer-remind-me-if-nobody-replies")),
            );
        for (i, (label, wait)) in crate::manage::backend::follow_up::wait_choices().into_iter().enumerate() {
            menu = menu.child(
                row(format!("follow-up-{i}"), label, chosen == Some(wait))
                    .on_click(cx.listener(move |this, _, _, cx| this.set_follow_up(Some(wait), cx))),
            );
        }
        menu.child(div().h(px(1.)).my_1().bg(rgb(c.border))).child(
            row("follow-up-none".into(), t!("composer-dont-remind-me").into(), chosen.is_none())
                .on_click(cx.listener(|this, _, _, cx| this.set_follow_up(None, cx))),
        )
    }

    /// While a reminder is set: when it comes, and a way to drop it.
    fn follow_up_banner(&self, cx: &Context<Self>) -> Option<Div> {
        let wait = self.draft.follow_up?;
        let c = self.colors;
        Some(
            div()
                .flex_shrink_0()
                .px_4()
                .py_2()
                .flex()
                .items_center()
                .gap_2()
                .border_b_1()
                .border_color(rgb(c.border))
                .bg(rgb(c.soft))
                .text_size(px(12.))
                .child(icon_el("waiting", 14., c.accent))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(t!("composer-reminds-you-if-no-reply", days = wait_days(wait))),
                )
                .child(
                    div()
                        .id("compose-no-follow-up")
                        .debug_selector(|| "compose-no-follow-up".into())
                        .px_2()
                        .py(px(3.))
                        .rounded(px(5.))
                        .flex_shrink_0()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(rgb(c.accent))
                        .cursor_pointer()
                        .hover(move |s| s.bg(rgb(c.border)))
                        .child(t!("composer-dont-remind-me"))
                        .on_click(cx.listener(|this, _, _, cx| this.set_follow_up(None, cx))),
                ),
        )
    }

    /// The templates menu: each template, then keeping this message as one.
    fn templates_menu(&self, cx: &Context<Self>) -> Stateful<Div> {
        let c = self.colors;
        let row = |id: String, label: String| {
            div()
                .id(SharedString::from(id))
                .h(px(30.))
                .px_2()
                .flex()
                .items_center()
                .rounded(px(6.))
                .cursor_pointer()
                .text_size(px(13.))
                .hover(move |s| s.bg(rgb(c.soft)))
                .child(div().flex_1().min_w_0().truncate().child(label))
        };
        let mut menu = div()
            .id("templates-menu")
            .debug_selector(|| "templates-menu".into())
            .absolute()
            .bottom(px(48.))
            .left(px(12.))
            .w(px(300.))
            .max_h(px(360.))
            .overflow_y_scroll()
            .p_2()
            .rounded(px(12.))
            .border_1()
            .border_color(rgb(c.border))
            .bg(rgb(c.raised))
            .shadow_lg()
            .occlude()
            .flex()
            .flex_col()
            .gap(px(2.))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.templates_open = false;
                cx.notify();
            }))
            .child(
                div()
                    .px_2()
                    .pb_1()
                    .text_size(px(11.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(c.muted))
                    .child("TEMPLATES"),
            );
        if self.options.templates.is_empty() {
            menu = menu.child(
                div()
                    .px_2()
                    .py_1()
                    .text_size(px(12.))
                    .text_color(rgb(c.muted))
                    .child(t!("composer-no-templates-yet-write-a-message")),
            );
        }
        for (i, t) in self.options.templates.iter().enumerate() {
            let template = t.clone();
            menu = menu.child(row(format!("template-{i}"), t.name.clone()).on_click(cx.listener(
                move |this, _, w, cx| {
                    this.templates_open = false;
                    this.insert_template(&template, w, cx);
                },
            )));
        }
        menu.child(div().h(px(1.)).my_1().bg(rgb(c.border))).child(
            row("template-save".into(), t!("composer-save-this-message-as-a-template").into()).on_click(
                cx.listener(|this, _, _, cx| {
                    this.templates_open = false;
                    let subject = this.subject.read(cx).value().to_string();
                    let body = if this.draft.formatted {
                        // Templates are Markdown: the formatting as that,
                        // without the signature and quote below.
                        let doc = &this.rich.read(cx).doc;
                        let end = doc.signature_range().map(|r| r.start).or(doc.quote_start()).unwrap_or(doc.blocks.len());
                        let kept = rt::Doc { blocks: doc.blocks[..end].to_vec(), sheets: vec![] };
                        super::paste::html_to_markdown(&rhtml::to_html(&kept)).unwrap_or_else(|| kept.to_plain())
                    } else {
                        this.body.read(cx).value().to_string()
                    };
                    // Without the signature and quoted history below it.
                    let body = body
                        .split("\n-- \n")
                        .next()
                        .unwrap_or("")
                        .trim_end()
                        .to_owned();
                    if body.trim().is_empty() && subject.trim().is_empty() {
                        this.error = Some(t!("composer-write-something-first-then-save-it").into());
                        cx.notify();
                        return;
                    }
                    let name = if subject.trim().is_empty() {
                        body.lines().next().unwrap_or(t!("composer-template")).chars().take(40).collect()
                    } else {
                        subject.clone()
                    };
                    cx.emit(ComposerEvent::SaveTemplate(crate::manage::composing::Template {
                        name,
                        subject,
                        body,
                    }));
                    cx.notify();
                }),
            ),
        )
    }

    /// Puts a template in the draft: its subject where there is none yet,
    /// its text at the top of the body, with its placeholders filled in.
    pub(super) fn insert_template(
        &mut self,
        t: &crate::manage::composing::Template,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use crate::manage::composing::fill_placeholders;
        let mut values = self.placeholders(cx);
        if self.subject.read(cx).value().trim().is_empty() && !t.subject.is_empty() {
            let subject = fill_placeholders(&t.subject, &values);
            values.subject = subject.clone();
            self.subject.update(cx, |s, cx| s.set_text(subject, window, cx));
        }
        let text = fill_placeholders(&t.body, &values);
        if self.draft.formatted {
            // A template's Markdown, formatted.
            let fragment = rhtml::from_markdown(&text, "");
            self.rich.update(cx, |r, cx| {
                r.insert_at_start(&fragment, Some(window), cx);
                r.focus(window, cx);
            });
            return;
        }
        self.insert_reply_text(&text, window, cx);
    }

    /// What a template's placeholders stand for now: the first recipient
    /// (named as in To, else as in Contacts), the sender, today and the
    /// subject.
    fn placeholders(&self, cx: &App) -> crate::manage::composing::Placeholders {
        let to = self.recipients[TO].read(cx).value().to_string();
        let (mut name, email) = crate::sync::smtp::recipients(&to)
            .ok()
            .and_then(|list| list.into_iter().next())
            .map(|(name, email)| (name.unwrap_or_default(), email))
            .unwrap_or_default();
        if name.trim().is_empty() && !email.is_empty() {
            name = self
                .contacts
                .iter()
                .find(|(_, e)| e.eq_ignore_ascii_case(&email))
                .map(|(n, _)| n.clone())
                .unwrap_or_default();
        }
        let my_email = self.from_address();
        let my_name = if let Some(i) = self.send_identity()
            && !i.name.trim().is_empty()
        {
            i.name.clone()
        } else {
            self.accounts
                .iter()
                .find(|a| a.id == self.draft.account)
                .map(|a| a.name.clone())
                .unwrap_or_default()
        };
        let now = chrono::Local::now();
        crate::manage::composing::Placeholders {
            name: name.trim().trim_matches('"').to_owned(),
            email,
            my_name,
            my_email,
            // Written into the message: in the language of mail Picacho writes.
            date: crate::i18n::in_outgoing(|| now.local(t!("dates-month-name-day-year"))),
            time: crate::i18n::in_outgoing(|| now.local(t!("dates-time"))),
            weekday: crate::i18n::in_outgoing(|| now.local("%A")),
            subject: self.subject.read(cx).value().to_string(),
        }
    }

    /// Fills the recipient and subject fields, as typing would (tests).
    #[cfg(test)]
    pub(super) fn fill(&mut self, to: &str, subject: &str, w: &mut Window, cx: &mut Context<Self>) {
        let (to, subject) = (to.to_string(), subject.to_string());
        self.recipients[TO].update(cx, |s, cx| s.set_text(to, w, cx));
        self.subject.update(cx, |s, cx| s.set_text(subject, w, cx));
    }

    fn attach(&mut self, cx: &mut Context<Self>) {
        let picker = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some(t!("composer-attach").into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = picker.await {
                let _ = this.update(cx, |this, cx| this.add_files(paths, cx));
            }
        })
        .detach();
    }

    pub(super) fn add_files(&mut self, paths: Vec<std::path::PathBuf>, cx: &mut Context<Self>) {
        for p in paths {
            // The picker's copies in the app's cache are pruned after a
            // week; the draft keeps its own.
            #[cfg(target_os = "android")]
            let p = self.backend.adopt_picked(&self.draft.id, p);
            if p.is_file() && !self.draft.attachments.contains(&p) {
                self.draft.attachments.push(p);
            }
        }
        self.changed(cx);
    }
}

/// A formatted draft's document: its HTML, or (kept before the composer
/// wrote HTML, or just written as Markdown) its Markdown body, whose HTML
/// the draft keeps from then on.
fn rich_doc(draft: &mut Draft) -> rt::Doc {
    if !draft.html.is_empty() {
        return rhtml::parse(&draft.html, true);
    }
    let doc = rhtml::from_markdown(&draft.body, &draft.signature_html);
    draft.html = rhtml::to_html(&doc);
    doc
}

/// The formatted editor's look from the composer's colors.
fn look_of(c: Colors, mobile: bool) -> Look {
    let lum = |v: u32| ((v >> 16) & 0xff) + ((v >> 8) & 0xff) + (v & 0xff);
    Look {
        text: c.text,
        muted: c.muted,
        accent: c.accent,
        border: c.border,
        soft: c.soft,
        surface: c.surface,
        danger: c.danger,
        dark: lum(c.surface) < 3 * 0x80,
        // Phones read body text at their system size (17pt).
        size: if mobile { 17. } else { 14. },
    }
}

/// The blocks a signature adds to a formatted message: the "-- " line and
/// the signature, its HTML or its Markdown.
fn signature_blocks(sig: &crate::manage::composing::Signature) -> Vec<Rc<rt::Block>> {
    if sig.text.trim().is_empty() && sig.html.trim().is_empty() {
        return vec![];
    }
    let inner = if sig.html.trim().is_empty() {
        crate::sync::smtp::markdown_fragment(&sig.text, "")
    } else {
        sig.html.clone()
    };
    rhtml::parse(&format!("<p>--&nbsp;</p><div class=\"picacho-signature\">{inner}</div>"), false).blocks
}

/// Block formats applied by `prefix_lines`.
#[derive(Clone, Copy)]
pub(super) enum Lines {
    Bullets,
    Numbers,
    Quote,
    Heading,
}

impl Composer {
    fn icon_button(
        &self,
        id: &'static str,
        icon: &'static str,
        tip: impl Into<SharedString>,
        on: bool,
        cx: &Context<Self>,
        click: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> Stateful<Div> {
        let c = self.colors;
        let tip: SharedString = tip.into();
        let shown = keys(&tip);
        div()
            .id(id)
            .debug_selector(move || format!("compose-{id}"))
            .role(Role::Button)
            .aria_label(super::spoken(&tip))
            .when_some(super::spoken_keys(&tip), |d, k| d.aria_keyshortcuts(k))
            .when(on, |d| d.aria_toggled(Toggled::True))
            .focusable()
            .tab_stop(true)
            .focus_visible(move |s| s.border_2().border_color(rgb(c.accent)))
            // A fingertip needs a bigger target than a pointer.
            .size(px(if self.mobile { super::mobile::TOUCH } else { 28. }))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(6.))
            .cursor_pointer()
            .when(on, |d| d.bg(rgb(c.soft)))
            .hover(move |s| s.bg(rgb(c.border)))
            .active(|s| s.opacity(0.7))
            .child(icon_el(icon, if self.mobile { 20. } else { 15. }, if on { c.accent } else { c.muted }))
            .tooltip(move |w, cx| Tooltip::new(shown.clone()).build(w, cx))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |this, _, w, cx| {
                cx.stop_propagation();
                click(this, w, cx)
            }))
    }

    /// A compose window's title bar: its title, Save draft and Move into
    /// main window, then the platform's window controls.
    fn window_header(&self, window: &mut Window, cx: &mut Context<Self>) -> Stateful<Div> {
        let c = self.colors;
        let bar = super::window::detached_bar("compose-titlebar", c)
            .child(icon_el("compose", 15., c.muted))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(px(13.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(self.title(cx)),
            )
            .child(self.icon_button(
                "save",
                "save",
                super::shortcuts::tip(t!("composer-save-draft"), "save-draft", cx),
                false,
                cx,
                |this, _, cx| this.save(cx),
            ))
            .child(self.icon_button(
                "dock-main",
                "shrink",
                t!("composer-move-into-main-window"),
                false,
                cx,
                |_, _, cx| cx.emit(ComposerEvent::Place(Placement::Floating)),
            ));
        let me = cx.weak_entity();
        super::window::detached_titlebar(
            bar,
            c,
            move |_, _, cx| {
                let _ = me.update(cx, |_, cx| cx.emit(ComposerEvent::Close));
            },
            window,
            cx,
        )
    }

    fn header(&self, cx: &Context<Self>) -> Stateful<Div> {
        let c = self.colors;
        let floating = matches!(self.placement, Placement::Floating | Placement::Maximized);
        let mut header = div()
            .id("compose-header")
            .debug_selector(|| "compose-header".into())
            .h(px(40.))
            .flex_shrink_0()
            .pl_4()
            .pr_1()
            .flex()
            .items_center()
            .gap(px(2.))
            .bg(rgb(if floating { c.raised } else { c.surface }))
            .border_b_1()
            .border_color(rgb(c.border))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(px(13.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(self.title(cx)),
            );
        if self.placement == Placement::Floating && !self.minimized {
            // Drag the header to move the panel around the window.
            let me = cx.weak_entity();
            header = header.cursor_move().on_drag(
                ComposerDrag(cx.entity_id()),
                move |_, offset, _, cx| {
                    let _ = me.update(cx, |c, _| c.grab = offset);
                    cx.new(|_| NoPreview)
                },
            );
        }
        if self.minimized {
            header = header
                .cursor_pointer()
                .on_click(cx.listener(|this, _, w, cx| {
                    this.minimized = false;
                    this.focus_first(w, cx);
                    cx.notify();
                }));
        }
        let save = self.icon_button(
            "save",
            "save",
            super::shortcuts::tip(t!("composer-save-draft"), "save-draft", cx),
            false,
            cx,
            |this, _, cx| {
                this.save(cx);
            },
        );
        header = header.child(save);
        match self.placement {
            Placement::Floating | Placement::Maximized => {
                let maximized = self.placement == Placement::Maximized;
                header = header
                    .child(self.icon_button(
                        "minimize",
                        if self.minimized { "undock" } else { "dock" },
                        if self.minimized {
                            t!("composer-restore")
                        } else {
                            t!("composer-minimize")
                        },
                        false,
                        cx,
                        |this, w, cx| {
                            this.minimized = !this.minimized;
                            if !this.minimized {
                                this.focus_first(w, cx);
                            }
                            cx.notify();
                        },
                    ))
                    .child(self.icon_button(
                        "maximize",
                        if maximized { "shrink" } else { "expand" },
                        if maximized {
                            t!("composer-exit-full-size")
                        } else {
                            t!("composer-full-size")
                        },
                        false,
                        cx,
                        move |this, _, cx| {
                            this.minimized = false;
                            cx.emit(ComposerEvent::Place(if maximized {
                                Placement::Floating
                            } else {
                                Placement::Maximized
                            }));
                        },
                    ))
                    .child(self.icon_button(
                        "new-window",
                        "popout",
                        t!("composer-open-in-new-window"),
                        false,
                        cx,
                        |_, _, cx| cx.emit(ComposerEvent::Place(Placement::Window)),
                    ));
            }
            Placement::Inline => {
                header = header
                    .child(self.icon_button(
                        "pop-out",
                        "expand",
                        t!("composer-pop-out-reply"),
                        false,
                        cx,
                        |_, _, cx| cx.emit(ComposerEvent::Place(Placement::Floating)),
                    ))
                    .child(self.icon_button(
                        "new-window",
                        "popout",
                        t!("composer-open-in-new-window"),
                        false,
                        cx,
                        |_, _, cx| cx.emit(ComposerEvent::Place(Placement::Window)),
                    ));
            }
            Placement::Window => {
                header = header.child(self.icon_button(
                    "dock-main",
                    "shrink",
                    t!("composer-move-into-main-window"),
                    false,
                    cx,
                    |_, _, cx| cx.emit(ComposerEvent::Place(Placement::Floating)),
                ));
            }
        }
        header.child(
            self.icon_button("close", "close", t!("composer-save-and-close"), false, cx, |_, _, cx| {
                cx.emit(ComposerEvent::Close)
            }),
        )
    }

    fn row(&self, label: &'static str, field: impl IntoElement) -> Div {
        let c = self.colors;
        div()
            .h(px(if self.mobile { super::mobile::TOUCH + 4. } else { 38. }))
            .flex_shrink_0()
            .px_4()
            .flex()
            .items_center()
            .gap_2()
            .border_b_1()
            .border_color(rgb(c.border))
            .child(
                div()
                    .w(px(52.))
                    .flex_shrink_0()
                    .text_size(px(12.))
                    .text_color(rgb(c.muted))
                    .child(label),
            )
            .child(div().flex_1().min_w_0().child(field))
    }

    fn input(&self, state: &Entity<InputState>) -> Input {
        Input::new(state)
            .appearance(false)
            .disabled(self.sending)
            .text_size(px(13.))
    }

    /// The alias the draft is sent as, when it is one Picacho knows.
    fn send_alias(&self) -> Option<&fwd::SendAlias> {
        (!self.draft.send_as.is_empty())
            .then(|| {
                self.aliases
                    .iter()
                    .find(|a| a.email.eq_ignore_ascii_case(&self.draft.send_as))
            })
            .flatten()
    }

    /// The accounts, after one changed (its sending addresses).
    pub(super) fn set_accounts(&mut self, accounts: Vec<Account>) {
        self.accounts = accounts;
    }

    /// The aliases and alias services, after they change in Settings.
    pub(super) fn set_forwarding(
        &mut self,
        aliases: Vec<fwd::SendAlias>,
        services: Vec<(String, fwd::ForwardingService, Option<String>)>,
        alias_only: HashSet<String>,
    ) {
        self.aliases = aliases;
        self.alias_services = services;
        self.alias_only = alias_only;
    }

    /// The draft would go out as an account that sends only as an alias.
    fn needs_alias(&self) -> bool {
        self.draft.send_as.trim().is_empty()
            && self.draft.identity.is_empty()
            && self.alias_only.contains(&self.draft.account)
    }

    /// The account's other address the draft is sent as, when it is one.
    fn send_identity(&self) -> Option<&crate::manage::model::Identity> {
        if self.draft.identity.is_empty() || !self.draft.send_as.trim().is_empty() {
            return None;
        }
        self.accounts
            .iter()
            .find(|a| a.id == self.draft.account)?
            .identity(&self.draft.identity)
    }

    /// Sends as the mailbox again, after the alias in From went away.
    pub(super) fn reset_sender(&mut self, cx: &mut Context<Self>) {
        self.draft.send_as.clear();
        self.draft.identity.clear();
        self.from_open = false;
        self.changed(cx);
    }

    /// The service that makes aliases for `account`: one forwarding to it,
    /// else one forwarding to any mailbox.
    fn alias_service_for(&self, account: &str) -> Option<&(String, fwd::ForwardingService, Option<String>)> {
        self.alias_services
            .iter()
            .find(|(_, _, to)| to.as_deref() == Some(account))
            .or_else(|| self.alias_services.iter().find(|(_, _, to)| to.is_none()))
    }

    /// A button that opens Settings to make a new alias for `account`.
    fn new_alias_button(&self, id: (&'static str, usize), account: &Account, cx: &Context<Self>) -> Option<Stateful<Div>> {
        let c = self.colors;
        let (service, kind, _) = self.alias_service_for(&account.id)?;
        let tip = t!("composer-new-alias-for", label = kind.label(), email = account.email);
        let (service, mailbox) = (service.clone(), account.id.clone());
        Some(
            div()
                .id(id)
                .debug_selector(move || format!("{}-{}", id.0, id.1))
                .size(px(24.))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(5.))
                .cursor_pointer()
                .hover(move |s| s.bg(rgb(c.soft)))
                .child(icon_el("alias", 14., c.muted))
                .tooltip(move |w, cx| Tooltip::new(tip.clone()).build(w, cx))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.from_open = false;
                    cx.emit(ComposerEvent::NewAlias(service.clone(), mailbox.clone()));
                    cx.notify();
                })),
        )
    }

    /// Opens or closes the From menu, starting it unfiltered.
    fn toggle_from(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        if self
            .from_closed
            .take()
            .is_some_and(|t| t.elapsed() < Duration::from_millis(400))
        {
            return;
        }
        self.from_open = !self.from_open;
        if self.from_open {
            self.from_filter.update(cx, |s, cx| {
                s.set_text("", w, cx);
                if self.aliases.len() > FROM_FILTER_AT {
                    s.focus(w, cx);
                }
            });
        }
        cx.notify();
    }

    /// Sends as an account (`alias` and `identity` None), as an alias
    /// through `account`, or as one of the account's other addresses.
    /// The signature goes with it: the previous sender's, where the body
    /// still has it as it was put there, becomes the new sender's.
    fn choose_sender(
        &mut self,
        account: String,
        alias: Option<String>,
        identity: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let before = self.signature();
        self.draft.account = account;
        self.draft.send_as = alias.unwrap_or_default();
        self.draft.identity = identity.unwrap_or_default();
        self.from_open = false;
        self.swap_signature(before, window, cx);
        self.changed(cx);
    }

    /// The address the draft goes from: its alias, the account's other
    /// address it is sent as, or the account's own.
    #[allow(clippy::wrong_self_convention)]
    fn from_address(&self) -> String {
        if !self.draft.send_as.trim().is_empty() {
            return self.draft.send_as.trim().to_owned();
        }
        if let Some(i) = self.send_identity() {
            return i.email.clone();
        }
        self.accounts
            .iter()
            .find(|a| a.id == self.draft.account)
            .map(|a| a.email.clone())
            .unwrap_or_default()
    }

    /// The signature the draft's sender gets; none for replies and
    /// forwards when they get none.
    fn signature(&self) -> crate::manage::composing::Signature {
        let answering = !self.draft.original.is_empty() || !self.draft.reply_id.is_empty();
        if answering && !self.options.signature_on_replies {
            return Default::default();
        }
        self.options.signature(&self.draft.account, &self.from_address())
    }

    /// Replaces signature `before` in the body with the sender's own
    /// (`composing::swap_signature`): not where it was changed by hand.
    fn swap_signature(
        &mut self,
        before: crate::manage::composing::Signature,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let after = self.signature();
        if self.draft.formatted {
            return self.swap_rich_signature(before, after, cx);
        }
        let body = self.body.read(cx).value().to_string();
        let Some(swapped) = crate::manage::composing::swap_signature(&body, &before.block(), &after.block()) else {
            return;
        };
        self.draft.body = swapped.clone();
        self.draft.signature_html = after.html;
        self.body.update(cx, |s, cx| s.set_text(swapped, window, cx));
    }

    /// `swap_signature` in the formatted editor: the signature's blocks
    /// (after the "-- " line) replaced where they read as `before` does.
    fn swap_rich_signature(
        &mut self,
        before: crate::manage::composing::Signature,
        after: crate::manage::composing::Signature,
        cx: &mut Context<Self>,
    ) {
        if before == after {
            return;
        }
        let same = |a: &str, b: &str| a.split_whitespace().eq(b.split_whitespace());
        let doc = self.rich.read(cx).doc.clone();
        let new = signature_blocks(&after);
        match doc.signature_range() {
            Some(range) => {
                let written: Vec<String> = doc.blocks[range.start + 1..range.end].iter().map(|b| b.text()).collect();
                // Changed by hand: it stays.
                if !same(&written.join("\n"), &before.text) {
                    return;
                }
                self.rich.update(cx, |r, cx| r.replace_blocks(range, new, cx));
            }
            None if before.text.trim().is_empty() && !new.is_empty() => {
                // Above the quote or forwarded message, or at the end,
                // after a blank line.
                let at = doc.quote_start().unwrap_or(doc.blocks.len());
                let mut blocks = vec![Rc::new(rt::Block::paragraph("div", ""))];
                blocks.extend(new);
                self.rich.update(cx, |r, cx| r.replace_blocks(at..at, blocks, cx));
            }
            None => return,
        }
        self.draft.signature_html = after.html;
    }

    fn sender_row(&self, cx: &Context<Self>) -> Option<Div> {
        let c = self.colors;
        let current = self
            .accounts
            .iter()
            .find(|a| a.id == self.draft.account)
            .or(self.accounts.first())?;
        let alias = self.send_alias();
        let identity = self.send_identity();
        // Sending as an alias, or another address, that is no longer
        // listed; sending says so.
        let missing = alias.is_none() && !self.draft.send_as.is_empty();
        let gone =
            !self.draft.identity.is_empty() && self.draft.send_as.is_empty() && identity.is_none();
        let identities: usize = self.accounts.iter().map(|a| a.identities.len()).sum();
        let several = self.accounts.len() + self.aliases.len() + identities > 1
            || missing
            || gone
            || self.needs_alias();
        let blocked = alias.is_none() && !missing && self.needs_alias();
        let missing = missing || gone;
        let (label, tip) = match alias {
            None if gone => (
                t!("composer-removed-address").to_owned(),
                t!("composer-this-address-was-removed-from-choose", email = current.email),
            ),
            None if identity.is_some() => {
                let i = identity.unwrap();
                let through = match &i.smtp {
                    Some(s) => t!("composer-sent-through", host = s.host),
                    None => t!("composer-sent-through-s-server", email = current.email),
                };
                let name = if i.name.is_empty() {
                    current.name.as_str()
                } else {
                    i.name.as_str()
                };
                (
                    if name.is_empty() || name == i.email {
                        i.email.clone()
                    } else {
                        format!("{name} <{}>", i.email)
                    },
                    through,
                )
            }
            None if blocked => (
                t!("composer-choose-an-alias").to_owned(),
                t!("composer-sends-only-as-its-aliases-settings", email = current.email),
            ),
            None if missing => (
                self.draft.send_as.clone(),
                t!("composer-no-longer-exists-choose-another-address", send_as = self.draft.send_as),
            ),
            Some(a) => (
                a.email.clone(),
                t!("composer-sent-through-from-recipients-see", label = a.service.label(), email = current.email, email2 = a.email),
            ),
            None if current.name.is_empty() || current.name == current.email => {
                (current.email.clone(), current.email.clone())
            }
            None => (
                format!("{} <{}>", current.name, current.email),
                current.email.clone(),
            ),
        };
        let mut picker = div()
            .id("compose-from")
            .debug_selector(|| "compose-from".into())
            .flex()
            .items_center()
            .gap_1()
            .min_w_0()
            .max_w_full()
            .text_size(px(13.))
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .when(missing, |d| d.text_color(rgb(c.danger)).line_through())
                    .when(blocked, |d| d.text_color(rgb(c.danger)))
                    .child(label),
            )
            .when(missing || blocked, |d| d.child(icon_el("warning", 13., c.danger)))
            .when_some(alias, |d, a| d.child(service_badge(a.service, c)))
            .tooltip(move |w, cx| Tooltip::new(tip.clone()).build(w, cx));
        if several {
            picker = picker
                .cursor_pointer()
                .px_1()
                .ml(px(-4.))
                .rounded(px(5.))
                .hover(move |s| s.bg(rgb(c.raised)))
                .child(icon_el("chevron-down", 12., c.muted))
                .on_click(cx.listener(|this, _, w, cx| this.toggle_from(w, cx)));
        }
        let field = div()
            .flex()
            .items_center()
            .gap_1()
            .min_w_0()
            .child(picker)
            .children(self.new_alias_button(("compose-new-alias", 0), current, cx));
        let mut row = self.row(t!("composer-from"), field).relative();
        if self.from_open && several {
            row = row.child(deferred(self.sender_menu(current, cx)).with_priority(2));
        }
        Some(row)
    }

    /// Accounts, then each service's aliases, filtered when the list is long.
    fn sender_menu(&self, current: &Account, cx: &Context<Self>) -> Stateful<Div> {
        let c = self.colors;
        let query = self.from_filter.read(cx).value().trim().to_lowercase();
        let matches = |text: &[&str]| {
            query.is_empty() || text.iter().any(|t| t.to_lowercase().contains(&query))
        };
        let alias = self.send_alias().map(|a| a.email.clone());
        let identity = self.send_identity().map(|i| i.id.clone());
        let option = |id: (&'static str, usize),
                      title: String,
                      caption: String,
                      on: bool,
                      account: String,
                      alias: Option<String>,
                      chosen: Option<String>| {
            div()
                .id(id)
                .min_h(px(40.))
                .px_2()
                .py_1()
                .flex()
                .items_center()
                .gap_2()
                .rounded(px(6.))
                .cursor_pointer()
                .hover(move |s| s.bg(rgb(c.soft)))
                .child(
                    div()
                        .size(px(14.))
                        .flex_shrink_0()
                        .when(on, |d| d.child(icon_el("check", 14., c.accent))),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(div().text_size(px(13.)).truncate().child(title))
                        .when(!caption.is_empty(), |d| {
                            d.child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(rgb(c.muted))
                                    .truncate()
                                    .child(caption),
                            )
                        }),
                )
                .on_click(cx.listener(move |this, _, w, cx| {
                    this.choose_sender(account.clone(), alias.clone(), chosen.clone(), w, cx)
                }))
        };
        let heading = |text: String| {
            div()
                .px_2()
                .pt_2()
                .pb_1()
                .text_size(px(11.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(c.muted))
                .child(text)
        };
        let mut list = div().flex().flex_col();
        let mut shown = 0;
        // Accounts kept to their aliases can't be chosen to write as.
        let accounts: Vec<&Account> = self
            .accounts
            .iter()
            .filter(|a| !self.alias_only.contains(&a.id))
            .filter(|a| matches(&[&a.email, &a.name]))
            .collect();
        if !accounts.is_empty() && !self.aliases.is_empty() {
            list = list.child(heading(t!("composer-accounts").into()));
        }
        let mut other = 0;
        for (i, a) in accounts.into_iter().enumerate() {
            let named = !a.name.is_empty() && a.name != a.email;
            list = list.child(
                option(
                    ("from-account", i),
                    a.email.clone(),
                    if named { a.name.clone() } else { String::new() },
                    alias.is_none() && identity.is_none() && a.id == current.id,
                    a.id.clone(),
                    None,
                    None,
                )
                .children(self.new_alias_button(("from-account-new-alias", i), a, cx)),
            );
            shown += 1;
            // The account's other addresses, under it.
            for ident in &a.identities {
                if !matches(&[&ident.email, &ident.name]) {
                    continue;
                }
                let through = match &ident.smtp {
                    Some(s) => t!("composer-through", host = s.host),
                    None => t!("composer-through-2", email = a.email),
                };
                let caption = if ident.name.is_empty() {
                    through
                } else {
                    format!("{} · {through}", ident.name)
                };
                list = list.child(
                    option(
                        ("from-identity", other),
                        ident.email.clone(),
                        caption,
                        identity.as_deref() == Some(ident.id.as_str()) && a.id == current.id,
                        a.id.clone(),
                        None,
                        Some(ident.id.clone()),
                    )
                    .pl(px(22.)),
                );
                other += 1;
                shown += 1;
            }
        }
        let mut service = None;
        for (i, a) in self.aliases.iter().enumerate() {
            if !matches(&[&a.email, &a.note]) {
                continue;
            }
            if service != Some(a.service) {
                service = Some(a.service);
                list = list.child(heading(t!("composer-aliases", label = a.service.label())));
            }
            // Through the mailbox the service delivers to, else this one.
            let through = a
                .deliver_to
                .as_ref()
                .and_then(|id| self.accounts.iter().find(|x| &x.id == id))
                .unwrap_or(current);
            let caption = if a.note.is_empty() {
                t!("composer-sends-through", email = through.email)
            } else {
                t!("composer-through-3", note = a.note, email = through.email)
            };
            list = list.child(option(
                ("from-alias", i),
                a.email.clone(),
                caption,
                alias.as_deref() == Some(a.email.as_str()),
                through.id.clone(),
                Some(a.email.clone()),
                None,
            ));
            shown += 1;
        }
        if shown == 0 {
            list = list.child(
                div()
                    .px_2()
                    .py_2()
                    .text_size(px(12.))
                    .text_color(rgb(c.muted))
                    .child(t!("composer-no-address-matches")),
            );
        }
        div()
            .id("from-menu")
            .debug_selector(|| "from-menu".into())
            .absolute()
            .top(px(36.))
            .left(px(66.))
            .right(px(12.))
            .max_w(px(440.))
            .p_1()
            .rounded(px(10.))
            .border_1()
            .border_color(rgb(c.border))
            .bg(rgb(c.raised))
            .shadow_lg()
            .occlude()
            .flex()
            .flex_col()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.from_open = false;
                this.from_closed = Some(std::time::Instant::now());
                cx.notify();
            }))
            .when(self.aliases.len() > FROM_FILTER_AT, |d| {
                d.child(
                    div()
                        .px_1()
                        .pb_1()
                        .mb_1()
                        .border_b_1()
                        .border_color(rgb(c.border))
                        .child(self.input(&self.from_filter)),
                )
            })
            .child(
                div()
                    .id("from-menu-list")
                    .max_h(px(300.))
                    .overflow_y_scroll()
                    .child(list),
            )
    }

    fn recipient_row(&self, field: usize, cx: &Context<Self>) -> Div {
        let c = self.colors;
        let label = [t!("composer-to"), t!("composer-cc"), t!("composer-bcc"), t!("composer-reply-to-2")][field];
        let mut input = self.input(&self.recipients[field]);
        if field == TO {
            let toggle = |id: &'static str, label: &'static str, on: bool, which: usize| {
                div()
                    .id(id)
                    .debug_selector(move || id.into())
                    .px_1()
                    .text_size(px(12.))
                    .text_color(rgb(c.muted))
                    .cursor_pointer()
                    .hover(move |s| s.text_color(rgb(c.text)))
                    .when(!on, |d| d.child(label))
                    .on_click(cx.listener(move |this, _, w, cx| {
                        match which {
                            CC => this.show_cc = true,
                            BCC => this.show_bcc = true,
                            _ => this.show_reply_to = true,
                        }
                        this.recipients[which].update(cx, |s, cx| s.focus(w, cx));
                        cx.notify();
                    }))
            };
            input = input.suffix(
                div()
                    .flex()
                    .gap_1()
                    .child(toggle("compose-show-cc", t!("composer-cc"), self.show_cc, CC))
                    .child(toggle("compose-show-bcc", t!("composer-bcc"), self.show_bcc, BCC))
                    .child(toggle(
                        "compose-show-reply-to",
                        t!("composer-reply-to-2"),
                        self.show_reply_to,
                        REPLY_TO,
                    )),
            );
        }
        let mut row = self.row(label, input).relative();
        if self.suggest_field == field && !self.suggestions.is_empty() {
            let mut list = div()
                .id("compose-suggestions")
                .debug_selector(|| "compose-suggestions".into())
                .absolute()
                .top(px(36.))
                .left(px(70.))
                .w(px(340.))
                .p_1()
                .rounded(px(10.))
                .border_1()
                .border_color(rgb(c.border))
                .bg(rgb(c.raised))
                .shadow_lg()
                .occlude()
                .flex()
                .flex_col();
            for (i, (name, email)) in self.suggestions.iter().enumerate() {
                list = list.child(
                    div()
                        .id(("suggestion", i))
                        .px_2()
                        .py_1()
                        .flex()
                        .items_center()
                        .gap_2()
                        .rounded(px(6.))
                        .cursor_pointer()
                        .hover(move |s| s.bg(rgb(c.soft)))
                        .when(i == 0, |d| d.bg(rgb(c.surface)))
                        .child(avatar(&crate::manage::model::initials(name), 24., i, c))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .child(div().text_size(px(13.)).truncate().child(name.clone()))
                                .child(
                                    div()
                                        .text_size(px(11.))
                                        .text_color(rgb(c.muted))
                                        .truncate()
                                        .child(email.clone()),
                                ),
                        )
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(
                            cx.listener(move |this, _, w, cx| this.accept_suggestion(i, w, cx)),
                        ),
                );
            }
            row = row.child(deferred(list).with_priority(2));
        }
        row
    }

    fn toolbar(&self, cx: &Context<Self>) -> Stateful<Div> {
        let c = self.colors;
        let sep = || div().w(px(1.)).h(px(16.)).mx_1().bg(rgb(c.border));
        let active = self.rich.read(cx).active();
        let tip = |label: &str, name: &str| super::shortcuts::tip(label, name, cx);
        div()
            .id("compose-toolbar")
            .debug_selector(|| "compose-toolbar".into())
            .h(px(36.))
            .flex_shrink_0()
            .px_3()
            .flex()
            .items_center()
            .gap(px(2.))
            .overflow_x_scroll()
            .border_b_1()
            .border_color(rgb(c.border))
            // Pressing a button leaves the text focused, with its selection.
            .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
            .child(self.icon_button("bold", "bold", tip(t!("composer-bold"), "bold"), active.bold, cx, |t, w, cx| {
                t.format(Mark::Bold, w, cx)
            }))
            .child(self.icon_button("italic", "italic", tip(t!("composer-italic"), "italic"), active.italic, cx, |t, w, cx| {
                t.format(Mark::Italic, w, cx)
            }))
            .child(self.icon_button(
                "underline",
                "underline",
                tip(t!("composer-underline"), "underline"),
                active.underline,
                cx,
                |t, w, cx| t.format(Mark::Underline, w, cx),
            ))
            .child(self.icon_button(
                "strike",
                "strike",
                tip(t!("composer-strikethrough"), "strikethrough"),
                active.strike,
                cx,
                |t, w, cx| t.format(Mark::Strike, w, cx),
            ))
            .child(self.icon_button("code", "code", tip(t!("composer-code"), "code"), active.code, cx, |t, w, cx| {
                t.format(Mark::Code, w, cx)
            }))
            .child(sep())
            .child(self.icon_button(
                "heading",
                "heading",
                tip(t!("composer-heading"), "heading"),
                active.heading,
                cx,
                |t, w, cx| t.prefix_lines(Lines::Heading, w, cx),
            ))
            .child(self.icon_button(
                "bullets",
                "list",
                tip(t!("composer-bulleted-list"), "bulleted-list"),
                active.bullets,
                cx,
                |t, w, cx| t.prefix_lines(Lines::Bullets, w, cx),
            ))
            .child(self.icon_button(
                "numbers",
                "list-ordered",
                tip(t!("composer-numbered-list"), "numbered-list"),
                active.numbers,
                cx,
                |t, w, cx| t.prefix_lines(Lines::Numbers, w, cx),
            ))
            .child(self.icon_button("quote", "quote", tip(t!("composer-quote"), "quote"), active.quote, cx, |t, w, cx| {
                t.prefix_lines(Lines::Quote, w, cx)
            }))
            .child(self.icon_button("link", "link", tip(t!("composer-link"), "link"), active.link, cx, |t, w, cx| {
                t.insert_link(w, cx)
            }))
            .child(sep())
            .child(self.icon_button(
                "align",
                match active.align {
                    rt::Align::Left => "text-align-start",
                    rt::Align::Center => "text-align-center",
                    rt::Align::Right => "text-align-end",
                    rt::Align::Justify => "text-align-justify",
                },
                match active.align {
                    rt::Align::Center => t!("composer-centered-align-right"),
                    rt::Align::Right => t!("composer-aligned-right-align-left"),
                    _ => t!("composer-aligned-left-center"),
                },
                active.align != rt::Align::Left,
                cx,
                |t, w, cx| {
                    let next = match t.rich.read(cx).active().align {
                        rt::Align::Left | rt::Align::Justify => rt::Align::Center,
                        rt::Align::Center => rt::Align::Right,
                        rt::Align::Right => rt::Align::Left,
                    };
                    t.rich.update(cx, |r, cx| {
                        r.set_align(next, Some(w), cx);
                        r.focus(w, cx);
                    });
                },
            ))
            .child(self.icon_button(
                "clear-formatting",
                "remove-formatting",
                t!("composer-clear-formatting"),
                false,
                cx,
                |t, w, cx| {
                    t.rich.update(cx, |r, cx| {
                        r.clear_formatting(Some(w), cx);
                        r.focus(w, cx);
                    });
                },
            ))
            .child(div().flex_1())
            .child(self.icon_button(
                "preview",
                "preview",
                tip(t!("composer-preview"), "preview-formatting"),
                self.preview,
                cx,
                |t, _, cx| {
                    t.preview = !t.preview;
                    cx.notify();
                },
            ))
    }

    /// The bar asking a link's address, above the formatted text.
    fn link_bar(&self, cx: &Context<Self>) -> Option<Div> {
        let input = self.link_input.as_ref()?;
        let c = self.colors;
        Some(
            div()
                .flex_shrink_0()
                .px_3()
                .py_1()
                .flex()
                .items_center()
                .gap_2()
                .border_b_1()
                .border_color(rgb(c.border))
                .child(icon_el("link", 13., c.muted))
                .child(div().flex_1().child(Input::new(input).appearance(false).text_size(px(13.))))
                .child(
                    div()
                        .id("link-remove")
                        .px_2()
                        .h(px(24.))
                        .flex()
                        .items_center()
                        .rounded(px(6.))
                        .text_size(px(12.))
                        .text_color(rgb(c.muted))
                        .cursor_pointer()
                        .hover(move |s| s.bg(rgb(c.soft)))
                        .child(t!("composer-remove-link"))
                        .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
                        .on_click(cx.listener(|this, _, w, cx| {
                            this.link_input = None;
                            this.rich.update(cx, |r, cx| {
                                r.set_link(None, Some(w), cx);
                                r.focus(w, cx);
                            });
                            cx.notify();
                        })),
                ),
        )
    }

    fn body_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let c = self.colors;
        let inline = self.placement == Placement::Inline;
        if self.preview && self.draft.formatted {
            let html = self.rich.read(cx).html();
            return div()
                .id("compose-preview")
                .debug_selector(|| "compose-preview".into())
                .when(!inline, |d| d.flex_1().min_h_0())
                .when(inline, |d| d.min_h(px(220.)))
                .overflow_y_scroll()
                .px_4()
                .py_3()
                .child(
                    gpui_component::text::TextView::html("compose-preview-text", html)
                        .selectable(true)
                        .h_auto()
                        .text_size(px(14.)),
                )
                .into_any_element();
        }
        let body = div()
            .id("compose-body")
            .debug_selector(|| "compose-body".into())
            .relative()
            .when(!inline, |d| d.flex_1().min_h_0())
            .when(inline, |d| d.h(px(240.)))
            .capture_action(cx.listener(|this, _: &gpui_component::input::Paste, w, cx| {
                if this.paste(w, cx) {
                    cx.stop_propagation();
                }
            }));
        let body = if self.draft.formatted {
            body.px_1().child(self.rich.clone())
        } else {
            body.px_2().py_1().child(
                Textarea::new(&self.body)
                    .appearance(false)
                    .disabled(self.sending)
                    .h_full()
                    // Phones read body text at their system size (17pt).
                    .text_size(px(if self.mobile { 17. } else { 14. })),
            )
        };
        body
            // Over the text, so it comes after.
            .children(self.paste_offer(cx))
            .bg(rgb(c.surface))
            .into_any_element()
    }

    fn attachments(&self, cx: &Context<Self>) -> Option<Div> {
        // Pictures shown in the formatted text are there, not here.
        let shown: Vec<String> = if self.draft.formatted { self.rich.read(cx).doc.file_images() } else { vec![] };
        let listed = |p: &std::path::PathBuf| {
            !p.file_name().is_some_and(|n| shown.iter().any(|s| *s == n.to_string_lossy()))
        };
        if !self.draft.attachments.iter().any(listed) {
            return None;
        }
        let c = self.colors;
        let mut list = div()
            .flex_shrink_0()
            .px_4()
            .py_2()
            .flex()
            .flex_wrap()
            .gap_2()
            .border_t_1()
            .border_color(rgb(c.border));
        for (i, path) in self.draft.attachments.iter().enumerate() {
            if !listed(path) {
                continue;
            }
            let size = crate::manage::sealed_files::len(path).unwrap_or(0);
            list = list.child(
                div()
                    .id(("attachment", i))
                    .debug_selector(move || format!("compose-attachment-{i}"))
                    .h(px(30.))
                    .max_w(px(260.))
                    .pl_2()
                    .pr_1()
                    .flex()
                    .items_center()
                    .gap_2()
                    .rounded(px(8.))
                    .border_1()
                    .border_color(rgb(c.border))
                    .bg(rgb(c.bg))
                    .text_size(px(12.))
                    .child(icon_el("paperclip", 13., c.muted))
                    .child(
                        div().min_w_0().truncate().child(
                            path.file_name()
                                .unwrap_or_default()
                                .to_string_lossy()
                                .to_string(),
                        ),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_color(rgb(c.muted))
                            .child(human_size(size)),
                    )
                    .child(
                        div()
                            .id(("remove-attachment", i))
                            .size(px(20.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(5.))
                            .cursor_pointer()
                            .hover(move |s| s.bg(rgb(c.border)))
                            .child(icon_el("close", 11., c.muted))
                            .tooltip(|w, cx| Tooltip::new(t!("composer-remove")).build(w, cx))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if i < this.draft.attachments.len() {
                                    this.draft.attachments.remove(i);
                                    this.changed(cx);
                                }
                            })),
                    ),
            );
        }
        Some(list)
    }

    fn footer(&self, cx: &Context<Self>) -> Div {
        let c = self.colors;
        let send = div()
            .id("compose-send")
            .debug_selector(|| "compose-send".into())
            .h(px(if self.mobile { super::mobile::TOUCH } else { 34. }))
            .px_4()
            .flex()
            .items_center()
            .gap_2()
            .rounded(px(9.))
            .bg(rgb(c.accent))
            .text_color(rgb(c.on_accent))
            .text_size(px(13.))
            .font_weight(FontWeight::SEMIBOLD)
            .cursor_pointer()
            .hover(move |s| s.bg(rgb(c.accent_hover)))
            .active(|s| s.opacity(0.8))
            .when(self.sending, |d| d.opacity(0.6))
            .child(icon_el("send", 14., c.on_accent))
            .child(if self.sending { t!("composer-sending") } else { t!("composer-send") })
            .tooltip(|w, cx| Tooltip::new(keys(&super::shortcuts::tip(t!("composer-send"), "send", cx))).build(w, cx))
            .on_click(cx.listener(|this, _, w, cx| this.request_send(w, cx)));
        let status = if let Some(error) = &self.error {
            div()
                .min_w_0()
                .truncate()
                .text_color(rgb(c.danger))
                .child(error.clone())
        } else {
            div()
                .min_w_0()
                .truncate()
                .text_color(rgb(c.muted))
                .when(self.saved && !self.draft.is_empty(), |d| {
                    d.child(t!("composer-draft-saved"))
                })
                .when(!self.saved, |d| d.child(t!("composer-editing")))
        };
        div()
            .h(px(if self.mobile { 60. } else { 52. }))
            .flex_shrink_0()
            .px_3()
            .relative()
            .flex()
            .items_center()
            .gap_1()
            .border_t_1()
            .border_color(rgb(c.border))
            .child(send)
            .child(self.icon_button(
                "send-later",
                "clock",
                t!("composer-send-later-3"),
                self.later_open,
                cx,
                |t, w, cx| {
                    if t.sending {
                        return;
                    }
                    if t.later_open {
                        t.close_later();
                    } else {
                        t.open_later(w, cx);
                    }
                    cx.notify();
                },
            ))
            .when(self.later_open && !self.mobile, |d| {
                d.child(deferred(self.later_menu(cx)).with_priority(2))
            })
            .child(div().w(px(6.)))
            .child(self.icon_button(
                "attach",
                "paperclip",
                t!("composer-attach-files-or-drop-them-here"),
                false,
                cx,
                |t, _, cx| t.attach(cx),
            ))
            .child(self.icon_button(
                "templates",
                "template",
                t!("composer-templates"),
                self.templates_open,
                cx,
                |t, _, cx| {
                    t.templates_open = !t.templates_open;
                    cx.notify();
                },
            ))
            .when(self.templates_open, |d| {
                d.child(deferred(self.templates_menu(cx)).with_priority(2))
            })
            .child(self.icon_button(
                "follow-up",
                "waiting",
                match self.draft.follow_up {
                    Some(wait) => t!("composer-reminds-you-if-no-reply", days = wait_days(wait)),
                    None => t!("composer-remind-me-if-no-reply").to_owned(),
                },
                self.draft.follow_up.is_some() || self.follow_up_open,
                cx,
                |t, _, cx| {
                    t.follow_up_open = !t.follow_up_open;
                    cx.notify();
                },
            ))
            .when(self.follow_up_open, |d| {
                d.child(deferred(self.follow_up_menu(cx)).with_priority(2))
            })
            .child(if self.mobile {
                self.rich_text_toggle(cx)
            } else {
                self.icon_button(
                    "format",
                    "format",
                    if self.draft.formatted {
                        t!("composer-formatting-on-sent-with-bold-lists")
                    } else {
                        t!("composer-plain-text-turn-formatting-on")
                    },
                    self.draft.formatted,
                    cx,
                    |t, w, cx| {
                        let on = !t.draft.formatted;
                        t.set_formatted(on, w, cx)
                    },
                )
            })
            // Signing and encrypting, once there's a key or certificate of
            // yours (Settings → Encryption).
            .when(self.can_protect(), |d| {
                let smime = self.draft.smime;
                d.child(self.icon_button(
                    "sign",
                    "shield-check",
                    match (self.draft.sign, smime) {
                        (true, false) => t!("composer-signed-with-your-openpgp-key"),
                        (false, false) => t!("composer-sign-with-your-openpgp-key"),
                        (true, true) => t!("composer-signed-with-your-s-mime-certificate"),
                        (false, true) => t!("composer-sign-with-your-s-mime-certificate"),
                    },
                    self.draft.sign,
                    cx,
                    |t, _, cx| {
                        t.draft.sign = !t.draft.sign;
                        t.changed(cx);
                        cx.notify();
                    },
                ))
                .child(self.icon_button(
                    "priority",
                    "warning",
                    match self.draft.priority {
                        crate::manage::model::Priority::Normal => t!("composer-normal-priority-mark-as-important"),
                        crate::manage::model::Priority::High => t!("composer-high-priority-mark-as-low-priority"),
                        crate::manage::model::Priority::Low => t!("composer-low-priority-back-to-normal"),
                    },
                    !self.draft.priority.is_normal(),
                    cx,
                    |t, _, cx| {
                        t.draft.priority = t.draft.priority.next();
                        t.changed(cx);
                        cx.notify();
                    },
                ))
                .child(self.icon_button(
                    "encrypt",
                    "lock",
                    match (self.draft.encrypt, smime) {
                        (true, false) => t!("composer-encrypted-to-each-recipients-openpgp-key"),
                        (false, false) => t!("composer-encrypt-with-openpgp-picacho-looks-up"),
                        (true, true) => t!("composer-encrypted-to-each-recipients-s-mime"),
                        (false, true) => t!("composer-encrypt-with-s-mime-recipients-must"),
                    },
                    self.draft.encrypt,
                    cx,
                    |t, _, cx| {
                        t.draft.encrypt = !t.draft.encrypt;
                        t.changed(cx);
                        cx.notify();
                    },
                ))
                // RFC 9788: OpenPGP and S/MIME both hide it in the
                // encrypted part; inline PGP has no protected headers.
                .when(self.draft.encrypt && !self.sends_inline_pgp(), |d| {
                    d.child(self.icon_button(
                        "encrypt-subject",
                        "eye-off",
                        if self.draft.encrypt_subject {
                            t!("composer-the-subject-is-encrypted-too-others")
                        } else {
                            t!("composer-encrypt-the-subject-too")
                        },
                        self.draft.encrypt_subject,
                        cx,
                        |t, _, cx| {
                            t.draft.encrypt_subject = !t.draft.encrypt_subject;
                            t.changed(cx);
                            cx.notify();
                        },
                    ))
                })
                // OpenPGP as inline PGP, for apps that read only that.
                .when(!smime && self.can_protect_openpgp() && (self.draft.sign || self.draft.encrypt), |d| {
                    d.child(self.icon_button(
                        "pgp-inline",
                        "scroll-text",
                        if self.draft.pgp_inline {
                            t!("composer-sent-as-inline-pgp")
                        } else {
                            t!("composer-sent-as-pgp-mime")
                        },
                        self.draft.pgp_inline,
                        cx,
                        |t, _, cx| {
                            t.draft.pgp_inline = !t.draft.pgp_inline;
                            t.changed(cx);
                            cx.notify();
                        },
                    ))
                })
                .when(self.has_both_kinds() && (self.draft.sign || self.draft.encrypt), |d| {
                    d.child(self.icon_button(
                        "protect-kind",
                        "key-round",
                        if smime {
                            t!("composer-using-s-mime-switch-to-openpgp")
                        } else {
                            t!("composer-using-openpgp-switch-to-s-mime")
                        },
                        smime,
                        cx,
                        |t, _, cx| {
                            t.draft.smime = !t.draft.smime;
                            t.changed(cx);
                            cx.notify();
                        },
                    ))
                })
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .px_2()
                    .text_size(px(11.))
                    .child(status),
            )
            .child(self.icon_button(
                "discard",
                "trash",
                t!("composer-discard-draft"),
                false,
                cx,
                |_, _, cx| cx.emit(ComposerEvent::Discard),
            ))
    }
}

impl Render for Composer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = self.colors;
        let inline = self.placement == Placement::Inline;
        let mut root = div()
            .id("composer")
            .role(Role::Form)
            .aria_label(t!("composer-new-message-2"))
            .key_context("Composer")
            .track_focus(&self.focus)
            .debug_selector(|| "composer".into())
            .relative()
            .flex()
            .flex_col()
            .when(!inline, |d| d.size_full())
            .when(inline, |d| d.w_full())
            .bg(rgb(c.surface))
            .text_color(rgb(c.text))
            .on_action(cx.listener(|t, _: &Bold, w, cx| t.format(Mark::Bold, w, cx)))
            .on_action(cx.listener(|t, _: &Italic, w, cx| t.format(Mark::Italic, w, cx)))
            .on_action(cx.listener(|t, _: &Underline, w, cx| t.format(Mark::Underline, w, cx)))
            .on_action(cx.listener(|t, _: &Strike, w, cx| t.format(Mark::Strike, w, cx)))
            .on_action(cx.listener(|t, _: &InlineCode, w, cx| t.format(Mark::Code, w, cx)))
            .on_action(cx.listener(|t, _: &InsertLink, w, cx| t.insert_link(w, cx)))
            .on_action(cx.listener(|t, _: &Bullets, w, cx| t.prefix_lines(Lines::Bullets, w, cx)))
            .on_action(cx.listener(|t, _: &Numbers, w, cx| t.prefix_lines(Lines::Numbers, w, cx)))
            .on_action(cx.listener(|t, _: &QuoteLines, w, cx| t.prefix_lines(Lines::Quote, w, cx)))
            .on_action(cx.listener(|t, _: &Heading, w, cx| t.prefix_lines(Lines::Heading, w, cx)))
            .on_action(cx.listener(|t, _: &SaveDraft, _, cx| t.save(cx)))
            .on_action(cx.listener(|t, _: &SendDraft, w, cx| t.request_send(w, cx)))
            .on_action(cx.listener(|t, _: &TogglePreview, _, cx| t.toggle_preview(cx)))
            .on_drop(cx.listener(|this, paths: &ExternalPaths, w, cx| {
                let paths = paths.paths().to_vec();
                // Pictures dropped on formatted text go in it.
                let pictures: Vec<std::path::PathBuf> = paths
                    .iter()
                    .filter(|p| mime_guess::from_path(p).first().is_some_and(|m| m.type_() == "image" && m.subtype() != "svg"))
                    .cloned()
                    .collect();
                this.add_files(paths, cx);
                this.place_pictures(&pictures, w, cx);
            }))
            .child(if self.mobile {
                self.mobile_header(window, cx)
            } else if self.placement == Placement::Window {
                self.window_header(window, cx)
            } else {
                self.header(cx)
            });
        if self.minimized {
            return root;
        }
        root = root.children(self.schedule_banner(cx));
        root = root.children(self.follow_up_banner(cx));
        root = root.children(self.sender_row(cx));
        root = root.child(self.recipient_row(TO, cx));
        if self.show_cc {
            root = root.child(self.recipient_row(CC, cx));
        }
        if self.show_bcc {
            root = root.child(self.recipient_row(BCC, cx));
        }
        if self.show_reply_to {
            root = root.child(self.recipient_row(REPLY_TO, cx));
        }
        if !inline {
            root = root.child(self.row(t!("composer-subject"), self.input(&self.subject)));
        }
        if self.draft.formatted {
            let (look, sending, files) = (look_of(c, self.mobile), self.sending, self.draft.attachments.clone());
            // Phones' keyboards check spelling themselves.
            let spellcheck = self.options.spellcheck && !self.mobile;
            self.rich.update(cx, |r, _| {
                r.look = look;
                r.spellcheck = spellcheck;
                r.disabled = sending;
                r.set_files(&files);
            });
            root = root.child(self.toolbar(cx));
            root = root.children(self.link_bar(cx));
        }
        root.child(self.body_view(cx))
            .children(self.attachments(cx))
            .child(self.footer(cx))
            // A phone shows Send Later as a page over the message.
            .when(self.mobile && self.later_open, |d| d.child(self.later_page(cx)))
    }
}

/// The root view of a composer's own window.
pub(super) struct ComposeWindow {
    pub(super) composer: Entity<Composer>,
}
impl Render for ComposeWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = self.composer.read(cx).colors;
        let title = self.composer.read(cx).title(cx);
        super::window::set_title(window, &title);
        if super::applock::covered(cx) {
            return super::applock::cover(c, window);
        }
        let composer = self.composer.clone();
        div()
            .key_context("Detached")
            .map(|d| super::window::window_frame(d, c, window))
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(c.surface))
            .on_action(move |_: &CloseWindow, _, cx| {
                composer.update(cx, |_, cx| cx.emit(ComposerEvent::Close))
            })
            .font_family(".SystemUIFont")
            .text_size(px(14.))
            .child(self.composer.clone())
            .into_any_element()
    }
}


/// Draft writes made from the window (saves and discards), done by one
/// thread in the order made: a later save never lands before an earlier one
/// (two autosaves on the executor's threads could write older text over
/// newer), a discard never before a save of the same draft (which brought
/// it back), and the window never waits for the cache's writer, which a
/// sync can hold a while. What reads drafts back waits first for the writes
/// queued before it (`wait_for_draft_writes`), and quitting waits for all of
/// them. In tests, at once (their executor doesn't wait for other threads).
struct DraftWrites {
    sender: std::sync::Mutex<Option<std::sync::mpsc::Sender<Box<dyn FnOnce() + Send>>>>,
    /// Writes queued and done so far.
    counts: std::sync::Mutex<(u64, u64)>,
    finished: std::sync::Condvar,
}

fn draft_writes() -> &'static DraftWrites {
    static WRITES: std::sync::OnceLock<DraftWrites> = std::sync::OnceLock::new();
    WRITES.get_or_init(|| {
        let (sender, receiver) = std::sync::mpsc::channel::<Box<dyn FnOnce() + Send>>();
        let started = std::thread::Builder::new()
            .name("picacho-draft-writes".into())
            .spawn(move || {
                for write in receiver {
                    write();
                }
            });
        DraftWrites {
            // No thread to take them: written where they're made.
            sender: std::sync::Mutex::new(started.ok().map(|_| sender)),
            counts: std::sync::Mutex::new((0, 0)),
            finished: std::sync::Condvar::new(),
        }
    })
}

/// Queues `write` after the draft writes queued before it; the receiver
/// hears how it went.
pub(super) fn queue_draft_write(
    write: impl FnOnce() -> anyhow::Result<()> + Send + 'static,
) -> futures::channel::oneshot::Receiver<anyhow::Result<()>> {
    let (tx, rx) = futures::channel::oneshot::channel();
    let run = move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(write))
            .unwrap_or_else(|_| Err(anyhow::anyhow!(t!("ui-something-went-wrong-while-doing-this"))));
        let _ = tx.send(result);
    };
    if cfg!(test) {
        run();
        return rx;
    }
    queue_on_thread(run);
    rx
}

/// Queues `run` on the draft writes' thread (see `queue_draft_write`).
fn queue_on_thread(run: impl FnOnce() + Send + 'static) {
    let writes = draft_writes();
    writes.counts.lock().unwrap_or_else(|e| e.into_inner()).0 += 1;
    let job: Box<dyn FnOnce() + Send> = Box::new(move || {
        run();
        let writes = draft_writes();
        writes.counts.lock().unwrap_or_else(|e| e.into_inner()).1 += 1;
        writes.finished.notify_all();
    });
    let sender = writes.sender.lock().unwrap_or_else(|e| e.into_inner());
    let job = match sender.as_ref() {
        Some(s) => match s.send(job) {
            Ok(()) => return,
            Err(std::sync::mpsc::SendError(job)) => job,
        },
        None => job,
    };
    drop(sender);
    // No thread to take it (`draft_writes`): written here.
    job();
}

/// Waits until the draft writes queued so far are done, at most `limit`
/// (None: as long as they take). False if it stopped waiting first.
pub(super) fn wait_for_draft_writes(limit: Option<Duration>) -> bool {
    if cfg!(test) {
        return true;
    }
    wait_on_thread(limit)
}

/// Waits for the draft writes' thread (see `wait_for_draft_writes`).
fn wait_on_thread(limit: Option<Duration>) -> bool {
    let writes = draft_writes();
    let counts = writes.counts.lock().unwrap_or_else(|e| e.into_inner());
    let wanted = counts.0;
    let done = |c: &mut (u64, u64)| c.1 >= wanted;
    match limit {
        None => {
            let waited = writes.finished.wait_while(counts, |c| !done(c));
            drop(waited);
            true
        }
        Some(limit) => writes
            .finished
            .wait_timeout_while(counts, limit, |c| !done(c))
            .map(|(_, timeout)| !timeout.timed_out())
            .unwrap_or(false),
    }
}

/// How many days a reminder waits, rounded.
fn wait_days(wait: i64) -> i64 {
    ((wait + 43_199) / 86_400).max(1)
}

#[cfg(test)]
mod draft_write_tests {
    use super::*;
    use std::prelude::v1::test;

    /// Draft writes are done one after another in the order queued, off the
    /// thread that queued them, and waiting for them waits for all queued
    /// before.
    #[test]
    fn draft_writes_keep_their_order_and_can_be_waited_for() {
        let done = Arc::new(std::sync::Mutex::new(Vec::new()));
        for i in 0..50 {
            let done = done.clone();
            queue_on_thread(move || {
                // The first ones slowest: they still land first.
                std::thread::sleep(Duration::from_millis(if i < 3 { 30 } else { 0 }));
                done.lock().unwrap().push(i);
            });
        }
        assert!(wait_on_thread(Some(Duration::from_secs(10))));
        assert_eq!(*done.lock().unwrap(), (0..50).collect::<Vec<_>>());
        // A write that takes longer than the wait: it stops waiting.
        queue_on_thread(|| std::thread::sleep(Duration::from_millis(300)));
        assert!(!wait_on_thread(Some(Duration::from_millis(20))));
        assert!(wait_on_thread(None));
    }
}

//! The message editor. One `Composer` entity is shown floating over the main
//! window (or minimized to a bar, or maximized to fill it), below a message
//! in the reader as an inline reply, or in a window of its own. The app
//! owns sending; the composer edits, formats and saves its draft.
use super::*;
use crate::manage::composing::{ComposeOptions, mentions_attachment};
use gpui_component::input::RopeExt as _;

actions!(
    composer,
    [
        Bold,
        Italic,
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

/// Key bindings while a composer has focus. The "Composer" context is
/// deeper than "Mail", so these win over the main window's shortcuts.
pub(super) fn bindings(cx: &App) -> Vec<KeyBinding> {
    let ctx = Some("Composer");
    vec![
        key(cx, "secondary-b", Bold, ctx),
        key(cx, "secondary-i", Italic, ctx),
        key(cx, "secondary-shift-x", Strike, ctx),
        key(cx, "secondary-e", InlineCode, ctx),
        key(cx, "secondary-k", InsertLink, ctx),
        key(cx, "secondary-shift-8", Bullets, ctx),
        key(cx, "secondary-shift-7", Numbers, ctx),
        key(cx, "secondary-shift-9", QuoteLines, ctx),
        key(cx, "secondary-alt-1", Heading, ctx),
        key(cx, "secondary-s", SaveDraft, ctx),
        key(cx, "secondary-enter", SendDraft, ctx),
        key(cx, "secondary-shift-p", TogglePreview, ctx),
    ]
}

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
    /// Show this briefly in the main window: an icon-only button's name,
    /// long-pressed on a touch device.
    Notice(String),
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
struct PasteOffer {
    /// Byte offset of the pasted text in the body.
    start: usize,
    inserted: String,
    other: String,
    inserted_formatted: bool,
    /// Whether the draft was formatted before the paste turned it on.
    was_formatted: bool,
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
    /// The device has one window (a touch device): no window of its own.
    pub(super) single_window: bool,
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
    pub(super) fn new(
        draft: Draft,
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
            field("Recipients", window, cx),
            field("Cc", window, cx),
            field("Bcc", window, cx),
            field("Where replies go", window, cx),
        ];
        let subject = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Subject")
                .input_action(TextInputAction::Next)
                .autocorrect(true)
                .autocapitalize(Autocapitalize::Sentences)
        });
        let from_filter = field("Find an address", window, cx);
        let body = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("Write your message…")
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
        let mut subscriptions = vec![];
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
            single_window: false,
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
            show_cc,
            show_bcc,
            show_reply_to,
            preview: false,
            paste: None,
            from_open: false,
            from_closed: None,
            from_filter,
            later_open: false,
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
        self.draft.body = self.body.read(cx).value().to_string();
        self.draft.clone()
    }

    /// The window and tab title: the subject, or what kind of message it is.
    pub(super) fn title(&self, cx: &App) -> String {
        let subject = self.subject.read(cx).value().trim().to_string();
        if self.placement == Placement::Inline {
            return if self.draft.subject.to_lowercase().starts_with("fwd:") {
                "Forward".into()
            } else {
                "Reply".into()
            };
        }
        if subject.is_empty() {
            "New Message".into()
        } else {
            subject
        }
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
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { store.save_draft(&draft) })
                .await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(()) => {
                        this.saved = true;
                        cx.emit(ComposerEvent::Saved);
                    }
                    Err(e) => this.error = Some(e.to_string()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Saves at once, on this thread: for quitting and closing windows.
    pub(super) fn save_now(&mut self, cx: &App) {
        if self.sending {
            return;
        }
        let draft = self.snapshot(cx);
        if !draft.is_empty() {
            let _ = self.backend.store.save_draft(&draft);
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

    // Formatting. The body is Markdown; these edit the text around the
    // selection the way a rich editor's buttons would.

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

    /// Wraps the selection in `mark` (or unwraps it), as for bold.
    fn wrap(&mut self, mark: &str, window: &mut Window, cx: &mut Context<Self>) {
        if !self.draft.formatted {
            self.set_formatted(true, cx);
        }
        let (text, range) = self.selection(window, cx);
        let selected = &text[range.clone()];
        let n = mark.len();
        // Already wrapped, either inside or just outside the selection.
        if selected.len() >= 2 * n && selected.starts_with(mark) && selected.ends_with(mark) {
            let inner = selected[n..selected.len() - n].to_string();
            let end = range.start + inner.len();
            return self.edit(range, &inner, end, window, cx);
        }
        if range.start >= n
            && text[range.end..].starts_with(mark)
            && text[..range.start].ends_with(mark)
        {
            let outer = range.start - n..range.end + n;
            let end = range.end - n;
            return self.edit(outer, selected.to_string().as_str(), end, window, cx);
        }
        let with = format!("{mark}{selected}{mark}");
        // An empty selection leaves the cursor between the marks.
        let cursor = if selected.is_empty() {
            range.start + n
        } else {
            range.start + with.len()
        };
        self.edit(range, &with, cursor, window, cx);
    }

    /// Adds `prefix` to each selected line, or removes it when every line has
    /// it already, as for lists and quotes.
    fn prefix_lines(&mut self, kind: Lines, window: &mut Window, cx: &mut Context<Self>) {
        if !self.draft.formatted {
            self.set_formatted(true, cx);
        }
        let (text, range) = self.selection(window, cx);
        let start = text[..range.start].rfind('\n').map_or(0, |i| i + 1);
        let end = text[range.end..]
            .find('\n')
            .map_or(text.len(), |i| range.end + i);
        let lines: Vec<&str> = text[start..end].split('\n').collect();
        let has = |l: &str| kind.strip(l).is_some();
        let all = lines
            .iter()
            .filter(|l| !l.trim().is_empty())
            .all(|l| has(l))
            && lines.iter().any(|l| !l.trim().is_empty());
        let mut number = 0;
        let changed: Vec<String> = lines
            .iter()
            .map(|l| {
                if all {
                    kind.strip(l).unwrap_or(l).to_string()
                } else if l.trim().is_empty() && lines.len() > 1 {
                    l.to_string()
                } else {
                    number += 1;
                    let bare = Lines::ALL.iter().find_map(|k| k.strip(l)).unwrap_or(l);
                    format!("{}{bare}", kind.prefix(number))
                }
            })
            .collect();
        let with = changed.join("\n");
        let cursor = start + with.len();
        self.edit(start..end, &with, cursor, window, cx);
    }

    fn insert_link(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.draft.formatted {
            self.set_formatted(true, cx);
        }
        let (text, range) = self.selection(window, cx);
        let selected = text[range.clone()].trim().to_string();
        let url_like = selected.starts_with("http://")
            || selected.starts_with("https://")
            || selected.starts_with("www.");
        let (with, cursor) = if url_like {
            let url = if selected.starts_with("www.") {
                format!("https://{selected}")
            } else {
                selected.clone()
            };
            let with = format!("[{selected}]({url})");
            let cursor = range.start + with.len();
            (with, cursor)
        } else {
            let label = if selected.is_empty() {
                "link".to_string()
            } else {
                selected
            };
            let with = format!("[{label}](https://)");
            // Inside the parentheses, ready for the address.
            let cursor = range.start + with.len() - 1;
            (with, cursor)
        };
        self.edit(range, &with, cursor, window, cx);
    }

    /// Pastes copied rich text as plain text or with its formatting (as
    /// Markdown), per Settings, and offers the other. Returns false when the
    /// clipboard has no formatting, leaving the paste to the text field.
    fn paste(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        self.paste = None;
        let Some(plain) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return false;
        };
        let Some(pasted) = super::paste::Pasted::from_clipboard(plain) else {
            return false;
        };
        self.insert_pasted(pasted, window, cx);
        true
    }

    pub(super) fn insert_pasted(
        &mut self,
        pasted: super::paste::Pasted,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let formatted = self.options.paste_formatted;
        let (inserted, other) = if formatted {
            (pasted.formatted, pasted.plain)
        } else {
            (pasted.plain, pasted.formatted)
        };
        let was_formatted = self.draft.formatted;
        if formatted && !was_formatted {
            self.set_formatted(true, cx);
        }
        let (_, range) = self.selection(window, cx);
        let start = range.start;
        self.edit(range, &inserted, start + inserted.len(), window, cx);
        self.paste = Some(PasteOffer {
            start,
            inserted,
            other,
            inserted_formatted: formatted,
            was_formatted,
            body_len: self.body.read(cx).value().len(),
        });
        cx.notify();
    }

    /// Puts the last paste's other version in its place.
    pub(super) fn swap_paste(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(offer) = self.paste.take() else {
            return;
        };
        let end = offer.start + offer.inserted.len();
        let text = self.body.read(cx).value().to_string();
        if text.get(offer.start..end) != Some(offer.inserted.as_str()) {
            cx.notify();
            return;
        }
        let formatted = !offer.inserted_formatted;
        if formatted && !self.draft.formatted {
            self.set_formatted(true, cx);
        } else if !formatted && !offer.was_formatted && self.draft.formatted {
            self.set_formatted(false, cx);
        }
        let cursor = offer.start + offer.other.len();
        self.edit(offer.start..end, &offer.other, cursor, window, cx);
        self.body.update(cx, |s, cx| s.focus(window, cx));
        self.paste = Some(PasteOffer {
            start: offer.start,
            inserted: offer.other,
            other: offer.inserted,
            inserted_formatted: formatted,
            was_formatted: offer.was_formatted,
            body_len: self.body.read(cx).value().len(),
        });
        cx.notify();
    }

    pub(super) fn paste_offer_label(&self) -> Option<&'static str> {
        Some(if self.paste.as_ref()?.inserted_formatted {
            "Paste without formatting"
        } else {
            "Paste with formatting"
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

    fn set_formatted(&mut self, on: bool, cx: &mut Context<Self>) {
        self.draft.formatted = on;
        if !on {
            self.preview = false;
        }
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
            self.error = Some("Choose a time in the future.".into());
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
            self.error = Some(format!(
                "{email} sends only as its aliases. Choose an alias in From."
            ));
            self.from_open = true;
            cx.notify();
            return;
        }
        let d = self.snapshot(cx);
        let any = [&d.to, &d.cc, &d.bcc].iter().any(|f| !f.trim().is_empty());
        if !any {
            self.error = Some("Add at least one recipient.".into());
            self.recipients[TO].update(cx, |s, cx| s.focus(window, cx));
            cx.notify();
            return;
        }
        if let Err(e) = crate::sync::smtp::recipients(&d.reply_to) {
            self.error = Some(format!("Reply-To: {e:#}"));
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
        let mut doubts = vec![];
        if self.options.warn_empty_subject && d.subject.trim().is_empty() {
            doubts.push("This message has no subject.");
        }
        if self.options.attachment_reminder
            && d.attachments.is_empty()
            && mentions_attachment(&d.body)
        {
            doubts.push("The message mentions an attachment, but nothing is attached.");
        }
        if doubts.is_empty() {
            cx.emit(event());
            return;
        }
        let answer = window.prompt(
            PromptLevel::Warning,
            if at.is_some() {
                "Schedule anyway?"
            } else {
                "Send anyway?"
            },
            Some(&doubts.join("\n")),
            &[
                if at.is_some() { "Schedule" } else { "Send" },
                "Keep Editing",
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
            let picker = DateTimePicker::new("later", "Send", presets, colors).phone(mobile);
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
            format!("Not sent {when}: {}", self.draft.send_error)
        } else {
            format!(
                "Scheduled to send {when}. It waits in Drafts until then while Picacho is open."
            )
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
                        .child("Don't Send Later")
                        .on_click(cx.listener(|this, _, _, cx| this.unschedule(cx))),
                ),
        )
    }

    /// Send Later's menu: quick choices, and a calendar and time of day.
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
            .child("Rich text")
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
            .on_click(cx.listener(|t, _, _, cx| {
                cx.stop_propagation();
                let on = !t.draft.formatted;
                t.set_formatted(on, cx)
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
                            .child("Send Later"),
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
                    .child("SEND LATER"),
            )
            .children(self.later_picker.clone())
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
            prompt: Some("Attach".into()),
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
            if p.is_file() && !self.draft.attachments.contains(&p) {
                self.draft.attachments.push(p);
            }
        }
        self.changed(cx);
    }
}

/// Line formats applied by `prefix_lines`.
#[derive(Clone, Copy)]
enum Lines {
    Bullets,
    Numbers,
    Quote,
    Heading,
}
impl Lines {
    const ALL: [Lines; 4] = [Lines::Bullets, Lines::Numbers, Lines::Quote, Lines::Heading];
    fn prefix(self, n: usize) -> String {
        match self {
            Lines::Bullets => "- ".into(),
            Lines::Numbers => format!("{n}. "),
            Lines::Quote => "> ".into(),
            Lines::Heading => "## ".into(),
        }
    }
    fn strip(self, line: &str) -> Option<&str> {
        match self {
            Lines::Bullets => line.strip_prefix("- ").or_else(|| line.strip_prefix("* ")),
            Lines::Numbers => {
                let (n, rest) = line.split_once(". ")?;
                (!n.is_empty() && n.chars().all(|c| c.is_ascii_digit())).then_some(rest)
            }
            Lines::Quote => line.strip_prefix("> ").or_else(|| line.strip_prefix('>')),
            Lines::Heading => line
                .trim_start_matches('#')
                .strip_prefix(' ')
                .filter(|_| line.starts_with('#')),
        }
    }
}

impl Composer {
    fn icon_button(
        &self,
        id: &'static str,
        icon: &'static str,
        tip: &'static str,
        on: bool,
        cx: &Context<Self>,
        click: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> Stateful<Div> {
        let c = self.colors;
        div()
            .id(id)
            .debug_selector(move || format!("compose-{id}"))
            // A fingertip needs a bigger target than a pointer.
            .size(px(if self.mobile {
                super::mobile::TOUCH
            } else if self.single_window {
                36.
            } else {
                28.
            }))
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
            .tooltip(move |w, cx| Tooltip::new(keys(tip)).build(w, cx))
            .map(|d| self.long_press_tip(d, tip, cx))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |this, _, w, cx| {
                cx.stop_propagation();
                click(this, w, cx)
            }))
    }

    /// An icon-only button's name, shown in the main window on a long press
    /// (which arrives as a right click) in the desktop layout on a touch
    /// device, where there is no pointer to hover for its tooltip.
    fn long_press_tip(&self, el: Stateful<Div>, tip: &str, cx: &Context<Self>) -> Stateful<Div> {
        if !self.single_window || self.mobile {
            return el;
        }
        // Without its keyboard shortcut.
        let tip = tip.split(" · ").next().unwrap_or_default().to_owned();
        el.on_mouse_down(
            MouseButton::Right,
            cx.listener(move |_, _: &MouseDownEvent, _, cx| {
                cx.stop_propagation();
                cx.emit(ComposerEvent::Notice(tip.clone()));
            }),
        )
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
                "Save draft · ⌘S",
                false,
                cx,
                |this, _, cx| this.save(cx),
            ))
            .child(self.icon_button(
                "dock-main",
                "shrink",
                "Move into main window",
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
            "Save draft · ⌘S",
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
                            "Restore"
                        } else {
                            "Minimize"
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
                            "Exit full size"
                        } else {
                            "Full size"
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
                    .when(!self.single_window, |d| {
                        d.child(self.icon_button(
                            "new-window",
                            "popout",
                            "Open in new window",
                            false,
                            cx,
                            |_, _, cx| cx.emit(ComposerEvent::Place(Placement::Window)),
                        ))
                    });
            }
            Placement::Inline => {
                header = header
                    .child(self.icon_button(
                        "pop-out",
                        "expand",
                        "Pop out reply",
                        false,
                        cx,
                        |_, _, cx| cx.emit(ComposerEvent::Place(Placement::Floating)),
                    ))
                    .when(!self.single_window, |d| {
                        d.child(self.icon_button(
                            "new-window",
                            "popout",
                            "Open in new window",
                            false,
                            cx,
                            |_, _, cx| cx.emit(ComposerEvent::Place(Placement::Window)),
                        ))
                    });
            }
            Placement::Window => {
                header = header.child(self.icon_button(
                    "dock-main",
                    "shrink",
                    "Move into main window",
                    false,
                    cx,
                    |_, _, cx| cx.emit(ComposerEvent::Place(Placement::Floating)),
                ));
            }
        }
        header.child(
            self.icon_button("close", "close", "Save and close", false, cx, |_, _, cx| {
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
        let tip = format!("New {} alias for {}", kind.label(), account.email);
        let (service, mailbox) = (service.clone(), account.id.clone());
        Some(
            div()
                .id(id)
                .debug_selector(move || format!("{}-{}", id.0, id.1))
                .size(px(if self.single_window { 32. } else { 24. }))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(5.))
                .cursor_pointer()
                .hover(move |s| s.bg(rgb(c.soft)))
                .child(icon_el("alias", 14., c.muted))
                .map(|d| self.long_press_tip(d, &tip, cx))
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
    fn choose_sender(
        &mut self,
        account: String,
        alias: Option<String>,
        identity: Option<String>,
        cx: &mut Context<Self>,
    ) {
        self.draft.account = account;
        self.draft.send_as = alias.unwrap_or_default();
        self.draft.identity = identity.unwrap_or_default();
        self.from_open = false;
        self.changed(cx);
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
                "Removed address".to_owned(),
                format!(
                    "This address was removed from {}. Choose another.",
                    current.email
                ),
            ),
            None if identity.is_some() => {
                let i = identity.unwrap();
                let through = match &i.smtp {
                    Some(s) => format!("Sent through {}.", s.host),
                    None => format!("Sent through {}'s server.", current.email),
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
                "Choose an alias".to_owned(),
                format!(
                    "{} sends only as its aliases (Settings → Forwarding).",
                    current.email
                ),
            ),
            None if missing => (
                self.draft.send_as.clone(),
                format!(
                    "{} no longer exists. Choose another address.",
                    self.draft.send_as
                ),
            ),
            Some(a) => (
                a.email.clone(),
                format!(
                    "Sent through {} from {}. Recipients see {}.",
                    a.service.label(),
                    current.email,
                    a.email
                ),
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
        let mut row = self.row("From", field).relative();
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
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.choose_sender(account.clone(), alias.clone(), chosen.clone(), cx)
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
            list = list.child(heading("Accounts".into()));
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
                    Some(s) => format!("Through {}", s.host),
                    None => format!("Through {}", a.email),
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
                list = list.child(heading(format!("{} aliases", a.service.label())));
            }
            // Through the mailbox the service delivers to, else this one.
            let through = a
                .deliver_to
                .as_ref()
                .and_then(|id| self.accounts.iter().find(|x| &x.id == id))
                .unwrap_or(current);
            let caption = if a.note.is_empty() {
                format!("Sends through {}", through.email)
            } else {
                format!("{} · through {}", a.note, through.email)
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
                    .child("No address matches."),
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
        let label = ["To", "Cc", "Bcc", "Reply-To"][field];
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
                    .child(toggle("compose-show-cc", "Cc", self.show_cc, CC))
                    .child(toggle("compose-show-bcc", "Bcc", self.show_bcc, BCC))
                    .child(toggle(
                        "compose-show-reply-to",
                        "Reply-To",
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
            .child(
                self.icon_button("bold", "bold", "Bold · ⌘B", false, cx, |t, w, cx| {
                    t.wrap("**", w, cx)
                }),
            )
            .child(self.icon_button(
                "italic",
                "italic",
                "Italic · ⌘I",
                false,
                cx,
                |t, w, cx| t.wrap("_", w, cx),
            ))
            .child(self.icon_button(
                "strike",
                "strike",
                "Strikethrough · ⌘⇧X",
                false,
                cx,
                |t, w, cx| t.wrap("~~", w, cx),
            ))
            .child(
                self.icon_button("code", "code", "Code · ⌘E", false, cx, |t, w, cx| {
                    t.wrap("`", w, cx)
                }),
            )
            .child(sep())
            .child(self.icon_button(
                "heading",
                "heading",
                "Heading · ⌘⌥1",
                false,
                cx,
                |t, w, cx| t.prefix_lines(Lines::Heading, w, cx),
            ))
            .child(self.icon_button(
                "bullets",
                "list",
                "Bulleted list · ⌘⇧8",
                false,
                cx,
                |t, w, cx| t.prefix_lines(Lines::Bullets, w, cx),
            ))
            .child(self.icon_button(
                "numbers",
                "list-ordered",
                "Numbered list · ⌘⇧7",
                false,
                cx,
                |t, w, cx| t.prefix_lines(Lines::Numbers, w, cx),
            ))
            .child(self.icon_button(
                "quote",
                "quote",
                "Quote · ⌘⇧9",
                false,
                cx,
                |t, w, cx| t.prefix_lines(Lines::Quote, w, cx),
            ))
            .child(
                self.icon_button("link", "link", "Link · ⌘K", false, cx, |t, w, cx| {
                    t.insert_link(w, cx)
                }),
            )
            .child(div().flex_1())
            .child(self.icon_button(
                "preview",
                "preview",
                "Preview · ⌘⇧P",
                self.preview,
                cx,
                |t, _, cx| {
                    t.preview = !t.preview;
                    cx.notify();
                },
            ))
    }

    fn body_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let c = self.colors;
        let inline = self.placement == Placement::Inline;
        if self.preview && self.draft.formatted {
            let text = self.body.read(cx).value().to_string();
            return div()
                .id("compose-preview")
                .debug_selector(|| "compose-preview".into())
                .when(!inline, |d| d.flex_1().min_h_0())
                .when(inline, |d| d.min_h(px(220.)))
                .overflow_y_scroll()
                .px_4()
                .py_3()
                .child(
                    gpui_component::text::TextView::markdown("compose-preview-text", text)
                    .selectable(true)
                    .h_auto()
                    .text_size(px(14.)),
                )
                .into_any_element();
        }
        div()
            .id("compose-body")
            .debug_selector(|| "compose-body".into())
            .relative()
            .when(!inline, |d| d.flex_1().min_h_0())
            .when(inline, |d| d.h(px(240.)))
            .px_2()
            .py_1()
            .capture_action(cx.listener(|this, _: &gpui_component::input::Paste, w, cx| {
                if this.paste(w, cx) {
                    cx.stop_propagation();
                }
            }))
            .child(
                Textarea::new(&self.body)
                    .appearance(false)
                    .disabled(self.sending)
                    .h_full()
                    // Phones read body text at their system size (17pt).
                    .text_size(px(if self.mobile { 17. } else { 14. })),
            )
            // Over the text, so it comes after.
            .children(self.paste_offer(cx))
            .bg(rgb(c.surface))
            .into_any_element()
    }

    fn attachments(&self, cx: &Context<Self>) -> Option<Div> {
        if self.draft.attachments.is_empty() {
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
            let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
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
                            .size(px(if self.single_window { 28. } else { 20. }))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(5.))
                            .cursor_pointer()
                            .hover(move |s| s.bg(rgb(c.border)))
                            .child(icon_el("close", 11., c.muted))
                            .tooltip(|w, cx| Tooltip::new("Remove").build(w, cx))
                            .map(|d| self.long_press_tip(d, "Remove", cx))
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
            .child(if self.sending { "Sending…" } else { "Send" })
            .tooltip(|w, cx| Tooltip::new(keys("Send · ⌘Enter")).build(w, cx))
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
                    d.child("Draft saved")
                })
                .when(!self.saved, |d| d.child("Editing…"))
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
                "Send later…",
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
                "Attach files (or drop them here)",
                false,
                cx,
                |t, _, cx| t.attach(cx),
            ))
            .child(if self.mobile {
                self.rich_text_toggle(cx)
            } else {
                self.icon_button(
                    "format",
                    "format",
                    if self.draft.formatted {
                        "Formatting on: sent with bold, lists and links"
                    } else {
                        "Plain text: turn formatting on"
                    },
                    self.draft.formatted,
                    cx,
                    |t, _, cx| {
                        let on = !t.draft.formatted;
                        t.set_formatted(on, cx)
                    },
                )
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
                "Discard draft",
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
            .on_action(cx.listener(|t, _: &Bold, w, cx| t.wrap("**", w, cx)))
            .on_action(cx.listener(|t, _: &Italic, w, cx| t.wrap("_", w, cx)))
            .on_action(cx.listener(|t, _: &Strike, w, cx| t.wrap("~~", w, cx)))
            .on_action(cx.listener(|t, _: &InlineCode, w, cx| t.wrap("`", w, cx)))
            .on_action(cx.listener(|t, _: &InsertLink, w, cx| t.insert_link(w, cx)))
            .on_action(cx.listener(|t, _: &Bullets, w, cx| t.prefix_lines(Lines::Bullets, w, cx)))
            .on_action(cx.listener(|t, _: &Numbers, w, cx| t.prefix_lines(Lines::Numbers, w, cx)))
            .on_action(cx.listener(|t, _: &QuoteLines, w, cx| t.prefix_lines(Lines::Quote, w, cx)))
            .on_action(cx.listener(|t, _: &Heading, w, cx| t.prefix_lines(Lines::Heading, w, cx)))
            .on_action(cx.listener(|t, _: &SaveDraft, _, cx| t.save(cx)))
            .on_action(cx.listener(|t, _: &SendDraft, w, cx| t.request_send(w, cx)))
            .on_action(cx.listener(|t, _: &TogglePreview, _, cx| t.toggle_preview(cx)))
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                this.add_files(paths.paths().to_vec(), cx)
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
            root = root.child(self.row("Subject", self.input(&self.subject)));
        }
        if self.draft.formatted {
            root = root.child(self.toolbar(cx));
        }
        root.child(self.body_view(cx))
            .children(self.attachments(cx))
            .child(self.footer(cx))
            // A phone shows Send Later as a page over the message.
            .when(self.mobile && self.later_open, |d| d.child(self.later_page(cx)))
    }
}

fn human_size(bytes: u64) -> String {
    match bytes {
        0..1024 => format!("{bytes} B"),
        1024..1_048_576 => format!("{} KB", bytes / 1024),
        _ => format!("{:.1} MB", bytes as f64 / 1_048_576.),
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
        window.set_window_title(&title);
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::prelude::v1::test;

    #[test]
    fn line_formats_recognize_their_prefixes() {
        assert_eq!(Lines::Numbers.strip("12. item"), Some("item"));
        assert_eq!(Lines::Numbers.strip("x. item"), None);
        assert_eq!(Lines::Bullets.strip("* item"), Some("item"));
        assert_eq!(Lines::Heading.strip("## Title"), Some("Title"));
        assert_eq!(Lines::Heading.strip("Title"), None);
        assert_eq!(Lines::Quote.strip(">quoted"), Some("quoted"));
    }
}

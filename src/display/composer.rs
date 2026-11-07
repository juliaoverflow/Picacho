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
pub(super) fn bindings() -> Vec<KeyBinding> {
    let ctx = Some("Composer");
    vec![
        KeyBinding::new("secondary-b", Bold, ctx),
        KeyBinding::new("secondary-i", Italic, ctx),
        KeyBinding::new("secondary-shift-x", Strike, ctx),
        KeyBinding::new("secondary-e", InlineCode, ctx),
        KeyBinding::new("secondary-k", InsertLink, ctx),
        KeyBinding::new("secondary-shift-8", Bullets, ctx),
        KeyBinding::new("secondary-shift-7", Numbers, ctx),
        KeyBinding::new("secondary-shift-9", QuoteLines, ctx),
        KeyBinding::new("secondary-alt-1", Heading, ctx),
        KeyBinding::new("secondary-s", SaveDraft, ctx),
        KeyBinding::new("secondary-enter", SendDraft, ctx),
        KeyBinding::new("secondary-shift-p", TogglePreview, ctx),
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

/// Recipient fields, in order.
const TO: usize = 0;
const CC: usize = 1;
const BCC: usize = 2;

pub(super) struct Composer {
    pub(super) draft: Draft,
    pub(super) placement: Placement,
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
    recipients: [Entity<InputState>; 3],
    subject: Entity<InputState>,
    pub(super) body: Entity<InputState>,
    show_cc: bool,
    show_bcc: bool,
    preview: bool,
    from_open: bool,
    accounts: Vec<Account>,
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
        contacts: Rc<Vec<(String, String)>>,
        options: ComposeOptions,
        colors: Colors,
        backend: Arc<Backend>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let field = |placeholder: &str, window: &mut Window, cx: &mut Context<Self>| {
            cx.new(|cx| InputState::new(window, cx).placeholder(placeholder.to_owned()))
        };
        let recipients = [
            field("Recipients", window, cx),
            field("Cc", window, cx),
            field("Bcc", window, cx),
        ];
        let subject = field("Subject", window, cx);
        let body = cx.new(|cx| {
            InputState::new(window, cx)
                .multi_line(true)
                .placeholder("Write your message…")
        });
        let values = [&draft.to, &draft.cc, &draft.bcc];
        for (state, value) in recipients.iter().zip(values) {
            let value = value.clone();
            state.update(cx, |s, cx| s.set_value(value, window, cx));
        }
        subject.update(cx, |s, cx| s.set_value(draft.subject.clone(), window, cx));
        body.update(cx, |s, cx| s.set_value(draft.body.clone(), window, cx));
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
                            Timer::after(Duration::from_millis(150)).await;
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
        for state in [&subject, &body] {
            subscriptions.push(cx.subscribe(state, |this, _, e, cx| {
                if matches!(e, InputEvent::Change) {
                    this.changed(cx);
                }
            }));
        }
        let show_cc = !draft.cc.is_empty();
        let show_bcc = !draft.bcc.is_empty();
        Self {
            draft,
            placement,
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
            preview: false,
            from_open: false,
            accounts,
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
            &self.recipients[TO]
        } else if self.subject.read(cx).value().trim().is_empty()
            && self.placement != Placement::Inline
        {
            &self.subject
        } else {
            &self.body
        };
        target.update(cx, |s, cx| {
            s.focus(window, cx);
            // Replies start writing above the quoted message.
            if std::ptr::eq(target, &self.body) {
                s.set_cursor_position(gpui_component::input::Position::new(0, 0), window, cx);
            }
        });
    }

    /// The draft as currently written.
    pub(super) fn snapshot(&mut self, cx: &App) -> Draft {
        let value = |s: &Entity<InputState>| s.read(cx).value().to_string();
        self.draft.to = value(&self.recipients[TO]);
        self.draft.cc = value(&self.recipients[CC]);
        self.draft.bcc = value(&self.recipients[BCC]);
        self.draft.subject = value(&self.subject);
        self.draft.body = value(&self.body);
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
            Timer::after(Duration::from_millis(800)).await;
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
            s.set_value(next.clone(), window, cx);
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
        if self.sending {
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
            cx.emit(ComposerEvent::Send);
            return;
        }
        let answer = window.prompt(
            PromptLevel::Warning,
            "Send anyway?",
            Some(&doubts.join("\n")),
            &["Send", "Keep Editing"],
            cx,
        );
        cx.spawn(async move |this, cx| {
            if let Ok(0) = answer.await {
                let _ = this.update(cx, |_, cx| cx.emit(ComposerEvent::Send));
            }
        })
        .detach();
    }

    /// Fills the recipient and subject fields, as typing would (tests).
    #[cfg(test)]
    pub(super) fn fill(&mut self, to: &str, subject: &str, w: &mut Window, cx: &mut Context<Self>) {
        let (to, subject) = (to.to_string(), subject.to_string());
        self.recipients[TO].update(cx, |s, cx| s.set_value(to, w, cx));
        self.subject.update(cx, |s, cx| s.set_value(subject, w, cx));
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

    fn add_files(&mut self, paths: Vec<std::path::PathBuf>, cx: &mut Context<Self>) {
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
            .size(px(28.))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(6.))
            .cursor_pointer()
            .when(on, |d| d.bg(rgb(c.soft)))
            .hover(move |s| s.bg(rgb(c.border)))
            .active(|s| s.opacity(0.7))
            .child(icon_el(icon, 15., if on { c.accent } else { c.muted }))
            .tooltip(move |w, cx| Tooltip::new(keys(tip)).build(w, cx))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |this, _, w, cx| {
                cx.stop_propagation();
                click(this, w, cx)
            }))
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
                    .child(self.icon_button(
                        "new-window",
                        "popout",
                        "Open in new window",
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
                        "Pop out reply",
                        false,
                        cx,
                        |_, _, cx| cx.emit(ComposerEvent::Place(Placement::Floating)),
                    ))
                    .child(self.icon_button(
                        "new-window",
                        "popout",
                        "Open in new window",
                        false,
                        cx,
                        |_, _, cx| cx.emit(ComposerEvent::Place(Placement::Window)),
                    ));
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
            .h(px(38.))
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

    fn sender_row(&self, cx: &Context<Self>) -> Option<Div> {
        let c = self.colors;
        let current = self
            .accounts
            .iter()
            .find(|a| a.id == self.draft.account)
            .or(self.accounts.first())?;
        let several = self.accounts.len() > 1;
        let label = if current.name.is_empty() || current.name == current.email {
            current.email.clone()
        } else {
            format!("{} <{}>", current.name, current.email)
        };
        let mut picker = div()
            .id("compose-from")
            .debug_selector(|| "compose-from".into())
            .flex()
            .items_center()
            .gap_1()
            .min_w_0()
            .text_size(px(13.))
            .child(div().min_w_0().truncate().child(label));
        if several {
            picker = picker
                .cursor_pointer()
                .px_1()
                .ml(px(-4.))
                .rounded(px(5.))
                .hover(move |s| s.bg(rgb(c.raised)))
                .child(icon_el("chevron-down", 12., c.muted))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.from_open = !this.from_open;
                    cx.notify();
                }));
        }
        let mut row = self.row("From", picker).relative();
        if self.from_open {
            let mut list = div()
                .absolute()
                .top(px(36.))
                .left(px(70.))
                .min_w(px(260.))
                .p_1()
                .rounded(px(10.))
                .border_1()
                .border_color(rgb(c.border))
                .bg(rgb(c.raised))
                .shadow_lg()
                .occlude()
                .flex()
                .flex_col();
            for (i, a) in self.accounts.iter().enumerate() {
                let id = a.id.clone();
                let on = a.id == current.id;
                list = list.child(
                    div()
                        .id(("from-account", i))
                        .h(px(30.))
                        .px_2()
                        .flex()
                        .items_center()
                        .gap_2()
                        .rounded(px(6.))
                        .cursor_pointer()
                        .text_size(px(13.))
                        .hover(move |s| s.bg(rgb(c.soft)))
                        .child(
                            div()
                                .size(px(14.))
                                .when(on, |d| d.child(icon_el("check", 14., c.accent))),
                        )
                        .child(div().flex_1().truncate().child(a.email.clone()))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.draft.account = id.clone();
                            this.from_open = false;
                            this.changed(cx);
                        })),
                );
            }
            row = row.child(deferred(list).with_priority(2));
        }
        Some(row)
    }

    fn recipient_row(&self, field: usize, cx: &Context<Self>) -> Div {
        let c = self.colors;
        let label = ["To", "Cc", "Bcc"][field];
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
                        if which == CC {
                            this.show_cc = true;
                        } else {
                            this.show_bcc = true;
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
                    .child(toggle("compose-show-bcc", "Bcc", self.show_bcc, BCC)),
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

    fn body_view(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
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
                    gpui_component::text::TextView::markdown(
                        "compose-preview-text",
                        text,
                        window,
                        cx,
                    )
                    .selectable(true)
                    .h_auto()
                    .text_size(px(14.)),
                )
                .into_any_element();
        }
        div()
            .id("compose-body")
            .debug_selector(|| "compose-body".into())
            .when(!inline, |d| d.flex_1().min_h_0())
            .when(inline, |d| d.h(px(240.)))
            .px_2()
            .py_1()
            .child(
                Input::new(&self.body)
                    .appearance(false)
                    .disabled(self.sending)
                    .h_full()
                    .text_size(px(14.)),
            )
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
                            .size(px(20.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(5.))
                            .cursor_pointer()
                            .hover(move |s| s.bg(rgb(c.border)))
                            .child(icon_el("close", 11., c.muted))
                            .tooltip(|w, cx| Tooltip::new("Remove").build(w, cx))
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
            .h(px(34.))
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
            .h(px(52.))
            .flex_shrink_0()
            .px_3()
            .flex()
            .items_center()
            .gap_1()
            .border_t_1()
            .border_color(rgb(c.border))
            .child(send)
            .child(div().w(px(6.)))
            .child(self.icon_button(
                "attach",
                "paperclip",
                "Attach files (or drop them here)",
                false,
                cx,
                |t, _, cx| t.attach(cx),
            ))
            .child(self.icon_button(
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
            ))
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
            .on_action(cx.listener(|t, _: &TogglePreview, _, cx| {
                if t.draft.formatted {
                    t.preview = !t.preview;
                    cx.notify();
                }
            }))
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                this.add_files(paths.paths().to_vec(), cx)
            }))
            .child(self.header(cx));
        if self.minimized {
            return root;
        }
        if !inline || self.accounts.len() > 1 {
            root = root.children(self.sender_row(cx));
        }
        root = root.child(self.recipient_row(TO, cx));
        if self.show_cc {
            root = root.child(self.recipient_row(CC, cx));
        }
        if self.show_bcc {
            root = root.child(self.recipient_row(BCC, cx));
        }
        if !inline {
            root = root.child(self.row("Subject", self.input(&self.subject)));
        }
        if self.draft.formatted {
            root = root.child(self.toolbar(cx));
        }
        root.child(self.body_view(window, cx))
            .children(self.attachments(cx))
            .child(self.footer(cx))
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
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(c.surface))
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

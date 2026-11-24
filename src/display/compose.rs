//! Writing mail: opening composers for new messages, replies and drafts,
//! placing them (floating, maximized, inline or in a window), and sending
//! with a short undo delay. The editor itself is `composer.rs`.
use super::composer::{ComposeWindow, ComposerEvent};
use super::*;

/// A message handed to the outbox: waiting out its undo delay, then sent.
pub(super) struct Outgoing {
    pub(super) id: u64,
    composer: Entity<Composer>,
    /// Where it was written, to put it back on Undo or failure.
    placement: Placement,
    pub(super) delivering: bool,
}

/// Floating composer sizes.
const FLOAT_W: f32 = 560.;
const FLOAT_H: f32 = 620.;
const BAR_W: f32 = 280.;
const BAR_H: f32 = 40.;
const GAP: f32 = 12.;

impl MailApp {
    /// Starts a new message (mode 0), a reply (1), a reply to all (2), a
    /// forward (3, with the original's attachments) or a forward as an
    /// attachment (4) of the open message.
    pub(super) fn new_compose(&mut self, mode: u8, w: &mut Window, cx: &mut Context<Self>) {
        let original = self.selected_mail();
        self.compose_reply(mode, original, None, w, cx);
    }

    /// Like `new_compose`, answering `original` (when mode > 0), in
    /// `placement` if given rather than where the settings put it.
    pub(super) fn compose_reply(
        &mut self,
        mode: u8,
        original: Option<Mail>,
        placement: Option<Placement>,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.compose_with(mode, original, placement, None, None, w, cx);
    }

    /// Answers the open message with `text` (a suggested reply) written in:
    /// a reply (to everyone, if Reply does that) where replies open, ready
    /// to send or go on writing. A reply already open to the message gets
    /// the text instead.
    pub(super) fn reply_with_text(&mut self, text: String, w: &mut Window, cx: &mut Context<Self>) {
        let Some(original) = self.selected_mail() else {
            return;
        };
        if let Some(open) = self.open_answer(&original, false, cx) {
            // Typed into that reply, brought into view as Reply would.
            open.update(cx, |c, cx| {
                c.minimized = false;
                cx.notify();
            });
            match open.read(cx).window {
                Some(handle) => {
                    let _ = handle.update(cx, |_, w, cx| {
                        open.update(cx, |c, cx| c.insert_reply_text(&text, w, cx));
                        w.activate_window();
                    });
                }
                None => {
                    open.update(cx, |c, cx| c.insert_reply_text(&text, w, cx));
                    if open.read(cx).placement == Placement::Inline {
                        self.scroll_reader_to_end(w);
                    }
                }
            }
            cx.notify();
            return;
        }
        let mode = if self.compose_options.default_reply_all {
            2
        } else {
            1
        };
        let end = text.len();
        self.compose_with(mode, Some(original.clone()), None, None, Some(text), w, cx);
        // Writing goes on after the suggested reply, not before it.
        if let Some(open) = self.open_answer(&original, false, cx) {
            match open.read(cx).window {
                Some(handle) => {
                    let _ = handle.update(cx, |_, w, cx| {
                        open.update(cx, |c, cx| c.put_cursor(end, w, cx))
                    });
                }
                None => open.update(cx, |c, cx| c.put_cursor(end, w, cx)),
            }
        }
    }

    /// A new message to `to` (an address list), sent from the account of
    /// the open message, as from a reader's address menu.
    pub(super) fn compose_to(&mut self, to: String, w: &mut Window, cx: &mut Context<Self>) {
        self.compose_with(0, None, None, Some(to), None, w, cx);
    }

    /// A new message from a `mailto:` link (RFC 6068), in Picacho's own
    /// composer: its recipients, Cc, Bcc, subject and body. Other header
    /// fields the link names are left out (§4: In-Reply-To aside, a link
    /// mustn't set what isn't shown).
    pub(super) fn compose_mailto(&mut self, link: &str, w: &mut Window, cx: &mut Context<Self>) {
        let Some(m) = super::links::Mailto::parse(link) else {
            self.notice("That email link isn't valid.", true, cx);
            return;
        };
        self.compose_with(0, None, None, Some(m.to.clone()), None, w, cx);
        // The composer just opened is the newest.
        if let Some(composer) = self.composers.last().cloned() {
            composer.update(cx, |c, cx| {
                if !m.cc.is_empty() {
                    c.draft.cc = m.cc.clone();
                }
                if !m.bcc.is_empty() {
                    c.draft.bcc = m.bcc.clone();
                }
                if !m.subject.is_empty() {
                    c.draft.subject = m.subject.clone();
                }
                if !m.body.is_empty() {
                    c.draft.body = format!("{}{}", m.body, c.draft.body);
                }
                if !m.in_reply_to.is_empty() {
                    c.draft.reply_id = m.in_reply_to.clone();
                }
                c.reload_fields(w, cx);
                cx.notify();
            });
        }
    }

    /// `intro` (a suggested reply) starts the body, above the signature and
    /// quote.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn compose_with(
        &mut self,
        mode: u8,
        original: Option<Mail>,
        placement: Option<Placement>,
        to: Option<String>,
        intro: Option<String>,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.onboarding {
            return;
        }
        let mut forced = placement;
        let options = self.compose_options.clone();
        let original = original.filter(|_| mode > 0);
        // Asked again to answer a message whose answer of that kind is open
        // (Reply and Reply all are one kind), go back to it rather than start
        // another. A message has one inline answer: asked for the other kind
        // (Reply while forwarding it), open that where new messages open, so
        // it is seen and neither is lost.
        if let Some(m) = &original {
            if let Some(existing) = self.open_answer(m, mode >= 3, cx) {
                let inline = existing.read(cx).placement == Placement::Inline;
                let window = existing.read(cx).window;
                existing.update(cx, |c, cx| {
                    c.minimized = false;
                    c.focus_first(w, cx);
                    cx.notify();
                });
                if inline {
                    self.scroll_reader_to_end(w);
                } else if let Some(window) = window {
                    let _ = window.update(cx, |_, w, _| w.activate_window());
                }
                cx.notify();
                return;
            }
            if self.inline_composer(&m.id).is_some() {
                forced = forced.or(Some(match options.open_in {
                    ComposeOpen::Floating => Placement::Floating,
                    ComposeOpen::Maximized => Placement::Maximized,
                    ComposeOpen::Window => Placement::Window,
                }));
            }
        }
        // Writing to someone from an open message: from the account it came to.
        let account = original
            .as_ref()
            .and_then(|m| self.account_of(m))
            .or_else(|| {
                to.as_ref()
                    .and_then(|_| self.selected_mail())
                    .and_then(|m| self.account_of(&m))
            })
            .or_else(|| self.current_account());
        let own = account
            .as_ref()
            .map(|a| a.email.clone())
            .unwrap_or_default();
        // The sending address's signature, or its mailbox's, or everyone's.
        // An HTML signature's HTML is kept with the draft, for its HTML.
        let sign = |reply: bool, draft: &mut Draft| -> String {
            let address = if !draft.send_as.is_empty() {
                draft.send_as.clone()
            } else {
                account
                    .as_ref()
                    .and_then(|a| a.identities.iter().find(|i| i.id == draft.identity))
                    .map(|i| i.email.clone())
                    .unwrap_or_else(|| own.clone())
            };
            if reply && !options.signature_on_replies {
                return String::new();
            }
            let signature = options.signature(&draft.account, &address);
            draft.signature_html = signature.html.clone();
            signature.block()
        };
        let mut draft = Draft {
            id: uuid::Uuid::new_v4().to_string(),
            account: account
                .as_ref()
                .map(|a| a.id.clone())
                .unwrap_or_else(|| "demo".into()),
            formatted: options.formatted,
            ..Draft::default()
        };
        match &original {
            None => draft.body = sign(false, &mut draft),
            Some(m) => {
                let forward = mode >= 3;
                // RFC 9788 §6.1: the real subject, not a hidden one's "...".
                draft.subject = reply_subject(&self.backend.real_subject(m), forward);
                // Marked answered or forwarded once this is sent.
                draft.original = m.id.clone();
                draft.forwarding = forward;
                // RFC 9788 §6.1: as protected as what it answers.
                let start = self.backend.answering(m).start();
                if start.encrypt {
                    draft.encrypt = true;
                    draft.encrypt_subject = start.hide_subject;
                }
                // Answer as the alias the message was sent to.
                let aliases = fwd::sendable(&self.forwarding);
                if let Some(alias) = alias_addressed(m, &aliases) {
                    draft.send_as = alias.email.clone();
                }
                // Sent to one of the account's other addresses: answer as it.
                if draft.send_as.is_empty()
                    && let Some(i) = account.as_ref().and_then(|a| identity_addressed(m, a))
                {
                    draft.identity = i.id.clone();
                }
                // A mailbox kept to its aliases answers its own sent mail as
                // the alias it went out as.
                let mask = self.mask_for(m);
                if draft.send_as.is_empty()
                    && let Some(k) = &mask
                {
                    draft.send_as = k.alias.clone();
                }
                // Nothing quoted names the mailbox behind the alias.
                let masked = |text: &str| match &mask {
                    Some(k) => k.apply(text).into_owned(),
                    None => text.to_owned(),
                };
                let recipients = |list: &str| match &mask {
                    Some(k) => k.recipients(list),
                    None => list.to_owned(),
                };
                let sender = match &mask {
                    Some(k) if k.sent => k.sender(),
                    _ => m.sender.clone(),
                };
                if !forward {
                    // None of the account's own addresses answer themselves.
                    let mut mine = vec![own.as_str(), draft.send_as.as_str()];
                    if let Some(a) = &account {
                        mine.extend(a.identities.iter().map(|i| i.email.as_str()));
                    }
                    let (to, cc) = reply_recipients(m, mode == 2, &mine);
                    // Reverse aliases show as the contacts they stand for;
                    // sending routes them through the alias again.
                    let (to, cc) = match &mask {
                        Some(k) => (
                            fwd::unreverse(&to, &k.contacts).into_owned(),
                            fwd::unreverse(&cc, &k.contacts).into_owned(),
                        ),
                        None => (to, cc),
                    };
                    draft.to = to;
                    draft.cc = cc;
                    draft.reply_id = m.message_id.clone();
                    draft.references = format!("{} {}", m.references(), m.message_id)
                        .trim()
                        .to_string();
                }
                let quoted = masked(&m.body.clone().unwrap_or_default());
                let quote = if mode == 4 {
                    // The message itself goes as an attachment.
                    String::new()
                } else if forward {
                    format!(
                        "\n\n---------- Forwarded message ----------\nFrom: {} <{}>\nDate: {}\nSubject: {}\nTo: {}\n{}\n{}",
                        sender,
                        masked(&m.from),
                        date_full(m.date),
                        m.subject,
                        recipients(&m.to),
                        if m.cc().is_empty() {
                            String::new()
                        } else {
                            format!("Cc: {}\n", recipients(m.cc()))
                        },
                        quoted
                    )
                } else if options.quote_original {
                    format!(
                        "\n\nOn {}, {} wrote:\n{}",
                        date_full(m.date),
                        sender,
                        quoted
                            .lines()
                            // Nested quotes stay compact: ">>", not "> >".
                            .map(|l| if l.is_empty() || l.starts_with('>') {
                                format!(">{l}")
                            } else {
                                format!("> {l}")
                            })
                            .collect::<Vec<_>>()
                            .join("\n")
                    )
                } else {
                    String::new()
                };
                let signature = sign(true, &mut draft);
                draft.body = format!("{signature}{quote}");
            }
        }
        if let Some(to) = to {
            draft.to = to;
        }
        if let Some(intro) = intro {
            draft.body = format!("{intro}{}", draft.body);
        }
        // A mailbox kept to its aliases writes as one of them.
        if draft.send_as.is_empty()
            && draft.identity.is_empty()
            && let Some(alias) =
                fwd::default_alias(&self.forwarding, &self.accounts, &draft.account)
        {
            draft.send_as = alias;
        }
        let placement = forced.unwrap_or(match (&original, options.reply_placement) {
            (Some(_), ReplyPlacement::Inline) if self.reader_visible(w) => Placement::Inline,
            _ => match options.open_in {
                ComposeOpen::Floating => Placement::Floating,
                ComposeOpen::Maximized => Placement::Maximized,
                ComposeOpen::Window => Placement::Window,
            },
        });
        let answers = original
            .as_ref()
            .filter(|_| placement == Placement::Inline)
            .map(|m| m.id.clone());
        let composer = self.open_composer(draft, placement, answers, w, cx);
        // A forward carries the original's attachments, or the original.
        if let (Some(composer), Some(m)) = (composer, original)
            && (mode == 4 || (mode == 3 && m.has_attachments()))
        {
            self.attach_forwarded(composer, m, mode == 4, cx);
        }
    }

    /// Opens a saved draft where new messages open.
    pub(super) fn open_draft(&mut self, draft: Draft, w: &mut Window, cx: &mut Context<Self>) {
        // Here, or by the background monitor as the window opened.
        if self.scheduled_sending.contains(&draft.id)
            || self.backend.sending_scheduled(&draft.id)
        {
            self.notice("This scheduled message is being sent now.", false, cx);
            return;
        }
        if let Some(open) = self
            .composers
            .iter()
            .find(|c| c.read(cx).draft.id == draft.id)
            .cloned()
        {
            open.update(cx, |c, cx| {
                c.minimized = false;
                c.focus_first(w, cx);
                cx.notify();
            });
            return;
        }
        let placement = match self.compose_options.open_in {
            ComposeOpen::Floating => Placement::Floating,
            ComposeOpen::Maximized => Placement::Maximized,
            ComposeOpen::Window => Placement::Window,
        };
        self.open_composer(draft, placement, None, w, cx);
    }

    /// Whether the reader is on screen, so an inline reply can be seen.
    fn reader_visible(&self, w: &Window) -> bool {
        self.selected.is_some()
            && !self.reader_collapsed
            && (super::window::content_size(w).width >= px(900.) || self.selected.is_some())
    }

    /// An open composer answering `m`: forwarding it, or replying to it.
    fn open_answer(&self, m: &Mail, forward: bool, cx: &App) -> Option<Entity<Composer>> {
        if forward {
            // Forwards don't name the message; only its inline one is known.
            return self
                .inline_composer(&m.id)
                .filter(|c| c.read(cx).draft.reply_id.is_empty());
        }
        if m.message_id.is_empty() {
            return self
                .inline_composer(&m.id)
                .filter(|c| !c.read(cx).draft.reply_id.is_empty());
        }
        let replies = |c: &&Entity<Composer>| c.read(cx).draft.reply_id == m.message_id;
        // The inline one first: it is where the reader is.
        self.inline_composer(&m.id)
            .filter(|c| replies(&c))
            .or_else(|| self.composers.iter().find(replies).cloned())
    }

    pub(super) fn inline_composer(&self, message: &str) -> Option<Entity<Composer>> {
        let id = self.inline_replies.get(message)?;
        self.composers
            .iter()
            .find(|c| c.entity_id() == *id)
            .cloned()
    }

    /// Creates a composer for `draft` in `placement`.
    pub(super) fn open_composer(
        &mut self,
        draft: Draft,
        placement: Placement,
        answers: Option<String>,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Entity<Composer>> {
        // A phone has one window: every composer covers it.
        let placement = if self.mobile.on {
            Placement::Maximized
        } else {
            placement
        };
        let answers = answers.filter(|_| placement == Placement::Inline);
        let accounts = self.accounts.clone();
        let aliases = fwd::sendable(&self.forwarding);
        let contacts = self.contacts();
        let options = self.compose_options.clone();
        let colors = self.colors();
        let backend = self.backend.clone();
        let composer = if placement == Placement::Window {
            let made: Rc<std::cell::RefCell<Option<Entity<Composer>>>> = Rc::default();
            let out = made.clone();
            let bounds = Bounds::centered(None, size(px(680.), px(640.)), cx);
            let handle = cx
                .open_window(
                    window::detached_options(
                        SharedString::from(if draft.subject.is_empty() {
                            "New Message".to_string()
                        } else {
                            draft.subject.clone()
                        }),
                        bounds,
                    ),
                    move |window, cx| {
                        let composer = cx.new(|cx| {
                            Composer::new(
                                draft, placement, accounts, aliases, contacts, options, colors,
                                backend, window, cx,
                            )
                        });
                        composer.update(cx, |c, cx| {
                            c.window = Some(window.window_handle());
                            c.focus_first(window, cx);
                        });
                        *out.borrow_mut() = Some(composer.clone());
                        let view = cx.new(|_| ComposeWindow { composer });
                        cx.new(|cx| Root::new(view, window, cx))
                    },
                )
                .ok()?;
            let composer = made.borrow_mut().take()?;
            let app = cx.weak_entity();
            let watched = composer.clone();
            let _ = handle.update(cx, |_, window, cx| {
                window.on_window_should_close(cx, move |_, cx| {
                    watched.update(cx, |c, cx| c.save_now(cx));
                    let gone = watched.clone();
                    let _ = app.update(cx, |this, cx| {
                        this.forget_composer(&gone);
                        this.refresh_drafts(cx);
                    });
                    true
                });
            });
            composer
        } else {
            let composer = cx.new(|cx| {
                Composer::new(
                    draft, placement, accounts, aliases, contacts, options, colors, backend, w, cx,
                )
            });
            let mobile = self.mobile.on;
            composer.update(cx, |c, cx| {
                c.answers = answers.clone();
                c.mobile = mobile;
                c.focus_first(w, cx);
            });
            composer
        };
        let services = Self::alias_services(&self.forwarding);
        let alias_only = self.alias_only_accounts();
        let when = self.when.clone();
        composer.update(cx, |c, _| {
            c.when = when;
            c.set_forwarding(fwd::sendable(&self.forwarding), services, alias_only)
        });
        // The From menu should not offer aliases deleted elsewhere.
        self.refresh_aliases_due(super::views::ALIAS_REFRESH_SOON, cx);
        if let Some(message) = answers {
            self.inline_replies.insert(message, composer.entity_id());
            self.scroll_reader_to_end(w);
        }
        let subscription = cx.subscribe_in(&composer, w, |this, composer, event, w, cx| {
            this.on_composer_event(composer.clone(), event, w, cx)
        });
        self.composer_subscriptions
            .insert(composer.entity_id(), subscription);
        self.composers.push(composer.clone());
        cx.notify();
        Some(composer)
    }

    fn on_composer_event(
        &mut self,
        composer: Entity<Composer>,
        event: &ComposerEvent,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            ComposerEvent::Send => self.queue_send(composer, cx),
            ComposerEvent::Close => {
                composer.update(cx, |c, cx| c.save_now(cx));
                let account = composer.read(cx).draft.account.clone();
                self.close_composer(&composer, cx);
                self.refresh_drafts(cx);
                self.sync_drafts_soon(&account, cx);
            }
            ComposerEvent::Discard => {
                let (id, account) = {
                    let d = &composer.read(cx).draft;
                    (d.id.clone(), d.account.clone())
                };
                let _ = self.backend.discard_draft(&id);
                self.close_composer(&composer, cx);
                self.refresh_drafts(cx);
                self.sync_drafts_soon(&account, cx);
            }
            ComposerEvent::Place(placement) => self.place_composer(composer, *placement, w, cx),
            ComposerEvent::Saved => self.refresh_drafts(cx),
            ComposerEvent::NewAlias(service, mailbox) => {
                self.handle(Action::NewAliasFor(service.clone(), mailbox.clone()), w, cx);
            }
            ComposerEvent::Schedule(at) => self.schedule_send(composer, *at, cx),
            ComposerEvent::SaveTemplate(t) => self.save_template(t.clone(), cx),
        }
        cx.notify();
    }

    pub(super) fn save_compose_options(&mut self, options: ComposeOptions, cx: &mut Context<Self>) {
        let before = self.compose_options.clone();
        self.save_preference(
            crate::manage::composing::PREFERENCE,
            &serde_json::to_string(&options).unwrap(),
            cx,
            move |this, cx| this.use_compose_options(before, cx),
        );
        self.use_compose_options(options, cx);
        cx.notify();
    }

    fn use_compose_options(&mut self, options: ComposeOptions, cx: &mut Context<Self>) {
        for composer in &self.composers {
            composer.update(cx, |c, cx| {
                c.options = options.clone();
                cx.notify();
            });
        }
        self.compose_options = options;
    }

    pub(super) fn refresh_drafts(&mut self, cx: &mut Context<Self>) {
        if self.folder == Folder::Drafts {
            self.load_drafts(cx);
        }
    }
    /// Brings an account's drafts and its server's Drafts folder in step,
    /// in the background; the Drafts list reloads if anything changed.
    pub(super) fn sync_drafts_soon(&mut self, account: &str, cx: &mut Context<Self>) {
        let Some(a) = self.accounts.iter().find(|a| a.id == account).cloned() else {
            return;
        };
        if a.provider == Provider::Pop3
            || self.account_blocked(&a.id)
            || !self.draft_syncs.insert(a.id.clone())
        {
            return;
        }
        self.draft_synced
            .insert(a.id.clone(), std::time::Instant::now());
        let id = a.id.clone();
        self.task(
            cx,
            move |b| b.sync_drafts(&a),
            move |this, result, cx| {
                this.draft_syncs.remove(&id);
                this.account_result(&id, IssueSource::Sync(Folder::Drafts.key().into()), &result);
                if result.is_ok() {
                    this.refresh_drafts(cx);
                }
            },
        );
    }

    /// Moves a composer. Within the main window it only changes how it is
    /// drawn; to or from a window of its own, it is reopened there with the
    /// same draft.
    fn place_composer(
        &mut self,
        composer: Entity<Composer>,
        placement: Placement,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let from = composer.read(cx).placement;
        // A phone keeps composers full screen.
        if self.mobile.on && placement != Placement::Maximized {
            return;
        }
        let windowed = |p: Placement| p == Placement::Window;
        if !windowed(from) && !windowed(placement) {
            self.inline_replies
                .retain(|_, id| *id != composer.entity_id());
            composer.update(cx, |c, cx| {
                c.placement = placement;
                c.answers = None;
                c.minimized = false;
                c.origin = None;
                c.focus_first(w, cx);
                cx.notify();
            });
            return;
        }
        let draft = composer.update(cx, |c, cx| c.snapshot(cx));
        self.close_composer(&composer, cx);
        self.open_composer(draft, placement, None, w, cx);
    }

    /// Removes a composer from view, closing its window if it has one.
    pub(super) fn close_composer(&mut self, composer: &Entity<Composer>, cx: &mut App) {
        let window = composer.read(cx).window;
        self.forget_composer(composer);
        if let Some(window) = window {
            let _ = window.update(cx, |_, w, _| w.remove_window());
        }
    }

    fn forget_composer(&mut self, composer: &Entity<Composer>) {
        let id = composer.entity_id();
        self.composers.retain(|c| c.entity_id() != id);
        self.inline_replies.retain(|_, c| *c != id);
        self.composer_subscriptions.remove(&id);
    }

    /// Closes any composer editing a draft that was deleted.
    pub(super) fn drop_composer_for_draft(&mut self, draft: &str, cx: &mut App) {
        let open: Vec<_> = self
            .composers
            .iter()
            .filter(|c| c.read(cx).draft.id == draft)
            .cloned()
            .collect();
        for composer in open {
            self.close_composer(&composer, cx);
        }
    }

    /// Saves every open draft now, on this thread: before quitting.
    pub(super) fn persist_draft(&mut self, cx: &mut App) {
        for composer in self.composers.clone() {
            composer.update(cx, |c, cx| c.save_now(cx));
        }
    }

    /// People to suggest as recipients: senders of loaded mail, most recent
    /// first, one entry per address.
    /// People to suggest in a composer's address fields: everyone in the
    /// listed mail, newest first, up to 2,000, not counting your own
    /// addresses. Kept until the list or your addresses change.
    fn contacts(&self) -> Rc<Vec<(String, String)>> {
        let mut own: Vec<String> = self
            .accounts
            .iter()
            .flat_map(|a| {
                std::iter::once(a.email.to_lowercase())
                    .chain(a.identities.iter().map(|i| i.email.to_lowercase()))
            })
            .collect();
        own.sort();
        if let Some((revision, addresses, contacts)) = &*self.contacts_cache.borrow()
            && *revision == self.list_revision
            && *addresses == own
        {
            return contacts.clone();
        }
        // The address book first, then people seen in mail.
        let mut people =
            crate::manage::backend::contacts::people(&self.backend.contacts());
        let mut seen: HashSet<String> = people.iter().map(|(_, e)| e.to_lowercase()).collect();
        seen.extend(own.iter().cloned());
        people.retain(|(_, e)| !own.contains(&e.to_lowercase()));
        people.extend(
            gather_contacts(&self.messages, &own)
                .into_iter()
                .filter(|(_, e)| seen.insert(e.to_lowercase())),
        );
        let contacts = Rc::new(people);
        *self.contacts_cache.borrow_mut() = Some((self.list_revision, own, contacts.clone()));
        contacts
    }

    /// Brings the end of the reader, where its inline reply is, into view.
    /// The reader asks for it while it is drawn with the reply in it, so
    /// the scroll uses the layout the reply is part of; a few times over,
    /// as the reply sizes itself once it is shown. (Waiting for frames
    /// instead could scroll before the reply was laid out, leaving it
    /// below a long email, out of sight.)
    fn scroll_reader_to_end(&self, w: &mut Window) {
        self.reader_to_end.set(3);
        w.refresh();
    }

    // Sending.

    /// Takes a checked draft from its composer and sends it after the undo
    /// delay. The composer is hidden meanwhile and comes back on Undo or on
    /// failure, with the draft kept.
    fn queue_send(&mut self, composer: Entity<Composer>, cx: &mut Context<Self>) {
        let placement = composer.read(cx).placement;
        composer.update(cx, |c, cx| {
            c.save_now(cx);
            c.sending = true;
            c.error = None;
            cx.notify();
        });
        let window = composer.read(cx).window;
        self.forget_composer(&composer);
        if let Some(window) = window {
            composer.update(cx, |c, _| c.window = None);
            let _ = window.update(cx, |_, w, _| w.remove_window());
        }
        self.outbox_generation += 1;
        let id = self.outbox_generation;
        self.outbox.push(Outgoing {
            id,
            composer,
            placement,
            delivering: false,
        });
        let delay = Duration::from_secs(u64::from(self.compose_options.undo_send_seconds));
        let timer = cx.background_executor().timer(delay);
        cx.spawn(async move |this, cx| {
            timer.await;
            let _ = this.update(cx, |this, cx| this.deliver(id, cx));
        })
        .detach();
        cx.notify();
    }

    fn deliver(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(out) = self.outbox.iter_mut().find(|o| o.id == id) else {
            return;
        };
        if out.delivering {
            return;
        }
        out.delivering = true;
        let composer = out.composer.clone();
        let draft = composer.update(cx, |c, cx| c.snapshot(cx));
        let Some(a) = self
            .accounts
            .iter()
            .find(|a| a.id == draft.account)
            .or(self.accounts.first())
            .cloned()
        else {
            self.send_failed(
                id,
                "Connect an account before sending. Your draft is saved.".into(),
                cx,
            );
            return;
        };
        let draft = Draft {
            account: a.id.clone(),
            ..draft
        };
        self.sending = true;
        let account_id = a.id.clone();
        self.task(
            cx,
            move |b| b.send_noting(&a, &draft),
            move |this, result, cx| {
                let gone = result
                    .as_ref()
                    .err()
                    .and_then(|e| e.downcast_ref::<fwd::AliasGone>())
                    .cloned();
                // A deleted alias is not a problem with the mailbox.
                if gone.is_none() {
                    this.account_result(&account_id, IssueSource::Send, &result);
                }
                match (result, gone) {
                    (Ok(notes), _) => {
                        // Sent as an alias: its routed addresses are known now.
                        this.alias_routes = fwd::routes(&this.backend.store);
                        this.reverse_contacts =
                            Arc::new(fwd::reverse_contacts(&this.backend.store));
                        this.outbox.retain(|o| o.id != id);
                        if notes.is_empty() {
                            this.notice("Message sent", false, cx);
                        } else {
                            this.notice(format!("Message sent. {}", notes.join(" ")), false, cx);
                        }
                        this.refresh_drafts(cx);
                        this.sync_drafts_soon(&account_id, cx);
                    }
                    (Err(_), Some(gone)) => this.alias_gone(id, gone, cx),
                    (Err(e), None) => this.send_failed(id, format!("{e:#}"), cx),
                }
                this.sending = this.outbox.iter().any(|o| o.delivering);
            },
        );
        cx.notify();
    }

    /// Puts a checked draft aside to be sent at `at` (Send Later): it is
    /// saved with the time and waits in Drafts.
    fn schedule_send(&mut self, composer: Entity<Composer>, at: i64, cx: &mut Context<Self>) {
        composer.update(cx, |c, cx| {
            c.draft.send_at = Some(at);
            c.draft.send_error.clear();
            c.save_now(cx);
        });
        let account = composer.read(cx).draft.account.clone();
        let id = composer.read(cx).draft.id.clone();
        self.scheduled_retry.remove(&id);
        self.close_composer(&composer, cx);
        let when = crate::manage::composing::describe_later(at, chrono::Local::now());
        self.notice(format!("Scheduled to send {when}."), false, cx);
        self.refresh_drafts(cx);
        self.sync_drafts_soon(&account, cx);
        // Handed to a server that holds it until then, where the account
        // lets it; otherwise it stays scheduled here.
        if let Some(a) = self
            .accounts
            .iter()
            .find(|a| a.id == account && a.server_send_later)
            .cloned()
        {
            self.scheduled_sending.insert(id.clone());
            let handed = id.clone();
            self.task(
                cx,
                move |b| b.hand_to_server(&a, &handed),
                move |this, result, cx| {
                    match result {
                        Ok(true) => this.notice(
                            format!("Handed to the server, which sends it {when}, even with Picacho closed."),
                            false,
                            cx,
                        ),
                        Ok(false) => {}
                        Err(e) => this.notice(
                            format!("The server couldn't take it ({e:#}); Picacho sends it {when}."),
                            true,
                            cx,
                        ),
                    }
                    this.scheduled_sending.remove(&id);
                    this.refresh_drafts(cx);
                },
            );
        }
    }

    /// Sends the scheduled drafts whose time has come, except any open in
    /// a composer (being changed) or already going. With no window open the
    /// background monitor sends them (`manage::background`); ones whose time
    /// passed while Picacho wasn't running go at the next check after it
    /// starts.
    pub(super) fn send_due(&mut self, cx: &mut Context<Self>) {
        let mut open: HashSet<String> = self
            .composers
            .iter()
            .map(|c| c.read(cx).draft.id.clone())
            .collect();
        open.extend(
            self.outbox
                .iter()
                .map(|o| o.composer.read(cx).draft.id.clone()),
        );
        self.task(
            cx,
            |b| b.store.scheduled_drafts(),
            move |this, result, cx| {
                let Ok(drafts) = result else {
                    return;
                };
                let now = chrono::Utc::now().timestamp();
                let moment = std::time::Instant::now();
                // Retry times of drafts no longer scheduled (sent, deleted,
                // unscheduled) aren't kept.
                this.scheduled_retry
                    .retain(|id, _| drafts.iter().any(|d| &d.id == id));
                for d in drafts {
                    let due = d.send_at.is_some_and(|t| t <= now) && d.send_error.is_empty();
                    let waiting = this.scheduled_retry.get(&d.id).is_some_and(|t| *t > moment);
                    if due
                        && !waiting
                        && !open.contains(&d.id)
                        && !this.scheduled_sending.contains(&d.id)
                    {
                        this.send_scheduled(d, cx);
                    }
                }
            },
        );
    }

    fn send_scheduled(&mut self, d: Draft, cx: &mut Context<Self>) {
        let Some(a) = self.accounts.iter().find(|a| a.id == d.account).cloned() else {
            self.schedule_failed(d.id, "Its account was removed.".into(), cx);
            return;
        };
        if self.account_blocked(&a.id) {
            return;
        }
        let id = d.id.clone();
        let account_id = a.id.clone();
        self.scheduled_sending.insert(id.clone());
        self.task(
            cx,
            move |b| b.send_scheduled(&a, &d.id),
            move |this, result, cx| {
                this.scheduled_sending.remove(&id);
                match result {
                    // Sent already, or being sent, by the background
                    // monitor; or no longer due.
                    Ok(None) => this.refresh_drafts(cx),
                    Ok(Some((_, notes))) => {
                        this.scheduled_retry.remove(&id);
                        this.notice(
                            format!("Scheduled message sent. {}", notes.join(" "))
                                .trim()
                                .to_owned(),
                            false,
                            cx,
                        );
                        this.alias_routes = fwd::routes(&this.backend.store);
                        this.refresh_drafts(cx);
                        this.sync_drafts_soon(&account_id, cx);
                    }
                    // Offline, or the server's away: nothing went; try again
                    // in a few minutes.
                    Err(e) if crate::sync::retry::later(&e) => {
                        this.scheduled_retry
                            .insert(id, std::time::Instant::now() + Duration::from_secs(5 * 60));
                    }
                    Err(e) => this.schedule_failed(id, format!("{e:#}"), cx),
                }
            },
        );
    }

    /// A scheduled draft couldn't be sent: it stays in Drafts saying why,
    /// for the person to fix and send.
    fn schedule_failed(&mut self, id: String, error: String, cx: &mut Context<Self>) {
        self.scheduled_retry.remove(&id);
        self.backend.schedule_failed(&id, &error);
        self.notice(
            format!("A scheduled message wasn't sent: {error} It's in Drafts."),
            true,
            cx,
        );
        self.refresh_drafts(cx);
    }

    /// The alias a message was to be sent as no longer exists: it is
    /// forgotten here, From goes back to the mailbox, and a dialog says so.
    fn alias_gone(&mut self, id: u64, gone: fwd::AliasGone, cx: &mut Context<Self>) {
        let Some(out) = self.outbox.iter().find(|o| o.id == id) else {
            return;
        };
        let composer = out.composer.clone();
        let account = composer.read(cx).draft.account.clone();
        let from = self
            .accounts
            .iter()
            .find(|a| a.id == account)
            .map(|a| a.email.clone())
            .unwrap_or_default();
        composer.update(cx, |c, cx| c.reset_sender(cx));
        let mut changed = false;
        for f in &mut self.forwarding {
            let before = f.aliases.len();
            f.aliases.retain(|a| !a.email.eq_ignore_ascii_case(&gone.email));
            changed |= f.aliases.len() != before;
        }
        if changed {
            self.views_changed(cx);
        }
        self.send_failed(
            id,
            format!("{gone} From was reset to {from}."),
            cx,
        );
        // It was out of the list of open composers while sending.
        self.update_composer_aliases(cx);
        self.modal = Some(Modal::AliasGone(gone.to_string(), from));
        self.anim += 1;
        // Others may have gone too.
        self.refresh_aliases_due(Duration::ZERO, cx);
    }

    fn send_failed(&mut self, id: u64, error: String, cx: &mut Context<Self>) {
        let Some(i) = self.outbox.iter().position(|o| o.id == id) else {
            return;
        };
        let out = self.outbox.remove(i);
        out.composer.update(cx, |c, cx| {
            c.sending = false;
            c.error = Some(error);
            cx.notify();
        });
        self.restore_composer(out, cx);
    }

    /// Puts a message whose undo delay is still running back in its composer.
    pub(super) fn undo_send(&mut self, id: u64, w: &mut Window, cx: &mut Context<Self>) {
        let Some(i) = self.outbox.iter().position(|o| o.id == id && !o.delivering) else {
            return;
        };
        let out = self.outbox.remove(i);
        out.composer.update(cx, |c, cx| {
            c.sending = false;
            c.focus_first(w, cx);
            cx.notify();
        });
        self.restore_composer(out, cx);
    }

    fn restore_composer(&mut self, out: Outgoing, cx: &mut Context<Self>) {
        let composer = out.composer;
        // A composer from a closed window comes back floating.
        let placement = match out.placement {
            Placement::Window => Placement::Floating,
            p => p,
        };
        let answers = composer.read(cx).answers.clone();
        composer.update(cx, |c, _| {
            c.placement = placement;
            c.minimized = false;
        });
        if let Some(message) = answers.filter(|_| placement == Placement::Inline) {
            self.inline_replies.insert(message, composer.entity_id());
        }
        let subscription = cx.subscribe(&composer, |this, composer, event, cx| {
            match event {
                ComposerEvent::Send => this.queue_send(composer, cx),
                ComposerEvent::Close => {
                    composer.update(cx, |c, cx| c.save_now(cx));
                    let account = composer.read(cx).draft.account.clone();
                    this.close_composer(&composer, cx);
                    this.refresh_drafts(cx);
                    this.sync_drafts_soon(&account, cx);
                }
                ComposerEvent::Discard => {
                    let (id, account) = {
                        let d = &composer.read(cx).draft;
                        (d.id.clone(), d.account.clone())
                    };
                    let _ = this.backend.discard_draft(&id);
                    this.close_composer(&composer, cx);
                    this.refresh_drafts(cx);
                    this.sync_drafts_soon(&account, cx);
                }
                // Moving needs the window; the next render applies it.
                ComposerEvent::Place(p) => {
                    this.pending_place = Some((composer.entity_id(), *p));
                }
                ComposerEvent::Saved => this.refresh_drafts(cx),
                // Opening Settings needs the window; the next render does it.
                ComposerEvent::NewAlias(service, mailbox) => {
                    this.pending_new_alias = Some((service.clone(), mailbox.clone()));
                }
                ComposerEvent::Schedule(at) => this.schedule_send(composer, *at, cx),
                ComposerEvent::SaveTemplate(t) => this.save_template(t.clone(), cx),
            }
            cx.notify();
        });
        self.composer_subscriptions
            .insert(composer.entity_id(), subscription);
        self.composers.push(composer);
        cx.notify();
    }

    /// Applies a placement change asked for by a composer that was restored
    /// after Undo (its subscription has no window to reopen it with).
    pub(super) fn apply_pending_place(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        let Some((id, placement)) = self.pending_place.take() else {
            return;
        };
        if let Some(composer) = self.composers.iter().find(|c| c.entity_id() == id).cloned() {
            self.place_composer(composer, placement, w, cx);
        }
    }

    // Drawing.

    /// Floating, minimized and maximized composers over the main window.
    pub(super) fn composer_layer(&self, window: &Window, cx: &Context<Self>) -> Vec<AnyElement> {
        let c = self.colors();
        // Composers are placed inside the window's frame, which is smaller
        // than the window while Picacho draws its own decorations.
        let frame = super::window::content_size(window);
        let (vw, vh) = (f32::from(frame.width), f32::from(frame.height));
        let mut right = 16.;
        let mut out = vec![];
        for composer in &self.composers {
            let state = composer.read(cx);
            let (w, h, x, y) = match state.placement {
                Placement::Inline | Placement::Window => continue,
                Placement::Maximized if !state.minimized => {
                    let inset = 20.;
                    out.push(
                        div()
                            .absolute()
                            .inset_0()
                            .top(px(TITLEBAR_HEIGHT))
                            .bg(rgba(0x101c2c55))
                            .occlude()
                            .on_mouse_down(MouseButton::Left, swallow_press)
                            .flex()
                            .p(px(inset))
                            .child(
                                div()
                                    .id(("composer-max", composer.entity_id().as_u64()))
                                    .debug_selector(|| "compose-panel".into())
                                    .size_full()
                                    .rounded(px(14.))
                                    .border_1()
                                    .border_color(rgb(c.border))
                                    .shadow_2xl()
                                    .overflow_hidden()
                                    .child(composer.clone()),
                            )
                            .into_any_element(),
                    );
                    continue;
                }
                _ if state.minimized => {
                    let w = BAR_W.min(vw - 32.);
                    let x = vw - right - w;
                    right += w + GAP;
                    (w, BAR_H, x, vh - BAR_H)
                }
                _ => {
                    let w = FLOAT_W.min(vw - 32.);
                    let h = FLOAT_H.min(vh - TITLEBAR_HEIGHT - 24.).max(BAR_H);
                    let (x, y) = match state.origin {
                        Some(o) => clamp_origin(o, w, h, vw, vh),
                        None => {
                            let x = (vw - right - w).max(16.);
                            right += w + GAP;
                            (x, vh - h - 12.)
                        }
                    };
                    (w, h, x, y)
                }
            };
            let minimized = state.minimized;
            out.push(
                div()
                    .id(("composer-float", composer.entity_id().as_u64()))
                    .debug_selector(|| "compose-panel".into())
                    .absolute()
                    .left(px(x))
                    .top(px(y))
                    .w(px(w))
                    .h(px(h))
                    .occlude()
                    .on_mouse_down(MouseButton::Left, swallow_press)
                    .when(minimized, |d| d.rounded_t(px(10.)))
                    .when(!minimized, |d| d.rounded(px(14.)))
                    .border_1()
                    .border_color(rgb(c.border))
                    .shadow_2xl()
                    .overflow_hidden()
                    .child(composer.clone())
                    .into_any_element(),
            );
        }
        if let Some(out_toast) = self.outbox_toast(cx) {
            out.push(out_toast);
        }
        out
    }

    /// "Sending…" with Undo while a message waits out its delay.
    pub(super) fn outbox_toast(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let c = self.colors();
        let last = self.outbox.last()?;
        let (id, waiting) = (last.id, !last.delivering);
        let count = self.outbox.len();
        Some(
            div()
                .id("outbox-toast")
                .debug_selector(|| "outbox-toast".into())
                .absolute()
                .bottom_5()
                .left_5()
                .h(px(44.))
                .pl_4()
                .pr_2()
                .flex()
                .items_center()
                .gap_3()
                .rounded_lg()
                .bg(rgb(c.text))
                .text_color(rgb(c.bg))
                .shadow_lg()
                .text_size(px(13.))
                .child(spinner(14., c.bg))
                .child(if count > 1 {
                    format!("Sending {count} messages…")
                } else {
                    "Sending…".to_string()
                })
                .when(waiting, |d| {
                    d.child(
                        div()
                            .id("undo-send")
                            .debug_selector(|| "undo-send".into())
                            .px_3()
                            .h(px(30.))
                            .flex()
                            .items_center()
                            .rounded(px(7.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(c.soft))
                            .cursor_pointer()
                            .hover(|s| s.opacity(0.8))
                            .child("Undo")
                            .on_click(cx.listener(move |this, _, w, cx| {
                                this.handle(Action::UndoSend(id), w, cx)
                            })),
                    )
                })
                .into_any_element(),
        )
    }

    /// Follows the pointer while a floating composer's header is dragged.
    pub(super) fn drag_composer(
        &mut self,
        e: &DragMoveEvent<ComposerDrag>,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let id = e.drag(cx).0;
        let Some(composer) = self.composers.iter().find(|c| c.entity_id() == id).cloned() else {
            return;
        };
        // The pointer is in window coordinates; composers are placed in the
        // frame's, which start past the shadow band and borders of a window
        // drawing its own decorations.
        let frame = super::window::content_bounds(w);
        let (vw, vh) = (f32::from(frame.size.width), f32::from(frame.size.height));
        let w_ = FLOAT_W.min(vw - 32.);
        let h_ = FLOAT_H.min(vh - TITLEBAR_HEIGHT - 24.).max(BAR_H);
        composer.update(cx, |c, cx| {
            let p = e.event.position - c.grab - frame.origin;
            let (x, y) = clamp_origin(p, w_, h_, vw, vh);
            c.origin = Some(point(px(x), px(y)));
            cx.notify();
        });
        cx.notify();
    }
}

fn clamp_origin(o: Point<Pixels>, w: f32, h: f32, vw: f32, vh: f32) -> (f32, f32) {
    let x = f32::from(o.x).clamp(0., (vw - w).max(0.));
    let y = f32::from(o.y).clamp(TITLEBAR_HEIGHT, (vh - BAR_H).max(TITLEBAR_HEIGHT));
    let _ = h;
    (x, y)
}

/// "Re: …" or "Fwd: …", without stacking prefixes.
pub(super) fn reply_subject(subject: &str, forward: bool) -> String {
    let lower = subject.trim_start().to_lowercase();
    if forward {
        if lower.starts_with("fwd:") || lower.starts_with("fw:") {
            subject.to_string()
        } else {
            format!("Fwd: {subject}")
        }
    } else if lower.starts_with("re:") {
        subject.to_string()
    } else {
        format!("Re: {subject}")
    }
}

/// To and Cc for a reply: the sender (or where they asked replies to go);
/// for Reply All also everyone else on To and Cc, never yourself. `own`
/// holds your addresses: the account's first, then the alias it was sent
/// to. Only the account's marks mail as your own sent message: addy.io
/// forwards mail as if it came from the alias.
pub(super) fn reply_recipients(m: &Mail, all: bool, own: &[&str]) -> (String, String) {
    let own: Vec<&str> = own.iter().copied().filter(|o| !o.is_empty()).collect();
    let mut seen: HashSet<String> = own.iter().map(|o| o.to_lowercase()).collect();
    let format = |name: Option<String>, email: &str| match name {
        Some(n) if !n.is_empty() && n != email => {
            if n.contains([',', '"']) {
                format!("\"{}\" <{email}>", n.replace('"', ""))
            } else {
                format!("{n} <{email}>")
            }
        }
        _ => email.to_string(),
    };
    let mut to = vec![];
    // Replying to your own sent message answers its recipients.
    if own.first().is_some_and(|o| m.from.eq_ignore_ascii_case(o)) {
        for (name, email) in crate::sync::smtp::recipients(&m.to).unwrap_or_default() {
            if seen.insert(email.to_lowercase()) {
                to.push(format(name, &email));
            }
        }
    } else {
        // Alias services and mailing lists name their reply address in
        // Reply-To; the sender is then left out, as mail clients do.
        let reply_to = crate::sync::smtp::recipients(&m.reply_to).unwrap_or_default();
        if !reply_to.is_empty() {
            seen.insert(m.from.to_lowercase());
            for (name, email) in reply_to {
                if seen.insert(email.to_lowercase()) {
                    to.push(format(name, &email));
                }
            }
        } else if !m.from.is_empty() {
            seen.insert(m.from.to_lowercase());
            to.push(format(Some(m.sender.clone()), &m.from));
        }
    }
    let mut cc = vec![];
    if all {
        for list in [m.to.as_str(), m.cc()] {
            for (name, email) in crate::sync::smtp::recipients(list).unwrap_or_default() {
                if seen.insert(email.to_lowercase()) {
                    cc.push(format(name, &email));
                }
            }
        }
    }
    (to.join(", "), cc.join(", "))
}

/// The sendable alias a message was addressed to, if any.
pub(super) fn alias_addressed<'a>(
    m: &Mail,
    aliases: &'a [fwd::SendAlias],
) -> Option<&'a fwd::SendAlias> {
    let addressed: Vec<String> = [m.to.as_str(), m.cc()]
        .into_iter()
        .flat_map(|list| crate::sync::smtp::recipients(list).unwrap_or_default())
        .map(|(_, email)| email.to_lowercase())
        .collect();
    aliases.iter().find(|a| addressed.contains(&a.email))
}

/// The account's other address a message was sent to (To or Cc), if any.
pub(super) fn identity_addressed<'a>(
    m: &Mail,
    a: &'a Account,
) -> Option<&'a crate::manage::model::Identity> {
    if a.identities.is_empty() {
        return None;
    }
    let addressed: Vec<String> = [m.to.as_str(), m.cc()]
        .into_iter()
        .flat_map(|list| crate::sync::smtp::recipients(list).unwrap_or_default())
        .map(|(_, email)| email.to_lowercase())
        .collect();
    a.identities
        .iter()
        .find(|i| addressed.contains(&i.email.to_lowercase()))
}

/// Everyone in `messages`, newest first, up to 2,000, but those in `own`
/// (lowercase). Recipient lists already read are skipped: mail from lists
/// is sent to the same few addresses over and over, and parsing every
/// list of tens of thousands of messages took most of the time.
pub(super) fn gather_contacts(messages: &[Mail], own: &[String]) -> Vec<(String, String)> {
    let own: HashSet<&str> = own.iter().map(String::as_str).collect();
    let mut seen = HashSet::new();
    let mut lists_read: HashSet<&str> = HashSet::new();
    let mut out = vec![];
    let mut by_date: Vec<&Mail> = messages.iter().filter(|m| !m.pending).collect();
    by_date.sort_by_key(|m| std::cmp::Reverse(m.date));
    for m in by_date {
        let mut people = vec![(m.sender.clone(), m.from.clone())];
        for list in [m.to.as_str(), m.cc()] {
            if !lists_read.insert(list) {
                continue;
            }
            if let Ok(parsed) = crate::sync::smtp::recipients(list) {
                people.extend(
                    parsed
                        .into_iter()
                        .map(|(name, email)| (name.unwrap_or_else(|| email.clone()), email)),
                );
            }
        }
        for (name, email) in people {
            let key = email.to_lowercase();
            if email.contains('@') && !own.contains(key.as_str()) && seen.insert(key) {
                out.push((name, email));
            }
        }
        if out.len() >= 2000 {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::prelude::v1::test;

    #[test]
    fn subjects_gain_one_prefix() {
        assert_eq!(reply_subject("Plans", false), "Re: Plans");
        assert_eq!(reply_subject("RE: Plans", false), "RE: Plans");
        assert_eq!(reply_subject("Fw: Plans", true), "Fw: Plans");
        assert_eq!(reply_subject("Re: Plans", true), "Fwd: Re: Plans");
    }

    /// Contacts come out as gathering every message's recipients would:
    /// newest first, each once with its first name, never your own.
    #[test]
    fn contacts_skip_lists_already_read_and_come_out_the_same() {
        let own = vec!["me@example.com".to_owned()];
        let mut messages = Vec::new();
        for i in 0..60 {
            let mut m = Mail {
                id: i.to_string(),
                from: format!("sender{}@x.com", i % 7),
                sender: format!("Sender {}", i % 7),
                to: if i % 3 == 0 { "Me <me@example.com>".into() } else { format!("Ana <ana@x.com>, bo{}@x.com", i % 4) },
                date: i as i64 * if i % 2 == 0 { 1 } else { -1 },
                ..Mail::default()
            };
            if i % 5 == 0 {
                m.set_cc("Cy <cy@x.com>, ME@example.com, not an address");
            }
            messages.push(m);
        }
        let old = |messages: &[Mail]| {
            let mut seen = HashSet::new();
            let mut out = vec![];
            let mut by_date: Vec<&Mail> = messages.iter().filter(|m| !m.pending).collect();
            by_date.sort_by_key(|m| std::cmp::Reverse(m.date));
            for m in by_date {
                let mut people = vec![(m.sender.clone(), m.from.clone())];
                for list in [m.to.as_str(), m.cc()] {
                    if let Ok(parsed) = crate::sync::smtp::recipients(list) {
                        people.extend(parsed.into_iter().map(|(n, e)| (n.unwrap_or_else(|| e.clone()), e)));
                    }
                }
                for (name, email) in people {
                    let key = email.to_lowercase();
                    if email.contains('@') && key != "me@example.com" && seen.insert(key) {
                        out.push((name, email));
                    }
                }
            }
            out
        };
        assert_eq!(gather_contacts(&messages, &own), old(&messages));
    }

    #[test]
    fn reply_all_copies_everyone_but_you() {
        let mut m = Mail {
            from: "ana@example.com".into(),
            sender: "Ana".into(),
            to: "me@example.com, Bo <bo@example.com>".into(),
            ..Mail::default()
        };
        m.set_cc("cy@example.com, ana@example.com");
        assert_eq!(
            reply_recipients(&m, false, &["me@example.com"]),
            ("Ana <ana@example.com>".into(), String::new())
        );
        let (to, cc) = reply_recipients(&m, true, &["ME@example.com"]);
        assert_eq!(to, "Ana <ana@example.com>");
        assert_eq!(cc, "Bo <bo@example.com>, cy@example.com");
        // Replying to your own sent message goes to its recipients.
        let sent = Mail {
            from: "me@example.com".into(),
            to: "bo@example.com".into(),
            ..Mail::default()
        };
        assert_eq!(
            reply_recipients(&sent, false, &["me@example.com"]).0,
            "bo@example.com"
        );
    }

    #[test]
    fn replies_go_where_reply_to_asks_and_never_to_your_alias() {
        // How addy.io forwards: from the alias, replies to a routed address.
        let m = Mail {
            from: "shop@me.anonaddy.com".into(),
            sender: "Ana".into(),
            reply_to: "shop+ana=example.org@me.anonaddy.com".into(),
            to: "shop@me.anonaddy.com, bo@example.com".into(),
            ..Mail::default()
        };
        let own = ["me@example.com", "shop@me.anonaddy.com"];
        let (to, cc) = reply_recipients(&m, true, &own);
        assert_eq!(to, "shop+ana=example.org@me.anonaddy.com");
        assert_eq!(cc, "bo@example.com");
        let aliases = [fwd::SendAlias {
            email: "shop@me.anonaddy.com".into(),
            note: String::new(),
            service: fwd::ForwardingService::Addy,
            deliver_to: None,
        }];
        assert_eq!(
            alias_addressed(&m, &aliases).map(|a| a.email.as_str()),
            Some("shop@me.anonaddy.com")
        );
        assert!(alias_addressed(&Mail::default(), &aliases).is_none());
    }
}

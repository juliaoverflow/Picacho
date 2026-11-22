//! Local AI in the app: the Settings page that turns it and its features on
//! (downloading their models) and off (deleting them), hiding the sign-in
//! codes the model finds wherever mail is shown, the date and time model
//! that reads times typed into Snooze and Send Later, the tracking number
//! model that confirms the parcels the reader and Summary show, and the
//! reply model that suggests replies under received mail.
use super::*;
use crate::manage::local_ai::{self, otp, replies};
use std::borrow::Cow;

/// Replies suggested for a message, with the lengths of the subject and
/// text and the history revision they were suggested for.
pub(super) type Suggested = ((usize, usize, u64), Rc<replies::Suggestions>);

/// Codes found in messages, kept per message until what was looked at
/// changes; cleared past this many.
const FOUND_CACHE: usize = 4000;
/// The codes found in a message, with the lengths of the subject and text
/// they were found in.
pub(super) type Found = ((usize, usize), Rc<Vec<String>>);

impl MailApp {
    /// The sign-in codes the model finds in received mail `m`'s subject and
    /// body (or its preview, before the body is downloaded). None while the
    /// feature is off or its model isn't loaded.
    pub(super) fn found_codes(&self, m: &Mail) -> Rc<Vec<String>> {
        let Some(detector) = &self.otp else {
            return Rc::default();
        };
        // Only received mail: not what was written here.
        if matches!(m.folder, Folder::Sent | Folder::Drafts) {
            return Rc::default();
        }
        let body = m.body.as_deref().unwrap_or(&m.snippet);
        let key = (m.subject.len(), body.len());
        if let Some((k, codes)) = self.otp_found.borrow().get(&m.id)
            && *k == key
        {
            return codes.clone();
        }
        let codes = Rc::new(detector.codes(&m.subject, body));
        let mut found = self.otp_found.borrow_mut();
        if found.len() >= FOUND_CACHE {
            found.clear();
        }
        found.insert(m.id.clone(), (key, codes.clone()));
        codes
    }
    /// The codes to hide in `m`: those found, unless they were shown.
    pub(super) fn hidden_codes(&self, m: &Mail) -> Rc<Vec<String>> {
        if self.codes_shown.contains(&m.id) {
            return Rc::default();
        }
        self.found_codes(m)
    }
    /// `text` from `m` (its subject, preview or body) with its codes hidden.
    pub(super) fn hide_codes<'a>(&self, m: &Mail, text: &'a str) -> Cow<'a, str> {
        let codes = self.hidden_codes(m);
        if codes.is_empty() {
            return text.into();
        }
        otp::mask(text, &codes)
    }
    /// `m`'s HTML with its codes hidden in the text.
    pub(super) fn hide_codes_html<'a>(&self, m: &Mail, html: &'a str) -> Cow<'a, str> {
        let codes = self.hidden_codes(m);
        if codes.is_empty() {
            return html.into();
        }
        otp::mask_html(html, &codes)
    }

    /// Above a message whose codes were found: that they are hidden, and a
    /// way to show them (or hide them again).
    pub(super) fn codes_banner(&self, m: &Mail, cx: &Context<Self>) -> Option<AnyElement> {
        if self.found_codes(m).is_empty() {
            return None;
        }
        let c = self.colors();
        let shown = self.codes_shown.contains(&m.id);
        let toggle = self.link(
            "codes-toggle",
            if shown { "Hide Code" } else { "Show Code" },
            Action::ShowCodes(m.id.clone(), !shown),
            cx,
        );
        // Copies without showing it.
        let copy = self.link("codes-copy", "Copy Code", Action::CopyCode(m.id.clone()), cx);
        Some(
            self.banner(
                "codes-banner",
                "lock",
                BannerStyle::Filled,
                if shown {
                    "This message's sign-in code is shown."
                } else {
                    "This message's sign-in code is hidden."
                },
                [
                    toggle.into_any_element(),
                    div().text_color(rgb(c.muted)).child("·").into_any_element(),
                    copy.into_any_element(),
                ],
            )
            .into_any_element(),
        )
    }
    /// Copies a message's sign-in code, as it is typed into a sign-in form.
    pub(super) fn copy_code(&mut self, id: String, cx: &mut Context<Self>) {
        let Some(m) = self
            .messages
            .iter()
            .find(|m| m.id == id)
            .cloned()
            .or_else(|| self.selected_mail().filter(|m| m.id == id))
        else {
            return;
        };
        // The reader's copy has the body; the list's only the preview.
        let m = self
            .backend
            .store
            .message(&id)
            .ok()
            .flatten()
            .filter(|full| full.body.is_some())
            .map(|full| Mail {
                folder: m.folder,
                ..full
            })
            .unwrap_or(m);
        let Some(code) = self.found_codes(&m).first().cloned() else {
            return;
        };
        let copied = otp::copyable(&code);
        cx.write_to_clipboard(ClipboardItem::new_string(copied.clone()));
        // Marked sensitive on Android's own clipboard (see the function).
        #[cfg(target_os = "android")]
        crate::android::copy_sensitive_to_system(&copied);
        #[cfg(not(target_os = "android"))]
        drop(copied);
        self.notice("Code copied", false, cx);
    }
    pub(super) fn show_codes(&mut self, id: String, show: bool, cx: &mut Context<Self>) {
        if show {
            self.codes_shown.insert(id);
        } else {
            self.codes_shown.remove(&id);
        }
        // The HTML view is told the new content when the reader draws.
        cx.notify();
    }

    /// Loads the installed models of the features that are on at launch, or
    /// installs them if they should be there and aren't (deleted outside
    /// Picacho).
    pub(super) fn start_local_ai(&mut self, cx: &mut Context<Self>) {
        self.start_otp(cx);
        self.start_when(cx);
        self.start_parcels(cx);
        self.start_replies(cx);
    }

    /// The replies the model suggests for received mail `m` (from its body,
    /// or its preview before the body is downloaded), worded in its
    /// language. Empty while the feature is off or its model isn't loaded,
    /// for mail written here, from senders who don't read replies, mailing
    /// lists, sign-in codes, receipts and package updates, and where the
    /// model finds the message wants no reply.
    pub(super) fn smart_replies(&self, m: &Mail) -> Rc<replies::Suggestions> {
        let Some(model) = self.reply_model.as_ref().filter(|_| self.suggest_replies) else {
            return Rc::default();
        };
        let kinds = self.mail_kinds.as_deref();
        if matches!(m.folder, Folder::Sent | Folder::Drafts)
            || replies::wants_no_reply(&m.from, &m.list_unsubscribe)
            || crate::manage::mail_kinds::Kinds::code(kinds, m)
            || crate::manage::mail_kinds::Kinds::package(kinds, m)
            || crate::manage::mail_kinds::Kinds::receipt(kinds, m)
        {
            return Rc::default();
        }
        let body = m.body.as_deref().unwrap_or(&m.snippet);
        let key = (m.subject.len(), body.len(), self.reply_history_rev);
        if let Some((k, found)) = self.replies_found.borrow().get(&m.id)
            && *k == key
        {
            return found.clone();
        }
        let found = Rc::new(model.suggest(
            &m.subject,
            body,
            &self.reply_history,
            replies::fallback_language(),
            replies::MAX_SUGGESTIONS,
        ));
        let mut cache = self.replies_found.borrow_mut();
        if cache.len() >= FOUND_CACHE {
            cache.clear();
        }
        cache.insert(m.id.clone(), (key, found.clone()));
        found
    }

    /// Answers the open message with its suggested reply `intent`, and
    /// remembers it was chosen.
    pub(super) fn smart_reply(&mut self, intent: String, w: &mut Window, cx: &mut Context<Self>) {
        let Some(m) = self.selected_mail() else {
            return;
        };
        let found = self.smart_replies(&m);
        let Some(s) = found.replies.iter().find(|s| s.intent == intent) else {
            return;
        };
        let text = s.text.to_owned();
        self.reply_history.chose(&intent);
        self.save_reply_history();
        self.reply_with_text(text, w, cx);
    }

    /// Stops suggesting reply `intent`.
    pub(super) fn dismiss_smart_reply(&mut self, intent: String, cx: &mut Context<Self>) {
        self.reply_history.dismiss(&intent);
        self.save_reply_history();
        cx.notify();
    }

    /// Forgets which suggestions were chosen and turned away.
    pub(super) fn forget_reply_history(&mut self, cx: &mut Context<Self>) {
        self.reply_history = replies::History::default();
        self.save_reply_history();
        self.notice(
            "Picacho forgot which replies you chose and turned away",
            false,
            cx,
        );
        cx.notify();
    }

    fn save_reply_history(&mut self) {
        self.reply_history_rev += 1;
        if let Ok(json) = serde_json::to_string(&self.reply_history) {
            self.backend
                .store
                .set_preference_soon(local_ai::REPLY_HISTORY_PREFERENCE, &json);
        }
    }

    fn start_replies(&mut self, cx: &mut Context<Self>) {
        if !self.suggest_replies {
            return;
        }
        let _ = local_ai::remove_stale(&self.backend.store);
        if local_ai::REPLIES.is_installed(&self.backend.store) {
            self.load_replies(cx);
        } else {
            self.install_replies(cx);
        }
    }
    fn load_replies(&mut self, cx: &mut Context<Self>) {
        match local_ai::reply_suggester(&self.backend.store) {
            Ok(model) => {
                self.reply_model = Some(Rc::new(model));
                self.replies_error = None;
            }
            Err(e) => {
                self.reply_model = None;
                self.replies_error = Some(format!("{e:#}"));
            }
        }
        self.replies_found.borrow_mut().clear();
        cx.notify();
    }
    fn install_replies(&mut self, cx: &mut Context<Self>) {
        if self.replies_busy {
            return;
        }
        self.replies_busy = true;
        self.replies_error = None;
        self.replies_progress.store(0, Ordering::Relaxed);
        let received = self.replies_progress.clone();
        cx.notify();
        self.task(
            cx,
            move |b| local_ai::REPLIES.install(&b.store, &received),
            |this, result, cx| {
                this.replies_busy = false;
                match result {
                    // Turned off while installing: don't keep it.
                    Ok(()) if !this.suggest_replies => {
                        let _ = local_ai::REPLIES.remove(&this.backend.store);
                    }
                    Ok(()) => this.load_replies(cx),
                    Err(e) => {
                        this.replies_error = Some(format!("{e:#}"));
                        this.set_suggest_replies(false, cx);
                    }
                }
                cx.notify();
            },
        );
        self.follow_download(cx);
    }
    pub(super) fn set_suggest_replies(&mut self, on: bool, cx: &mut Context<Self>) {
        let on = on && self.local_ai;
        self.suggest_replies = on;
        self.backend.store.set_preference_soon(
            local_ai::REPLIES_PREFERENCE,
            if on { "true" } else { "false" },
        );
        if on {
            self.start_replies(cx);
        } else {
            self.reply_model = None;
            self.replies_found.borrow_mut().clear();
            if let Err(e) = local_ai::REPLIES.remove(&self.backend.store) {
                self.notice(format!("{e:#}"), true, cx);
            }
        }
        cx.notify();
    }
    fn start_otp(&mut self, cx: &mut Context<Self>) {
        if !self.hide_codes {
            return;
        }
        // An earlier version of the model is replaced, not kept beside it.
        let _ = local_ai::remove_stale(&self.backend.store);
        if local_ai::OTP.is_installed(&self.backend.store) {
            self.load_otp(cx);
        } else {
            self.install_otp(cx);
        }
    }
    fn load_otp(&mut self, cx: &mut Context<Self>) {
        match local_ai::otp_detector(&self.backend.store) {
            Ok(detector) => {
                self.otp = Some(Rc::new(detector));
                self.model_error = None;
            }
            Err(e) => {
                self.otp = None;
                self.model_error = Some(format!("{e:#}"));
            }
        }
        self.otp_found.borrow_mut().clear();
        cx.notify();
    }
    fn install_otp(&mut self, cx: &mut Context<Self>) {
        if self.model_busy {
            return;
        }
        self.model_busy = true;
        self.model_error = None;
        self.model_progress.store(0, Ordering::Relaxed);
        let received = self.model_progress.clone();
        cx.notify();
        self.task(
            cx,
            move |b| local_ai::OTP.install(&b.store, &received),
            |this, result, cx| {
                this.model_busy = false;
                match result {
                    // Turned off while downloading: don't keep it.
                    Ok(()) if !this.hide_codes => {
                        let _ = local_ai::OTP.remove(&this.backend.store);
                    }
                    Ok(()) => this.load_otp(cx),
                    Err(e) => {
                        this.model_error = Some(format!("{e:#}"));
                        this.set_hide_codes(false, cx);
                    }
                }
                cx.notify();
            },
        );
        self.follow_download(cx);
    }
    /// Redraws while a model downloads, so its progress ring follows it.
    fn follow_download(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(100)).await;
                let busy = this
                    .update(cx, |this, cx| {
                        cx.notify();
                        this.model_busy || this.when_busy || this.parcel_busy || this.replies_busy
                    })
                    .unwrap_or(false);
                if !busy {
                    break;
                }
            }
        })
        .detach();
    }

    fn start_when(&mut self, cx: &mut Context<Self>) {
        if !self.understand_dates {
            return;
        }
        let _ = local_ai::remove_stale(&self.backend.store);
        if local_ai::WHEN.is_installed(&self.backend.store) {
            self.load_when(cx);
        } else {
            self.install_when(cx);
        }
    }
    fn load_when(&mut self, cx: &mut Context<Self>) {
        match local_ai::when_parser(&self.backend.store) {
            Ok(parser) => {
                self.set_parser(Some(Rc::new(parser)), cx);
                self.when_error = None;
            }
            Err(e) => {
                self.set_parser(None, cx);
                self.when_error = Some(format!("{e:#}"));
            }
        }
        cx.notify();
    }
    /// The date and time reader, here and in open composers (their Send
    /// Later picks it up the next time it opens).
    fn set_parser(&mut self, parser: Option<Rc<local_ai::when::Parser>>, cx: &mut Context<Self>) {
        self.when = parser.clone();
        for composer in self.composers.clone() {
            let parser = parser.clone();
            composer.update(cx, |c, _| c.when = parser);
        }
    }
    fn install_when(&mut self, cx: &mut Context<Self>) {
        if self.when_busy {
            return;
        }
        self.when_busy = true;
        self.when_error = None;
        self.when_progress.store(0, Ordering::Relaxed);
        let received = self.when_progress.clone();
        cx.notify();
        self.task(
            cx,
            move |b| local_ai::WHEN.install(&b.store, &received),
            |this, result, cx| {
                this.when_busy = false;
                match result {
                    // Turned off while downloading: don't keep it.
                    Ok(()) if !this.understand_dates => {
                        let _ = local_ai::WHEN.remove(&this.backend.store);
                    }
                    Ok(()) => this.load_when(cx),
                    Err(e) => {
                        this.when_error = Some(format!("{e:#}"));
                        this.set_understand_dates(false, cx);
                    }
                }
                cx.notify();
            },
        );
        self.follow_download(cx);
    }

    fn start_parcels(&mut self, cx: &mut Context<Self>) {
        if !self.find_parcels {
            return;
        }
        let _ = local_ai::remove_stale(&self.backend.store);
        if local_ai::PARCEL.is_installed(&self.backend.store) {
            self.load_parcels(cx);
        } else {
            self.install_parcels(cx);
        }
    }
    fn load_parcels(&mut self, cx: &mut Context<Self>) {
        match local_ai::parcel_confirmer(&self.backend.store) {
            Ok(model) => {
                self.parcel_model = Some(Rc::new(model));
                self.parcel_error = None;
            }
            Err(e) => {
                self.parcel_model = None;
                self.parcel_error = Some(format!("{e:#}"));
            }
        }
        self.forget_parcels(cx);
        cx.notify();
    }
    fn install_parcels(&mut self, cx: &mut Context<Self>) {
        if self.parcel_busy {
            return;
        }
        self.parcel_busy = true;
        self.parcel_error = None;
        self.parcel_progress.store(0, Ordering::Relaxed);
        let received = self.parcel_progress.clone();
        cx.notify();
        self.task(
            cx,
            move |b| local_ai::PARCEL.install(&b.store, &received),
            |this, result, cx| {
                this.parcel_busy = false;
                match result {
                    // Turned off while installing: don't keep it.
                    Ok(()) if !this.find_parcels => {
                        let _ = local_ai::PARCEL.remove(&this.backend.store);
                    }
                    Ok(()) => this.load_parcels(cx),
                    Err(e) => {
                        this.parcel_error = Some(format!("{e:#}"));
                        this.set_find_parcels(false, cx);
                    }
                }
                cx.notify();
            },
        );
        self.follow_download(cx);
    }
    pub(super) fn set_find_parcels(&mut self, on: bool, cx: &mut Context<Self>) {
        let on = on && self.local_ai;
        self.find_parcels = on;
        self.backend
            .store
            .set_preference_soon(local_ai::PARCEL_PREFERENCE, if on { "true" } else { "false" });
        if on {
            self.start_parcels(cx);
        } else {
            self.parcel_model = None;
            self.forget_parcels(cx);
            if let Err(e) = local_ai::PARCEL.remove(&self.backend.store) {
                self.notice(format!("{e:#}"), true, cx);
            }
        }
        cx.notify();
    }

    /// Whether a model is downloading.
    fn model_downloading(&self, model: &local_ai::Model) -> bool {
        if model.id == local_ai::WHEN.id {
            self.when_busy
        } else if model.id == local_ai::PARCEL.id {
            self.parcel_busy
        } else if model.id == local_ai::REPLIES.id {
            self.replies_busy
        } else {
            self.model_busy
        }
    }

    /// How far the model download has got, from 0 to 1.
    pub(super) fn model_fraction(&self, model: &local_ai::Model) -> f32 {
        let progress = if model.id == local_ai::WHEN.id {
            &self.when_progress
        } else if model.id == local_ai::PARCEL.id {
            &self.parcel_progress
        } else if model.id == local_ai::REPLIES.id {
            &self.replies_progress
        } else {
            &self.model_progress
        };
        (progress.load(Ordering::Relaxed) as f32 / model.size.max(1) as f32).min(1.)
    }

    /// Asks before deleting a model, saying which features go with it.
    pub(super) fn ask_delete_model(&mut self, id: String, cx: &mut Context<Self>) {
        if local_ai::model(&id).is_some() {
            self.model_confirm = Some(id);
            self.menu_anim += 1;
            cx.notify();
        }
    }
    /// Deletes a model, turning off the features that need it.
    pub(super) fn delete_model(&mut self, id: String, cx: &mut Context<Self>) {
        self.model_confirm = None;
        let Some(model) = local_ai::model(&id) else {
            return;
        };
        if model.id == local_ai::OTP.id {
            self.set_hide_codes(false, cx);
        }
        if model.id == local_ai::WHEN.id {
            self.set_understand_dates(false, cx);
        }
        if model.id == local_ai::PARCEL.id {
            self.set_find_parcels(false, cx);
        }
        if model.id == local_ai::REPLIES.id {
            self.set_suggest_replies(false, cx);
        }
        match model.remove(&self.backend.store) {
            Ok(()) => self.notice(
                format!("Deleted the {}", model.label.to_lowercase()),
                false,
                cx,
            ),
            Err(e) => self.notice(format!("{e:#}"), true, cx),
        }
        cx.notify();
    }

    pub(super) fn set_local_ai(&mut self, on: bool, cx: &mut Context<Self>) {
        self.local_ai = on;
        self.backend.store.set_preference_soon(
            local_ai::ENABLED_PREFERENCE,
            if on { "true" } else { "false" },
        );
        if !on {
            // Everything it installed goes with it.
            self.set_hide_codes(false, cx);
            self.set_understand_dates(false, cx);
            self.set_find_parcels(false, cx);
            self.set_suggest_replies(false, cx);
            self.model_error = None;
            self.when_error = None;
            self.parcel_error = None;
            self.replies_error = None;
            if let Err(e) = local_ai::remove_all(&self.backend.store) {
                self.notice(format!("{e:#}"), true, cx);
            }
        }
        cx.notify();
    }
    pub(super) fn set_hide_codes(&mut self, on: bool, cx: &mut Context<Self>) {
        let on = on && self.local_ai;
        self.hide_codes = on;
        self.backend
            .store
            .set_preference_soon(local_ai::OTP_PREFERENCE, if on { "true" } else { "false" });
        if on {
            self.start_otp(cx);
        } else {
            self.otp = None;
            self.otp_found.borrow_mut().clear();
            self.codes_shown.clear();
            if let Err(e) = local_ai::OTP.remove(&self.backend.store) {
                self.notice(format!("{e:#}"), true, cx);
            }
        }
        cx.notify();
    }

    pub(super) fn set_understand_dates(&mut self, on: bool, cx: &mut Context<Self>) {
        let on = on && self.local_ai;
        self.understand_dates = on;
        self.backend
            .store
            .set_preference_soon(local_ai::WHEN_PREFERENCE, if on { "true" } else { "false" });
        if on {
            self.start_when(cx);
        } else {
            self.set_parser(None, cx);
            if let Err(e) = local_ai::WHEN.remove(&self.backend.store) {
                self.notice(format!("{e:#}"), true, cx);
            }
        }
        cx.notify();
    }

    /// Why a feature's model couldn't be installed or loaded, and a way to
    /// try again.
    fn feature_error(
        &self,
        id: &'static str,
        error: Option<String>,
        retry: Action,
        cx: &Context<Self>,
    ) -> Option<AnyElement> {
        let c = self.colors();
        let error = error?;
        Some(
            div()
                .id(id)
                .debug_selector(move || id.into())
                .flex()
                .items_center()
                .gap_2()
                .text_size(px(12.))
                .text_color(rgb(c.danger))
                .child(div().flex_1().min_w_0().child(error))
                .when(self.local_ai, |d| {
                    d.child(self.link(&format!("{id}-retry"), "Try Again", retry, cx))
                })
                .into_any_element(),
        )
    }

    pub(super) fn local_ai_page(&self, cx: &Context<Self>) -> AnyElement {
        let c = self.colors();
        let model = &local_ai::OTP;
        let error = self.model_error.clone();
        div()
            .flex()
            .flex_col()
            .gap_5()
            .child(self.section("Local AI"))
            .child(div().text_size(px(12.)).text_color(rgb(c.muted)).child(
                "Small models that run on this computer to understand your mail. \
                     Nothing is downloaded until you turn on a feature that needs a model, \
                     and your mail never leaves this computer.",
            ))
            .child(self.switch_setting(
                "Local AI",
                "Turning this off deletes every model it downloaded.",
                "local-ai".into(),
                self.local_ai,
                Action::SetLocalAi(!self.local_ai),
                cx,
            ))
            .child(self.section("Features"))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .when(!self.local_ai, |d| d.opacity(0.5))
                    .child(self.switch_setting(
                        "Hide sign-in codes",
                        &format!(
                            "Find two-factor, verification and sign-in codes in received mail (in 37 \
                             languages), and \
                             show them as {b}{b}{b}{b}{b}{b} in subjects, message text and \
                             notifications. Downloads the {} ({}).",
                            model.label.to_lowercase(),
                            mb(model.size),
                            b = otp::BULLET,
                        ),
                        "local-ai-otp".into(),
                        self.hide_codes,
                        // Needs Local AI on first.
                        Action::SetHideCodes(self.local_ai && !self.hide_codes),
                        cx,
                    ))
                    .children(self.feature_error(
                        "local-ai-status",
                        error,
                        Action::SetHideCodes(true),
                        cx,
                    ))
                    .child(self.switch_setting(
                        "Understand typed dates",
                        &format!(
                            "Type when to snooze a message or send one later in your own words, in 37 \
                             languages (“tmrw at 6”, “the monday in 3 weeks”, “übermorgen um 18 Uhr”, \
                             “明天下午6点”), typos and all. Downloads the {} ({}).",
                            local_ai::WHEN.label.to_lowercase(),
                            mb(local_ai::WHEN.size),
                        ),
                        "local-ai-when".into(),
                        self.understand_dates,
                        Action::SetUnderstandDates(self.local_ai && !self.understand_dates),
                        cx,
                    ))
                    .children(self.feature_error(
                        "local-ai-when-status",
                        self.when_error.clone(),
                        Action::SetUnderstandDates(true),
                        cx,
                    ))
                    .child(self.switch_setting(
                        "Find tracking numbers",
                        &format!(
                            "Confirm the tracking numbers found in mail by the words around \
                             them, in many languages, so the reader and each account's \
                             Summary find the ones given without a shipping word beside \
                             them, and skip order and phone numbers that look like them. \
                             Installs the {} ({}).",
                            local_ai::PARCEL.label.to_lowercase(),
                            mb(local_ai::PARCEL.size),
                        ),
                        "local-ai-parcel".into(),
                        self.find_parcels,
                        Action::SetFindParcels(self.local_ai && !self.find_parcels),
                        cx,
                    ))
                    .children(self.feature_error(
                        "local-ai-parcel-status",
                        self.parcel_error.clone(),
                        Action::SetFindParcels(true),
                        cx,
                    ))
                    .child(self.switch_setting(
                        "Suggest replies",
                        &format!(
                            "Offer a few short replies under received mail, chosen for what it \
                             says and worded in its language (37 languages): “Let me check my \
                             calendar.”, “Congratulations!”, “¡Tengo muchas ganas!”. Choosing one \
                             starts a reply with it written in. Replies you choose are offered \
                             more, and right-clicking one lets you stop it. \
                             Installs the {} ({}).",
                            local_ai::REPLIES.label.to_lowercase(),
                            mb(local_ai::REPLIES.size),
                        ),
                        "local-ai-replies".into(),
                        self.suggest_replies,
                        Action::SetSuggestReplies(self.local_ai && !self.suggest_replies),
                        cx,
                    ))
                    .children(self.feature_error(
                        "local-ai-replies-status",
                        self.replies_error.clone(),
                        Action::SetSuggestReplies(true),
                        cx,
                    ))
                    .when(!self.reply_history.is_empty(), |d| {
                        let hidden = self.reply_history.dismissed().len();
                        d.child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .text_size(px(12.))
                                .text_color(rgb(c.muted))
                                .child(div().flex_1().min_w_0().child(if hidden == 0 {
                                    "Picacho remembers which suggested replies you choose, on this computer.".to_owned()
                                } else {
                                    format!(
                                        "Picacho remembers which suggested replies you choose, and stopped {hidden} you turned away."
                                    )
                                }))
                                .child(self.link(
                                    "local-ai-replies-forget",
                                    "Forget",
                                    Action::ForgetReplyHistory,
                                    cx,
                                )),
                        )
                    }),
            )
            .child(self.section("Models"))
            .child(
                div()
                    .id("local-ai-models")
                    .debug_selector(|| "local-ai-models".into())
                    .flex()
                    .flex_col()
                    .rounded(px(10.))
                    .border_1()
                    .border_color(rgb(c.border))
                    .children(
                        local_ai::MODELS
                            .into_iter()
                            .enumerate()
                            .map(|(i, m)| self.model_row(m, i > 0, cx)),
                    ),
            )
            .child(div().text_size(px(12.)).text_color(rgb(c.muted)).child({
                let total = local_ai::installed_size(&self.backend.store);
                let used = if total > 0 {
                    format!("{} on this computer. ", mb(total))
                } else {
                    String::new()
                };
                format!(
                    "{used}Models download when a feature that needs them is turned on. \
                     They run on this computer; your mail isn't sent anywhere."
                )
            }))
            .into_any_element()
    }

    /// A model in Settings: installed or not (or how far its download has
    /// got), its size, what uses it, and a way to delete it.
    fn model_row(
        &self,
        model: &'static local_ai::Model,
        divided: bool,
        cx: &Context<Self>,
    ) -> AnyElement {
        let c = self.colors();
        let store = &self.backend.store;
        let installed = model.is_installed(store);
        let downloading = self.model_downloading(model);
        let status: AnyElement = if downloading {
            let fraction = self.model_fraction(model);
            div()
                .id(SharedString::from(format!("model-progress-{}", model.id)))
                .debug_selector(|| "model-progress".into())
                .relative()
                .size(px(22.))
                .flex_shrink_0()
                .child(ring(fraction, c.border, c.accent).absolute().inset_0())
                .into_any_element()
        } else if installed {
            div()
                .size(px(22.))
                .flex_shrink_0()
                .rounded_full()
                .bg(rgb(c.accent))
                .flex()
                .items_center()
                .justify_center()
                .child(icon_el("check", 13., c.on_accent))
                .into_any_element()
        } else {
            div()
                .size(px(22.))
                .flex_shrink_0()
                .rounded_full()
                .border_1()
                .border_color(rgb(c.border))
                .flex()
                .items_center()
                .justify_center()
                .child(icon_el("close", 12., c.muted))
                .into_any_element()
        };
        let detail = if downloading {
            let fraction = self.model_fraction(model);
            if fraction >= 1. {
                "Installing…".to_owned()
            } else {
                format!("Downloading… {:.0}% of {}", fraction * 100., mb(model.size))
            }
        } else if installed {
            format!("Installed · {}", mb(model.installed_size(store)))
        } else {
            format!("Not installed · {} download", mb(model.size))
        };
        let id = model.id;
        div()
            .id(SharedString::from(format!("model-{id}")))
            .debug_selector(move || format!("model-{id}"))
            .px_3()
            .py_2()
            .flex()
            .items_center()
            .gap_3()
            .when(divided, |d| d.border_t_1().border_color(rgb(c.border)))
            .child(status)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(1.))
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_weight(FontWeight::MEDIUM)
                            .child(model.label),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(rgb(c.muted))
                            .child(format!(
                                "{} · Used by {}",
                                detail,
                                model.features.join(", ")
                            )),
                    ),
            )
            .when(installed && !downloading, |d| {
                d.child(
                    self.button(
                        format!("model-delete-{id}"),
                        "@Delete model",
                        "trash",
                        Action::AskDeleteModel(id.into()),
                        false,
                        cx,
                    )
                    .debug_selector(move || format!("model-delete-{id}")),
                )
            })
            .into_any_element()
    }

    /// The confirmation over Settings before a model is deleted.
    pub(super) fn delete_model_sheet(&self, id: &str, cx: &Context<Self>) -> AnyElement {
        let c = self.colors();
        let Some(model) = local_ai::model(id) else {
            return div().into_any_element();
        };
        let features: Vec<&str> = model.features.to_vec();
        let content = div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .text_size(px(14.))
                    .font_weight(FontWeight::MEDIUM)
                    .child(format!(
                        "Delete the {} ({})?",
                        model.label.to_lowercase(),
                        mb(model.installed_size(&self.backend.store))
                    )),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(c.muted))
                    .child(if features.len() == 1 {
                        "Deleting it turns off this feature:"
                    } else {
                        "Deleting it turns off these features:"
                    }),
            )
            .children(features.into_iter().map(|f| {
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_size(px(13.))
                    .child(icon_el("close", 12., c.danger))
                    .child(f)
            }))
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(c.muted))
                    .child("Turning the feature on again downloads the model again."),
            );
        let footer = div()
            .child(div().flex_1())
            .child(self.button(
                "delete-model-cancel",
                "Cancel",
                "",
                Action::CloseModelSheet,
                false,
                cx,
            ))
            .child(
                self.button(
                    "delete-model-confirm",
                    "Delete",
                    "",
                    Action::DeleteModel(model.id.into()),
                    true,
                    cx,
                )
                .bg(rgb(c.danger)),
            );
        self.sheet_over_settings(
            super::settings::Sheet {
                id: "delete-model-sheet",
                title: "Delete Model",
                width: 400.,
                close: Action::CloseModelSheet,
            },
            content,
            Some(footer),
            cx,
        )
    }
}

/// Megabytes, as Settings shows sizes.
fn mb(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / 1_000_000.)
}

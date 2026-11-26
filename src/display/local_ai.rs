//! Local AI in the app: the Settings page that turns it and its features on
//! (downloading their models) and off (deleting them), hiding the sign-in
//! codes the model finds wherever mail is shown, the date and time model
//! that reads times typed into Snooze and Send Later, the tracking number
//! model that confirms the parcels the reader and Summary show, and the
//! reply model that suggests replies under received mail.
use super::*;
use crate::manage::local_ai::{self, otp, replies};
use std::borrow::Cow;

/// Replies suggested per message, by the lengths of the subject and text
/// and the history revision they were suggested for.
pub(super) type Suggested = Inferred<(usize, usize, u64), replies::Suggestions>;

/// What a model found per message (by message id), with what it was worked
/// out from (`K`: lengths of the text, a revision); and the messages it is
/// still working on, off the window's thread. Drawing finds what is there
/// and starts what is missing; a result arrives only if it is still wanted
/// (`finish`), so one started before `clear` is dropped.
pub(super) struct Inferred<K, V> {
    found: HashMap<String, (K, Rc<V>)>,
    running: HashMap<String, K>,
}
impl<K, V> Default for Inferred<K, V> {
    fn default() -> Self {
        Self { found: HashMap::new(), running: HashMap::new() }
    }
}
impl<K: PartialEq + Clone, V> Inferred<K, V> {
    /// What was found in message `id` from `key`.
    pub(super) fn get(&self, id: &str, key: &K) -> Option<Rc<V>> {
        self.found.get(id).filter(|(k, _)| k == key).map(|(_, v)| v.clone())
    }
    /// Notes that the model starts on message `id` from `key`: false where
    /// it already is.
    pub(super) fn start(&mut self, id: &str, key: &K) -> bool {
        if self.running.get(id) == Some(key) {
            return false;
        }
        self.running.insert(id.to_owned(), key.clone());
        true
    }
    /// Keeps what the model found in message `id` from `key`, if that is
    /// still what was asked for. False where it no longer is.
    pub(super) fn finish(&mut self, id: &str, key: K, value: V) -> bool {
        if self.running.get(id) != Some(&key) {
            return false;
        }
        self.running.remove(id);
        if self.found.len() >= FOUND_CACHE {
            self.found.clear();
        }
        self.found.insert(id.to_owned(), (key, Rc::new(value)));
        true
    }
    /// Forgets what was found, and what is still being worked out.
    pub(super) fn clear(&mut self) {
        self.found.clear();
        self.running.clear();
    }
}

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
    ///
    /// Worked out while drawing, unlike suggested replies and parcels: a
    /// code must never show unmasked while the model is still looking, and
    /// the code model is a single small layer over a message's candidates.
    pub(super) fn found_codes(&self, m: &Mail) -> Rc<Vec<String>> {
        if !self.hide_codes {
            return Rc::default();
        }
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
        let codes = Rc::new(match &self.otp {
            Some(detector) => detector.codes(&m.subject, body),
            // Codes are hidden but the model isn't loaded (still installing,
            // or it couldn't load): which is the code can't be told, so
            // every string shaped like one is hidden until it is, or until
            // Show code is clicked. Loading the model forgets these.
            None => otp::candidates(&format!("{}\n{body}", m.subject)),
        });
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
    /// `m`'s subject as a new-mail notification shows it (on the lock
    /// screen), its codes hidden: those the model finds, or, while it isn't
    /// loaded, every string shaped like a code (`found_codes`). Whether or
    /// not they were shown in the app: a notification has no Show code to
    /// click. As the background monitor and the widgets do.
    pub(super) fn announced_subject<'a>(&self, m: &'a Mail) -> Cow<'a, str> {
        let codes = self.found_codes(m);
        if codes.is_empty() {
            return m.subject.as_str().into();
        }
        otp::mask(&m.subject, &codes)
    }
    /// `text` standing for a message where the message itself isn't at hand
    /// (a Summary or Receipts row's subject), with its codes hidden: those
    /// the model finds in it alone, or, before the model is loaded, every
    /// string shaped like one. Such rows have no Show code; the message,
    /// opened, does.
    pub(super) fn hide_codes_in<'a>(&self, text: &'a str) -> Cow<'a, str> {
        if !self.hide_codes {
            return text.into();
        }
        let Some(detector) = &self.otp else {
            return otp::mask_all(text);
        };
        // Kept with the messages' codes (forgotten with them), under a key
        // no message id has.
        let id = format!("\0text\0{text}");
        let key = (text.len(), 0);
        let cached = self.otp_found.borrow().get(&id).filter(|(k, _)| *k == key).map(|(_, c)| c.clone());
        let codes = cached.unwrap_or_else(|| {
            let codes = Rc::new(detector.codes(text, ""));
            let mut found = self.otp_found.borrow_mut();
            if found.len() >= FOUND_CACHE {
                found.clear();
            }
            found.insert(id, (key, codes.clone()));
            codes
        });
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
            if shown { t!("local-ai-hide-code") } else { t!("local-ai-show-code") },
            Action::ShowCodes(m.id.clone(), !shown),
            cx,
        );
        // Copies without showing it.
        let copy = self.link("codes-copy", t!("local-ai-copy-code"), Action::CopyCode(m.id.clone()), cx);
        Some(
            self.banner(
                "codes-banner",
                "lock",
                BannerStyle::Filled,
                if shown {
                    t!("local-ai-this-messages-sign-in-code-is")
                } else {
                    t!("local-ai-this-messages-sign-in-code-is-2")
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
        // Before the model has read it, which string is the code isn't
        // known (`found_codes` hides every one shaped like a code).
        if self.otp.is_none() {
            self.notice(t!("local-ai-the-code-model-isnt-ready"), true, cx);
            return;
        }
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
        self.notice(t!("local-ai-code-copied"), false, cx);
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
    /// Only what the model already found: `smart_replies_for` starts it on
    /// a message it hasn't read.
    pub(super) fn smart_replies(&self, m: &Mail) -> Rc<replies::Suggestions> {
        match self.replies_wanted(m) {
            Some((_, key)) => self.replies_found.borrow().get(&m.id, &key).unwrap_or_default(),
            None => Rc::default(),
        }
    }
    /// `smart_replies`, starting the model off the window's thread where it
    /// hasn't read `m` yet: the suggestions appear once it has.
    pub(super) fn smart_replies_for(&self, m: &Mail, cx: &Context<Self>) -> Rc<replies::Suggestions> {
        let Some((model, key)) = self.replies_wanted(m) else {
            return Rc::default();
        };
        if let Some(found) = self.replies_found.borrow().get(&m.id, &key) {
            return found;
        }
        if !self.replies_found.borrow_mut().start(&m.id, &key) {
            return Rc::default();
        }
        let id = m.id.clone();
        let subject = m.subject.clone();
        let body = m.body.clone().unwrap_or_else(|| m.snippet.clone());
        let history = self.reply_history.clone();
        self.infer(
            cx,
            move || model.suggest(&subject, &body, &history, replies::fallback_language(), replies::MAX_SUGGESTIONS),
            move |this, found| this.replies_found.borrow_mut().finish(&id, key, found.unwrap_or_default()),
        );
        Rc::default()
    }
    /// For `smart_replies`: the model and what its suggestions for `m` are
    /// worked out from, or None where none are suggested at all.
    fn replies_wanted(&self, m: &Mail) -> Option<(Arc<replies::Suggester>, (usize, usize, u64))> {
        let model = self.reply_model.as_ref().filter(|_| self.suggest_replies)?;
        let kinds = self.mail_kinds.as_deref();
        if matches!(m.folder, Folder::Sent | Folder::Drafts)
            || replies::wants_no_reply(&m.from, &m.list_unsubscribe)
            || crate::manage::mail_kinds::Kinds::code(kinds, m)
            || crate::manage::mail_kinds::Kinds::package(kinds, m)
            || crate::manage::mail_kinds::Kinds::receipt(kinds, m)
        {
            return None;
        }
        let body = m.body.as_deref().unwrap_or(&m.snippet);
        Some((model.clone(), (m.subject.len(), body.len(), self.reply_history_rev)))
    }

    /// Runs a model's `work` off the window's thread, then `done` with what
    /// it found (None if it failed), redrawing when `done` says what it
    /// found is kept. For what drawing finds missing: it has only `&self`.
    pub(super) fn infer<T: Send + 'static>(
        &self,
        cx: &Context<Self>,
        work: impl FnOnce() -> T + Send + 'static,
        done: impl FnOnce(&mut Self, Option<T>) -> bool + 'static,
    ) {
        cx.spawn(async move |this, cx| {
            let found = cx
                .background_executor()
                .spawn(async move { std::panic::catch_unwind(std::panic::AssertUnwindSafe(work)).ok() })
                .await;
            let _ = this.update(cx, |this, cx| {
                if done(this, found) {
                    cx.notify();
                }
            });
        })
        .detach();
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
            t!("local-ai-picacho-forgot-which-replies-you-chose"),
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
                self.reply_model = Some(Arc::new(model));
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
                self.otp = Some(Arc::new(detector));
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
                    // Codes stay hidden, every string shaped like one, until
                    // the model is there: the error says why, with a retry,
                    // and the next start tries again.
                    Err(e) => this.model_error = Some(format!("{e:#}")),
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
                self.parcel_model = Some(Arc::new(model));
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
                t!("local-ai-deleted-the", to_lowercase = model.label.to_lowercase()),
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
                    d.child(self.link(&format!("{id}-retry"), t!("local-ai-try-again"), retry, cx))
                })
                .into_any_element(),
        )
    }

    pub(super) fn local_ai_page(&self, cx: &Context<Self>) -> AnyElement {
        let c = self.colors();
        // A feature's switch, and what went wrong getting its model.
        let feature = |title: &str,
                       caption: &str,
                       id: &str,
                       on: bool,
                       toggle: Action,
                       status: &'static str,
                       error: Option<String>,
                       retry: Action| {
            div()
                .flex()
                .flex_col()
                .gap_2()
                .when(!self.local_ai, |d| d.opacity(0.5))
                .child(self.switch_setting(title, caption, id.into(), on, toggle, cx))
                .children(self.feature_error(status, error, retry, cx))
        };
        let features = Rows::new()
            .row(feature(
                t!("local-ai-hide-sign-in-codes"),
                &t!(
                    "local-ai-find-two-factor-verification-and-sign",
                    b = otp::BULLET,
                    size = mb(local_ai::OTP.size)
                ),
                "local-ai-otp",
                self.hide_codes,
                // Needs Local AI on first.
                Action::SetHideCodes(self.local_ai && !self.hide_codes),
                "local-ai-status",
                self.model_error.clone(),
                Action::SetHideCodes(true),
            ))
            .row(feature(
                t!("local-ai-understand-typed-dates"),
                &t!("local-ai-type-when-to-snooze-a-message", size = mb(local_ai::WHEN.size)),
                "local-ai-when",
                self.understand_dates,
                Action::SetUnderstandDates(self.local_ai && !self.understand_dates),
                "local-ai-when-status",
                self.when_error.clone(),
                Action::SetUnderstandDates(true),
            ))
            .row(feature(
                t!("local-ai-find-tracking-numbers"),
                &t!("local-ai-confirm-the-tracking-numbers-found-in", size = mb(local_ai::PARCEL.size)),
                "local-ai-parcel",
                self.find_parcels,
                Action::SetFindParcels(self.local_ai && !self.find_parcels),
                "local-ai-parcel-status",
                self.parcel_error.clone(),
                Action::SetFindParcels(true),
            ))
            .row(feature(
                t!("local-ai-suggest-replies"),
                &t!("local-ai-offer-a-few-short-replies-under", size = mb(local_ai::REPLIES.size)),
                "local-ai-replies",
                self.suggest_replies,
                Action::SetSuggestReplies(self.local_ai && !self.suggest_replies),
                "local-ai-replies-status",
                self.replies_error.clone(),
                Action::SetSuggestReplies(true),
            ))
            .when(!self.reply_history.is_empty(), |rows| {
                let hidden = self.reply_history.dismissed().len();
                rows.row(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .text_size(px(12.))
                        .text_color(rgb(c.muted))
                        .child(div().flex_1().min_w_0().child(if hidden == 0 {
                            t!("local-ai-picacho-remembers-which-suggested-replies-you").to_owned()
                        } else {
                            t!("local-ai-picacho-remembers-which-suggested-replies-you-2", hidden = hidden)
                        }))
                        .child(self.link(
                            "local-ai-replies-forget",
                            t!("local-ai-forget"),
                            Action::ForgetReplyHistory,
                            cx,
                        )),
                )
            });
        let total = local_ai::installed_size(&self.backend.store);
        self.settings_column()
            .child(self.settings_group(
                t!("local-ai-local-ai"),
                Rows::new().row(self.switch_setting(
                    t!("local-ai-local-ai"),
                    t!("local-ai-turning-this-off-deletes-every-model"),
                    "local-ai".into(),
                    self.local_ai,
                    Action::SetLocalAi(!self.local_ai),
                    cx,
                )),
            ))
            .child(self.settings_group(t!("local-ai-features"), features))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(self.section(t!("local-ai-models")))
                    .child(
                        div()
                            .id("local-ai-models")
                            .debug_selector(|| "local-ai-models".into())
                            .flex()
                            .flex_col()
                            .rounded(px(12.))
                            .border_1()
                            .border_color(rgb(c.border))
                            .bg(rgb(c.raised))
                            .children(
                                local_ai::MODELS
                                    .into_iter()
                                    .enumerate()
                                    .map(|(i, m)| self.model_row(m, i > 0, cx)),
                            ),
                    )
                    .when(total > 0, |d| {
                        d.child(
                            div()
                                .px_1()
                                .text_size(px(12.))
                                .text_color(rgb(c.muted))
                                .child(t!("local-ai-on-this-computer", size = mb(total))),
                        )
                    }),
            )
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
                t!("local-ai-installing").to_owned()
            } else {
                t!("local-ai-downloading-of", percent = format!("{:.0}", fraction * 100.), size = mb(model.size))
            }
        } else if installed {
            t!("local-ai-installed", size = mb(model.installed_size(store)))
        } else {
            t!("local-ai-not-installed-download", size = mb(model.size))
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
                            .child(t!("local-ai-used-by", detail = detail, features = model.features.join(", "))),
                    ),
            )
            .when(installed && !downloading, |d| {
                d.child(
                    self.button(
                        format!("model-delete-{id}"),
                        t!("local-ai-delete-model"),
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
                    .child(t!("local-ai-delete-the", model = model.label.to_lowercase(), size = mb(model.installed_size(&self.backend.store)))),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(c.muted))
                    .child(t!("local-ai-deleting-it-turns-off-these-features", count = features.len())),
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
                    .child(t!("local-ai-turning-the-feature-on-again-downloads")),
            );
        let footer = div()
            .child(div().flex_1())
            .child(self.button(
                "delete-model-cancel",
                t!("local-ai-cancel"),
                "",
                Action::CloseModelSheet,
                false,
                cx,
            ))
            .child(
                self.button(
                    "delete-model-confirm",
                    t!("local-ai-delete"),
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
                title: t!("local-ai-delete-model-2"),
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
    t!("local-ai-mb", mb = format!("{:.1}", bytes as f64 / 1_000_000.))
}

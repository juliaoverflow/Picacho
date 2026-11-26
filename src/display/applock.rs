//! The app lock's screen, Settings → Privacy (the app lock and the
//! encrypted database), and when Picacho locks.
//!
//! Locked, the main window shows only the lock screen, and Picacho's other
//! windows (messages, composers, Settings, the desktop widget) a cover
//! (`cover`), until the passcode or the device's biometrics unlock it.
//! Picacho locks as it opens, from Lock Now (the Picacho menu, Settings),
//! and once it has been away (no window of it in front, or in the
//! background on a phone) for the chosen time; "Immediately" locks as soon
//! as it is left. Mail is still checked and announced meanwhile.
use super::*;
use crate::display::biometric::{self, Biometry, Failure};
use crate::manage::applock::{self, Failures, LockSettings, PasscodeHash};
use std::time::Instant;

/// What the lock's settings form is doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LockForm {
    /// Choosing a passcode, to turn the lock on.
    Enable,
    /// The current passcode, then a new one.
    Change,
    /// The current passcode, to turn the lock off.
    Disable,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum LockAction {
    /// Unlock with the passcode typed on the lock screen.
    Unlock,
    /// Unlock with Touch ID, Face ID or the phone's biometrics.
    UseBiometrics,
    LockNow,
    /// Opens (or, with `None`, closes) a settings form.
    Form(Option<LockForm>),
    SubmitForm,
    SetBiometrics(bool),
    /// Seconds away before locking.
    SetAfter(u64),
    /// Asks to confirm encrypting (true) or decrypting (false) the
    /// database; `None` takes the question back.
    AskEncrypt(Option<bool>),
    SetEncrypted(bool),
    /// Keeps senders, subjects and previews out of notifications and widgets.
    SetHideContents(bool),
}

/// Whether Picacho's windows are covered by the lock.
#[derive(Default)]
pub(super) struct Covered(bool);
impl Global for Covered {}

/// Whether the lock covers Picacho's windows now.
pub(super) fn covered(cx: &App) -> bool {
    cx.try_global::<Covered>().is_some_and(|c| c.0)
}

/// What a window other than the main one shows while Picacho is locked.
pub(super) fn cover(c: Colors, window: &Window) -> AnyElement {
    div()
        .id("lock-cover")
        .debug_selector(|| "lock-cover".into())
        .map(|d| super::window::window_frame(d, c, window))
        .size_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap_3()
        .bg(rgb(c.bg))
        .text_color(rgb(c.muted))
        .font_family(".SystemUIFont")
        .text_size(px(13.))
        .child(icon_el("lock", 28., c.muted))
        .child(t!("applock-picacho-is-locked"))
        .into_any_element()
}

/// How long a prompt that just ended keeps the window's coming back to the
/// front from asking again: on phones the prompt itself sends the app to
/// the background and back.
const PROMPT_SETTLE: Duration = Duration::from_millis(1500);
/// How long after a window leaves the front Picacho counts as away, if no
/// other window of it came forward meanwhile.
const AWAY_CHECK: Duration = Duration::from_millis(400);

pub(super) struct LockUi {
    pub(super) settings: LockSettings,
    pub(super) locked: bool,
    /// Since when Picacho has been away (no window in front), while unlocked.
    away_since: Option<Instant>,
    /// Counts departures, so a check for one that ended does nothing.
    away_turn: u64,
    /// A biometric prompt is up.
    authenticating: bool,
    /// When the last biometric prompt ended.
    prompt_ended: Option<Instant>,
    /// A passcode is being checked.
    checking: bool,
    error: Option<String>,
    /// The biometrics this device has, as found when the window opened or
    /// Settings → Privacy was last opened.
    pub(super) biometry: Option<Biometry>,
    pub(super) passcode: Entity<InputState>,
    pub(super) current: Entity<InputState>,
    pub(super) new_code: Entity<InputState>,
    pub(super) confirm: Entity<InputState>,
    form: Option<LockForm>,
    form_error: Option<String>,
    saving: bool,
    /// Encrypting (true) or decrypting (false) waits for a confirmation.
    confirm_encrypt: Option<bool>,
    encrypting: bool,
    /// Notifications and widgets leave out what mail says.
    hide_contents: bool,
    /// A phone's screen is covered while Picacho isn't in front, so the
    /// app switcher's picture of it shows no mail.
    pub(super) obscured: bool,
    encrypted: bool,
    encryption_error: Option<String>,
    /// Notifications tapped, and their buttons pressed, while locked: acted
    /// on once unlocked, not behind the lock screen (a reply's composer
    /// is a window of its own; Archive and Delete change mail).
    pub(super) deferred: Vec<Deferred>,
}

/// What a notification asked for while Picacho was locked.
pub(super) enum Deferred {
    Open(crate::display::notify::Target),
    Quick(crate::display::notify::Target, String),
}

impl LockUi {
    pub(super) fn new(store: &Store, window: &mut Window, cx: &mut App) -> Self {
        let settings = LockSettings::load(store);
        let locked = settings.enabled();
        cx.set_global(Covered(locked));
        Self {
            settings,
            locked,
            away_since: None,
            away_turn: 0,
            authenticating: false,
            prompt_ended: None,
            checking: false,
            error: None,
            biometry: biometric::available(),
            passcode: input(t!("applock-passcode"), true, window, cx),
            current: input(t!("applock-current-passcode"), true, window, cx),
            new_code: input(t!("applock-new-passcode"), true, window, cx),
            confirm: input(t!("applock-new-passcode-again"), true, window, cx),
            form: None,
            form_error: None,
            saving: false,
            confirm_encrypt: None,
            encrypting: false,
            encrypted: store.encrypted(),
            encryption_error: None,
            hide_contents: applock::hides_contents(store),
            obscured: false,
            deferred: Vec::new(),
        }
    }

    /// Enter in the lock's fields: unlocks, or submits the settings form.
    pub(super) fn subscribe(
        &self,
        window: &mut Window,
        cx: &mut Context<MailApp>,
    ) -> Vec<Subscription> {
        let mut out = vec![
            cx.subscribe_in(&self.passcode, window, |this, _, event, w, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.lock_action(LockAction::Unlock, w, cx);
                }
            }),
        ];
        for state in [&self.current, &self.new_code, &self.confirm] {
            out.push(cx.subscribe_in(state, window, |this, _, event, w, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.lock_action(LockAction::SubmitForm, w, cx);
                }
            }));
        }
        out
    }

    /// The lock's text fields, for the phone's keyboard.
    pub(super) fn fields(&self) -> [&Entity<InputState>; 4] {
        [&self.passcode, &self.current, &self.new_code, &self.confirm]
    }

    /// Biometrics unlock: on, and available on this device.
    fn biometrics(&self) -> Option<Biometry> {
        self.biometry.filter(|_| self.settings.biometrics)
    }
}

impl MailApp {
    pub(super) fn lock_action(
        &mut self,
        action: LockAction,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            LockAction::Unlock => self.unlock_with_passcode(w, cx),
            LockAction::UseBiometrics => self.unlock_with_biometrics(w, cx),
            LockAction::LockNow if !self.lock.settings.enabled() => self.notice(
                t!("applock-turn-on-the-app-lock-in"),
                false,
                cx,
            ),
            LockAction::LockNow => self.lock_now(w, cx),
            LockAction::Form(form) => {
                self.lock.form = form;
                self.lock.form_error = None;
                for state in [&self.lock.current, &self.lock.new_code, &self.lock.confirm] {
                    Self::set(state, "", w, cx);
                }
                let first = match form {
                    Some(LockForm::Enable) => Some(&self.lock.new_code),
                    Some(_) => Some(&self.lock.current),
                    None => None,
                };
                if let Some(first) = first {
                    first.read(cx).focus_handle(cx).focus(w, cx);
                }
                // Biometrics may have been set up since the window opened.
                self.lock.biometry = biometric::available();
            }
            LockAction::SubmitForm => self.submit_lock_form(w, cx),
            LockAction::SetBiometrics(on) => {
                let mut settings = self.lock.settings.clone();
                settings.biometrics = on;
                self.save_lock_settings(settings, cx);
            }
            LockAction::SetAfter(after) => {
                let mut settings = self.lock.settings.clone();
                settings.after = after;
                self.save_lock_settings(settings, cx);
            }
            LockAction::AskEncrypt(on) => {
                self.lock.confirm_encrypt = on;
                self.lock.encryption_error = None;
            }
            LockAction::SetEncrypted(on) => self.set_database_encrypted(on, cx),
            LockAction::SetHideContents(on) => self.set_hide_contents(on, cx),
        }
        cx.notify();
    }

    fn save_lock_settings(&mut self, settings: LockSettings, cx: &mut Context<Self>) {
        let saved = settings.clone();
        self.write_in_order(
            cx,
            move |b| saved.save(&b.store),
            move |this, result, cx| match result {
                Ok(()) => this.lock.settings = settings,
                Err(e) => this.notice(
                    t!("applock-couldnt-save-the-app-locks-settings", e = format!("{:#}", e)),
                    true,
                    cx,
                ),
            },
        );
    }

    /// Locks Picacho now (when the lock is on).
    pub(super) fn lock_now(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        if !self.lock.settings.enabled() {
            return;
        }
        self.lock.locked = true;
        self.lock.away_since = None;
        self.lock.error = None;
        self.lock.checking = false;
        self.lock.form = None;
        self.menu = None;
        Self::set(&self.lock.passcode, "", w, cx);
        cx.set_global(Covered(true));
        cx.refresh_windows();
        if w.is_window_active() {
            self.lock_appeared(w, cx);
        }
        cx.notify();
    }

    /// The lock screen is in front: asks for biometrics when they're on,
    /// otherwise (or on a computer, as well) puts the cursor in the
    /// passcode field.
    pub(super) fn lock_appeared(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        if !self.lock.locked {
            return;
        }
        let settling = self
            .lock
            .prompt_ended
            .is_some_and(|t| t.elapsed() < PROMPT_SETTLE);
        if self.lock.biometrics().is_some() && !self.lock.authenticating && !settling {
            self.unlock_with_biometrics(w, cx);
        }
        // A phone's keyboard would cover the screen the prompt is asking over.
        if !self.mobile.on || self.lock.biometrics().is_none() {
            self.lock.passcode.read(cx).focus_handle(cx).focus(w, cx);
        }
    }

    fn unlocked(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        self.lock.locked = false;
        self.lock.error = None;
        self.lock.away_since = None;
        Self::set(&self.lock.passcode, "", w, cx);
        Failures::clear(&self.backend.store);
        cx.set_global(Covered(false));
        cx.refresh_windows();
        self.focus.focus(w, cx);
        // What notifications asked for meanwhile; what Picacho was opened
        // with is taken as it draws again (`take_launch_requests`).
        for deferred in std::mem::take(&mut self.lock.deferred) {
            match deferred {
                Deferred::Open(target) => self.open_notified(target, w, cx),
                Deferred::Quick(target, action) => self.quick_action(target, &action, w, cx),
            }
        }
        cx.notify();
    }

    fn unlock_with_passcode(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        if !self.lock.locked || self.lock.checking {
            return;
        }
        let typed = zeroize::Zeroizing::new(Self::value(&self.lock.passcode, cx));
        if typed.is_empty() {
            return;
        }
        let now = chrono::Utc::now().timestamp();
        if let Some(until) = Failures::load(&self.backend.store).wait_until()
            && until > now
        {
            self.lock.error = Some(wait_message(until - now));
            self.retry_later(until - now, cx);
            return;
        }
        self.lock.checking = true;
        self.lock.error = None;
        let settings = self.lock.settings.clone();
        let check = cx.background_spawn(async move { settings.verify(&typed) });
        cx.spawn_in(w, async move |this, cx| {
            let right = check.await;
            let _ = this.update_in(cx, |this, w, cx| {
                this.lock.checking = false;
                if right {
                    this.unlocked(w, cx);
                    return;
                }
                let now = chrono::Utc::now().timestamp();
                let failures = Failures::record(&this.backend.store, now);
                this.lock.error = Some(match failures.wait_until() {
                    Some(until) if until > now => {
                        this.retry_later(until - now, cx);
                        wait_message(until - now)
                    }
                    _ => t!("applock-that-isnt-the-passcode").into(),
                });
                Self::set(&this.lock.passcode, "", w, cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Clears the wait's message once it is over.
    fn retry_later(&self, seconds: i64, cx: &mut Context<Self>) {
        let wait = Duration::from_secs(seconds.max(0) as u64 + 1);
        // The message `wait_message(seconds)` set just before (compared
        // whole: its words are the reader's language).
        let shown = wait_message(seconds);
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(wait).await;
            let _ = this.update(cx, |this, cx| {
                if this
                    .lock
                    .error
                    .as_deref()
                    .is_some_and(|e| *e == shown)
                {
                    this.lock.error = None;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn unlock_with_biometrics(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        if !self.lock.locked || self.lock.authenticating || self.lock.biometrics().is_none() {
            return;
        }
        self.lock.authenticating = true;
        self.lock.error = None;
        let answer = biometric::authenticate(t!("applock-unlock-your-mail"));
        cx.spawn_in(w, async move |this, cx| {
            let answer = answer.await.unwrap_or(Err(Failure::Cancelled));
            let _ = this.update_in(cx, |this, w, cx| {
                this.lock.authenticating = false;
                this.lock.prompt_ended = Some(Instant::now());
                match answer {
                    Ok(()) => this.unlocked(w, cx),
                    Err(Failure::Cancelled) => {
                        this.lock.passcode.read(cx).focus_handle(cx).focus(w, cx);
                    }
                    Err(Failure::Failed(why)) => {
                        this.lock.error = Some(why);
                        this.lock.passcode.read(cx).focus_handle(cx).focus(w, cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn set_hide_contents(&mut self, on: bool, cx: &mut Context<Self>) {
        let store = self.backend.store.clone();
        if let Err(e) = applock::set_hides_contents(&store, on) {
            self.notice(t!("applock-couldnt-save-the-setting", e = format!("{:#}", e)), true, cx);
            return;
        }
        self.lock.hide_contents = on;
        // Widgets show the change now, not at the next new mail.
        cx.background_spawn(async move {
            crate::manage::widgets::publish_force(&store);
        })
        .detach();
        cx.notify();
    }

    /// Tells the system what it may show of Picacho outside it: on Android
    /// 13 and later, no picture of it in Recents while the lock is on.
    pub(super) fn sync_platform_privacy(&self) {
        #[cfg(target_os = "android")]
        crate::android::set_recents_private(self.lock.settings.enabled());
        // iOS: the window is covered natively as Picacho leaves the screen.
        #[cfg(target_os = "ios")]
        crate::ios::set_privacy_cover(self.lock.settings.enabled());
    }

    /// The main window came to the front (`active`) or left it.
    pub(super) fn lock_activation(&mut self, active: bool, w: &mut Window, cx: &mut Context<Self>) {
        // A phone's app switcher pictures the screen as Picacho leaves it.
        if self.mobile.on && cfg!(any(target_os = "android", target_os = "ios")) {
            let obscured = !active && self.lock.settings.enabled();
            if obscured != self.lock.obscured {
                self.lock.obscured = obscured;
                cx.notify();
            }
        }
        if !self.lock.settings.enabled() || self.lock.authenticating {
            return;
        }
        if active {
            if let Some(since) = self.lock.away_since.take()
                && self.lock.settings.locks_after(since.elapsed())
            {
                self.lock_now(w, cx);
            } else if self.lock.locked {
                self.lock_appeared(w, cx);
            }
            return;
        }
        if self.lock.locked || self.lock.away_since.is_some() {
            return;
        }
        self.lock.away_turn += 1;
        let turn = self.lock.away_turn;
        // A phone has only this window. On a computer, focus may have gone
        // to another of Picacho's windows: away only if none is in front.
        let check = if cfg!(any(target_os = "android", target_os = "ios")) {
            Duration::ZERO
        } else {
            AWAY_CHECK
        };
        cx.spawn_in(w, async move |this, cx| {
            cx.background_executor().timer(check).await;
            let _ = this.update_in(cx, |this, w, cx| {
                let phone = cfg!(any(target_os = "android", target_os = "ios"));
                if this.lock.away_turn != turn
                    || this.lock.locked
                    || w.is_window_active()
                    || (!phone && cx.active_window().is_some())
                {
                    return;
                }
                if this.lock.settings.after == 0 {
                    this.lock_now(w, cx);
                    return;
                }
                let since = Instant::now();
                this.lock.away_since = Some(since);
                // Locks while still away once the time is up, whichever
                // window comes back first (a phone's timers wait while it
                // sleeps; coming back checks too).
                let after = Duration::from_secs(this.lock.settings.after);
                cx.spawn_in(w, async move |this, cx| {
                    cx.background_executor().timer(after).await;
                    let _ = this.update_in(cx, |this, w, cx| {
                        if this.lock.away_since == Some(since) && !w.is_window_active() {
                            this.lock_now(w, cx);
                        }
                    });
                })
                .detach();
            });
        })
        .detach();
    }

    fn submit_lock_form(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        let Some(form) = self.lock.form else {
            return;
        };
        if self.lock.saving {
            return;
        }
        let current = zeroize::Zeroizing::new(Self::value(&self.lock.current, cx));
        let new_code = zeroize::Zeroizing::new(Self::value(&self.lock.new_code, cx));
        let confirm = zeroize::Zeroizing::new(Self::value(&self.lock.confirm, cx));
        if form != LockForm::Disable {
            if let Err(e) = applock::check_passcode(&new_code) {
                self.lock.form_error = Some(e.to_string());
                return;
            }
            if *new_code != *confirm {
                self.lock.form_error = Some(t!("applock-the-two-new-passcodes-arent-the").into());
                return;
            }
        }
        if form != LockForm::Enable && current.is_empty() {
            self.lock.form_error = Some(t!("applock-enter-the-current-passcode").into());
            return;
        }
        self.lock.saving = true;
        self.lock.form_error = None;
        let before = self.lock.settings.clone();
        let biometrics = self.lock.biometry.is_some();
        let store = self.backend.store.clone();
        let work = cx.background_spawn(async move {
            if form != LockForm::Enable && !before.verify(&current) {
                anyhow::bail!(t!("applock-that-isnt-the-current-passcode"));
            }
            let settings = match form {
                LockForm::Disable => LockSettings {
                    after: before.after,
                    ..LockSettings::default()
                },
                LockForm::Enable => LockSettings {
                    passcode: Some(PasscodeHash::new(&new_code)?),
                    // Offered from the start where the device has them.
                    biometrics,
                    after: before.after,
                },
                LockForm::Change => LockSettings {
                    passcode: Some(PasscodeHash::new(&new_code)?),
                    ..before
                },
            };
            // Saved here too, off the UI thread.
            settings.save(&store)?;
            Ok(settings)
        });
        cx.spawn_in(w, async move |this, cx| {
            let result = work.await;
            let _ = this.update_in(cx, |this, w, cx| {
                this.lock.saving = false;
                match result {
                    Ok(settings) => {
                        this.lock.settings = settings;
                        this.sync_platform_privacy();
                        // A lock that notifications and widgets give away
                        // locks little: they stop showing mail too (one
                        // switch away).
                        if form == LockForm::Enable && !this.lock.hide_contents {
                            this.set_hide_contents(true, cx);
                        }
                        this.lock_action(LockAction::Form(None), w, cx);
                        this.notice(
                            match form {
                                LockForm::Enable => t!("applock-picacho-will-ask-for-its-passcode"),
                                LockForm::Change => t!("applock-the-passcode-was-changed"),
                                LockForm::Disable => t!("applock-the-app-lock-is-off"),
                            },
                            false,
                            cx,
                        );
                    }
                    Err(e) => this.lock.form_error = Some(format!("{e:#}")),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn set_database_encrypted(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.lock.encrypting {
            return;
        }
        self.lock.encrypting = true;
        self.lock.confirm_encrypt = None;
        self.lock.encryption_error = None;
        // Its own thread rather than the executor's: copying a large
        // database takes a while, and holds the cache meanwhile.
        let backend = self.backend.clone();
        let (tx, rx) = futures::channel::oneshot::channel();
        let started = std::thread::Builder::new()
            .name(t!("applock-encrypt-database").into())
            .spawn(move || {
                let result = backend.store.set_encrypted(on);
                // Decrypted copies of opened attachments lie outside the
                // database: gone, written again when opened. Also when the
                // database is encrypted but its draft files haven't all
                // followed yet (an error, retried later).
                if on && backend.store.encrypted() {
                    backend.forget_opened_attachments();
                }
                let _ = tx.send(result);
            });
        if let Err(e) = started {
            self.lock.encrypting = false;
            self.lock.encryption_error = Some(e.to_string());
            return;
        }
        cx.spawn(async move |this, cx| {
            let result = rx
                .await
                .unwrap_or_else(|_| Err(anyhow::anyhow!(t!("applock-encrypting-stopped-part-way"))));
            let _ = this.update(cx, |this, cx| {
                this.lock.encrypting = false;
                this.lock.encrypted = this.backend.store.encrypted();
                // The widgets' snapshot is a file outside the database.
                if on && this.lock.encrypted && !this.lock.hide_contents {
                    this.set_hide_contents(true, cx);
                }
                match result {
                    Ok(()) => this.notice(
                        if on {
                            t!("applock-the-local-database-is-encrypted")
                        } else {
                            t!("applock-the-local-database-is-no-longer")
                        },
                        false,
                        cx,
                    ),
                    Err(e) => this.lock.encryption_error = Some(format!("{e:#}")),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// A phone's screen while Picacho is out of front with the lock on: no
    /// mail for the app switcher's picture.
    pub(super) fn privacy_cover(&self, window: &mut Window) -> Stateful<Div> {
        let c = self.colors();
        div()
            .id("privacy-cover")
            .debug_selector(|| "privacy-cover".into())
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgb(c.bg))
            .child(icon_el("logo", 44., c.accent))
            .map(|d| super::window::window_frame(d, c, window))
    }

    /// The lock screen, in place of the whole window.
    pub(super) fn lock_screen(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let c = self.colors();
        let phone = self.mobile.on;
        let insets = phone.then(|| self.mobile_insets(window));
        let biometrics = self.lock.biometrics();
        let busy = self.lock.checking || self.lock.authenticating;
        let mut column = div()
            .w_full()
            .max_w(px(320.))
            .px_4()
            .flex()
            .flex_col()
            .items_center()
            .gap_3()
            .child(icon_el("logo", 44., c.accent))
            .child(
                div()
                    .mt_2()
                    .text_size(px(20.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(c.text))
                    .child(t!("applock-picacho-is-locked")),
            )
            .child(
                div()
                    .text_size(px(13.))
                    .text_color(rgb(c.muted))
                    .text_center()
                    .child(match biometrics {
                        Some(b) => t!("applock-use-or-enter-your-passcode", label = b.label()),
                        None => t!("applock-enter-your-passcode").into(),
                    }),
            )
            .child(
                div()
                    .w_full()
                    .mt_2()
                    .child(field(Input::new(&self.lock.passcode), c, false)),
            )
            .when_some(self.lock.error.clone(), |d, error| {
                d.child(
                    div()
                        .id("lock-error")
                        .debug_selector(|| "lock-error".into())
                        .role(Role::Alert)
                        .text_size(px(12.))
                        .text_color(rgb(c.danger))
                        .text_center()
                        .child(error),
                )
            })
            .child(
                self.button(
                    "unlock",
                    if self.lock.checking {
                        t!("applock-checking")
                    } else {
                        t!("applock-unlock")
                    },
                    "",
                    Action::Lock(LockAction::Unlock),
                    true,
                    cx,
                )
                .w_full()
                .when(busy, |d| d.opacity(0.6)),
            );
        if let Some(b) = biometrics {
            column = column.child(
                self.button(
                    "unlock-biometrics",
                    &t!("applock-use", title = b.title()),
                    b.icon(),
                    Action::Lock(LockAction::UseBiometrics),
                    false,
                    cx,
                )
                .w_full()
                .border_1()
                .border_color(rgb(c.border)),
            );
        }
        div()
            .id("lock-screen")
            .debug_selector(|| "lock-screen".into())
            .key_context("Locked")
            .track_focus(&self.focus)
            .map(|d| super::window::window_frame(d, c, window))
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .bg(rgb(c.bg))
            .text_color(rgb(c.text))
            .font_family(".SystemUIFont")
            .text_size(px(14.))
            .when_some(insets, |d, i| {
                d.pt(px(i.top))
                    .pb(px(i.bottom.max(i.keyboard)))
                    .pl(px(i.left))
                    .pr(px(i.right))
            })
            .on_action(cx.listener(|this, _: &Quit, _, cx| this.quit_app(cx)))
            .on_action(|_: &Minimize, w, _| w.minimize_window())
            .child(column)
    }

    /// Settings → Privacy.
    pub(super) fn privacy_page(&self, cx: &Context<Self>) -> AnyElement {
        let c = self.colors();
        let lock = &self.lock;
        let on = lock.settings.enabled();
        let caption = |text: &str| {
            div()
                .text_size(px(12.))
                .text_color(rgb(c.muted))
                .child(text.to_owned())
        };
        let unlocks = match lock.biometry {
            Some(b) => t!("applock-your-passcode-or", label = b.label()),
            None => t!("applock-your-passcode").into(),
        };
        let mut app_lock = Rows::new().row(self.switch_setting(
            t!("applock-lock-picacho"),
            &t!("applock-ask-for-when-picacho-opens-and", unlocks = unlocks),
            "app-lock".into(),
            on,
            Action::Lock(LockAction::Form(Some(if on { LockForm::Disable } else { LockForm::Enable }))),
            cx,
        ));
        if matches!(lock.form, Some(LockForm::Enable | LockForm::Disable)) {
            app_lock = app_lock.row(self.lock_form(cx));
        }
        if on {
            if let Some(b) = lock.biometry {
                app_lock = app_lock.row(self.switch_setting(
                    &t!("applock-unlock-with", title = b.title()),
                    "",
                    "app-lock-biometrics".into(),
                    lock.settings.biometrics,
                    Action::Lock(LockAction::SetBiometrics(!lock.settings.biometrics)),
                    cx,
                ));
            } else if cfg!(any(
                target_os = "macos",
                target_os = "ios",
                target_os = "android"
            )) {
                app_lock = app_lock.row(caption(t!("applock-set-up-touch-id-face-id")));
            }
            app_lock = app_lock
                .row(self.setting(
                    t!("applock-require-the-passcode"),
                    "",
                    self.segmented(
                        "app-lock-after",
                        applock::AFTER_CHOICES
                            .iter()
                            .map(|(after, label)| {
                                (
                                    (*label).to_owned(),
                                    Action::Lock(LockAction::SetAfter(*after)),
                                    lock.settings.after == *after,
                                )
                            })
                            .collect(),
                        cx,
                    ),
                ))
                .row(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap_2()
                        .child(
                            self.button(
                                "app-lock-change",
                                t!("applock-change-passcode"),
                                "",
                                Action::Lock(LockAction::Form(Some(LockForm::Change))),
                                false,
                                cx,
                            )
                            .border_1()
                            .border_color(rgb(c.border)),
                        )
                        .child(
                            self.button(
                                "app-lock-now",
                                t!("applock-lock-now"),
                                "lock",
                                Action::Lock(LockAction::LockNow),
                                false,
                                cx,
                            )
                            .border_1()
                            .border_color(rgb(c.border)),
                        ),
                );
            if lock.form == Some(LockForm::Change) {
                app_lock = app_lock.row(self.lock_form(cx));
            }
        }
        let outside = Rows::new().row(self.switch_setting(
            t!("applock-hide-mail-outside-picacho"),
            t!("applock-notifications-and-widgets-say-how-much"),
            "hide-contents".into(),
            lock.hide_contents,
            Action::Lock(LockAction::SetHideContents(!lock.hide_contents)),
            cx,
        ));
        let encrypted = lock.encrypted;
        let mut database = Rows::new().row(self.switch_setting(
            t!("applock-encrypt-the-local-database"),
            t!("applock-mail-drafts-contacts-calendars-and-settings"),
            "encrypt-database".into(),
            encrypted,
            Action::Lock(LockAction::AskEncrypt(Some(!encrypted))),
            cx,
        ));
        if lock.encrypting {
            database = database.row(div().text_size(px(12.)).text_color(rgb(c.accent)).child(
                if encrypted {
                    t!("applock-decrypting-the-local-database")
                } else {
                    t!("applock-encrypting-the-local-database")
                },
            ));
        }
        if let Some(on) = lock.confirm_encrypt {
            let (text, label) = if on {
                (
                    t!("applock-encrypting-copies-the-whole-database-which"),
                    t!("applock-encrypt"),
                )
            } else {
                (
                    t!("applock-the-database-will-be-stored-without"),
                    t!("applock-decrypt"),
                )
            };
            database = database.row(
                div()
                    .id("encrypt-database-confirm")
                    .debug_selector(|| "encrypt-database-confirm".into())
                    .p_3()
                    .rounded(px(10.))
                    .border_1()
                    .border_color(rgb(c.warning))
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(caption(text))
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap_2()
                            .child(self.button(
                                "encrypt-database-cancel",
                                t!("applock-cancel"),
                                "",
                                Action::Lock(LockAction::AskEncrypt(None)),
                                false,
                                cx,
                            ))
                            .child(self.button(
                                "encrypt-database-go",
                                label,
                                "lock",
                                Action::Lock(LockAction::SetEncrypted(on)),
                                true,
                                cx,
                            )),
                    ),
            );
        }
        if let Some(error) = &lock.encryption_error {
            database = database.row(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(c.danger))
                    .child(error.clone()),
            );
        }
        self.settings_column()
            .child(self.settings_group(t!("applock-app-lock"), app_lock))
            .child(self.settings_group(t!("applock-notifications-widgets"), outside))
            .child(self.settings_group(t!("applock-local-database"), database))
            .into_any_element()
    }

    /// The form that turns the lock on or off, or changes its passcode.
    fn lock_form(&self, cx: &Context<Self>) -> Stateful<Div> {
        let c = self.colors();
        let lock = &self.lock;
        let Some(form) = lock.form else {
            return div().id("app-lock-form");
        };
        let (title, submit) = match form {
            LockForm::Enable => (
                t!("applock-choose-a-passcode-of-at-least"),
                t!("applock-turn-on"),
            ),
            LockForm::Change => (
                t!("applock-enter-the-current-passcode-then-the"),
                t!("applock-change-passcode-2"),
            ),
            LockForm::Disable => (t!("applock-enter-the-passcode-to-turn-the"), t!("applock-turn-off")),
        };
        div()
            .id("app-lock-form")
            .debug_selector(|| "app-lock-form".into())
            .max_w(px(360.))
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(c.muted))
                    .child(title),
            )
            .when(form != LockForm::Enable, |d| {
                d.child(field(Input::new(&lock.current), c, false))
            })
            .when(form != LockForm::Disable, |d| {
                d.child(field(Input::new(&lock.new_code), c, false))
                    .child(field(Input::new(&lock.confirm), c, false))
            })
            .when_some(lock.form_error.clone(), |d, e| {
                d.child(div().text_size(px(12.)).text_color(rgb(c.danger)).child(e))
            })
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .child(self.button(
                        "app-lock-cancel",
                        t!("applock-cancel"),
                        "",
                        Action::Lock(LockAction::Form(None)),
                        false,
                        cx,
                    ))
                    .child(
                        self.button(
                            "app-lock-submit",
                            if lock.saving { t!("applock-saving") } else { submit },
                            "",
                            Action::Lock(LockAction::SubmitForm),
                            true,
                            cx,
                        )
                        .when(lock.saving, |d| d.opacity(0.6)),
                    ),
            )
    }
}

#[cfg(test)]
impl MailApp {
    pub(super) fn lock_error(&self) -> Option<&str> {
        self.lock.error.as_deref()
    }
    pub(super) fn lock_form_error(&self) -> Option<&str> {
        self.lock.form_error.as_deref()
    }
    /// As if Picacho had left the front `away` ago.
    pub(super) fn lock_away_for(&mut self, away: Duration) {
        self.lock.away_since = Instant::now().checked_sub(away);
    }
}

fn wait_message(seconds: i64) -> String {
    let wait = if seconds >= 120 {
        t!("applock-minutes", minutes = (seconds + 59) / 60)
    } else {
        t!("applock-seconds", seconds = seconds)
    };
    t!("applock-too-many-wrong-passcodes-try-again", wait = wait)
}

//! Proton Mail sign-in, one step at a time, for adding an account and for
//! signing in to one again: the password first, then (where the account
//! has them) a second factor (an authenticator app's code, a security key
//! or a passkey) and the mailbox password. The backend keeps the sign-in
//! between steps and says which comes next (`ProtonNext`).
//!
//! "Proton Mail" in the service list has two tabs: Direct (Proton's API,
//! these steps) and Bridge (Proton Mail Bridge's password).
use super::*;
use crate::manage::backend::{KeyMethod, KeyPrompt, NeedsPin, ProtonNext};

/// The step of a Proton sign-in after the password.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ProtonStage {
    /// Proton wants a second factor: a code (`code`), and/or a security key
    /// or passkey (`key`); `pin` once the key asked for its PIN.
    SecondFactor { code: bool, key: bool, pin: bool },
    /// The mailbox password, in two-password mode.
    MailboxPassword,
}

impl MailApp {
    /// Continues a Proton sign-in with what its step asks for: the
    /// password, the code, or the mailbox password. `reconnect` names the
    /// account when signing in to it again.
    pub(super) fn proton_continue(&mut self, reconnect: Option<String>, cx: &mut Context<Self>) {
        if self.connecting {
            return;
        }
        let value = |i: usize, cx: &Context<Self>| Self::value(&self.form[i], cx);
        match self.proton_stage {
            None => {
                let password = value(PASSWORD, cx);
                let (account, taken) = match &reconnect {
                    Some(id) => {
                        let Some(a) = self.accounts.iter().find(|a| &a.id == id).cloned() else {
                            return;
                        };
                        (a, Vec::new())
                    }
                    None => {
                        let a = self.setup_account(cx);
                        if let Err(e) = a.validate() {
                            self.notice(e.to_string(), true, cx);
                            return;
                        }
                        let taken: Vec<String> =
                            self.accounts.iter().map(|a| a.email.clone()).collect();
                        if taken.iter().any(|e| e.eq_ignore_ascii_case(&a.email)) {
                            self.notice(t!("proton-login-this-account-is-already-added"), true, cx);
                            return;
                        }
                        (a, taken)
                    }
                };
                if password.is_empty() {
                    self.notice(t!("proton-login-enter-your-proton-password"), true, cx);
                    return;
                }
                self.start_proton(reconnect, cx, move |b, cancel| {
                    b.proton_begin(&account, password, &taken, cancel)
                });
            }
            Some(ProtonStage::SecondFactor { code, key, .. }) => {
                let typed = value(TOTP, cx);
                if !code {
                    return;
                }
                if typed.trim().is_empty() {
                    self.notice(
                        if key {
                            t!("proton-login-enter-the-code-from-your-authenticator")
                        } else {
                            t!("proton-login-enter-the-code-from-your-authenticator-2")
                        },
                        true,
                        cx,
                    );
                    return;
                }
                self.start_proton(reconnect, cx, move |b, cancel| b.proton_code(typed, cancel));
            }
            Some(ProtonStage::MailboxPassword) => {
                let password = value(MAILBOX_PASSWORD, cx);
                if password.is_empty() {
                    self.notice(t!("proton-login-enter-the-mailbox-password"), true, cx);
                    return;
                }
                self.start_proton(reconnect, cx, move |b, cancel| {
                    b.proton_mailbox(password, cancel)
                });
            }
        }
    }
    /// Signs in with a security key or passkey.
    pub(super) fn proton_use_key(
        &mut self,
        method: KeyMethod,
        reconnect: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if self.connecting {
            return;
        }
        let pin = match self.proton_stage {
            Some(ProtonStage::SecondFactor { key: true, pin, .. }) => {
                pin.then(|| Self::value(&self.form[KEY_PIN], cx))
            }
            _ => return,
        };
        self.key_method = Some(method);
        self.start_proton(reconnect, cx, move |b, cancel| {
            b.proton_key(method, pin, cancel)
        });
    }
    /// Back to the password: the sign-in in progress is given up.
    pub(super) fn proton_back(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        self.drop_proton_login(cx);
        for i in [TOTP, MAILBOX_PASSWORD, KEY_PIN] {
            Self::set(&self.form[i], "", w, cx);
        }
        self.status.clear();
        self.error = false;
        self.anim += 1;
        self.form[PASSWORD].update(cx, |s, cx| s.focus(w, cx));
    }
    /// Forgets the Proton sign-in in progress, if any, and signs its
    /// session out in the background.
    pub(super) fn drop_proton_login(&mut self, cx: &mut Context<Self>) {
        if self.connecting && self.proton_stage.is_some() {
            self.cancel.store(true, Ordering::Relaxed);
            self.connecting = false;
        }
        self.key_method = None;
        self.key_prompt = KeyPrompt::Idle;
        // A Proton Pass sign-in (Forwarding settings) ends with it.
        self.fwd_pass_login = None;
        self.fwd_pass_again = None;
        self.fwd_pending_extra = Default::default();
        if self.proton_stage.take().is_some() {
            self.task(
                cx,
                |b| {
                    b.proton_abandon();
                    Ok(())
                },
                |_, _, _| {},
            );
        }
    }
    /// The account a sign-in-again dialog is for, when it is a Proton one.
    pub(super) fn proton_reconnecting(&self) -> Option<String> {
        match &self.modal {
            Some(Modal::Reconnect(id))
                if self
                    .accounts
                    .iter()
                    .any(|a| &a.id == id && a.service == Service::Proton) =>
            {
                Some(id.clone())
            }
            _ => None,
        }
    }
    pub(super) fn start_proton(
        &mut self,
        reconnect: Option<String>,
        cx: &mut Context<Self>,
        job: impl FnOnce(&Backend, Arc<AtomicBool>) -> anyhow::Result<ProtonNext> + Send + 'static,
    ) {
        self.connecting = true;
        self.sign_in_retry = false;
        self.cancel = Arc::new(AtomicBool::new(false));
        let cancel = self.cancel.clone();
        let cancelled = cancel.clone();
        self.status.clear();
        self.error = false;
        if let Some(id) = &reconnect {
            self.retrying.insert(id.clone());
        }
        self.sign_in_step = SignInStep::Mailbox;
        self.follow_sign_in(cx);
        self.task(
            cx,
            move |b| job(&b, cancel),
            move |this, result, cx| {
                if let Some(id) = &reconnect {
                    this.retrying.remove(id);
                }
                if !Arc::ptr_eq(&this.cancel, &cancelled) || cancelled.load(Ordering::Relaxed) {
                    return;
                }
                this.connecting = false;
                this.key_method = None;
                this.key_prompt = KeyPrompt::Idle;
                match result {
                    Ok(ProtonNext::SecondFactor { code, key }) => {
                        this.proton_stage = Some(ProtonStage::SecondFactor {
                            code,
                            key,
                            pin: false,
                        });
                        this.anim += 1;
                    }
                    Ok(ProtonNext::MailboxPassword) => {
                        this.proton_stage = Some(ProtonStage::MailboxPassword);
                        this.anim += 1;
                    }
                    Ok(ProtonNext::PassSignedIn) => {
                        this.proton_stage = None;
                        this.anim += 1;
                        match this.fwd_pass_again.take() {
                            Some(service) => this.pass_signed_in_again(service, cx),
                            None => this.connect_proton_aliases(cx),
                        }
                    }
                    Ok(ProtonNext::Done(account)) => {
                        this.proton_stage = None;
                        match reconnect {
                            Some(id) => this.reconnected(id, *account, false, cx),
                            None => this.connected(*account, false, cx),
                        }
                    }
                    Err(e) => {
                        if e.downcast_ref::<NeedsPin>().is_some()
                            && let Some(ProtonStage::SecondFactor { pin, .. }) =
                                &mut this.proton_stage
                        {
                            *pin = true;
                        }
                        this.notice(format!("{e:#}"), true, cx);
                    }
                }
                cx.notify();
            },
        );
    }
    /// The main button's label for the Proton step on screen, or `None`
    /// when the step has only its own choices (a security key, no code).
    pub(super) fn proton_submit_label(&self) -> Option<&'static str> {
        if self.connecting {
            return Some(match (self.key_method, self.sign_in_step) {
                (Some(_), _) => t!("proton-login-waiting-for-your-key"),
                (None, SignInStep::Verifying) => t!("proton-login-waiting-for-protons-check"),
                _ => t!("proton-login-signing-in"),
            });
        }
        match self.proton_stage {
            None => Some(t!("proton-login-continue")),
            Some(ProtonStage::SecondFactor { code: true, .. }) => Some(t!("proton-login-verify")),
            Some(ProtonStage::SecondFactor { code: false, .. }) => None,
            Some(ProtonStage::MailboxPassword) => Some(t!("proton-login-unlock")),
        }
    }
    /// The title and explanation of the Proton step on screen.
    pub(super) fn proton_heading(&self) -> Option<(&'static str, &'static str)> {
        Some(match self.proton_stage? {
            ProtonStage::SecondFactor {
                code: true,
                key: true,
                ..
            } => (
                t!("proton-login-two-factor-authentication"),
                t!("proton-login-enter-the-code-from-your-authenticator"),
            ),
            ProtonStage::SecondFactor { code: true, .. } => (
                t!("proton-login-two-factor-authentication"),
                t!("proton-login-enter-the-code-from-your-authenticator-2"),
            ),
            ProtonStage::SecondFactor { key: true, .. } => (
                t!("proton-login-two-factor-authentication"),
                t!("proton-login-use-the-security-key-or-passkey"),
            ),
            ProtonStage::SecondFactor { .. } => (
                t!("proton-login-two-factor-authentication"),
                t!("proton-login-proton-asks-for-a-second-factor"),
            ),
            ProtonStage::MailboxPassword => (
                t!("proton-login-mailbox-password"),
                t!("proton-login-this-account-uses-a-second-password"),
            ),
        })
    }
    /// Direct (Proton's API) or Bridge, at the top of Proton's sign-in.
    pub(super) fn proton_tabs(&self, cx: &Context<Self>) -> Div {
        self.segmented(
            "proton-mode",
            vec![
                (
                    t!("proton-login-direct").to_owned(),
                    Action::ProtonMode(Service::Proton),
                    self.service == Service::Proton,
                ),
                (
                    t!("proton-login-proton-mail-bridge").to_owned(),
                    Action::ProtonMode(Service::ProtonBridge),
                    self.service == Service::ProtonBridge,
                ),
            ],
            cx,
        )
    }
    /// The fields and choices of the Proton step after the password. With
    /// `heading`, its title and explanation too (the setup card shows them
    /// as its own title instead). Back is the setup's own "Back" link, or
    /// a button in the sign-in-again dialog's footer.
    pub(super) fn proton_step_view(&self, heading: bool, cx: &Context<Self>) -> Option<AnyElement> {
        let stage = self.proton_stage?;
        let c = self.colors();
        let mut content = div().w_full().flex().flex_col().gap_4();
        if let Some((title, about)) = self.proton_heading().filter(|_| heading) {
            content = content.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_size(px(15.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(title),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(rgb(c.muted))
                            .child(about),
                    ),
            );
        }
        match stage {
            ProtonStage::SecondFactor { code, key, pin } => {
                if code {
                    content = content.child(self.field(t!("proton-login-authentication-code"), TOTP));
                }
                if code && key {
                    content = content.child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .text_size(px(11.))
                            .text_color(rgb(c.muted))
                            .child(div().h(px(1.)).flex_1().bg(rgb(c.border)))
                            .child("or")
                            .child(div().h(px(1.)).flex_1().bg(rgb(c.border))),
                    );
                }
                if key {
                    if pin {
                        content = content.child(self.field(t!("proton-login-security-key-pin"), KEY_PIN));
                    }
                    content = content.children(KeyMethod::offered().iter().map(|m| {
                        let busy = self.connecting && self.key_method == Some(*m);
                        self.button(
                            format!("proton-key-{m:?}"),
                            m.label(),
                            "lock",
                            Action::ProtonKey(*m),
                            !code,
                            cx,
                        )
                        .w_full()
                        .when(self.connecting && !busy, |d| d.opacity(0.5))
                    }));
                    if self.connecting && self.key_method.is_some() {
                        content = content.child(self.key_prompt_view());
                    }
                }
            }
            ProtonStage::MailboxPassword => {
                content = content.child(self.field(t!("proton-login-mailbox-password"), MAILBOX_PASSWORD));
            }
        }
        Some(content.into_any_element())
    }
    /// What the security key or phone is waiting for.
    fn key_prompt_view(&self) -> AnyElement {
        let c = self.colors();
        let line = |text: &str| {
            div()
                .flex()
                .items_center()
                .gap_2()
                .text_size(px(13.))
                .text_color(rgb(c.muted))
                .child(spinner(16., c.accent))
                .child(text.to_owned())
                .into_any_element()
        };
        match &self.key_prompt {
            KeyPrompt::Qr(url) => div()
                .w_full()
                .flex()
                .flex_col()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .p(px(10.))
                        .rounded(px(8.))
                        .bg(rgb(0xffffff))
                        .child(qr_code(url, 200.)),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(rgb(c.muted))
                        .text_center()
                        .child(if cfg!(target_os = "ios") {
                            t!("proton-login-scan-this-with-the-camera-of")
                        } else {
                            t!("proton-login-scan-this-with-your-phones-camera")
                        }),
                )
                .into_any_element(),
            KeyPrompt::Touch => line(t!("proton-login-touch-your-security-key")),
            KeyPrompt::Insert if cfg!(target_os = "android") => {
                line(t!("proton-login-hold-your-security-key-against-the"))
            }
            KeyPrompt::Insert if cfg!(target_os = "ios") => {
                line(t!("proton-login-hold-your-security-key-near-the"))
            }
            KeyPrompt::Insert => line(t!("proton-login-plug-in-your-security-key")),
            KeyPrompt::Phone(status) => line(status),
            KeyPrompt::Working | KeyPrompt::Idle => line(t!("proton-login-waiting-for-your-key")),
        }
    }
}

/// A QR code, black on white, `size` wide.
pub(super) fn qr_code(text: &str, size: f32) -> AnyElement {
    // Made once for its text, not on every frame (encoding a QR code is
    // the most work a frame of these sheets does).
    /// A code's width and which of its modules are dark.
    type Code = (usize, std::rc::Rc<Vec<bool>>);
    thread_local! {
        static MADE: std::cell::RefCell<Option<(String, Option<Code>)>> =
            const { std::cell::RefCell::new(None) };
    }
    let made = MADE.with(|made| {
        let mut made = made.borrow_mut();
        if made.as_ref().is_none_or(|(t, _)| t != text) {
            let code = qrcode::QrCode::new(text.as_bytes()).ok().map(|code| {
                let dark: Vec<bool> = code
                    .to_colors()
                    .into_iter()
                    .map(|c| c == qrcode::Color::Dark)
                    .collect();
                (code.width(), std::rc::Rc::new(dark))
            });
            *made = Some((text.to_owned(), code));
        }
        made.as_ref().and_then(|(_, code)| code.clone())
    });
    let Some((width, dark)) = made else {
        return div()
            .child(t!("proton-login-the-qr-code-couldnt-be-made"))
            .into_any_element();
    };
    canvas(
        |_, _, _| {},
        move |bounds, _, w, _| {
            let module = bounds.size.width / width as f32;
            for (i, _) in dark.iter().enumerate().filter(|(_, d)| **d) {
                let (x, y) = ((i % width) as f32, (i / width) as f32);
                w.paint_quad(gpui::fill(
                    gpui::Bounds::new(
                        point(bounds.origin.x + module * x, bounds.origin.y + module * y),
                        gpui::size(module, module),
                    ),
                    rgb(0x000000),
                ));
            }
        },
    )
    .size(px(size))
    .into_any_element()
}

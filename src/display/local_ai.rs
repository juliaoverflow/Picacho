//! Local AI in the app: the Settings page that turns it and its features on
//! (downloading their models) and off (deleting them), and hiding the
//! sign-in codes the model finds wherever mail is shown.
use super::*;
use crate::manage::local_ai::{self, otp};
use std::borrow::Cow;

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
        Some(
            div()
                .id("codes-banner")
                .debug_selector(|| "codes-banner".into())
                .mb_4()
                .px_3()
                .py_2()
                .flex()
                .items_center()
                .gap_2()
                .rounded(px(8.))
                .bg(rgb(c.soft))
                .text_size(px(12.))
                .child(icon_el("lock", 13., c.accent))
                .child(div().flex_1().min_w_0().child(if shown {
                    "This message's sign-in code is shown."
                } else {
                    "This message's sign-in code is hidden."
                }))
                .child(self.link(
                    "codes-toggle",
                    if shown { "Hide Code" } else { "Show Code" },
                    Action::ShowCodes(m.id.clone(), !shown),
                    cx,
                ))
                .into_any_element(),
        )
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

    /// Loads the installed sign-in code model at launch, or installs it if
    /// it should be there and isn't (deleted outside Picacho).
    pub(super) fn start_local_ai(&mut self, cx: &mut Context<Self>) {
        if !self.hide_codes {
            return;
        }
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
        cx.notify();
        self.task(
            cx,
            |b| local_ai::OTP.install(&b.store),
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
            self.model_error = None;
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
            self.start_local_ai(cx);
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

    pub(super) fn local_ai_page(&self, cx: &Context<Self>) -> AnyElement {
        let c = self.colors();
        let model = &local_ai::OTP;
        let mb = |bytes: u64| format!("{:.1} MB", bytes as f64 / 1_000_000.);
        let status: Option<(String, bool)> = if self.model_busy {
            Some((
                format!(
                    "Downloading the {} ({})…",
                    model.label.to_lowercase(),
                    mb(model.size)
                ),
                false,
            ))
        } else if let Some(e) = &self.model_error {
            Some((e.clone(), true))
        } else if self.hide_codes && self.otp.is_some() {
            Some((
                format!(
                    "{} installed ({}). It runs on this computer; mail isn't sent anywhere.",
                    model.label,
                    mb(local_ai::installed_size(&self.backend.store))
                ),
                false,
            ))
        } else {
            None
        };
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
                            "Find two-factor, verification and sign-in codes in received mail, and \
                             show them as ●●●●●● in subjects, message text and notifications. \
                             Downloads a {} model; turning this off deletes it.",
                            mb(model.size)
                        ),
                        "local-ai-otp".into(),
                        self.hide_codes,
                        // Needs Local AI on first.
                        if self.local_ai {
                            Action::SetHideCodes(!self.hide_codes)
                        } else {
                            Action::SetHideCodes(false)
                        },
                        cx,
                    ))
                    .when_some(status, |d, (status, error)| {
                        d.child(
                            div()
                                .id("local-ai-status")
                                .debug_selector(|| "local-ai-status".into())
                                .flex()
                                .items_center()
                                .gap_2()
                                .text_size(px(12.))
                                .text_color(rgb(if error { c.danger } else { c.muted }))
                                .when(self.model_busy, |d| d.child(spinner(12., c.muted)))
                                .child(div().flex_1().min_w_0().child(status))
                                .when(error && self.local_ai, |d| {
                                    d.child(self.link(
                                        "local-ai-retry",
                                        "Try Again",
                                        Action::SetHideCodes(true),
                                        cx,
                                    ))
                                }),
                        )
                    }),
            )
            .into_any_element()
    }
}

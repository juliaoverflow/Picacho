//! Security keys (`manage/hardware.rs`): Settings → Encryption's list of
//! OpenPGP cards and PKCS#11 tokens, adding them, unlocking them with their
//! PIN, and the reader's offer to unlock one for a message encrypted to it.
use super::*;
use crate::manage::hardware::{Kind, SecurityKey};
use crate::manage::openpgp::card::CardInfo;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SecurityKeyAction {
    /// Looks for OpenPGP cards connected now.
    FindCards,
    /// Adds a connected card (its ident) as your OpenPGP key.
    AddCard(String),
    /// Adds the S/MIME identities on the tokens the module field names.
    AddToken,
    /// Unlocks a key (its PIN's id) with the PIN field.
    Unlock(String),
    Lock(String),
    Remove(String),
}

/// What Settings and the reader show of security keys.
#[derive(Default)]
pub(super) struct SecurityKeys {
    /// Cards found connected, or why none could be looked for.
    pub cards: Option<Result<Vec<CardInfo>, String>>,
    pub busy: bool,
    pub error: Option<String>,
    /// The open message, when it is encrypted to a locked security key.
    pub needed: Option<String>,
}

impl MailApp {
    /// Settings → Encryption's Security keys section.
    pub(super) fn security_keys_section(&self, cx: &Context<Self>) -> AnyElement {
        let c = self.colors();
        let keys = self.backend.security_keys();
        let busy = self.hw.busy;
        let list = if keys.is_empty() {
            div()
                .px_4()
                .py_3()
                .text_size(px(12.))
                .text_color(rgb(c.muted))
                .child(t!("security-keys-none-yet"))
                .into_any_element()
        } else {
            div()
                .id("security-keys")
                .flex()
                .flex_col()
                .role(Role::List)
                .aria_label(t!("security-keys-title"))
                .children(keys.iter().map(|k| self.security_key_row(k, cx)))
                .into_any_element()
        };
        let pin = div()
            .id("security-key-pin")
            .debug_selector(|| "security-key-pin".into())
            .w(px(220.))
            .max_w_full()
            .child(field(Input::new(&self.hw_pin), c, busy));
        let cards = match &self.hw.cards {
            None => None,
            Some(Err(e)) => Some(div().text_size(px(12.)).text_color(rgb(c.danger)).child(e.clone()).into_any_element()),
            Some(Ok(found)) if found.is_empty() => Some(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(c.muted))
                    .child(t!("security-keys-no-new-cards"))
                    .into_any_element(),
            ),
            Some(Ok(found)) => Some(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .children(found.iter().map(|card| {
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap_2()
                            .child(icon_el("lock", 14., c.muted))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_size(px(12.))
                                    .child(card.name.clone())
                                    .child(
                                        div()
                                            .text_size(px(11.))
                                            .text_color(rgb(c.muted))
                                            .child(match card.tries {
                                                Some(tries) => t!("security-keys-card-facts", ident = card.ident.clone(), tries = tries),
                                                None => card.ident.clone(),
                                            }),
                                    ),
                            )
                            .child(self.button(
                                format!("security-key-add-{}", card.ident),
                                t!("security-keys-add"),
                                "plus",
                                Action::SecurityKey(SecurityKeyAction::AddCard(card.ident.clone())),
                                true,
                                cx,
                            ))
                    }))
                    .into_any_element(),
            ),
        };
        let openpgp = self.setting(
            t!("security-keys-openpgp-cards"),
            t!("security-keys-openpgp-cards-caption"),
            div()
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    self.button(
                        "security-keys-find",
                        if busy { t!("security-keys-looking") } else { t!("security-keys-find-cards") },
                        "search",
                        Action::SecurityKey(SecurityKeyAction::FindCards),
                        false,
                        cx,
                    )
                    .when(busy, |d| d.opacity(0.6)),
                )
                .children(cards),
        );
        let suggested = crate::manage::smime::token::suggested_modules();
        let pkcs11 = self.setting(
            t!("security-keys-pkcs11"),
            &if suggested.is_empty() {
                t!("security-keys-pkcs11-caption").to_owned()
            } else {
                t!("security-keys-pkcs11-found", modules = suggested.join(", "))
            },
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .id("security-key-module")
                        .w(px(320.))
                        .max_w_full()
                        .child(field(Input::new(&self.hw_module), c, busy)),
                )
                .child(self.button(
                    "security-keys-add-token",
                    t!("security-keys-add-from-token"),
                    "plus",
                    Action::SecurityKey(SecurityKeyAction::AddToken),
                    false,
                    cx,
                )),
        );
        let rows = Rows::new()
            .row(self.setting(t!("security-keys-pin"), t!("security-keys-pin-caption"), pin))
            .row(openpgp)
            .row(pkcs11);
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(self.section(t!("security-keys-title")))
            .child(
                div()
                    .rounded(px(12.))
                    .border_1()
                    .border_color(rgb(c.border))
                    .bg(rgb(c.raised))
                    .overflow_hidden()
                    .child(list),
            )
            .child(self.settings_group("", rows))
            .when_some(self.hw.error.clone(), |d, e| {
                d.child(
                    div()
                        .id("security-key-error")
                        .role(Role::Alert)
                        .text_size(px(12.))
                        .text_color(rgb(c.danger))
                        .child(e),
                )
            })
            .into_any_element()
    }

    fn security_key_row(&self, k: &SecurityKey, cx: &Context<Self>) -> impl IntoElement {
        let c = self.colors();
        let kind = match k.kind {
            Kind::OpenPgpCard => t!("security-keys-kind-openpgp"),
            Kind::Pkcs11 => t!("security-keys-kind-smime"),
        };
        let state = if k.unlocked { t!("security-keys-unlocked") } else { t!("security-keys-locked") };
        let id = k.id.clone();
        div()
            .id(SharedString::from(format!("security-key-{id}")))
            .role(Role::ListItem)
            .aria_label(format!("{}, {kind}, {state}", k.name))
            .flex()
            .flex_wrap()
            .items_center()
            .gap_3()
            .px_4()
            .py_3()
            .border_b_1()
            .border_color(rgb(c.border))
            .child(icon_el("lock", 16., if k.unlocked { c.accent } else { c.muted }))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(div().text_size(px(13.)).font_weight(FontWeight::MEDIUM).truncate().child(k.name.clone()))
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(rgb(c.muted))
                            .truncate()
                            .child([kind.to_owned(), state.to_owned(), k.addresses.join(", ")].iter().filter(|s| !s.is_empty()).cloned().collect::<Vec<_>>().join(" · ")),
                    ),
            )
            .child(if k.unlocked {
                self.button(
                    format!("security-key-lock-{id}"),
                    t!("security-keys-lock"),
                    "lock",
                    Action::SecurityKey(SecurityKeyAction::Lock(id.clone())),
                    false,
                    cx,
                )
            } else {
                self.button(
                    format!("security-key-unlock-{id}"),
                    t!("security-keys-unlock"),
                    "lock",
                    Action::SecurityKey(SecurityKeyAction::Unlock(id.clone())),
                    true,
                    cx,
                )
            })
            .child(self.button(
                format!("security-key-remove-{id}"),
                t!("security-keys-remove"),
                "trash",
                Action::SecurityKey(SecurityKeyAction::Remove(id)),
                false,
                cx,
            ))
    }

    /// The reader's offer, above a message encrypted to a locked security
    /// key: its PIN, and Unlock for each locked key.
    pub(super) fn security_key_card(&self, m: &Mail, cx: &Context<Self>) -> Option<AnyElement> {
        if self.hw.needed.as_deref() != Some(m.id.as_str()) {
            return None;
        }
        let c = self.colors();
        let locked: Vec<SecurityKey> = self.backend.locked_security_keys();
        if locked.is_empty() {
            return None;
        }
        Some(
            div()
                .id("security-key-needed")
                .debug_selector(|| "security-key-needed".into())
                .mb_4()
                .p_3()
                .rounded(px(10.))
                .border_1()
                .border_color(rgb(c.border))
                .bg(rgb(c.raised))
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .text_size(px(13.))
                        .font_weight(FontWeight::MEDIUM)
                        .child(icon_el("lock", 14., c.accent))
                        .child(t!("security-keys-message-needs-key")),
                )
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap_2()
                        .child(div().w(px(200.)).max_w_full().child(field(Input::new(&self.hw_pin), c, self.hw.busy)))
                        .children(locked.into_iter().map(|k| {
                            self.button(
                                format!("security-key-reader-unlock-{}", k.id),
                                &t!("security-keys-unlock-named", name = k.name.clone()),
                                "lock",
                                Action::SecurityKey(SecurityKeyAction::Unlock(k.id.clone())),
                                true,
                                cx,
                            )
                        })),
                )
                .when_some(self.hw.error.clone(), |d, e| {
                    d.child(div().id("security-key-reader-error").role(Role::Alert).text_size(px(12.)).text_color(rgb(c.danger)).child(e))
                })
                .into_any_element(),
        )
    }

    /// After a message is read: whether it stays encrypted for want of a
    /// locked security key, for the reader's offer.
    pub(super) fn check_security_key_needed(&mut self, m: &Mail, cx: &mut Context<Self>) {
        if self.hw.needed.as_ref().is_some_and(|id| *id != m.id) {
            self.hw.needed = None;
        }
        let id = m.id.clone();
        let mail = m.clone();
        self.task(
            cx,
            move |b| Ok(b.needs_security_key(&mail)),
            move |this, result, cx| {
                if result.unwrap_or(false) && this.selected.as_ref() == Some(&id) {
                    this.hw.needed = Some(id);
                    cx.notify();
                }
            },
        );
    }

    pub(super) fn security_key_action(&mut self, action: SecurityKeyAction, w: &mut Window, cx: &mut Context<Self>) {
        self.hw.error = None;
        match action {
            SecurityKeyAction::FindCards => {
                if self.hw.busy {
                    return;
                }
                self.hw.busy = true;
                self.task(
                    cx,
                    |b| b.connected_cards(),
                    |this, result, cx| {
                        this.hw.busy = false;
                        this.hw.cards = Some(result.map_err(|e| format!("{e:#}")));
                        cx.notify();
                    },
                );
            }
            SecurityKeyAction::AddCard(ident) => {
                let Some(card) = self
                    .hw
                    .cards
                    .as_ref()
                    .and_then(|c| c.as_ref().ok())
                    .and_then(|c| c.iter().find(|c| c.ident == ident))
                    .cloned()
                else {
                    return;
                };
                let pin = Self::value(&self.hw_pin, cx);
                Self::set(&self.hw_pin, "", w, cx);
                self.hw.busy = true;
                self.task(
                    cx,
                    move |b| b.add_card(&card, "", &pin),
                    move |this, result, cx| {
                        this.hw.busy = false;
                        match result {
                            Ok(key) => {
                                if let Some(Ok(cards)) = this.hw.cards.as_mut() {
                                    cards.retain(|c| c.ident != key.ident);
                                }
                                this.notice(t!("security-keys-added", name = key.name), false, cx);
                            }
                            Err(e) => this.hw.error = Some(format!("{e:#}")),
                        }
                        cx.notify();
                    },
                );
            }
            SecurityKeyAction::AddToken => {
                let module = Self::value(&self.hw_module, cx);
                let pin = Self::value(&self.hw_pin, cx);
                Self::set(&self.hw_pin, "", w, cx);
                self.hw.busy = true;
                self.task(
                    cx,
                    move |b| b.add_token_identities(&module, &pin),
                    move |this, result, cx| {
                        this.hw.busy = false;
                        match result {
                            Ok(added) => this.notice(t!("security-keys-certificates-added", count = added.len()), false, cx),
                            Err(e) => this.hw.error = Some(format!("{e:#}")),
                        }
                        cx.notify();
                    },
                );
            }
            SecurityKeyAction::Unlock(id) => {
                let pin = Self::value(&self.hw_pin, cx);
                Self::set(&self.hw_pin, "", w, cx);
                if pin.trim().is_empty() {
                    self.hw.error = Some(t!("security-keys-enter-the-pin").into());
                    return;
                }
                self.hw.busy = true;
                self.task(
                    cx,
                    move |b| b.unlock_security_key(&id, &pin),
                    move |this, result, cx| {
                        this.hw.busy = false;
                        match result {
                            Ok(()) => {
                                this.hw.needed = None;
                                // The open message is read again with the key.
                                if let Some(id) = this.selected.take() {
                                    this.select(id, cx);
                                }
                            }
                            Err(e) => this.hw.error = Some(format!("{e:#}")),
                        }
                        cx.notify();
                    },
                );
            }
            SecurityKeyAction::Lock(id) => self.backend.lock_security_key(&id),
            SecurityKeyAction::Remove(id) => {
                if let Err(e) = self.backend.remove_security_key(&id) {
                    self.hw.error = Some(format!("{e:#}"));
                }
            }
        }
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{act, frame, two_accounts};
    use super::*;
    use std::prelude::v1::test;

    #[gpui::test]
    fn security_keys_show_in_encryption_settings(cx: &mut gpui::TestAppContext) {
        let (view, cx, _store) = two_accounts(cx);
        act(&view, Action::Settings, cx);
        act(&view, Action::SettingsPage(Page::Encryption), cx);
        frame(cx);
        assert!(cx.debug_bounds("security-key-pin").is_some(), "the PIN field is shown");
        // Unlocking without a PIN says so, and asks nothing of a key.
        act(&view, Action::SecurityKey(SecurityKeyAction::Unlock("openpgp-card:0006:00000001".into())), cx);
        view.read_with(cx, |app, _| {
            assert_eq!(app.hw.error.as_deref(), Some(t!("security-keys-enter-the-pin")));
        });
    }
}

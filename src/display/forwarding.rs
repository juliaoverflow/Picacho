//! Forwarding settings: the connected alias services and their aliases.
//!
//! Each service is a card: one line saying what it is, opening to a
//! toolbar (where its mail goes, search, New Alias) above its aliases.
//! An alias row shows its address, a few status pills and its switch; the
//! rest is in its menu. SimpleLogin aliases can list their reverse
//! aliases, which are shown fully hidden, with how much mail each has had.
use super::*;
use crate::manage::forwarding::{Alias, ReverseAlias};

/// Aliases listed per page of a service.
const ALIASES_PER_PAGE: usize = 25;
/// Reverse aliases listed per page.
const CONTACTS_PER_PAGE: usize = 50;
/// Aliases past which a service offers a search field.
const SEARCH_ALIASES_AT: usize = 6;
/// A reverse alias's row height.
const CONTACT_ROW: f32 = 46.;
/// Reverse aliases past which the panel offers a search field.
const CONTACT_SEARCH_AT: usize = 8;

/// An alias's field edited below its row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AliasField {
    Note,
    /// The name recipients see on mail from the alias.
    Name,
}
impl AliasField {
    fn label(self) -> &'static str {
        match self {
            Self::Note => t!("forwarding-description"),
            Self::Name => t!("forwarding-display-name"),
        }
    }
    fn placeholder(self) -> &'static str {
        match self {
            Self::Note => t!("forwarding-description"),
            Self::Name => t!("forwarding-the-name-recipients-see-like-jane"),
        }
    }
}

/// A reverse alias ready to list: what its row shows, worked out once
/// when fetched rather than on every frame.
pub(super) struct ContactRow {
    pub(super) contact: ReverseAlias,
    title: SharedString,
    hidden: SharedString,
    when: SharedString,
    /// "Received 3 (14d) · 12 total" and the same for sent; empty when
    /// the alias's activity couldn't be read.
    received: SharedString,
    sent: SharedString,
    /// The contact's address, lowercase. The reverse alias is not
    /// searched, so a search can't reveal it either.
    search: String,
}
impl ContactRow {
    fn new(contact: ReverseAlias) -> Self {
        Self {
            title: if contact.contact.is_empty() {
                t!("forwarding-unknown-contact").into()
            } else {
                contact.contact.clone().into()
            },
            hidden: fwd::obscure(&contact.address).into(),
            when: contact
                .last_sent
                .map(|ts| t!("forwarding-last-written", date = date_short(ts)))
                .unwrap_or_default()
                .into(),
            received: Self::count(t!("forwarding-received"), contact.stats.map(|s| {
                (s.received_recent, s.received, s.partial)
            })),
            sent: Self::count(t!("forwarding-sent"), contact.stats.map(|s| (s.sent_recent, s.sent, s.partial))),
            search: contact.contact.to_lowercase(),
            contact,
        }
    }
    /// "Received 3 (14d) · 12 total", with a "+" when not all of the
    /// alias's activity could be read.
    fn count(what: &str, counts: Option<(u64, u64, bool)>) -> SharedString {
        counts
            .map(|(recent, total, partial)| {
                t!("forwarding-d-total", what = what, recent = recent, contact_recent_days = fwd::CONTACT_RECENT_DAYS, total = total, partial = if partial { "yes" } else { "no" })
            })
            .unwrap_or_default()
            .into()
    }
}
/// An alias's reverse aliases: most recently written first, and which of
/// them match the panel's search.
pub(super) struct Contacts {
    pub(super) rows: Vec<ContactRow>,
    pub(super) shown: Vec<usize>,
}
impl Contacts {
    pub(super) fn new(mut list: Vec<ReverseAlias>, query: &str) -> Self {
        list.sort_by(|a, b| {
            b.last_sent
                .cmp(&a.last_sent)
                .then_with(|| a.contact.to_lowercase().cmp(&b.contact.to_lowercase()))
        });
        let mut this = Self {
            rows: list.into_iter().map(ContactRow::new).collect(),
            shown: Vec::new(),
        };
        this.filter(query);
        this
    }
    pub(super) fn filter(&mut self, query: &str) {
        let query = query.trim().to_lowercase();
        self.shown = (0..self.rows.len())
            .filter(|&i| query.is_empty() || self.rows[i].search.contains(&query))
            .collect();
    }
    /// A contact just made (or found), listed first.
    fn add(&mut self, mut contact: ReverseAlias, query: &str) {
        // One found rather than made keeps the mail already counted.
        if contact.stats.is_none() {
            contact.stats = self
                .rows
                .iter()
                .find(|r| r.contact.id == contact.id || r.contact.address == contact.address)
                .and_then(|r| r.contact.stats);
        }
        self.rows
            .retain(|r| r.contact.id != contact.id && r.contact.address != contact.address);
        self.rows.insert(0, ContactRow::new(contact));
        self.filter(query);
    }
    fn remove(&mut self, id: &str, query: &str) {
        self.rows.retain(|r| r.contact.id != id);
        self.filter(query);
    }
}

/// A status pill's colors.
#[derive(Clone, Copy)]
enum Tone {
    Muted,
    Accent,
    Danger,
}

impl MailApp {
    /// A small status pill.
    fn pill(&self, text: impl Into<SharedString>, tone: Tone) -> Div {
        let c = self.colors();
        let (fg, bg) = match tone {
            Tone::Muted => (c.muted, rgba((c.border << 8) | 0x99)),
            Tone::Accent => (c.accent, rgba((c.accent << 8) | 0x1f)),
            Tone::Danger => (c.danger, rgba((c.danger << 8) | 0x1a)),
        };
        div()
            .flex_shrink_0()
            .px(px(6.))
            .py(px(1.))
            .rounded(px(5.))
            .bg(bg)
            .text_size(px(11.))
            .font_weight(FontWeight::MEDIUM)
            .text_color(rgb(fg))
            .child(text.into())
    }
    /// A card that holds one service, or the form to add one.
    fn card(&self) -> Div {
        let c = self.colors();
        div()
            .w_full()
            .min_w_0()
            .rounded(px(12.))
            .border_1()
            .border_color(rgb(c.border))
            .bg(rgb(c.raised))
            .overflow_hidden()
            .flex()
            .flex_col()
    }
    /// A small bordered button for settings rows.
    fn small_button(
        &self,
        id: String,
        label: &str,
        icon: &str,
        action: Action,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let c = self.colors();
        self.button(id, label, icon, action, false, cx)
            .h(px(28.))
            .border_1()
            .border_color(rgb(c.border))
    }
    /// A square icon-only button with a tooltip.
    fn fwd_icon_button(
        &self,
        id: String,
        icon: &'static str,
        tip: &'static str,
        action: Action,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let c = self.colors();
        div()
            .id(SharedString::from(id.clone()))
            .debug_selector(move || id.clone())
            .size(px(28.))
            .flex_shrink_0()
            .rounded(px(7.))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .hover(move |s| s.bg(rgb(c.soft)))
            .active(|s| s.opacity(0.75))
            .child(icon_el(icon, 14., c.muted))
            .tooltip(move |w, cx| Tooltip::new(tip).build(w, cx))
            .on_click(cx.listener(move |this, _, w, cx| {
                // Inside rows that open when clicked.
                cx.stop_propagation();
                this.handle(action.clone(), w, cx)
            }))
    }
    /// A sentence asking to confirm, with Cancel and the action.
    fn confirm_strip(
        &self,
        id: String,
        question: String,
        confirm: &str,
        action: Action,
        cx: &Context<Self>,
    ) -> Div {
        let c = self.colors();
        div()
            .mx_4()
            .mb_3()
            .px_3()
            .py_2()
            .rounded(px(9.))
            .bg(rgba((c.danger << 8) | 0x14))
            .border_1()
            .border_color(rgba((c.danger << 8) | 0x55))
            .flex()
            .flex_wrap()
            .items_center()
            .gap_2()
            .child(
                div()
                    .flex_1()
                    .min_w(px(160.))
                    .text_size(px(12.))
                    .child(question),
            )
            .child(self.small_button(
                format!("{id}-cancel"),
                t!("forwarding-cancel"),
                "",
                Action::CancelForwardingConfirm,
                cx,
            ))
            .child(
                self.small_button(id, confirm, "", action, cx)
                    .text_color(rgb(c.danger))
                    .border_color(rgb(c.danger)),
            )
    }

    /// Forwarding services as cards, then a way to add another.
    pub(super) fn forwarding_page(&self, cx: &Context<Self>) -> AnyElement {
        let adding = self.fwd_adding || self.forwarding.is_empty();
        let list = div()
            .id("forwarding-list")
            .debug_selector(|| "forwarding-list".into())
            // Stretched by the page: a percentage width resolves to the
            // content's width inside the scrolling page.
            .min_w_0()
            .flex()
            .flex_col()
            .gap_3()
            .children(self.forwarding.iter().map(|f| self.service_card(f, cx)))
            .child(if adding {
                self.add_service_card(cx)
            } else {
                self.add_service_button(cx)
            });
        self.settings_column()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(self.section(t!("forwarding-forwarding")))
                    .child(list),
            )
            .into_any_element()
    }
    /// The quiet "Add Service" line below the services.
    fn add_service_button(&self, cx: &Context<Self>) -> AnyElement {
        let c = self.colors();
        div()
            .id("forwarding-add")
            .debug_selector(|| "forwarding-add".into())
            .h(px(44.))
            .px_4()
            .flex()
            .items_center()
            .justify_center()
            .gap_2()
            .rounded(px(12.))
            .border_1()
            .border_color(rgb(c.border))
            .cursor_pointer()
            .text_size(px(13.))
            .font_weight(FontWeight::MEDIUM)
            .text_color(rgb(c.accent))
            .hover(move |s| s.bg(rgb(c.raised)))
            .child(icon_el("plus", 14., c.accent))
            .child(t!("forwarding-add-service"))
            .on_click(cx.listener(|this, _, w, cx| this.handle(Action::AddForwarding, w, cx)))
            .into_any_element()
    }
    /// A card with the form that connects a service.
    fn add_service_card(&self, cx: &Context<Self>) -> AnyElement {
        let first = self.forwarding.is_empty();
        self.card()
            .child(
                div()
                    .id("forwarding-add")
                    .debug_selector(|| "forwarding-add".into())
                    .px_4()
                    .pt_3()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(self.view_mark("plus", 30., false))
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(14.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(if first { t!("forwarding-add-a-service") } else { t!("forwarding-add-service") }),
                    )
                    .when(!first, |d| {
                        d.child(self.fwd_icon_button(
                            "forwarding-add-close".into(),
                            "close",
                            t!("forwarding-cancel"),
                            Action::AddForwarding,
                            cx,
                        ))
                    }),
            )
            .child(div().p_4().child(self.forwarding_form(cx)))
            .into_any_element()
    }
    fn forwarding_form(&self, cx: &Context<Self>) -> Div {
        let c = self.colors();
        let service = self.fwd_service;
        let busy = self.fwd_busy.contains("connect");
        let (help, url) = service.key_help();
        let label = |text: &'static str| {
            div()
                .text_size(px(12.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(rgb(c.muted))
                .child(text)
        };
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                self.segmented(
                    "forwarding-service",
                    ForwardingService::ALL
                        .into_iter()
                        .map(|s| {
                            (
                                s.label().to_owned(),
                                Action::ForwardingService(s),
                                s == service,
                            )
                        })
                        .collect(),
                    cx,
                )
                .max_w(px(420.)),
            )
            .when(service.uses_proton_account(), |d| d.child(self.proton_account_choice(cx)))
            .when(!service.uses_proton_account(), |d| d.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(label(t!("forwarding-api-key")))
                    .child(
                        div()
                            .id("forwarding-key")
                            .debug_selector(|| "forwarding-key".into())
                            .max_w(px(420.))
                            .child(field(Input::new(&self.fwd_key), c, busy)),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap_2()
                            .text_size(px(11.))
                            .text_color(rgb(c.muted))
                            .child(help)
                            .child(self.link(
                                "forwarding-key-help",
                                t!("forwarding-open"),
                                Action::OpenUrl(url.into()),
                                cx,
                            )),
                    ),
            ))
            .when(!service.uses_proton_account(), |d| d.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(label(t!("forwarding-server")))
                    .child(div().id("forwarding-server").max_w(px(420.)).child(field(
                        Input::new(&self.fwd_server),
                        c,
                        busy,
                    )))
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(rgb(c.muted))
                            .child(t!("forwarding-only-for-a-self-hosted-server", default_server = service.default_server())),
                    ),
            ))
            .when_some(self.fwd_error.clone(), |d, e| {
                d.child(
                    div()
                        .id("forwarding-error")
                        .text_size(px(12.))
                        .text_color(rgb(c.danger))
                        .child(e),
                )
            })
            .child(self.forwarding_buttons(busy, cx))
    }
    /// Connect; or, signing in to Proton Pass, the buttons Proton Mail's
    /// sign-in has at each step: Continue, Verify and the like, and Back.
    fn forwarding_buttons(&self, busy: bool, cx: &Context<Self>) -> Div {
        let row = div().flex().gap_2();
        if !(self.fwd_service == ForwardingService::Proton && crate::sync::proton::pass_sign_in()) {
            return row.child(
                self.button(
                    "forwarding-connect",
                    if busy { t!("forwarding-connecting") } else { t!("forwarding-connect") },
                    "plus",
                    Action::ConnectForwarding,
                    true,
                    cx,
                )
                .when(busy, |d| d.opacity(0.6)),
            );
        }
        let stepping = self.proton_stage.is_some() && self.fwd_pass_login.is_some();
        let label = if busy { Some(t!("forwarding-connecting")) } else { self.proton_submit_label() };
        row.when(stepping, |d| {
            d.child(self.button(
                "forwarding-pass-back",
                if self.connecting { t!("ui-cancel") } else { t!("ui-back") },
                "",
                Action::ProtonBack,
                false,
                cx,
            ))
        })
        // A security key alone has its own buttons.
        .when_some(label, |d, label| {
            d.child(
                self.button("forwarding-connect", label, "", Action::ConnectForwarding, true, cx)
                    .when(busy || self.connecting, |d| d.opacity(0.7)),
            )
        })
    }

    /// For Proton's own aliases: which Proton account (signed in without
    /// Bridge) they come from, since they need no key.
    fn proton_account_choice(&self, cx: &Context<Self>) -> Div {
        let c = self.colors();
        // Signing in to Proton Pass, past the password: Proton Mail's
        // sign-in step, as in its sign-in-again dialog.
        if let Some(id) = &self.fwd_pass_login
            && let Some(step) = self.proton_step_view(true, cx)
        {
            let email = self.accounts.iter().find(|a| &a.id == id).map(|a| a.email.clone()).unwrap_or_default();
            return div()
                .max_w(px(420.))
                .flex()
                .flex_col()
                .gap_3()
                .child(div().text_size(px(13.)).child(email))
                .child(step);
        }
        // Signing in with the password, or (signed in) listing the
        // account's Proton addresses and aliases, which takes a while: said
        // so with a spinner, rather than the form looking as it did before
        // (its password field empty again) with only a faded button.
        let loading = self.fwd_busy.contains("connect");
        let signing_in = self.connecting && self.fwd_pass_login.is_some() && self.proton_stage.is_none();
        if loading || signing_in {
            let email = self
                .fwd_pass_login
                .as_ref()
                .or(self.fwd_proton.as_ref())
                .and_then(|id| self.accounts.iter().find(|a| &a.id == id))
                .or_else(|| self.forwarding_proton_account())
                .map(|a| a.email.clone())
                .unwrap_or_default();
            return div()
                .debug_selector(|| "forwarding-proton-loading".into())
                .max_w(px(420.))
                .flex()
                .flex_col()
                .gap_2()
                .child(div().text_size(px(13.)).child(email))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .text_size(px(13.))
                        .text_color(rgb(c.muted))
                        .child(spinner(14., c.muted))
                        .child(if loading {
                            t!("forwarding-loading-proton-addresses")
                        } else {
                            t!("proton-login-signing-in")
                        }),
                );
        }
        let open = fwd::proton_accounts(&self.forwarding, &self.accounts);
        let chosen = self.forwarding_proton_account().map(|a| a.id.clone());
        let (help, url) = ForwardingService::Proton.key_help();
        let mut d = div().flex().flex_col().gap_1().child(
            div()
                .text_size(px(12.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(rgb(c.muted))
                .child(t!("forwarding-proton-account")),
        );
        d = if open.is_empty() {
            d.child(
                div()
                    .id("forwarding-proton-none")
                    .text_size(px(12.))
                    .text_color(rgb(c.muted))
                    .child(t!("forwarding-no-proton-account")),
            )
        } else {
            d.child(
                self.segmented(
                    "forwarding-proton",
                    open.iter()
                        .map(|a| {
                            (
                                a.email.clone(),
                                Action::ForwardingProtonAccount(a.id.clone()),
                                chosen.as_deref() == Some(a.id.as_str()),
                            )
                        })
                        .collect(),
                    cx,
                )
                .max_w(px(420.)),
            )
        };
        let busy = self.fwd_busy.contains("connect");
        // Proton gives Pass access only to a session signed in as an app
        // with Pass, so a build with a Pass registration signs in again:
        // the password, as Proton Mail's sign-in asks for it; a second
        // factor follows, where the account has one.
        d = d.when(!open.is_empty() && crate::sync::proton::pass_sign_in(), |d| {
            d.child(
                div()
                    .id("forwarding-proton-password")
                    .pt_2()
                    .max_w(px(420.))
                    .child(self.field(t!("forwarding-proton-password"), PASSWORD)),
            )
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(rgb(c.muted))
                    .child(t!("forwarding-proton-pass-signs-in-on-its-own")),
            )
        });
        d = d.when(!open.is_empty(), |d| {
            d.child(
                div()
                    .pt_2()
                    .text_size(px(12.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(rgb(c.muted))
                    .child(t!("forwarding-proton-pass-extra-password")),
            )
            .child(
                div()
                    .id("forwarding-pass-password")
                    .debug_selector(|| "forwarding-pass-password".into())
                    .max_w(px(420.))
                    .child(field(Input::new(&self.fwd_pass_password), c, busy)),
            )
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(rgb(c.muted))
                    .child(t!("forwarding-only-if-you-protect-proton-pass")),
            )
        });
        d.child(
            div()
                .flex()
                .flex_wrap()
                .gap_2()
                .text_size(px(11.))
                .text_color(rgb(c.muted))
                .child(help)
                .child(self.link(
                    "forwarding-proton-help",
                    t!("forwarding-open"),
                    Action::OpenUrl(url.into()),
                    cx,
                )),
        )
    }

    /// One service: what it is and where its mail goes on one line,
    /// opening to its toolbar and aliases.
    fn service_card(&self, f: &ForwardingAccount, cx: &Context<Self>) -> AnyElement {
        let c = self.colors();
        let busy = self.fwd_busy.contains(&f.id);
        let removing = self.fwd_confirm.as_deref() == Some(f.id.as_str());
        let open = self.fwd_open.contains(&f.id);
        let count = f.aliases.len();
        let mut caption = vec![t!("forwarding-alias", count = count)];
        caption.push(match self.deliver_label(f) {
            Some(email) => t!("forwarding-to", email = email),
            None => t!("forwarding-to-any-mailbox").into(),
        });
        // A self-hosted server is named; the service's own is implied.
        if !f.server.is_empty() {
            caption.push(fwd_server_label(f));
        }
        let toggle = Action::ToggleForwarding(f.id.clone());
        let header = div()
            .id(SharedString::from(format!("forwarding-info-{}", f.id)))
            .px_4()
            .py_3()
            .flex()
            .items_center()
            .gap_3()
            .cursor_pointer()
            .hover(move |s| s.bg(rgb(c.surface)))
            .child(self.view_mark("alias", 34., false))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(1.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .min_w_0()
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .text_size(px(14.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(f.service.label()),
                            )
                            .when(!f.name.is_empty(), |d| {
                                d.child(
                                    div()
                                        .min_w_0()
                                        .truncate()
                                        .text_size(px(12.))
                                        .text_color(rgb(c.muted))
                                        .child(f.name.clone()),
                                )
                            }),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(rgb(c.muted))
                            .truncate()
                            .child(caption.join(" · ")),
                    ),
            )
            .when(busy, |d| d.child(spinner(14., c.muted)))
            .child(self.fwd_icon_button(
                format!("forwarding-menu-{}", f.id),
                "ellipsis",
                t!("forwarding-more"),
                Action::ServiceMenu(f.id.clone()),
                cx,
            ))
            .child(icon_el(
                if open { "chevron-up" } else { "chevron-down" },
                14.,
                c.muted,
            ))
            .on_click(cx.listener(move |this, _, w, cx| this.handle(toggle.clone(), w, cx)));
        self.card()
            .id(SharedString::from(format!("forwarding-{}", f.id)))
            .debug_selector({
                let id = f.id.clone();
                move || format!("forwarding-{id}")
            })
            .child(header)
            .when(self.fwd_pass_expired.contains(&f.id), |d| d.child(self.pass_expired_strip(f, cx)))
            .when(removing, |d| {
                d.child(self.confirm_strip(
                    format!("forwarding-remove-{}", f.id),
                    t!("forwarding-remove-its-aliases-keep-working-picacho", label = f.service.label()),
                    t!("forwarding-remove"),
                    Action::RemoveForwarding(f.id.clone()),
                    cx,
                ))
            })
            .when(open, |d| d.child(self.service_body(f, cx)))
            .into_any_element()
    }
    /// The address of the mailbox a service delivers to; None for any.
    fn deliver_label(&self, f: &ForwardingAccount) -> Option<String> {
        let id = f.deliver_to.as_ref()?;
        Some(
            self.accounts
                .iter()
                .find(|a| &a.id == id)
                .map(|a| a.email.clone())
                .unwrap_or_else(|| t!("forwarding-a-removed-mailbox").into()),
        )
    }
    /// A service's toolbar, new alias form and aliases.
    fn service_body(&self, f: &ForwardingAccount, cx: &Context<Self>) -> Div {
        let c = self.colors();
        let busy = self.fwd_busy.contains(&f.id);
        let creating = self.fwd_creating.as_deref() == Some(f.id.as_str());
        let deliver = div()
            .id(SharedString::from(format!("deliver-{}", f.id)))
            .debug_selector({
                let id = f.id.clone();
                move || format!("deliver-{id}")
            })
            .h(px(28.))
            .max_w(px(260.))
            .min_w_0()
            .flex_shrink_1()
            .pl_3()
            .pr_2()
            .flex()
            .items_center()
            .gap_2()
            .rounded_full()
            .border_1()
            .border_color(rgb(c.border))
            .bg(rgb(c.bg))
            .cursor_pointer()
            .hover(move |s| s.border_color(rgb(c.muted)))
            .text_size(px(12.))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .child(self.deliver_label(f).unwrap_or_else(|| t!("forwarding-any-mailbox").into())),
            )
            .child(icon_el("chevrons-up-down", 12., c.muted))
            .tooltip(|w, cx| {
                Tooltip::new(t!("forwarding-where-the-aliases-mail-arrives-any"))
                    .build(w, cx)
            })
            .on_click(cx.listener({
                let action = Action::DeliverMenu(f.id.clone());
                move |this, _, w, cx| this.handle(action.clone(), w, cx)
            }));
        let sending: SharedString = if f.service.can_send() {
            t!("forwarding-send-as-an-alias-by-choosing", mailbox = self.deliver_label(f)
                    .unwrap_or_else(|| t!("forwarding-the-mailbox-you-write-from").into()), label = f.service.label())
        } else {
            t!("forwarding-only-relays-replies-to-mail-it", label = f.service.label())
        }
        .into();
        let info = div()
            .id(SharedString::from(format!("send-info-{}", f.id)))
            .size(px(24.))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
            .child(icon_el("info", 14., c.muted))
            .tooltip(move |w, cx| Tooltip::new(sending.clone()).build(w, cx));
        let toolbar = div()
            .px_4()
            .py_3()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_2()
            .child(
                div()
                    .flex_shrink_0()
                    .text_size(px(12.))
                    .text_color(rgb(c.muted))
                    .child(t!("forwarding-forwards-to")),
            )
            .child(deliver)
            .child(info)
            .child(div().flex_1())
            .when(f.aliases.len() > SEARCH_ALIASES_AT, |d| {
                d.child(
                    div()
                        .id(SharedString::from(format!("aliases-search-{}", f.id)))
                        .w(px(180.))
                        .flex_shrink_1()
                        .min_w(px(120.))
                        .child(field(
                            Input::new(&self.fwd_search).prefix(icon_el("search", 13., c.muted)),
                            c,
                            false,
                        )),
                )
            })
            .child(if creating {
                self.small_button(
                    format!("alias-new-{}", f.id),
                    t!("forwarding-cancel"),
                    "",
                    Action::NewAlias(f.id.clone()),
                    cx,
                )
            } else {
                self.button(
                    format!("alias-new-{}", f.id),
                    t!("forwarding-new-alias"),
                    "plus",
                    Action::NewAlias(f.id.clone()),
                    true,
                    cx,
                )
                .h(px(28.))
            });
        // Keeping the mailboxes to their aliases: they don't write as
        // themselves, and show the alias used where their address was.
        let protected: Vec<String> = fwd::forwards_to(f, &self.accounts)
            .iter()
            .filter_map(|id| self.accounts.iter().find(|a| &a.id == id))
            .map(|a| a.email.clone())
            .collect();
        let aliases_only = f.service.can_send().then(|| {
            let mailboxes = if protected.is_empty() {
                t!("forwarding-no-mailbox-here-receives-its-aliases").to_owned()
            } else {
                t!("forwarding-cant-write-as-new-mail-and", aliases = protected.join(", "), count = protected.len())
            };
            div()
                .px_4()
                .pb_3()
                .flex()
                .items_start()
                .gap_3()
                .child(
                    self.switch(
                        format!("aliases-only-{}", f.id),
                        f.aliases_only,
                        Action::SetAliasesOnly(f.id.clone(), !f.aliases_only),
                        cx,
                    )
                    .debug_selector({
                        let id = f.id.clone();
                        move || format!("aliases-only-{id}")
                    }),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .child(
                            div()
                                .text_size(px(13.))
                                .font_weight(FontWeight::MEDIUM)
                                .child(t!("forwarding-send-only-as-aliases")),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(rgb(c.muted))
                                .child(mailboxes),
                        ),
                )
        });
        div()
            .border_t_1()
            .border_color(rgb(c.border))
            .bg(rgb(c.surface))
            .flex()
            .flex_col()
            .child(toolbar)
            .children(aliases_only)
            .when(creating, |d| {
                d.child(div().px_4().pb_3().child(self.alias_form(f, busy, cx)))
            })
            .child(self.alias_list(f, cx))
    }
    /// A service's aliases, pinned ones first, narrowed by the search, with
    /// "Show all" when long.
    fn alias_list(&self, f: &ForwardingAccount, cx: &Context<Self>) -> Div {
        let c = self.colors();
        let query = Self::value(&self.fwd_search, cx).trim().to_lowercase();
        let mut matching: Vec<&Alias> = f
            .aliases
            .iter()
            .filter(|a| {
                query.is_empty()
                    || a.email.to_lowercase().contains(&query)
                    || a.note.to_lowercase().contains(&query)
                    || a.name.to_lowercase().contains(&query)
            })
            .collect();
        // Stable: otherwise in the service's order.
        matching.sort_by_key(|a| !a.pinned);
        let pages = matching.len().div_ceil(ALIASES_PER_PAGE).max(1);
        let page = self.fwd_pages.get(&f.id).copied().unwrap_or(0).min(pages - 1);
        let start = page * ALIASES_PER_PAGE;
        let end = (start + ALIASES_PER_PAGE).min(matching.len());
        let mut list = div()
            .border_t_1()
            .border_color(rgb(c.border))
            .flex()
            .flex_col();
        for (i, a) in matching[start..end].iter().enumerate() {
            list = list.child(self.alias_row(f, a, i > 0, cx));
        }
        if matching.is_empty() {
            list = list.child(
                div()
                    .px_4()
                    .py_5()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_1()
                    .text_size(px(12.))
                    .text_color(rgb(c.muted))
                    .child(icon_el(
                        if f.aliases.is_empty() { "alias" } else { "search" },
                        18.,
                        c.muted,
                    ))
                    .child(if f.aliases.is_empty() {
                        t!("forwarding-no-aliases-yet")
                    } else {
                        t!("forwarding-no-alias-matches")
                    }),
            );
        }
        if pages > 1 {
            let id = f.id.clone();
            list = list.child(self.pager(
                &format!("aliases-{}", f.id),
                page,
                pages,
                t!("forwarding-of-aliases", first = start + 1, end = end, count = matching.len()),
                move |p| Action::AliasPage(id.clone(), p),
                cx,
            ));
        }
        list
    }
    /// Previous / next links around where a paged list is.
    fn pager(
        &self,
        id: &str,
        page: usize,
        pages: usize,
        label: String,
        action: impl Fn(usize) -> Action,
        cx: &Context<Self>,
    ) -> Div {
        let c = self.colors();
        let side = |enabled: bool, key: &str, text: &str, to: usize| {
            if enabled {
                self.link(&format!("{id}-{key}"), text, action(to), cx)
                    .into_any_element()
            } else {
                div()
                    .text_size(px(12.))
                    .text_color(rgb(c.muted))
                    .opacity(0.5)
                    .child(text.to_owned())
                    .into_any_element()
            }
        };
        div()
            .px_4()
            .py_2()
            .border_t_1()
            .border_color(rgb(c.border))
            .flex()
            .items_center()
            .gap_3()
            .child(side(page > 0, "first", t!("forwarding-first"), 0))
            .child(side(page > 0, "prev", t!("forwarding-previous"), page.saturating_sub(1)))
            .child(
                div()
                    .flex_1()
                    .flex()
                    .justify_center()
                    .text_size(px(12.))
                    .text_color(rgb(c.muted))
                    .child(t!("forwarding-page-of", page = page + 1, pages = pages, label = label)),
            )
            .child(side(page + 1 < pages, "next", t!("forwarding-next"), page + 1))
            .child(side(page + 1 < pages, "last", t!("forwarding-last"), pages - 1))
    }
    /// The new alias form, laid out like the address it makes: a prefix
    /// (typed, suggested or random) and a domain where the service lets
    /// them be chosen, then a description.
    fn alias_form(&self, f: &ForwardingAccount, busy: bool, cx: &Context<Self>) -> Stateful<Div> {
        let c = self.colors();
        let id = f.id.clone();
        let picks = f.service != ForwardingService::FirefoxRelay;
        let loading = self.fwd_busy.contains(&format!("options:{id}"));
        let options = self.fwd_options.get(&id);
        let domain = self.alias_domain(&id);
        let prefix_ok = domain.is_none_or(|d| d.custom_prefix);
        let prefix = fwd::clean_prefix(&Self::value(&self.fwd_prefix, cx));
        let random = match self.fwd_format {
            Some(format) => t!("forwarding-a-prefix", to_lowercase = format.label().to_lowercase()),
            None => t!("forwarding-a-random-prefix").into(),
        };
        let preview = domain.map(|d| {
            if prefix_ok && !prefix.is_empty() {
                t!("forwarding-creates", prefix = prefix, suffix = d.suffix)
            } else if prefix_ok {
                t!("forwarding-creates-with", random = random, suffix = d.suffix)
            } else {
                t!("forwarding-creates-with-this-domain-only-takes", random = random, suffix = d.suffix)
            }
        });
        let domain_button = div()
            .id(SharedString::from(format!("alias-domain-{id}")))
            .debug_selector({
                let id = id.clone();
                move || format!("alias-domain-{id}")
            })
            .h(px(32.))
            .w(px(210.))
            .flex_shrink_1()
            .min_w(px(120.))
            .px_2()
            .flex()
            .items_center()
            .gap_2()
            .rounded(px(7.))
            .border_1()
            .border_color(rgb(c.border))
            .bg(rgb(c.bg))
            .cursor_pointer()
            .hover(move |s| s.border_color(rgb(c.muted)))
            .text_size(px(12.))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .child(match domain {
                        Some(d) => d.suffix.clone(),
                        None if loading => t!("forwarding-loading-domains").into(),
                        None => t!("forwarding-default-domain").to_string(),
                    }),
            )
            .when_some(domain, |d, domain| {
                d.child(self.pill(
                    domain.kind.label(),
                    match domain.kind {
                        fwd::DomainKind::Custom | fwd::DomainKind::Username => Tone::Accent,
                        _ => Tone::Muted,
                    },
                ))
            })
            .child(if loading {
                spinner(12., c.muted).into_any_element()
            } else {
                icon_el("chevrons-up-down", 12., c.muted).into_any_element()
            })
            .on_click(cx.listener({
                let action = Action::DomainMenu(id.clone());
                move |this, _, w, cx| this.handle(action.clone(), w, cx)
            }));
        let hint = |text: String, color: u32| {
            div()
                .text_size(px(11.))
                .text_color(rgb(color))
                .child(text)
        };
        let mailbox = self
            .fwd_mailbox
            .as_ref()
            .and_then(|m| self.accounts.iter().find(|a| &a.id == m))
            .map(|a| a.email.clone())
            .filter(|_| f.service.needs_mailbox());
        let mut hints = div().flex().flex_col().gap(px(3.));
        if let Some(p) = preview.filter(|_| picks) {
            hints = hints.child(hint(p, c.muted));
        }
        if matches!(options, Some(Ok(o)) if !o.can_create) {
            hints = hints.child(hint(
                t!("forwarding-wont-make-more-aliases-on-this", label = f.service.label()),
                c.danger,
            ));
        }
        if let Some(Err(e)) = options.filter(|_| picks) {
            hints = hints.child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .child(hint(
                        t!("forwarding-couldnt-list-domains-new-aliases-use", e = e),
                        c.danger,
                    ))
                    .child(self.link(
                        &format!("alias-domains-retry-{id}"),
                        t!("forwarding-try-again"),
                        Action::DomainMenu(id.clone()),
                        cx,
                    )),
            );
        }
        div()
            .id(SharedString::from(format!("alias-form-{id}")))
            .debug_selector(|| "alias-form".into())
            .p_3()
            .rounded(px(10.))
            .border_1()
            .border_color(rgb(c.border))
            .bg(rgb(c.raised))
            .shadow_sm()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(t!("forwarding-new-alias-2")),
                    )
                    .child(div().flex_1())
                    .when_some(mailbox, |d, email| {
                        d.child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_size(px(11.))
                                .text_color(rgb(c.muted))
                                .child(t!("forwarding-forwards-to-2", email = email)),
                        )
                    }),
            )
            .when(picks, |d| {
                d.child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .id("alias-prefix")
                                .debug_selector(|| "alias-prefix".into())
                                .flex_1()
                                .min_w(px(120.))
                                .child(field(
                                    Input::new(&self.fwd_prefix).disabled(busy || !prefix_ok),
                                    c,
                                    busy || !prefix_ok,
                                )),
                        )
                        .child(
                            self.fwd_icon_button(
                                format!("alias-prefix-menu-{id}"),
                                "ellipsis",
                                t!("forwarding-random-prefix"),
                                Action::PrefixMenu(id.clone()),
                                cx,
                            )
                            .size(px(32.))
                            .border_1()
                            .border_color(rgb(c.border)),
                        )
                        .child(domain_button),
                )
            })
            .child(
                div()
                    .id("alias-note")
                    .min_w_0()
                    .child(field(Input::new(&self.fwd_note), c, busy)),
            )
            .child(
                div()
                    .flex()
                    .items_end()
                    .gap_3()
                    .child(div().flex_1().min_w_0().child(hints))
                    .child(
                        self.button(
                            format!("alias-create-{id}"),
                            if busy { t!("forwarding-creating") } else { t!("forwarding-create") },
                            "",
                            Action::CreateAlias(id.clone()),
                            true,
                            cx,
                        )
                        .h(px(30.))
                        .flex_shrink_0()
                        .when(busy, |d| d.opacity(0.6)),
                    ),
            )
    }

    /// One alias: its address (marked when pinned), status pills, display
    /// name and description, its switch, and a menu for the rest. Its
    /// counts are in the address's tooltip. Below it, when open: editing a
    /// field, confirming its deletion, or its reverse aliases.
    fn alias_row(
        &self,
        f: &ForwardingAccount,
        a: &Alias,
        divided: bool,
        cx: &Context<Self>,
    ) -> AnyElement {
        let c = self.colors();
        let key = format!("{}:{}", f.id, a.id);
        let deleting = self.fwd_confirm.as_deref() == Some(key.as_str());
        let editing = self
            .fwd_editing
            .as_ref()
            .filter(|(s, x, _)| s == &f.id && x == &a.id)
            .map(|(_, _, which)| *which);
        let separate = a.display == AliasDisplay::Separate;
        let stats = a.stats.summary();
        let tip: SharedString = if stats.is_empty() {
            a.email.clone().into()
        } else {
            format!("{}\n{stats}", a.email).into()
        };
        let switch = self
            .switch(
                format!("alias-enabled-{key}"),
                a.enabled,
                Action::SetAliasEnabled(f.id.clone(), a.id.clone(), !a.enabled),
                cx,
            )
            .tooltip(|w, cx| Tooltip::new(t!("forwarding-forward-mail-sent-to-this-alias")).build(w, cx));
        let details = !a.enabled || separate || !a.note.is_empty() || !a.name.is_empty() || a.is_address();
        let row = div()
            .id(SharedString::from(format!("alias-{key}")))
            .min_h(px(46.))
            .px_4()
            .py_2()
            .flex()
            .items_center()
            .gap_3()
            .hover(move |s| s.bg(rgb(c.raised)))
            .child(
                div()
                    .id(SharedString::from(format!("alias-address-{key}")))
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(3.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .min_w_0()
                            .when(a.pinned, |d| d.child(icon_el("pin", 12., c.accent)))
                            .child(
                                div()
                                    .min_w_0()
                                    .text_size(px(13.))
                                    .font_weight(FontWeight::MEDIUM)
                                    .truncate()
                                    .when(!a.enabled, |d| d.text_color(rgb(c.muted)))
                                    .child(a.email.clone()),
                            ),
                    )
                    .when(details, |d| {
                        d.child(
                            div()
                                .flex()
                                .items_center()
                                .gap_1()
                                .min_w_0()
                                .text_size(px(12.))
                                .text_color(rgb(c.muted))
                                .when(!a.enabled, |d| d.child(self.pill(t!("forwarding-off"), Tone::Danger)))
                                .when_some(a.proton_address, |d, kind| d.child(self.pill(kind.label(), Tone::Muted)))
                                .when(separate, |d| d.child(self.pill(t!("forwarding-own-account"), Tone::Accent)))
                                .when(!a.name.is_empty(), |d| {
                                    d.child(
                                        div()
                                            .flex_shrink_0()
                                            .max_w(px(180.))
                                            .flex()
                                            .items_center()
                                            .gap(px(3.))
                                            .child(icon_el("user", 11., c.muted))
                                            .child(div().truncate().child(a.name.clone())),
                                    )
                                })
                                .when(!a.note.is_empty(), |d| {
                                    d.child(div().min_w_0().truncate().child(a.note.clone()))
                                }),
                        )
                    })
                    .tooltip(move |w, cx| Tooltip::new(tip.clone()).build(w, cx)),
            )
            .child(switch)
            .child(self.fwd_icon_button(
                format!("alias-menu-{key}"),
                "ellipsis",
                t!("forwarding-more"),
                Action::AliasMenu(f.id.clone(), a.id.clone()),
                cx,
            ));
        div()
            .when(divided, |d| d.border_t_1().border_color(rgb(c.border)))
            .child(row)
            .when_some(editing, |d, which| {
                d.child(
                    div()
                        .id(SharedString::from(format!("alias-note-edit-{key}")))
                        .debug_selector(|| "alias-note-edit".into())
                        .px_4()
                        .pb_3()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .flex_shrink_0()
                                .text_size(px(12.))
                                .text_color(rgb(c.muted))
                                .child(which.label()),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .child(field(Input::new(&self.fwd_description), c, false)),
                        )
                        .child(self.small_button(
                            format!("alias-note-cancel-{key}"),
                            t!("forwarding-cancel"),
                            "",
                            Action::CancelAliasEdit,
                            cx,
                        ))
                        .child(
                            self.button(
                                format!("alias-note-save-{key}"),
                                t!("forwarding-save"),
                                "",
                                Action::SaveAliasEdit,
                                true,
                                cx,
                            )
                            .h(px(28.)),
                        ),
                )
            })
            .when(deleting, |d| {
                d.child(self.confirm_strip(
                    format!("alias-delete-{key}"),
                    t!("forwarding-delete-mail-sent-to-it-will", email = a.email),
                    t!("forwarding-delete"),
                    Action::DeleteAlias(f.id.clone(), a.id.clone()),
                    cx,
                ))
            })
            .into_any_element()
    }
    /// The open alias's reverse aliases, as a subpage of Forwarding with
    /// a way back, so they scroll with the page rather than inside it.
    pub(super) fn reverse_page(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let (id, alias) = self.fwd_reverse.as_deref()?.split_once(':')?;
        let f = self.forwarding.iter().find(|f| f.id == id)?;
        let a = f.aliases.iter().find(|a| a.id == alias)?;
        let c = self.colors();
        let back = div()
            .id("reverse-back")
            .debug_selector(|| "reverse-back".into())
            .flex()
            .items_center()
            .gap_1()
            .cursor_pointer()
            .text_size(px(12.))
            .text_color(rgb(c.accent))
            .hover(|s| s.opacity(0.75))
            .child(icon_el("arrow-left", 13., c.accent))
            .child(t!("forwarding-forwarding"))
            .on_click(cx.listener({
                let action = Action::ToggleReverseAliases(f.id.clone(), a.id.clone());
                move |this, _, w, cx| this.handle(action.clone(), w, cx)
            }));
        let key = format!("{}:{}", f.id, a.id);
        let loading = self.fwd_busy.contains(&format!("contacts:{key}"));
        let total = match self.fwd_contacts.get(&key) {
            Some(Ok(list)) => Some(list.rows.len()),
            _ => None,
        };
        // The page's title: the alias, how many it has, and a refresh.
        let title = div()
            .flex()
            .items_center()
            .gap_2()
            .min_w_0()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .text_size(px(15.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(t!("forwarding-reverse-aliases")),
                            )
                            .when_some(total, |d, n| {
                                d.child(self.pill(n.to_string(), Tone::Muted))
                            }),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(rgb(c.muted))
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .debug_selector(|| "reverse-alias-email".into())
                            .child(a.email.clone()),
                    ),
            )
            .when(loading, |d| d.child(spinner(13., c.muted)))
            .child(self.fwd_icon_button(
                format!("reverse-refresh-{key}"),
                "refresh",
                t!("forwarding-refresh"),
                Action::LoadReverseAliases(f.id.clone(), a.id.clone()),
                cx,
            ));
        Some(
            div()
                .flex()
                .flex_col()
                .gap_3()
                .min_w_0()
                .child(back)
                .child(title)
                .child(self.reverse_panel(f, a, cx))
                .into_any_element(),
        )
    }

    /// A SimpleLogin alias's reverse aliases: a search, a way to write to
    /// someone new, and the contacts, a page at a time.
    fn reverse_panel(
        &self,
        f: &ForwardingAccount,
        a: &Alias,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let c = self.colors();
        let key = format!("{}:{}", f.id, a.id);
        let adding = self.fwd_busy.contains(&format!("contact-add:{key}"));
        let state = self.fwd_contacts.get(&key);
        let note = |text: SharedString, color: u32| {
            div()
                .py_2()
                .text_size(px(12.))
                .text_color(rgb(color))
                .child(text)
        };
        let mut panel = div()
            .id(SharedString::from(format!("reverse-aliases-{key}")))
            .debug_selector(|| "reverse-aliases".into())
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(rgb(c.muted))
                    .child(t!("forwarding-writing-to-one-of-these-from")),
            );
        let list = match state {
            None => return panel.child(note(t!("forwarding-loading").into(), c.muted)),
            Some(Err(e)) => {
                return panel.child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap_2()
                        .text_size(px(12.))
                        .child(div().text_color(rgb(c.danger)).child(e.clone()))
                        .child(self.link(
                            &format!("reverse-retry-{key}"),
                            t!("forwarding-try-again"),
                            Action::LoadReverseAliases(f.id.clone(), a.id.clone()),
                            cx,
                        )),
                );
            }
            Some(Ok(list)) => list,
        };
        panel = panel.child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap_2()
                .when(list.rows.len() > CONTACT_SEARCH_AT, |d| {
                    d.child(
                        div()
                            .id(SharedString::from(format!("reverse-search-{key}")))
                            .flex_1()
                            .min_w(px(140.))
                            .child(field(
                                Input::new(&self.fwd_contact_search)
                                    .prefix(icon_el("search", 13., c.muted)),
                                c,
                                false,
                            )),
                    )
                })
                .child(
                    div()
                        .id(SharedString::from(format!("reverse-new-{key}")))
                        .flex_1()
                        .min_w(px(160.))
                        .child(field(Input::new(&self.fwd_contact_new), c, adding)),
                )
                .child(
                    self.small_button(
                        format!("reverse-add-{key}"),
                        if adding { t!("forwarding-adding") } else { t!("forwarding-add") },
                        "plus",
                        Action::AddContact(f.id.clone(), a.id.clone()),
                        cx,
                    )
                    .when(adding, |d| d.opacity(0.6)),
                ),
        );
        // Deleting a contact asks first, above the list.
        let confirm = self
            .fwd_confirm
            .as_deref()
            .and_then(|k| k.strip_prefix("contact:"))
            .and_then(|k| k.strip_prefix(key.as_str()))
            .and_then(|k| k.strip_prefix(':'))
            .and_then(|id| list.rows.iter().find(|r| r.contact.id == id));
        if let Some(row) = confirm {
            panel = panel.child(div().child(self.confirm_strip(
                format!("reverse-delete-{key}"),
                t!("forwarding-delete-the-contact-its-reverse-alias", title = row.title),
                t!("forwarding-delete"),
                Action::DeleteContact(f.id.clone(), a.id.clone(), row.contact.id.clone()),
                cx,
            ).mx_0().mb_0()));
        }
        if list.shown.is_empty() {
            return panel.child(note(
                if list.rows.is_empty() {
                    t!("forwarding-none-yet-simplelogin-makes-one-for", label = f.service.label()).into()
                } else {
                    t!("forwarding-no-contact-matches").into()
                },
                c.muted,
            ));
        }
        let pages = list.shown.len().div_ceil(CONTACTS_PER_PAGE).max(1);
        let page = self.fwd_contact_page.min(pages - 1);
        let start = page * CONTACTS_PER_PAGE;
        let end = (start + CONTACTS_PER_PAGE).min(list.shown.len());
        let total_shown = list.shown.len();
        let rows = list.shown[start..end].iter().map(|&index| {
            self.contact_row(&key, &f.id, &a.id, index, &list.rows[index], index != list.shown[start], cx)
        });
        panel
            .child(
                div()
                    .w_full()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .border_t_1()
                    .border_color(rgb(c.border))
                    .children(rows),
            )
        .when(pages > 1, |d| {
            d.child(self.pager(
                &format!("reverse-pages-{key}"),
                page,
                pages,
                t!("forwarding-of-contacts", first = start + 1, end = end, total_shown = total_shown),
                Action::ContactPage,
                cx,
            ).px_0())
        })
    }
    /// One contact, at the list's fixed row height, with how much mail it
    /// has had. Its reverse alias is never shown; it can be copied.
    fn contact_row(
        &self,
        key: &str,
        service: &str,
        alias: &str,
        index: usize,
        row: &ContactRow,
        divided: bool,
        cx: &Context<Self>,
    ) -> AnyElement {
        let c = self.colors();
        let blocked = row.contact.blocked;
        div()
            .id(SharedString::from(format!("reverse-{key}-{index}")))
            .h(px(CONTACT_ROW))
            .w_full()
            .min_w_0()
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap_2()
            .when(divided, |d| d.border_t_1().border_color(rgb(c.border)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .text_size(px(12.))
                            .font_weight(FontWeight::MEDIUM)
                            .truncate()
                            .when(blocked, |d| d.text_color(rgb(c.muted)))
                            .child(row.title.clone()),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .min_w_0()
                            .text_size(px(11.))
                            .text_color(rgb(c.muted))
                            .child(div().min_w_0().truncate().child(row.hidden.clone()))
                            .when(blocked, |d| d.child(self.pill(t!("forwarding-blocked"), Tone::Danger)))
                            .when(!row.when.is_empty(), |d| {
                                d.child(div().flex_shrink_0().child(row.when.clone()))
                            }),
                    ),
            )
            .when(!row.received.is_empty(), |d| {
                d.child(
                    div()
                        .debug_selector(|| "reverse-stats".into())
                        .flex_shrink_0()
                        .flex()
                        .flex_col()
                        .items_end()
                        .gap(px(2.))
                        .text_size(px(11.))
                        .text_color(rgb(c.muted))
                        .child(row.received.clone())
                        .child(row.sent.clone()),
                )
            })
            .child(self.fwd_icon_button(
                format!("reverse-copy-{key}-{index}"),
                "copy",
                t!("forwarding-copy-reverse-alias"),
                Action::CopyText(row.contact.address.clone()),
                cx,
            ))
            .child(self.fwd_icon_button(
                format!("reverse-menu-{key}-{index}"),
                "ellipsis",
                t!("forwarding-more"),
                Action::ContactMenu(service.into(), alias.into(), row.contact.id.clone()),
                cx,
            ))
            .into_any_element()
    }

    /// Shows or hides an alias's reverse aliases. Shown, any already known
    /// appear at once while they are fetched again.
    pub(super) fn toggle_reverse_aliases(
        &mut self,
        id: String,
        alias: String,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = format!("{id}:{alias}");
        if self.fwd_reverse.as_deref() == Some(key.as_str()) {
            // Forwarding settings' scroll was left alone, so this is back
            // where the alias was in the list.
            self.fwd_reverse = None;
            return;
        }
        self.fwd_reverse = Some(key);
        self.fwd_reverse_scroll.set_offset(point(px(0.), px(0.)));
        Self::set(&self.fwd_contact_search, "", w, cx);
        Self::set(&self.fwd_contact_new, "", w, cx);
        self.filter_contacts("");
        self.fwd_contact_page = 0;
        self.load_reverse_aliases(id, alias, cx);
    }
    pub(super) fn load_reverse_aliases(&mut self, id: String, alias: String, cx: &mut Context<Self>) {
        let Some(account) = self.forwarding.iter().find(|f| f.id == id).cloned() else {
            return;
        };
        let Some(a) = account.aliases.iter().find(|a| a.id == alias).cloned() else {
            return;
        };
        let key = format!("{id}:{alias}");
        let busy = format!("contacts:{key}");
        if !self.fwd_busy.insert(busy.clone()) {
            return;
        }
        self.task(
            cx,
            move |b| {
                let list = fwd::reverse_aliases(&b, &account, &a)?;
                // Mail sent to these went out as the alias.
                let _ = fwd::remember_routes(
                    &b.store,
                    &a.email,
                    list.iter().map(|r| r.address.clone()),
                );
                // And show as these contacts on mail of a mailbox kept to its aliases.
                let _ = fwd::remember_contacts(
                    &b.store,
                    list.iter().map(|r| (r.address.clone(), r.contact.clone())),
                );
                Ok(list)
            },
            move |this, result, cx| {
                this.fwd_busy.remove(&busy);
                match result {
                    Ok(list) => {
                        this.alias_routes = fwd::routes(&this.backend.store);
                        this.reverse_contacts =
                            Arc::new(fwd::reverse_contacts(&this.backend.store));
                        let query = if this.fwd_reverse.as_deref() == Some(key.as_str()) {
                            Self::value(&this.fwd_contact_search, cx)
                        } else {
                            String::new()
                        };
                        this.fwd_contacts.insert(key, Ok(Contacts::new(list, &query)));
                    }
                    // Keep the ones already listed, and say why they weren't refreshed.
                    Err(e) if matches!(this.fwd_contacts.get(&key), Some(Ok(_))) => {
                        this.notice(format!("{e:#}"), true, cx);
                    }
                    Err(e) => {
                        this.fwd_contacts.insert(key, Err(format!("{e:#}")));
                    }
                }
                cx.notify();
            },
        );
    }
    /// Narrows the open reverse aliases to contacts matching `query`.
    pub(super) fn filter_contacts(&mut self, query: &str) {
        if let Some(key) = self.fwd_reverse.as_ref()
            && let Some(Ok(list)) = self.fwd_contacts.get_mut(key)
        {
            list.filter(query);
        }
    }
    fn contact_mut(&mut self, key: &str, contact: &str) -> Option<&mut ReverseAlias> {
        match self.fwd_contacts.get_mut(key) {
            Some(Ok(list)) => list
                .rows
                .iter_mut()
                .find(|r| r.contact.id == contact)
                .map(|r| &mut r.contact),
            _ => None,
        }
    }
    /// A contact's menu: copy its reverse alias, block it, delete it.
    pub(super) fn contact_entries(&self, id: &str, alias: &str, contact: &str) -> Vec<Entry> {
        let key = format!("{id}:{alias}");
        let Some(Ok(list)) = self.fwd_contacts.get(&key) else {
            return vec![];
        };
        let Some(row) = list.rows.iter().find(|r| r.contact.id == contact) else {
            return vec![];
        };
        vec![
            entry(
                t!("forwarding-copy-reverse-alias-2"),
                "copy",
                Action::CopyText(row.contact.address.clone()),
            ),
            entry(
                if row.contact.blocked {
                    t!("forwarding-unblock-contact")
                } else {
                    t!("forwarding-block-contact")
                },
                "ban",
                Action::ToggleContactBlock(id.into(), alias.into(), contact.into()),
            ),
            Entry::Separator,
            danger(
                t!("forwarding-delete-contact"),
                "trash",
                Action::DeleteContact(id.into(), alias.into(), contact.into()),
            ),
        ]
    }
    /// Blocks or unblocks mail from a contact to the alias. Shown at once;
    /// put back if SimpleLogin refuses.
    pub(super) fn toggle_contact_block(
        &mut self,
        id: String,
        alias: String,
        contact: String,
        cx: &mut Context<Self>,
    ) {
        let key = format!("{id}:{alias}");
        let Some(account) = self.forwarding.iter().find(|f| f.id == id).cloned() else {
            return;
        };
        let Some(before) = self.contact_mut(&key, &contact).map(|c| c.clone()) else {
            return;
        };
        let busy = format!("contact:{key}:{contact}");
        if !self.fwd_busy.insert(busy.clone()) {
            return;
        }
        let was = before.blocked;
        if let Some(c) = self.contact_mut(&key, &contact) {
            c.blocked = !was;
        }
        self.task(
            cx,
            move |b| fwd::toggle_contact(&b, &account, &before),
            move |this, result, cx| {
                this.fwd_busy.remove(&busy);
                let blocked = match result {
                    // As SimpleLogin says it is now.
                    Ok(blocked) => blocked,
                    Err(e) => {
                        this.notice(format!("{e:#}"), true, cx);
                        was
                    }
                };
                if let Some(c) = this.contact_mut(&key, &contact) {
                    c.blocked = blocked;
                }
                cx.notify();
            },
        );
    }
    /// Deletes a contact and its reverse alias; the first call asks.
    pub(super) fn delete_contact(
        &mut self,
        id: String,
        alias: String,
        contact: String,
        cx: &mut Context<Self>,
    ) {
        let key = format!("{id}:{alias}");
        let confirm = format!("contact:{key}:{contact}");
        if self.fwd_confirm.as_deref() != Some(confirm.as_str()) {
            self.fwd_confirm = Some(confirm);
            return;
        }
        self.fwd_confirm = None;
        let Some(account) = self.forwarding.iter().find(|f| f.id == id).cloned() else {
            return;
        };
        let Some(gone) = self.contact_mut(&key, &contact).map(|c| c.clone()) else {
            return;
        };
        if !self.fwd_busy.insert(confirm.clone()) {
            return;
        }
        self.task(
            cx,
            move |b| fwd::delete_contact(&b, &account, &gone),
            move |this, result, cx| {
                this.fwd_busy.remove(&confirm);
                match result {
                    Ok(()) => {
                        let query = Self::value(&this.fwd_contact_search, cx);
                        if let Some(Ok(list)) = this.fwd_contacts.get_mut(&key) {
                            list.remove(&contact, &query);
                        }
                        this.notice(t!("forwarding-contact-deleted"), false, cx);
                    }
                    Err(e) => this.notice(format!("{e:#}"), true, cx),
                }
                cx.notify();
            },
        );
    }
    /// Makes (or finds) the reverse alias for writing to the address typed
    /// in the panel, lists it first and copies it.
    pub(super) fn add_contact(
        &mut self,
        id: String,
        alias: String,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = format!("{id}:{alias}");
        let address = Self::value(&self.fwd_contact_new, cx).trim().to_owned();
        if !address.contains('@') {
            self.notice(t!("forwarding-enter-the-address-you-want-to"), true, cx);
            return;
        }
        let Some(account) = self.forwarding.iter().find(|f| f.id == id).cloned() else {
            return;
        };
        let Some(a) = account.aliases.iter().find(|a| a.id == alias).cloned() else {
            return;
        };
        let busy = format!("contact-add:{key}");
        if !self.fwd_busy.insert(busy.clone()) {
            return;
        }
        Self::set(&self.fwd_contact_new, "", w, cx);
        self.task(
            cx,
            move |b| {
                let contact =
                    fwd::add_contact(&b, &account, &a, &address).map_err(|e| e.context(address))?;
                let _ = fwd::remember_contacts(
                    &b.store,
                    [(contact.address.clone(), contact.contact.clone())],
                );
                Ok(contact)
            },
            move |this, result, cx| {
                this.fwd_busy.remove(&busy);
                match result {
                    Ok(contact) => {
                        this.reverse_contacts =
                            Arc::new(fwd::reverse_contacts(&this.backend.store));
                        cx.write_to_clipboard(ClipboardItem::new_string(contact.address.clone()));
                        this.notice(
                            t!("forwarding-reverse-alias-for-ready-copied", contact = contact.contact),
                            false,
                            cx,
                        );
                        let query = Self::value(&this.fwd_contact_search, cx);
                        if let Some(Ok(list)) = this.fwd_contacts.get_mut(&key) {
                            list.add(contact, &query);
                        }
                    }
                    Err(e) => this.notice(format!("{e:#}"), true, cx),
                }
                cx.notify();
            },
        );
    }

    /// Edits an alias's description or display name, below its row.
    pub(super) fn edit_alias(
        &mut self,
        id: String,
        alias: String,
        which: AliasField,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(a) = self
            .forwarding
            .iter()
            .find(|f| f.id == id)
            .and_then(|f| f.aliases.iter().find(|a| a.id == alias))
        else {
            return;
        };
        let value = match which {
            AliasField::Note => a.note.clone(),
            AliasField::Name => a.name.clone(),
        };
        self.fwd_editing = Some((id, alias, which));
        self.fwd_description.update(cx, |s, cx| {
            s.set_placeholder(which.placeholder(), w, cx);
            s.set_text(value, w, cx);
            s.focus(w, cx);
        });
    }
    /// Saves the field being edited, at the service.
    pub(super) fn save_alias_edit(&mut self, cx: &mut Context<Self>) {
        let Some((id, alias, which)) = self.fwd_editing.take() else {
            return;
        };
        let value = Self::value(&self.fwd_description, cx).trim().to_owned();
        let Some(current) = self
            .forwarding
            .iter()
            .find(|f| f.id == id)
            .and_then(|f| f.aliases.iter().find(|a| a.id == alias))
            .map(|a| match which {
                AliasField::Note => a.note.clone(),
                AliasField::Name => a.name.clone(),
            })
        else {
            return;
        };
        if current == value {
            return;
        }
        let shown = value.clone();
        match which {
            AliasField::Note => self.change_alias(
                &id,
                &alias,
                move |a| a.note = shown,
                move |b, f, a| fwd::set_note(b, f, a, &value),
                cx,
            ),
            AliasField::Name => self.change_alias(
                &id,
                &alias,
                move |a| a.name = shown,
                move |b, f, a| fwd::set_name(b, f, a, &value),
                cx,
            ),
        }
    }
    pub(super) fn set_alias_pinned(
        &mut self,
        id: String,
        alias: String,
        pinned: bool,
        cx: &mut Context<Self>,
    ) {
        self.change_alias(
            &id,
            &alias,
            move |a| a.pinned = pinned,
            move |b, f, a| fwd::set_pinned(b, f, a, pinned),
            cx,
        );
    }
    pub(super) fn set_alias_pgp(&mut self, id: String, alias: String, on: bool, cx: &mut Context<Self>) {
        self.change_alias(
            &id,
            &alias,
            move |a| a.pgp = Some(on),
            move |b, f, a| fwd::set_pgp(b, f, a, on),
            cx,
        );
    }
    /// The service's mailboxes (or recipients), checked where the alias
    /// forwards. An addy.io alias with none uses the default recipient.
    pub(super) fn alias_mailbox_entries(&self, id: &str, alias: &str) -> Vec<Entry> {
        let Some((f, a)) = self.forwarding.iter().find(|f| f.id == id).and_then(|f| {
            f.aliases
                .iter()
                .find(|a| a.id == alias)
                .map(|a| (f, a))
        }) else {
            return vec![];
        };
        let mut entries = Vec::new();
        if f.service == ForwardingService::Addy {
            entries.push(checked(
                t!("forwarding-default-recipient"),
                a.mailboxes.is_empty(),
                Action::ToggleAliasMailbox(id.into(), alias.into(), String::new()),
            ));
            entries.push(Entry::Separator);
        }
        entries.extend(f.mailboxes.iter().map(|m| {
            checked(
                m.email.clone(),
                a.mailboxes.contains(&m.id),
                Action::ToggleAliasMailbox(id.into(), alias.into(), m.id.clone()),
            )
        }));
        entries
    }
    /// Adds a mailbox to the alias's, or takes it away. A SimpleLogin alias
    /// keeps at least one; an empty `mailbox` (addy.io) means the default.
    pub(super) fn toggle_alias_mailbox(
        &mut self,
        id: String,
        alias: String,
        mailbox: String,
        cx: &mut Context<Self>,
    ) {
        let Some((service, current)) = self.forwarding.iter().find(|f| f.id == id).and_then(|f| {
            f.aliases
                .iter()
                .find(|a| a.id == alias)
                .map(|a| (f.service, a.mailboxes.clone()))
        }) else {
            return;
        };
        let mut next = current;
        if mailbox.is_empty() {
            next.clear();
        } else if let Some(i) = next.iter().position(|m| *m == mailbox) {
            next.remove(i);
        } else {
            next.push(mailbox);
        }
        if next.is_empty() && service.needs_mailbox() {
            self.notice(t!("forwarding-an-alias-needs-at-least-one"), true, cx);
            return;
        }
        let ids = next.clone();
        self.change_alias(
            &id,
            &alias,
            move |a| a.mailboxes = next,
            move |b, f, a| fwd::set_mailboxes(b, f, a, &ids),
            cx,
        );
    }
    /// Changes an alias here at once and then at its service, putting it
    /// back if the service refuses.
    fn change_alias(
        &mut self,
        id: &str,
        alias: &str,
        change: impl FnOnce(&mut Alias),
        at_service: impl FnOnce(&Backend, &ForwardingAccount, &Alias) -> anyhow::Result<()> + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        let Some(account) = self.forwarding.iter().find(|f| f.id == id).cloned() else {
            return;
        };
        let Some(before) = account.aliases.iter().find(|a| a.id == alias).cloned() else {
            return;
        };
        let busy = format!("{id}:{alias}");
        if !self.fwd_busy.insert(busy.clone()) {
            return;
        }
        let mut after = before.clone();
        change(&mut after);
        self.replace_alias(id, &after);
        let id = id.to_owned();
        self.task(
            cx,
            move |b| at_service(&b, &account, &after),
            move |this, result, cx| {
                this.fwd_busy.remove(&busy);
                if let Err(e) = result {
                    this.replace_alias(&id, &before);
                    this.note_pass_sign_in(&id, &e);
                    this.notice(format!("{e:#}"), true, cx);
                }
                let _ = fwd::save(&this.backend.store, &this.forwarding);
                this.update_composer_aliases(cx);
                cx.notify();
            },
        );
    }
    fn replace_alias(&mut self, id: &str, alias: &Alias) {
        if let Some(slot) = self
            .forwarding
            .iter_mut()
            .find(|f| f.id == id)
            .and_then(|f| f.aliases.iter_mut().find(|a| a.id == alias.id))
        {
            *slot = alias.clone();
        }
    }

    /// The menu of a service's card.
    pub(super) fn service_entries(&self, id: &str) -> Vec<Entry> {
        let pass = self
            .forwarding
            .iter()
            .any(|f| f.id == id && f.service.uses_proton_account())
            && crate::sync::proton::pass_sign_in();
        let mut entries = vec![entry(
            t!("forwarding-refresh-aliases"),
            "refresh",
            Action::RefreshAliases(id.into()),
        )];
        if pass {
            entries.push(entry(t!("forwarding-sign-in-to-proton-pass-again"), "lock", Action::PassSignIn(id.into())));
        }
        entries.extend([
            Entry::Separator,
            danger(
                t!("forwarding-remove-service"),
                "trash",
                Action::RemoveForwarding(id.into()),
            ),
        ]);
        entries
    }
    /// Where a service's aliases deliver: any mailbox, or one account.
    pub(super) fn deliver_entries(&self, id: &str) -> Vec<Entry> {
        let current = self
            .forwarding
            .iter()
            .find(|f| f.id == id)
            .and_then(|f| f.deliver_to.clone());
        let mut entries = vec![
            checked(
                t!("forwarding-any-mailbox"),
                current.is_none(),
                Action::SetDeliverTo(id.into(), None),
            ),
            Entry::Separator,
        ];
        entries.extend(tree::sidebar_ordered(&self.tree, &self.accounts).into_iter().map(|(_, a)| {
            checked(
                a.email.clone(),
                current.as_deref() == Some(a.id.as_str()),
                Action::SetDeliverTo(id.into(), Some(a.id.clone())),
            )
        }));
        entries
    }
    /// The menu of an alias's row: what its service lets be changed.
    pub(super) fn alias_entries(&self, id: &str, alias: &str) -> Vec<Entry> {
        let Some((f, a)) = self.forwarding.iter().find(|f| f.id == id).and_then(|f| {
            f.aliases
                .iter()
                .find(|a| a.id == alias)
                .map(|a| (f, a))
        }) else {
            return vec![];
        };
        let show = |label: &str, display: AliasDisplay| {
            checked(
                label.to_owned(),
                a.display == display,
                Action::SetAliasDisplay(id.into(), alias.into(), display),
            )
        };
        // One of a Proton account's own addresses: a name, on or off, and
        // deleting, as Proton's address settings; the rest is Pass's.
        let address = a.is_address();
        let mut entries = vec![entry(t!("forwarding-copy-address"), "copy", Action::CopyText(a.email.clone()))];
        if !address {
            entries.push(entry(
                if a.note.is_empty() {
                    t!("forwarding-add-description")
                } else {
                    t!("forwarding-edit-description")
                },
                "compose",
                Action::EditAlias(id.into(), alias.into(), AliasField::Note),
            ));
        }
        if f.service.has_display_name() {
            entries.push(entry(
                if a.name.is_empty() {
                    t!("forwarding-set-display-name")
                } else {
                    t!("forwarding-edit-display-name")
                },
                "user",
                Action::EditAlias(id.into(), alias.into(), AliasField::Name),
            ));
        }
        if f.service.can_pin() && !address {
            entries.push(entry(
                if a.pinned { t!("forwarding-unpin") } else { t!("forwarding-pin") },
                "pin",
                Action::SetAliasPinned(id.into(), alias.into(), !a.pinned),
            ));
        }
        if let Some(label) = f.service.mailboxes_label()
            && !f.mailboxes.is_empty()
            && !address
        {
            entries.push(entry(
                format!("{label}…"),
                "inbox",
                Action::AliasMailboxMenu(id.into(), alias.into()),
            ));
        }
        if let Some(on) = a.pgp {
            entries.push(checked(
                t!("forwarding-encrypt-with-pgp"),
                on,
                Action::SetAliasPgp(id.into(), alias.into(), !on),
            ));
        }
        if f.service.has_contacts() && !address {
            let open = self.fwd_reverse.as_deref() == Some(format!("{id}:{alias}").as_str());
            entries.push(entry(
                if open {
                    t!("forwarding-hide-reverse-aliases")
                } else {
                    t!("forwarding-show-reverse-aliases")
                },
                "reply",
                Action::ToggleReverseAliases(id.into(), alias.into()),
            ));
        }
        entries.extend([
            Entry::Separator,
            show(t!("forwarding-show-in-its-mailbox"), AliasDisplay::Included),
            show(t!("forwarding-show-as-its-own-account"), AliasDisplay::Separate),
            Entry::Separator,
            danger(
                if address { t!("forwarding-delete-address") } else { t!("forwarding-delete-alias") },
                "trash",
                Action::DeleteAlias(id.into(), alias.into()),
            ),
        ]);
        entries
    }
}
fn fwd_server_label(f: &ForwardingAccount) -> String {
    let server = if f.server.is_empty() {
        f.service.default_server()
    } else {
        &f.server
    };
    server
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .to_owned()
}

/// Signing Proton's aliases in to Proton Pass again, when Proton ended the
/// Pass session: a dialog with the same steps as signing in to a Proton
/// Mail account again (`proton_login.rs`), plus Pass's extra password.
impl MailApp {
    /// Notes a failure of a Proton service that needs Pass signed in to
    /// again, so its card offers to.
    pub(super) fn note_pass_sign_in(&mut self, id: &str, e: &anyhow::Error) {
        if e.downcast_ref::<fwd::PassSignInNeeded>().is_some() {
            self.fwd_pass_expired.insert(id.to_owned());
        }
    }
    /// What a service whose Pass session ended shows on its card.
    fn pass_expired_strip(&self, f: &ForwardingAccount, cx: &Context<Self>) -> Stateful<Div> {
        let c = self.colors();
        div()
            .id(SharedString::from(format!("forwarding-pass-expired-{}", f.id)))
            .mx_4()
            .mb_3()
            .px_3()
            .py_2()
            .rounded(px(9.))
            .bg(rgba((c.warning << 8) | 0x14))
            .border_1()
            .border_color(rgba((c.warning << 8) | 0x55))
            .flex()
            .flex_wrap()
            .items_center()
            .gap_2()
            .child(
                div()
                    .flex_1()
                    .min_w(px(160.))
                    .text_size(px(12.))
                    .child(t!("forwarding-sign-in-to-proton-pass-to-manage")),
            )
            .child(self.small_button(
                format!("forwarding-pass-sign-in-{}", f.id),
                t!("forwarding-sign-in"),
                "",
                Action::PassSignIn(f.id.clone()),
                cx,
            ))
    }
    pub(super) fn open_pass_sign_in(&mut self, id: String, w: &mut Window, cx: &mut Context<Self>) {
        if self.connecting || !self.forwarding.iter().any(|f| f.id == id) {
            return;
        }
        self.drop_proton_login(cx);
        for i in [PASSWORD, TOTP, MAILBOX_PASSWORD, KEY_PIN] {
            Self::set(&self.form[i], "", w, cx);
        }
        Self::set(&self.fwd_pass_password, "", w, cx);
        self.status.clear();
        self.error = false;
        self.modal = Some(Modal::PassSignIn(id));
        self.anim += 1;
        self.form[PASSWORD].update(cx, |s, cx| s.focus(w, cx));
    }
    /// The next step: the password (and extra password), then whatever
    /// Proton asks for, as signing in to Proton Mail again does.
    pub(super) fn submit_pass_sign_in(&mut self, id: String, w: &mut Window, cx: &mut Context<Self>) {
        if self.connecting || self.fwd_busy.contains(&format!("pass:{id}")) {
            return;
        }
        if self.proton_stage.is_some() && self.fwd_pass_again.as_deref() == Some(id.as_str()) {
            self.proton_continue(None, cx);
            return;
        }
        let Some(account) = self
            .forwarding
            .iter()
            .find(|f| f.id == id)
            .and_then(|f| self.accounts.iter().find(|a| a.id == f.proton_account))
            .cloned()
        else {
            self.notice(t!("views-sign-in-to-proton-directly-first"), true, cx);
            return;
        };
        let password = Self::value(&self.form[PASSWORD], cx);
        if password.is_empty() {
            self.notice(t!("proton-login-enter-your-proton-password"), true, cx);
            return;
        }
        // Neither password stays in its field once sent.
        self.fwd_pending_extra = zeroize::Zeroizing::new(Self::value(&self.fwd_pass_password, cx));
        Self::set(&self.form[PASSWORD], "", w, cx);
        Self::set(&self.fwd_pass_password, "", w, cx);
        self.fwd_pass_again = Some(id);
        self.start_proton(None, cx, move |b, cancel| b.proton_pass_begin(&account, password, cancel));
    }
    /// Keeps the Pass session just signed in for the service, lists its
    /// aliases with it, and goes back to Forwarding settings.
    pub(super) fn pass_signed_in_again(&mut self, id: String, cx: &mut Context<Self>) {
        let Some(account) = self.forwarding.iter().find(|f| f.id == id).cloned() else {
            return;
        };
        let busy = format!("pass:{id}");
        if !self.fwd_busy.insert(busy.clone()) {
            return;
        }
        let extra = std::mem::take(&mut self.fwd_pending_extra);
        self.task(
            cx,
            move |b| {
                let pass = b
                    .take_proton_pass_session(&account.proton_account)
                    .ok_or_else(|| anyhow::anyhow!(t!("sync-proton-aliases-sign-in-to-proton-pass")))?;
                fwd::sign_in_to_pass_again(&b, &account, pass, &extra)
            },
            move |this, result, cx| {
                this.fwd_busy.remove(&busy);
                match result {
                    Ok(fresh) => {
                        this.fwd_pass_expired.remove(&id);
                        if let Some(f) = this.forwarding.iter_mut().find(|f| f.id == id) {
                            if let Some(mailboxes) = fresh.mailboxes {
                                f.mailboxes = mailboxes;
                            }
                            f.aliases = fresh.aliases;
                        }
                        this.views_changed(cx);
                        this.status.clear();
                        this.modal = Some(Modal::Settings);
                        this.page = Page::Forwarding;
                        this.fwd_open.insert(id);
                        this.notice(t!("forwarding-signed-in-to-proton-pass"), false, cx);
                    }
                    Err(e) => this.notice(format!("{e:#}"), true, cx),
                }
                cx.notify();
            },
        );
    }
    /// The dialog: the account, then Proton Mail's sign-in fields for the
    /// step on screen; at the password, Pass's extra password too.
    pub(super) fn pass_sign_in_form(&self, id: &str, cx: &Context<Self>) -> AnyElement {
        let c = self.colors();
        let email = self
            .forwarding
            .iter()
            .find(|f| f.id == id)
            .and_then(|f| self.accounts.iter().find(|a| a.id == f.proton_account))
            .map(|a| a.email.clone())
            .unwrap_or_default();
        let content = div().w_full().flex().flex_col().gap_3().child(div().text_size(px(13.)).child(email));
        if let Some(step) = self.proton_step_view(true, cx).filter(|_| self.fwd_pass_again.as_deref() == Some(id)) {
            return content.child(step).into_any_element();
        }
        let busy = self.connecting;
        content
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(c.muted))
                    .child(t!("forwarding-proton-ended-the-pass-session-sign-in")),
            )
            .child(self.field(t!("forwarding-proton-password"), PASSWORD))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(12.))
                            .font_weight(FontWeight::MEDIUM)
                            .child(t!("forwarding-proton-pass-extra-password")),
                    )
                    .child(
                        div()
                            .id("pass-sign-in-extra")
                            .child(field(Input::new(&self.fwd_pass_password), c, busy)),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(rgb(c.muted))
                            .child(t!("forwarding-only-if-you-protect-proton-pass")),
                    ),
            )
            .into_any_element()
    }
}

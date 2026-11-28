//! The reader's contact panel: a collapsible drawer beside the open
//! message with the other person of it (who sent it, or whom you sent it
//! to): their contact card, their latest mail, the attachments of that
//! mail, and the events with them ahead, together. It sits beside the
//! email where the reader has room, and over its right edge where it
//! doesn't. What it lists is read off the window's thread
//! (`manage::attachments::Person`), from the workspace's accounts,
//! address books and calendars.
use super::*;
use crate::manage::attachments::{self as lib, Item, Person};

/// The panel's width.
pub(super) const PANEL_WIDTH: f32 = 300.;
/// The reader's least width with the panel beside the email; narrower, it
/// covers the email's right edge.
const BESIDE: f32 = PANEL_WIDTH + 540.;
/// The preference keeping whether the panel is open ("1") or not.
const PREFERENCE: &str = "contact_panel";

/// The panel's state.
pub(super) struct ContactPanel {
    pub(super) open: bool,
    /// What it shows, for which message (its id).
    person: Option<Person>,
    for_message: String,
    /// The message whose person is being read.
    loading: Option<String>,
    generation: u64,
}

impl ContactPanel {
    pub(super) fn new(store: &crate::manage::store::Store) -> Self {
        Self {
            open: store.preference(PREFERENCE).as_deref() == Some("1"),
            person: None,
            for_message: String::new(),
            loading: None,
            generation: 0,
        }
    }
}

/// What the panel does.
#[derive(Clone)]
pub(super) enum PanelAction {
    /// Opens or closes the panel.
    Toggle,
    /// Opens the Calendar on the day an event starts (its start).
    OpenEvent(i64),
    /// Opens their card for editing in Contacts (its UID), or a new card
    /// with their name and address.
    EditContact(String),
    AddContact(String, String),
    /// Opens an attachment of theirs with its app.
    OpenAttachment(Item),
}

impl MailApp {
    pub(super) fn panel_action(&mut self, action: PanelAction, w: &mut Window, cx: &mut Context<Self>) {
        match action {
            PanelAction::Toggle => {
                self.contact_panel.open = !self.contact_panel.open;
                self.backend
                    .store
                    .set_preference_soon(PREFERENCE, if self.contact_panel.open { "1" } else { "0" });
            }
            PanelAction::OpenEvent(start) => {
                use calendar_views::{AppView, CalAction};
                self.cal_action(CalAction::Open(AppView::Calendar), w, cx);
                if let Some(day) = chrono::DateTime::from_timestamp(start, 0) {
                    self.cal_action(CalAction::OpenDay(day.with_timezone(&chrono::Local).date_naive()), w, cx);
                }
            }
            PanelAction::EditContact(uid) => {
                use calendar_views::{AppView, CalAction};
                // Their card is read again once the message shows again.
                self.forget_contact_panel();
                self.cal_action(CalAction::Open(AppView::Contacts), w, cx);
                self.handle(Action::EditContact(uid), w, cx);
            }
            PanelAction::AddContact(name, address) => {
                use calendar_views::{AppView, CalAction};
                self.forget_contact_panel();
                self.cal_action(CalAction::Open(AppView::Contacts), w, cx);
                self.handle(Action::NewContact, w, cx);
                Self::set(&self.contact_name, name, w, cx);
                Self::set(&self.contact_emails, address, w, cx);
            }
            PanelAction::OpenAttachment(item) => self.library_open_file(item, false, cx),
        }
        cx.notify();
    }

    /// Has the panel read the open message's person again when it next shows.
    fn forget_contact_panel(&mut self) {
        self.contact_panel.for_message.clear();
        self.contact_panel.loading = None;
        self.contact_panel.generation += 1;
    }

    /// Whether the panel shows beside the reader now (on a desktop, open,
    /// with the reader beside the list rather than alone in the window).
    pub(super) fn shows_contact_panel(&self, single: bool) -> bool {
        self.contact_panel.open && !self.mobile.on && !single
    }

    /// Whether the panel neither shows the person of the message `id` nor
    /// is reading them.
    pub(super) fn contact_panel_stale(&self, id: &str) -> bool {
        let panel = &self.contact_panel;
        panel.for_message != id && panel.loading.as_deref() != Some(id)
    }

    /// Reads the person of `m` for the panel, unless it shows them already
    /// or is reading them.
    pub(super) fn ensure_contact_panel(&mut self, m: &Mail, cx: &mut Context<Self>) {
        if !self.contact_panel_stale(&m.id) {
            return;
        }
        let own: HashSet<String> = self
            .accounts
            .iter()
            .flat_map(|a| std::iter::once(a.email.clone()).chain(a.identities.iter().map(|i| i.email.clone())))
            .map(|e| e.trim().to_lowercase())
            .collect();
        let Some((name, address)) = lib::counterpart(m, &own) else {
            self.contact_panel.person = None;
            self.contact_panel.for_message = m.id.clone();
            self.contact_panel.loading = None;
            return;
        };
        // The person already shown (another message of theirs): kept
        // while they are read again.
        if self.contact_panel.person.as_ref().is_some_and(|p| p.address != address) {
            self.contact_panel.person = None;
        }
        self.contact_panel.generation += 1;
        let generation = self.contact_panel.generation;
        self.contact_panel.loading = Some(m.id.clone());
        let accounts: Vec<String> = self.workspace_accounts().into_iter().map(|a| a.id).collect();
        let workspace = self.workspace().cloned();
        let open = m.id.clone();
        self.task(
            cx,
            move |b| {
                let cards = match workspace.as_ref().filter(|w| w.contacts.is_some()) {
                    Some(w) => b.contacts_in(|id| w.shows_contacts(id)),
                    None => b.contacts(),
                };
                let card = lib::card_for(&cards, &address).cloned();
                let mail = b.store.mail_with(&address, &accounts, lib::PANEL_RECENT * 6)?;
                let now = chrono::Utc::now().timestamp();
                let until = now + lib::PANEL_EVENT_DAYS * 86_400;
                let agenda = match workspace.as_ref().filter(|w| w.calendars.is_some()) {
                    Some(w) => b.agenda_in(now, until, |c| w.shows_calendar(c)),
                    None => b.agenda(now, until),
                };
                let events = lib::events_with(agenda, &address, lib::PANEL_EVENTS);
                Ok(Person::from_mail(address, name, card, mail, &open, events))
            },
            move |this, result, cx| {
                if generation != this.contact_panel.generation {
                    return;
                }
                let panel = &mut this.contact_panel;
                panel.for_message = panel.loading.take().unwrap_or_default();
                match result {
                    Ok(person) => panel.person = Some(person),
                    Err(e) => {
                        panel.person = None;
                        this.notice(format!("{e:#}"), true, cx);
                    }
                }
                cx.notify();
            },
        );
    }

    /// The reader's button that opens or closes the panel.
    pub(super) fn contact_panel_toggle(&self, cx: &Context<Self>) -> Stateful<Div> {
        let open = self.contact_panel.open;
        let label = if open { t!("contact-panel-hide") } else { t!("contact-panel-show") };
        self.button(
            "contact-panel-toggle",
            &shortcuts::tip(label, "toggle-contact-panel", cx),
            if open { "panel-right-close" } else { "panel-right-open" },
            Action::ContactPanel(PanelAction::Toggle),
            false,
            cx,
        )
        .aria_expanded(open)
    }

    /// The panel beside (or, in a narrow reader, over) the email `m`; the
    /// reader is `width` wide.
    pub(super) fn contact_panel(&self, m: &Mail, width: f32, cx: &Context<Self>) -> AnyElement {
        let c = self.colors();
        let over = width < BESIDE;
        let loading = self.contact_panel.loading.as_deref() == Some(m.id.as_str());
        let person = self.contact_panel.person.as_ref().filter(|_| self.contact_panel.for_message == m.id || loading);
        let content: AnyElement = match person {
            Some(p) => self.panel_person(p, cx).into_any_element(),
            None if loading || self.contact_panel.for_message != m.id => div()
                .flex()
                .justify_center()
                .py_8()
                .child(spinner(18., c.muted))
                .into_any_element(),
            None => div()
                .py_6()
                .text_size(px(12.))
                .text_color(rgb(c.muted))
                .child(t!("contact-panel-nobody"))
                .into_any_element(),
        };
        div()
            .id("contact-panel")
            .debug_selector(|| "contact-panel".into())
            .role(Role::Complementary)
            .aria_label(t!("contact-panel-contact"))
            .w(px(PANEL_WIDTH))
            .flex_shrink_0()
            .h_full()
            .flex()
            .flex_col()
            .bg(rgb(c.surface))
            .border_l_1()
            .border_color(rgb(c.border))
            .when(over, |d| d.absolute().top_0().right_0().bottom_0().shadow_lg())
            // The email underneath mustn't take presses meant for the panel.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .h(px(44.))
                    .flex_shrink_0()
                    .pl_4()
                    .pr_2()
                    .flex()
                    .items_center()
                    .gap_2()
                    .border_b_1()
                    .border_color(rgb(c.border))
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(12.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(c.muted))
                            .child(t!("contact-panel-contact")),
                    )
                    .when(loading && person.is_some(), |d| d.child(spinner(12., c.muted)))
                    .child(
                        self.button(
                            "contact-panel-close",
                            t!("contact-panel-hide"),
                            "panel-right-close",
                            Action::ContactPanel(PanelAction::Toggle),
                            false,
                            cx,
                        )
                        .h(px(30.)),
                    ),
            )
            .child(
                div()
                    .id("contact-panel-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_4()
                    .pb_6()
                    .child(content),
            )
            .into_any_element()
    }

    fn panel_person(&self, p: &Person, cx: &Context<Self>) -> Div {
        let c = self.colors();
        let label = |text: &'static str| {
            div()
                .mt_5()
                .mb_1()
                .text_size(px(11.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(c.muted))
                .child(text.to_uppercase())
        };
        let row = |id: String, action: Action| {
            div()
                .id(SharedString::from(id.clone()))
                .debug_selector(move || id.clone())
                .role(Role::Button)
                .px_2()
                .py(px(6.))
                .mx(px(-8.))
                .rounded(px(7.))
                .flex()
                .items_center()
                .gap_2()
                .cursor_pointer()
                .hover(move |s| s.bg(rgb(c.raised)))
                .on_click(cx.listener(move |this, _, w, cx| this.handle(action.clone(), w, cx)))
        };
        let empty = |text: &'static str| div().py_1().text_size(px(12.)).text_color(rgb(c.muted)).child(text);
        let name = if p.name.trim().is_empty() { p.address.clone() } else { p.name.clone() };
        let card = p.card.as_ref();
        let facts: Vec<(&'static str, String)> = card
            .map(|card| {
                let mut facts = Vec::new();
                let work = [card.title.trim(), card.org.trim()]
                    .into_iter()
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
                    .join(" · ");
                if !work.is_empty() {
                    facts.push(("building", work));
                }
                facts.extend(card.phones.iter().take(2).map(|ph| ("phone", ph.clone())));
                facts.extend(
                    card.emails
                        .iter()
                        .filter(|e| !e.eq_ignore_ascii_case(&p.address))
                        .take(2)
                        .map(|e| ("at-sign", e.clone())),
                );
                if !card.birthday.trim().is_empty() {
                    facts.push(("cake", card.birthday.clone()));
                }
                facts
            })
            .unwrap_or_default();
        let compose = Action::ComposeTo(name_and_address(Some(&p.name), &p.address));
        let contact = match card {
            Some(card) => (t!("contact-panel-edit-contact"), "user-pen", PanelAction::EditContact(card.uid.clone())),
            None => (t!("contact-panel-add-contact"), "user-plus", PanelAction::AddContact(p.name.clone(), p.address.clone())),
        };
        let browse = Action::Attachments(super::attachments::LibAction::Open {
            account: ALL_MAIL.into(),
            mailbox: None,
            person: Some(p.address.clone()),
        });
        div()
            .flex()
            .flex_col()
            // Who they are.
            .child(
                div()
                    .pt_4()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(avatar(&initials(&name), 44., 0, c))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .child(
                                div()
                                    .id("contact-panel-name")
                                    .debug_selector(|| "contact-panel-name".into())
                                    .text_size(px(15.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .whitespace_normal()
                                    .child(name.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(11.5))
                                    .text_color(rgb(c.muted))
                                    .truncate()
                                    .child(p.address.clone()),
                            ),
                    ),
            )
            .children(facts.into_iter().map(|(icon, text)| {
                div()
                    .mt_2()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_size(px(12.))
                    .child(icon_el(icon, 13., c.muted))
                    .child(div().min_w_0().truncate().child(text))
            }))
            .child(
                div()
                    .mt_3()
                    .flex()
                    .flex_wrap()
                    .gap_1()
                    .child(
                        self.button("contact-panel-email", t!("contact-panel-email"), "compose", compose, false, cx)
                            .h(px(30.))
                            .text_size(px(12.))
                            .border_1()
                            .border_color(rgb(c.border)),
                    )
                    .child(
                        self.button("contact-panel-card", contact.0, contact.1, Action::ContactPanel(contact.2), false, cx)
                            .h(px(30.))
                            .text_size(px(12.))
                            .border_1()
                            .border_color(rgb(c.border)),
                    ),
            )
            // Their latest mail.
            .child(label(t!("contact-panel-recent-mail")))
            .when(p.recent.is_empty(), |d| d.child(empty(t!("contact-panel-no-other-mail"))))
            .children(p.recent.iter().enumerate().map(|(i, m)| {
                let open = Action::OpenSummaryMail(m.account.clone(), m.mailbox(), m.id.clone());
                let subject = if m.subject.trim().is_empty() { t!("contact-panel-no-subject").to_owned() } else { m.subject.clone() };
                row(format!("contact-panel-mail-{i}"), open)
                    .aria_label(subject.clone())
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .text_size(px(12.5))
                                    .when(m.unread, |d| d.font_weight(FontWeight::SEMIBOLD))
                                    .truncate()
                                    .child(subject),
                            )
                            .child(div().text_size(px(11.)).text_color(rgb(c.muted)).truncate().child(m.snippet.clone())),
                    )
                    .child(div().flex_shrink_0().text_size(px(11.)).text_color(rgb(c.muted)).child(date_short(m.date)))
            }))
            // What they sent or were sent.
            .child(label(t!("contact-panel-shared-attachments")))
            .when(p.attachments.is_empty(), |d| d.child(empty(t!("contact-panel-no-attachments"))))
            .children(p.attachments.iter().enumerate().map(|(i, item)| {
                let a = item.attachment();
                row(format!("contact-panel-attachment-{i}"), Action::ContactPanel(PanelAction::OpenAttachment(item.clone())))
                    .aria_label(a.name.clone())
                    .child(icon_el(item.kind().icon(), 16., c.muted))
                    .child(div().flex_1().min_w_0().text_size(px(12.5)).truncate().child(a.name.clone()))
                    .child(div().flex_shrink_0().text_size(px(11.)).text_color(rgb(c.muted)).child(human_size(a.size as u64)))
            }))
            .when(!p.attachments.is_empty(), |d| {
                d.child(div().mt_1().child(self.link("contact-panel-browse", t!("contact-panel-browse-attachments"), browse, cx)))
            })
            // What is ahead with them.
            .child(label(t!("contact-panel-upcoming-events")))
            .when(p.events.is_empty(), |d| d.child(empty(t!("contact-panel-no-events"))))
            .children(p.events.iter().enumerate().map(|(i, e)| {
                let when = event_when(e);
                row(format!("contact-panel-event-{i}"), Action::ContactPanel(PanelAction::OpenEvent(e.start)))
                    .aria_label(e.summary.clone())
                    .child(icon_el("calendar", 15., c.accent))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(div().text_size(px(12.5)).truncate().child(e.summary.clone()))
                            .child(div().text_size(px(11.)).text_color(rgb(c.muted)).truncate().child(when)),
                    )
            }))
    }
}

#[cfg(test)]
impl MailApp {
    pub(super) fn contact_panel_person(&self) -> Option<&Person> {
        self.contact_panel.person.as_ref()
    }
}

/// When an event is, as the panel lists it: its day, and its time unless
/// it lasts all day.
fn event_when(e: &crate::sync::ical::Event) -> String {
    use crate::i18n::dates::format;
    let Some(start) = chrono::DateTime::from_timestamp(e.start, 0) else {
        return String::new();
    };
    // All-day events start at midnight UTC of their date.
    if e.all_day {
        format(&start, t!("dates-weekday-month-day"))
    } else {
        format(&start.with_timezone(&chrono::Local), t!("dates-weekday-month-day-time"))
    }
}

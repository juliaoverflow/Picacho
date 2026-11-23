//! Settings → account → Filters & vacation: rules the mail server applies
//! as mail arrives, and an automatic reply while away (ManageSieve), plus
//! what the server says about itself (METADATA).
use super::*;
use crate::manage::backend::ServerNotes;
use crate::sync::sieve::{Act, Field, Filters, State};

/// One account's filters as shown in Settings.
#[derive(Default)]
pub(super) struct FiltersUi {
    pub(super) loading: bool,
    pub(super) saving: bool,
    pub(super) error: Option<String>,
    /// What the server has, once loaded.
    pub(super) state: Option<State>,
    /// What is being edited, saved with Save.
    pub(super) draft: Filters,
    /// The new rule's field and action.
    pub(super) new_field: Field,
    pub(super) new_act: Act,
    /// The text fields still show another account's (or nothing): fill
    /// them from `draft` at the next chance.
    pub(super) populate: bool,
    pub(super) changed: bool,
}

/// The text fields of the filters section, shared by account pages (one
/// shows at a time).
pub(super) struct FilterInputs {
    pub(super) value: Entity<InputState>,
    pub(super) subject: Entity<InputState>,
    pub(super) body: Entity<TextareaState>,
    pub(super) from: Entity<InputState>,
    pub(super) until: Entity<InputState>,
    /// The account whose values they show.
    pub(super) shows: Option<String>,
}
impl FilterInputs {
    pub(super) fn new(window: &mut Window, cx: &mut App) -> Self {
        Self {
            value: input("Text to look for", false, window, cx),
            subject: input("Out of office", false, window, cx),
            body: textarea(
                "I'm away until Monday and will reply when I'm back.",
                window,
                cx,
            ),
            from: input("2026-12-20 (optional)", false, window, cx),
            until: input("2026-12-31 (optional)", false, window, cx),
            shows: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum FilterAction {
    Load(String),
    Save(String),
    /// Adds the rule being written.
    Add(String),
    Remove(String, usize),
    NewField(String, Field),
    NewAct(String, Act),
    ToggleStop(String, usize),
    ToggleVacation(String),
    VacationDays(String, u32),
    /// Runs the script that was active before Picacho's first.
    ToggleInclude(String),
}

impl MailApp {
    /// Loads what a newly shown account page needs from the server: its
    /// filters and the server's notes.
    pub(super) fn load_account_extras(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(a) = self.accounts.iter().find(|a| a.id == id).cloned() else {
            return;
        };
        if !(a.provider.imap() || a.provider == Provider::Jmap)
            || a.incoming_host.is_empty()
            || self.account_blocked(id)
        {
            return;
        }
        let notes_for = a.clone();
        let account = id.to_owned();
        self.task(
            cx,
            move |b| b.server_notes(&notes_for),
            move |this, result, cx| {
                if let Ok(notes) = result {
                    this.server_notes.insert(account, notes);
                    cx.notify();
                }
            },
        );
        if Backend::may_filter(&a) && !self.filters.contains_key(id) {
            self.load_filters(id.to_owned(), cx);
        }
    }
    fn load_filters(&mut self, id: String, cx: &mut Context<Self>) {
        let Some(a) = self.accounts.iter().find(|a| a.id == id).cloned() else {
            return;
        };
        let ui = self.filters.entry(id.clone()).or_default();
        if ui.loading {
            return;
        }
        ui.loading = true;
        ui.error = None;
        self.task(
            cx,
            move |b| b.filters(&a),
            move |this, result, cx| {
                let ui = this.filters.entry(id).or_default();
                ui.loading = false;
                match result {
                    Ok(state) => {
                        ui.draft = state.filters.clone();
                        ui.state = Some(state);
                        ui.populate = true;
                        ui.changed = false;
                    }
                    Err(e) => ui.error = Some(format!("{e:#}")),
                }
                cx.notify();
            },
        );
    }
    /// Fills the text fields from the shown account's filters, when they
    /// were loaded or another account's page opened.
    pub(super) fn populate_filter_inputs(
        &mut self,
        id: &str,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let switched = self.filter_inputs.shows.as_deref() != Some(id);
        let Some(ui) = self.filters.get_mut(id) else {
            return;
        };
        if !ui.populate && !switched {
            return;
        }
        ui.populate = false;
        let v = ui.draft.vacation.clone();
        self.filter_inputs.shows = Some(id.to_owned());
        Self::set(&self.filter_inputs.body, &v.body, w, cx);
        let inputs = [
            (self.filter_inputs.subject.clone(), v.subject),
            (self.filter_inputs.from.clone(), v.from.unwrap_or_default()),
            (
                self.filter_inputs.until.clone(),
                v.until.unwrap_or_default(),
            ),
            (self.filter_inputs.value.clone(), String::new()),
        ];
        for (input, value) in inputs {
            Self::set(&input, &value, w, cx);
        }
    }
    /// The vacation reply as typed, into the account's draft.
    fn read_vacation(&mut self, id: &str, cx: &App) {
        let subject = Self::value(&self.filter_inputs.subject, cx);
        let body = Self::value(&self.filter_inputs.body, cx);
        let from = Self::value(&self.filter_inputs.from, cx).trim().to_owned();
        let until = Self::value(&self.filter_inputs.until, cx).trim().to_owned();
        if self.filter_inputs.shows.as_deref() != Some(id) {
            return;
        }
        if let Some(ui) = self.filters.get_mut(id) {
            let v = &mut ui.draft.vacation;
            v.subject = subject;
            v.body = body;
            v.from = (!from.is_empty()).then_some(from);
            v.until = (!until.is_empty()).then_some(until);
        }
    }
    pub(super) fn filter_action(
        &mut self,
        action: FilterAction,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            FilterAction::Load(id) => {
                self.filters.remove(&id);
                self.load_filters(id, cx);
            }
            FilterAction::Save(id) => self.save_filters(id, cx),
            FilterAction::Add(id) => {
                let value = Self::value(&self.filter_inputs.value, cx).trim().to_owned();
                let Some(ui) = self.filters.get_mut(&id) else {
                    return;
                };
                if value.is_empty() {
                    ui.error = Some("Type what the filter looks for.".into());
                    return;
                }
                if ui.new_act == Act::Inbox {
                    ui.error = Some("Choose what the filter does.".into());
                    return;
                }
                ui.draft.rules.push(crate::sync::sieve::Rule {
                    field: ui.new_field,
                    value,
                    act: ui.new_act.clone(),
                    stop: matches!(ui.new_act, Act::Folder(_) | Act::Discard),
                });
                ui.error = None;
                ui.changed = true;
                Self::set(&self.filter_inputs.value, "", w, cx);
            }
            FilterAction::Remove(id, i) => {
                if let Some(ui) = self.filters.get_mut(&id)
                    && i < ui.draft.rules.len()
                {
                    ui.draft.rules.remove(i);
                    ui.changed = true;
                }
            }
            FilterAction::NewField(id, field) => {
                if let Some(ui) = self.filters.get_mut(&id) {
                    ui.new_field = field;
                }
            }
            FilterAction::NewAct(id, act) => {
                if let Some(ui) = self.filters.get_mut(&id) {
                    ui.new_act = act;
                    ui.error = None;
                }
            }
            FilterAction::ToggleStop(id, i) => {
                if let Some(rule) = self
                    .filters
                    .get_mut(&id)
                    .and_then(|ui| ui.draft.rules.get_mut(i))
                {
                    rule.stop = !rule.stop;
                }
                if let Some(ui) = self.filters.get_mut(&id) {
                    ui.changed = true;
                }
            }
            FilterAction::ToggleVacation(id) => {
                self.read_vacation(&id, cx);
                if let Some(ui) = self.filters.get_mut(&id) {
                    let v = &mut ui.draft.vacation;
                    v.on = !v.on;
                    if v.days == 0 {
                        v.days = 7;
                    }
                    ui.changed = true;
                }
            }
            FilterAction::VacationDays(id, days) => {
                if let Some(ui) = self.filters.get_mut(&id) {
                    ui.draft.vacation.days = days;
                    ui.changed = true;
                }
            }
            FilterAction::ToggleInclude(id) => {
                if let Some(ui) = self.filters.get_mut(&id) {
                    let other = ui.state.as_ref().and_then(|s| s.other_active.clone());
                    ui.draft.include = match ui.draft.include {
                        Some(_) => None,
                        None => other,
                    };
                    ui.changed = true;
                }
            }
        }
        cx.notify();
    }
    fn save_filters(&mut self, id: String, cx: &mut Context<Self>) {
        self.read_vacation(&id, cx);
        let Some(a) = self.accounts.iter().find(|a| a.id == id).cloned() else {
            return;
        };
        let Some(ui) = self.filters.get_mut(&id) else {
            return;
        };
        if ui.saving {
            return;
        }
        ui.saving = true;
        ui.error = None;
        let filters = ui.draft.clone();
        self.task(
            cx,
            move |b| b.save_filters(&a, &filters),
            move |this, result, cx| {
                let ui = this.filters.entry(id).or_default();
                ui.saving = false;
                match result {
                    Ok(state) => {
                        ui.draft = state.filters.clone();
                        ui.state = Some(state);
                        ui.changed = false;
                        this.notice("Filters saved on the server", false, cx);
                    }
                    Err(e) => ui.error = Some(format!("{e:#}")),
                }
                cx.notify();
            },
        );
    }

    /// The server's notes for its users, under Storage.
    pub(super) fn server_notes_view(&self, a: &Account) -> Option<Div> {
        let ServerNotes { comment, admin } = self.server_notes.get(&a.id)?.clone();
        if comment.is_none() && admin.is_none() {
            return None;
        }
        let c = self.colors();
        Some(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .text_size(px(12.))
                .text_color(rgb(c.muted))
                .children(comment.map(|t| div().child(format!("From the server: {t}"))))
                .children(admin.map(|t| div().child(format!("Server administrator: {t}")))),
        )
    }

    /// The server says the same mail is reachable over JMAP (RFC 9698):
    /// worth knowing, since JMAP syncs faster and pushes changes.
    pub(super) fn jmap_access_note(&self, a: &Account) -> Option<Div> {
        let url = self.jmap_access.get(&a.id)?;
        let host = url::Url::parse(url).ok()?.host_str()?.to_owned();
        Some(
            div()
                .text_size(px(12.))
                .text_color(rgb(self.colors().muted))
                .child(format!(
                    "This server also offers this mailbox over JMAP ({host}), which syncs faster and gets changes pushed. Add it again choosing JMAP to use it."
                )),
        )
    }

    /// Changes made offline that haven't reached the server yet.
    pub(super) fn offline_note(&self, a: &Account) -> Option<Div> {
        let n: usize = self
            .backend
            .queued_changes(&a.id)
            .iter()
            .map(|q| q.mails.len())
            .sum();
        (n > 0)
            .then(|| {
                div()
                    .id("offline-changes")
                    .text_size(px(12.))
                    .text_color(rgb(self.colors().muted))
                    .child(format!(
                        "{} made while offline will reach the server with the next connection.",
                        if n == 1 {
                            "One change".to_owned()
                        } else {
                            format!("{} changes", fmt_count(n))
                        }
                    ))
            })
            .map(|d| div().child(d))
    }
    /// The filters and vacation section of an account's Settings page.
    pub(super) fn filters_setting(&self, a: &Account, cx: &Context<Self>) -> AnyElement {
        let c = self.colors();
        let id = a.id.clone();
        let act = |action: FilterAction| Action::Filters(action);
        let Some(ui) = self.filters.get(&a.id) else {
            return div()
                .text_size(px(12.))
                .text_color(rgb(c.muted))
                .child("Asking the server…")
                .into_any_element();
        };
        if ui.loading {
            return div()
                .flex()
                .items_center()
                .gap_2()
                .text_size(px(12.))
                .text_color(rgb(c.muted))
                .child(spinner(12., c.accent))
                .child("Asking the server for its filters…")
                .into_any_element();
        }
        let Some(state) = &ui.state else {
            return div()
                .flex()
                .flex_col()
                .gap_2()
                .text_size(px(12.))
                .text_color(rgb(c.muted))
                .child(
                    ui.error.clone().unwrap_or_else(|| {
                        "This server doesn't offer filters (ManageSieve).".into()
                    }),
                )
                .child(
                    div().flex().child(
                        self.button(
                            format!("filters-retry-{id}"),
                            "Try Again",
                            "refresh",
                            act(FilterAction::Load(id.clone())),
                            false,
                            cx,
                        )
                        .border_1()
                        .border_color(rgb(c.border)),
                    ),
                )
                .into_any_element();
        };
        let folders = self.folders.get(&a.id).cloned().unwrap_or_default();
        let folder_label = |name: &str| {
            folders
                .iter()
                .find(|f| f.name == name)
                .map(|f| f.label.clone())
                .unwrap_or_else(|| crate::sync::transport::decode_name(name))
        };
        let describe = |r: &crate::sync::sieve::Rule| {
            let what = match &r.act {
                Act::Inbox => "keep in the inbox".to_owned(),
                Act::Folder(name) => format!("move to {}", folder_label(name)),
                Act::MarkRead => "mark as read".to_owned(),
                Act::Star => "star".to_owned(),
                Act::Discard => "delete".to_owned(),
            };
            format!("{} contains “{}”: {what}", r.field.label(), r.value)
        };
        let mut page = div().flex().flex_col().gap_4();
        if let Some(other) = &state.other_active {
            if state.has("include") {
                let including = ui.draft.include.is_some();
                page = page.child(self.switch_setting(
                    &format!("Keep running “{other}”"),
                    "Another app's filters are on. Picacho's filters can run them first, so both apply.",
                    format!("filters-include-{id}"),
                    including,
                    act(FilterAction::ToggleInclude(id.clone())),
                    cx,
                ));
            } else {
                // Nothing to choose: the server runs one set of filters.
                page = page.child(
                    div()
                        .text_size(px(12.))
                        .text_color(rgb(c.muted))
                        .child(format!(
                            "Another app's filters (“{other}”) are on. This server can run only one set, so saving Picacho's turns those off."
                        )),
                );
            }
        }
        // Rules.
        let mut rules = div().flex().flex_col().gap_1();
        if ui.draft.rules.is_empty() {
            rules = rules.child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(c.muted))
                    .child("No filters yet. Mail arriving on the server is sorted by them, even while Picacho is closed."),
            );
        }
        for (i, rule) in ui.draft.rules.iter().enumerate() {
            rules = rules.child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .rounded(px(8.))
                    .bg(rgb(c.bg))
                    .child(icon_el("filter", 13., c.muted))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(12.))
                            .child(describe(rule)),
                    )
                    .child(self.link(
                        &format!("filter-stop-{id}-{i}"),
                        if rule.stop {
                            "Then stop"
                        } else {
                            "Then continue"
                        },
                        act(FilterAction::ToggleStop(id.clone(), i)),
                        cx,
                    ))
                    .child(self.button(
                        format!("filter-remove-{id}-{i}"),
                        "@Remove filter",
                        "close",
                        act(FilterAction::Remove(id.clone(), i)),
                        false,
                        cx,
                    )),
            );
        }
        // A server with only an away reply (JMAP's) runs no rules.
        if !state.vacation_only {
            page = page.child(self.setting("Filters", "", rules));
        }
        // A new rule.
        let fields = self.segmented(
            &format!("filter-field-{id}"),
            Field::ALL
                .iter()
                .map(|f| {
                    (
                        f.label().to_owned(),
                        act(FilterAction::NewField(id.clone(), *f)),
                        ui.new_field == *f,
                    )
                })
                .collect(),
            cx,
        );
        let mut acts: Vec<(String, Action, bool)> = vec![
            (
                "Mark read".into(),
                act(FilterAction::NewAct(id.clone(), Act::MarkRead)),
                ui.new_act == Act::MarkRead,
            ),
            (
                "Star".into(),
                act(FilterAction::NewAct(id.clone(), Act::Star)),
                ui.new_act == Act::Star,
            ),
            (
                "Delete".into(),
                act(FilterAction::NewAct(id.clone(), Act::Discard)),
                ui.new_act == Act::Discard,
            ),
        ];
        if state.has("fileinto") {
            acts.insert(
                0,
                (
                    "Move to folder".into(),
                    act(FilterAction::NewAct(
                        id.clone(),
                        Act::Folder(folders.first().map(|f| f.name.clone()).unwrap_or_default()),
                    )),
                    matches!(ui.new_act, Act::Folder(_)),
                ),
            );
        }
        let mut form = div()
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .rounded(px(10.))
            .border_1()
            .border_color(rgb(c.border))
            .child(
                div()
                    .text_size(px(12.))
                    .font_weight(FontWeight::MEDIUM)
                    .child("New filter: when"),
            )
            .child(fields)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(rgb(c.muted))
                            .child("contains"),
                    )
                    .child(div().flex_1().child(field(
                        Input::new(&self.filter_inputs.value),
                        c,
                        false,
                    ))),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .font_weight(FontWeight::MEDIUM)
                    .child("then"),
            )
            .child(self.segmented(&format!("filter-act-{id}"), acts, cx));
        if let Act::Folder(chosen) = &ui.new_act {
            if folders.is_empty() {
                form = form.child(
                    div()
                        .text_size(px(12.))
                        .text_color(rgb(c.muted))
                        .child("Make a folder in the sidebar first; filters move mail into the account's own folders."),
                );
            } else {
                form = form.child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap_1()
                        .children(folders.iter().map(|f| {
                            let on = &f.name == chosen;
                            self.button(
                                format!("filter-folder-{id}-{}", f.name),
                                &f.label,
                                "folder",
                                act(FilterAction::NewAct(
                                    id.clone(),
                                    Act::Folder(f.name.clone()),
                                )),
                                false,
                                cx,
                            )
                            .h(px(26.))
                            .border_1()
                            .border_color(rgb(if on { c.accent } else { c.border }))
                            .when(on, |d| d.bg(rgb(c.soft)))
                        })),
                );
            }
        }
        form = form.child(
            div().flex().child(
                self.button(
                    format!("filter-add-{id}"),
                    "Add Filter",
                    "plus",
                    act(FilterAction::Add(id.clone())),
                    false,
                    cx,
                )
                .border_1()
                .border_color(rgb(c.border)),
            ),
        );
        if !state.vacation_only {
            page = page.child(form);
        }
        // Vacation.
        if state.has("vacation") {
            let v = &ui.draft.vacation;
            page = page.child(self.switch_setting(
                "Vacation reply",
                if state.vacation_only {
                    "The server answers mail to this account while you're away, once per sender."
                } else {
                    "The server answers mail to this account's addresses while you're away, once per sender in the days chosen below."
                },
                format!("vacation-{id}"),
                v.on,
                act(FilterAction::ToggleVacation(id.clone())),
                cx,
            ));
            if v.on {
                page = page
                    .child(self.setting(
                        "Subject",
                        "",
                        div().max_w(px(420.)).child(field(
                            Input::new(&self.filter_inputs.subject),
                            c,
                            false,
                        )),
                    ))
                    .child(
                        self.setting(
                            "Message",
                            "",
                            div().h(px(110.)).child(
                                textarea_field(Textarea::new(&self.filter_inputs.body), c, false)
                                    .h_full(),
                            ),
                        ),
                    )
                    .when(!state.vacation_only, |page| page.child(
                        self.setting(
                            "Answer each sender once every",
                            "",
                            self.segmented(
                                &format!("vacation-days-{id}"),
                                [(1, "Day"), (3, "3 days"), (7, "Week"), (14, "2 weeks")]
                                    .into_iter()
                                    .map(|(d, l)| {
                                        (
                                            l.to_owned(),
                                            act(FilterAction::VacationDays(id.clone(), d)),
                                            v.days == d,
                                        )
                                    })
                                    .collect(),
                                cx,
                            ),
                        ),
                    ));
                if state.has("date") && state.has("relational") {
                    page = page.child(
                        self.setting(
                            "Only between",
                            "Leave empty to answer from now until you turn it off.",
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .max_w(px(420.))
                                .child(div().flex_1().child(field(
                                    Input::new(&self.filter_inputs.from),
                                    c,
                                    false,
                                )))
                                .child(
                                    div()
                                        .text_size(px(12.))
                                        .text_color(rgb(c.muted))
                                        .child("and"),
                                )
                                .child(div().flex_1().child(field(
                                    Input::new(&self.filter_inputs.until),
                                    c,
                                    false,
                                ))),
                        ),
                    );
                }
            }
        }
        page = page.children(
            ui.error
                .clone()
                .map(|e| div().text_size(px(12.)).text_color(rgb(c.danger)).child(e)),
        );
        page = page.child(
            div()
                .flex()
                .items_center()
                .gap_3()
                .child(
                    div()
                        .flex_1()
                        .text_size(px(12.))
                        .text_color(rgb(c.muted))
                        .child(if state.active {
                            "Picacho's filters are on for this account."
                        } else {
                            "Save to turn these filters on."
                        }),
                )
                .child(self.button(
                    format!("filters-save-{id}"),
                    if ui.saving {
                        "Saving…"
                    } else {
                        "Save Filters"
                    },
                    "",
                    act(FilterAction::Save(id.clone())),
                    true,
                    cx,
                )),
        );
        page.into_any_element()
    }
}

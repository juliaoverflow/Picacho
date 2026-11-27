//! Settings → account → Filters & vacation: rules the mail server applies
//! as mail arrives, and an automatic reply while away (ManageSieve), plus
//! what the server says about itself (METADATA).
use super::*;
use crate::manage::backend::ServerNotes;
use crate::sync::sieve::{Act, Field, Filters, State, Test};

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
    /// How the new rule compares, and whether it applies when it doesn't
    /// match.
    pub(super) new_match: crate::sync::sieve::Match,
    pub(super) new_negate: bool,
    /// Tests and actions added to the new rule so far, and whether any
    /// of its tests is enough (rather than all).
    pub(super) new_tests: Vec<Test>,
    pub(super) new_acts: Vec<Act>,
    pub(super) new_any: bool,
    /// The text fields still show another account's (or nothing): fill
    /// them from `draft` at the next chance.
    pub(super) populate: bool,
    pub(super) changed: bool,
}

/// The text fields of the filters section, shared by account pages (one
/// shows at a time).
pub(super) struct FilterInputs {
    pub(super) value: Entity<InputState>,
    /// A header's name (Header), or an action's text: the address to send
    /// on to, the refusal's reason, the label.
    pub(super) extra: Entity<InputState>,
    pub(super) act_text: Entity<InputState>,
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
            value: input(t!("filters-text-to-look-for"), false, window, cx),
            extra: input(t!("filters-header-name"), false, window, cx),
            act_text: input(t!("filters-address-reason-or-label"), false, window, cx),
            subject: input(t!("filters-out-of-office"), false, window, cx),
            body: textarea(
                t!("filters-im-away-until-monday-and-will"),
                window,
                cx,
            ),
            from: input(t!("filters-2026-12-20-optional"), false, window, cx),
            until: input(t!("filters-2026-12-31-optional"), false, window, cx),
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
    /// Adds the test typed to the rule being written.
    AddTest(String),
    RemoveTest(String, usize),
    /// Adds the action chosen to the rule being written.
    AddAct(String),
    RemoveAct(String, usize),
    /// Any test of the rule being written is enough, or all must match.
    ToggleAny(String),
    Remove(String, usize),
    NewField(String, Field),
    NewAct(String, Act),
    NewMatch(String, crate::sync::sieve::Match),
    ToggleNegate(String),
    ToggleStop(String, usize),
    ToggleVacation(String),
    VacationDays(String, u32),
    /// Runs the script that was active before Picacho's first.
    ToggleInclude(String),
    /// Takes the other app's rules into Picacho's, to edit here.
    AdoptOtherRules(String),
}

impl MailApp {
    /// Loads what a newly shown account page needs from the server: its
    /// filters and the server's notes.
    pub(super) fn load_account_extras(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(a) = self.accounts.iter().find(|a| a.id == id).cloned() else {
            return;
        };
        if !(a.imap() || a.provider == Provider::Jmap || a.native_api() || a.ews())
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
                let typed = Self::value(&self.filter_inputs.value, cx).trim().to_owned();
                let Some(ui) = self.filters.get_mut(&id) else {
                    return;
                };
                // What's typed and not yet added is the last test and action.
                let take_test = !typed.is_empty() || (ui.new_field == Field::Attachment && ui.new_tests.is_empty()) || ui.new_tests.is_empty();
                if take_test && let Err(e) = self.take_filter_test(&id, w, cx) {
                    if let Some(ui) = self.filters.get_mut(&id) {
                        ui.error = Some(e.into());
                    }
                    return;
                }
                let Some(ui) = self.filters.get_mut(&id) else {
                    return;
                };
                if (ui.new_acts.is_empty() || ui.new_act != Act::Inbox) && let Err(e) = self.take_filter_act(&id, w, cx) {
                    if let Some(ui) = self.filters.get_mut(&id) {
                        ui.error = Some(e.into());
                    }
                    return;
                }
                let ui = self.filters.get_mut(&id).expect("looked up above");
                let tests = std::mem::take(&mut ui.new_tests);
                let acts = std::mem::take(&mut ui.new_acts);
                let mut rule = crate::sync::sieve::Rule {
                    stop: acts.iter().any(|a| matches!(a, Act::Folder(_) | Act::Discard | Act::Reject(_) | Act::Redirect(_))),
                    ..Default::default()
                };
                rule.set_tests(tests);
                rule.any = ui.new_any && !rule.more.is_empty();
                rule.set_acts(acts);
                ui.draft.rules.push(rule);
                ui.new_any = false;
                ui.new_act = Act::Inbox;
                ui.error = None;
                ui.changed = true;
            }
            FilterAction::AddTest(id) => {
                if let Err(e) = self.take_filter_test(&id, w, cx)
                    && let Some(ui) = self.filters.get_mut(&id)
                {
                    ui.error = Some(e.into());
                }
            }
            FilterAction::RemoveTest(id, i) => {
                if let Some(ui) = self.filters.get_mut(&id)
                    && i < ui.new_tests.len()
                {
                    ui.new_tests.remove(i);
                }
            }
            FilterAction::AddAct(id) => {
                if let Err(e) = self.take_filter_act(&id, w, cx)
                    && let Some(ui) = self.filters.get_mut(&id)
                {
                    ui.error = Some(e.into());
                }
            }
            FilterAction::RemoveAct(id, i) => {
                if let Some(ui) = self.filters.get_mut(&id)
                    && i < ui.new_acts.len()
                {
                    ui.new_acts.remove(i);
                }
            }
            FilterAction::ToggleAny(id) => {
                if let Some(ui) = self.filters.get_mut(&id) {
                    ui.new_any = !ui.new_any;
                }
            }
            FilterAction::AdoptOtherRules(id) => {
                if let Some(ui) = self.filters.get_mut(&id)
                    && let Some(read) = ui.state.as_ref().and_then(|s| s.other_rules.clone())
                {
                    for rule in read.filters.rules {
                        if !ui.draft.rules.contains(&rule) {
                            ui.draft.rules.push(rule);
                        }
                    }
                    if read.filters.vacation.on && !ui.draft.vacation.on {
                        ui.draft.vacation = read.filters.vacation;
                    }
                    // Picacho's now do what that script did: it needn't run too.
                    if read.left_out == 0 {
                        ui.draft.include = None;
                    }
                    ui.changed = true;
                    ui.populate = true;
                }
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
            FilterAction::NewMatch(id, m) => {
                if let Some(ui) = self.filters.get_mut(&id) {
                    ui.new_match = m;
                }
            }
            FilterAction::ToggleNegate(id) => {
                if let Some(ui) = self.filters.get_mut(&id) {
                    ui.new_negate = !ui.new_negate;
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
    /// Adds the test typed to the account's new rule, or says what's
    /// missing.
    fn take_filter_test(&mut self, id: &str, w: &mut Window, cx: &mut Context<Self>) -> Result<(), &'static str> {
        let value = Self::value(&self.filter_inputs.value, cx).trim().to_owned();
        let extra = Self::value(&self.filter_inputs.extra, cx).trim().to_owned();
        let Some(ui) = self.filters.get_mut(id) else {
            return Ok(());
        };
        if value.is_empty() && ui.new_field.has_value() {
            return Err(t!("filters-type-what-the-filter-looks-for"));
        }
        let header = if ui.new_field == Field::Header {
            if extra.is_empty() {
                return Err(t!("filters-name-the-header-the-filter-looks"));
            }
            extra
        } else {
            String::new()
        };
        let test = Test {
            field: ui.new_field,
            value: if ui.new_field.has_value() { value } else { String::new() },
            matching: if ui.new_field.has_value() && !ui.new_field.is_size() { ui.new_match } else { Default::default() },
            negate: ui.new_negate,
            header,
        };
        if !ui.new_tests.contains(&test) {
            ui.new_tests.push(test);
        }
        ui.new_negate = false;
        ui.error = None;
        Self::set(&self.filter_inputs.value, "", w, cx);
        Self::set(&self.filter_inputs.extra, "", w, cx);
        Ok(())
    }
    /// Adds the action chosen to the account's new rule, or says what's
    /// missing.
    fn take_filter_act(&mut self, id: &str, w: &mut Window, cx: &mut Context<Self>) -> Result<(), &'static str> {
        let act_text = Self::value(&self.filter_inputs.act_text, cx).trim().to_owned();
        let Some(ui) = self.filters.get_mut(id) else {
            return Ok(());
        };
        let act = match &ui.new_act {
            Act::Inbox => return Err(t!("filters-choose-what-the-filter-does")),
            Act::Redirect(_) if act_text.is_empty() => return Err(t!("filters-enter-the-address-mail-is-sent")),
            Act::Reject(_) if act_text.is_empty() => return Err(t!("filters-write-why-mail-is-refused")),
            Act::Label(_) if act_text.is_empty() => return Err(t!("filters-name-the-label")),
            Act::Redirect(_) => Act::Redirect(act_text),
            Act::Reject(_) => Act::Reject(act_text),
            Act::Label(_) => Act::Label(act_text),
            other => other.clone(),
        };
        if matches!(act, Act::Folder(_)) && ui.new_acts.iter().any(|a| matches!(a, Act::Folder(_))) {
            return Err(t!("filters-one-folder-per-filter"));
        }
        if !ui.new_acts.contains(&act) {
            ui.new_acts.push(act);
        }
        ui.new_act = Act::Inbox;
        ui.error = None;
        Self::set(&self.filter_inputs.act_text, "", w, cx);
        Ok(())
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
                        this.notice(t!("filters-filters-saved-on-the-server"), false, cx);
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
                .children(comment.map(|t| div().child(t!("filters-from-the-server", t = t))))
                .children(admin.map(|t| div().child(t!("filters-server-administrator", t = t)))),
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
                .child(t!("filters-this-server-also-offers-this-mailbox", host = host)),
        )
    }

    /// Changes made offline that haven't reached the server yet.
    pub(super) fn offline_note(&self, a: &Account) -> Option<Div> {
        // Counted by the cache, not by reading every queued change's
        // messages on each frame.
        let n = self.backend.queued_messages(&a.id);
        (n > 0)
            .then(|| {
                div()
                    .id("offline-changes")
                    .text_size(px(12.))
                    .text_color(rgb(self.colors().muted))
                    .child(t!("filters-made-while-offline-will-reach-the", count = n, shown = fmt_count(n)))
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
                .child(t!("filters-asking-the-server"))
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
                .child(t!("filters-asking-the-server-for-its-filters"))
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
                        t!("filters-this-server-doesnt-offer-filters-managesieve").into()
                    }),
                )
                .child(
                    div().flex().child(
                        self.button(
                            format!("filters-retry-{id}"),
                            t!("filters-try-again"),
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
        let mut folders = self.folders.get(&a.id).cloned().unwrap_or_default();
        // The standard folders by their role, which filters name so.
        for (role, label) in [("\\Archive", t!("filters-archive")), ("\\Junk", t!("filters-spam")), ("\\Trash", t!("filters-trash"))] {
            folders.push(crate::manage::model::RemoteFolder {
                name: role.into(),
                label: label.into(),
                ..Default::default()
            });
        }
        let folder_label = |name: &str| {
            folders
                .iter()
                .find(|f| f.name == name)
                .map(|f| f.label.clone())
                .unwrap_or_else(|| crate::sync::transport::decode_name(name))
        };
        let describe_act = |r: &crate::sync::sieve::Rule, act: &Act| match act {
            Act::Inbox => t!("filters-keep-in-the-inbox").to_owned(),
            Act::Folder(name) => t!("filters-move-to", folder = folder_label(name)),
            Act::MarkRead => t!("filters-mark-as-read").to_owned(),
            Act::Star => t!("filters-act-star").to_owned(),
            Act::Discard => t!("filters-act-delete").to_owned(),
            Act::Redirect(to) if r.keep_copy => t!("filters-send-a-copy-to", to = to),
            Act::Redirect(to) => t!("filters-send-on-to", to = to),
            Act::Reject(_) => t!("filters-act-refuse").to_owned(),
            Act::Label(l) => t!("filters-label", l = l),
        };
        let describe_test = |t: &Test| {
            let negate = if t.negate { "yes" } else { "no" };
            match t.field {
                Field::LargerThan => t!("filters-test-larger", negate = negate, kb = t.value),
                Field::SmallerThan => t!("filters-test-smaller", negate = negate, kb = t.value),
                Field::Attachment => t!("filters-test-attachment", negate = negate),
                _ => {
                    let field = if t.field == Field::Header {
                        t!("filters-header", header = t.header)
                    } else {
                        t.field.label().to_owned()
                    };
                    t!("filters-test", field = field, negate = negate, matching = t.matching.key(), text = t.value)
                }
            }
        };
        let join_tests = |tests: &[Test], any: bool| {
            let join = if any { t!("filters-or") } else { t!("filters-and") };
            tests.iter().map(describe_test).collect::<Vec<_>>().join(&format!(" {join} "))
        };
        let describe = |r: &crate::sync::sieve::Rule| {
            let what = r.acts().map(|a| describe_act(r, a)).collect::<Vec<_>>().join(", ");
            t!("filters-rule", tests = join_tests(&r.tests(), r.any), what = what)
        };
        let mut page = div().flex().flex_col().gap_4();
        if let Some(other) = &state.other_active
            && let Some(read) = state.other_rules.as_ref().filter(|r| !r.filters.rules.is_empty())
        {
            let n = read.filters.rules.len();
            page = page.child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_size(px(12.))
                    .child(div().flex_1().text_color(rgb(c.muted)).child(t!("filters-has-picacho-can-show", other = other, n = n, left_out = read.left_out)))
                    .child(self.button(
                        format!("filters-adopt-{id}"),
                        t!("filters-edit-them-here"),
                        "filter",
                        act(FilterAction::AdoptOtherRules(id.clone())),
                        false,
                        cx,
                    )),
            );
        }
        if let Some(other) = &state.other_active {
            if state.has("include") {
                let including = ui.draft.include.is_some();
                page = page.child(self.switch_setting(
                    &t!("filters-keep-running", other = other),
                    t!("filters-another-apps-filters-are-on-picachos"),
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
                        .child(t!("filters-another-apps-filters-are-on-this", other = other)),
                );
            }
        }
        // Filters made elsewhere that Picacho's rules can't say: kept.
        if state.kept_elsewhere > 0 {
            page = page.child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(c.muted))
                    .child(t!("filters-kept-as-they-are", n = state.kept_elsewhere)),
            );
        }
        // Rules.
        let mut rules = div().flex().flex_col().gap_1();
        if ui.draft.rules.is_empty() {
            rules = rules.child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(c.muted))
                    .child(t!("filters-no-filters-yet-mail-arriving-on")),
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
                            .text_size(px(12.))
                            .child(describe(rule)),
                    )
                    .child(self.link(
                        &format!("filter-stop-{id}-{i}"),
                        if rule.stop {
                            t!("filters-then-stop")
                        } else {
                            t!("filters-then-continue")
                        },
                        act(FilterAction::ToggleStop(id.clone(), i)),
                        cx,
                    ))
                    .child(self.button(
                        format!("filter-remove-{id}-{i}"),
                        t!("filters-remove-filter"),
                        "close",
                        act(FilterAction::Remove(id.clone(), i)),
                        false,
                        cx,
                    )),
            );
        }
        // A server with only an away reply (JMAP's) runs no rules.
        if !state.vacation_only {
            page = page.child(self.setting(t!("filters-filters"), "", rules));
        }
        // A new rule.
        let fields = self.segmented(
            &format!("filter-field-{id}"),
            Field::ALL
                .iter()
                .filter(|f| f.needs().is_none_or(|e| state.has(e)))
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
        let mut acts: Vec<(String, Action, bool)> = vec![(
            t!("filters-delete").into(),
            act(FilterAction::NewAct(id.clone(), Act::Discard)),
            ui.new_act == Act::Discard,
        )];
        // Setting flags needs the server's imap4flags (RFC 5232).
        if state.has("imap4flags") {
            acts.insert(
                0,
                (
                    t!("filters-star").into(),
                    act(FilterAction::NewAct(id.clone(), Act::Star)),
                    ui.new_act == Act::Star,
                ),
            );
            acts.insert(
                0,
                (
                    t!("filters-mark-read").into(),
                    act(FilterAction::NewAct(id.clone(), Act::MarkRead)),
                    ui.new_act == Act::MarkRead,
                ),
            );
        }
        if state.has("imap4flags") {
            acts.push((
                t!("filters-label-2").into(),
                act(FilterAction::NewAct(id.clone(), Act::Label(String::new()))),
                matches!(ui.new_act, Act::Label(_)),
            ));
        }
        acts.push((
            t!("filters-send-on").into(),
            act(FilterAction::NewAct(id.clone(), Act::Redirect(String::new()))),
            matches!(ui.new_act, Act::Redirect(_)),
        ));
        if state.has("reject") {
            acts.push((
                t!("filters-refuse").into(),
                act(FilterAction::NewAct(id.clone(), Act::Reject(String::new()))),
                matches!(ui.new_act, Act::Reject(_)),
            ));
        }
        if state.has("fileinto") {
            acts.insert(
                0,
                (
                    t!("filters-move-to-folder").into(),
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
                    .child(t!("filters-new-filter-when")),
            )
            .when(!ui.new_tests.is_empty(), |d| {
                d.child(div().flex().flex_wrap().gap_1().children(ui.new_tests.iter().enumerate().map(|(i, t)| {
                    self.filter_chip(format!("filter-new-test-{id}-{i}"), describe_test(t), act(FilterAction::RemoveTest(id.clone(), i)), cx)
                })))
                .child(self.segmented(
                    &format!("filter-any-{id}"),
                    vec![
                        (t!("filters-all-tests").into(), act(FilterAction::ToggleAny(id.clone())), !ui.new_any),
                        (t!("filters-any-test").into(), act(FilterAction::ToggleAny(id.clone())), ui.new_any),
                    ],
                    cx,
                ).max_w(px(360.)))
            })
            .child(fields)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .when(ui.new_field == Field::Header, |d| {
                        d.child(div().w(px(140.)).child(field(Input::new(&self.filter_inputs.extra), c, false)))
                    })
                    .child(self.link(
                        &format!("filter-negate-{id}"),
                        if ui.new_negate { "doesn't" } else { "does" },
                        act(FilterAction::ToggleNegate(id.clone())),
                        cx,
                    ))
                    .when(ui.new_field.has_value() && !ui.new_field.is_size(), |d| {
                        d.child(self.segmented(
                            &format!("filter-match-{id}"),
                            crate::sync::sieve::Match::ALL
                                .iter()
                                .filter(|m| **m != crate::sync::sieve::Match::Regex || state.has("regex"))
                                .map(|m| {
                                    (
                                        m.label().to_owned(),
                                        act(FilterAction::NewMatch(id.clone(), *m)),
                                        ui.new_match == *m,
                                    )
                                })
                                .collect(),
                            cx,
                        ))
                    })
                    .child(div().flex_1().when(ui.new_field.has_value(), |d| {
                        d.child(field(Input::new(&self.filter_inputs.value), c, false))
                    }))
                    .child(self.button(
                        format!("filter-add-test-{id}"),
                        t!("filters-add-test"),
                        "plus",
                        act(FilterAction::AddTest(id.clone())),
                        false,
                        cx,
                    )),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .font_weight(FontWeight::MEDIUM)
                    .child(t!("filters-then")),
            )
            .when(!ui.new_acts.is_empty(), |d| {
                d.child(div().flex().flex_wrap().gap_1().children(ui.new_acts.iter().enumerate().map(|(i, a)| {
                    let text = match a {
                        Act::Folder(name) => t!("filters-move-to", folder = folder_label(name)),
                        Act::Redirect(to) => t!("filters-send-on-to", to = to),
                        Act::Label(l) => t!("filters-label", l = l),
                        Act::MarkRead => t!("filters-mark-as-read").to_owned(),
                        Act::Star => t!("filters-act-star").to_owned(),
                        Act::Discard => t!("filters-act-delete").to_owned(),
                        Act::Reject(_) => t!("filters-act-refuse").to_owned(),
                        Act::Inbox => t!("filters-keep-in-the-inbox").to_owned(),
                    };
                    self.filter_chip(format!("filter-new-act-{id}-{i}"), text, act(FilterAction::RemoveAct(id.clone(), i)), cx)
                })))
            })
            .child(self.segmented(&format!("filter-act-{id}"), acts, cx));
        // The action's own text.
        if matches!(ui.new_act, Act::Redirect(_) | Act::Reject(_) | Act::Label(_)) {
            form = form.child(div().max_w(px(360.)).child(field(Input::new(&self.filter_inputs.act_text), c, false)));
        }
        if let Act::Folder(chosen) = &ui.new_act {
            if folders.is_empty() {
                form = form.child(
                    div()
                        .text_size(px(12.))
                        .text_color(rgb(c.muted))
                        .child(t!("filters-make-a-folder-in-the-sidebar")),
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
            div().flex().gap_2().child(
                self.button(
                    format!("filter-add-act-{id}"),
                    t!("filters-add-action"),
                    "plus",
                    act(FilterAction::AddAct(id.clone())),
                    false,
                    cx,
                )
                .border_1()
                .border_color(rgb(c.border)),
            ).child(
                self.button(
                    format!("filter-add-{id}"),
                    t!("filters-add-filter"),
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
                t!("filters-vacation-reply"),
                if state.vacation_only {
                    t!("filters-the-server-answers-mail-to-this")
                } else {
                    t!("filters-the-server-answers-mail-to-this-2")
                },
                format!("vacation-{id}"),
                v.on,
                act(FilterAction::ToggleVacation(id.clone())),
                cx,
            ));
            if v.on {
                page = page
                    .child(self.setting(
                        t!("filters-subject"),
                        "",
                        div().max_w(px(420.)).child(field(
                            Input::new(&self.filter_inputs.subject),
                            c,
                            false,
                        )),
                    ))
                    .child(
                        self.setting(
                            t!("filters-message"),
                            "",
                            div().h(px(110.)).child(
                                textarea_field(Textarea::new(&self.filter_inputs.body), c, false)
                                    .h_full(),
                            ),
                        ),
                    )
                    .when(!state.vacation_only, |page| page.child(
                        self.setting(
                            t!("filters-answer-each-sender-once-every"),
                            "",
                            self.segmented(
                                &format!("vacation-days-{id}"),
                                [(1, t!("filters-day")), (3, t!("filters-3-days")), (7, t!("filters-week")), (14, t!("filters-2-weeks"))]
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
                            t!("filters-only-between"),
                            t!("filters-leave-empty-to-answer-from-now"),
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
                            t!("filters-picachos-filters-are-on-for-this")
                        } else {
                            t!("filters-save-to-turn-these-filters-on")
                        }),
                )
                .child(self.button(
                    format!("filters-save-{id}"),
                    if ui.saving {
                        t!("filters-saving")
                    } else {
                        t!("filters-save-filters")
                    },
                    "",
                    act(FilterAction::Save(id.clone())),
                    true,
                    cx,
                )),
        );
        page.into_any_element()
    }

    /// A test or action added to the new filter, with a button removing it.
    fn filter_chip(&self, id: String, text: String, remove: Action, cx: &Context<Self>) -> Stateful<Div> {
        let c = self.colors();
        div()
            .id(SharedString::from(id.clone()))
            .debug_selector(move || id.clone())
            .max_w_full()
            .h(px(24.))
            .pl_2()
            .pr_1()
            .flex()
            .items_center()
            .gap_1()
            .rounded(px(12.))
            .bg(rgb(c.soft))
            .text_size(px(12.))
            .child(div().min_w_0().truncate().child(text.clone()))
            .child(
                div()
                    .id("remove")
                    .role(Role::Button)
                    .aria_label(t!("filters-remove-part", part = text))
                    .size(px(18.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .cursor_pointer()
                    .hover(move |s| s.bg(rgb(c.border)))
                    .child(icon_el("close", 10., c.muted))
                    .on_click(cx.listener(move |this, _, w, cx| this.handle(remove.clone(), w, cx))),
            )
    }
}

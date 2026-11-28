//! The Tasks view: every source's tasks in a workspace of their own, from
//! the sidebar (and from Mailboxes on phones) beside Calendar and Contacts.
//! Today (with what's overdue), Upcoming, Overdue, All and the Completed
//! history; tasks by list and by tag; a priority, tags, notes and subtasks;
//! and tasks made from email ("Add to Tasks" on a message or the text
//! chosen in it), which open that email again. Task services that aren't
//! calendars (Microsoft To Do, Google Tasks) are added here too.
//!
//! What tasks are and where they're kept is the backend's
//! (`manage::backend::calendar::tasks`).
use super::*;
use crate::manage::backend::calendar::tasks::{self as model, Due, TaskDraft};
use crate::sync::caldav::Stored;

/// Which tasks the view lists.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum TaskFilter {
    /// Due today, and what's overdue above it.
    #[default]
    Today,
    Upcoming,
    Overdue,
    All,
    Completed,
}

impl TaskFilter {
    const ALL: [Self; 5] = [Self::Today, Self::Upcoming, Self::Overdue, Self::All, Self::Completed];
    fn label(self) -> &'static str {
        match self {
            Self::Today => t!("tasks-today"),
            Self::Upcoming => t!("tasks-upcoming"),
            Self::Overdue => t!("tasks-overdue"),
            Self::All => t!("tasks-all"),
            Self::Completed => t!("tasks-completed"),
        }
    }
    /// Whether a task due as `due` is listed.
    fn lists(self, due: Due) -> bool {
        match self {
            Self::Today => matches!(due, Due::Today | Due::Overdue),
            Self::Upcoming => due == Due::Upcoming,
            Self::Overdue => due == Due::Overdue,
            Self::All => true,
            Self::Completed => false,
        }
    }
}

/// The priorities offered, as RFC 5545 has them: none, low, medium, high.
const PRIORITIES: [u8; 4] = [0, 9, 5, 1];

fn priority_label(p: u8) -> &'static str {
    match p {
        1..=4 => t!("tasks-priority-high"),
        5 => t!("tasks-priority-medium"),
        6..=9 => t!("tasks-priority-low"),
        _ => t!("tasks-priority-none"),
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum TaskAction {
    Filter(TaskFilter),
    /// Only this list's tasks (its href), or every list's.
    List(Option<String>),
    /// Only tasks with this tag, or all.
    Tag(Option<String>),
    /// Adds the task typed in the quick-add field.
    QuickAdd,
    /// Opens the editor for a new task, a subtask of this one (its UID).
    New(Option<String>),
    /// Opens the editor on a task (its UID).
    Edit(String),
    Priority(u8),
    /// The list the edited task goes to (its href).
    TargetList(String),
    Save,
    Cancel,
    Delete(String),
    /// Marks a task (its UID) done.
    Done(String),
    /// Opens a done task (its UID) again.
    Reopen(String),
    ClearCompleted,
    /// Opens the email a task links to (its `mid:` link), or the link.
    OpenLink(String),
    /// Adds a task about a message (its id), with the text chosen in it.
    FromMail(String, Option<String>),
    Sync,
    /// Adds a mail account's task service (its id): To Do or Google Tasks.
    ConnectService(String),
    /// Makes a list (its href) the one new tasks go to.
    DefaultList(String),
    /// Reads a task service (its source id) again, or removes it.
    SyncSource(String),
    RemoveSource(String),
}

/// The task editor: a new task, or the one being changed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Editing {
    New,
    Task(String),
}

pub(super) struct TasksView {
    pub(super) filter: TaskFilter,
    pub(super) list: Option<String>,
    pub(super) tag: Option<String>,
    pub(super) quick: Entity<InputState>,
    pub(super) quick_due: Entity<InputState>,
    pub(super) title: Entity<InputState>,
    pub(super) due: Entity<InputState>,
    pub(super) notes: Entity<InputState>,
    pub(super) tags: Entity<InputState>,
    pub(super) editing: Option<Editing>,
    pub(super) priority: u8,
    pub(super) target: String,
    pub(super) parent: String,
    pub(super) url: String,
    pub(super) busy: bool,
    pub(super) error: Option<String>,
    /// The sidebar's count of tasks due now, with what it was counted from
    /// (`tasks_due_now`): the sidebar is drawn often, the tasks seldom change.
    due_now: std::cell::Cell<Option<(u64, usize)>>,
}

impl TasksView {
    pub(super) fn new(window: &mut Window, cx: &mut App) -> Self {
        Self {
            filter: TaskFilter::default(),
            list: None,
            tag: None,
            quick: input(t!("tasks-add-a-task"), false, window, cx),
            quick_due: input(t!("tasks-due-placeholder"), false, window, cx),
            title: input(t!("tasks-title"), false, window, cx),
            due: input(t!("tasks-due-placeholder"), false, window, cx),
            notes: input(t!("tasks-notes"), false, window, cx),
            tags: input(t!("tasks-tags-placeholder"), false, window, cx),
            editing: None,
            priority: 0,
            target: String::new(),
            parent: String::new(),
            url: String::new(),
            busy: false,
            error: None,
            due_now: std::cell::Cell::new(None),
        }
    }
}

/// When a task is due, as the list says it.
fn due_text(e: &crate::sync::ical::Event) -> String {
    let Some(at) = chrono::DateTime::from_timestamp(e.start, 0).filter(|_| e.start != 0) else {
        return String::new();
    };
    if e.all_day {
        at.local(t!("dates-weekday-month-day"))
    } else {
        at.with_timezone(&chrono::Local).local(t!("dates-weekday-month-day-time"))
    }
}

impl MailApp {
    /// Runs a task job in the background, the view busy meanwhile; its
    /// text is the notice shown when done, its error the view's.
    fn tasks_run(&mut self, cx: &mut Context<Self>, job: impl FnOnce(Arc<Backend>) -> anyhow::Result<String> + Send + 'static) {
        if self.tasks.busy {
            return;
        }
        self.tasks.busy = true;
        self.tasks.error = None;
        self.task(cx, job, |this, result, cx| {
            this.tasks.busy = false;
            match result {
                Ok(done) => this.notice(done, false, cx),
                Err(e) => this.tasks.error = Some(format!("{e:#}")),
            }
        });
    }

    /// Reads every source's tasks again, quietly (opening the view).
    pub(super) fn sync_tasks_quietly(&mut self, cx: &mut Context<Self>) {
        if self.backend.calendar_server().is_none() {
            return;
        }
        self.task(cx, |b| b.sync_tasks(), |_, _, cx| cx.notify());
    }

    /// Reads what was done lately on the servers into the Completed
    /// history, quietly.
    pub(super) fn sync_completed_quietly(&mut self, cx: &mut Context<Self>) {
        if self.backend.calendar_server().is_none() {
            return;
        }
        self.task(cx, |b| b.sync_completed_tasks(), |_, _, cx| cx.notify());
    }

    /// Reads the task editor's due field: empty is none; else a date, a
    /// date and time, or words ("tomorrow 5pm").
    fn read_due(&self, text: &str) -> Option<(i64, bool)> {
        let text = text.trim();
        if text.is_empty() {
            return Some((0, false));
        }
        self.read_when(text).map(|(t, all_day, _)| (t, all_day))
    }

    fn set_input(input: &Entity<InputState>, text: impl Into<SharedString>, w: &mut Window, cx: &mut Context<Self>) {
        let text = text.into();
        input.update(cx, |i, cx| i.set_text(text, w, cx));
    }

    pub(super) fn tasks_action(&mut self, action: TaskAction, w: &mut Window, cx: &mut Context<Self>) {
        match action {
            TaskAction::Filter(f) => {
                // What was done elsewhere too, read when the history is shown.
                if f == TaskFilter::Completed && self.tasks.filter != f {
                    self.sync_completed_quietly(cx);
                }
                self.tasks.filter = f;
            }
            TaskAction::List(l) => self.tasks.list = l,
            TaskAction::Tag(t) => self.tasks.tag = t,
            TaskAction::QuickAdd => {
                let summary = self.tasks.quick.read(cx).value().trim().to_owned();
                if summary.is_empty() {
                    self.tasks.error = Some(t!("manage-calendar-give-the-task-a-name").into());
                    cx.notify();
                    return;
                }
                let due_text = self.tasks.quick_due.read(cx).value().to_string();
                let Some((due, all_day)) = self.read_due(&due_text) else {
                    self.tasks.error = Some(t!("tasks-write-the-due-date").into());
                    cx.notify();
                    return;
                };
                Self::set_input(&self.tasks.quick, "", w, cx);
                Self::set_input(&self.tasks.quick_due, "", w, cx);
                // In the list shown, where one is.
                let draft = TaskDraft {
                    summary,
                    due,
                    all_day,
                    list: self.tasks.list.clone().unwrap_or_default(),
                    tags: self.tasks.tag.iter().cloned().collect(),
                    ..Default::default()
                };
                self.tasks_run(cx, move |b| {
                    let t = b.add_task_draft(&draft)?;
                    Ok(t!("calendar-added-the-task", summary = t.event.summary))
                });
            }
            TaskAction::New(parent) => {
                for input in [&self.tasks.title, &self.tasks.due, &self.tasks.notes, &self.tasks.tags] {
                    Self::set_input(input, "", w, cx);
                }
                if let Some(tag) = &self.tasks.tag {
                    Self::set_input(&self.tasks.tags, tag.clone(), w, cx);
                }
                self.tasks.priority = 0;
                self.tasks.url.clear();
                // A subtask goes in its parent's list.
                let parent_list = parent
                    .as_ref()
                    .and_then(|p| self.backend.calendar_tasks().into_iter().find(|t| &t.event.uid == p))
                    .map(|t| t.calendar);
                self.tasks.target = parent_list
                    .or_else(|| self.tasks.list.clone())
                    .or_else(|| self.backend.default_task_list())
                    .unwrap_or_default();
                self.tasks.parent = parent.unwrap_or_default();
                self.tasks.error = None;
                self.tasks.editing = Some(Editing::New);
            }
            TaskAction::Edit(uid) => {
                let Some(t) = self.backend.calendar_tasks().into_iter().find(|t| t.event.uid == uid) else {
                    return;
                };
                let due = if t.event.start == 0 {
                    String::new()
                } else {
                    calendar::format_in(t.event.start, t.event.all_day, calendar::find_zone(&t.event.tzid))
                };
                Self::set_input(&self.tasks.title, t.event.summary.clone(), w, cx);
                Self::set_input(&self.tasks.due, due, w, cx);
                Self::set_input(&self.tasks.notes, t.event.description.clone(), w, cx);
                Self::set_input(&self.tasks.tags, t.event.categories.join(", "), w, cx);
                self.tasks.priority = t.event.priority;
                self.tasks.target = t.calendar.clone();
                self.tasks.parent = t.event.parent.clone();
                self.tasks.url = t.event.url.clone();
                self.tasks.error = None;
                self.tasks.editing = Some(Editing::Task(uid));
            }
            TaskAction::Priority(p) => self.tasks.priority = p,
            TaskAction::TargetList(href) => self.tasks.target = href,
            TaskAction::Cancel => {
                self.tasks.editing = None;
                self.tasks.error = None;
            }
            TaskAction::Save => {
                let Some(editing) = self.tasks.editing.clone() else { return };
                let summary = self.tasks.title.read(cx).value().trim().to_owned();
                if summary.is_empty() {
                    self.tasks.error = Some(t!("manage-calendar-give-the-task-a-name").into());
                    cx.notify();
                    return;
                }
                let due_text = self.tasks.due.read(cx).value().to_string();
                // In the task's own zone, where it keeps one.
                let zone = match &editing {
                    Editing::Task(uid) => self
                        .backend
                        .calendar_tasks()
                        .into_iter()
                        .find(|t| &t.event.uid == uid)
                        .and_then(|t| calendar::find_zone(&t.event.tzid)),
                    Editing::New => None,
                };
                let read = calendar::parse_in(&due_text, zone).or_else(|| self.read_due(&due_text));
                let Some((due, all_day)) = read else {
                    self.tasks.error = Some(t!("tasks-write-the-due-date").into());
                    cx.notify();
                    return;
                };
                let draft = TaskDraft {
                    summary,
                    notes: self.tasks.notes.read(cx).value().to_string(),
                    due,
                    all_day,
                    priority: self.tasks.priority,
                    tags: TaskDraft::tags_from(&self.tasks.tags.read(cx).value()),
                    parent: self.tasks.parent.clone(),
                    url: self.tasks.url.clone(),
                    list: self.tasks.target.clone(),
                };
                self.tasks.editing = None;
                self.tasks_run(cx, move |b| {
                    let t = match editing {
                        Editing::New => b.add_task_draft(&draft)?,
                        Editing::Task(uid) => b.edit_task_draft(&uid, &draft)?,
                    };
                    Ok(t!("calendar-saved-the-task", summary = t.event.summary))
                });
            }
            TaskAction::Delete(uid) => {
                self.tasks.editing = None;
                self.tasks_run(cx, move |b| b.delete_task(&uid).map(|()| t!("tasks-deleted").into()));
            }
            TaskAction::Done(uid) => {
                if self.tasks.editing == Some(Editing::Task(uid.clone())) {
                    self.tasks.editing = None;
                }
                // Undone from the history; a repeating one goes on instead.
                let repeats = self.backend.calendar_tasks().iter().any(|t| t.event.uid == uid && !t.event.recurrence.is_empty());
                if self.tasks.busy {
                    return;
                }
                self.tasks.busy = true;
                self.tasks.error = None;
                let done = uid.clone();
                self.task(cx, move |b| b.complete_task(&done, true), move |this, result, cx| {
                    this.tasks.busy = false;
                    match result {
                        Ok(()) if repeats => this.notice(t!("tasks-done"), false, cx),
                        Ok(()) => this.notice_undo(t!("tasks-done"), Undo::TaskDone(uid), cx),
                        Err(e) => this.tasks.error = Some(format!("{e:#}")),
                    }
                });
            }
            TaskAction::Reopen(uid) => self.reopen_task_now(uid, cx),
            TaskAction::ClearCompleted => {
                self.tasks_run(cx, |b| b.clear_completed_tasks().map(|()| t!("tasks-history-cleared").into()));
            }
            TaskAction::OpenLink(url) => {
                if url.get(..4).is_some_and(|p| p.eq_ignore_ascii_case("mid:")) {
                    self.app_view = None;
                }
                self.open_email_link(url, cx);
            }
            TaskAction::FromMail(id, selection) => self.add_mail_task(id, selection, cx),
            TaskAction::Sync => {
                let history = self.tasks.filter == TaskFilter::Completed;
                self.tasks_run(cx, move |b| {
                    let n = b.sync_tasks()?;
                    if history {
                        // Done elsewhere: not worth failing the tasks for.
                        let _ = b.sync_completed_tasks();
                    }
                    Ok(t!("tasks-read", n = n))
                });
            }
            TaskAction::ConnectService(id) => {
                let Some(a) = self.accounts.iter().find(|a| a.id == id).cloned() else { return };
                self.tasks_run(cx, move |b| {
                    let cancel = Arc::new(AtomicBool::new(false));
                    let n = b.connect_task_service(&a, cancel)?;
                    Ok(t!("tasks-read-from", n = n, email = a.email))
                });
            }
            TaskAction::SyncSource(id) => {
                self.tasks_run(cx, move |b| {
                    let n = b.in_calendar_source(&id, || b.sync_tasks())?;
                    Ok(t!("tasks-read", n = n))
                });
            }
            TaskAction::RemoveSource(id) => {
                self.tasks.list = None;
                self.tasks_run(cx, move |b| b.remove_calendar_source(&id).map(|()| t!("tasks-service-removed").into()));
            }
            TaskAction::DefaultList(href) => {
                if let Err(e) = self.backend.set_default_task_list(&href) {
                    self.tasks.error = Some(format!("{e:#}"));
                }
            }
        }
        cx.notify();
    }

    /// Makes a done task (its UID) to do again.
    pub(super) fn reopen_task_now(&mut self, uid: String, cx: &mut Context<Self>) {
        self.tasks_run(cx, move |b| {
            let t = b.reopen_task(&uid)?;
            Ok(t!("tasks-opened-again", summary = t.event.summary))
        });
    }
    /// Takes back a task just added (Undo).
    pub(super) fn remove_added_task(&mut self, uid: String, cx: &mut Context<Self>) {
        self.tasks_run(cx, move |b| b.delete_task(&uid).map(|()| t!("tasks-deleted").into()));
    }

    /// "Add to Tasks": a task about message `id` at once, where new tasks
    /// go, linking back to it; the text chosen in it in its notes.
    pub(super) fn add_mail_task(&mut self, id: String, selection: Option<String>, cx: &mut Context<Self>) {
        let Some(m) = self.messages.iter().find(|m| m.id == id) else { return };
        if self.backend.calendar_server().is_none() {
            self.notice(t!("tasks-set-up-a-task-list-first"), true, cx);
            return;
        }
        let draft = TaskDraft::from_mail(m, selection.as_deref());
        self.menu = None;
        self.task(
            cx,
            move |b| b.add_task_draft(&draft),
            |this, result, cx| match result {
                Ok(t) => this.notice_undo(t!("tasks-added-to-tasks", summary = t.event.summary), Undo::TaskAdded(t.event.uid), cx),
                Err(e) => this.notice(format!("{e:#}"), true, cx),
            },
        );
    }

    /// How many tasks are due today or overdue, for the sidebar.
    pub(super) fn tasks_due_now(&self) -> usize {
        use std::hash::{Hash, Hasher};
        let today = chrono::Local::now().date_naive();
        // Counted again only when the tasks, the day or the workspace changed.
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        today.hash(&mut hasher);
        self.workspace().map(|w| (w.id.clone(), w.calendars.clone())).hash(&mut hasher);
        for id in self.backend.calendar_source_ids() {
            let key = crate::manage::backend::pim_sources::key_in("calendar_tasks", &id);
            self.backend.store.preference(&key).hash(&mut hasher);
        }
        let fingerprint = hasher.finish();
        if let Some((seen, count)) = self.tasks.due_now.get()
            && seen == fingerprint
        {
            return count;
        }
        let count = self.count_due_now(today);
        self.tasks.due_now.set(Some((fingerprint, count)));
        count
    }
    fn count_due_now(&self, today: chrono::NaiveDate) -> usize {
        self.backend
            .calendar_tasks()
            .iter()
            .filter(|t| self.shows_calendar(&t.calendar))
            .filter(|t| matches!(Due::of(&t.event, today), Due::Today | Due::Overdue))
            .count()
    }

    /// The Tasks view's page.
    pub(super) fn tasks_page(&self, cx: &Context<Self>) -> AnyElement {
        let c = self.colors();
        let act = |a: TaskAction| Action::Tasks(a);
        let today = chrono::Local::now().date_naive();
        let busy = self.tasks.busy;
        // The workspace's lists' tasks.
        let open: Vec<Stored> = self
            .backend
            .calendar_tasks()
            .into_iter()
            .filter(|t| self.shows_calendar(&t.calendar))
            .collect();
        let lists = self.backend.task_list_choices();
        // Each list's name, read once for the page.
        let mut names: HashMap<String, String> = HashMap::new();
        for l in self.backend.calendars().into_iter().chain(self.backend.task_lists()).chain(lists.iter().cloned()) {
            names.insert(l.href, l.name);
        }
        let list_name = |href: &str| names.get(href).cloned().unwrap_or_default();
        let completed = self.backend.completed_tasks().len();
        let count = |f: TaskFilter| match f {
            TaskFilter::Completed => completed,
            f => open.iter().filter(|t| f.lists(Due::of(&t.event, today))).count(),
        };
        let filters = self.segmented(
            "tasks-filter",
            TaskFilter::ALL
                .into_iter()
                .map(|f| {
                    let n = count(f);
                    let label = if n > 0 && f != TaskFilter::All { format!("{} {n}", f.label()) } else { f.label().to_owned() };
                    (label, act(TaskAction::Filter(f)), self.tasks.filter == f)
                })
                .collect(),
            cx,
        );
        let header = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_2()
            .child(div().flex_1().min_w(px(320.)).child(filters))
            .child(
                self.button("tasks-sync", if busy { t!("tasks-reading") } else { t!("tasks-read-now") }, "refresh", act(TaskAction::Sync), false, cx)
                    .when(busy, |d| d.opacity(0.6)),
            )
            .child(self.button("tasks-new", t!("tasks-new-task"), "plus", act(TaskAction::New(None)), true, cx));

        let quick = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_2()
            .child(
                div()
                    .id("tasks-quick")
                    .debug_selector(|| "tasks-quick".into())
                    .flex_1()
                    .min_w(px(220.))
                    .child(field(Input::new(&self.tasks.quick), c, busy)),
            )
            .child(
                div()
                    .id("tasks-quick-due")
                    .debug_selector(|| "tasks-quick-due".into())
                    .w(px(200.))
                    .child(field(Input::new(&self.tasks.quick_due), c, busy)),
            )
            .child(self.button("tasks-quick-add", t!("tasks-add"), "plus", act(TaskAction::QuickAdd), false, cx));

        // Lists and tags to show tasks of.
        let chip = |id: String, label: String, on: bool, action: TaskAction| {
            let selector = id.clone();
            div()
                .id(SharedString::from(id))
                .debug_selector(move || selector)
                .role(Role::Button)
                .aria_label(label.clone())
                .aria_toggled(if on { Toggled::True } else { Toggled::False })
                .px_2()
                .h(px(26.))
                .flex()
                .items_center()
                .rounded(px(13.))
                .cursor_pointer()
                .text_size(px(12.))
                .border_1()
                .border_color(rgb(if on { c.accent } else { c.border }))
                .bg(rgb(if on { c.soft } else { c.surface }))
                .text_color(rgb(if on { c.text } else { c.muted }))
                .child(label)
                .on_click(cx.listener(move |this, _, w, cx| this.handle(Action::Tasks(action.clone()), w, cx)))
        };
        let mut shown_lists: Vec<(String, String)> = lists.iter().map(|l| (l.href.clone(), l.name.clone())).collect();
        for t in &open {
            if !shown_lists.iter().any(|(h, _)| *h == t.calendar) {
                shown_lists.push((t.calendar.clone(), list_name(&t.calendar)));
            }
        }
        shown_lists.retain(|(href, _)| self.shows_calendar(href));
        let mut tags: Vec<String> = Vec::new();
        for t in &open {
            for tag in &t.event.categories {
                if !tags.iter().any(|x| x.eq_ignore_ascii_case(tag)) {
                    tags.push(tag.clone());
                }
            }
        }
        tags.sort_by_key(|t| t.to_lowercase());
        let default_list = self.backend.default_task_list();
        let mut chips = div().flex().flex_wrap().gap_1().items_center();
        if shown_lists.len() > 1 {
            chips = chips.child(chip("tasks-list-all".into(), t!("tasks-all-lists").into(), self.tasks.list.is_none(), TaskAction::List(None)));
            for (i, (href, name)) in shown_lists.iter().enumerate() {
                let name = if name.is_empty() { t!("tasks-unnamed-list").to_owned() } else { name.clone() };
                let label = if default_list.as_deref() == Some(href.as_str()) { format!("★ {name}") } else { name };
                chips = chips.child(chip(
                    format!("tasks-list-{i}"),
                    label,
                    self.tasks.list.as_deref() == Some(href.as_str()),
                    TaskAction::List(Some(href.clone())),
                ));
            }
        }
        if !tags.is_empty() {
            if shown_lists.len() > 1 {
                chips = chips.child(div().w(px(1.)).h(px(18.)).mx_1().bg(rgb(c.border)));
            }
            for (i, tag) in tags.iter().enumerate() {
                let on = self.tasks.tag.as_ref().is_some_and(|t| t.eq_ignore_ascii_case(tag));
                chips = chips.child(chip(
                    format!("tasks-tag-{i}"),
                    format!("#{tag}"),
                    on,
                    TaskAction::Tag(if on { None } else { Some(tag.clone()) }),
                ));
            }
        }
        // The list shown can take new tasks: it can be the default.
        let default_link = self
            .tasks
            .list
            .as_ref()
            .filter(|l| lists.iter().any(|x| &x.href == *l) && default_list.as_ref() != Some(*l))
            .map(|l| self.link("tasks-make-default", t!("tasks-make-default-list"), act(TaskAction::DefaultList(l.clone())), cx));

        let mut page = div()
            .flex()
            .flex_col()
            .gap_4()
            .child(header)
            .child(quick)
            .child(chips.children(default_link))
            .children(self.tasks.error.clone().map(|e| {
                div().id("tasks-error").debug_selector(|| "tasks-error".into()).text_size(px(12.)).text_color(rgb(c.danger)).child(e)
            }));
        if self.tasks.editing.is_some() {
            page = page.child(self.task_editor(&lists, &list_name, cx));
        }
        page = if self.tasks.filter == TaskFilter::Completed {
            page.child(self.completed_list(&list_name, cx))
        } else {
            page.child(self.open_task_list(open, today, &list_name, cx))
        };
        page.child(self.task_services(cx)).into_any_element()
    }

    /// The open tasks the filter, list and tag pick, grouped by when due.
    fn open_task_list(
        &self,
        open: Vec<Stored>,
        today: chrono::NaiveDate,
        list_name: &dyn Fn(&str) -> String,
        cx: &Context<Self>,
    ) -> AnyElement {
        let c = self.colors();
        let filter = self.tasks.filter;
        let tag = self.tasks.tag.clone();
        let picked: Vec<Stored> = open
            .iter()
            .filter(|t| self.tasks.list.as_ref().is_none_or(|l| &t.calendar == l))
            .filter(|t| tag.as_ref().is_none_or(|g| t.event.categories.iter().any(|x| x.eq_ignore_ascii_case(g))))
            .filter(|t| filter.lists(Due::of(&t.event, today)))
            .cloned()
            .collect();
        if picked.is_empty() {
            let text = if self.backend.task_list_choices().is_empty() && open.is_empty() {
                t!("tasks-no-task-lists")
            } else {
                match filter {
                    TaskFilter::Today => t!("tasks-nothing-today"),
                    TaskFilter::Upcoming => t!("tasks-nothing-upcoming"),
                    TaskFilter::Overdue => t!("tasks-nothing-overdue"),
                    _ => t!("tasks-nothing-to-do"),
                }
            };
            return div()
                .id("tasks-empty")
                .debug_selector(|| "tasks-empty".into())
                .py_6()
                .text_size(px(13.))
                .text_color(rgb(c.muted))
                .child(text)
                .into_any_element();
        }
        // Grouped where more than one kind is listed.
        let groups = [
            (Due::Overdue, t!("tasks-overdue")),
            (Due::Today, t!("tasks-today")),
            (Due::Upcoming, t!("tasks-upcoming")),
            (Due::Someday, t!("tasks-no-due-date")),
        ];
        let mut out = div().flex().flex_col().gap_4();
        let mut index = 0;
        let several = groups.iter().filter(|(d, _)| picked.iter().any(|t| Due::of(&t.event, today) == *d)).count() > 1;
        for (due, title) in groups {
            let group: Vec<Stored> = picked.iter().filter(|t| Due::of(&t.event, today) == due).cloned().collect();
            if group.is_empty() {
                continue;
            }
            let rows = model::ordered(group);
            let count = rows.len();
            let mut list = div()
                .id(SharedString::from(format!("tasks-group-{due:?}")))
                .role(Role::List)
                .aria_label(title)
                .rounded(px(12.))
                .border_1()
                .border_color(rgb(c.border))
                .bg(rgb(c.raised))
                .overflow_hidden()
                .flex()
                .flex_col();
            for (n, (t, depth)) in rows.into_iter().enumerate() {
                list = list.child(self.task_row(index, n, count, &t, depth, due, list_name, cx));
                index += 1;
            }
            out = out.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .when(several, |d| {
                        d.child(
                            div()
                                .text_size(px(12.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(if due == Due::Overdue { c.danger } else { c.muted }))
                                .child(format!("{title} · {count}")),
                        )
                    })
                    .child(list),
            );
        }
        out.into_any_element()
    }

    /// One open task: done, its name, when it's due, its priority, list
    /// and tags; its email, a subtask, and editing it.
    #[allow(clippy::too_many_arguments)]
    fn task_row(
        &self,
        i: usize,
        position: usize,
        count: usize,
        t: &Stored,
        depth: usize,
        due: Due,
        list_name: &dyn Fn(&str) -> String,
        cx: &Context<Self>,
    ) -> AnyElement {
        let c = self.colors();
        let e = &t.event;
        let uid = e.uid.clone();
        let when = due_text(e);
        let mut details: Vec<AnyElement> = Vec::new();
        if !when.is_empty() {
            details.push(
                div()
                    .text_color(rgb(if due == Due::Overdue { c.danger } else { c.muted }))
                    .child(if due == Due::Overdue { t!("calendar-overdue", due = when) } else { t!("calendar-due-2", due = when) })
                    .into_any_element(),
            );
        }
        if e.priority > 0 {
            let color = match e.priority {
                1..=4 => c.danger,
                5 => c.warning,
                _ => c.muted,
            };
            details.push(div().text_color(rgb(color)).child(priority_label(e.priority)).into_any_element());
        }
        let list = list_name(&t.calendar);
        if !list.is_empty() {
            details.push(div().child(list).into_any_element());
        }
        for tag in &e.categories {
            details.push(div().text_color(rgb(c.accent)).child(format!("#{tag}")).into_any_element());
        }
        let done = {
            let uid = uid.clone();
            let id = format!("task-done-{i}");
            let selector = id.clone();
            div()
                .id(SharedString::from(id))
                .debug_selector(move || selector)
                .role(Role::CheckBox)
                .aria_label(t!("tasks-mark-done", summary = e.summary))
                .aria_toggled(Toggled::False)
                .focusable()
                .tab_stop(true)
                .focus_visible(move |s| s.border_2().border_color(rgb(c.accent)))
                .size(px(20.))
                .flex_shrink_0()
                .rounded_full()
                .border_2()
                .border_color(rgb(if e.priority > 0 && e.priority <= 4 { c.danger } else { c.border }))
                .cursor_pointer()
                .hover(move |s| s.bg(rgb(c.soft)))
                .on_click(cx.listener(move |this, _, w, cx| this.handle(Action::Tasks(TaskAction::Done(uid.clone())), w, cx)))
        };
        let mut actions = div().flex().items_center().gap_1();
        if !e.url.is_empty() {
            actions = actions.child(self.button(
                SharedString::from(format!("task-link-{i}")),
                if e.url.to_ascii_lowercase().starts_with("mid:") { t!("tasks-open-email") } else { t!("tasks-open-link") },
                "mail",
                Action::Tasks(TaskAction::OpenLink(e.url.clone())),
                false,
                cx,
            ));
        }
        if depth == 0 {
            actions = actions.child(self.button(
                SharedString::from(format!("task-subtask-{i}")),
                t!("tasks-add-subtask"),
                "plus",
                Action::Tasks(TaskAction::New(Some(uid.clone()))),
                false,
                cx,
            ));
        }
        actions = actions.child(self.button(
            SharedString::from(format!("task-edit-{i}")),
            t!("calendar-edit"),
            "compose",
            Action::Tasks(TaskAction::Edit(uid.clone())),
            false,
            cx,
        ));
        div()
            .id(SharedString::from(format!("task-row-{i}")))
            .debug_selector(move || format!("task-row-{i}"))
            .role(Role::ListItem)
            .aria_label(e.summary.clone())
            .aria_position_in_set(position + 1)
            .aria_size_of_set(count)
            .pl(px(16. + 28. * depth as f32))
            .pr_4()
            .py(px(10.))
            .when(position > 0, |d| d.border_t_1().border_color(rgb(c.border)))
            .flex()
            .items_center()
            .gap_3()
            .child(done)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(div().text_size(px(13.5)).truncate().child(e.summary.clone()))
                    .when(!details.is_empty(), |d| {
                        d.child(div().flex().flex_wrap().gap_x_3().text_size(px(11.5)).text_color(rgb(c.muted)).children(details))
                    })
                    .when(!e.description.is_empty(), |d| {
                        d.child(
                            div()
                                .text_size(px(11.5))
                                .text_color(rgb(c.muted))
                                .truncate()
                                .child(e.description.lines().next().unwrap_or_default().to_owned()),
                        )
                    }),
            )
            .child(actions)
            .into_any_element()
    }

    /// The tasks done lately, each to open again.
    fn completed_list(&self, list_name: &dyn Fn(&str) -> String, cx: &Context<Self>) -> AnyElement {
        let c = self.colors();
        let done: Vec<Stored> = self
            .backend
            .completed_tasks()
            .into_iter()
            .filter(|t| self.shows_calendar(&t.calendar))
            .filter(|t| self.tasks.list.as_ref().is_none_or(|l| &t.calendar == l))
            .collect();
        if done.is_empty() {
            return div()
                .id("tasks-empty")
                .debug_selector(|| "tasks-empty".into())
                .py_6()
                .text_size(px(13.))
                .text_color(rgb(c.muted))
                .child(t!("tasks-nothing-done"))
                .into_any_element();
        }
        let count = done.len();
        let rows = done.into_iter().take(200).enumerate().map(|(i, t)| {
            let when = chrono::DateTime::from_timestamp(t.event.completed_at, 0)
                .filter(|_| t.event.completed_at != 0)
                .map(|d| d.with_timezone(&chrono::Local).local(t!("dates-weekday-month-day-time")))
                .unwrap_or_default();
            let list = list_name(&t.calendar);
            let detail = match (when.is_empty(), list.is_empty()) {
                (false, false) => t!("tasks-done-on-in", when = when, list = list),
                (false, true) => t!("tasks-done-on", when = when),
                _ => list,
            };
            div()
                .id(SharedString::from(format!("task-completed-{i}")))
                .debug_selector(move || format!("task-completed-{i}"))
                .role(Role::ListItem)
                .aria_position_in_set(i + 1)
                .aria_size_of_set(count)
                .px_4()
                .py(px(10.))
                .when(i > 0, |d| d.border_t_1().border_color(rgb(c.border)))
                .flex()
                .items_center()
                .gap_3()
                .child(icon_el("check", 16., c.accent))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(div().text_size(px(13.)).truncate().text_color(rgb(c.muted)).child(t.event.summary.clone()))
                        .child(div().text_size(px(11.5)).text_color(rgb(c.muted)).child(detail)),
                )
                .when(!t.event.url.is_empty(), |d| {
                    d.child(self.button(
                        SharedString::from(format!("task-completed-link-{i}")),
                        t!("tasks-open-email"),
                        "mail",
                        Action::Tasks(TaskAction::OpenLink(t.event.url.clone())),
                        false,
                        cx,
                    ))
                })
                .child(self.button(
                    SharedString::from(format!("task-reopen-{i}")),
                    t!("tasks-reopen"),
                    "undo",
                    Action::Tasks(TaskAction::Reopen(t.event.uid.clone())),
                    false,
                    cx,
                ))
        });
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .id("tasks-completed")
                    .role(Role::List)
                    .aria_label(t!("tasks-completed"))
                    .rounded(px(12.))
                    .border_1()
                    .border_color(rgb(c.border))
                    .bg(rgb(c.raised))
                    .overflow_hidden()
                    .flex()
                    .flex_col()
                    .children(rows),
            )
            .child(div().flex().child(self.link("tasks-clear-completed", t!("tasks-clear-history"), Action::Tasks(TaskAction::ClearCompleted), cx)))
            .into_any_element()
    }

    /// The task editor: name, due, notes, priority, tags and list.
    fn task_editor(&self, lists: &[crate::sync::caldav::Calendar], list_name: &dyn Fn(&str) -> String, cx: &Context<Self>) -> AnyElement {
        let c = self.colors();
        let busy = self.tasks.busy;
        let act = |a: TaskAction| Action::Tasks(a);
        let label = |text: &'static str| div().text_size(px(12.)).font_weight(FontWeight::MEDIUM).text_color(rgb(c.muted)).child(text);
        let labelled = |text: &'static str, id: &'static str, input: &Entity<InputState>| {
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(label(text))
                .child(div().id(id).debug_selector(move || id.into()).child(field(Input::new(input), c, busy)))
        };
        let editing = self.tasks.editing.clone();
        let title = match (&editing, self.tasks.parent.is_empty()) {
            (Some(Editing::Task(_)), _) => t!("calendar-edit-task-2").to_owned(),
            (_, false) => t!("tasks-new-subtask", parent = list_parent_name(&self.backend.calendar_tasks(), &self.tasks.parent)),
            _ => t!("tasks-new-task").to_owned(),
        };
        let priorities = self.segmented(
            "task-priority",
            PRIORITIES
                .into_iter()
                .map(|p| (priority_label(p).to_owned(), act(TaskAction::Priority(p)), priority_label(self.tasks.priority) == priority_label(p)))
                .collect(),
            cx,
        );
        // A subtask stays in its parent's list.
        let choose_list = self.tasks.parent.is_empty() && lists.len() > 1;
        let list_choice = choose_list.then(|| {
            let target = if self.tasks.target.is_empty() { lists.first().map(|l| l.href.clone()).unwrap_or_default() } else { self.tasks.target.clone() };
            self.segmented(
                "task-list",
                lists
                    .iter()
                    .map(|l| {
                        let name = if l.name.is_empty() { list_name(&l.href) } else { l.name.clone() };
                        (name, act(TaskAction::TargetList(l.href.clone())), l.href == target)
                    })
                    .collect(),
                cx,
            )
        });
        let mut buttons = div()
            .flex()
            .flex_wrap()
            .gap_2()
            .child(self.button("task-save", t!("calendar-save"), "check", act(TaskAction::Save), true, cx))
            .child(self.button("task-cancel", t!("calendar-cancel"), "close", act(TaskAction::Cancel), false, cx));
        if let Some(Editing::Task(uid)) = &editing {
            buttons = buttons.child(
                self.button("task-delete", t!("calendar-delete"), "trash", act(TaskAction::Delete(uid.clone())), false, cx)
                    .text_color(rgb(c.danger)),
            );
        }
        div()
            .id("tasks-editor")
            .debug_selector(|| "tasks-editor".into())
            .role(Role::Form)
            .aria_label(title.clone())
            .p_4()
            .rounded(px(12.))
            .border_1()
            .border_color(rgb(c.accent))
            .bg(rgb(c.raised))
            .flex()
            .flex_col()
            .gap_3()
            .max_w(px(560.))
            .child(div().text_size(px(14.)).font_weight(FontWeight::SEMIBOLD).child(title))
            .child(labelled(t!("tasks-title"), "task-title", &self.tasks.title))
            .child(labelled(t!("tasks-due"), "task-due", &self.tasks.due))
            .child(labelled(t!("tasks-notes"), "task-notes", &self.tasks.notes))
            .child(div().flex().flex_col().gap_1().child(label(t!("tasks-priority"))).child(priorities))
            .child(labelled(t!("tasks-tags"), "task-tags", &self.tasks.tags))
            .children(list_choice.map(|l| div().flex().flex_col().gap_1().child(label(t!("tasks-list"))).child(l)))
            .when(!self.tasks.url.is_empty(), |d| {
                d.child(self.link("task-editor-link", t!("tasks-open-linked-email"), act(TaskAction::OpenLink(self.tasks.url.clone())), cx))
            })
            .child(buttons)
            .into_any_element()
    }

    /// Task services to add (Microsoft To Do, Google Tasks), and those added.
    fn task_services(&self, cx: &Context<Self>) -> AnyElement {
        let mut rows = Rows::new();
        for (id, server, synced) in self.backend.calendar_sources().into_iter().filter(|(_, s, _)| s.tasks_only) {
            let synced = synced
                .and_then(|t| chrono::DateTime::from_timestamp(t, 0))
                .map(|t| t.with_timezone(&chrono::Local).local(t!("dates-month-day-time")))
                .unwrap_or_else(|| t!("calendar-never-read").to_owned());
            rows = rows.row(self.setting_row(
                &server.label,
                &t!("tasks-service-read", synced = synced),
                div()
                    .flex()
                    .gap_2()
                    .child(self.button(
                        SharedString::from(format!("tasks-service-sync-{id}")),
                        t!("calendar-read-now"),
                        "refresh",
                        Action::Tasks(TaskAction::SyncSource(id.clone())),
                        false,
                        cx,
                    ))
                    .child(self.button(
                        SharedString::from(format!("tasks-service-remove-{id}")),
                        t!("calendar-disconnect"),
                        "close",
                        Action::Tasks(TaskAction::RemoveSource(id.clone())),
                        false,
                        cx,
                    )),
            ));
        }
        for a in self.backend.task_service_accounts() {
            let google = a.provider == Provider::Gmail;
            rows = rows.row(self.setting_row(
                &a.email,
                if google { t!("tasks-google-tasks-caption") } else { t!("tasks-microsoft-to-do-caption") },
                self.button(
                    SharedString::from(format!("tasks-connect-{}", a.id)),
                    if google { t!("tasks-add-google-tasks") } else { t!("tasks-add-microsoft-to-do") },
                    "plus",
                    Action::Tasks(TaskAction::ConnectService(a.id.clone())),
                    false,
                    cx,
                ),
            ));
        }
        self.settings_group(t!("tasks-task-services"), rows).into_any_element()
    }
}

/// The name of task `uid` among `tasks`, for "Subtask of …".
fn list_parent_name(tasks: &[Stored], uid: &str) -> String {
    tasks.iter().find(|t| t.event.uid == uid).map(|t| t.event.summary.clone()).unwrap_or_default()
}

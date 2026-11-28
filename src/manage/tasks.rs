//! Tasks as a workspace of their own (the Tasks view): every source's task
//! lists together, tasks made with a priority, tags, a parent (subtasks)
//! and a link, the Completed history, tasks made from email with a link
//! back to it, and the task services that aren't calendars: Microsoft To
//! Do (through Microsoft Graph) and Google Tasks.
//!
//! Tasks are kept where they always were (`calendar.rs`: each calendar
//! source's `calendar_tasks`), CalDAV's VTODOs and JMAP's Task Lists as
//! before. A To Do or Google Tasks account is a calendar source of tasks
//! alone (`Server::tasks_only`), set up beside the others; its lists are
//! that source's task lists (`todo:` and `gtasks:` hrefs), as a JMAP
//! server's are.
//!
//! The Completed history is what was done here, newest first, per source
//! (`TASKS_DONE`): servers are read for what is still to do, and a done
//! task can be opened again from it.
use super::*;
use crate::manage::backend::dav_accounts::MailProvider;
use crate::sync::tasks::{RemoteList, RemoteTask};

/// The list new tasks go to when none is chosen (its href), across sources.
const DEFAULT_TASK_LIST: &str = "tasks_default_list";
/// The most done tasks kept in the history, per source.
const MOST_DONE: usize = 300;
/// How long a done task stays in the history.
const DONE_FOR: i64 = 90 * 86_400;

/// A task as the Tasks view writes it, to add or to change.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TaskDraft {
    pub summary: String,
    pub notes: String,
    /// When it's due (0: no due date); a date alone with `all_day`.
    pub due: i64,
    pub all_day: bool,
    /// 1 the highest … 9 the lowest; 0 none.
    pub priority: u8,
    pub tags: Vec<String>,
    /// The task it's part of (its UID), for a subtask.
    pub parent: String,
    /// Where there's more about it: an email's `mid:` link.
    pub url: String,
    /// The list it goes to (its href); empty: where new tasks go.
    pub list: String,
}

impl TaskDraft {
    /// Tags as typed: separated by commas, `#` before each allowed.
    pub fn tags_from(text: &str) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for t in text.split([',', ';']) {
            let t = t.trim().trim_start_matches('#').trim();
            if !t.is_empty() && !out.iter().any(|o| o.eq_ignore_ascii_case(t)) {
                out.push(t.to_owned());
            }
        }
        out
    }
    /// A task to do about email `m`: named after its subject (or the first
    /// line of the text chosen in it), with what was chosen, who it's from
    /// and a link back to it (RFC 2392 `mid:`) in its notes.
    pub fn from_mail(m: &Mail, selection: Option<&str>) -> Self {
        let chosen = selection.map(str::trim).filter(|s| !s.is_empty());
        let subject = m.subject.trim();
        let summary = match chosen {
            Some(text) if subject.is_empty() => first_line(text),
            _ if subject.is_empty() => t!("manage-tasks-no-subject").to_owned(),
            _ => subject.to_owned(),
        };
        let mut notes = Vec::new();
        if let Some(text) = chosen {
            notes.push(text.to_owned());
        }
        let from = m.from.trim();
        if !from.is_empty() {
            let when = chrono::DateTime::from_timestamp(m.date, 0)
                .map(|d| d.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string())
                .unwrap_or_default();
            notes.push(t!("manage-tasks-from-email", from = from, when = when));
        }
        Self {
            summary,
            notes: notes.join("\n\n"),
            url: mail_link(&m.message_id),
            ..Default::default()
        }
    }
}

/// The first line of `text`, not too long to name a task.
fn first_line(text: &str) -> String {
    let line = text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or_default();
    match line.char_indices().nth(120) {
        Some((at, _)) => format!("{}…", line[..at].trim_end()),
        None => line.to_owned(),
    }
}

/// What may not stand in a `mid:` link as it is (RFC 2392, RFC 3986).
const MID: &percent_encoding::AsciiSet = &percent_encoding::CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'%')
    .add(b'/')
    .add(b'?')
    .add(b'<')
    .add(b'>')
    .add(b'[')
    .add(b']')
    .add(b'\\')
    .add(b'^')
    .add(b'`')
    .add(b'{')
    .add(b'|')
    .add(b'}');

/// A link to the email with Message-ID `id` (RFC 2392): what opens it here
/// again (`links::open_email_link`); empty without one.
pub fn mail_link(id: &str) -> String {
    let id = id.trim().trim_start_matches('<').trim_end_matches('>').trim();
    if id.is_empty() {
        return String::new();
    }
    format!("mid:{}", percent_encoding::utf8_percent_encode(id, MID))
}

/// Where a task stands for the Tasks view's lists.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Due {
    /// Due before today.
    Overdue,
    Today,
    /// Due after today.
    Upcoming,
    /// No due date.
    Someday,
}

impl Due {
    /// Where task `e` stands on `today` (the local date). An all-day
    /// task is due on its date; a timed one on its local date.
    pub fn of(e: &Event, today: chrono::NaiveDate) -> Self {
        if e.start == 0 {
            return Self::Someday;
        }
        let Some(at) = chrono::DateTime::from_timestamp(e.start, 0) else {
            return Self::Someday;
        };
        let day = if e.all_day { at.date_naive() } else { at.with_timezone(&chrono::Local).date_naive() };
        match day.cmp(&today) {
            std::cmp::Ordering::Less => Self::Overdue,
            std::cmp::Ordering::Equal => Self::Today,
            std::cmp::Ordering::Greater => Self::Upcoming,
        }
    }
}

/// Tasks in the order the Tasks view lists them: soonest due first, then
/// the most important, then by name; each task's subtasks right after it.
/// Returns each with how deep it is (0, or 1 for a subtask).
pub fn ordered(tasks: Vec<Stored>) -> Vec<(Stored, usize)> {
    let rank = |s: &Stored| {
        let due = if s.event.start == 0 { i64::MAX } else { s.event.start };
        let priority = if s.event.priority == 0 { 10 } else { s.event.priority };
        (due, priority, s.event.summary.to_lowercase())
    };
    let uids: std::collections::HashSet<String> = tasks.iter().map(|t| t.event.uid.clone()).collect();
    let (mut top, mut subs): (Vec<Stored>, Vec<Stored>) =
        tasks.into_iter().partition(|t| t.event.parent.is_empty() || !uids.contains(&t.event.parent));
    top.sort_by_key(rank);
    subs.sort_by_key(rank);
    let mut out = Vec::with_capacity(top.len() + subs.len());
    for t in top {
        let uid = t.event.uid.clone();
        out.push((t, 0));
        let (mine, rest): (Vec<Stored>, Vec<Stored>) = subs.into_iter().partition(|s| s.event.parent == uid);
        subs = rest;
        out.extend(mine.into_iter().map(|s| (s, 1)));
    }
    // Subtasks of subtasks: after the rest, one deep.
    out.extend(subs.into_iter().map(|s| (s, 1)));
    out
}

impl Backend {
    /// Every list a new task can go to, in every source: CalDAV calendars
    /// that take tasks, and JMAP's, To Do's and Google's task lists.
    pub fn task_list_choices(&self) -> Vec<caldav::Calendar> {
        if !within_one() {
            return self
                .calendar_source_ids()
                .into_iter()
                .flat_map(|id| self.in_calendar_source(&id, || self.task_list_choices()))
                .collect();
        }
        let Some(server) = self.calendar_server() else { return Vec::new() };
        if server.feed {
            return Vec::new();
        }
        if !server.jmap_account.is_empty() || server.tasks_only {
            return self.task_lists().into_iter().filter(|l| l.writable).collect();
        }
        if !server.dav() {
            return Vec::new();
        }
        self.calendars()
            .into_iter()
            .filter(|c| c.writable && (c.components.is_empty() || c.components.iter().any(|k| k == "VTODO")))
            .collect()
    }

    /// Adds a task as written in the Tasks view: in the list chosen, else
    /// where new tasks go.
    pub fn add_task_draft(&self, d: &TaskDraft) -> Result<Stored> {
        ensure!(!d.summary.trim().is_empty(), t!("manage-calendar-give-the-task-a-name"));
        if !within_one() {
            // In the list chosen; a subtask with its parent; else in the
            // list chosen for new tasks, or where new events go where it
            // keeps tasks, else the first source that does.
            let mut d = d.clone();
            if d.list.is_empty() && d.parent.is_empty() {
                d.list = self.default_task_list().unwrap_or_default();
            }
            let id = if !d.list.is_empty() {
                self.calendar_source_of(&d.list)
            } else if !d.parent.is_empty() {
                self.calendar_source_of_event(&d.parent)
            } else {
                None
            };
            let id = id.unwrap_or_else(|| {
                let takes = |id: &String| {
                    self.in_calendar_source(id, || {
                        self.calendar_server().is_some_and(|s| !s.jmap_account.is_empty()) || self.task_calendar().is_some()
                    })
                };
                let first = self.default_calendar_source();
                std::iter::once(first.clone()).chain(self.calendar_source_ids()).find(takes).unwrap_or(first)
            });
            return self.in_calendar_source(&id, || self.add_task_draft(&d));
        }
        let server = self
            .calendar_server()
            .with_context(|| t!("manage-calendar-set-up-a-calendar-server-in"))?;
        let list = if !d.list.is_empty() {
            d.list.clone()
        } else if !d.parent.is_empty()
            && let Some(p) = self.calendar_tasks().into_iter().find(|t| t.event.uid == d.parent)
        {
            p.calendar
        } else if let Some(a) = self.calendar_jmap(&server)? {
            self.jmap_task_list(&a)?
        } else {
            self.task_calendar().with_context(|| t!("manage-calendar-none-of-your-calendars-can-take"))?.href
        };
        let mut event = Event {
            uid: format!("{}@picacho", uuid::Uuid::new_v4()),
            task: true,
            utc: true,
            ..Default::default()
        };
        apply(&mut event, d);
        let stored = Stored { event, calendar: list.clone(), ..Default::default() };
        let saved = self.calendar_save(&server, &list, &stored)?;
        let mut tasks = self.calendar_tasks();
        tasks.push(saved.clone());
        caldav::sort_tasks(&mut tasks);
        self.set_cal_pref(TASKS, &serde_json::to_string(&tasks)?)?;
        Ok(saved)
    }

    /// Changes a task as written in the Tasks view, keeping the rest of it
    /// as the server has it. A task moved to another list is made there
    /// and taken out of the one it was in.
    pub fn edit_task_draft(&self, uid: &str, d: &TaskDraft) -> Result<Stored> {
        ensure!(!d.summary.trim().is_empty(), t!("manage-calendar-give-the-task-a-name"));
        if let Some(id) = self.route_event(uid) {
            return self.in_calendar_source(&id, || self.edit_task_draft(uid, d));
        }
        let mut tasks = self.calendar_tasks();
        let index = tasks
            .iter()
            .position(|t| t.event.uid == uid)
            .with_context(|| t!("manage-calendar-this-task-is-no-longer-on"))?;
        if !d.list.is_empty() && d.list != tasks[index].calendar {
            let mut moved = tasks[index].clone();
            apply(&mut moved.event, d);
            return pim_sources::outside(Pim::Calendar, || self.move_task(moved, &d.list));
        }
        let server = self
            .calendar_server()
            .with_context(|| t!("manage-calendar-no-calendar-server-is-set-up"))?;
        let task = &mut tasks[index];
        let due_changed = task.event.start != d.due;
        apply(&mut task.event, d);
        task.event.sequence += 1;
        let calendar = task.calendar.clone();
        *task = self.calendar_save(&server, &calendar, task)?;
        if due_changed && !task.event.source.is_empty() {
            task.event.alarms = ical::task_alarms(&task.event.source, d.due);
        }
        let saved = task.clone();
        caldav::sort_tasks(&mut tasks);
        self.set_cal_pref(TASKS, &serde_json::to_string(&tasks)?)?;
        if !saved.event.recurrence.is_empty() && self.calendar_jmap(&server)?.is_some() {
            self.sync_tasks()?;
        }
        Ok(saved)
    }

    /// Moves task `task` (as edited) to list `to`, with what Picacho doesn't
    /// edit of it (its alarms, other apps' properties, its link) and its
    /// subtasks. A JMAP task moves between its account's lists where it is;
    /// elsewhere it is made in the list and taken out of the one it was in.
    /// Outside any source.
    fn move_task(&self, task: Stored, to: &str) -> Result<Stored> {
        let uid = task.event.uid.clone();
        let from = self.calendar_source_of_event(&uid).with_context(|| t!("manage-calendar-this-task-is-no-longer-on"))?;
        let target = self.calendar_source_of(to).with_context(|| t!("manage-tasks-not-a-task-list"))?;
        let children: Vec<Stored> = self
            .in_calendar_source(&from, || self.calendar_tasks())
            .into_iter()
            .filter(|t| t.event.parent == uid && t.calendar == task.calendar)
            .collect();
        let jmap = self.in_calendar_source(&target, || self.calendar_server()).is_some_and(|s| !s.jmap_account.is_empty());
        if from == target && jmap {
            // Its list changed in place (`taskListIds`); its subtasks with it.
            return self.in_calendar_source(&from, || {
                let server = self.calendar_server().with_context(|| t!("manage-calendar-no-calendar-server-is-set-up"))?;
                let mut tasks = self.calendar_tasks();
                let mut saved = None;
                for t in std::iter::once(task.clone()).chain(children.clone()) {
                    let mut moving = t.clone();
                    moving.event.sequence += 1;
                    moving.calendar = to.to_owned();
                    let done = self.calendar_save(&server, to, &moving)?;
                    tasks.retain(|k| k.event.uid != done.event.uid);
                    tasks.push(done.clone());
                    saved.get_or_insert(done);
                }
                caldav::sort_tasks(&mut tasks);
                self.set_cal_pref(TASKS, &serde_json::to_string(&tasks)?)?;
                saved.with_context(|| t!("manage-calendar-this-task-is-no-longer-on"))
            });
        }
        let made = self.in_calendar_source(&target, || {
            let server = self.calendar_server().with_context(|| t!("manage-calendar-no-calendar-server-is-set-up"))?;
            let mut new = task.clone();
            new.calendar = to.to_owned();
            new.href = String::new();
            new.etag = String::new();
            new.schedule_tag = String::new();
            // Its parent stays behind in the other list.
            new.event.parent = String::new();
            let saved = self.calendar_save(&server, to, &new)?;
            let mut tasks = self.calendar_tasks();
            tasks.push(saved.clone());
            caldav::sort_tasks(&mut tasks);
            self.set_cal_pref(TASKS, &serde_json::to_string(&tasks)?)?;
            Ok::<_, anyhow::Error>(saved)
        })?;
        // Its subtasks go with it, under it there.
        for child in children {
            let mut child = child;
            child.event.parent = made.event.uid.clone();
            let child_uid = child.event.uid.clone();
            let mut new = child.clone();
            new.calendar = to.to_owned();
            new.href = String::new();
            new.etag = String::new();
            new.schedule_tag = String::new();
            let saved = self.in_calendar_source(&target, || {
                let server = self.calendar_server().with_context(|| t!("manage-calendar-no-calendar-server-is-set-up"))?;
                let saved = self.calendar_save(&server, to, &new)?;
                let mut tasks = self.calendar_tasks();
                tasks.push(saved.clone());
                caldav::sort_tasks(&mut tasks);
                self.set_cal_pref(TASKS, &serde_json::to_string(&tasks)?)?;
                Ok::<_, anyhow::Error>(saved)
            });
            if saved.is_ok() {
                self.in_calendar_source(&from, || self.delete_task(&child_uid))?;
            }
        }
        self.in_calendar_source(&from, || self.delete_task(&uid))?;
        Ok(made)
    }

    /// Reads what was done lately on the servers into the Completed
    /// history, beside what was done here: CalDAV's VTODOs completed since
    /// (COMPLETED), JMAP's finished tasks, To Do's completed tasks and
    /// checked steps, Google's completed tasks. Returns how many there are.
    pub fn sync_completed_tasks(&self) -> Result<usize> {
        if !within_one() {
            return together(self.each_calendar_source(|| self.sync_completed_tasks()), |id| self.calendar_source_label(id));
        }
        let server = self.calendar_server().with_context(|| t!("manage-calendar-no-calendar-server-is-set-up"))?;
        let now = chrono::Utc::now().timestamp();
        let since = now - DONE_FOR;
        let found: Vec<Stored> = if server.feed {
            return Ok(0);
        } else if server.tasks_only {
            let (prefix, read) = if let Some(a) = self.calendar_graph(&server)? {
                ("todo", self.with_graph_calendars(&a, |s| s.todo_completed(&s.todo_lists()?, since))?)
            } else if let Some(a) = self.calendar_google(&server)? {
                ("gtasks", self.with_google(&a, |s| s.completed_tasks(&s.task_lists()?, since))?)
            } else {
                return Ok(0);
            };
            let mut found = remote_stored(prefix, read);
            self.merge_kept(&mut found);
            found
        } else if let Some(a) = self.calendar_jmap(&server)? {
            self.with_jmap_calendars(&a, |s| {
                let lists = crate::sync::jscal::task_lists(s)?;
                crate::sync::jscal::completed_tasks(s, &lists, since)
            })?
            .into_iter()
            .map(|t| Stored {
                etag: t.version,
                event: t.event,
                calendar: format!("jmap:{}", t.list),
                href: format!("jmap:{}", t.id),
                ..Default::default()
            })
            .collect()
        } else if server.dav() {
            let calendars: Vec<caldav::Calendar> = self
                .calendars()
                .into_iter()
                .filter(|c| c.components.is_empty() || c.components.iter().any(|k| k == "VTODO"))
                .collect();
            caldav::completed_tasks(&server, &self.calendar_password()?, self.tls_connector()?, &calendars, since)?
        } else {
            return Ok(0);
        };
        let mut history = self.own_done();
        for t in found {
            match history.iter_mut().find(|h| h.event.uid == t.event.uid) {
                // As the server has it; when it was done as known here
                // where the server doesn't say.
                Some(h) => {
                    let at = if t.event.completed_at > 0 { t.event.completed_at } else { h.event.completed_at };
                    *h = t;
                    h.event.completed_at = at;
                }
                None => history.push(t),
            }
        }
        history.retain(|t| t.event.completed_at > since);
        history.sort_by_key(|t| std::cmp::Reverse(t.event.completed_at));
        history.truncate(MOST_DONE);
        let count = history.len();
        self.set_cal_pref(TASKS_DONE, &serde_json::to_string(&history)?)?;
        Ok(count)
    }

    /// The list new tasks go to when none is chosen (its href), where it
    /// is still there.
    pub fn default_task_list(&self) -> Option<String> {
        let chosen = self.store.preference(DEFAULT_TASK_LIST).filter(|l| !l.is_empty())?;
        pim_sources::outside(Pim::Calendar, || self.task_list_choices())
            .iter()
            .any(|l| l.href == chosen)
            .then_some(chosen)
    }
    /// Makes list `href` the one new tasks go to (empty: as before, where
    /// new events go).
    pub fn set_default_task_list(&self, href: &str) -> Result<()> {
        self.store.set_preference(DEFAULT_TASK_LIST, href)
    }

    /// Keeps a task just done in the Completed history.
    pub(super) fn remember_done(&self, mut done: Stored) -> Result<()> {
        let now = chrono::Utc::now().timestamp();
        if done.event.completed_at == 0 {
            done.event.completed_at = now;
        }
        let mut history = self.own_done();
        history.retain(|t| t.event.uid != done.event.uid);
        history.insert(0, done);
        history.retain(|t| t.event.completed_at > now - DONE_FOR);
        history.truncate(MOST_DONE);
        self.set_cal_pref(TASKS_DONE, &serde_json::to_string(&history)?)
    }
    fn own_done(&self) -> Vec<Stored> {
        self.cal_pref(TASKS_DONE).and_then(|j| serde_json::from_str(&j).ok()).unwrap_or_default()
    }
    /// The tasks done lately, newest first; outside any source, every
    /// source's.
    pub fn completed_tasks(&self) -> Vec<Stored> {
        if !within_one() {
            let mut all: Vec<Stored> = self
                .calendar_source_ids()
                .into_iter()
                .flat_map(|id| self.in_calendar_source(&id, || self.completed_tasks()))
                .collect();
            all.sort_by_key(|t| std::cmp::Reverse(t.event.completed_at));
            return all;
        }
        self.own_done()
    }
    /// Opens a done task (from the Completed history) again, on its server.
    pub fn reopen_task(&self, uid: &str) -> Result<Stored> {
        if !within_one() {
            let id = self
                .calendar_source_ids()
                .into_iter()
                .find(|id| self.in_calendar_source(id, || self.own_done().iter().any(|t| t.event.uid == uid)))
                .with_context(|| t!("manage-calendar-this-task-is-no-longer-on"))?;
            return self.in_calendar_source(&id, || self.reopen_task(uid));
        }
        let server = self
            .calendar_server()
            .with_context(|| t!("manage-calendar-no-calendar-server-is-set-up"))?;
        let mut history = self.own_done();
        let index = history
            .iter()
            .position(|t| t.event.uid == uid)
            .with_context(|| t!("manage-calendar-this-task-is-no-longer-on"))?;
        let mut task = history.remove(index);
        task.event.completed = false;
        task.event.completed_at = 0;
        task.event.status = "NEEDS-ACTION".into();
        task.event.sequence += 1;
        let calendar = task.calendar.clone();
        let saved = self.calendar_save(&server, &calendar, &task)?;
        let mut tasks = self.calendar_tasks();
        tasks.retain(|t| t.event.uid != saved.event.uid);
        tasks.push(saved.clone());
        caldav::sort_tasks(&mut tasks);
        self.set_cal_pref(TASKS, &serde_json::to_string(&tasks)?)?;
        self.set_cal_pref(TASKS_DONE, &serde_json::to_string(&history)?)?;
        Ok(saved)
    }
    /// Empties the Completed history (the tasks stay done on their servers).
    pub fn clear_completed_tasks(&self) -> Result<()> {
        if !within_one() {
            for id in self.calendar_source_ids() {
                self.in_calendar_source(&id, || self.clear_completed_tasks())?;
            }
            return Ok(());
        }
        if self.cal_pref(TASKS_DONE).is_some_and(|d| !d.is_empty() && d != "[]") {
            self.set_cal_pref(TASKS_DONE, "[]")?;
        }
        Ok(())
    }

    /// The mail accounts whose task service can be added: Outlook on
    /// Microsoft Graph (To Do) and Google (Google Tasks), not set up already.
    pub fn task_service_accounts(&self) -> Vec<Account> {
        let taken: Vec<String> = self
            .calendar_sources()
            .into_iter()
            .filter(|(_, s, _)| s.tasks_only)
            .filter_map(|(_, s, _)| s.account().map(str::to_owned))
            .collect();
        self.store
            .accounts()
            .unwrap_or_default()
            .into_iter()
            .filter(|a| matches!(MailProvider::of(a), Some(MailProvider::Graph | MailProvider::Google)) && a.oauth)
            .filter(|a| !taken.contains(&a.id))
            .collect()
    }
    /// Adds mail account `a`'s task service as a source of tasks: Microsoft
    /// To Do for Outlook on Microsoft Graph, Google Tasks for Google. Asks
    /// for access in the browser first, where it wasn't given (`cancel`
    /// stops that). Returns how many tasks are to do there.
    pub fn connect_task_service(&self, a: &Account, cancel: Arc<AtomicBool>) -> Result<usize> {
        let server = match MailProvider::of(a) {
            Some(MailProvider::Graph) => {
                self.grant_graph(a, crate::sync::oauth::GRAPH_TASKS, cancel)?;
                caldav::Server {
                    url: crate::sync::graph::BASE.to_owned(),
                    user: a.email.clone(),
                    graph_account: a.id.clone(),
                    tasks_only: true,
                    label: t!("manage-tasks-microsoft-to-do", email = a.email),
                    ..Default::default()
                }
            }
            Some(MailProvider::Google) => {
                self.grant_google(a, crate::sync::oauth::GOOGLE_TASKS, cancel)?;
                caldav::Server {
                    url: crate::sync::google::CALENDAR_BASE.to_owned(),
                    user: a.email.clone(),
                    google_account: a.id.clone(),
                    tasks_only: true,
                    label: t!("manage-tasks-google-tasks", email = a.email),
                    ..Default::default()
                }
            }
            _ => bail!(t!("manage-tasks-only-outlook-and-google")),
        };
        self.add_calendar_source(|| {
            self.start_source(&server)?;
            self.sync_remote_tasks(&server)
        })
    }
    /// Microsoft Graph: asks, in the browser, for `scope` on top of what the
    /// account was given, and keeps the sign-in that covers it all.
    fn grant_graph(&self, a: &Account, scope: &str, cancel: Arc<AtomicBool>) -> Result<()> {
        ensure!(a.graph() && a.oauth, t!("manage-tasks-only-outlook-and-google"));
        let secret = self.credentials(a)?;
        let mut asked: Vec<&str> = secret.extra_scopes.split_whitespace().collect();
        if asked.contains(&scope) {
            return Ok(());
        }
        asked.push(scope);
        let grant = crate::sync::oauth::authorize_with(a, &asked, cancel)?;
        let (secret, _) = crate::sync::oauth::redeem(a, &grant)?;
        self.save_credentials(&a.id, &secret)?;
        self.graph.lock().unwrap().remove(&a.id);
        Ok(())
    }

    /// Reads a source of tasks alone (To Do, Google Tasks): its lists, and
    /// the tasks still to do in them.
    pub(super) fn sync_remote_tasks(&self, server: &caldav::Server) -> Result<usize> {
        let now = chrono::Utc::now().timestamp();
        let (prefix, lists, found): (&str, Vec<RemoteList>, Vec<RemoteTask>) = if let Some(a) = self.calendar_graph(server)? {
            let (lists, found) = self.with_graph_calendars(&a, |s| {
                let lists = s.todo_lists()?;
                let found = s.todo_tasks(&lists)?;
                Ok((lists, found))
            })?;
            ("todo", lists, found)
        } else if let Some(a) = self.calendar_google(server)? {
            let (lists, found) = self.with_google(&a, |s| {
                let lists = s.task_lists()?;
                let found = s.tasks(&lists)?;
                Ok((lists, found))
            })?;
            ("gtasks", lists, found)
        } else {
            bail!(t!("manage-tasks-only-outlook-and-google"));
        };
        if let Some(first) = lists.iter().find(|l| l.default && l.writable).or(lists.iter().find(|l| l.writable))
            && self.cal_pref(DEFAULT).is_none_or(|d| d.is_empty())
        {
            self.set_cal_pref(DEFAULT, &format!("{prefix}:{}", first.id))?;
        }
        let lists: Vec<caldav::Calendar> = lists
            .into_iter()
            .map(|l| caldav::Calendar {
                href: format!("{prefix}:{}", l.id),
                name: l.name,
                writable: l.writable,
                components: vec!["VTODO".into()],
                ..Default::default()
            })
            .collect();
        let mut tasks = remote_stored(prefix, found);
        self.merge_kept(&mut tasks);
        // What's kept of tasks gone (done elsewhere and cleared, deleted)
        // goes, but not of those in the history, which can come back.
        let mut kept = self.kept();
        let done = self.own_done();
        let before = kept.len();
        kept.retain(|uid, _| tasks.iter().chain(&done).any(|t| &t.event.uid == uid));
        if kept.len() != before {
            self.set_cal_pref(TASK_EXTRAS, &serde_json::to_string(&kept)?)?;
        }
        caldav::sort_tasks(&mut tasks);
        let count = tasks.len();
        self.set_cal_pref(TASK_LISTS, &serde_json::to_string(&lists)?)?;
        self.set_cal_pref(TASKS, &serde_json::to_string(&tasks)?)?;
        self.set_cal_pref(SYNCED, &now.to_string())?;
        Ok(count)
    }
    /// Saves a task in a source of tasks alone, in list `calendar`. Its UID
    /// becomes its id there (`sync::tasks`).
    pub(super) fn save_remote_task(&self, server: &caldav::Server, calendar: &str, stored: &Stored) -> Result<Stored> {
        ensure!(stored.event.task, t!("manage-tasks-this-account-keeps-only-tasks"));
        let before = self.calendar_tasks().into_iter().find(|t| !stored.href.is_empty() && t.href == stored.href);
        let (prefix, list) = calendar.split_once(':').with_context(|| t!("manage-tasks-not-a-task-list"))?;
        let id = stored.href.split_once(':').map(|(_, id)| id).filter(|i| !i.is_empty());
        let (id, etag) = match (prefix, self.calendar_graph(server)?, self.calendar_google(server)?) {
            ("todo", Some(a), _) => {
                let link_added = !stored.event.url.is_empty() && before.is_some_and(|b| b.event.url != stored.event.url);
                self.with_graph_calendars(&a, |s| s.save_todo_task(list, id, &stored.event, link_added))?
            }
            ("gtasks", _, Some(a)) => self.with_google(&a, |s| s.save_task(list, id, &stored.event))?,
            _ => bail!(t!("manage-tasks-not-a-task-list")),
        };
        ensure!(!id.is_empty(), t!("manage-tasks-the-service-didnt-say-which-task"));
        let saved = Stored {
            event: Event { uid: id.clone(), ..stored.event.clone() },
            calendar: calendar.to_owned(),
            href: format!("{prefix}:{id}"),
            etag,
            ..stored.clone()
        };
        self.keep_extras(&saved)?;
        Ok(saved)
    }
    /// Removes a task from a source of tasks alone.
    pub(super) fn delete_remote_task(&self, server: &caldav::Server, stored: &Stored) -> Result<()> {
        let Some((prefix, id)) = stored.href.split_once(':').filter(|(_, id)| !id.is_empty()) else {
            return Ok(());
        };
        let list = stored.calendar.split_once(':').map(|(_, l)| l).unwrap_or_default();
        match (prefix, self.calendar_graph(server)?, self.calendar_google(server)?) {
            ("todo", Some(a), _) => self.with_graph_calendars(&a, |s| s.delete_todo_task(list, id))?,
            ("gtasks", _, Some(a)) => self.with_google(&a, |s| s.delete_task(list, id))?,
            _ => bail!(t!("manage-tasks-not-a-task-list")),
        }
        let mut kept = self.kept();
        if kept.remove(&stored.event.uid).is_some() {
            self.set_cal_pref(TASK_EXTRAS, &serde_json::to_string(&kept)?)?;
        }
        Ok(())
    }

    /// What's kept here of a source's tasks that its service doesn't keep.
    fn kept(&self) -> std::collections::BTreeMap<String, Kept> {
        self.cal_pref(TASK_EXTRAS).and_then(|j| serde_json::from_str(&j).ok()).unwrap_or_default()
    }
    /// Keeps what a task service doesn't of task `t`, just saved there.
    fn keep_extras(&self, t: &Stored) -> Result<()> {
        let Some(keeps) = Keeps::of(&t.href) else { return Ok(()) };
        let mut kept = self.kept();
        let mine = Kept::of(&t.event, &keeps);
        let changed = if mine == Kept::default() { kept.remove(&t.event.uid).is_some() } else { kept.insert(t.event.uid.clone(), mine.clone()) != Some(mine) };
        if changed {
            self.set_cal_pref(TASK_EXTRAS, &serde_json::to_string(&kept)?)?;
        }
        Ok(())
    }
    /// Puts back on tasks read from a service what's kept here of them.
    fn merge_kept(&self, tasks: &mut [Stored]) {
        let kept = self.kept();
        for t in tasks {
            if let (Some(keeps), Some(k)) = (Keeps::of(&t.href), kept.get(&t.event.uid)) {
                k.merge(&mut t.event, &keeps);
            }
        }
    }
}

/// Tasks read from a task service as kept: `prefix:` lists and hrefs.
fn remote_stored(prefix: &str, found: Vec<RemoteTask>) -> Vec<Stored> {
    found
        .into_iter()
        .map(|t| Stored {
            event: t.event,
            calendar: format!("{prefix}:{}", t.list),
            href: format!("{prefix}:{}", t.id),
            etag: t.etag,
            ..Default::default()
        })
        .collect()
}

/// What a task service keeps of a task itself (by its href); `None` for
/// servers that keep it all (CalDAV, JMAP).
#[derive(Clone, Copy, Debug, Default)]
struct Keeps {
    /// Exactly (`false`: To Do's three steps, or none at all).
    priority: bool,
    /// Some priority: To Do's high, normal or low.
    some_priority: bool,
    tags: bool,
    due: bool,
    due_time: bool,
    notes: bool,
    url: bool,
}

impl Keeps {
    fn of(href: &str) -> Option<Self> {
        let (prefix, id) = href.split_once(':')?;
        match prefix {
            // Google: a due day, notes (and the link on them).
            "gtasks" => Some(Self { due: true, notes: true, url: true, ..Default::default() }),
            // A To Do step: its name alone.
            "todo" if id.contains(crate::sync::graph::todo::STEP) => Some(Self::default()),
            "todo" => Some(Self { some_priority: true, tags: true, due: true, notes: true, url: true, ..Default::default() }),
            _ => None,
        }
    }
}

/// What's kept here of a task that its service doesn't keep.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct Kept {
    #[serde(default, skip_serializing_if = "is_zero")]
    priority: u8,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    tags: Vec<String>,
    /// A due time (or date, where the service keeps none).
    #[serde(default, skip_serializing_if = "is_zero_i64")]
    due: i64,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    all_day: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    notes: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    url: String,
}

fn is_zero(n: &u8) -> bool {
    *n == 0
}
fn is_zero_i64(n: &i64) -> bool {
    *n == 0
}

impl Kept {
    /// What `keeps` leaves out of task `e`.
    fn of(e: &Event, keeps: &Keeps) -> Self {
        Self {
            priority: if keeps.priority { 0 } else { e.priority },
            tags: if keeps.tags { Vec::new() } else { e.categories.clone() },
            due: match (keeps.due, keeps.due_time) {
                (false, _) => e.start,
                (true, false) if !e.all_day => e.start,
                _ => 0,
            },
            all_day: !keeps.due && e.all_day,
            notes: if keeps.notes { String::new() } else { e.description.clone() },
            url: if keeps.url { String::new() } else { e.url.clone() },
        }
    }
    /// Puts it back on task `e` as read from the service, where the
    /// service still says what it said: a due time only on the day the
    /// service has it due, a priority only where it's To Do's same step.
    fn merge(&self, e: &mut Event, keeps: &Keeps) {
        use crate::sync::tasks::importance;
        if !keeps.priority
            && self.priority != 0
            && (!keeps.some_priority || importance(self.priority) == importance(e.priority))
        {
            e.priority = self.priority;
        }
        if !keeps.tags && e.categories.is_empty() {
            e.categories = self.tags.clone();
        }
        if !keeps.notes && e.description.is_empty() {
            e.description = self.notes.clone();
        }
        if !keeps.url && e.url.is_empty() {
            e.url = self.url.clone();
        }
        if !keeps.due {
            if self.due != 0 {
                e.start = self.due;
                e.end = self.due;
                e.all_day = self.all_day;
            }
        } else if !keeps.due_time && self.due != 0 && e.start != 0 && e.all_day {
            let day = chrono::DateTime::from_timestamp(self.due, 0).map(|d| d.with_timezone(&chrono::Local).date_naive());
            let due = chrono::DateTime::from_timestamp(e.start, 0).map(|d| d.date_naive());
            if day.is_some() && day == due {
                e.start = self.due;
                e.end = self.due;
                e.all_day = false;
            }
        }
    }
}

/// A draft's fields on a task.
fn apply(e: &mut Event, d: &TaskDraft) {
    e.summary = d.summary.trim().to_owned();
    e.description = d.notes.trim().to_owned();
    e.start = d.due;
    e.end = d.due;
    e.all_day = d.all_day && d.due != 0;
    e.priority = d.priority.min(9);
    e.categories = d.tags.clone();
    e.parent = d.parent.clone();
    e.url = d.url.trim().to_owned();
}

#[cfg(test)]
#[path = "tasks_tests.rs"]
mod tests;

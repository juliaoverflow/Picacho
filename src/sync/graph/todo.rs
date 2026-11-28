//! Microsoft To Do through Microsoft Graph (`/me/todo`), for an Outlook
//! account that uses Graph and was given To Do access (`Tasks.ReadWrite`).
//!
//! A task's importance is its priority (high 1, low 9), its categories its
//! tags, and the email it was made from a linked resource of Picacho's
//! (`linkedResources`, the email's `mid:` link as its `externalId`, as
//! Outlook links its own flagged emails). To Do keeps due dates as days.
//! A task's steps (`checklistItems`) are its subtasks here, as steps keep
//! only a name and whether they're checked; a subtask made here is a step.
use super::Session;
use crate::sync::ical::Event;
use crate::sync::rest::escape;
use crate::sync::tasks::{RemoteList, RemoteTask, due_day, due_of, importance, priority_of};
use anyhow::Result;
use serde_json::{Value, json};

/// What Picacho calls itself on the links it adds.
const APP: &str = "Picacho";
/// The most tasks read from one list.
const MOST: usize = 5000;

fn text(v: &Value) -> String {
    v.as_str().unwrap_or_default().to_owned()
}

/// A To Do task as a task.
pub fn task_of(v: &Value) -> Event {
    let due = v["dueDateTime"]["dateTime"].as_str().and_then(due_of).unwrap_or(0);
    let status = v["status"].as_str().unwrap_or("notStarted");
    let completed_at = v["completedDateTime"]["dateTime"]
        .as_str()
        .and_then(|d| chrono::NaiveDateTime::parse_from_str(d.split('.').next().unwrap_or(d), "%Y-%m-%dT%H:%M:%S").ok())
        .map_or(0, |d| d.and_utc().timestamp());
    // Picacho's own link first, else the first with an address.
    let links = v["linkedResources"].as_array().cloned().unwrap_or_default();
    let url = links
        .iter()
        .find(|l| l["applicationName"].as_str() == Some(APP))
        .map(|l| text(&l["externalId"]))
        .filter(|u| !u.is_empty())
        .or_else(|| links.iter().map(|l| text(&l["webUrl"])).find(|u| !u.is_empty()))
        .unwrap_or_default();
    let reminder = v["reminderDateTime"]["dateTime"]
        .as_str()
        .filter(|_| v["isReminderOn"].as_bool() == Some(true))
        .and_then(|d| chrono::NaiveDateTime::parse_from_str(d.split('.').next().unwrap_or(d), "%Y-%m-%dT%H:%M:%S").ok())
        .map(|d| d.and_utc().timestamp());
    Event {
        uid: text(&v["id"]),
        summary: text(&v["title"]),
        description: text(&v["body"]["content"]).trim().to_owned(),
        start: due,
        end: due,
        all_day: due != 0,
        task: true,
        completed: status == "completed",
        completed_at,
        status: match status {
            "inProgress" => "IN-PROCESS",
            "completed" => "COMPLETED",
            _ => "NEEDS-ACTION",
        }
        .to_owned(),
        priority: priority_of(v["importance"].as_str().unwrap_or("normal")),
        categories: v["categories"].as_array().into_iter().flatten().map(text).filter(|c| !c.is_empty()).collect(),
        url,
        alarms: match (reminder, due) {
            (Some(at), due) if due != 0 => vec![due - at],
            _ => Vec::new(),
        },
        utc: true,
        ..Default::default()
    }
}

/// A task as To Do's JSON: what Picacho edits. `new`: with its link.
pub fn task_json(e: &Event, new: bool) -> Value {
    let mut v = json!({
        "title": e.summary,
        "body": {"content": e.description, "contentType": "text"},
        "importance": importance(e.priority),
        "status": if e.completed { "completed" } else if e.status == "IN-PROCESS" { "inProgress" } else { "notStarted" },
        "categories": e.categories,
    });
    v["dueDateTime"] = match due_day(e) {
        Some(day) => json!({"dateTime": format!("{day}T00:00:00"), "timeZone": "UTC"}),
        None => Value::Null,
    };
    if new && !e.url.is_empty() {
        v["linkedResources"] = json!([link_json(e)]);
    }
    v
}

fn link_json(e: &Event) -> Value {
    let mut link = json!({"applicationName": APP, "displayName": e.summary, "externalId": e.url});
    if e.url.starts_with("https://") || e.url.starts_with("http://") {
        link["webUrl"] = e.url.clone().into();
    }
    link
}

/// Between a task's id and one of its steps' in a step's id here: To Do's
/// ids are URL-safe base64, which never has it.
pub const STEP: char = '#';

/// One of a task's steps (`checklistItem`) as a subtask of it: its UID is
/// `task#step`, and To Do keeps only its name and whether it's checked.
pub fn step_of(task: &str, v: &Value) -> Event {
    let checked = v["isChecked"].as_bool() == Some(true);
    Event {
        uid: format!("{task}{STEP}{}", text(&v["id"])),
        summary: text(&v["displayName"]),
        task: true,
        completed: checked,
        completed_at: v["checkedDateTime"]
            .as_str()
            .and_then(|d| chrono::DateTime::parse_from_rfc3339(d).ok())
            .map_or(0, |d| d.timestamp()),
        status: if checked { "COMPLETED" } else { "NEEDS-ACTION" }.to_owned(),
        parent: task.to_owned(),
        utc: true,
        ..Default::default()
    }
}

/// A task's id, and its step's where it is one (`task#step`).
pub fn split_step(id: &str) -> (&str, Option<&str>) {
    match id.split_once(STEP) {
        Some((task, step)) => (task, Some(step)),
        None => (id, None),
    }
}

impl Session {
    /// The task lists: the default one first. Lists Outlook keeps of its
    /// own (flagged emails) can't take new tasks.
    pub fn todo_lists(&self) -> Result<Vec<RemoteList>> {
        let mut lists: Vec<RemoteList> = self
            .all_pages("/me/todo/lists?$top=100", 1000)?
            .iter()
            .map(|l| {
                let kind = l["wellknownListName"].as_str().unwrap_or("none");
                RemoteList {
                    id: text(&l["id"]),
                    name: text(&l["displayName"]),
                    writable: kind != "flaggedEmails",
                    default: kind == "defaultList",
                }
            })
            .filter(|l| !l.id.is_empty())
            .collect();
        lists.sort_by_key(|l| !l.default);
        Ok(lists)
    }
    /// `list`'s tasks as `filter` picks them, with their links and steps.
    fn listed(&self, list: &str, filter: &str, most: usize) -> Result<Vec<Value>> {
        let path = format!(
            "/me/todo/lists/{}/tasks?$top=100&$filter={}&$expand=linkedResources,checklistItems",
            escape(list),
            escape(filter)
        );
        self.all_pages(&path, most)
    }
    /// The tasks still to do in `lists`, and their steps not yet checked,
    /// as subtasks.
    pub fn todo_tasks(&self, lists: &[RemoteList]) -> Result<Vec<RemoteTask>> {
        let mut out = Vec::new();
        for l in lists {
            for v in self.listed(&l.id, "status ne 'completed'", MOST)? {
                let event = task_of(&v);
                if event.uid.is_empty() || event.completed {
                    continue;
                }
                for step in v["checklistItems"].as_array().into_iter().flatten() {
                    let step = step_of(&event.uid, step);
                    if !step.completed && !step.summary.is_empty() {
                        out.push(RemoteTask { list: l.id.clone(), id: step.uid.clone(), etag: String::new(), event: step });
                    }
                }
                out.push(RemoteTask {
                    list: l.id.clone(),
                    id: event.uid.clone(),
                    etag: text(&v["@odata.etag"]),
                    event,
                });
            }
        }
        Ok(out)
    }
    /// What was done in `lists` since `since`: tasks completed, and steps
    /// checked (of tasks done or not).
    pub fn todo_completed(&self, lists: &[RemoteList], since: i64) -> Result<Vec<RemoteTask>> {
        let mut out = Vec::new();
        for l in lists {
            // The most recent first isn't something To Do sorts by: the
            // finished ones are read, up to a bound, and the recent kept.
            let mut listed = self.listed(&l.id, "status eq 'completed'", 2000)?;
            listed.extend(self.listed(&l.id, "status ne 'completed'", MOST)?);
            for v in listed {
                let event = task_of(&v);
                if event.uid.is_empty() {
                    continue;
                }
                for step in v["checklistItems"].as_array().into_iter().flatten() {
                    let step = step_of(&event.uid, step);
                    if step.completed && step.completed_at >= since {
                        out.push(RemoteTask { list: l.id.clone(), id: step.uid.clone(), etag: String::new(), event: step });
                    }
                }
                if event.completed && event.completed_at >= since {
                    out.push(RemoteTask { list: l.id.clone(), id: event.uid.clone(), etag: text(&v["@odata.etag"]), event });
                }
            }
        }
        Ok(out)
    }
    /// Adds a task to `list` (`id` none), or changes it; returns its id and
    /// version. A link given to a task made before is added to it. A
    /// subtask is one of its parent's steps: only its name and whether
    /// it's done are To Do's.
    pub fn save_todo_task(&self, list: &str, id: Option<&str>, e: &Event, link_added: bool) -> Result<(String, String)> {
        let tasks = format!("/me/todo/lists/{}/tasks", escape(list));
        let step_json = || json!({"displayName": e.summary, "isChecked": e.completed});
        match id.map(split_step) {
            Some((task, Some(step))) => {
                let path = Self::p(&format!("{tasks}/{}/checklistItems/{}", escape(task), escape(step)));
                self.rest.patch(&path, &step_json())?;
                return Ok((format!("{task}{STEP}{step}"), String::new()));
            }
            None if !e.parent.is_empty() => {
                anyhow::ensure!(!e.parent.contains(STEP), t!("sync-todo-steps-have-no-steps"));
                let path = Self::p(&format!("{tasks}/{}/checklistItems", escape(&e.parent)));
                let saved = self.rest.post(&path, &step_json())?;
                return Ok((format!("{}{STEP}{}", e.parent, text(&saved["id"])), String::new()));
            }
            _ => {}
        }
        let saved = match id {
            Some(id) => {
                let path = Self::p(&format!("{tasks}/{}", escape(id)));
                let saved = self.rest.patch(&path, &task_json(e, false))?;
                if link_added && !e.url.is_empty() {
                    self.rest.post(&format!("{path}/linkedResources"), &link_json(e))?;
                }
                saved
            }
            None => self.rest.post(&Self::p(&tasks), &task_json(e, true))?,
        };
        let id = saved["id"].as_str().map(str::to_owned).or_else(|| id.map(str::to_owned)).unwrap_or_default();
        Ok((id, text(&saved["@odata.etag"])))
    }
    /// Removes a task (with its steps), or a step.
    pub fn delete_todo_task(&self, list: &str, id: &str) -> Result<()> {
        let path = match split_step(id) {
            (task, Some(step)) => format!("/me/todo/lists/{}/tasks/{}/checklistItems/{}", escape(list), escape(task), escape(step)),
            (task, None) => format!("/me/todo/lists/{}/tasks/{}", escape(list), escape(task)),
        };
        match self.rest.delete(&Self::p(&path)) {
            Err(e) if crate::sync::rest::not_found(&e) => Ok(()),
            other => other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::rest::test_server::{json as answer, start};
    use std::sync::{Arc, Mutex};

    /// To Do's JSON, as Microsoft documents it, read as a task and written
    /// back: importance is priority, the day due, Picacho's link.
    #[test]
    fn to_do_tasks_are_read_and_written() {
        let v = json!({
            "id": "AAMk1", "title": "Pay rent", "importance": "high", "status": "inProgress",
            "body": {"content": "By transfer\r\n", "contentType": "text"},
            "dueDateTime": {"dateTime": "2026-11-30T00:00:00.0000000", "timeZone": "UTC"},
            "isReminderOn": true, "reminderDateTime": {"dateTime": "2026-11-29T09:00:00.0000000", "timeZone": "UTC"},
            "categories": ["Home"],
            "linkedResources": [
                {"webUrl": "https://outlook.office.com/x", "applicationName": "Outlook"},
                {"applicationName": "Picacho", "externalId": "mid:rent@example.org"},
            ],
        });
        let e = task_of(&v);
        assert_eq!((e.uid.as_str(), e.summary.as_str(), e.description.as_str()), ("AAMk1", "Pay rent", "By transfer"));
        assert!(e.task && e.all_day && !e.completed);
        assert_eq!(e.start, crate::sync::tasks::due_of("2026-11-30").unwrap());
        assert_eq!((e.priority, e.status.as_str()), (1, "IN-PROCESS"));
        assert_eq!(e.categories, ["Home"]);
        assert_eq!(e.url, "mid:rent@example.org");
        assert_eq!(e.alarms, [15 * 3600]);
        let back = task_json(&e, true);
        assert_eq!(back["importance"], "high");
        assert_eq!(back["status"], "inProgress");
        assert_eq!(back["dueDateTime"], json!({"dateTime": "2026-11-30T00:00:00", "timeZone": "UTC"}));
        assert_eq!(back["linkedResources"][0]["externalId"], "mid:rent@example.org");
        assert!(back["linkedResources"][0].get("webUrl").is_none(), "a mid: link isn't a web address");
        let done = task_json(&Event { completed: true, start: 0, ..e.clone() }, false);
        assert_eq!((&done["status"], &done["dueDateTime"]), (&json!("completed"), &Value::Null));
        assert!(done.get("linkedResources").is_none());
    }

    /// Lists, the default first; only tasks to do are asked for; a task
    /// made, changed (with a link added) and deleted.
    #[test]
    fn to_do_session() {
        let made: Arc<Mutex<Vec<(String, String, Value)>>> = Arc::default();
        let kept = made.clone();
        let server = start(move |r| {
            let path = r.path.clone();
            kept.lock().unwrap().push((r.method.clone(), path.clone(), r.json()));
            match (r.method.as_str(), path.split('?').next().unwrap_or_default()) {
                ("GET", "/v1.0/me/todo/lists") => answer(200, json!({"value": [
                    {"id": "flag", "displayName": "Flagged email", "wellknownListName": "flaggedEmails"},
                    {"id": "work", "displayName": "Work", "wellknownListName": "none"},
                    {"id": "main", "displayName": "Tasks", "wellknownListName": "defaultList"},
                ]})),
                ("GET", p) if p.ends_with("/tasks") => {
                    assert!(path.contains("status%20ne%20%27completed%27"), "{path}");
                    if p.contains("/main/") {
                        answer(200, json!({"value": [{"id": "t1", "title": "One", "status": "notStarted", "@odata.etag": "W/\"1\""}]}))
                    } else {
                        answer(200, json!({"value": []}))
                    }
                }
                ("POST", "/v1.0/me/todo/lists/main/tasks") => answer(201, json!({"id": "t2", "@odata.etag": "W/\"2\""})),
                ("PATCH", "/v1.0/me/todo/lists/main/tasks/t2") => answer(200, json!({"id": "t2", "@odata.etag": "W/\"3\""})),
                ("POST", "/v1.0/me/todo/lists/main/tasks/t2/linkedResources") => answer(201, json!({"id": "l1"})),
                ("DELETE", "/v1.0/me/todo/lists/main/tasks/t2") => (204, String::new(), Vec::new()),
                ("DELETE", _) => answer(404, json!({"error": {"code": "ErrorItemNotFound"}})),
                _ => answer(404, json!({"error": {"message": "no such thing"}})),
            }
        });
        let s = Session::open(&server.base, "token", native_tls::TlsConnector::new().unwrap()).unwrap();
        let lists = s.todo_lists().unwrap();
        assert_eq!(lists.iter().map(|l| l.id.as_str()).collect::<Vec<_>>(), ["main", "flag", "work"]);
        assert!(lists[0].default && !lists[1].writable);
        let tasks = s.todo_tasks(&lists).unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!((tasks[0].list.as_str(), tasks[0].id.as_str(), tasks[0].etag.as_str()), ("main", "t1", "W/\"1\""));
        let new = Event { summary: "Two".into(), task: true, ..Default::default() };
        assert_eq!(s.save_todo_task("main", None, &new, false).unwrap(), ("t2".into(), "W/\"2\"".into()));
        let linked = Event { url: "mid:m@x".into(), ..new.clone() };
        assert_eq!(s.save_todo_task("main", Some("t2"), &linked, true).unwrap().1, "W/\"3\"");
        s.delete_todo_task("main", "t2").unwrap();
        s.delete_todo_task("main", "gone").unwrap();
        let seen = made.lock().unwrap();
        let link = seen.iter().find(|(m, p, _)| m == "POST" && p.ends_with("/linkedResources")).unwrap();
        assert_eq!(link.2["externalId"], "mid:m@x");
    }
}

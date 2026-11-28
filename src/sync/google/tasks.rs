//! Google Tasks (`tasks/v1`), for a Google account that was given Tasks
//! access (`auth/tasks`), with the same sign-in as its calendars.
//!
//! Google keeps a title, notes, a due day, whether it's done and which task
//! a subtask is under (`parent`, by id). Its links are Google's own (Gmail's
//! "Add to Tasks"), so the email a task was made from here rides on the
//! notes' last line (`sync::tasks::notes_with_link`). It has no priority or
//! tags.
use super::{Session, text};
use crate::sync::ical::Event;
use crate::sync::rest::escape;
use crate::sync::tasks::{RemoteList, RemoteTask, due_day, due_of, link_from_notes, notes_with_link};
use anyhow::Result;
use serde_json::{Value, json};

const TASKS: &str = "/tasks/v1";

/// A Google task as a task.
pub fn task_of(v: &Value) -> Event {
    let due = v["due"].as_str().and_then(due_of).unwrap_or(0);
    let (notes, mut url) = link_from_notes(&text(&v["notes"]));
    if url.is_empty() {
        url = v["links"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|l| text(&l["link"]))
            .find(|l| !l.is_empty())
            .unwrap_or_default();
    }
    let completed = v["status"].as_str() == Some("completed");
    Event {
        uid: text(&v["id"]),
        summary: text(&v["title"]),
        description: notes,
        start: due,
        end: due,
        all_day: due != 0,
        task: true,
        completed,
        completed_at: v["completed"]
            .as_str()
            .and_then(|c| chrono::DateTime::parse_from_rfc3339(c).ok())
            .map_or(0, |c| c.timestamp()),
        status: if completed { "COMPLETED" } else { "NEEDS-ACTION" }.to_owned(),
        parent: text(&v["parent"]),
        url,
        utc: true,
        ..Default::default()
    }
}

/// A task as Google's JSON: what Picacho edits.
pub fn task_json(e: &Event) -> Value {
    json!({
        "title": e.summary,
        "notes": notes_with_link(e),
        "status": if e.completed { "completed" } else { "needsAction" },
        // Google reads only the day (RFC 3339, at midnight UTC).
        "due": due_day(e).map(|d| format!("{d}T00:00:00.000Z")),
    })
}

impl Session {
    /// The task lists, the default one (Google's first) first.
    pub fn task_lists(&self) -> Result<Vec<RemoteList>> {
        let items = Self::pages(&self.calendar, &format!("{TASKS}/users/@me/lists?maxResults=100"), "items")?;
        Ok(items
            .iter()
            .enumerate()
            .map(|(i, l)| RemoteList { id: text(&l["id"]), name: text(&l["title"]), writable: true, default: i == 0 })
            .filter(|l| !l.id.is_empty())
            .collect())
    }
    /// The tasks still to do in `lists`, subtasks too.
    pub fn tasks(&self, lists: &[RemoteList]) -> Result<Vec<RemoteTask>> {
        let mut out = Vec::new();
        for l in lists {
            let path = format!("{TASKS}/lists/{}/tasks?showCompleted=false&showHidden=false&maxResults=100", escape(&l.id));
            for v in Self::pages(&self.calendar, &path, "items")? {
                let event = task_of(&v);
                if event.uid.is_empty() || event.completed || v["deleted"].as_bool() == Some(true) {
                    continue;
                }
                out.push(RemoteTask { list: l.id.clone(), id: event.uid.clone(), etag: text(&v["etag"]), event });
            }
        }
        Ok(out)
    }
    /// The tasks done in `lists` since `since` (hidden ones too: Google
    /// hides what was cleared away).
    pub fn completed_tasks(&self, lists: &[RemoteList], since: i64) -> Result<Vec<RemoteTask>> {
        let mut out = Vec::new();
        for l in lists {
            let path = format!(
                "{TASKS}/lists/{}/tasks?showCompleted=true&showHidden=true&completedMin={}&maxResults=100",
                escape(&l.id),
                escape(&super::rfc3339(since))
            );
            for v in Self::pages(&self.calendar, &path, "items")? {
                let event = task_of(&v);
                if event.uid.is_empty() || !event.completed || v["deleted"].as_bool() == Some(true) {
                    continue;
                }
                out.push(RemoteTask { list: l.id.clone(), id: event.uid.clone(), etag: text(&v["etag"]), event });
            }
        }
        Ok(out)
    }
    /// Adds a task to `list` (`id` none; under its `parent` where it has
    /// one), or changes it; returns its id and version.
    pub fn save_task(&self, list: &str, id: Option<&str>, e: &Event) -> Result<(String, String)> {
        let saved = match id {
            Some(id) => self.calendar.patch(&format!("{TASKS}/lists/{}/tasks/{}", escape(list), escape(id)), &task_json(e))?,
            None => {
                let parent = if e.parent.is_empty() { String::new() } else { format!("?parent={}", escape(&e.parent)) };
                self.calendar.post(&format!("{TASKS}/lists/{}/tasks{parent}", escape(list)), &task_json(e))?
            }
        };
        let id = saved["id"].as_str().map(str::to_owned).or_else(|| id.map(str::to_owned)).unwrap_or_default();
        Ok((id, text(&saved["etag"])))
    }
    pub fn delete_task(&self, list: &str, id: &str) -> Result<()> {
        match self.calendar.delete(&format!("{TASKS}/lists/{}/tasks/{}", escape(list), escape(id))) {
            Err(e) if crate::sync::rest::not_found(&e) || format!("{e:#}").contains("410") => Ok(()),
            other => other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::rest::test_server::{json as answer, start};

    /// Google's task JSON, as Google documents it: a subtask names its
    /// parent, the email link rides on the notes, a due date is a day.
    #[test]
    fn google_tasks_are_read_and_written() {
        let v = json!({
            "id": "g2", "title": "Forms", "status": "needsAction", "parent": "g1",
            "notes": "Two of them\n\nmid:taxes@example.org", "due": "2026-11-30T00:00:00.000Z", "etag": "\"e\"",
        });
        let e = task_of(&v);
        assert_eq!((e.uid.as_str(), e.parent.as_str(), e.description.as_str()), ("g2", "g1", "Two of them"));
        assert_eq!(e.url, "mid:taxes@example.org");
        assert!(e.all_day && !e.completed);
        let back = task_json(&e);
        assert_eq!(back["notes"], "Two of them\n\nmid:taxes@example.org");
        assert_eq!(back["due"], "2026-11-30T00:00:00.000Z");
        assert_eq!(back["status"], "needsAction");
        let done = task_json(&Event { completed: true, start: 0, ..e.clone() });
        assert_eq!((&done["status"], &done["due"]), (&json!("completed"), &Value::Null));
        // Gmail's own link, where the notes have none.
        let gmail = task_of(&json!({"id": "g3", "title": "Reply", "links": [{"type": "email", "link": "https://mail.google.com/x"}]}));
        assert_eq!(gmail.url, "https://mail.google.com/x");
    }

    /// Lists and the tasks to do; a subtask made under its parent.
    #[test]
    fn google_tasks_session() {
        let server = start(|r| {
            let path = r.path.split('?').next().unwrap_or_default().to_owned();
            match (r.method.as_str(), path.as_str()) {
                ("GET", "/tasks/v1/users/@me/lists") => answer(200, json!({"items": [{"id": "L1", "title": "My Tasks"}, {"id": "L2", "title": "Work"}]})),
                ("GET", "/tasks/v1/lists/L1/tasks") => {
                    assert!(r.path.contains("showCompleted=false"), "{}", r.path);
                    answer(200, json!({"items": [{"id": "g1", "title": "Taxes", "status": "needsAction", "etag": "\"1\""}, {"id": "gx", "title": "Old", "status": "completed"}]}))
                }
                ("GET", "/tasks/v1/lists/L2/tasks") => answer(200, json!({})),
                ("POST", "/tasks/v1/lists/L1/tasks") => {
                    assert!(r.path.ends_with("?parent=g1"), "{}", r.path);
                    let mut t = r.json();
                    t["id"] = "g2".into();
                    t["etag"] = "\"2\"".into();
                    answer(200, t)
                }
                ("DELETE", "/tasks/v1/lists/L1/tasks/g2") => (204, String::new(), Vec::new()),
                _ => answer(404, json!({"error": {"message": "no such thing"}})),
            }
        });
        let s = Session::open(&server.base, &server.base, "token", native_tls::TlsConnector::new().unwrap()).unwrap();
        let lists = s.task_lists().unwrap();
        assert_eq!((lists.len(), lists[0].default, lists[1].default), (2, true, false));
        let tasks = s.tasks(&lists).unwrap();
        assert_eq!(tasks.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(), ["g1"]);
        let sub = Event { summary: "Forms".into(), task: true, parent: "g1".into(), ..Default::default() };
        assert_eq!(s.save_task("L1", None, &sub).unwrap(), ("g2".into(), "\"2\"".into()));
        s.delete_task("L1", "g2").unwrap();
    }
}

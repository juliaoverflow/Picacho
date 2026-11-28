// The Tasks workspace's backend: a Google Tasks account added as a source
// of tasks alone (against a loopback Google Tasks, as Google documents
// it), tasks made from email, subtasks, lists, the Completed history, and
// where tasks stand for Today, Upcoming and Overdue.
use super::*;
use crate::sync::rest::test_server::{self, json as answer};
use serde_json::{Value, json};
use std::sync::Mutex as M;

fn backend() -> Result<(Backend, std::path::PathBuf)> {
    let path = std::env::temp_dir().join(format!("picacho-tasks-{}", uuid::Uuid::new_v4()));
    let mut b = Backend::new(Arc::new(Store::open(&path)?));
    b.test_secret = Some(Secret {
        password: "secret".into(),
        access_token: "token".into(),
        expires_at: chrono::Utc::now().timestamp() + 3600,
        ..Default::default()
    });
    Ok((b, path))
}

/// Google Tasks keeping tasks (each with its `list`) as they're made,
/// changed and deleted.
fn google_tasks(tasks: Arc<M<Vec<Value>>>) -> test_server::Server {
    test_server::start(move |r| {
        let path = r.path.split('?').next().unwrap_or_default().to_owned();
        let query = r.path.split_once('?').map(|(_, q)| q.to_owned()).unwrap_or_default();
        let mut kept = tasks.lock().unwrap();
        let parts: Vec<&str> = path.trim_start_matches("/tasks/v1/").split('/').collect();
        match (r.method.as_str(), parts.as_slice()) {
            ("GET", ["users", "@me", "lists"]) => answer(200, json!({"items": [{"id": "L1", "title": "My Tasks"}, {"id": "L2", "title": "Errands"}]})),
            ("GET", ["lists", list, "tasks"]) => {
                let open: Vec<Value> = kept
                    .iter()
                    .filter(|t| t["list"] == *list && (t["status"] != "completed" || !query.contains("showCompleted=false")))
                    .cloned()
                    .collect();
                answer(200, json!({ "items": open }))
            }
            ("POST", ["lists", list, "tasks"]) => {
                let mut t = r.json();
                t["id"] = format!("g{}", kept.len() + 1).into();
                t["etag"] = "\"1\"".into();
                t["list"] = (*list).into();
                if let Some(parent) = query.strip_prefix("parent=") {
                    t["parent"] = parent.into();
                }
                kept.push(t.clone());
                answer(200, t)
            }
            ("PATCH", ["lists", list, "tasks", id]) => {
                let Some(t) = kept.iter_mut().find(|t| t["id"] == *id && t["list"] == *list) else {
                    return answer(404, json!({"error": {"message": "gone"}}));
                };
                for (k, v) in r.json().as_object().cloned().unwrap_or_default() {
                    t[k] = v;
                }
                t["etag"] = "\"2\"".into();
                answer(200, t.clone())
            }
            ("DELETE", ["lists", list, "tasks", id]) => {
                kept.retain(|t| !(t["id"] == *id && t["list"] == *list));
                (204, "application/json".into(), Vec::new())
            }
            _ => answer(404, json!({"error": {"message": "no such thing"}})),
        }
    })
}

#[test]
fn google_tasks_are_a_source_of_tasks_alone() -> Result<()> {
    let (b, path) = backend()?;
    let tasks = Arc::new(M::new(vec![json!({"id": "g0", "list": "L1", "title": "Taxes", "status": "needsAction", "etag": "\"1\""})]));
    let google = google_tasks(tasks.clone());
    let mut a = Account::new(Provider::Gmail);
    a.email = "ana@example.org".into();
    a.oauth = true;
    a.incoming_host = "127.0.0.1".into();
    a.incoming_port = google.port;
    b.store.save_account(&a)?;
    // Tasks access given already: nothing is asked in the browser.
    b.store.set_preference(&format!("google_granted:{}", a.id), &json!([crate::sync::oauth::GOOGLE_TASKS]).to_string())?;
    assert_eq!(b.task_service_accounts().len(), 1);
    assert_eq!(b.connect_task_service(&a, Arc::new(AtomicBool::new(false)))?, 1);
    let (source, server, _) = b.calendar_sources().remove(0);
    assert!(server.tasks_only && server.google_account == a.id);
    assert!(server.label.contains("Google Tasks"));
    assert!(b.task_service_accounts().is_empty(), "added once");
    // New events don't go to it.
    assert_eq!(b.default_calendar_source(), "");
    assert!(b.default_calendar().is_none());
    let lists: Vec<String> = b.task_list_choices().into_iter().map(|l| l.href).collect();
    assert_eq!(lists, ["gtasks:L1", "gtasks:L2"]);
    assert_eq!(b.calendar_tasks()[0].event.uid, "g0");

    // A task made from an email: named after it, with a link back to it.
    let m = Mail {
        id: "m1".into(),
        subject: "Invoice 42".into(),
        from: "Bo <bo@example.se>".into(),
        message_id: "<inv/42@example.se>".into(),
        date: 1_795_000_000,
        ..Default::default()
    };
    let draft = TaskDraft::from_mail(&m, Some("Pay by Friday"));
    assert_eq!(draft.summary, "Invoice 42");
    assert_eq!(draft.url, "mid:inv%2F42@example.se");
    assert!(draft.notes.starts_with("Pay by Friday") && draft.notes.contains("Bo <bo@example.se>"), "{}", draft.notes);
    let invoice = b.add_task_draft(&draft)?;
    assert_eq!((invoice.calendar.as_str(), invoice.href.as_str()), ("gtasks:L1", "gtasks:g2"));
    assert_eq!(invoice.event.uid, "g2", "its id at Google");
    assert_eq!(invoice.event.url, draft.url);
    let at_google = tasks.lock().unwrap().iter().find(|t| t["id"] == "g2").cloned().unwrap();
    assert!(at_google["notes"].as_str().unwrap().ends_with("\n\nmid:inv%2F42@example.se"));

    // A subtask: under it, in its list.
    let sub = b.add_task_draft(&TaskDraft { summary: "Find the account".into(), parent: invoice.event.uid.clone(), ..Default::default() })?;
    assert_eq!((sub.calendar.as_str(), sub.event.parent.as_str()), ("gtasks:L1", "g2"));
    assert_eq!(tasks.lock().unwrap().iter().find(|t| t["id"] == sub.event.uid.as_str()).unwrap()["parent"], "g2");
    // In the list chosen; and once it's the default, there unasked.
    let milk = b.add_task_draft(&TaskDraft { summary: "Milk".into(), list: "gtasks:L2".into(), ..Default::default() })?;
    assert_eq!(milk.calendar, "gtasks:L2");
    b.set_default_task_list("gtasks:L2")?;
    assert_eq!(b.default_task_list().as_deref(), Some("gtasks:L2"));
    let bread = b.add_task_draft(&TaskDraft { summary: "Bread".into(), ..Default::default() })?;
    assert_eq!(bread.calendar, "gtasks:L2");
    assert_eq!(b.calendar_tasks().len(), 5);

    // Done: off the list, into the history; to do again: back.
    b.complete_task(&invoice.event.uid, true)?;
    assert!(!b.calendar_tasks().iter().any(|t| t.event.uid == invoice.event.uid));
    let done = b.completed_tasks();
    assert_eq!(done.len(), 1);
    assert!(done[0].event.completed && done[0].event.completed_at > 0);
    assert_eq!(tasks.lock().unwrap().iter().find(|t| t["id"] == "g2").unwrap()["status"], "completed");
    // Read again, Google lists only what's to do.
    assert_eq!(b.sync_tasks()?, 4);
    let again = b.reopen_task(&invoice.event.uid)?;
    assert!(!again.event.completed);
    assert!(b.completed_tasks().is_empty());
    assert!(b.calendar_tasks().iter().any(|t| t.event.uid == invoice.event.uid));
    assert_eq!(tasks.lock().unwrap().iter().find(|t| t["id"] == "g2").unwrap()["status"], "needsAction");

    // Edited: priority and tags (Google keeps neither, Picacho does until
    // read again); moved to another list: made there, gone from the first.
    let edited = b.edit_task_draft(&milk.event.uid, &TaskDraft { summary: "Oat milk".into(), list: "gtasks:L2".into(), ..Default::default() })?;
    assert_eq!(edited.event.summary, "Oat milk");
    let moved = b.edit_task_draft(&bread.event.uid, &TaskDraft { summary: "Bread".into(), list: "gtasks:L1".into(), ..Default::default() })?;
    assert_eq!(moved.calendar, "gtasks:L1");
    assert_ne!(moved.event.uid, bread.event.uid);
    assert!(!tasks.lock().unwrap().iter().any(|t| t["id"] == bread.event.uid.as_str()));
    assert_eq!(b.calendar_tasks().len(), 5);

    // Done again, then the history cleared; removed, the source's tasks go.
    b.complete_task(&moved.event.uid, true)?;
    assert_eq!(b.completed_tasks().len(), 1);
    b.clear_completed_tasks()?;
    assert!(b.completed_tasks().is_empty());
    b.remove_calendar_source(&source)?;
    assert!(b.calendar_tasks().is_empty() && b.task_list_choices().is_empty());
    assert!(b.default_task_list().is_none(), "its list is gone");
    drop(b);
    std::fs::remove_dir_all(path)?;
    Ok(())
}

fn task(uid: &str, start: i64, all_day: bool, priority: u8, parent: &str) -> Stored {
    Stored {
        event: Event { uid: uid.into(), summary: uid.into(), start, end: start, all_day, priority, parent: parent.into(), task: true, ..Default::default() },
        ..Default::default()
    }
}

#[test]
fn tasks_stand_by_their_due_day_and_order_with_their_subtasks() {
    let today = chrono::NaiveDate::from_ymd_opt(2026, 11, 24).unwrap();
    let day = |d: u32| chrono::NaiveDate::from_ymd_opt(2026, 11, d).unwrap().and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp();
    // All-day ones by their date, wherever the reader is.
    assert_eq!(Due::of(&task("a", day(23), true, 0, "").event, today), Due::Overdue);
    assert_eq!(Due::of(&task("b", day(24), true, 0, "").event, today), Due::Today);
    assert_eq!(Due::of(&task("c", day(25), true, 0, "").event, today), Due::Upcoming);
    assert_eq!(Due::of(&task("d", 0, false, 0, "").event, today), Due::Someday);
    // Timed ones by their local date.
    use chrono::TimeZone;
    let noon = chrono::Local
        .from_local_datetime(&today.and_hms_opt(12, 0, 0).unwrap())
        .earliest()
        .unwrap()
        .timestamp();
    assert_eq!(Due::of(&task("e", noon, false, 0, "").event, today), Due::Today);

    // Soonest first, then the most important, subtasks under their task.
    let order = ordered(vec![
        task("later", day(26), true, 0, ""),
        task("sub-b", day(30), true, 0, "first"),
        task("first", day(25), true, 5, ""),
        task("urgent", day(25), true, 1, ""),
        task("someday", 0, false, 0, ""),
        task("sub-a", day(28), true, 0, "first"),
        task("orphan", day(27), true, 0, "gone"),
    ]);
    let got: Vec<(&str, usize)> = order.iter().map(|(t, d)| (t.event.uid.as_str(), *d)).collect();
    assert_eq!(
        got,
        [("urgent", 0), ("first", 0), ("sub-a", 1), ("sub-b", 1), ("later", 0), ("orphan", 0), ("someday", 0)]
    );
}

#[test]
fn tasks_from_email_and_tags_as_typed() {
    assert_eq!(TaskDraft::tags_from(" #home, Work;home ,, #"), ["home", "Work"]);
    assert_eq!(mail_link("<a b/c@x>"), "mid:a%20b%2Fc@x");
    assert_eq!(mail_link(""), "");
    // Without a subject, the first line chosen names it.
    let m = Mail { subject: " ".into(), message_id: "<m@x>".into(), ..Default::default() };
    let d = TaskDraft::from_mail(&m, Some("\n  Call the bank tomorrow\nabout the card"));
    assert_eq!(d.summary, "Call the bank tomorrow");
    assert_eq!(d.url, "mid:m@x");
    let none = TaskDraft::from_mail(&m, None);
    assert_eq!(none.summary, t!("manage-tasks-no-subject"));
    assert!(none.notes.is_empty());
}

/// A Google account with Google Tasks added, against `tasks`.
fn google_source(b: &Backend, tasks: Arc<M<Vec<Value>>>) -> Result<(test_server::Server, String)> {
    let google = google_tasks(tasks);
    let mut a = Account::new(Provider::Gmail);
    a.email = "ana@example.org".into();
    a.oauth = true;
    a.incoming_host = "127.0.0.1".into();
    a.incoming_port = google.port;
    b.store.save_account(&a)?;
    b.store.set_preference(&format!("google_granted:{}", a.id), &json!([crate::sync::oauth::GOOGLE_TASKS]).to_string())?;
    b.connect_task_service(&a, Arc::new(AtomicBool::new(false)))?;
    let source = b.calendar_sources().remove(0).0;
    Ok((google, source))
}

/// What Google doesn't keep (priority, tags, the time it's due) is kept
/// here and stays through syncs, a due time only while Google has it due
/// that day; a task moved to another list takes all it has and its
/// subtasks; what was done at Google lately is in the history.
#[test]
fn what_google_doesnt_keep_is_kept_here_and_moves_with_the_task() -> Result<()> {
    let (b, path) = backend()?;
    let tasks = Arc::new(M::new(Vec::new()));
    let (_google, _source) = google_source(&b, tasks.clone())?;
    use chrono::TimeZone;
    let tomorrow = chrono::Local::now().date_naive() + chrono::Duration::days(1);
    let five = chrono::Local.from_local_datetime(&tomorrow.and_hms_opt(17, 0, 0).unwrap()).earliest().unwrap().timestamp();
    let draft = TaskDraft { summary: "Report".into(), priority: 2, tags: vec!["work".into()], due: five, list: "gtasks:L1".into(), ..Default::default() };
    let report = b.add_task_draft(&draft)?;
    // Google has the day.
    let day = tasks.lock().unwrap()[0]["due"].as_str().unwrap().to_owned();
    assert_eq!(day, format!("{}T00:00:00.000Z", tomorrow.format("%Y-%m-%d")));
    let sub = b.add_task_draft(&TaskDraft { summary: "Charts".into(), parent: report.event.uid.clone(), tags: vec!["work".into()], ..Default::default() })?;
    b.sync_tasks()?;
    let read = b.calendar_tasks().into_iter().find(|t| t.event.uid == report.event.uid).unwrap();
    assert_eq!((read.event.priority, read.event.categories.clone(), read.event.start, read.event.all_day), (2, vec!["work".to_owned()], five, false));
    // Moved: in the other list with all it has, its subtask under it.
    let moved = b.edit_task_draft(&report.event.uid, &TaskDraft { list: "gtasks:L2".into(), ..draft.clone() })?;
    assert_eq!((moved.calendar.as_str(), moved.event.priority), ("gtasks:L2", 2));
    let now = b.calendar_tasks();
    assert_eq!(now.len(), 2);
    let charts = now.iter().find(|t| t.event.summary == "Charts").unwrap();
    assert_eq!((charts.calendar.as_str(), charts.event.parent.as_str()), ("gtasks:L2", moved.event.uid.as_str()));
    assert_ne!(charts.event.uid, sub.event.uid);
    assert_eq!(charts.event.categories, ["work"]);
    {
        let kept = tasks.lock().unwrap();
        assert_eq!(kept.len(), 2, "the first ones gone from Google: {kept:?}");
        assert!(kept.iter().all(|t| t["list"] == "L2"));
    }
    b.sync_tasks()?;
    let read = b.calendar_tasks().into_iter().find(|t| t.event.uid == moved.event.uid).unwrap();
    assert_eq!((read.event.priority, read.event.start), (2, five));
    // Due another day at Google: that day, without the time kept here.
    {
        let mut kept = tasks.lock().unwrap();
        let t = kept.iter_mut().find(|t| t["id"] == moved.event.uid.as_str()).unwrap();
        t["due"] = "2030-01-15T00:00:00.000Z".into();
    }
    b.sync_tasks()?;
    let read = b.calendar_tasks().into_iter().find(|t| t.event.uid == moved.event.uid).unwrap();
    assert!(read.event.all_day);
    assert_eq!(read.event.priority, 2, "its priority stays");
    // Done at Google: into the history when it's read.
    {
        let mut kept = tasks.lock().unwrap();
        let t = kept.iter_mut().find(|t| t["id"] == charts.event.uid.as_str()).unwrap();
        t["status"] = "completed".into();
        t["completed"] = chrono::Utc::now().to_rfc3339().into();
    }
    assert!(b.completed_tasks().is_empty());
    assert_eq!(b.sync_completed_tasks()?, 1);
    let done = b.completed_tasks();
    assert_eq!(done[0].event.summary, "Charts");
    assert_eq!(done[0].event.categories, ["work"], "with what's kept here");
    drop(b);
    std::fs::remove_dir_all(path)?;
    Ok(())
}

/// Microsoft To Do, a list's tasks and their steps as stored by To Do.
fn to_do(tasks: Arc<M<Vec<Value>>>) -> test_server::Server {
    test_server::start(move |r| {
        let path = r.path.split('?').next().unwrap_or_default().to_owned();
        let query = percent_encoding::percent_decode_str(r.path.split_once('?').map(|(_, q)| q).unwrap_or_default())
            .decode_utf8_lossy()
            .into_owned();
        let mut kept = tasks.lock().unwrap();
        let parts: Vec<&str> = path.trim_start_matches("/v1.0/me/todo/").split('/').collect();
        let next = kept.len() + kept.iter().map(|t| t["checklistItems"].as_array().map_or(0, Vec::len)).sum::<usize>() + 1;
        match (r.method.as_str(), parts.as_slice()) {
            ("GET", ["lists"]) => answer(200, json!({"value": [{"id": "main", "displayName": "Tasks", "wellknownListName": "defaultList"}]})),
            ("GET", ["lists", "main", "tasks"]) => {
                let done = query.contains("status eq 'completed'");
                let listed: Vec<Value> = kept.iter().filter(|t| (t["status"] == "completed") == done).cloned().collect();
                answer(200, json!({ "value": listed }))
            }
            ("POST", ["lists", "main", "tasks"]) => {
                let mut t = r.json();
                t["id"] = format!("t{next}").into();
                t["checklistItems"] = json!([]);
                kept.push(t.clone());
                answer(201, t)
            }
            ("PATCH", ["lists", "main", "tasks", id]) => {
                let Some(t) = kept.iter_mut().find(|t| t["id"] == *id) else { return answer(404, json!({})) };
                for (k, v) in r.json().as_object().cloned().unwrap_or_default() {
                    t[k] = v;
                }
                answer(200, t.clone())
            }
            ("POST", ["lists", "main", "tasks", id, "checklistItems"]) => {
                let Some(t) = kept.iter_mut().find(|t| t["id"] == *id) else { return answer(404, json!({})) };
                let mut step = r.json();
                step["id"] = format!("s{next}").into();
                t["checklistItems"].as_array_mut().unwrap().push(step.clone());
                answer(201, step)
            }
            ("PATCH", ["lists", "main", "tasks", id, "checklistItems", step]) => {
                let Some(t) = kept.iter_mut().find(|t| t["id"] == *id) else { return answer(404, json!({})) };
                let Some(s) = t["checklistItems"].as_array_mut().unwrap().iter_mut().find(|s| s["id"] == *step) else {
                    return answer(404, json!({}));
                };
                for (k, v) in r.json().as_object().cloned().unwrap_or_default() {
                    s[k] = v;
                }
                if s["isChecked"] == true {
                    s["checkedDateTime"] = chrono::Utc::now().to_rfc3339().into();
                }
                answer(200, s.clone())
            }
            _ => answer(404, json!({"error": {"message": "no such thing"}})),
        }
    })
}

/// To Do's steps are subtasks: listed under their task, made, checked and
/// in the history; what a step doesn't keep (notes, a due date) and the
/// time a task is due are kept here.
#[test]
fn to_do_steps_are_subtasks() -> Result<()> {
    let path = std::env::temp_dir().join(format!("picacho-todo-{}", uuid::Uuid::new_v4()));
    let tasks = Arc::new(M::new(vec![json!({
        "id": "t1", "title": "Report", "status": "notStarted", "importance": "normal",
        "checklistItems": [{"id": "s1", "displayName": "Draft", "isChecked": false}, {"id": "s0", "displayName": "Outline", "isChecked": true}],
    })]));
    let server = to_do(tasks.clone());
    let mut b = Backend::new(Arc::new(Store::open(&path)?));
    b.test_secret = Some(Secret {
        access_token: "token".into(),
        expires_at: chrono::Utc::now().timestamp() + 3600,
        // To Do access given already: nothing is asked in the browser.
        extra_scopes: crate::sync::oauth::GRAPH_TASKS.into(),
        ..Default::default()
    });
    let mut a = Account::new(Provider::Outlook);
    a.api = true;
    a.oauth = true;
    a.client_id = "client".into();
    a.email = "qa@example.com".into();
    a.incoming_host = "127.0.0.1".into();
    a.incoming_port = server.port;
    b.store.save_account(&a)?;
    assert_eq!(b.task_service_accounts().len(), 1);
    assert_eq!(b.connect_task_service(&a, Arc::new(AtomicBool::new(false)))?, 2, "the task and its step to do");
    let step = b.calendar_tasks().into_iter().find(|t| t.event.summary == "Draft").unwrap();
    assert_eq!((step.event.uid.as_str(), step.event.parent.as_str(), step.calendar.as_str()), ("t1#s1", "t1", "todo:main"));
    // A subtask made here is a step; its notes and due date are kept here.
    let due = chrono::NaiveDate::from_ymd_opt(2030, 3, 1).unwrap().and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp();
    let figures = b.add_task_draft(&TaskDraft { summary: "Figures".into(), notes: "Q1 and Q2".into(), due, all_day: true, parent: "t1".into(), ..Default::default() })?;
    assert!(figures.event.uid.starts_with("t1#"), "{}", figures.event.uid);
    assert_eq!(tasks.lock().unwrap()[0]["checklistItems"].as_array().unwrap().len(), 3);
    b.sync_tasks()?;
    let read = b.calendar_tasks().into_iter().find(|t| t.event.uid == figures.event.uid).unwrap();
    assert_eq!((read.event.description.as_str(), read.event.start, read.event.all_day), ("Q1 and Q2", due, true));
    // A priority To Do has only in steps (3: high) and a time it doesn't keep.
    let at = due + 17 * 3600;
    let call = b.add_task_draft(&TaskDraft { summary: "Call".into(), priority: 3, due: at, ..Default::default() })?;
    assert_eq!(tasks.lock().unwrap().last().unwrap()["importance"], "high");
    b.sync_tasks()?;
    let read = b.calendar_tasks().into_iter().find(|t| t.event.uid == call.event.uid).unwrap();
    assert_eq!((read.event.priority, read.event.start, read.event.all_day), (3, at, false));
    // A step checked: done there, and in the history.
    b.complete_task("t1#s1", true)?;
    assert_eq!(tasks.lock().unwrap()[0]["checklistItems"][0]["isChecked"], true);
    assert!(!b.calendar_tasks().iter().any(|t| t.event.uid == "t1#s1"));
    assert_eq!(b.sync_completed_tasks()?, 1, "checked here (and To Do says when); the old step has no time");
    // The task done: its steps go with it.
    b.complete_task("t1", true)?;
    assert!(!b.calendar_tasks().iter().any(|t| t.event.parent == "t1"));
    drop(b);
    std::fs::remove_dir_all(path)?;
    Ok(())
}

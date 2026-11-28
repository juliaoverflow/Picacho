# Tasks: the Tasks view, tasks made from email, and task services
# (Microsoft To Do, Google Tasks).

manage-tasks-no-subject = (No subject)
manage-tasks-from-email = From { $from }, { $when }
manage-tasks-microsoft-to-do = Microsoft To Do ({ $email })
manage-tasks-google-tasks = Google Tasks ({ $email })
manage-tasks-only-outlook-and-google = Only Outlook accounts that use Microsoft Graph and Google accounts have a task service to add.
manage-tasks-this-account-keeps-only-tasks = This account keeps only tasks.
manage-tasks-not-a-task-list = That isn't one of this account's task lists.
manage-tasks-the-service-didnt-say-which-task = The task service didn't say which task it made.

## The Tasks view.

tasks-today = Today
tasks-upcoming = Upcoming
tasks-overdue = Overdue
tasks-all = All
tasks-completed = Completed
tasks-no-due-date = No due date
tasks-priority = Priority
tasks-priority-none = None
tasks-priority-low = Low
tasks-priority-medium = Medium
tasks-priority-high = High
tasks-add-a-task = Add a task
tasks-due-placeholder = Due: 2026-11-24, 2026-11-24 17:00 or “tomorrow 5pm”
tasks-title = Task
tasks-due = Due
tasks-notes = Notes
tasks-tags = Tags
tasks-tags-placeholder = Tags, separated by commas
tasks-list = List
tasks-write-the-due-date = Write the due date as 2026-11-24 or 2026-11-24 17:00, or in words (“tomorrow 5pm”).
tasks-deleted = Task deleted
tasks-done = Task done
tasks-opened-again = “{ $summary }” is to do again
tasks-history-cleared = The completed history is cleared
tasks-read = { $n ->
    [one] One task to do
   *[other] { $n } tasks to do
}
tasks-read-from = { $n ->
    [one] One task to do from { $email }
   *[other] { $n } tasks to do from { $email }
}
tasks-service-removed = Task service removed
tasks-set-up-a-task-list-first = Set up somewhere to keep tasks first: a calendar server in Settings → Calendar, or Microsoft To Do or Google Tasks in Tasks.
tasks-added-to-tasks = Added “{ $summary }” to Tasks
tasks-reading = Reading…
tasks-read-now = Read now
tasks-new-task = New task
tasks-new-subtask = New subtask of “{ $parent }”
tasks-add = Add
tasks-all-lists = All lists
tasks-unnamed-list = Tasks
tasks-make-default-list = New tasks go to this list
tasks-no-task-lists = Nowhere keeps tasks yet. Connect a calendar server that keeps tasks in Settings → Calendar, or add Microsoft To Do or Google Tasks below.
tasks-nothing-today = Nothing is due today.
tasks-nothing-upcoming = Nothing is coming up.
tasks-nothing-overdue = Nothing is overdue.
tasks-nothing-to-do = Nothing to do.
tasks-nothing-done = No tasks were done here lately.
tasks-mark-done = Mark “{ $summary }” done
tasks-open-email = Open email
tasks-open-link = Open link
tasks-open-linked-email = Open the email this task was made from
tasks-add-subtask = Subtask
tasks-done-on-in = Done { $when } · { $list }
tasks-done-on = Done { $when }
tasks-reopen = To do again
tasks-clear-history = Clear the completed history
tasks-task-services = Task services
tasks-service-read = Tasks to do are read from it. Last read: { $synced }.
tasks-google-tasks-caption = Google Tasks, with this account’s sign-in. Google asks once for access to your tasks.
tasks-microsoft-to-do-caption = Microsoft To Do, with this account’s sign-in. Microsoft asks once for access to your tasks.
tasks-add-google-tasks = Add Google Tasks
tasks-add-microsoft-to-do = Add Microsoft To Do

## Tasks made from email.

tasks-add-to-tasks = Add to Tasks
tasks-add-to-tasks-button = @Add to Tasks
tasks-add-selection-to-tasks = Add selection to Tasks

## Task services.

sync-todo-steps-have-no-steps = Microsoft To Do keeps steps only under tasks, not under other steps.

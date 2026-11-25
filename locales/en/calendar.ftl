# src/display/calendar.rs and nearby views.

# When an event is here, then in its own time zone: "Mon, Nov 23, 9:00 – 10:00 (16:00 – 17:00 in Asia/Tokyo)"
calendar-when-in-its-zone = { $when } ({ $start } – { $end } in { $zone })

calendar-all-day = { $first } – { $last }, all day

calendar-all-day-2 = { $day }, all day

calendar-cancelled = Cancelled

calendar-answer-to-your-invitation = Answer to your invitation

calendar-new-time-proposed = New time proposed

calendar-your-proposed-time-wasnt-taken = Your proposed time wasn't taken

calendar-a-guest-asks-for-the-latest = A guest asks for the latest version

calendar-more-times-added = More times added

calendar-event = Event

calendar-updated-invitation = Updated invitation

calendar-invitation = Invitation

calendar-optional = { $name }, optional

calendar-yes = { $name } (yes)

calendar-no = { $name } (no)

calendar-maybe = { $name } (maybe)

calendar-delegated = { $name } (delegated)

calendar-no-title = (No title)

calendar-when = When

calendar-repeats = Repeats

calendar-yes-on-a-schedule-of-its = Yes, on a schedule of its own (each time is listed in your calendar)

calendar-yes-2 = Yes

calendar-where = Where

calendar-organizer = Organizer

calendar-guests = Guests

calendar-accept = Accept

calendar-maybe-2 = Maybe

calendar-decline = Decline

calendar-remove-from-calendar = Remove from Calendar

calendar-add-to-calendar = Add to Calendar

calendar-was = was { $when }

calendar-was-at = was at { $location }

calendar-changed-since-its-in-your-calendar = Changed since it's in your calendar.

calendar-changed = Changed: { $changes }.

calendar-update-my-calendar = Update My Calendar

calendar-propose-a-new-time = Propose a New Time…

calendar-note-their-answer = Note Their Answer

calendar-now = Now: { $before }

calendar-move-the-event = Move the Event

calendar-keep-the-time = Keep the Time

calendar-send-it-again = Send It Again

calendar-note = Note

calendar-you-answered = You answered: { $verb }

calendar-the-organizer-was-told = { $verb } — the organizer was told

calendar-removed-from-your-calendar = Removed from your calendar

calendar-write-the-times-as-2026-11 = Write the times as 2026-11-24 10:00.

calendar-your-proposed-time-was-sent-to = Your proposed time was sent to the organizer

calendar-the-event-moved-told = { $n ->
        [one] The event moved; { $n } guest was told
       *[other] The event moved; { $n } guests were told
    }

calendar-added-to-your-calendar = Added to your calendar

calendar-read-events = { $n ->
        [one] Read { $n } event
       *[other] Read { $n } events
    }

calendar-isnt-a-time-zone-picacho-knows = “{ $trim }” isn't a time zone Picacho knows. Write it as Europe/Berlin or America/New_York, or a city such as Tokyo.

calendar-picacho-couldnt-tell-when-it-starts = Picacho couldn't tell when it starts. Try “tomorrow at 10”, or 2026-11-24 10:00.

calendar-write-the-start-as-2026-11 = Write the start as 2026-11-24 10:00, or 2026-11-24 for all day. (Settings → Local AI → Understand dates reads “tomorrow at 10” too.)

calendar-write-the-end-the-same-way = Write the end the same way as the start, or leave it empty for an hour.

calendar-add-an-email-account-first-invitations = Add an email account first: invitations are sent from it.

calendar-event-saved-and-invited = { $n ->
        [one] Event saved, and { $n } guest invited
       *[other] Event saved, and { $n } guests invited
    }

calendar-event-saved = Event saved

calendar-event-deleted-told-its-cancelled = Event deleted; { $told } told it's cancelled

calendar-event-deleted = Event deleted

calendar-read-events-from = { $n ->
        [one] Read { $n } event from { $email }
       *[other] Read { $n } events from { $email }
    }

calendar-write-the-new-name-in-the = Write the new name in the field first.

calendar-renamed-the-calendar = Renamed the calendar { $name }

calendar-attach = Attach

calendar-attached-the-file-to-the-event = Attached the file to the event

calendar-removed-the-attachment = Removed the attachment

calendar-deleted-the-calendar = Deleted the calendar

calendar-made-the-calendar = Made the calendar { $name }

calendar-write-the-due-date-as-2026 = Write the due date as 2026-11-24 17:00 or 2026-11-24, or leave it empty.

calendar-added-the-task = Added the task { $summary }

calendar-saved-the-task = Saved the task { $summary }

calendar-the-calendar-server-now-sends-your = The calendar server now sends your invitations

calendar-picacho-emails-your-invitations-again = Picacho emails your invitations again

calendar-invitations-sent-again-to = { $n ->
        [one] Invitations sent again to { $n } guest
       *[other] Invitations sent again to { $n } guests
    }

calendar-looking = Looking…

calendar-everyone-is-free-that-day = Everyone is free that day.

calendar-is-free = { $attendee } is free

calendar-is-busy = { $attendee } is busy { $times }

calendar-taken-back-its-in-drafts = Taken back; it's in Drafts

calendar-it-had-already-been-sent = It had already been sent

# A file and its size
calendar-file-and-size = { $name } ({ $kb } KB)

calendar-attachment = Attachment

calendar-remove = Remove

calendar-the-server-couldnt-deliver-the-invitation = The server couldn't deliver the invitation to { $addresses }.

calendar-send-again = Send Again

calendar-thats-here = That's { $here } here.

calendar-propose-a-new-time-2 = Propose a new time

calendar-starts = Starts

calendar-ends = Ends

calendar-note-to-the-organizer-optional = Note to the organizer (optional)

calendar-send-proposal = Send Proposal

calendar-cancel = Cancel

calendar-or-use-your-email-providers-signed = Or use your email provider's, signed in as the account already is (Google asks once for access):

calendar-calendar = Calendar

calendar-invitations-in-mail-show-what-when = Invitations in mail show what, when and where, with Accept, Maybe and Decline; the organizer is told. With a calendar server, answered invitations go in your calendar, and you can add and change events here.

calendar-calendar-server-caldav = Calendar server (CalDAV)

calendar-fastmail-icloud-nextcloud-radicale-mailbox-org = Fastmail, iCloud, Nextcloud, Radicale, mailbox.org or another CalDAV server. iCloud and Fastmail need an app password.

calendar-server = Server

calendar-user-name = User name

calendar-password = Password

calendar-connecting = Connecting…

calendar-connect = Connect

calendar-calendar-server-jmap = Calendar server (JMAP)

calendar-read-read-again-each-time-picacho = { $how ->
        [jmap] The calendars of { $user }. Read { $synced }; read again each time Picacho starts.
        [account] Signed in as { $user }, with your { $email } account's sign-in. Read { $synced }; read again each time Picacho starts.
       *[password] Signed in as { $user }. Read { $synced }; read again each time Picacho starts.
    }

calendar-reading = Reading…

calendar-read-now = Read Now

calendar-disconnect = Disconnect

calendar-shared-calendars = Shared calendars

calendar-dismiss = Dismiss

calendar-new-events-go-to = New events go to

calendar-also-where-invitations-you-accept-are = Also where invitations you accept are put.

calendar-let-the-server-send-invitations = Let the server send invitations

calendar-the-calendar-server-invites-guests-to = The calendar server invites guests to events you organize, and tells them about changes and cancellations. Invitations you receive are still answered by mail.

calendar-picacho-emails-invitations-updates-and-cancellations = Picacho emails invitations, updates and cancellations for events you organize. Turn on to have the calendar server send them, where it can.

calendar-new-calendar-or-task = New calendar or task

calendar-a-calendar-is-made-on-the = A calendar is made on the server. A task goes to the calendar new events go to (or the first that keeps tasks), due when written in Due, else with no due date.

calendar-make-calendar = Make Calendar

calendar-add-task = Add Task

calendar-rename = Rename

calendar-delete = Delete

calendar-the-server-keeps-mb-of-yours = The server keeps { $used } MB of yours, with { $left } MB left.

calendar-calendars = Calendars

calendar-rename-uses-the-name-written-in = Rename uses the name written in the field above. Deleting removes the calendar and its events from the server.

calendar-tasks = Tasks

calendar-no-due-date = no due date

calendar-due = { $summary }, due { $due }

calendar-no-due-date-2 = No due date

calendar-overdue = Overdue: { $due }

calendar-due-2 = Due { $due }

calendar-edit = Edit

calendar-done = Done

calendar-edit-task = Edit task

calendar-edit-task-2 = Edit Task

calendar-task = Task

calendar-due-2026-11-24-17-00 = Due (2026-11-24 17:00, a date, or empty for none)

calendar-notes = Notes

calendar-save = Save

calendar-held-to-send-later = Held to send later

calendar-messages-handed-to-the-server-to = Messages handed to the server to send later can be taken back until they go.

calendar-nothing-is-waiting = Nothing is waiting.

calendar-waiting = { $count } waiting.

calendar-show = Show

calendar-no-subject = (No subject)

calendar-goes = Goes { $when }

calendar-take-back = Take Back

calendar-new-event = New event

calendar-edit-event = Edit event

calendar-new-event-2 = New Event

calendar-edit-event-2 = Edit Event

calendar-title = Title

calendar-starts-2026-11-24-10-00 = Starts (2026-11-24 10:00, a date for all day, or words like “tomorrow at 10” with Understand dates)

calendar-ends-empty-an-hour-later = Ends (empty: an hour later)

calendar-time-zone-the-times-above-are = Time zone (the times above are in it)

calendar-guests-invited-by-mail-separated-by = Guests (invited by mail; separated by commas)

calendar-when-are-they-free = When Are They Free?

calendar-remind-me = Remind me

calendar-never = Never

calendar-10-minutes-before = 10 minutes before

calendar-an-hour-before = An hour before

calendar-a-day-before = A day before

calendar-this-event-repeats-change = This event repeats. Change

calendar-only-this-one = Only this one

calendar-every-repeat = Every repeat

calendar-daily = Daily

calendar-weekly = Weekly

calendar-monthly = Monthly

calendar-yearly = Yearly

calendar-as-it-was = As it was

calendar-attach-file = Attach File

calendar-coming-up = Coming up

calendar-coming-events = Coming events

calendar-nothing-coming-up = Nothing coming up.

# When the server hasn't been read yet ("Read never")
calendar-never-read = never

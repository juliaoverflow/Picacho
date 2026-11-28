# src/display/palette.rs: the command palette (⌘K, Ctrl+K), one field that
# finds and runs actions, accounts, folders, labels, saved searches,
# contacts, templates and quick actions on the open or selected messages.

# The palette's name, for screen readers.
palette-command-palette = Command Palette

# Its item in the View menu.
palette-command-palette-menu = Command Palette…

palette-type-a-command-or-search = Type a command, or search folders, contacts…

# The field's placeholder in each quick action's choices.
palette-find-a-folder = Move to which folder?

palette-snooze-until-when = Snooze until when?

palette-find-a-label = Add or remove which label?

palette-results = Results

palette-nothing-matches = Nothing matches

palette-back-to-everything = Back to everything

# The footer's key hints: "↑↓ to move", "↩ to choose", "Esc to close".
palette-to-move = to move

palette-to-choose = to choose

palette-to-close = to close

palette-to-go-back = to go back

# What each result is, at the end of its row.
palette-kind-quick-action = Quick action

palette-kind-command = Command

palette-kind-account = Account

palette-kind-folder = Folder

palette-kind-saved-search = Saved search

palette-kind-label = Label

palette-kind-template = Template

palette-kind-contact = Contact

# Quick actions on the open message, or the messages selected.
palette-move-to = Move to…

palette-snooze-until = Snooze until…

palette-apply-label = Apply label…

# Beside a quick action when several messages are selected.
palette-messages = { $count ->
        [one] { $count } message
       *[other] { $count } messages
    }

# Other words each quick action is found by (not shown).
palette-move-to-keywords = folder file archive

palette-snooze-keywords = later remind defer

palette-label-keywords = tag labels

palette-template-keywords = new message template

# Snooze until…'s last choice: the snooze dialog.
palette-pick-a-date-and-time = Pick a date and time…

palette-all-accounts = All Accounts

# Commands without a keyboard shortcut.
palette-mark-all-as-read = Mark All as Read

palette-show-unread-only = Show Unread Only

palette-show-starred-only = Show Starred Only

palette-show-messages-with-attachments = Show Messages with Attachments

palette-show-all-messages = Show All Messages

palette-save-as-pdf = Save as PDF…

palette-open-calendar = Open Calendar

palette-open-contacts = Open Contacts

palette-open-tasks = Open Tasks

palette-add-account = Add Account…

palette-about-picacho = About Picacho

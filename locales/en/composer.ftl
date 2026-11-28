# src/display/composer.rs and nearby views.

composer-recipients = Recipients

composer-cc = Cc

composer-bcc = Bcc

composer-where-replies-go = Where replies go

composer-subject = Subject

composer-find-an-address = Find an address

composer-write-your-message = Write your message…

composer-forward = Forward

composer-reply = Reply

composer-new-message = New Message

composer-pasted-image = Pasted image.{ $extension }

composer-paste-without-formatting = Paste without formatting

composer-paste-with-formatting = Paste with formatting

composer-choose-a-time-in-the-future = Choose a time in the future.

composer-sends-only-as-its-aliases-choose = { $email } sends only as its aliases. Choose an alias in From.

composer-add-at-least-one-recipient = Add at least one recipient.

composer-reply-to = Reply-To: { $e }

composer-send-unencrypted = Send unencrypted?

composer-send-without-the-quote = Send Without the Quote

composer-keep-editing = Keep Editing

composer-this-message-has-no-subject = This message has no subject.

composer-the-message-mentions-an-attachment-but = The message mentions an attachment, but nothing is attached.

composer-schedule-anyway = Schedule anyway?

composer-send-anyway = Send anyway?

composer-schedule = Schedule

composer-send = Send

composer-not-sent = Not sent { $when }: { $send_error }

composer-scheduled-to-send-it-waits-in = Scheduled to send { $when }. It waits in Drafts until then while Picacho is open.

composer-dont-send-later = Don't Send Later

composer-remind-me-if-no-reply = Remind me if no reply

composer-reminds-you-if-no-reply = { $days ->
        [1] If nobody replies within a day, Picacho reminds you.
        [7] If nobody replies within a week, Picacho reminds you.
        [14] If nobody replies within two weeks, Picacho reminds you.
       *[other] If nobody replies within { $days } days, Picacho reminds you.
    }

composer-remind-me-if-nobody-replies = REMIND ME IF NOBODY REPLIES

composer-dont-remind-me = Don't Remind Me

composer-rich-text = Rich text

composer-send-later = Send Later

composer-send-later-2 = SEND LATER

composer-no-templates-yet-write-a-message = No templates yet. Write a message you send often, then save it here.

composer-save-this-message-as-a-template = Save This Message as a Template

composer-write-something-first-then-save-it = Write something first, then save it as a template

composer-template = Template

composer-attach = Attach

composer-save-draft = Save draft

composer-move-into-main-window = Move into main window

composer-restore = Restore

composer-minimize = Minimize

composer-exit-full-size = Exit full size

composer-full-size = Full size

composer-open-in-new-window = Open in new window

composer-pop-out-reply = Pop out reply

composer-save-and-close = Save and close

composer-new-alias-for = New { $label } alias for { $email }

composer-removed-address = Removed address

composer-this-address-was-removed-from-choose = This address was removed from { $email }. Choose another.

composer-sent-through = Sent through { $host }.

composer-sent-through-s-server = Sent through { $email }'s server.

composer-choose-an-alias = Choose an alias

composer-sends-only-as-its-aliases-settings = { $email } sends only as its aliases (Settings → Forwarding).

composer-no-longer-exists-choose-another-address = { $send_as } no longer exists. Choose another address.

composer-sent-through-from-recipients-see = Sent through { $label } from { $email }. Recipients see { $email2 }.

composer-from = From

composer-accounts = Accounts

composer-through = Through { $host }

composer-through-2 = Through { $email }

composer-aliases = { $label } aliases

composer-sends-through = Sends through { $email }

composer-through-3 = { $note } · through { $email }

composer-no-address-matches = No address matches.

composer-to = To

composer-reply-to-2 = Reply-To

composer-bold = Bold

composer-italic = Italic

composer-underline = Underline

composer-strikethrough = Strikethrough

composer-code = Code

composer-heading = Heading

composer-bulleted-list = Bulleted list

composer-numbered-list = Numbered list

composer-quote = Quote

composer-link = Link

composer-link-address = Link address

composer-remove-link = Remove link

composer-aligned-left-center = Aligned left: center

composer-centered-align-right = Centered: align right

composer-aligned-right-align-left = Aligned right: align left

composer-clear-formatting = Clear formatting

composer-no-spelling-suggestions = No suggestions

composer-learn-spelling = Learn spelling

composer-ignore-spelling = Ignore spelling

composer-cut = Cut

composer-copy = Copy

composer-paste = Paste

composer-select-all = Select all

composer-preview = Preview

composer-remove = Remove

composer-sending = Sending…

composer-draft-saved = Draft saved

composer-editing = Editing…

composer-send-later-3 = Send later…

composer-attach-files-or-drop-them-here = Attach files (or drop them here)

composer-templates = Templates

composer-formatting-on-sent-with-bold-lists = Formatting on: sent with bold, lists and links

composer-plain-text-turn-formatting-on = Plain text: turn formatting on

composer-signed-with-your-openpgp-key = Signed with your OpenPGP key

composer-sign-with-your-openpgp-key = Sign with your OpenPGP key

composer-signed-with-your-s-mime-certificate = Signed with your S/MIME certificate

composer-sign-with-your-s-mime-certificate = Sign with your S/MIME certificate

composer-normal-priority-mark-as-important = Normal priority: mark as important

composer-high-priority-mark-as-low-priority = High priority: mark as low priority

composer-low-priority-back-to-normal = Low priority: back to normal

composer-encrypted-to-each-recipients-openpgp-key = Encrypted to each recipient's OpenPGP key

composer-encrypt-with-openpgp-picacho-looks-up = Encrypt with OpenPGP (Picacho looks up recipients' keys)

composer-encrypted-to-each-recipients-s-mime = Encrypted to each recipient's S/MIME certificate

composer-encrypt-with-s-mime-recipients-must = Encrypt with S/MIME (recipients must have sent you signed mail)

composer-the-subject-is-encrypted-too-others = The subject is encrypted too (others see “...”)

composer-encrypt-the-subject-too = Encrypt the subject too

composer-using-s-mime-switch-to-openpgp = Using S/MIME: switch to OpenPGP

composer-using-openpgp-switch-to-s-mime = Using OpenPGP: switch to S/MIME

composer-sent-as-inline-pgp = Sent as inline PGP (plain text, for apps without PGP/MIME): switch to PGP/MIME

composer-sent-as-pgp-mime = Sent as PGP/MIME: switch to inline PGP for apps that read only that

composer-discard-draft = Discard draft

composer-new-message-2 = New message

composer-slash-heading-1 = Heading 1

composer-slash-heading-1-detail = A large section title

composer-slash-heading-2 = Heading 2

composer-slash-heading-2-detail = A section title

composer-slash-heading-3 = Heading 3

composer-slash-heading-3-detail = A smaller title

composer-slash-bulleted-list = Bulleted list

composer-slash-bulleted-list-detail = A list with bullets

composer-slash-numbered-list = Numbered list

composer-slash-numbered-list-detail = A list with numbers

composer-slash-quote = Quote

composer-slash-quote-detail = Text set apart as quoted

composer-slash-code-block = Code block

composer-slash-code-block-detail = Text in a fixed-width font, as typed

composer-slash-table = Table

composer-slash-table-detail = A grid of 2 rows and 3 columns

composer-slash-divider = Divider

composer-slash-divider-detail = A line across the message

composer-slash-emoji = Emoji

composer-slash-emoji-detail = Choose an emoji

composer-slash-date = Today’s date

composer-slash-date-detail = The date, written out

composer-slash-time = The time

composer-slash-time-detail = The time now

composer-slash-no-match = No commands match

composer-slash-template = Template

composer-find = Find

composer-replace-with = Replace with

composer-replaced = { $count ->
        [one] Replaced 1 match
       *[other] Replaced { $count } matches
    }

composer-no-matches = No matches

composer-match-of = { $current } of { $count }

composer-previous-match = Previous

composer-next-match = Next

composer-match-case = Match case

composer-replace = Replace

composer-replace-all = Replace all

composer-close-find = Close find

composer-find-and-replace = Find and Replace

composer-find-an-emoji = Find an emoji

composer-recently-used = Recently used

composer-no-emoji-found = No emoji found

composer-insert-emoji = Insert emoji

composer-text-style = Text color, font and size

composer-automatic-color = Automatic

composer-no-highlight = No highlight

composer-default-font = The message’s font

composer-normal-size = Normal

composer-text-color = Text color

composer-highlight = Highlight

composer-size = Size

composer-font = Font

composer-table = Table

composer-insert-table = Insert table

composer-table-size = { $rows } × { $columns } table

composer-insert-row-above = Insert row above

composer-insert-row-below = Insert row below

composer-insert-column-left = Insert column left

composer-insert-column-right = Insert column right

composer-widen-column = Widen column

composer-narrow-column = Narrow column

composer-equal-columns = Equal column widths

composer-delete-row = Delete row

composer-delete-column = Delete column

composer-delete-table = Delete table

composer-drag-column-edges = Drag the edges between columns to resize them.

composer-pictures-size = Pictures: { $size }

composer-pictures-size-tip = How large pictures are sent: smaller ones go faster and fill less of everyone’s mailbox. Click to change.

composer-too-large-encrypted = These attachments may be too large to send, and an encrypted message can’t share them as links.

composer-too-large-shared = Too large to send by mail: the largest files go as links from { $host }.

composer-too-large-set-up-cloud = These attachments may be too large to send. Set up cloud storage in Settings → Composing to share them as links.

composer-more = More

composer-mail-merge = Mail merge

composer-mail-merge-2 = Mail merge…

composer-close-mail-merge = Close mail merge

composer-merge-explained = Send this message to each person in a spreadsheet saved as CSV. Put a column’s placeholder in the subject or text, and each message gets that person’s value.

composer-merge-choose = Choose a CSV file…

composer-merge-other-file = Other file…

composer-merge-file = { $file }: { $count ->
        [one] 1 row
       *[other] { $count } rows
    }

composer-merge-addresses-from = Addresses from the column

composer-merge-next-column = Use the next column

composer-merge-placeholders = Placeholders (put where the cursor is)

composer-merge-preview = Message { $current } of { $count }

composer-merge-to = To: { $to }

composer-merge-skipped = { $count ->
        [one] 1 row has no email address and is skipped.
       *[other] { $count } rows have no email address and are skipped.
    }

composer-merge-no-copies = Cc and Bcc aren’t used: each message goes only to its row’s address.

composer-merge-no-addresses = No row has an email address in the column chosen.

composer-merge-confirm = { $count ->
        [one] Send 1 personalized message?
       *[other] Send { $count } personalized messages?
    }

composer-merge-send = { $count ->
        [one] Send 1 message
       *[other] Send { $count } messages
    }

composer-merge-sending = Sending { $current } of { $count }…

composer-snippets = Snippets

composer-snippet = Snippet

composer-save-selection-as-snippet = Save selection as snippet

composer-select-text-for-a-snippet = Select the text to keep as a snippet first.

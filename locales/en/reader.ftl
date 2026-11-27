# src/display/reader.rs and nearby views.

# Some senders' names, and how many others there are
reader-names-and-more = { $count ->
        [one] { $names } and { $shown } more
       *[other] { $names } and { $shown } more
    }

reader-message = Message

reader-messages-selected = { $count ->
        [one] { $shown } message selected
       *[other] { $shown } messages selected
    }

reader-unread-from = { $count ->
       *[other] { $shown } unread · from { $who }
    }

reader-from = From { $who }

reader-mark-as-read = Mark as read

reader-mark-as-unread = Mark as unread

reader-unstar = Unstar

reader-star = Star

reader-archive = Archive

reader-move-to-inbox = Move to Inbox

reader-move-to-spam = Move to spam

reader-move-to-trash = Move to trash

reader-move-to-folder = Move to folder…

reader-clear-selection = Clear selection

reader-no-message-selected = No message selected

reader-message-actions = Message actions

reader-back-to-messages = @Back to messages

reader-reply = Reply

reader-reply-all = @Reply all

reader-reply-all-2 = Reply all

reader-forward = Forward

reader-open-in-new-window = @Open in new window

reader-archive-message = @Archive message

reader-snooze-until = @Snooze until…

reader-remind-me-if-no-reply = @Remind me if no reply…

reader-mark-read-unread = @Mark read / unread

reader-move-to-trash-2 = @Move to trash

reader-preview = Preview

reader-save = Save

reader-failed = Failed

reader-delayed = Delayed

reader-delivered = Delivered

reader-relayed = Relayed

reader-expanded = Expanded

reader-unknown = Unknown

reader-delivery-report = Delivery report

reader-this-message-is-encrypted = This message is encrypted

reader-enter-your-pinal-password-to-read = Enter your Pinal password to read encrypted mail on this computer. After a password reset, use your previous password or your recovery key.

reader-password-or-recovery-key = Password or recovery key

reader-unlocking = Unlocking…

reader-unlock = Unlock

reader-loading-message = Loading message…

reader-hide-details = Hide details

reader-show-details = Show details

reader-message-details = Message details

reader-checking-the-sender = Checking the sender…

reader-check-sender = Check sender

reader-looks-up-spf-dkim-and-dmarc = Looks up SPF, DKIM and DMARC; the sender's DNS can see when.

reader-couldnt-check-the-sender-no-dns = Couldn't check the sender (no DNS answer)

reader-show-raw = Show raw

reader-light-email = Light email

reader-dark-email = Dark email

reader-formatted = Formatted

reader-show-the-senders-formatting = Show the sender's formatting

reader-plain-text = Plain text

reader-show-the-plain-text-version = Show the plain text version

reader-remote-images-and-styles-load-from = Remote images and styles load from the sender's servers

reader-remote-images-are-blocked-so-senders = Remote images are blocked so senders can't tell you opened this

reader-remote-images = Remote images

reader-view-headers = View headers

reader-view-html = View HTML

reader-view-raw = View raw

reader-couldnt-verify-the-signing-key-may = Couldn't verify (the signing key may have changed since)

reader-with-domain = { $to_uppercase } with domain { $d }

reader-not-checked-use-check-sender-in = Not checked (use Check sender in the message's details)

reader-not-recorded = Not recorded

reader-message-id = Message ID

reader-created-at = Created at

reader-from-2 = From

reader-sent-by = Sent by

reader-to = To

reader-subject = Subject

reader-domain-asks-to-failures = { $verdict } (domain asks to { $p } failures)

reader-note = Note

reader-only-the-start-of-the-message = Only the start of the message is signed; text after it could have been added.

reader-security = Security

reader-copy-to-clipboard = Copy to clipboard

reader-download-raw = Download raw

reader-this-message-has-no-html-part = This message has no HTML part; it is plain text.

reader-suggested-replies = Suggested replies

reader-suggested-by-local-ai-right-click = Suggested by Local AI. Right-click to stop suggesting it.

reader-couldnt-verify-key-may-have-changed = { $name } couldn't verify (key may have changed)

reader-conversation-messages = CONVERSATION · { $count } MESSAGES

reader-to-2 = to { $to }

reader-more-messages = { $hidden } more messages

reader-to-undisclosed-recipients = to undisclosed recipients

# Who a message went to, and who got copies
reader-to-cc = to { $to } · cc { $cc }

# A message sent to undisclosed recipients, with copies to some
reader-cc-only = to undisclosed recipients · cc { $cc }

reader-this-message-has-no-text-body = This message has no text body.

reader-hide-quoted-text = Hide quoted text

reader-show-quoted-text = Show quoted text

reader-show-mail-labeled = Show mail labeled “{ $shown }”

# The reader's own address, among a message's recipients ("to Ana, me")
reader-me = me

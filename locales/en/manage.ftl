# Text from src/manage (accounts, mail, calendars, encryption): errors
# and notices shown in banners and dialogs, statuses, labels and widget
# text. Ids name the source file. Variants keyed by a word ([accepted],
# [move]…) keep their keys; plural variants ([one], [other]) take your
# language's plural forms instead.

# src/manage/applock.rs

manage-applock-a-passcode-needs-at-least-characters = A passcode needs at least { $min_passcode } characters.

manage-applock-a-passcode-cant-begin-or-end = A passcode can't begin or end with a space.

# src/manage/archive.rs

manage-archive-the-export-was-stopped = The export was stopped.

manage-archive-mail-cant-be-imported-into-this = Mail can't be imported into this mailbox. Choose the inbox, Archive or one of your folders.

manage-archive-the-import-was-stopped = The import was stopped.

manage-archive-these-messages-are-already-in-this = These messages are already in this account.

manage-archive-message-belongs-to-another-account = Message belongs to another account

manage-archive-mail-cant-be-moved-into-this = Mail can't be moved into this mailbox.

# Notices after exporting, importing or moving mail. Picacho joins the
# parts with "; ".
manage-archive-exported = Exported { $count ->
        [one] { $count } message
       *[other] { $count } messages
    }

manage-archive-imported = Imported { $count ->
        [one] { $count } message
       *[other] { $count } messages
    }

manage-archive-moved-to = Moved { $count ->
        [one] { $count } message
       *[other] { $count } messages
    } to { $name }

# Adds a second thing to say to a notice about exported or imported
# messages ("Imported 12 messages" and "2 messages couldn't be")
manage-archive-and = { $notice }; { $more }

manage-archive-skipped = { $count ->
        [one] { $count } message couldn't be
       *[other] { $count } messages couldn't be
    }

manage-archive-marked-deleted = { $count ->
        [one] { $count } marked deleted in the file was kept
       *[other] { $count } marked deleted in the file were kept
    }

# src/manage/avatar.rs

manage-avatar-this-image-could-not-be-read = This image could not be read.

# src/manage/backend.rs

manage-backend-the-server-didnt-allow-this-account = The server didn't allow this account to { $what }: { $why }

manage-backend-couldnt = Couldn't { $what }

manage-backend-sign-in-to-proton-with-its = Sign in to Proton with its own steps.

manage-backend-choose-the-same-email-address-to = Choose the same email address to reconnect this mailbox.

manage-backend-enter-your-password-or-app-password = Enter your password or app password.

manage-backend-this-sign-in-can-no-longer = This sign-in can no longer be retried. Sign in again.

manage-backend-the-server-didnt-say-which-address = The server didn't say which address signed in. Enter your email address and try again.

manage-backend-is-already-added = { $email } is already added.

manage-backend-sign-in-cancelled = Sign-in cancelled.

manage-backend-microsoft-signed-you-in-but-outlooks = Microsoft signed you in, but Outlook's mail server refused { $email }. Outlook.com turns IMAP off for new accounts: at outlook.live.com, open Settings → Mail → Forwarding and IMAP, turn on IMAP, save, then choose Retry. If you sign in to Microsoft with a different address than your Outlook address, make the Outlook address your primary alias. Work and school accounts whose IMAP is turned off can be added with “Use Microsoft Graph” instead.

manage-backend-the-account-this-one-was-shared = The account this one was shared with is gone. Remove this one.

manage-backend-the-account-was-removed = The account was removed.

manage-backend-already-sends-as = { $email } already sends as { $address }.

manage-backend-enter-the-password-for-its-outgoing = Enter the password for its outgoing server.

manage-backend-couldnt-sign-in-to = Couldn't sign in to { $host }

manage-backend-remove-first-with-this-accounts-sign = Remove { $accounts } first: { $count ->
        [one] it was shared
       *[other] they were shared
    } with this account's sign-in.

manage-backend-this-account-has-no-saved-password = This account has no saved password. Sign in again with the password to stop using Kerberos.

manage-backend-this-account-signs-in-as-sign = This account signs in as { $owner }: sign in again there.

manage-backend-this-accounts-mail-is-not-encrypted = This account's mail is not encrypted.

manage-backend-this-message-is-encrypted-remove-this = This message is encrypted. Remove this Pinal account and add it again so Picacho receives your encryption keys.

manage-backend-the-server-omitted-requested-headers-cached = The server omitted requested headers; cached mail has been kept. Refresh to retry.

manage-backend-the-server-reset-this-mailboxs-message = The server reset this mailbox's message identities. Refresh this mailbox.

manage-backend-duplicate-imap-flags-response = Duplicate IMAP flags response

manage-backend-duplicate-imap-header-response = Duplicate IMAP header response

manage-backend-this-message-isnt-from-this-pop3 = This message isn't from this POP3 account.

manage-backend-this-message-is-no-longer-on = This message is no longer on the POP3 server.

manage-backend-a-download-failed = A download failed.

manage-backend-the-original-message-is-not-available = The original message is not available offline.

manage-backend-message-no-longer-exists-on-the = Message no longer exists on the server

manage-backend-message-exceeds-the-download-limit = Message exceeds the download limit.

manage-backend-this-accounts-folders-arent-kept-on = This account's folders aren't kept on the server.

manage-backend-message-belongs-to-another-account = Message belongs to another account

manage-backend-gmail-deletes-mail-for-good-only = Gmail deletes mail for good only from Trash and Spam. Move it to Trash first.

manage-backend-only-trash-and-spam-can-be = Only Trash and Spam can be emptied.

manage-backend-the-pop3-server-didnt-confirm-deleting = The POP3 server didn't confirm deleting downloaded mail; it is checked again next time.

manage-backend-the-folder-is-no-longer-on = The folder “{ $folder }” is no longer on the server.

manage-backend-this-server-doesnt-allow-new-labels = This server doesn't allow new labels in “{ $folder }”.

manage-backend-could-not-download-messages-from-their = Could not download messages from their mailbox. Refresh that mailbox first.

manage-backend-this-sender-offers-no-way-to = This sender offers no way to unsubscribe.

manage-backend-sends-only-as-its-aliases-settings = { $email } sends only as its aliases (Settings → Forwarding). Choose an alias in From.

manage-backend-the-message-has-no-sender = The message has no sender.

manage-backend-the-server-didnt-say-where-it = The server didn't say where it filed the message.

manage-backend-the-server-didnt-keep-the-uploaded = The server didn't keep the uploaded draft.

manage-backend-a-draft-on-the-server-is = A draft on the server is encrypted to a certificate this device doesn't have.

manage-backend-a-draft-on-the-server-couldnt = A draft on the server couldn't be decrypted.

manage-backend-a-draft-on-the-server-is-2 = A draft on the server is encrypted to a key this device doesn't have.

manage-backend-open-this-message-first = Open this message first

manage-backend-attachment-not-found = Attachment not found

manage-backend-the-folder-no-longer-exists-on = The folder “{ $name }” no longer exists on the server.

manage-backend-the-account-it-was-shared-with = the account it was shared with

manage-backend-make-folders = make folders

manage-backend-create = create “{ $label }”

manage-backend-rename-the-folder = rename the folder

manage-backend-rename = rename “{ $label }”

manage-backend-delete-the-folder = delete the folder

manage-backend-delete = delete “{ $label }”

manage-backend-delete-messages = delete messages

manage-backend-put-messages = put messages

manage-backend-move-messages-out = move messages out

manage-backend-change-labels = change labels

manage-backend-change-messages = change messages

manage-backend-remove-messages = remove messages

manage-backend-mark-messages-read = mark messages read

manage-backend-star-messages = star messages

manage-backend-a-copy-wasnt-saved-in-sent = A copy wasn't saved in Sent: { $error }

manage-backend-a-copy-couldnt-be-saved-in = A copy couldn't be saved in Sent yet ({ $error }); Picacho will try again with the next mail check.

manage-backend-its-larger-than-the-server-keeps = it's larger than the server keeps ({ $kb } KB)

manage-backend-it-was-encrypted-to-keys-found = { $count ->
        [one] It was encrypted to a key found automatically for { $addresses }. Compare its fingerprint with its owner in Settings → Encryption.
       *[other] It was encrypted to keys found automatically for { $addresses }. Compare each fingerprint with their owners in Settings → Encryption.
    }

manage-backend-it-may-bounce-at = { $count ->
        [one] It may bounce at { $domains }: encryption all the way was required, and that domain publishes no MTA-STS policy or DANE records for servers to insist on it by.
       *[other] It may bounce at { $domains }: encryption all the way was required, and those domains publish no MTA-STS policy or DANE records for servers to insist on it by.
    }

manage-backend-cant-request-delivery-receipts = { $host } can't request delivery receipts, so none were asked for.

# src/manage/background.rs

manage-background-its-account-was-removed = Its account was removed.

# src/manage/calendar.rs

manage-calendar-the-calendar-servers-password-is-missing = The calendar server's password is missing. Set it up again.

manage-calendar-no-calendar-server-is-set-up = No calendar server is set up.

manage-calendar-this-jmap-server-doesnt-say-which = This JMAP server doesn't say which of your addresses it sends invitations as, so Picacho keeps emailing them.

manage-calendar-this-calendar-server-doesnt-send-invitations = This calendar server doesn't send invitations itself (it doesn't offer calendar-auto-schedule), so Picacho keeps emailing them.

manage-calendar-only-jmap-accounts-keep-calendars-this = Only JMAP accounts keep calendars this way.

manage-calendar-the-account-your-calendars-come-from = The account your calendars come from was removed. Set up a calendar again.

manage-calendar-not-a-jmap-calendar = Not a JMAP calendar.

manage-calendar-give-the-calendar-a-name = Give the calendar a name.

manage-calendar-set-up-a-calendar-server-in = Set up a calendar server in Settings → Calendar first.

manage-calendar-this-task-is-no-longer-on = This task is no longer on your list. Read it again.

manage-calendar-give-the-task-a-name = Give the task a name.

manage-calendar-none-of-your-calendars-can-take = None of your calendars can take new tasks.

manage-calendar-this-jmap-server-doesnt-keep-tasks = This JMAP server doesn't keep tasks.

# The name of a task list Picacho makes where the account has none.
manage-calendar-tasks = Tasks

manage-calendar-add-someone-to-see-when-theyre = Add someone to see when they're free.

manage-calendar-attach-files-to-events-on-a = Attach files to events on a CalDAV calendar.

manage-calendar-this-event-is-no-longer-in = This event is no longer in your calendar. Read it again.

manage-calendar-attachments-are-kept-on-caldav-calendars = Attachments are kept on CalDAV calendars.

manage-calendar-give-the-event-a-title = Give the event a title.

manage-calendar-the-event-ends-before-it-starts = The event ends before it starts.

manage-calendar-none-of-your-calendars-can-take-2 = None of your calendars can take new events.

manage-calendar-this-message-has-no-invitation = This message has no invitation.

manage-calendar-this-event-was-cancelled = This event was cancelled.

manage-calendar-this-invitation-doesnt-say-who-to = This invitation doesn't say who to answer.

manage-calendar-this-message-has-no-answer-to = This message has no answer to an invitation.

manage-calendar-this-message-isnt-an-answer-to = This message isn't an answer to an invitation.

manage-calendar-the-answer-doesnt-say-who-its = The answer doesn't say who it's from.

manage-calendar-this-answer-is-for-but-came = This answer is for { $email }, but came from { $from }; it wasn't applied.

manage-calendar-the-event-this-answers-isnt-in = The event this answers isn't in your calendar.

manage-calendar-isnt-a-guest-of-this-event = { $email } isn't a guest of this event.

manage-calendar-this-message-proposes-no-time = This message proposes no time.

manage-calendar-this-proposal-is-for-but-came = This proposal is for { $guest }, but came from { $from }.

manage-calendar-this-proposal-isnt-from-a-guest = This proposal, from { $from }, isn't from a guest of the event; it wasn't applied.

manage-calendar-the-event-this-proposal-is-for = The event this proposal is for isn't in your calendar.

manage-calendar-the-proposal-doesnt-say-who-its = The proposal doesn't say who it's from.

manage-calendar-this-update-is-from-not-the = This update is from { $from }, not the event's organizer { $email }; it wasn't applied.

manage-calendar-sender-isnt-vouched-for = { $from }'s domain doesn't vouch for this message (DMARC: { $result }), so it could be from anyone. Your calendar wasn't changed.

manage-calendar-this-event-is-already-in-your-calendar = This event is already in your calendar.

manage-calendar-this-update-would-replace-your-own = This update is for an event of your own, not one you were invited to; it wasn't applied.

manage-calendar-this-update-is-older-than-the = This update is older than the event in your calendar; it wasn't applied.

manage-calendar-this-message-asks-for-no-event = This message asks for no event.

manage-calendar-the-event-they-ask-about-isnt = The event they ask about isn't in your calendar.

manage-calendar-isnt-a-guest-of-this-event-2 = { $from } isn't a guest of this event.

manage-calendar-this-message-adds-nothing = This message adds nothing.

manage-calendar-the-event-this-adds-to-isnt = The event this adds to isn't in your calendar; ask the organizer for it.

manage-calendar-this-addition-isnt-from-the-events = This addition isn't from the event's organizer; it wasn't applied.

manage-calendar-this-cancellation-isnt-from-the-events = This cancellation isn't from the event's organizer; it wasn't applied.

manage-calendar-this-event-is-no-longer-in-2 = This event is no longer in your calendar.

manage-calendar-only-the-organizer-sends-the-invitations = Only the organizer sends the invitations.

manage-calendar-only-the-organizer-can-send-this = Only the organizer can send this event's invitations.

manage-calendar-this-event-has-no-guests-to = This event has no guests to invite.

manage-calendar-isnt-a-valid-address = “{ $to }” isn't a valid address.

manage-calendar-the-answer-was-sent-but-the = The answer was sent, but the calendar wasn't changed: { $error }

# A guest's answer to your event, from the calendar server.
manage-calendar-guest-answered = { $answer ->
        [accepted] { $who } accepted { $event }.
        [declined] { $who } declined { $event }.
        [tentative] { $who } may come to { $event }.
       *[other] { $who } answered { $event }.
    }

manage-calendar-invited-you = { $who } invited you to { $event }.

manage-calendar-someone-invited-you = Someone invited you to { $event }.

manage-calendar-was-cancelled = { $event } was cancelled.

# src/manage/composing.rs

manage-composing-later-today = Later today

manage-composing-tomorrow-morning = Tomorrow morning

manage-composing-tomorrow-afternoon = Tomorrow afternoon

manage-composing-monday-morning = Monday morning

manage-composing-this-evening = This evening

manage-composing-tomorrow = Tomorrow

manage-composing-this-weekend = This weekend

manage-composing-next-week = Next week

manage-composing-today-at = today at { $time }

manage-composing-tomorrow-at = tomorrow at { $time }

manage-composing-at = { $day } at { $time }

manage-composing-later = later

# src/manage/connection.rs

manage-connection-this-connection-cant-send-messages-that = This connection can't send messages that way.

manage-connection-the-server-cant-authorize-urls-no = The server can't authorize URLs (no URLAUTH).

manage-connection-this-build-cant-read-the-servers = This build can't read the server's URL authorizations.

manage-connection-the-server-didnt-authorize-the-url = The server didn't authorize the URL.

manage-connection-the-server-has-no-folder-and = The server has no { $label } folder and didn't make one.

manage-connection-this-account-doesnt-have-permission-to = This account doesn't have permission to { $what } in “{ $label }” (the server gives it “{ $rights }”, without “{ $missing }”). Ask the mailbox's owner or administrator.

manage-connection-can-only-be-read-on-the = “{ $folder }” can only be read on the server, so Picacho can't { $what } there. Another mail app or the server's settings may be holding it.

manage-connection-other-messages-in-this-folder-are = Other messages in this folder are already marked for deletion, and this server can't remove only some (no UIDPLUS). Remove or unmark them in another mail app first.

manage-connection-cached-message-identity-is-invalid-refresh = Cached message identity is invalid. Refresh this mailbox.

manage-connection-the-server-reset-this-mailboxs-message = The server reset this mailbox's message identities. Refresh before opening or changing this message.

manage-connection-the-mailbox-is-full-the-servers = The mailbox is full: the server's storage quota is used up. Delete or archive mail to make room.

manage-connection-remove-messages = remove messages

manage-connection-message-from-your-mail-server = Message from your mail server

# src/manage/contacts.rs

manage-contacts-the-contacts-servers-password-is-missing = The contacts server's password is missing. Set it up again.

manage-contacts-only-jmap-accounts-keep-contacts-this = Only JMAP accounts keep contacts this way.

manage-contacts-the-account-your-contacts-come-from = The account your contacts come from was removed. Set up contacts again.

manage-contacts-give-the-address-book-a-name = Give the address book a name.

manage-contacts-set-up-a-contacts-server-in = Set up a contacts server in Settings → Contacts first.

manage-contacts-no-contacts-server-is-set-up = No contacts server is set up.

manage-contacts-this-servers-contacts-are-all-read = This server's contacts are all read already.

manage-contacts-not-a-jmap-address-book = Not a JMAP address book.

manage-contacts-give-the-contact-a-name-or = Give the contact a name or an email address.

manage-contacts-isnt-a-valid-email-address = “{ $email }” isn't a valid email address.

manage-contacts-is-too-large-to-be-an = { $path } is too large to be an address book.

manage-contacts-no-contacts-were-found-in-that = No contacts were found in that file.

# src/manage/crl_files.rs

manage-crl-files-that-isnt-a-certificate-revocation-list = That isn't a certificate revocation list (.crl or .pem).

manage-crl-files-theres-no-such-revocation-list = There's no such revocation list.

manage-crl-files-its-issuers-certificate-isnt-here = { $who }: its issuer's certificate isn't here

manage-crl-files-revoked-s-certificate-has-been-revoked = Revoked: { $who }'s certificate has been revoked.

manage-crl-files-revocation-couldnt-be-checked = Revocation couldn't be checked ({ $reasons }).

manage-crl-files-not-revoked-checked-with-the-certificates = Not revoked: checked with the certificate's issuer.

# src/manage/crypto.rs

manage-crypto-these-key-settings-are-not-allowed = These key settings are not allowed.

manage-crypto-the-password-slot-has-no-key = The password slot has no key settings.

manage-crypto-bad-salt = Bad salt.

manage-crypto-check-the-recovery-key-it-has = Check the recovery key. It has 56 letters and numbers.

manage-crypto-a-mail-key-could-not-be = A mail key could not be read.

manage-crypto-a-mail-keys-public-half-could = A mail key's public half could not be read.

manage-crypto-a-mail-key-does-not-match = A mail key does not match its public key.

manage-crypto-this-message-is-too-large-to = This message is too large to open.

manage-crypto-this-mail-is-encrypted-enter-your = This mail is encrypted. Enter your Pinal password to read it on this device.

manage-crypto-that-does-not-unlock-your-mail = That does not unlock your mail. If you changed your Pinal password recently, use your previous password or your recovery key.

# src/manage/dav_accounts.rs

manage-dav-accounts-the-mail-account-your-calendars-or = The mail account your calendars or contacts use was removed. Choose another in Settings.

manage-dav-accounts-this-accounts-provider-doesnt-offer-calendars = This account's provider doesn't offer calendars to other apps.

manage-dav-accounts-this-accounts-provider-doesnt-offer-contacts = This account's provider doesn't offer contacts to other apps.

manage-dav-accounts-this-accounts-address-has-no-domain = This account's address has no domain to find a server by.

# src/manage/extras.rs

manage-extras-the-sender-didnt-ask-for-a = The sender didn't ask for a read receipt.

manage-extras-the-read-receipts-address-isnt-valid = The read receipt's address “{ $to }” isn't valid.

manage-extras-message-belongs-to-another-account = Message belongs to another account

manage-extras-this-message-is-too-large-to = This message is too large to forward whole. Forward it inline, with its attachments.

manage-extras-this-mail-service-doesnt-offer-server = This mail service doesn't offer server filters (ManageSieve).

manage-extras-only-imap-mailboxes-have-access-lists = Only IMAP mailboxes have access lists here.

manage-extras-this-server-doesnt-share-mailboxes-no = This server doesn't share mailboxes (no ACL).

# src/manage/forwarding.rs

manage-forwarding-proton-aliases-use-the-accounts-sign-in = Proton's aliases (hide-my-email aliases, kept in Proton Pass) use the Proton account's own sign-in: no API key.

manage-forwarding-proton-address = Proton address

manage-forwarding-premium-address = Premium address

manage-forwarding-external-address = External address

manage-forwarding-proton-aliases-need-a-proton-account = { $email } isn't signed in to Proton directly, so its Proton aliases can't be listed here.

manage-forwarding-reading-the-aliass-activity-failed = Reading the alias's activity failed

manage-forwarding-is-turned-off-at-turn-it = { $email } is turned off at { $label }. Turn it on in Forwarding settings, or choose another address in From.

manage-forwarding-is-not-an-email-address = { $alias } is not an email address

manage-forwarding-is-not-an-email-address-2 = { $recipient } is not an email address

manage-forwarding-mailboxes = Mailboxes

manage-forwarding-recipients = Recipients

manage-forwarding-make-an-api-key-in-simplelogin = Make an API key in SimpleLogin under API Keys.

manage-forwarding-make-an-api-access-token-in = Make an API access token in addy.io under Settings, API.

manage-forwarding-copy-the-api-key-from-the = Copy the API key from the bottom of Firefox Relay's settings.

manage-forwarding-your-domain = Your domain

manage-forwarding-your-subdomain = Your subdomain

manage-forwarding-premium = Premium

manage-forwarding-free = Free

manage-forwarding-random-words = Random Words

manage-forwarding-random-characters = Random Characters

manage-forwarding-no-longer-exists-at = { $email } no longer exists at { $service }.

manage-forwarding-is-no-longer-an-alias-picacho = { $email } is no longer an alias Picacho can send as.

# src/manage/jmap.rs

manage-jmap-this-account-has-no-folder = This account has no { $label } folder.

manage-jmap-the-folder-no-longer-exists-on = The folder “{ $name }” no longer exists on the server.

manage-jmap-mail-cant-be-moved-to = Mail can't be moved to { $label }.

manage-jmap-only-jmap-accounts-are-shared-this = Only JMAP accounts are shared this way.

manage-jmap-only-a-jmap-accounts-own-sign = Only a JMAP account's own sign-in has accounts shared with it.

manage-jmap-this-shared-account-is-already-added = This shared account is already added.

manage-jmap-this-message-isnt-from-this-jmap = This message isn't from this JMAP account.

manage-jmap-add-at-least-one-recipient = Add at least one recipient.

manage-jmap-the-server-has-no-sending-identity = The server has no sending identity for { $from }. Add it in your mail service's settings.

manage-jmap-the-message-has-no-sender = The message has no sender.

manage-jmap-the-message-went-to-its-other = The message went to its other recipients, but not to { $address }.

manage-jmap-only-jmap-servers-hold-messages-this = Only JMAP servers hold messages this way.

manage-jmap-only-jmap-servers-keep-threads = Only JMAP servers keep threads.

manage-jmap-only-jmap-servers-report-this = Only JMAP servers report this.

manage-jmap-only-jmap-servers-keep-identities = Only JMAP servers keep identities.

manage-jmap-this-message-has-no-conversation-on = This message has no conversation on the server.

manage-jmap-only-jmap-accounts-copy-between-themselves = Only JMAP accounts copy between themselves on the server.

manage-jmap-these-accounts-dont-share-a-sign = These accounts don't share a sign-in, so the server can't copy between them.

manage-jmap-the-account-to-copy-from-isnt = The account to copy from isn't known.

manage-jmap-attachment-not-found = Attachment not found

manage-jmap-enter-a-folder-name-without = Enter a folder name without “/”.

manage-jmap-only-jmap-folders-are-moved-this = Only JMAP folders are moved this way.

manage-jmap-a-folder-cant-go-inside-itself = A folder can't go inside itself.

manage-jmap-you-may-not-make-folders-inside = You may not make folders inside “{ $folder }”.

manage-jmap-only-jmap-folders-are-subscribed-to = Only JMAP folders are subscribed to this way.

manage-jmap-only-jmap-folders-are-listed-this = Only JMAP folders are listed this way.

manage-jmap-this-jmap-server-has-no-server = This JMAP server has no server filters, only an away reply.

manage-jmap-this-jmap-server-doesnt-offer-server = This JMAP server doesn't offer server filters (JMAP for Sieve Scripts).

# src/manage/large.rs

manage-large-invalid-download-folder = Invalid download folder

manage-large-this-message-is-larger-than-picacho = This message is larger than Picacho downloads whole.

manage-large-message-no-longer-exists-on-the = Message no longer exists on the server

manage-large-this-attachment-is-larger-than-picacho = This attachment is larger than Picacho downloads (1 GB).

manage-large-invalid-imap-section = Invalid IMAP section

manage-large-the-message-no-longer-exists-on = The message no longer exists on the server

manage-large-this-attachment-is-too-large = This attachment is too large.

manage-large-the-attachments-encoding-is-damaged = The attachment's encoding is damaged

# src/manage/local_ai/coreml.rs

manage-local-ai-coreml-core-ml-gave-no-compiled-model = Core ML gave no compiled model

manage-local-ai-coreml-core-ml-input-has-the-wrong = Core ML input has the wrong size

manage-local-ai-coreml-core-ml-gave-no-probability = Core ML gave no probability

# src/manage/local_ai/mod.rs

manage-local-ai-the-downloaded-model-isnt-the-published = The downloaded model isn't the published one, so it wasn't installed.

manage-local-ai-the-isnt-published-yet-and-isnt = The { $model } isn't published yet, and { $path } isn't there to install it from (train it with models/{ $id }/train.py)

manage-local-ai-isnt-installed = { $label } isn't installed.

manage-local-ai-core-ml-couldnt-load-the-model = Core ML couldn't load the model

manage-local-ai-the-model-has-no-threshold = The model has no threshold

manage-local-ai-couldnt-delete = Couldn't delete { $path }

manage-local-ai-couldnt-delete-2 = Couldn't delete { $dir }

# src/manage/local_ai/replies.rs

manage-local-ai-replies-the-reply-model-has-outputs-but = The reply model has { $classes } outputs, but { $count } replies are expected

manage-local-ai-categories-the-model-has-outputs-but = The inbox category model has { $classes } outputs, but { $count } categories are expected

# src/manage/local_ai/runtime.rs

manage-local-ai-runtime-couldnt-read = Couldn't read { $path }

manage-local-ai-runtime-not-a-picacho-model = Not a Picacho model

manage-local-ai-runtime-the-models-dimensions-are-out-of = The model's dimensions are out of range

manage-local-ai-runtime-the-model-file-is-incomplete = The model file is incomplete

manage-local-ai-runtime-the-model-has-invalid-weights = The model has invalid weights

manage-local-ai-runtime-not-a-picacho-tagger-model = Not a Picacho tagger model

# src/manage/model.rs

manage-model-enter-a-valid-email-address = Enter a valid email address.

manage-model-enter-a-name-without-line-breaks = Enter a name without line breaks.

manage-model-enter-a-valid-reply-to-address = Enter a valid Reply-To address.

manage-model-enter-a-valid-outgoing-server-hostname = Enter a valid outgoing server hostname.

manage-model-ports-must-be-between-1-and = Ports must be between 1 and 65535.

manage-model-enter-a-sign-in-name-without = Enter a sign-in name without line breaks.

manage-model-enter-valid-incoming-and-smtp-server = Enter valid incoming and SMTP server hostnames.

manage-model-browser-sign-in-isnt-set-up = Browser sign-in isn't set up for { $label } in this build.

manage-model-oauth-is-available-for-pinal-mail = OAuth is available for Pinal Mail, Gmail, Outlook, JMAP servers, and IMAP, POP3 and Exchange servers with an organization's sign-in.

manage-model-single-sign-on-needs-browser-sign = Single sign-on needs browser sign-in.

manage-model-the-gmail-api-and-microsoft-graph = The Gmail API and Microsoft Graph are for Gmail and Outlook accounts with browser sign-in.

manage-model-single-sign-on-is-for-imap = Single sign-on is for IMAP, POP3 and Exchange Server accounts.

manage-model-enter-the-exchange-server-to-sign-in = Enter the Exchange server to sign in with your organization: the sign-in is for that server.

manage-model-choose-kerberos-or-single-sign-on = Choose Kerberos or single sign-on, not both.

manage-model-enter-the-sign-in-services-address = Enter the sign-in service's address (its issuer URL).

manage-model-the-sign-in-services-address-must = The sign-in service's address must start with https://.

manage-model-the-sign-in-scopes-cant-contain = The sign-in scopes can't contain line breaks.

manage-model-starttls-is-only-used-for-imap = STARTTLS is only used for IMAP and POP3.

manage-model-the-sign-in-name-cant-contain = The sign-in name can't contain line breaks.

manage-model-kerberos-sign-in-is-for-imap = Kerberos sign-in is for IMAP and POP3 accounts.

manage-model-kerberos-sign-in-isnt-available-on = Kerberos sign-in isn't available on phones and tablets.

manage-model-basic-sign-in-is-for-exchange = Basic sign-in is for Exchange accounts that sign in with a password.

manage-model-other = Other

manage-model-api-token = API token

manage-model-app-password = app password

manage-model-app-password-2 = App password

manage-model-app-specific-password = App-specific password

manage-model-bridge-password = Bridge password

manage-model-proton-password = Proton password

manage-model-password = Password

manage-model-this-copy-of-picacho-was-built = This copy of Picacho was built without Google sign-in, so an app password is the only way to connect Gmail.

manage-model-this-copy-of-picacho-was-built-2 = This copy of Picacho was built without Microsoft sign-in, so an app password is the only way to connect Outlook.

manage-model-turn-on-2-step-verification-in = Turn on 2-Step Verification in your Google Account, then make an app password and paste it here. Spaces are ignored.

manage-model-icloud-needs-an-app-specific-password = iCloud needs an app-specific password, made in your Apple Account.

manage-model-picacho-talks-to-fastmail-over-jmap = Picacho talks to Fastmail over JMAP. Make an API token with access to mail (and to sending and filters, to use them here), then paste it here.

manage-model-turn-on-imap-in-zoho-mail = Turn on IMAP in Zoho Mail settings, then make an app password.

manage-model-open-proton-mail-bridge-choose-your = Open Proton Mail Bridge, choose your account and copy its mailbox password. Bridge must be running, with its certificate trusted.

manage-model-picacho-signs-in-to-proton-directly = Picacho signs in to Proton directly; Proton Mail Bridge isn't needed. Mail is decrypted on this computer with your keys, which never leave it.

manage-model-turn-on-two-step-verification-in = Turn on two-step verification in your Microsoft account, then make an app password and paste it here. Work and school accounts can use one only if their admin allows it.

manage-model-inbox = Inbox

manage-model-starred = Starred

manage-model-sent = Sent

manage-model-drafts = Drafts

manage-model-archive = Archive

manage-model-spam = Spam

manage-model-trash = Trash

manage-model-folder = Folder

manage-model-all-mail = All Mail

# src/manage/openpgp.rs

manage-openpgp-the-openpgp-keyring-couldnt-be-read = The OpenPGP keyring couldn't be read.

manage-openpgp-paste-or-choose-an-armored-openpgp = Paste or choose an armored OpenPGP key.

manage-openpgp-that-secret-key-couldnt-be-read = That secret key couldn't be read.

manage-openpgp-that-keys-signatures-dont-check-out = That key's signatures don't check out.

manage-openpgp-that-public-key-couldnt-be-read = That public key couldn't be read.

manage-openpgp-no-openpgp-keys-were-found-keys = No OpenPGP keys were found. Keys start with -----BEGIN PGP PUBLIC KEY BLOCK----- or -----BEGIN PGP PRIVATE KEY BLOCK-----.

manage-openpgp-no-usable-key-for-was-found = No usable key for { $email } was found.

manage-openpgp-paste-the-whole-fingerprint-40-characters = Paste the whole fingerprint (40 characters); a short key id can be faked.

manage-openpgp-that-fingerprint-doesnt-match-this-key = That fingerprint doesn't match this key. Don't use the key until you know why: it may not be theirs.

manage-openpgp-this-key-has-a-passphrase-enter = This key has a passphrase. Enter it to import the key.

manage-openpgp-the-passphrase-is-wrong = The passphrase is wrong.

manage-openpgp-the-key-couldnt-be-unlocked = The key couldn't be unlocked.

manage-openpgp-a-subkey-couldnt-be-unlocked = A subkey couldn't be unlocked.

manage-openpgp-theres-no-openpgp-key-of-yours = There's no OpenPGP key of yours for { $from }. Import one in Settings → Encryption, or turn off Sign.

manage-openpgp-theres-no-openpgp-key-for-import = There's no OpenPGP key for { $addresses }. Import theirs in Settings → Encryption, or turn off Encrypt.

manage-openpgp-inline-pgp-sends-plain-text-only = Inline PGP sends plain text only. Turn off formatting, or switch to PGP/MIME.

manage-openpgp-enter-the-address-the-key-is = Enter the address the key is for.

manage-openpgp-the-key-couldnt-be-made = The key couldn't be made.

manage-openpgp-this-isnt-an-autocrypt-setup-message = This isn't an Autocrypt Setup Message.

manage-openpgp-enter-the-setup-codes-36-digits = Enter the setup code's 36 digits.

manage-openpgp-the-setup-message-cant-be-read = The setup message can't be read.

manage-openpgp-that-setup-code-doesnt-open-it = That setup code doesn't open it.

manage-openpgp-warning-the-sender-inside-the-message = Warning: the sender inside the message ({ $protected }) isn't the sender shown ({ $outer }), and no signature proves it

manage-openpgp-not-signed = Not signed

manage-openpgp-files-not-signed = Warning: the signature doesn't cover these files, which anyone could have added: { $files }

manage-openpgp-files-not-encrypted = Warning: these files weren't encrypted with the message, and anyone could have added them: { $files }

manage-openpgp-an-unnamed-file = an unnamed file

manage-openpgp-signed-by = Signed by { $who }

manage-openpgp-signed-by-who-isnt-the-sender = Signed by { $who }, who isn't the sender shown

manage-openpgp-signed-by-a-key-you-havent = Signed, by a key you haven't imported

manage-openpgp-the-signature-doesnt-match-the-message = The signature doesn't match: the message may have been changed

# src/manage/openpgp/keys.rs

manage-openpgp-keys-the-revocation-couldnt-be-made = The revocation couldn't be made.

manage-openpgp-keys-a-self-signature-on-the-key = A self-signature on the key can't be read.

manage-openpgp-keys-choose-a-time-between-a-second = Choose a time between a second and 136 years after the key was made.

manage-openpgp-keys-the-key-couldnt-be-re-signed = The key couldn't be re-signed.

manage-openpgp-keys-this-key-has-no-self-signature = This key has no self-signature to set its expiry on.

manage-openpgp-keys-that-isnt-one-of-your-keys = That isn't one of your keys.

# src/manage/organize.rs

manage-organize-from = From

manage-organize-to-or-cc = To or Cc

manage-organize-subject = Subject

manage-organize-anywhere = Anywhere

manage-organize-mark-as-read = Mark as read

manage-organize-star = Star

manage-organize-archive = Archive

manage-organize-move-to-spam = Move to Spam

manage-organize-move-to-trash = Move to Trash

manage-organize-move-to = Move to “{ $folder }”

manage-organize-label = Label “{ $label }”

# src/manage/parcels/mod.rs

manage-parcels-order = Order { $order }

manage-parcels-ordered = Ordered

manage-parcels-shipped = Shipped

manage-parcels-in-transit = In transit

manage-parcels-out-for-delivery = Out for delivery

manage-parcels-ready-for-pickup = Ready for pickup

manage-parcels-delivered = Delivered

manage-parcels-delayed-or-a-problem = Delayed or a problem

manage-parcels-returned = Returned

manage-parcels-cancelled = Cancelled

# src/manage/parcels/summary.rs

manage-parcels-summary-probably-delivered = Probably delivered

# src/manage/proton.rs

manage-proton-the-proton-account-its-aliases-come = The Proton account these aliases come from is no longer in Picacho.

manage-proton-choose-one-of-the-accounts-folders = Choose one of the account's folders.

manage-proton-the-folder-no-longer-exists-at = The folder “{ $name }” no longer exists at Proton.

manage-proton-enter-your-proton-password = Enter your Proton password.

manage-proton-sign-in-cancelled = Sign-in cancelled.

manage-proton-this-sign-in-has-ended-start = This sign-in has ended. Start again with your password.

manage-proton-that-proton-account-doesnt-have-sign = That Proton account doesn't have { $email }. Sign in to the account it belongs to.

manage-proton-is-already-added = { $email } is already added.

manage-proton-this-message-isnt-from-proton = This message isn't from Proton.

manage-proton-message-exceeds-the-download-limit = Message exceeds the download limit.

manage-proton-proton-doesnt-move-mail-to = Proton doesn't move mail to { $label }.

manage-proton-a-draft-at-proton-is-too = A draft at Proton is too large to open here.

manage-proton-add-at-least-one-recipient = Add at least one recipient.

# src/manage/remote_search.rs

manage-remote-search-pop3-servers-cant-search-only-mail = POP3 servers can't search; only mail on this computer is searched.

manage-remote-search-drafts-are-searched-on-this-computer = Drafts are searched on this computer.

manage-remote-search-type-something-to-search-for = Type something to search for.

# src/manage/smime.rs

manage-smime-the-s-mime-certificates-couldnt-be = The S/MIME certificates couldn't be read.

manage-smime-that-isnt-a-pkcs-12-file = That isn't a PKCS#12 file (.p12 or .pfx).

manage-smime-the-password-is-wrong-or-the = The password is wrong, or the file is damaged.

manage-smime-the-file-has-no-certificate = The file has no certificate.

manage-smime-the-file-has-no-private-key = The file has no private key.

manage-smime-the-files-key-doesnt-belong-to = The file's key doesn't belong to its certificate.

manage-smime-the-certificate-names-no-email-address = The certificate names no email address.

manage-smime-that-isnt-a-certificate-pem-crt = That isn't a certificate (.pem, .crt or .cer).

manage-smime-theres-no-s-mime-certificate-of = There's no S/MIME certificate of yours for { $from }. Import one in Settings → Encryption, or turn off Sign.

manage-smime-theres-no-s-mime-certificate-for = There's no S/MIME certificate for { $addresses }. They need to send you signed mail first, or you can import theirs in Settings → Encryption.

manage-smime-the-message-couldnt-be-encrypted-with = The message couldn't be encrypted with S/MIME.

manage-smime-a-revocation-lists-path-cant-be = A revocation list's path can't be read: { $file }

manage-smime-isnt-a-revocation-list-pem = { $file } isn't a revocation list (PEM).

manage-smime-the-signers-certificate-names-no-address = The signer's certificate names no address.

manage-smime-not-signed = Not signed

manage-smime-signed-by-s-mime = Signed by { $who } (S/MIME)

manage-smime-signed-by-s-mime-with-a = Signed by { $who } (S/MIME), with a certificate no trusted authority vouches for

manage-smime-signed-by-s-mime-who-isnt = Signed by { $who } (S/MIME), who isn't the sender shown

manage-smime-signed-by-s-mime-whose-certificate = Signed by { $who } (S/MIME), whose certificate has been revoked

manage-smime-signed-with-s-mime-by-a = Signed with S/MIME, by a certificate the message doesn't include: it couldn't be checked

manage-smime-the-s-mime-signature-doesnt-match = The S/MIME signature doesn't match: the message may have been changed

manage-smime-an-unknown-signer = an unknown signer

# src/manage/smime/revocation.rs

manage-smime-revocation-isnt-a-valid-address = { $url } isn't a valid address.

manage-smime-revocation-revocation-is-fetched-over-http-only = Revocation is fetched over HTTP only, not { $scheme }.

manage-smime-revocation-names-no-server = { $url } names no server.

manage-smime-revocation-is-on-a-local-network = { $url } is on a local network.

manage-smime-revocation-the-revocation-servers-answer-is-too = The revocation server's answer is too large.

manage-smime-revocation-answered = { $url } answered { $status }.

manage-smime-revocation-couldnt-fetch = Couldn't fetch { $url }.

manage-smime-revocation-couldnt-ask = Couldn't ask { $url }.

manage-smime-revocation-the-revocation-servers-answer-isnt-a = The revocation server's answer isn't a revocation list.

manage-smime-revocation-its-certificate-names-no-revocation-list = Its certificate names no revocation list.

manage-smime-revocation-its-revocation-list-hasnt-been-fetched = Its revocation list hasn't been fetched.

manage-smime-revocation-no-revocation-list-could-be-fetched = No revocation list could be fetched.

manage-smime-revocation-gave-a-list-its-issuer-didnt = { $url } gave a list its issuer didn't sign, or an outdated one.

# Why a revocation list, signed by the certificate's issuer, can't say
# whether that certificate is revoked. { $why } is one of the reasons below.
manage-smime-revocation-list-doesnt-apply = { $url } gave a list that doesn't cover this certificate: { $why }

manage-smime-revocation-list-unreadable = it couldn't be read

manage-smime-revocation-list-indirect = it is issued on another authority's behalf

manage-smime-revocation-list-delta = it lists only changes since another list

manage-smime-revocation-list-unsupported = it has a limit Picacho doesn't understand

manage-smime-revocation-list-for-other-certificates = it is for other certificates

manage-smime-revocation-list-only-some-reasons = { $url } gave a list of certificates revoked for some reasons only, which doesn't name this one.

manage-smime-revocation-the-request-couldnt-be-made = the request couldn't be made

manage-smime-revocation-the-answer-isnt-an-ocsp-response = the answer isn't an OCSP response

manage-smime-revocation-the-responder-couldnt-answer = the responder couldn't answer

manage-smime-revocation-the-answer-is-incomplete = the answer is incomplete

manage-smime-revocation-the-answer-isnt-signed-by-the = the answer isn't signed by the certificate's issuer

manage-smime-revocation-the-answer-isnt-about-this-certificate = the answer isn't about this certificate

manage-smime-revocation-the-answer-is-out-of-date = the answer is out of date

manage-smime-revocation-the-responder-doesnt-know-this-certificate = the responder doesn't know this certificate

# src/manage/follow_up.rs

manage-follow-up-in-a-day = In a day

manage-follow-up-in-days = In { $count } days

manage-follow-up-in-a-week = In a week

manage-follow-up-in-two-weeks = In two weeks

manage-follow-up-tomorrow = Tomorrow

manage-follow-up-no-message-id = This message has no Message-ID, so replies to it can't be recognized.

manage-follow-up-another-account = Message belongs to another account

manage-follow-up-choose-a-time-in-the-future = Choose a time in the future.

manage-follow-up-already-replied = { $sender } already replied.

manage-follow-up-not-noted = No reminder was set for it: { $error }

# src/manage/snooze.rs

manage-snooze-snoozing-needs-mail-kept-on-the = Snoozing needs mail kept on the server.

manage-snooze-choose-a-time-in-the-future = Choose a time in the future.

manage-snooze-choose-messages-to-snooze = Choose messages to snooze.

manage-snooze-message-belongs-to-another-account = Message belongs to another account

manage-snooze-this-message-has-no-message-id = This message has no Message-ID, so it couldn't be found again to wake it.

manage-snooze-proton-didnt-keep-the-new-folder = Proton didn't keep the new folder.

manage-snooze-the-server-didnt-keep-the-new = The server didn't keep the new folder.

manage-snooze-create-the-folder = create the “{ $label }” folder

manage-snooze-move-snoozed-mail-back = move snoozed mail back

# src/manage/status.rs

manage-status-waiting-for-your-browser = Waiting for your browser…

manage-status-signed-in-finishing-up = Signed in. Finishing up…

manage-status-connecting-to-your-mailbox = Connecting to your mailbox…

manage-status-finish-protons-check-in-the-window = Finish Proton's check in the window that opened…

manage-status-cannot-access-saved-credentials = Cannot access saved credentials

manage-status-sign-in-needs-attention = Sign-in needs attention

manage-status-proton-didnt-register-this-device-for = Proton didn't register this device for push notifications

manage-status-checking-mail = Checking mail

manage-status-downloading-messages = Downloading messages

manage-status-opening-a-message = Opening a message

manage-status-updating-messages = Updating messages

manage-status-sending-mail = Sending mail

manage-status-push-notifications = Push notifications

manage-status-this-mailbox-gets-no-new-mail = This mailbox gets no new-mail notifications until Proton accepts this device: with push notifications on, checks for new mail don't announce it. Choose Try Again, or turn Push notifications off to have checks announce new mail again.

manage-status-your-draft-has-been-kept-check = Your draft has been kept. Check your sign-in, recipient, and outgoing server settings before sending again.

manage-status-the-server-is-limiting-how-much = The server is limiting how much this account can download, which Gmail does after heavy use. Background checks and downloads pause and resume on their own; cached mail stays available, and opening a message still works.

manage-status-allow-picacho-to-access-your-system = Allow Picacho to access your system keychain, then choose Try Again. If the saved credentials are missing, sign in again. Automatic checks are paused for this mailbox.

manage-status-the-server-could-not-accept-your = The server could not accept your sign-in. Sign in again with your provider or update your app password. Automatic checks are paused for this mailbox.

manage-status-check-your-connection-and-server-settings = Check your connection and server settings, then try again. Your cached mail remains available. Automatic checks will retry on this mailbox's schedule.

manage-status-todays-background-download-allowance = Today's background download allowance for this account is used up; it resumes at { $local }

manage-status-the-server-is-limiting-this-account = The server is limiting this account; background sync resumes at { $local }

# src/manage/store.rs

manage-store-couldnt-copy-the-local-database = Couldn't copy the local database

manage-store-couldnt-replace-the-local-database = Couldn't replace the local database

manage-store-couldnt-open-the-local-database-again = Couldn't open the local database again

manage-sealed-files-couldnt-encrypt-a-file = Couldn't encrypt a draft's file.

manage-sealed-files-an-encrypted-file-is-damaged = A draft's encrypted file is damaged and can't be read.

manage-sealed-files-this-file-is-encrypted-with-a-key = This draft's file is encrypted with the local database's key, which is no longer here.

# After turning the local database's encryption on or off, when some of
# the drafts' files kept beside it couldn't be converted yet.
manage-sealed-files-some-files-couldnt-follow = { $count ->
        [one] One draft file couldn't
       *[other] { $count } draft files couldn't
    } be converted to follow the local database's encryption. Picacho keeps the key they need and will try again.

manage-sealed-files-couldnt-keep-track = Couldn't keep track of converting the drafts' files

manage-store-the-server-did-not-supply-a = The server did not supply a valid UIDVALIDITY

manage-store-invalid-mailbox-snapshot = Invalid mailbox snapshot

manage-store-couldnt-compact-the-mail-cache = Couldn't compact the mail cache

manage-store-invalid-mailbox-update = Invalid mailbox update

manage-store-couldnt-save-this-setting = Couldn't save this setting: { $why }

manage-store-couldnt-save-this-setting-the-cache = Couldn't save this setting: the cache stayed busy

manage-store-a-saved-message-source-is-damaged = A saved message source is damaged

manage-store-a-saved-message-source-is-damaged-2 = A saved message source is damaged: it inflates past { $limit } bytes

manage-store-cannot-access-the-system-keychain = Cannot access the system keychain

manage-store-cannot-access-the-system-keychain-2 = Cannot access the system keychain: { $error }

manage-store-the-keychain-entry-is-missing-locked = The keychain entry is missing, locked, or access was denied

manage-store-saved-credentials-could-not-be-read = Saved credentials could not be read

manage-store-a-part-of-a-keychain-entry = A part of a keychain entry is missing

manage-store-the-keychain-entry-is-locked-or = The keychain entry is locked, or access was denied

manage-store-could-not-securely-save-to-the = Could not securely save to the system keychain

manage-store-could-not-remove-a-keychain-entry = Could not remove a keychain entry

manage-store-could-not-securely-save-credentials-to = Could not securely save credentials to the system keychain

# src/manage/subscriptions.rs

manage-subscriptions-the-senders-request-to-unsubscribe-couldnt = The sender's request to unsubscribe couldn't be verified.

manage-subscriptions-the-senders-unsubscribe-address-isnt-valid = The sender's unsubscribe address isn't valid.

manage-subscriptions-the-senders-unsubscribe-address-is-on = The sender's unsubscribe address is on a local network.

manage-subscriptions-the-senders-redirect-had-no-address = The sender's redirect had no address.

manage-subscriptions-the-sender-redirected-the-request-to = The sender redirected the request to an address that isn't HTTPS.

manage-subscriptions-the-sender-redirected-the-request-to-2 = The sender redirected the request to an address on a local network.

manage-subscriptions-the-sender-refused-the-request = The sender refused the request ({ $status }).

manage-subscriptions-the-sender-redirected-the-request-too = The sender redirected the request too many times.

# src/manage/undo.rs

manage-undo-only-moves-of-this-accounts-mail = Only moves of this account's mail can be undone.

manage-undo-move-messages-back = move messages back

# src/manage/vault.rs

manage-vault-couldnt-move-the-unreadable-database-aside = Couldn't move the unreadable database aside

manage-vault-couldnt-read-the-local-database = Couldn't read the local database

manage-vault-couldnt-read-which-keychain-entry-has = Couldn't read which keychain entry has the database key

manage-vault-picachos-local-database-is-encrypted-and = Picacho's local database is encrypted, and the system keychain didn't give its key. Unlock the keychain, or allow Picacho to use it, and try again.

manage-vault-couldnt-save-the-database-key-in = Couldn't save the database key in the system keychain

manage-vault-the-system-keychain-didnt-keep-the = The system keychain didn't keep the database key, so the database wasn't encrypted.

manage-vault-picachos-local-database-is-encrypted-but = Picacho's local database is encrypted, but its key isn't in the system keychain. If the keychain was reset, or this folder came from another computer or account, the database can't be read.

manage-vault-picachos-local-database-couldnt-be-decrypted = Picacho's local database couldn't be decrypted with the key in the system keychain.

# src/manage/watch.rs

manage-watch-protons-events-failed = Proton's events failed

manage-watch-jmaps-events-failed = JMAP's events failed

manage-watch-idle-failed = IDLE failed

# src/manage/widgets.rs

manage-widgets-details-hidden = Details hidden

manage-widgets-event = Event

manage-widgets-package = Package

manage-widgets-refund = Refund

manage-widgets-purchase = Purchase

manage-widgets-no-title = (No title)

manage-widgets-picacho-open-to-sign-in = Picacho — open to sign in

manage-widgets-picacho-all-caught-up = Picacho — All caught up

manage-widgets-picacho-unread-messages = Picacho — { $count ->
        [one] { $count } unread message
       *[other] { $count } unread messages
    }

manage-widgets-unknown = (Unknown)

manage-widgets-no-subject = (No subject)

manage-widgets-upcoming = Upcoming:

manage-widgets-workspace = Workspace: { $name }

# src/manage/gmail.rs and src/manage/graph.rs.

manage-gmail-this-message-isnt-from-this-gmail = This message isn't from this account.

manage-gmail-gmail-doesnt-move-mail-to = Mail can't be moved to { $label } here.

manage-gmail-a-draft-at-gmail-is-too-large = A draft on the server is too large to open here.

manage-connection-this-isnt-available-for-accounts = This isn't available for { $service } accounts that use { $service }'s own API instead of IMAP.

manage-api-filters-cant-do-this = { $service }'s filters can't do this: { $what }.

manage-calendar-exchange-always-sends-invitations = Exchange always sends invitations for events with guests itself.

manage-calendar-not-an-exchange-calendar = Not an Exchange calendar.

manage-calendar-only-accounts-that-use-microsoft = Only Outlook accounts that use Microsoft Graph keep Exchange calendars here.

manage-contacts-only-accounts-that-use-microsoft = Only Outlook accounts that use Microsoft Graph keep Exchange contacts here.

manage-contacts-exchange-contacts-have-one-address-book = Exchange contacts are kept in one address book here.

manage-contacts-contacts = Contacts

manage-backend-it-went-as-inline-pgp = It went as inline PGP, each file protected on its own: Microsoft 365 can't take signed or encrypted mail this large whole.

manage-model-the-exchange-address-must-start-with = The Exchange (EWS) address must start with https://.

manage-calendar-only-exchange-accounts = Only Exchange accounts keep calendars and contacts here this way.

manage-calendar-not-a-google-calendar = That isn't one of your Google calendars.

manage-calendar-not-a-proton-calendar = That isn't one of your Proton calendars.

manage-calendar-only-google-accounts = Only Google accounts signed in with Google keep calendars and contacts here this way.

manage-calendar-only-proton-accounts = Only Proton accounts signed in without Bridge keep calendars and contacts here this way.

manage-calendar-proton-calendar-doesnt-send-invitations = Proton Calendar's invitations are sent from Proton's own apps.

manage-calendar-proton-calendars-are-made-in-proton = Proton calendars are made, renamed and deleted in Proton Calendar.

manage-calendar-subscribed-calendars-are-read-only = A subscribed calendar is read-only: its events are its publisher's.

manage-calendar-this-calendar-cant-say-when-people = This calendar can't say when people are busy.

manage-calendar-this-calendar-is-no-longer-on = This calendar is no longer there; read your calendars again.

manage-pim-sources-couldnt-read = Couldn't read { $source }

manage-organize-text = Text

manage-organize-header = Header

manage-organize-mailing-list = Mailing list

manage-organize-size-kb = Size (KB)

manage-organize-attachments = Attachments

manage-organize-category = Category

manage-organize-op-contains = contains

manage-organize-op-is = is

manage-organize-op-starts-with = starts with

manage-organize-op-ends-with = ends with

manage-organize-op-matches-pattern = matches the pattern

manage-organize-op-over = is over

manage-organize-op-under = is under

manage-organize-condition = { $field } { $op } “{ $value }”

manage-organize-condition-not = Not { $field } { $op } “{ $value }”

manage-organize-has-attachments = Has attachments

manage-organize-has-no-attachments = Has no attachments

manage-organize-and = and

manage-organize-or = or

manage-organize-rule = When { $conditions } → { $actions }

manage-organize-dont-notify = Don't notify

manage-organize-each-condition-needs-a-value = Type what the condition looks for

manage-organize-name-the-header = Name the header the condition looks at, as X-Mailer

manage-organize-enter-a-size-in-kilobytes = Enter a size in kilobytes, as 500

manage-organize-choose-a-category = Choose a category

manage-organize-the-pattern-isnt-valid = The pattern isn't a valid regular expression

manage-organize-name-the-folder-or-label = Type the folder or label's name

manage-organize-add-a-condition = Add a condition to the rule

manage-organize-add-an-action = Add an action to the rule

manage-organize-a-rule-moves-mail-once = A rule can move mail to one place only

manage-composing-image-size-actual = Actual size

manage-composing-image-size-large = Large (2048 px)

manage-composing-image-size-medium = Medium (1280 px)

manage-composing-image-size-small = Small (640 px)

manage-merge-too-many-rows = The spreadsheet has more than { $max } rows.

manage-merge-the-file-is-empty = The file is empty.

manage-merge-no-rows-below-the-headings = The file has no rows below its headings.

manage-merge-too-large = The file is too large for a mail merge.

manage-outgoing-cloud-couldnt-share = { $name } couldn’t be shared as a link

manage-outgoing-cloud-full = The cloud storage is full.

manage-outgoing-cloud-no-link = The cloud storage didn’t give a link to the file.

manage-outgoing-cloud-not-set-up = Cloud storage isn’t set up.

manage-outgoing-cloud-server-answered = The cloud storage answered with error { $status }.

manage-outgoing-cloud-sign-in-failed = The cloud storage didn’t accept the user name and password.

manage-outgoing-cloud-the-address-isnt-valid = The cloud storage’s address isn’t valid.

manage-backend-shared-as-links = { $count ->
        [one] { $names } was shared as a link.
       *[other] { $names } were shared as links.
    }

manage-outgoing-cloud-server-said = The cloud storage said: { $message }

manage-outgoing-cloud-sign-in-again = The storage didn’t accept the account’s sign-in: connect it again in Settings → Composing → Large attachments.

manage-outgoing-cloud-account-gone = The account whose storage large attachments go to was removed. Choose another in Settings → Composing.

manage-outgoing-cloud-not-this-account = This account can’t share files that way.

manage-merge-interrupted = Picacho stopped while it was being sent; it may not have gone

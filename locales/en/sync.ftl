# Text from src/sync (the protocols: IMAP, SMTP, JMAP, POP, DAV, OAuth,
# …): mostly errors, shown when signing in, syncing or sending fails, and
# some labels and prompts. Ids name the source file. Protocol names,
# commands, header names and codes in [brackets] stay as they are.
# Variants keyed by a word ([sign-in], [copy]…) keep their keys; plural
# variants ([one], [other]) take your language's plural forms instead.

# src/sync/auth.rs

sync-auth-dns-isnt-available = DNS isn't available

sync-auth-dns-didnt-answer-in-time = DNS didn't answer in time

sync-auth-s-mail-servers-dns-records-fail = { $domain }'s mail servers' DNS records fail DNSSEC validation

sync-auth-s-dane-records-fail-dnssec-validation = { $host }'s DANE records fail DNSSEC validation, so mail to { $domain } can't be delivered securely

sync-auth-not-an-address = Not an address

# src/sync/caldav.rs

sync-caldav-the-calendar-server-didnt-list-your = The calendar server didn't list your tasks.

sync-caldav-the-server-couldnt-say-when-free = The server couldn't say when { $people } { $count ->
        [one] is
       *[other] are
    } free.

sync-caldav-the-servers-answer-couldnt-be-read = The server's answer couldn't be read.

sync-caldav-the-time-asked-about-ends-before = The time asked about ends before it starts.

sync-caldav-this-calendar-takes-at-most-guests = This calendar takes at most { $most } guests an event.

sync-caldav-this-calendar-doesnt-take-events-at = This calendar doesn't take events at that date.

sync-caldav-this-calendar-takes-at-most-repeats = This calendar takes at most { $most } repeats of an event. Give the repeats an end.

sync-caldav-the-event-has-no-identifier = The event has no identifier.

sync-caldav-this-event-is-too-large-for = This event is too large for the calendar ({ $kilobytes } KB at most). Shorten its notes or remove guests.

sync-caldav-give-the-calendar-a-name = Give the calendar a name.

sync-caldav-save-the-event-before-attaching-files = Save the event before attaching files.

sync-caldav-this-calendar-server-cant-keep-attachments = This calendar server can't keep attachments.

sync-caldav-this-event-has-as-many-attachments = This event has as many attachments as the server allows ({ $most }).

sync-caldav-this-attachment-isnt-kept-by-the = This attachment isn't kept by the calendar server.

sync-caldav-this-event-has-no-resource-name = This event has no resource name.

sync-caldav-this-file-is-too-large-to-mb = This file is too large to attach to this event ({ $megabytes } MB at most).

sync-caldav-this-file-is-too-large-to-kb = This file is too large to attach to this event ({ $kilobytes } KB at most).

# src/sync/carddav.rs

sync-carddav-write-something-to-look-for = Write something to look for.

sync-carddav-give-the-address-book-a-name = Give the address book a name.

sync-carddav-no-address-book-on-the-server = No address book on the server can take new contacts.

sync-carddav-this-contact-is-too-large-for = This contact is too large for the address book ({ $kilobytes } KB at most). Use a smaller photo.

# src/sync/dav.rs

sync-dav-the-servers-answer-couldnt-be-read = The server's answer couldn't be read.

sync-dav-couldnt-reach = Couldn't reach { $host }

sync-dav-the-server-is-out-of-storage = The server is out of storage for your account. Free some space there and retry.

sync-dav-the-server-didnt-accept-the-user = The server didn't accept the user name and password.

sync-dav-this-was-changed-on-the-server = This was changed on the server meanwhile. Read it again and retry.

sync-dav-the-server-sent-picacho-to-an = The server sent Picacho to an address without encryption.

sync-dav-the-server-sent-picacho-to-outside = The server sent Picacho to { $host }, outside its domain; enter that address instead if it's right.

sync-dav-the-server-redirected-too-many-times = The server redirected too many times.

sync-dav-this-server-doesnt-say-where-your = This server doesn't say where your collections go.

sync-dav-this-server-cant-say-when-people = This server can't say when people are free (no scheduling outbox).

sync-dav-the-server-refused = The server refused { $properties }.

sync-dav-the-server-didnt-say-the-saved = The server didn't say the saved version's ETag.

sync-dav-the-server-didnt-accept-the-new = The server didn't accept the new name or colour.

sync-dav-this-server-has-no-scheduling-inbox = This server has no scheduling inbox.

sync-dav-the-server-has-no-room-for = The server has no room for this attachment.

sync-dav-the-server-didnt-change-the-attachment = The server didn't change the attachment ({ $status }).

sync-dav-a-copy-or-move-stays-on = A copy or move stays on one server.

sync-dav-the-server-didnt-all-of-it = { $action ->
        [copy] The server didn't copy all of it.
       *[move] The server didn't move all of it.
    }

sync-dav-the-server-didnt-say-the-locks = The server didn't say the lock's token.

sync-dav-the-servers-answer-is-too-large = The server's answer is too large.

sync-dav-the-servers-changes-came-without-a = The server's changes came without a sync token.

sync-dav-enter-the-servers-address = Enter the server's address.

sync-dav-the-servers-address-isnt-valid = The server's address isn't valid.

sync-dav-the-servers-address-must-start-with = The server's address must start with https://.

sync-dav-this-server-doesnt-list-any-for = This server doesn't list any for your account.

sync-dav-nothing-was-found-on-the-server = Nothing was found on the server for your account.

sync-dav-the-server-no-longer-knows-what = The server no longer knows what changed since the last sync.

sync-dav-the-server = the server

sync-dav-another-server = another server

sync-dav-this-file-is-larger-than-the = This file is larger than the calendar server takes.

sync-dav-this-event-has-as-many-attachments = This event has as many attachments as the server allows.

sync-dav-that-attachment-is-no-longer-on = That attachment is no longer on the event. Read it again.

sync-dav-that-occurrence-isnt-part-of-the = That occurrence isn't part of the event.

sync-dav-the-calendar-server-refused-the-attachment = The calendar server refused the attachment.

sync-dav-the-server-doesnt-allow-this-change = The server doesn't allow this change.

sync-dav-another-address = another address

sync-dav-the-server-has-no-such-item = The server has no such item (404).

sync-dav-the-server-answered = The server answered { $status }.

sync-dav-the-server-is-busy-seconds = The server is busy; try again in { $seconds ->
        [one] { $seconds } second
       *[other] { $seconds } seconds
    }.

sync-dav-the-server-is-busy-minutes = The server is busy; try again in { $minutes ->
        [one] { $minutes } minute
       *[other] { $minutes } minutes
    }.

sync-dav-the-server-moved-this-to = The server moved this to { $host }.

# src/sync/dsn.rs

# What a delivery report says of one recipient.
sync-dsn-status = { $action ->
        [failed] { $recipient } couldn't be delivered.
        [delayed] { $recipient } is delayed; the server is still trying.
        [delivered] { $recipient } was delivered.
        [relayed] { $recipient } was passed on to a server that doesn't report delivery.
        [expanded] { $recipient } was delivered and passed on to other addresses.
       *[other] { $recipient } has an unknown status.
    }

# The same for a failure or delay, with the reason.
sync-dsn-status-why = { $action ->
        [failed] { $recipient } couldn't be delivered: { $why }.
       *[delayed] { $recipient } is delayed; the server is still trying: { $why }.
    }

# src/sync/fido.rs

sync-fido-proton-didnt-say-which-security-keys = Proton didn't say which security keys to use.

sync-fido-proton-sent-no-challenge = Proton sent no challenge.

sync-fido-proton-didnt-name-itself-for-the = Proton didn't name itself for the key.

sync-fido-proton-asked-for-a-security-key = Proton asked for a security key for { $rp_id }, which isn't Proton.

sync-fido-this-security-key-is-locked-after = This security key is locked after too many wrong PINs. Unplug it and plug it in again, or reset it.

sync-fido-the-security-key-wasnt-touched-in = The security key wasn't touched in time. Try again.

sync-fido-using-the-security-key-was-cancelled = Using the security key was cancelled.

sync-fido-picacho-isnt-allowed-to-use-bluetooth = Picacho isn't allowed to use Bluetooth or USB security keys here. Allow it in the system settings and try again.

sync-fido-a-passkey-on-another-device-needs = A passkey on another device needs Bluetooth on, here and there, to know it's nearby. Turn Bluetooth on (and allow Picacho to use it in Settings → Privacy & Security → Bluetooth), or use a security key.

sync-fido-a-passkey-on-a-phone-needs = A passkey on a phone needs Bluetooth on this computer, to know the phone is nearby. Turn Bluetooth on, or use a security key.

sync-fido-that-key-or-passkey-isnt-registered = That key or passkey isn't registered with this Proton account. Use another, or the code from your authenticator app.

sync-fido-the-connection-to-the-security-key = The connection to the security key was lost. Keep it in place until it's done, and try again.

sync-fido-the-security-key-or-passkey-didnt = The security key or passkey didn't work ({ $error }).

sync-fido-windows-hello-isnt-available-on-this = Windows Hello isn't available on this version of Windows.

sync-fido-that-security-key-couldnt-be-used = That security key couldn't be used. Keep it in place until it's done, or try another.

sync-fido-no-security-key-was-found-plug = No security key was found. Plug one in and try again.

sync-fido-sign-in-cancelled = Sign-in cancelled.

sync-fido-your-phone-didnt-connect-in-time = Your phone didn't connect in time. Try again, with Bluetooth on on both devices.

sync-fido-proton-named-itself-oddly-for-the = Proton named itself oddly for the key.

sync-fido-windows-hello-stopped-unexpectedly = Windows Hello stopped unexpectedly ({ $error }).

sync-fido-that-security-key-couldnt-be-used-2 = That security key couldn't be used. Hold it in place until it's done, or try another.

sync-fido-use-a-security-key = Use a security key

sync-fido-use-a-passkey = Use a passkey

sync-fido-use-a-passkey-on-another-device = Use a passkey on another device

sync-fido-use-a-passkey-on-your-phone = Use a passkey on your phone

sync-fido-use-a-passkey-or-security-key = Use a passkey or security key

sync-fido-connecting-to-your-phone = Connecting to your phone…

sync-fido-confirm-the-sign-in-on-your = Confirm the sign-in on your phone.

sync-fido-talking-to-your-phone = Talking to your phone…

sync-fido-that-pin-isnt-right-enter-your = That PIN isn't right. Enter your security key's PIN.

sync-fido-this-security-key-has-a-pin = This security key has a PIN. Enter it to use the key.

sync-fido-that-pin-isnt-the-right-length = That PIN isn't the right length. Enter your security key's PIN.

sync-fido-plug-in-your-security-key-or = Plug in your security key, or turn on NFC to hold it against your phone…

sync-fido-plug-in-your-security-key = Plug in your security key…

sync-fido-keep-holding-your-security-key = Keep holding your security key…

sync-fido-that-security-key-couldnt-be-used-3 = That security key couldn't be used.

sync-fido-the-security-key-didnt-sign-in = The security key didn't sign in.

# src/sync/forwarding.rs

sync-forwarding-enter-the-servers-address-like-https = Enter the server's address, like https://…

sync-forwarding-the-server-must-use-https = The server must use https://.

sync-forwarding-enter-an-api-key = Enter an API key.

sync-forwarding-the-api-key-has-a-line = The API key has a line break in it.

sync-forwarding-could-not-reach = Could not reach { $label }

sync-forwarding-did-not-accept-the-api-key = { $label } did not accept the API key. Check it and try again.

sync-forwarding-answered = { $label } answered { $status }{ $detail }

sync-forwarding-sent-a-reply-picacho-cant-read = { $label } sent a reply Picacho can't read

sync-forwarding-firefox-relay-sent-an-unexpected-reply = Firefox Relay sent an unexpected reply

sync-forwarding-simplelogin-wont-make-more-aliases-on = SimpleLogin won't make more aliases on this plan.

sync-forwarding-simplelogin-no-longer-offers-that-domain = SimpleLogin no longer offers that domain. Choose another.

sync-forwarding-simplelogin-has-no-mailbox-to-forward = SimpleLogin has no mailbox to forward the alias to

sync-forwarding-the-new-alias-was-made-but = The new alias was made, but its address was missing from the reply

sync-forwarding-answered-2 = { $label } answered { $status }

sync-forwarding-simplelogin-did-not-change-the-alias = SimpleLogin did not change the alias

sync-forwarding-only-simplelogin-has-reverse-aliases = Only SimpleLogin has reverse aliases

sync-forwarding-listing-the-reverse-aliases-failed = Listing the reverse aliases failed

sync-forwarding-only-simplelogin-lists-an-aliass-activity = Only SimpleLogin lists an alias's activity

sync-forwarding-reading-the-aliass-activity-failed = Reading the alias's activity failed

sync-forwarding-only-simplelogin-contacts-can-be-blocked = Only SimpleLogin contacts can be blocked

sync-forwarding-simplelogin-did-not-say-whether-the = SimpleLogin did not say whether the contact is blocked

sync-forwarding-only-simplelogin-contacts-can-be-deleted = Only SimpleLogin contacts can be deleted

sync-forwarding-simplelogin-did-not-give-an-address = SimpleLogin did not give an address for the contact

sync-forwarding-firefox-relay-aliases-have-no-display = Firefox Relay aliases have no display name

sync-forwarding-firefox-relay-aliases-cant-be-pinned = Firefox Relay aliases can't be pinned

sync-forwarding-an-alias-needs-somewhere-to-forward = An alias needs somewhere to forward to.

sync-forwarding-firefox-relay-forwards-to-your-accounts = Firefox Relay forwards to your account's address

sync-forwarding-only-simplelogin-encrypts-forwarded-mail-per = Only SimpleLogin encrypts forwarded mail per alias

sync-forwarding-simplelogin-did-not-give-an-address-2 = SimpleLogin did not give an address for writing to { $contact }

# src/sync/gss.rs

sync-gss-kerberos-sign-in-failed-get-a = Kerberos sign-in failed: { $detail }. Get a ticket for your realm first (kinit, or sign in to your domain), then try again.

sync-gss-this-computer-has-no-kerberos-gss = This computer has no Kerberos (GSS-API) library{ $detail }. Install your system's Kerberos package (krb5 or heimdal).

sync-gss-the-kerberos-library-lacks = The Kerberos library lacks { $function }

sync-gss-the-kerberos-library-doesnt-name-host = The Kerberos library doesn't name host-based services

sync-gss-the-kerberos-library-has-no-host = The Kerberos library has no host-based service name type.

sync-gss-the-kerberos-sign-in-was-already = The Kerberos sign-in was already complete.

sync-gss-the-servers-kerberos-answer-was-replayed = The server's Kerberos answer was replayed or out of order; not signed in.

sync-gss-the-server-didnt-prove-it-is = The server didn't prove it is the service it claims to be (no Kerberos mutual authentication); not signed in.

sync-gss-the-kerberos-sign-in-has-no = The Kerberos sign-in has no integrity protection; not signed in.

sync-gss-the-kerberos-sign-in-isnt-complete = The Kerberos sign-in isn't complete.

sync-gss-the-servers-kerberos-message-couldnt-be = The server's Kerberos message couldn't be read: { $error }

sync-gss-picacho-couldnt-sign-its-kerberos-answer = Picacho couldn't sign its Kerberos answer: { $error }

sync-gss-the-servers-kerberos-message-couldnt-be-2 = The server's Kerberos message couldn't be read (SSPI { $status }).

sync-gss-the-servers-kerberos-message-couldnt-be-3 = The server's Kerberos message couldn't be read.

sync-gss-picacho-couldnt-sign-its-kerberos-answer-2 = Picacho couldn't sign its Kerberos answer (SSPI { $status }).

sync-gss-kerberos-sign-in-isnt-available-on = Kerberos sign-in isn't available on phones and tablets.

sync-gss-windows-has-no-kerberos-credentials-for = Windows has no Kerberos credentials for you (SSPI { $status })

sync-gss-sspi-refused = SSPI refused ({ $status })

# src/sync/ical.rs

sync-ical-this-invitation-has-no-identifier-to = This invitation has no identifier to answer.

sync-ical-this-invitation-doesnt-say-who-to = This invitation doesn't say who to answer.

sync-ical-the-time-proposed-ends-before-it = The time proposed ends before it starts.

sync-ical-accepted = Accepted

sync-ical-tentatively-accepted = Tentatively accepted

sync-ical-declined = Declined

sync-ical-invitation = Invitation

sync-ical-delegated = Delegated

sync-ical-completed = Completed

sync-ical-in-progress = In progress

sync-ical-join = Join

sync-ical-where = Where

sync-ical-categories = Categories

sync-ical-privacy = Privacy

sync-ical-confidential = Confidential

sync-ical-private = Private

sync-ical-min-before = { $how ->
        [email] email { $minutes } min before
        [sound] sound { $minutes } min before
       *[other] other { $minutes } min before
    }

sync-ical-min-after = { $how ->
        [email] email { $minutes } min after
        [sound] sound { $minutes } min after
       *[other] other { $minutes } min after
    }

# A reminder ("Email 10 min before") that comes again
sync-ical-more-every-min = { $reminder }, { $repeat } more every { $minutes } min

sync-ical-reminders = Reminders

sync-ical-all-day = { $date }, all day

# How an event repeats (“Every 2 weeks on Monday, Wednesday, until
# Dec 24, 2026”), built from the messages below: sync-ical-every, then its
# details, each joined by sync-ical-repeat-detail or sync-ical-repeat-end
# (their order: the months, the days, then the end). Word and number keys
# ([day], [1], [-1]) stay; add plural forms ([one], [few]…) where your
# language needs them.

sync-ical-every = { $unit ->
        [day] { $interval ->
            [1] Every day
           *[other] Every { $interval } days
        }
        [week] { $interval ->
            [1] Every week
           *[other] Every { $interval } weeks
        }
        [month] { $interval ->
            [1] Every month
           *[other] Every { $interval } months
        }
       *[year] { $interval ->
            [1] Every year
           *[other] Every { $interval } years
        }
    }

sync-ical-month-name = { $month ->
        [1] January
        [2] February
        [3] March
        [4] April
        [5] May
        [6] June
        [7] July
        [8] August
        [9] September
        [10] October
        [11] November
       *[12] December
    }

sync-ical-in-months = in { $months }

# $pos: which of them in the month or year (-1 the last).
sync-ical-on-the-nth-kind-of-day = on { $pos ->
        [-1] the last
        [1] the first
        [2] the second
        [3] the third
        [4] the fourth
       *[other] the { $pos }th
    } { $kind ->
        [weekday] weekday
       *[weekend] weekend day
    }

sync-ical-weekday-name = { $day ->
        [0] Monday
        [1] Tuesday
        [2] Wednesday
        [3] Thursday
        [4] Friday
        [5] Saturday
       *[6] Sunday
    }

# $pos: which of them in the month or year (0 every one, -1 the last).
sync-ical-nth-weekday = { $pos ->
        [0] { $day }
        [-1] the last { $day }
        [1] the first { $day }
        [2] the second { $day }
        [3] the third { $day }
        [4] the fourth { $day }
       *[other] { $day } #{ $pos }
    }

sync-ical-month-day = { $kind ->
        [last] the last day
        [before-end] { $before_end ->
            [one] { $before_end } day before the end
           *[other] { $before_end } days before the end
        }
       *[day] day { $day }
    }

sync-ical-on-days = on { $days }

sync-ical-times = { $count ->
        [one] { $count } time
       *[other] { $count } times
    }

sync-ical-until = until { $date }

# How often an event repeats ("Every week") and one detail of it ("on
# Monday", "in March", "on the first weekday"), as one phrase. More details
# are joined one at a time.
sync-ical-repeat-detail = { $repeat } { $detail }

# How often an event repeats and when it stops ("5 times", "until Nov 26,
# 2026"), as one phrase
sync-ical-repeat-end = { $repeat }, { $end }

# src/sync/imap.rs

sync-imap-imap-tls-handshake-failed = IMAP TLS handshake failed: { $error }

sync-imap-doesnt-offer-kerberos-sign-in-auth = { $host } doesn't offer Kerberos sign-in (AUTH=GSSAPI). Turn off Kerberos for this account to use a password.

sync-imap-offers-no-sign-in-method-picacho = { $host } offers no sign-in method Picacho supports.

sync-imap-imap-authentication-failed = IMAP authentication failed

sync-imap-the-server-refused-the-token = The server refused the token ({ $reason })

sync-imap-imap-server-rejected-connection = IMAP server rejected connection

sync-imap-the-imap-server-refused-capability = The IMAP server refused CAPABILITY

sync-imap-the-imap-servers-reply-is-too = The IMAP server's reply is too long

sync-imap-the-imap-server-closed-the-connection = The IMAP server closed the connection

sync-imap-is-no-longer-on-the-server = “{ $name }” is no longer on the server.

sync-imap-is-one-of-the-mailboxs-standard = “{ $name }” is one of the mailbox's standard folders and can't be deleted.

sync-imap-has-folders-inside-it-delete-those = “{ $name }” has folders inside it. Delete those first.

sync-imap-enter-a-name-for-the-folder = Enter a name for the folder.

sync-imap-folder-names-cant-contain-line-breaks = Folder names can't contain line breaks or control characters.

sync-imap-folder-names-cant-contain-or = Folder names can't contain * or %.

sync-imap-folder-names-cant-contain = Folder names can't contain “{ $delimiter }”.

sync-imap-that-folder-name-is-too-long = That folder name is too long.

sync-imap-is-the-name-of-a-standard = “{ $label }” is the name of a standard folder. Choose another name.

sync-imap-theres-already-a-folder-named-there = There's already a folder named “{ $label }” there.

sync-imap-is-no-longer-on-the-server-2 = “{ $folder }” is no longer on the server.

sync-imap-folders-cant-be-made-inside = Folders can't be made inside “{ $folder }”.

sync-imap-the-server-doesnt-allow-folders-inside = The server doesn't allow folders inside “{ $folder }”.

sync-imap-is-one-of-the-mailboxs-standard-2 = “{ $folder }” is one of the mailbox's standard folders and can't be renamed.

sync-imap-all-mail-is-not-a-folder = All Mail is not a folder on the server.

sync-imap-the-server-has-no-folder = The server has no { $label } folder.

sync-imap-invalid-imap-starttls-response = Invalid IMAP STARTTLS response

sync-imap-imap-starttls-was-rejected-credentials-were = IMAP STARTTLS was rejected; credentials were not sent

sync-imap-unexpected-data-following-starttls = Unexpected data following STARTTLS

sync-imap-rights-are-written-as-letters-rfc = Rights are written as letters (RFC 4314 §2.1), such as “lrs”.

sync-imap-enter-a-name-for-the-label = Enter a name for the label.

sync-imap-labels-on-this-server-cant-contain = Labels on this server can't contain spaces, accents or ( ) {"{"} {"}"} % * " \ ] — try “{ $suggestion }”.

sync-imap-is-used-by-mail-apps-themselves = “{ $label }” is used by mail apps themselves. Choose another name.

sync-imap-is-one-of-gmails-own-labels = “{ $label }” is one of Gmail's own labels.

sync-imap-invalid-mailbox-name-from-the-server = Invalid mailbox name from the server

sync-imap-the-server-did-not-supply-a = The server did not supply a valid UIDVALIDITY; cached mail has been kept.

sync-imap-invalid-imap-fetch = Invalid IMAP FETCH

sync-imap-invalid-imap-response = Invalid IMAP response

sync-imap-server-hostname-could-not-be-resolved = Server hostname could not be resolved

sync-imap-could-not-connect-to = Could not connect to { $host }:{ $port }: { $error }

sync-imap-the-message-is-too-large-for = The message is too large for this server.

sync-imap-a-folder-with-that-name-already = A folder with that name already exists.

sync-imap-that-folder-doesnt-exist-on-the = That folder doesn't exist on the server any more.

sync-imap-the-folder-doesnt-exist-on-the = The folder doesn't exist on the server; make it first.

sync-imap-this-account-isnt-allowed-to-do = This account isn't allowed to do that here.

sync-imap-someone-or-something-else-is-using = Someone or something else is using that folder now; try again shortly.

sync-imap-the-server-cant-do-that-its = The server can't do that (its rules don't allow it).

sync-imap-that-goes-past-one-of-the = That goes past one of the server's limits.

sync-imap-some-of-those-messages-were-deleted = Some of those messages were deleted meanwhile.

sync-imap-the-server-says-that-folder-is = The server says that folder is damaged; ask its administrator.

sync-imap-the-server-failed-try-again-later = The server failed; try again later.

sync-imap-the-server-wants-an-encrypted-connection = The server wants an encrypted connection for that.

sync-imap-the-server-asks-you-to-contact = The server asks you to contact its administrator.

sync-imap-delete-the-folders-inside-it-first = Delete the folders inside it first.

sync-imap-that-folder-can-only-be-read = That folder can only be read, not changed.

sync-imap-this-folder-cant-keep-message-numbers = This folder can't keep message numbers; it may be read again whole.

sync-imap-the-server-cant-search-for-text = The server can't search for text in that alphabet.

# src/sync/imap_ext.rs

sync-imap-ext-the-server-only-accepts-utf-8 = The server only accepts UTF-8 sessions and refused to enable one.

sync-imap-ext-the-server-only-accepts-utf-8-2 = The server only accepts UTF-8 sessions and didn't enable one.

sync-imap-ext-invalid-imap-sequence-set = Invalid IMAP sequence set

sync-imap-ext-invalid-imap-uid-0 = Invalid IMAP UID 0

sync-imap-ext-the-servers-search-result-is-too = The server's search result is too large

sync-imap-ext-invalid-imap-search = Invalid IMAP search

sync-imap-ext-invalid-imap-uid-in-search-response = Invalid IMAP UID in search response

sync-imap-ext-invalid-imap-sort-response = Invalid IMAP SORT response

sync-imap-ext-invalid-imap-thread-response = Invalid IMAP THREAD response

sync-imap-ext-invalid-metadata-entry = Invalid METADATA entry

sync-imap-ext-the-server-didnt-describe-this-messages = The server didn't describe this message's parts.

sync-imap-ext-invalid-imap-section = Invalid IMAP section

sync-imap-ext-this-part-is-larger-than-picacho = This part is larger than Picacho downloads ({ $megabytes } MB).

sync-imap-ext-the-message-no-longer-exists-on = The message no longer exists on the server

sync-imap-ext-the-server-named-an-invalid-transfer = The server named an invalid transfer encoding.

sync-imap-ext-this-connection-cant-send-text-with = This connection can't send text with accents to the server's search.

# src/sync/jmap.rs

sync-jmap-the-jmap-server-sent-picacho-on = The JMAP server sent Picacho on to { $target } without https://, so it wasn't followed.

sync-jmap-the-jmap-server-sent-picacho-on-2 = The JMAP server sent Picacho on to { $target }; enter that as the server instead.

sync-jmap-jmap-failed = { $what ->
        [sign-in] JMAP sign-in failed
        [download] JMAP download failed
        [upload] JMAP upload failed
        [event-stream] JMAP event stream failed
       *[request] JMAP request failed
    }

sync-jmap-the-jmap-server-answered = The JMAP server answered { $status }: { $detail }

sync-jmap-enter-your-password-or-api-token = Enter your password or API token.

sync-jmap-your-sign-in-expired-reconnect-the = Your sign-in expired. Reconnect the account.

sync-jmap-the-jmap-server-redirected-without-saying = The JMAP server redirected without saying where.

sync-jmap-the-jmap-server-didnt-accept-the = The JMAP server didn't accept the sign-in. Check the password, or use an API token if the server asks for one.

sync-jmap-doesnt-offer-jmap-no-well-known = { $host } doesn't offer JMAP (no /.well-known/jmap).

sync-jmap-couldnt-read-the-jmap-servers-session = Couldn't read the JMAP server's session.

sync-jmap-the-jmap-servers-session-isnt-json = The JMAP server's session isn't JSON Picacho can read.

sync-jmap-the-jmap-server-redirected-too-many = The JMAP server redirected too many times.

sync-jmap-this-jmap-server-doesnt-offer-mail = This JMAP server doesn't offer mail.

sync-jmap-this-jmap-account-has-no-mail = This JMAP account has no mail.

sync-jmap-this-shared-mail-account-is-no = This shared mail account is no longer shared with you.

sync-jmap-the-jmap-session-has-no = The JMAP session has no { $key }.

sync-jmap-the-jmap-sessions-isnt-an-address = The JMAP session's { $key } isn't an address.

sync-jmap-the-jmap-servers-doesnt-use-https = The JMAP server's { $key } doesn't use https://.

sync-jmap-this-jmap-server-takes-only-method = This JMAP server takes only { $max_calls } method calls at a time.

sync-jmap-this-request-is-larger-than-the = This request is larger than the JMAP server takes ({ $max_request } bytes at most).

sync-jmap-the-jmap-server-no-longer-accepts = The JMAP server no longer accepts the sign-in.

sync-jmap-couldnt-read-the-jmap-servers-reply = Couldn't read the JMAP server's reply.

sync-jmap-the-jmap-servers-reply-isnt-json = The JMAP server's reply isn't JSON.

sync-jmap-the-jmap-servers-reply-has-no = The JMAP server's reply has no method responses.

sync-jmap-the-jmap-server-didnt-answer = The JMAP server didn't answer { $name }.

sync-jmap-the-jmap-servers-mailboxes-arent-in = The JMAP server's mailboxes aren't in a form Picacho can read.

sync-jmap-this-server-takes-folder-names-of = This server takes folder names of up to { $max_size_mailbox_name } bytes.

sync-jmap-this-server-makes-new-folders-only = This server makes new folders only inside others.

sync-jmap-you-may-not-make-folders-inside = You may not make folders inside “{ $name }”.

sync-jmap-this-server-takes-folders-only-deep = This server takes folders only { $most } deep.

sync-jmap-the-server-didnt-make-the-folder = The server didn't make the folder: { $error }

sync-jmap-the-server-didnt-say-which-folder = The server didn't say which folder it made.

sync-jmap-you-may-not-the-folder = { $what ->
        [rename] You may not rename the folder “{ $name }”.
        [move] You may not move the folder “{ $name }”.
       *[subscribe] You may not subscribe to the folder “{ $name }”.
    }

sync-jmap-the-server-didnt-the-folder = { $what ->
        [rename] The server didn't rename the folder: { $error }
        [move] The server didn't move the folder: { $error }
       *[subscribe] The server didn't subscribe to the folder: { $error }
    }

sync-jmap-you-may-not-delete-the-folder = You may not delete the folder “{ $name }”.

sync-jmap-the-folder-still-has-mail-in = The folder still has mail in it. Move or delete its mail first.

sync-jmap-the-folder-has-folders-inside-it = The folder has folders inside it. Delete those first.

sync-jmap-the-server-didnt-delete-the-folder = The server didn't delete the folder: { $error }

sync-jmap-the-jmap-server-didnt-answer-the = The JMAP server didn't answer the query.

sync-jmap-the-jmap-servers-messages-arent-in = The JMAP server's messages aren't in a form Picacho can read.

sync-jmap-you-may-not-put-messages-in = You may not put messages in the folder “{ $name }”.

sync-jmap-the-server-didnt-change = The server didn't change { $count ->
        [one] { $count } message
       *[other] { $count } messages
    }: { $reasons }

sync-jmap-the-server-didnt-delete = The server didn't delete { $count ->
        [one] { $count } message
       *[other] { $count } messages
    }: { $reasons }

sync-jmap-message-no-longer-exists-on-the = Message no longer exists on the server

sync-jmap-message-exceeds-the-download-limit = Message exceeds the download limit.

sync-jmap-jmap-download-failed = JMAP download failed

sync-jmap-the-jmap-download-address-isnt-an = The JMAP download address isn't an address.

sync-jmap-the-jmap-server-redirected-the-download = The JMAP server redirected the download without saying where.

sync-jmap-the-jmap-server-redirected-the-download-2 = The JMAP server redirected the download to an address that isn't https://.

sync-jmap-the-jmap-server-redirected-the-download-3 = The JMAP server redirected the download too many times.

sync-jmap-the-message-is-larger-than-the = The message is larger than the server accepts.

sync-jmap-couldnt-read-the-jmap-servers-answer = Couldn't read the JMAP server's answer to the upload.

sync-jmap-the-jmap-uploads-reply-isnt-json = The JMAP upload's reply isn't JSON.

sync-jmap-the-jmap-server-didnt-say-where = The JMAP server didn't say where the upload went.

sync-jmap-this-server-keeps-a-message-in = { $most ->
        [one] This server keeps a message in at most { $most } folder.
       *[other] This server keeps a message in at most { $most } folders.
    }

sync-jmap-the-attachments-are-larger-than-this = The attachments are larger than this server sends ({ $megabytes } MB at most).

sync-jmap-this-jmap-server-doesnt-send-mail = This JMAP server doesn't send mail.

sync-jmap-the-jmap-servers-identities-arent-in = The JMAP server's identities aren't in a form Picacho can read.

sync-jmap-this-jmap-server-cant-promise-this = This JMAP server can't promise this message stays encrypted all the way (it doesn't offer REQUIRETLS), so it wasn't sent. Turn off Require encryption in transit (Settings → Sending) to send it anyway.

sync-jmap-this-jmap-server-cant-hold-a = This JMAP server can't hold a message that long before sending it.

sync-jmap-the-time-to-send-is-out = The time to send is out of range.

sync-jmap-the-account-has-neither-a-drafts = The account has neither a Drafts nor a Sent folder to send from.

sync-jmap-the-jmap-server-didnt-answer-the-2 = The JMAP server didn't answer the send.

sync-jmap-the-server-didnt-send-the-message = The server didn't send the message.

sync-jmap-the-server-didnt-send-the-message-2 = The server didn't send the message: { $error }

sync-jmap-the-server-didnt-say-whether-it = The server didn't say whether it sent the message.

sync-jmap-the-server-kept-its-record-of = The server kept its record of sending: { $error }

sync-jmap-the-server-couldnt-take-the-message = The server couldn't take the message back: { $error }

sync-jmap-the-server-didnt-change-the-sending = The server didn't change the sending address: { $error }

sync-jmap-the-server-doesnt-let-this-account = The server doesn't let this account send as { $email }.

sync-jmap-the-server-didnt-add-the-sending = The server didn't add the sending address: { $error }

sync-jmap-the-server-didnt-say-which-sending = The server didn't say which sending address it added.

sync-jmap-the-server-didnt-remove-the-sending = The server didn't remove the sending address: { $error }

sync-jmap-the-jmap-servers-reading-of-the = The JMAP server's reading of the message isn't in a form Picacho can read.

sync-jmap-the-server-didnt-take-the-data = The server didn't take the data: { $error }

sync-jmap-the-server-didnt-say-where-it = The server didn't say where it kept the data.

sync-jmap-the-servers-data-isnt-base64 = The server's data isn't base64.

sync-jmap-the-servers-data-doesnt-match-its = The server's data doesn't match its digest; it was changed on the way.

sync-jmap-this-jmap-server-cant-say-what = This JMAP server can't say what uses its data.

sync-jmap-this-jmap-server-cant-send-read = This JMAP server can't send read receipts.

sync-jmap-the-server-didnt-send-the-read = The server didn't send the read receipt: { $error }

sync-jmap-this-jmap-server-cant-read-read = This JMAP server can't read read receipts.

sync-jmap-the-jmap-servers-away-reply-isnt = The JMAP server's away reply isn't in a form Picacho can read.

sync-jmap-this-jmap-server-doesnt-offer-an = This JMAP server doesn't offer an automatic reply while away.

sync-jmap-the-server-didnt-save-the-away = The server didn't save the away reply: { $error }

sync-jmap-this-jmap-server-doesnt-report-storage = This JMAP server doesn't report storage.

sync-jmap-this-jmap-server-doesnt-push-changes = This JMAP server doesn't push changes.

sync-jmap-the-jmap-event-stream-answered = The JMAP event stream answered { $status }.

sync-jmap-the-jmap-event-stream-sent-more = The JMAP event stream sent more than an event can be.

sync-jmap-this-jmap-server-doesnt-offer-server = This JMAP server doesn't offer server filters (JMAP for Sieve Scripts).

sync-jmap-the-jmap-servers-filter-scripts-arent = The JMAP server's filter scripts aren't in a form Picacho can read.

sync-jmap-these-filters-are-larger-than-the = These filters are larger than the server keeps ({ $most } bytes at most). Remove some rules.

sync-jmap-a-filter-script-named-is-already = A filter script named “{ $name }” is already on the server.

sync-jmap-the-server-didnt-rename-the-filters = The server didn't rename the filters: { $error }

sync-jmap-these-are-the-filters-in-use = These are the filters in use; turn them off before removing them.

sync-jmap-the-server-didnt-remove-the-filters = The server didn't remove the filters: { $error }

sync-jmap-this-server-takes-filter-names-of = This server takes filter names of up to { $most } bytes.

sync-jmap-the-filter-server-rejected-the-filters = The filter server rejected the filters: { $error }

sync-jmap-the-server-keeps-at-most-filter = The server keeps at most { $most } filter scripts, and has that many. Remove one first.

sync-jmap-the-server-didnt-save-the-filters = The server didn't save the filters: { $error }

sync-jmap-the-server-didnt-take-the-message = The server didn't take the message: { $error }

sync-jmap-the-server-didnt-say-where-the = The server didn't say where the message went.

sync-jmap-the-jmap-server-didnt-answer-2 = The JMAP server didn't answer { $method }.

sync-jmap-another-server = another server

sync-jmap-couldnt-reach-the-jmap-server = Couldn't reach the JMAP server: { $error }

sync-jmap-the-server-didnt-copy-message = The server didn't copy message { $id }: { $error }

sync-jmap-the-jmap-event-stream-went-silent = The JMAP event stream went silent.

sync-jmap-the-jmap-server-is-limiting-requests = The JMAP server is limiting requests ({ $status }); background sync waits a while.

sync-jmap-the-jmap-server-refused-the-request = The JMAP server refused the request

sync-jmap-the-jmap-server-refused-method = The JMAP server refused { $method }

sync-jmap-this-account-is-shared-with-you = This account is shared with you read-only, so it can't be changed ({ $method }).

sync-jmap-the-jmap-server-cant-say-what = The JMAP server can't say what changed ({ $method }), so everything is read again.

# src/sync/jscal.rs

sync-jscal-this-jmap-server-has-no-calendars = This JMAP server has no calendars.

sync-jscal-this-jmap-server-doesnt-keep-those = This JMAP server doesn't keep those.

sync-jscal-the-server-didnt-copy = The server didn't copy { $id }: { $error }

sync-jscal-only-calendars-and-address-books-are = Only calendars and address books are shared this way.

sync-jscal-the-server-didnt-share-it = The server didn't share it: { $error }

sync-jscal-this-jmap-server-has-no-directory = This JMAP server has no directory of people.

sync-jscal-the-server-didnt-change-it = The server didn't change it: { $error }

sync-jscal-the-server-didnt-change-the-event = The server didn't change the event: { $error }

sync-jscal-the-server-didnt-add-the-event = The server didn't add the event: { $error }

sync-jscal-the-server-didnt-say-which-event = The server didn't say which event it added.

sync-jscal-the-server-didnt-remove-the-event = The server didn't remove the event.

sync-jscal-the-server-didnt-make-the-calendar = The server didn't make the calendar: { $error }

sync-jscal-no-calendar-id = No calendar id.

sync-jscal-this-jmap-server-has-no-contacts = This JMAP server has no contacts.

sync-jscal-the-server-didnt-change-the-contact = The server didn't change the contact: { $error }

sync-jscal-no-address-book-on-the-server = No address book on the server can take new contacts.

sync-jscal-the-server-didnt-add-the-contact = The server didn't add the contact: { $error }

sync-jscal-no-contact-id = No contact id.

sync-jscal-this-contact-isnt-on-the-jmap = This contact isn't on the JMAP server.

sync-jscal-the-server-didnt-remove-the-contact = The server didn't remove the contact.

sync-jscal-the-server-didnt-make-the-address = The server didn't make the address book: { $error }

sync-jscal-no-address-book-id = No address book id.

sync-jscal-your-calendar-server-cant-say-when = Your calendar server can't say when others are free.

sync-jscal-the-server-didnt-rename-the-calendar = The server didn't rename the calendar: { $error }

sync-jscal-the-server-didnt-delete-the-calendar = The server didn't delete the calendar: { $error }

sync-jscal-the-server-didnt-rename-the-address = The server didn't rename the address book: { $error }

sync-jscal-the-server-didnt-delete-the-address = The server didn't delete the address book: { $error }

sync-jscal-this-jmap-server-doesnt-offer-calendars = This JMAP server doesn't offer calendars.

sync-jscal-this-jmap-server-doesnt-offer-contacts = This JMAP server doesn't offer contacts.

sync-jscal-someone = Someone

sync-jscal-a-calendar = a calendar

sync-jscal-stopped-sharing-with-you = { $who } stopped sharing { $name } with you.

sync-jscal-shared-with-you = { $who } shared { $name } with you.

sync-jscal-changed-what-you-may-do-with = { $who } changed what you may do with { $name }.

sync-jscal-an-event = an event

sync-jscal-added = { $who } added { $title }.

sync-jscal-removed = { $who } removed { $title }.

sync-jscal-changed = { $who } changed { $title }.

# Said when the server refuses without saying why.
sync-jscal-refused = refused

sync-jscal-this-jmap-server-has-no-tasks = This JMAP server has no tasks.

sync-jscal-the-server-didnt-make-the-task-list = The server didn't make the task list: { $error }

sync-jscal-no-task-list-id = No task list id.

sync-jscal-the-server-didnt-change-the-task = The server didn't change the task: { $error }

sync-jscal-the-server-didnt-add-the-task = The server didn't add the task: { $error }

sync-jscal-the-server-didnt-say-which-task = The server didn't say which task it added.

sync-jscal-the-server-didnt-remove-the-task = The server didn't remove the task.

sync-jscal-you-may-not-make-task-lists = This account doesn't let you make task lists.

# Who changed what, where a task's title isn't known: "Bo changed a task."
sync-jscal-a-task = a task

# src/sync/keys.rs

sync-keys-the-key-found-is-too-large = The key found is too large.

sync-keys-keys-openpgp-org-couldnt-be-reached = keys.openpgp.org couldn't be reached.

sync-keys-keys-openpgp-org-answered = keys.openpgp.org answered { $status }.

sync-keys-enter-a-keyserver-address = Enter a keyserver address.

sync-keys-that-isnt-a-valid-keyserver-address = That isn't a valid keyserver address.

sync-keys-keyservers-must-use-https-or-hkps = Keyservers must use HTTPS (or hkps://).

sync-keys-keyserver-address-has-no-host = Keyserver address has no host.

sync-keys-couldnt-be-reached = { $host } couldn't be reached.

sync-keys-answered = { $host } answered { $status }.

sync-keys-no-email-addresses-on-your-keys = No email addresses on your keys could be exported for WKD.

sync-keys-keys-openpgp-org-answered-2 = keys.openpgp.org answered { $status }: { $why }

sync-keys-keys-openpgp-org-didnt-answer-as = keys.openpgp.org didn't answer as expected.

sync-keys-keys-openpgp-org-couldnt-send-the = keys.openpgp.org couldn't send the confirmation email ({ $status }).

sync-keys-a-whole-fingerprint-is-needed-to = A whole fingerprint is needed to look up a key.

sync-keys-the-addresss-own-domain-wkd = the address's own domain (WKD)

sync-keys-the-addresss-domains-dnssec-signed-dns = the address's domain's DNSSEC-signed DNS (OPENPGPKEY)

sync-keys-the-addresss-domains-dns-openpgpkey-unverified = the address's domain's DNS (OPENPGPKEY, unverified)

sync-keys-it-refused-the-key = it refused the key

# src/sync/mime.rs

sync-mime-end-to-end-encryption = End-to-end encryption

sync-mime-encrypted-but-not-decrypted-here = Encrypted, but not decrypted here

sync-mime-standard-encryption-tls = Standard encryption (TLS)

sync-mime-not-encrypted-in-transit = Not encrypted in transit

sync-mime-not-recorded = Not recorded

# src/sync/models.rs

sync-models-couldnt-reach-the-model-download = Couldn't reach the model download

sync-models-the-model-download-answered = The model download answered { $status }

sync-models-the-model-download-is-larger-than = The model download is larger than expected

sync-models-the-model-download-was-interrupted = The model download was interrupted

# src/sync/msg.rs

sync-msg-this-msg-file-couldnt-be-read = This .msg file couldn't be read.

sync-msg-this-msg-file-is-too-large = This .msg file is too large to open.

sync-msg-embedded-msg-files-are-nested-too = Embedded .msg files are nested too deeply.

sync-msg-unexpected-end-of-msg-file = Unexpected end of .msg file

sync-msg-not-a-valid-outlook-msg-file = Not a valid Outlook .msg file (missing OLE2 header).

sync-msg-unsupported-msg-sector-size = Unsupported .msg sector size.

sync-msg-unsupported-msg-mini-sector-size = Unsupported .msg mini-sector size.

sync-msg-empty-msg-directory = Empty .msg directory.

sync-msg-missing-msg-stream = Missing .msg stream

sync-msg-this-msg-file-is-larger-than = This .msg file is larger than it says.

sync-msg-sector-overflow = Sector overflow

sync-msg-truncated-msg-sector = Truncated .msg sector

sync-msg-mini-sector-overflow = Mini-sector overflow

sync-msg-an-embedded-message-in-this-msg = An embedded message in this .msg file loops back on itself.

sync-msg-truncated-compressed-rtf = Truncated compressed RTF

sync-msg-compressed-rtf-too-large = Compressed RTF too large

sync-msg-unsupported-compressed-rtf-magic = Unsupported compressed RTF magic

# src/sync/mta_sts.rs

sync-mta-sts-the-policy-gives-twice = the policy gives { $key } twice

sync-mta-sts-unknown-policy-mode = unknown policy mode { $mode }

sync-mta-sts-max-age-isnt-a-number = max_age isn't a number

sync-mta-sts-not-an-stsv1-policy = not an STSv1 policy

sync-mta-sts-the-policy-has-no-mode = the policy has no mode

sync-mta-sts-the-policy-names-no-mail-servers = the policy names no mail servers

sync-mta-sts-the-policy-has-no-max-age = the policy has no max_age

sync-mta-sts-the-policys-max-age-is-too = the policy's max_age is too large

sync-mta-sts-isnt-a-domain-name-that-can = { $domain } isn't a domain name that can publish a policy

sync-mta-sts-couldnt-fetch = couldn't fetch { $url }

sync-mta-sts-answered = { $url } answered { $status }

sync-mta-sts-isnt-plain-text = { $url } isn't plain text

sync-mta-sts-couldnt-read = couldn't read { $url }

sync-mta-sts-is-too-large = { $url } is too large

sync-mta-sts-isnt-a-domain-name = { $domain } isn't a domain name

# src/sync/oauth.rs

sync-oauth-the-servers-sign-in-keys-jwks = The server's sign-in keys (jwks_uri) aren't served over https.

sync-oauth-the-servers-sign-in-doesnt-support = The server's sign-in doesn't support PKCE with S256, which Picacho requires.

sync-oauth-the-servers-sign-in-settings-have = The server's sign-in settings have no https authorization endpoint.

sync-oauth-the-servers-sign-in-settings-have-2 = The server's sign-in settings have no https token endpoint.

sync-oauth-doesnt-offer-browser-sign-in-for = { $host } doesn't offer browser sign-in for mail apps{ $detail }. Use a password or an API token instead.

sync-oauth-the-sign-in-services-address-isnt = The sign-in service's address isn't valid.

sync-oauth-the-sign-in-services-address-must = The sign-in service's address must start with https://.

sync-oauth-its-sign-in-settings-are-for = its sign-in settings are for { $named }, not { $issuer }

sync-oauth-isnt-a-sign-in-service-picacho = { $issuer } isn't a sign-in service Picacho can use{ $detail }. Check the address with your organization.

sync-oauth-its-sign-in-settings-name-no = its sign-in settings name no issuer

sync-oauth-its-sign-in-settings-are-for-2 = its sign-in settings are for { $issuer }, not { $origin }

sync-oauth-the-server-registered-picacho-without-a = The server registered Picacho without a client id.

sync-oauth-couldnt-reach-the-server-to-register = Couldn't reach the server to register Picacho for sign-in.

sync-oauth-the-server-didnt-register-picacho-for = The server didn't register Picacho for browser sign-in.

sync-oauth-tokens-are-only-given-up-over = Tokens are only given up over https.

sync-oauth-unexpected-profile-picture-address = Unexpected profile picture address

sync-oauth-could-not-open-the-system-browser = Could not open the system browser

sync-oauth-sign-in-cancelled = Sign-in cancelled.

sync-oauth-sign-in-timed-out-please-try = Sign-in timed out. Please try again.

sync-oauth-the-sign-in-answered-for-another = The sign-in answered for another server ({ $given }); nothing was signed in.

sync-oauth-the-sign-ins-id-token-couldnt = The sign-in's ID token couldn't be read.

sync-oauth-the-sign-ins-id-token-came = The sign-in's ID token came from another server ({ $issuer }).

sync-oauth-the-sign-ins-id-token-was = The sign-in's ID token was issued to another app.

sync-oauth-the-sign-ins-id-token-doesnt = The sign-in's ID token doesn't say which of its apps it was issued to.

sync-oauth-the-sign-ins-id-token-has = The sign-in's ID token has no expiry.

sync-oauth-the-sign-ins-id-token-has-2 = The sign-in's ID token has expired. Check this device's clock.

sync-oauth-the-sign-ins-id-token-doesnt-2 = The sign-in's ID token doesn't belong to this sign-in.

sync-oauth-the-sign-ins-id-token-names = The sign-in's ID token names no account.

sync-oauth-only-google-accounts-are-asked-for = Only Google accounts, and Outlook accounts using Microsoft Graph, are asked for more access this way.

sync-oauth-doesnt-let-mail-apps-sign-in = { $host } doesn't let mail apps sign in in the browser without registering them first. Use a password or an API token instead.

sync-oauth-your-organizations-sign-in-doesnt-let = Your organization's sign-in doesn't let mail apps register themselves. Ask your administrator for a client id for Picacho (a public client with a http://127.0.0.1 redirect) and enter it under Server settings.

sync-oauth-the-sign-in-didnt-allow-everything = The sign-in didn't allow everything Picacho asked for ({ $scopes } wasn't granted). Sign in again and allow it.

sync-oauth-could-not-reach-pinal-accounts-check = Could not reach Pinal Accounts. Check your connection.

sync-oauth-pinal-accounts-is-not-answering-sign = Pinal Accounts is not answering sign-in requests right now.

sync-oauth-pinal-accounts-is-misconfigured = Pinal Accounts is misconfigured

sync-oauth-pinal-accounts-returned-an-unexpected-sign = Pinal Accounts returned an unexpected sign-in address.

sync-oauth-pinal-accounts-answered-for-another-account = Pinal Accounts answered for another account.

sync-oauth-has-no-pinal-mail-address-pinal = { $account } has no Pinal Mail address. Pinal Mail is for accounts made with a @{ $domain } address.

sync-oauth-pinal-did-not-let-picacho-stay = Pinal did not let Picacho stay signed in. Please try again.

sync-oauth-sign-in-failed-at-the-token = Sign-in failed at the token step ({ $status }: { $code }{ $aadsts }). Please try again.

sync-oauth-provider-did-not-return-an-access = Provider did not return an access token

sync-oauth-the-sign-in-issued-a-token = The sign-in issued { $kind ->
        [untyped] an untyped token
       *[other] a { $kind } token
    }, which Picacho can't use; it needs a bearer token.

sync-oauth-your-sign-in-expired-reconnect-the = Your sign-in expired. Reconnect the account.

sync-oauth-no-issuer = no issuer

sync-oauth-sign-in-was-cancelled = Sign-in was cancelled.

sync-oauth-sign-in-was-declined = Sign-in was declined: { $error }

sync-oauth-sign-in-complete-you-can-return = Sign-in complete. You can return to Picacho.

sync-oauth-sign-in-could-not-be-completed = Sign-in could not be completed. Return to Picacho and try again.

sync-oauth-none-named = none named

sync-oauth-this-pinal-account = This Pinal account

# src/sync/pop.rs

sync-pop-incomplete-or-oversized-pop3-status = Incomplete or oversized POP3 status

sync-pop-invalid-pop3-status = Invalid POP3 status

sync-pop-invalid-pop3-message-number = Invalid POP3 message number

sync-pop-invalid-pop3-command = Invalid POP3 command

sync-pop-the-server-refused-the-token = The server refused the token ({ $reason })

sync-pop-invalid-pop3-sign-in-reply = Invalid POP3 sign-in reply

sync-pop-pop3-connection-closed-unexpectedly = POP3 connection closed unexpectedly

sync-pop-incomplete-pop3-response-line = Incomplete POP3 response line

sync-pop-pop3-response-exceeds-the-download-limit = POP3 response exceeds the download limit

sync-pop-invalid-pop3-stat-reply = Invalid POP3 STAT reply

sync-pop-invalid-pop3-uidl-encoding = Invalid POP3 UIDL encoding

sync-pop-invalid-pop3-uidl = Invalid POP3 UIDL

sync-pop-invalid-pop3-uidl-entry = Invalid POP3 UIDL entry

sync-pop-invalid-pop3-uidl-identity = Invalid POP3 UIDL identity

sync-pop-duplicate-pop3-uidl-identity-or-message = Duplicate POP3 UIDL identity or message number

sync-pop-doesnt-offer-encryption-stls-on-port = { $host } doesn't offer encryption (STLS) on port { $port }; Picacho won't sign in to it. Use port 995 with TLS from the start.

sync-pop-the-pop3-server-refused-to-start = The POP3 server refused to start encryption (STLS); credentials were not sent

sync-pop-unexpected-data-from-the-pop3-server = Unexpected data from the POP3 server before encryption started

sync-pop-pop3-tls-handshake-failed = POP3 TLS handshake failed: { $error }

sync-pop-doesnt-take-sign-in-tokens-sasl = { $host } doesn't take sign-in tokens (SASL OAUTHBEARER or XOAUTH2). Ask your organization whether its POP3 server supports single sign-on.

sync-pop-doesnt-offer-kerberos-sign-in-sasl = { $host } doesn't offer Kerberos sign-in (SASL GSSAPI). Turn off Kerberos for this account to use a password.

sync-pop-offers-no-sign-in-method-picacho = { $host } offers no sign-in method Picacho supports.

sync-pop-cant-take-a-sign-in-name = { $host } can't take a sign-in name or password with non-ASCII characters (POP3 UTF8 USER).

sync-pop-the-pop3-mailbox-is-in-use = The POP3 mailbox is in use by another session

sync-pop-the-pop3-server-limits-how-often-this = The POP3 server limits how often this account signs in

sync-pop-the-pop3-server-has-a-temporary = The POP3 server has a temporary problem

sync-pop-the-pop3-server-has-a-lasting = The POP3 server has a lasting problem

sync-pop-the-pop3-server-cant-handle-utf-8 = The POP3 server can't handle UTF-8 here

sync-pop-pop3-server-rejected-the-request = POP3 server rejected the request

# src/sync/proton.rs

sync-proton-proton-ended-this-session-sign-in = Proton ended this session. Sign in again to keep using this account.

sync-proton-this-build-has-no-pass-registration = This copy of Picacho was built without a Proton Pass registration (PICACHO_PROTON_PASS_APP_TOKEN).

sync-proton-proton-wants-to-check-that-its = Proton wants to check that it's you. Sign in again to this account to pass its check.

sync-proton-proton-didnt-accept-this-app-proton = Proton didn't accept this app ({ $message }). Proton Mail Bridge still works with this account.

sync-proton-proton-refused-this-sign-in-start = Proton refused this sign-in { $step } ({ $error }). Start again with your password.

sync-proton-protons-check-was-closed-before-it = Proton's check was closed before it finished. Sign in again to try it again.

sync-proton-proton-wants-to-check-that-its-2 = Proton wants to check that it's you, which needs Microsoft Edge WebView2. Install it from go.microsoft.com/fwlink/p/?LinkId=2124703 and sign in again.

sync-proton-couldnt-find-picacho-to-show-protons = Couldn't find Picacho to show Proton's check

sync-proton-couldnt-open-a-window-for-protons = Couldn't open a window for Proton's check

sync-proton-sign-in-cancelled = Sign-in cancelled.

sync-proton-proton-wants-to-check-that-its-3 = Proton wants to check that it's you, but its check couldn't be shown here. Use Proton Mail Bridge, or sign in once at account.proton.me and try again.

sync-proton-proton-still-asks-to-check-that = Proton still asks to check that it's you after { $checks } tries. Wait a while, then sign in again.

sync-proton-proton-didnt-accept-this-email-address = Proton didn't accept this email address and password.

sync-proton-protons-sign-in-couldnt-be-verified = Proton's sign-in couldn't be verified ({ $why }). Nothing was sent; try again.

sync-proton-couldnt-sign-in-to-proton = Couldn't sign in to Proton

sync-proton-enter-the-code-from-your-authenticator = Enter the code from your authenticator app.

sync-proton-proton-didnt-accept-that-code-enter = Proton didn't accept that code. Enter the current one from your authenticator app.

sync-proton-proton-didnt-accept-that-security-key = Proton didn't accept that security key or passkey. Use one registered with this account.

sync-proton-enter-the-mailbox-password = Enter the mailbox password.

sync-proton-sign-in-to-proton-again = Sign in to Proton again.

sync-proton-this-accounts-keys-changed-at-proton = This account's keys changed at Proton (a new password?). Sign in again.

sync-proton-this-message-is-no-longer-at = This message is no longer at Proton.

sync-proton-the-keys-of-the-address-this = The keys of the address this message was sent to are locked or gone.

sync-proton-that-attachment-is-no-longer-at = That attachment is no longer at Proton.

sync-proton-proton-sent-an-attachment-key-that = Proton sent an attachment key that couldn't be read.

sync-proton-that-folder-no-longer-exists-at = That folder no longer exists at Proton.

sync-proton-this-proton-account-has-no-address = This Proton account has no address to write from.

sync-proton-the-message-couldnt-be-read-to = The message couldn't be read to save it at Proton.

sync-proton-proton-didnt-say-whether-it-kept = Proton didn't say whether it kept the message.

sync-proton-isnt-one-of-this-proton-accounts = { $from } isn't one of this Proton account's addresses.

sync-proton-couldnt-reach-proton = Couldn't reach Proton: { $why }

# $message: Proton's own words; $step: at which step, for example "at the password"
sync-proton-proton-said-http-code = Proton refused this sign-in { $step }. Proton said: “{ $message }” (HTTP { $status }, code { $code }). Start again with your password.

sync-proton-http-code = Proton refused this sign-in { $step } (HTTP { $status }, code { $code }). Start again with your password.

sync-proton-at-the-password = at the password

sync-proton-at-the-authenticator-code = at the authenticator code

sync-proton-at-the-security-key = at the security key

sync-proton-that-mailbox-password-didnt-unlock-this = That mailbox password didn't unlock this account's keys.

sync-proton-the-password-signed-in-but-didnt = The password signed in, but didn't unlock this account's keys.

sync-proton-while-unlocking-the-account = while unlocking the account

sync-proton-this-sign-in-has-ended = This sign-in has ended. Start again with your password.

# src/sync/proton_push.rs

sync-proton-push-this-copy-of-picacho-has-no = This copy of Picacho has no Firebase project for push.

sync-proton-push-couldnt-make-the-push-device-key = Couldn't make the push device key: { $error }

sync-proton-push-couldnt-start-receiving-pushes = Couldn't start receiving pushes: { $error }

# src/sync/public_net.rs

sync-public-net-the-address-leads-to-a-local = The address leads to a local network.

sync-public-net-the-servers-answer-is-too-large = The server's answer is too large.

# src/sync/sasl.rs

sync-sasl-the-server-kept-asking-for-more = The server kept asking for more during sign-in.

sync-sasl-the-connections-channel-binding-isnt-known = The connection's channel binding isn't known.

sync-sasl-the-servers-kerberos-sign-in-didnt = The server's Kerberos sign-in didn't offer its security layers.

sync-sasl-the-servers-kerberos-security-layer-offer = The server's Kerberos security-layer offer is malformed.

sync-sasl-the-server-requires-a-kerberos-security = The server requires a Kerberos security layer Picacho doesn't use (it offers none without one).

sync-sasl-the-servers-scram-reply-isnt-text = The server's SCRAM reply isn't text.

sync-sasl-the-server-refused-the-sign-in = The server refused the sign-in ({ $error }).

sync-sasl-the-servers-scram-reply-has-no = The server's SCRAM reply has no signature.

sync-sasl-the-servers-scram-signature-isnt-base64 = The server's SCRAM signature isn't base64.

sync-sasl-the-server-couldnt-prove-it-knows = The server couldn't prove it knows your password (SCRAM signature mismatch); not signed in.

sync-sasl-the-server-sent-an-unexpected-challenge = The server sent an unexpected { $name } challenge.

sync-sasl-the-server-didnt-prove-it-knows = The server didn't prove it knows your password (no SCRAM signature); not signed in.

sync-sasl-the-server-accepted-the-kerberos-sign = The server accepted the Kerberos sign-in before proving who it is; not signed in.

sync-sasl-kerberos-made-a-malformed-first-token = Kerberos made a malformed first token.

sync-sasl-the-servers-scram-challenge-isnt-text = The server's SCRAM challenge isn't text.

sync-sasl-the-server-refused-the-sign-in-2 = The server refused the sign-in ({ $error }).

sync-sasl-the-server-asked-for-a-scram = The server asked for a SCRAM extension Picacho doesn't know.

sync-sasl-the-servers-scram-challenge-has-no = The server's SCRAM challenge has no nonce.

sync-sasl-the-servers-scram-nonce-doesnt-extend = The server's SCRAM nonce doesn't extend Picacho's; not signed in.

sync-sasl-the-servers-scram-challenge-has-no-2 = The server's SCRAM challenge has no salt.

sync-sasl-the-servers-scram-salt-isnt-base64 = The server's SCRAM salt isn't base64.

sync-sasl-the-servers-scram-challenge-has-no-3 = The server's SCRAM challenge has no iteration count.

sync-sasl-the-servers-scram-iteration-count-isnt = The server's SCRAM iteration count isn't a number.

sync-sasl-the-server-asked-for-scram-iterations = The server asked for { $iterations } SCRAM iterations; Picacho allows up to { $max_iterations }.

sync-sasl-the-password-has-a-character-scram = The password has a character SCRAM sign-in can't use.

sync-sasl-the-servers-sign-in-challenge-isnt = The server's sign-in challenge isn't base64.

sync-sasl-it-needs = { $status }; it needs: { $scope }

# src/sync/sieve.rs

sync-sieve-the-server-named-a-filter-script = The server named a filter script with a line break in it.

sync-sieve-this-server-cant-run-another-script = This server can't run another script from Picacho's.

sync-sieve-this-server-cant-send-vacation-replies = This server can't send vacation replies.

sync-sieve-write-the-vacation-replys-message = Write the vacation reply's message.

sync-sieve-enter-dates-like-2026-12-24 = Enter dates like 2026-12-24.

sync-sieve-this-server-cant-limit-vacation-replies = This server can't limit vacation replies to dates.

sync-sieve-enter-the-time-zone-like-0100 = Enter the time zone like +0100.

sync-sieve-this-server-cant-send-vacation-replies-2 = This server can't send vacation replies more often than daily.

sync-sieve-enter-the-address-the-vacation-reply = Enter the address the vacation reply comes from.

sync-sieve-this-server-cant-keep-copies-of = This server can't keep copies of vacation replies.

sync-sieve-each-filter-needs-something-to-look = Each filter needs something to look for.

sync-sieve-this-server-cant-match-patterns-in = This server can't match patterns in filters.

sync-sieve-this-server-cant-filter-on = This server can't filter on { $label } ({ $ext }).

sync-sieve-enter-a-size-in-kilobytes-such = Enter a size in kilobytes, such as 5000.

sync-sieve-name-the-header-the-filter-looks = Name the header the filter looks at.

sync-sieve-this-server-cant-file-mail-into = This server can't file mail into folders.

sync-sieve-isnt-a-folder-filters-can-use = “{ $folder }” isn't a folder filters can use.

sync-sieve-this-server-cant-mark-mail-read = This server can't mark mail read from a filter.

sync-sieve-this-server-cant-star-mail-from = This server can't star mail from a filter.

sync-sieve-enter-the-one-address-mail-is = Enter the one address mail is sent on to.

sync-sieve-this-server-cant-send-a-copy = This server can't send a copy and keep the message too.

sync-sieve-this-server-cant-refuse-mail-from = This server can't refuse mail from a filter.

sync-sieve-this-server-cant-label-mail-from = This server can't label mail from a filter.

sync-sieve-the-filter-server-closed-the-connection = The filter server closed the connection.

sync-sieve-the-filter-servers-reply-is-too = The filter server's reply is too long.

sync-sieve-the-filter-server-sent-too-much = The filter server sent too much.

sync-sieve-the-filter-server-refused-to = { $what ->
        [start-encryption] The filter server refused to start encryption: { $explain }
        [list-scripts] The filter server refused to list filter scripts: { $explain }
        [read-filters] The filter server refused to read Picacho's filters: { $explain }
        [turn-off] The filter server refused to turn the filters off: { $explain }
        [remove] The filter server refused to remove the filters: { $explain }
       *[turn-on] The filter server refused to turn the filters on: { $explain }
    }

sync-sieve-offers-no-filter-server-managesieve-on = { $host } offers no filter server (ManageSieve) on port { $port }

sync-sieve-the-filter-server-refused-the-connection = The filter server refused the connection: { $text }

sync-sieve-s-filter-server-doesnt-offer-encryption = { $host }'s filter server doesn't offer encryption (STARTTLS); Picacho won't sign in to it.

sync-sieve-unexpected-data-from-the-filter-server = Unexpected data from the filter server before encryption started.

sync-sieve-tls-with-s-filter-server-failed = TLS with { $host }'s filter server failed: { $error }

sync-sieve-the-filter-server-failed-after-starttls = The filter server failed after STARTTLS: { $text }

sync-sieve-the-filter-server-offers-no-sign = The filter server offers no sign-in method Picacho supports.

sync-sieve-the-filter-server-is-busy-try = The filter server is busy; try again later.

sync-sieve-the-filter-server-refused-the-sign = The filter server refused the sign-in: { $reply }

sync-sieve-the-filter-server-isnt-answering = The filter server isn't answering: { $explain }

sync-sieve-the-filter-server-sends-this-account = The filter server sends this account to { $host }, outside its domain; Picacho won't sign in there.

sync-sieve-this-accounts-filters-are-kept-on = This account's filters are kept on { $authority }; set it as the filter server.

sync-sieve-the-filter-server-didnt-say-what = The filter server didn't say what its filters can do.

sync-sieve-the-filter-server-cant-take-the = The filter server can't take the filters: { $explain }

sync-sieve-the-filter-server-rejected-the-filters = The filter server rejected the filters: { $explain }

sync-sieve-the-filter-server-refused-to-save = The filter server refused to save the filters: { $explain }

sync-sieve-from = From

sync-sieve-to = To

sync-sieve-to-or-cc = To or Cc

sync-sieve-subject = Subject

sync-sieve-mailing-list = Mailing list

sync-sieve-text = Text

sync-sieve-larger-than-kb = Larger than (KB)

sync-sieve-header = Header

sync-sieve-delivered-to = Delivered to

# How a filter compares a message's field with its text
sync-sieve-contains = contains

sync-sieve-is = is

# With * and ? standing for any text
sync-sieve-matches = matches

sync-sieve-matches-the-pattern = matches the pattern

sync-sieve-the-account-has-no-room-for = the account has no room for more filters

sync-sieve-the-server-is-busy-try-again = the server is busy; try again later

sync-sieve-the-script-doesnt-exist = the script doesn't exist

sync-sieve-a-script-by-that-name-already = a script by that name already exists

sync-sieve-the-script-is-in-use = the script is in use

sync-sieve-the-accounts-filters-are-kept-on = the account's filters are kept on another server ({ $server })

sync-sieve-the-accounts-filters-are-kept-on-unnamed = the account's filters are kept on another server (not named)

# src/sync/sieve_parse.rs

sync-sieve-parse-a-comment-in-the-script-isnt = A comment in the script isn't closed.

sync-sieve-parse-a-string-in-the-script-isnt = A string in the script isn't closed.

sync-sieve-parse-a-text-string-in-the-script = A text: string in the script isn't closed.

sync-sieve-parse-the-script-has-a-character-sieve = The script has a character Sieve doesn't use: { $character }

sync-sieve-parse-the-script-nests-too-deeply = The script nests too deeply.

sync-sieve-parse-the-script-has-something-where-a = The script has something where a command should be.

sync-sieve-parse-a-block-in-the-script-isnt = A block in the script isn't closed.

sync-sieve-parse-a-command-in-the-script-doesnt = A command in the script doesn't end with ; or a block.

sync-sieve-parse-a-list-in-the-script-isnt = A list in the script isn't written right.

sync-sieve-parse-a-test-list-in-the-script = A test list in the script isn't written right.

sync-sieve-parse-the-script-has-something-where-a-2 = The script has something where a test should be.

sync-sieve-parse-the-script-has-an-unmatched-closing = The script has an unmatched closing brace.

# src/sync/smtp.rs

sync-smtp-the-address-this-message-is-from = The address this message is from was removed from { $email } (Settings → Sending). Choose another in From.

sync-smtp-the-address-in-from-isnt-valid = The address in From isn't valid.

sync-smtp-add-at-least-one-recipient = Add at least one recipient.

sync-smtp-isnt-a-valid-address = “{ $to }” isn't a valid address.

sync-smtp-enter-a-valid-reply-to-address = Enter a valid Reply-To address.

sync-smtp-the-picture-cant-be-read = The picture { $path } can't be read.

sync-smtp-attachments-are-too-large-to-send = Attachments are too large to send by email. Share them by link instead.

sync-smtp-the-attachment-cant-be-read = The attachment { $path } can't be read.

sync-smtp-the-outgoing-server-closed-the-connection = The outgoing server closed the connection.

sync-smtp-the-outgoing-server-sent-an-invalid = The outgoing server sent an invalid reply.

sync-smtp-invalid-smtp-command = Invalid SMTP command

sync-smtp-offers-no-sign-in-method-picacho = { $host } offers no sign-in method Picacho supports.

# $methods: the ones it offers, for example "PLAIN, XOAUTH2"
sync-smtp-offers-no-sign-in-method-it-offers = { $host } offers no sign-in method Picacho supports (it offers { $methods }).

sync-smtp-no-reply-to-the-message = No reply to the message

sync-smtp-tls-with-failed = TLS with { $host } failed: { $error }

sync-smtp-refused-the-connection = { $host } refused the connection: { $explained }

sync-smtp-doesnt-offer-starttls-on-port-picacho = { $host } doesn't offer STARTTLS on port { $port }; Picacho only sends over encrypted connections. If this server uses TLS from the start, turn on "TLS from the start" for it.

sync-smtp-refused-starttls = { $host } refused STARTTLS: { $explained }

sync-smtp-unexpected-data-from-before-encryption-started = Unexpected data from { $host } before encryption started; nothing was sent.

sync-smtp-the-message-went-to-its-other = The message went to its other recipients, but not to { $address }.

sync-smtp-this-message-is-with-its-attachments = This message is { $size } with its attachments; { $host } accepts up to { $limit }. Remove an attachment or share it by link.

sync-smtp-cant-promise-this-message-stays-encrypted = { $host } can't promise this message stays encrypted all the way (it doesn't offer REQUIRETLS), so it wasn't sent. Turn off Require encryption in transit (Settings → { $email } → Sending) to send it anyway.

sync-smtp-cant-send-to-or-from-addresses = { $host } can't send to or from addresses with accents or non-Latin letters (no SMTPUTF8).

sync-smtp-the-message-went-to-some-recipients = The message went to some recipients only.

sync-smtp-refused-to-start-another-message = { $host } refused to start another message: { $explained }

sync-smtp-the-server-refused-the-recipient = The server refused the recipient { $to }.

sync-smtp-the-message-went-to-some-recipients-2 = The message went to some recipients, but the server refused { $to }.

sync-smtp-took-none-of-this-messages-recipients = { $host } took none of this message's recipients (too many).

sync-smtp-isnt-a-valid-address-2 = “{ $address }” isn't a valid address.

sync-smtp-cant-promise-this-message-stays-encrypted-2 = { $host } can't promise this message stays encrypted all the way (no REQUIRETLS), so it wasn't sent.

sync-smtp-this-message-is-accepts-up-to = This message is { $size }; { $host } accepts up to { $limit }.

sync-smtp-cant-send-to-this-address-no = { $host } can't send to this address (no SMTPUTF8).

sync-smtp-cant-take-this-messages-8-bit = { $host } can't take this message's 8-bit content (no 8BITMIME).

sync-smtp-isnt-a-valid-address-3 = “{ $address }” isn't a valid address.

sync-smtp-the-server-refused-to-send-from = The server refused to send from { $address }: { $explained }

sync-smtp-the-server-refused-the-recipient-2 = The server refused the recipient { $address }: { $explained }

sync-smtp-enter-valid-recipient-email-addresses-separated = Enter valid recipient email addresses, separated by commas

sync-smtp-the-outgoing-server-cant-hold-this = The outgoing server can't hold this message until it's due.

sync-smtp-mb = { $megabytes } MB

sync-smtp-that-mailbox-doesnt-exist = That mailbox doesn't exist

sync-smtp-that-domain-doesnt-exist-or-takes = That domain doesn't exist or takes no mail

sync-smtp-that-address-isnt-valid = That address isn't valid

sync-smtp-that-person-has-moved-and-no = That person has moved and no longer gets mail there

sync-smtp-the-server-wont-send-from-this = The server won't send from this address

sync-smtp-that-domain-takes-no-mail-null = That domain takes no mail (null MX)

sync-smtp-that-mailbox-isnt-taking-mail-now = That mailbox isn't taking mail now

sync-smtp-that-mailbox-is-full = That mailbox is full

sync-smtp-the-message-is-too-large = The message is too large

sync-smtp-the-servers-storage-is-full = The server's storage is full

sync-smtp-the-recipients-server-couldnt-be-reached = The recipient's server couldn't be reached

sync-smtp-the-recipients-domain-couldnt-be-routed = The recipient's domain couldn't be routed to

sync-smtp-too-many-recipients-for-one-message = Too many recipients for one message

sync-smtp-the-server-cant-take-this-messages = The server can't take this message's format

sync-smtp-the-server-refused-this-by-its = The server refused this by its policy

sync-smtp-the-sign-in-wasnt-accepted = The sign-in wasn't accepted

sync-smtp-this-account-is-disabled = This account is disabled

sync-smtp-spf-says-this-server-may-not = SPF says this server may not send for this address

sync-smtp-the-message-failed-the-receiving-servers = The message failed the receiving server's sender checks

sync-smtp-the-server-cant-keep-the-message = The server can't keep the message encrypted all the way

sync-smtp-the-server-cant-take-this-now = The server can't take this now; it may later

sync-smtp-the-outgoing-server-refused-ehlo = The outgoing server refused EHLO: { $explained }

sync-smtp-refused-the-sign-in-with = { $host } refused the sign-in with { $name }: { $why }

sync-smtp-the-server-refused-the-message = The server refused the message: { $explained }

sync-smtp-the-server-couldnt-fetch-the-message = The server couldn't fetch the message to send: { $explained }

sync-smtp-smtp-delivery-failed-your-draft-has = SMTP delivery failed. Your draft has been kept; check the server and account settings.

sync-smtp-the-server-refused-to-send-from-2 = The server refused to send from { $from }: { $explained }

sync-smtp-encrypted-mail-with-bcc-cant-be = Encrypted mail with Bcc can't be sent through { $service } without showing who got a Bcc. Send to them separately, or turn off Encrypt.

# src/sync/stream.rs

sync-stream-mail-server-response-exceeds-the-32 = Mail server response exceeds the 32 MB session-command limit

sync-stream-the-server-closed-the-connection-during = The server closed the connection during a command.

sync-stream-compressed-imap-stream = Compressed IMAP stream: { $error }

sync-stream-the-server-ended-compression = the server ended compression

# src/sync/websocket.rs

sync-websocket-the-jmap-server-agreed-to-more = The JMAP server agreed to more than one WebSocket extension.

sync-websocket-the-jmap-server-turned-on-a = The JMAP server turned on a WebSocket extension that wasn't asked for.

sync-websocket-the-jmap-server-asked-for-a = The JMAP server asked for a smaller compression window than Picacho uses.

sync-websocket-the-jmap-server-agreed-to-websocket = The JMAP server agreed to WebSocket compression with a setting Picacho doesn't know.

sync-websocket-the-jmap-server-agreed-to-websocket-2 = The JMAP server agreed to WebSocket compression without what was asked.

sync-websocket-the-jmap-server-sent-a-websocket = The JMAP server sent a WebSocket message that doesn't inflate.

sync-websocket-the-jmap-server-sent-too-large = The JMAP server sent too large a WebSocket message.

sync-websocket-the-websocket-address-isnt-valid = The WebSocket address isn't valid.

sync-websocket-the-websocket-address-doesnt-use-wss = The WebSocket address doesn't use wss://.

sync-websocket-the-websocket-address-has-no-host = The WebSocket address has no host.

sync-websocket-tls-with-failed = TLS with { $host } failed: { $error }

sync-websocket-the-jmap-servers-websocket-address-isnt = The JMAP server's WebSocket address isn't valid.

sync-websocket-the-jmap-server-no-longer-accepts = The JMAP server no longer accepts the sign-in.

sync-websocket-the-jmap-server-didnt-open-the = The JMAP server didn't open the WebSocket: { $reply }

sync-websocket-the-jmap-servers-websocket-answer-is = The JMAP server's WebSocket answer is too long.

sync-websocket-the-jmap-servers-answer-didnt-upgrade = The JMAP server's answer didn't upgrade to a WebSocket.

sync-websocket-the-jmap-servers-websocket-answer-doesnt = The JMAP server's WebSocket answer doesn't match the request.

sync-websocket-the-jmap-server-didnt-agree-to = The JMAP server didn't agree to speak JMAP over the WebSocket.

sync-websocket-the-jmap-server-sent-a-websocket-2 = The JMAP server sent a WebSocket frame with reserved bits set.

sync-websocket-the-jmap-server-sent-a-masked = The JMAP server sent a masked WebSocket frame.

sync-websocket-the-jmap-server-sent-an-invalid = The JMAP server sent an invalid WebSocket control frame.

sync-websocket-the-jmap-server-sent-a-websocket-3 = The JMAP server sent a WebSocket continuation without a message.

sync-websocket-the-jmap-server-started-a-websocket = The JMAP server started a WebSocket message inside another.

sync-websocket-the-jmap-server-sent-a-binary = The JMAP server sent a binary WebSocket message.

sync-websocket-the-jmap-server-sent-text-that = The JMAP server sent text that isn't UTF-8.

sync-websocket-the-jmap-server-sent-an-unknown = The JMAP server sent an unknown WebSocket frame.

sync-websocket-could-not-be-resolved = { $host } could not be resolved

sync-websocket-could-not-connect-to = Could not connect to { $host }:{ $port }: { $error }

# src/sync/wire.rs

sync-wire-mail-server-response-exceeds-the-32 = Mail server response exceeds the 32 MB session-command limit

sync-wire-mail-server-command-timed-out = Mail server command timed out

# src/sync/rest.rs, src/sync/gmail.rs and src/sync/graph.rs.

sync-rest-it-no-longer-exists-at-the = It no longer exists at the server.

sync-rest-your-sign-in-expired-reconnect-the = Your sign-in expired. Reconnect the account in Settings.

sync-rest-the-server-pointed-elsewhere = { $service } pointed somewhere else; the request wasn't sent.

sync-rest-couldnt-reach = Couldn't reach { $service }. Check your connection.

sync-rest-didnt-accept-the-sign-in = { $service } didn't accept the sign-in: { $message }

sync-rest-is-limiting-requests = { $service } is limiting requests ({ $status }); Picacho will try again later.

sync-rest-answered = { $service } answered { $status }: { $message }

sync-rest-the-answer-was-too-large = The server's answer was too large.

sync-gmail-gmail-left-a-request-unanswered = Gmail left part of a request unanswered. Try again.

sync-gmail-gmail-sent-no-source = Gmail didn't send the message's source.

sync-gmail-gmail-answered-a-batch-in-a = Gmail answered in a form Picacho doesn't read.

sync-graph-signed-or-encrypted-mail-this-large = Signed or encrypted mail this large can't be sent through Microsoft 365 whole: turn off formatting (it then goes as inline PGP), send the files separately, or turn off Sign and Encrypt.

sync-graph-microsoft-gave-nowhere-to-upload = Microsoft 365 gave nowhere to upload the attachment.

sync-graph-the-message-is-too-large-to = The message is too large to save in one piece at Microsoft 365.

# src/sync/ews.rs.

sync-ews-couldnt-reach-the-exchange-server = Couldn't reach the Exchange server at { $url }. Check your connection.

sync-ews-exchange-didnt-accept-the-sign-in = Exchange didn't accept the sign-in. Check the user name (DOMAIN\name or name@domain) and password.

sync-ews-this-server-accepts-only-basic-sign-in = This Exchange server accepts only Basic sign-in, which sends your password itself instead of proof of it. If you trust the server, turn on “Allow Basic sign-in” and try again.

sync-ews-basic-sign-in-is-only-sent-over-https = Basic sign-in sends your password itself, so Picacho uses it only with the account's own Exchange server over HTTPS.

sync-ews-exchange-wont-hold-the-stream-open = Exchange (or something on the way to it) keeps ending the wait for new mail at once; the inbox is checked every half minute instead.

sync-ews-exchange-answered = Exchange answered { $status }: { $message }

sync-feed-isnt-a-calendar-address = { $url } isn't a calendar's address: it starts with webcal://, https:// or http://.

sync-feed-couldnt-reach = Couldn't reach { $host }

sync-feed-answered = { $host } answered { $status }

sync-feed-is-too-large = The calendar at { $host } is too large to subscribe to.

sync-feed-isnt-a-calendar = What { $host } sent isn't a calendar.

sync-google-youre-not-a-guest-of-this-event = Google doesn't list you as a guest of this event.

sync-proton-pim-proton-answered = Proton answered { $status } (code { $code }): { $message }

sync-proton-pim-this-contact-couldnt-be-read-whole = Part of this Proton contact couldn't be read or verified here, so it isn't changed here: saving it would lose that part. Edit it in Proton Mail.

sync-proton-pim-proton-didnt-take-the-contact = Proton didn't take the contact: { $message }

sync-proton-pim-proton-didnt-take-the-event = Proton didn't take the event: { $message }

sync-proton-pim-this-calendars-keys-couldnt-be = This calendar's keys couldn't be unlocked.

sync-proton-pim-this-proton-calendar-is-read-only = This Proton calendar is read-only.

sync-proton-pim-events-with-guests-are-made-in = Events with guests are made and changed in Proton Calendar, which sends their invitations.

sync-proton-pim-proton-didnt-delete-the-contact = Proton didn't delete the contact.

# src/sync/proton/aliases.rs: Proton's own aliases (hide-my-email aliases,
# kept in Proton Pass). "Proton Pass" stays as it is.

sync-proton-aliases-this-sign-in-has-no-pass-access = Proton didn't give Picacho's sign-in access to Proton Pass, where Proton keeps its aliases (missing: { $missing }).

sync-proton-aliases-pass-has-an-extra-password = Proton refused Proton Pass for this session (code 9108): it needs the account's Pass extra password (enter it with the Proton account in Settings → Forwarding), or, without one, a registration Proton gives Pass access to.

sync-proton-aliases-the-pass-extra-password-is-wrong = Proton didn't accept the Proton Pass extra password.

sync-proton-aliases-pass-is-locked = Proton Pass is locked with a PIN for this session. Unlock it in Proton Pass, then try again.

sync-proton-aliases-this-registration-has-no-pass-access = Proton gives the session Picacho uses for Proton Pass ({ $app }) no Pass access, and the account has no Pass extra password that would. Its scopes: { $scopes }.

sync-proton-aliases-sign-in-to-proton-pass = Sign in to Proton Pass to manage these aliases.

sync-proton-aliases-proton-ended-the-pass-session = Proton ended the Proton Pass session. Sign in to Proton Pass again to manage these aliases.

sync-proton-aliases-addresses-dont-have-that = { $email } is one of the account's own Proton addresses, which don't have that; only Proton Pass's aliases do.

sync-proton-aliases-proton-doesnt-let-this-account-delete-addresses = Proton doesn't let this account delete addresses. Turn it off instead.

sync-proton-aliases-not-a-proton-alias = { $email } isn't one of this Proton account's aliases.

sync-proton-aliases-not-a-proton-contact = That contact isn't one of a Proton alias's.

sync-proton-aliases-not-a-proton-mailbox = That isn't one of Proton's alias mailboxes.

sync-proton-aliases-a-pass-key-wouldnt-open = A Proton Pass key couldn't be opened with this account's keys.

sync-proton-aliases-proton-wont-make-more-aliases = Proton won't make more aliases on this plan.

sync-proton-aliases-proton-no-longer-offers-that-domain = Proton no longer offers that domain. Choose another.

sync-proton-aliases-proton-has-no-mailbox-to-forward = Proton has no mailbox to forward the alias to.

sync-proton-aliases-the-new-alias-was-made-but = The new alias was made, but Proton's reply couldn't be read. Refresh to see it.

sync-proton-aliases-proton-didnt-give-a-reverse-alias = Proton didn't give an address for writing to { $contact }.

sync-sieve-smaller-than-kb = Smaller than (KB)

sync-sieve-has-attachments = Has attachments

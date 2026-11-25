# src/display/filters.rs and nearby views.

filters-text-to-look-for = Text to look for

filters-header-name = Header name

filters-address-reason-or-label = Address, reason or label

filters-out-of-office = Out of office

filters-im-away-until-monday-and-will = I'm away until Monday and will reply when I'm back.

filters-2026-12-20-optional = 2026-12-20 (optional)

filters-2026-12-31-optional = 2026-12-31 (optional)

filters-type-what-the-filter-looks-for = Type what the filter looks for.

filters-choose-what-the-filter-does = Choose what the filter does.

filters-name-the-header-the-filter-looks = Name the header the filter looks at.

filters-enter-the-address-mail-is-sent = Enter the address mail is sent on to.

filters-write-why-mail-is-refused = Write why mail is refused.

filters-name-the-label = Name the label.

filters-filters-saved-on-the-server = Filters saved on the server

filters-from-the-server = From the server: { $t }

filters-server-administrator = Server administrator: { $t }

filters-this-server-also-offers-this-mailbox = This server also offers this mailbox over JMAP ({ $host }), which syncs faster and gets changes pushed. Add it again choosing JMAP to use it.

filters-made-while-offline-will-reach-the = { $count ->
        [one] One change
       *[other] { $shown } changes
    } made while offline will reach the server with the next connection.

filters-asking-the-server = Asking the server…

filters-asking-the-server-for-its-filters = Asking the server for its filters…

filters-this-server-doesnt-offer-filters-managesieve = This server doesn't offer filters (ManageSieve).

filters-try-again = Try Again

filters-archive = Archive

filters-spam = Spam

filters-trash = Trash

filters-keep-in-the-inbox = keep in the inbox

filters-move-to = move to { $folder }

filters-mark-as-read = mark as read

filters-send-a-copy-to = send a copy to { $to }

filters-send-on-to = send on to { $to }

filters-label = label “{ $l }”

filters-header = Header { $header }

filters-larger-than-kb = { $negate ->
        [yes] Not larger than { $kb } KB: { $what }
       *[no] larger than { $kb } KB: { $what }
    }

filters-has-picacho-can-show = “{ $other }” has { $n ->
        [one] { $n } filter
       *[other] { $n } filters
    } Picacho can show{ $left_out ->
        [0] {""}
       *[other] , and { $left_out } more it can't
    }.

filters-edit-them-here = Edit Them Here

filters-keep-running = Keep running “{ $other }”

filters-another-apps-filters-are-on-picachos = Another app's filters are on. Picacho's filters can run them first, so both apply.

filters-another-apps-filters-are-on-this = Another app's filters (“{ $other }”) are on. This server can run only one set, so saving Picacho's turns those off.

filters-no-filters-yet-mail-arriving-on = No filters yet. Mail arriving on the server is sorted by them, even while Picacho is closed.

filters-then-stop = Then stop

filters-then-continue = Then continue

filters-remove-filter = @Remove filter

filters-filters = Filters

filters-delete = Delete

filters-star = Star

filters-mark-read = Mark read

filters-label-2 = Label

filters-send-on = Send on

filters-refuse = Refuse

filters-move-to-folder = Move to folder

filters-new-filter-when = New filter: when

filters-make-a-folder-in-the-sidebar = Make a folder in the sidebar first; filters move mail into the account's own folders.

filters-add-filter = Add Filter

filters-vacation-reply = Vacation reply

filters-the-server-answers-mail-to-this = The server answers mail to this account while you're away, once per sender.

filters-the-server-answers-mail-to-this-2 = The server answers mail to this account's addresses while you're away, once per sender in the days chosen below.

filters-subject = Subject

filters-message = Message

filters-answer-each-sender-once-every = Answer each sender once every

filters-day = Day

filters-3-days = 3 days

filters-week = Week

filters-2-weeks = 2 weeks

filters-only-between = Only between

filters-leave-empty-to-answer-from-now = Leave empty to answer from now until you turn it off.

filters-picachos-filters-are-on-for-this = Picacho's filters are on for this account.

filters-save-to-turn-these-filters-on = Save to turn these filters on.

filters-saving = Saving…

filters-save-filters = Save Filters

# A filter, in the list: "From contains “ana@example.com”: move to Work".
# $field: what it looks at; $negate: yes for "doesn't"; $matching: how it
# compares; $text: the text it looks for; $what: what it does.
filters-rule = { $field } { $negate ->
        [yes] { $matching ->
            [contains] doesn't contain
            [is] doesn't equal
            [matches] doesn't match
           *[pattern] doesn't match the pattern
        }
       *[no] { $matching ->
            [contains] contains
            [is] is
            [matches] matches
           *[pattern] matches the pattern
        }
    } “{ $text }”: { $what }

# What a filter does, in the list of filters
filters-act-star = star

filters-act-delete = delete

# Sends the message back to its sender
filters-act-refuse = refuse

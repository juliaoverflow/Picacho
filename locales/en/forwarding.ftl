# src/display/forwarding.rs and nearby views.

forwarding-description = Description

forwarding-display-name = Display name

forwarding-the-name-recipients-see-like-jane = The name recipients see, like Jane Doe

forwarding-unknown-contact = Unknown contact

forwarding-last-written = Last written { $date }

forwarding-received = Received

forwarding-sent = Sent

forwarding-d-total = { $what } { $recent } ({ $contact_recent_days }d) · { $total }{ $partial ->
        [yes] +
       *[no] {""}
    } total

forwarding-cancel = Cancel

forwarding-forwarding = Forwarding

forwarding-add-service = Add Service

forwarding-add-a-service = Add a Service

forwarding-api-key = API key

forwarding-proton-account = Proton account

forwarding-proton-pass-extra-password = Proton Pass extra password

forwarding-proton-password = Proton password

forwarding-sign-in-to-proton-pass-to-manage = Proton ended the Proton Pass session. Sign in to Proton Pass again to manage these aliases.

forwarding-proton-ended-the-pass-session-sign-in = Proton keeps these aliases in Proton Pass, which signs in on its own. Sign in with the account's Proton password.

forwarding-sign-in = Sign In…

forwarding-sign-in-to-proton-pass-again = Sign In to Proton Pass…

forwarding-signed-in-to-proton-pass = Signed in to Proton Pass

forwarding-proton-pass-signs-in-on-its-own = Proton Pass, where Proton keeps these aliases, signs in on its own: Proton gives the mail sign-in no access to it. Only the Pass session is kept, in the system keychain; the password isn't.

forwarding-only-if-you-protect-proton-pass = Only if you protect Proton Pass with an extra password. It's kept in the system keychain, like API keys.

forwarding-no-proton-account = No Proton account here is signed in directly, or each one's aliases are already listed. Add a Proton account (not through Proton Mail Bridge) first.

forwarding-open = Open

forwarding-server = Server

forwarding-only-for-a-self-hosted-server = Only for a self-hosted server. Blank uses { $default_server }.

forwarding-connecting = Connecting…
forwarding-loading-proton-addresses = Loading your Proton addresses and aliases…

forwarding-connect = Connect

forwarding-alias = { $count ->
        [one] { $count } alias
       *[other] { $count } aliases
    }

forwarding-to = to { $email }

forwarding-to-any-mailbox = to any mailbox

forwarding-more = More

forwarding-remove-its-aliases-keep-working-picacho = Remove { $label }? Its aliases keep working; Picacho stops managing them.

forwarding-remove = Remove

forwarding-a-removed-mailbox = a removed mailbox

forwarding-any-mailbox = Any mailbox

forwarding-where-the-aliases-mail-arrives-any = Where the aliases' mail arrives. Any mailbox looks in every account.

forwarding-send-as-an-alias-by-choosing = Send as an alias by choosing it in From. It goes out from { $mailbox }, which must be a verified mailbox at { $label }.

forwarding-only-relays-replies-to-mail-it = { $label } only relays replies to mail it forwarded; new messages can't be sent as an alias.

forwarding-forwards-to = Forwards to

forwarding-new-alias = New Alias

forwarding-no-mailbox-here-receives-its-aliases = No mailbox here receives its aliases yet; choose one in Forwards to.

forwarding-cant-write-as-new-mail-and = { $aliases } can't write as { $count ->
        [one] itself
       *[other] themselves
    }: new mail and replies go out as an alias, and the alias used is shown wherever its address was, in received and sent mail.

forwarding-send-only-as-aliases = Send only as aliases

forwarding-no-aliases-yet = No aliases yet.

forwarding-no-alias-matches = No alias matches.

forwarding-of-aliases = { $count ->
        [one] { $first }–{ $end } of { $count } alias
       *[other] { $first }–{ $end } of { $count } aliases
    }

forwarding-first = « First

forwarding-previous = ‹ Previous

forwarding-page-of = Page { $page } of { $pages } · { $label }

forwarding-next = Next ›

forwarding-last = Last »

forwarding-a-prefix = a { $to_lowercase } prefix

forwarding-a-random-prefix = a random prefix

forwarding-creates = Creates { $prefix }{ $suffix }

forwarding-creates-with = Creates { $random } with { $suffix }

forwarding-creates-with-this-domain-only-takes = Creates { $random } with { $suffix }. This domain only takes random aliases.

forwarding-loading-domains = Loading domains…

forwarding-default-domain = Default domain

forwarding-wont-make-more-aliases-on-this = { $label } won't make more aliases on this plan.

forwarding-couldnt-list-domains-new-aliases-use = Couldn't list domains: { $e }. New aliases use the default domain.

forwarding-try-again = Try Again

forwarding-new-alias-2 = New alias

forwarding-forwards-to-2 = Forwards to { $email }

forwarding-random-prefix = Random prefix

forwarding-creating = Creating…

forwarding-create = Create

forwarding-forward-mail-sent-to-this-alias = Forward mail sent to this alias

forwarding-off = Off

forwarding-own-account = Own account

forwarding-save = Save

forwarding-delete-mail-sent-to-it-will = Delete { $email }? Mail sent to it will be refused.

forwarding-delete = Delete

forwarding-reverse-aliases = Reverse Aliases

forwarding-refresh = Refresh

forwarding-writing-to-one-of-these-from = Writing to one of these from your mailbox sends as the alias. They're partly hidden here; copy one to use it.

forwarding-loading = Loading…

forwarding-adding = Adding…

forwarding-add = Add

forwarding-delete-the-contact-its-reverse-alias = Delete the contact { $title }? Its reverse alias stops working.

forwarding-none-yet-simplelogin-makes-one-for = None yet. { $label } makes one for each person who writes to this alias, or whom you write to from it; add one above to write to someone new.

forwarding-no-contact-matches = No contact matches.

forwarding-of-contacts = { $total_shown ->
        [one] { $first }–{ $end } of { $total_shown } contact
       *[other] { $first }–{ $end } of { $total_shown } contacts
    }

forwarding-blocked = Blocked

forwarding-copy-reverse-alias = Copy reverse alias

forwarding-copy-reverse-alias-2 = Copy Reverse Alias

forwarding-unblock-contact = Unblock Contact

forwarding-block-contact = Block Contact

forwarding-delete-contact = Delete Contact…

forwarding-contact-deleted = Contact deleted

forwarding-enter-the-address-you-want-to = Enter the address you want to write to.

forwarding-reverse-alias-for-ready-copied = Reverse alias for { $contact } ready · copied

forwarding-default-recipient = Default recipient

forwarding-an-alias-needs-at-least-one = An alias needs at least one mailbox.

forwarding-refresh-aliases = Refresh Aliases

forwarding-remove-service = Remove Service…

forwarding-copy-address = Copy Address

forwarding-add-description = Add Description…

forwarding-edit-description = Edit Description…

forwarding-set-display-name = Set Display Name…

forwarding-edit-display-name = Edit Display Name…

forwarding-unpin = Unpin

forwarding-pin = Pin

forwarding-encrypt-with-pgp = Encrypt with PGP

forwarding-hide-reverse-aliases = Hide Reverse Aliases

forwarding-show-reverse-aliases = Show Reverse Aliases

forwarding-show-in-its-mailbox = Show in Its Mailbox

forwarding-show-as-its-own-account = Show as Its Own Account

forwarding-delete-alias = Delete Alias…

forwarding-delete-address = Delete Address…

forwarding-the-mailbox-you-write-from = the mailbox you write from

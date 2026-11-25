# Text Picacho writes into mail other people receive: a reply's or
# forward's introduction, invitations, and read receipts. It's written in
# the language Settings → General chooses for outgoing mail (the
# interface's, unless another is chosen). "Re:" and "Fwd:" before subjects
# stay as they are: mail programs everywhere read them.

# Before the quoted original in a reply. Picacho also looks for this line
# in replies, to tell what was written from what's quoted.
outgoing-wrote = On { $date }, { $name } wrote:

# Starts a forwarded message's header, on its own line. Picacho also looks
# for this line, to tell what was written from what's forwarded.
outgoing-forwarded-message = ---------- Forwarded message ----------

# The forwarded message's header, below the line above. $name and $address:
# its sender's name and address.
outgoing-forward-header =
    From: { $name } <{ $address }>
    Date: { $date }
    Subject: { $subject }
    To: { $to }

# Below the forwarded message's header, when it had copies
outgoing-forward-cc = Cc: { $cc }

# An invitation's subject. $event: the event's title
outgoing-invitation = Invitation: { $event }

# A cancelled event's notice's subject
outgoing-cancelled = Cancelled: { $event }

# Starts a cancelled event's notice, before when it was
outgoing-was-cancelled = { $event } was cancelled.

# Starts an invitation, before when and where. $name: who invites
outgoing-invites-you = { $name } invites you to { $event }.

# Starts an invitation sent again to one guest who asked for it
outgoing-the-event-as-it-is-now = The event as it is now.

# Starts a proposal of another time for an event, before the time proposed
outgoing-proposes = { $name } proposes another time for { $event }:

# A read receipt's subject. $subject: the message's
outgoing-read = Read: { $subject }

# A read receipt's text, when the mail server writes the rest of it
outgoing-your-message-was-displayed = Your message “{ $subject }” was displayed.

# Starts a read receipt. $address: who it was shown to; $date: when
outgoing-your-message-to-was-displayed-on = Your message to { $address } was displayed on { $date }.

# In a read receipt, below the line above
outgoing-receipt-subject = Subject: { $subject }

# Ends a read receipt
outgoing-this-only-says-the-message-was = This only says the message was shown; it doesn't guarantee that it was read or understood.

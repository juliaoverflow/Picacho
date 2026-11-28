# src/display/compose.rs and nearby views.

compose-that-email-link-isnt-valid = That email link isn't valid.

compose-this-scheduled-message-is-being-sent = This scheduled message is being sent now.

compose-new-message = New Message

compose-connect-an-account-before-sending-your = Connect an account before sending. Your draft is saved.

compose-message-sent = Message sent

compose-message-sent-waiting = Message sent. It's in Waiting For until someone replies.

compose-message-sent-2 = Message sent. { $notes }

compose-scheduled-to-send = Scheduled to send { $when }.

compose-handed-to-the-server-which-sends = Handed to the server, which sends it { $when }, even with Picacho closed.

compose-the-server-couldnt-take-it-picacho = The server couldn't take it ({ $e }); Picacho sends it { $when }.

compose-its-account-was-removed = Its account was removed.

compose-scheduled-message-sent = Scheduled message sent. { $notes }

compose-a-scheduled-message-wasnt-sent-its = A scheduled message wasn't sent: { $error } It's in Drafts.

compose-from-was-reset-to = { $gone } From was reset to { $from }.

compose-sending-messages = { $count ->
        [one] Sending { $count } message…
       *[other] Sending { $count } messages…
    }

compose-sending = Sending…

compose-undo = Undo

compose-merge-sent = { $count ->
        [one] Sent 1 message
       *[other] Sent { $count } messages
    }

compose-merge-some-failed = Sent { $sent } of { $count }. Not sent: { $failed }

compose-merge-already-sending = A mail merge is being sent already; this one can go once it's done.

compose-merge-resuming = { $count ->
        [one] Sending the mail merge’s last message, left when Picacho stopped
       *[other] Sending the mail merge’s { $count } messages left when Picacho stopped
    }

# src/display/later.rs and nearby views.

later-snoozing-needs-mail-kept-on-the = Snoozing needs mail kept on the server (IMAP).

later-snooze-until = Snooze until

later-snoozed-messages-until = { $count ->
        [1] Snoozed until { $when }
       *[other] Snoozed { $count } messages until { $when }
    }

later-couldnt-snooze = Couldn't snooze: { $e }

later-back-from-snooze = Back from snooze: { $subject }

later-snoozed-messages-are-back-in-the = { $count } snoozed messages are back in the inbox

later-couldnt-bring-back-snoozed-mail = Couldn't bring back snoozed mail: { $e }

later-unsnooze = Unsnooze

later-snooze-until-another-time = @Snooze until another time

later-snoozed-until-it-comes-back-to = Snoozed until { $when }. It comes back to the inbox, unread.

later-searching-on-the-server = Searching { $mailbox_label } on the server…

later-try-again = Try Again

later-the-newest-of-matches-on-the = { $count ->
        [one] The newest { $newest } of { $total } match on the server
       *[other] The newest { $newest } of { $total } matches on the server
    }

later-no-matches-on-the-server = No matches on the server

later-matches-on-the-server = { $count ->
        [one] { $shown } match on the server
       *[other] { $shown } matches on the server
    }

later-show-local-matches = Show Local Matches

later-only-mail-on-this-computer-is = Only mail on this computer is searched.

later-search-gmail = Search Gmail · ↩

later-search-the-server = Search the Server · ↩

later-dont-send = Don't Send

later-send-receipt = Send Receipt

later-asks-to-be-told-when-youve = { $sender } asks to be told when you've read this message.

later-asks-that-be-told-when-youve = { $sender } asks that { $receipt_to } be told when you've read this message.

later-read-receipt-sent = Read receipt sent

later-the-read-receipt-wasnt-sent = The read receipt wasn't sent: { $e }

later-the-originals-attachments-couldnt-be-added = The original's attachments couldn't be added: { $e }

# src/display/subscriptions.rs and nearby views.

subscriptions-mark-all-as-read = Mark All as Read

subscriptions-mark-all-as-unread = Mark All as Unread

subscriptions-move-all-to-inbox = Move All to Inbox

subscriptions-archive-all = Archive All

subscriptions-move-all-to-spam = Move All to Spam

subscriptions-move-all-to-trash = Move All to Trash

subscriptions-move-all-to-folder = Move All to Folder…

subscriptions-message = { $count ->
        [one] { $shown } message
       *[other] { $shown } messages
    }

subscriptions-marked-from-as-read = Marked { $messages } from { $who } as read

subscriptions-marked-from-as-unread = Marked { $messages } from { $who } as unread

subscriptions-moved-from-to = Moved { $messages } from { $who } to { $to }

subscriptions-couldnt-change-s-mail = Couldn't change { $who }'s mail: { $e }

subscriptions-add-this-mailboxs-account-to-unsubscribe = Add this mailbox's account to unsubscribe.

subscriptions-checking-how-to-unsubscribe-from = Checking how to unsubscribe from { $who }…

subscriptions-couldnt-unsubscribe = Couldn't unsubscribe: { $e }

subscriptions-email-to-unsubscribe = Email { $to } to unsubscribe?

subscriptions-a-message-saying-unsubscribe-with-the = A message saying "unsubscribe", with the subject "{ $subject }", will be sent from { $email } to { $to }.

subscriptions-send = Send

subscriptions-open-s-unsubscribe-page = Open { $who }'s unsubscribe page?

subscriptions-will-open-in-your-browser = { $url } will open in your browser.

subscriptions-open = Open

subscriptions-the-senders-signature-doesnt-cover-this =
    {""}

    The sender's signature doesn't cover this request, so it may not come from { $address }.

subscriptions-cancel = Cancel

subscriptions-unsubscribed-from = Unsubscribed from { $who }

subscriptions-picacho-only-opens-web-and-email = Picacho only opens web and email links.

subscriptions-finish-unsubscribing-from-on-the-page = Finish unsubscribing from { $who } on the page that opened

subscriptions-unsubscribed = Unsubscribed

subscriptions-unsubscribing = Unsubscribing…

subscriptions-unsubscribe = Unsubscribe

subscriptions-this-message-is-from-a-mailing = This message is from a mailing list or subscription.

subscriptions-subscriptions = Subscriptions

subscriptions-choose-a-subscription-to-see-its = Choose a subscription to see its mail

subscriptions-subscribed = Subscribed · { $shown }

subscriptions-unsubscribed-2 = Unsubscribed · { $shown }

subscriptions-senders-whose-mail-offers-to-unsubscribe = Senders whose mail offers to unsubscribe

subscriptions-sender-message-unread = { $senders ->
        [one] { $senders_shown } sender
       *[other] { $senders_shown } senders
    } · { $messages ->
        [one] { $messages_shown } message
       *[other] { $messages_shown } messages
    } · { $unread } unread

subscriptions-loading = Loading…

subscriptions-you-havent-unsubscribed-from-anything-yet = You haven't unsubscribed from anything yet

subscriptions-no-subscriptions-found = No subscriptions found

subscriptions-more-actions = @More actions

subscriptions-unread = { $count ->
        [one] { $shown } unread
       *[other] { $shown } unread
    }

subscriptions-message-2 = { $count ->
        [one] { $shown } message
       *[other] { $shown } messages
    }

subscriptions-selected = { $count ->
       *[other] { $shown } selected
    }

subscriptions-mark-as-read = @Mark as read

subscriptions-move-to-trash = @Move to trash

subscriptions-done = Done

subscriptions-back-to-subscriptions = @Back to subscriptions

subscriptions-message-unread = { $address } · { $count ->
        [one] { $shown } message
       *[other] { $shown } messages
    } · { $unread } unread

subscriptions-no-messages = No messages

subscriptions-back-to-the-list = @Back to the list

subscriptions-subscriptions-2 = Subscriptions › { $name }

# When the latest email came, after how many there are
subscriptions-last-email = Last email { $date }

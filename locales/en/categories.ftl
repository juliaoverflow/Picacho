# src/manage/categories.rs and the inbox's category tabs (src/display/categories.rs).

categories-primary = Primary

categories-transactions = Transactions

categories-travel = Travel

categories-finance = Finance

categories-updates = Updates

categories-newsletters = Newsletters

categories-promotions = Promotions

categories-social = Social

categories-forums = Forums

categories-primary-about = Mail from people, and anything not sorted elsewhere

categories-transactions-about = Receipts, orders and package updates

categories-updates-about = Notifications, alerts and reminders

categories-travel-about = Flights, trains, hotels and other bookings

categories-finance-about = Banks, cards, bills, statements and payments

categories-newsletters-about = Newsletters and digests to read

categories-all-mail = All Mail

categories-all-mail-about = Everything in the inbox, each message tagged with its category

categories-more-tabs = More categories

categories-earlier-tabs = Earlier categories

categories-promotions-about = Offers, deals and marketing

categories-social-about = Social networks, dating and sharing sites

categories-forums-about = Discussion lists and online groups

categories-tabs = Inbox categories

categories-unread = { $count ->
        [one] { $shown } unread
       *[other] { $shown } unread
    }

categories-nothing-in = Nothing in { $category }

categories-move-to-category = Move to Category…

categories-move-to = Move to { $category }

categories-moved-sender = Mail from { $from } goes to { $category } from now on

categories-moved-senders = { $count ->
        [one] Mail from { $count } sender goes to { $category } from now on
       *[other] Mail from { $count } senders goes to { $category } from now on
    }

categories-show-categories = Show Categories

categories-section = Inbox Categories

categories-sort-the-inbox = Sort the inbox into categories

categories-sort-the-inbox-caption = Tabs above the inbox: Primary, Promotions, Social and others, then All Mail. A category without a tab shows in the one it belongs to.

categories-all-mail-first = Open the inbox on All Mail

categories-all-mail-first-caption = All Mail becomes the first tab, before Primary

categories-senders = Sorted senders

categories-senders-none = Right-click a message and choose Move to Category… to always sort its sender there.

categories-sender-goes-to = { $from } → { $category }

categories-forget-sender = Forget

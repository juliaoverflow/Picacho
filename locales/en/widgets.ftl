# The home screen and desktop widgets' own words (src/manage/widgets.rs,
# labels), for widgets that take them from Picacho: KDE Plasma's, and the
# Windows widgets' counts. (Android's, iOS's and Windows' other words are
# in their own files: android/app/src/main/res/values/strings.xml,
# packaging/ios/Widgets/Localizable.xcstrings and
# packaging/windows/widgets/Strings.resx.)

widgets-inbox = Inbox

widgets-all-inboxes = All Inboxes

widgets-all-accounts = All Accounts

# Labels the account choice in a widget's settings
widgets-account = Account:

# An account in a widget's settings: its name, then its address
widgets-account-name = { $name } ({ $address })

widgets-show-unread-messages-only = Show unread messages only

# The Inbox widget's title when it shows only unread messages. $title: the account, or All Inboxes
widgets-title-unread = { $title } · Unread

# Button
widgets-refresh = Refresh

# Button: write a new message
widgets-compose = Compose

widgets-unknown-sender = (Unknown sender)

widgets-open-picacho-to-add-an-account = Open Picacho to add an account

# No unread messages
widgets-all-caught-up = All caught up

widgets-no-messages-in-inbox = No messages in Inbox

widgets-today = Today

widgets-tomorrow = Tomorrow

# An event that lasts all day, after its day
widgets-all-day = All day

# The Calendar widget's title
widgets-calendar = Calendar

widgets-no-upcoming-events = No upcoming events

widgets-connect-a-calendar-in-settings = Connect a calendar in Settings

# The Packages widget's title
widgets-packages = Packages

# The Packages widget's title when it shows one account's
widgets-packages-of = Packages · { $account }

# When a package should arrive. $date: for example Nov 26
widgets-expected = Expected { $date }

widgets-no-recent-packages = No recent packages

# A tooltip: the widget's packages or receipts open in Picacho's Summary
widgets-open-them-in-the-summary = Open them in Picacho's Summary

# The Receipts widget's title
widgets-receipts = Receipts

# The Receipts widget's title when it shows one account's
widgets-receipts-of = Receipts · { $account }

# Stands in for a receipt without a shop name
widgets-receipt = Receipt

# A receipt's order number
widgets-order = Order { $number }

widgets-no-recent-receipts = No recent receipts

# Opens the whole list of packages or receipts
widgets-view-all = { $count ->
        [one] View all { $count }
       *[other] View all { $count }
    }

# Under the unread count, which is shown larger above it
widgets-unread = { $count ->
        [one] unread message
       *[other] unread messages
    }

# The KDE Plasma widgets' names and descriptions, in Plasma's list of
# widgets to add
widgets-plasmoid-inbox = Picacho Inbox

widgets-plasmoid-inbox-about = Recent inbox messages, unread count, and quick compose for Picacho

widgets-plasmoid-calendar = Picacho Calendar

widgets-plasmoid-calendar-about = Upcoming calendar events from Picacho

widgets-plasmoid-packages = Picacho Packages

widgets-plasmoid-packages-about = Your recent tracked packages and delivery progress from Picacho

widgets-plasmoid-receipts = Picacho Receipts

widgets-plasmoid-receipts-about = Your recent receipts and refunds from Picacho

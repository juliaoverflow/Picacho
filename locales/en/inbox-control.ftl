# src/display/inbox_control.rs and src/manage/inbox_control.rs.

inbox-control-inbox-control = INBOX CONTROL

inbox-control-vip = VIP

inbox-control-vips = VIPs

inbox-control-screener = Screener

inbox-control-notifications = Notifications

inbox-control-quiet-hours = Quiet Hours

inbox-control-vip-placeholder = Address or @domain, as boss@work.example

inbox-control-starts-placeholder = From, as 22:00

inbox-control-ends-placeholder = Until, as 07:00

inbox-control-day-mon = Mon

inbox-control-day-tue = Tue

inbox-control-day-wed = Wed

inbox-control-day-thu = Thu

inbox-control-day-fri = Fri

inbox-control-day-sat = Sat

inbox-control-day-sun = Sun

inbox-control-every-day = Every day

inbox-control-weekdays = Weekdays

inbox-control-weekends = Weekends

inbox-control-no-days = No days

inbox-control-notify-everything = Everything

inbox-control-notify-primary-and-vips = Primary & VIPs

inbox-control-notify-vips-only = VIPs only

inbox-control-add-to-vips = Add Sender to VIPs

inbox-control-remove-from-vips = Remove Sender from VIPs

inbox-control-approve-sender = Approve Sender

inbox-control-route-to-category = Approve into a Category…

inbox-control-block-sender = Block Sender

inbox-control-now-a-vip = { $from } is a VIP: their mail is marked and always announced

inbox-control-no-longer-a-vip = { $from } is no longer a VIP

inbox-control-approved = Approved { $from }: their mail arrives in your inbox

inbox-control-approved-several = Approved { $count ->
        [one] { $count } sender
       *[other] { $count } senders
    }: their mail arrives in your inbox

inbox-control-blocked-several = Blocked { $count ->
        [one] { $count } sender
       *[other] { $count } senders
    }: their mail goes to Trash

inbox-control-enter-times-like = Enter times like 22:00 and 07:00

inbox-control-choose-a-day = Choose the days it applies to

inbox-control-paused-until = Notifications paused until { $time }

inbox-control-back-to-inbox = Back to Inbox

inbox-control-waiting-in-screener = { $count ->
        [one] { $count } message from someone new is waiting in the Screener
       *[other] { $count } messages from people new to you are waiting in the Screener
    }

inbox-control-review = Review

inbox-control-vips-about = Mail from your VIPs

inbox-control-screener-empty = No one new is waiting: first-time senders' mail shows here until you decide

inbox-control-first-time-sender = { $count ->
        [one] First email from this sender
       *[other] { $count } emails from this first-time sender
    }

inbox-control-approve = Approve

inbox-control-category = Category

inbox-control-block = Block

inbox-control-remove = Remove

inbox-control-vips-caption = VIPs' mail is marked with a crown, gathered under VIPs in the sidebar, and announced whatever the notification settings below say.

inbox-control-add-vip = Add VIP

inbox-control-screen-new-senders = Screen first-time senders

inbox-control-screen-new-senders-caption = Mail from someone not in your contacts and never written to waits in the Screener, unannounced, until you approve, block or route them. Nothing moves on the server.

inbox-control-none-approved = No senders approved yet

inbox-control-approved-sender = { $from } is let in

inbox-control-forget = Forget

inbox-control-alert-for = Alert for

inbox-control-alert-for-caption = Which new mail Picacho announces. VIPs are always announced.

inbox-control-alert-category = Alert for { $category }

inbox-control-quiet-hours-caption = Nothing is announced during these times; mail still arrives.

inbox-control-window = { $start } – { $end }

inbox-control-add-quiet-time = Add a quiet time

inbox-control-add = Add

inbox-control-vips-break-through = Let VIPs through

inbox-control-vips-break-through-caption = Announce VIPs' mail during quiet hours

inbox-control-resume = Resume

inbox-control-pause-notifications = Pause notifications

inbox-control-for-an-hour = For an Hour

inbox-control-until-tomorrow = Until Tomorrow Morning

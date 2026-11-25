# src/display/datetime.rs and nearby views.

datetime-type-a-time-tmrw-at-6 = Type a time: tmrw at 6, next fri 9am, in 2 hrs…

datetime-the-first-time-of-a-repeating = { $when } · the first time of a repeating time ({ $repeat })

datetime-not-a-time-picacho-can-read = Not a time Picacho can read. Try “tomorrow at 6”.

datetime-choose-a-time-in-the-future = Choose a time in the future.

datetime-that-time-doesnt-exist-here-the = That time doesn't exist here (the clocks change). Choose another.

datetime-mo = Mo

datetime-tu = Tu

datetime-we = We

datetime-th = Th

datetime-fr = Fr

datetime-sa = Sa

datetime-su = Su

datetime-cancel = Cancel

# How often a time repeats, after "the first time of a repeating time"
datetime-repeat = { $unit ->
        [minute] { $interval ->
            [1] every minute
           *[other] every { $interval } minutes
        }
        [hour] { $interval ->
            [1] every hour
           *[other] every { $interval } hours
        }
        [day] { $interval ->
            [1] every day
           *[other] every { $interval } days
        }
        [week] { $interval ->
            [1] every week
           *[other] every { $interval } weeks
        }
        [month] { $interval ->
            [1] every month
           *[other] every { $interval } months
        }
       *[year] { $interval ->
            [1] every year
           *[other] every { $interval } years
        }
    }

# $repeat: how often (the message above); $days: the weekdays, for example "Mon, Wed"
datetime-repeat-on-days = { $repeat } on { $days }

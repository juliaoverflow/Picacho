# Dates and times (src/i18n/dates.rs).
#
# The dates-… patterns below are chrono patterns
# (docs.rs/chrono/latest/chrono/format/strftime): %Y year, %-d day, %b and
# %B short and long month name, %a and %A short and long weekday name,
# %H or %-I hour (24 or 12 hours), %M minute, %p AM or PM. Put them in your
# language's order, with its own words and punctuation around them; the
# names come from the messages at the end of this file.

# A time of day in lists and notices (12 or 24 hours, as your language
# usually writes them)
dates-time = %-I:%M %p

# A time of day in the calendar and in events
dates-calendar-time = %H:%M

# When a scheduled message is sent or a snoozed one comes back ("tomorrow at 8:00")
dates-later-time = %-H:%M

# A day of this year ("Nov 26", and a package's expected arrival)
dates-month-day = %b %-d

# A day of any year
dates-month-day-year = %b %-d, %Y

# A day with its weekday
dates-weekday-month-day = %a, %b %-d

# A day with its weekday, of any year
dates-weekday-month-day-year = %a, %b %-d %Y

# A day of this year and a time
dates-month-day-time = %b %-d, %H:%M

# A day with its weekday, and a time
dates-weekday-month-day-time = %a, %b %-d, %H:%M

# A day with its weekday, of any year, and a time
dates-weekday-month-day-year-time = %a, %b %-d %Y, %H:%M

# A month, as a heading ("November 2026")
dates-month-year = %B %Y

# A day written out in full ("Thursday, November 26, 2026"), as in the
# calendar's day heading and in invitations
dates-long-date = %A, %B %-d, %Y

# A day as a template's {date} writes it into a message
dates-month-name-day-year = %B %-d, %Y

# A message's date and time, in the reader and in a reply's or forward's
# introduction
dates-full = %b %-d, %Y · %-I:%M %p

# When a message was shown, in a read receipt ("Your message … was displayed on …")
dates-receipt = %a, %b %-d %Y at %-H:%M

# The last day an event repeats ("Every week, until Nov 26, 2026")
dates-repeat-until = %b %-d, %Y

# When an invitation's event starts (followed by when it ends)
dates-event-start = %A, %B %-d, %Y, %H:%M

# When an invitation's event ends, on the day it starts
dates-event-end = %H:%M

dates-month = { $month ->
        [1] January
        [2] February
        [3] March
        [4] April
        [5] May
        [6] June
        [7] July
        [8] August
        [9] September
        [10] October
        [11] November
       *[12] December
    }

dates-month-short = { $month ->
        [1] Jan
        [2] Feb
        [3] Mar
        [4] Apr
        [5] May
        [6] Jun
        [7] Jul
        [8] Aug
        [9] Sep
        [10] Oct
        [11] Nov
       *[12] Dec
    }

# 1 is Monday
dates-weekday = { $day ->
        [1] Monday
        [2] Tuesday
        [3] Wednesday
        [4] Thursday
        [5] Friday
        [6] Saturday
       *[7] Sunday
    }

dates-weekday-short = { $day ->
        [1] Mon
        [2] Tue
        [3] Wed
        [4] Thu
        [5] Fri
        [6] Sat
       *[7] Sun
    }

# Morning or afternoon, after a 12-hour time
dates-am-pm = { $half ->
        [am] AM
       *[pm] PM
    }

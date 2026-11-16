"""Synthetic text for training the date and time model.

Every example is a short text someone might type where Picacho asks when
("snooze until", "send later"): a time expression ("tmrw at 6", "next fri
morning", "every other monday at 9", "in 2 hrs") on its own or inside other
words ("remind me to call mom tomorow at 6 pls"), or text with no time in it
at all, much of it with words that look like one ("may i", "sat down", "the
second one", "room 204", "6 apples"). Each token is labelled with what it
means (see `features.LABELS`); words that are no part of a time are "O".

Typing is untidy, so the text is too: common misspellings and abbreviations
("tmrw", "wensday", "febuary", "hrs", "2nite"), random slips (a letter
dropped, doubled, swapped or hit beside), no capitals or all capitals,
missing spaces ("6pm", "@6"), and broken grammar ("in 2 hour", "6 tomorow",
"fri 3pm"). The less common spellings and some of the surrounding wording
are split in two by `split`, so the held-out half measures how the model
does on spellings and wording it never saw.
"""

import random

from features import tokens

# Ways of writing each meaning. The first is the usual spelling; the rest are
# abbreviations and common misspellings.
FORMS = {
    "TODAY": ["today", "tdy", "2day", "todya", "toady", "tday", "todat", "tody"],
    "TOMORROW": ["tomorrow", "tmrw", "tmr", "tmrow", "tomorow", "tommorow", "tommorrow",
                 "tomorrw", "tmw", "2moro", "2morrow", "tomoz", "tomorro", "tomrrow",
                 "tomorrrow", "tmrrw", "tomm", "tomrw", "tomorroe", "tomorriw"],
    "YESTERDAY": ["yesterday", "yday", "yest", "yesterdy", "yestarday", "yesteday"],
    "TONIGHT": ["tonight", "tonite", "2nite", "tonihgt", "tonigt", "tnight", "toinght", "2night"],
    "NOW": ["now", "rn", "nw"],
    "LATER": ["later", "l8r", "ltr", "latr", "laterr"],
    "MON": ["monday", "mon", "mondya", "monady", "mnday", "mondy", "moday"],
    "TUE": ["tuesday", "tue", "tues", "tuesdy", "teusday", "tuseday", "tusday", "tuesay"],
    "WED": ["wednesday", "wed", "weds", "wensday", "wednsday", "wendsday", "wedensday",
            "wednesay", "wenesday", "wednessday"],
    "THU": ["thursday", "thu", "thur", "thurs", "thrs", "thursdy", "thrusday", "thurday", "thusday"],
    "FRI": ["friday", "fri", "firday", "fridya", "frday", "friady", "fryday", "fridy", "frid"],
    "SAT": ["saturday", "sat", "satuday", "saturdy", "saterday", "satruday", "satarday", "satday"],
    "SUN": ["sunday", "sun", "sundya", "sundy", "sundat", "suday", "sumday"],
    "JAN": ["january", "jan", "janurary", "jaunary", "janury", "januray"],
    "FEB": ["february", "feb", "febuary", "febrary", "feburary", "febr"],
    "MAR": ["march", "mar", "mrach", "marhc"],
    "APR": ["april", "apr", "aprl", "apirl", "aprli"],
    "MAY": ["may"],
    "JUN": ["june", "jun", "juen", "jnue"],
    "JUL": ["july", "jul", "jluy", "juyl"],
    "AUG": ["august", "aug", "agust", "augst", "auguts"],
    "SEP": ["september", "sep", "sept", "septmber", "setpember", "septembr"],
    "OCT": ["october", "oct", "ocotber", "octobr", "octber", "ocober"],
    "NOV": ["november", "nov", "novmber", "novemeber", "novembr", "noveber"],
    "DEC": ["december", "dec", "decmber", "decembr", "decemebr", "dicember"],
    "U_MIN": ["minutes", "minute", "min", "mins", "mnts", "minuts", "minits", "mintues", "minites", "mints"],
    "U_HOUR": ["hours", "hour", "hr", "hrs", "hor", "hourz", "horus", "huors", "hous", "houres"],
    "U_DAY": ["days", "day", "dys", "dayz", "dyas", "dsys"],
    "U_WEEK": ["weeks", "week", "wk", "wks", "weks", "weeek", "weekz", "wekks"],
    "U_MONTH": ["months", "month", "mos", "mth", "mnth", "mnths", "monthes", "moths", "montsh"],
    "U_YEAR": ["years", "year", "yr", "yrs", "yeras", "yaer", "yeers", "yearz"],
    "WEEKEND": ["weekend", "wknd", "wkend", "weekned", "weeknd", "wekend"],
    "WEEKDAYS": ["weekday", "weekdays", "wkday", "wkdays", "weekdy", "weekdys"],
    "MORNING": ["morning", "morn", "mornin", "morng", "mornig", "moring", "mornng", "monring"],
    "AFTERNOON": ["afternoon", "aftn", "arvo", "afternon", "aftrnoon", "aftenoon", "afternooon", "aftrnn"],
    "EVENING": ["evening", "eve", "evenin", "evning", "evenng", "eveing", "evneing", "evenign"],
    "NIGHT": ["night", "nite", "nigth", "nght", "nihgt"],
    "NOON": ["noon", "midday", "lunch", "lunchtime", "nooon", "noo"],
    "MIDNIGHT": ["midnight", "midnite", "midnigth", "midngiht", "midnght"],
    "HALF": ["half", "haf", "hlf", "hafl"],
    "QUARTER": ["quarter", "qtr", "quater", "quartr", "quartre"],
    "A": ["a", "an"],
    "COUPLE": ["couple", "coupla", "cpl", "coulpe"],
    "FEW": ["few", "several", "fw"],
    "AT": ["at", "@", "around", "about", "abt", "arround", "aroun", "ard", "att"],
    # Not a meaning of its own ("on friday" is "friday"): labelled O.
    "ON": ["on", "by", "onn"],
    "IN": ["in", "within", "inn", "n"],
    "NEXT": ["next", "nxt", "nex", "following", "nextt"],
    "THIS": ["this", "ths", "coming", "dis", "thsi"],
    "LAST": ["last", "previous", "prev", "lst"],
    "EVERY": ["every", "each", "evry", "evey", "eveyr", "everyy", "evrey"],
    "OTHER": ["other", "othr", "othe", "alternate"],
    "AFTER": ["after", "aftr", "afer"],
    "AGO": ["ago", "agao", "agoo"],
    "PAST": ["past", "after", "passed", "pas", "pst"],
    "TO": ["to", "till", "til", "before", "too"],
    "AND": ["and", "n", "nd", "an"],
    "END": ["end", "close", "ned"],
    "EOD": ["eod", "cob", "eob"],
    "EOW": ["eow"],
    "DAILY": ["daily", "everyday", "dialy", "daly"],
    "WEEKLY": ["weekly", "weekyl", "wkly"],
    "MONTHLY": ["monthly", "monthy", "mnthly"],
    "YEARLY": ["yearly", "annually", "anually", "yearl"],
    "HOURLY": ["hourly", "horly"],
    "OVERMORROW": ["overmorrow", "overmorow", "ovrmorrow", "overmoro"],
    "U_FORTNIGHT": ["fortnight", "fortnights", "fortnite", "fortnigth", "fornight"],
    "TOP": ["top", "tp", "topp"],
    "SAME": ["same", "this", "sme"],
    "START": ["start", "beginning", "begining", "strt", "early", "starting"],
    "MID": ["middle", "mid", "midle", "midway"],
    "FROM": ["from", "frm", "fr0m", "after"],
}

# Number words, as counts and as ordinals ("twenty", "twentieth").
NUMBER_WORDS = {
    1: ("one", "first"), 2: ("two", "second"), 3: ("three", "third"), 4: ("four", "fourth"),
    5: ("five", "fifth"), 6: ("six", "sixth"), 7: ("seven", "seventh"), 8: ("eight", "eighth"),
    9: ("nine", "ninth"), 10: ("ten", "tenth"), 11: ("eleven", "eleventh"),
    12: ("twelve", "twelfth"), 13: ("thirteen", "thirteenth"), 14: ("fourteen", "fourteenth"),
    15: ("fifteen", "fifteenth"), 16: ("sixteen", "sixteenth"), 17: ("seventeen", "seventeenth"),
    18: ("eighteen", "eighteenth"), 19: ("nineteen", "nineteenth"), 20: ("twenty", "twentieth"),
    30: ("thirty", "thirtieth"), 40: ("forty", "fortieth"), 50: ("fifty", "fiftieth"),
}
NUMBER_TYPOS = {
    2: ["tow"], 3: ["thre", "tree"], 4: ["for"], 5: ["fiv"], 6: ["sx", "siz"], 7: ["sevn", "sven"],
    8: ["eigth", "eihgt", "ate"], 9: ["nien", "nin"], 10: ["tne"], 11: ["elevn", "elven"],
    12: ["twelv", "twleve"], 15: ["fiveteen", "fifteeen"], 20: ["tweny", "twnety"],
    30: ["thrity", "thirtty"], 40: ["fourty", "froty"], 50: ["fivety", "fifity"],
}

WEEKDAYS = ["MON", "TUE", "WED", "THU", "FRI", "SAT", "SUN"]
MONTHS = ["JAN", "FEB", "MAR", "APR", "MAY", "JUN", "JUL", "AUG", "SEP", "OCT", "NOV", "DEC"]
UNITS = ["U_MIN", "U_HOUR", "U_DAY", "U_WEEK", "U_MONTH", "U_YEAR"]
PARTS = ["MORNING", "AFTERNOON", "EVENING", "NIGHT"]

# Words around a time: what someone types before and after it. Their tokens
# are "O", including the ones that look like a time ("may", "second").
PREFIXES = [
    "", "", "", "", "", "remind me", "remind me to call mom", "snooze till", "snooze until",
    "snooze", "send", "send it", "send this", "schedule for", "schedule it", "lets do",
    "let's meet", "meet", "call bob", "ping me", "can you remind me", "remind me to pay rent",
    "follow up", "book it", "set for", "move it to", "push to", "reschedule to", "do it",
    "pls remind me", "need this", "dentist", "gym", "standup", "dinner with sam", "pick up kids",
    "check back", "bring it back", "show me again", "wake me up", "deliver", "due",
    "remind me about the second draft", "may i get a reminder", "ask jo again", "submit report",
    "renew passport", "pay the 6 invoices", "call the 3 of them", "hmm", "um", "ok so",
    "wait till", "hold until", "not before", "anytime", "maybe", "how about", "what about",
    "try", "lmk", "back", "return", "followup", "remind", "rmd me", "snoze until", "snoz",
    "remidn me", "reminde me", "sned it", "schedual for", "shcedule", "re-send", "resend it",
    "email them", "text her", "reply", "standup with the team", "check the oven", "flight",
    "bday party", "the meeting", "meeting w/ legal", "1:1 with ana", "call room 204",
    "page me", "nudge me", "bump this", "come back to this", "look at it again", "revisit",
]
SUFFIXES = [
    "", "", "", "", "", "", "pls", "plz", "please", "thx", "thanks", "ok", "?", "!", ".",
    "lol", "if possible", "or so", "ish", "maybe", "for the call", "for lunch", "with the team",
    "at the office", "in the office", "at home", "on zoom", "for 30 min", "for a bit",
    "is fine", "works", "works for me", "would be great", "thank u", "ty", "asap", "k",
    "if thats ok", "before the meeting", "after work", "sharp", "latest",
    "for the second time", "again", "pls and thx", "kthx", "...", "!!", "??", "no rush",
]
# Text with no time in it, much of it with words that look like one.
NEGATIVES = [
    "hello there", "thanks for the update", "can you send me the file", "may i ask a question",
    "i sat down with the team", "the sun is out", "the second one is better", "room 204",
    "i have 6 apples", "call 555 0199", "version 2.3", "page 21", "chapter 3", "top 10 list",
    "we came 1st place", "the 2nd floor", "march on", "may the best one win", "at home",
    "in the office", "next to the door", "this is it", "last thing", "every time", "one more thing",
    "a few people", "a couple of things", "half the team", "a quarter of the budget",
    "on the table", "in the box", "the end", "end of story", "the other one", "after all",
    "now what", "ok", "yes", "no", "sure", "sounds good", "lol", "idk", "what", "huh",
    "remind me", "snooze", "send it", "when you can", "whenever",
    "not sure when", "someday", "eventually", "asap", "soon", "at some point",
    "the meeting about the q3 numbers", "invoice 4821", "order #839201", "zip 94043",
    "gate 12", "seat 14b", "flight ba 117", "3 kids", "2 dogs and a cat", "5 stars",
    "i owe you 20", "it costs 45 dollars", "the 30 day trial", "a 2 hour movie",
    "a 10 minute walk", "the 5 day forecast", "a week long trip", "my 3 year old",
    "the year of the dragon", "a day in the life", "happy new year", "good morning",
    "good night", "have a nice weekend", "how was your weekend", "monday blues",
    "thank god its friday", "night owl", "morning person", "evening news", "the noon show",
    "midnight snack", "every day is a gift", "day by day", "week by week", "now and then",
    "the time is now", "see you", "ttyl", "brb", "omw", "on my way",
    "at the station", "in a minute of silence", "the hour of need", "minute details",
    "second hand", "a second opinion", "first come first served", "third party",
    "the 4th of july parade route", "fifth avenue", "sixth sense", "seventh heaven",
    "sat nav", "sun cream", "wed lock", "mar a lago", "jan and dean", "may flowers",
    "the april fools", "june bug", "august rush", "dec 2 install guide v4",
    "ps5", "covid 19", "web 3", "ipv6", "mp3", "a4 paper", "4k tv", "9 to 5 job",
    "24 7 support", "365 days", "the 80s", "1990s music", "tps reports", "pm me",
    "am i right", "the pm said no", "on and on", "in and out", "back and forth",
    "every other thing", "a couple more", "a few more days of work", "this week was long",
    "last week was great", "next steps", "the last time", "this time", "next time",
    "what time is it", "time flies", "about time", "around the corner", "at least 3",
    "in 3 colors", "in 2 sizes", "for 6 people", "by the way", "by far", "on top",
    "talk to you", "to be honest", "half and half", "quarter final", "the 21 pilots",
    "catch 22", "apollo 13", "ocean's 11", "route 66", "studio 54", "area 51",
]
FILLER_WORDS = ["the", "a", "my", "your", "with", "and", "for", "to", "of", "it", "that",
                "team", "call", "project", "mom", "dad", "boss", "jo", "ana", "doc", "car",
                "rent", "bills", "report", "email", "slides", "lunch", "coffee", "gym"]

# Pieces for building surrounding words and texts without a time, so the
# model learns from the context rather than memorizing whole phrases.
VERBS = ["remind me to", "call", "email", "text", "send", "finish", "pay", "book", "check",
         "review", "fix", "update", "clean", "buy", "order", "cancel", "renew", "print",
         "sign", "submit", "read", "water", "feed", "pick up", "drop off", "bring", "move",
         "ask", "tell", "ping", "message", "follow up with", "meet", "visit", "see", "talk to",
         "return", "collect", "prepare", "write", "plan", "pack", "charge", "wash", "walk"]
NOUNS = ["mom", "dad", "the plants", "the cat", "the dog", "the car", "rent", "the bills",
         "the report", "the slides", "the invoice", "the contract", "jo", "ana", "sam", "the boss",
         "the doctor", "the dentist", "the bank", "the landlord", "the team", "my sister",
         "the kids", "groceries", "the package", "the laptop", "the form", "the tickets",
         "the hotel", "the flight", "the deck", "the budget", "the draft", "my passport",
         "the gym", "the library books", "the recycling", "the trash", "the oven", "lunch"]
NUMBER_NOUNS = ["apples", "people", "tickets", "pages", "items", "emails", "files", "kids",
                "boxes", "dollars", "bucks", "euros", "slides", "points", "steps", "rooms",
                "chairs", "copies", "reports", "photos", "bugs", "issues", "tasks", "miles",
                "km", "kg", "lbs", "percent", "stars", "votes", "seats", "guests"]
PLACES = ["room", "gate", "seat", "floor", "page", "chapter", "section", "table", "line",
          "route", "bus", "platform", "aisle", "unit", "suite", "box", "order", "invoice",
          "ticket", "case", "version", "level", "step", "ward", "apt", "building"]
# Words that look like part of a time, in sentences where they aren't.
LOOKALIKES = [
    "may i {v} {n}", "you may {v} {n}", "i sat with {n}", "we sat down", "the sun is {adj}",
    "the second {n2}", "a second {n2}", "the first {n2}", "the third {n2}", "my last {n2}",
    "the next {n2}", "this {n2} is {adj}", "every {n2} counts", "a few {n2}",
    "a couple of {n2}", "half of {n}", "a quarter of {n}", "at {place}", "in {place2}",
    "on {place3}", "next to {n}", "after {n}", "before {n}", "till {n} is done",
    "the end of {n2_the}", "{n} and {n}", "one more {n2}", "two {n2}", "{num} {nn}",
    "{place} {num}", "{place} {num}b", "{num} {nn} and {num} {nn}", "march to {place2}",
    "{n} will march", "wed {n}", "mon ami", "sat nav", "sun cream", "{num}/{num} split",
    "{num}-{num} score", "{num}.{num} rating", "{num}:{num} ratio", "v{num}.{num}",
    "{num}% off", "${num}", "{num} x {num}", "call {num} {num4}", "ext {num}",
    "the {adj} {n2}", "is it {adj}", "{n} is {adj}", "it was {adj}", "{v} {n}",
    "{v} {n} {pp} {place2}", "the {n2} {pp} {place2}", "good {part}", "{part} person",
    "{part} routine", "the {part} news", "{day} blues", "{day} vibes", "happy {day}",
    "{month} is my favorite", "born in {month}", "{year} was a good year", "back in {year}",
    "the {num}{ord} {n2}", "{num}{ord} place", "{num}{ord} floor", "the {unit} {n2}",
    "a {num} {unit} {n2}", "per {unit}", "{unit} by {unit}", "{n2} of the {unit}",
]
ADJ = ["great", "fine", "late", "early", "done", "open", "closed", "ready", "out", "nice",
       "broken", "big", "small", "new", "old", "good", "bad", "cold", "hot", "free"]
N2 = ["one", "option", "draft", "version", "thing", "idea", "time", "chance", "place",
      "step", "item", "round", "page", "try", "person", "call", "point", "email", "reply"]
PLACES2 = ["the office", "the box", "the car", "the kitchen", "town", "paris", "the cloud",
           "the inbox", "the meeting", "the shop", "the queue", "the folder", "spam", "bed"]
PLACES3 = ["the table", "the desk", "the list", "the website", "zoom", "the shelf", "it",
           "my phone", "the wall", "slack", "the agenda", "the call"]
PREPS = ["in", "at", "to", "from", "on", "near", "by"]
PARTS_WORDS = ["morning", "evening", "night", "afternoon"]
DAY_WORDS = ["monday", "friday", "sunday", "tuesday", "saturday"]
MONTH_WORDS = ["march", "may", "june", "april", "august", "october", "december"]
UNIT_WORDS = ["day", "week", "month", "year", "hour", "minute"]


def fill_slots(rng, template):
    """A template from LOOKALIKES with its slots filled."""
    def num():
        return str(rng.choice([2, 3, 4, 5, 6, 7, 8, 10, 12, 15, 20, 24, 30, 45, 50, 99, 100,
                               rng.randint(1, 999)]))
    slots = {
        "v": lambda: rng.choice(VERBS), "n": lambda: rng.choice(NOUNS),
        "n2": lambda: rng.choice(N2), "n2_the": lambda: "the " + rng.choice(N2),
        "adj": lambda: rng.choice(ADJ), "place": lambda: rng.choice(PLACES),
        "place2": lambda: rng.choice(PLACES2), "place3": lambda: rng.choice(PLACES3),
        "num": num, "num4": lambda: str(rng.randint(1000, 9999)),
        "nn": lambda: rng.choice(NUMBER_NOUNS), "pp": lambda: rng.choice(PREPS),
        "part": lambda: rng.choice(PARTS_WORDS), "day": lambda: rng.choice(DAY_WORDS),
        "month": lambda: rng.choice(MONTH_WORDS), "year": lambda: str(rng.randint(1980, 2030)),
        "ord": lambda: rng.choice(["st", "nd", "rd", "th"]), "unit": lambda: rng.choice(UNIT_WORDS),
    }
    out = template
    while "{" in out:
        a = out.index("{")
        b = out.index("}", a)
        out = out[:a] + slots[out[a + 1:b]]() + out[b + 1:]
    return out


def carrier(rng, lookalikes):
    """Words before a time: "remind me to call mom", "pay the 6 invoices",
    "check room 204"."""
    r = rng.random()
    if r < 0.4:
        return rng.choice(VERBS) + " " + rng.choice(NOUNS)
    if r < 0.55:
        return rng.choice(VERBS) + " the " + str(rng.randint(2, 30)) + " " + rng.choice(NUMBER_NOUNS)
    if r < 0.65:
        return rng.choice(VERBS) + " " + rng.choice(PLACES) + " " + str(rng.randint(1, 999))
    return fill_slots(rng, rng.choice(lookalikes))


KEYS = ["qwertyuiop", "asdfghjkl", "zxcvbnm"]
NEAR = {}
for r, row in enumerate(KEYS):
    for c, ch in enumerate(row):
        near = set()
        for dr in (-1, 0, 1):
            if 0 <= r + dr < len(KEYS):
                other = KEYS[r + dr]
                for dc in (-1, 0, 1):
                    if 0 <= c + dc < len(other) and (dr or dc):
                        near.add(other[c + dc])
        NEAR[ch] = "".join(sorted(near))


class Gen:
    """Renders examples, choosing spellings and wording from one half (or
    all) of the forms."""

    def __init__(self, rng, half=None):
        self.rng = rng
        self.half = half
        self.forms = {k: pick_half(v, half) for k, v in FORMS.items()}
        self.prefixes = pick_half(PREFIXES, half, keep=6)
        self.suffixes = pick_half(SUFFIXES, half, keep=7)
        self.negatives = pick_half(NEGATIVES, half, keep=0)
        self.lookalikes = pick_half(LOOKALIKES, half, keep=0)
        self.typo = 0.08

    # Pieces are (text, label, space before): a list of them is rendered by
    # `render`.
    def w(self, label, usual=0.55):
        forms = self.forms[label]
        shown = "O" if label == "ON" else label
        if len(forms) == 1 or self.rng.random() < usual:
            return (forms[0], shown, True)
        return (self.rng.choice(forms[1:]), shown, True)

    def num(self, n, label="NUM", pad=False):
        return (f"{n:02d}" if pad else str(n), label, True)

    def number_word(self, n, ordinal=False):
        """n (1-59) in words: "twenty five", "thirty", "fifth"."""
        def word(k, last):
            if not ordinal and k in NUMBER_TYPOS and self.rng.random() < 0.15:
                return self.rng.choice(NUMBER_TYPOS[k])
            return NUMBER_WORDS[k][1 if ordinal and last else 0]
        if n <= 20 or n % 10 == 0:
            return [(word(n, True), f"W{n}", True)]
        tens, ones = n - n % 10, n % 10
        joint = self.rng.random() < 0.3
        return [(word(tens, False), f"W{tens}", True), (word(ones, True), f"W{ones}", not joint)]

    def glue(self, piece):
        return (piece[0], piece[1], False)

    # Times of day.
    def meridiem(self, pm):
        """"pm", "p.m.", "p", with or without a space before it."""
        label = "PM" if pm else "AM"
        letter = "p" if pm else "a"
        r = self.rng.random()
        space = self.rng.random() < 0.35
        if r < 0.7:
            return [(letter + "m", label, space)]
        if r < 0.85:
            return [(letter, label, space), (".", label, False), ("m", label, False), (".", label, False)]
        return [(letter, label, space)]

    def clock(self, explicit_half=None):
        """A time on the clock: "6", "6pm", "6:30 pm", "18:30", "630pm",
        "six thirty", "half past 6", "quarter to 7", "6 o'clock"."""
        rng = self.rng
        r = rng.random()
        h = rng.randint(1, 12)
        m = rng.choice([0, 0, 0, 0, 15, 30, 30, 45, 5, 10, 20, 40, 50, rng.randint(0, 59)])
        out = []
        if r < 0.62:
            out.append(self.num(h))
            if m or rng.random() < 0.15:
                sep = rng.choice([":", ":", ":", "."])
                out += [(sep, "COLON", False), (f"{m:02d}", "NUM", False)]
            if rng.random() < 0.7:
                out += self.meridiem(rng.random() < 0.6)
            elif rng.random() < 0.25:
                out += [("o", "OCLOCK", True), ("clock", "OCLOCK", rng.random() < 0.5)] \
                    if rng.random() < 0.6 else [("oclock", "OCLOCK", True)]
        elif r < 0.72:
            # 24 hours.
            h = rng.randint(0, 23)
            out += [self.num(h, pad=rng.random() < 0.5), (":", "COLON", False),
                    (f"{m:02d}", "NUM", False)]
        elif r < 0.77:
            # "630pm", "1130 am"
            out.append((f"{h}{m:02d}", "NUM", True))
            out += self.meridiem(rng.random() < 0.6)
        elif r < 0.85:
            out += self.number_word(h)
            if m and rng.random() < 0.6:
                out += self.number_word(m) if m >= 10 else [("oh", "O", True)] + self.number_word(m)
            if rng.random() < 0.5:
                out += self.meridiem(rng.random() < 0.6)
        elif r < 0.95:
            # "half past 6", "quarter to seven", "10 past 5"
            kind = rng.choice(["half", "qpast", "qto", "npast", "nto"])
            hour = self.num(h) if rng.random() < 0.6 else None
            hw = [hour] if hour else self.number_word(h)
            if kind == "half":
                out += [self.w("HALF"), self.w("PAST")] + hw
            elif kind == "qpast":
                out += [self.w("QUARTER"), self.w("PAST")] + hw
            elif kind == "qto":
                out += [self.w("QUARTER"), self.w("TO")] + hw
            else:
                n = rng.choice([5, 10, 20, 25])
                out += ([self.num(n)] if rng.random() < 0.6 else self.number_word(n)) + \
                    [self.w("PAST" if kind == "npast" else "TO")] + hw
            if rng.random() < 0.3:
                out += self.meridiem(rng.random() < 0.6)
        else:
            out.append(self.w(rng.choice(["NOON", "NOON", "MIDNIGHT"])))
            if out[-1][1] == "NOON" and rng.random() < 0.2:
                out.insert(0, ("12", "NUM", True))
        return out

    def at_clock(self):
        p = self.clock()
        if self.rng.random() < 0.55 and p[0][1] not in ("HALF", "QUARTER"):
            at = self.w("AT", usual=0.7)
            if at[0] == "@" and self.rng.random() < 0.5:
                p[0] = self.glue(p[0])
            return [at] + p
        return p

    def part(self, with_word=True):
        """A part of the day: "in the morning", "this evening", "tonight",
        "at night", "morning"."""
        rng = self.rng
        label = rng.choice(PARTS)
        r = rng.random()
        if r < 0.4:
            words = [("in", "O", True), ("the", "O", True)]
            if label == "NIGHT":
                words = [("at", "O", True)]
            return words + [self.w(label)]
        if r < 0.6 and label != "NIGHT":
            return [self.w("THIS"), self.w(label)]
        return [self.w(label)]

    # Days.
    def weekday(self):
        rng = self.rng
        wd = self.w(rng.choice(WEEKDAYS))
        r = rng.random()
        if r < 0.3:
            return [wd]
        if r < 0.55:
            return [self.w("ON"), wd]
        if r < 0.75:
            return [self.w("NEXT"), wd]
        if r < 0.92:
            return [self.w("THIS"), wd]
        return [("on", "O", True), self.w("LAST"), wd] if rng.random() < 0.3 else [self.w("LAST"), wd]

    def month_day(self):
        rng = self.rng
        mo = rng.randrange(12)
        mw = self.w(MONTHS[mo])
        d = rng.randint(1, 28 if mo == 1 else 30)
        dayp = [self.num(d)]
        if rng.random() < 0.4:
            suffix = {1: "st", 2: "nd", 3: "rd"}.get(d % 10 if d not in (11, 12, 13) else 0, "th")
            dayp.append((suffix, "ORD", False))
        elif rng.random() < 0.08 and d in NUMBER_WORDS:
            dayp = self.number_word(d, ordinal=True)
        year = []
        if rng.random() < 0.2:
            y = rng.choice([2026, 2027, 2028])
            year = ([(",", "O", False)] if rng.random() < 0.5 else []) + [self.num(y)]
        r = rng.random()
        if r < 0.5:
            out = [mw] + dayp + year
        elif r < 0.75:
            out = dayp + [mw] + year
        else:
            out = [("the", "O", True)] if rng.random() < 0.5 else []
            out += dayp + [("of", "O", True), mw] + year
        if rng.random() < 0.3:
            out = [self.w("ON")] + out
        return out

    def numeric_date(self):
        rng = self.rng
        mo, d = rng.randint(1, 12), rng.randint(1, 28)
        r = rng.random()
        sep = rng.choice(["/", "/", "-", "."])
        if r < 0.15:
            y = rng.choice([2026, 2027])
            return [self.num(y), ("-", "DATESEP", False), (f"{mo:02d}", "NUM", False),
                    ("-", "DATESEP", False), (f"{d:02d}", "NUM", False)]
        first, second = (mo, d) if rng.random() < 0.8 else (max(d, 13), mo)
        out = [self.num(first), (sep, "DATESEP", False), (str(second), "NUM", False)]
        if rng.random() < 0.3:
            y = rng.choice(["2026", "2027", "26", "27"])
            out += [(sep, "DATESEP", False), (y, "NUM", False)]
        if rng.random() < 0.3:
            out = [self.w("ON")] + out
        return out

    def day_only(self):
        rng = self.rng
        d = rng.randint(1, 28)
        suffix = {1: "st", 2: "nd", 3: "rd"}.get(d % 10 if d not in (11, 12, 13) else 0, "th")
        p = [self.num(d), (suffix, "ORD", False)]
        if rng.random() < 0.15 and d in NUMBER_WORDS:
            p = self.number_word(d, ordinal=True)
        return ([self.w("ON")] if rng.random() < 0.5 else []) + [("the", "O", True)] + p

    def relative_day(self):
        rng = self.rng
        r = rng.random()
        if r < 0.12:
            return ([("the", "O", True)] if rng.random() < 0.5 else []) + \
                [self.w("U_DAY"), self.w("AFTER"), self.w("TOMORROW")]
        return [self.w(rng.choice(["TOMORROW", "TOMORROW", "TOMORROW", "TODAY", "TODAY",
                                   "TONIGHT", "YESTERDAY", "OVERMORROW"]))]

    def period(self):
        """"next week", "this weekend", "end of the month", "eod"."""
        rng = self.rng
        r = rng.random()
        if r < 0.45:
            unit = rng.choice(["U_WEEK", "U_WEEK", "U_MONTH", "U_YEAR", "WEEKEND"])
            return [self.w("NEXT"), self.w(unit)]
        if r < 0.6:
            return [self.w("THIS"), self.w("WEEKEND")] if rng.random() < 0.7 else [self.w("WEEKEND")]
        if r < 0.85:
            unit = rng.choice(["U_DAY", "U_DAY", "U_WEEK", "U_MONTH"])
            out = [self.w("END"), ("of", "O", True)]
            if rng.random() < 0.6:
                out.append(("the", "O", True))
            return out + [self.w(unit)]
        return [self.w(rng.choice(["EOD", "EOD", "EOW"]))]

    def date(self):
        r = self.rng.random()
        if r < 0.38:
            return self.relative_day()
        if r < 0.63:
            return self.weekday()
        if r < 0.8:
            return self.month_day()
        if r < 0.9:
            return self.numeric_date()
        return self.day_only()

    # Lengths of time.
    def amount(self, unit):
        rng = self.rng
        r = rng.random()
        n = rng.choice([1, 2, 2, 3, 3, 4, 5, 10, 15, 20, 30, 45, 90, rng.randint(1, 59)])
        if unit in ("U_MONTH", "U_YEAR", "U_WEEK", "U_FORTNIGHT"):
            n = min(n, rng.randint(1, 12))
        if r < 0.55:
            return [self.num(n)], n
        if r < 0.7 and n <= 59:
            return self.number_word(n), n
        if r < 0.82:
            return [self.w("A", usual=0.7)], 1
        if r < 0.92:
            return [self.w("COUPLE")] + ([("of", "O", True)] if rng.random() < 0.6 else []), 2
        return [self.w("FEW")], 3

    def duration(self):
        """"2 hours", "an hour and a half", "half an hour", "1h30",
        "2 hrs and 30 mins"."""
        rng = self.rng
        unit = rng.choice(["U_MIN", "U_MIN", "U_HOUR", "U_HOUR", "U_HOUR", "U_DAY", "U_DAY",
                           "U_WEEK", "U_MONTH", "U_YEAR", "U_FORTNIGHT"])
        r = rng.random()
        if r < 0.07 and unit in ("U_HOUR", "U_DAY"):
            return [self.w("HALF"), self.w("A", usual=0.8), self.w(unit)]
        amount, n = self.amount(unit)
        u = self.w(unit)
        # "2h", "30m", "3d"
        if amount[-1][1] == "NUM" and unit in ("U_MIN", "U_HOUR", "U_DAY", "U_WEEK") \
                and rng.random() < 0.25:
            u = (dict(U_MIN="m", U_HOUR="h", U_DAY="d", U_WEEK="w")[unit], unit, False)
        out = amount + [u]
        if unit in ("U_HOUR", "U_DAY") and rng.random() < 0.12:
            out += [self.w("AND"), self.w("A"), self.w("HALF")]
        elif unit == "U_HOUR" and rng.random() < 0.15:
            mins = rng.choice([15, 30, 45, 10, 20])
            if u[0] == "h" and rng.random() < 0.5:
                out += [(str(mins), "NUM", False)]
            else:
                out += ([self.w("AND")] if rng.random() < 0.5 else []) + [self.num(mins), self.w("U_MIN")]
        return out

    def offset(self):
        rng = self.rng
        d = self.duration()
        r = rng.random()
        if r < 0.6:
            return [self.w("IN", usual=0.85)] + d
        if r < 0.75:
            return d + [("from", "FROM_NOW", True), ("now", "FROM_NOW", True)]
        if r < 0.85:
            return d + [self.w("LATER")]
        if r < 0.9:
            return [self.w("AFTER")] + d
        return d + [self.w("AGO")]

    # Repeating times.
    def repeat(self):
        rng = self.rng
        r = rng.random()
        every = [self.w("EVERY")]
        if rng.random() < 0.15:
            every.append(self.w("OTHER"))
        if r < 0.3:
            days = [self.w(rng.choice(WEEKDAYS))]
            if rng.random() < 0.2:
                days += [self.w("AND") if rng.random() < 0.6 else (",", "O", False),
                         self.w(rng.choice(WEEKDAYS))]
            out = every + days
        elif r < 0.45:
            out = every + [self.w("U_DAY")] if rng.random() < 0.6 else [self.w("DAILY")]
        elif r < 0.53:
            out = [self.w("EVERY"), self.w("WEEKDAYS")] if rng.random() < 0.7 else [("on", "O", True), self.w("WEEKDAYS")]
        elif r < 0.6:
            out = [self.w("EVERY"), self.w(rng.choice(PARTS[:3]))]
        elif r < 0.72:
            unit = rng.choice(["U_MIN", "U_HOUR", "U_DAY", "U_WEEK", "U_MONTH"])
            n = rng.choice([2, 3, 5, 10, 15, 30]) if unit == "U_MIN" else rng.randint(2, 6)
            if rng.random() < 0.1:
                return [self.w("EVERY"), self.w("U_FORTNIGHT")] + (self.at_clock() if rng.random() < 0.5 else [])
            out = [self.w("EVERY")] + ([self.num(n)] if rng.random() < 0.7 else self.number_word(n)) + [self.w(unit)]
        elif r < 0.84:
            word = rng.choice(["WEEKLY", "MONTHLY", "YEARLY", "HOURLY"])
            unit = dict(WEEKLY="U_WEEK", MONTHLY="U_MONTH", YEARLY="U_YEAR", HOURLY="U_HOUR")[word]
            out = [self.w(word)] if rng.random() < 0.6 else every + [self.w(unit)]
            if word == "WEEKLY" and rng.random() < 0.5:
                out += [self.w("ON"), self.w(rng.choice(WEEKDAYS))]
            elif word == "MONTHLY" and rng.random() < 0.6:
                out += self.day_only()
            elif word == "YEARLY" and rng.random() < 0.5:
                md = self.month_day()
                out += [p for p in md if p[1] != "NUM" or len(p[0]) < 4]
        else:
            out = every + [self.w("U_MONTH")] + self.day_only()
        if out[-1][1] not in ("HOURLY", "U_MIN", "U_HOUR") and not (
                len(out) > 2 and out[-1][1] == "U_HOUR") and rng.random() < 0.55:
            out += self.at_clock()
        return out

    # Newer phrasings.
    def top_of_hour(self):
        """"at the top of the hour", "on the hour"."""
        if self.rng.random() < 0.7:
            out = [self.w("TOP"), ("of", "O", True), ("the", "O", True), self.w("U_HOUR", usual=0.9)]
            return ([("at", "O", True)] if self.rng.random() < 0.6 else []) + out
        return [("on", "TOP", True), ("the", "O", True), ("hour", "U_HOUR", True)]

    def week_shift(self):
        """A week from this one: "next week", "in 3 weeks", "the week after
        next"."""
        rng = self.rng
        r = rng.random()
        if r < 0.45:
            return [self.w("NEXT"), self.w("U_WEEK", usual=0.8)]
        if r < 0.8:
            amount, _ = self.amount("U_WEEK")
            return [self.w("IN", usual=0.85)] + amount + [self.w("U_WEEK", usual=0.8)]
        return [("the", "O", True), self.w("U_WEEK", usual=0.9), self.w("AFTER"), self.w("NEXT")]

    def weekday_in_week(self):
        """"the monday in three weeks", "friday next week", "next week on
        tue", "in 2 weeks on friday"."""
        rng = self.rng
        wd = self.w(rng.choice(WEEKDAYS))
        shift = self.week_shift()
        r = rng.random()
        if r < 0.45:
            return ([("the", "O", True)] if rng.random() < 0.5 else []) + [wd] + shift
        if r < 0.8:
            return shift + ([("on", "O", True)] if rng.random() < 0.5 else []) + [wd]
        return [("the", "O", True), wd, ("of", "O", True)] + shift

    def after_next(self):
        """"friday after next", "the weekend after next"."""
        what = self.w(self.rng.choice(WEEKDAYS + ["WEEKEND"]))
        return ([("the", "O", True)] if self.rng.random() < 0.4 else []) + [what, self.w("AFTER"), self.w("NEXT")]

    def edge(self):
        """"start of next week", "the beginning of the month", "mid next
        month", "end of next week", "midweek"."""
        rng = self.rng
        kind = rng.choice(["START", "START", "MID", "END"])
        unit = rng.choice(["U_WEEK", "U_WEEK", "U_MONTH", "U_MONTH", "U_YEAR"])
        if kind == "MID" and rng.random() < 0.2:
            # "midweek", "mid-month"
            joined = rng.random() < 0.5
            return [self.w("MID"), (("-", "O", False) if joined else ("", "O", True)),
                    (self.w(unit)[0], unit, not joined and rng.random() < 0.5)]
        out = [self.w(kind)]
        if rng.random() < 0.75:
            out.append(("of", "O", True))
        r = rng.random()
        if r < 0.5:
            out.append(self.w("NEXT"))
        elif r < 0.65:
            out.append(self.w("THIS"))
        else:
            out.append(("the", "O", True))
        out.append(self.w(unit))
        return out

    def nth_weekday(self):
        """"the first monday of next month", "2nd tue in december", "last
        friday of the month"."""
        rng = self.rng
        n = rng.choice([1, 1, 2, 3, 4, 0])
        if n == 0:
            nth = [self.w("LAST")]
        elif rng.random() < 0.6:
            nth = [(NUMBER_WORDS[n][1], f"W{n}", True)]
        else:
            nth = [self.num(n), ({1: "st", 2: "nd", 3: "rd"}.get(n, "th"), "ORD", False)]
        wd = self.w(rng.choice(WEEKDAYS))
        r = rng.random()
        if r < 0.4:
            month = [self.w("NEXT"), self.w("U_MONTH")]
        elif r < 0.6:
            month = [("the", "O", True), self.w("U_MONTH")]
        else:
            month = [self.w(rng.choice(MONTHS))]
        return [("the", "O", True)] + nth + [wd, (rng.choice(["of", "in"]), "O", True)] + month

    def month_day_edge(self):
        """"the first day of next month", "last day of the month"."""
        rng = self.rng
        first = rng.random() < 0.5
        nth = [("first", "W1", True)] if first else [self.w("LAST")]
        month = [self.w("NEXT"), self.w("U_MONTH")] if rng.random() < 0.5 else [("the", "O", True), self.w("U_MONTH")]
        return [("the", "O", True)] + nth + [self.w("U_DAY", usual=0.9), ("of", "O", True)] + month

    def anchored(self):
        """"a week from friday", "2 weeks from tomorrow", "3 days after
        monday", "a fortnight from today"."""
        rng = self.rng
        unit = rng.choice(["U_DAY", "U_WEEK", "U_WEEK", "U_FORTNIGHT"])
        amount, _ = self.amount(unit if unit != "U_FORTNIGHT" else "U_WEEK")
        if unit == "U_FORTNIGHT":
            amount = [self.w("A")]
        anchor = rng.choice([[self.w(rng.choice(WEEKDAYS))], [self.w("TOMORROW")], [self.w("TODAY")],
                             [self.w("NEXT"), self.w(rng.choice(WEEKDAYS))]])
        return amount + [self.w(unit), self.w("FROM")] + anchor

    def same_time(self):
        """"this time tomorrow", "same time next week", "same time friday"."""
        rng = self.rng
        same = [(rng.choice(["same", "this", "same", "sme"]), "SAME", True), ("time", "O", True)]
        when = rng.choice([[self.w("TOMORROW")], [self.w("NEXT"), self.w("U_WEEK")], [self.w(rng.choice(WEEKDAYS))],
                           [self.w("OVERMORROW")]])
        return ([("at", "O", True)] if rng.random() < 0.3 else []) + same + when

    def first_thing(self):
        """"first thing tomorrow", "first thing monday"."""
        day = self.rng.choice([[self.w("TOMORROW")], [self.w(self.rng.choice(WEEKDAYS))],
                               [self.w("NEXT"), self.w(self.rng.choice(WEEKDAYS))]])
        return [("first", "MORNING", True), ("thing", "MORNING", True)] + day

    def newer(self):
        """One of the newer phrasings."""
        rng = self.rng
        r = rng.random()
        if r < 0.14:
            o = self.offset() if rng.random() < 0.7 else [self.w("IN"), self.num(rng.randint(1, 6)), self.w("U_HOUR")]
            return o + self.top_of_hour()
        if r < 0.2:
            return self.top_of_hour()
        if r < 0.34:
            return self.weekday_in_week() + (self.at_clock() if rng.random() < 0.4 else [])
        if r < 0.42:
            return self.after_next() + (self.at_clock() if rng.random() < 0.3 else [])
        if r < 0.56:
            return self.edge() + (self.at_clock() if rng.random() < 0.4 else [])
        if r < 0.64:
            return self.nth_weekday() + (self.at_clock() if rng.random() < 0.3 else [])
        if r < 0.69:
            return self.month_day_edge()
        if r < 0.78:
            return self.anchored() + (self.at_clock() if rng.random() < 0.3 else [])
        if r < 0.84:
            return self.same_time()
        if r < 0.89:
            return self.first_thing()
        if r < 0.95:
            return [self.w("IN")] + ([self.w("A")] if rng.random() < 0.6 else [self.num(rng.randint(2, 4))]) + \
                [self.w("U_FORTNIGHT")] + (self.at_clock() if rng.random() < 0.3 else [])
        return [self.w("OVERMORROW")] + (self.at_clock() if rng.random() < 0.5 else [])

    # Whole expressions.
    def expression(self):
        rng = self.rng
        if rng.random() < 0.22:
            return self.newer()
        r = rng.random()
        if r < 0.3:
            d, t = self.date(), self.at_clock()
            order = rng.random()
            if order < 0.65:
                return d + t
            if order < 0.85:
                if d[0][1] in WEEKDAYS or d[0][1] in MONTHS or (d[0][1] == "NUM" and len(d) > 1 and d[1][1] == "DATESEP"):
                    d = [self.w("ON")] + d if rng.random() < 0.5 else d
                return t + d
            return d + t + ([self.w(rng.choice(PARTS[:3]))] if rng.random() < 0.3 else [])
        if r < 0.38:
            d = rng.choice([self.relative_day, self.weekday])()
            if d[-1][1] == "TONIGHT":
                return d
            return d + [self.w(rng.choice(PARTS))] + (self.at_clock() if rng.random() < 0.3 else [])
        if r < 0.5:
            return self.date()
        if r < 0.62:
            t = self.at_clock()
            if rng.random() < 0.25:
                t += self.part()
            return t
        if r < 0.67:
            return self.part()
        if r < 0.8:
            o = self.offset()
            if o[0][1] == "IN" and o[-1][1] in ("U_DAY", "U_WEEK") and rng.random() < 0.25:
                o += self.at_clock()
            return o
        if r < 0.87:
            p = self.period()
            if p[-1][1] not in ("EOD", "EOW") and p[-1][1] != "U_DAY" and rng.random() < 0.3:
                p += self.at_clock()
            return p
        if r < 0.97:
            return self.repeat()
        return rng.choice([[self.w("NOW")], [("right", "O", True), self.w("NOW")],
                           [self.w("LATER")], [self.w("LATER"), self.w("TODAY")],
                           [self.w("LATER"), self.w("TONIGHT")]])

    def words(self, text):
        return [(t, "O", True) for t in text.split()]

    def example(self):
        """(text, tokens, labels) for one example; about one in four has no
        time in it."""
        rng = self.rng
        if rng.random() < 0.28:
            r = rng.random()
            if r < 0.35:
                text = rng.choice(self.negatives)
            elif r < 0.85:
                text = fill_slots(rng, rng.choice(self.lookalikes))
            else:
                text = carrier(rng, self.lookalikes)
            pieces = self.words(text)
            if rng.random() < 0.3:
                pieces += self.words(fill_slots(rng, rng.choice(self.lookalikes)))
            if rng.random() < 0.25:
                pieces = self.words(rng.choice(self.prefixes)) + pieces
            if rng.random() < 0.2:
                pieces += self.words(rng.choice(self.suffixes))
        else:
            r = rng.random()
            before = rng.choice(self.prefixes) if r < 0.6 else carrier(rng, self.lookalikes) if r < 0.85 else ""
            after = rng.choice(self.suffixes)
            if rng.random() < 0.12:
                after = (after + " " + fill_slots(rng, rng.choice(self.lookalikes))).strip()
            pieces = self.words(before) + self.expression() + self.words(after)
            if rng.random() < 0.1:
                # The time first: "tmrw 6pm call mom".
                pieces = self.expression() + self.words(carrier(rng, self.lookalikes))
        return self.render(pieces)

    def render(self, pieces):
        return render(self.rng, pieces, self.typo)

from languages import L as LANGS, WEIGHT

THIS_AFTER = {"id", "ms", "vi", "th", "sw", "he", "ro"}
# Scripts whose words take typing slips and capitals.
CASED = set("de fr es pt it nl sv da no pl ru uk cs sk hr bg ro hu fi el tr id ms vi tl sw ca hi-latn".split())
LATIN = set("de fr es pt it nl sv da no pl cs sk hr ro hu fi tr id ms vi tl sw ca hi-latn".split())
# A number written right after these meanings (no space) in Korean and Bengali.
GLUED_KO = {"OCLOCK", "U_MIN", "U_HOUR", "U_DAY", "U_WEEK", "U_MONTH", "U_YEAR", "ORD", "MONTH_MARK"}


def forms(value):
    return [f for f in value.split("|") if f] if isinstance(value, str) else value


class LangGen:
    """Renders expressions in one of the languages in `languages.py`, in
    its own word order."""

    def __init__(self, rng, code, half=None):
        self.rng = rng
        self.code = code
        spec = LANGS[code]
        self.spec = spec
        self.dense = spec.get("dense", False)
        self.f = {}
        for k, v in spec.items():
            if k in ("days", "months"):
                self.f[k] = [pick_half(forms(x), half) for x in v]
            elif k in ("carriers", "tail", "negatives"):
                self.f[k] = pick_half(forms(v), half, keep=2)
            elif k.isupper() and isinstance(v, str):
                self.f[k] = pick_half(forms(v), half)
        self.nums = spec.get("NUMS", {})
        self.typo = 0.03 if code in LATIN else 0.015 if code in CASED else 0.0

    def has(self, key):
        return bool(self.f.get(key))

    def form(self, key, usual=0.55):
        options = self.f[key]
        if len(options) == 1 or self.rng.random() < usual:
            return options[0]
        return self.rng.choice(options[1:])

    def say(self, text, label, glued=False):
        """Pieces for a form, every word with the label."""
        out = []
        for i, word in enumerate(text.split(" ")):
            out.append((word, label, not self.dense and not (glued and i == 0)))
        return out

    def w(self, label, key=None, glued=False):
        key = key or label
        return self.say(self.form(key), "O" if label == "ON" else label, glued)

    def day(self, i):
        return self.say(self.rng.choice(self.f["days"][i]) if self.rng.random() > 0.55 else self.f["days"][i][0],
                        WEEKDAYS[i])

    def month(self, i):
        options = self.f["months"][i]
        return self.say(options[0] if self.rng.random() < 0.55 or len(options) == 1 else self.rng.choice(options[1:]),
                        MONTHS[i])

    def num(self, n, label="NUM", pad=False, glued=False):
        text = f"{n:02d}" if pad else str(n)
        zero = self.spec.get("digits")
        if zero and self.rng.random() < (0.5 if self.code in ("ar", "fa") else 0.25):
            z = int(zero, 16)
            text = "".join(chr(z + int(c)) for c in text)
        return [(text, label, not self.dense and not glued)]

    def count(self, n=None, one_word=True):
        """A count before a unit: digits, a number word, or "a"/"ein"."""
        rng = self.rng
        n = n or rng.choice([1, 2, 2, 3, 3, 5, 10, 15, 20, 30, 45, rng.randint(1, 48)])
        if n == 1 and self.has("A") and rng.random() < 0.6:
            return self.w("A"), 1
        if n in self.nums and rng.random() < 0.3:
            return self.say(rng.choice(forms(self.nums[n])), f"W{n}"), n
        return self.num(n), n

    def unit(self, label, after_number=True):
        glued = (self.code == "ko" and after_number) or self.dense
        return self.w(label, glued=glued)

    # Days.
    def relday(self):
        options = [k for k in ("TODAY", "TOMORROW", "TOMORROW", "OVERMORROW", "YESTERDAY", "TONIGHT") if self.has(k)]
        return self.w(self.rng.choice(options))

    def with_next(self, thing, mode):
        """`thing` with "next"/"last"/"this" on the side the language puts it."""
        rng = self.rng
        label = {"next": "NEXT", "last": "LAST", "this": "THIS"}[mode]
        if not self.has(label):
            return thing
        word = self.w(label)
        pos = self.spec.get("next", "pre")
        if mode == "this":
            pos = "post" if self.code in THIS_AFTER else "pre"
        if pos == "both":
            pos = rng.choice(["pre", "post"])
        return word + thing if pos == "pre" else thing + word

    def weekday(self):
        rng = self.rng
        i = rng.randrange(7)
        mode = rng.choice(["bare", "bare", "next", "this", "last"])
        days = self.f["days"][i]
        text = days[0] if rng.random() < 0.55 or len(days) == 1 else rng.choice(days[1:])
        if self.code in ("zh", "zh-hant") and mode != "bare" and text[:1] in "周週" or \
                self.code in ("zh", "zh-hant") and mode != "bare" and text[:2] in ("星期", "礼拜", "禮拜"):
            # 下周五: next week's Friday.
            head, tail = text[:-1], text[-1]
            label = {"next": "NEXT", "last": "LAST", "this": "THIS"}[mode]
            return self.w(label) + self.say(head, "U_WEEK") + self.say(tail, WEEKDAYS[i])
        thing = self.say(text, WEEKDAYS[i])
        if mode == "bare":
            if self.has("ON") and rng.random() < 0.4:
                return self.w("ON") + thing
            return thing
        return self.with_next(thing, mode)

    def week(self, mode=None):
        """"next week", "la semaine prochaine", "下周"."""
        rng = self.rng
        mode = mode or rng.choice(["next", "next", "last"])
        unit = rng.choice(["U_WEEK", "U_WEEK", "U_MONTH", "U_YEAR"])
        key = "P_MONTH" if unit == "U_MONTH" and self.code == "ja" else unit
        if key == "P_MONTH":
            return self.with_next(self.say("月", "U_MONTH"), mode)
        return self.with_next(self.unit(unit, after_number=False), mode)

    def weekday_in_week(self):
        """"nächste Woche Freitag", "来週の金曜日", "vendredi de la semaine
        prochaine"."""
        rng = self.rng
        i = rng.randrange(7)
        shift = self.with_next(self.unit("U_WEEK", after_number=False), rng.choice(["next", "next", "this"]))
        joint = [("の", "O", False)] if self.code == "ja" else []
        if self.spec.get("next", "pre") == "post":
            return self.day(i) + shift
        return shift + joint + self.day(i)

    def date(self):
        rng = self.rng
        style = self.spec.get("date", "dM")
        mo, d = rng.randrange(12), rng.randint(1, 28)
        if rng.random() < 0.2 or "months" not in self.f and style not in ("cjk", "ko", "vi"):
            # Numeric: day first, except where the month comes first.
            if style in ("cjk", "ko"):
                out = self.num(mo + 1) + [("/", "DATESEP", False)] + self.num(d, glued=True)
            else:
                sep = rng.choice(["/", ".", "/"]) if style != "d.M" else "."
                out = self.num(d) + [(sep, "DATESEP", False)] + self.num(mo + 1, glued=True)
                if sep == "." and rng.random() < 0.5:
                    out.append((".", "O", False))
            return out
        if style == "cjk":
            out = []
            if rng.random() < 0.15:
                out += self.num(rng.choice([2026, 2027])) + self.say("年", "U_YEAR")
            return out + self.num(mo + 1) + self.w("MONTH_MARK") + self.num(d) + self.w("ORD")
        if style == "ko":
            return self.num(mo + 1) + self.w("MONTH_MARK", glued=True) + self.num(d) + self.w("ORD", glued=True)
        if style == "vi":
            return [("ngày", "O", True)] + self.num(d) + self.w("MONTH_MARK") + self.num(mo + 1)
        day = self.num(d)
        if style == "d.M":
            day += [(".", "ORD", False)]
        elif self.has("ORD") and (d == 1 or self.code == "hu") and rng.random() < 0.6:
            day += self.w("ORD", glued=True) if self.code != "hu" else [("-", "O", False)] + self.w("ORD", glued=True)
        month = self.month(mo)
        if style == "Md":
            out = month + day
        elif style == "d_de_M":
            out = day + [("de", "O", True)] + month
        else:
            out = day + month
        if self.has("ON") and rng.random() < 0.3:
            out = self.w("ON") + out
        return out

    # Times.
    def clock(self):
        rng = self.rng
        style = self.spec.get("clock")
        h = rng.randint(1, 12)
        m = rng.choice([0, 0, 0, 15, 30, 30, 45, 10, 20, rng.randint(0, 59)])
        part = self.part_word() if rng.random() < 0.45 else []
        if self.spec.get("no_clock") or rng.random() < 0.2:
            h24 = rng.randint(0, 23)
            out = self.num(h24, pad=rng.random() < 0.4) + [(":", "COLON", False)] + self.num(m, pad=True, glued=True)
            if self.has("AT") and rng.random() < 0.5:
                out = self.w("AT") + out
            return out
        if style == "cjk":
            out = part + self.num(h, glued=not part) + self.w("OCLOCK", glued=True)
            if m == 30 and self.has("HALF") and rng.random() < 0.6:
                out += self.w("HALF", glued=self.dense)
            elif m:
                out += self.num(m) + self.unit("U_MIN")
            return out
        if style == "hi":
            half = m == 30 and self.has("HALF") and rng.random() < 0.6
            out = part + (self.w("HALF") if half else []) + self.num(h) + self.w("OCLOCK", glued=self.code == "bn")
            return out
        if style == "h" and rng.random() < 0.6:
            h24 = h + 12 if part == [] and rng.random() < 0.5 else h
            out = self.num(h24) + self.w("OCLOCK", glued=True)
            if m:
                out += self.num(m, pad=True, glued=True)
            if self.has("AT") and rng.random() < 0.5:
                out = self.w("AT") + out
            return out + (part if h24 <= 12 else [])
        out = self.num(h)
        frac = "HALF" if m == 30 else "QUARTER" if m == 15 else None
        if frac and self.has("AND") and self.has(frac) and rng.random() < 0.5:
            out += self.w("AND") + self.w(frac)
        elif m and self.has("TO") and self.has("QUARTER") and m == 45 and rng.random() < 0.5 and self.code != "de":
            out = self.num(h % 12 + 1) + self.w("TO") + self.w("QUARTER")
        elif m == 15 and self.code == "de" and rng.random() < 0.5:
            out = self.w("QUARTER") + self.w("PAST") + self.num(h)
        elif m:
            out += [(":", "COLON", False)] + self.num(m, pad=True, glued=True)
        if self.has("OCLOCK") and rng.random() < 0.4:
            out += self.w("OCLOCK", glued=self.code in ("hu",))
        if self.has("AT") and rng.random() < 0.65:
            out = self.w("AT") + out
        return (part + out) if rng.random() < 0.3 else (out + part)

    def part_word(self):
        options = [k for k in ("MORNING", "AFTERNOON", "EVENING", "NIGHT") if self.has(k)]
        return self.w(self.rng.choice(options))

    def noonish(self):
        options = [k for k in ("NOON", "MIDNIGHT") if self.has(k)]
        return self.w(self.rng.choice(options)) if options else self.part_word()

    # Lengths of time.
    def duration(self):
        rng = self.rng
        unit = rng.choice(["U_MIN", "U_HOUR", "U_HOUR", "U_DAY", "U_DAY", "U_WEEK", "U_MONTH", "U_YEAR"])
        amount, n = self.count()
        if self.has("FORTNIGHT") and rng.random() < 0.06:
            amount, u = [], self.w("U_FORTNIGHT", key="FORTNIGHT")
        else:
            u = self.unit(unit)
        if self.spec.get("unit_first"):
            core = u + amount
        else:
            core = amount + u
        ago = rng.random() < 0.15
        if ago:
            if self.has("AGO_PRE"):
                return self.w("AGO", key="AGO_PRE") + core
            if self.has("AGO"):
                return core + self.w("AGO")
        if self.has("IN") and (not self.has("FROM_NOW") or rng.random() < 0.5):
            return self.w("IN") + core
        if self.has("FROM_NOW"):
            return core + self.w("FROM_NOW", glued=self.dense)
        return core

    def repeat(self):
        rng = self.rng
        r = rng.random()
        if r < 0.4 and self.has("EVERY"):
            out = self.w("EVERY") + self.day(rng.randrange(7))
        elif r < 0.7 and self.has("EVERY"):
            out = self.w("EVERY") + self.unit(rng.choice(["U_DAY", "U_WEEK"]), after_number=False)
        elif self.has("DAILY"):
            out = self.w("DAILY")
        else:
            out = self.w("EVERY") + self.unit("U_DAY", after_number=False)
        if rng.random() < 0.6:
            out += self.clock()
        return out

    def expression(self):
        rng = self.rng
        r = rng.random()
        if r < 0.28:
            d = rng.choice([self.relday, self.weekday, self.weekday, self.date])()
            t = self.clock() if rng.random() < 0.8 else self.noonish()
            return d + t if rng.random() < 0.8 else t + d
        if r < 0.38:
            return self.relday() + self.part_word()
        if r < 0.5:
            return rng.choice([self.relday, self.weekday, self.date, self.date])()
        if r < 0.6:
            return self.clock()
        if r < 0.64:
            return self.part_word() if rng.random() < 0.6 else self.noonish()
        if r < 0.77:
            return self.duration()
        if r < 0.84:
            w = self.week()
            return w + (self.clock() if rng.random() < 0.3 else [])
        if r < 0.9:
            return self.weekday_in_week() + (self.clock() if rng.random() < 0.3 else [])
        if r < 0.97:
            return self.repeat()
        return self.w("NOW") if self.has("NOW") else self.relday()

    def example(self):
        rng = self.rng
        if rng.random() < 0.27:
            text = rng.choice(self.f["negatives"])
            pieces = self.say(text, "O")
            if rng.random() < 0.3:
                pieces = self.say(rng.choice(self.f["carriers"]), "O") + pieces
            if rng.random() < 0.15:
                n = rng.randint(2, 999)
                pieces += self.num(n) + self.say(rng.choice(["!", "?", "ok"]), "O")
        else:
            pieces = self.expression()
            if rng.random() < 0.6:
                before = self.say(rng.choice(self.f["carriers"]), "O")
                pieces = before + pieces if rng.random() < 0.75 else pieces + before
            if rng.random() < 0.35:
                pieces += self.say(rng.choice(self.f["tail"]), "O")
        if self.dense:
            # Thai and Japanese are sometimes spaced between phrases.
            pieces = [(t, l, i > 0 and rng.random() < 0.08) for i, (t, l, _) in enumerate(pieces)]
        strip = self.code in LATIN and rng.random() < 0.25
        return render(rng, pieces, self.typo, cased=self.code in CASED, strip=strip)


def strip_marks(text):
    import unicodedata
    out = unicodedata.normalize("NFKD", text)
    return unicodedata.normalize("NFC", "".join(c for c in out if not unicodedata.combining(c)))


def render(rng, pieces, typo, cased=True, strip=False):
    """(text, tokens, labels) for pieces of (text, label, space before),
    with untidy typing; None when the pieces don't tokenize as the text."""
    case = rng.random() if cased else 1.0
    parts = []
    out = []
    for text, label, space in pieces:
        if not text:
            continue
        if strip:
            text = strip_marks(text)
        if text.isalpha() and len(text) >= 3 and rng.random() < typo:
            text = slip(rng, text)
        if case < 0.08:
            text = text.upper()
        elif case < 0.2 and text.isalpha():
            text = text.capitalize()
        if parts and space:
            parts.append("  " if rng.random() < 0.02 else " ")
        start = sum(len(p) for p in parts)
        parts.append(text)
        for t, s, e in tokens(text):
            out.append((t, start + s, start + e, label))
    text = "".join(parts)
    if 0.2 <= case < 0.5 and text:
        text = text[0].upper() + text[1:]
    toks = tokens(text)
    if not toks or [(t, s, e) for t, s, e, _ in out] != toks:
        return None
    return text, toks, [label for *_, label in out]


def slip(rng, word):
    """One typing slip in `word`."""
    i = rng.randrange(len(word))
    kind = rng.random()
    if kind < 0.3:
        return word[:i] + word[i + 1:]
    if kind < 0.5:
        return word[:i] + word[i] + word[i:]
    if kind < 0.7 and i + 1 < len(word):
        return word[:i] + word[i + 1] + word[i] + word[i + 2:]
    c = word[i].lower()
    if c in NEAR:
        return word[:i] + rng.choice(NEAR[c]) + word[i + 1:]
    return word


def pick_half(items, half, keep=1):
    """The first `keep` items, then every other one of the rest: the even
    ones for half 0, the odd ones for half 1, all of them for None."""
    if half is None:
        return list(items)
    head, rest = list(items[:keep]), list(items[keep:])
    chosen = [x for i, x in enumerate(rest) if i % 2 == half]
    return head + chosen if head + chosen else list(items)


def examples(rng, count, half=None, typo=0.08, english=0.42, langs=None):
    """`count` distinct examples: about `english` of them in English, the
    rest across the other languages by WEIGHT. Repeats are dropped, so the
    count is of different texts."""
    gen = Gen(rng, half)
    gen.typo = typo
    codes = list(langs or LANGS)
    weights = [WEIGHT.get(c, 1) for c in codes]
    others = {}
    out, seen = [], set()
    tries = 0
    while len(out) < count and tries < count * 4:
        tries += 1
        if not codes or rng.random() < english:
            e = gen.example()
            code = "en"
        else:
            code = rng.choices(codes, weights)[0]
            if code not in others:
                others[code] = LangGen(rng, code, half)
                if typo == 0.0:
                    others[code].typo = 0.0
            e = others[code].example()
        if e is None or not e[1] or e[0] in seen:
            continue
        seen.add(e[0])
        out.append(e + (code,))
    return out

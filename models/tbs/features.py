"""Tokens, their labels and their features for Epoch TBS, the date and time
model.

This is the reference for `src/manage/local_ai/when.rs`: the app splits text
into tokens and builds their features exactly as here, so the model trained
on them labels the same tokens in the app. `parity.json`, written by
`train.py`, holds examples both must agree on.

The model only reads: for every token it says what the token *means*
("tmrw" and "tomorow" are TOMORROW, "sat" in "sat down" is nothing, the "6"
in "at 6" is a number). Working out the moment from those meanings (which
Friday, 6 in the morning or the evening, the first of a repeating time) is
done by rules, in `resolve.py` and in the app, so the arithmetic is always
exact.
"""

import unicodedata

# Hashed feature space (a power of two).
DIM = 1 << 14
# Tokens looked at per text.
MAX_TOKENS = 64
MAX_TEXT = 300

# What a token can mean. The order is the model's output order; the app has
# the same list.
LABELS = [
    "O", "NUM", "ORD", "COLON", "DATESEP",
    "TODAY", "TOMORROW", "YESTERDAY", "TONIGHT", "NOW", "LATER",
    "MON", "TUE", "WED", "THU", "FRI", "SAT", "SUN",
    "JAN", "FEB", "MAR", "APR", "MAY", "JUN", "JUL", "AUG", "SEP", "OCT", "NOV", "DEC",
    "U_MIN", "U_HOUR", "U_DAY", "U_WEEK", "U_MONTH", "U_YEAR",
    "WEEKEND", "WEEKDAYS",
    "MORNING", "AFTERNOON", "EVENING", "NIGHT", "NOON", "MIDNIGHT",
    "AM", "PM",
    "W1", "W2", "W3", "W4", "W5", "W6", "W7", "W8", "W9", "W10", "W11", "W12",
    "W13", "W14", "W15", "W16", "W17", "W18", "W19", "W20", "W30", "W40", "W50",
    "HALF", "QUARTER", "A", "COUPLE", "FEW",
    "AT", "IN", "NEXT", "THIS", "LAST", "EVERY", "OTHER", "AFTER", "FROM_NOW",
    "AGO", "PAST", "TO", "AND", "OCLOCK", "END", "EOD", "EOW",
    "DAILY", "WEEKLY", "MONTHLY", "YEARLY", "HOURLY",
    # The day after tomorrow in one word ("overmorrow", "übermorgen", "后天").
    "OVERMORROW",
    # Two weeks.
    "U_FORTNIGHT",
    # "the top of the hour", "on the hour".
    "TOP",
    # "this time tomorrow", "same time next week".
    "SAME",
    # "start of next week", "middle of the month" (END is the end).
    "START", "MID",
    # Counting from a day rather than now: "a week from friday".
    "FROM",
    # A month given as a number with a mark: "11月", "11월", "tháng 11".
    "MONTH_MARK",
]
INDEX = {name: i for i, name in enumerate(LABELS)}

# Punctuation kept as tokens of their own; any other character that isn't a
# letter or digit separates tokens.
KEPT = ":/-.,@+"
VOWELS = set("aeiou")


# Digits in other scripts, by their zero: Arabic-Indic, Persian, Devanagari,
# Bengali, Thai and fullwidth. They are read as ASCII digits.
ZEROS = [0x0660, 0x06F0, 0x0966, 0x09E6, 0x0E50, 0xFF10]
# Scripts written without spaces between words: Thai, Lao, Myanmar, Khmer,
# kana and CJK ideographs. Each of their characters (with the marks on it) is
# a token of its own.
DENSE = [
    (0x0E00, 0x0EFF), (0x1000, 0x109F), (0x1780, 0x17FF), (0x3040, 0x30FF),
    (0x3400, 0x4DBF), (0x4E00, 0x9FFF), (0xF900, 0xFAFF), (0xFF66, 0xFF9F),
]


def digit(c):
    """The ASCII digit `c` is, in any of the scripts above, or None."""
    if "0" <= c <= "9":
        return c
    o = ord(c)
    for z in ZEROS:
        if z <= o <= z + 9:
            return chr(ord("0") + o - z)
    return None


def is_mark(c):
    return unicodedata.category(c)[0] == "M"


def is_letter(c):
    """Letters, and the marks written on them (Thai, Devanagari and Bengali
    vowel signs), but not digits."""
    return digit(c) is None and (c.isalpha() or is_mark(c))


def is_dense(c):
    o = ord(c)
    return any(a <= o <= b for a, b in DENSE)


def tokens(text):
    """(token, start, end) for each token: lowercase runs of letters, runs of
    digits (as ASCII digits), single characters of scripts written without
    spaces, and the punctuation in KEPT. A run of letters and a run of digits
    are separate tokens ("6pm" is "6" and "pm"; "明天6点" is "明", "天", "6",
    "点")."""
    text = text[:MAX_TEXT]
    out = []
    i, n = 0, len(text)
    while i < n and len(out) < MAX_TOKENS:
        c = text[i]
        if digit(c) is not None:
            j = i
            while j < n and digit(text[j]) is not None:
                j += 1
            out.append(("".join(digit(x) for x in text[i:j]), i, j))
            i = j
            continue
        if is_letter(c) and is_dense(c):
            j = i + 1
            while j < n and is_mark(text[j]):
                j += 1
        elif is_letter(c):
            j = i
            while j < n and is_letter(text[j]) and not (is_dense(text[j]) and not is_mark(text[j])):
                j += 1
        elif c in KEPT:
            j = i + 1
        else:
            i += 1
            continue
        out.append((text[i:j].lower(), i, j))
        i = j
    return out


def skeleton(t):
    """The first letter and the consonants after it, without repeats:
    "tomorrow", "tommorow" and "tmrw" are all "tmrw"."""
    if not t or not t[0].isalpha():
        return t
    out = [t[0]]
    for c in t[1:]:
        if c in VOWELS:
            continue
        if c != out[-1]:
            out.append(c)
    return "".join(out)


def kind(t):
    if t[0].isdigit():
        return "#" + str(min(len(t), 5))
    if t[0].isalpha():
        return t
    return t


def joined(spans):
    """For each token, whether the next one follows it with nothing between
    ("6pm", "2nite", "11/21")."""
    return [i + 1 < len(spans) and spans[i][2] == spans[i + 1][1] for i in range(len(spans))]


def features(toks, i, glue):
    """The feature strings of token i of `toks` (strings), where `glue` is
    `joined` of their spans."""
    t = toks[i]
    f = ["w:" + kind(t)]
    if glue[i]:
        f.append("j+:" + kind(toks[i + 1]))
    if i > 0 and glue[i - 1]:
        f.append("j-:" + kind(toks[i - 1]))
    if t[0].isdigit():
        v = int(t) if len(t) <= 4 else 99999
        f.append("v:" + (str(v) if v <= 60 else "big" if v < 1900 else "year" if v <= 2100 else "huge"))
        if len(t) >= 2 and t[0] == "0":
            f.append("lead0")
    elif t[0].isalpha():
        f.append("k:" + skeleton(t))
        f.append("p:" + t[:3])
        f.append("s:" + t[-3:])
        f.append("len:" + str(min(len(t), 9)))
        g = "<" + t + ">"
        for j in range(len(g) - 2):
            f.append("g:" + g[j:j + 3])
    for o in (-3, -2, -1, 1, 2, 3):
        j = i + o
        n = toks[j] if 0 <= j < len(toks) else ("<s>" if j < 0 else "</s>")
        k = kind(n) if n not in ("<s>", "</s>") else n
        f.append(f"w{o}:{k}")
        if abs(o) <= 2 and n[0].isalpha() and len(n) > 1:
            f.append(f"k{o}:{skeleton(n)}")
    prev = kind(toks[i - 1]) if i > 0 else "<s>"
    nxt = kind(toks[i + 1]) if i + 1 < len(toks) else "</s>"
    f.append(f"b-:{prev}|{kind(t)}")
    f.append(f"b+:{kind(t)}|{nxt}")
    f.append(f"tri:{prev}|{kind(t)}|{nxt}")
    return f


def fnv1a(s):
    h = 0x811C9DC5
    for b in s.encode("utf-8"):
        h ^= b
        h = (h * 0x01000193) & 0xFFFFFFFF
    return h


def vector(feats):
    """Sparse, L2-normalized counts: {index: value}."""
    counts = {}
    for s in feats:
        i = fnv1a(s) & (DIM - 1)
        counts[i] = counts.get(i, 0.0) + 1.0
    norm = sum(v * v for v in counts.values()) ** 0.5
    return {i: v / norm for i, v in counts.items()} if norm else {}

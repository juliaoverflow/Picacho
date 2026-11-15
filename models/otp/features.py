"""Sign-in code candidates and their features.

This is the reference for `src/manage/local_ai/otp.rs`: the app finds
candidates and builds features exactly as here, so the model trained on
them scores the same numbers in the app. `parity.json`, written by
`train.py`, holds examples both must agree on.
"""

import json
import re
import unicodedata
from pathlib import Path

# Hashed feature space (a power of two) and context sizes.
DIM = 1 << 14
WINDOW = 200
CONTEXT = 8
NEAR = 3
SUBJECT_WORDS = 16
# Candidates looked at per message, and the text they are looked for in.
MAX_CANDIDATES = 40
MAX_TEXT = 20000

# 2-4 groups of 3-6 uppercase letters and digits split by hyphens, with a
# letter in them (Bluesky's "BAJ6K-654XM"), or two groups of 3-4 digits split
# by a space or hyphen, or 4-10 uppercase letters and digits (at least one
# digit), or 4-10 digits in any script (fullwidth, Arabic-Indic...);
# optionally after a short uppercase prefix and hyphen, as in Google's
# "G-123456". The candidate is the group: what comes before it is the start
# of the text or a character that isn't an ASCII letter or digit, so it can
# follow Chinese or Japanese without a space.
CANDIDATE = re.compile(
    r"(?:^|[^A-Za-z0-9])((?:[A-Z]{1,3}-)?(?:[A-Z0-9]{3,6}(?:-[A-Z0-9]{3,6}){1,3}|\d{3,4}[ \-]\d{3,4}|[A-Z0-9]{4,10}|\d{4,10}))"
)
# The two digit groups a code may be split into.
DIGIT_GROUPS = re.compile(r"\d{3,4}[ \-]\d{3,4}")

# Scripts written without spaces between words: Hiragana, Katakana, CJK
# ideographs, Thai, Lao, Khmer and Myanmar. Their words are taken as
# overlapping pairs of characters.
DENSE = [
    (0x0E00, 0x0EFF), (0x1000, 0x109F), (0x1780, 0x17FF), (0x3040, 0x30FF),
    (0x3400, 0x4DBF), (0x4E00, 0x9FFF), (0xF900, 0xFAFF), (0xFF66, 0xFF9F),
]


# Concepts that carry across languages ("code", "verify", "order"...), each
# with the word stems that name it; "=" marks a stem that must be the whole
# word and "^" one the word must start with ("^order" names "orders" but not
# German "angefordert", requested). The app builds the same features from
# the same file.
LEXICON = json.loads((Path(__file__).parent / "lexicon.json").read_text(encoding="utf-8"))


def concepts(ws):
    """The concepts named by any of the words, in lexicon order."""
    found = []
    for name, stems in LEXICON:
        for st in stems:
            if st.startswith("="):
                hit = st[1:] in ws
            elif st.startswith("^"):
                hit = any(w.startswith(st[1:]) for w in ws)
            else:
                hit = any(st in w for w in ws)
            if hit:
                found.append(name)
                break
    return found


def candidates(text):
    """(start, end) of each candidate in `text`, in code points."""
    text = text[:MAX_TEXT]
    out = []
    for m in CANDIDATE.finditer(text):
        s, e = m.start(1), m.end(1)
        code = m.group(1)
        # Ends where a word would: not before an ASCII letter or digit.
        if e < len(text) and text[e].isascii() and text[e].isalnum():
            continue
        core = code.split("-", 1)[1] if re.match(r"[A-Z]{1,3}-", code) else code
        if not any(c.isdigit() for c in core):
            continue
        # Hyphenated groups need a letter, unless they are two digit groups:
        # "94043-1351" or "12345-678" are a postcode or part of a number.
        if "-" in core and not any("A" <= c <= "Z" for c in core) and not DIGIT_GROUPS.fullmatch(core):
            continue
        # Part of a longer number: a phone number, a date, an amount.
        before = text[s - 1] if s > 0 else ""
        before2 = text[s - 2] if s > 1 else ""
        after = text[e] if e < len(text) else ""
        after2 = text[e + 1] if e + 1 < len(text) else ""
        if before in "-./:,+" and before != "" and before2.isdigit():
            continue
        if after in "-./:," and after != "" and after2.isdigit():
            continue
        if before == "+" :
            continue
        out.append((s, e))
        if len(out) == MAX_CANDIDATES:
            break
    return out


def is_word(c):
    return c == "_" or unicodedata.category(c)[0] in "LNM"


def is_dense(c):
    o = ord(c)
    return any(a <= o <= b for a, b in DENSE)


def words(text, pairs=True):
    """Lowercase words: runs of letters, marks, digits and underscores, with
    runs in scripts written without spaces split into pairs of characters
    (or, without `pairs`, kept whole, so that concepts can be looked for in
    them: "ワンタイムコード" names *once* and *code*)."""
    out = []
    run = []

    def flush():
        # Split the run where it moves in or out of a dense script.
        start = 0
        for i in range(1, len(run) + 1):
            if i == len(run) or is_dense(run[i]) != is_dense(run[start]):
                seg = "".join(run[start:i])
                if pairs and is_dense(run[start]) and len(seg) > 1:
                    out.extend(seg[j:j + 2] for j in range(len(seg) - 1))
                else:
                    out.append(seg)
                start = i
        run.clear()

    for c in text.lower():
        if is_word(c):
            run.append(c)
        elif run:
            flush()
    if run:
        flush()
    return out


def shape(code):
    out = []
    for c in code:
        if c.isdigit():
            out.append("9")
        elif "A" <= c <= "Z":
            out.append("A")
        else:
            out.append(c)
    return "".join(out)


def features(subject, text, start, end):
    """The feature strings of the candidate at text[start:end], where text is
    the subject, a newline, and the body."""
    code = text[start:end]
    left = text[max(0, start - WINDOW):start]
    right = text[end:end + WINDOW]
    lw = words(left)[-CONTEXT:]
    rw = words(right)[:CONTEXT]
    f = []
    # Words, and their first four letters, which carry across forms and
    # languages ("verification", "verificación", "verificatie").
    for w in lw:
        f.append("L:" + w)
        f.append("LP:" + w[:4])
    for w in rw:
        f.append("R:" + w)
        f.append("RP:" + w[:4])
    for i, w in enumerate(reversed(lw[-NEAR:])):
        f.append(f"L{i}:{w}")
    for i, w in enumerate(rw[:NEAR]):
        f.append(f"R{i}:{w}")
    if len(lw) >= 2:
        f.append(f"LB:{lw[-2]}_{lw[-1]}")
    if len(rw) >= 2:
        f.append(f"RB:{rw[0]}_{rw[1]}")
    prev = left.rstrip()[-1:]
    nxt = right.lstrip()[:1]
    f.append("P:" + prev.lower())
    f.append("N:" + nxt.lower())
    f.append("shape:" + shape(code))
    alnum = sum(1 for c in code if c.isalnum())
    f.append(f"len:{alnum}")
    core = code.replace(" ", "").replace("-", "")
    f.append("kind:digits" if core.isdigit() else "kind:mixed")
    if " " in code or "-" in code:
        f.append("sep")
    line_start = text.rfind("\n", 0, start) + 1
    line_end = text.find("\n", end)
    if line_end < 0:
        line_end = len(text)
    line = text[line_start:line_end].strip()
    if line == code:
        f.append("ownline")
    f.append(f"line:{min(len(line) // 10, 10)}")
    if start < len(subject):
        f.append("insubject")
    sw = words(subject)[:SUBJECT_WORDS]
    for w in sw:
        f.append("S:" + w)
        f.append("SP:" + w[:4])
    # What the nearby words are about, whatever the language.
    for region, ws in (("CL", lw), ("CR", rw), ("CS", sw), ("CNL", lw[-NEAR:]), ("CNR", rw[:NEAR])):
        for name in concepts(ws):
            f.append(f"{region}:{name}")
    # And what the whole window on each side, and the subject, are about,
    # with words in scripts written without spaces kept whole: a code often
    # sits far from the words that say what it is ("dies ist ein einmaliger
    # Roblox-Code, der für ein Konto angefordert wurde, ...: 456360", or
    # "293393は、...ワンタイムコードです").
    for region, t in (("CWL", left), ("CWR", right), ("CWS", subject)):
        for name in concepts(words(t, pairs=False)):
            f.append(f"{region}:{name}")
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

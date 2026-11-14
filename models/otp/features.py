"""Sign-in code candidates and their features.

This is the reference for `src/manage/local_ai/otp.rs`: the app finds
candidates and builds features exactly as here, so the model trained on
them scores the same numbers in the app. `parity.json`, written by
`train.py`, holds examples both must agree on.
"""

import re

# Hashed feature space (a power of two) and context sizes.
DIM = 1 << 14
WINDOW = 200
CONTEXT = 8
NEAR = 3
SUBJECT_WORDS = 16
# Candidates looked at per message, and the text they are looked for in.
MAX_CANDIDATES = 40
MAX_TEXT = 20000

# 4-10 digits, or uppercase letters and digits (at least one digit), or two
# groups of 3-4 digits split by a space or hyphen; optionally after a short
# uppercase prefix and hyphen, as in Google's "G-123456".
CANDIDATE = re.compile(r"\b(?:[A-Z]{1,3}-)?(?:\d{3,4}[ \-]\d{3,4}|[A-Z0-9]{4,10})\b")
WORD = re.compile(r"\w+")


def candidates(text):
    """(start, end) of each candidate in `text`, in code points."""
    text = text[:MAX_TEXT]
    out = []
    for m in CANDIDATE.finditer(text):
        s, e = m.start(), m.end()
        code = m.group(0)
        core = code.split("-", 1)[1] if re.match(r"[A-Z]{1,3}-", code) else code
        if not any(c.isdigit() for c in core):
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


def words(text):
    return WORD.findall(text.lower())


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
    for w in words(subject)[:SUBJECT_WORDS]:
        f.append("S:" + w)
        f.append("SP:" + w[:4])
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

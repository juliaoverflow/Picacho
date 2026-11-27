"""What the inbox category model reads in a message.

This is the reference for `src/manage/local_ai/categories.rs`: the app builds
features exactly as here, with lexicon.json and senders.json compiled in, so
the model scores the same message the same way in the app. parity.json,
written by train.py, holds examples both must agree on.

Only what a message list holds is read: the sender's name and address, the
subject, the preview (the start of the text), and the list headers
(List-Unsubscribe, List-Id, whether List-Post gives an address). Words are
the sign-in code model's (../otp/features.py): lowercase runs of letters,
marks and digits, with scripts written without spaces split into
overlapping pairs of characters. Concepts (lexicon.json) name the same
thing in several languages ("rebaja", "Rabatt" and "セール" are all *sale*).
Everything is hashed into DIM slots.
"""

import importlib.util
import json
from pathlib import Path

HERE = Path(__file__).parent


def _otp_features():
    spec = importlib.util.spec_from_file_location("otp_features", HERE.parent / "otp" / "features.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


OTP = _otp_features()
words = OTP.words
fnv1a = OTP.fnv1a

# Hashed feature space (a power of two), and how much is read.
DIM = 1 << 15
SUBJECT_WORDS = 16
PREVIEW_CHARS = 400
PREVIEW_WORDS = 60
PREVIEW_PAIRS = 30
NAME_WORDS = 4
LOCAL_TOKENS = 4

# Concepts with stems in several languages: "=" marks a stem that must be a
# whole word, "^" one a word must start with, and a stem with a space is
# looked for across words ("free shipping").
LEXICON = json.loads((HERE / "lexicon.json").read_text(encoding="utf-8"))
# Marketing services (seen in List-Unsubscribe or the sender's domain), and
# what the addresses no one reads have in them.
SENDERS = json.loads((HERE / "senders.json").read_text(encoding="utf-8"))

# Emoji shops put in subjects.
PROMO_EMOJI = "🛍🎁💸💰🏷🤑🛒🔥⚡🎉⏰💥"
CURRENCY = "$€£¥₹₽₩₺₫฿₪₴৳₱"
REPLY_PREFIXES = ("re:", "aw:", "sv:", "fw:", "fwd:", "wg:", "tr:", "rv:", "odp:", "回复:", "答复:")


def concepts(ws):
    """The concepts named by the words, in lexicon order."""
    joined = " " + " ".join(ws) + " "
    whole = set(ws)
    found = []
    for name, stems in LEXICON:
        for st in stems:
            if st.startswith("="):
                hit = st[1:] in whole
            elif st.startswith("^"):
                s = st[1:]
                hit = (" " + s) in joined if " " in s else any(w.startswith(s) for w in ws)
            elif " " in st:
                hit = st in joined
            else:
                hit = any(st in w for w in ws)
            if hit:
                found.append(name)
                break
    return found


def is_digit(c):
    return "0" <= c <= "9"


def address_parts(address):
    """The address's part before the "@" and its domain, lowercase."""
    a = address.strip().lower()
    if "@" not in a:
        return a, ""
    local, domain = a.rsplit("@", 1)
    return local, domain


def tokens(text):
    """Runs of ASCII letters and digits, without trailing digits, and
    leaving out those that were only digits."""
    out = []
    run = ""
    for c in text + " ":
        if c.isascii() and c.isalnum():
            run += c
        else:
            t = run.rstrip("0123456789")
            if t:
                out.append(t)
            run = ""
    return out


def priced(text):
    """A currency sign beside a digit ("$20", "20 €" does not count, "20€")."""
    for i, c in enumerate(text):
        if c in CURRENCY:
            before = text[i - 1] if i > 0 else ""
            after = text[i + 1] if i + 1 < len(text) else ""
            if is_digit(before) or is_digit(after):
                return True
    return False


def percent(text):
    for i, c in enumerate(text):
        if c in "%％" and i > 0 and is_digit(text[i - 1]):
            return True
    return False


def person(name):
    """A name like a person's: two or three words, each starting with an
    uppercase letter, without digits or an "@"."""
    parts = name.split()
    if not 2 <= len(parts) <= 3 or "@" in name or any(is_digit(c) for c in name):
        return False
    return all(p[0].isupper() for p in parts)


def features(name, address, subject, preview, unsubscribe, list_id, list_post):
    """The feature strings of a message."""
    preview = preview[:PREVIEW_CHARS]
    name = name.strip()
    sw = words(subject)[:SUBJECT_WORDS]
    pw = words(preview)[:PREVIEW_WORDS]
    nw = words(name)[:NAME_WORDS]
    local, domain = address_parts(address)
    lt = tokens(local)[:LOCAL_TOKENS]
    f = ["S:" + w for w in sw]
    f += [f"SB:{a}_{b}" for a, b in zip(sw, sw[1:])]
    f += ["P:" + w for w in pw]
    pp = pw[:PREVIEW_PAIRS]
    f += [f"PB:{a}_{b}" for a, b in zip(pp, pp[1:])]
    f += ["N:" + w for w in nw]
    f += ["L:" + t for t in lt]
    if lt:
        f.append("LW:" + lt[0])
    labels = [l for l in domain.split(".") if l]
    if len(labels) >= 2:
        f.append("D:" + labels[-2])
        f.append("DT:" + labels[-1])
        f.append("R:" + labels[-2] + "." + labels[-1])
        for sub in labels[:-2]:
            for t in tokens(sub)[:1]:
                f.append("DS:" + t)
        f.append(f"sub:{min(len(labels) - 2, 2)}")
    f += ["C:" + c for c in concepts(words(subject + "\n" + preview, False))]
    f += ["CS:" + c for c in concepts(words(subject, False))]
    lower_unsub = unsubscribe.lower()
    if unsubscribe.strip():
        f.append("unsub")
        if "mailto:" in lower_unsub:
            f.append("unsub:mailto")
        if "http" in lower_unsub:
            f.append("unsub:http")
    if any(s in lower_unsub or s in domain for s in SENDERS["services"]):
        f.append("esp")
    if list_id:
        f.append("listid")
        for l in list_id.lower().split(".")[:-1]:
            for t in tokens(l)[:1]:
                f.append("LI:" + t)
    if list_post:
        f.append("listpost")
    squeezed = "".join(c for c in local if c.isascii() and c.isalnum())
    if any(a in squeezed for a in SENDERS["automated"]):
        f.append("auto")
    text = subject + "\n" + preview
    if percent(text):
        f.append("pct")
    if priced(text):
        f.append("money")
    if any(e in subject for e in PROMO_EMOJI):
        f.append("emoji")
    s = subject.strip().lower()
    if s.startswith(REPLY_PREFIXES):
        f.append("re")
    if s.startswith("[") and "]" in s:
        f.append("tag")
    if any(len(w) >= 3 and w.isascii() and w.isalpha() and w.isupper() for w in subject.split()):
        f.append("caps")
    f.append(f"digits:{min(sum(is_digit(c) for c in subject), 9) // 3}")
    brand = labels[-2].replace("-", "") if len(labels) >= 2 else ""
    squashed = "".join(name.lower().split())
    if len(brand) >= 3 and squashed and (brand in squashed or squashed in brand):
        f.append("brand")
    if person(name):
        f.append("person")
    if not name or name.lower() in (address.strip().lower(), local):
        f.append("noname")
    f.append(f"slen:{min(len(sw) // 4, 4)}")
    return f


def vector(feats):
    """Sparse, L2-normalized counts: {index: value}."""
    counts = {}
    for s in feats:
        i = fnv1a(s) & (DIM - 1)
        counts[i] = counts.get(i, 0.0) + 1.0
    norm = sum(v * v for v in counts.values()) ** 0.5
    return {i: v / norm for i, v in counts.items()} if norm else {}

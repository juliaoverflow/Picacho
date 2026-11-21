"""What the reply model reads in a message.

This is the reference for `src/manage/local_ai/replies.rs`: the app cleans
messages and builds features exactly as here, with lexicon.json compiled in,
so the model scores the same message the same way in the app. parity.json,
written by train.py, holds examples both must agree on.

Words are the sign-in code model's (../otp/features.py): lowercase runs of
letters, marks and digits, with scripts written without spaces (Chinese,
Japanese, Thai...) split into overlapping pairs of characters. The features
of a message are its subject's words, its body's words and word pairs, the
words of lines that ask something, the concepts (lexicon.json) the body,
those lines and the subject name in any language, how many questions it
asks, whether it exclaims, and its length. They are hashed into DIM slots.
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

# Hashed feature space (a power of two), and how much of a message is read.
DIM = 1 << 15
MAX_TEXT = 4000
SUBJECT_WORDS = 16
BODY_WORDS = 160
QUESTION_LINES = 4
QUESTION_WORDS = 24
QUESTION_MARKS = "?？؟"

# Concepts that carry across languages (invite, sorry, sick, deadline...),
# each with stems in every language; "=" marks a stem that must be a whole
# word, "^" one a word must start with, and a stem with a space is looked
# for across words ("how are you"). The app reads the same file.
LEXICON = json.loads((HERE / "lexicon.json").read_text(encoding="utf-8"))


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


def clean(body):
    """The part of a body its sender wrote: up to the signature ("--") or a
    forwarded or quoted original ("-----", "_____"), without quoted lines."""
    out = []
    for line in body[:MAX_TEXT].split("\n"):
        t = line.strip()
        if t == "--" or t.startswith("-----") or t.startswith("_____"):
            break
        if t.startswith(">"):
            continue
        out.append(line)
    return "\n".join(out)


def features(subject, body):
    """The feature strings of a message."""
    text = clean(body)
    sw = words(subject)[:SUBJECT_WORDS]
    bw = words(text)[:BODY_WORDS]
    f = ["S:" + w for w in sw]
    f += ["B:" + w for w in bw]
    f += [f"BB:{a}_{b}" for a, b in zip(bw, bw[1:])]
    asked = 0
    for line in text.split("\n"):
        if asked == QUESTION_LINES:
            break
        if any(q in line for q in QUESTION_MARKS):
            asked += 1
            f += ["QW:" + w for w in words(line)[:QUESTION_WORDS]]
            f += ["QC:" + c for c in concepts(words(line, False))]
    f += ["C:" + c for c in concepts(words(text, False)[:BODY_WORDS])]
    f += ["CS:" + c for c in concepts(words(subject, False))]
    q = sum(text.count(m) for m in QUESTION_MARKS)
    f.append(f"q:{min(q, 3)}")
    if "!" in text or "！" in text:
        f.append("ex")
    f.append(f"len:{min(len(bw) // 20, 6)}")
    return f


def vector(feats):
    """Sparse, L2-normalized counts: {index: value}."""
    counts = {}
    for s in feats:
        i = fnv1a(s) & (DIM - 1)
        counts[i] = counts.get(i, 0.0) + 1.0
    norm = sum(v * v for v in counts.values()) ** 0.5
    return {i: v / norm for i, v in counts.items()} if norm else {}

"""Tracking number features: the sign-in code model's (`../otp/features.py`)
and what the words around the number say about parcels.

This is the reference for `src/manage/local_ai/parcel.rs`: the app builds
the same features, with `lexicon.json` compiled in, and `parity.json`
(written by `train.py`) holds examples both must agree on.
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
DIM = OTP.DIM
MAX_TEXT = OTP.MAX_TEXT
vector = OTP.vector
words = OTP.words
# How far before the number the close window reaches, in characters.
CLOSE = 60

# What words around a number say it is: concepts for parcels (track, parcel,
# deliver, ship) and for the other numbers mail gives (order, invoice,
# customer, phone, reference), each with stems in many languages; "=" marks
# a stem that must be the whole word and "^" one the word must start with.
LEXICON = json.loads((HERE / "lexicon.json").read_text(encoding="utf-8"))


def concepts(ws):
    """The parcel concepts named by any of the words, in lexicon order."""
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


def features(subject, text, start, end):
    """The feature strings of the number at text[start:end], where text is
    the subject, a newline and the body."""
    f = OTP.features(subject, text, start, end)
    left = text[max(0, start - OTP.WINDOW):start]
    right = text[end:end + OTP.WINDOW]
    lw = words(left)[-OTP.CONTEXT:]
    rw = words(right)[:OTP.CONTEXT]
    sw = words(subject)[:OTP.SUBJECT_WORDS]
    for region, ws in (("QL", lw), ("QR", rw), ("QNL", lw[-OTP.NEAR:]), ("QNR", rw[:OTP.NEAR]),
                       ("QS", sw)):
        for name in concepts(ws):
            f.append(f"{region}:{name}")
    # Close before the number on its line, and close before it at all, with
    # words in scripts written without spaces kept whole.
    line_start = text.rfind("\n", 0, start) + 1
    for region, t in (("QLL", text[line_start:start]), ("QWL", left[-CLOSE:]), ("QWS", subject)):
        for name in concepts(words(t, pairs=False)):
            f.append(f"{region}:{name}")
    return f

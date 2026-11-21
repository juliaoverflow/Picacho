"""Synthetic mail for training the reply model.

Each message is built from languages.py in one language: perhaps a greeting
and some small talk, what the message is about (one situation, sometimes
with a second one after or before it), perhaps a sign-off, and a subject.
Its target is a distribution over the model's classes (catalog.json's
replies, then "none"): the replies that fit its situation, the likeliest
weighted most, with a share for the second situation's; automated mail
(newsletters, notifications) is "none".

`half` picks the alternatives used (0: even-numbered, 1: odd), so the
held-out half measures how the model does on wording it never saw.
"""

import json
import random
import re
import unicodedata
from pathlib import Path

from languages import L, LANGUAGES, NAMES, SITUATIONS, TIMES, WEIGHT, alternatives, days

HERE = Path(__file__).parent
CATALOG = json.loads((HERE / "catalog.json").read_text(encoding="utf-8"))
# The model's classes: every reply, then "none".
CLASSES = CATALOG["intents"] + ["none"]
INDEX = {c: i for i, c in enumerate(CLASSES)}
TOPICS = [s for s in SITUATIONS if s != "automated"]
QUESTION_MARKS = re.compile("[?？؟]")


def phrases(lang, key, half):
    alts = alternatives(lang, key)
    if half is None:
        return alts
    return [a for i, a in enumerate(alts) if i % 2 == half] or alts


def strip_accents(s):
    return "".join(c for c in unicodedata.normalize("NFD", s) if unicodedata.category(c) != "Mn")


def target(sit, second=None):
    """The distribution over classes a message about `sit` (and `second`)
    should get."""
    t = {}
    if sit == "automated":
        return {"none": 1.0}
    for share, s in ((0.65 if second else 1.0, sit), (0.35, second)):
        if not s:
            continue
        fits = SITUATIONS[s]
        weights = [len(fits) - i for i in range(len(fits))]
        total = sum(weights)
        for intent, w in zip(fits, weights):
            t[intent] = t.get(intent, 0.0) + share * w / total
    return t


def message(rng, lang, half):
    """(subject, body, situation, second situation or None)."""
    pick = lambda key: rng.choice(phrases(lang, key, half))
    day_names = days(lang)

    def fill(s):
        return (s.replace("{day}", rng.choice(day_names))
                .replace("{time}", rng.choice(TIMES))
                .replace("{name}", rng.choice(NAMES)))

    sit = "automated" if rng.random() < 0.12 else rng.choice(TOPICS)
    main = fill(pick(sit))
    second = None
    sentences = [main]
    if sit != "automated" and rng.random() < 0.25:
        second = rng.choice([s for s in TOPICS if s != sit])
        other = fill(pick(second))
        sentences = [main, other] if rng.random() < 0.7 else [other, main]
    parts = []
    if sit != "automated" and rng.random() < 0.7:
        parts.append(fill(pick("greet")))
    if sit != "automated" and rng.random() < 0.25:
        parts.append(fill(pick("filler")))
    glue = rng.choice([" ", "\n", "\n\n"])
    parts.append(glue.join(sentences))
    if sit != "automated" and rng.random() < 0.6:
        parts.append(fill(pick("sign")))
    body = rng.choice(["\n\n", "\n"]).join(parts)
    r = rng.random()
    if r < 0.45:
        subject = fill(pick("subject"))
    elif r < 0.6:
        subject = "Re: " + fill(pick("subject"))
    elif r < 0.8:
        subject = main[:60]
    else:
        subject = ""
    # Untidy writing: all lowercase, questions without their marks, accents
    # left off.
    if rng.random() < 0.1:
        subject, body = subject.lower(), body.lower()
    if rng.random() < 0.1:
        body = QUESTION_MARKS.sub("", body)
    if rng.random() < 0.06:
        subject, body = strip_accents(subject), strip_accents(body)
    return subject, body, sit, second


def examples(rng, n, half=None):
    """n messages: (subject, body, language, target, situation, second)."""
    weights = [WEIGHT.get(lang, 1.5) for lang in LANGUAGES]
    out = []
    for _ in range(n):
        lang = rng.choices(LANGUAGES, weights)[0]
        h = half if half is not None else rng.randrange(2)
        subject, body, sit, second = message(rng, lang, h)
        out.append((subject, body, lang, target(sit, second), sit, second))
    return out


if __name__ == "__main__":
    rng = random.Random(1)
    for subject, body, lang, t, sit, second in examples(rng, 8):
        print(f"--- {lang} {sit} {second or ''}\nSubject: {subject}\n{body}\n{t}\n")

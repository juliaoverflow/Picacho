"""Trains Echo Replies, the reply suggestion model, and writes what the
app installs.

    python3 train.py            # numpy only: replies-v1.bin, classes.json,
                                # parity.json, eval.json
    python3 train.py --quick    # a small run that reports how it does

The model reads a message (features.py) and scores every reply in
catalog.json, and "none" for mail that wants no reply: a bag of hashed
features summed into one small hidden layer, then a softmax over the
classes. It is trained on soft targets (data.py): the replies that fit a
message share its probability. The embedding is stored as 8-bit integers
with a scale per row, in the date model's `PICAWHN1` format, which the
app's own runtime (`src/manage/local_ai/runtime.rs`, `NativeTagger`) reads
on every platform. After retraining, update the size and SHA-256 of
`replies-v1.bin` in `src/manage/local_ai/mod.rs` (REPLIES).

catalog.json's wordings can change without retraining; its list of replies
can't, since the model's outputs are in classes.json's order.
"""

import hashlib
import json
import random
import struct
import sys
import time
from pathlib import Path

import numpy as np

np.seterr(all="ignore")

import data
from data import CLASSES, INDEX
from features import DIM, features, fnv1a, vector

HIDDEN = 32
NONE = INDEX["none"]
HERE = Path(__file__).parent


# Features naming the message's own words, rather than what they are about.
LEXICAL = ("S:", "B:", "BB:", "QW:")


def featurize(exs):
    """CSR arrays of hashed features (one row per message) and whether
    each is one of the message's words, and the targets as a dense
    matrix."""
    idx, lexical, ptr = [], [], [0]
    T = np.zeros((len(exs), len(CLASSES)), np.float32)
    for row, (subject, body, _, target, *_) in enumerate(exs):
        for f in features(subject, body):
            idx.append(fnv1a(f) & (DIM - 1))
            lexical.append(f.startswith(LEXICAL))
        ptr.append(len(idx))
        for c, p in target.items():
            T[row, INDEX[c]] = p
    return np.array(idx, np.int64), np.array(lexical, bool), np.array(ptr, np.int64), T


def normalized(idx, ptr, keep):
    """Rows of (index, value) as `vector` makes them, from the features
    kept: duplicates summed, each row L2-normalized."""
    rows = np.repeat(np.arange(len(ptr) - 1), np.diff(ptr))
    rows, idx = rows[keep], idx[keep]
    key = rows * DIM + idx
    uniq, counts = np.unique(key, return_counts=True)
    r = uniq // DIM
    i = uniq % DIM
    c = counts.astype(np.float32)
    norm = np.sqrt(np.bincount(r, weights=c * c, minlength=len(ptr) - 1)).astype(np.float32)
    v = c / np.maximum(norm[r], 1e-9)
    return r, i, v


class Model:
    def __init__(self, rng):
        self.E = (rng.standard_normal((DIM, HIDDEN)) * 0.1).astype(np.float32)
        self.b1 = np.zeros(HIDDEN, np.float32)
        self.W2 = (rng.standard_normal((HIDDEN, len(CLASSES))) * 0.1).astype(np.float32)
        self.b2 = np.zeros(len(CLASSES), np.float32)

    def params(self):
        return [self.E, self.b1, self.W2, self.b2]

    def forward(self, rows, idx, val, n):
        pre = np.zeros((n, HIDDEN), np.float32)
        np.add.at(pre, rows, self.E[idx] * val[:, None])
        pre += self.b1
        h = np.maximum(pre, 0)
        z = h @ self.W2 + self.b2
        z -= z.max(1, keepdims=True)
        p = np.exp(z)
        p /= p.sum(1, keepdims=True)
        return pre, h, p

    def predict(self, subject, body):
        """The probability of each class for one message."""
        x = vector(features(subject, body))
        if not x:
            idx, val = np.zeros(0, np.int64), np.zeros(0, np.float32)
        else:
            idx = np.array(list(x.keys()), np.int64)
            val = np.array(list(x.values()), np.float32)
        _, _, p = self.forward(np.zeros(len(idx), np.int64), idx, val, 1)
        return p[0]


def train(exs, epochs=10, lr=0.01, l2=1e-6, drop=0.15, words_out=0.3, seed=1, batch=256):
    """Trains on the examples. Each epoch leaves out a random `drop` share
    of every message's features, so no one word decides, and all the words
    of a `words_out` share of messages, keeping what they are about (the
    concepts, questions and length): wording the model never saw still
    names concepts it knows."""
    t0 = time.time()
    idx, lexical, ptr, T = featurize(exs)
    n = len(T)
    print(f"{n} messages, {len(idx)} features ({time.time() - t0:.0f}s)")
    rng = np.random.default_rng(seed)
    model = Model(rng)
    m = [np.zeros_like(p) for p in model.params()]
    v = [np.zeros_like(p) for p in model.params()]
    step = 0
    for epoch in range(epochs):
        keep = rng.random(len(idx)) >= drop
        wordless = rng.random(n) < words_out
        keep &= ~(lexical & wordless[np.repeat(np.arange(n), np.diff(ptr))])
        r, i, val = normalized(idx, ptr, keep)
        perm = rng.permutation(n)
        rank = np.empty(n, np.int64)
        rank[perm] = np.arange(n)
        order = np.argsort(rank[r], kind="stable")
        r, i, val = rank[r][order], i[order], val[order]
        bounds = np.searchsorted(r, np.arange(0, n + batch, batch))
        total = 0.0
        rate = lr * (0.5 * (1 + np.cos(np.pi * epoch / epochs)) * 0.9 + 0.1)
        for b in range(len(bounds) - 1):
            lo, hi = bounds[b], bounds[b + 1]
            first = b * batch
            count = min(batch, n - first)
            if count <= 0:
                continue
            br, bi, bv = r[lo:hi] - first, i[lo:hi], val[lo:hi]
            bt = T[perm[first:first + count]]
            pre, h, p = model.forward(br, bi, bv, count)
            total += float(-(bt * np.log(p + 1e-9)).sum())
            dz = (p - bt) / count
            g_W2 = h.T @ dz + l2 * model.W2
            g_b2 = dz.sum(0)
            dh = (dz @ model.W2.T) * (pre > 0)
            g_b1 = dh.sum(0)
            g_E = np.zeros_like(model.E)
            np.add.at(g_E, bi, dh[br] * bv[:, None])
            g_E += l2 * model.E
            step += 1
            for k, (p_, g) in enumerate(zip(model.params(), [g_E, g_b1, g_W2, g_b2])):
                m[k] = 0.9 * m[k] + 0.1 * g
                v[k] = 0.999 * v[k] + 0.001 * g * g
                mh = m[k] / (1 - 0.9 ** step)
                vh = v[k] / (1 - 0.999 ** step)
                p_ -= (rate * mh / (np.sqrt(vh) + 1e-8)).astype(np.float32)
        print(f"epoch {epoch + 1}: loss {total / n:.4f} ({time.time() - t0:.0f}s)")
    return model


def quantize(model):
    """The embedding as int8 rows with a scale each; the model with it
    dequantized, so evaluation sees what the app runs."""
    scale = np.abs(model.E).max(1) / 127.0
    scale[scale == 0] = 1.0
    q = np.clip(np.round(model.E / scale[:, None]), -127, 127).astype(np.int8)
    out = Model(np.random.default_rng(0))
    out.E = (q.astype(np.float32) * scale[:, None]).astype(np.float32)
    out.b1, out.W2, out.b2 = model.b1, model.W2, model.b2
    return out, q, scale.astype(np.float32)


def evaluate(model, exs, name, by_language=False):
    """How often the best reply fits, how often one of the top three is
    among the main situation's, and how well mail wanting no reply is told
    apart."""
    if by_language:
        for lang in sorted({e[2] for e in exs}):
            evaluate(model, [e for e in exs if e[2] == lang], f"  {lang}")
    top1 = top3 = n = 0
    none_tp = none_fp = none_fn = 0
    for subject, body, _, target, sit, _ in exs:
        p = model.predict(subject, body)
        gold_none = sit == "automated"
        said_none = int(np.argmax(p)) == NONE
        none_tp += gold_none and said_none
        none_fp += said_none and not gold_none
        none_fn += gold_none and not said_none
        if gold_none:
            continue
        n += 1
        ranked = [k for k in np.argsort(-p) if k != NONE]
        fits = {INDEX[c] for c in target}
        main = {INDEX[c] for c in data.SITUATIONS[sit]}
        top1 += ranked[0] in fits
        top3 += bool(main & set(ranked[:3]))
    print(f"{name}: best reply fits {top1 / max(n, 1):.3f}, a main fit in the top 3 "
          f"{top3 / max(n, 1):.3f}, no-reply mail found {none_tp / max(none_tp + none_fn, 1):.3f} "
          f"(wrongly {none_fp} of {n})")


def write_bin(q, scale, model, path):
    with open(path, "wb") as f:
        f.write(b"PICAWHN1")
        f.write(struct.pack("<III", DIM, HIDDEN, len(CLASSES)))
        f.write(scale.astype("<f4").tobytes())
        f.write(q.astype(np.int8).tobytes())
        f.write(model.b1.astype("<f4").tobytes())
        f.write(model.W2.T.astype("<f4").tobytes())  # one row of HIDDEN per class
        f.write(model.b2.astype("<f4").tobytes())


PARITY = [
    ("Dinner on Friday?", "Hi Sam,\n\nWe're having a few friends over on Friday, would you like to come?\n\nCheers,\nAna"),
    ("Re: report", "Thanks for sending it over!\n\nOn Mon, Ana wrote:\n> Here is the report.\n> Can you check it?"),
    ("", "Could you send me the contract today? It's urgent.\n--\nLuis Pérez\nSales"),
    ("Your order has shipped", "Your order #12345 has shipped. Unsubscribe from these emails at any time."),
    ("Termin", "Hallo,\n\nkönnen wir uns am Dienstag um 10:00 treffen?\n\nViele Grüße\nMarta"),
    ("Hola", "¡Felicidades por el nuevo trabajo! ¿Cómo estás?"),
    ("Absence", "Je suis malade aujourd'hui et ne pourrai pas venir."),
    ("", "明天下午3点可以开会吗？"),
    ("お知らせ", "15分ほど遅れます、すみません！"),
    ("회의", "내일 회의는 취소해야 할 것 같아요."),
    ("Привет", "Как дела? Давно не общались!"),
    ("", "مرفق التقرير الذي طلبته."),
    ("ข่าวดี", "ข่าวดี เราได้สัญญาแล้ว!"),
    ("", "क्या आप कल मुझे कॉल कर सकते हैं?"),
    ("Habari", "Asante sana kwa msaada wako jana."),
    ("Re: Re: plan", "What do you think?\n\n-----Original Message-----\nFrom: Ana\nLet's meet on Friday."),
    ("", ""),
]


def parity(model):
    cases = []
    for subject, body in PARITY:
        feats = features(subject, body)
        p = model.predict(subject, body)
        top = [int(k) for k in np.argsort(-p)[:5]]
        cases.append({
            "subject": subject,
            "body": body,
            "features": feats,
            "vector": sorted([j, round(v, 6)] for j, v in vector(feats).items()),
            "top": [[CLASSES[k], round(float(p[k]), 5)] for k in top],
        })
    return {"classes": CLASSES, "cases": cases}


def main():
    rng = random.Random(20261121)
    if "--quick" in sys.argv:
        model = train(data.examples(rng, 60000, half=0), epochs=4)
        evaluate(model, data.examples(rng, 3000, half=0), "seen wording")
        evaluate(model, data.examples(rng, 6000, half=1), "held-out wording", by_language=True)
        return
    model = train(data.examples(rng, 240000, half=0), epochs=8)
    evaluate(model, data.examples(rng, 4000, half=0), "seen wording")
    evaluate(model, data.examples(rng, 8000, half=1), "held-out wording", by_language=True)
    # The published model learns from every wording.
    everything = data.examples(rng, 360000)
    model = train(everything, epochs=10)
    qmodel, q, scale = quantize(model)
    fresh = data.examples(rng, 6000)
    evaluate(model, fresh, "final, fresh messages")
    evaluate(qmodel, fresh, "final, 8-bit")
    path = HERE / "replies-v1.bin"
    write_bin(q, scale, qmodel, path)
    (HERE / "classes.json").write_text(json.dumps(CLASSES, indent=1) + "\n")
    (HERE / "parity.json").write_text(json.dumps(parity(qmodel), ensure_ascii=False, indent=1) + "\n")
    # Held-out messages for the app's tests: what fits each, and its
    # language (trained on by the published model only as wordings; the
    # messages themselves are fresh).
    ev = data.examples(random.Random(7), 3000, half=1)
    (HERE / "eval.json").write_text(json.dumps([
        {"subject": s, "body": b, "language": lang, "fits": sorted(t), "main": data.SITUATIONS[sit],
         "none": sit == "automated"}
        for s, b, lang, t, sit, _ in ev], ensure_ascii=False) + "\n")
    digest = hashlib.sha256(path.read_bytes()).hexdigest()
    print(f"replies-v1.bin: {path.stat().st_size} bytes, sha256 {digest}")


if __name__ == "__main__":
    main()

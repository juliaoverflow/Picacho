"""Trains Epoch TBS, the date and time model, and writes what the app
downloads.

    python3 train.py            # numpy only: tbs-v1.bin, parity.json, eval.json

The model labels each token of a text with what it means (see features.py):
a bag of hashed features of the token and its neighbours, summed into one
small hidden layer, and a softmax over the labels. The embedding is stored
as 8-bit integers with a scale per row, so the file is about a quarter of
the size of 32-bit floats. `tbs-v1.bin` is read by the app's own runtime
(`src/manage/local_ai/runtime.rs`) on every platform and checked against the
SHA-256 digest in `src/manage/local_ai/mod.rs`; after retraining, update it.

`eval.json` holds texts from the held-out half (wording and spellings never
trained on, with fresh slips) and their true labels; the app's tests run the
model and the rules over them and compare the moments they give with the
moments the true labels give.
"""

import hashlib
import json
import random
import struct
import sys
import time
from pathlib import Path

import numpy as np

# numpy's Accelerate matmul on macOS raises spurious floating-point warnings.
np.seterr(all="ignore")

import data
from features import DIM, INDEX, LABELS, features, fnv1a, joined, tokens, vector

HIDDEN = 32
CLASSES = len(LABELS)
HERE = Path(__file__).parent


def featurize(exs):
    """CSR arrays of hashed features (one row per token), and the labels."""
    idx, ptr, ys = [], [0], []
    for _, spans, labels, *_ in exs:
        toks, glue = [t for t, _, _ in spans], joined(spans)
        for i, label in enumerate(labels):
            for f in features(toks, i, glue):
                idx.append(fnv1a(f) & (DIM - 1))
            ptr.append(len(idx))
            ys.append(INDEX[label])
    return np.array(idx, np.int64), np.array(ptr, np.int64), np.array(ys, np.int64)


def normalized(idx, ptr, keep):
    """Rows of (index, value) as `vector` makes them, from the features
    kept: duplicates summed, each row L2-normalized. Entries sorted by row."""
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
        self.W2 = (rng.standard_normal((HIDDEN, CLASSES)) * 0.1).astype(np.float32)
        self.b2 = np.zeros(CLASSES, np.float32)

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

    def predict(self, spans):
        """Label indices and their probabilities for one text's tokens."""
        if not spans:
            return [], []
        toks, glue = [t for t, _, _ in spans], joined(spans)
        rows, idx, val = [], [], []
        for i in range(len(toks)):
            for j, v in vector(features(toks, i, glue)).items():
                rows.append(i)
                idx.append(j)
                val.append(v)
        _, _, p = self.forward(np.array(rows), np.array(idx), np.array(val, np.float32), len(toks))
        return p.argmax(1).tolist(), p.max(1).tolist()


def train(exs, epochs=10, lr=0.008, l2=1e-6, drop=0.1, seed=1, batch=512):
    """Trains on the examples; each epoch leaves out a random `drop` share
    of every token's features, so no one feature decides."""
    t0 = time.time()
    idx, ptr, ys = featurize(exs)
    n = len(ys)
    print(f"{len(exs)} texts, {n} tokens, {(ys != 0).sum()} in times ({time.time() - t0:.0f}s)")
    rng = np.random.default_rng(seed)
    model = Model(rng)
    m = [np.zeros_like(p) for p in model.params()]
    v = [np.zeros_like(p) for p in model.params()]
    step = 0
    for epoch in range(epochs):
        keep = rng.random(len(idx)) >= drop
        r, i, val = normalized(idx, ptr, keep)
        # Shuffle tokens: a permutation of rows, then entries regrouped.
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
            if lo >= hi:
                continue
            first = b * batch
            count = min(batch, n - first)
            br, bi, bv = r[lo:hi] - first, i[lo:hi], val[lo:hi]
            by = ys[perm[first:first + count]]
            pre, h, p = model.forward(br, bi, bv, count)
            total += float(-np.log(p[np.arange(count), by] + 1e-9).sum())
            dz = p
            dz[np.arange(count), by] -= 1
            dz /= count
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
    if by_language:
        codes = sorted({e[3] for e in exs})
        for code in codes:
            evaluate(model, [e for e in exs if e[3] == code], f"  {code}")
    tok_ok = tok = time_ok = time_n = sent_ok = 0
    neg = neg_clean = 0
    for _, toks, labels, *_ in exs:
        pred, _ = model.predict(toks)
        gold = [INDEX[l] for l in labels]
        tok += len(gold)
        tok_ok += sum(a == b for a, b in zip(pred, gold))
        for a, b in zip(pred, gold):
            if b != 0:
                time_n += 1
                time_ok += a == b
        sent_ok += pred == gold
        if all(g == 0 for g in gold):
            neg += 1
            neg_clean += all(p == 0 for p in pred)
    print(f"{name}: tokens {tok_ok / tok:.4f}, time tokens {time_ok / max(time_n, 1):.4f}, "
          f"texts exactly right {sent_ok / len(exs):.4f}, "
          f"texts without a time left alone {neg_clean / max(neg, 1):.4f}")


def write_bin(q, scale, model, path):
    with open(path, "wb") as f:
        f.write(b"PICAWHN1")
        f.write(struct.pack("<III", DIM, HIDDEN, CLASSES))
        f.write(scale.astype("<f4").tobytes())
        f.write(q.astype(np.int8).tobytes())
        f.write(model.b1.astype("<f4").tobytes())
        f.write(model.W2.T.astype("<f4").tobytes())  # one row of HIDDEN per class
        f.write(model.b2.astype("<f4").tobytes())


PARITY = [
    "tmrw at 6",
    "remind me to call mom tomorow at 6 pls",
    "next fri 3pm",
    "every other monday @ 9:30am",
    "in 2 hrs",
    "nov 21st at noon",
    "may i ask a question",
    "I sat down with the team on the 2nd floor",
    "Snooze until 11/21/2026 18:30",
    "quarter past six tonite",
    "Wensday mornin",
    "an hour and a half from now",
    "the monday in three weeks",
    "start of next week at 7am",
    "in 4 hours at the top of the hour",
    "overmorrow at noon",
    "in a fortnight",
    "Erinnere mich übermorgen um 18:30 Uhr",
    "rappelle-moi vendredi prochain à 18h30",
    "recuérdame pasado mañana a las 6 de la tarde",
    "明天下午6点半提醒我",
    "来週の金曜日の午後3時",
    "내일 오후 6시 30분",
    "พรุ่งนี้ 6 โมงเย็น",
    "कल शाम ६ बजे",
    "غدا الساعة ٦ مساء",
    "через 2 часа",
    "2 saat sonra",
]


def parity(model):
    out = []
    for text in PARITY:
        toks = tokens(text)
        words, glue = [t for t, _, _ in toks], joined(toks)
        pred, prob = model.predict(toks)
        out.append({
            "text": text,
            "tokens": [[t, s, e] for t, s, e in toks],
            "features": [features(words, i, glue) for i in range(len(words))],
            "vectors": [sorted([j, round(v, 6)] for j, v in vector(features(words, i, glue)).items())
                        for i in range(len(words))],
            "labels": [LABELS[p] for p in pred],
            "probabilities": [round(p, 5) for p in prob],
        })
    return {"labels": LABELS, "cases": out}


def main():
    rng = random.Random(20261116)
    t0 = time.time()
    if "--final-only" in sys.argv:
        return publish(rng)
    train_exs = data.examples(rng, 220000, half=0)
    print(f"generated {len(train_exs)} distinct texts ({time.time() - t0:.0f}s)")
    seen = data.examples(rng, 6000, half=0)
    held = data.examples(rng, 8000, half=1)
    model = train(train_exs)
    evaluate(model, seen, "seen wording")
    evaluate(model, held, "held-out wording and spellings", by_language=True)
    publish(rng)


def publish(rng):
    # The published model learns from every form.
    everything = data.examples(rng, 420000)
    print(f"generated {len(everything)} distinct texts")
    model = train(everything, epochs=12)
    qmodel, q, scale = quantize(model)
    fresh = data.examples(rng, 8000)
    evaluate(model, fresh, "final, fresh texts")
    evaluate(qmodel, fresh, "final, 8-bit", by_language=True)
    write_bin(q, scale, qmodel, HERE / "tbs-v1.bin")
    (HERE / "parity.json").write_text(json.dumps(parity(qmodel), ensure_ascii=False, indent=1))
    # Held-out texts for the app's end-to-end test (trained on by the
    # published model only as forms; the texts and slips are fresh).
    ev = data.examples(random.Random(7), 4000, half=1)
    (HERE / "eval.json").write_text(json.dumps(
        [{"text": t, "labels": l, "language": c} for t, _, l, c in ev], ensure_ascii=False))
    path = HERE / "tbs-v1.bin"
    digest = hashlib.sha256(path.read_bytes()).hexdigest()
    print(f"tbs-v1.bin: {path.stat().st_size} bytes, sha256 {digest}")


if __name__ == "__main__":
    if "--quick" in sys.argv:
        rng = random.Random(1)
        m = train(data.examples(rng, 60000, half=0), epochs=4)
        evaluate(m, data.examples(rng, 3000, half=0), "seen")
        evaluate(m, data.examples(rng, 4000, half=1), "held-out", by_language=True)
    else:
        main()

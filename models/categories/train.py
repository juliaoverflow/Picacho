"""Trains Sift Categories, the inbox category model, and writes what the app
installs.

    python3 train.py            # numpy only: categories-v1.bin, classes.json,
                                # parity.json, eval.json
    python3 train.py --quick    # a small run that reports how it does

The model reads what a message list holds of a message (features.py) and
scores the inbox's categories (data.CLASSES): a bag of hashed features
summed into one small hidden layer, then a softmax. It is trained on the
made-up mail of data.py, with soft targets where mail could be either of
two categories. The embedding is stored as 8-bit integers with a scale per
row, in the date model's `PICAWHN1` format, which the app's own runtime
(`src/manage/local_ai/runtime.rs`, `NativeTagger`) reads on every
platform. After retraining, update the size and SHA-256 of
`categories-v1.bin` in `src/manage/local_ai/mod.rs` (CATEGORIES).
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
HERE = Path(__file__).parent

# Features naming the message's own words, rather than what they are about
# or who sent it; and those naming who sent it.
LEXICAL = ("S:", "SB:", "P:", "PB:")
SENDER = ("N:", "L:", "LW:", "D:", "DT:", "R:", "DS:", "sub:", "brand", "person", "noname", "auto")


def feats(m):
    return features(m["name"], m["address"], m["subject"], m["preview"], m["unsub"], m["list_id"], m["list_post"])


def featurize(exs):
    """CSR arrays of hashed features (one row per message), whether each is
    one of the message's words or names its sender, and the targets as a
    dense matrix."""
    idx, lexical, sender, ptr = [], [], [], [0]
    T = np.zeros((len(exs), len(CLASSES)), np.float32)
    for row, m in enumerate(exs):
        for f in feats(m):
            idx.append(fnv1a(f) & (DIM - 1))
            lexical.append(f.startswith(LEXICAL))
            sender.append(f.startswith(SENDER))
        ptr.append(len(idx))
        for c, p in m["target"].items():
            T[row, INDEX[c]] = p
    return (np.array(idx, np.int64), np.array(lexical, bool), np.array(sender, bool),
            np.array(ptr, np.int64), T)


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

    def predict(self, m):
        """The probability of each class for one message."""
        x = vector(feats(m))
        idx = np.array(list(x.keys()), np.int64)
        val = np.array(list(x.values()), np.float32)
        _, _, p = self.forward(np.zeros(len(idx), np.int64), idx, val, 1)
        return p[0]


def train(exs, epochs=8, lr=0.01, l2=1e-6, drop=0.15, words_out=0.3, sender_out=0.15, seed=1, batch=256):
    """Trains on the examples. Each epoch leaves out a random `drop` share
    of every message's features, so no one word decides; all the words of
    a `words_out` share of messages, keeping who sent it, its headers and
    what it is about (concepts), so wording the model never saw still has
    the rest to go on; and who sent a `sender_out` share, so what a message
    says decides too, whoever sends it."""
    t0 = time.time()
    idx, lexical, sender, ptr, T = featurize(exs)
    n = len(T)
    print(f"{n} messages, {len(idx)} features ({time.time() - t0:.0f}s)")
    rng = np.random.default_rng(seed)
    model = Model(rng)
    m = [np.zeros_like(p) for p in model.params()]
    v = [np.zeros_like(p) for p in model.params()]
    step = 0
    for epoch in range(epochs):
        keep = rng.random(len(idx)) >= drop
        row_of = np.repeat(np.arange(n), np.diff(ptr))
        wordless = rng.random(n) < words_out
        keep &= ~(lexical & wordless[row_of])
        nameless = (rng.random(n) < sender_out) & ~wordless
        keep &= ~(sender & nameless[row_of])
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


def evaluate(model, exs, name):
    """Accuracy by category; how often people's mail stays in Primary, and
    how often the model is sure enough (p >= 0.9) to take mail out of it
    wrongly; and what each category is mistaken for."""
    right = {c: 0 for c in CLASSES}
    seen = {c: 0 for c in CLASSES}
    confusion = {c: {d: 0 for d in CLASSES} for c in CLASSES}
    people_out = people_sure_out = people = 0
    sure_wrong = 0
    for m in exs:
        p = model.predict(m)
        guess = CLASSES[int(np.argmax(p))]
        # Wrong while sure enough (70%) to sort mail elsewhere than the
        # rules would in the app.
        sure_wrong += m["target"].get(guess, 0.0) < 0.2 and float(p.max()) >= 0.7
        gold = m["category"]
        best = max(m["target"], key=m["target"].get)
        seen[gold] += 1
        right[gold] += guess in m["target"] and (guess == best or m["target"][guess] >= 0.2)
        confusion[gold][guess] += 1
        if gold == "primary":
            people += 1
            people_out += guess != "primary"
            people_sure_out += guess != "primary" and float(p.max()) >= 0.9
    total = sum(right.values()) / max(sum(seen.values()), 1)
    by = ", ".join(f"{c} {right[c] / max(seen[c], 1):.3f}" for c in CLASSES)
    print(f"{name}: right {total:.3f} ({by}); people's mail out of Primary "
          f"{people_out / max(people, 1):.4f}, surely {people_sure_out / max(people, 1):.4f}; "
          f"wrong while 70% sure {sure_wrong / max(len(exs), 1):.4f}")
    for c in CLASSES:
        row = " ".join(f"{confusion[c][d]:6d}" for d in CLASSES)
        print(f"    {c:>12} {row}")


def by_language(model, exs, name):
    """How often each language's mail (people's, and the rest apart) is
    sorted right: every language the other models have must do well."""
    rows = {}
    for m in exs:
        p = model.predict(m)
        guess = CLASSES[int(np.argmax(p))]
        ok = m["target"].get(guess, 0.0) >= 0.2
        key = "people" if m["category"] == "primary" else "other"
        r = rows.setdefault(m["lang"], {"people": [0, 0], "other": [0, 0]})
        r[key][0] += ok
        r[key][1] += 1
    worst = []
    line = []
    for lang in sorted(rows):
        r = rows[lang]
        share = lambda k: r[k][0] / max(r[k][1], 1)
        worst.append((share("other"), lang))
        line.append(f"{lang} {share('people'):.2f}/{share('other'):.2f}")
    print(f"{name}, by language (people's mail / the rest):")
    for i in range(0, len(line), 8):
        print("    " + "  ".join(line[i:i + 8]))
    worst.sort()
    print(f"    weakest: {', '.join(f'{l} {x:.3f}' for x, l in worst[:5])}")


def write_bin(q, scale, model, path):
    with open(path, "wb") as f:
        f.write(b"PICAWHN1")
        f.write(struct.pack("<III", DIM, HIDDEN, len(CLASSES)))
        f.write(scale.astype("<f4").tobytes())
        f.write(q.astype(np.int8).tobytes())
        f.write(model.b1.astype("<f4").tobytes())
        f.write(model.W2.T.astype("<f4").tobytes())  # one row of HIDDEN per class
        f.write(model.b2.astype("<f4").tobytes())


# Messages whose features and scores the app must reproduce: (name,
# address, subject, preview, List-Unsubscribe, List-Id, List-Post).
PARITY = [
    ("Ana Silva", "ana.silva@gmail.com", "Dinner on Friday?", "Hi Sam, we're having a few friends over, would you like to come?", "", "", False),
    ("Kestrel Outfitters", "news@e.kestrel.co", "Up to 70% off ends tonight 🔥", "View this email in your browser. Our biggest sale of the season. Shop now!", "<https://kestrel.us4.list-manage.com/unsubscribe?u=1>", "4f2a.list-id.mcsv.net", False),
    ("Amazon.com", "auto-confirm@amazon.com", "Your Amazon.com order #112-4456789 has shipped", "Your package is on its way and should arrive by Tuesday.", "", "", False),
    ("Instagram", "no-reply@mail.instagram.com", "lucia.m started following you", "You have 3 new followers.", "<https://instagram.com/emails/unsubscribe>", "", False),
    ("'Bo' via rust-users", "rust-users@googlegroups.com", "Re: [rust-users] Lifetimes question", "I ran into the same problem. On Mon, Ana wrote:", "<mailto:rust-users+unsubscribe@googlegroups.com>", "rust-users.googlegroups.com", True),
    ("Banco Sur", "alertas@ealerts.bancosur.es", "Tu extracto ya está disponible", "Consulta el extracto de tu cuenta terminada en 4821.", "", "", False),
    ("", "Kontakt@Studio-Luma.design", "Angebot für Ihr Projekt", "Hallo Frau Müller, anbei unser Angebot. Viele Grüße, Jonas", "", "", False),
    ("Zalando", "info@service-mail.zalando.de", "Kostenloser Versand auf alles – nur heute", "Jetzt shoppen: 20€ Rabatt ab 100€.", "<mailto:unsub@zalando.de>", "", False),
    ("山田 太郎", "taro.yamada@example.co.jp", "明日の会議について", "明日の会議は15時からでよろしいでしょうか？", "", "", False),
    ("Svea Bank", "noreply@sveabank.se", "Ny inloggning på ditt konto", "Vi har upptäckt en ny inloggning.", "", "", False),
    ("Iberia", "checkin@iberia.com", "Ya puedes hacer el check-in de tu vuelo a Madrid", "Descarga tu tarjeta de embarque. Salida el lunes a las 08:40.", "", "", False),
    ("Revolut", "no-reply@revolut.com", "You received €45.00", "Ana sent you €45.00. The money is in your account.", "", "", False),
    ("Field Notes", "ana@substack.com", "Issue #12: on slow mornings", "In this issue: why slow mornings matter. Read online.", "<https://ana.substack.com/action/disable_email>", "fieldnotes.substack.com", False),
    ("نون", "noreply@noon.com", "خصومات تصل إلى 50% – اليوم فقط", "أكبر تخفيضات الموسم. تسوّق الآن!", "<https://noon.com/u>", "", False),
    ("Shopee", "info@mail.shopee.co.th", "ลดสูงสุด 70% – วันนี้วันเดียว", "ส่งฟรีทุกชิ้นสุดสัปดาห์นี้", "<https://shopee.co.th/u>", "", False),
    ("楽天カード", "info@mail.rakuten-card.co.jp", "ご請求金額のお知らせ", "楽天カードのご請求金額¥12,400のお支払い期限は月曜日です。", "", "", False),
    ("토스", "no-reply@toss.im", "₩50,000이 입금되었습니다", "민지님이 ₩50,000을 보냈습니다.", "", "", False),
    ("Priya", "priya.sharma@gmail.com", "कल मिलते हैं?", "नमस्ते! क्या तुम कल शाम को खाली हो?", "", "", False),
    ("", "", "", "", "", "", False),
]


def parity(model):
    cases = []
    for name, address, subject, preview, unsub, list_id, list_post in PARITY:
        m = dict(name=name, address=address, subject=subject, preview=preview, unsub=unsub,
                 list_id=list_id, list_post=list_post)
        f = feats(m)
        p = model.predict(m)
        cases.append({
            "name": name, "address": address, "subject": subject, "preview": preview,
            "unsubscribe": unsub, "list_id": list_id, "list_post": list_post,
            "features": f,
            "vector": sorted([j, round(v, 6)] for j, v in vector(f).items()),
            "probabilities": [round(float(x), 5) for x in p],
        })
    return {"classes": CLASSES, "cases": cases}


def main():
    rng = random.Random(20261125)
    data.check()
    if "--held-out" in sys.argv:
        # Only the held-out measures, without writing the published model.
        model = train(data.examples(rng, 200000, half=0), epochs=6)
        held = data.examples(rng, 16000, half=1)
        evaluate(model, held, "held-out wording (and kinds)")
        by_language(model, held, "held-out wording (and kinds)")
        return
    if "--quick" in sys.argv:
        model = train(data.examples(rng, 40000, half=0), epochs=4)
        evaluate(model, data.examples(rng, 3000, half=0), "seen wording")
        evaluate(model, data.examples(rng, 6000, half=1), "held-out wording")
        return
    model = train(data.examples(rng, 200000, half=0), epochs=6)
    evaluate(model, data.examples(rng, 4000, half=0), "seen wording")
    held = data.examples(rng, 16000, half=1)
    evaluate(model, held, "held-out wording")
    # The 8 languages with several wordings have wording held out; the
    # others, kinds of mail (data.kinds_for).
    by_language(model, held, "held-out wording")
    # The published model learns from every wording.
    model = train(data.examples(rng, 300000), epochs=8)
    qmodel, q, scale = quantize(model)
    fresh = data.examples(rng, 12000)
    evaluate(model, fresh, "final, fresh messages")
    evaluate(qmodel, fresh, "final, 8-bit")
    by_language(qmodel, fresh, "final, 8-bit")
    path = HERE / "categories-v1.bin"
    write_bin(q, scale, qmodel, path)
    (HERE / "classes.json").write_text(json.dumps(CLASSES, indent=1) + "\n")
    (HERE / "parity.json").write_text(json.dumps(parity(qmodel), ensure_ascii=False, indent=1) + "\n")
    # Held-out wordings for the app's tests.
    ev = data.examples(random.Random(7), 4000, half=1)
    (HERE / "eval.json").write_text(json.dumps([
        {k: m[k] for k in ("name", "address", "subject", "preview", "unsub", "list_id", "list_post", "category", "lang")}
        for m in ev], ensure_ascii=False) + "\n")
    digest = hashlib.sha256(path.read_bytes()).hexdigest()
    print(f"categories-v1.bin: {path.stat().st_size} bytes, sha256 {digest}")


if __name__ == "__main__":
    main()

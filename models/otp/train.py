"""Trains the sign-in code model and writes what the app downloads.

    python3 train.py            # numpy only: otp-v1.bin and parity.json
    python3 train.py --coreml   # also otp-v1.mlmodel (needs coremltools)

The model scores one candidate number at a time from its features (see
features.py): a bag of hashed context features, averaged into a small hidden
layer, and a logistic output. `otp-v1.bin` is read by the app's own runtime;
`otp-v1.mlmodel` is the same network for Core ML on Apple platforms. Both
are published as release assets and checked against the SHA-256 digests in
`src/manage/local_ai/mod.rs`; after retraining, update those digests.
"""

import hashlib
import json
import random
import struct
import sys
from pathlib import Path

import numpy as np

import data
from features import DIM, MAX_TEXT, candidates, features, vector

HIDDEN = 16
THRESHOLD = 0.3
HERE = Path(__file__).parent


def examples(msgs):
    """Feature lists and labels for every candidate in `msgs`, and how many
    codes the candidate finder missed."""
    xs, ys, missed = [], [], 0
    for subject, body, code in msgs:
        text = (subject + "\n" + body)[:MAX_TEXT]
        found = False
        for s, e in candidates(text):
            label = 1 if code is not None and text[s:e] == code else 0
            found |= bool(label)
            xs.append(features(subject, text, s, e))
            ys.append(label)
        if code is not None and not found:
            missed += 1
    return xs, np.array(ys, dtype=np.float32), missed


def batch(xs):
    idx, val, seg = [], [], []
    for row, x in enumerate(xs):
        for i, v in x.items():
            idx.append(i)
            val.append(v)
            seg.append(row)
    return np.array(idx), np.array(val, dtype=np.float32), np.array(seg)


class Model:
    def __init__(self, rng):
        self.E = (rng.standard_normal((DIM, HIDDEN)) * 0.1).astype(np.float32)
        self.b1 = np.zeros(HIDDEN, np.float32)
        self.w2 = (rng.standard_normal(HIDDEN) * 0.1).astype(np.float32)
        self.b2 = np.float32(0.0)

    def forward(self, xs):
        idx, val, seg = batch(xs)
        pre = np.zeros((len(xs), HIDDEN), np.float32)
        np.add.at(pre, seg, self.E[idx] * val[:, None])
        pre += self.b1
        h = np.maximum(pre, 0)
        z = h @ self.w2 + self.b2
        return (idx, val, seg, pre, h), 1 / (1 + np.exp(-z))

    def params(self):
        return [self.E, self.b1, self.w2, np.array([self.b2])]


def train(feats, ys, epochs=8, lr=0.01, l2=1e-5, drop=0.3, seed=1):
    """Trains on feature lists; each epoch leaves out a random `drop` share
    of every candidate's features, so no one word decides."""
    rng = np.random.default_rng(seed)
    model = Model(rng)
    m = [np.zeros_like(p) for p in model.params()]
    v = [np.zeros_like(p) for p in model.params()]
    pos_weight = (len(ys) - ys.sum()) / max(ys.sum(), 1)
    xs = feats
    step = 0
    for epoch in range(epochs):
        xs = [vector([f for f in fs if rng.random() >= drop]) for fs in feats]
        order = rng.permutation(len(xs))
        total = 0.0
        for at in range(0, len(order), 256):
            sel = order[at:at + 256]
            bx = [xs[i] for i in sel]
            by = ys[sel]
            (idx, val, seg, pre, h), p = model.forward(bx)
            w = np.where(by == 1, pos_weight ** 0.5, 1.0).astype(np.float32)
            total += float(-(w * (by * np.log(p + 1e-7) + (1 - by) * np.log(1 - p + 1e-7))).sum())
            dz = w * (p - by) / len(sel)
            g_w2 = h.T @ dz + l2 * model.w2
            g_b2 = dz.sum()
            dh = np.outer(dz, model.w2) * (pre > 0)
            g_b1 = dh.sum(0)
            g_E = np.zeros_like(model.E)
            np.add.at(g_E, idx, dh[seg] * val[:, None])
            g_E += l2 * model.E
            step += 1
            grads = [g_E, g_b1, g_w2, np.array([g_b2], np.float32)]
            params = model.params()
            for k, (p_, g) in enumerate(zip(params, grads)):
                m[k] = 0.9 * m[k] + 0.1 * g
                v[k] = 0.999 * v[k] + 0.001 * g * g
                mh = m[k] / (1 - 0.9 ** step)
                vh = v[k] / (1 - 0.999 ** step)
                p_ -= (lr * mh / (np.sqrt(vh) + 1e-8)).astype(np.float32)
            model.b2 = np.float32(params[3][0])
        print(f"epoch {epoch + 1}: loss {total / len(xs):.4f}")
    return model


def evaluate(model, msgs, name):
    tp = fp = fn = 0
    msg_ok = 0
    for subject, body, code in msgs:
        text = (subject + "\n" + body)[:MAX_TEXT]
        cands = candidates(text)
        found = set()
        if cands:
            _, p = model.forward([vector(features(subject, text, s, e)) for s, e in cands])
            found = {text[s:e] for (s, e), q in zip(cands, p) if q >= THRESHOLD}
        want = {code} if code else set()
        tp += len(found & want)
        fp += len(found - want)
        fn += len(want - found)
        msg_ok += found == want
    precision = tp / max(tp + fp, 1)
    recall = tp / max(tp + fn, 1)
    print(f"{name}: precision {precision:.3f}, recall {recall:.3f}, "
          f"messages exactly right {msg_ok / len(msgs):.3f}")
    return precision, recall


def write_bin(model, path):
    with open(path, "wb") as f:
        f.write(b"PICAOTP1")
        f.write(struct.pack("<IIf", DIM, HIDDEN, THRESHOLD))
        f.write(model.E.astype("<f4").tobytes())
        f.write(model.b1.astype("<f4").tobytes())
        f.write(model.w2.astype("<f4").tobytes())
        f.write(struct.pack("<f", float(model.b2)))


def write_coreml(model, path):
    import coremltools as ct
    from coremltools.models import datatypes
    from coremltools.models.neural_network import NeuralNetworkBuilder

    builder = NeuralNetworkBuilder(
        [("features", datatypes.Array(DIM))],
        [("probability", datatypes.Array(1))],
    )
    builder.add_inner_product("hidden", W=model.E.T.copy(), b=model.b1, input_channels=DIM,
                              output_channels=HIDDEN, has_bias=True,
                              input_name="features", output_name="hidden_pre")
    builder.add_activation("relu", "RELU", "hidden_pre", "hidden")
    builder.add_inner_product("logit", W=model.w2.reshape(1, HIDDEN), b=np.array([model.b2]),
                              input_channels=HIDDEN, output_channels=1, has_bias=True,
                              input_name="hidden", output_name="logit")
    builder.add_activation("sigmoid", "SIGMOID", "logit", "probability")
    spec = builder.spec
    # The app passes and reads 32-bit floats.
    from coremltools.proto import FeatureTypes_pb2
    float32 = FeatureTypes_pb2.ArrayFeatureType.FLOAT32
    spec.description.input[0].type.multiArrayType.dataType = float32
    spec.description.output[0].type.multiArrayType.dataType = float32
    spec.description.metadata.shortDescription = (
        "Picacho: whether a number in an email is a sign-in or verification code.")
    spec.description.metadata.license = "MIT"
    ct.utils.save_spec(spec, str(path))


def parity(model):
    """Examples the app's feature code and runtime must reproduce."""
    cases = [
        ("Your Example verification code", "Hi Sam,\n\nYour verification code is 482913.\n\nIt expires in 10 minutes.\n\n© 2026 Example, 1600 Amphitheatre Parkway, Mountain View, CA 94043"),
        ("G-551203 is your Google verification code", "G-551203 is your Google verification code."),
        ("Your order #839201 has shipped", "Tracking number: 1Z999AA10123456784\nCall 1-800-555-0199 or visit us. Total: $45.10"),
        ("Código de verificación", "Tu código es 773 210. Caduca pronto. Pedido 99120 enviado."),
        ("Ihr Bestätigungscode", "Ihr Code lautet: AB12CD\nÜbermittelt am 2026-03-04 – Größe 9000"),
        ("", "Join Zoom meeting ID 812 3456 7890 Passcode: 123456"),
    ]
    out = []
    for subject, body in cases:
        text = (subject + "\n" + body)[:MAX_TEXT]
        rows = []
        for s, e in candidates(text):
            f = features(subject, text, s, e)
            x = vector(f)
            _, p = model.forward([x])
            rows.append({"start": s, "end": e, "code": text[s:e], "features": f,
                         "vector": sorted([i, round(v, 6)] for i, v in x.items()),
                         "score": round(float(p[0]), 6)})
        out.append({"subject": subject, "body": body, "candidates": rows})
    return out


def main():
    rng = random.Random(20261114)
    pos_train, pos_test = data.split(data.POSITIVE)
    neg_train, neg_test = data.split(data.NEGATIVE)
    train_msgs = data.messages(rng, pos_train, neg_train, 24000)
    test_msgs = data.messages(rng, pos_test, neg_test, 4000)
    xs, ys, missed = examples(train_msgs)
    print(f"{len(xs)} candidates, {int(ys.sum())} codes, {missed} codes not found as candidates")
    model = train(xs, ys)
    evaluate(model, train_msgs[:4000], "seen wording")
    evaluate(model, test_msgs, "held-out wording")
    # The published model learns from every template.
    all_msgs = data.messages(rng, data.POSITIVE, data.NEGATIVE, 30000)
    xs, ys, _ = examples(all_msgs)
    model = train(xs, ys)
    evaluate(model, data.messages(rng, data.POSITIVE, data.NEGATIVE, 3000), "final, fresh messages")
    write_bin(model, HERE / "otp-v1.bin")
    (HERE / "parity.json").write_text(json.dumps(parity(model), ensure_ascii=False, indent=1))
    if "--coreml" in sys.argv:
        write_coreml(model, HERE / "otp-v1.mlmodel")
    for name in ["otp-v1.bin", "otp-v1.mlmodel"]:
        path = HERE / name
        if path.exists():
            digest = hashlib.sha256(path.read_bytes()).hexdigest()
            print(f"{name}: {path.stat().st_size} bytes, sha256 {digest}")


if __name__ == "__main__":
    main()

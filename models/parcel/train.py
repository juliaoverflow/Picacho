"""Trains Vast Parcel, the tracking number model, and writes what the app
downloads.

    python3 train.py            # numpy only: parcel-v1.bin and parity.json
    python3 train.py --coreml   # also parcel-v1.mlmodel (needs coremltools)

The app finds numbers that have a carrier's format and check digit
(`src/manage/parcels.rs`); this model decides from the words around each
whether the message gives it as a tracking number. It reads the sign-in code
model's features and the parcel concepts around the number (`features.py`)
into the same kind of network: a bag of hashed
features, one small hidden layer and a logistic output. `parcel-v1.bin`
(the `PICAOTP1` format) is read by the app's own runtime; `parcel-v1.mlmodel`
is the same network for Core ML on Apple platforms. After retraining, update
the sizes and SHA-256 digests of `PARCEL_NATIVE` and `PARCEL_COREML` in
`src/manage/local_ai/mod.rs`.
"""

import hashlib
import json
import random
import struct
import sys
from pathlib import Path

import numpy as np

import data
import features as F

HERE = Path(__file__).parent


DIM = F.DIM
HIDDEN = 8
THRESHOLD = 0.5
FILE = "parcel-v1.bin"
COREML_FILE = "parcel-v1.mlmodel"


def examples(msgs):
    """Feature lists and labels for every number in `msgs`."""
    xs, ys = [], []
    for subject, body, spans in msgs:
        text = (subject + "\n" + body)[:F.MAX_TEXT]
        for s, e, y in spans:
            xs.append(F.features(subject, text, s, e))
            ys.append(y)
    return xs, np.array(ys, dtype=np.float32)


def batch(xs):
    idx, val, seg = [], [], []
    for row, x in enumerate(xs):
        for i, v in x.items():
            idx.append(i)
            val.append(v)
            seg.append(row)
    return np.array(idx, dtype=np.int64), np.array(val, dtype=np.float32), np.array(seg, dtype=np.int64)


class Model:
    """The sign-in code model's network (models/otp/train.py), smaller."""

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


def train(feats, ys, epochs=8, lr=0.01, l2=1e-5, drop=0.1, seed=1):
    """Adam on the weighted log loss; each epoch leaves out a random `drop`
    share of every number's features, so no one word decides."""
    rng = np.random.default_rng(seed)
    model = Model(rng)
    m = [np.zeros_like(p) for p in model.params()]
    v = [np.zeros_like(p) for p in model.params()]
    pos_weight = (len(ys) - ys.sum()) / max(ys.sum(), 1)
    step = 0
    for epoch in range(epochs):
        xs = [F.vector([f for f in fs if rng.random() >= drop]) for fs in feats]
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
    xs, ys = examples(msgs)
    _, p = model.forward([F.vector(f) for f in xs])
    found = p >= THRESHOLD
    tp = int((found & (ys == 1)).sum())
    fp = int((found & (ys == 0)).sum())
    fn = int((~found & (ys == 1)).sum())
    precision = tp / max(tp + fp, 1)
    recall = tp / max(tp + fn, 1)
    print(f"{name}: precision {precision:.3f}, recall {recall:.3f}")
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
    """The same network for Core ML, as models/otp/train.py writes it."""
    import coremltools as ct
    from coremltools.models import datatypes
    from coremltools.models.neural_network import NeuralNetworkBuilder
    from coremltools.proto import FeatureTypes_pb2

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
    float32 = FeatureTypes_pb2.ArrayFeatureType.FLOAT32
    spec.description.input[0].type.multiArrayType.dataType = float32
    spec.description.output[0].type.multiArrayType.dataType = float32
    spec.description.metadata.shortDescription = (
        "Picacho: whether a number in an email is the tracking number of a parcel.")
    spec.description.metadata.license = "MIT"
    ct.utils.save_spec(spec, str(path))


def parity(model):
    """Examples the app's candidate finder, feature code and runtime must
    reproduce: each listed number is found by the app as a candidate at the
    same place, with the same features, vector and score."""
    cases = [
        ("Your order has shipped", "Hi Sam,\n\nYour order is on its way.\nTracking number: 1Z5R89390357567127\nOrder number: 986578788855\n\n© 2026 Example, 500 Main St, Springfield, IL 62701", ["1Z5R89390357567127", "986578788855"]),
        ("Invoice 3318810025", "Thanks for your payment. Invoice number 3318810025.", ["3318810025"]),
        ("Shipped", "USPS tracking: 9400 1112 0620 6406 2607 87", ["9400 1112 0620 6406 2607 87"]),
        ("Ihre Bestellung wurde versandt", "Sendungsnummer: RB123456785GB\nBestellnummer 477179081230", ["RB123456785GB", "477179081230"]),
        ("ご注文の商品を発送しました", "お問い合わせ伝票番号：TBA000000000000\n注文番号：0073938000549297", ["TBA000000000000", "0073938000549297"]),
        ("您的订单已发货", "运单号：GM2951173225174494，请注意查收。", ["GM2951173225174494"]),
        ("Ваш заказ отправлен", "Трек-номер: LX17635036", ["LX17635036"]),
    ]
    out = []
    for subject, body, numbers in cases:
        text = (subject + "\n" + body)[:F.MAX_TEXT]
        rows = []
        for number in numbers:
            s = text.index(number)
            e = s + len(number)
            f = F.features(subject, text, s, e)
            x = F.vector(f)
            _, p = model.forward([x])
            rows.append({"start": s, "end": e, "number": number, "features": f,
                         "vector": sorted([i, round(v, 6)] for i, v in x.items()),
                         "score": round(float(p[0]), 6)})
        out.append({"subject": subject, "body": body, "candidates": rows})
    return out


def main():
    rng = random.Random(20261120)
    # Labels, sentences and subjects held out of training.
    train_msgs = data.messages(rng, data.parts(held_out=False), 20000)
    held_out = data.messages(rng, data.parts(held_out=True), 3000)
    xs, ys = examples(train_msgs)
    print(f"{len(xs)} numbers, {int(ys.sum())} tracking numbers")
    model = train(xs, ys)
    evaluate(model, train_msgs[:3000], "seen wording")
    evaluate(model, held_out, "held-out wording")
    # The published model learns from every template.
    all_msgs = data.messages(rng, data.parts(), 25000)
    xs, ys = examples(all_msgs)
    model = train(xs, ys)
    evaluate(model, data.messages(rng, data.parts(), 3000), "final, fresh messages")
    write_bin(model, HERE / FILE)
    (HERE / "parity.json").write_text(json.dumps(parity(model), ensure_ascii=False, indent=1))
    if "--coreml" in sys.argv:
        write_coreml(model, HERE / COREML_FILE)
    for name in [FILE, COREML_FILE]:
        path = HERE / name
        if path.exists():
            digest = hashlib.sha256(path.read_bytes()).hexdigest()
            print(f"{name}: {path.stat().st_size} bytes, sha256 {digest}")


if __name__ == "__main__":
    main()

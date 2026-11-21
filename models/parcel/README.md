# Vast Parcel: the tracking number model

Confirms tracking numbers found in mail (`src/manage/parcels.rs`). The app
finds candidates without a model: runs of uppercase letters and digits
(possibly in groups split by spaces) that have a carrier's format and check
digit, checked by the [`tracking-numbers`](https://crates.io/crates/tracking-numbers)
crate against the [tracking_number_data](https://github.com/jkeen/tracking_number_data)
catalog. About one random 10- or 12-digit number in ten passes some carrier's
check, so order, invoice and phone numbers would pass too. This model decides
from the words around each candidate whether the message gives it as a
tracking number.

It is installed when **Settings → Local AI → Find tracking numbers** is
turned on, and deleted when it is turned off. Like the other models, it
isn't kept in git:

| File | Used on | Runtime |
| --- | --- | --- |
| `parcel-v1.mlmodel` | macOS | Core ML (compiled for the Mac when installed) |
| `parcel-v1.bin` | Windows, Linux, phones | Picacho's own (`src/manage/local_ai/runtime.rs`) |

Both hold the same network, and each is checked against the size and
SHA-256 of `PARCEL_COREML` / `PARCEL_NATIVE` in `src/manage/local_ai/mod.rs`.

It will be published on Hugging Face as
[pinaltech/vast-parcel](https://huggingface.co/pinaltech/vast-parcel).
**Until it is**, the model is marked `local: true` there and
installed from this directory (where `train.py` writes it) instead of being
downloaded: the path is compiled in from the source tree Picacho was built
from, or `PICACHO_LOCAL_MODELS` names another directory holding the files.
`PICACHO_MODELS_URL` still downloads it from a mirror. A build used on
another computer can't install it until it is published.

Without the model, the app accepts a candidate only with a shipping word
shortly before it on its line (or in the subject), and never a bare number
shorter than 12 digits. Tracking numbers in schema.org `ParcelDelivery`
markup are taken as the sender gives them, with or without the model.

The reader shows the tracking numbers of the open message, and each
account's **Summary** (under its Inbox) lists every parcel its mail tells
of, last updated first: the newest update date a message gives (a delivery
event's time in the markup, or an "Updated:" date in the text), or else when
the newest message was sent.

## How it works

1. **Features.** The sign-in code model's features (`../otp/features.py`:
   the words around the number, their prefixes, its shape and line, the
   subject, and the OTP lexicon's concepts), plus the concepts in
   `lexicon.json` named by the words before and after the number, close
   before it on its line, and in the subject: *track*, *parcel*, *deliver*,
   *ship*, and *order*, *invoice*, *customer*, *phone*, *reference*, with
   stems in many languages. Everything is hashed into 16,384 slots.
2. **Model.** A bag-of-features network with one hidden layer of 8 units and
   a logistic output (512 KB). A candidate scoring 0.5 or more is a tracking
   number.

`features.py` and `lexicon.json` are the reference; the app builds the same
features in `src/manage/local_ai/parcel.rs`, with the lexicon compiled in, so
a lexicon change needs an app update as well as a new model. `parity.json`
is checked by `cargo test matches_the_training_code`: each of its numbers is
found by the app as a candidate at the same place, with the same features,
vector and score.

## Training

```sh
python3 train.py            # numpy: parcel-v1.bin, parity.json
python3 train.py --coreml   # also parcel-v1.mlmodel (needs coremltools)
```

`--coreml` needs [coremltools](https://pypi.org/project/coremltools/),
which has builds only for Python 3.13 and older: on a newer Python it
installs without its compiled parts and can't write the file, so
`train.py` stops before training. Use a Python 3.13 environment, on any
system (Linux works too: only *running* the model needs a Mac):

```sh
uv venv --python 3.13 .venv && uv pip install --python .venv numpy coremltools
.venv/bin/python train.py --coreml
```

Lines saying `Failed to load ... libcoremlpython` are expected outside
macOS: those parts run models, and writing one doesn't need them.

The weights depend slightly on the numpy version, so after training, update
both sizes and digests in `src/manage/local_ai/mod.rs` with what `train.py`
prints (`parity.json` is rewritten to match).

The training mail is synthetic (`data.py`). Every number in it is one of
the catalog's valid test numbers (`numbers.json`: those the
`tracking-numbers` crate identifies, with the carrier it names), given as a
tracking number in shipping mail, or as an order, invoice, account, phone,
booking or other number in other mail and beside tracking numbers. Labels,
sentences and subjects are held out a quarter at a time:

| Wording | Precision | Recall |
| --- | --- | --- |
| Held out of training | 99.9% | 93% |
| Seen in training | 100% | 100% |

Real mail will have phrasings the templates don't cover; more varied
examples improve it.

## Publishing a new version

1. Retrain with `--coreml`, then update the sizes and digests `train.py`
   prints in `PARCEL_NATIVE` and `PARCEL_COREML`
   (`src/manage/local_ai/mod.rs`). Bump the version in the file names when
   the features or lexicon change.
2. Upload both files:
   `hf upload pinaltech/vast-parcel models/parcel/parcel-v1.bin parcel-v1.bin`,
   and the same for `parcel-v1.mlmodel`. Then set `local: false` on both
   models, so the app downloads them. Upload before releasing that app.
3. `cargo test the_published_models_are_the_ones_the_app_expects` checks the
   uploaded file (delete `target/test-models` first).

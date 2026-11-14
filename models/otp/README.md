# Sign-in code model

The model behind **Settings → Local AI → Hide sign-in codes**. It decides
whether a number in an email is the message's sign-in, verification or
one-time code, so Picacho can show it as `●●●●●●` in the subject, the message
and notifications.

Nothing here ships inside the app. When the feature is turned on, Picacho
downloads one file from the `models-otp-v1` release of this repository,
checks its size and SHA-256 against `src/manage/local_ai/mod.rs`, and keeps
it in the data directory under `models/`. Turning the feature (or Local AI)
off deletes it.

| File | Used on | Runtime |
| --- | --- | --- |
| `otp-v1.mlmodel` | macOS | Core ML (compiled for the Mac when installed) |
| `otp-v1.bin` | Windows, Linux | Picacho's own (`src/manage/local_ai/runtime.rs`) |

Both hold the same network.

## How it works

1. **Candidates.** A pattern finds numbers that could be codes:
   4–10 digits, uppercase letters with digits (`AB12CD`), two groups of 3–4
   digits (`482 913`, `482-913`), optionally with a short prefix (`G-482913`).
   Parts of phone numbers, dates and amounts are skipped.
2. **Features.** For each candidate: the words before and after it (and their
   first four letters, which carry across languages: *verification*,
   *verificación*, *verificatie*), the nearest words and word pairs, the
   characters next to it, its shape (`999999`, `A-999999`), whether it is on a
   line of its own or in the subject, and the subject's words. These are
   hashed into 16,384 slots.
3. **Model.** A bag-of-features network (fastText style): one hidden layer of
   16 units and a logistic output, about 260,000 weights (1 MB). A candidate
   scoring 0.3 or more is a code.

`features.py` is the reference for the features; the app builds them the same
way in `src/manage/local_ai/otp.rs`, and `parity.json` (candidates, features,
vectors and scores for a few messages) is checked by
`cargo test matches_the_training_code`.

## Training

```sh
python3 train.py            # numpy: otp-v1.bin, parity.json
python3 train.py --coreml   # also otp-v1.mlmodel (pip install coremltools)
```

The training mail is synthetic (`data.py`): sign-in messages in English,
Spanish, German, French, Portuguese, Italian, Dutch, Swedish and Polish, and
mail full of numbers that aren't codes (orders, invoices, tracking and booking
references, phone numbers, addresses, account endings, coupons, meeting IDs).
Training is deterministic.

On messages built from templates held out of training, the model finds
**90% of codes with 98% precision** (94% of messages exactly right). That is
the honest figure for wording it hasn't seen; on familiar wording it is
right every time. Real mail will have phrasings the templates don't cover,
so this is a first version to improve with more varied examples.

## Publishing a new version

1. Retrain, then update the sizes and digests `train.py` prints in
   `src/manage/local_ai/mod.rs` (`cargo test the_published_models_are_the_trained_ones`
   fails until they match).
2. Upload `otp-v1.bin` and `otp-v1.mlmodel` as assets of a release tagged
   `models-otp-v1` (or bump the version, the file names and `MODELS_URL`).

`PICACHO_MODELS_URL` makes the app download from somewhere else (an HTTPS
mirror) while developing.

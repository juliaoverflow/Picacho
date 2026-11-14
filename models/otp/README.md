# Sign-in code model

The model behind **Settings → Local AI → Hide sign-in codes**. It decides
whether a number in an email is the message's sign-in, verification or
one-time code, so Picacho can show it as `••••••` in the subject, the message
and notifications.

Nothing here ships inside the app. When the feature is turned on, Picacho
downloads one file from this directory at a pinned commit of the repository
(`MODELS_URL` in `src/manage/local_ai/mod.rs`), checks its size and SHA-256,
and keeps it in the data directory under `models/`. Turning the feature (or
Local AI) off deletes it, and a model left from an earlier version is deleted
when the new one is installed.

| File | Used on | Runtime |
| --- | --- | --- |
| `otp-v1.mlmodel` | macOS | Core ML (compiled for the Mac when installed) |
| `otp-v1.bin` | Windows, Linux | Picacho's own (`src/manage/local_ai/runtime.rs`) |

Both hold the same network.

## Languages

Version 2 is trained on sign-in mail in English, Spanish, German, French,
Portuguese, Italian, Dutch, Swedish, Polish, Japanese, Chinese (simplified
and traditional), Korean, Russian, Ukrainian, Turkish, Arabic, Persian,
Hebrew, Hindi, Bengali, Indonesian, Malay, Vietnamese, Thai, Greek, Czech,
Slovak, Danish, Norwegian, Finnish, Romanian, Hungarian, Catalan, Croatian,
Bulgarian, Tagalog and Swahili, with codes in ASCII, fullwidth, Arabic-Indic,
Persian, Devanagari, Bengali and Thai digits. Copy Code gives ASCII digits.

## How it works

1. **Candidates.** A pattern finds numbers that could be codes: two groups of
   3–4 digits (`482 913`, `482-913`), 4–10 uppercase letters and digits
   (`AB12CD`, `482913`), or 4–10 digits in any script (`４８２９１３`,
   `٤٨٢٩١٣`), optionally with a short prefix (`G-482913`). A candidate may
   touch Chinese or Japanese text but not a Latin letter or another digit;
   parts of phone numbers, dates and amounts are skipped.
2. **Words.** Runs of letters, marks and digits. Scripts written without
   spaces (Chinese, Japanese kana, Thai, Lao, Khmer, Myanmar) are split into
   overlapping pairs of characters (`验证码是` → `验证`, `证码`, `码是`).
3. **Features.** For each candidate: the words before and after it and their
   first four letters, the nearest words and word pairs, the characters next
   to it, its shape (`999999`, `A-999999`), whether it is on a line of its
   own or in the subject, the subject's words, and the **concepts** the
   nearby words and the subject name. Concepts (`lexicon.json`: code,
   verify, sign in, one-time, expiry, don't share, order, money, shipping,
   contact, promotion, reference) list word stems in every language above,
   so "код", "コード" and "kód" all count as *code*. Everything is hashed
   into 16,384 slots.
4. **Model.** A bag-of-features network (fastText style): one hidden layer of
   16 units and a logistic output, about 260,000 weights (1 MB). A candidate
   scoring 0.3 or more is a code.

`features.py` and `lexicon.json` are the reference; the app builds the same
features in `src/manage/local_ai/otp.rs`, with the lexicon compiled in, so a
lexicon change needs an app update as well as a new model. `parity.json`
(candidates, features, vectors and scores for messages in several languages
and scripts) is checked by `cargo test matches_the_training_code`.

## Training

```sh
python3 train.py            # numpy: otp-v1.bin, parity.json
python3 train.py --coreml   # also otp-v1.mlmodel (pip install coremltools)
```

The training mail is synthetic (`data.py`): sign-in messages in the languages
above, mail full of numbers that aren't codes (orders, invoices, tracking and
booking references, phone numbers, addresses, account endings, coupons,
meeting IDs) in many of them, and sign-in messages with another number beside
the code. Training is deterministic.

On messages built from templates held out of training:

| Held-out wording | Precision | Recall | Messages exactly right |
| --- | --- | --- | --- |
| English (and other Latin-script) templates | 96% | 95% | 95% |
| Templates in the newer languages | 99% | 90% | 94% |

Those are the honest figures for wording the model hasn't seen; on familiar
wording it is right nearly every time. The lexicon was written knowing every
template, so for an unseen language the second row is optimistic. Real mail
will have phrasings the templates don't cover; more varied examples improve
it.

## Publishing a new version

1. Retrain, then update the sizes and digests `train.py` prints in
   `src/manage/local_ai/mod.rs` (`cargo test the_published_models_are_the_trained_ones`
   fails until they match). Bump the version in the file names when the
   features or lexicon change.
2. Commit and push the model files, then point `MODELS_URL` at that commit:
   `https://raw.githubusercontent.com/juliaoverflow/Picacho/<commit>/models/otp`.
   A commit, not a branch, so the file at the address never changes under the
   pinned digest.

`PICACHO_MODELS_URL` makes the app download from somewhere else (an HTTPS
mirror) while developing.

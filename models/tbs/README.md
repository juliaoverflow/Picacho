---
license: mit
library_name: picacho
tags:
- email
- date-parsing
- natural-language-time
- rrule
- multilingual
---

# Epoch TBS: the date and time model

The model behind **Settings → Local AI → Understand typed dates**. Where
Picacho asks when (Snooze, Send Later), a field takes a time typed in words,
typos and all, in 37 languages, and sets the calendar and clock to it:

| Typed | Read as (from Sunday, Nov 15, 14:20) |
| --- | --- |
| `tmrw at 6`, `remind me to call mom tomorow at 6 pls` | Mon, Nov 16, 18:00 |
| `next fri 3pm`, `Wensday mornin` | Fri, Nov 20, 15:00 · Wed, Nov 18, 8:00 |
| `the monday in three weeks` | Mon, Nov 30, 8:00 |
| `start of next week at 7am` | Mon, Nov 16, 7:00 |
| `in 4 hours at the top of the hour` | 19:00 |
| `overmorrow at noon`, `in a fortnight` | Tue, Nov 17, 12:00 · Sun, Nov 29, 14:20 |
| `friday after next`, `a week from friday`, `the first monday of next month` | Nov 27 · Nov 27 · Dec 7 |
| `this time tomorrow`, `end of next month`, `mid next month` | Nov 16, 14:20 · Dec 31, 17:00 · Dec 15 |
| `every other monday @ 9:30am` | Mon, Nov 16, 9:30, and `FREQ=WEEKLY;INTERVAL=2;BYDAY=MO;BYHOUR=9;BYMINUTE=30` |
| `Erinnere mich übermorgen um 18:30 Uhr` | Tue, Nov 17, 18:30 |
| `rappelle-moi vendredi prochain à 18h30` | Fri, Nov 20, 18:30 |
| `recuérdame pasado mañana a las 6 de la tarde` | Tue, Nov 17, 18:00 |
| `明天下午6点半提醒我`, `来週の金曜日の午後3時`, `내일 오후 6시 30분` | Nov 16, 18:30 · Nov 20, 15:00 · Nov 16, 18:30 |
| `कल शाम ६ बजे`, `غدا الساعة ٦ مساء`, `พรุ่งนี้ 6 โมงเย็น` | Mon, Nov 16, 18:00 |
| `через 2 часа`, `2 saat sonra` | 16:20 |
| `may i ask a question`, `I sat down with the team on the 2nd floor` | nothing |

Nothing ships inside the app. Turning the feature on downloads one file from
[pinaltech/epoch-tbs](https://huggingface.co/pinaltech/epoch-tbs) (in the
[Picacho Models](https://huggingface.co/collections/pinaltech/picacho-models-6af95b1ea617ddc3325ee964)
collection), checks its size and SHA-256 against the values built into the
app (`WHEN` in `src/manage/local_ai/mod.rs`), and keeps it in the data
directory under `models/`. Turning the feature (or Local AI) off, or
deleting the model in Settings, deletes it. The model file isn't kept in
git.

| File | Used on | Runtime |
| --- | --- | --- |
| `tbs-v1.bin` | every platform | Picacho's own (`NativeTagger` in `src/manage/local_ai/runtime.rs`) |

It runs on every keystroke on any CPU: a few thousand multiply-adds per
token, less than one Core ML call would cost, so there is no Core ML
version.

## Languages

English, Spanish, German, French, Portuguese, Italian, Dutch, Swedish,
Polish, Japanese, Chinese (simplified and traditional), Korean, Russian,
Ukrainian, Turkish, Arabic, Persian, Hebrew, Hindi (and Hindi in Latin
letters), Bengali, Indonesian, Malay, Vietnamese, Thai, Greek, Czech,
Slovak, Danish, Norwegian, Finnish, Romanian, Hungarian, Catalan, Croatian,
Bulgarian, Tagalog and Swahili, the same as
[Veil OTP](https://huggingface.co/pinaltech/veil-otp), with times in ASCII,
fullwidth, Arabic-Indic, Persian, Devanagari, Bengali and Thai digits.

## How it works

Two steps, so the model does the reading and never the arithmetic.

1. **The model labels tokens.** Text is split into lowercase runs of
   letters (with the vowel signs written on them), runs of digits in any of
   the scripts above (read as ASCII digits), single characters of scripts
   written without spaces (Chinese, Japanese, Thai), and `: / - . , @ +`
   (`6pm` is `6`, `pm`; `明天6点` is `明`, `天`, `6`, `点`). For each token
   the model says what it means (`features.LABELS`, 104 labels): `TOMORROW`,
   `OVERMORROW`, `FRI`, `NOV`, `U_HOUR`, `U_FORTNIGHT`, `PM`, `NUM`, `NEXT`,
   `START`, `TOP`, `MONTH_MARK` (the 月 of 11月), `W20` (twenty, twentieth)...
   or `O`, no part of a time. Features: the token, its first and last three
   letters, its letter trigrams, its *skeleton* (first letter and consonants
   without repeats, so `tomorrow`, `tommorow` and `tmrw` are all `tmrw`),
   digit count and value bucket, whether it touches its neighbours (`2nite`,
   `6pm`, `18h30`), and the three tokens on each side. Hashed into 16,384
   slots → 32 hidden units (ReLU) → softmax. The embedding is stored as
   8-bit integers with a scale per row.
2. **Rules work out the moment** (`src/manage/local_ai/when.rs`) from the
   labels and the current local time. They read meanings, not words, so one
   set serves every language and word order ("vendredi prochain" and "next
   friday", "3 giorni fa" and "il y a 3 jours", "下午6点" and "6pm"):
   - A day without a time is at 8:00, as Picacho's own quick choices;
     morning 8:00, afternoon 14:00, evening 19:00, night and tonight 20:00,
     the end of a day, week, month or year 17:00.
   - A bare hour on a named day is in the afternoon up to 6 and the morning
     from 7 (`tomorrow at 6` is 18:00, `friday at 9` is 9:00); a time alone
     is its next time (`at 3` at 14:20 is 15:00, `at 2` is 2:00 tomorrow).
   - `friday` and `this friday` are the coming Friday (today, while the
     time is still to come); `next friday` is the first Friday after today;
     `friday next week`, `下周五` and `the monday in three weeks` are that
     day in that week; `friday after next` is a week after next Friday.
   - `next week` is next Monday, `next month` the 1st; `start`, `middle`
     and `end` of a week are Monday, Wednesday and Friday, of a month the
     1st, the 15th and the last day; said of this one, they are the next
     such day.
   - A date without a year is this year, or next year once it has passed.
     A date in numbers follows the computer's region (`3/4` is March 4 in
     the US, 3 April in the UK), unless a number can't be a month; dates
     with dots (`21.11.`) put the day first.
   - `in 3 days`, `2 weeks from now`, `3 days ago`, `in a fortnight` count
     from now; `in 3 days at 5pm` takes the day and sets the time; `a week
     from friday` and `2 weeks from tomorrow` count from that day; `at the
     top of the hour` moves to the next full hour; `this time tomorrow`
     keeps the time it is now.
   - Repeating times (`every`, `daily`, `weekly`, `every other`, `every 2
     weeks`, `every fortnight`, `weekdays`, `毎週月曜日`) give an RRULE
     (RFC 5545) and their first time. Snooze and Send Later happen once, so
     they use the first time.
   - Text with nothing that anchors a time (`at home`, `6 apples`, `the
     second one`) gives nothing.

`features.py` is the reference for the tokens and features; the app builds
the same, and `parity.json` (tokens, features, vectors, labels and
probabilities for texts in many languages and scripts) is checked by `cargo
test matches_the_training_code`.

## Training data

`data.py` and `languages.py` write the text; nothing in it is real. Every
text is different (repeats are dropped), and about three in four have a
time in them:

- **English** (about two in five texts), rendered from building blocks in
  many orders, often inside other words (`remind me to pay rent ... pls`,
  `pay the 6 invoices ...`): relative days (`overmorrow`), weekdays,
  month-day and numeric dates, clock times in digits and words, `half
  past`/`quarter to`/`quarter after`, parts of the day, lengths of time
  (`fortnight`, `1h30`, `an hour and a half`), weeks from now (`the monday
  in three weeks`, `friday next week`, `the week after next`), the start,
  middle and end of periods, the nth weekday of a month, days counted from
  a day (`a week from friday`), `this time tomorrow`, `first thing monday`,
  `at the top of the hour`, repeating times, `now`/`later`.
- **36 other languages**, each with its own words for every meaning
  (including inflected forms, abbreviations and the spellings people type
  without accents) and its own word order: "next" before or after
  (`nächsten Freitag`, `vendredi prochain`, `jumat depan`), "in" and "ago"
  as prefix or postfix (`dans 2 heures`, `2 saat sonra`, `il y a 3 jours`,
  `3日前`), clock styles (`18h30`, `a las 6 y media`, `6点半`, `साढ़े 6 बजे`,
  `6 โมงเย็น`), dates (`21. November`, `21 de noviembre`, `11月21日`,
  `11월 21일`, `ngày 21 tháng 11`), and Swahili's unit before the count.
- **Texts without a time**, many with words that look like part of one:
  `may i`, `sat down`, `the sun is out`, `the second draft`, `room 204`,
  `6 apples`, `3-2 score`, `9 to 5 job`, `1st place`, `good morning`,
  `guten Morgen`, `buenas noches`, `หน้า 21` (page 21; หน้า is also "next").
- **Untidy typing**: common misspellings and abbreviations of every word,
  random slips (a letter dropped, doubled, swapped or a neighbouring key),
  accents left off, all lowercase or all capitals, missing spaces (`6pm`,
  `@6`, `1h30`), and broken grammar (`in 2 hour`, `6 tomorow`, `fri 3pm`).

## Training

```sh
python3 train.py      # numpy only: tbs-v1.bin, parity.json, eval.json
```

It takes about 25 minutes on a laptop CPU (`--final-only` trains just the published model) and is deterministic. The
spellings, the surrounding wording and the texts without a time are split
in two: a model trained on one half is measured on the other, then the
published model learns from everything.

| Measured on | Time tokens right | Texts exactly right | Texts without a time left alone |
| --- | --- | --- | --- |
| Wording and spellings seen in training | 99.2% | 97.4% | 98.6% |
| Held-out wording and spellings | 94.1% | 67.5% | 53.5% |
| Published model, fresh texts (8-bit) | 99.2% | 97.3% | 98.0% |

The held-out row is the honest figure for spellings and sentences the model
has never seen (measured before Thai, Vietnamese, Indonesian, Swedish and
Ukrainian were given more of the data); many of its misses are labels the
rules ignore anyway. What matters is the moment: on `eval.json` (4,000 fresh
texts from the held-out wording, in every language) the published model's
labels give the same moment and rule as the true labels for **98.6%** of
texts (98.4% of those with a time). By language:

| | | | | | | | |
| --- | --- | --- | --- | --- | --- | --- | --- |
| en 98.5% | de 99.3% | es 98.5% | fr 98.4% | pt 99.1% | it 100% | nl 100% | sv 100% |
| da 100% | no 100% | pl 100% | ru 100% | uk 100% | cs 96.3% | sk 97.0% | hr 95.8% |
| bg 97.4% | ro 100% | hu 96.3% | fi 90.0% | el 96.8% | tr 100% | ar 100% | fa 100% |
| he 100% | hi 100% | hi (Latin) 98.4% | bn 100% | id 97.6% | ms 100% | vi 92.2% | th 96.6% |
| ja 100% | zh 100% | zh-Hant 100% | ko 100% | tl 96.6% | sw 100% | ca 100% | |

Smaller languages have 20–60 texts each, so their figures move a few points
between runs; the wording is synthetic, and real typing will have
phrasings the templates don't cover. Storing the embedding in 8 bits costs
nothing measurable. The file is 604 KB: 16,384 rows of 32 weights.

## Publishing a new version

1. Retrain, then update the size and digest `train.py` prints in `WHEN` in
   `src/manage/local_ai/mod.rs`. Bump the version in the file name when the
   features or labels change.
2. Upload it before releasing the app that names it:
   `hf upload pinaltech/epoch-tbs models/tbs/tbs-v1.bin tbs-v1.bin`, and
   this README as the model card:
   `hf upload pinaltech/epoch-tbs models/tbs/README.md README.md`. Then delete
   `models/tbs/tbs-v1.bin`: the model is kept on Hugging Face, not in git.
3. `cargo test the_published_models_are_the_ones_the_app_expects` checks
   the uploaded file (delete `target/test-models` first).

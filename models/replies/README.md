---
license: mit
library_name: picacho
tags:
- email
- smart-reply
- multilingual
---

# Echo Replies: the reply suggestion model

The model behind **Settings → Local AI → Suggest replies**. It reads a
received message and offers a few short replies under it, worded in the
message's language: a meeting request gets "That works for me." and "Let me
check my calendar.", a German one "Das passt mir." and "Ich schaue in meinen
Kalender.", news of a new job "Congratulations!". Choosing one starts a reply
with it written in, ready to send or to go on writing.

It doesn't write text. It picks from the replies in `catalog.json`: 59 of
them, each worded by hand in 37 languages (38 codes: Chinese in simplified
and traditional characters). So every suggestion is a sentence a person
wrote, and the model stays small and fast enough to run whenever a message
opens.

| File | Used on | Runtime |
| --- | --- | --- |
| `replies-v1.bin` | every platform | Picacho's own (`NativeTagger` in `src/manage/local_ai/runtime.rs`) |

Like the other models, it isn't kept in git: it is published on Hugging Face
as [pinaltech/echo-replies](https://huggingface.co/pinaltech/echo-replies),
and Picacho downloads it from there when the feature is turned on, checking
its size and SHA-256 (`REPLIES` in `src/manage/local_ai/mod.rs`). A model
trained again here with `train.py` needs a new file name, size and digest
there, and publishing.

## Languages

English, Spanish, German, French, Portuguese, Italian, Dutch, Swedish,
Danish, Norwegian, Polish, Russian, Ukrainian, Czech, Slovak, Croatian,
Bulgarian, Romanian, Hungarian, Finnish, Greek, Turkish, Arabic, Persian,
Hebrew, Hindi, Bengali, Indonesian, Malay, Vietnamese, Thai, Japanese,
Chinese (simplified and traditional), Korean, Tagalog, Swahili and Catalan:
the date model's languages.

The wordings were written for this catalog and haven't been reviewed by
native speakers yet. Where a language marks the speaker's gender in the
verb (Hebrew, Hindi, Polish, Russian…), most wordings are phrased to avoid
it; a few in Hindi ("मैं इसे देखता हूँ।", "मैं जाँच करके बताता हूँ।") are
masculine. Review before relying on them.

## How it works

1. **Cleaning.** The body up to the sender's signature (`--`) or a
   forwarded or quoted original (`-----`, `_____`), without quoted lines
   (`>`), at most 4,000 characters.
2. **Words.** The sign-in code model's (`../otp/features.py`): lowercase
   runs of letters, marks and digits, with scripts written without spaces
   split into pairs of characters.
3. **Features.** The subject's words, the body's words and word pairs (the
   first 160), the words of up to four lines that ask something (`?`, `？`,
   `؟`), the **concepts** (`lexicon.json`) the body, those lines and the
   subject name, how many questions there are, whether the message exclaims,
   and its length. Concepts (invite, meet, call, thank, sorry, sick, late,
   cancel, deadline, birthday, holiday, introduce, interview, invoice,
   opinion, how-are-you, follow-up, news, loss, offer of help, automated…)
   list word stems in every language above, so "Einladung", "invitación" and
   "招待" all name *invite*. Everything is hashed into 32,768 slots.
4. **Network.** The features, summed into 32 hidden units, then a softmax
   over the 59 replies and "none" (mail that wants no reply). The embedding
   is stored as 8-bit integers with a scale per row (the date model's
   `PICAWHN1` format): about 1.2 MB.
5. **Suggestions** (`src/manage/local_ai/replies.rs`). Nothing when "none"
   wins; else the likeliest replies (at most four, each at least 3%),
   worded in the message's language. The language is told from the script
   of its letters and, among languages sharing one, their common words
   (`stopwords.json`) and particular letters (ñ, ø, ő, ș…); when it can't be
   told, the computer's language is used, or English. What the person
   chooses makes it likelier (up to three times), and a reply turned away
   ("Don't Suggest This") isn't offered again; this history stays on the
   device and can be forgotten in Settings.

The app also offers nothing for mail from addresses like `no-reply@`, for
mailing lists (`List-Unsubscribe`), sign-in codes, receipts and package
updates, and for drafts and sent mail.

## Data

`languages.py` and `phrases_a.py` to `phrases_d.py` write the mail; nothing in
it is real. For each language there are, for each of 33 situations
(thanks, a meeting request, an invitation, a request, a review, an
attachment, news, bad news, a sick day, running late, an apology, a yes/no
question, a call, a deadline, a check-in, wishes, an introduction, an
interview, praise, a cancellation, a follow-up…) and automated mail, a few
ways a message might say it, and greetings, small talk, sign-offs and
subjects to wrap them in; `SITUATIONS` says which replies fit each.
`data.py` builds messages of one or two situations, sometimes all lowercase,
without question marks or accents, and trains on the share of each
situation's fitting replies.

English has four wordings of each situation and the other languages two.
That is the model's main weakness: real mail says things in many more ways.
Training leaves out all the words of 30% of messages, keeping their
concepts, so wording it never saw still names concepts it knows; more
wordings, and native speakers' review of them, would help most.

## Training

```sh
python3 train.py --quick   # a small run, scored on wording it never saw
python3 train.py           # numpy only: replies-v1.bin, classes.json, parity.json, eval.json
```

`train.py` first trains on half of the wordings and reports how the model
does on the other half, by language, then trains the published model on all
of them. Afterwards update `REPLIES`' size and SHA-256 in
`src/manage/local_ai/mod.rs`. `parity.json` holds messages whose features,
vectors and scores the app must reproduce (`cargo test replies`), and
`eval.json` held-out messages in every language, on which the app's tests
check that the language is told and the best suggestion fits.

`catalog.json`'s wordings can change without retraining. Its list of
replies can't: the model's outputs are in `classes.json`'s order.

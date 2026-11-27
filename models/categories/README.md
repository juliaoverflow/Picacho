---
license: mit
library_name: picacho
tags:
- email
- classification
- multilingual
---

# Sift Categories: the inbox category model

The model behind **Settings → Local AI → Sort the inbox**. It reads what a
message list holds of a message (the sender's name and address, the
subject, the preview, and its list headers) and scores the inbox's
categories: Primary, Transactions, Travel, Finance, Updates, Newsletters,
Promotions, Social and Forums.

Picacho sorts the inbox without it, by rules (`src/manage/categories.rs`).
The model is asked where the rules are unsure. It runs after the steps that
are nearly certain: senders you sorted yourself, your own mail and mail
you replied to, receipts and package updates, sign-in codes, Gmail's and
Outlook's own sorting, and people you write to. Its category then wins
only when it is sure enough to disagree with the rules: 70% between other
categories, 80% to put mail in Primary, and 90% to take it out of Primary,
so a person's mail stays there.

| File | Used on | Runtime |
| --- | --- | --- |
| `categories-v1.bin` | every platform | Picacho's own (`NativeTagger` in `src/manage/local_ai/runtime.rs`) |

Like the other models, it isn't kept in git: it is published on Hugging Face
as [pinaltech/sift-categories](https://huggingface.co/pinaltech/sift-categories),
and Picacho downloads it from there when the feature is turned on, checking
its size and SHA-256 (`CATEGORIES` in `src/manage/local_ai/mod.rs`). A model
trained again here with `train.py` needs a new file name, size and digest
there, and publishing: upload it before releasing the app that names it,
`hf upload pinaltech/sift-categories models/categories/categories-v1.bin categories-v1.bin`,
and this README as the model card,
`hf upload pinaltech/sift-categories models/categories/README.md README.md`.

## Languages

Every language the other local models read: English, Spanish, German,
French, Portuguese, Italian, Dutch, Swedish, Danish, Norwegian, Catalan,
Polish, Russian, Ukrainian, Czech, Slovak, Croatian, Bulgarian, Romanian,
Hungarian, Finnish, Greek, Turkish, Arabic, Persian, Hebrew, Hindi,
Bengali, Indonesian, Malay, Vietnamese, Thai, Japanese, Chinese
(simplified and traditional), Korean, Tagalog and Swahili: the reply
model's 38 codes. People's mail comes from the reply model's messages in
each; the rest from the templates. The wordings were written for these
templates and haven't been reviewed by native speakers yet.

The sorting rules beside the model (`src/manage/categories.rs`) read the
same languages: their words of offers, trips, money and newsletters are
looked for as whole words, as stems where a language adds endings
("скидк*", "hesap özet*"), and anywhere in scripts that attach words or
write without spaces (Arabic, Persian, Hebrew, Thai, Japanese, Chinese,
Korean).

## Learning on this computer

Moving a sender to another category (**Move to Category…** in a message's
menu, or dragging messages onto a tab) sorts that sender there from now on,
by rule. Where the model is on, the newest messages from that sender (up
to 12 each) also teach it: a small linear layer over the same features,
added to the model's scores (`Learned` in
`src/manage/local_ai/categories.rs`), takes a few gradient steps toward the
category they were moved to. Mail like them from other senders then goes
there too. The model's own weights don't change. What was learned is kept
as a preference on this computer (`local_ai_categories_learned`, at most
6,000 features), survives turning the feature off and on, and is forgotten
with **Forget** in Settings → Local AI.

## How it works

1. **Words.** The sign-in code model's (`../otp/features.py`): lowercase
   runs of letters, marks and digits, with scripts written without spaces
   split into pairs of characters.
2. **Features** (`features.py`). The subject's words and word pairs, the
   preview's (its first 400 characters), the sender's name; the address's
   parts (`ship-confirm` → *ship*, *confirm*), the domain, its subdomains
   (`e.`, `alerts.`) and top-level domain; the **concepts**
   (`lexicon.json`) the subject and preview name, with stems in every
   language below (*sale*, *discount*, *order*, *receipt*, *ship*,
   *security*, *statement*, *follow*, *comment*, *group*, *topic*,
   *flight*, *hotel*, *bank*, *transfer*, *issue*…: 48 concepts, about
   2,100 stems); the list headers (List-Unsubscribe, through a marketing
   service in `senders.json` or not; List-Id and its parts; List-Post); and
   signs such as a percentage or a price beside a digit, shopping emoji, a
   reply's or a list's subject prefix (`Re:`, `[group]`), words in capitals,
   a name like a person's and a name matching the domain. Everything is
   hashed into 32,768 slots.
3. **Network.** The features, summed into 32 hidden units, then a softmax
   over the nine categories. The embedding is stored as 8-bit integers with
   a scale per row (the date model's `PICAWHN1` format): about 1.2 MB.

## Data

`data.py` writes the mail; nothing in it is real.

- **Primary** is people's mail: the reply model's messages
  (`../replies/languages.py`) in its 38 languages, from people's addresses
  at mail providers, workplaces and universities, a few from a small
  business's role address (`hello@`), a few forwarded.
- **Transactions, Travel, Finance, Updates, Newsletters, Promotions,
  Social** are put together from the templates: for 37 kinds of mail (an order, a shipment, a delivery,
  a receipt, a refund; a booking, a flight's check-in, boarding pass or
  delay; a statement, a bill, a card or transfer alert, a balance or tax
  document, money received; a security alert, a reminder, a renewal, a
  notification, a welcome, a policy change; a newsletter, an issue of one;
  a sale, a discount code, new arrivals, free shipping, loyalty points, an
  event, a "we miss you"; a comment, a follower, a message, a friend
  request, a mention, memories, a like), in every language below:
  English with four wordings of each, Spanish, German, French, Portuguese,
  Italian, Dutch and Polish two (`templates_a.py` to `templates_c.py`),
  the other 30 one, with their own products, topics, preheader, footer and
  currency (`templates_d.py` to `templates_i.py`), sent by made-up and real shops,
  airlines, banks, publications, services and social networks (and
  writers through newsletter services), from the
  addresses such mail comes from, with the list headers it has, a
  preheader or footer, and slips (lowercase, no preview). A quarter of the
  time the sender is another kind's, and now and then a shop writes from a
  personal address, so the sender alone doesn't decide.
- **Forums** is discussion lists' mail: people's messages from the reply
  model sent through a list, and digests and forum notices, with List-Id
  and List-Post.

Mail that could be counted either way is trained toward both, mostly the
first: a newsletter (some Updates, some Promotions), a welcome mail, a
booking (some Transactions), a bill (some Updates), a card alert or money
received (a quarter Transactions: Apple Mail counts such mail as
transactions).

The data's weakness is that it is made up: real mail says these things in
many more ways, and outside English and the 7 languages with two wordings,
each kind of message is written one way.
Training leaves out all the words of 30% of messages (keeping who sent it,
the headers and the concepts) and who sent 15%, so neither alone decides.
Mail in a language the model doesn't know falls back to the rules, and to
Primary, the safe side.

## Training

```sh
python3 train.py --quick   # a small run, scored on wording it never saw
python3 train.py           # numpy only: categories-v1.bin, classes.json, parity.json, eval.json
```

`train.py` first trains on half of the wordings and reports how the model
does on the other half, by category, with how often people's mail left
Primary and how often the model was sure enough (90%) for it to count;
then it trains the published model on all of them (`--held-out` runs only
the first part). In English and the 7 languages with two wordings, the
held-out half is wording never seen; in the other 30, which have one
wording of each kind of mail, it is kinds of mail never seen in that
language (a flight update when only bookings were). On it, v1 was right
for 96.5% of messages: people's mail 100% (taken out of Primary for 0.02%,
never surely), transactions, travel, newsletters and promotions 96–97%,
finance 91%, updates 90%, social and forums 100%. It was wrong while at
least 70% sure, the confidence at which it can overrule the rules, for
1.1% of messages. By language, people's mail was kept in Primary for 99%
or more in every one; the rest was right for 88% (Thai) to 100% (Chinese,
Hindi, Turkish), and 92% or more in 31 of the 38. Afterwards update
`CATEGORIES`' size and SHA-256 in `src/manage/local_ai/mod.rs`.
`parity.json` holds messages whose features, vectors and probabilities the
app must reproduce (`cargo test categories`), and `eval.json` held-out
messages on which the app's tests check the model's categories.

`classes.json`'s order is `Category::ALL`'s; the model's outputs follow it.

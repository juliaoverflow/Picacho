# Design note: keeping cached mail across folder renames (MAILBOXID, RFC 8474)

Status: implemented. Parts 1 and 2 are `Store::rename_mailbox`, used by
`rename_remote_folder`. Part 3 reads MAILBOXID from SELECT
(`transport::mailbox_id`), keeps it in `MailboxState`, and re-keys in
`sync_remote` (`renamed_from`). MAILBOXID is read from SELECT, not LIST-STATUS,
because the vendored imap-proto refuses unknown STATUS items. So a rename made
elsewhere is caught when the new name is first checked, not by the listing
itself.

## The problem, and what it costs today

A cached IMAP message's id is `{account}:{remote}:{validity}:{uid}`
(`a1:Projects/2026:42:7`). It is the `messages` table's primary key, and
copies of it are kept elsewhere (see the inventory below).

Renaming a folder (`Backend::rename_remote_folder`) calls `forget_mailbox`
for the old name and every folder inside it. RFC 3501 §6.3.5 keeps UIDs
and UIDVALIDITY across a rename, but the cache:

- drops every cached message of the folder and its subfolders, downloaded
  sources included (`raw`/`source` columns);
- re-lists them on the next check (headers: about 1 to 2 KB each), and
  downloads their sources again in the background up to the offline window
  (`download_batch`, 12 MB per batch). A 5,000-message, 2 GB folder costs
  ~10 MB of headers and up to 2 GB of re-download. On Gmail that counts
  against the 1 GB/day background budget (`GMAIL_DAILY_BUDGET`);
- loses per-message state that is only kept here: reading choices
  (`reading:` preferences), parcel and receipt findings, which are
  re-scanned, and a draft's link to the message it answers.

A rename done by another client looks the same: the old name's mail goes
when its folder disappears from the listing, and the new name's is fetched
from scratch.

## Inventory of references to a message id

Found by grepping for the id's construction (`format!("{}:{remote}:…")`,
`format!("{account}:{remote}:{validity}:")`) and for every struct field or
preference that holds a `Mail::id`. The list is exhaustive for the code at
commit a30540b, as far as those greps reach. Ids are never built anywhere
else (the constructions are in backend.rs, archive.rs, extras.rs,
remote_search.rs and store.rs), and the stored copies are these:

| Where | Field | Kind |
|---|---|---|
| `messages` | `id` (primary key), `data.id` (JSON), `data.remote_folder`, `folder` (the mailbox key, `remote:` + name for custom folders) | row |
| `parcel_scan` | `id` (= message id), `data` → `Sighting.mail_id` | row |
| `receipt_scan` | `id` (= message id), `data` → `Sighting.mail_id` | row |
| `offline_changes` | `data` → `QueuedChange.mails[].id` and `.remote_folder` (whole `Mail` values) | row |
| `mailbox_state` | `remote` (part of the primary key) | row, by name |
| `drafts` | `data` → `Draft.original` (the answered message's id); `Draft.remote: RemoteDraft { mailbox, … }` (Drafts folder only) | row |
| preference `reading:["acct","id"]` | key (`manage::reading::message_key`) | preference |
| preference `draft_removals:{account}` | `RemoteDraft.mailbox` (Drafts folder only) | preference |
| preference `parcel_statuses` | `Override.ids` (tracking numbers and orders, not message ids) | not affected |
| preference `hidden_receipts` | keys by order or Message-ID header | not affected |
| snoozed mail | by Message-ID header and folder name (`Snoozed.folder`) | by name |
| in memory | layouts (`{acct}:{remote}:{v}:` prefixes), the UI's message list, undo entries (`undo::Moved.before`) | rebuilt or dropped |
| thread ids `t:{remote}:{validity}:{root}` (`extras.rs`) | `data.thread_id` | JSON, by name |

Ids are also parsed for their validity and UID (`rsplit(':')`) in
backend.rs (`validity_of`, the catalog) and connection.rs (`select_for`).
Those parts don't change in a rename.

## Proposal

### 1. Re-key on our own RENAME

`Store::rename_mailbox(account, old, new) -> Result<()>`, called by
`rename_remote_folder` for each `(old, new)` in `moved` in place of
`forget_mailbox`, in one SQLite transaction (`BEGIN IMMEDIATE`, as the
store's transactions already are):

1. `messages`: for the rows with `id` in `[{acct}:{old}:, {acct}:{old}:\u{10FFFF})`
   (a primary-key range, as `cached_ids` uses), rewrite `id`, `data.id`,
   `data.remote_folder` and `folder` (the mailbox key), and `data.thread_id`
   where it is `t:{old}:…`.
2. `parcel_scan`, `receipt_scan`: rewrite `id` by the same range, and the
   `mail_id` of their sightings.
3. `mailbox_state`: move the `(account, old)` row to `(account, new)`.
4. `drafts`: rewrite `data.original` where it starts with the old prefix.
5. `offline_changes`: rewrite each queued `Mail`'s `id` and `remote_folder`
   (see below).
6. preferences: rename `reading:` keys whose id has the old prefix (a range
   on the preference key's JSON form), and `Snoozed.folder` where it names
   `old`.

After the commit: `forget_layout` for both names, so the next view is
built from the store, plus the UI's usual refresh after a folder change.

Crash safety: everything above is one transaction. A crash before the
commit leaves the old ids, and the server has the new name. That is today's
situation, because the next check sees the old name gone and forgets it,
so nothing worse than today can happen. The RENAME itself goes before the
transaction: if the RENAME fails, nothing is re-keyed. If the transaction
fails after a successful RENAME, fall back to `forget_mailbox` (today's
behaviour).

### 2. Offline changes queued against the old name

A queued change names messages by id and `remote_folder`. Rewriting them in
the same transaction (step 5) lets them replay against the new name with
the same UIDs. That is correct, since UIDs survive the rename. A queued
`MoveTo(old)` (into the renamed folder) is rewritten to `MoveTo(new)`. Not
rewriting would make them fail and be dropped at replay (`replay_queue`
drops refused changes), which loses the user's offline actions.

### 3. Renames made elsewhere (MAILBOXID)

With OBJECTID, keep each folder's MAILBOXID (STATUS `MAILBOXID`, or the
SELECT response code) in `mailbox_state`. When a listing lacks a known
name but has a new one with the same MAILBOXID, run `rename_mailbox` before
the check would forget it. Without OBJECTID, the behaviour stays as today.

### Rejected: ids by MAILBOXID

`{account}:{mailboxid}:{uid}` would make renames free. But it means
migrating every row and reference above, and keeping two schemes for
servers without OBJECTID. The migration is the risk this design avoids.

## Test plan

- `rename_mailbox` unit test (store): a folder with two messages (one
  downloaded), a parcel_scan and a receipt_scan row, a `reading:`
  preference, a draft whose `original` is one of them, a queued offline
  `Read(true)` on one and a `MoveTo(old)` on another, a `t:` thread id, and
  a `mailbox_state` row. After the rename all of them name the new folder,
  the source is still there (no re-download), and nothing names the old
  prefix (checked with a `LIKE '{acct}:{old}:%'` count on every table).
- Same with a folder inside the renamed one (`Projects/2026` →
  `Archive/2026`, from `folder_and_inside`).
- Mock server: rename a folder holding downloaded mail. The next check
  sends no `BODY.PEEK[]` and no header FETCH for those UIDs (SELECT with
  CONDSTORE only).
- Failure path: the RENAME is refused, so nothing is re-keyed. The
  transaction fails (simulated) after the RENAME, so the result is
  `forget_mailbox` (today's behaviour), not a half re-key.
- MAILBOXID (part 3): a listing where `Old` is replaced by `New` with the
  same MAILBOXID re-keys, and one with a different MAILBOXID forgets.

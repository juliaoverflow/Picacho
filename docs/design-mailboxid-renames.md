# Design note: keeping cached mail across folder renames (MAILBOXID, RFC 8474)

Status: proposal only. Nothing in it is implemented.

## The problem

A cached IMAP message's id is `{account}:{remote}:{validity}:{uid}`
(for example `a1:Projects/2026:42:7`). The id is the `messages` table's
primary key and is used as a key in many places:

- the `messages` table itself, and the parcel and receipt scans
  (`parcel_scan.id`, `receipt_scan.id`, `mail_id` inside their JSON);
- offline changes (`offline_changes.data`, whole `Mail` values with ids);
- drafts (`Draft::original`, the message a reply or forward answers);
- snoozed mail (by Message-ID, so it isn't affected), undo entries, and
  layouts (`{account}:{remote}:{validity}:` prefixes);
- code that parses the id: `rsplit(':')` for UID and validity in
  `backend.rs` (`validity_of`, the catalog) and `connection.rs`
  (`select_for`).

Renaming a folder (`rename_remote_folder`) calls `forget_mailbox` for the
old name and every folder inside it. That drops their cached mail, its
downloaded sources included, and the next check downloads it again. Under
RFC 3501 §6.3.5 a rename keeps UIDVALIDITY and UIDs, so this re-download
isn't needed. The same applies when another client renames a folder: the
old name's mail disappears from the cache and the new name's is fetched
from scratch.

## What RFC 8474 offers

With OBJECTID, `MAILBOXID` (STATUS, or the `MAILBOXID` response code on
SELECT/CREATE) names a mailbox and survives renames (§4.1). A folder that
was renamed elsewhere can be recognized: the same MAILBOXID under a new
name.

## Options

1. **Re-key on rename (no MAILBOXID needed for our own renames).** After a
   successful RENAME, in one transaction:
   - Rewrite `messages.id`, `json data.id` and `data.remote_folder` from
     `{account}:{old}:{v}:` to `{account}:{new}:{v}:`. The same goes for
     the parcel and receipt scans' ids and `mail_id`, `mailbox_state.remote`,
     and drafts' `original`.
   - Rewrite queued offline changes, or drop the ones naming the old
     mailbox (they'd fail anyway).
   - Rebuild layouts (they are in memory: `forget_layout` is enough).

   That covers our own renames, which is the common case.
2. **Detect renames elsewhere (needs MAILBOXID).** Keep each folder's
   MAILBOXID in `mailbox_state`. When a listing no longer has a name but
   has a new one with the same MAILBOXID, apply the same re-keying as (1).
   Without OBJECTID, renames done elsewhere still look like delete plus
   create, as they do today.
3. **Stable ids (`{account}:{mailboxid}:{uid}`).** Changing the id scheme
   would mean migrating every row and every stored reference, and two
   schemes would coexist on servers without OBJECTID. That's too risky for
   the benefit.

## Proposal

Do (1), then (2). Both use one store function
(`Store::rename_mailbox(account, old, new)`) that re-keys in a single
transaction, with:

- a prefix match on the primary key's range (as `cached_ids` already
  does), so only the affected rows are visited;
- a test that renames a folder holding downloaded mail with parcel and
  receipt scan rows, an offline change and a draft reply, and finds them
  all under the new name;
- folders inside the renamed one (`folder_and_inside`) re-keyed the same
  way.

## Risks (why this wasn't done in this lane)

- It touches store.rs's schema users across lanes: parcels and receipts
  (scan tables), offline changes, drafts and layouts. One missed
  reference leaves a dangling id: a draft whose "original" is gone, or a
  parcel pointing nowhere.
- A crash halfway is safe only if everything happens in one SQLite
  transaction. Layouts and the UI's in-memory state also hold ids, and
  have to be refreshed after it commits.
- UIDVALIDITY could change across a rename on a non-conforming server.
  Then the next SELECT's validity check forgets the mail anyway (as it
  does today), so it isn't a correctness risk, just a wasted re-key.

These touch other lanes' tables, so the supervisor should decide before
anyone builds this.

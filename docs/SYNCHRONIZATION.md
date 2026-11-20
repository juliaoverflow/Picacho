# Synchronization contracts

Sync implements the wire protocols; Manage owns cache policy and per-account serialization. The cache is useful offline and is not evidence that a remote command succeeded. New protocol work should preserve these rules.

## Identities and snapshots

An IMAP identity is `(account, remote mailbox, UIDVALIDITY, UID)`. Never use message sequence numbers as persistent identities. A missing or zero UIDVALIDITY is an error. Opening, downloading, or modifying cached mail selects its mailbox and checks the generation before using its UID. A completed snapshot invalidates previous generations across standard, custom and virtual Starred views, including their raw bodies.

SEARCH describes the entire retention window. Only the newest requested headers need to be fetched (and only those not already cached; cached ones get a flags-only FETCH); older cached messages within that window remain. FETCH may contain unsolicited flag updates. Only requested UIDs with actual headers become summaries. If requested headers are missing, repeat SEARCH: confirmed expunges can be reconciled, while omitted but still-present messages fail the snapshot and preserve the cache. A new message arriving after SEARCH is picked up on the next refresh.

One mailbox's header updates, old-generation removal, flag clearing and reconciliation commit in one SQLite transaction. Failure rolls the transaction back. Starred synchronization clears stale stars in that source mailbox, rather than deleting unrelated non-starred messages. Database failure tests deliberately abort writes after reconciliation has started.

POP3 uses `(account, UIDL)`, never the session's message number. Validate the entire UIDL response before retrieving mail: numbers must be positive and unique; identities must be unique, printable ASCII and 1–70 bytes. Sort by message number, since response row order is not guaranteed. Save the summary and raw bytes atomically. A cached identity without raw bytes is repaired by downloading again while preserving local folders/read/star flags. Interrupted downloads never create a completed identity. Originals remain on the server; Picacho never sends DELE.

## Failure and retry boundaries

Display tracks failures per account and operation (each synchronized mailbox,
body downloads, reading, changes, and sending). A sidebar warning opens the
account's settings with the full error chain, time, and recovery controls.
Successful work clears its own failure; incoming synchronization also resolves
earlier incoming access failures, while an SMTP or rejected message change is
kept until its corresponding operation succeeds. Removed-account completions
cannot restore warnings. Account failures are session state, never credentials
or provider responses persisted to SQLite.

Rejected IMAP/POP3 logins, rejected OAuth grants, and Keychain failures have
typed access errors. Automatic incoming checks/downloads pause for those access
failures, while transient/protocol failures retain the mailbox's polling schedule.
Explicit retry reopens a previously denied Keychain entry; sign-in replacement
checks the same email address and preserves the account/cache/drafts/preferences.
Credentials and Keychain failures are cached per account in memory, and token
refresh, credential writes, and encryption-key updates share account serialization.
No Keychain read or browser-token refresh runs on the UI thread.

Polling coalesces in-flight requests by account and mailbox, independently of
global UI busy state. While the server limits an account (see Gmail below),
automatic checks, the catalog and body downloads pause; explicit refreshes and
opening a message still try.

## Incremental synchronization

Each server mailbox's last synchronization is saved in `mailbox_state`
(UIDVALIDITY, UIDNEXT, message count, HIGHESTMODSEQ, sync window), only after
the mail it describes has committed. A check SELECTs the mailbox (with
`(CONDSTORE)` when advertised) and then asks only for what changed:

- New mail: `UID FETCH <old UIDNEXT>:*`, keeping only UIDs at or above it
  (`n:*` always returns the newest message).
- Flags: with CONDSTORE, `UID FETCH 1:* (UID FLAGS) (CHANGEDSINCE <modseq>)`,
  or nothing when HIGHESTMODSEQ is unchanged; without it, the flags of the
  newest cached messages.
- Removals: without QRESYNC there is no list of expunged UIDs, but the count
  shows them. If EXISTS is not the old count plus the new messages, the check
  falls back to the full SEARCH snapshot above, which reconciles them.

The full snapshot also runs on the first check, after UIDVALIDITY or the sync
window changes, without UIDNEXT, or when more new mail arrived than a check
lists (the catalog lists the rest). A confirmed MOVE lowers the source's saved
count, so moving mail does not force the next check into a search. An
unchanged mailbox costs one SELECT.

## Connections

Each account keeps one signed-in IMAP connection (`connection.rs`), taken by
an operation under the account lock and put back when it succeeds. A failed
operation drops it. A connection idle for 20 seconds is checked with NOOP
before use, so STORE/MOVE (never replayed) are not sent on a closed one; after
ten idle minutes it is replaced. The mailbox list (LIST) is read once per
connection except by sync, which refreshes the folder list, and a mailbox stays
selected between operations. Replacing an account's sign-in or removing it
signs the connection out.

## Server extensions

After signing in, a session sends `ID` where offered and turns on
`COMPRESS=DEFLATE` (raw deflate in both directions above TLS,
`sync/stream.rs`); neither is required. `CAPABILITY` decides the rest:

- `SPECIAL-USE`: LIST asks servers to mark standard folders by role
  (`RETURN (SPECIAL-USE)`), rather than guessing from names.
- `NAMESPACE`: new top-level folders go in the personal namespace's prefix
  (`INBOX.` on servers that keep everything inside the inbox); without it,
  the folder list's own shape decides.
- `ACL`: `MYRIGHTS` is checked before folder changes (k, x), moves (i on the
  target, t and e on the source), deleting messages (t, e) and labels (w), so
  a refusal changes nothing and says which right is missing. RFC 2086's older
  `c` and `d` letters count too.
- `MOVE` (RFC 6851) is used when offered; otherwise `UID COPY`, then the moved
  messages are marked `\Deleted` and removed with `UID EXPUNGE` (UIDPLUS).
  Without UIDPLUS, a plain `EXPUNGE` is used only when `UID SEARCH DELETED`
  shows no other marked message; else the move is refused before the copy.
  Permanent deletion and emptying Trash/Spam use the same removal.
- `QUOTA`: `GETQUOTAROOT INBOX` reports storage for Settings.
- `APPENDLIMIT`: drafts and sent copies larger than the server keeps are not
  uploaded.
- Labels: Gmail's `X-GM-LABELS` (fetched with headers and flags, set with
  `UID STORE ±X-GM-LABELS`), else keywords among `FLAGS` (set with
  `±FLAGS.SILENT`, refused where `PERMANENTFLAGS` lacks `\*`). System
  keywords (`$Forwarded`, `$MDNSent`, junk markers…) aren't shown. Labels
  travel in the server's form; Gmail's are modified UTF-7 like folder names.

- `THREAD=REFERENCES` gives recent mail conversation ids (`t:` ids, within a mailbox); Gmail's `X-GM-THRID` and OBJECTID's `THREADID` come with headers and are permanent. `PREVIEW` fills snippets before bodies download; `SAVEDATE` is kept. `METADATA` provides the server's `/shared/comment` and `/shared/admin` notes and stores snooze times.
- `NOTIFY`: the inbox watcher asks for every personal mailbox's changes.

`imap-proto` doesn't model NAMESPACE, QUOTA, MYRIGHTS, ID or ENABLED replies,
and `imap` 2.4 mistakes any untagged line it can't parse for the server
leaving; the patched `imap-proto` keeps them as text (`Response::Other`). Its patch 0004 in [Picacho-Patches](https://github.com/pinaltech/Picacho-Patches/tree/vendor-split-dev/imap-proto) adds ESEARCH, SORT, THREAD, VANISHED and the extension FETCH attributes.

## Push (IDLE)

Each IMAP account whose server offers `IDLE` has one more connection
(`manage/watch.rs`), outside the connection pool and the account lock. It
opens the inbox read-only (`EXAMINE`) and waits in IDLE, renewed every nine
minutes. Any change it hears makes Display run the regular inbox check, so
the cache only changes under the usual rules. While waiting, the wire allows
reads to wait for the IDLE period instead of its usual limits; sending
anything restores them. Stopping a watch closes its socket from another
thread. Failures back off from 30 seconds to 15 minutes (the longest for
rejected sign-ins and server limits) and trigger a check, since mail may
have arrived meanwhile. Polling continues as the fallback.

## Drafts

`Backend::sync_drafts` pairs local drafts with the server's Drafts folder
(`\Drafts`, or by name). Each uploaded copy carries an `X-Picacho-Draft`
header with the draft's id, and the draft records its copy's mailbox,
UIDVALIDITY and UID plus a fingerprint of what was uploaded. In order:

1. Copies of drafts sent or discarded here (queued by `discard_draft`, which
   reads the stored draft since a composer's copy may predate an upload) are
   removed with the safe expunge above.
2. A local draft whose copy is gone is deleted if unchanged since upload (it
   was sent or deleted elsewhere); a changed one uploads again.
3. Server messages no local draft claims (by UID or header id) become drafts,
   with recipients, Bcc, Reply-To, threading headers, text and attachments
   (saved under `draft-files/` in the data directory).
4. Drafts not uploaded in their current form are appended (`\Draft \Seen`);
   the new copy is found as the UID that appeared, and earlier copies of the
   same draft (by UID or header id) are removed. If a composer saved a newer
   version meanwhile, it keeps its words and uploads next time.

It runs when the Drafts view opens or refreshes, when a composer closes or
is discarded, after sending, and with inbox checks at most every two minutes.

## Catalog and placeholder rows

There is no "Load more" step. The first full snapshot's SEARCH is the
mailbox's *layout*: every UID within the sync window, held in memory per
account and mailbox, updated by later checks and moves, and replaced when
UIDVALIDITY or the window changes. The message list draws a row for each
UID from the start (`Backend::listed`): cached messages, and placeholders
for the rest, in arrival (UID) order, newest first, so rows keep their places
and the scrollbar covers the whole mailbox while headers load. Without a
layout (Starred, POP3, before the first check) the list is cached mail by
date, as before.

Placeholders around the visible rows are listed first (`Backend::fill`, only
the latest visible range is kept while one request runs). `Backend::catalog`
lists the rest in the background, 200 headers per request, newest first,
reusing the layout rather than searching again; if no layout matches it
searches once and reconciles the cache with the result. Listed messages
replace their placeholders in place; a changed layout reloads the list.
Placeholders cannot be opened or selected, and filters and searches show only
listed mail. POP3 has no header-only listing, so older messages are
downloaded whole, 25 at a time.

Headers are fetched as `BODY.PEEK[HEADER.FIELDS (FROM TO SUBJECT DATE
MESSAGE-ID CONTENT-TYPE)]` with FLAGS, RFC822.SIZE and INTERNALDATE. Full
headers (Received, DKIM and ARC chains) are often several times larger and
are only needed once a message is opened, when its whole source downloads.
The list loads cached messages without bodies (`Store::summaries`); opening a
message reads its body. When the list reloads, its scroll position stays on
the same message. Inbox polling timestamps only advance for inbox checks;
refreshing other folders cannot postpone inbox checks indefinitely.

## Bodies

Each account has its own body-download worker, with account locks released
between small batches. Only messages inside the account's offline range
(Settings → "Download for offline reading": one month by default, one year,
all, or only when opened) download ahead of time; the rest download when
opened. Download errors retain their account/cause; selecting a stale or
inaccessible mailbox no longer silently pauses the downloader.

## Gmail

Gmail exposes labels as folders, so one message appears in INBOX, All Mail
and each label. With `X-GM-EXT-1`, header listing also reads `X-GM-MSGID`,
Gmail's permanent per-message id (`Mail::gmail_id`). Body downloads skip a
message whose id is already downloading, copy content already downloaded
under another label (`Store::share_gmail_raw`), and opening a message uses
such a copy instead of the network. The id is server-assigned, so unlike
Message-ID a sender cannot make two different messages share one. Parsing
it needs the patched `imap-proto`
([Picacho-Patches](https://github.com/pinaltech/Picacho-Patches/tree/HEAD/imap-proto)).

Gmail allows 2,500 MB of IMAP downloads per account per day and then slows
the account to a crawl. Background traffic (headers and bodies) is counted per
account per UTC day; at 1 GB the catalog and body downloads stop until the
next day, leaving room for opening messages and other mail apps. Responses
that report a server limit (`[THROTTLED]`, `[OVERQUOTA]`, "bandwidth limits",
"Too many simultaneous connections", `[UNAVAILABLE]`) pause background work
for the account for ten minutes, doubling while they continue (up to four
hours), and are shown as a typed `Throttled` account issue rather than a
connection failure.

- Account operations serialize, including token refresh and local changes. Separate accounts can continue independently.
- Read-only mailbox synchronization retries once on a fresh connection after a transient I/O failure, with a 250 ms delay. Protocol rejection, malformed responses, authentication rejection and certificate failures are not automatically replayed. The normal polling schedule or an explicit refresh can try again later.
- UID STORE/MOVE are never automatically replayed. Bulk changes use at most 200 UIDs per command; each acknowledged chunk commits immediately. If a later command fails, earlier confirmed changes remain. Display reloads confirmed cache state after a failed bulk operation.
- A disconnect after an acknowledged operation during LOGOUT/QUIT does not turn success into failure. POP3 cleanup has no pending deletions.
- SMTP acceptance is the send boundary. Filing a sent copy or cleaning up a draft cannot make an accepted message appear unsent. No automatic SMTP resend is attempted. If the server accepts DATA but its final reply is lost, delivery is ambiguous; exactly-once delivery cannot be guaranteed by SMTP.

## Resource limits

TCP attempts have a shared 12-second budget after name resolution, with at most three seconds per address. Verified TLS is mandatory, including STARTTLS; rejection never falls back to plaintext. The socket has a 30-second inactivity timeout. A wire wrapper adds a 90-second response budget and a 32 MiB byte budget between outgoing writes, including TLS framing. This bounds the IMAP library's otherwise unbounded response buffering, even when a peer lies about RFC822.SIZE. TLS handshake writes can also restart this wire budget. The decoded-message limit remains 25 MiB. POP3 status lines and multiline responses have separate length/completeness checks.

System DNS resolution is synchronous and not covered by the TCP budget. Full SEARCH responses and headers are intentionally capped by the wire budget: an unusually large mailbox can report an error while keeping cached mail. With ESEARCH, full searches come back as compact sets; without it, mailboxes over 500,000 messages are searched in UID windows, so no reply outgrows the wire budget. QRESYNC (ENABLEd per connection) reports removals with SELECT's VANISHED (EARLIER); without it (Gmail) removals still cost one full SEARCH. With `UTF8=ACCEPT` names travel in UTF-8 on the wire and are kept in modified UTF-7 in the cache. Large messages download in resumable pieces (`partial-downloads/` in the data directory). Changes that can't reach the server are queued per account (`offline_changes:<account>`) and replayed first by the next sync or change; STOREs are idempotent and a replayed MOVE of vanished UIDs changes nothing.

SMTP is Picacho's own client (`sync/smtp.rs`); lettre only builds the MIME message. It runs over the same bounded wire as IMAP and POP3. The client says hello as `[127.0.0.1]`, so the computer's name isn't written into Received headers. STARTTLS is required unless the port is 465 (implicit TLS). Anything the server sends before the TLS handshake is refused. From the EHLO reply:

- `SIZE` limits the message, checked before `MAIL FROM` and declared on it.
- `DSN` lets delivery receipts ask for `NOTIFY=SUCCESS,FAILURE,DELAY` with `RET=HDRS`.
- `SMTPUTF8` and `8BITMIME` are declared where needed.
- `PIPELINING`: `MAIL FROM` and every `RCPT TO` are sent together and their replies read in order. The message waits for those replies, so a refused recipient stops the send before any data goes.
- `CHUNKING`: the message goes in `BDAT` chunks of 1 MiB, without dot-stuffing (pipelined where allowed); otherwise `DATA`.
- `REQUIRETLS`: added to `MAIL FROM` when the account requires encryption in transit. Without it, nothing is sent.
- `AUTH`: the best mechanism Picacho speaks (`sync/sasl.rs`). For passwords: SCRAM-SHA-256, SCRAM-SHA-1, PLAIN, LOGIN, then CRAM-MD5. For tokens: OAUTHBEARER, then XOAUTH2. The first message goes with the command (RFC 4954). SCRAM checks the server's signature, so a server that can't prove it knows the password fails the sign-in.

The reply to the end of the message is waited for up to ten minutes (RFC 5321).

IMAP asks for `CAPABILITY` before signing in, or reads it from the greeting. It does this directly on the stream, since the imap crate can't before login. SCRAM is used where offered, otherwise `LOGIN` unless `LOGINDISABLED` is listed, then `AUTHENTICATE` with PLAIN, LOGIN or CRAM-MD5. POP3 asks `CAPA` (servers without it get `USER`/`PASS`) and uses `AUTH` (RFC 5034) for SCRAM, or for another mechanism where `USER` isn't listed.

OpenSSL 3 reports a connection closed without TLS's goodbye as an error. IMAP and SMTP treat it as the end of the stream, as earlier versions reported it, so a dropped IMAP connection is seen as lost and a read-only sync may retry it.

A sent copy that couldn't be appended is queued as a file under `sent-copies/<account>/` in the data directory. It is filed by the next draft sync, after a `UID SEARCH HEADER Message-ID` in Sent finds no copy already there. Removing the account removes its queue.

MTA-STS checks (`sync/mta_sts.rs`) only run when encryption in transit is required. The `_mta-sts` TXT record goes through the resolver used for sender checks. The policy is fetched from `https://mta-sts.<domain>/.well-known/mta-sts.txt` without redirects (at most 64 KB, `text/plain`) and kept until its max_age or a new record id, for at most a day. A policy in `testing` counts, since REQUIRETLS treats it as enforce. DANE means a TLSA record at `_25._tcp.<mx>` for the most preferred MX; the sending servers check its DNSSEC signatures. These checks only inform: delivery is the outgoing server's.

## JMAP

JMAP accounts (RFC 8620 core, RFC 8621 mail) talk HTTPS and JSON to one
server for reading and sending (`sync/jmap.rs`, `manage/jmap.rs`). The
session comes from `https://<server>/.well-known/jmap`. Redirects are
followed only on the same host, so credentials never go to another one.
Sign-in is HTTP Basic with the sign-in name and password, then the same
password as a bearer token (Fastmail's API tokens). A 401 or 403 is a typed
`Authentication` access failure, and the session is dropped so that the
next try signs in again.

- Identity: a message keeps its JMAP id wherever it goes, so it is one row
  (`{account}:jmap:{id}`). Moving a message replaces its `mailboxIds` and
  updates the row instead of removing it and finding it again.
- Mailboxes: standard folders are found by role (RFC 8457). Servers without
  roles fall back to the usual names at the top level. Other mailboxes are
  the account's own folders, named by path. Collections (`all`, `flagged`…)
  are left out. A message in several mailboxes shows in the inbox, then
  Trash, Spam, Drafts, Sent, Archive, then its first own folder.
- Flags: `$seen`, `$flagged`, `$answered`, `$forwarded` and `$mdnsent`.
  Other keywords are labels, except the system keywords IMAP hides too.
  Starred is a `hasKeyword` query and All Mail an `inMailboxOtherThan`
  (Drafts) query, so neither is put together from the other mailboxes.
- Sync: each mailbox remembers the Email state it was last read in. If
  `Email/changes` reports nothing since then, a check costs that one
  request. Otherwise the newest messages are listed (`Email/query` and
  `Email/get` in one request, through a back-reference). Cached messages
  that weren't listed are asked about again: moved ones update their row,
  and ones that are gone are removed. The catalog pages on with `position`.
- Bodies download as blobs. Messages over the download limit read their
  text and HTML body values alone, and each attachment downloads by its
  blob id when it is saved.
- Changes are one `Email/set` per change. Offline changes queue and replay
  as they do for IMAP. Deleting for good is `Email/set destroy`.
- Sending uploads the message, imports it into Drafts (`$draft`) and creates
  an `EmailSubmission` for the identity that covers the From address, all
  in one request. Once the server accepts it, `onSuccessUpdateEmail` moves
  it to Sent. If the submission is refused, the imported copy is destroyed.
  Mail sent as an address with its own SMTP server gets a copy imported into
  Sent, as for IMAP. Copies that couldn't be filed are queued the same way.
- Drafts sync with the Drafts mailbox, as Proton's do (`jmap:` remote ids).
- Push: the event source (`types=Email,Mailbox`, a ping every 30 seconds)
  stands in for IDLE. When it reports a change, `Email/changes` says which
  mailboxes to check. Servers without an event source are asked for changes
  every minute.
- Search sends Picacho's search terms as JMAP filter conditions, so the
  server searches message contents too. Quota comes from RFC 9425
  `Quota/get` (octets) where offered.

- Filters: where the server offers JMAP for Sieve Scripts (RFC 9661),
  Settings → Filters & vacation writes the same `picacho` script that
  ManageSieve gets. It is uploaded as a blob, checked with
  `SieveScript/validate`, created or replaced, and turned on with
  `onSuccessActivateScript`. With no rules and no vacation reply, the
  script it included is turned back on (`onSuccessDeactivateScript` if
  there wasn't one). Reading the scripts and choosing what to include work
  the same as with ManageSieve (`sieve::loaded`).
- Browser sign-in (OAuth): the server's RFC 8414 metadata
  (`/.well-known/oauth-authorization-server`, else OpenID Connect's
  configuration) gives the endpoints. Fastmail uses Picacho's built-in
  registration (`PICACHO_FASTMAIL_CLIENT_ID`). Other servers register
  Picacho for each sign-in where their metadata has a registration
  endpoint (RFC 7591, a public native client). The flow is PKCE through a
  loopback redirect. The scopes are JMAP's (core, mail, submission, Sieve)
  plus `offline_access`, among those the server lists. The client id is kept
  with the account so tokens can be renewed. A session opened with a token
  is opened again shortly before the token expires, after renewing it. If
  neither the token nor the address entered names the account, the session's
  `username` does.
- Discovery: Fastmail's domains and custom domains whose MX is at
  `messagingengine.com` go to Fastmail (`api.fastmail.com`). For other
  domains, an `_jmap._tcp` SRV record (looked up with the resolver used for
  sender checks) wins over autoconfig, since the domain names its JMAP server
  itself. A domain that only answers at `/.well-known/jmap` (a 401 with a
  Basic or Bearer challenge, or a redirect to a JMAP path) is used when
  autoconfig, the ISPDB and MX records found nothing, before ports are
  probed.

Fastmail is offered over JMAP. Fastmail accounts added over IMAP before
this keep using IMAP.

## Reference implementation review

Reviewed these pinned upstream sources on October 26, 2026 (no upstream source was copied into Picacho):

- [Thunderbird nsImapMailFolder.cpp, 689401a](https://github.com/mozilla/releases-comm-central/blob/689401a6977ae564a80f26155531ddee73073758/mailnews/imap/src/nsImapMailFolder.cpp): `HandleUidInvalidation` discards old-generation linkage, and header completion controls persistent synchronization progress. Picacho similarly rejects stale identities and commits only validated snapshots.
- [Thunderbird Pop3Client.sys.mjs, 689401a](https://github.com/mozilla/releases-comm-central/blob/689401a6977ae564a80f26155531ddee73073758/mailnews/local/src/Pop3Client.sys.mjs): download incorporation completes before UIDL state advances; write failures abort incorporation. Picacho makes its body and UIDL identity one database transaction.
- [NeoMutt imap/message.c, 3a2e4b6](https://github.com/neomutt/neomutt/blob/3a2e4b6eb03c0439ffcac34c157a021a0d710dc1/imap/message.c): body cache keys include UIDVALIDITY, and header retrieval explicitly handles expunges and vanished messages. This informed Picacho's generation checks and SEARCH/FETCH race tests. NeoMutt's incremental synchronization is more extensive and remains a useful next reference.

## Reproduce the independent-server comparison

Install Dovecot **2.4**, its IMAP/POP3 workers, Python 3 and OpenSSL. No system daemon or credentials are needed. On Linux, an extracted installation can also be selected using `PICACHO_DOVECOT_PREFIX` (the prefix containing `bin/doveconf` and `lib/dovecot`). Then run:

```sh
cargo test --locked dovecot_matches -- --ignored --test-threads=1
```

`scripts/local-mail-server.py` creates a temporary Maildir, generates a certificate, binds ephemeral loopback TLS ports and launches unprivileged Dovecot workers. The TLS gateway accepts synthetic fixture logins; Dovecot implements mailbox commands. This checks protocol interoperability, not Dovecot's authentication daemon. Production certificate verification stays enabled; only test builds accept the fixture's certificate.

The Rust test compares Picacho with Python's independent `imaplib`/`poplib` clients: UID sets, raw-message SHA-256 hashes (including dot unstuffing), server flags, atomic moves, repeated POP3 downloads and local-folder preservation. It closes the gateway and cleans up temporary workers/mail afterward. This is an operational server/client-library comparison, not a full Thunderbird or NeoMutt UI comparison. No live provider, external recipient or real mailbox is involved.

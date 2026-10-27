# Synchronization contracts

Sync implements the wire protocols; Manage owns cache policy and per-account serialization. The cache is useful offline and is not evidence that a remote command succeeded. New protocol work should preserve these rules.

## Identities and snapshots

An IMAP identity is `(account, remote mailbox, UIDVALIDITY, UID)`. Never use message sequence numbers as persistent identities. A missing or zero UIDVALIDITY is an error. Opening, downloading, or modifying cached mail selects its mailbox and checks the generation before using its UID. A completed snapshot invalidates previous generations across standard, custom and virtual Starred views, including their raw bodies.

SEARCH describes the entire retention window. Only the newest requested headers need to be fetched; older cached messages within that window remain. FETCH may contain unsolicited flag updates. Only requested UIDs with actual headers become summaries. If requested headers are missing, repeat SEARCH: confirmed expunges can be reconciled, while omitted but still-present messages fail the snapshot and preserve the cache. A new message arriving after SEARCH is picked up on the next refresh.

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
global UI busy state. A larger visible-message request is refreshed after a
smaller background result, so navigation/Load More cannot be replaced with a
shorter result. Inbox polling timestamps only advance for inbox checks; refreshing
other folders cannot postpone inbox checks indefinitely. Each account has its
own body-download worker, with account locks released between small batches.
Download errors retain their account/cause; selecting a stale or inaccessible
mailbox no longer silently pauses the downloader.

- Account operations serialize, including token refresh and local changes. Separate accounts can continue independently.
- Read-only mailbox synchronization retries once on a fresh connection after a transient I/O failure, with a 250 ms delay. Protocol rejection, malformed responses, authentication rejection and certificate failures are not automatically replayed. The normal polling schedule or an explicit refresh can try again later.
- UID STORE/MOVE are never automatically replayed. Bulk changes use at most 200 UIDs per command; each acknowledged chunk commits immediately. If a later command fails, earlier confirmed changes remain. Display reloads confirmed cache state after a failed bulk operation.
- A disconnect after an acknowledged operation during LOGOUT/QUIT does not turn success into failure. POP3 cleanup has no pending deletions.
- SMTP acceptance is the send boundary. Filing a sent copy or cleaning up a draft cannot make an accepted message appear unsent. No automatic SMTP resend is attempted. If the server accepts DATA but its final reply is lost, delivery is ambiguous; exactly-once delivery cannot be guaranteed by SMTP.

## Resource limits

TCP attempts have a shared 12-second budget after name resolution, with at most three seconds per address. Verified TLS is mandatory, including STARTTLS; rejection never falls back to plaintext. The socket has a 30-second inactivity timeout. A wire wrapper adds a 90-second response budget and a 32 MiB byte budget between outgoing writes, including TLS framing. This bounds the IMAP library's otherwise unbounded response buffering, even when a peer lies about RFC822.SIZE. TLS handshake writes can also restart this wire budget. The decoded-message limit remains 25 MiB. POP3 status lines and multiline responses have separate length/completeness checks.

System DNS resolution is synchronous and not covered by the TCP budget. Full SEARCH responses and headers are intentionally capped by the wire budget: an unusually large mailbox can report an error while keeping cached mail. IDLE, QRESYNC/CONDSTORE, persistent sessions, resumable partial MIME downloads, and an offline mutation queue remain future work. Polling performs full searches; this task does not claim best-in-class throughput on very large mailboxes.

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

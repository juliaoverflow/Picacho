# Draft imap-proto patches

Two patches for Picacho-Patches' `imap-proto` are drafted here. Each applies on its own to `imap-proto/crate` with patches 0001 to 0007, using `patch -p1` from `crate/`.

## 0008: literals in extension replies

`imap-proto-resp_other-literals.patch` is a draft of the proper fix for
literals in untagged extension replies (NAMESPACE, QUOTA, ACL, MYRIGHTS, ID,
ENABLED, ESEARCH, SORT, THREAD, GENURLAUTH and others). RFC 9051 §4.3 allows
such a reply to carry any string as a literal: `{n}`, CRLF, then n bytes.

Currently, the vendored parser's `resp_other` keeps the reply as text up to the
first CRLF. A literal therefore cuts the reply short, and the `imap` crate fails
the whole command when it tries to parse the literal's bytes as the next
response.

The patch changes `resp_other` to read `other_text`. That text keeps each
literal whole (`{n}`, `{n+}` and literal8's `~{n}`) and is Incomplete until the
literal and the rest of its line have arrived. `src/sync/imap.rs`'s `tokens()`
already reads literals in that form.

How to land it:
1. Base it on Picacho-Patches' `imap-proto/crate` with patches 0001–0007
   applied. Save it there as `imap-proto/patches/0008-other-literals.patch`,
   apply it to `crate/`, and add a README section.
2. Run `cargo test` in `imap-proto/crate`. The new
   `test_other_with_literals` test passes, along with the 44 existing tests.
3. In Picacho, run `cargo update -p imap-proto`.
4. Remove the stopgap: `Framer`, `REWRITTEN` and `UNREADABLE` in
   `src/sync/stream.rs`, plus `unreadable_reply` and its use in `other_lines`
   (src/sync/imap.rs). Keep the mock test
   `extension_replies_with_literals_are_read`, but expect the line-broken
   NAMESPACE prefix to be read and not refused.

## 0009: APPENDLIMIT in STATUS

`imap-proto-status-appendlimit.patch` parses STATUS's `APPENDLIMIT n` or
`APPENDLIMIT NIL` (RFC 7889 §4) as `StatusAttribute::AppendLimit`. A server
that lists `APPENDLIMIT` without a number sets each mailbox's own limit
(RFC 7889 §3). Without this patch, that reply ends the session.

The client already asks a mailbox for its limit (`Connection::append_limit`).
It does so only once `reads_status_appendlimit()` reports that the parser
reads the reply, so nothing has to change in Picacho when the patch lands
besides running `cargo update -p imap-proto`. The patch adds the test
`test_status_appendlimit`.

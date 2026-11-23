# Draft imap-proto patch 0008: literals in extension replies

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

# imap-proto Gmail patch

Source: `imap-proto` 0.10.2 from crates.io, https://github.com/djc/tokio-imap.
License: MIT/Apache-2.0.

The only change parses Gmail's `X-GM-MSGID` FETCH attribute
(`src/parser/gmail.rs`, `AttributeValue::GmailMsgId`). Without it, `imap` 2.4
rejects any response that carries the attribute, so Picacho could not use
Gmail's permanent message ids to download a message once across labels.

Remove this patch when Picacho moves to an `imap`/`imap-proto` release that
parses Gmail attributes.

Building it as a path dependency also needed two mechanical fixes that the
registry build hides with `--cap-lints`: three `macros.rs` arms drop a
trailing semicolon in expression position, and `lib.rs` allows warnings.

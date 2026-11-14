# imap-proto Gmail patch

Source: `imap-proto` 0.10.2 from crates.io, https://github.com/djc/tokio-imap.
License: MIT/Apache-2.0.

The changes:

- Parse Gmail's `X-GM-MSGID` and `X-GM-LABELS` FETCH attributes
  (`src/parser/gmail.rs`, `AttributeValue::GmailMsgId` and
  `AttributeValue::GmailLabels`). Without them, `imap` 2.4 rejects any
  response that carries one, so Picacho could not use Gmail's permanent
  message ids or show its labels.
- Keep untagged NAMESPACE, QUOTA, QUOTAROOT, MYRIGHTS, LISTRIGHTS, ACL, ID
  and ENABLED data as `Response::Other` text (`resp_other` in
  `src/parser/rfc3501.rs`). `imap` 2.4 otherwise takes any untagged
  response it can't parse for the server hanging up.

Remove this patch when Picacho moves to an `imap`/`imap-proto` release that
parses these.

Building it as a path dependency also needed two mechanical fixes that the
registry build hides with `--cap-lints`: three `macros.rs` arms drop a
trailing semicolon in expression position, and `lib.rs` allows warnings.

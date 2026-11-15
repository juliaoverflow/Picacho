# imap-proto (vendored)

`imap-proto` 0.10.2 with Picacho's patches 0001–0003 from
[Picacho-Patches](https://github.com/pinaltech/Picacho-Patches/tree/HEAD/imap-proto)
(Gmail ids and labels, unmodeled untagged data as `Response::Other`, building
as a patched dependency), plus patch 0004, kept here until it moves there:

### 0004 Extension data

`src/parser/extensions.rs`:

- FETCH attributes: Gmail's `X-GM-THRID`, OBJECTID's `EMAILID` and
  `THREADID` (RFC 8474), `PREVIEW` (RFC 8970), `SAVEDATE` (RFC 8514), and
  BINARY's `BINARY[…]` and `BINARY.SIZE[…]` (RFC 3516), as new
  `AttributeValue` variants.
- Untagged `ESEARCH`, `SORT`, `THREAD` and `VANISHED (EARLIER)` are kept as
  `Response::Other` text.
- An unsolicited `VANISHED` (QRESYNC, RFC 7162) is reported as
  `Response::Expunge(0)`: `imap` 2.4 passes expunges on as unilateral data,
  where any other unknown response would end the command as if the server
  had hung up. Sequence number 0 never names a message.

License: MIT OR Apache-2.0 (`LICENSE-MIT`, `LICENSE-APACHE`).

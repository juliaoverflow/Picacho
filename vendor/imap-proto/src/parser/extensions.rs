//! FETCH attributes and untagged data from IMAP extensions that upstream
//! 0.10 doesn't parse. Picacho patch 0004; not part of upstream.
//!
//! - Gmail's thread id, `X-GM-THRID` (X-GM-EXT-1).
//! - RFC 8474 (OBJECTID): `EMAILID (id)` and `THREADID (id)` / `THREADID NIL`.
//! - RFC 8970 (PREVIEW): `PREVIEW "text"` / `PREVIEW NIL`.
//! - RFC 8514 (SAVEDATE): `SAVEDATE "date"` / `SAVEDATE NIL`.
//! - RFC 3516 (BINARY): `BINARY[1.2]<0> ~{n}` and `BINARY.SIZE[1.2] n`.
//! - RFC 7162 (QRESYNC): an unsolicited `VANISHED uids` is reported as
//!   `Response::Expunge(0)` (0 is never a sequence number), which `imap`
//!   treats as unilateral data rather than the server leaving. The earlier
//!   form, `VANISHED (EARLIER) uids`, answers a command and is kept as text.

// rustfmt doesn't do a very good job on nom parser invocations.
#![cfg_attr(rustfmt, rustfmt_skip)]

use std::str;

use crate::core::*;
use crate::body::section_part;
use crate::types::*;

named!(pub (crate) msg_att_gmail_thrid<AttributeValue>, do_parse!(
    tag_no_case!("X-GM-THRID ") >>
    num: number_64 >>
    (AttributeValue::GmailThrId(num))
));

// objectid = 1*255(ALPHA / DIGIT / "_" / "-")
fn objectid_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c == b'-'
}

named!(objectid<&str>, map_res!(take_while1!(objectid_char), str::from_utf8));

named!(pub (crate) msg_att_email_id<AttributeValue>, do_parse!(
    tag_no_case!("EMAILID ") >>
    id: paren_delimited!(objectid) >>
    (AttributeValue::EmailId(id))
));

named!(pub (crate) msg_att_thread_id<AttributeValue>, do_parse!(
    tag_no_case!("THREADID ") >>
    id: alt!(
        map!(nil, |_| None) |
        map!(paren_delimited!(objectid), |id| Some(id))
    ) >>
    (AttributeValue::ThreadId(id))
));

named!(pub (crate) msg_att_preview<AttributeValue>, do_parse!(
    tag_no_case!("PREVIEW ") >>
    // RFC 8970 drafts sent the algorithm first: `PREVIEW (FUZZY) "…"`.
    opt!(do_parse!(paren_delimited!(atom) >> tag!(" ") >> ())) >>
    text: nstring >>
    (AttributeValue::Preview(text))
));

named!(pub (crate) msg_att_save_date<AttributeValue>, do_parse!(
    tag_no_case!("SAVEDATE ") >>
    date: nstring_utf8 >>
    (AttributeValue::SaveDate(date))
));

// literal8 = "~{" number "}" CRLF *OCTET
named!(literal8<&[u8]>, do_parse!(
    opt!(tag!("~")) >>
    tag!("{") >>
    len: number >>
    tag!("}") >>
    tag!("\r\n") >>
    data: take!(len) >>
    (data)
));

named!(binary_section<Vec<u32>>, do_parse!(
    tag!("[") >>
    part: opt!(section_part) >>
    tag!("]") >>
    (part.unwrap_or_default())
));

named!(pub (crate) msg_att_binary<AttributeValue>, do_parse!(
    tag_no_case!("BINARY") >>
    section: binary_section >>
    index: opt!(do_parse!(
        tag!("<") >>
        num: number >>
        tag!(">") >>
        (num)
    )) >>
    tag!(" ") >>
    data: alt!(
        map!(nil, |_| None) |
        map!(literal8, |d| Some(d)) |
        map!(quoted, |d| Some(d))
    ) >>
    (AttributeValue::Binary { section, index, data })
));

named!(pub (crate) msg_att_binary_size<AttributeValue>, do_parse!(
    tag_no_case!("BINARY.SIZE") >>
    section: binary_section >>
    tag!(" ") >>
    size: number >>
    (AttributeValue::BinarySize { section, size })
));

// An unsolicited VANISHED (no EARLIER): reported as an expunge of
// sequence number 0, which `imap` passes on as unilateral data.
named!(pub (crate) resp_vanished_now<Response>, do_parse!(
    tag_no_case!("VANISHED ") >>
    not!(tag!("(")) >>
    take_while1!(|c: u8| c.is_ascii_digit() || c == b':' || c == b',') >>
    (Response::Expunge(0))
));

#[cfg(test)]
mod tests {
    use crate::parser::parse_response;
    use crate::types::*;

    #[test]
    fn extension_fetch_attributes() {
        let data = b"* 1 FETCH (UID 4 X-GM-THRID 1266894439832287888 EMAILID (M6d99ac3275bb4e) THREADID (T64b478a75b7ea9) PREVIEW \"Hello there\" SAVEDATE \"17-Jul-2026 02:44:25 -0700\")\r\n";
        let (rest, response) = parse_response(data).unwrap();
        assert!(rest.is_empty());
        let Response::Fetch(_, attributes) = response else { panic!("not a fetch") };
        assert!(attributes.contains(&AttributeValue::GmailThrId(1266894439832287888)));
        assert!(attributes.contains(&AttributeValue::EmailId("M6d99ac3275bb4e")));
        assert!(attributes.contains(&AttributeValue::ThreadId(Some("T64b478a75b7ea9"))));
        assert!(attributes.contains(&AttributeValue::Preview(Some(b"Hello there"))));
        let data = b"* 2 FETCH (UID 5 THREADID NIL PREVIEW NIL BINARY[1.2] ~{3}\r\nabc BINARY.SIZE[2] 1024)\r\n";
        let (_, response) = parse_response(data).unwrap();
        let Response::Fetch(_, attributes) = response else { panic!("not a fetch") };
        assert!(attributes.contains(&AttributeValue::ThreadId(None)));
        assert!(attributes.contains(&AttributeValue::Binary { section: vec![1, 2], index: None, data: Some(b"abc") }));
        assert!(attributes.contains(&AttributeValue::BinarySize { section: vec![2], size: 1024 }));
    }

    #[test]
    fn extension_untagged_data() {
        for line in [
            &b"* ESEARCH (TAG \"a1\") UID ALL 1:5,9\r\n"[..],
            b"* SORT 4 2 9\r\n",
            b"* THREAD (2)(3 6 (4 23)(44 7 96))\r\n",
            b"* VANISHED (EARLIER) 41,43:116\r\n",
        ] {
            let (_, response) = parse_response(line).unwrap();
            assert!(matches!(response, Response::Other(_)), "{response:?}");
        }
        let (_, response) = parse_response(b"* VANISHED 405,407\r\n").unwrap();
        assert_eq!(response, Response::Expunge(0));
    }
}

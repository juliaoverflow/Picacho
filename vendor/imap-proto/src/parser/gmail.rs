//! Gmail IMAP extensions (X-GM-EXT-1). Picacho patch; not part of upstream 0.10.
//!
//! https://developers.google.com/gmail/imap/imap-extensions

// rustfmt doesn't do a very good job on nom parser invocations.
#![cfg_attr(rustfmt, rustfmt_skip)]

use crate::core::{atom_char, astring_utf8, number_64};
use crate::types::*;

// A message's permanent identifier, shared by every label it appears under.
named!(pub (crate) msg_att_gmail_msgid<AttributeValue>, do_parse!(
    tag_no_case!("X-GM-MSGID ") >>
    num: number_64 >>
    (AttributeValue::GmailMsgId(num))
));

// A system label (`\\Inbox`, `\\Important`) or a label name.
named!(gmail_label<&str>, alt!(
    map_res!(recognize!(pair!(tag!("\\"), take_while!(atom_char))), std::str::from_utf8) |
    astring_utf8
));

// The labels on a message, like folders it is in.
named!(pub (crate) msg_att_gmail_labels<AttributeValue>, do_parse!(
    tag_no_case!("X-GM-LABELS ") >>
    labels: parenthesized_list!(gmail_label) >>
    (AttributeValue::GmailLabels(labels))
));

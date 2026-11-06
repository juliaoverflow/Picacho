//! Gmail IMAP extensions (X-GM-EXT-1). Picacho patch; not part of upstream 0.10.
//!
//! https://developers.google.com/gmail/imap/imap-extensions

// rustfmt doesn't do a very good job on nom parser invocations.
#![cfg_attr(rustfmt, rustfmt_skip)]

use crate::core::number_64;
use crate::types::*;

// A message's permanent identifier, shared by every label it appears under.
named!(pub (crate) msg_att_gmail_msgid<AttributeValue>, do_parse!(
    tag_no_case!("X-GM-MSGID ") >>
    num: number_64 >>
    (AttributeValue::GmailMsgId(num))
));

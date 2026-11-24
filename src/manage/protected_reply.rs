//! RFC 9788 §6.1: answering (replying to or forwarding) a message that
//! came encrypted must not put in the clear what its encryption protected.
//! That is its text, which a reply quotes, and any header fields it kept
//! inside, a hidden Subject above all. This module decides, for one
//! original, what the answer starts as and what it may still send in the
//! clear. It is crypto-neutral: the same rules cover OpenPGP and S/MIME.
//!
//! References and In-Reply-To are Structural Header Fields (RFC 9787
//! §1.1.2), which §6.1 lets go outside, so threading is unaffected.

/// What the original was, as far as answering it goes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Original {
    /// It came encrypted (PGP/MIME, S/MIME or inline PGP).
    pub encrypted: bool,
    /// Its subject was hidden: the outer Subject was obscured ("..."),
    /// and the real one was only inside (RFC 9788 §5.2.1 step 2).
    pub hidden_subject: bool,
}

impl Original {
    /// Reads `raw`, the original's source as stored. Encrypted mail is
    /// recognized from its structure (see `crypto::is_encrypted`), and a
    /// hidden subject from the outer Subject alone, which is all an
    /// unencrypted copy would show: "...", or no Subject at all on
    /// encrypted mail.
    pub fn of(raw: &[u8]) -> Self {
        let encrypted = crate::manage::crypto::is_encrypted(raw);
        let outer_subject = {
            use mailparse::MailHeaderMap as _;
            mailparse::parse_headers(raw)
                .ok()
                .and_then(|(h, _)| h.get_first_value("Subject"))
        };
        let hidden_subject = encrypted
            && outer_subject
                .as_deref()
                .is_none_or(|s| matches!(s.trim(), "" | "..." | "…" | "[...]"));
        Self {
            encrypted,
            hidden_subject,
        }
    }

    /// Whether answering it starts encrypted, and hides its subject
    /// (RFC 9788 §6.1: as protected as what it answers). An answer to
    /// unencrypted mail starts as any other message does.
    pub fn start(&self) -> Start {
        Start {
            encrypt: self.encrypted,
            hide_subject: self.hidden_subject,
        }
    }
}

/// How an answer starts out.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Start {
    pub encrypt: bool,
    pub hide_subject: bool,
}

/// What may go out in the clear when an answer to `original` is sent
/// without encryption after all (a recipient has no key, or the sender
/// turned Encrypt off).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cleartext {
    /// The Subject to send: "..." in place of a hidden one, else as written.
    pub subject: String,
    /// The body to send: what was written, without the quoted original.
    pub body: String,
}

/// The warning shown before such an answer is sent unencrypted.
pub const WARNING: &str = "This replies to an encrypted message. Sending it unencrypted would reveal its text and subject, so Picacho will leave out the quoted text and send the subject as \u{201c}...\u{201d}.";

/// RFC 9788 §6.1 for an answer sent in the clear: the hidden subject is
/// obscured, and the quote of the original is left out. `quote` is the
/// quoted part as the composer wrote it (see `quote_start`); `None` when
/// there is nothing to remove.
pub fn cleartext(original: &Original, subject: &str, body: &str) -> Cleartext {
    let subject = if original.hidden_subject {
        "...".to_owned()
    } else {
        subject.to_owned()
    };
    let body = match quote_start(body) {
        Some(at) => body[..at].trim_end().to_owned(),
        None => body.to_owned(),
    };
    Cleartext { subject, body }
}

/// Where the composer's quote of the original begins in `body`, if it
/// does: a reply's "On <date>, <name> wrote:" line followed by quoted
/// lines, or a forward's "---------- Forwarded message ----------".
/// Anything written below the quote (bottom-posting) is left out with it:
/// it is past the point where the original's text starts.
pub fn quote_start(body: &str) -> Option<usize> {
    if let Some(at) = body.find("\n---------- Forwarded message ----------") {
        return Some(at);
    }
    let mut offset = 0;
    let lines: Vec<&str> = body.split_inclusive('\n').collect();
    for (i, line) in lines.iter().enumerate() {
        let text = line.trim_end();
        let introduces = text.starts_with("On ") && text.ends_with(" wrote:");
        let quoted_next = lines
            .get(i + 1)
            .is_some_and(|next| next.starts_with('>'));
        if introduces && quoted_next {
            return Some(offset);
        }
        offset += line.len();
    }
    // A quote without its introduction (edited away): the first block of
    // quoted lines.
    let mut offset = 0;
    for line in &lines {
        if line.starts_with("> ") || line.trim_end() == ">" {
            return Some(offset);
        }
        offset += line.len();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const ENCRYPTED_HIDDEN: &[u8] = b"From: Ana <ana@example.org>\r\nTo: bo@example.se\r\nSubject: ...\r\nMessage-ID: <1@example.org>\r\nMIME-Version: 1.0\r\nContent-Type: multipart/encrypted; protocol=\"application/pgp-encrypted\"; boundary=b\r\n\r\n--b\r\nContent-Type: application/pgp-encrypted\r\n\r\nVersion: 1\r\n--b\r\nContent-Type: application/octet-stream\r\n\r\n-----BEGIN PGP MESSAGE-----\r\n\r\nwcBMA0\r\n-----END PGP MESSAGE-----\r\n--b--\r\n";
    const SMIME_SHOWN: &[u8] = b"From: Ana <ana@example.org>\r\nSubject: Plans\r\nMIME-Version: 1.0\r\nContent-Type: application/pkcs7-mime; smime-type=enveloped-data; name=\"smime.p7m\"\r\nContent-Transfer-Encoding: base64\r\n\r\nMIAGCSqGSIb3DQEHA6CAMIACAQAx\r\n";
    const PLAIN: &[u8] = b"From: Ana <ana@example.org>\r\nSubject: ...\r\nContent-Type: text/plain\r\n\r\nNot encrypted, just an odd subject.\r\n";

    #[test]
    fn originals_are_read_from_their_outside() {
        assert_eq!(Original::of(ENCRYPTED_HIDDEN), Original { encrypted: true, hidden_subject: true });
        assert_eq!(Original::of(SMIME_SHOWN), Original { encrypted: true, hidden_subject: false });
        // "..." on unencrypted mail is just a subject.
        assert_eq!(Original::of(PLAIN), Original::default());
        let no_subject = b"From: a@b\r\nContent-Type: multipart/encrypted; protocol=\"application/pgp-encrypted\"; boundary=b\r\n\r\n--b--\r\n";
        assert!(Original::of(no_subject).hidden_subject);
    }

    /// RFC 9788 §6.1: an answer starts as protected as what it answers.
    #[test]
    fn answers_start_as_protected_as_the_original() {
        assert_eq!(Original::of(ENCRYPTED_HIDDEN).start(), Start { encrypt: true, hide_subject: true });
        assert_eq!(Original::of(SMIME_SHOWN).start(), Start { encrypt: true, hide_subject: false });
        assert_eq!(Original::of(PLAIN).start(), Start::default());
    }

    #[test]
    fn cleartext_answers_lose_the_quote_and_a_hidden_subject() {
        let original = Original::of(ENCRYPTED_HIDDEN);
        let body = "Sounds good.\n\n-- \nBo\n\nOn Mon, 23 Nov 2026, Ana wrote:\n> Meet at noon.\n> At the usual place.";
        let sent = cleartext(&original, "Re: Plans", body);
        assert_eq!(sent.subject, "...");
        assert_eq!(sent.body, "Sounds good.\n\n-- \nBo");
        // A forward: the forwarded part goes too.
        let forward = "FYI\n\n---------- Forwarded message ----------\nFrom: Ana\nSubject: Plans\n\nMeet at noon.";
        let sent = cleartext(&original, "Fwd: Plans", forward);
        assert_eq!((sent.subject.as_str(), sent.body.as_str()), ("...", "FYI"));
        // A subject that wasn't hidden stays; there is nothing to remove.
        let sent = cleartext(&Original::of(SMIME_SHOWN), "Re: Plans", "Thanks!");
        assert_eq!((sent.subject.as_str(), sent.body.as_str()), ("Re: Plans", "Thanks!"));
    }

    #[test]
    fn quotes_are_found_with_or_without_their_introduction() {
        assert_eq!(quote_start("Hi\n\nOn Mon, Ana wrote:\n> x"), Some(4));
        assert_eq!(quote_start("Hi\n> x\nmore"), Some(3));
        assert_eq!(quote_start("On my way, I wrote:\nno quote follows"), None);
        assert_eq!(quote_start("Just text."), None);
    }
}

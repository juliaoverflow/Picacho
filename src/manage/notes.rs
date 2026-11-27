//! Notes Picacho keeps for itself in the account's mailbox, for its other
//! computers and phones to read: one message per kind of note, in Picacho's
//! own folder (made the first time there is something to keep). It is how
//! accounts without IMAP METADATA share what they can't annotate: the
//! Gmail and Microsoft APIs, Exchange, JMAP, Proton, and IMAP servers
//! without METADATA.
//!
//! A note is an ordinary, read message from the account to itself, titled
//! so a person seeing it knows what it is. It is marked with an
//! `X-Picacho-Note` header naming its kind, and its text holds the data
//! between two marker lines. Writing puts a new note in place and then
//! removes the ones read before it, so a note is never missing; a reader
//! finding two (from two computers at once) merges them.
use super::*;

/// The header naming a note's kind.
pub(crate) const NOTE_HEADER: &str = "X-Picacho-Note";
const BEGIN: &str = "-----BEGIN PICACHO NOTE-----";
const END: &str = "-----END PICACHO NOTE-----";
/// The messages that may be notes, by id there, with their sources;
/// `None` where the account has no Picacho folder.
pub(crate) type NoteSources = Option<Vec<(String, Vec<u8>)>>;
/// The most messages of Picacho's folder looked through for notes.
const MOST: usize = 50;

/// What a note's title says, by kind.
fn title(kind: &str) -> String {
    match kind {
        "follow-ups" => "Picacho: follow-up reminders".to_owned(),
        "snoozed" => "Picacho: snoozed messages".to_owned(),
        other => format!("Picacho: {other}"),
    }
}

/// JSON text as ASCII alone, other characters as `\u` escapes (in pairs
/// past U+FFFF): the same JSON, which no service's change of character set
/// (one rebuilding the note from its parts) can spoil.
fn ascii_json(data: &str) -> String {
    let mut out = String::with_capacity(data.len());
    for c in data.chars() {
        if c.is_ascii() {
            out.push(c);
        } else {
            let mut units = [0u16; 2];
            for u in c.encode_utf16(&mut units) {
                out.push_str(&format!("\\u{:04x}", u));
            }
        }
    }
    out
}

/// A note of `kind` holding `data` (JSON), from and to `email`.
pub(crate) fn note_message(kind: &str, email: &str, data: &str) -> Vec<u8> {
    use base64::Engine as _;
    let data = ascii_json(data);
    let text = format!(
        "Picacho keeps this message so that your other computers and phones \
         know your {what}. It changes as they do; leave it here, or they \
         forget them.\r\n\r\n{BEGIN}\r\n{data}\r\n{END}\r\n",
        what = match kind {
            "follow-ups" => "follow-up reminders",
            "snoozed" => "snoozed messages",
            other => other,
        }
    );
    let encoded = base64::engine::general_purpose::STANDARD.encode(text.as_bytes());
    let mut body = String::new();
    for chunk in encoded.as_bytes().chunks(76) {
        body.push_str(std::str::from_utf8(chunk).unwrap_or_default());
        body.push_str("\r\n");
    }
    let domain = email.rsplit_once('@').map_or("picacho.invalid", |(_, d)| d);
    format!(
        "From: Picacho <{email}>\r\nTo: <{email}>\r\nSubject: {}\r\nDate: {}\r\nMessage-ID: <note-{}@{domain}>\r\n{NOTE_HEADER}: {kind}\r\nAuto-Submitted: auto-generated\r\nMIME-Version: 1.0\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Transfer-Encoding: base64\r\n\r\n{body}",
        title(kind),
        chrono::Utc::now().to_rfc2822(),
        uuid::Uuid::new_v4(),
    )
    .into_bytes()
}

/// The data of a note of `kind`, if `raw` is one: marked as one, or,
/// where the service rebuilt it from its parts and dropped the mark
/// (Microsoft does), titled as one.
pub(crate) fn note_data(raw: &[u8], kind: &str) -> Option<String> {
    use mailparse::MailHeaderMap;
    let parsed = mailparse::parse_mail(raw).ok()?;
    match parsed.headers.get_first_value(NOTE_HEADER) {
        Some(marked) if !marked.trim().eq_ignore_ascii_case(kind) => return None,
        Some(_) => {}
        None if titled(&parsed.headers.get_first_value("Subject").unwrap_or_default(), kind) => {}
        None => return None,
    }
    let text = parsed.get_body().ok()?;
    let start = text.find(BEGIN)? + BEGIN.len();
    let end = text[start..].find(END)? + start;
    Some(text[start..end].trim().to_owned())
}

/// Whether a message's subject is a note's of `kind`: only those are
/// downloaded to be read.
pub(crate) fn titled(subject: &str, kind: &str) -> bool {
    subject.trim() == title(kind)
}

impl Backend {
    /// The account's notes of `kind` (each one's id there, and its data);
    /// `None` when it has no Picacho folder yet (nothing was ever kept).
    pub(crate) fn read_notes(&self, a: &Account, kind: &str) -> Result<Option<Vec<(String, String)>>> {
        let sources = if a.imap() {
            self.read_notes_imap(a, kind)?
        } else if a.gmail_api() {
            self.read_notes_gmail(a, kind)?
        } else if a.graph() {
            self.read_notes_graph(a, kind)?
        } else if a.ews() {
            self.read_notes_ews(a, kind)?
        } else if a.provider == Provider::Jmap {
            self.read_notes_jmap(a, kind)?
        } else if a.provider == Provider::Proton {
            self.read_notes_proton(a, kind)?
        } else {
            return Ok(None);
        };
        Ok(sources.map(|sources| {
            sources
                .into_iter()
                .filter_map(|(id, raw)| note_data(&raw, kind).map(|d| (id, d)))
                .collect()
        }))
    }
    /// Keeps `data` as the account's note of `kind`, in Picacho's folder
    /// (made if missing), then removes the notes `old` (by id, as read).
    pub(crate) fn write_note(&self, a: &Account, kind: &str, data: &str, old: &[String]) -> Result<()> {
        let raw = note_message(kind, &a.email, data);
        if a.imap() {
            self.write_note_imap(a, &raw, old)
        } else if a.gmail_api() {
            self.write_note_gmail(a, &raw, old)
        } else if a.graph() {
            self.write_note_graph(a, &raw, old)
        } else if a.ews() {
            self.write_note_ews(a, &raw, old)
        } else if a.provider == Provider::Jmap {
            self.write_note_jmap(a, &raw, old)
        } else if a.provider == Provider::Proton {
            self.write_note_proton(a, &raw, old)
        } else {
            Ok(())
        }
    }

    /// IMAP: the notes in Picacho's folder, by UID.
    fn read_notes_imap(&self, a: &Account, kind: &str) -> Result<NoteSources> {
        self.with_imap(a, |c| {
            let Some(folder) = self.picacho_folder(c, a, false)? else {
                return Ok(None);
            };
            c.select(&folder)?;
            let marked = format!("HEADER {NOTE_HEADER} {}", quote(kind)?);
            let mut uids: Vec<u32> = match c.uid_search(&marked) {
                Ok(found) => found.into_iter().collect(),
                // A server that can't search headers: the newest are read.
                Err(imap::error::Error::No(_) | imap::error::Error::Bad(_)) => {
                    c.uid_search("ALL")?.into_iter().collect()
                }
                Err(e) => return Err(e.into()),
            };
            uids.sort_unstable();
            let uids: Vec<u32> = uids.into_iter().rev().take(MOST).collect();
            if uids.is_empty() {
                return Ok(Some(Vec::new()));
            }
            let fetched = c.session.uid_fetch(uid_ranges(&uids), "(UID BODY.PEEK[])")?;
            Ok(Some(
                fetched
                    .iter()
                    .filter_map(|f| Some((format!("{folder}\u{1f}{}", f.uid?), f.body()?.to_vec())))
                    .collect(),
            ))
        })
    }
    /// IMAP: appends the note (read) to Picacho's folder, then erases `old`.
    fn write_note_imap(&self, a: &Account, raw: &[u8], old: &[String]) -> Result<()> {
        self.with_imap(a, |c| {
            let folder = self
                .picacho_folder(c, a, true)?
                .with_context(|| t!("manage-snooze-the-server-didnt-keep-the-new"))?;
            let wire = c.wire(&folder);
            c.append_heard(&wire, raw, &[imap::types::Flag::Seen])?;
            let uids: Vec<u32> = old
                .iter()
                .filter_map(|id| id.split_once('\u{1f}'))
                .filter(|(f, _)| *f == folder)
                .filter_map(|(_, uid)| uid.parse().ok())
                .collect();
            if !uids.is_empty() {
                c.select(&folder)?;
                c.require_writable(t!("manage-snooze-move-snoozed-mail-back"))?;
                c.erase(&uids)?;
            }
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_note_reads_back_and_only_as_its_kind() {
        let data = r#"{"waiting":[{"message_id":"<a@b>","subject":"Ünïcode “quotes” 🎉","sent":1,"due":2}],"gone":[]}"#;
        let raw = note_message("follow-ups", "me@example.com", data);
        // The same JSON, written in ASCII.
        let read = note_data(&raw, "follow-ups").unwrap();
        assert!(read.is_ascii());
        let back: serde_json::Value = serde_json::from_str(&read).unwrap();
        assert_eq!(back, serde_json::from_str::<serde_json::Value>(data).unwrap());
        let data = read.as_str();
        assert_eq!(note_data(&raw, "snoozed"), None);
        let parsed = mailparse::parse_mail(&raw).unwrap();
        use mailparse::MailHeaderMap;
        let subject = parsed.headers.get_first_value("Subject").unwrap();
        assert!(titled(&subject, "follow-ups"));
        // Its own sender, and marked automatic: never taken for a reply.
        assert_eq!(parsed.headers.get_first_value("Auto-Submitted").as_deref(), Some("auto-generated"));
        // Ordinary mail isn't a note.
        assert_eq!(note_data(b"Subject: Picacho: follow-up reminders\r\n\r\nhello", "follow-ups"), None);
        // Rebuilt from its parts, without its mark: still read, by its title.
        let rebuilt = format!("From: me@example.com\r\nSubject: Picacho: follow-up reminders\r\n\r\nText.\r\n\r\n{BEGIN}\r\n{data}\r\n{END}\r\n");
        assert_eq!(note_data(rebuilt.as_bytes(), "follow-ups").as_deref(), Some(data));
        let other = rebuilt.replace("follow-up reminders", "something else");
        assert_eq!(note_data(other.as_bytes(), "follow-ups"), None);
    }
}

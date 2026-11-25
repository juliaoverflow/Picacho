//! Moving mail in and out of Picacho as files: a mailbox exported as one
//! mbox file (mboxrd, as Thunderbird, Apple Mail and mutt read), and mbox or
//! `.eml` files imported into one of an account's mailboxes on its server.
//!
//! Exported messages are the readable ones: Pinal Mail's are decrypted here
//! first, so the file opens in any other app. Imports go through each
//! protocol's own way of adding a message (IMAP APPEND, JMAP Email/import,
//! Proton's import), so they are kept on the server like any other mail; a
//! POP3 account keeps them on this computer.
use super::*;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;

/// Messages larger than this are left out of an import.
const MAX_IMPORT: usize = 40 * 1024 * 1024;

/// How an export or import went.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Moved {
    /// Messages written or added.
    pub done: usize,
    /// Messages left out: unavailable offline, too large, or refused.
    pub skipped: usize,
    /// Of those imported, how many the mbox file marked deleted (Status
    /// D): imported anyway, without the mark, as their app hadn't removed
    /// them.
    pub marked_deleted: usize,
}

/// What was done with the messages, for `Moved::notice`.
#[derive(Clone, Copy, Debug)]
pub enum Done<'a> {
    Exported,
    Imported,
    /// Moved to another account, named.
    MovedTo(&'a str),
}

impl Moved {
    /// A notice for the user, such as "Exported 12 messages".
    pub fn notice(&self, done: Done) -> String {
        let mut text = match done {
            Done::Exported => t!("manage-archive-exported", count = self.done),
            Done::Imported => t!("manage-archive-imported", count = self.done),
            Done::MovedTo(name) => t!("manage-archive-moved-to", count = self.done, name = name),
        };
        // More to say, joined as the language joins such clauses.
        let more = |text: String, more: String| t!("manage-archive-and", notice = text, more = more);
        if self.skipped > 0 {
            text = more(text, t!("manage-archive-skipped", count = self.skipped));
        }
        if self.marked_deleted > 0 {
            text = more(text, t!("manage-archive-marked-deleted", count = self.marked_deleted));
        }
        text
    }
}

/// Writes one message in mboxrd form: a `From ` line, the message with
/// lines starting `From ` (after any `>`s) quoted with one more `>`, and a
/// blank line.
pub fn write_mbox_message(
    out: &mut impl Write,
    raw: &[u8],
    sender: &str,
    date: i64,
) -> std::io::Result<()> {
    let sender = sender.trim();
    let sender = if sender.is_empty() || sender.contains(char::is_whitespace) {
        "MAILER-DAEMON"
    } else {
        sender
    };
    let when = chrono::DateTime::from_timestamp(date, 0)
        .unwrap_or_default()
        .format("%a %b %e %H:%M:%S %Y");
    writeln!(out, "From {sender} {when}")?;
    let mut lines = raw.split(|&b| b == b'\n').peekable();
    while let Some(line) = lines.next() {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        // The split leaves an empty piece after a final newline.
        if line.is_empty() && lines.peek().is_none() {
            break;
        }
        let quotes = line.iter().take_while(|&&b| b == b'>').count();
        if line[quotes..].starts_with(b"From ") {
            out.write_all(b">")?;
        }
        out.write_all(line)?;
        out.write_all(b"\n")?;
    }
    out.write_all(b"\n")
}

/// A message read from an mbox file, with what the file says about it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MboxMessage {
    /// The message as it was sent, CRLF line ends.
    pub raw: Vec<u8>,
    /// When its `From ` line says it arrived (asctime, UTC unless the line
    /// gives a zone), for messages without a usable Date header.
    pub received: Option<i64>,
}

/// What mail apps note about a message in mbox's Status and X-Status
/// headers (RFC 4155 §5's practice, as mutt, Thunderbird and Pine write
/// them): R read, O old (seen as new, not read), A answered, F flagged,
/// D deleted, T draft.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MboxStatus {
    /// Neither header was there: nothing is known.
    pub known: bool,
    pub read: bool,
    pub answered: bool,
    pub flagged: bool,
    pub deleted: bool,
    pub draft: bool,
}
impl MboxStatus {
    /// From a message's headers.
    pub fn of(raw: &[u8]) -> Self {
        use mailparse::MailHeaderMap as _;
        let Ok((headers, _)) = mailparse::parse_headers(raw) else {
            return Self::default();
        };
        let status = headers.get_first_value("Status");
        let x_status = headers.get_first_value("X-Status");
        if status.is_none() && x_status.is_none() {
            return Self::default();
        }
        let letters: String = [status, x_status].into_iter().flatten().collect();
        let has = |c: char| letters.contains(c);
        Self {
            known: true,
            read: has('R'),
            answered: has('A'),
            flagged: has('F'),
            deleted: has('D'),
            draft: has('T'),
        }
    }
}

/// Splits an mbox file (mboxrd or mboxo, or mboxcl2 where Content-Length
/// says where each message ends) into its messages, with CRLF line ends as
/// mail is sent. Quoting is undone where mboxrd added it. A message larger
/// than an import may be comes as `None`, without its text.
pub fn read_mbox(
    input: impl Read,
    mut each: impl FnMut(Option<MboxMessage>) -> Result<()>,
) -> Result<()> {
    let mut reader = BufReader::new(input);
    let mut lines = MboxLines::default();
    let mut current: Option<Reading> = None;
    loop {
        let Some(line) = lines.next(&mut reader)? else {
            break;
        };
        let text = line
            .strip_suffix(b"\n")
            .map(|l| l.strip_suffix(b"\r").unwrap_or(l))
            .unwrap_or(&line)
            .to_vec();
        if is_separator(&text) && (lines.blank_before || current.is_none()) {
            if let Some(message) = current.take() {
                message.finish(&mut each)?;
            }
            current = Some(Reading::new(separator_date(&text)));
            lines.blank_before = false;
            continue;
        }
        lines.blank_before = text.is_empty();
        let Some(message) = current.as_mut() else {
            continue;
        };
        if message.too_large {
            continue;
        }
        if message.in_headers {
            if text.is_empty() {
                message.in_headers = false;
                message.push(&text);
                // Content-Length (mboxcl2): the body is exactly that many
                // bytes, unquoted, where it ends at a message boundary.
                if let Some(n) = message.content_length
                    && let Some(body) = lines.exact_body(&mut reader, n)?
                {
                    message.push_body(&body);
                    if let Some(done) = current.take() {
                        done.finish(&mut each)?;
                    }
                    lines.blank_before = true;
                }
                continue;
            }
            if let Some(value) = header_value(&text, b"Content-Length") {
                message.content_length = std::str::from_utf8(value)
                    .ok()
                    .and_then(|v| v.trim().parse::<usize>().ok())
                    .filter(|n| *n <= MAX_IMPORT);
            }
            message.push(&text);
            continue;
        }
        // Un-quoting (RFC 4155 §2): mboxrd adds one `>` to every line that
        // starts with `>`s then `From `; mboxo only to `From ` itself. A
        // file can't be told to be one or the other: the only telling
        // lines, `>>From `, mean mboxrd if any are there, and without them
        // both decode the same. So one `>` comes off every such line, which
        // is right for mboxrd and for all but those lines of mboxo.
        let quotes = text.iter().take_while(|&&b| b == b'>').count();
        let text = if quotes > 0 && text[quotes..].starts_with(b"From ") {
            &text[1..]
        } else {
            &text[..]
        };
        message.push(text);
    }
    if let Some(message) = current {
        message.finish(&mut each)?;
    }
    Ok(())
}

/// A message being read from an mbox file.
struct Reading {
    message: Vec<u8>,
    received: Option<i64>,
    /// It outgrew the limit: its text is dropped, not padded out to say so.
    too_large: bool,
    in_headers: bool,
    content_length: Option<usize>,
}
impl Reading {
    fn new(received: Option<i64>) -> Self {
        Self {
            message: Vec::new(),
            received,
            too_large: false,
            in_headers: true,
            content_length: None,
        }
    }
    /// Adds a line (without its line end), ended with CRLF.
    fn push(&mut self, text: &[u8]) {
        if self.message.len() + text.len() <= MAX_IMPORT + 2 {
            self.message.extend_from_slice(text);
            self.message.extend_from_slice(b"\r\n");
        } else {
            self.message = Vec::new();
            self.too_large = true;
        }
    }
    /// Adds a body taken whole, its bare line feeds made CRLF.
    fn push_body(&mut self, body: &[u8]) {
        let body = crlf(body);
        if self.message.len() + body.len() <= MAX_IMPORT + 2 {
            self.message.extend_from_slice(&body);
        } else {
            self.message = Vec::new();
            self.too_large = true;
        }
    }
    fn finish(self, each: &mut dyn FnMut(Option<MboxMessage>) -> Result<()>) -> Result<()> {
        if self.too_large {
            return each(None);
        }
        // The blank line separating messages isn't part of the message.
        let mut message = self.message;
        if message.ends_with(b"\r\n\r\n") {
            message.truncate(message.len() - 2);
        }
        if !message.iter().all(u8::is_ascii_whitespace) {
            each(Some(MboxMessage {
                raw: message,
                received: self.received,
            }))?;
        }
        Ok(())
    }
}

/// An mbox file's lines, with what one line ahead says.
#[derive(Default)]
struct MboxLines {
    line: Vec<u8>,
    /// Bytes read ahead (for a Content-Length check) that didn't fit.
    ahead: Vec<u8>,
    blank_before: bool,
}
impl MboxLines {
    /// The next line, its line end included; `None` at the end.
    fn next(&mut self, reader: &mut impl BufRead) -> Result<Option<Vec<u8>>> {
        // A very long line's room isn't kept for the short ones after it.
        if self.line.capacity() > 1024 * 1024 {
            self.line = Vec::new();
        }
        self.line.clear();
        if !self.ahead.is_empty() {
            match memchr::memchr(b'\n', &self.ahead) {
                Some(at) => {
                    let rest = self.ahead.split_off(at + 1);
                    let line = std::mem::replace(&mut self.ahead, rest);
                    return Ok(Some(line));
                }
                None => self.line = std::mem::take(&mut self.ahead),
            }
        }
        // A line is read up to the import limit: a file without line breaks
        // (not mail) isn't held in memory whole.
        let limit = (MAX_IMPORT as u64 + 1).saturating_sub(self.line.len() as u64);
        reader.take(limit).read_until(b'\n', &mut self.line)?;
        Ok((!self.line.is_empty()).then(|| self.line.clone()))
    }
    /// The next `n` bytes as a message's body, if they end where a message
    /// does: at the end of the file, or followed by a blank line or
    /// directly by a `From ` separator. Otherwise they are left to be read
    /// as lines, and `None` says Content-Length can't be trusted.
    fn exact_body(&mut self, reader: &mut impl BufRead, n: usize) -> Result<Option<Vec<u8>>> {
        let mut body = std::mem::take(&mut self.ahead);
        if body.len() < n {
            let want = (n - body.len()) as u64;
            reader.take(want).read_to_end(&mut body)?;
        }
        if body.len() < n {
            // The file ends sooner than it says.
            self.ahead = body;
            return Ok(None);
        }
        let mut rest = body.split_off(n);
        // What follows: up to two lines, enough to see a blank line and a
        // separator.
        while rest.iter().filter(|b| **b == b'\n').count() < 2 {
            let before = rest.len();
            reader.take(4096).read_until(b'\n', &mut rest)?;
            if rest.len() == before {
                break;
            }
        }
        let after = rest.strip_prefix(b"\r\n").or_else(|| rest.strip_prefix(b"\n"));
        let next_line = |bytes: &[u8]| {
            let end = memchr::memchr(b'\n', bytes).unwrap_or(bytes.len());
            let line = &bytes[..end];
            is_separator(line.strip_suffix(b"\r").unwrap_or(line))
        };
        let ends = rest.is_empty()
            || after.is_some_and(|a| a.is_empty() || next_line(a))
            || next_line(&rest);
        if ends {
            self.ahead = rest;
            Ok(Some(body))
        } else {
            body.extend_from_slice(&rest);
            self.ahead = body;
            Ok(None)
        }
    }
}

/// The value of header `name` on `line`, if the line is that header.
fn header_value<'a>(line: &'a [u8], name: &[u8]) -> Option<&'a [u8]> {
    let (key, value) = line.split_at(line.iter().position(|b| *b == b':')?);
    key.eq_ignore_ascii_case(name).then(|| &value[1..])
}

/// The time in a `From ` line's asctime date (`Wed Nov 23 10:00:00 2026`,
/// perhaps with a zone such as `+0100` or `-0500` after it), as UTC
/// seconds; without a zone it is taken as UTC.
fn separator_date(line: &[u8]) -> Option<i64> {
    let text = String::from_utf8_lossy(line.strip_prefix(b"From ")?);
    let words: Vec<&str> = text.split_whitespace().skip(1).collect();
    let year_at = words
        .iter()
        .position(|w| w.len() == 4 && w.bytes().all(|b| b.is_ascii_digit()))?;
    if year_at < 3 {
        return None;
    }
    // The weekday isn't read: some writers get it wrong, and it says
    // nothing the rest doesn't.
    let date = words[year_at - 3..=year_at].join(" ");
    let naive = chrono::NaiveDateTime::parse_from_str(&date, "%b %e %H:%M:%S %Y").ok()?;
    // A numeric zone after the year, where the writer gave one.
    let offset = words
        .get(year_at + 1)
        .and_then(|z| {
            chrono::DateTime::parse_from_str(&format!("2000-01-01 00:00:00 {z}"), "%Y-%m-%d %H:%M:%S %z").ok()
        })
        .map(|d| d.offset().local_minus_utc())
        .unwrap_or(0);
    Some(naive.and_utc().timestamp() - i64::from(offset))
}

/// Whether a line is an mbox separator (RFC 4155 §2: `From <sender>
/// <date>`): a `From ` line naming a sender and ending in a year, not text
/// that happens to start with "From " after a blank line (an mboxo file's
/// unquoted "From here on…").
fn is_separator(line: &[u8]) -> bool {
    let Some(rest) = line.strip_prefix(b"From ") else {
        return false;
    };
    let text = String::from_utf8_lossy(rest);
    let mut words = text.split_whitespace();
    let Some(sender) = words.next() else {
        return false;
    };
    let date: Vec<&str> = words.collect();
    // asctime: "Wed Nov 23 10:00:00 2026", perhaps with a zone after.
    !sender.is_empty()
        && date.len() >= 4
        && date
            .iter()
            .any(|w| w.len() == 4 && w.bytes().all(|b| b.is_ascii_digit()))
        && date.iter().any(|w| w.matches(':').count() >= 1)
}

/// Whether a file looks like an mbox (it starts with a `From ` line).
fn is_mbox(path: &Path) -> Result<bool> {
    let mut start = [0u8; 5];
    let mut file = std::fs::File::open(path)?;
    let read = file.read(&mut start)?;
    Ok(read == 5 && &start == b"From ")
}

impl Backend {
    /// Writes every message of `mailbox` to `path` as one mbox file, oldest
    /// first. Messages only known by their headers are downloaded first.
    pub fn export_mailbox(
        &self,
        a: &Account,
        mailbox: &Mailbox,
        path: &Path,
        cancel: &AtomicBool,
    ) -> Result<Moved> {
        // Without bodies: each message's source is read on its own below,
        // so the whole mailbox's text is never held at once.
        let mut messages = self.store.summaries(&a.id, mailbox.clone())?;
        messages.sort_by_key(|m| m.date);
        // Written whole, then named, so a half-written file never stands in
        // for one. The partial file is new (never an existing file, or one a
        // link points to), readable by you alone, and removed however the
        // export ends short of naming it.
        let partial = path.with_extension(format!("{}.partial", uuid::Uuid::new_v4().simple()));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let file = options.open(&partial)?;
        struct Partial<'a>(&'a Path, bool);
        impl Drop for Partial<'_> {
            fn drop(&mut self) {
                if !self.1 {
                    let _ = std::fs::remove_file(self.0);
                }
            }
        }
        let mut cleanup = Partial(&partial, false);
        let mut out = std::io::BufWriter::new(file);
        let mut moved = Moved::default();
        for m in &messages {
            if cancel.load(Ordering::Relaxed) {
                drop(out);
                bail!(t!("manage-archive-the-export-was-stopped"));
            }
            let raw = (|| -> Result<Vec<u8>> {
                let raw = self.original(a, m)?;
                let lock = self.account_lock(&a.id);
                let _operation = lock.lock().unwrap();
                Ok(self.plaintext(a, &raw)?.1)
            })();
            match raw {
                Ok(raw) => {
                    write_mbox_message(&mut out, &raw, &m.from, m.date)?;
                    moved.done += 1;
                }
                Err(_) => moved.skipped += 1,
            }
        }
        out.into_inner().map_err(|e| e.into_error())?.sync_all()?;
        std::fs::rename(&partial, path)?;
        cleanup.1 = true;
        Ok(moved)
    }

    /// Adds the messages in `paths` (mbox files, or single `.eml` messages)
    /// to `mailbox` on the account's server, read.
    pub fn import_messages(
        &self,
        a: &Account,
        mailbox: &Mailbox,
        paths: &[std::path::PathBuf],
        cancel: &AtomicBool,
    ) -> Result<Moved> {
        ensure!(
            !mailbox.folder().is_virtual() && mailbox.folder() != Folder::Drafts,
            t!("manage-archive-mail-cant-be-imported-into-this")
        );
        let mut moved = Moved::default();
        // IMAP servers with MULTIAPPEND take messages in batches, in one
        // command each, rather than one round trip per message.
        let batching = matches!(a.provider, Provider::Imap | Provider::Gmail | Provider::Outlook)
            && self
                .with_imap(a, |c| Ok(c.extensions()?.multiappend))
                .unwrap_or(false);
        let mut batch: Vec<Imported> = Vec::new();
        let flush = |batch: &mut Vec<Imported>, moved: &mut Moved| -> Result<()> {
            if batch.is_empty() {
                return Ok(());
            }
            let taken = std::mem::take(batch);
            if taken.len() > 1 && self.import_batch(a, mailbox, &taken).is_ok() {
                moved.done += taken.len();
                return Ok(());
            }
            // One at a time, so one the server refuses doesn't stop the rest.
            for raw in &taken {
                match self.import_one(a, mailbox, raw) {
                    Ok(()) => moved.done += 1,
                    Err(e) if moved.done == 0 && moved.skipped == 0 => return Err(e),
                    Err(_) => moved.skipped += 1,
                }
            }
            Ok(())
        };
        let mut add = |found: Option<MboxMessage>| -> Result<()> {
            if cancel.load(Ordering::Relaxed) {
                bail!(t!("manage-archive-the-import-was-stopped"));
            }
            let Some(found) = found else {
                moved.skipped += 1;
                return Ok(());
            };
            if found.raw.len() > MAX_IMPORT || mailparse::parse_headers(&found.raw).is_err() {
                moved.skipped += 1;
                return Ok(());
            }
            let raw = Imported::from(found);
            if raw.status.deleted {
                moved.marked_deleted += 1;
            }
            if batching {
                batch.push(raw);
                let bytes: usize = batch.iter().map(|m| m.raw.len()).sum();
                if batch.len() >= BATCH_MESSAGES || bytes >= BATCH_BYTES {
                    flush(&mut batch, &mut moved)?;
                }
                return Ok(());
            }
            match self.import_one(a, mailbox, &raw) {
                Ok(()) => moved.done += 1,
                Err(e) if moved.done == 0 && moved.skipped == 0 => return Err(e),
                Err(_) => moved.skipped += 1,
            }
            Ok(())
        };
        let mut too_large = 0;
        for path in paths {
            if is_mbox(path)? {
                read_mbox(std::fs::File::open(path)?, &mut add)?;
            } else {
                // Not read at all when it is larger than an import may be.
                if std::fs::metadata(path)?.len() > MAX_IMPORT as u64 {
                    too_large += 1;
                    continue;
                }
                let raw = std::fs::read(path)?;
                let raw = if crate::sync::msg::is_msg(&raw)
                    || path.extension().is_some_and(|e| e.eq_ignore_ascii_case("msg"))
                {
                    crate::sync::msg::to_eml(&raw)?
                } else {
                    raw
                };
                // Mail travels with CRLF; files saved elsewhere may use LF.
                add(Some(MboxMessage {
                    raw: crlf(&raw),
                    received: None,
                }))?;
            }
        }
        drop(add);
        flush(&mut batch, &mut moved)?;
        moved.skipped += too_large;
        Ok(moved)
    }

    fn import_one(&self, a: &Account, mailbox: &Mailbox, item: &Imported) -> Result<()> {
        let lock = self.account_lock(&a.id);
        let _operation = lock.lock().unwrap();
        let raw = &item.raw[..];
        match a.provider {
            // Gmail takes a message as it was, with the mailbox's label and
            // its flags as labels.
            _ if a.gmail_api() => self.with_gmail(a, |s| {
                use crate::sync::gmail::label;
                let labels = s.labels()?;
                let (place, _) = super::gmail::listing(&labels, mailbox, 0)?;
                let mut on: Vec<String> = place.into_iter().collect();
                if !item.read() {
                    on.push(label::UNREAD.into());
                }
                if item.status.flagged {
                    on.push(label::STARRED.into());
                }
                let on: Vec<&str> = on.iter().map(String::as_str).collect();
                s.insert(raw, &on).map(|_| ())
            }),
            // Made from its parts, as received mail (`graph::Session::import`).
            _ if a.ews() => self.import_ews(a, mailbox, raw, item.read()),
            _ if a.graph() => self.with_graph(a, |s| {
                let folders = s.folders()?;
                let folder = match super::graph::listing(&folders, mailbox)? {
                    super::graph::Listing::Folder(id) => id,
                    super::graph::Listing::Everywhere(_) => bail!(t!("manage-proton-choose-one-of-the-accounts-folders")),
                };
                s.import(raw, &folder, item.read(), item.status.flagged).map(|_| ())
            }),
            Provider::Jmap => self.with_jmap(a, |s| {
                let places = self.jmap_places(a, s, false)?;
                let target = super::jmap::mailbox_id(&places, mailbox)?;
                s.import(raw, &[&target], &item.keywords()).map(|_| ())
            }),
            Provider::Proton => self.with_proton(a, |s| {
                let label = super::proton::label_for(&s.folders(), mailbox)?;
                s.import(raw, &label, mailbox.folder() == Folder::Sent)
                    .map(|_| ())
            }),
            Provider::Pop3 => {
                let id = format!("{}:import:{}", a.id, uuid::Uuid::new_v4());
                let mut m = parse_message(raw, a, &id, mailbox.folder(), "", 0)?;
                item.apply(&mut m);
                self.store.save_messages(&[m])?;
                self.store.save_raw(&id, raw)
            }
            _ => self.with_imap(a, |c| {
                let remote = remote_name(c.names(false)?, a, mailbox)?;
                let wire = c.wire(&remote);
                c.append_dated(&wire, raw, &item.flags(), item.received)?;
                Ok(())
            }),
        }
    }

    /// The source of `m` to hand to another account or a draft: Pinal
    /// Mail's own encryption opened, as exports are, since nothing else
    /// can read it; any other message exactly as delivered.
    fn portable(&self, a: &Account, m: &Mail) -> Result<Vec<u8>> {
        let raw = self.original(a, m)?;
        if a.service != Service::Pinal {
            return Ok(raw);
        }
        let lock = self.account_lock(&a.id);
        let _operation = lock.lock().unwrap();
        Ok(self.plaintext(a, &raw)?.1)
    }

    /// Moves messages of account `from` into `mailbox` of account `to`:
    /// each is added there as imports are (IMAP APPEND, JMAP Email/import,
    /// Proton's import, this computer for POP3), keeping its date and its
    /// read, starred and answered marks, and once added is removed from
    /// `from`: deleted for good, or on Gmail (which deletes only from
    /// Trash) moved to its Trash. Starred adds starred mail to the inbox,
    /// All Mail to Archive; one that couldn't be added stays where it was.
    pub fn move_to_account(&self, from: &Account, mails: &[Mail], to: &Account, mailbox: &Mailbox) -> Result<Moved> {
        ensure!(from.id != to.id, t!("manage-archive-these-messages-are-already-in-this"));
        ensure!(
            mails.iter().all(|m| m.account == from.id),
            t!("manage-archive-message-belongs-to-another-account")
        );
        let (target, star) = match mailbox {
            Mailbox::Standard(Folder::Starred) => (Mailbox::Standard(Folder::Inbox), true),
            Mailbox::Standard(Folder::All) => (Mailbox::Standard(Folder::Archive), false),
            other => (other.clone(), false),
        };
        ensure!(
            !matches!(target, Mailbox::Standard(Folder::Drafts | Folder::Other)),
            t!("manage-archive-mail-cant-be-moved-into-this")
        );
        let mut moved = Moved::default();
        let mut added = Vec::new();
        let mut first_error = None;
        for m in mails {
            let result = self.portable(from, m).and_then(|raw| {
                let item = Imported::of_mail(m, crlf(&raw), star);
                self.import_one(to, &target, &item)
            });
            match result {
                Ok(()) => {
                    moved.done += 1;
                    added.push(m.clone());
                }
                Err(e) => {
                    moved.skipped += 1;
                    first_error.get_or_insert(e);
                }
            }
        }
        if let Some(e) = first_error
            && added.is_empty()
        {
            return Err(e);
        }
        if !added.is_empty() {
            if from.provider == Provider::Gmail {
                let (trash, elsewhere): (Vec<Mail>, Vec<Mail>) = added
                    .into_iter()
                    .partition(|m| matches!(m.folder, Folder::Trash | Folder::Junk));
                self.change_many(from, &elsewhere, Change::Move(Folder::Trash))?;
                self.delete_forever(from, &trash)?;
            } else {
                self.delete_forever(from, &added)?;
            }
        }
        Ok(moved)
    }

    /// Starts a new draft in account `to` from each of `mails` (of account
    /// `from`): its recipients, subject, text and attachments, to send
    /// again or as a new message. The messages themselves stay. Returns the
    /// new drafts' ids.
    pub fn copy_as_drafts(&self, from: &Account, mails: &[Mail], to: &Account) -> Result<Vec<String>> {
        let mut ids = Vec::new();
        let mut first_error = None;
        for m in mails {
            let id = uuid::Uuid::new_v4().to_string();
            let result = self.original(from, m).and_then(|raw| {
                // Pinal Mail's, signed or encrypted to you: the draft holds
                // what it says.
                let raw = {
                    let lock = self.account_lock(&from.id);
                    let _operation = lock.lock().unwrap();
                    self.plaintext(from, &raw).map(|(_, plain)| plain).unwrap_or(raw)
                };
                let mut d = self.draft_from(to, &id, &raw)?;
                // A new message, answering nothing.
                d.reply_id.clear();
                d.references.clear();
                d.bcc.clear();
                self.store.save_draft(&d)
            });
            match result {
                Ok(()) => ids.push(id),
                Err(e) => {
                    first_error.get_or_insert(e);
                }
            }
        }
        match first_error {
            Some(e) if ids.is_empty() => Err(e),
            _ => Ok(ids),
        }
    }

    /// Adds several messages to `mailbox` on an IMAP server in one command
    /// where it can (MULTIAPPEND): all are added, or none.
    fn import_batch(&self, a: &Account, mailbox: &Mailbox, raws: &[Imported]) -> Result<()> {
        let lock = self.account_lock(&a.id);
        let _operation = lock.lock().unwrap();
        self.with_imap(a, |c| {
            let remote = remote_name(c.names(false)?, a, mailbox)?;
            let wire = c.wire(&remote);
            let flags: Vec<Vec<imap::types::Flag<'static>>> = raws.iter().map(Imported::flags).collect();
            let items: Vec<(&[u8], &[imap::types::Flag<'_>], Option<i64>)> = raws
                .iter()
                .zip(&flags)
                .map(|(m, f)| (&m.raw[..], &f[..], m.received))
                .collect();
            let heard = c.append_many(&wire, &items)?;
            // Where they went (APPENDUID with a UID each): listed at once.
            if let [heard] = heard.as_slice()
                && let Some((validity, uids)) = crate::sync::transport::append_uids(heard)
                && uids.len() == raws.len()
            {
                let rows: Vec<Mail> = raws
                    .iter()
                    .zip(uids)
                    .filter_map(|(item, uid)| {
                        let id = format!("{}:{remote}:{validity}:{uid}", a.id);
                        let mut m = parse_message(&item.raw, a, &id, mailbox.folder(), &remote, uid).ok()?;
                        item.apply(&mut m);
                        Some(m)
                    })
                    .collect();
                self.store.save_messages(&rows)?;
            }
            Ok(())
        })
    }
}

/// A message to import, with the marks its mbox file gave it.
struct Imported {
    raw: Vec<u8>,
    status: MboxStatus,
    /// When it arrived, from its `From ` line, for an APPEND date where
    /// the message has no usable Date.
    received: Option<i64>,
}
impl From<MboxMessage> for Imported {
    fn from(m: MboxMessage) -> Self {
        Self {
            status: MboxStatus::of(&m.raw),
            raw: m.raw,
            received: m.received,
        }
    }
}
impl Imported {
    /// A message moving from another account, with its marks there.
    fn of_mail(m: &Mail, raw: Vec<u8>, star: bool) -> Self {
        Self {
            raw,
            status: MboxStatus {
                known: true,
                read: !m.unread,
                answered: m.answered,
                flagged: m.starred || star,
                deleted: false,
                draft: false,
            },
            received: (m.date > 0).then_some(m.date),
        }
    }
    /// Read where the file says so, or says nothing (as imports always
    /// were); unread where it says the message wasn't (Status without R).
    fn read(&self) -> bool {
        !self.status.known || self.status.read
    }
    /// APPEND's flags. `\Deleted` is never set: on the server it would
    /// let the next expunge remove mail the person kept.
    fn flags(&self) -> Vec<imap::types::Flag<'static>> {
        use imap::types::Flag;
        let s = self.status;
        [
            (self.read(), Flag::Seen),
            (s.answered, Flag::Answered),
            (s.flagged, Flag::Flagged),
            (s.draft, Flag::Draft),
        ]
        .into_iter()
        .filter_map(|(on, flag)| on.then_some(flag))
        .collect()
    }
    /// The same as JMAP keywords (RFC 8621 §4.1.1).
    fn keywords(&self) -> Vec<&'static str> {
        let s = self.status;
        [
            (self.read(), "$seen"),
            (s.answered, "$answered"),
            (s.flagged, "$flagged"),
            (s.draft, "$draft"),
        ]
        .into_iter()
        .filter_map(|(on, k)| on.then_some(k))
        .collect()
    }
    /// The same on a message cached here.
    fn apply(&self, m: &mut Mail) {
        m.unread = !self.read();
        m.answered = self.status.answered;
        m.starred = self.status.flagged;
    }
}

/// At most this many messages, or bytes, go in one MULTIAPPEND.
const BATCH_MESSAGES: usize = 50;
const BATCH_BYTES: usize = 4 * 1024 * 1024;

/// `raw` with every bare LF made CRLF.
fn crlf(raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(raw.len() + raw.len() / 40);
    let mut previous = 0u8;
    for &b in raw {
        if b == b'\n' && previous != b'\r' {
            out.push(b'\r');
        }
        out.push(b);
        previous = b;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn messages(mbox: &[u8]) -> Vec<Vec<u8>> {
        read(mbox).into_iter().map(|m| m.raw).collect()
    }
    fn read(mbox: &[u8]) -> Vec<MboxMessage> {
        let mut out = Vec::new();
        read_mbox(mbox, |m| {
            // A message too large to import, as an empty one.
            out.push(m.unwrap_or_default());
            Ok(())
        })
        .unwrap();
        out
    }

    #[test]
    fn mbox_round_trips_with_quoted_from_lines() {
        let first =
            b"From: a@example.org\r\nSubject: One\r\n\r\nHello\r\nFrom here on\r\n>From quoted\r\n"
                .to_vec();
        let second = b"From: b@example.org\r\nSubject: Two\r\n\r\nBye\r\n".to_vec();
        let mut file = Vec::new();
        write_mbox_message(&mut file, &first, "a@example.org", 1_700_000_000).unwrap();
        write_mbox_message(&mut file, &second, "", 1_700_000_100).unwrap();
        let text = String::from_utf8_lossy(&file);
        assert!(text.starts_with("From a@example.org Tue Nov 14 22:13:20 2023\n"));
        assert!(text.contains("\n>From here on\n>>From quoted\n"));
        assert!(text.contains("\nFrom MAILER-DAEMON "));
        assert_eq!(messages(&file), vec![first, second]);
    }

    #[test]
    fn from_lines_inside_a_paragraph_do_not_split_mboxo() {
        let file = b"From x Mon Jan  1 00:00:00 2024\nSubject: A\n\nline\nFrom inside\n\nFrom y Mon Jan  1 00:00:00 2024\nSubject: B\n\nb\n";
        let got = messages(file);
        assert_eq!(got.len(), 2);
        assert!(String::from_utf8_lossy(&got[0]).contains("line\r\nFrom inside\r\n"));
        assert!(String::from_utf8_lossy(&got[1]).starts_with("Subject: B\r\n"));
    }

    #[test]
    fn imported_mail_exports_again_unchanged() -> Result<()> {
        let path = std::env::temp_dir().join(format!("picacho-archive-{}", uuid::Uuid::new_v4()));
        let b = Backend::new(Arc::new(crate::manage::store::Store::open(&path)?));
        let mut a = Account::new(Provider::Pop3);
        a.email = "qa@example.com".into();
        let one = b"From: Ana <ana@example.org>\nSubject: Kept\nDate: Mon, 1 Jan 2024 10:00:00 +0000\nMessage-ID: <1@x>\n\nFrom the archive\n";
        let eml = path.join("one.eml");
        std::fs::write(&eml, one)?;
        let mut mbox = Vec::new();
        let two = b"From: Bo <bo@example.org>\r\nSubject: Second\r\nDate: Tue, 2 Jan 2024 10:00:00 +0000\r\n\r\nFrom a line\r\n";
        write_mbox_message(&mut mbox, two, "bo@example.org", 1_704_189_600)?;
        let mbox_path = path.join("two.mbox");
        std::fs::write(&mbox_path, &mbox)?;
        let stop = AtomicBool::new(false);
        let archive = Mailbox::Standard(Folder::Archive);
        let moved = b.import_messages(&a, &archive, &[eml, mbox_path], &stop)?;
        assert_eq!(
            moved,
            Moved {
                done: 2,
                skipped: 0,
                marked_deleted: 0,
            }
        );
        let listed = b.store.messages(&a.id, archive.clone(), 10)?;
        assert_eq!(listed.len(), 2);
        assert!(listed.iter().all(|m| !m.unread));
        let out = path.join("out.mbox");
        assert_eq!(b.export_mailbox(&a, &archive, &out, &stop)?.done, 2);
        let back = messages(&std::fs::read(&out)?);
        assert_eq!(back[0], crlf(one));
        assert_eq!(back[1], two);
        assert!(
            b.import_messages(&a, &Mailbox::Standard(Folder::Starred), &[], &stop)
                .is_err()
        );
        drop(b);
        std::fs::remove_dir_all(path)?;
        Ok(())
    }

    /// Mail dropped on another account's mailbox is added there with its
    /// marks and leaves its own; dropped on Drafts, it starts a draft and
    /// stays.
    #[test]
    fn mail_moves_to_another_account_and_starts_drafts() -> Result<()> {
        let path = std::env::temp_dir().join(format!("picacho-transfer-{}", uuid::Uuid::new_v4()));
        let b = Backend::new(Arc::new(crate::manage::store::Store::open(&path)?));
        let mut from = Account::new(Provider::Pop3);
        from.email = "one@example.com".into();
        let mut to = Account::new(Provider::Pop3);
        to.email = "two@example.com".into();
        assert_ne!(from.id, to.id);
        let eml = path.join("one.eml");
        std::fs::write(
            &eml,
            b"From: Ana <ana@example.org>\r\nTo: one@example.com\r\nSubject: Moving\r\nDate: Mon, 1 Jan 2024 10:00:00 +0000\r\nIn-Reply-To: <0@x>\r\n\r\nThe text\r\n",
        )?;
        let inbox = Mailbox::Standard(Folder::Inbox);
        b.import_messages(&from, &inbox, &[eml], &AtomicBool::new(false))?;
        let mut m = b.store.messages(&from.id, inbox.clone(), 10)?.remove(0);
        m.starred = true;
        m.unread = true;
        b.store.save_messages(&[m.clone()])?;

        let ids = b.copy_as_drafts(&from, &[m.clone()], &to)?;
        let d = b.store.draft(&ids[0])?.expect("the draft");
        assert_eq!((d.account.as_str(), d.subject.as_str()), (to.id.as_str(), "Moving"));
        assert!(d.body.contains("The text") && d.reply_id.is_empty());
        assert_eq!(b.store.messages(&from.id, inbox.clone(), 10)?.len(), 1, "the message stays");

        let archive = Mailbox::Standard(Folder::Archive);
        assert!(b.move_to_account(&from, &[m.clone()], &from, &archive).is_err());
        let moved = b.move_to_account(&from, &[m], &to, &archive)?;
        assert_eq!((moved.done, moved.skipped), (1, 0));
        assert!(b.store.messages(&from.id, inbox, 10)?.is_empty());
        let there = b.store.messages(&to.id, archive, 10)?;
        assert_eq!(there.len(), 1);
        assert!(there[0].starred && there[0].unread);
        assert_eq!(there[0].subject, "Moving");
        drop(b);
        std::fs::remove_dir_all(path)?;
        Ok(())
    }

    /// An export writes a new file of its own beside the target (never one
    /// already there) and leaves none behind when it stops.
    #[test]
    fn exports_leave_no_partial_files() -> Result<()> {
        let path = std::env::temp_dir().join(format!("picacho-export-{}", uuid::Uuid::new_v4()));
        let b = Backend::new(Arc::new(crate::manage::store::Store::open(&path)?));
        let a = Account::new(Provider::Pop3);
        let dir = path.join("out");
        std::fs::create_dir_all(&dir)?;
        // The name earlier versions wrote to, here someone else's file.
        let bystander = dir.join("mail.mbox.partial");
        std::fs::write(&bystander, b"keep me")?;
        let inbox = Mailbox::Standard(Folder::Inbox);
        b.export_mailbox(&a, &inbox, &dir.join("mail.mbox"), &AtomicBool::new(false))?;
        assert_eq!(std::fs::read(&bystander)?, b"keep me");
        let names = |dir: &Path| -> Result<Vec<String>> {
            let mut names: Vec<String> = std::fs::read_dir(dir)?
                .map(|e| Ok(e?.file_name().to_string_lossy().into_owned()))
                .collect::<Result<_>>()?;
            names.sort();
            Ok(names)
        };
        assert_eq!(names(&dir)?, ["mail.mbox", "mail.mbox.partial"]);
        // Into a directory that isn't there: it fails, leaving nothing.
        assert!(b.export_mailbox(&a, &inbox, &dir.join("gone/x.mbox"), &AtomicBool::new(false)).is_err());
        assert_eq!(names(&dir)?, ["mail.mbox", "mail.mbox.partial"]);
        drop(b);
        std::fs::remove_dir_all(path)?;
        Ok(())
    }

    /// RFC 4155 §2: `>>From ` loses one `>` (mboxrd's quoting; mboxo
    /// can't be told apart, see `read_mbox`), and the `From ` line's date
    /// is kept, in UTC unless it gives a zone.
    #[test]
    fn quoting_comes_off_once_and_dates_are_kept() {
        let file = b"From a@x Wed Nov 23 10:00:00 2026\nSubject: A\n\n>>From twice\n>From once\n\nFrom b@x Wed Nov 23 10:00:00 2026 +0100\nSubject: B\n\nb\n";
        let got = read(file);
        assert_eq!(got.len(), 2);
        assert!(String::from_utf8_lossy(&got[0].raw).contains("\r\n>From twice\r\nFrom once\r\n"));
        assert_eq!(got[0].received, Some(1_795_428_000));
        assert_eq!(got[1].received, Some(1_795_428_000 - 3600));
        assert_eq!(separator_date(b"From a@x Mon Jan  1 00:00:00 2024"), Some(1_704_067_200));
    }

    /// mboxcl2: a correct Content-Length is the body exactly, `From ` lines
    /// and all, unquoted; a wrong one is ignored for that message only.
    #[test]
    fn content_length_is_trusted_only_where_it_fits() {
        let body = "Hello\nFrom inside the body\n>From kept as is\n";
        let file = format!(
            "From a@x Wed Nov 23 10:00:00 2026\nSubject: A\nContent-Length: {}\n\n{body}\nFrom b@x Wed Nov 23 10:00:00 2026\nSubject: B\nContent-Length: 999\n\nshort\n\nFrom c@x Wed Nov 23 10:00:00 2026\nSubject: C\nContent-Length: 2\n\nlonger than two\n",
            body.len()
        );
        let got = messages(file.as_bytes());
        assert_eq!(got.len(), 3, "{got:?}");
        let first = String::from_utf8_lossy(&got[0]);
        assert!(first.ends_with("\r\n\r\nHello\r\nFrom inside the body\r\n>From kept as is\r\n"), "{first}");
        assert!(String::from_utf8_lossy(&got[1]).ends_with("\r\n\r\nshort\r\n"));
        assert!(String::from_utf8_lossy(&got[2]).ends_with("\r\n\r\nlonger than two\r\n"));
    }

    /// Status and X-Status give the flags an import sets; D is not one.
    #[test]
    fn status_headers_give_the_import_flags() {
        use imap::types::Flag;
        let item = |headers: &str| {
            Imported::from(MboxMessage {
                raw: format!("{headers}Subject: x\r\n\r\nx\r\n").into_bytes(),
                received: None,
            })
        };
        assert_eq!(item("").flags(), [Flag::Seen], "no Status: read, as before");
        assert!(item("Status: O\r\n").flags().is_empty(), "seen as new, not read");
        assert_eq!(
            item("Status: RO\r\nX-Status: AFD\r\n").flags(),
            [Flag::Seen, Flag::Answered, Flag::Flagged]
        );
        assert_eq!(item("Status: RO\r\nX-Status: T\r\n").keywords(), ["$seen", "$draft"]);
        let deleted = item("Status: RO\r\nX-Status: D\r\n");
        assert!(deleted.status.deleted && !deleted.flags().contains(&Flag::Deleted));
        let mut m = Mail::default();
        item("Status: O\r\nX-Status: F\r\n").apply(&mut m);
        assert!(m.unread && m.starred && !m.answered);
    }

    #[test]
    fn bare_line_feeds_become_crlf() {
        assert_eq!(crlf(b"a\nb\r\nc\n"), b"a\r\nb\r\nc\r\n");
    }

    #[test]
    fn notices_count_what_was_left_out() {
        assert_eq!(
            Moved {
                done: 1,
                ..Moved::default()
            }
            .notice(Done::Exported),
            "Exported 1 message"
        );
        assert_eq!(
            Moved {
                done: 3,
                skipped: 2,
                marked_deleted: 0,
            }
            .notice(Done::Imported),
            "Imported 3 messages; 2 messages couldn't be"
        );
        assert_eq!(
            Moved {
                done: 2,
                skipped: 0,
                marked_deleted: 1,
            }
            .notice(Done::Imported),
            "Imported 2 messages; 1 marked deleted in the file was kept"
        );
    }
}

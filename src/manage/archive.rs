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
}

impl Moved {
    /// A notice for the user, such as "Exported 12 messages".
    pub fn notice(&self, verb: &str) -> String {
        let plural = |n: usize| if n == 1 { "message" } else { "messages" };
        let mut text = format!("{verb} {} {}", self.done, plural(self.done));
        if self.skipped > 0 {
            text.push_str(&format!(
                "; {} {} couldn't be",
                self.skipped,
                plural(self.skipped)
            ));
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

/// Splits an mbox file (mboxrd or mboxo) into its messages, with CRLF line
/// ends as mail is sent. Quoting is undone where mboxrd added it. A message
/// larger than an import may be comes as `None`, without its text.
pub fn read_mbox(
    input: impl Read,
    mut each: impl FnMut(Option<Vec<u8>>) -> Result<()>,
) -> Result<()> {
    let mut reader = BufReader::new(input);
    // The message being read, and whether it outgrew the limit (its text
    // is then dropped, not padded out to the limit to say so).
    let mut current: Option<(Vec<u8>, bool)> = None;
    let mut line = Vec::new();
    let mut blank_before = true;
    let finish = |(message, too_large): (Vec<u8>, bool),
                  each: &mut dyn FnMut(Option<Vec<u8>>) -> Result<()>|
     -> Result<()> {
        if too_large {
            return each(None);
        }
        // The blank line separating messages isn't part of the message.
        let mut message = message;
        if message.ends_with(b"\r\n\r\n") {
            message.truncate(message.len() - 2);
        }
        if !message.iter().all(u8::is_ascii_whitespace) {
            each(Some(message))?;
        }
        Ok(())
    };
    loop {
        // A very long line's room isn't kept for the short ones after it.
        if line.capacity() > 1024 * 1024 {
            line = Vec::new();
        }
        line.clear();
        // A line is read up to the import limit: a file without line breaks
        // (not mail) isn't held in memory whole.
        if (&mut reader)
            .take(MAX_IMPORT as u64 + 1)
            .read_until(b'\n', &mut line)?
            == 0
        {
            break;
        }
        let text = line
            .strip_suffix(b"\n")
            .map(|l| l.strip_suffix(b"\r").unwrap_or(l))
            .unwrap_or(&line);
        if is_separator(text) && (blank_before || current.is_none()) {
            if let Some(message) = current.take() {
                finish(message, &mut each)?;
            }
            current = Some((Vec::new(), false));
            blank_before = false;
            continue;
        }
        blank_before = text.is_empty();
        let Some((message, too_large)) = current.as_mut() else {
            continue;
        };
        if *too_large {
            continue;
        }
        let quotes = text.iter().take_while(|&&b| b == b'>').count();
        let text = if quotes > 0 && text[quotes..].starts_with(b"From ") {
            &text[1..]
        } else {
            text
        };
        if message.len() + text.len() <= MAX_IMPORT + 2 {
            message.extend_from_slice(text);
            message.extend_from_slice(b"\r\n");
        } else {
            // Over the limit: marked as too large, its text let go.
            *message = Vec::new();
            *too_large = true;
        }
    }
    if let Some(message) = current {
        finish(message, &mut each)?;
    }
    Ok(())
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
                bail!("The export was stopped.");
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
            "Mail can't be imported into this mailbox. Choose the inbox, Archive or one of your folders."
        );
        let mut moved = Moved::default();
        // IMAP servers with MULTIAPPEND take messages in batches, in one
        // command each, rather than one round trip per message.
        let batching = matches!(a.provider, Provider::Imap | Provider::Gmail | Provider::Outlook)
            && self
                .with_imap(a, |c| Ok(c.extensions()?.multiappend))
                .unwrap_or(false);
        let mut batch: Vec<Vec<u8>> = Vec::new();
        let flush = |batch: &mut Vec<Vec<u8>>, moved: &mut Moved| -> Result<()> {
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
        let mut add = |raw: Option<Vec<u8>>| -> Result<()> {
            if cancel.load(Ordering::Relaxed) {
                bail!("The import was stopped.");
            }
            let Some(raw) = raw else {
                moved.skipped += 1;
                return Ok(());
            };
            if raw.len() > MAX_IMPORT || mailparse::parse_headers(&raw).is_err() {
                moved.skipped += 1;
                return Ok(());
            }
            if batching {
                batch.push(raw);
                let bytes: usize = batch.iter().map(Vec::len).sum();
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
                // Mail travels with CRLF; files saved elsewhere may use LF.
                add(Some(crlf(&raw)))?;
            }
        }
        drop(add);
        flush(&mut batch, &mut moved)?;
        moved.skipped += too_large;
        Ok(moved)
    }

    fn import_one(&self, a: &Account, mailbox: &Mailbox, raw: &[u8]) -> Result<()> {
        let lock = self.account_lock(&a.id);
        let _operation = lock.lock().unwrap();
        match a.provider {
            Provider::Jmap => self.with_jmap(a, |s| {
                let places = self.jmap_places(a, s, false)?;
                let target = super::jmap::mailbox_id(&places, mailbox)?;
                s.import(raw, &[&target], &["$seen"]).map(|_| ())
            }),
            Provider::Proton => self.with_proton(a, |s| {
                let label = super::proton::label_for(&s.folders(), mailbox)?;
                s.import(raw, &label, mailbox.folder() == Folder::Sent)
                    .map(|_| ())
            }),
            Provider::Pop3 => {
                let id = format!("{}:import:{}", a.id, uuid::Uuid::new_v4());
                let mut m = parse_message(raw, a, &id, mailbox.folder(), "", 0)?;
                m.unread = false;
                self.store.save_messages(&[m])?;
                self.store.save_raw(&id, raw)
            }
            _ => self.with_imap(a, |c| {
                let remote = remote_name(c.names(false)?, a, mailbox)?;
                let wire = c.wire(&remote);
                c.append_dated(&wire, raw, &[imap::types::Flag::Seen])?;
                Ok(())
            }),
        }
    }

    /// Adds several messages to `mailbox` on an IMAP server in one command
    /// where it can (MULTIAPPEND): all are added, or none.
    fn import_batch(&self, a: &Account, mailbox: &Mailbox, raws: &[Vec<u8>]) -> Result<()> {
        let lock = self.account_lock(&a.id);
        let _operation = lock.lock().unwrap();
        self.with_imap(a, |c| {
            let remote = remote_name(c.names(false)?, a, mailbox)?;
            let wire = c.wire(&remote);
            let refs: Vec<&[u8]> = raws.iter().map(Vec::as_slice).collect();
            let heard = c.append_many(&wire, &refs, &[imap::types::Flag::Seen])?;
            // Where they went (APPENDUID with a UID each): listed at once.
            if let [heard] = heard.as_slice()
                && let Some((validity, uids)) = crate::sync::transport::append_uids(heard)
                && uids.len() == raws.len()
            {
                let rows: Vec<Mail> = raws
                    .iter()
                    .zip(uids)
                    .filter_map(|(raw, uid)| {
                        let id = format!("{}:{remote}:{validity}:{uid}", a.id);
                        let mut m = parse_message(raw, a, &id, mailbox.folder(), &remote, uid).ok()?;
                        m.unread = false;
                        Some(m)
                    })
                    .collect();
                self.store.save_messages(&rows)?;
            }
            Ok(())
        })
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
                skipped: 0
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

    #[test]
    fn bare_line_feeds_become_crlf() {
        assert_eq!(crlf(b"a\nb\r\nc\n"), b"a\r\nb\r\nc\r\n");
    }

    #[test]
    fn notices_count_what_was_left_out() {
        assert_eq!(
            Moved {
                done: 1,
                skipped: 0
            }
            .notice("Exported"),
            "Exported 1 message"
        );
        assert_eq!(
            Moved {
                done: 3,
                skipped: 2
            }
            .notice("Imported"),
            "Imported 3 messages; 2 messages couldn't be"
        );
    }
}

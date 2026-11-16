//! Large messages. Up to `MAX_DOWNLOAD`, a message's source downloads in
//! pieces (`BODY.PEEK[]<offset.length>`), each kept in a file as it
//! arrives, so a download that is interrupted resumes where it stopped.
//! Larger messages are never downloaded whole: their structure
//! (BODYSTRUCTURE) says which parts are text, those are fetched alone
//! (decoded by the server with BINARY where it can), and each attachment
//! downloads by itself when saved.
use super::*;
use crate::sync::transport::{Leaf, decode_part, fetch_part, fetch_range, structure};

/// The largest message downloaded whole.
pub(super) const MAX_DOWNLOAD: usize = 200 * 1024 * 1024;
/// Pieces of a large download: well within the connection's 32 MB
/// budget per reply.
pub(super) const CHUNK: usize = 8 * 1024 * 1024;
/// Parts fetched in one request up to this size; larger ones in pieces.
const PART_IN_ONE: usize = 16 * 1024 * 1024;
/// The largest attachment downloaded from a message too large to keep.
const MAX_PART: usize = 1024 * 1024 * 1024;

impl Backend {
    /// Where a download in progress keeps what has arrived.
    fn partial_file(&self, id: &str) -> std::path::PathBuf {
        // Named by a hash of the id: any mapping of ids to file names that
        // drops characters can give two messages one file, and a download
        // would then resume from another message's bytes.
        use sha2::Digest as _;
        let name: String = sha2::Sha256::digest(id.as_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        self.store.path.join("partial-downloads").join(name)
    }
    /// Pieces of downloads left this long (the message was deleted, or
    /// never opened again) are deleted rather than kept forever.
    fn remove_stale_partials(&self, dir: &std::path::Path) {
        const STALE: std::time::Duration = std::time::Duration::from_secs(7 * 86_400);
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let old = entry
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|age| age > STALE);
            if old {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
    /// Downloads a selected message's source in pieces, resuming from what
    /// an earlier, interrupted download kept. Callers selected its mailbox.
    pub(super) fn download_resumable(&self, c: &mut Connection, m: &Mail) -> Result<Vec<u8>> {
        use std::io::Write;
        let path = self.partial_file(&m.id);
        let dir = path.parent().context("Invalid download folder")?;
        std::fs::create_dir_all(dir)?;
        self.remove_stale_partials(dir);
        let mut have = std::fs::read(&path).unwrap_or_default();
        // A piece from another generation, or past the message, is no use.
        if have.len() > m.size as usize && m.size > 0 {
            have.clear();
        }
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(have.is_empty())
            .append(!have.is_empty())
            .open(&path)?;
        loop {
            let piece = fetch_range(&mut c.session, m.uid, have.len(), CHUNK)?;
            file.write_all(&piece)?;
            file.flush()?;
            have.extend_from_slice(&piece);
            ensure!(
                have.len() <= MAX_DOWNLOAD,
                "This message is larger than Picacho downloads whole."
            );
            if piece.len() < CHUNK {
                break;
            }
        }
        ensure!(!have.is_empty(), "Message no longer exists on the server");
        drop(file);
        let _ = std::fs::remove_file(&path);
        Ok(have)
    }
    /// A message too large to download whole, read from its parts: the
    /// headers, the first plain and HTML text, and a list of attachments
    /// that download when saved.
    pub(super) fn read_parts(&self, c: &mut Connection, a: &Account, m: &Mail) -> Result<Mail> {
        c.select_for(m)?;
        let ext = c.extensions()?;
        let rows = uid_fetch_rows(
            &mut c.session,
            &m.uid.to_string(),
            "(UID BODY.PEEK[HEADER])",
        )?;
        let header = rows
            .into_iter()
            .find(|r| r.uid == m.uid)
            .and_then(|r| r.header)
            .context("Message no longer exists on the server")?;
        let mut next = parse_message(&header, a, &m.id, m.folder, &m.remote_folder, m.uid)?;
        let leaves = structure(&mut c.session, m.uid)?;
        let mut body = None;
        let mut html = None;
        let mut attachments = Vec::new();
        for (index, leaf) in leaves.iter().enumerate() {
            let text = !leaf.attachment && leaf.filename.is_none();
            if text && leaf.mime == "text/plain" && body.is_none() {
                body = Some(decode_text(
                    &fetch_part(&mut c.session, &ext, m.uid, leaf, PART_IN_ONE)?,
                    leaf,
                ));
            } else if text && leaf.mime == "text/html" && html.is_none() {
                html = Some(decode_text(
                    &fetch_part(&mut c.session, &ext, m.uid, leaf, PART_IN_ONE)?,
                    leaf,
                ));
            } else if !text || !leaf.mime.starts_with("text/") {
                attachments.push(Attachment {
                    name: leaf
                        .filename
                        .clone()
                        .unwrap_or_else(|| format!("attachment-{}", index + 1)),
                    // Base64 takes four bytes for every three.
                    size: if leaf.encoding == "base64" {
                        leaf.size / 4 * 3
                    } else {
                        leaf.size
                    },
                    mime: leaf.mime.clone(),
                    part: index,
                    section: leaf.section.clone(),
                    encoding: leaf.encoding.clone(),
                });
            }
        }
        if body.is_none()
            && let Some(html) = &html
        {
            body = Some(html_to_text(html));
        }
        next.snippet = body.as_deref().map(snippet).unwrap_or_default();
        next.body = body.or_else(|| Some(String::new()));
        next.html = html;
        next.has_attachments = !attachments.is_empty();
        next.set_attachments(attachments);
        next.partial = true;
        next.unread = m.unread;
        next.starred = m.starred;
        next.size = m.size;
        next.set_labels(m.labels().to_vec());
        next.answered = m.answered;
        next.forwarded = m.forwarded;
        next.mdn_sent = m.mdn_sent;
        next.set_thread_id(m.thread_id());
        next.gmail_id = m.gmail_id;
        Ok(next)
    }
    /// One attachment of a message too large to keep, decoded. Large
    /// attachments come in pieces, so no reply outgrows the connection's
    /// limit. Callers selected its mailbox.
    pub(super) fn fetch_attachment(
        &self,
        c: &mut Connection,
        m: &Mail,
        attachment: &Attachment,
    ) -> Result<Vec<u8>> {
        let ext = c.extensions()?;
        let leaf = Leaf {
            section: attachment.section.clone(),
            mime: attachment.mime.clone(),
            encoding: attachment.encoding.clone(),
            // `size` is decoded; the part on the server may be larger.
            size: attachment.size,
            ..Leaf::default()
        };
        if attachment.size <= PART_IN_ONE {
            return fetch_part(&mut c.session, &ext, m.uid, &leaf, PART_IN_ONE * 2);
        }
        ensure!(
            attachment.size <= MAX_PART,
            "This attachment is larger than Picacho downloads (1 GB)."
        );
        ensure!(
            leaf.section
                .bytes()
                .all(|b| b.is_ascii_digit() || b == b'.'),
            "Invalid IMAP section"
        );
        let mut raw = Vec::new();
        loop {
            let rows = uid_fetch_rows(
                &mut c.session,
                &m.uid.to_string(),
                &format!("(UID BODY.PEEK[{}]<{}.{CHUNK}>)", leaf.section, raw.len()),
            )?;
            let piece = rows
                .into_iter()
                .find(|r| r.uid == m.uid)
                .and_then(|r| r.header)
                .context("The message no longer exists on the server")?;
            let done = piece.len() < CHUNK;
            raw.extend_from_slice(&piece);
            ensure!(raw.len() <= MAX_PART * 2, "This attachment is too large.");
            if done {
                break;
            }
        }
        decode_part(&raw, &leaf.encoding)
    }
}

/// A text part's decoded bytes as text, in its charset.
fn decode_text(bytes: &[u8], leaf: &Leaf) -> String {
    let mut wrapped = format!(
        "Content-Type: {}; charset=\"{}\"\r\n\r\n",
        leaf.mime,
        leaf.charset.as_deref().unwrap_or("utf-8").replace('"', "")
    )
    .into_bytes();
    wrapped.extend_from_slice(bytes);
    mailparse::parse_mail(&wrapped)
        .and_then(|p| p.get_body())
        .unwrap_or_else(|_| String::from_utf8_lossy(bytes).into_owned())
}

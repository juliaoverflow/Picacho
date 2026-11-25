//! Large messages. Up to `MAX_DOWNLOAD`, a message's source downloads in
//! pieces (`BODY.PEEK[]<offset.length>`), each kept in a file as it
//! arrives, so a download that is interrupted resumes where it stopped.
//! Larger messages are never downloaded whole: their structure
//! (BODYSTRUCTURE) says which parts are text, those are fetched alone
//! (decoded by the server with BINARY where it can), and each attachment
//! downloads by itself when saved.
use super::*;
use crate::sync::transport::{Leaf, binary_sizes, decode_part, fetch_part, fetch_range, structure};

/// The largest message downloaded whole: the largest the cache reads back.
pub(super) const MAX_DOWNLOAD: usize = super::store::MAX_SOURCE as usize;
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
    /// Pieces of downloads left a week (the message was deleted, or never
    /// opened again) are deleted rather than kept forever. Run when the
    /// app starts (`tidy_files`) and, at most once a day, when a download
    /// starts, rather than listing the folder on every download.
    pub(super) fn remove_stale_partials(&self) {
        use std::sync::atomic::{AtomicU64, Ordering};
        static LAST: AtomicU64 = AtomicU64::new(0);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let last = LAST.load(Ordering::Relaxed);
        if now.saturating_sub(last) < 86_400
            || LAST.compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed).is_err()
        {
            return;
        }
        remove_older_than(
            &self.store.path.join("partial-downloads"),
            std::time::Duration::from_secs(7 * 86_400),
            |_| true,
        );
    }
    /// Downloads a selected message's source in pieces, resuming from what
    /// an earlier, interrupted download kept. Callers selected its mailbox.
    pub(super) fn download_resumable(&self, c: &mut Connection, m: &Mail) -> Result<Vec<u8>> {
        use std::io::Write;
        let path = self.partial_file(&m.id);
        let dir = path.parent().with_context(|| t!("manage-large-invalid-download-folder"))?;
        std::fs::create_dir_all(dir)?;
        self.remove_stale_partials();
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
            // Checked before it is kept: a piece past the limit is neither
            // written nor held.
            if have.len() + piece.len() > MAX_DOWNLOAD {
                // It will never finish: what was kept of it goes now, not
                // after a week.
                drop(file);
                let _ = std::fs::remove_file(&path);
                bail!(t!("manage-large-this-message-is-larger-than-picacho"));
            }
            file.write_all(&piece)?;
            file.flush()?;
            have.extend_from_slice(&piece);
            if piece.len() < CHUNK {
                break;
            }
        }
        ensure!(!have.is_empty(), t!("manage-large-message-no-longer-exists-on-the"));
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
            .with_context(|| t!("manage-large-message-no-longer-exists-on-the"))?;
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
        // The pictures the HTML shows by Content-ID (`cid:`), fetched where
        // it names them, within the inline budget.
        if let Some(text) = &html {
            let lower = text.to_ascii_lowercase();
            let mut inline = crate::manage::model::InlineParts::default();
            let mut any = false;
            for leaf in &leaves {
                let Some(id) = &leaf.content_id else { continue };
                let bare = id.trim().trim_start_matches('<').trim_end_matches('>').to_ascii_lowercase();
                if bare.is_empty()
                    || leaf.mime.starts_with("text/")
                    || !lower.contains(&format!("cid:{bare}"))
                    || leaf.size > crate::manage::model::InlineParts::MAX_BYTES
                {
                    continue;
                }
                if let Ok(bytes) = fetch_part(&mut c.session, &ext, m.uid, leaf, PART_IN_ONE) {
                    any |= inline.insert(id, bytes);
                }
            }
            if any {
                next.inline = std::sync::Arc::new(inline);
            }
        }
        if body.is_none()
            && let Some(html) = &html
        {
            body = Some(html_to_text(html));
        }
        // Exact decoded sizes where the server can say (BINARY.SIZE, RFC
        // 3516 §4.2), in place of the estimate from the encoded size.
        if ext.binary && !attachments.is_empty() {
            let sections: Vec<&str> = attachments.iter().map(|a| a.section.as_str()).collect();
            if let Ok(sizes) = binary_sizes(&mut c.session, m.uid, &sections) {
                for attachment in &mut attachments {
                    if let Some(size) = sizes.get(&attachment.section) {
                        attachment.size = *size as usize;
                    }
                }
            }
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
            t!("manage-large-this-attachment-is-larger-than-picacho")
        );
        ensure!(
            leaf.section
                .bytes()
                .all(|b| b.is_ascii_digit() || b == b'.'),
            t!("manage-large-invalid-imap-section")
        );
        // Base64 (nearly every large attachment) is decoded piece by piece
        // as it arrives, so only the decoded file is held, not the encoded
        // part as well, and copies of it, as decoding it whole needed.
        let mut base64 = leaf.encoding.eq_ignore_ascii_case("base64").then(Base64Stream::default);
        let mut raw = Vec::new();
        let mut offset = 0;
        loop {
            let rows = uid_fetch_rows(
                &mut c.session,
                &m.uid.to_string(),
                &format!("(UID BODY.PEEK[{}]<{offset}.{CHUNK}>)", leaf.section),
            )?;
            let piece = rows
                .into_iter()
                .find(|r| r.uid == m.uid)
                .and_then(|r| r.header)
                .with_context(|| t!("manage-large-the-message-no-longer-exists-on"))?;
            let done = piece.len() < CHUNK;
            offset += piece.len();
            ensure!(offset <= MAX_PART * 2, t!("manage-large-this-attachment-is-too-large"));
            match &mut base64 {
                Some(stream) => {
                    stream.feed(&piece)?;
                    ensure!(stream.out.len() <= MAX_PART, t!("manage-large-this-attachment-is-too-large"));
                }
                None => raw.extend_from_slice(&piece),
            }
            if done {
                break;
            }
        }
        match base64 {
            Some(stream) => stream.finish(),
            None => decode_part(&raw, &leaf.encoding),
        }
    }
}

/// Base64 decoded as it arrives, in pieces: whitespace is passed over, and
/// what doesn't yet make a whole group of four is kept for the next piece.
/// As lenient as decoding a part whole (`decode_part`): padding may be left
/// out, and stray bits at the end are ignored.
#[derive(Default)]
struct Base64Stream {
    /// Characters waiting for the rest of their group.
    pending: Vec<u8>,
    out: Vec<u8>,
}

impl Base64Stream {
    const ENGINE: base64::engine::GeneralPurpose = base64::engine::GeneralPurpose::new(
        &base64::alphabet::STANDARD,
        base64::engine::GeneralPurposeConfig::new()
            .with_decode_allow_trailing_bits(true)
            .with_decode_padding_mode(base64::engine::DecodePaddingMode::Indifferent),
    );
    fn feed(&mut self, piece: &[u8]) -> Result<()> {
        self.pending
            .extend(piece.iter().copied().filter(|b| !b.is_ascii_whitespace()));
        let whole = self.pending.len() / 4 * 4;
        // A group with padding ends a run; any after it decode on their own.
        let mut start = 0;
        while start < whole {
            let end = self.pending[start..whole]
                .chunks(4)
                .position(|g| g.contains(&b'='))
                .map_or(whole, |i| start + (i + 1) * 4);
            self.decode(start, end)?;
            start = end;
        }
        self.pending.drain(..whole);
        Ok(())
    }
    fn decode(&mut self, start: usize, end: usize) -> Result<()> {
        use base64::Engine as _;
        Self::ENGINE
            .decode_vec(&self.pending[start..end], &mut self.out)
            .with_context(|| t!("manage-large-the-attachments-encoding-is-damaged"))
    }
    fn finish(mut self) -> Result<Vec<u8>> {
        let len = self.pending.len();
        // A lone character at the end holds no whole byte.
        if len % 4 > 1 {
            self.decode(0, len)?;
        }
        Ok(self.out)
    }
}

/// Removes the files in `dir` that `wanted` picks and were last changed
/// longer than `age` ago.
pub(super) fn remove_older_than(
    dir: &std::path::Path,
    age: std::time::Duration,
    wanted: impl Fn(&std::path::Path) -> bool,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|elapsed| elapsed > age);
        if old && wanted(&path) && path.is_file() {
            let _ = std::fs::remove_file(path);
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Decoded in pieces split anywhere (inside groups, inside line breaks),
    /// a part comes out as decoding it whole does.
    #[test]
    fn base64_decodes_in_pieces_as_whole() -> Result<()> {
        use base64::Engine as _;
        let data: Vec<u8> = (0..5000u32).map(|i| (i * 7 % 251) as u8).collect();
        for cut in [0, 1, 2, 3] {
            let data = &data[..data.len() - cut];
            let encoded = base64::engine::general_purpose::STANDARD.encode(data);
            let wrapped: Vec<u8> = encoded
                .as_bytes()
                .chunks(76)
                .flat_map(|l| l.iter().copied().chain(*b"\r\n"))
                .collect();
            for size in [1, 3, 5, 77, 1000] {
                let mut s = Base64Stream::default();
                for piece in wrapped.chunks(size) {
                    s.feed(piece)?;
                }
                assert_eq!(s.finish()?, data, "cut {cut}, pieces of {size}");
            }
            assert_eq!(decode_part(&wrapped, "base64")?, data);
        }
        // Without padding, and two encoded runs one after the other.
        let mut s = Base64Stream::default();
        s.feed(b"aGk")?;
        assert_eq!(s.finish()?, b"hi");
        let mut s = Base64Stream::default();
        s.feed(b"aGk=aGk=")?;
        assert_eq!(s.finish()?, b"hihi");
        assert!({
            let mut s = Base64Stream::default();
            s.feed(b"a*b?").and_then(|()| s.finish()).is_err()
        });
        Ok(())
    }
}

//! Large messages. Up to `MAX_DOWNLOAD`, a message's source downloads in
//! pieces (`BODY.PEEK[]<offset.length>`), each kept in a file as it
//! arrives (sealed while the database is encrypted, see
//! `sealed_files::SealedPartial`), so a download that is interrupted
//! resumes where it stopped.
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
pub(super) const MAX_PART: usize = 1024 * 1024 * 1024;

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
        self.store.path.join(crate::manage::sealed_files::PARTIALS).join(name)
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
        // While the database is encrypted, none are kept in the clear.
        if self.store.encrypted() {
            let _ = crate::manage::sealed_files::discard_plain_partials(&self.store.path);
        }
        remove_older_than(
            &self.store.path.join(crate::manage::sealed_files::PARTIALS),
            std::time::Duration::from_secs(7 * 86_400),
            |_| true,
        );
    }
    /// Downloads a selected message's source in pieces, resuming from what
    /// an earlier, interrupted download kept. Callers selected its mailbox.
    ///
    /// While the database is encrypted the pieces are sealed
    /// (`download_sealed`): raw mail mustn't lie readable beside it.
    pub(super) fn download_resumable(&self, c: &mut Connection, m: &Mail) -> Result<Vec<u8>> {
        use std::io::Write;
        let path = self.partial_file(&m.id);
        if self.store.encrypted() {
            return match crate::manage::sealed_files::current_key(&self.store.path) {
                Some(key) => self.download_sealed(c, m, &path, &key),
                // No key to seal with (never so while encrypted): nothing
                // kept, not even what an earlier download left.
                None => {
                    let _ = std::fs::remove_file(&path);
                    download_in_memory(c, m)
                }
            };
        }
        // Sealed while the database was encrypted: not appended to in the
        // clear, and not resumed with the key gone.
        if crate::manage::sealed_files::is_sealed_partial(&path) {
            let _ = std::fs::remove_file(&path);
        }
        let dir = path.parent().with_context(|| t!("manage-large-invalid-download-folder"))?;
        std::fs::create_dir_all(dir)?;
        self.remove_stale_partials();
        // What arrived before, by its length alone: the pieces stay in the
        // file, not held in memory as well, until the whole is read back.
        let mut have = std::fs::metadata(&path).map_or(0, |meta| meta.len() as usize);
        // A piece from another generation, or past the message, is no use.
        if have > m.size as usize && m.size > 0 {
            have = 0;
        }
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(have == 0)
            .append(have > 0)
            .open(&path)?;
        loop {
            let piece = fetch_range(&mut c.session, m.uid, have, CHUNK)?;
            // Checked before it is kept: a piece past the limit is neither
            // written nor held.
            if have + piece.len() > MAX_DOWNLOAD {
                // It will never finish: what was kept of it goes now, not
                // after a week.
                drop(file);
                let _ = std::fs::remove_file(&path);
                bail!(t!("manage-large-this-message-is-larger-than-picacho"));
            }
            file.write_all(&piece)?;
            file.flush()?;
            have += piece.len();
            if piece.len() < CHUNK {
                break;
            }
        }
        ensure!(have > 0, t!("manage-large-message-no-longer-exists-on-the"));
        drop(file);
        let whole = std::fs::read(&path)?;
        let _ = std::fs::remove_file(&path);
        Ok(whole)
    }
    /// `download_resumable` while the database is encrypted: each piece
    /// sealed as it arrives with keys from `key`, the database's file key,
    /// and every kept piece checked before the download resumes after it.
    fn download_sealed(&self, c: &mut Connection, m: &Mail, path: &std::path::Path, key: &[u8; 32]) -> Result<Vec<u8>> {
        use crate::manage::sealed_files::SealedPartial;
        let dir = path.parent().with_context(|| t!("manage-large-invalid-download-folder"))?;
        std::fs::create_dir_all(dir)?;
        self.remove_stale_partials();
        let mut partial = SealedPartial::open(path, key, &m.id, CHUNK)?;
        // A piece from another generation, or past the message, is no use.
        if partial.len() > u64::from(m.size) && m.size > 0 {
            drop(partial);
            let _ = std::fs::remove_file(path);
            partial = SealedPartial::open(path, key, &m.id, CHUNK)?;
        }
        loop {
            let have = partial.len() as usize;
            let piece = fetch_range(&mut c.session, m.uid, have, CHUNK)?;
            if have + piece.len() > MAX_DOWNLOAD {
                drop(partial);
                let _ = std::fs::remove_file(path);
                bail!(t!("manage-large-this-message-is-larger-than-picacho"));
            }
            partial.append(&piece)?;
            if piece.len() < CHUNK {
                break;
            }
        }
        ensure!(partial.len() > 0, t!("manage-large-message-no-longer-exists-on-the"));
        let whole = partial.read_all(CHUNK);
        let _ = std::fs::remove_file(path);
        whole
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
        let mut out = Vec::new();
        self.fetch_attachment_into(c, m, attachment, &mut out)?;
        Ok(out)
    }
    /// `fetch_attachment`, written to `out` as it arrives: a large one is
    /// decoded piece by piece and passed on, so only a piece (not the whole
    /// attachment) is in memory at once. Returns the decoded size.
    pub(super) fn fetch_attachment_into(
        &self,
        c: &mut Connection,
        m: &Mail,
        attachment: &Attachment,
        out: &mut dyn std::io::Write,
    ) -> Result<u64> {
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
            let bytes = fetch_part(&mut c.session, &ext, m.uid, &leaf, PART_IN_ONE * 2)?;
            out.write_all(&bytes)?;
            return Ok(bytes.len() as u64);
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
        // Base64 (nearly every large attachment), quoted-printable and the
        // encodings that need none are decoded piece by piece as they
        // arrive and written on; any other is decoded whole.
        let mut base64 = leaf.encoding.eq_ignore_ascii_case("base64").then(Base64Stream::default);
        let mut qp = leaf.encoding.eq_ignore_ascii_case("quoted-printable").then(QpStream::default);
        let identity = matches!(
            leaf.encoding.trim().to_ascii_lowercase().as_str(),
            "" | "7bit" | "8bit" | "binary"
        );
        let mut raw = Vec::new();
        let mut written: u64 = 0;
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
                    written += stream.out.len() as u64;
                    ensure!(written <= MAX_PART as u64, t!("manage-large-this-attachment-is-too-large"));
                    out.write_all(&stream.out)?;
                    stream.out.clear();
                }
                None if identity => {
                    // Not encoded: as large as the file, which may be no
                    // larger than `MAX_PART`.
                    written += piece.len() as u64;
                    ensure!(written <= MAX_PART as u64, t!("manage-large-this-attachment-is-too-large"));
                    out.write_all(&piece)?;
                }
                None => match &mut qp {
                    Some(stream) => {
                        let decoded = stream.feed(&piece)?;
                        written += decoded.len() as u64;
                        ensure!(written <= MAX_PART as u64, t!("manage-large-this-attachment-is-too-large"));
                        out.write_all(&decoded)?;
                    }
                    None => raw.extend_from_slice(&piece),
                },
            }
            if done {
                break;
            }
        }
        match base64 {
            Some(stream) => {
                let rest = stream.finish()?;
                out.write_all(&rest)?;
                Ok(written + rest.len() as u64)
            }
            None if identity => Ok(written),
            None if qp.is_some() => {
                let rest = qp.map(QpStream::finish).transpose()?.unwrap_or_default();
                out.write_all(&rest)?;
                Ok(written + rest.len() as u64)
            }
            None => {
                let decoded = decode_part(&raw, &leaf.encoding)?;
                out.write_all(&decoded)?;
                Ok(decoded.len() as u64)
            }
        }
    }
}

/// `download_resumable` without a file: the pieces held in memory, within
/// the same limit.
fn download_in_memory(c: &mut Connection, m: &Mail) -> Result<Vec<u8>> {
    let mut whole = Vec::new();
    loop {
        let piece = fetch_range(&mut c.session, m.uid, whole.len(), CHUNK)?;
        if whole.len() + piece.len() > MAX_DOWNLOAD {
            bail!(t!("manage-large-this-message-is-larger-than-picacho"));
        }
        let last = piece.len() < CHUNK;
        whole.extend_from_slice(&piece);
        if last {
            break;
        }
    }
    ensure!(!whole.is_empty(), t!("manage-large-message-no-longer-exists-on-the"));
    Ok(whole)
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

/// Quoted-printable decoded as it arrives, in pieces: whole lines are
/// decoded as they come (an escape, `=XX`, or a soft line break never
/// spans lines), the rest kept for the next piece. A piece without line
/// breaks past `CHUNK` is decoded up to an escape it would cut in two.
#[derive(Default)]
struct QpStream {
    pending: Vec<u8>,
}

impl QpStream {
    fn feed(&mut self, piece: &[u8]) -> Result<Vec<u8>> {
        self.pending.extend_from_slice(piece);
        let len = self.pending.len();
        let cut = match memchr::memrchr(b'\n', &self.pending) {
            Some(at) => at + 1,
            None if len > CHUNK => (len.saturating_sub(2)..len).find(|&i| self.pending[i] == b'=').unwrap_or(len),
            None => 0,
        };
        if cut == 0 {
            return Ok(Vec::new());
        }
        let decoded = decode_part(&self.pending[..cut], "quoted-printable")?;
        self.pending.drain(..cut);
        Ok(decoded)
    }
    fn finish(self) -> Result<Vec<u8>> {
        if self.pending.is_empty() {
            return Ok(Vec::new());
        }
        decode_part(&self.pending, "quoted-printable")
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

    /// Quoted-printable decoded in pieces split anywhere (inside escapes,
    /// soft line breaks and line breaks) comes out as decoding it whole.
    #[test]
    fn quoted_printable_decodes_in_pieces_as_whole() -> Result<()> {
        let text = "Caf=C3=A9 au lait, cr=C3=A8me br=C3=BBl=C3=A9e and a very long line that =\r\nwraps softly =3D still one line.\r\nTrailing spaces   \r\nLast=\r\n line, no break at the end=21";
        let whole = decode_part(text.as_bytes(), "quoted-printable")?;
        for size in [1, 2, 3, 5, 17, 1000] {
            let mut s = QpStream::default();
            let mut out = Vec::new();
            for piece in text.as_bytes().chunks(size) {
                out.extend(s.feed(piece)?);
            }
            out.extend(s.finish()?);
            assert_eq!(out, whole, "pieces of {size}");
        }
        Ok(())
    }

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

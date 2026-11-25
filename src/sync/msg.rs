//! Reading Outlook `.msg` files (MS-CFB OLE2 compound files + MS-OXMSG MAPI
//! properties) and turning them into standard RFC 5322 `.eml` messages so
//! Picacho can open, display, reply to, forward, import and index them just
//! like `.eml` files.
use anyhow::{Context, Result, bail, ensure};
use base64::Engine as _;
use std::collections::HashMap;

/// OLE2 Compound File Binary Format magic header (MS-CFB §2.2).
const CFBF_MAGIC: [u8; 8] = [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];
const NOSTREAM: u32 = 0xFFFF_FFFF;
const ENDOFCHAIN: u32 = 0xFFFF_FFFE;
const FREESECT: u32 = 0xFFFF_FFFF;
/// Cap on decompressed/assembled streams so malformed files cannot OOM.
const MAX_STREAM_BYTES: usize = 64 * 1024 * 1024;
/// Cap on directory entries / FAT hops to prevent loops in corrupt files.
const MAX_ENTRIES: usize = 16_384;
/// Maximum nesting depth for embedded `.msg` attachments.
const MAX_DEPTH: usize = 6;
/// Recipients and attachments read per message at most.
const MAX_PARTS: usize = 2048;
/// Bytes read out of a file's streams in all, as a multiple of its size (plus
/// a little): streams may share sectors, so without a budget a small crafted
/// file could be read out into gigabytes.
const READ_BUDGET_FACTOR: usize = 8;

/// Returns true when `bytes` begins with the OLE2 CFBF signature.
pub fn is_msg(bytes: &[u8]) -> bool {
    bytes.len() >= 512 && bytes[..8] == CFBF_MAGIC
}

/// Converts an Outlook `.msg` file into standard RFC 5322 MIME (`.eml`) bytes.
///
/// The file comes from mail or disk, so it is untrusted: a panic in reading it
/// is turned into an error rather than taking down the caller (the search
/// indexer reads attached `.msg` files of received mail).
pub fn to_eml(bytes: &[u8]) -> Result<Vec<u8>> {
    std::panic::catch_unwind(|| to_eml_depth(bytes, 0))
        .unwrap_or_else(|_| bail!(t!("sync-msg-this-msg-file-couldnt-be-read")))
}

fn to_eml_depth(bytes: &[u8], depth: usize) -> Result<Vec<u8>> {
    ensure!(depth <= MAX_DEPTH, t!("sync-msg-embedded-msg-files-are-nested-too"));
    let cf = CompoundFile::parse(bytes)?;
    let msg = parse_storage(&cf, 0, depth)?;
    Ok(build_eml(&msg))
}

#[derive(Debug, Default)]
struct ParsedMsg {
    subject: String,
    from: String,
    to: String,
    cc: String,
    bcc: String,
    date: Option<String>,
    message_id: String,
    in_reply_to: String,
    references: String,
    plain: String,
    html: String,
    attachments: Vec<MsgAttachment>,
}

#[derive(Debug, Default)]
struct MsgAttachment {
    name: String,
    mime: String,
    content_id: String,
    data: Vec<u8>,
}

#[derive(Debug, Clone)]
struct DirEntry {
    name: String,
    obj_type: u8, // 1 = storage, 2 = stream, 5 = root
    left: u32,
    right: u32,
    child: u32,
    start_sector: u32,
    size: u64,
}

struct CompoundFile<'a> {
    raw: &'a [u8],
    sector_size: usize,
    mini_sector_size: usize,
    mini_cutoff: u32,
    fat: Vec<u32>,
    mini_fat: Vec<u32>,
    mini_stream: Vec<u8>,
    entries: Vec<DirEntry>,
    /// Bytes still allowed to be read out of streams (`READ_BUDGET_FACTOR`).
    budget: std::cell::Cell<usize>,
    /// Storages already read as messages, so an embedded message pointing
    /// back at an ancestor (or shared by many attachments) is read once.
    parsed: std::cell::RefCell<std::collections::HashSet<usize>>,
}

fn u16_le(b: &[u8], off: usize) -> Result<u16> {
    let s: [u8; 2] = b
        .get(off..off + 2)
        .with_context(|| t!("sync-msg-unexpected-end-of-msg-file"))?
        .try_into()?;
    Ok(u16::from_le_bytes(s))
}

fn u32_le(b: &[u8], off: usize) -> Result<u32> {
    let s: [u8; 4] = b
        .get(off..off + 4)
        .with_context(|| t!("sync-msg-unexpected-end-of-msg-file"))?
        .try_into()?;
    Ok(u32::from_le_bytes(s))
}

fn u64_le(b: &[u8], off: usize) -> Result<u64> {
    let s: [u8; 8] = b
        .get(off..off + 8)
        .with_context(|| t!("sync-msg-unexpected-end-of-msg-file"))?
        .try_into()?;
    Ok(u64::from_le_bytes(s))
}

impl<'a> CompoundFile<'a> {
    fn parse(raw: &'a [u8]) -> Result<Self> {
        ensure!(is_msg(raw), t!("sync-msg-not-a-valid-outlook-msg-file"));
        let major_ver = u16_le(raw, 26)?;
        let sector_shift = u16_le(raw, 30)?;
        let mini_shift = u16_le(raw, 32)?;
        ensure!(
            sector_shift == 9 || sector_shift == 12,
            t!("sync-msg-unsupported-msg-sector-size")
        );
        ensure!(
            (3..=sector_shift).contains(&mini_shift),
            t!("sync-msg-unsupported-msg-mini-sector-size")
        );
        let sector_size = 1usize << sector_shift;
        let mini_sector_size = 1usize << mini_shift;
        let first_dir_sector = u32_le(raw, 48)?;
        let mini_cutoff = u32_le(raw, 56)?;
        let first_mini_fat = u32_le(raw, 60)?;
        let num_mini_fat = u32_le(raw, 64)?;
        let mut difat_sector = u32_le(raw, 68)?;
        let num_difat = u32_le(raw, 72)?;

        // Collect FAT sector numbers from header DIFAT (109 entries at offset 76) + DIFAT chain.
        let mut fat_sectors = Vec::new();
        for i in 0..109 {
            let s = u32_le(raw, 76 + i * 4)?;
            if s != FREESECT && s != ENDOFCHAIN {
                fat_sectors.push(s);
            }
        }
        let entries_per_sector = sector_size / 4;
        let mut difat_hops = 0u32;
        while difat_sector != ENDOFCHAIN
            && difat_sector != FREESECT
            && difat_hops < num_difat.min(4096)
        {
            let slice = sector_slice(raw, sector_size, difat_sector)?;
            for i in 0..entries_per_sector.saturating_sub(1) {
                let s = u32_le(slice, i * 4)?;
                if s != FREESECT && s != ENDOFCHAIN {
                    fat_sectors.push(s);
                }
            }
            difat_sector = u32_le(slice, (entries_per_sector - 1) * 4)?;
            difat_hops += 1;
        }

        // Build FAT table.
        let mut fat = Vec::with_capacity(fat_sectors.len().saturating_mul(entries_per_sector));
        for &sec in &fat_sectors {
            let slice = sector_slice(raw, sector_size, sec)?;
            for i in 0..entries_per_sector {
                fat.push(u32_le(slice, i * 4)?);
            }
        }

        // Read directory stream.
        let dir_bytes = read_fat_chain(raw, sector_size, &fat, first_dir_sector, MAX_STREAM_BYTES)?;
        let mut entries = Vec::new();
        for chunk in dir_bytes.as_chunks::<128>().0.iter().take(MAX_ENTRIES) {
            let name_len = (u16_le(chunk, 64)? as usize).min(64);
            let name = decode_utf16le(&chunk[..name_len.saturating_sub(2)]);
            let obj_type = chunk[66];
            let left = u32_le(chunk, 68)?;
            let right = u32_le(chunk, 72)?;
            let child = u32_le(chunk, 76)?;
            let start_sector = u32_le(chunk, 116)?;
            let size = if major_ver >= 4 {
                u64_le(chunk, 120)?
            } else {
                u32_le(chunk, 120)? as u64
            };
            entries.push(DirEntry {
                name,
                obj_type,
                left,
                right,
                child,
                start_sector,
                size,
            });
        }
        ensure!(!entries.is_empty(), t!("sync-msg-empty-msg-directory"));

        // Read MiniFAT.
        let mut mini_fat = Vec::new();
        if first_mini_fat != ENDOFCHAIN && first_mini_fat != FREESECT && num_mini_fat > 0 {
            let mf_bytes =
                read_fat_chain(raw, sector_size, &fat, first_mini_fat, MAX_STREAM_BYTES)?;
            for &chunk in mf_bytes.as_chunks::<4>().0 {
                mini_fat.push(u32::from_le_bytes(chunk));
            }
        }

        // Read MiniStream (stored as the Root Entry's stream).
        let root = &entries[0];
        let mini_stream = if root.start_sector != ENDOFCHAIN
            && root.start_sector != FREESECT
            && root.size > 0
        {
            let mut ms = read_fat_chain(
                raw,
                sector_size,
                &fat,
                root.start_sector,
                (root.size as usize).min(MAX_STREAM_BYTES),
            )?;
            ms.truncate((root.size as usize).min(ms.len()));
            ms
        } else {
            Vec::new()
        };

        Ok(Self {
            raw,
            sector_size,
            mini_sector_size,
            mini_cutoff,
            fat,
            mini_fat,
            mini_stream,
            entries,
            budget: std::cell::Cell::new(
                raw.len().saturating_mul(READ_BUDGET_FACTOR).saturating_add(1 << 20),
            ),
            parsed: Default::default(),
        })
    }

    /// Direct children of storage `storage_idx` (traversing its red-black tree safely).
    fn children(&self, storage_idx: usize) -> Vec<usize> {
        let Some(root) = self.entries.get(storage_idx) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        let mut visited = std::collections::HashSet::new();
        let mut stack = Vec::new();
        if root.child != NOSTREAM {
            stack.push(root.child as usize);
        }
        while let Some(idx) = stack.pop() {
            if idx >= self.entries.len() || !visited.insert(idx) || out.len() >= MAX_ENTRIES {
                continue;
            }
            let e = &self.entries[idx];
            out.push(idx);
            if e.left != NOSTREAM {
                stack.push(e.left as usize);
            }
            if e.right != NOSTREAM {
                stack.push(e.right as usize);
            }
        }
        out
    }

    fn read_stream(&self, idx: usize) -> Result<Vec<u8>> {
        let e = self.entries.get(idx).with_context(|| t!("sync-msg-missing-msg-stream"))?;
        let size = (e.size as usize).min(MAX_STREAM_BYTES);
        if size == 0 || e.start_sector == ENDOFCHAIN || e.start_sector == FREESECT {
            return Ok(Vec::new());
        }
        let left = self.budget.get();
        ensure!(size <= left, t!("sync-msg-this-msg-file-is-larger-than"));
        self.budget.set(left - size);
        let mut data = if e.size < self.mini_cutoff as u64 && !self.mini_stream.is_empty() {
            read_mini_chain(
                &self.mini_stream,
                self.mini_sector_size,
                &self.mini_fat,
                e.start_sector,
                size,
            )?
        } else {
            read_fat_chain(self.raw, self.sector_size, &self.fat, e.start_sector, size)?
        };
        data.truncate(size);
        Ok(data)
    }
}

fn sector_slice(raw: &[u8], sector_size: usize, sector: u32) -> Result<&[u8]> {
    // Sector 0 starts immediately after the 512-byte header (or 1 sector for v4).
    let header_len = sector_size.max(512);
    let offset = header_len
        .checked_add((sector as usize).checked_mul(sector_size).with_context(|| t!("sync-msg-sector-overflow"))?)
        .with_context(|| t!("sync-msg-sector-overflow"))?;
    let end = offset.checked_add(sector_size).with_context(|| t!("sync-msg-sector-overflow"))?;
    raw.get(offset..end).with_context(|| t!("sync-msg-truncated-msg-sector"))
}

fn read_fat_chain(
    raw: &[u8],
    sector_size: usize,
    fat: &[u32],
    start: u32,
    limit: usize,
) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut cur = start;
    let mut hops = 0usize;
    let max_hops = (limit / sector_size + 2).min(fat.len().max(1) + 1);
    // A chain that comes back to a sector loops: it ends there.
    let mut seen = std::collections::HashSet::new();
    while cur != ENDOFCHAIN
        && cur != FREESECT
        && out.len() < limit
        && hops < max_hops
        && seen.insert(cur)
    {
        let slice = sector_slice(raw, sector_size, cur)?;
        let take = (limit - out.len()).min(slice.len());
        out.extend_from_slice(&slice[..take]);
        cur = *fat.get(cur as usize).unwrap_or(&ENDOFCHAIN);
        hops += 1;
    }
    Ok(out)
}

fn read_mini_chain(
    mini_stream: &[u8],
    mini_sector_size: usize,
    mini_fat: &[u32],
    start: u32,
    limit: usize,
) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut cur = start;
    let mut hops = 0usize;
    let max_hops = (limit / mini_sector_size + 2).min(mini_fat.len().max(1) + 1);
    let mut seen = std::collections::HashSet::new();
    while cur != ENDOFCHAIN
        && cur != FREESECT
        && out.len() < limit
        && hops < max_hops
        && seen.insert(cur)
    {
        let off = (cur as usize)
            .checked_mul(mini_sector_size)
            .with_context(|| t!("sync-msg-mini-sector-overflow"))?;
        let end = off
            .checked_add(mini_sector_size)
            .with_context(|| t!("sync-msg-mini-sector-overflow"))?;
        let Some(slice) = mini_stream.get(off..end) else {
            break;
        };
        let take = (limit - out.len()).min(slice.len());
        out.extend_from_slice(&slice[..take]);
        cur = *mini_fat.get(cur as usize).unwrap_or(&ENDOFCHAIN);
        hops += 1;
    }
    Ok(out)
}

fn decode_utf16le(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&c| u16::from_le_bytes(c))
        .take_while(|&u| u != 0)
        .collect();
    String::from_utf16_lossy(&units)
}

fn decode_ansi(bytes: &[u8]) -> String {
    let trimmed = match bytes.iter().position(|&b| b == 0) {
        Some(n) => &bytes[..n],
        None => bytes,
    };
    if let Ok(s) = std::str::from_utf8(trimmed) {
        s.to_owned()
    } else {
        encoding_rs::WINDOWS_1252.decode(trimmed).0.into_owned()
    }
}

/// Parses a `__substg1.0_<tag><type>` stream name into `(prop_tag, prop_type)`.
fn parse_prop_name(name: &str) -> Option<(u16, u16)> {
    let rest = name
        .strip_prefix("__substg1.0_")
        .or_else(|| name.strip_prefix("__SUBSTG1.0_"))?;
    // Names come from the file: `get` rather than slicing, which would panic
    // inside a character that isn't ASCII.
    let tag = u16::from_str_radix(rest.get(..4)?, 16).ok()?;
    let typ = u16::from_str_radix(rest.get(4..8)?, 16).ok()?;
    Some((tag, typ))
}

/// Fixed-length properties from `__properties_version1.0` (header is 32 bytes
/// in root storage, 24 bytes in embedded message, 8 bytes in recip/attach).
fn parse_fixed_props(bytes: &[u8], header_len: usize) -> HashMap<u16, (u16, u64)> {
    let mut out = HashMap::new();
    let Some(body) = bytes.get(header_len..) else {
        return out;
    };
    for chunk in body.as_chunks::<16>().0 {
        let typ = u16::from_le_bytes([chunk[0], chunk[1]]);
        let tag = u16::from_le_bytes([chunk[2], chunk[3]]);
        let val = u64::from_le_bytes(chunk[8..16].try_into().unwrap_or([0; 8]));
        out.insert(tag, (typ, val));
    }
    out
}

/// Converts a 64-bit Windows FILETIME (100-ns ticks since 1601-01-01) to an
/// RFC 2822 date string.
fn filetime_to_rfc2822(ft: u64) -> Option<String> {
    const EPOCH_DIFF_SECS: u64 = 11_644_473_600;
    let secs = ft / 10_000_000;
    let unix = secs.checked_sub(EPOCH_DIFF_SECS)? as i64;
    let dt = chrono::DateTime::from_timestamp(unix, 0)?;
    Some(dt.to_rfc2822())
}

fn read_props(cf: &CompoundFile<'_>, storage_idx: usize) -> (HashMap<u16, String>, HashMap<u16, Vec<u8>>) {
    let mut strings = HashMap::new();
    let mut binaries = HashMap::new();
    for child in cf.children(storage_idx) {
        let entry = &cf.entries[child];
        if entry.obj_type != 2 {
            continue;
        }
        let Some((tag, typ)) = parse_prop_name(&entry.name) else {
            continue;
        };
        let Ok(data) = cf.read_stream(child) else {
            continue;
        };
        match typ {
            // PtypString (UTF-16LE) — preferred over PtypString8 if both exist.
            0x001F => {
                strings.insert(tag, decode_utf16le(&data));
            }
            // PtypString8 (ANSI / UTF-8)
            0x001E => {
                strings.entry(tag).or_insert_with(|| decode_ansi(&data));
            }
            // PtypBinary
            0x0102 => {
                binaries.insert(tag, data);
            }
            _ => {}
        }
    }
    (strings, binaries)
}

fn format_mailbox(name: &str, email: &str) -> String {
    let name = name.trim();
    let email = email.trim();
    if email.is_empty() {
        return name.to_owned();
    }
    if name.is_empty() || name.eq_ignore_ascii_case(email) {
        email.to_owned()
    } else {
        let safe = name.replace(['"', '\r', '\n'], "");
        format!("\"{safe}\" <{email}>")
    }
}

fn parse_storage(cf: &CompoundFile<'_>, storage_idx: usize, depth: usize) -> Result<ParsedMsg> {
    ensure!(
        cf.parsed.borrow_mut().insert(storage_idx),
        t!("sync-msg-an-embedded-message-in-this-msg")
    );
    let (strings, binaries) = read_props(cf, storage_idx);

    let mut fixed = HashMap::new();
    let mut recip_storages = Vec::new();
    let mut attach_storages = Vec::new();

    for child in cf.children(storage_idx) {
        let e = &cf.entries[child];
        if e.obj_type == 2 && e.name.eq_ignore_ascii_case("__properties_version1.0") {
            if let Ok(bytes) = cf.read_stream(child) {
                let header_len = if storage_idx == 0 { 32 } else { 24 };
                fixed = parse_fixed_props(&bytes, header_len);
            }
        } else if e.obj_type == 1 {
            let lower = e.name.to_ascii_lowercase();
            if lower.starts_with("__recip_version1.0_#") {
                recip_storages.push((e.name.clone(), child));
            } else if lower.starts_with("__attach_version1.0_#") {
                attach_storages.push((e.name.clone(), child));
            }
        }
    }
    recip_storages.sort_by(|a, b| a.0.cmp(&b.0));
    attach_storages.sort_by(|a, b| a.0.cmp(&b.0));
    recip_storages.truncate(MAX_PARTS);
    attach_storages.truncate(MAX_PARTS);

    let get_str = |tag: u16| strings.get(&tag).map(String::as_str).unwrap_or("").trim().to_owned();

    // Parse transport headers if present (often carries original Date, Message-ID, References, From).
    let mut hdr_from = String::new();
    let mut hdr_to = String::new();
    let mut hdr_cc = String::new();
    let mut hdr_date = None;
    let mut hdr_mid = String::new();
    let mut hdr_irt = String::new();
    let mut hdr_refs = String::new();
    let transport = get_str(0x007D);
    if !transport.is_empty()
        && let Ok((headers, _)) = mailparse::parse_headers(transport.as_bytes())
    {
        use mailparse::MailHeaderMap as _;
        hdr_from = headers.get_first_value("From").unwrap_or_default();
        hdr_to = headers.get_first_value("To").unwrap_or_default();
        hdr_cc = headers.get_first_value("Cc").unwrap_or_default();
        hdr_date = headers.get_first_value("Date");
        hdr_mid = headers.get_first_value("Message-ID").unwrap_or_default();
        hdr_irt = headers.get_first_value("In-Reply-To").unwrap_or_default();
        hdr_refs = headers.get_first_value("References").unwrap_or_default();
    }

    let sender_name = [0x0C1A, 0x0042]
        .into_iter()
        .map(get_str)
        .find(|s| !s.is_empty())
        .unwrap_or_default();
    let sender_email = [0x5D01, 0x0C1F, 0x0065]
        .into_iter()
        .map(get_str)
        .find(|s| s.contains('@'))
        .or_else(|| {
            [0x0C1F, 0x0065]
                .into_iter()
                .map(get_str)
                .find(|s| !s.is_empty())
        })
        .unwrap_or_default();
    let from = if !sender_email.is_empty() && sender_email.contains('@') {
        format_mailbox(&sender_name, &sender_email)
    } else if !hdr_from.is_empty() {
        hdr_from
    } else {
        format_mailbox(&sender_name, &sender_email)
    };

    // Recipients from __recip_version1.0 storages, grouped by PR_RECIPIENT_TYPE (0x0C15).
    let mut to_list = Vec::new();
    let mut cc_list = Vec::new();
    let mut bcc_list = Vec::new();
    for (_, ridx) in recip_storages {
        let (rstr, _) = read_props(cf, ridx);
        let mut rfixed = HashMap::new();
        for c in cf.children(ridx) {
            if cf.entries[c].obj_type == 2
                && cf.entries[c].name.eq_ignore_ascii_case("__properties_version1.0")
                && let Ok(b) = cf.read_stream(c)
            {
                rfixed = parse_fixed_props(&b, 8);
            }
        }
        let rname = rstr.get(&0x3001).map(String::as_str).unwrap_or("");
        let remail = [0x39FE, 0x5D01, 0x3003]
            .into_iter()
            .filter_map(|t| rstr.get(&t))
            .find(|s| s.contains('@'))
            .or_else(|| rstr.get(&0x3003))
            .map(String::as_str)
            .unwrap_or("");
        let formatted = format_mailbox(rname, remail);
        if formatted.is_empty() {
            continue;
        }
        let rtype = rfixed.get(&0x0C15).map(|(_, v)| *v as u32).unwrap_or(1);
        match rtype {
            2 => cc_list.push(formatted),
            3 => bcc_list.push(formatted),
            _ => to_list.push(formatted),
        }
    }

    let to = if !to_list.is_empty() {
        to_list.join(", ")
    } else if !hdr_to.is_empty() {
        hdr_to
    } else {
        get_str(0x0E04)
    };
    let cc = if !cc_list.is_empty() {
        cc_list.join(", ")
    } else if !hdr_cc.is_empty() {
        hdr_cc
    } else {
        get_str(0x0E03)
    };
    let bcc = if !bcc_list.is_empty() {
        bcc_list.join(", ")
    } else {
        get_str(0x0E02)
    };

    let date = hdr_date.or_else(|| {
        // PR_CLIENT_SUBMIT_TIME (0x0039) or PR_MESSAGE_DELIVERY_TIME (0x0E06)
        fixed
            .get(&0x0039)
            .or_else(|| fixed.get(&0x0E06))
            .and_then(|(_, ft)| filetime_to_rfc2822(*ft))
    });

    let message_id = {
        let mid = get_str(0x1035);
        if !mid.is_empty() { mid } else { hdr_mid }
    };
    let subject = get_str(0x0037);
    let mut plain = get_str(0x1000);
    let mut html = strings
        .get(&0x1013)
        .cloned()
        .or_else(|| binaries.get(&0x1013).map(|b| decode_ansi(b)))
        .unwrap_or_default();

    // If neither HTML nor plain body is present, try PR_RTF_COMPRESSED (0x1009).
    if html.trim().is_empty()
        && let Some(rtf_comp) = binaries.get(&0x1009)
        && let Ok(rtf) = decompress_rtf(rtf_comp)
    {
        if let Some(extracted_html) = extract_html_from_rtf(&rtf) {
            html = extracted_html;
        } else if plain.trim().is_empty() {
            plain = strip_rtf_to_text(&rtf);
        }
    }

    // Attachments.
    let mut attachments = Vec::new();
    for (_, aidx) in attach_storages {
        let (astr, abin) = read_props(cf, aidx);
        let name = [0x3707, 0x3704, 0x3001]
            .into_iter()
            .filter_map(|t| astr.get(&t))
            .map(|s| s.trim())
            .find(|s| !s.is_empty())
            .unwrap_or("attachment")
            .to_owned();
        let content_id = astr
            .get(&0x3712)
            .map(|s| s.trim().trim_matches(['<', '>']).to_owned())
            .unwrap_or_default();

        if let Some(data) = abin.get(&0x3701) {
            let mime = astr
                .get(&0x370E)
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| {
                    mime_guess::from_path(&name)
                        .first_or_octet_stream()
                        .essence_str()
                        .to_owned()
                });
            attachments.push(MsgAttachment {
                name,
                mime,
                content_id,
                data: data.clone(),
            });
            continue;
        }

        // Embedded .msg attachment (__substg1.0_3701000D storage).
        for c in cf.children(aidx) {
            let ce = &cf.entries[c];
            if ce.obj_type == 1 && ce.name.eq_ignore_ascii_case("__substg1.0_3701000D") {
                if depth < MAX_DEPTH
                    && let Ok(nested) = parse_storage(cf, c, depth + 1)
                {
                    let eml = build_eml(&nested);
                    let eml_name = if name.to_ascii_lowercase().ends_with(".eml") {
                        name
                    } else {
                        format!("{name}.eml")
                    };
                    attachments.push(MsgAttachment {
                        name: eml_name,
                        mime: "message/rfc822".into(),
                        content_id: String::new(),
                        data: eml,
                    });
                }
                break;
            }
        }
    }

    Ok(ParsedMsg {
        subject,
        from,
        to,
        cc,
        bcc,
        date,
        message_id,
        in_reply_to: hdr_irt,
        references: hdr_refs,
        plain,
        html,
        attachments,
    })
}

/// Decompresses Outlook's LZFu-compressed RTF (`PR_RTF_COMPRESSED`, MS-OXRTFCP).
fn decompress_rtf(data: &[u8]) -> Result<String> {
    ensure!(data.len() >= 16, t!("sync-msg-truncated-compressed-rtf"));
    let raw_size = u32_le(data, 4)? as usize;
    let magic = &data[8..12];
    ensure!(raw_size <= MAX_STREAM_BYTES, t!("sync-msg-compressed-rtf-too-large"));
    if magic == b"MELA" {
        let end = (16 + raw_size).min(data.len());
        return Ok(decode_ansi(&data[16..end]));
    }
    if magic != b"LZFu" {
        bail!(t!("sync-msg-unsupported-compressed-rtf-magic"));
    }
    // Pre-populated 4096-byte ring dictionary (MS-OXRTFCP §2.1.3.1).
    const INIT: &[u8] = b"{\\rtf1\\ansi\\mac\\deff0\\deftab720{\\fonttbl;}{\\f0\\fnil \\froman \\fswiss \\fmodern \\fscript \\fdecor MS Sans SerifSymbolArialTimes New RomanCourier{\\colortbl\\red0\\green0\\blue0\r\n\\par \\pard\\plain\\f0\\fs20\\b\\i\\u\\tab\\tx";
    let mut dict = [0u8; 4096];
    dict[..INIT.len()].copy_from_slice(INIT);
    let mut write_pos = INIT.len();
    let mut out = Vec::with_capacity(raw_size.min(1024 * 1024));
    let mut i = 16usize;
    'outer: while i < data.len() && out.len() < raw_size {
        let flags = data[i];
        i += 1;
        for bit in 0..8 {
            if i >= data.len() || out.len() >= raw_size {
                break 'outer;
            }
            if (flags >> bit) & 1 == 0 {
                let b = data[i];
                i += 1;
                out.push(b);
                dict[write_pos & 4095] = b;
                write_pos = (write_pos + 1) & 4095;
            } else {
                if i + 1 >= data.len() {
                    break 'outer;
                }
                let hi = data[i] as usize;
                let lo = data[i + 1] as usize;
                i += 2;
                let offset = (hi << 4) | (lo >> 4);
                let length = (lo & 0x0F) + 2;
                if offset == write_pos {
                    break 'outer;
                }
                for k in 0..length {
                    let b = dict[(offset + k) & 4095];
                    out.push(b);
                    dict[write_pos & 4095] = b;
                    write_pos = (write_pos + 1) & 4095;
                    if out.len() >= raw_size {
                        break;
                    }
                }
            }
        }
    }
    Ok(decode_ansi(&out))
}

/// Extracts encapsulated HTML from `{\fromhtml1 ... {\*\htmltag ...}}` RTF.
fn extract_html_from_rtf(rtf: &str) -> Option<String> {
    if !rtf.contains("\\fromhtml") {
        return None;
    }
    let mut out = String::new();
    let bytes = rtf.as_bytes();
    let mut i = 0;
    let mut in_htmltag = false;
    let mut tag_depth = 0usize;
    let mut skip_depth: Option<usize> = None;
    let mut depth = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'{' => {
                depth += 1;
                if bytes[i..].starts_with(b"{\\*\\htmltag") {
                    in_htmltag = true;
                    tag_depth = depth;
                    i += "{\\*\\htmltag".len();
                    while i < bytes.len() && bytes[i].is_ascii_digit() {
                        i += 1;
                    }
                    if i < bytes.len() && bytes[i] == b' ' {
                        i += 1;
                    }
                    continue;
                }
                if (bytes[i..].starts_with(b"{\\*\\mhtmltag") || bytes[i..].starts_with(b"{\\*\\"))
                    && skip_depth.is_none()
                    && !in_htmltag
                {
                    skip_depth = Some(depth);
                }
                i += 1;
            }
            b'}' => {
                if in_htmltag && depth == tag_depth {
                    in_htmltag = false;
                }
                if skip_depth == Some(depth) {
                    skip_depth = None;
                }
                depth = depth.saturating_sub(1);
                i += 1;
            }
            b'\\' => {
                i += 1;
                if i >= bytes.len() {
                    break;
                }
                if matches!(bytes[i], b'\\' | b'{' | b'}') {
                    if skip_depth.is_none() {
                        out.push(bytes[i] as char);
                    }
                    i += 1;
                } else if bytes[i] == b'\'' {
                    // `\'hh`: a byte in the document's code page. The two
                    // digits are read with `get`, as what follows may not be
                    // ASCII (slicing inside a character would panic).
                    match rtf.get(i + 1..i + 3).map(|h| u8::from_str_radix(h, 16)) {
                        Some(Ok(val)) => {
                            if skip_depth.is_none() {
                                out.push_str(&encoding_rs::WINDOWS_1252.decode(&[val]).0);
                            }
                            i += 3;
                        }
                        _ => i += 1,
                    }
                } else {
                    let start = i;
                    while i < bytes.len() && bytes[i].is_ascii_alphabetic() {
                        i += 1;
                    }
                    let word = &rtf[start..i];
                    if i < bytes.len() && (bytes[i] == b'-' || bytes[i].is_ascii_digit()) {
                        i += 1;
                        while i < bytes.len() && bytes[i].is_ascii_digit() {
                            i += 1;
                        }
                    }
                    if i < bytes.len() && bytes[i] == b' ' {
                        i += 1;
                    }
                    if skip_depth.is_none() {
                        match word {
                            "par" | "line" => out.push('\n'),
                            "tab" => out.push('\t'),
                            _ => {}
                        }
                    }
                }
            }
            b'\r' | b'\n' => i += 1,
            c if c.is_ascii() => {
                if skip_depth.is_none() {
                    out.push(c as char);
                }
                i += 1;
            }
            // Text that isn't ASCII (the RTF was UTF-8): the whole character,
            // which also keeps `i` on a character boundary.
            _ => {
                let ch = rtf[i..].chars().next().unwrap_or('\u{FFFD}');
                if skip_depth.is_none() {
                    out.push(ch);
                }
                i += ch.len_utf8().max(1);
            }
        }
    }
    let trimmed = out.trim();
    (!trimmed.is_empty() && trimmed.contains('<')).then(|| trimmed.to_owned())
}

fn strip_rtf_to_text(rtf: &str) -> String {
    let mut out = String::new();
    let bytes = rtf.as_bytes();
    let mut i = 0;
    let mut depth = 0usize;
    let mut skip_depth: Option<usize> = None;
    while i < bytes.len() {
        match bytes[i] {
            b'{' => {
                depth += 1;
                if skip_depth.is_none()
                    && (bytes[i..].starts_with(b"{\\fonttbl")
                        || bytes[i..].starts_with(b"{\\colortbl")
                        || bytes[i..].starts_with(b"{\\stylesheet")
                        || bytes[i..].starts_with(b"{\\*\\"))
                {
                    skip_depth = Some(depth);
                }
                i += 1;
            }
            b'}' => {
                if skip_depth == Some(depth) {
                    skip_depth = None;
                }
                depth = depth.saturating_sub(1);
                i += 1;
            }
            b'\\' => {
                i += 1;
                if i >= bytes.len() {
                    break;
                }
                if matches!(bytes[i], b'\\' | b'{' | b'}') {
                    if skip_depth.is_none() {
                        out.push(bytes[i] as char);
                    }
                    i += 1;
                } else if bytes[i] == b'\'' {
                    // `\'hh`: a byte in the document's code page. The two
                    // digits are read with `get`, as what follows may not be
                    // ASCII (slicing inside a character would panic).
                    match rtf.get(i + 1..i + 3).map(|h| u8::from_str_radix(h, 16)) {
                        Some(Ok(val)) => {
                            if skip_depth.is_none() {
                                out.push_str(&encoding_rs::WINDOWS_1252.decode(&[val]).0);
                            }
                            i += 3;
                        }
                        _ => i += 1,
                    }
                } else {
                    let start = i;
                    while i < bytes.len() && bytes[i].is_ascii_alphabetic() {
                        i += 1;
                    }
                    let word = &rtf[start..i];
                    if i < bytes.len() && (bytes[i] == b'-' || bytes[i].is_ascii_digit()) {
                        i += 1;
                        while i < bytes.len() && bytes[i].is_ascii_digit() {
                            i += 1;
                        }
                    }
                    if i < bytes.len() && bytes[i] == b' ' {
                        i += 1;
                    }
                    if skip_depth.is_none() {
                        match word {
                            "par" | "line" => out.push('\n'),
                            "tab" => out.push('\t'),
                            _ => {}
                        }
                    }
                }
            }
            b'\r' | b'\n' => i += 1,
            c if c.is_ascii() => {
                if skip_depth.is_none() {
                    out.push(c as char);
                }
                i += 1;
            }
            // Text that isn't ASCII (the RTF was UTF-8): the whole character,
            // which also keeps `i` on a character boundary.
            _ => {
                let ch = rtf[i..].chars().next().unwrap_or('\u{FFFD}');
                if skip_depth.is_none() {
                    out.push(ch);
                }
                i += ch.len_utf8().max(1);
            }
        }
    }
    out.trim().to_owned()
}

fn sanitize_header(v: &str) -> String {
    v.replace(['\r', '\n'], " ").trim().to_owned()
}

fn encode_header_utf8(v: &str) -> String {
    let clean = sanitize_header(v);
    if clean.is_ascii() {
        clean
    } else {
        format!(
            "=?UTF-8?B?{}?=",
            base64::engine::general_purpose::STANDARD.encode(clean.as_bytes())
        )
    }
}

fn b64_wrapped(bytes: &[u8]) -> String {
    let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
    let mut out = String::with_capacity(encoded.len() + encoded.len() / 76 * 2 + 2);
    for chunk in encoded.as_bytes().chunks(76) {
        out.push_str(std::str::from_utf8(chunk).unwrap_or(""));
        out.push_str("\r\n");
    }
    out
}

/// Builds a standard RFC 5322 MIME message from `ParsedMsg`.
fn build_eml(msg: &ParsedMsg) -> Vec<u8> {
    let mut out = String::new();
    let mut push_hdr = |name: &str, value: &str, encode: bool| {
        let v = sanitize_header(value);
        if !v.is_empty() {
            let v = if encode { encode_header_utf8(&v) } else { v };
            out.push_str(&format!("{name}: {v}\r\n"));
        }
    };
    push_hdr("From", &msg.from, false);
    push_hdr("To", &msg.to, false);
    push_hdr("Cc", &msg.cc, false);
    push_hdr("Bcc", &msg.bcc, false);
    push_hdr("Subject", &msg.subject, true);
    if let Some(date) = &msg.date {
        push_hdr("Date", date, false);
    }
    push_hdr("Message-ID", &msg.message_id, false);
    push_hdr("In-Reply-To", &msg.in_reply_to, false);
    push_hdr("References", &msg.references, false);
    out.push_str("MIME-Version: 1.0\r\n");

    let has_plain = !msg.plain.is_empty();
    let has_html = !msg.html.is_empty();
    let (inline_atts, regular_atts): (Vec<&MsgAttachment>, Vec<&MsgAttachment>) = msg
        .attachments
        .iter()
        .partition(|a| !a.content_id.is_empty() && msg.html.contains(&a.content_id));

    let write_body = |buf: &mut String| {
        if has_plain && has_html {
            let alt = "picacho-msg-alt";
            buf.push_str(&format!(
                "Content-Type: multipart/alternative; boundary=\"{alt}\"\r\n\r\n"
            ));
            buf.push_str(&format!(
                "--{alt}\r\nContent-Type: text/plain; charset=\"utf-8\"\r\nContent-Transfer-Encoding: base64\r\n\r\n"
            ));
            buf.push_str(&b64_wrapped(msg.plain.as_bytes()));
            buf.push_str(&format!(
                "--{alt}\r\nContent-Type: text/html; charset=\"utf-8\"\r\nContent-Transfer-Encoding: base64\r\n\r\n"
            ));
            buf.push_str(&b64_wrapped(msg.html.as_bytes()));
            buf.push_str(&format!("--{alt}--\r\n"));
        } else if has_html {
            buf.push_str(
                "Content-Type: text/html; charset=\"utf-8\"\r\nContent-Transfer-Encoding: base64\r\n\r\n",
            );
            buf.push_str(&b64_wrapped(msg.html.as_bytes()));
        } else {
            buf.push_str(
                "Content-Type: text/plain; charset=\"utf-8\"\r\nContent-Transfer-Encoding: base64\r\n\r\n",
            );
            buf.push_str(&b64_wrapped(msg.plain.as_bytes()));
        }
    };

    let write_related = |buf: &mut String| {
        if inline_atts.is_empty() {
            write_body(buf);
            return;
        }
        let rel = "picacho-msg-rel";
        buf.push_str(&format!(
            "Content-Type: multipart/related; boundary=\"{rel}\"\r\n\r\n"
        ));
        buf.push_str(&format!("--{rel}\r\n"));
        write_body(buf);
        for a in &inline_atts {
            let safe_cid = sanitize_header(&a.content_id);
            buf.push_str(&format!(
                "--{rel}\r\nContent-Type: {}\r\n\
                 Content-Transfer-Encoding: base64\r\n\
                 Content-Disposition: inline\r\n\
                 Content-ID: <{safe_cid}>\r\n\r\n",
                sanitize_header(&a.mime)
            ));
            buf.push_str(&b64_wrapped(&a.data));
        }
        buf.push_str(&format!("--{rel}--\r\n"));
    };

    if regular_atts.is_empty() {
        write_related(&mut out);
    } else {
        let mixed = "picacho-msg-mixed";
        out.push_str(&format!(
            "Content-Type: multipart/mixed; boundary=\"{mixed}\"\r\n\r\n"
        ));
        out.push_str(&format!("--{mixed}\r\n"));
        write_related(&mut out);
        for a in &regular_atts {
            let safe_name = sanitize_header(&a.name).replace('"', "");
            out.push_str(&format!(
                "--{mixed}\r\nContent-Type: {}; name=\"{safe_name}\"\r\n\
                 Content-Transfer-Encoding: base64\r\n\
                 Content-Disposition: attachment; filename=\"{safe_name}\"\r\n\r\n",
                sanitize_header(&a.mime)
            ));
            out.push_str(&b64_wrapped(&a.data));
        }
        out.push_str(&format!("--{mixed}--\r\n"));
    }

    out.into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal CFBF v3 builder for unit tests: supports storages and streams
    /// (both MiniStream and regular FAT sectors).
    struct CfbfBuilder {
        nodes: Vec<TestNode>,
    }
    struct TestNode {
        name: String,
        is_storage: bool,
        data: Vec<u8>,
        children: Vec<usize>,
    }
    impl CfbfBuilder {
        fn new() -> Self {
            Self {
                nodes: vec![TestNode {
                    name: "Root Entry".into(),
                    is_storage: true,
                    data: Vec::new(),
                    children: Vec::new(),
                }],
            }
        }
        fn add_storage(&mut self, parent: usize, name: &str) -> usize {
            let idx = self.nodes.len();
            self.nodes.push(TestNode {
                name: name.into(),
                is_storage: true,
                data: Vec::new(),
                children: Vec::new(),
            });
            self.nodes[parent].children.push(idx);
            idx
        }
        fn add_stream(&mut self, parent: usize, name: &str, data: Vec<u8>) {
            let idx = self.nodes.len();
            self.nodes.push(TestNode {
                name: name.into(),
                is_storage: false,
                data,
                children: Vec::new(),
            });
            self.nodes[parent].children.push(idx);
        }
        fn utf16(s: &str) -> Vec<u8> {
            s.encode_utf16().flat_map(u16::to_le_bytes).collect()
        }
        fn build(self) -> Vec<u8> {
            // Put all streams directly into regular 512-byte sectors (mini_cutoff = 0)
            // OR mini-stream if small: use mini_cutoff = 4096 to exercise MiniStream too!
            let mut sectors: Vec<[u8; 512]> = Vec::new();
            let mut fat: Vec<u32> = Vec::new();
            let mut mini_stream: Vec<u8> = Vec::new();
            let mut mini_fat: Vec<u32> = Vec::new();

            let mut alloc_regular = |data: &[u8]| -> u32 {
                if data.is_empty() {
                    return ENDOFCHAIN;
                }
                let start = sectors.len() as u32;
                let chunks: Vec<&[u8]> = data.chunks(512).collect();
                for (i, ch) in chunks.iter().enumerate() {
                    let mut sec = [0u8; 512];
                    sec[..ch.len()].copy_from_slice(ch);
                    sectors.push(sec);
                    fat.push(if i + 1 < chunks.len() {
                        start + i as u32 + 1
                    } else {
                        ENDOFCHAIN
                    });
                }
                start
            };

            let mut starts = vec![ENDOFCHAIN; self.nodes.len()];
            for (idx, node) in self.nodes.iter().enumerate().skip(1) {
                if !node.is_storage && !node.data.is_empty() {
                    if node.data.len() < 4096 {
                        let start = (mini_stream.len() / 64) as u32;
                        let chunks: Vec<&[u8]> = node.data.chunks(64).collect();
                        for (i, ch) in chunks.iter().enumerate() {
                            let mut ms = [0u8; 64];
                            ms[..ch.len()].copy_from_slice(ch);
                            mini_stream.extend_from_slice(&ms);
                            mini_fat.push(if i + 1 < chunks.len() {
                                start + i as u32 + 1
                            } else {
                                ENDOFCHAIN
                            });
                        }
                        starts[idx] = start;
                    } else {
                        starts[idx] = alloc_regular(&node.data);
                    }
                }
            }

            // Root entry holds the mini-stream.
            let mini_stream_len = mini_stream.len() as u32;
            let mini_stream_start = alloc_regular(&mini_stream);
            starts[0] = mini_stream_start;

            // MiniFAT sectors.
            let mini_fat_bytes: Vec<u8> = mini_fat.iter().flat_map(|v| v.to_le_bytes()).collect();
            let first_mini_fat = alloc_regular(&mini_fat_bytes);
            let num_mini_fat = mini_fat_bytes.len().div_ceil(512) as u32;

            // Build directory entries (link siblings linearly through `right`).
            let mut dir_bytes = Vec::new();
            for (idx, node) in self.nodes.iter().enumerate() {
                let mut entry = [0u8; 128];
                let mut name_utf16: Vec<u16> = node.name.encode_utf16().collect();
                name_utf16.push(0);
                let name_bytes: Vec<u8> = name_utf16.iter().flat_map(|u| u.to_le_bytes()).collect();
                let n = name_bytes.len().min(64);
                entry[..n].copy_from_slice(&name_bytes[..n]);
                entry[64..66].copy_from_slice(&(n as u16).to_le_bytes());
                entry[66] = if idx == 0 {
                    5
                } else if node.is_storage {
                    1
                } else {
                    2
                };
                entry[67] = 1; // black
                // Find right sibling inside parent's children.
                let right = self
                    .nodes
                    .iter()
                    .find_map(|p| {
                        let pos = p.children.iter().position(|&c| c == idx)?;
                        p.children.get(pos + 1).map(|&next| next as u32)
                    })
                    .unwrap_or(NOSTREAM);
                let child = node.children.first().map(|&c| c as u32).unwrap_or(NOSTREAM);
                entry[68..72].copy_from_slice(&NOSTREAM.to_le_bytes());
                entry[72..76].copy_from_slice(&right.to_le_bytes());
                entry[76..80].copy_from_slice(&child.to_le_bytes());
                entry[116..120].copy_from_slice(&starts[idx].to_le_bytes());
                let size = if idx == 0 {
                    mini_stream_len
                } else {
                    node.data.len() as u32
                };
                entry[120..124].copy_from_slice(&size.to_le_bytes());
                dir_bytes.extend_from_slice(&entry);
            }
            let first_dir = alloc_regular(&dir_bytes);

            // Allocate FAT sectors themselves (mark them FATSECT = 0xFFFFFFFD).
            let fat_count = (fat.len() + 2).div_ceil(128).max(1);
            let mut fat_sector_ids = Vec::new();
            for _ in 0..fat_count {
                fat_sector_ids.push(sectors.len() as u32);
                sectors.push([0u8; 512]);
                fat.push(0xFFFF_FFFD);
            }
            while fat.len() < fat_count * 128 {
                fat.push(FREESECT);
            }
            for (k, &sec_id) in fat_sector_ids.iter().enumerate() {
                for j in 0..128 {
                    let v = fat[k * 128 + j];
                    sectors[sec_id as usize][j * 4..j * 4 + 4].copy_from_slice(&v.to_le_bytes());
                }
            }

            // Build 512-byte header.
            let mut hdr = [0u8; 512];
            hdr[..8].copy_from_slice(&CFBF_MAGIC);
            hdr[24..26].copy_from_slice(&0x003Eu16.to_le_bytes()); // minor version
            hdr[26..28].copy_from_slice(&0x0003u16.to_le_bytes()); // major version 3
            hdr[28..30].copy_from_slice(&0xFFFEu16.to_le_bytes()); // little-endian
            hdr[30..32].copy_from_slice(&9u16.to_le_bytes()); // 512-byte sectors
            hdr[32..34].copy_from_slice(&6u16.to_le_bytes()); // 64-byte mini-sectors
            hdr[44..48].copy_from_slice(&(fat_count as u32).to_le_bytes());
            hdr[48..52].copy_from_slice(&first_dir.to_le_bytes());
            hdr[56..60].copy_from_slice(&4096u32.to_le_bytes()); // mini cutoff
            hdr[60..64].copy_from_slice(&first_mini_fat.to_le_bytes());
            hdr[64..68].copy_from_slice(&num_mini_fat.to_le_bytes());
            hdr[68..72].copy_from_slice(&ENDOFCHAIN.to_le_bytes());
            hdr[72..76].copy_from_slice(&0u32.to_le_bytes());
            for i in 0..109 {
                let v = fat_sector_ids.get(i).copied().unwrap_or(FREESECT);
                hdr[76 + i * 4..76 + i * 4 + 4].copy_from_slice(&v.to_le_bytes());
            }

            let mut out = Vec::with_capacity(512 + sectors.len() * 512);
            out.extend_from_slice(&hdr);
            for sec in sectors {
                out.extend_from_slice(&sec);
            }
            out
        }
    }

    #[test]
    fn outlook_msg_converts_headers_html_inline_image_and_attachment() -> Result<()> {
        let mut b = CfbfBuilder::new();
        b.add_stream(0, "__substg1.0_0037001F", CfbfBuilder::utf16("Quarterly résumé"));
        b.add_stream(0, "__substg1.0_0C1A001F", CfbfBuilder::utf16("Ana López"));
        b.add_stream(0, "__substg1.0_5D01001F", CfbfBuilder::utf16("ana@example.org"));
        b.add_stream(0, "__substg1.0_1000001F", CfbfBuilder::utf16("Plain body text."));
        // Large (>4096 bytes) HTML stream to exercise regular FAT sectors alongside MiniStream.
        let padding = "<!-- pad -->".repeat(400);
        let html = format!("<p>Hello <b>world</b> <img src=\"cid:logo@msg\"></p>{padding}");
        b.add_stream(0, "__substg1.0_10130102", html.into_bytes());

        // Recipient (To)
        let r = b.add_storage(0, "__recip_version1.0_#00000000");
        b.add_stream(r, "__substg1.0_3001001F", CfbfBuilder::utf16("Bo"));
        b.add_stream(r, "__substg1.0_39FE001F", CfbfBuilder::utf16("bo@example.org"));
        let mut rprops = vec![0u8; 24]; // 8-byte header + 16-byte PR_RECIPIENT_TYPE = 1 (To)
        rprops[8..10].copy_from_slice(&0x0003u16.to_le_bytes());
        rprops[10..12].copy_from_slice(&0x0C15u16.to_le_bytes());
        rprops[16..24].copy_from_slice(&1u64.to_le_bytes());
        b.add_stream(r, "__properties_version1.0", rprops);

        // Regular attachment
        let a1 = b.add_storage(0, "__attach_version1.0_#00000000");
        b.add_stream(a1, "__substg1.0_3707001F", CfbfBuilder::utf16("report.txt"));
        b.add_stream(a1, "__substg1.0_370E001F", CfbfBuilder::utf16("text/plain"));
        b.add_stream(a1, "__substg1.0_37010102", b"Report contents".to_vec());

        // Inline cid: image attachment
        let a2 = b.add_storage(0, "__attach_version1.0_#00000001");
        b.add_stream(a2, "__substg1.0_3707001F", CfbfBuilder::utf16("logo.png"));
        b.add_stream(a2, "__substg1.0_370E001F", CfbfBuilder::utf16("image/png"));
        b.add_stream(a2, "__substg1.0_3712001F", CfbfBuilder::utf16("logo@msg"));
        b.add_stream(a2, "__substg1.0_37010102", vec![0x89, b'P', b'N', b'G']);

        let raw = b.build();
        assert!(is_msg(&raw));
        let eml = to_eml(&raw)?;

        let account = crate::manage::model::Account::new(crate::manage::model::Provider::Imap);
        let parsed = crate::sync::mime::parse_message(
            &eml,
            &account,
            "file:test.msg",
            crate::manage::model::Folder::Inbox,
            "",
            0,
        )?;
        assert_eq!(parsed.subject, "Quarterly résumé");
        assert_eq!(parsed.from, "ana@example.org");
        assert!(parsed.to.contains("bo@example.org"));
        assert!(parsed.body.as_deref().unwrap_or("").contains("Plain body text."));
        assert!(parsed.html.as_deref().unwrap_or("").contains("<b>world</b>"));
        assert_eq!(parsed.attachments().len(), 1);
        assert_eq!(parsed.attachments()[0].name, "report.txt");
        assert!(parsed.inline.get("cid:logo@msg").is_some());
        Ok(())
    }

    #[test]
    fn rejects_non_ole2_files() {
        assert!(!is_msg(b"From: a@b\r\nSubject: Hi\r\n\r\nPlain eml"));
        assert!(to_eml(b"not an msg file").is_err());
    }

    /// Names and RTF from a crafted file must never panic (the indexer reads
    /// `.msg` attachments of received mail).
    #[test]
    fn crafted_names_and_rtf_with_non_ascii_text_do_not_panic() {
        assert_eq!(parse_prop_name("__substg1.0_ab€cdefg"), None);
        assert_eq!(parse_prop_name("__substg1.0_0037"), None);
        assert_eq!(parse_prop_name("__substg1.0_0037001F"), Some((0x0037, 0x001F)));
        for rtf in [
            "{\\rtf1 \\'€ caf\\'e9 naïve \\'a€}",
            "{\\rtf1\\fromhtml1 {\\*\\htmltag <p>}ünï\\'€{\\*\\htmltag </p>}}",
            "\\'",
            "\\'a",
        ] {
            let _ = strip_rtf_to_text(rtf);
            let _ = extract_html_from_rtf(rtf);
        }
        assert_eq!(strip_rtf_to_text("{\\rtf1 caf\\'e9 naïve}"), "café naïve");
        assert_eq!(
            extract_html_from_rtf("{\\rtf1\\fromhtml1 {\\*\\htmltag <p>}ünï{\\*\\htmltag </p>}}")
                .as_deref(),
            Some("<p>ünï</p>")
        );
    }

    /// A FAT chain that loops is read once around, not until the size cap.
    #[test]
    fn looping_sector_chains_end() {
        let raw = vec![0xABu8; 512 + 512 * 2];
        let fat = vec![1, 0]; // 0 -> 1 -> 0 -> ...
        let out = read_fat_chain(&raw, 512, &fat, 0, MAX_STREAM_BYTES).unwrap();
        assert_eq!(out.len(), 1024);
        let mini = vec![0u8; 64 * 2];
        let out = read_mini_chain(&mini, 64, &[0, 0], 0, MAX_STREAM_BYTES).unwrap();
        assert_eq!(out.len(), 64);
    }
}

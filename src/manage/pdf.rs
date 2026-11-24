//! A small PDF writer for saving a message as a PDF without a browser or
//! printer: its headers and text on A4 pages, in the PDF standard fonts
//! (Helvetica), wrapped to the page. Characters outside Windows-1252, which
//! those fonts cover, show as `?`; printing (which goes through the
//! system's browser) keeps the email's own formatting and every script.

const PAGE_W: f32 = 595.;
const PAGE_H: f32 = 842.;
const MARGIN: f32 = 54.;
const SIZE: f32 = 10.5;
const LEADING: f32 = 15.;
const TITLE_SIZE: f32 = 16.;

/// A line of the document: its text, and whether it is bold.
struct Line {
    text: Vec<u8>,
    bold: bool,
    size: f32,
}

/// Helvetica's advance widths (per 1000 units of the font size) for
/// Windows-1252 bytes 32–126; others count as the average.
const HELVETICA: [u16; 95] = [
    278, 278, 355, 556, 556, 889, 667, 191, 333, 333, 389, 584, 278, 333, 278, 278, 556, 556, 556,
    556, 556, 556, 556, 556, 556, 556, 278, 278, 584, 584, 584, 556, 1015, 667, 667, 722, 722, 667,
    611, 778, 722, 278, 500, 667, 556, 833, 722, 778, 667, 778, 722, 667, 611, 722, 667, 944, 667,
    667, 611, 278, 278, 278, 469, 556, 333, 556, 556, 500, 556, 556, 278, 556, 556, 222, 222, 500,
    222, 833, 556, 556, 556, 556, 333, 500, 278, 556, 500, 722, 500, 500, 500, 334, 260, 334, 584,
];

fn width(bytes: &[u8], size: f32, bold: bool) -> f32 {
    let units: u32 = bytes
        .iter()
        .map(|&b| match b {
            32..=126 => HELVETICA[(b - 32) as usize] as u32,
            _ => 556,
        })
        .sum();
    // Bold runs about 5% wider.
    units as f32 * size / 1000. * if bold { 1.05 } else { 1. }
}

/// `text` in Windows-1252, `?` for what it can't hold.
fn encode(text: &str) -> Vec<u8> {
    let (bytes, _, _) = encoding_rs::WINDOWS_1252.encode(text);
    // encoding_rs writes unmappable characters as HTML numeric references.
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'&'
            && bytes.get(i + 1) == Some(&b'#')
            && let Some(end) = bytes[i..].iter().position(|&b| b == b';')
        {
            out.push(b'?');
            i += end + 1;
            continue;
        }
        let b = bytes[i];
        out.push(if b == b'\t' { b' ' } else if b < 32 { b' ' } else { b });
        i += 1;
    }
    out
}

/// Wraps one paragraph into lines no wider than `room`.
fn wrap(text: &[u8], size: f32, bold: bool, room: f32) -> Vec<Vec<u8>> {
    let mut lines = Vec::new();
    let mut line: Vec<u8> = Vec::new();
    for word in text.split(|&b| b == b' ') {
        let mut candidate = line.clone();
        if !candidate.is_empty() {
            candidate.push(b' ');
        }
        candidate.extend_from_slice(word);
        if width(&candidate, size, bold) <= room || line.is_empty() && width(word, size, bold) <= room {
            line = candidate;
            continue;
        }
        if !line.is_empty() {
            lines.push(std::mem::take(&mut line));
        }
        // A word wider than the line is cut where it fills it.
        let mut rest = word;
        while width(rest, size, bold) > room {
            let mut n = rest.len();
            while n > 1 && width(&rest[..n], size, bold) > room {
                n -= 1;
            }
            lines.push(rest[..n].to_vec());
            rest = &rest[n..];
        }
        line = rest.to_vec();
    }
    lines.push(line);
    lines
}

fn escape(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len() + 8);
    for &b in bytes {
        if matches!(b, b'(' | b')' | b'\\') {
            out.push(b'\\');
        }
        out.push(b);
    }
    out
}

/// A PDF of a message: `title` (its subject) large, `headers` as
/// (name, value) in bold and plain, a rule, then `body`.
pub fn message_pdf(title: &str, headers: &[(&str, String)], body: &str) -> Vec<u8> {
    let room = PAGE_W - 2. * MARGIN;
    let mut lines: Vec<Line> = Vec::new();
    for l in wrap(&encode(title), TITLE_SIZE, true, room) {
        lines.push(Line { text: l, bold: true, size: TITLE_SIZE });
    }
    lines.push(Line { text: vec![], bold: false, size: SIZE });
    for (name, value) in headers {
        if value.trim().is_empty() {
            continue;
        }
        let text = encode(&format!("{name}: {value}"));
        for l in wrap(&text, SIZE, false, room) {
            lines.push(Line { text: l, bold: false, size: SIZE });
        }
    }
    // The rule under the headers is drawn where this empty bold line is.
    lines.push(Line { text: vec![], bold: true, size: 0. });
    for paragraph in body.replace("\r\n", "\n").split('\n') {
        for l in wrap(&encode(paragraph.trim_end()), SIZE, false, room) {
            lines.push(Line { text: l, bold: false, size: SIZE });
        }
    }
    // Pages of content streams.
    let mut pages: Vec<Vec<u8>> = Vec::new();
    let mut stream: Vec<u8> = Vec::new();
    let mut y = PAGE_H - MARGIN;
    for line in &lines {
        let lead = if line.size == TITLE_SIZE { TITLE_SIZE * 1.3 } else { LEADING };
        if y - lead < MARGIN {
            pages.push(std::mem::take(&mut stream));
            y = PAGE_H - MARGIN;
        }
        if line.size == 0. {
            // The rule.
            stream.extend_from_slice(
                format!(
                    "0.75 G 0.5 w {MARGIN} {:.1} m {:.1} {:.1} l S\n",
                    y - 4.,
                    PAGE_W - MARGIN,
                    y - 4.
                )
                .as_bytes(),
            );
            y -= LEADING;
            continue;
        }
        y -= lead;
        if line.text.is_empty() {
            continue;
        }
        let font = if line.bold { "F2" } else { "F1" };
        stream.extend_from_slice(
            format!("BT /{font} {} Tf {MARGIN} {y:.1} Td (", line.size).as_bytes(),
        );
        stream.extend_from_slice(&escape(&line.text));
        stream.extend_from_slice(b") Tj ET\n");
    }
    pages.push(stream);

    // Objects: 1 catalog, 2 pages, 3 and 4 fonts, then a page and its
    // contents for each page.
    let mut objects: Vec<Vec<u8>> = Vec::new();
    let kids: Vec<String> = (0..pages.len()).map(|i| format!("{} 0 R", 5 + 2 * i)).collect();
    objects.push(b"<< /Type /Catalog /Pages 2 0 R >>".to_vec());
    objects.push(
        format!(
            "<< /Type /Pages /Kids [{}] /Count {} >>",
            kids.join(" "),
            pages.len()
        )
        .into_bytes(),
    );
    objects.push(
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".to_vec(),
    );
    objects.push(
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica-Bold /Encoding /WinAnsiEncoding >>"
            .to_vec(),
    );
    for (i, content) in pages.iter().enumerate() {
        objects.push(
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {PAGE_W} {PAGE_H}] /Resources << /Font << /F1 3 0 R /F2 4 0 R >> >> /Contents {} 0 R >>",
                6 + 2 * i
            )
            .into_bytes(),
        );
        let mut obj = format!("<< /Length {} >>\nstream\n", content.len()).into_bytes();
        obj.extend_from_slice(content);
        obj.extend_from_slice(b"\nendstream");
        objects.push(obj);
    }
    let mut out = b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n".to_vec();
    let mut offsets = Vec::with_capacity(objects.len());
    for (i, obj) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(obj);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_long_message_makes_several_valid_pages() {
        let body = "A line of text that goes on for a while (with parentheses).\n".repeat(200);
        let pdf = message_pdf("Hello — world ✓", &[("From", "Ana <ana@example.com>".into())], &body);
        let text = String::from_utf8_lossy(&pdf);
        assert!(text.starts_with("%PDF-1.4"));
        assert!(text.ends_with("%%EOF\n"));
        assert!(text.contains("/Count 5") || text.contains("/Count 6"));
        assert!(text.contains("\\(with parentheses\\)"));
        // The xref's offsets point at the objects.
        let xref = text.rfind("startxref\n").unwrap();
        let at: usize = text[xref + 10..].lines().next().unwrap().parse().unwrap();
        assert!(pdf[at..].starts_with(b"xref"));
    }

    #[test]
    fn words_wider_than_the_line_are_cut() {
        let lines = wrap(&[b'w'; 400], SIZE, false, 200.);
        assert!(lines.len() > 1);
        assert!(lines.iter().all(|l| width(l, SIZE, false) <= 200.));
    }
}

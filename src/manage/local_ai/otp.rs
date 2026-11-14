//! Sign-in code detection: numbers that look like a code are found with a
//! pattern, and the local model decides from the words around each whether
//! it is the sign-in, verification or one-time code of the message.
//!
//! Candidates and features are built exactly as `models/otp/features.py`
//! builds them for training; `models/otp/parity.json` checks the two agree.
use super::runtime::Scorer;
use regex::Regex;
use std::borrow::Cow;
use std::sync::LazyLock;

/// The hashed feature space (a power of two).
pub const DIM: usize = 1 << 14;
const WINDOW: usize = 200;
const CONTEXT: usize = 8;
const NEAR: usize = 3;
const SUBJECT_WORDS: usize = 16;
const MAX_CANDIDATES: usize = 40;
const MAX_TEXT: usize = 20_000;
/// What a hidden code's letters and digits are shown as.
pub const BULLET: char = '●';

/// 4-10 digits, or uppercase letters and digits, or two groups of 3-4
/// digits split by a space or hyphen; optionally after a short uppercase
/// prefix and hyphen, as in Google's "G-123456".
static CANDIDATE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b(?:[A-Z]{1,3}-)?(?:\d{3,4}[ \-]\d{3,4}|[A-Z0-9]{4,10})\b").expect("pattern")
});

/// A message's text as the model sees it, and its candidates' places.
struct Text {
    subject_len: usize,
    chars: Vec<char>,
}

impl Text {
    fn new(subject: &str, body: &str) -> Self {
        let chars: Vec<char> = subject
            .chars()
            .chain(std::iter::once('\n'))
            .chain(body.chars())
            .take(MAX_TEXT)
            .collect();
        Self {
            subject_len: subject.chars().count(),
            chars,
        }
    }
    fn slice(&self, from: usize, to: usize) -> String {
        self.chars[from.min(self.chars.len())..to.min(self.chars.len())]
            .iter()
            .collect()
    }
    fn at(&self, i: usize) -> Option<char> {
        self.chars.get(i).copied()
    }

    /// (start, end) of each candidate, in characters.
    fn candidates(&self) -> Vec<(usize, usize)> {
        let text: String = self.chars.iter().collect();
        // Byte offset of each character, to turn matches into characters.
        let starts: Vec<usize> = text.char_indices().map(|(i, _)| i).collect();
        let char_at = |byte: usize| starts.partition_point(|&b| b < byte);
        let mut out = Vec::new();
        for m in CANDIDATE.find_iter(&text) {
            let (s, e) = (char_at(m.start()), char_at(m.end()));
            let code = m.as_str();
            let core = match code.find('-') {
                Some(p)
                    if (1..=3).contains(&p)
                        && code[..p].bytes().all(|b| b.is_ascii_uppercase()) =>
                {
                    &code[p + 1..]
                }
                _ => code,
            };
            if !core.chars().any(char::is_numeric) {
                continue;
            }
            // Part of a longer number: a phone number, a date, an amount.
            let digit = |c: Option<char>| c.is_some_and(char::is_numeric);
            let before = s.checked_sub(1).and_then(|i| self.at(i));
            let before2 = s.checked_sub(2).and_then(|i| self.at(i));
            let after = self.at(e);
            let after2 = self.at(e + 1);
            if before.is_some_and(|c| "-./:,+".contains(c)) && digit(before2) {
                continue;
            }
            if after.is_some_and(|c| "-./:,".contains(c)) && digit(after2) {
                continue;
            }
            if before == Some('+') {
                continue;
            }
            out.push((s, e));
            if out.len() == MAX_CANDIDATES {
                break;
            }
        }
        out
    }

    /// The feature strings of the candidate at (start, end).
    fn features(&self, subject: &str, start: usize, end: usize) -> Vec<String> {
        let code = self.slice(start, end);
        let left = self.slice(start.saturating_sub(WINDOW), start);
        let right = self.slice(end, end + WINDOW);
        let lw = words(&left);
        let lw = &lw[lw.len().saturating_sub(CONTEXT)..];
        let rw = words(&right);
        let rw = &rw[..rw.len().min(CONTEXT)];
        let mut f = Vec::new();
        for w in lw {
            f.push(format!("L:{w}"));
            f.push(format!("LP:{}", prefix(w)));
        }
        for w in rw {
            f.push(format!("R:{w}"));
            f.push(format!("RP:{}", prefix(w)));
        }
        for (i, w) in lw[lw.len().saturating_sub(NEAR)..].iter().rev().enumerate() {
            f.push(format!("L{i}:{w}"));
        }
        for (i, w) in rw.iter().take(NEAR).enumerate() {
            f.push(format!("R{i}:{w}"));
        }
        if lw.len() >= 2 {
            f.push(format!("LB:{}_{}", lw[lw.len() - 2], lw[lw.len() - 1]));
        }
        if rw.len() >= 2 {
            f.push(format!("RB:{}_{}", rw[0], rw[1]));
        }
        let lower = |c: Option<char>| -> String {
            c.map(|c| c.to_lowercase().collect()).unwrap_or_default()
        };
        f.push(format!("P:{}", lower(left.trim_end().chars().last())));
        f.push(format!("N:{}", lower(right.trim_start().chars().next())));
        f.push(format!("shape:{}", shape(&code)));
        f.push(format!(
            "len:{}",
            code.chars().filter(|c| c.is_alphanumeric()).count()
        ));
        let core: String = code.chars().filter(|&c| c != ' ' && c != '-').collect();
        f.push(if !core.is_empty() && core.chars().all(char::is_numeric) {
            "kind:digits".into()
        } else {
            "kind:mixed".into()
        });
        if code.contains([' ', '-']) {
            f.push("sep".into());
        }
        let line_start = self.chars[..start]
            .iter()
            .rposition(|&c| c == '\n')
            .map_or(0, |i| i + 1);
        let line_end = self.chars[end..]
            .iter()
            .position(|&c| c == '\n')
            .map_or(self.chars.len(), |i| end + i);
        let line = self.slice(line_start, line_end);
        let line = line.trim();
        if line == code {
            f.push("ownline".into());
        }
        f.push(format!("line:{}", (line.chars().count() / 10).min(10)));
        if start < self.subject_len {
            f.push("insubject".into());
        }
        for w in words(subject).iter().take(SUBJECT_WORDS) {
            f.push(format!("S:{w}"));
            f.push(format!("SP:{}", prefix(w)));
        }
        f
    }
}

/// Lowercase words: runs of letters, digits and underscores.
fn words(text: &str) -> Vec<String> {
    let lower = text.to_lowercase();
    lower
        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .filter(|w| !w.is_empty())
        .map(str::to_owned)
        .collect()
}

fn prefix(w: &str) -> String {
    w.chars().take(4).collect()
}

fn shape(code: &str) -> String {
    code.chars()
        .map(|c| {
            if c.is_numeric() {
                '9'
            } else if c.is_ascii_uppercase() {
                'A'
            } else {
                c
            }
        })
        .collect()
}

fn fnv1a(s: &str) -> u32 {
    let mut h: u32 = 0x811C_9DC5;
    for b in s.bytes() {
        h ^= u32::from(b);
        h = h.wrapping_mul(0x0100_0193);
    }
    h
}

/// Sparse, L2-normalized counts of the hashed features: (index, value),
/// by index.
fn vector(features: &[String]) -> Vec<(usize, f32)> {
    let mut counts: std::collections::BTreeMap<usize, f64> = Default::default();
    for f in features {
        *counts.entry(fnv1a(f) as usize & (DIM - 1)).or_default() += 1.;
    }
    let norm = counts.values().map(|v| v * v).sum::<f64>().sqrt();
    if norm == 0. {
        return Vec::new();
    }
    counts
        .into_iter()
        .map(|(i, v)| (i, (v / norm) as f32))
        .collect()
}

/// Finds the sign-in codes in mail with a loaded model.
pub struct Detector {
    scorer: Box<dyn Scorer>,
    threshold: f32,
}

impl Detector {
    pub fn new(scorer: Box<dyn Scorer>, threshold: f32) -> Self {
        Self { scorer, threshold }
    }

    /// Each candidate's text and the model's probability that it is the
    /// message's code.
    pub fn scores(&self, subject: &str, body: &str) -> Vec<(String, f32)> {
        let text = Text::new(subject, body);
        text.candidates()
            .into_iter()
            .filter_map(|(s, e)| {
                let x = vector(&text.features(subject, s, e));
                let score = self.scorer.score(&x).ok()?;
                Some((text.slice(s, e), score))
            })
            .collect()
    }

    /// The distinct sign-in codes in a message, longest first (so a code
    /// isn't left half hidden by a shorter one inside it).
    pub fn codes(&self, subject: &str, body: &str) -> Vec<String> {
        let mut codes: Vec<String> = Vec::new();
        for (code, score) in self.scores(subject, body) {
            if score >= self.threshold && !codes.contains(&code) {
                codes.push(code);
            }
        }
        codes.sort_by_key(|c| std::cmp::Reverse(c.chars().count()));
        codes
    }
}

/// `code` with its letters and digits replaced by bullets; separators stay.
fn bullets(code: &str) -> String {
    code.chars()
        .map(|c| if c.is_alphanumeric() { BULLET } else { c })
        .collect()
}

/// `text` with every whole occurrence of each code replaced by bullets.
/// Occurrences inside a longer word or number are left alone.
pub fn mask<'a>(text: &'a str, codes: &[String]) -> Cow<'a, str> {
    let pairs: Vec<(String, String)> = codes.iter().map(|c| (c.clone(), bullets(c))).collect();
    replace_whole(text, &pairs)
}

/// HTML with each code hidden in its text, but not in tags or attributes
/// (a link's address keeps working). Codes split by a space may be written
/// with `&nbsp;` there.
pub fn mask_html<'a>(html: &'a str, codes: &[String]) -> Cow<'a, str> {
    if codes.is_empty() {
        return html.into();
    }
    let mut pairs: Vec<(String, String)> = Vec::new();
    for c in codes {
        pairs.push((c.clone(), bullets(c)));
        if c.contains(' ') {
            pairs.push((c.replace(' ', "&nbsp;"), bullets(c).replace(' ', "&nbsp;")));
        }
    }
    let mut out = String::with_capacity(html.len());
    let mut changed = false;
    let mut rest = html;
    while !rest.is_empty() {
        // Text up to the next tag, then the tag itself.
        let text_end = rest.find('<').unwrap_or(rest.len());
        let masked = replace_whole(&rest[..text_end], &pairs);
        changed |= matches!(masked, Cow::Owned(_));
        out.push_str(&masked);
        rest = &rest[text_end..];
        if rest.is_empty() {
            break;
        }
        let tag_end = rest.find('>').map_or(rest.len(), |i| i + 1);
        out.push_str(&rest[..tag_end]);
        rest = &rest[tag_end..];
    }
    if changed { out.into() } else { html.into() }
}

fn replace_whole<'a>(text: &'a str, pairs: &[(String, String)]) -> Cow<'a, str> {
    let mut current: Cow<'a, str> = text.into();
    for (from, to) in pairs {
        if from.is_empty() || !current.contains(from.as_str()) {
            continue;
        }
        let mut out = String::with_capacity(current.len());
        let mut last = 0;
        for (at, _) in current.match_indices(from.as_str()) {
            let before = current[..at].chars().next_back();
            let after = current[at + from.len()..].chars().next();
            let whole = !before.is_some_and(char::is_alphanumeric)
                && !after.is_some_and(char::is_alphanumeric);
            if whole && at >= last {
                out.push_str(&current[last..at]);
                out.push_str(to);
                last = at + from.len();
            }
        }
        if last > 0 {
            out.push_str(&current[last..]);
            current = out.into();
        }
    }
    current
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manage::local_ai::runtime::NativeModel;

    fn model() -> NativeModel {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("models/otp/otp-v1.bin");
        NativeModel::load(&path).expect("the trained model in models/otp")
    }

    /// The candidates, features, vectors and scores the training code
    /// computed for its examples come out the same here.
    #[test]
    fn matches_the_training_code() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("models/otp/parity.json");
        let cases: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let model = model();
        for case in cases.as_array().unwrap() {
            let subject = case["subject"].as_str().unwrap();
            let body = case["body"].as_str().unwrap();
            let text = Text::new(subject, body);
            let found = text.candidates();
            let want = case["candidates"].as_array().unwrap();
            assert_eq!(found.len(), want.len(), "candidates in {subject:?}");
            for ((s, e), w) in found.into_iter().zip(want) {
                assert_eq!(s as u64, w["start"].as_u64().unwrap(), "{subject:?}");
                assert_eq!(e as u64, w["end"].as_u64().unwrap(), "{subject:?}");
                let features = text.features(subject, s, e);
                let expected: Vec<String> = w["features"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|f| f.as_str().unwrap().to_owned())
                    .collect();
                assert_eq!(features, expected, "features of {:?}", w["code"]);
                let x = vector(&features);
                let expected: Vec<(usize, f32)> = w["vector"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|p| {
                        (
                            p[0].as_u64().unwrap() as usize,
                            p[1].as_f64().unwrap() as f32,
                        )
                    })
                    .collect();
                assert_eq!(x.len(), expected.len());
                for ((i, v), (j, u)) in x.iter().zip(&expected) {
                    assert_eq!(i, j);
                    assert!((v - u).abs() < 1e-5, "{v} vs {u}");
                }
                let score = model.score(&x).unwrap();
                let want = w["score"].as_f64().unwrap() as f32;
                assert!(
                    (score - want).abs() < 1e-4,
                    "{:?}: {score} vs {want}",
                    w["code"]
                );
            }
        }
    }

    #[test]
    fn finds_the_code_and_not_the_other_numbers() {
        let detector = Detector::new(Box::new(model()), super::super::OTP.threshold);
        let codes = detector.codes(
            "Your Acme sign-in code",
            "Hi Sam,\n\nUse 739204 to sign in. It expires in 10 minutes.\n\nOrder #551920 is unaffected. Call 1-800-555-0199.\n© 2026 Acme, 500 Main St, Springfield, IL 62701",
        );
        assert_eq!(codes, ["739204"]);
        let none = detector.codes(
            "Your order #839201 has shipped",
            "Total: $45.10. Tracking number 9400111899223. Delivery on 2026-03-04.",
        );
        assert!(none.is_empty(), "{none:?}");
    }

    #[test]
    fn masks_whole_codes_in_text_and_html_text() {
        let codes = vec!["482 913".to_owned(), "G-551203".to_owned()];
        assert_eq!(
            mask("Code 482 913 or G-551203; not 1482 9130.", &codes),
            "Code ●●● ●●● or ●-●●●●●●; not 1482 9130."
        );
        assert!(matches!(mask("nothing", &codes), Cow::Borrowed(_)));
        assert_eq!(
            mask_html(
                r#"<a href="https://x.test/?c=G-551203">G-551203</a><td>482&nbsp;913</td>"#,
                &codes
            ),
            r#"<a href="https://x.test/?c=G-551203">●-●●●●●●</a><td>●●●&nbsp;●●●</td>"#
        );
    }
}

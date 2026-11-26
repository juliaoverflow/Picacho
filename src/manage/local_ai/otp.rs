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
pub(crate) const WINDOW: usize = 200;
pub(crate) const CONTEXT: usize = 8;
pub(crate) const NEAR: usize = 3;
pub(crate) const SUBJECT_WORDS: usize = 16;
const MAX_CANDIDATES: usize = 40;
const MAX_TEXT: usize = 20_000;
/// What a hidden code's letters and digits are shown as: the light bullet
/// password fields use, which sits in text like a lowercase letter rather
/// than standing out as a large black circle (●) does.
pub const BULLET: char = '•';

/// 2-4 groups of 3-6 uppercase letters and digits split by hyphens, with a
/// letter in them (Bluesky's "BAJ6K-654XM"), or two groups of 3-4 digits split
/// by a space or hyphen, or 4-10 uppercase letters and digits (at least one
/// digit), or 4-10 digits in any script (fullwidth, Arabic-Indic...);
/// optionally after a short uppercase prefix and hyphen, as in Google's
/// "G-123456". The candidate is the group: what comes before it is the start
/// of the text or a character that isn't an ASCII letter or digit, so it can
/// follow Chinese or Japanese without a space.
static CANDIDATE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?:^|[^A-Za-z0-9])((?:[A-Z]{1,3}-)?(?:[A-Z0-9]{3,6}(?:-[A-Z0-9]{3,6}){1,3}|\d{3,4}[ \-]\d{3,4}|[A-Z0-9]{4,10}|\d{4,10}))",
    )
    .expect("pattern")
});
/// The two digit groups a code may be split into.
static DIGIT_GROUPS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\d{3,4}[ \-]\d{3,4}$").expect("pattern"));
/// Letters, marks, digits and underscores make words.
static WORD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[\p{L}\p{N}\p{M}_]+").expect("pattern"));

/// Concepts that carry across languages ("code", "verify", "order"...), each
/// with the word stems that name it; "=" marks a stem that must be the whole
/// word and "^" one the word must start with ("^order" names "orders" but not
/// German "angefordert", requested). The model was trained with this same
/// file.
static LEXICON: LazyLock<Vec<(String, Vec<String>)>> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../../../models/otp/lexicon.json")).expect("lexicon")
});

/// The concepts named by any of the words, in lexicon order.
pub(crate) fn concepts(words: &[String]) -> Vec<&'static str> {
    LEXICON
        .iter()
        .filter(|(_, stems)| {
            stems.iter().any(|st| {
                if let Some(whole) = st.strip_prefix('=') {
                    words.iter().any(|w| w == whole)
                } else if let Some(start) = st.strip_prefix('^') {
                    words.iter().any(|w| w.starts_with(start))
                } else {
                    words.iter().any(|w| w.contains(st.as_str()))
                }
            })
        })
        .map(|(name, _)| name.as_str())
        .collect()
}

/// Whether any of the words names the concept `name` (as `concepts`
/// would list it), looking at its stems alone.
pub(crate) fn names(words: &[String], name: &str) -> bool {
    LEXICON
        .iter()
        .filter(|(concept, _)| concept == name)
        .any(|(_, stems)| {
            stems.iter().any(|st| {
                if let Some(whole) = st.strip_prefix('=') {
                    words.iter().any(|w| w == whole)
                } else if let Some(start) = st.strip_prefix('^') {
                    words.iter().any(|w| w.starts_with(start))
                } else {
                    words.iter().any(|w| w.contains(st.as_str()))
                }
            })
        })
}

/// Scripts written without spaces between words: Hiragana, Katakana, CJK
/// ideographs, Thai, Lao, Khmer and Myanmar. Their words are taken as
/// overlapping pairs of characters.
fn dense(c: char) -> bool {
    matches!(
        u32::from(c),
        0x0E00..=0x0EFF
            | 0x1000..=0x109F
            | 0x1780..=0x17FF
            | 0x3040..=0x30FF
            | 0x3400..=0x4DBF
            | 0x4E00..=0x9FFF
            | 0xF900..=0xFAFF
            | 0xFF66..=0xFF9F
    )
}

/// A message's text as the model sees it (the subject, a newline and the
/// body), and its candidates' places. The parcel model reads the same text.
pub(crate) struct Text {
    subject_len: usize,
    pub(crate) chars: Vec<char>,
}

impl Text {
    pub(crate) fn new(subject: &str, body: &str) -> Self {
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
    pub(crate) fn slice(&self, from: usize, to: usize) -> String {
        self.chars[from.min(self.chars.len())..to.min(self.chars.len())]
            .iter()
            .collect()
    }
    pub(crate) fn at(&self, i: usize) -> Option<char> {
        self.chars.get(i).copied()
    }

    /// (start, end) of each candidate, in characters: the first
    /// `MAX_CANDIDATES`, which is what the model scores.
    fn candidates(&self) -> Vec<(usize, usize)> {
        self.candidates_within(MAX_CANDIDATES)
    }
    /// (start, end) of each candidate, in characters, the first `most`.
    fn candidates_within(&self, most: usize) -> Vec<(usize, usize)> {
        let text: String = self.chars.iter().collect();
        // Byte offset of each character, to turn matches into characters.
        let starts: Vec<usize> = text.char_indices().map(|(i, _)| i).collect();
        let char_at = |byte: usize| starts.partition_point(|&b| b < byte);
        let mut out = Vec::new();
        for caps in CANDIDATE.captures_iter(&text) {
            let Some(m) = caps.get(1) else { continue };
            let (s, e) = (char_at(m.start()), char_at(m.end()));
            let code = m.as_str();
            // Ends where a word would: not before an ASCII letter or digit.
            if self.at(e).is_some_and(|c| c.is_ascii_alphanumeric()) {
                continue;
            }
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
            // Hyphenated groups need a letter, unless they are two digit
            // groups: "94043-1351" or "12345-678" are a postcode or part of
            // a number.
            if core.contains('-')
                && !core.bytes().any(|b| b.is_ascii_uppercase())
                && !DIGIT_GROUPS.is_match(core)
            {
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
            if out.len() == most {
                break;
            }
        }
        out
    }

    /// The feature strings of the candidate at (start, end).
    pub(crate) fn features(&self, subject: &str, start: usize, end: usize) -> Vec<String> {
        let code = self.slice(start, end);
        let left = self.slice(start.saturating_sub(WINDOW), start);
        let right = self.slice(end, end + WINDOW);
        let lw = words(&left, true);
        let lw = &lw[lw.len().saturating_sub(CONTEXT)..];
        let rw = words(&right, true);
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
        let sw = words(subject, true);
        let sw = &sw[..sw.len().min(SUBJECT_WORDS)];
        for w in sw {
            f.push(format!("S:{w}"));
            f.push(format!("SP:{}", prefix(w)));
        }
        // What the nearby words are about, whatever the language.
        let near_left = &lw[lw.len().saturating_sub(NEAR)..];
        let near_right = &rw[..rw.len().min(NEAR)];
        for (region, ws) in [
            ("CL", lw),
            ("CR", rw),
            ("CS", sw),
            ("CNL", near_left),
            ("CNR", near_right),
        ] {
            for name in concepts(ws) {
                f.push(format!("{region}:{name}"));
            }
        }
        // And what the whole window on each side, and the subject, are
        // about, with words in scripts written without spaces kept whole: a
        // code often sits far from the words that say what it is ("dies ist
        // ein einmaliger Roblox-Code, der für ein Konto angefordert wurde,
        // ...: 456360", or "293393は、...ワンタイムコードです").
        for (region, t) in [
            ("CWL", left.as_str()),
            ("CWR", right.as_str()),
            ("CWS", subject),
        ] {
            for name in concepts(&words(t, false)) {
                f.push(format!("{region}:{name}"));
            }
        }
        f
    }
}

/// Whether a message has anything shaped like a code at all (see
/// `Text::candidates`), for telling code mail apart without the model.
pub(crate) fn has_candidate(subject: &str, body: &str) -> bool {
    !Text::new(subject, body).candidates().is_empty()
}

/// Lowercase words: runs of letters, marks, digits and underscores, with
/// runs in scripts written without spaces split into pairs of characters
/// (or, without `pairs`, kept whole, so that concepts can be looked for in
/// them: "ワンタイムコード" names *once* and *code*).
pub(crate) fn words(text: &str, pairs: bool) -> Vec<String> {
    let lower = text.to_lowercase();
    let mut out = Vec::new();
    for run in WORD.find_iter(&lower) {
        let chars: Vec<char> = run.as_str().chars().collect();
        // Split the run where it moves in or out of a dense script.
        let mut start = 0;
        for i in 1..=chars.len() {
            if i < chars.len() && dense(chars[i]) == dense(chars[start]) {
                continue;
            }
            let seg = &chars[start..i];
            if pairs && dense(seg[0]) && seg.len() > 1 {
                out.extend(seg.windows(2).map(|w| w.iter().collect::<String>()));
            } else {
                out.push(seg.iter().collect());
            }
            start = i;
        }
    }
    out
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
pub(crate) fn vector(features: &[String]) -> Vec<(usize, f32)> {
    vector_in(features, DIM)
}

/// `vector` in a hashed feature space of `dim` slots (a power of two).
pub(crate) fn vector_in(features: &[String], dim: usize) -> Vec<(usize, f32)> {
    let mut counts: std::collections::BTreeMap<usize, f64> = Default::default();
    for f in features {
        *counts.entry(fnv1a(f) as usize & (dim - 1)).or_default() += 1.;
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

/// A code as it is typed into a sign-in form: without the spaces or hyphens
/// that split it for reading, without a prefix before digits (Google's
/// "G-123456" is entered as "123456"), and in ASCII digits (forms take
/// "482913" for "４８２９１３" or "٤٨٢٩١٣").
pub fn copyable(code: &str) -> String {
    let code = code.trim();
    let code = match code.split_once('-') {
        Some((prefix, rest))
            if (1..=3).contains(&prefix.len())
                && prefix.bytes().all(|b| b.is_ascii_uppercase())
                && rest.chars().all(|c| c.is_numeric() || c == ' ' || c == '-') =>
        {
            rest
        }
        _ => code,
    };
    code.chars()
        .filter(|&c| c != ' ' && c != '-')
        .map(ascii_digit)
        .collect()
}

/// A digit of another script as its ASCII digit; other characters as they are.
fn ascii_digit(c: char) -> char {
    /// The zero of each script's digits.
    const ZEROS: [u32; 19] = [
        0x0660, 0x06F0, 0x07C0, 0x0966, 0x09E6, 0x0A66, 0x0AE6, 0x0B66, 0x0BE6, 0x0C66, 0x0CE6,
        0x0D66, 0x0E50, 0x0ED0, 0x0F20, 0x1040, 0x17E0, 0x1810, 0xFF10,
    ];
    let code = u32::from(c);
    ZEROS
        .iter()
        .find(|&&zero| (zero..zero + 10).contains(&code))
        .and_then(|zero| char::from_digit(code - zero, 10))
        .unwrap_or(c)
}

/// Every string in `text` shaped like a sign-in code, whether or not the
/// model would take it for one (it only ever scores these), longest first:
/// what is hidden where a code must never show (notifications, widgets) or
/// before the model has read the message. No model is needed.
pub fn candidates(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    // All of it, however long and however many: read in pieces of whole
    // lines within what one reading takes (a line longer than that, at a
    // space), every candidate of each.
    for piece in pieces(text, MAX_TEXT / 2) {
        let t = Text::new(piece, "");
        for (s, e) in t.candidates_within(usize::MAX) {
            let code = t.slice(s, e);
            if !out.contains(&code) {
                out.push(code);
            }
        }
    }
    out.sort_by_key(|c| std::cmp::Reverse(c.chars().count()));
    out
}

/// `text` cut into pieces of at most about `most` characters, at line
/// breaks, or at spaces within a line longer than that (a candidate never
/// spans either).
fn pieces(text: &str, most: usize) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = text;
    while rest.chars().count() > most {
        let limit = rest.char_indices().nth(most).map_or(rest.len(), |(i, _)| i);
        let cut = rest[..limit]
            .rfind('\n')
            .or_else(|| rest[..limit].rfind(char::is_whitespace))
            .map_or(limit, |i| i + rest[i..].chars().next().map_or(1, char::len_utf8));
        out.push(&rest[..cut]);
        rest = &rest[cut..];
    }
    out.push(rest);
    out
}

/// The codes to hide in a message's `subject` and `body` (its text or
/// preview) where there is no Show code to click (notifications, widgets):
/// those the model finds when it is loaded, else every string shaped like
/// a code in either (`candidates`).
pub fn codes_to_hide(detector: Option<&Detector>, subject: &str, body: &str) -> Vec<String> {
    match detector {
        Some(detector) => detector.codes(subject, body),
        None => {
            let mut codes = candidates(subject);
            for code in candidates(body) {
                if !codes.contains(&code) {
                    codes.push(code);
                }
            }
            codes.sort_by_key(|c| std::cmp::Reverse(c.chars().count()));
            codes
        }
    }
}

/// `text` with every string shaped like a sign-in code hidden (see
/// `candidates`).
pub fn mask_all(text: &str) -> Cow<'_, str> {
    mask(text, &candidates(text))
}

/// `code` with its letters and digits replaced by bullets; separators stay.
fn bullets(code: &str) -> String {
    code.chars()
        .map(|c| if c.is_alphanumeric() { BULLET } else { c })
        .collect()
}

/// `text` with every whole occurrence of each code replaced by bullets.
/// Occurrences inside a longer number or Latin word are left alone.
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
            // As candidates are found: a code can touch Chinese or
            // Japanese text, but not another Latin letter or any digit.
            let joined = |c: char| c.is_ascii_alphanumeric() || c.is_numeric();
            let whole = !before.is_some_and(joined) && !after.is_some_and(joined);
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
        NativeModel::load(&super::super::test_model("otp-v1.bin")).expect("the published model")
    }

    /// The candidates, features, vectors and scores the training code
    /// computed for its examples come out the same here.
    /// Outside the app: the model's codes when it is loaded, else every
    /// string shaped like a code, in the subject and the preview alike.
    #[test]
    fn codes_to_hide_use_the_model_else_the_pattern() {
        let detector = Detector::new(Box::new(model()), super::super::OTP.threshold);
        let (subject, body) = ("Your order #839201 has shipped", "Order #839201 is on its way.");
        assert!(codes_to_hide(Some(&detector), subject, body).is_empty());
        assert_eq!(codes_to_hide(None, subject, body), ["839201"]);
        let (subject, body) = ("Acme sign-in", "Your verification code is 482913. It expires in 10 minutes.");
        assert_eq!(codes_to_hide(Some(&detector), subject, body), ["482913"]);
        assert!(codes_to_hide(None, subject, body).contains(&"482913".to_owned()));
    }

    /// Where a code must never show, every string shaped like one is
    /// hidden without the model: however long the text, however many there
    /// are, and across lines cut at any character (a no-break space too).
    #[test]
    fn every_code_shaped_string_is_hidden_without_the_model() {
        assert_eq!(mask_all("482913 is your Acme verification code"), "•••••• is your Acme verification code");
        assert_eq!(mask_all("Your order #839201 has shipped"), "Your order #•••••• has shipped");
        assert_eq!(mask_all("Lunch on Friday?"), "Lunch on Friday?");
        let mut long = String::new();
        for i in 0..3000u32 {
            long.push_str(&format!("Line {i}\u{a0}code {}\n", 100_000 + i));
        }
        long.push_str(&"word ".repeat(9000));
        long.push_str("last 987654");
        let hidden = mask_all(&long);
        assert!(!hidden.contains("100000") && !hidden.contains("102999") && !hidden.contains("987654"));
        assert!(candidates(&long).len() > MAX_CANDIDATES);
    }

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
    fn finds_codes_in_many_languages_and_scripts() {
        let detector = Detector::new(Box::new(model()), super::super::OTP.threshold);
        let cases = [
            (
                "登录验证",
                "您的验证码是739204，5分钟内有效。订单号：20260304",
                "739204",
            ),
            (
                "認証コード",
                "ログイン確認コード：４８２９１３\n有効期限：10分",
                "４８２９１３",
            ),
            (
                "인증번호",
                "로그인 인증번호는 [551203] 입니다. 5분 이내에 입력해 주세요.",
                "551203",
            ),
            (
                "رمز التحقق",
                "رمز التحقق الخاص بك هو ٧٣٩٢٠٤ صالح لمدة 10 دقائق",
                "٧٣٩٢٠٤",
            ),
            (
                "Код подтверждения",
                "Ваш код подтверждения: 7731. Заказ №558120 отправлен.",
                "7731",
            ),
            (
                "सत्यापन कोड",
                "आपका सत्यापन कोड 482913 है। इसे किसी के साथ साझा न करें।",
                "482913",
            ),
            (
                "Doğrulama kodu",
                "Doğrulama kodunuz: 615 204. Kodu kimseyle paylaşmayın.",
                "615 204",
            ),
            ("รหัสยืนยัน", "รหัสยืนยันของคุณคือ 902817 ห้ามบอกรหัสนี้กับผู้อื่น", "902817"),
        ];
        for (subject, body, code) in cases {
            assert_eq!(detector.codes(subject, body), [code], "{subject}");
        }
        assert!(
            detector
                .codes("ご注文の確認", "注文番号：58194433\n合計金額：¥4,500")
                .is_empty()
        );
    }

    /// Codes far from the words that say what they are, as Roblox writes.
    #[test]
    fn finds_codes_far_from_the_words_naming_them() {
        let detector = Detector::new(Box::new(model()), super::super::OTP.threshold);
        let cases = [
            (
                "こんにちは。\n\n293393は、ご自身またはお子様のアカウントに対してリクエストされたRobloxのワンタイムコードです。ログインするには、このコードをログイン画面に入力してください。このコードは15分で期限切れになります。",
                "293393",
            ),
            (
                "Hallo,\n\ndies ist ein einmaliger Roblox-Code, der für ein Konto angefordert wurde, das dir oder deinem Kind gehört: 456360. Gib ihn im Anmeldebildschirm ein, um dich anzumelden. Der Code ist 15 Minuten lang gültig.",
                "456360",
            ),
        ];
        for (body, code) in cases {
            assert_eq!(detector.codes("", body), [code], "{body}");
            assert_eq!(detector.codes("Roblox", body), [code], "{body}");
        }
    }

    /// Codes in hyphenated groups of letters and digits, as Bluesky sends.
    #[test]
    fn finds_codes_in_hyphenated_groups() {
        let detector = Detector::new(Box::new(model()), super::super::OTP.threshold);
        let body = "Sign in to Bluesky\nWe received a sign-in request for the account\n@juliaoverflow.com.\nUse the code below to sign in.\nBAJ6K-654XM\nIf this wasn't you, we recommend taking steps to\nprotect your account by\nchanging your password.\nBluesky, the social internet";
        for subject in ["We received a sign in request for your account", ""] {
            assert_eq!(detector.codes(subject, body), ["BAJ6K-654XM"], "{subject}");
        }
        let text = Text::new("", "Call 800-555-0199, ZIP 94043-1351, on 2026-03-04.");
        assert!(text.candidates().is_empty());
    }

    #[test]
    fn codes_are_copied_as_they_are_typed() {
        assert_eq!(copyable("482913"), "482913");
        assert_eq!(copyable("482 913"), "482913");
        assert_eq!(copyable("482-913"), "482913");
        assert_eq!(copyable("G-551203"), "551203");
        assert_eq!(copyable("AB12CD"), "AB12CD");
        assert_eq!(copyable("AB-12CD"), "AB12CD");
        assert_eq!(copyable("４８２９１３"), "482913");
        assert_eq!(copyable("١٢٣ ٤٥٦"), "123456");
        assert_eq!(copyable("۸۲۵۶"), "8256");
    }

    #[test]
    fn masks_whole_codes_in_text_and_html_text() {
        let codes = vec!["482 913".to_owned(), "G-551203".to_owned()];
        assert_eq!(
            mask("Code 482 913 or G-551203; not 1482 9130.", &codes),
            "Code ••• ••• or •-••••••; not 1482 9130."
        );
        assert!(matches!(mask("nothing", &codes), Cow::Borrowed(_)));
        let cjk = vec!["739204".to_owned(), "４８２９１３".to_owned()];
        assert_eq!(
            mask(
                "您的验证码是739204，请勿泄露。コード：４８２９１３です",
                &cjk
            ),
            "您的验证码是••••••，请勿泄露。コード：••••••です"
        );
        assert_eq!(
            mask_html(
                r#"<a href="https://x.test/?c=G-551203">G-551203</a><td>482&nbsp;913</td>"#,
                &codes
            ),
            r#"<a href="https://x.test/?c=G-551203">•-••••••</a><td>•••&nbsp;•••</td>"#
        );
    }
}

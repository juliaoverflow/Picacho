//! Writing aids for the composer that don't depend on how it is drawn:
//! finding text, the Markdown typed as shortcuts (`- `, `> `, `**bold**`),
//! the emoji picker's catalog and mail merge.
pub mod emoji;
pub mod merge;

use std::ops::Range;

/// Where `needle` is in `hay`, by byte range, without overlaps. Ignoring
/// case compares each character's lowercase (so "É" finds "é").
pub fn find_matches(hay: &str, needle: &str, match_case: bool) -> Vec<Range<usize>> {
    if needle.is_empty() {
        return vec![];
    }
    if match_case {
        return hay.match_indices(needle).map(|(i, m)| i..i + m.len()).collect();
    }
    let fold = |c: char| c.to_lowercase().next().unwrap_or(c);
    let needle: Vec<char> = needle.chars().map(fold).collect();
    let chars: Vec<(usize, char)> = hay.char_indices().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i + needle.len() <= chars.len() {
        if chars[i..i + needle.len()].iter().zip(&needle).all(|((_, a), b)| fold(*a) == *b) {
            let start = chars[i].0;
            let end = chars.get(i + needle.len()).map_or(hay.len(), |(j, _)| *j);
            out.push(start..end);
            i += needle.len();
        } else {
            i += 1;
        }
    }
    out
}

/// What a line's start becomes when a space is typed after it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockRule {
    Bullets,
    /// A numbered list starting at this number.
    Numbers(i64),
    Quote,
    Heading(u8),
}

/// The block rule for a line reading `before` (what's before the caret,
/// the space just typed not included): "-", "*" and "+" make a bulleted
/// list, "1." or "1)" a numbered one, ">" a quote, "#" to "######" a
/// heading.
pub fn block_rule(before: &str) -> Option<BlockRule> {
    match before {
        "-" | "*" | "+" | "•" => return Some(BlockRule::Bullets),
        ">" => return Some(BlockRule::Quote),
        _ => {}
    }
    if !before.is_empty() && before.len() <= 6 && before.chars().all(|c| c == '#') {
        return Some(BlockRule::Heading(before.len() as u8));
    }
    let digits = before.strip_suffix('.').or_else(|| before.strip_suffix(')'))?;
    if (1..=4).contains(&digits.len()) && digits.chars().all(|c| c.is_ascii_digit()) {
        return digits.parse().ok().map(BlockRule::Numbers);
    }
    None
}

/// An inline format Markdown's markers ask for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InlineMark {
    Bold,
    Italic,
    Strike,
    Code,
}

/// Text just closed by Markdown markers, as typed: `**bold**`, `__bold__`,
/// `*italic*`, `_italic_`, `~~strike~~`, `` `code` ``.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InlineRule {
    pub mark: InlineMark,
    /// Byte range of the opening marker in the text.
    pub open: Range<usize>,
    /// Byte range of the closing marker (ending where the caret is).
    pub close: Range<usize>,
}

/// The inline rule `before` (a line's text up to the caret, the marker's
/// last character just typed) ends with, if any. The text inside doesn't
/// start or end with a space; an opening marker starts the line or follows
/// a space or punctuation, and `_` markers stand apart from words (so
/// snake_case stays as it is).
pub fn inline_rule(before: &str) -> Option<InlineRule> {
    const RULES: [(&str, InlineMark); 6] = [
        ("**", InlineMark::Bold),
        ("__", InlineMark::Bold),
        ("~~", InlineMark::Strike),
        ("`", InlineMark::Code),
        ("*", InlineMark::Italic),
        ("_", InlineMark::Italic),
    ];
    for (marker, mark) in RULES {
        let Some(body) = before.strip_suffix(marker) else { continue };
        // `**` typed: its first `*` isn't an italic's end.
        if marker.len() == 1 && body.ends_with(marker) {
            continue;
        }
        let close = body.len()..before.len();
        // The latest opening marker before it, with text between.
        let Some(open) = body.rfind(marker) else { continue };
        let inner = &body[open + marker.len()..];
        if inner.is_empty() || inner.starts_with(char::is_whitespace) || inner.ends_with(char::is_whitespace) {
            continue;
        }
        // One `*` that is half of a `**` belongs to a bold.
        if marker.len() == 1 && (body[..open].ends_with(marker) || inner.starts_with(marker)) {
            continue;
        }
        if mark == InlineMark::Code && inner.contains('`') {
            continue;
        }
        let prev = body[..open].chars().next_back();
        let opens = prev.is_none_or(|c| c.is_whitespace() || (c.is_ascii_punctuation() && c.to_string() != marker[..1]) || "([{\"'«“‘".contains(c));
        if opens {
            return Some(InlineRule { mark, open: open..open + marker.len(), close });
        }
    }
    None
}

/// Whether a line reading `text`, Enter pressed at its end, becomes a
/// horizontal rule ("---", "***", "___").
pub fn is_rule_line(text: &str) -> bool {
    matches!(text.trim(), "---" | "***" | "___")
}

/// Whether a line reading `text`, Enter pressed at its end, becomes a
/// code block ("```", with or without a language name).
pub fn is_fence_line(text: &str) -> bool {
    let t = text.trim();
    t.starts_with("```") && t[3..].chars().all(|c| c.is_alphanumeric() || c == '-' || c == '+')
}

/// How well `query` matches an entry named `label` with `keywords` (a
/// slash command, an emoji): `None` for not at all, lower for better.
pub fn rank(query: &str, label: &str, keywords: &str) -> Option<u8> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return Some(3);
    }
    let label = label.to_lowercase();
    if label.starts_with(&q) {
        return Some(0);
    }
    if label.split(|c: char| !c.is_alphanumeric()).any(|w| w.starts_with(&q)) {
        return Some(1);
    }
    let keywords = keywords.to_lowercase();
    if keywords.split_whitespace().any(|w| w.starts_with(&q)) {
        return Some(2);
    }
    (label.contains(&q) || keywords.contains(&q)).then_some(4)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_is_found_with_and_without_case() {
        assert_eq!(find_matches("Ana and ana", "ana", true), vec![8..11]);
        assert_eq!(find_matches("Ana and ana", "ana", false), [0..3, 8..11]);
        assert_eq!(find_matches("Élan, élan", "ÉLAN", false), [0..5, 7..12]);
        assert_eq!(find_matches("aaaa", "aa", false), [0..2, 2..4]);
        assert!(find_matches("x", "", false).is_empty());
    }

    #[test]
    fn line_starts_become_lists_quotes_and_headings() {
        assert_eq!(block_rule("-"), Some(BlockRule::Bullets));
        assert_eq!(block_rule("*"), Some(BlockRule::Bullets));
        assert_eq!(block_rule("1."), Some(BlockRule::Numbers(1)));
        assert_eq!(block_rule("12)"), Some(BlockRule::Numbers(12)));
        assert_eq!(block_rule(">"), Some(BlockRule::Quote));
        assert_eq!(block_rule("###"), Some(BlockRule::Heading(3)));
        assert_eq!(block_rule("#######"), None);
        assert_eq!(block_rule("a."), None);
        assert_eq!(block_rule("Hi -"), None);
        assert_eq!(block_rule(""), None);
    }

    #[test]
    fn markers_closing_text_format_it() {
        let r = inline_rule("say **hi**").unwrap();
        assert_eq!((r.mark, r.open, r.close), (InlineMark::Bold, 4..6, 8..10));
        let r = inline_rule("an *aside*").unwrap();
        assert_eq!((r.mark, r.open, r.close), (InlineMark::Italic, 3..4, 9..10));
        assert_eq!(inline_rule("run `ls -l`").unwrap().mark, InlineMark::Code);
        assert_eq!(inline_rule("~~gone~~").unwrap().mark, InlineMark::Strike);
        assert_eq!(inline_rule("(_this_").unwrap().mark, InlineMark::Italic);
        // Not when they don't wrap text, or are part of words.
        assert_eq!(inline_rule("say **hi*"), None, "half a bold's end");
        assert_eq!(inline_rule("2 * 3 *"), None);
        assert_eq!(inline_rule("snake_case_"), None);
        assert_eq!(inline_rule("a ** b**"), None);
        assert_eq!(inline_rule("**"), None);
        assert_eq!(inline_rule("plain"), None);
    }

    #[test]
    fn rules_and_fences_end_lines() {
        assert!(is_rule_line("---"));
        assert!(is_rule_line("***"));
        assert!(!is_rule_line("--"));
        assert!(is_fence_line("```"));
        assert!(is_fence_line("```rust"));
        assert!(!is_fence_line("``` x y"));
    }

    #[test]
    fn entries_rank_by_how_they_match() {
        assert_eq!(rank("tab", "Table", ""), Some(0));
        assert_eq!(rank("list", "Bulleted list", ""), Some(1));
        assert_eq!(rank("ul", "Bulleted list", "ul bullets"), Some(2));
        assert_eq!(rank("ble", "Table", ""), Some(4));
        assert_eq!(rank("zzz", "Table", ""), None);
        assert_eq!(rank("", "Table", ""), Some(3));
    }
}

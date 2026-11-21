//! Tracking numbers in a message's text.
use super::{Parcel, Source, known_url};
use crate::manage::local_ai::otp::{self, Text};
use crate::manage::local_ai::parcel::Confirmer;
use regex::Regex;
use std::sync::LazyLock;

/// Uppercase letters and digits, in groups split by single spaces, after the
/// start of the text or a character that isn't an ASCII letter or digit (so
/// a number can follow Chinese or Japanese without a space).
static GROUPS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:^|[^A-Za-z0-9])([A-Z0-9]{2,}(?: [A-Z0-9]{2,})*)").expect("pattern")
});
/// The catalog's shortest and longest numbers, without spaces.
const MIN_LEN: usize = 10;
const MAX_LEN: usize = 40;
/// Groups joined into one number at most.
const MAX_GROUPS: usize = 10;
/// Candidates looked at per message.
const MAX_CANDIDATES: usize = 40;
/// Without the model, bare digits must be at least this long: shorter
/// ones are too often phone, order or account numbers that pass a check.
const RULE_MIN_DIGITS: usize = 12;
/// Without the model, a shipping word must be this close before the number
/// (in characters), or in the subject.
const RULE_WINDOW: usize = 80;

/// A number in a message's text that has a carrier's format and check digit.
#[derive(Debug)]
pub(crate) struct Candidate {
    /// Its place in the text, in characters.
    pub start: usize,
    pub end: usize,
    /// Without spaces.
    pub number: String,
    pub known: tracking_numbers::TrackingResult,
}

/// The text's numbers that have a carrier's format and check digit. Groups
/// split by spaces are joined when together they are one: the longest run of
/// groups from each group that is a tracking number is taken.
pub(crate) fn candidates(text: &Text) -> Vec<Candidate> {
    let s: String = text.chars.iter().collect();
    // Byte offset of each character, to turn matches into characters.
    let starts: Vec<usize> = s.char_indices().map(|(i, _)| i).collect();
    let char_at = |byte: usize| starts.partition_point(|&b| b < byte);
    let mut out: Vec<Candidate> = Vec::new();
    for caps in GROUPS.captures_iter(&s) {
        let Some(m) = caps.get(1) else { continue };
        // Ends where a word would: not before an ASCII letter or digit.
        if s[m.end()..]
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphanumeric())
        {
            continue;
        }
        // (start, end) of each group, in bytes of `s`.
        let mut groups = Vec::new();
        let mut at = m.start();
        for g in m.as_str().split(' ') {
            groups.push((at, at + g.len()));
            at += g.len() + 1;
        }
        let mut i = 0;
        while i < groups.len() {
            let mut taken = None;
            let last = groups.len().min(i + MAX_GROUPS);
            for j in (i..last).rev() {
                let number: String = s[groups[i].0..groups[j].1]
                    .chars()
                    .filter(|&c| c != ' ')
                    .collect();
                if !(MIN_LEN..=MAX_LEN).contains(&number.len())
                    || !number.bytes().any(|b| b.is_ascii_digit())
                {
                    continue;
                }
                if let Some(known) = tracking_numbers::track(&number)
                    .filter(|k| !product_code(&number, k))
                {
                    taken = Some((j, number, known));
                    break;
                }
            }
            let Some((j, number, known)) = taken else {
                i += 1;
                continue;
            };
            let (start, end) = (char_at(groups[i].0), char_at(groups[j].1));
            if !part_of_longer_number(text, start, end) {
                out.push(Candidate {
                    start,
                    end,
                    number,
                    known,
                });
                if out.len() == MAX_CANDIDATES {
                    return out;
                }
            }
            i = j + 1;
        }
    }
    out
}

/// Whether a number some carrier's loose format (one without a check
/// digit, as DHL E-Commerce's) takes looks rather like a product's model
/// or part code: "RX9070XT16GB", "SM-S928U1", "B0CX23V2ZK". Tracking numbers
/// with letters keep them together at the start or end around a long run
/// of digits ("RB123456785GB", "TBA301234567890"), or are UPS's "1Z…"; a
/// code mixes short runs of letters and digits.
pub(super) fn product_code(number: &str, known: &tracking_numbers::TrackingResult) -> bool {
    if !number.bytes().any(|b| b.is_ascii_alphabetic()) || known.courier == "UPS" {
        return false;
    }
    let mut letter_runs = 0;
    let mut longest_digits = 0;
    let mut digits = 0;
    let mut prev_letter = false;
    for b in number.bytes() {
        if b.is_ascii_digit() {
            digits += 1;
            longest_digits = longest_digits.max(digits);
            prev_letter = false;
        } else {
            digits = 0;
            if !prev_letter {
                letter_runs += 1;
            }
            prev_letter = true;
        }
    }
    letter_runs > 2 || longest_digits < 6
}

/// Whether the number at `start..end` continues as a phone number, date or
/// amount does: "+1 ...", "2026-03-04", "1.234.567".
fn part_of_longer_number(text: &Text, start: usize, end: usize) -> bool {
    let digit = |c: Option<char>| c.is_some_and(|c| c.is_ascii_digit());
    let before = start.checked_sub(1).and_then(|i| text.at(i));
    let before2 = start.checked_sub(2).and_then(|i| text.at(i));
    let after = text.at(end);
    let after2 = text.at(end + 1);
    before == Some('+')
        || (before.is_some_and(|c| "-./:,".contains(c)) && digit(before2))
        || (after.is_some_and(|c| "-./:,".contains(c)) && digit(after2))
}

/// The tracking numbers in a message's text: each candidate the rule
/// accepts, and with the model, those it confirms besides.
pub fn from_text(subject: &str, body: &str, model: Option<&Confirmer>) -> Vec<Parcel> {
    let text = Text::new(subject, body);
    let mut out: Vec<Parcel> = Vec::new();
    for c in candidates(&text) {
        if out.iter().any(|p| p.tracking_number == c.number) {
            continue;
        }
        // The model finds what the rule can't (a number given without a
        // parcel word beside it); what the rule takes stays taken, as the
        // model reads short previews less well than whole messages.
        let source = if rule_confirms(&text, subject, &c) {
            Source::Rule
        } else if model.is_some_and(|m| m.confirms(&text, subject, c.start, c.end)) {
            Source::Model
        } else {
            continue;
        };
        out.push(Parcel {
            tracking_url: known_url(&c.known),
            tracking_number: c.number,
            order: None,
            carrier: Some(c.known.courier),
            service: Some(c.known.service),
            expected_arrival: None,
            status: None,
            updated: None,
            orders: Vec::new(),
            contents: None,
            products: Vec::new(),
            source,
        });
    }
    out
}

/// Without the model: a number with letters in it, or a long bare one, with
/// a parcel word ("tracking", "Sendung", "追跡"...) shortly before it on its
/// line; or in shipping mail (a parcel word in the subject), with no word
/// for another kind of number ("order", "invoice", "phone"...) there.
fn rule_confirms(text: &Text, subject: &str, c: &Candidate) -> bool {
    if c.number.bytes().all(|b| b.is_ascii_digit()) && c.number.len() < RULE_MIN_DIGITS {
        return false;
    }
    let line_start = text.chars[..c.start]
        .iter()
        .rposition(|&ch| ch == '\n')
        .map_or(0, |i| i + 1);
    let near = text.slice(line_start.max(c.start.saturating_sub(RULE_WINDOW)), c.start);
    let near = parcel_concepts(&near);
    if near.iter().any(|c| PARCEL_WORDS.contains(c)) {
        return true;
    }
    let other = near.iter().any(|c| !PARCEL_WORDS.contains(c));
    let shipping = otp::concepts(&otp::words(subject, false)).contains(&"shipping")
        || parcel_concepts(subject)
            .iter()
            .any(|c| PARCEL_WORDS.contains(c));
    shipping && !other
}

/// A shop's own package number after a word for one: "Package
/// UUSC000012345678", "Tracking number: SPXABC012345678901". Letters then a
/// long run of digits, as no product code is.
static PACKAGE_NUMBER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i:\b(?:package|parcel|shipment|tracking\s*(?:number|no\.?|#|id)?|waybill))\s*[:#]?\s*([A-Z]{2,6}[0-9]{8,20}[A-Z]{0,4})\b",
    )
    .expect("pattern")
});

/// A package number in no carrier's catalog that the subject, or else the
/// text, gives after a word for a package.
pub(super) fn package_number(subject: &str, body: &str) -> Option<String> {
    let head: String = body.chars().take(4000).collect();
    [subject, head.as_str()]
        .iter()
        .find_map(|s| Some(PACKAGE_NUMBER.captures(s)?.get(1)?.as_str().to_owned()))
}

/// The parcel model's concepts that name parcels; the others name other
/// kinds of number.
pub(super) const PARCEL_WORDS: [&str; 4] = ["track", "parcel", "deliver", "ship"];

/// The parcel model's concepts named in `text` (see `local_ai::parcel`).
pub(super) fn parcel_concepts(text: &str) -> Vec<&'static str> {
    crate::manage::local_ai::parcel::concepts(&otp::words(text, false))
}

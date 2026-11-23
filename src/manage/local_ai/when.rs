//! Typed dates and times, with Epoch TBS: "tmrw at 6", "next fri morning",
//! "the monday in three weeks", "in 4 hrs at the top of the hour",
//! "übermorgen um 18 Uhr", "明天下午6点半" become a moment (and, for a
//! repeating time, an RRULE) wherever Picacho asks when.
//!
//! Two steps. The local model reads the text: for every token it says what
//! the token means ("tmrw", "tomorow", "morgen" and "明天" are TOMORROW; the
//! "sat" in "sat down" is nothing; the "6" in "at 6" is a number). Then rules
//! work out the moment from those meanings, so the calendar arithmetic is
//! exact and the same every time: which Friday, 6 in the morning or the
//! evening, the first time a repeating rule falls. The rules read meanings,
//! not words, so they serve every language the model reads.
//!
//! Tokens and features are built exactly as `models/tbs/features.py` builds
//! them for training; `models/tbs/parity.json` checks the two agree.
use super::runtime::NativeTagger;
use chrono::{
    DateTime, Datelike, Days, Months, NaiveDate, NaiveDateTime, NaiveTime, TimeDelta, TimeZone,
    Timelike,
};
use regex::Regex;
use std::sync::LazyLock;

/// The hashed feature space (a power of two).
pub const DIM: usize = 1 << 14;
const MAX_TOKENS: usize = 64;
const MAX_TEXT: usize = 300;
/// Punctuation kept as tokens of their own.
const KEPT: &str = ":/-.,@+";

/// What a token can mean, in the model's output order (`features.LABELS`).
pub const LABELS: [&str; 104] = [
    "O",
    "NUM",
    "ORD",
    "COLON",
    "DATESEP",
    "TODAY",
    "TOMORROW",
    "YESTERDAY",
    "TONIGHT",
    "NOW",
    "LATER",
    "MON",
    "TUE",
    "WED",
    "THU",
    "FRI",
    "SAT",
    "SUN",
    "JAN",
    "FEB",
    "MAR",
    "APR",
    "MAY",
    "JUN",
    "JUL",
    "AUG",
    "SEP",
    "OCT",
    "NOV",
    "DEC",
    "U_MIN",
    "U_HOUR",
    "U_DAY",
    "U_WEEK",
    "U_MONTH",
    "U_YEAR",
    "WEEKEND",
    "WEEKDAYS",
    "MORNING",
    "AFTERNOON",
    "EVENING",
    "NIGHT",
    "NOON",
    "MIDNIGHT",
    "AM",
    "PM",
    "W1",
    "W2",
    "W3",
    "W4",
    "W5",
    "W6",
    "W7",
    "W8",
    "W9",
    "W10",
    "W11",
    "W12",
    "W13",
    "W14",
    "W15",
    "W16",
    "W17",
    "W18",
    "W19",
    "W20",
    "W30",
    "W40",
    "W50",
    "HALF",
    "QUARTER",
    "A",
    "COUPLE",
    "FEW",
    "AT",
    "IN",
    "NEXT",
    "THIS",
    "LAST",
    "EVERY",
    "OTHER",
    "AFTER",
    "FROM_NOW",
    "AGO",
    "PAST",
    "TO",
    "AND",
    "OCLOCK",
    "END",
    "EOD",
    "EOW",
    "DAILY",
    "WEEKLY",
    "MONTHLY",
    "YEARLY",
    "HOURLY",
    "OVERMORROW",
    "U_FORTNIGHT",
    "TOP",
    "SAME",
    "START",
    "MID",
    "FROM",
    "MONTH_MARK",
];

/// The hour a day means when no time is given ("tomorrow"), as Picacho's
/// own quick choices use; and the hours of the parts of the day.
const DEFAULT_HOUR: u32 = 8;
const END_OF_DAY: u32 = 17;
fn part_hour(part: &str) -> u32 {
    match part {
        "MORNING" => 8,
        "AFTERNOON" => 14,
        "EVENING" => 19,
        _ => 20,
    }
}

// Tokens.

/// A token: lowercase text (digits as ASCII digits) and where it is in the
/// text, in bytes (`start`, `end`) and in characters (as the training code
/// counts).
#[derive(Clone, Debug, PartialEq)]
pub struct Token {
    pub text: String,
    pub start: usize,
    pub end: usize,
    chars: (usize, usize),
}

/// Digits in other scripts, by their zero: Arabic-Indic, Persian,
/// Devanagari, Bengali, Thai and fullwidth.
const ZEROS: [u32; 6] = [0x0660, 0x06F0, 0x0966, 0x09E6, 0x0E50, 0xFF10];

/// The ASCII digit `c` is, in any of those scripts.
fn digit(c: char) -> Option<char> {
    if c.is_ascii_digit() {
        return Some(c);
    }
    let o = u32::from(c);
    ZEROS
        .iter()
        .find(|z| (**z..=**z + 9).contains(&o))
        .and_then(|z| char::from_u32(u32::from('0') + o - z))
}

static MARK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\p{M}$").expect("pattern"));

fn is_mark(c: char) -> bool {
    // No ASCII character is a mark: most text needs no regex.
    if c.is_ascii() {
        return false;
    }
    let mut buf = [0u8; 4];
    MARK.is_match(c.encode_utf8(&mut buf))
}

/// A letter as Python's `str.isalpha` has it (not a mark or a digit).
fn is_alpha(c: char) -> bool {
    c.is_alphabetic() && !is_mark(c) && digit(c).is_none()
}

/// Letters, and the marks written on them, but not digits.
fn is_letter(c: char) -> bool {
    digit(c).is_none() && (c.is_alphabetic() || is_mark(c))
}

/// Scripts written without spaces between words: Thai, Lao, Myanmar,
/// Khmer, kana and CJK ideographs. Each of their characters (with its
/// marks) is a token of its own.
fn is_dense(c: char) -> bool {
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

/// Lowercase runs of letters, runs of digits (as ASCII digits), single
/// characters of scripts written without spaces, and the punctuation in
/// `KEPT`; anything else separates tokens ("6pm" is "6" and "pm"; "明天6点"
/// is "明", "天", "6", "点").
pub fn tokens(text: &str) -> Vec<Token> {
    let chars: Vec<(usize, char)> = text.char_indices().take(MAX_TEXT).collect();
    let end = chars
        .last()
        .map_or(0, |(at, c)| at + c.len_utf8())
        .min(text.len());
    let byte_at = |i: usize| chars.get(i).map_or(end, |c| c.0);
    let mut out = Vec::new();
    let (mut i, n) = (0, chars.len());
    while i < n && out.len() < MAX_TOKENS {
        let c = chars[i].1;
        if digit(c).is_some() {
            let mut j = i;
            while j < n && digit(chars[j].1).is_some() {
                j += 1;
            }
            out.push(Token {
                text: chars[i..j].iter().filter_map(|c| digit(c.1)).collect(),
                start: byte_at(i),
                end: byte_at(j),
                chars: (i, j),
            });
            i = j;
            continue;
        }
        let j = if is_letter(c) && is_dense(c) {
            let mut j = i + 1;
            while j < n && is_mark(chars[j].1) {
                j += 1;
            }
            j
        } else if is_letter(c) {
            let mut j = i;
            while j < n && is_letter(chars[j].1) && !(is_dense(chars[j].1) && !is_mark(chars[j].1))
            {
                j += 1;
            }
            j
        } else if KEPT.contains(c) {
            i + 1
        } else {
            i += 1;
            continue;
        };
        let raw: String = chars[i..j].iter().map(|c| c.1).collect();
        out.push(Token {
            text: raw.to_lowercase(),
            start: byte_at(i),
            end: byte_at(j),
            chars: (i, j),
        });
        i = j;
    }
    out
}

/// The first letter and the consonants after it, without repeats:
/// "tomorrow", "tommorow" and "tmrw" are all "tmrw".
fn skeleton(t: &str) -> String {
    let mut chars = t.chars();
    let Some(first) = chars.next().filter(|c| is_alpha(*c)) else {
        return t.to_owned();
    };
    let mut out = vec![first];
    for c in chars {
        if "aeiou".contains(c) {
            continue;
        }
        if Some(&c) != out.last() {
            out.push(c);
        }
    }
    out.into_iter().collect()
}

fn kind(t: &str) -> String {
    if t.starts_with(|c: char| c.is_ascii_digit()) {
        format!("#{}", t.chars().count().min(5))
    } else {
        t.to_owned()
    }
}

/// For each token, whether the next one follows it with nothing between.
fn joined(toks: &[Token]) -> Vec<bool> {
    (0..toks.len())
        .map(|i| i + 1 < toks.len() && toks[i].chars.1 == toks[i + 1].chars.0)
        .collect()
}

/// The feature strings of token `i`, as `features.features` makes them.
fn features(toks: &[&str], i: usize, glue: &[bool]) -> Vec<String> {
    let t = toks[i];
    let chars: Vec<char> = t.chars().collect();
    let mut f = vec![format!("w:{}", kind(t))];
    if glue[i] {
        f.push(format!("j+:{}", kind(toks[i + 1])));
    }
    if i > 0 && glue[i - 1] {
        f.push(format!("j-:{}", kind(toks[i - 1])));
    }
    if chars[0].is_ascii_digit() {
        let v: u32 = if chars.len() <= 4 {
            t.parse().unwrap_or(0)
        } else {
            99_999
        };
        let bucket = if v <= 60 {
            v.to_string()
        } else if v < 1900 {
            "big".into()
        } else if v <= 2100 {
            "year".into()
        } else {
            "huge".into()
        };
        f.push(format!("v:{bucket}"));
        if chars.len() >= 2 && chars[0] == '0' {
            f.push("lead0".into());
        }
    } else if is_alpha(chars[0]) {
        let s: String = chars[chars.len().saturating_sub(3)..].iter().collect();
        let p: String = chars.iter().take(3).collect();
        f.push(format!("k:{}", skeleton(t)));
        f.push(format!("p:{p}"));
        f.push(format!("s:{s}"));
        f.push(format!("len:{}", chars.len().min(9)));
        let g: Vec<char> = std::iter::once('<')
            .chain(chars.iter().copied())
            .chain(std::iter::once('>'))
            .collect();
        for w in g.windows(3) {
            f.push(format!("g:{}", w.iter().collect::<String>()));
        }
    }
    for o in [-3i32, -2, -1, 1, 2, 3] {
        let j = i as i32 + o;
        let n = if j < 0 {
            "<s>"
        } else {
            toks.get(j as usize).copied().unwrap_or("</s>")
        };
        let k = if n == "<s>" || n == "</s>" {
            n.to_owned()
        } else {
            kind(n)
        };
        f.push(format!("w{o}:{k}"));
        if o.abs() <= 2 && n.starts_with(is_alpha) && n.chars().count() > 1 {
            f.push(format!("k{o}:{}", skeleton(n)));
        }
    }
    let prev = if i > 0 {
        kind(toks[i - 1])
    } else {
        "<s>".into()
    };
    let next = toks.get(i + 1).map_or("</s>".into(), |n| kind(n));
    let k = kind(t);
    f.push(format!("b-:{prev}|{k}"));
    f.push(format!("b+:{k}|{next}"));
    f.push(format!("tri:{prev}|{k}|{next}"));
    f
}

fn fnv1a(s: &str) -> u32 {
    let mut h: u32 = 0x811C_9DC5;
    for b in s.bytes() {
        h ^= u32::from(b);
        h = h.wrapping_mul(0x0100_0193);
    }
    h
}

/// Sparse, L2-normalized counts of the hashed features, by index.
fn vector(features: &[String]) -> Vec<(usize, f32)> {
    let mut counts: Vec<(usize, f32)> = Vec::with_capacity(features.len());
    for s in features {
        let i = fnv1a(s) as usize & (DIM - 1);
        match counts.iter_mut().find(|(j, _)| *j == i) {
            Some((_, c)) => *c += 1.,
            None => counts.push((i, 1.)),
        }
    }
    let norm = counts.iter().map(|(_, v)| v * v).sum::<f32>().sqrt();
    if norm > 0. {
        for (_, v) in &mut counts {
            *v /= norm;
        }
    }
    counts.sort_by_key(|(i, _)| *i);
    counts
}

// The model.

/// A moment found in text.
#[derive(Clone, Debug, PartialEq)]
pub struct When {
    /// Seconds since the epoch (for a repeating time, its first time).
    pub at: i64,
    /// For a repeating time, the rule (RFC 5545), without DTSTART:
    /// "FREQ=WEEKLY;INTERVAL=2;BYDAY=MO;BYHOUR=9;BYMINUTE=30".
    pub rule: Option<String>,
    /// Where in the text it was said, in bytes.
    pub start: usize,
    pub end: usize,
}

/// How dates written in numbers are read: "11/12" is the 11th of December
/// where the day comes first, and November 12 where the month does. A
/// number that can't be a month decides either way, and dates with dots
/// ("11.12.") always put the day first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Options {
    pub day_first: bool,
}

impl Options {
    /// As the computer's language and region write dates: the month first
    /// in the United States and a few others, and in Chinese, Japanese,
    /// Korean and Hungarian (year, month, day); the day first elsewhere.
    pub fn from_locale() -> Self {
        let locale = sys_locale::get_locale().unwrap_or_default();
        Self::for_locale(&locale)
    }
    fn for_locale(locale: &str) -> Self {
        let mut parts = locale.split(['-', '_']);
        let language = parts.next().unwrap_or("").to_ascii_lowercase();
        let region = parts
            .find(|p| p.len() == 2 && p.chars().all(|c| c.is_ascii_alphabetic()))
            .map(|p| p.to_ascii_uppercase());
        let month_first = matches!(language.as_str(), "zh" | "ja" | "ko" | "hu")
            || matches!(
                region.as_deref(),
                Some("US" | "PH" | "CA" | "FM" | "MH" | "PW" | "GU" | "AS" | "MP" | "UM" | "VI")
            )
            || (language == "en" && region.is_none());
        Self {
            day_first: !month_first,
        }
    }
}

/// Reads dates and times with the installed model.
pub struct Parser {
    tagger: NativeTagger,
    options: Options,
}

impl Parser {
    pub fn new(tagger: NativeTagger) -> Self {
        Self {
            tagger,
            options: Options::from_locale(),
        }
    }
    #[cfg(test)]
    pub fn with_options(mut self, options: Options) -> Self {
        self.options = options;
        self
    }

    /// Each token with the label the model gives it and how sure it is.
    pub fn label(&self, text: &str) -> Vec<(Token, &'static str, f32)> {
        let toks = tokens(text);
        let words: Vec<&str> = toks.iter().map(|t| t.text.as_str()).collect();
        let glue = joined(&toks);
        let labels: Vec<(usize, f32)> = (0..toks.len())
            .map(|i| self.tagger.classify(&vector(&features(&words, i, &glue))))
            .collect();
        toks.into_iter()
            .zip(labels)
            .map(|(t, (k, p))| (t, LABELS.get(k).copied().unwrap_or("O"), p))
            .collect()
    }

    /// The moment `text` names, from `now`; None when it names none.
    pub fn parse<Tz: TimeZone>(&self, text: &str, now: &DateTime<Tz>) -> Option<When> {
        let labelled: Vec<(Token, &'static str)> = self
            .label(text)
            .into_iter()
            .map(|(t, l, _)| (t, l))
            .collect();
        resolve(&labelled, now, self.options)
    }
}

// The rules: what the labels say.

/// A token that means something, after merging runs of the same meaning
/// ("o" "clock", "a" "." "m" ".", "pasado" "mañana", "明" "天").
#[derive(Clone, Debug)]
struct Item<'a> {
    label: &'static str,
    text: &'a str,
    start: usize,
    end: usize,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Mode {
    Bare,
    Next,
    Last,
}

/// The start, middle or end of a period.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Edge {
    Start,
    Mid,
    End,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Period {
    Day,
    Week,
    Month,
    Year,
}

/// Which month: one by name, or months from this one.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Month {
    Named(u32),
    Shift(i64),
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Day {
    /// Days from today.
    Rel(i64),
    /// Monday is 0.
    Weekday(u32, Mode),
    /// A weekday in the week so many weeks from this one ("friday next
    /// week", "the monday in three weeks").
    WeekdayIn(u32, i64),
    Date(Option<i32>, u32, u32),
    /// A day of the month: the next one, or in a month from this one.
    OfMonth(u32, Option<i64>),
    /// A period so many from this one ("next week", "this month"): its
    /// start, or with a weekday or day of the month, that day in it.
    Of(Period, i64),
    /// The start, middle or end of a period so many from this one.
    Edge(Edge, Period, i64),
    Weekend(Mode),
    /// The nth weekday of a month (-1: the last).
    Nth(i32, u32, Month),
    /// Months from today, the same day ("same time next month").
    Months(i32),
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Meridiem {
    None,
    Am,
    Pm,
}

/// A time on the clock as said: "6", "6pm", "18:30", "06:00".
#[derive(Clone, Copy, Debug, PartialEq)]
struct Clock {
    hour: u32,
    minute: u32,
    meridiem: Meridiem,
    /// Said as a 24-hour time ("18:30", "06:00", "0:15").
    full: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Freq {
    Minutely,
    Hourly,
    Daily,
    Weekly,
    Monthly,
    Yearly,
}

#[derive(Clone, Debug, PartialEq)]
struct Repeat {
    freq: Freq,
    interval: u32,
    days: Vec<u32>,
}

#[derive(Default, Debug)]
struct Said {
    /// The days said, in order; `settle` makes one of them.
    days: Vec<Day>,
    day: Option<Day>,
    /// Days added to the day ("friday after next").
    extra_days: i64,
    clock: Option<Clock>,
    /// A number that is an hour if a day or part of the day says so ("tmrw
    /// 6", "6 tonight").
    bare: Option<u32>,
    part: Option<&'static str>,
    noon: bool,
    midnight: bool,
    /// A length of time: minutes and months; from now, or from the day
    /// said when `anchored` ("a week from friday").
    offset: Option<(i64, i32)>,
    anchored: bool,
    /// At the next full hour ("at the top of the hour").
    top: bool,
    /// At the time it is now ("this time tomorrow").
    same: bool,
    repeat: Option<Repeat>,
    now: bool,
    later: bool,
    /// The tokens used, in bytes.
    span: Option<(usize, usize)>,
}

impl Said {
    fn used(&mut self, item: &Item) {
        self.span = Some(match self.span {
            Some((s, e)) => (s.min(item.start), e.max(item.end)),
            None => (item.start, item.end),
        });
    }
}

fn weekday_of(label: &str) -> Option<u32> {
    ["MON", "TUE", "WED", "THU", "FRI", "SAT", "SUN"]
        .iter()
        .position(|l| *l == label)
        .map(|i| i as u32)
}
fn month_of(label: &str) -> Option<u32> {
    [
        "JAN", "FEB", "MAR", "APR", "MAY", "JUN", "JUL", "AUG", "SEP", "OCT", "NOV", "DEC",
    ]
    .iter()
    .position(|l| *l == label)
    .map(|i| i as u32 + 1)
}
fn word_value(label: &str) -> Option<u32> {
    label.strip_prefix('W').and_then(|n| n.parse().ok())
}
fn is_part(label: &str) -> bool {
    matches!(label, "MORNING" | "AFTERNOON" | "EVENING" | "NIGHT")
}
fn shift_of(label: &str) -> Option<i64> {
    match label {
        "NEXT" => Some(1),
        "THIS" => Some(0),
        "LAST" => Some(-1),
        _ => None,
    }
}
fn period_of(label: &str) -> Option<Period> {
    match label {
        "U_DAY" => Some(Period::Day),
        "U_WEEK" | "WEEKEND" => Some(Period::Week),
        "U_MONTH" => Some(Period::Month),
        "U_YEAR" => Some(Period::Year),
        _ => None,
    }
}
/// A unit's length: minutes, or months.
fn unit_of(label: &str) -> Option<(i64, i32)> {
    Some(match label {
        "U_MIN" => (1, 0),
        "U_HOUR" => (60, 0),
        "U_DAY" => (24 * 60, 0),
        "U_WEEK" => (7 * 24 * 60, 0),
        "U_FORTNIGHT" => (14 * 24 * 60, 0),
        "U_MONTH" => (0, 1),
        "U_YEAR" => (0, 12),
        _ => return None,
    })
}

struct Reader<'a, 'b> {
    items: &'b [Item<'a>],
    i: usize,
    options: Options,
}

impl<'a> Reader<'a, '_> {
    fn label(&self, at: usize) -> &'static str {
        self.items.get(self.i + at).map_or("", |it| it.label)
    }
    fn item(&self, at: usize) -> Option<&Item<'a>> {
        self.items.get(self.i + at)
    }
    fn num(&self, at: usize) -> Option<u32> {
        let it = self.item(at)?;
        (it.label == "NUM" && it.text.len() <= 4)
            .then(|| it.text.parse().ok())
            .flatten()
    }
    fn digits(&self, at: usize) -> usize {
        self.item(at).map_or(0, |it| it.text.len())
    }
    /// A count at `at`: digits, number words ("twenty five"), "a", "a
    /// couple", "a few". (value, tokens it takes)
    fn count(&self, at: usize) -> Option<(u32, usize)> {
        if let Some(n) = self.num(at) {
            return Some((n, 1));
        }
        if let Some(v) = self.words(at) {
            return Some(v);
        }
        match self.label(at) {
            "A" if self.label(at + 1) == "COUPLE" => Some((2, 2)),
            "A" if self.label(at + 1) == "FEW" => Some((3, 2)),
            "A" => Some((1, 1)),
            "COUPLE" => Some((2, 1)),
            "FEW" => Some((3, 1)),
            _ => None,
        }
    }
    /// Number words from `at`: "twenty" "five" is 25. (value, tokens)
    fn words(&self, at: usize) -> Option<(u32, usize)> {
        let first = word_value(self.label(at))?;
        if first >= 20
            && let Some(ones) = word_value(self.label(at + 1)).filter(|v| *v < 10)
        {
            return Some((first + ones, 2));
        }
        Some((first, 1))
    }
    /// An hour at `at` (digits up to 24, or a number word up to 12).
    fn hour(&self, at: usize) -> Option<(u32, usize)> {
        if let Some(n) = self.num(at).filter(|n| *n <= 24 && self.digits(at) <= 2) {
            return Some((n, 1));
        }
        self.words(at).filter(|(v, _)| (1..=12).contains(v))
    }
    /// Which month at `at`: "next month", "the month", "month next",
    /// "december". (month, tokens)
    fn month_spec(&self, at: usize) -> Option<(Month, usize)> {
        if let Some(m) = month_of(self.label(at)) {
            return Some((Month::Named(m), 1));
        }
        if let Some(k) = shift_of(self.label(at))
            && self.label(at + 1) == "U_MONTH"
        {
            return Some((Month::Shift(k), 2));
        }
        if self.label(at) == "U_MONTH" {
            return Some(match shift_of(self.label(at + 1)) {
                Some(k) => (Month::Shift(k), 2),
                None => (Month::Shift(0), 1),
            });
        }
        None
    }
}

/// The moment the labelled tokens of a text name, from `now`.
fn resolve<Tz: TimeZone>(
    labelled: &[(Token, &'static str)],
    now: &DateTime<Tz>,
    options: Options,
) -> Option<When> {
    let items = merge(labelled);
    let said = read(&items, options)?;
    let (start, end) = said.span?;
    let local = now.naive_local();
    let local = local.with_second(0)?.with_nanosecond(0)?;
    let (at, rule) = moment(&said, local)?;
    let tz = now.timezone();
    // A time skipped when the clocks go forward is an hour later.
    let at = tz.from_local_datetime(&at).earliest().or_else(|| {
        tz.from_local_datetime(&(at + TimeDelta::hours(1)))
            .earliest()
    })?;
    Some(When {
        at: at.timestamp(),
        rule,
        start,
        end,
    })
}

fn merge<'a>(labelled: &'a [(Token, &'static str)]) -> Vec<Item<'a>> {
    let mut items: Vec<Item> = Vec::new();
    for (t, label) in labelled {
        if *label == "O" {
            continue;
        }
        let mergeable = !matches!(*label, "NUM" | "COLON" | "DATESEP" | "ORD" | "AND")
            && !label.starts_with('W');
        if let Some(last) = items.last_mut()
            && mergeable
            && last.label == *label
        {
            last.end = t.end;
            continue;
        }
        items.push(Item {
            label,
            text: &t.text,
            start: t.start,
            end: t.end,
        });
    }
    items
}

/// What the items say, read left to right. None when they say no time.
fn read(items: &[Item], options: Options) -> Option<Said> {
    let mut said = Said::default();
    let mut r = Reader {
        items,
        i: 0,
        options,
    };
    while r.i < items.len() {
        let took = step(&mut r, &mut said);
        if took == 0 {
            r.i += 1;
            continue;
        }
        for k in 0..took {
            if let Some(it) = r.item(k) {
                let it = it.clone();
                said.used(&it);
            }
        }
        r.i += took;
    }
    settle(&mut said);
    // A number on its own is only an hour beside a day or a part of the day.
    if said.clock.is_none()
        && !said.noon
        && !said.midnight
        && let Some(h) = said.bare
        && (said.day.is_some() || said.part.is_some() || said.repeat.is_some())
        && (1..=23).contains(&h)
    {
        said.clock = Some(Clock {
            hour: h,
            minute: 0,
            meridiem: Meridiem::None,
            full: h > 12,
        });
    }
    let anything = said.day.is_some()
        || said.clock.is_some()
        || said.part.is_some()
        || said.noon
        || said.midnight
        || said.offset.is_some()
        || said.repeat.is_some()
        || said.now
        || said.later
        || said.top;
    anything.then_some(said)
}

/// Makes one day of the days said: a weekday with a week ("friday next
/// week"), a day of the month with a month ("the 15th of next month"), or
/// the first that stands on its own.
fn settle(s: &mut Said) {
    let weekday = s.days.iter().find_map(|d| match d {
        Day::Weekday(wd, Mode::Bare) => Some(*wd),
        _ => None,
    });
    let week = s.days.iter().find_map(|d| match d {
        Day::Of(Period::Week, k) => Some(*k),
        _ => None,
    });
    let month = s.days.iter().find_map(|d| match d {
        Day::Of(Period::Month, k) => Some(*k),
        _ => None,
    });
    let month_day = s.days.iter().find_map(|d| match d {
        Day::OfMonth(d, None) => Some(*d),
        _ => None,
    });
    // Whole weeks from now, with a weekday: "the monday in three weeks".
    let weeks_ahead = match s.offset {
        Some((mins, 0)) if !s.anchored && mins > 0 && mins % (7 * 24 * 60) == 0 => {
            Some(mins / (7 * 24 * 60))
        }
        _ => None,
    };
    s.day = if let (Some(wd), Some(k)) = (weekday, week) {
        Some(Day::WeekdayIn(wd, k))
    } else if let (Some(wd), Some(k)) = (weekday, weeks_ahead) {
        s.offset = None;
        Some(Day::WeekdayIn(wd, k))
    } else if let (Some(d), Some(k)) = (month_day, month) {
        Some(Day::OfMonth(d, Some(k)))
    } else if s.same && week.is_some_and(|k| k > 0) && weekday.is_none() {
        Some(Day::Rel(7 * week.unwrap_or(1)))
    } else if s.same && month.is_some_and(|k| k > 0) && month_day.is_none() {
        Some(Day::Months(month.unwrap_or(1) as i32))
    } else {
        s.days.iter().find_map(|d| match *d {
            // "this week", "this month" alone say no day.
            Day::Of(_, k) if k <= 0 => None,
            Day::Of(p, k) => Some(Day::Edge(Edge::Start, p, k)),
            d => Some(d),
        })
    };
}

/// Reads one thing at the reader's position; how many items it took (0:
/// none, skip one).
fn step(r: &mut Reader, s: &mut Said) -> usize {
    let l = r.label(0);
    // Repeating times.
    if l == "EVERY" {
        let mut k = 1;
        let mut interval = 1;
        if r.label(k) == "OTHER" {
            interval = 2;
            k += 1;
        }
        if let Some((n, used)) = r.count(k)
            && unit_of(r.label(k + used)).is_some()
        {
            s.repeat = Some(repeat_of(r.label(k + used), n.max(1) * interval));
            return k + used + 1;
        }
        let next = r.label(k);
        if unit_of(next).is_some() {
            s.repeat = Some(repeat_of(next, interval));
            return k + 1;
        }
        if let Some(wd) = weekday_of(next) {
            let mut days = vec![wd];
            k += 1;
            loop {
                let and = usize::from(r.label(k) == "AND");
                match weekday_of(r.label(k + and)) {
                    Some(d) => {
                        days.push(d);
                        k += and + 1;
                    }
                    None => break,
                }
            }
            s.repeat = Some(Repeat {
                freq: Freq::Weekly,
                interval,
                days,
            });
            return k;
        }
        if next == "WEEKDAYS" || next == "WEEKEND" {
            s.repeat = Some(Repeat {
                freq: Freq::Weekly,
                interval,
                days: if next == "WEEKDAYS" {
                    vec![0, 1, 2, 3, 4]
                } else {
                    vec![5, 6]
                },
            });
            return k + 1;
        }
        if is_part(next) || next == "NOON" {
            s.repeat = Some(Repeat {
                freq: Freq::Daily,
                interval,
                days: vec![],
            });
            return k;
        }
        if month_of(next).is_some() {
            s.repeat = Some(Repeat {
                freq: Freq::Yearly,
                interval,
                days: vec![],
            });
            return k;
        }
        return 1;
    }
    let freq = match l {
        "DAILY" => Some(Freq::Daily),
        "WEEKLY" => Some(Freq::Weekly),
        "MONTHLY" => Some(Freq::Monthly),
        "YEARLY" => Some(Freq::Yearly),
        "HOURLY" => Some(Freq::Hourly),
        _ => None,
    };
    if let Some(freq) = freq {
        s.repeat = Some(Repeat {
            freq,
            interval: 1,
            days: vec![],
        });
        return 1;
    }
    if l == "WEEKDAYS" {
        s.repeat = Some(Repeat {
            freq: Freq::Weekly,
            interval: 1,
            days: vec![0, 1, 2, 3, 4],
        });
        return 1;
    }
    // "the first monday of next month", "the last friday of the month".
    let nth = match l {
        "LAST" => Some((-1, 1)),
        _ => match r.words(0) {
            Some((n, 1)) if (1..=5).contains(&n) => Some((n as i32, 1)),
            _ => r
                .num(0)
                .filter(|n| (1..=5).contains(n) && r.label(1) == "ORD")
                .map(|n| (n as i32, 2)),
        },
    };
    if let Some((n, used)) = nth {
        if let Some(wd) = weekday_of(r.label(used))
            && let Some((month, mused)) = r.month_spec(used + 1)
        {
            s.days.push(Day::Nth(n, wd, month));
            return used + 1 + mused;
        }
        // "the first day of next month", "the last day of the month".
        if (n == 1 || n == -1)
            && r.label(used) == "U_DAY"
            && let Some((Month::Shift(k), mused)) = r.month_spec(used + 1)
        {
            let edge = if n == 1 { Edge::Start } else { Edge::End };
            s.days.push(Day::Edge(edge, Period::Month, k));
            return used + 1 + mused;
        }
    }
    // Days.
    match l {
        "TODAY" => {
            s.days.push(Day::Rel(0));
            return 1;
        }
        "TOMORROW" => {
            s.days.push(Day::Rel(1));
            return 1;
        }
        "OVERMORROW" => {
            s.days.push(Day::Rel(2));
            return 1;
        }
        "YESTERDAY" => {
            s.days.push(Day::Rel(-1));
            return 1;
        }
        "TONIGHT" => {
            s.days.push(Day::Rel(0));
            s.part.get_or_insert("NIGHT");
            return 1;
        }
        "NOW" => {
            s.now = true;
            return 1;
        }
        "LATER" => {
            s.later = true;
            return 1;
        }
        "EOD" => {
            s.days.push(Day::Edge(Edge::End, Period::Day, 0));
            return 1;
        }
        "EOW" => {
            s.days.push(Day::Edge(Edge::End, Period::Week, 0));
            return 1;
        }
        "NOON" => {
            s.noon = true;
            return 1;
        }
        "MIDNIGHT" => {
            s.midnight = true;
            return 1;
        }
        "TOP" => {
            s.top = true;
            return 1 + usize::from(r.label(1) == "U_HOUR");
        }
        "SAME" => {
            s.same = true;
            return 1;
        }
        _ => {}
    }
    // "start of next week", "the middle of the month", "end of next week",
    // "midweek".
    if matches!(l, "START" | "MID" | "END") {
        let edge = match l {
            "START" => Edge::Start,
            "MID" => Edge::Mid,
            _ => Edge::End,
        };
        let mut k = 1;
        let mut shift = shift_of(r.label(k));
        if shift.is_some() {
            k += 1;
        }
        match period_of(r.label(k)) {
            Some(period) => {
                k += 1;
                if shift.is_none()
                    && let Some(after) = shift_of(r.label(k))
                {
                    shift = Some(after);
                    k += 1;
                }
                s.days.push(Day::Edge(edge, period, shift.unwrap_or(0)));
                return k;
            }
            None if edge == Edge::Mid => {
                s.days
                    .push(Day::Edge(edge, Period::Week, shift.unwrap_or(0)));
                return k;
            }
            None => return 0,
        }
    }
    // "the day after tomorrow", "the week after next".
    if l == "U_DAY" && r.label(1) == "AFTER" && r.label(2) == "TOMORROW" {
        s.days.push(Day::Rel(2));
        return 3;
    }
    if r.label(1) == "AFTER" && r.label(2) == "NEXT" {
        match l {
            "U_WEEK" => {
                s.days.push(Day::Of(Period::Week, 2));
                return 3;
            }
            "WEEKEND" => {
                s.days.push(Day::Weekend(Mode::Next));
                s.extra_days += 7;
                return 3;
            }
            _ => {}
        }
        if let Some(wd) = weekday_of(l) {
            s.days.push(Day::Weekday(wd, Mode::Next));
            s.extra_days += 7;
            return 3;
        }
    }
    if let Some(shift) = shift_of(l) {
        let mode = match l {
            "NEXT" => Mode::Next,
            "LAST" => Mode::Last,
            _ => Mode::Bare,
        };
        let next = r.label(1);
        if let Some(wd) = weekday_of(next) {
            s.days.push(Day::Weekday(wd, mode));
            return 2;
        }
        if next == "WEEKEND" && mode != Mode::Last {
            s.days.push(Day::Weekend(mode));
            return 2;
        }
        if let Some(period) = period_of(next).filter(|p| *p != Period::Day) {
            s.days.push(Day::Of(period, shift));
            return 2;
        }
        if is_part(next) && l == "THIS" {
            s.part = Some(r.item(1).map_or("MORNING", |it| it.label));
            return 2;
        }
        return 0;
    }
    if let Some(wd) = weekday_of(l) {
        // "vendredi prochain", "jumat depan", but not the "next" of
        // "friday next week".
        if let Some(shift) = shift_of(r.label(1))
            && period_of(r.label(2)).is_none()
        {
            let mode = match shift {
                1 => Mode::Next,
                -1 => Mode::Last,
                _ => Mode::Bare,
            };
            s.days.push(Day::Weekday(wd, mode));
            return 2;
        }
        s.days.push(Day::Weekday(wd, Mode::Bare));
        return 1;
    }
    // "la semaine prochaine", "minggu depan", "下周".
    if let Some(period) = period_of(l).filter(|p| *p != Period::Day)
        && let Some(shift) = shift_of(r.label(1))
    {
        if l == "WEEKEND" {
            s.days.push(Day::Weekend(if shift > 0 {
                Mode::Next
            } else {
                Mode::Bare
            }));
        } else {
            s.days.push(Day::Of(period, shift));
        }
        return 2;
    }
    if l == "WEEKEND" {
        s.days.push(Day::Weekend(Mode::Bare));
        return 1;
    }
    if is_part(l) {
        s.part = Some(r.item(0).map_or("MORNING", |it| it.label));
        return 1;
    }
    // Months as numbers: "11月21日", "11월 21일", "2026年11月21日", "ngày 21
    // tháng 11".
    let year_first = r
        .num(0)
        .filter(|y| r.digits(0) == 4 && (1900..=2200).contains(y));
    let at = if year_first.is_some() && r.label(1) == "U_YEAR" && r.label(3) == "MONTH_MARK" {
        2
    } else {
        0
    };
    if r.label(at) == "NUM" && r.label(at + 1) == "MONTH_MARK" {
        let latin = r
            .item(at + 1)
            .is_some_and(|it| it.text.starts_with(|c: char| c.is_ascii_alphabetic()));
        if latin {
            // Vietnamese: the day, then "tháng" and the month.
            if let (Some(d), Some(m)) = (r.num(at), r.num(at + 2)) {
                s.days.push(Day::Date(None, m, d));
                return at + 3;
            }
        } else if let Some(m) = r.num(at).filter(|m| (1..=12).contains(m)) {
            let mut k = at + 2;
            let mut d = 1;
            if let Some((day, used)) = day_number(r, k) {
                d = day;
                k += used;
            }
            s.days.push(Day::Date(
                year_first.filter(|_| at == 2).map(|y| y as i32),
                m,
                d,
            ));
            return k;
        }
    }
    if l == "MONTH_MARK"
        && let Some(m) = r.num(1).filter(|m| (1..=12).contains(m))
    {
        s.days.push(Day::Date(None, m, 1));
        return 2;
    }
    // Months: "nov 21", "november 21st 2027", "nov twenty first", "march".
    if let Some(m) = month_of(l) {
        if let Some((d, used)) = day_number(r, 1) {
            let mut k = 1 + used;
            let year = year_at(r, k);
            if year.is_some() {
                k += 1;
            }
            s.days.push(Day::Date(year, m, d));
            return k;
        }
        s.days.push(Day::Date(None, m, 1));
        return 1;
    }
    // "21 nov", "21st of november 2027", "21. November".
    if let Some((d, used)) = day_number(r, 0)
        && let Some(m) = month_of(r.label(used))
    {
        let mut k = used + 1;
        let year = year_at(r, k);
        if year.is_some() {
            k += 1;
        }
        s.days.push(Day::Date(year, m, d));
        return k;
    }
    // Numeric dates: "11/21", "21.11.2026", "2026-11-21".
    if l == "NUM" && r.label(1) == "DATESEP" && r.label(2) == "NUM" {
        let (a, b) = (r.num(0), r.num(2));
        if r.digits(0) == 4
            && r.label(3) == "DATESEP"
            && r.label(4) == "NUM"
            && let (Some(y), Some(m), Some(d)) = (a, b, r.num(4))
        {
            s.days.push(Day::Date(Some(y as i32), m, d));
            return 5;
        }
        if let (Some(a), Some(b)) = (a, b) {
            let dots = r.item(1).is_some_and(|it| it.text == ".");
            let day_first = (dots || r.options.day_first || a > 12) && b <= 12;
            let (m, d) = if day_first { (b, a) } else { (a, b) };
            let mut k = 3;
            let mut year = None;
            if r.label(3) == "DATESEP" && r.label(4) == "NUM" {
                year = r
                    .num(4)
                    .map(|y| if y < 100 { 2000 + y as i32 } else { y as i32 });
                k = 5;
            }
            s.days.push(Day::Date(year, m, d));
            return k;
        }
    }
    // "the 21st"
    if l == "NUM"
        && r.label(1) == "ORD"
        && let Some(d) = r.num(0).filter(|d| (1..=31).contains(d))
    {
        s.days.push(Day::OfMonth(d, None));
        return 2;
    }
    // Clock times: "half past 6", "quarter to seven", "10 past 5",
    // "viertel nach 6".
    let half = matches!(l, "HALF" | "QUARTER");
    let minutes = if half {
        Some((if l == "HALF" { 30 } else { 15 }, 1))
    } else {
        r.count(0).filter(|(n, _)| *n < 60)
    };
    if let Some((m, used)) = minutes
        && matches!(r.label(used), "PAST" | "TO")
        && let Some((h, hused)) = r.hour(used + 1)
    {
        let to = r.label(used) == "TO";
        let (hour, minute) = if to { ((h + 23) % 24, 60 - m) } else { (h, m) };
        let mut k = used + 1 + hused;
        let meridiem = meridiem_at(r, k);
        if meridiem != Meridiem::None {
            k += 1;
        }
        s.clock = Some(Clock {
            hour: if to && h == 1 { 12 } else { hour },
            minute,
            meridiem,
            full: false,
        });
        return k;
    }
    // Lengths of time: "in 2 hrs", "half an hour", "an hour and a half",
    // "1h30", "3 days from now", "2 weeks ago", "il y a 3 jours", "baada ya
    // saa 2", "a week from friday".
    let ago_first = l == "AGO";
    let lead = usize::from(matches!(l, "IN" | "AFTER" | "AGO"));
    if lead == 1 || duration_at(r, 0, false).is_some() {
        if let Some((total, used)) = duration_at(r, lead, lead == 1) {
            let mut k = lead + used;
            let mut sign = if ago_first { -1 } else { 1 };
            match r.label(k) {
                "FROM_NOW" | "LATER" => k += 1,
                "AGO" => {
                    sign = -1;
                    k += 1;
                }
                "FROM" | "AFTER" => {
                    s.anchored = true;
                    k += 1;
                }
                _ => {}
            }
            s.offset = Some((total.0 * sign, total.1 * sign as i32));
            return k;
        }
        if lead == 1 {
            return 0;
        }
    }
    // Clock times: "18:30", "6.30pm", "630pm", "6pm", "at 6", "6 o'clock",
    // "18h30", "6点半", "6시 30분", "a las 6 y media", "12 noon".
    let at = l == "AT";
    let k0 = usize::from(at);
    if r.label(k0) == "NUM" {
        let n = r.num(k0);
        let digits = r.digits(k0);
        if r.label(k0 + 1) == "COLON"
            && let (Some(h), Some(m)) = (n, r.num(k0 + 2))
            && h <= 24
            && m < 60
        {
            let mut k = k0 + 3;
            let meridiem = meridiem_at(r, k);
            if meridiem != Meridiem::None || r.label(k) == "OCLOCK" {
                k += 1;
            }
            let full = meridiem == Meridiem::None
                && (h > 12 || h == 0 || r.item(k0).is_some_and(|it| it.text.starts_with('0')));
            s.clock = Some(Clock {
                hour: h % 24,
                minute: m,
                meridiem,
                full,
            });
            return k;
        }
        if r.label(k0 + 1) == "NOON" {
            s.noon = true;
            return k0 + 2;
        }
        let meridiem = meridiem_at(r, k0 + 1);
        if let Some(n) = n {
            if (3..=4).contains(&digits) && (meridiem != Meridiem::None || at) {
                let (h, m) = (n / 100, n % 100);
                if m < 60 && h <= 23 && (meridiem == Meridiem::None || (1..=12).contains(&h)) {
                    s.clock = Some(Clock {
                        hour: h,
                        minute: m,
                        meridiem,
                        full: meridiem == Meridiem::None,
                    });
                    return k0 + 1 + usize::from(meridiem != Meridiem::None);
                }
            }
            if digits <= 2 && n <= 24 {
                if meridiem != Meridiem::None && (1..=12).contains(&n) {
                    s.clock = Some(Clock {
                        hour: n,
                        minute: 0,
                        meridiem,
                        full: false,
                    });
                    return k0 + 2;
                }
                let oclock = r.label(k0 + 1) == "OCLOCK";
                let mut k = k0 + 1 + usize::from(oclock);
                // Minutes or a half after the hour: "18h30", "6点15分",
                // "6点半", "6 y media", "7 menos cuarto".
                let mut minute = None;
                if oclock && let Some(m) = r.num(k).filter(|m| *m < 60) {
                    minute = Some(m);
                    k += 1 + usize::from(r.label(k + 1) == "U_MIN");
                } else {
                    let and = usize::from(r.label(k) == "AND");
                    match r.label(k + and) {
                        "HALF" if oclock || and == 1 => {
                            minute = Some(30);
                            k += and + 1;
                        }
                        "QUARTER" if and == 1 => {
                            minute = Some(15);
                            k += and + 1;
                        }
                        "TO" if r.label(k + 1) == "QUARTER" => {
                            s.clock = Some(Clock {
                                hour: if n <= 1 { 12 } else { n - 1 },
                                minute: 45,
                                meridiem: meridiem_at(r, k + 2),
                                full: false,
                            });
                            return k + 2;
                        }
                        _ => {}
                    }
                }
                if oclock || at || minute.is_some() {
                    let meridiem = meridiem_at(r, k);
                    if meridiem != Meridiem::None {
                        k += 1;
                    }
                    s.clock = Some(Clock {
                        hour: n % 24,
                        minute: minute.unwrap_or(0),
                        meridiem,
                        full: meridiem == Meridiem::None && (n > 12 || n == 0),
                    });
                    return k;
                }
                s.bare.get_or_insert(n);
                return 1;
            }
        }
        return k0.max(1);
    }
    // "half 7", "साढ़े 6 बजे": half past the hour.
    if r.label(k0) == "HALF"
        && let Some((h, used)) = r.hour(k0 + 1)
        && unit_of(r.label(k0 + 1 + used)).is_none()
    {
        let mut k = k0 + 1 + used;
        if r.label(k) == "OCLOCK" {
            k += 1;
        }
        let meridiem = meridiem_at(r, k);
        if meridiem != Meridiem::None {
            k += 1;
        }
        s.clock = Some(Clock {
            hour: h,
            minute: 30,
            meridiem,
            full: false,
        });
        return k;
    }
    if let Some((h, used)) = r.hour(k0).filter(|_| r.num(k0).is_none()) {
        // Number words: "six", "six thirty pm", "seven oh five".
        let mut k = k0 + used;
        let mut minute = 0;
        if let Some((m, mused)) = r.words(k).filter(|(m, _)| *m < 60) {
            minute = m;
            k += mused;
        }
        let meridiem = meridiem_at(r, k);
        let oclock = r.label(k) == "OCLOCK";
        if meridiem != Meridiem::None || oclock {
            k += 1;
        }
        if at || meridiem != Meridiem::None || oclock || minute > 0 {
            s.clock = Some(Clock {
                hour: h,
                minute,
                meridiem,
                full: false,
            });
            return k;
        }
        s.bare.get_or_insert(h);
        return k;
    }
    0
}

fn repeat_of(unit: &str, interval: u32) -> Repeat {
    let (freq, interval) = match unit {
        "U_MIN" => (Freq::Minutely, interval),
        "U_HOUR" => (Freq::Hourly, interval),
        "U_DAY" => (Freq::Daily, interval),
        "U_WEEK" => (Freq::Weekly, interval),
        "U_FORTNIGHT" => (Freq::Weekly, interval * 2),
        "U_MONTH" => (Freq::Monthly, interval),
        _ => (Freq::Yearly, interval),
    };
    Repeat {
        freq,
        interval: interval.max(1),
        days: vec![],
    }
}

fn meridiem_at(r: &Reader, at: usize) -> Meridiem {
    match r.label(at) {
        "AM" => Meridiem::Am,
        "PM" => Meridiem::Pm,
        _ => Meridiem::None,
    }
}

/// A day of the month at `at`: "21", "21st", "21.", "21日", "twenty first".
/// (day, tokens)
fn day_number(r: &Reader, at: usize) -> Option<(u32, usize)> {
    if let Some(d) = r
        .num(at)
        .filter(|d| (1..=31).contains(d) && r.digits(at) <= 2)
    {
        // Not the hour in "nov at 6pm" or the minutes of "6:30".
        if matches!(r.label(at + 1), "COLON" | "AM" | "PM" | "OCLOCK") {
            return None;
        }
        return Some((d, 1 + usize::from(r.label(at + 1) == "ORD")));
    }
    r.words(at).filter(|(d, _)| (1..=31).contains(d))
}

fn year_at(r: &Reader, at: usize) -> Option<i32> {
    r.num(at)
        .filter(|y| r.digits(at) == 4 && (1900..=2200).contains(y))
        .map(|y| y as i32)
}

/// A length of time at `at`: (minutes, months) and the tokens it takes.
/// `unit_first` also takes the unit before the count ("saa 2").
fn duration_at(r: &Reader, at: usize, unit_first: bool) -> Option<((i64, i32), usize)> {
    // "half an hour", "half a day"
    if r.label(at) == "HALF"
        && let Some((mins, months)) = unit_of(r.label(at + 1 + usize::from(r.label(at + 1) == "A")))
    {
        let used = 2 + usize::from(r.label(at + 1) == "A");
        return Some(((mins / 2, months / 2), used));
    }
    // "a fortnight", "quinze jours" without a count.
    if r.label(at) == "U_FORTNIGHT" {
        return Some(((14 * 24 * 60, 0), 1));
    }
    if unit_first
        && let Some((mins, months)) = unit_of(r.label(at))
        && let Some((n, used)) = r.count(at + 1)
    {
        let n = i64::from(n);
        return Some(((n * mins, n as i32 * months), 1 + used));
    }
    let (n, used) = r.count(at)?;
    let mut k = at + used;
    // "2 and a half hours"
    let mut half = false;
    if r.label(k) == "AND" && r.label(k + 1) == "A" && r.label(k + 2) == "HALF" {
        half = true;
        k += 3;
    }
    let (mins, months) = unit_of(r.label(k))?;
    let unit_label = r.label(k);
    k += 1;
    let n = i64::from(n);
    let mut total = (n * mins, n as i32 * months);
    if half {
        total = (total.0 + mins / 2, total.1 + months / 2);
    }
    // "an hour and a half"
    if r.label(k) == "AND" && r.label(k + 1) == "A" && r.label(k + 2) == "HALF" {
        total = (total.0 + mins / 2, total.1 + months / 2);
        k += 3;
    } else if unit_label == "U_HOUR" {
        // "1h30", "2 hours and 15 minutes"
        let and = usize::from(r.label(k) == "AND");
        if let Some(m) = r.num(k + and).filter(|m| *m < 60) {
            if r.label(k + and + 1) == "U_MIN" {
                total.0 += i64::from(m);
                k += and + 2;
            } else if and == 0 && r.item(k - 1).is_some_and(|it| it.text == "h") {
                total.0 += i64::from(m);
                k += 1;
            }
        }
    }
    Some((total, k - at))
}

// From what was said to a moment.

fn days_in_month(y: i32, m: u32) -> u32 {
    let next = if m == 12 {
        NaiveDate::from_ymd_opt(y + 1, 1, 1)
    } else {
        NaiveDate::from_ymd_opt(y, m + 1, 1)
    };
    next.and_then(|n| n.pred_opt()).map_or(28, |d| d.day())
}

fn plus_days(d: NaiveDate, n: i64) -> Option<NaiveDate> {
    if n >= 0 {
        d.checked_add_days(Days::new(n as u64))
    } else {
        d.checked_sub_days(Days::new((-n) as u64))
    }
}

fn plus_months(t: NaiveDateTime, n: i32) -> Option<NaiveDateTime> {
    if n >= 0 {
        t.checked_add_months(Months::new(n as u32))
    } else {
        t.checked_sub_months(Months::new((-n) as u32))
    }
}

/// The first day of the month `k` months from the one `d` is in.
fn month_start(d: NaiveDate, k: i64) -> Option<NaiveDate> {
    let first = d.with_day(1)?;
    if k >= 0 {
        first.checked_add_months(Months::new(k as u32))
    } else {
        first.checked_sub_months(Months::new((-k) as u32))
    }
}

fn last_of_month(first: NaiveDate) -> Option<NaiveDate> {
    NaiveDate::from_ymd_opt(
        first.year(),
        first.month(),
        days_in_month(first.year(), first.month()),
    )
}

fn at_time(d: NaiveDate, h: u32, m: u32) -> Option<NaiveDateTime> {
    Some(d.and_time(NaiveTime::from_hms_opt(h, m, 0)?))
}

/// The hour of a clock time said with a part of the day or a meridiem;
/// None when it could be either half of the day.
fn hour_of(c: Clock, part: Option<&str>) -> Option<u32> {
    match c.meridiem {
        Meridiem::Am => return Some(if c.hour == 12 { 0 } else { c.hour }),
        Meridiem::Pm => return Some(if c.hour < 12 { c.hour + 12 } else { c.hour }),
        Meridiem::None => {}
    }
    if c.full || c.hour == 0 || c.hour > 12 {
        return Some(c.hour);
    }
    match part {
        Some("AFTERNOON" | "EVENING" | "NIGHT") if c.hour < 12 => Some(c.hour + 12),
        Some("MORNING") => Some(if c.hour == 12 { 0 } else { c.hour }),
        Some(_) => Some(c.hour),
        None => None,
    }
}

/// The hour for a clock time that could be either half of a day that was
/// named: the afternoon up to 6 ("tomorrow at 6" is 18:00), the morning
/// from 7 ("friday at 9" is 9:00).
fn named_day_hour(h: u32) -> u32 {
    if (1..=6).contains(&h) { h + 12 } else { h }
}

/// The time of day said, if any: (hour, minute, and whether it moves to the
/// next day: midnight).
fn time_of(s: &Said, now: NaiveDateTime) -> Option<(u32, u32, bool)> {
    if s.midnight {
        return Some((0, 0, true));
    }
    if s.noon {
        return Some((12, 0, false));
    }
    if let Some(c) = s.clock {
        let h = hour_of(c, s.part).unwrap_or_else(|| named_day_hour(c.hour));
        return Some((h, c.minute, false));
    }
    if let Some(p) = s.part {
        return Some((part_hour(p), 0, false));
    }
    s.same.then(|| (now.hour(), now.minute(), false))
}

/// The upcoming weekday `wd` (Monday 0) from `today`, as `mode` says.
fn weekday_date(today: NaiveDate, wd: u32, mode: Mode, time_left_today: bool) -> Option<NaiveDate> {
    let d = (i64::from(wd) - i64::from(today.weekday().num_days_from_monday())).rem_euclid(7);
    let n = match mode {
        Mode::Bare if d == 0 && time_left_today => 0,
        Mode::Bare | Mode::Next if d == 0 => 7,
        Mode::Bare | Mode::Next => d,
        Mode::Last if d == 0 => -7,
        Mode::Last => d - 7,
    };
    plus_days(today, n)
}

/// The Monday of the week `k` weeks from this one.
fn week_start(today: NaiveDate, k: i64) -> Option<NaiveDate> {
    plus_days(
        today,
        7 * k - i64::from(today.weekday().num_days_from_monday()),
    )
}

/// The moment, and the rule of a repeating time.
fn moment(s: &Said, now: NaiveDateTime) -> Option<(NaiveDateTime, Option<String>)> {
    if let Some(rep) = &s.repeat {
        return repeating(s, rep, now);
    }
    let at = single(s, now)?;
    // "in 4 hours at the top of the hour": the next full hour.
    let at = if s.top && at.minute() != 0 {
        at.with_minute(0)? + TimeDelta::hours(1)
    } else {
        at
    };
    Some((at, None))
}

fn single(s: &Said, now: NaiveDateTime) -> Option<NaiveDateTime> {
    let today = now.date();
    if s.now {
        return Some(now);
    }
    if let Some((mins, months)) = s.offset
        && !(s.anchored && s.day.is_some())
    {
        let base = plus_months(now, months)? + TimeDelta::minutes(mins);
        // "in 3 days at 5pm"
        return match (time_of(s, now), mins.abs() >= 24 * 60 || months != 0) {
            (Some((h, m, next)), true) => at_time(plus_days(base.date(), i64::from(next))?, h, m),
            _ => Some(base),
        };
    }
    if s.later && matches!(s.day, None | Some(Day::Rel(0))) && s.clock.is_none() && s.part.is_none()
    {
        // As Picacho's "Later today": three hours on, on the hour.
        let later = now + TimeDelta::hours(3);
        return if later.date() == today {
            later.with_minute(0)
        } else {
            Some(later)
        };
    }
    let time = time_of(s, now);
    let Some(day) = s.day else {
        if s.top {
            return Some(now);
        }
        // A time alone: its next time.
        let (h, m, next) = time?;
        if next {
            return at_time(plus_days(today, 1)?, h, m);
        }
        let ambiguous = s
            .clock
            .is_some_and(|c| !s.noon && hour_of(c, s.part).is_none());
        let mut choices = vec![];
        if ambiguous && let Some(c) = s.clock {
            for d in 0..2 {
                let date = plus_days(today, d)?;
                choices.push(at_time(date, c.hour % 12, c.minute)?);
                choices.push(at_time(date, c.hour % 12 + 12, c.minute)?);
            }
        } else {
            choices.push(at_time(today, h, m)?);
            choices.push(at_time(plus_days(today, 1)?, h, m)?);
        }
        return choices.into_iter().filter(|t| *t > now).min();
    };
    let (h, m, next) = time.unwrap_or(match day {
        Day::Edge(Edge::End, _, _) => (END_OF_DAY, 0, false),
        Day::Edge(Edge::Mid, Period::Day, _) => (12, 0, false),
        _ => (DEFAULT_HOUR, 0, false),
    });
    let fits_today = |h: u32, m: u32| at_time(today, h, m).is_some_and(|t| t > now);
    let left = !next && fits_today(h, m);
    // Days from a later one: "a week from friday".
    let (extra, extra_months) = match s.offset {
        Some((mins, months)) if s.anchored => (mins.div_euclid(24 * 60), months),
        _ => (0, 0),
    };
    let date = match day {
        Day::Rel(0) if extra == 0 && extra_months == 0 => {
            // "today at 6": whichever half of the day is still to come.
            if let Some(c) = s.clock
                && !s.noon
                && !s.midnight
                && hour_of(c, s.part).is_none()
            {
                let am = c.hour % 12;
                let h = if fits_today(am, c.minute) {
                    am
                } else {
                    am + 12
                };
                return at_time(today, h, c.minute);
            }
            today
        }
        Day::Rel(n) => plus_days(today, n)?,
        Day::Months(k) => today.checked_add_months(Months::new(k.max(0) as u32))?,
        Day::Weekday(wd, mode) => weekday_date(today, wd, mode, left)?,
        Day::WeekdayIn(wd, k) => plus_days(week_start(today, k)?, i64::from(wd))?,
        Day::Weekend(mode) => weekday_date(today, 5, mode, left)?,
        Day::Of(period, k) => edge_date(Edge::Start, period, k, today, left)?,
        Day::Edge(edge, period, k) => edge_date(edge, period, k, today, left)?,
        Day::Date(year, month, d) => match year {
            Some(y) => NaiveDate::from_ymd_opt(y, month, d)?,
            None => match NaiveDate::from_ymd_opt(today.year(), month, d) {
                Some(date) if date >= today => date,
                _ => NaiveDate::from_ymd_opt(today.year() + 1, month, d)?,
            },
        },
        Day::OfMonth(d, None) => month_day(today, d, left)?,
        Day::OfMonth(d, Some(k)) => {
            let first = month_start(today, k)?;
            NaiveDate::from_ymd_opt(first.year(), first.month(), d)?
        }
        Day::Nth(n, wd, month) => nth_weekday(today, n, wd, month, left)?,
    };
    let date = plus_days(date, extra + s.extra_days)?;
    let date = if extra_months > 0 {
        date.checked_add_months(Months::new(extra_months as u32))?
    } else {
        date
    };
    at_time(plus_days(date, i64::from(next))?, h, m)
}

/// The start, middle or end of a period `k` from this one. This one (`k`
/// 0) is the next such day: "the start of the week" said on a Wednesday is
/// next Monday; "the end of the month", the last day of this one.
fn edge_date(
    edge: Edge,
    period: Period,
    k: i64,
    today: NaiveDate,
    left: bool,
) -> Option<NaiveDate> {
    let upcoming = |date: NaiveDate| date > today || (date == today && left);
    match period {
        Period::Day => {
            let date = plus_days(today, k)?;
            if k == 0 && !left {
                plus_days(today, 1)
            } else {
                Some(date)
            }
        }
        Period::Week => {
            let wd = match edge {
                Edge::Start => 0,
                Edge::Mid => 2,
                Edge::End => 4,
            };
            if k == 0 {
                weekday_date(today, wd, Mode::Bare, left)
            } else {
                plus_days(week_start(today, k)?, i64::from(wd))
            }
        }
        Period::Month => {
            let pick = |first: NaiveDate| match edge {
                Edge::Start => Some(first),
                Edge::Mid => first.with_day(15),
                Edge::End => last_of_month(first),
            };
            let date = pick(month_start(today, k)?)?;
            if k == 0 && !upcoming(date) {
                pick(month_start(today, 1)?)
            } else {
                Some(date)
            }
        }
        Period::Year => {
            let pick = |y: i32| match edge {
                Edge::Start => NaiveDate::from_ymd_opt(y, 1, 1),
                Edge::Mid => NaiveDate::from_ymd_opt(y, 7, 1),
                Edge::End => NaiveDate::from_ymd_opt(y, 12, 31),
            };
            let date = pick(today.year() + k as i32)?;
            if k == 0 && !upcoming(date) {
                pick(today.year() + 1)
            } else {
                Some(date)
            }
        }
    }
}

/// The nth weekday `wd` of a month (`n` -1: the last); a month by name or
/// this one that has passed it moves on to the next such month.
fn nth_weekday(today: NaiveDate, n: i32, wd: u32, month: Month, left: bool) -> Option<NaiveDate> {
    let in_month = |first: NaiveDate| -> Option<NaiveDate> {
        if n > 0 {
            let lead =
                (i64::from(wd) - i64::from(first.weekday().num_days_from_monday())).rem_euclid(7);
            let date = plus_days(first, lead + 7 * i64::from(n - 1))?;
            (date.month() == first.month()).then_some(date)
        } else {
            let last = last_of_month(first)?;
            let back =
                (i64::from(last.weekday().num_days_from_monday()) - i64::from(wd)).rem_euclid(7);
            plus_days(last, -back)
        }
    };
    let upcoming = |date: NaiveDate| date > today || (date == today && left);
    match month {
        Month::Shift(k) => {
            let date = in_month(month_start(today, k)?)?;
            if k == 0 && !upcoming(date) {
                in_month(month_start(today, 1)?)
            } else {
                Some(date)
            }
        }
        Month::Named(m) => {
            for y in today.year()..today.year() + 2 {
                if let Some(date) = in_month(NaiveDate::from_ymd_opt(y, m, 1)?)
                    && upcoming(date)
                {
                    return Some(date);
                }
            }
            None
        }
    }
}

/// The next day `d` of a month, from today (today counts while the time is
/// still to come); months without that day are passed over.
fn month_day(today: NaiveDate, d: u32, time_left_today: bool) -> Option<NaiveDate> {
    let mut first = today.with_day(1)?;
    for _ in 0..13 {
        if let Some(date) = NaiveDate::from_ymd_opt(first.year(), first.month(), d)
            && (date > today || (date == today && time_left_today))
        {
            return Some(date);
        }
        first = first.checked_add_months(Months::new(1))?;
    }
    None
}

/// The first time a repeating rule falls after `now`, and the rule.
fn repeating(
    s: &Said,
    rep: &Repeat,
    now: NaiveDateTime,
) -> Option<(NaiveDateTime, Option<String>)> {
    let today = now.date();
    let mut rule = vec![format!(
        "FREQ={}",
        match rep.freq {
            Freq::Minutely => "MINUTELY",
            Freq::Hourly => "HOURLY",
            Freq::Daily => "DAILY",
            Freq::Weekly => "WEEKLY",
            Freq::Monthly => "MONTHLY",
            Freq::Yearly => "YEARLY",
        }
    )];
    if rep.interval > 1 {
        rule.push(format!("INTERVAL={}", rep.interval));
    }
    if matches!(rep.freq, Freq::Minutely | Freq::Hourly) {
        let step = if rep.freq == Freq::Hourly { 60 } else { 1 };
        let first = now + TimeDelta::minutes(i64::from(rep.interval) * step);
        return Some((first, Some(rule.join(";"))));
    }
    let (h, m) = match time_of(s, now) {
        Some((h, m, _)) => (h, m),
        None => (DEFAULT_HOUR, 0),
    };
    let fits_today = at_time(today, h, m).is_some_and(|t| t > now);
    const BYDAY: [&str; 7] = ["MO", "TU", "WE", "TH", "FR", "SA", "SU"];
    // "ทุกวันเสาร์", "daily on saturday": every week, on that day.
    let freq = match (rep.freq, s.day) {
        (Freq::Daily, Some(Day::Weekday(..) | Day::WeekdayIn(..))) if rep.interval == 1 => {
            rule[0] = "FREQ=WEEKLY".into();
            Freq::Weekly
        }
        (f, _) => f,
    };
    let date = match freq {
        Freq::Daily => {
            if fits_today {
                today
            } else {
                plus_days(today, 1)?
            }
        }
        Freq::Weekly => {
            let mut days = rep.days.clone();
            if days.is_empty() {
                days = match s.day {
                    Some(Day::Weekday(wd, _) | Day::WeekdayIn(wd, _)) => vec![wd],
                    _ => vec![today.weekday().num_days_from_monday()],
                };
            }
            days.sort_unstable();
            days.dedup();
            rule.push(format!(
                "BYDAY={}",
                days.iter()
                    .map(|d| BYDAY[*d as usize % 7])
                    .collect::<Vec<_>>()
                    .join(",")
            ));
            days.iter()
                .filter_map(|wd| weekday_date(today, *wd, Mode::Bare, fits_today))
                .min()?
        }
        Freq::Monthly => {
            let d = match s.day {
                Some(Day::OfMonth(d, _) | Day::Date(_, _, d)) => d,
                _ => today.day(),
            };
            rule.push(format!("BYMONTHDAY={d}"));
            month_day(today, d, fits_today)?
        }
        Freq::Yearly => {
            let (month, d) = match s.day {
                Some(Day::Date(_, month, d)) => (month, d),
                _ => (today.month(), today.day()),
            };
            rule.push(format!("BYMONTH={month}"));
            rule.push(format!("BYMONTHDAY={d}"));
            let mut y = today.year();
            loop {
                if let Some(date) = NaiveDate::from_ymd_opt(y, month, d)
                    && (date > today || (date == today && fits_today))
                {
                    break date;
                }
                y += 1;
                if y > today.year() + 8 {
                    return None;
                }
            }
        }
        Freq::Minutely | Freq::Hourly => unreachable!(),
    };
    rule.push(format!("BYHOUR={h}"));
    rule.push(format!("BYMINUTE={m}"));
    Some((at_time(date, h, m)?, Some(rule.join(";"))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{FixedOffset, Utc};

    /// Dates in numbers with the month first, as in the United States.
    const MONTH_FIRST: Options = Options { day_first: false };

    /// Sunday, November 15, 2026, 14:20 (UTC).
    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 15, 14, 20, 0).unwrap()
    }
    fn show(t: i64) -> String {
        DateTime::from_timestamp(t, 0)
            .unwrap()
            .format("%Y-%m-%d %H:%M")
            .to_string()
    }
    /// Resolves `text` with labels given by hand, one per token.
    fn gold(text: &str, labels: &str) -> Option<When> {
        let toks = tokens(text);
        let labels: Vec<&'static str> = labels
            .split_whitespace()
            .map(|l| *LABELS.iter().find(|x| **x == l).expect(l))
            .collect();
        assert_eq!(toks.len(), labels.len(), "{text}: {toks:?}");
        let labelled: Vec<(Token, &'static str)> = toks.into_iter().zip(labels).collect();
        resolve(&labelled, &now(), MONTH_FIRST)
    }
    fn check(text: &str, labels: &str, want: &str) {
        let got = gold(text, labels).map(|w| show(w.at));
        assert_eq!(got.as_deref(), Some(want), "{text}");
    }

    #[test]
    fn tokens_split_letters_digits_and_kept_punctuation() {
        let t: Vec<String> = tokens("Tmrw@6:30PM, o'clock 2nite!")
            .into_iter()
            .map(|t| t.text)
            .collect();
        assert_eq!(
            t,
            [
                "tmrw", "@", "6", ":", "30", "pm", ",", "o", "clock", "2", "nite"
            ]
        );
        let t = tokens("é 6pm");
        assert_eq!((t[1].start, t[1].end, t[1].chars), (3, 4, (2, 3)));
    }

    #[test]
    fn days_and_times() {
        // Sunday 14:20.
        check("tomorrow", "TOMORROW", "2026-11-16 08:00");
        check("tmrw at 6", "TOMORROW AT NUM", "2026-11-16 18:00");
        check("tomorrow at 9", "TOMORROW AT NUM", "2026-11-16 09:00");
        check(
            "tomorrow 6:30am",
            "TOMORROW NUM COLON NUM AM",
            "2026-11-16 06:30",
        );
        check("today at 6", "TODAY AT NUM", "2026-11-15 18:00");
        check("at 3", "AT NUM", "2026-11-15 15:00");
        check("at 2", "AT NUM", "2026-11-16 02:00");
        check("18:30", "NUM COLON NUM", "2026-11-15 18:30");
        check("06:00", "NUM COLON NUM", "2026-11-16 06:00");
        check("630pm", "NUM PM", "2026-11-15 18:30");
        check("tonight", "TONIGHT", "2026-11-15 20:00");
        check("tonight at 9", "TONIGHT AT NUM", "2026-11-15 21:00");
        check("6 tonight", "NUM TONIGHT", "2026-11-15 18:00");
        check("this evening", "THIS EVENING", "2026-11-15 19:00");
        check("in the morning", "O O MORNING", "2026-11-16 08:00");
        check("noon", "NOON", "2026-11-16 12:00");
        check("midnight", "MIDNIGHT", "2026-11-16 00:00");
        check("tomorrow midnight", "TOMORROW MIDNIGHT", "2026-11-17 00:00");
        check("6 in the evening", "NUM O O EVENING", "2026-11-15 18:00");
        check("half past six", "HALF PAST W6", "2026-11-15 18:30");
        check("quarter to 7 pm", "QUARTER TO NUM PM", "2026-11-15 18:45");
        check("10 past 5", "NUM PAST NUM", "2026-11-15 17:10");
        check("six thirty pm", "W6 W30 PM", "2026-11-15 18:30");
        check("at 6 o'clock", "AT NUM OCLOCK OCLOCK", "2026-11-15 18:00");
        check(
            "the day after tomorrow",
            "O U_DAY AFTER TOMORROW",
            "2026-11-17 08:00",
        );
        check("yesterday", "YESTERDAY", "2026-11-14 08:00");
        check("tmrw 6", "TOMORROW NUM", "2026-11-16 18:00");
        check(
            "6 a.m. tomorrow",
            "NUM AM AM AM AM TOMORROW",
            "2026-11-16 06:00",
        );
        check("12 noon friday", "NUM NOON FRI", "2026-11-20 12:00");
    }

    #[test]
    fn weekdays_and_dates() {
        // Sunday, November 15.
        check("friday", "FRI", "2026-11-20 08:00");
        check("next fri 3pm", "NEXT FRI NUM PM", "2026-11-20 15:00");
        check(
            "this saturday at 6pm",
            "THIS SAT AT NUM PM",
            "2026-11-21 18:00",
        );
        check("saturday at 9am", "SAT AT NUM AM", "2026-11-21 09:00");
        check("sunday at 9am", "SUN AT NUM AM", "2026-11-22 09:00");
        check("sunday at 9pm", "SUN AT NUM PM", "2026-11-15 21:00");
        check("next saturday", "NEXT SAT", "2026-11-21 08:00");
        check("next sunday", "NEXT SUN", "2026-11-22 08:00");
        check("last monday", "LAST MON", "2026-11-09 08:00");
        check("monday morning", "MON MORNING", "2026-11-16 08:00");
        check("wed night at 10", "WED NIGHT AT NUM", "2026-11-18 22:00");
        check("nov 21", "NOV NUM", "2026-11-21 08:00");
        check(
            "nov 21st at noon",
            "NOV NUM ORD AT NOON",
            "2026-11-21 12:00",
        );
        check("21 nov", "NUM NOV", "2026-11-21 08:00");
        check("21st of march", "NUM ORD O MAR", "2027-03-21 08:00");
        check("march 3 2028 5pm", "MAR NUM NUM NUM PM", "2028-03-03 17:00");
        check("november twenty first", "NOV W20 W1", "2026-11-21 08:00");
        check("11/21", "NUM DATESEP NUM", "2026-11-21 08:00");
        check(
            "21/11/2027",
            "NUM DATESEP NUM DATESEP NUM",
            "2027-11-21 08:00",
        );
        check(
            "2027-01-05 9:15",
            "NUM DATESEP NUM DATESEP NUM NUM COLON NUM",
            "2027-01-05 09:15",
        );
        check("the 3rd", "O NUM ORD", "2026-12-03 08:00");
        check("the 20th at 4", "O NUM ORD AT NUM", "2026-11-20 16:00");
        check("next week", "NEXT U_WEEK", "2026-11-16 08:00");
        check("next month", "NEXT U_MONTH", "2026-12-01 08:00");
        check("next year", "NEXT U_YEAR", "2027-01-01 08:00");
        check("this weekend", "THIS WEEKEND", "2026-11-21 08:00");
        check("eod", "EOD", "2026-11-15 17:00");
        check("end of the week", "END O O U_WEEK", "2026-11-20 17:00");
        check("end of month", "END O U_MONTH", "2026-11-30 17:00");
        assert_eq!(gold("feb 30", "FEB NUM"), None);
    }

    #[test]
    fn lengths_of_time() {
        check("in 2 hrs", "IN NUM U_HOUR", "2026-11-15 16:20");
        check("in 30 mins", "IN NUM U_MIN", "2026-11-15 14:50");
        check("in an hour", "IN A U_HOUR", "2026-11-15 15:20");
        check("in half an hour", "IN HALF A U_HOUR", "2026-11-15 14:50");
        check(
            "an hour and a half from now",
            "A U_HOUR AND A HALF FROM_NOW FROM_NOW",
            "2026-11-15 15:50",
        );
        check("in 1h30", "IN NUM U_HOUR NUM", "2026-11-15 15:50");
        check(
            "in 2 hours and 15 minutes",
            "IN NUM U_HOUR AND NUM U_MIN",
            "2026-11-15 16:35",
        );
        check(
            "in a couple of days",
            "IN A COUPLE O U_DAY",
            "2026-11-17 14:20",
        );
        check(
            "in 3 days at 5pm",
            "IN NUM U_DAY AT NUM PM",
            "2026-11-18 17:00",
        );
        check(
            "2 weeks from now",
            "NUM U_WEEK FROM_NOW FROM_NOW",
            "2026-11-29 14:20",
        );
        check(
            "in twenty five minutes",
            "IN W20 W5 U_MIN",
            "2026-11-15 14:45",
        );
        check("in a month", "IN A U_MONTH", "2026-12-15 14:20");
        check("3 days ago", "NUM U_DAY AGO", "2026-11-12 14:20");
        check("now", "NOW", "2026-11-15 14:20");
        check("later", "LATER", "2026-11-15 17:00");
        check("2 hours", "NUM U_HOUR", "2026-11-15 16:20");
    }

    #[test]
    fn repeating_times_give_a_rule_and_their_first_time() {
        let rule = |text: &str, labels: &str| {
            let w = gold(text, labels).unwrap();
            (show(w.at), w.rule.unwrap())
        };
        assert_eq!(
            rule("every monday at 9", "EVERY MON AT NUM"),
            (
                "2026-11-16 09:00".into(),
                "FREQ=WEEKLY;BYDAY=MO;BYHOUR=9;BYMINUTE=0".into()
            )
        );
        assert_eq!(
            rule(
                "every other tue and thu @ 6pm",
                "EVERY OTHER TUE AND THU AT NUM PM"
            ),
            (
                "2026-11-17 18:00".into(),
                "FREQ=WEEKLY;INTERVAL=2;BYDAY=TU,TH;BYHOUR=18;BYMINUTE=0".into()
            )
        );
        assert_eq!(
            rule("daily at 7am", "DAILY AT NUM AM"),
            (
                "2026-11-16 07:00".into(),
                "FREQ=DAILY;BYHOUR=7;BYMINUTE=0".into()
            )
        );
        assert_eq!(
            rule("every weekday at 9:30", "EVERY WEEKDAYS AT NUM COLON NUM"),
            (
                "2026-11-16 09:30".into(),
                "FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR;BYHOUR=9;BYMINUTE=30".into()
            )
        );
        assert_eq!(
            rule("every month on the 15th", "EVERY U_MONTH O O NUM ORD"),
            (
                "2026-12-15 08:00".into(),
                "FREQ=MONTHLY;BYMONTHDAY=15;BYHOUR=8;BYMINUTE=0".into()
            )
        );
        assert_eq!(
            rule("every 2 weeks", "EVERY NUM U_WEEK"),
            (
                "2026-11-22 08:00".into(),
                "FREQ=WEEKLY;INTERVAL=2;BYDAY=SU;BYHOUR=8;BYMINUTE=0".into()
            )
        );
        assert_eq!(
            rule("every 15 minutes", "EVERY NUM U_MIN"),
            (
                "2026-11-15 14:35".into(),
                "FREQ=MINUTELY;INTERVAL=15".into()
            )
        );
        assert_eq!(
            rule("yearly on dec 25", "YEARLY O DEC NUM"),
            (
                "2026-12-25 08:00".into(),
                "FREQ=YEARLY;BYMONTH=12;BYMONTHDAY=25;BYHOUR=8;BYMINUTE=0".into()
            )
        );
        assert_eq!(
            rule("every evening", "EVERY EVENING"),
            (
                "2026-11-15 19:00".into(),
                "FREQ=DAILY;BYHOUR=19;BYMINUTE=0".into()
            )
        );
    }

    #[test]
    fn text_without_a_time_gives_nothing() {
        assert_eq!(gold("hello there", "O O"), None);
        assert_eq!(gold("6 apples", "NUM O"), None);
        assert_eq!(gold("at home", "AT O"), None);
        assert_eq!(gold("in the box", "IN O O"), None);
        assert_eq!(gold("the second one", "O W2 O"), None);
    }

    #[test]
    fn the_span_is_where_the_time_was_said() {
        let text = "remind me to call mom tmrw at 6 pls";
        let w = gold(text, "O O O O O TOMORROW AT NUM O").unwrap();
        assert_eq!(&text[w.start..w.end], "tmrw at 6");
    }

    #[test]
    fn local_time_is_used_and_clock_changes_are_survived() {
        let toks = tokens("tomorrow at 9am");
        let labelled: Vec<(Token, &'static str)> = toks
            .into_iter()
            .zip(["TOMORROW", "AT", "NUM", "AM"])
            .collect();
        let tz = FixedOffset::west_opt(7 * 3600).unwrap();
        let now = tz.with_ymd_and_hms(2026, 11, 15, 23, 30, 0).unwrap();
        let w = resolve(&labelled, &now, MONTH_FIRST).unwrap();
        assert_eq!(
            w.at,
            tz.with_ymd_and_hms(2026, 11, 16, 9, 0, 0)
                .unwrap()
                .timestamp()
        );
    }

    #[test]
    fn newer_phrasings() {
        // Sunday, November 15, 14:20.
        check("overmorrow", "OVERMORROW", "2026-11-17 08:00");
        check("in a fortnight", "IN A U_FORTNIGHT", "2026-11-29 14:20");
        check(
            "in 4 hours at the top of the hour",
            "IN NUM U_HOUR O O TOP O O U_HOUR",
            "2026-11-15 19:00",
        );
        check("on the hour", "TOP O U_HOUR", "2026-11-15 15:00");
        check(
            "the monday in three weeks",
            "O MON IN W3 U_WEEK",
            "2026-11-30 08:00",
        );
        check(
            "in 2 weeks on friday at 5pm",
            "IN NUM U_WEEK O FRI AT NUM PM",
            "2026-11-27 17:00",
        );
        check("friday next week", "FRI NEXT U_WEEK", "2026-11-20 08:00");
        check("next week tuesday", "NEXT U_WEEK TUE", "2026-11-17 08:00");
        check(
            "start of next week at 7am",
            "START O NEXT U_WEEK AT NUM AM",
            "2026-11-16 07:00",
        );
        check(
            "the beginning of next month",
            "O START O NEXT U_MONTH",
            "2026-12-01 08:00",
        );
        check(
            "start of the month",
            "START O O U_MONTH",
            "2026-12-01 08:00",
        );
        check("mid next month", "MID NEXT U_MONTH", "2026-12-15 08:00");
        check("middle of the week", "MID O O U_WEEK", "2026-11-18 08:00");
        check("end of next week", "END O NEXT U_WEEK", "2026-11-20 17:00");
        check("end of the year", "END O O U_YEAR", "2026-12-31 17:00");
        check(
            "the week after next",
            "O U_WEEK AFTER NEXT",
            "2026-11-23 08:00",
        );
        check("friday after next", "FRI AFTER NEXT", "2026-11-27 08:00");
        check(
            "a week from friday",
            "A U_WEEK FROM FRI",
            "2026-11-27 08:00",
        );
        check(
            "2 weeks from tomorrow at 9",
            "NUM U_WEEK FROM TOMORROW AT NUM",
            "2026-11-30 09:00",
        );
        check(
            "3 days after monday",
            "NUM U_DAY FROM MON",
            "2026-11-19 08:00",
        );
        check(
            "the first monday of next month",
            "O W1 MON O NEXT U_MONTH",
            "2026-12-07 08:00",
        );
        check(
            "the 2nd tuesday in january",
            "O NUM ORD TUE O JAN",
            "2027-01-12 08:00",
        );
        check(
            "last friday of the month",
            "LAST FRI O O U_MONTH",
            "2026-11-27 08:00",
        );
        check(
            "the first day of next month",
            "O W1 U_DAY O NEXT U_MONTH",
            "2026-12-01 08:00",
        );
        check(
            "last day of the month",
            "LAST U_DAY O O U_MONTH",
            "2026-11-30 17:00",
        );
        check(
            "the 15th of next month",
            "O NUM ORD O NEXT U_MONTH",
            "2026-12-15 08:00",
        );
        check("this time tomorrow", "SAME O TOMORROW", "2026-11-16 14:20");
        check(
            "same time next week",
            "SAME O NEXT U_WEEK",
            "2026-11-22 14:20",
        );
        check(
            "first thing tomorrow",
            "MORNING MORNING TOMORROW",
            "2026-11-16 08:00",
        );
        check("quarter after 6", "QUARTER PAST NUM", "2026-11-15 18:15");
        check("half 7", "HALF NUM", "2026-11-15 19:30");
        let w = gold("every fortnight", "EVERY U_FORTNIGHT").unwrap();
        assert_eq!(
            w.rule.as_deref(),
            Some("FREQ=WEEKLY;INTERVAL=2;BYDAY=SU;BYHOUR=8;BYMINUTE=0")
        );
    }

    #[test]
    fn other_languages_word_orders() {
        check(
            "vendredi prochain à 18h30",
            "FRI NEXT AT NUM OCLOCK NUM",
            "2026-11-20 18:30",
        );
        check("la semaine prochaine", "O U_WEEK NEXT", "2026-11-16 08:00");
        check(
            "il y a 3 jours",
            "AGO AGO AGO NUM U_DAY",
            "2026-11-12 14:20",
        );
        check(
            "übermorgen um 18 uhr",
            "OVERMORROW AT NUM OCLOCK",
            "2026-11-17 18:00",
        );
        check(
            "freitag nächste woche",
            "FRI NEXT U_WEEK",
            "2026-11-20 08:00",
        );
        check("am 21. november", "O NUM ORD NOV", "2026-11-21 08:00");
        check("21.11.", "NUM DATESEP NUM O", "2026-11-21 08:00");
        check(
            "a las 6 y media de la tarde",
            "AT AT NUM AND HALF AFTERNOON AFTERNOON AFTERNOON",
            "2026-11-15 18:30",
        );
        check(
            "a las 7 menos cuarto",
            "AT AT NUM TO QUARTER",
            "2026-11-15 18:45",
        );
        check("pasado mañana", "OVERMORROW OVERMORROW", "2026-11-17 08:00");
        check("dentro de 2 horas", "IN IN NUM U_HOUR", "2026-11-15 16:20");
        check("3 giorni fa", "NUM U_DAY AGO", "2026-11-12 14:20");
        check("через 2 часа", "IN NUM U_HOUR", "2026-11-15 16:20");
        check("2 saat sonra", "NUM U_HOUR FROM_NOW", "2026-11-15 16:20");
        check(
            "明天下午6点半",
            "TOMORROW TOMORROW AFTERNOON AFTERNOON NUM OCLOCK HALF",
            "2026-11-16 18:30",
        );
        check("下周五", "NEXT U_WEEK FRI", "2026-11-20 08:00");
        check("3天后", "NUM U_DAY FROM_NOW", "2026-11-18 14:20");
        check("11月21日", "NUM MONTH_MARK NUM ORD", "2026-11-21 08:00");
        check(
            "2027年1月5日",
            "NUM U_YEAR NUM MONTH_MARK NUM ORD",
            "2027-01-05 08:00",
        );
        check(
            "来週の金曜日の午後3時",
            "NEXT U_WEEK O FRI FRI FRI O AFTERNOON AFTERNOON NUM OCLOCK",
            "2026-11-20 15:00",
        );
        check(
            "내일 오후 6시 30분",
            "TOMORROW AFTERNOON NUM OCLOCK NUM U_MIN",
            "2026-11-16 18:30",
        );
        check("11월 21일", "NUM MONTH_MARK NUM ORD", "2026-11-21 08:00");
        check(
            "ngày 21 tháng 11",
            "O NUM MONTH_MARK NUM",
            "2026-11-21 08:00",
        );
        check(
            "6 giờ rưỡi chiều",
            "NUM OCLOCK HALF AFTERNOON",
            "2026-11-15 18:30",
        );
        check(
            "कल शाम ६ बजे",
            "TOMORROW EVENING NUM OCLOCK",
            "2026-11-16 18:00",
        );
        check("साढ़े 6 बजे", "HALF NUM OCLOCK", "2026-11-15 18:30");
        check("baada ya saa 2", "IN IN U_HOUR NUM", "2026-11-15 16:20");
        check("jumat depan jam 9", "FRI NEXT AT NUM", "2026-11-20 09:00");
        check("minggu depan", "U_WEEK NEXT", "2026-11-16 08:00");
        check(
            "غدا الساعة ٦ مساء",
            "TOMORROW AT NUM EVENING",
            "2026-11-16 18:00",
        );
        check("قبل 3 أيام", "AGO NUM U_DAY", "2026-11-12 14:20");
    }

    #[test]
    fn numeric_dates_follow_the_locale_unless_the_numbers_decide() {
        let labelled = |text: &str, labels: &[&'static str]| -> Vec<(Token, &'static str)> {
            tokens(text)
                .into_iter()
                .zip(labels.iter().copied())
                .collect()
        };
        let at = |text: &str, labels: &[&'static str], day_first: bool| {
            resolve(&labelled(text, labels), &now(), Options { day_first }).map(|w| show(w.at))
        };
        let slash = ["NUM", "DATESEP", "NUM"];
        assert_eq!(
            at("3/4", &slash, false).as_deref(),
            Some("2027-03-04 08:00")
        );
        assert_eq!(at("3/4", &slash, true).as_deref(), Some("2027-04-03 08:00"));
        assert_eq!(
            at("21/11", &slash, false).as_deref(),
            Some("2026-11-21 08:00")
        );
        assert_eq!(
            at("11/21", &slash, true).as_deref(),
            Some("2026-11-21 08:00")
        );
        assert_eq!(
            at("3.4", &slash, false).as_deref(),
            Some("2027-04-03 08:00")
        );
        assert!(!Options::for_locale("en-US").day_first);
        assert!(!Options::for_locale("ja-JP").day_first);
        assert!(!Options::for_locale("en_PH").day_first);
        assert!(Options::for_locale("en-GB").day_first);
        assert!(Options::for_locale("de-DE").day_first);
        assert!(Options::for_locale("fr").day_first);
        assert!(Options::for_locale("pt_BR.UTF-8").day_first);
    }

    // With the model.

    fn parser() -> Parser {
        let path = crate::manage::local_ai::test_model("tbs-v1.bin");
        Parser::new(NativeTagger::load(&path).unwrap()).with_options(MONTH_FIRST)
    }

    #[test]
    fn matches_the_training_code() {
        let parity: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("models/tbs/parity.json"),
            )
            .unwrap(),
        )
        .unwrap();
        let labels: Vec<&str> = parity["labels"]
            .as_array()
            .unwrap()
            .iter()
            .map(|l| l.as_str().unwrap())
            .collect();
        assert_eq!(labels, LABELS);
        let parser = parser();
        for case in parity["cases"].as_array().unwrap() {
            let text = case["text"].as_str().unwrap();
            let toks = tokens(text);
            let got: Vec<(String, usize, usize)> = toks
                .iter()
                .map(|t| (t.text.clone(), t.chars.0, t.chars.1))
                .collect();
            let want: Vec<(String, usize, usize)> = case["tokens"]
                .as_array()
                .unwrap()
                .iter()
                .map(|t| {
                    (
                        t[0].as_str().unwrap().into(),
                        t[1].as_u64().unwrap() as usize,
                        t[2].as_u64().unwrap() as usize,
                    )
                })
                .collect();
            assert_eq!(got, want, "{text}");
            let words: Vec<&str> = toks.iter().map(|t| t.text.as_str()).collect();
            let glue = joined(&toks);
            for i in 0..toks.len() {
                let mut f = features(&words, i, &glue);
                let mut w: Vec<String> = case["features"][i]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|s| s.as_str().unwrap().into())
                    .collect();
                f.sort();
                w.sort();
                assert_eq!(f, w, "{text}, token {i}");
                let v = vector(&features(&words, i, &glue));
                let want: Vec<(usize, f32)> = case["vectors"][i]
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
                assert_eq!(v.len(), want.len());
                for ((a, x), (b, y)) in v.iter().zip(&want) {
                    assert_eq!(a, b);
                    assert!((x - y).abs() < 1e-5);
                }
            }
            let labelled = parser.label(text);
            let got: Vec<&str> = labelled.iter().map(|(_, l, _)| *l).collect();
            let want: Vec<&str> = case["labels"]
                .as_array()
                .unwrap()
                .iter()
                .map(|l| l.as_str().unwrap())
                .collect();
            assert_eq!(got, want, "{text}");
            for ((_, _, p), q) in labelled
                .iter()
                .zip(case["probabilities"].as_array().unwrap())
            {
                assert!((p - q.as_f64().unwrap() as f32).abs() < 1e-3, "{text}");
            }
        }
    }

    #[test]
    fn reads_untidy_typing() {
        let parser = parser();
        let now = now();
        for (text, want) in [
            ("tmrw at 6", "2026-11-16 18:00"),
            ("remind me to call mom tomorow at 6 pls", "2026-11-16 18:00"),
            ("next fri 3pm", "2026-11-20 15:00"),
            ("in 2 hrs", "2026-11-15 16:20"),
            ("Wensday mornin", "2026-11-18 08:00"),
            ("nov 21st at noon", "2026-11-21 12:00"),
            ("Snooze until 11/21/2026 18:30", "2026-11-21 18:30"),
            ("quarter past six tonite", "2026-11-15 18:15"),
            ("an hour and a half from now", "2026-11-15 15:50"),
            ("tommorow 9am", "2026-11-16 09:00"),
            ("thurs @ 4", "2026-11-19 16:00"),
            ("in 30 min", "2026-11-15 14:50"),
            ("TMRW 8PM", "2026-11-16 20:00"),
            ("next week", "2026-11-16 08:00"),
            ("6pm tomorow", "2026-11-16 18:00"),
            ("the monday in three weeks", "2026-11-30 08:00"),
            ("start of next week at 7am", "2026-11-16 07:00"),
            ("in 4 hours at the top of the hour", "2026-11-15 19:00"),
            ("overmorrow at noon", "2026-11-17 12:00"),
            ("in a fortnight", "2026-11-29 14:20"),
            ("Erinnere mich übermorgen um 18:30 Uhr", "2026-11-17 18:30"),
            ("rappelle-moi vendredi prochain à 18h30", "2026-11-20 18:30"),
            (
                "recuérdame pasado mañana a las 6 de la tarde",
                "2026-11-17 18:00",
            ),
            ("明天下午6点半提醒我", "2026-11-16 18:30"),
            ("来週の金曜日の午後3時", "2026-11-20 15:00"),
            ("내일 오후 6시 30분", "2026-11-16 18:30"),
            ("พรุ่งนี้ 6 โมงเย็น", "2026-11-16 18:00"),
            ("कल शाम ६ बजे", "2026-11-16 18:00"),
            ("غدا الساعة ٦ مساء", "2026-11-16 18:00"),
            ("через 2 часа", "2026-11-15 16:20"),
            ("2 saat sonra", "2026-11-15 16:20"),
        ] {
            let got = parser.parse(text, &now).map(|w| show(w.at));
            assert_eq!(
                got.as_deref(),
                Some(want),
                "{text}: {:?}",
                parser.label(text)
            );
        }
        let w = parser.parse("every other monday @ 9:30am", &now).unwrap();
        assert_eq!(
            w.rule.as_deref(),
            Some("FREQ=WEEKLY;INTERVAL=2;BYDAY=MO;BYHOUR=9;BYMINUTE=30")
        );
        for text in [
            "may i ask a question",
            "I sat down with the team on the 2nd floor",
            "hello there",
            "the second one is better",
            "i have 6 apples",
        ] {
            assert_eq!(
                parser.parse(text, &now),
                None,
                "{text}: {:?}",
                parser.label(text)
            );
        }
    }

    /// Texts from the held-out half of the training wording, in every
    /// language (with fresh typing slips): the model's labels must give the
    /// same moment as the true labels, nearly always.
    #[test]
    fn gives_the_moments_the_true_labels_give() {
        let eval: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("models/tbs/eval.json"),
            )
            .unwrap(),
        )
        .unwrap();
        let parser = parser();
        let now = now();
        // (same, texts, with a time and same, with a time) by language.
        let mut by: std::collections::BTreeMap<String, [usize; 4]> = Default::default();
        let mut wrong = vec![];
        for case in eval.as_array().unwrap() {
            let text = case["text"].as_str().unwrap();
            let language = case["language"].as_str().unwrap_or("en").to_owned();
            let labels: Vec<&'static str> = case["labels"]
                .as_array()
                .unwrap()
                .iter()
                .map(|l| *LABELS.iter().find(|x| **x == l.as_str().unwrap()).unwrap())
                .collect();
            let labelled: Vec<(Token, &'static str)> =
                tokens(text).into_iter().zip(labels).collect();
            let want = resolve(&labelled, &now, MONTH_FIRST).map(|w| (w.at, w.rule));
            let got = parser.parse(text, &now).map(|w| (w.at, w.rule));
            let ok = got == want;
            let e = by.entry(language.clone()).or_default();
            e[0] += usize::from(ok);
            e[1] += 1;
            if want.is_some() {
                e[2] += usize::from(ok);
                e[3] += 1;
            }
            if !ok && wrong.len() < 40 {
                wrong.push(format!("{language} {text:?}: {got:?} vs {want:?}"));
            }
        }
        let share = |a: usize, b: usize| a as f64 / b.max(1) as f64;
        let total = by.values().fold([0; 4], |t, e| {
            [t[0] + e[0], t[1] + e[1], t[2] + e[2], t[3] + e[3]]
        });
        eprintln!(
            "same moment: {:.3} of {} texts; with a time {:.3} of {}",
            share(total[0], total[1]),
            total[1],
            share(total[2], total[3]),
            total[3]
        );
        for (language, e) in &by {
            eprintln!(
                "  {language}: {:.3} of {} ({:.3} of {} with a time)",
                share(e[0], e[1]),
                e[1],
                share(e[2], e[3]),
                e[3]
            );
        }
        for w in &wrong {
            eprintln!("  {w}");
        }
        assert!(share(total[0], total[1]) > 0.95);
        for (language, e) in &by {
            assert!(share(e[0], e[1]) > 0.85, "{language}");
        }
    }
}

//! Dates and times as the language shown writes them.
//!
//! A date's shape is a chrono pattern from `locales/<language>/dates.ftl`
//! (`t!("dates-month-day")` is `%b %-d` in English), so each language puts
//! the day, month and year in its own order. Chrono writes names in
//! English; `format` puts the language's own month and weekday names, and
//! its AM and PM, in their place.

use chrono::{DateTime, Datelike, NaiveDate, NaiveDateTime, TimeZone, Timelike, Weekday};

/// What `format` writes: a date, with a time of day or without.
pub trait Dated: Datelike {
    /// Chrono's own rendering of `pattern`.
    fn chrono(&self, pattern: &str) -> String;
    /// Whether it's afternoon, when it has a time of day.
    fn pm(&self) -> Option<bool>;
}

impl Dated for NaiveDate {
    fn chrono(&self, pattern: &str) -> String {
        self.format(pattern).to_string()
    }
    fn pm(&self) -> Option<bool> {
        None
    }
}

impl Dated for NaiveDateTime {
    fn chrono(&self, pattern: &str) -> String {
        self.format(pattern).to_string()
    }
    fn pm(&self) -> Option<bool> {
        Some(self.hour() >= 12)
    }
}

impl<Tz: TimeZone> Dated for DateTime<Tz>
where
    Tz::Offset: std::fmt::Display,
{
    fn chrono(&self, pattern: &str) -> String {
        self.format(pattern).to_string()
    }
    fn pm(&self) -> Option<bool> {
        Some(self.hour() >= 12)
    }
}

/// `at` written with `pattern` (a chrono pattern, usually a `dates-…`
/// message), its names (`%a %A %b %h %B %p`) in the language shown.
pub fn format(at: &impl Dated, pattern: &str) -> String {
    let mut out = String::with_capacity(pattern.len() + 16);
    let mut chars = pattern.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        // Padding flags (%-d, %_H, %0e) belong to the specifier after them.
        let mut spec = String::from('%');
        while let Some(&flag) = chars.peek().filter(|f| matches!(f, '-' | '_' | '0')) {
            spec.push(flag);
            chars.next();
        }
        let Some(letter) = chars.next() else {
            out.push_str(&spec);
            break;
        };
        let name = match letter {
            'a' => Some(weekday(at.weekday(), true)),
            'A' => Some(weekday(at.weekday(), false)),
            'b' | 'h' => Some(month(at.month(), true)),
            'B' => Some(month(at.month(), false)),
            'p' => at
                .pm()
                .map(|pm| t!("dates-am-pm", half = if pm { "pm" } else { "am" })),
            _ => None,
        };
        match name {
            // Chrono writes the name as it is.
            Some(name) => out.push_str(&name.replace('%', "%%")),
            None => {
                out.push_str(&spec);
                out.push(letter);
            }
        }
    }
    at.chrono(&out)
}

/// `format` as a method: `at.local(t!("dates-month-day"))`.
pub trait Local: Dated {
    /// This date written with `pattern`, in the language shown.
    fn local(&self, pattern: &str) -> String {
        format(self, pattern)
    }
}
impl<T: Dated> Local for T {}

/// A month's name (1 is January), or its short form.
pub fn month(number: u32, short: bool) -> String {
    if short {
        t!("dates-month-short", month = number)
    } else {
        t!("dates-month", month = number)
    }
}

/// A weekday's name, or its short form.
pub fn weekday(day: Weekday, short: bool) -> String {
    let day = day.number_from_monday();
    if short {
        t!("dates-weekday-short", day = day)
    } else {
        t!("dates-weekday", day = day)
    }
}

#[cfg(test)]
mod tests {
    use super::format;
    use chrono::{NaiveDate, TimeZone, Utc};

    #[test]
    fn english_dates_read_as_chrono_writes_them() {
        let at = Utc.with_ymd_and_hms(2026, 11, 24, 15, 5, 0).unwrap();
        for pattern in [
            "%a, %b %-d %Y at %-H:%M",
            "%A, %B %-d, %Y, %H:%M",
            "%b %-d, %Y · %-I:%M %p",
            "%h %e %%a",
        ] {
            assert_eq!(
                format(&at, pattern),
                at.format(pattern).to_string(),
                "{pattern}"
            );
        }
        let day = NaiveDate::from_ymd_opt(2026, 1, 4).unwrap();
        assert_eq!(format(&day, "%A, %B %-d"), "Sunday, January 4");
        // No time of day: %p stays chrono's (which leaves it out).
        assert_eq!(format(&day, t!("dates-month-day")), "Jan 4");
    }
}

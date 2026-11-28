//! Calendar and Contacts as views of their own beside the mail (from the
//! sidebar on desktop and from Mailboxes on phones), the calendar's Day and
//! Week hour-by-hour time grids and Month grid beside its agenda, and a full
//! custom repeat rule editor for Daily, Weekly, Monthly and Yearly events.
use super::*;
use chrono::{Datelike, Duration as Days, Local, NaiveDate, TimeZone, Timelike};

/// Height of one hour in the Day and Week time grids, in logical pixels.
const HOUR_HEIGHT: f32 = 52.;
/// Width of the left hour-label gutter in the Day and Week time grids.
const GUTTER_WIDTH: f32 = 52.;
/// Minimum visual height of a timed event block so short events stay readable.
const MIN_EVENT_HEIGHT: f32 = 22.;
/// Visible viewport height of the scrollable 24-hour grid.
const GRID_VIEWPORT_HEIGHT: f32 = 560.;

/// A view in place of the message list and reader.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AppView {
    Calendar,
    Contacts,
    Tasks,
    /// The attachment browser (`attachments.rs`).
    Attachments,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum CalendarView {
    #[default]
    Agenda,
    Day,
    Week,
    Month,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Freq {
    Daily,
    #[default]
    Weekly,
    Monthly,
    Yearly,
}
impl Freq {
    fn key(self) -> &'static str {
        match self {
            Self::Daily => "DAILY",
            Self::Weekly => "WEEKLY",
            Self::Monthly => "MONTHLY",
            Self::Yearly => "YEARLY",
        }
    }
    fn unit(self) -> &'static str {
        match self {
            Self::Daily => "days",
            Self::Weekly => "weeks",
            Self::Monthly => "months",
            Self::Yearly => "years",
        }
    }
}

/// How a Monthly or Yearly custom repeat chooses which day(s) it lands on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum PatternMode {
    /// Specific day(s) of the month (`BYMONTHDAY`, e.g. 15 or -1 for last day).
    #[default]
    OnDayOfMonth,
    /// An ordinal weekday/day (e.g. 2nd Tuesday, last Friday, last weekday).
    OnNthWeekday,
}

/// Ordinal for `PatternMode::OnNthWeekday`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Ordinal {
    #[default]
    First,
    Second,
    Third,
    Fourth,
    Last,
}
impl Ordinal {
    const ALL: [Self; 5] = [
        Self::First,
        Self::Second,
        Self::Third,
        Self::Fourth,
        Self::Last,
    ];
    fn label(self) -> &'static str {
        match self {
            Self::First => "1st",
            Self::Second => "2nd",
            Self::Third => "3rd",
            Self::Fourth => "4th",
            Self::Last => t!("calendar-views-last"),
        }
    }
    fn pos(self) -> i32 {
        match self {
            Self::First => 1,
            Self::Second => 2,
            Self::Third => 3,
            Self::Fourth => 4,
            Self::Last => -1,
        }
    }
    fn from_pos(n: i32) -> Option<Self> {
        match n {
            1 => Some(Self::First),
            2 => Some(Self::Second),
            3 => Some(Self::Third),
            4 => Some(Self::Fourth),
            -1 => Some(Self::Last),
            _ => None,
        }
    }
}

/// Day target for `PatternMode::OnNthWeekday`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum NthTarget {
    #[default]
    Mon,
    Tue,
    Wed,
    Thu,
    Fri,
    Sat,
    Sun,
    Day,
    Weekday,
    Weekend,
}
impl NthTarget {
    const ALL: [Self; 10] = [
        Self::Mon,
        Self::Tue,
        Self::Wed,
        Self::Thu,
        Self::Fri,
        Self::Sat,
        Self::Sun,
        Self::Day,
        Self::Weekday,
        Self::Weekend,
    ];
    fn label(self) -> &'static str {
        match self {
            Self::Mon => t!("calendar-views-mon"),
            Self::Tue => t!("calendar-views-tue"),
            Self::Wed => t!("calendar-views-wed"),
            Self::Thu => t!("calendar-views-thu"),
            Self::Fri => t!("calendar-views-fri"),
            Self::Sat => t!("calendar-views-sat"),
            Self::Sun => t!("calendar-views-sun"),
            Self::Day => t!("calendar-views-day"),
            Self::Weekday => t!("calendar-views-weekday"),
            Self::Weekend => t!("calendar-views-weekend-day"),
        }
    }
    fn weekday_code(self) -> Option<&'static str> {
        Some(match self {
            Self::Mon => "MO",
            Self::Tue => "TU",
            Self::Wed => "WE",
            Self::Thu => "TH",
            Self::Fri => "FR",
            Self::Sat => "SA",
            Self::Sun => "SU",
            _ => return None,
        })
    }
    fn from_chrono(d: chrono::Weekday) -> Self {
        match d {
            chrono::Weekday::Mon => Self::Mon,
            chrono::Weekday::Tue => Self::Tue,
            chrono::Weekday::Wed => Self::Wed,
            chrono::Weekday::Thu => Self::Thu,
            chrono::Weekday::Fri => Self::Fri,
            chrono::Weekday::Sat => Self::Sat,
            chrono::Weekday::Sun => Self::Sun,
        }
    }
}

/// How a custom repeat ends.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Ends {
    #[default]
    Never,
    After,
    On,
}

/// The days of the week, Monday first: their RFC 5545 codes and short names.
fn weekday_names() -> [(&'static str, &'static str); 7] {
    [
        ("MO", t!("calendar-views-mon")),
        ("TU", t!("calendar-views-tue")),
        ("WE", t!("calendar-views-wed")),
        ("TH", t!("calendar-views-thu")),
        ("FR", t!("calendar-views-fri")),
        ("SA", t!("calendar-views-sat")),
        ("SU", t!("calendar-views-sun")),
    ]
}

/// The months' short names, January first.
fn month_names() -> [&'static str; 12] {
    [
        t!("calendar-views-jan"),
        t!("calendar-views-feb"),
        t!("calendar-views-mar"),
        t!("calendar-views-apr"),
        t!("calendar-views-may"),
        t!("calendar-views-jun"),
        t!("calendar-views-jul"),
        t!("calendar-views-aug"),
        t!("calendar-views-sep"),
        t!("calendar-views-oct"),
        t!("calendar-views-nov"),
        t!("calendar-views-dec"),
    ]
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum CalAction {
    Open(AppView),
    Close,
    View(CalendarView),
    Previous,
    Next,
    Today,
    /// Shows this day (from the month grid).
    OpenDay(NaiveDate),
    /// Shows or hides the custom repeat form.
    Custom(bool),
    Freq(Freq),
    ToggleDay(usize),
    ToggleMonth(usize),
    Pattern(PatternMode),
    Ordinal(Ordinal),
    NthTarget(NthTarget),
    Ends(Ends),
    ApplyCustom,
}

/// Inputs to `custom_rule`, grouped so tests and UI build the exact same RRULE.
#[derive(Clone, Debug, Default)]
pub(super) struct CustomRuleSpec {
    pub freq: Freq,
    pub interval: u32,
    pub days: [bool; 7],
    pub months: [bool; 12],
    pub pattern: PatternMode,
    pub month_days: Vec<i32>,
    pub ordinal: Ordinal,
    pub nth_target: NthTarget,
    pub ends: Ends,
    pub count: u32,
    pub until: Option<NaiveDate>,
}

pub(super) struct CalViews {
    pub(super) view: CalendarView,
    /// The day the Day, Week and Month views show, or show around.
    pub(super) anchor: NaiveDate,
    pub(super) custom_open: bool,
    pub(super) freq: Freq,
    pub(super) interval: Entity<InputState>,
    pub(super) days: [bool; 7],
    pub(super) months: [bool; 12],
    pub(super) pattern: PatternMode,
    pub(super) month_day: Entity<InputState>,
    pub(super) ordinal: Ordinal,
    pub(super) nth_target: NthTarget,
    pub(super) ends: Ends,
    pub(super) count: Entity<InputState>,
    pub(super) until: Entity<InputState>,
    pub(super) scroll: ScrollHandle,
    /// Vertical scroll handle of the 24-hour Day/Week time grid.
    pub(super) grid_scroll: ScrollHandle,
    /// Whether the time grid has auto-scrolled to business hours yet.
    pub(super) grid_scrolled: bool,
}
impl CalViews {
    pub(super) fn new(window: &mut Window, cx: &mut App) -> Self {
        let today = Local::now().date_naive();
        let mut months = [false; 12];
        months[today.month0() as usize] = true;
        Self {
            view: CalendarView::default(),
            anchor: today,
            custom_open: false,
            freq: Freq::default(),
            interval: input("1", false, window, cx),
            days: [false; 7],
            months,
            pattern: PatternMode::default(),
            month_day: input(t!("calendar-views-15-or-1-for-last-day"), false, window, cx),
            ordinal: Ordinal::default(),
            nth_target: NthTarget::from_chrono(today.weekday()),
            ends: Ends::default(),
            count: input("10", false, window, cx),
            until: input("2027-06-30", false, window, cx),
            scroll: ScrollHandle::new(),
            grid_scroll: ScrollHandle::new(),
            grid_scrolled: false,
        }
    }
}

/// Parses a comma-separated list of month days (`1..=31` or `-31..=-1`).
fn parse_month_days(text: &str) -> Vec<i32> {
    let mut out = Vec::new();
    for part in text.split([',', ' ']) {
        let trimmed = part.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Ok(d) = trimmed.parse::<i32>()
            && ((-31..=-1).contains(&d) || (1..=31).contains(&d))
            && !out.contains(&d)
        {
            out.push(d);
        }
    }
    out
}

/// An RRULE from the custom repeat form's choices.
pub(super) fn custom_rule(spec: &CustomRuleSpec) -> String {
    let mut rule = format!("FREQ={}", spec.freq.key());
    if spec.interval > 1 {
        rule.push_str(&format!(";INTERVAL={}", spec.interval));
    }
    match spec.freq {
        Freq::Daily => {}
        Freq::Weekly => {
            let byday: Vec<&str> = weekday_names()
                .iter()
                .zip(&spec.days)
                .filter(|(_, on)| **on)
                .map(|((k, _), _)| *k)
                .collect();
            if !byday.is_empty() {
                rule.push_str(&format!(";BYDAY={}", byday.join(",")));
            }
        }
        Freq::Monthly => match spec.pattern {
            PatternMode::OnDayOfMonth => {
                if !spec.month_days.is_empty() {
                    let s: Vec<String> = spec.month_days.iter().map(i32::to_string).collect();
                    rule.push_str(&format!(";BYMONTHDAY={}", s.join(",")));
                }
            }
            PatternMode::OnNthWeekday => {
                let pos = spec.ordinal.pos();
                match spec.nth_target {
                    NthTarget::Day => rule.push_str(&format!(";BYMONTHDAY={pos}")),
                    NthTarget::Weekday => {
                        rule.push_str(&format!(";BYDAY=MO,TU,WE,TH,FR;BYSETPOS={pos}"))
                    }
                    NthTarget::Weekend => rule.push_str(&format!(";BYDAY=SA,SU;BYSETPOS={pos}")),
                    other => {
                        if let Some(code) = other.weekday_code() {
                            rule.push_str(&format!(";BYDAY={pos}{code}"));
                        }
                    }
                }
            }
        },
        Freq::Yearly => {
            let bymonth: Vec<String> = spec
                .months
                .iter()
                .enumerate()
                .filter(|(_, on)| **on)
                .map(|(i, _)| (i + 1).to_string())
                .collect();
            if !bymonth.is_empty() {
                rule.push_str(&format!(";BYMONTH={}", bymonth.join(",")));
            }
            match spec.pattern {
                PatternMode::OnDayOfMonth => {
                    if !spec.month_days.is_empty() {
                        let s: Vec<String> = spec.month_days.iter().map(i32::to_string).collect();
                        rule.push_str(&format!(";BYMONTHDAY={}", s.join(",")));
                    }
                }
                PatternMode::OnNthWeekday => {
                    let pos = spec.ordinal.pos();
                    match spec.nth_target {
                        NthTarget::Day => rule.push_str(&format!(";BYMONTHDAY={pos}")),
                        NthTarget::Weekday => {
                            rule.push_str(&format!(";BYDAY=MO,TU,WE,TH,FR;BYSETPOS={pos}"))
                        }
                        NthTarget::Weekend => {
                            rule.push_str(&format!(";BYDAY=SA,SU;BYSETPOS={pos}"))
                        }
                        other => {
                            if let Some(code) = other.weekday_code() {
                                rule.push_str(&format!(";BYDAY={pos}{code}"));
                            }
                        }
                    }
                }
            }
        }
    }
    match spec.ends {
        Ends::After if spec.count > 0 => rule.push_str(&format!(";COUNT={}", spec.count)),
        Ends::On => {
            if let Some(d) = spec.until {
                rule.push_str(&format!(";UNTIL={}T235959Z", d.format("%Y%m%d")));
            }
        }
        _ => {}
    }
    rule
}

/// Local midnight of `day`, in Unix seconds.
fn day_start(day: NaiveDate) -> i64 {
    Local
        .from_local_datetime(&day.and_hms_opt(0, 0, 0).unwrap_or_default())
        .earliest()
        .map(|t| t.timestamp())
        .unwrap_or(0)
}

/// Whether `e` falls on `day` (local time; all-day events by their date).
fn on_day(e: &crate::sync::ical::Event, day: NaiveDate) -> bool {
    if e.all_day {
        let start = chrono::DateTime::from_timestamp(e.start, 0).map(|t| t.date_naive());
        let end =
            chrono::DateTime::from_timestamp(e.end.max(e.start + 1) - 1, 0).map(|t| t.date_naive());
        return matches!((start, end), (Some(s), Some(t)) if s <= day && day <= t);
    }
    let (from, to) = (day_start(day), day_start(day + Days::days(1)));
    e.start < to && e.end.max(e.start + 1) > from
}

fn time_of(ts: i64) -> String {
    Local
        .timestamp_opt(ts, 0)
        .single()
        .map(|t| t.local(t!("dates-calendar-time")))
        .unwrap_or_default()
}

/// A timed event laid out on a single day's 24-hour column, with overlap
/// columns assigned so concurrent events sit cleanly side-by-side.
#[derive(Clone, Debug)]
struct PlacedEvent<'a> {
    event: &'a crate::sync::ical::Event,
    /// Visual top offset in pixels from midnight (`0.0..=24.0 * HOUR_HEIGHT`).
    top: f32,
    /// Visual height in pixels (at least `MIN_EVENT_HEIGHT`).
    height: f32,
    /// Zero-based horizontal slot inside its overlap cluster.
    col: usize,
    /// Number of horizontal slots in its overlap cluster (`>= 1`).
    cols: usize,
}

/// Computes non-overlapping vertical & horizontal geometry for all timed
/// events occurring on `day`.
fn layout_day_events<'a>(
    events: &[&'a crate::sync::ical::Event],
    day: NaiveDate,
) -> Vec<PlacedEvent<'a>> {
    let from = day_start(day);
    let to = day_start(day + Days::days(1));
    let span = (to - from).max(1) as f32;
    let day_px = 24. * HOUR_HEIGHT;

    let mut items: Vec<(&'a crate::sync::ical::Event, f32, f32)> = events
        .iter()
        .filter(|e| !e.all_day)
        .filter_map(|&e| {
            let s = e.start.max(from);
            let t = e.end.max(e.start + 1).min(to);
            if t <= from || s >= to {
                return None;
            }
            let top = ((s - from) as f32 / span) * day_px;
            let raw_h = ((t - s) as f32 / span) * day_px;
            let height = raw_h.max(MIN_EVENT_HEIGHT).min((day_px - top).max(MIN_EVENT_HEIGHT));
            Some((e, top, top + height))
        })
        .collect();

    // Earliest start first; longer events first on ties.
    items.sort_by(|a, b| {
        a.1.total_cmp(&b.1)
            .then_with(|| (b.2 - b.1).total_cmp(&(a.2 - a.1)))
    });

    // Partition into overlap clusters: each cluster shares a consistent column count.
    let mut out = Vec::with_capacity(items.len());
    let mut i = 0;
    while i < items.len() {
        let mut cluster_end = items[i].2;
        let mut j = i + 1;
        while j < items.len() && items[j].1 < cluster_end - 0.5 {
            cluster_end = cluster_end.max(items[j].2);
            j += 1;
        }
        // Greedy interval-graph coloring within items[i..j].
        let mut col_ends: Vec<f32> = Vec::new();
        let mut assigned: Vec<(usize, usize)> = Vec::with_capacity(j - i);
        for (idx, &(_, start, end)) in items[i..j].iter().enumerate() {
            let slot = col_ends
                .iter()
                .position(|&e| e <= start + 0.5)
                .unwrap_or_else(|| {
                    col_ends.push(0.);
                    col_ends.len() - 1
                });
            col_ends[slot] = end;
            assigned.push((i + idx, slot));
        }
        let cols = col_ends.len().max(1);
        for (item_idx, col) in assigned {
            let (event, top, bottom) = items[item_idx];
            out.push(PlacedEvent {
                event,
                top,
                height: (bottom - top).max(MIN_EVENT_HEIGHT),
                col,
                cols,
            });
        }
        i = j;
    }
    out
}

impl MailApp {
    /// Pre-fills the custom repeat editor from the event's current repeat rule
    /// (if any) and start date.
    fn populate_custom_repeat(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        use crate::sync::ical::Rule;
        let rule_str = match &self.event_repeat {
            calendar::Repeat::Never => String::new(),
            calendar::Repeat::Daily => "FREQ=DAILY".into(),
            calendar::Repeat::Weekly => "FREQ=WEEKLY".into(),
            calendar::Repeat::Monthly => "FREQ=MONTHLY".into(),
            calendar::Repeat::Yearly => "FREQ=YEARLY".into(),
            calendar::Repeat::Kept(r) => r.clone(),
        };
        // Reset to sensible defaults derived from the event's start date (or today).
        let start_date = calendar::parse_in(&Self::value(&self.event_start, cx), None)
            .and_then(|(ts, _)| Local.timestamp_opt(ts, 0).single())
            .map(|dt| dt.date_naive())
            .unwrap_or_else(|| Local::now().date_naive());
        self.cal.days = [false; 7];
        self.cal.months = [false; 12];
        self.cal.months[start_date.month0() as usize] = true;
        self.cal.pattern = PatternMode::OnDayOfMonth;
        self.cal.ordinal = Ordinal::from_pos(((start_date.day() - 1) / 7 + 1) as i32)
            .unwrap_or(Ordinal::Last);
        self.cal.nth_target = NthTarget::from_chrono(start_date.weekday());
        self.cal.ends = Ends::Never;
        Self::set(&self.cal.interval, "1", w, cx);
        Self::set(&self.cal.month_day, start_date.day().to_string(), w, cx);

        let Some(parsed) = Rule::parse(&rule_str) else {
            self.cal.days[start_date.weekday().num_days_from_monday() as usize] = true;
            return;
        };
        self.cal.freq = match parsed.freq.as_str() {
            "DAILY" => Freq::Daily,
            "WEEKLY" => Freq::Weekly,
            "MONTHLY" => Freq::Monthly,
            "YEARLY" => Freq::Yearly,
            _ => Freq::Weekly,
        };
        Self::set(&self.cal.interval, parsed.interval.to_string(), w, cx);

        if !parsed.by_month.is_empty() {
            self.cal.months = [false; 12];
            for m in &parsed.by_month {
                if (1..=12).contains(m) {
                    self.cal.months[(m - 1) as usize] = true;
                }
            }
        }
        if !parsed.by_month_day.is_empty() {
            self.cal.pattern = PatternMode::OnDayOfMonth;
            let text = parsed
                .by_month_day
                .iter()
                .map(i32::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            Self::set(&self.cal.month_day, &text, w, cx);
        }
        if !parsed.by_day.is_empty() {
            if self.cal.freq == Freq::Weekly {
                for (_, wd) in &parsed.by_day {
                    self.cal.days[wd.num_days_from_monday() as usize] = true;
                }
            } else {
                self.cal.pattern = PatternMode::OnNthWeekday;
                let weekdays_only = parsed.by_day.len() == 5
                    && parsed.by_day.iter().all(|(n, d)| {
                        *n == 0
                            && matches!(
                                d,
                                chrono::Weekday::Mon
                                    | chrono::Weekday::Tue
                                    | chrono::Weekday::Wed
                                    | chrono::Weekday::Thu
                                    | chrono::Weekday::Fri
                            )
                    });
                let weekend_only = parsed.by_day.len() == 2
                    && parsed
                        .by_day
                        .iter()
                        .all(|(n, d)| *n == 0 && matches!(d, chrono::Weekday::Sat | chrono::Weekday::Sun));
                if let Some(&pos) = parsed.by_set_pos.first() {
                    if let Some(ord) = Ordinal::from_pos(pos) {
                        self.cal.ordinal = ord;
                    }
                    if weekdays_only {
                        self.cal.nth_target = NthTarget::Weekday;
                    } else if weekend_only {
                        self.cal.nth_target = NthTarget::Weekend;
                    }
                } else if let Some(&(n, wd)) = parsed.by_day.first() {
                    if let Some(ord) = Ordinal::from_pos(n) {
                        self.cal.ordinal = ord;
                    }
                    self.cal.nth_target = NthTarget::from_chrono(wd);
                }
            }
        } else if self.cal.freq == Freq::Weekly {
            self.cal.days[start_date.weekday().num_days_from_monday() as usize] = true;
        }
        if let Some(c) = parsed.count {
            self.cal.ends = Ends::After;
            Self::set(&self.cal.count, c.to_string(), w, cx);
        } else if let Some(u) = parsed.until {
            self.cal.ends = Ends::On;
            let d = chrono::DateTime::from_timestamp(u, 0)
                .map(|t| t.format("%Y-%m-%d").to_string())
                .unwrap_or_default();
            Self::set(&self.cal.until, &d, w, cx);
        }
    }

    pub(super) fn cal_action(&mut self, action: CalAction, w: &mut Window, cx: &mut Context<Self>) {
        let step = |view: CalendarView, d: NaiveDate, by: i64| match view {
            CalendarView::Day | CalendarView::Agenda => d + Days::days(by),
            CalendarView::Week => d + Days::days(7 * by),
            CalendarView::Month => {
                let months = d.year() * 12 + d.month0() as i32 + by as i32;
                NaiveDate::from_ymd_opt(months.div_euclid(12), months.rem_euclid(12) as u32 + 1, 1)
                    .unwrap_or(d)
            }
        };
        match action {
            CalAction::Open(view) => {
                self.app_view = Some(view);
                self.leave_summary();
                self.subs_open = false;
                self.modal = None;
                if view == AppView::Calendar && self.backend.calendar_server().is_some() {
                    self.handle(Action::SyncCalendar, w, cx);
                }
                if view == AppView::Tasks {
                    self.sync_tasks_quietly(cx);
                }
                if view == AppView::Attachments {
                    self.load_library(cx);
                }
            }
            CalAction::Close => self.app_view = None,
            CalAction::View(v) => {
                if matches!(v, CalendarView::Day | CalendarView::Week) && !self.cal.grid_scrolled {
                    self.cal.grid_scrolled = true;
                    self.cal.grid_scroll.set_offset(point(px(0.), px(-7. * HOUR_HEIGHT)));
                }
                self.cal.view = v;
            }
            CalAction::Previous => self.cal.anchor = step(self.cal.view, self.cal.anchor, -1),
            CalAction::Next => self.cal.anchor = step(self.cal.view, self.cal.anchor, 1),
            CalAction::Today => self.cal.anchor = Local::now().date_naive(),
            CalAction::OpenDay(d) => {
                self.cal.anchor = d;
                if !self.cal.grid_scrolled {
                    self.cal.grid_scrolled = true;
                    self.cal.grid_scroll.set_offset(point(px(0.), px(-7. * HOUR_HEIGHT)));
                }
                self.cal.view = CalendarView::Day;
            }
            CalAction::Custom(open) => {
                if open && !self.cal.custom_open {
                    self.populate_custom_repeat(w, cx);
                }
                self.cal.custom_open = open;
            }
            CalAction::Freq(f) => self.cal.freq = f,
            CalAction::ToggleDay(i) => {
                if let Some(d) = self.cal.days.get_mut(i) {
                    *d = !*d;
                }
            }
            CalAction::ToggleMonth(i) => {
                if let Some(m) = self.cal.months.get_mut(i) {
                    *m = !*m;
                }
            }
            CalAction::Pattern(p) => self.cal.pattern = p,
            CalAction::Ordinal(o) => self.cal.ordinal = o,
            CalAction::NthTarget(t) => self.cal.nth_target = t,
            CalAction::Ends(e) => self.cal.ends = e,
            CalAction::ApplyCustom => {
                let interval = Self::value(&self.cal.interval, cx)
                    .trim()
                    .parse::<u32>()
                    .unwrap_or(1)
                    .clamp(1, 999);
                let count = Self::value(&self.cal.count, cx)
                    .trim()
                    .parse::<u32>()
                    .unwrap_or(0);
                let until = NaiveDate::parse_from_str(
                    Self::value(&self.cal.until, cx).trim(),
                    "%Y-%m-%d",
                )
                .ok();
                if self.cal.ends == Ends::After && count == 0 {
                    self.notice(t!("calendar-views-say-how-many-times-it-repeats"), true, cx);
                    return;
                }
                if self.cal.ends == Ends::On && until.is_none() {
                    self.notice(t!("calendar-views-type-the-last-date-as-2027"), true, cx);
                    return;
                }
                let md_raw = Self::value(&self.cal.month_day, cx);
                let month_days = parse_month_days(&md_raw);
                if matches!(self.cal.freq, Freq::Monthly | Freq::Yearly)
                    && self.cal.pattern == PatternMode::OnDayOfMonth
                    && !md_raw.trim().is_empty()
                    && month_days.is_empty()
                {
                    self.notice(t!("calendar-views-type-day-numbers-from-1-to"), true, cx);
                    return;
                }
                let spec = CustomRuleSpec {
                    freq: self.cal.freq,
                    interval,
                    days: self.cal.days,
                    months: self.cal.months,
                    pattern: self.cal.pattern,
                    month_days,
                    ordinal: self.cal.ordinal,
                    nth_target: self.cal.nth_target,
                    ends: self.cal.ends,
                    count,
                    until,
                };
                let rule = custom_rule(&spec);
                self.handle(Action::EventRepeat(calendar::Repeat::of(&rule)), w, cx);
                self.cal.custom_open = false;
            }
        }
        cx.notify();
    }

    /// The custom repeat form, under the event editor's Repeats choices.
    pub(super) fn custom_repeat_form(&self, cx: &Context<Self>) -> AnyElement {
        let c = self.colors();
        let act = |a: CalAction| Action::Calendar(a);
        let small = |t: &str| {
            div()
                .text_size(px(12.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(rgb(c.muted))
                .child(t.to_owned())
        };
        if !self.cal.custom_open {
            return div()
                .child(self.link(
                    "custom-repeat",
                    t!("calendar-views-custom-repeat"),
                    act(CalAction::Custom(true)),
                    cx,
                ))
                .into_any_element();
        }
        let pill = |id: String, label: &'static str, on: bool, action: CalAction| {
            div()
                .id(SharedString::from(id))
                .px_2()
                .h(px(28.))
                .flex()
                .items_center()
                .rounded(px(7.))
                .cursor_pointer()
                .text_size(px(12.))
                .border_1()
                .border_color(rgb(if on { c.accent } else { c.border }))
                .bg(rgb(if on { c.accent } else { c.surface }))
                .text_color(rgb(if on { c.on_accent } else { c.text }))
                .child(label)
                .on_click(cx.listener(move |this, _, w, cx| {
                    this.handle(Action::Calendar(action.clone()), w, cx)
                }))
        };
        let freqs = self.segmented(
            "custom-freq",
            [Freq::Daily, Freq::Weekly, Freq::Monthly, Freq::Yearly]
                .into_iter()
                .map(|f| {
                    let label = match f {
                        Freq::Daily => t!("calendar-views-daily"),
                        Freq::Weekly => t!("calendar-views-weekly"),
                        Freq::Monthly => t!("calendar-views-monthly"),
                        Freq::Yearly => t!("calendar-views-yearly"),
                    };
                    (label.to_owned(), act(CalAction::Freq(f)), self.cal.freq == f)
                })
                .collect(),
            cx,
        );
        let mut days = div().flex().flex_wrap().gap_1();
        for (i, (_, label)) in weekday_names().into_iter().enumerate() {
            days = days.child(pill(
                format!("custom-day-{i}"),
                label,
                self.cal.days[i],
                CalAction::ToggleDay(i),
            ));
        }
        let mut months = div().flex().flex_wrap().gap_1();
        for (i, label) in month_names().into_iter().enumerate() {
            months = months.child(pill(
                format!("custom-month-{i}"),
                label,
                self.cal.months[i],
                CalAction::ToggleMonth(i),
            ));
        }
        let pattern = self.segmented(
            "custom-pattern",
            vec![
                (
                    t!("calendar-views-on-day-of-month").into(),
                    act(CalAction::Pattern(PatternMode::OnDayOfMonth)),
                    self.cal.pattern == PatternMode::OnDayOfMonth,
                ),
                (
                    t!("calendar-views-on-the-nth-weekday").into(),
                    act(CalAction::Pattern(PatternMode::OnNthWeekday)),
                    self.cal.pattern == PatternMode::OnNthWeekday,
                ),
            ],
            cx,
        );
        let mut ordinals = div().flex().flex_wrap().gap_1();
        for ord in Ordinal::ALL {
            ordinals = ordinals.child(pill(
                format!("custom-ord-{}", ord.pos()),
                ord.label(),
                self.cal.ordinal == ord,
                CalAction::Ordinal(ord),
            ));
        }
        let mut targets = div().flex().flex_wrap().gap_1();
        for (i, target) in NthTarget::ALL.into_iter().enumerate() {
            targets = targets.child(pill(
                format!("custom-target-{i}"),
                target.label(),
                self.cal.nth_target == target,
                CalAction::NthTarget(target),
            ));
        }
        let ends = self.segmented(
            "custom-ends",
            vec![
                (
                    t!("calendar-views-never").into(),
                    act(CalAction::Ends(Ends::Never)),
                    self.cal.ends == Ends::Never,
                ),
                (
                    t!("calendar-views-after-a-number-of-times").into(),
                    act(CalAction::Ends(Ends::After)),
                    self.cal.ends == Ends::After,
                ),
                (
                    t!("calendar-views-on-a-date").into(),
                    act(CalAction::Ends(Ends::On)),
                    self.cal.ends == Ends::On,
                ),
            ],
            cx,
        );
        let monthly_or_yearly = matches!(self.cal.freq, Freq::Monthly | Freq::Yearly);
        div()
            .id("custom-repeat-form")
            .p_3()
            .rounded_lg()
            .border_1()
            .border_color(rgb(c.border))
            .flex()
            .flex_col()
            .gap_2()
            .child(small(t!("calendar-views-repeats")))
            .child(freqs)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(small(t!("calendar-views-every")))
                    .child(div().w(px(70.)).child(field(Input::new(&self.cal.interval), c, false)))
                    .child(small(self.cal.freq.unit())),
            )
            .when(self.cal.freq == Freq::Weekly, |d| {
                d.child(small(t!("calendar-views-on"))).child(days)
            })
            .when(self.cal.freq == Freq::Yearly, |d| {
                d.child(small(t!("calendar-views-in"))).child(months)
            })
            .when(monthly_or_yearly, |d| {
                d.child(pattern)
                    .when(self.cal.pattern == PatternMode::OnDayOfMonth, |d| {
                        d.child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(small(t!("calendar-views-day-s")))
                                .child(
                                    div()
                                        .w(px(190.))
                                        .child(field(Input::new(&self.cal.month_day), c, false)),
                                ),
                        )
                    })
                    .when(self.cal.pattern == PatternMode::OnNthWeekday, |d| {
                        d.child(ordinals).child(targets)
                    })
            })
            .child(small(t!("calendar-views-ends")))
            .child(ends)
            .when(self.cal.ends == Ends::After, |d| {
                d.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(div().w(px(70.)).child(field(Input::new(&self.cal.count), c, false)))
                        .child(small("times")),
                )
            })
            .when(self.cal.ends == Ends::On, |d| {
                d.child(div().w(px(160.)).child(field(Input::new(&self.cal.until), c, false)))
            })
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(self.button(
                        "custom-apply",
                        t!("calendar-views-use-this-repeat"),
                        "check",
                        act(CalAction::ApplyCustom),
                        true,
                        cx,
                    ))
                    .child(self.button(
                        "custom-cancel",
                        t!("calendar-views-cancel"),
                        "",
                        act(CalAction::Custom(false)),
                        false,
                        cx,
                    )),
            )
            .into_any_element()
    }

    /// The view switcher and, outside the agenda, the grid it shows.
    pub(super) fn calendar_grid(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let c = self.colors();
        let act = |a: CalAction| Action::Calendar(a);
        let view = self.cal.view;
        let switcher = self
            .segmented(
                "calendar-view",
                [
                    (t!("calendar-views-agenda"), CalendarView::Agenda),
                    (t!("calendar-views-day"), CalendarView::Day),
                    (t!("calendar-views-week"), CalendarView::Week),
                    (t!("calendar-views-month"), CalendarView::Month),
                ]
                .into_iter()
                .map(|(label, v)| (label.to_owned(), act(CalAction::View(v)), view == v))
                .collect(),
                cx,
            )
            .max_w(px(360.));
        if view == CalendarView::Agenda {
            return Some(switcher.into_any_element());
        }
        let anchor = self.cal.anchor;
        let (from, days): (NaiveDate, usize) = match view {
            CalendarView::Day | CalendarView::Agenda => (anchor, 1),
            CalendarView::Week => (
                anchor - Days::days(anchor.weekday().num_days_from_monday() as i64),
                7,
            ),
            CalendarView::Month => {
                let first = anchor.with_day(1).unwrap_or(anchor);
                (
                    first - Days::days(first.weekday().num_days_from_monday() as i64),
                    42,
                )
            }
        };
        let events = self.workspace_agenda(
            day_start(from) - 86_400,
            day_start(from + Days::days(days as i64)) + 86_400,
        );
        let title = match view {
            CalendarView::Month => anchor.local(t!("dates-month-year")),
            CalendarView::Week => format!(
                "{} – {}",
                from.local(t!("dates-month-day")),
                (from + Days::days(6)).local(t!("dates-month-day-year"))
            ),
            _ => anchor.local(t!("dates-long-date")),
        };
        let now = Local::now();
        let today = now.date_naive();
        let now_y = (now.hour() as f32 + now.minute() as f32 / 60.) * HOUR_HEIGHT;
        let header = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_2()
            .child(switcher)
            .child(div().flex_1())
            .child(self.button(
                "calendar-prev",
                "@Previous",
                "chevron-left",
                act(CalAction::Previous),
                false,
                cx,
            ))
            .child(self.button(
                "calendar-today",
                t!("calendar-views-today"),
                "",
                act(CalAction::Today),
                false,
                cx,
            ))
            .child(self.button(
                "calendar-next",
                "@Next",
                "chevron-right",
                act(CalAction::Next),
                false,
                cx,
            ))
            .child(
                div()
                    .text_size(px(15.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(title),
            );
        let chip = |e: &crate::sync::ical::Event, i: String| {
            let uid = e.uid.clone();
            let occurrence = e.recurrence_id;
            let label = if e.all_day {
                e.summary.clone()
            } else {
                format!("{} {}", time_of(e.start), e.summary)
            };
            div()
                .id(SharedString::from(i))
                .px_1()
                .rounded(px(4.))
                .bg(rgb(c.soft))
                .text_size(px(11.))
                .truncate()
                .cursor_pointer()
                .hover(move |s| s.bg(rgb(c.border)))
                .child(label)
                .on_click(cx.listener(move |this, _, w, cx| {
                    this.handle(Action::EditEvent(uid.clone(), occurrence), w, cx)
                }))
        };
        let grid = match view {
            CalendarView::Month => {
                let mut grid = div()
                    .flex()
                    .flex_col()
                    .border_1()
                    .border_color(rgb(c.border))
                    .rounded_lg()
                    .overflow_hidden();
                let mut names = div().flex().bg(rgb(c.bg));
                for (_, n) in weekday_names() {
                    names = names.child(
                        div()
                            .flex_1()
                            .px_2()
                            .py_1()
                            .text_size(px(11.))
                            .text_color(rgb(c.muted))
                            .child(n),
                    );
                }
                grid = grid.child(names);
                for week in 0..6 {
                    let mut row = div().flex().border_t_1().border_color(rgb(c.border));
                    for wd in 0..7 {
                        let day = from + Days::days((week * 7 + wd) as i64);
                        let inside = day.month() == anchor.month();
                        let todays: Vec<&crate::sync::ical::Event> =
                            events.iter().filter(|e| on_day(e, day)).collect();
                        let more = todays.len().saturating_sub(3);
                        let mut cell = div()
                            .id(SharedString::from(format!("month-{day}")))
                            .flex_1()
                            .min_w_0()
                            .h(px(96.))
                            .p_1()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .when(wd > 0, |d| d.border_l_1())
                            .border_color(rgb(c.border))
                            .when(!inside, |d| d.bg(rgb(c.bg)))
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, e: &ClickEvent, w, cx| {
                                if e.click_count() >= 1 {
                                    this.handle(Action::Calendar(CalAction::OpenDay(day)), w, cx)
                                }
                            }))
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .font_weight(if day == today {
                                        FontWeight::BOLD
                                    } else {
                                        FontWeight::NORMAL
                                    })
                                    .text_color(rgb(if day == today {
                                        c.accent
                                    } else if inside {
                                        c.text
                                    } else {
                                        c.muted
                                    }))
                                    .child(day.day().to_string()),
                            );
                        for (k, e) in todays.iter().take(3).enumerate() {
                            cell = cell.child(chip(e, format!("month-event-{day}-{k}")));
                        }
                        if more > 0 {
                            cell = cell.child(
                                div()
                                    .text_size(px(10.))
                                    .text_color(rgb(c.muted))
                                    .child(t!("calendar-views-more", more = more)),
                            );
                        }
                        row = row.child(cell);
                    }
                    grid = grid.child(row);
                }
                grid.into_any_element()
            }
            _ => {
                // Day and Week: hour-by-hour time grid (00:00–23:00), with an
                // all-day strip above and overlapping events placed side-by-side.
                let day_list: Vec<NaiveDate> = (0..days)
                    .map(|i| from + Days::days(i as i64))
                    .collect();
                let mut day_headers = div()
                    .flex()
                    .bg(rgb(c.bg))
                    .border_b_1()
                    .border_color(rgb(c.border))
                    .child(div().w(px(GUTTER_WIDTH)).flex_shrink_0());
                let mut has_all_day = false;
                let mut all_day_by_day: Vec<Vec<&crate::sync::ical::Event>> =
                    Vec::with_capacity(days);
                for &day in &day_list {
                    let all_day: Vec<&crate::sync::ical::Event> = events
                        .iter()
                        .filter(|e| e.all_day && on_day(e, day))
                        .collect();
                    has_all_day |= !all_day.is_empty();
                    all_day_by_day.push(all_day);
                    day_headers = day_headers.child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .px_2()
                            .py(px(6.))
                            .border_l_1()
                            .border_color(rgb(c.border))
                            .text_size(px(12.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(if day == today { c.accent } else { c.text }))
                            .truncate()
                            .child(
                                day.format(if days == 1 { "%A, %b %-d" } else { "%a %-d" })
                                    .to_string(),
                            ),
                    );
                }

                let all_day_row = has_all_day.then(|| {
                    let mut row = div()
                        .flex()
                        .border_b_1()
                        .border_color(rgb(c.border))
                        .bg(rgb(c.surface))
                        .child(
                            div()
                                .w(px(GUTTER_WIDTH))
                                .flex_shrink_0()
                                .px_1()
                                .py_1()
                                .text_size(px(10.))
                                .text_color(rgb(c.muted))
                                .child(t!("calendar-views-all-day")),
                        );
                    for (i, all_day) in all_day_by_day.iter().enumerate() {
                        let day = day_list[i];
                        let mut cell = div()
                            .flex_1()
                            .min_w_0()
                            .p_1()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .border_l_1()
                            .border_color(rgb(c.border));
                        for (k, e) in all_day.iter().enumerate() {
                            cell = cell.child(chip(e, format!("allday-{day}-{k}")));
                        }
                        row = row.child(cell);
                    }
                    row
                });

                let total_h = 24. * HOUR_HEIGHT;
                let mut gutter = div()
                    .w(px(GUTTER_WIDTH))
                    .flex_shrink_0()
                    .h(px(total_h))
                    .relative();
                for hour in 0..24u32 {
                    gutter = gutter.child(
                        div()
                            .absolute()
                            .top(px(hour as f32 * HOUR_HEIGHT + 2.))
                            .left_0()
                            .right(px(6.))
                            .text_size(px(10.))
                            .text_color(rgb(c.muted))
                            .flex()
                            .justify_end()
                            .child(format!("{hour:02}:00")),
                    );
                }

                let mut columns = div()
                    .flex()
                    .h(px(total_h))
                    .child(gutter);

                for &day in &day_list {
                    let todays: Vec<&crate::sync::ical::Event> =
                        events.iter().filter(|e| !e.all_day && on_day(e, day)).collect();
                    let placed = layout_day_events(&todays, day);
                    let mut col = div()
                        .id(SharedString::from(format!("time-col-{day}")))
                        .flex_1()
                        .min_w_0()
                        .h(px(total_h))
                        .relative()
                        .border_l_1()
                        .border_color(rgb(c.border));

                    // Hour gridlines (0..24) across the column.
                    for hour in 0..24u32 {
                        col = col.child(
                            div()
                                .absolute()
                                .top(px(hour as f32 * HOUR_HEIGHT))
                                .left_0()
                                .right_0()
                                .h(px(HOUR_HEIGHT))
                                .when(hour > 0, |d| {
                                    d.border_t_1().border_color(rgb(c.border))
                                }),
                        );
                    }

                    // Positioned timed events, placed side-by-side when overlapping.
                    for (k, p) in placed.iter().enumerate() {
                        let e = p.event;
                        let uid = e.uid.clone();
                        let occurrence = e.recurrence_id;
                        let when = format!("{} – {}", time_of(e.start), time_of(e.end));
                        let left_frac = p.col as f32 / p.cols as f32;
                        let width_frac = 1. / p.cols as f32;
                        let tall = p.height >= 36.;
                        col = col.child(
                            div()
                                .id(SharedString::from(format!("grid-event-{day}-{k}")))
                                .absolute()
                                .top(px(p.top + 1.))
                                .h(px((p.height - 2.).max(18.)))
                                .left(relative(left_frac))
                                .w(relative(width_frac))
                                .px(px(2.))
                                .child(
                                    div()
                                        .size_full()
                                        .px(px(6.))
                                        .py(px(2.))
                                        .rounded(px(5.))
                                        .bg(rgb(c.soft))
                                        .border_l_2()
                                        .border_color(rgb(c.accent))
                                        .overflow_hidden()
                                        .cursor_pointer()
                                        .hover(move |s| s.bg(rgb(c.border)))
                                        .child(
                                            div()
                                                .text_size(px(11.))
                                                .font_weight(FontWeight::MEDIUM)
                                                .truncate()
                                                .child(e.summary.clone()),
                                        )
                                        .when(tall, |d| {
                                            d.child(
                                                div()
                                                    .text_size(px(10.))
                                                    .text_color(rgb(c.muted))
                                                    .truncate()
                                                    .child(if e.location.is_empty() {
                                                        when
                                                    } else {
                                                        format!("{when} · {}", e.location)
                                                    }),
                                            )
                                        }),
                                )
                                .on_click(cx.listener(move |this, _, w, cx| {
                                    this.handle(Action::EditEvent(uid.clone(), occurrence), w, cx)
                                })),
                        );
                    }

                    // Current time indicator on today's column.
                    if day == today {
                        col = col.child(
                            div()
                                .absolute()
                                .top(px(now_y))
                                .left_0()
                                .right_0()
                                .h(px(2.))
                                .bg(rgb(c.accent))
                                .child(
                                    div()
                                        .absolute()
                                        .top(px(-3.))
                                        .left(px(-4.))
                                        .size(px(8.))
                                        .rounded_full()
                                        .bg(rgb(c.accent)),
                                ),
                        );
                    }

                    columns = columns.child(col);
                }

                div()
                    .id("calendar-time-grid")
                    .debug_selector(|| "calendar-time-grid".into())
                    .flex()
                    .flex_col()
                    .border_1()
                    .border_color(rgb(c.border))
                    .rounded_lg()
                    .overflow_hidden()
                    .child(day_headers)
                    .children(all_day_row)
                    .child(
                        div()
                            .id("calendar-time-grid-scroll")
                            .track_scroll(&self.cal.grid_scroll)
                            .h(px(GRID_VIEWPORT_HEIGHT))
                            .overflow_y_scroll()
                            .child(columns),
                    )
                    .into_any_element()
            }
        };
        Some(
            div()
                .flex()
                .flex_col()
                .gap_3()
                .child(header)
                .child(grid)
                .into_any_element(),
        )
    }

    /// Calendar or Contacts in place of the list and reader.
    pub(super) fn app_view_page(
        &self,
        view: AppView,
        corners: Corners<Pixels>,
        cx: &Context<Self>,
    ) -> AnyElement {
        let c = self.colors();
        let (title, page) = match view {
            AppView::Calendar => (t!("calendar-views-calendar"), self.calendar_page(cx)),
            AppView::Contacts => (t!("calendar-views-contacts"), self.contacts_page(cx)),
            AppView::Tasks => (t!("calendar-views-tasks"), self.tasks_page(cx)),
            // A list and a preview of its own, which scroll themselves.
            AppView::Attachments => return self.attachments_page(corners, false, cx),
        };
        div()
            .id("app-view")
            .debug_selector(|| "app-view".into())
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .bg(rgb(c.surface))
            .rounded_bl(corners.bottom_left)
            .rounded_br(corners.bottom_right)
            .child(
                div()
                    .h(px(57.))
                    .flex_shrink_0()
                    .px_4()
                    .flex()
                    .items_center()
                    .gap_2()
                    .border_b_1()
                    .border_color(rgb(c.border))
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(16.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(title),
                    )
                    .child(self.button(
                        "app-view-close",
                        t!("calendar-views-back-to-mail"),
                        "close",
                        Action::Calendar(CalAction::Close),
                        false,
                        cx,
                    )),
            )
            .child(
                div()
                    .id("app-view-scroll")
                    .track_scroll(&self.cal.scroll)
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p_6()
                    .child(div().max_w(px(1100.)).child(page)),
            )
            .into_any_element()
    }

    /// The sidebar's Calendar, Tasks, Contacts and Attachments entries
    /// (Tasks with how many are due today or overdue).
    pub(super) fn app_view_links(&self, collapsed: bool, cx: &Context<Self>) -> AnyElement {
        let c = self.colors();
        let due_now = self.tasks_due_now();
        let item = |id: &'static str, label: &'static str, icon: &'static str, view: AppView| {
            let count = (view == AppView::Tasks && due_now > 0 && !collapsed).then_some(due_now);
            let active = self.app_view == Some(view);
            div()
                .id(id)
                .debug_selector(move || id.into())
                .h(px(30.))
                .px_2()
                .rounded(px(7.))
                .flex()
                .items_center()
                .gap_2()
                .when(collapsed, |d| d.justify_center().w(px(36.)))
                .cursor_pointer()
                .text_size(px(13.))
                .when(active, |d| d.bg(rgb(c.soft)))
                .hover(move |s| s.bg(rgb(c.surface)))
                .text_color(rgb(if active { c.text } else { c.muted }))
                .when(!collapsed, |d| d.child(div().size(px(12.))))
                .child(icon_el(icon, 15., if active { c.accent } else { c.muted }))
                .when(!collapsed, |d| d.child(div().flex_1().child(label)))
                .when_some(count, |d, n| d.child(div().text_size(px(11.)).text_color(rgb(c.muted)).child(n.to_string())))
                .tooltip(move |w, cx| {
                    let tip = if view == AppView::Attachments {
                        keys(&shortcuts::tip(label, "attachments", cx))
                    } else {
                        label.to_owned()
                    };
                    Tooltip::new(tip).build(w, cx)
                })
                .on_click(cx.listener(move |this, _, w, cx| {
                    let action = if this.app_view == Some(view) {
                        CalAction::Close
                    } else {
                        CalAction::Open(view)
                    };
                    this.handle(Action::Calendar(action), w, cx)
                }))
        };
        div()
            .flex()
            .flex_col()
            .gap(px(1.))
            .mt_2()
            .when(collapsed, |d| d.items_center())
            .child(item("open-calendar", t!("calendar-views-calendar"), "clock", AppView::Calendar))
            .child(item("open-tasks", t!("calendar-views-tasks"), "tasks", AppView::Tasks))
            .child(item("open-contacts", t!("calendar-views-contacts"), "contact", AppView::Contacts))
            .child(item("open-attachments", t!("attachments-attachments"), "paperclip", AppView::Attachments))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::prelude::v1::test;

    #[test]
    fn custom_rules_cover_daily_weekly_monthly_and_yearly() {
        let mut days = [false; 7];
        days[0] = true;
        days[2] = true;
        let until = NaiveDate::from_ymd_opt(2027, 6, 30);

        let weekly = custom_rule(&CustomRuleSpec {
            freq: Freq::Weekly,
            interval: 2,
            days,
            ends: Ends::After,
            count: 10,
            ..Default::default()
        });
        assert_eq!(weekly, "FREQ=WEEKLY;INTERVAL=2;BYDAY=MO,WE;COUNT=10");
        assert!(crate::sync::ical::Rule::expandable(&weekly));

        let monthly_day = custom_rule(&CustomRuleSpec {
            freq: Freq::Monthly,
            interval: 1,
            pattern: PatternMode::OnDayOfMonth,
            month_days: vec![1, 15],
            ends: Ends::On,
            until,
            ..Default::default()
        });
        assert_eq!(
            monthly_day,
            "FREQ=MONTHLY;BYMONTHDAY=1,15;UNTIL=20270630T235959Z"
        );
        assert!(crate::sync::ical::Rule::expandable(&monthly_day));

        let monthly_nth = custom_rule(&CustomRuleSpec {
            freq: Freq::Monthly,
            interval: 1,
            pattern: PatternMode::OnNthWeekday,
            ordinal: Ordinal::Last,
            nth_target: NthTarget::Fri,
            ..Default::default()
        });
        assert_eq!(monthly_nth, "FREQ=MONTHLY;BYDAY=-1FR");
        assert!(crate::sync::ical::Rule::expandable(&monthly_nth));
        assert_eq!(
            crate::sync::ical::Rule::parse(&monthly_nth).unwrap().describe(),
            "Every month on the last Friday"
        );

        let monthly_last_weekday = custom_rule(&CustomRuleSpec {
            freq: Freq::Monthly,
            interval: 1,
            pattern: PatternMode::OnNthWeekday,
            ordinal: Ordinal::Last,
            nth_target: NthTarget::Weekday,
            ..Default::default()
        });
        assert_eq!(monthly_last_weekday, "FREQ=MONTHLY;BYDAY=MO,TU,WE,TH,FR;BYSETPOS=-1");
        assert!(crate::sync::ical::Rule::expandable(&monthly_last_weekday));
        assert_eq!(
            crate::sync::ical::Rule::parse(&monthly_last_weekday).unwrap().describe(),
            "Every month on the last weekday"
        );

        let mut nov = [false; 12];
        nov[10] = true;
        let yearly_nth = custom_rule(&CustomRuleSpec {
            freq: Freq::Yearly,
            interval: 1,
            months: nov,
            pattern: PatternMode::OnNthWeekday,
            ordinal: Ordinal::Fourth,
            nth_target: NthTarget::Thu,
            ..Default::default()
        });
        assert_eq!(yearly_nth, "FREQ=YEARLY;BYMONTH=11;BYDAY=4TH");
        assert!(crate::sync::ical::Rule::expandable(&yearly_nth));
        assert_eq!(
            crate::sync::ical::Rule::parse(&yearly_nth).unwrap().describe(),
            "Every year in November on the fourth Thursday"
        );
    }

    #[test]
    fn time_grid_places_overlapping_events_side_by_side() {
        let day = NaiveDate::from_ymd_opt(2026, 11, 24).unwrap();
        let base = day_start(day);
        let mk = |uid: &str, h0: i64, h1: i64| crate::sync::ical::Event {
            uid: uid.into(),
            summary: uid.into(),
            start: base + h0 * 3600,
            end: base + h1 * 3600,
            ..Default::default()
        };
        let a = mk("a", 9, 11);
        let b = mk("b", 10, 12);
        let c = mk("c", 14, 15);
        let placed = layout_day_events(&[&a, &b, &c], day);
        assert_eq!(placed.len(), 3);
        // `a` and `b` overlap -> 2 columns, slots 0 and 1.
        assert_eq!(placed[0].cols, 2);
        assert_eq!(placed[1].cols, 2);
        assert_ne!(placed[0].col, placed[1].col);
        assert!((placed[0].top - 9. * HOUR_HEIGHT).abs() < 1.);
        assert!((placed[0].height - 2. * HOUR_HEIGHT).abs() < 1.);
        // `c` is alone -> full width (1 column).
        assert_eq!((placed[2].col, placed[2].cols), (0, 1));
        assert!((placed[2].top - 14. * HOUR_HEIGHT).abs() < 1.);
    }
}

//! Calendar and Contacts as views of their own beside the mail (from the
//! sidebar), the calendar's Day, Week and Month grids beside its agenda,
//! and a custom repeat rule for events (every N days, weeks, months or
//! years; on chosen weekdays; until a date or for a number of times).
use super::*;
use chrono::{Datelike, Duration as Days, Local, NaiveDate, TimeZone};

/// A view in place of the message list and reader.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AppView {
    Calendar,
    Contacts,
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

/// How a custom repeat ends.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Ends {
    #[default]
    Never,
    After,
    On,
}

const WEEKDAYS: [(&str, &str); 7] = [
    ("MO", "Mon"),
    ("TU", "Tue"),
    ("WE", "Wed"),
    ("TH", "Thu"),
    ("FR", "Fri"),
    ("SA", "Sat"),
    ("SU", "Sun"),
];

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
    Ends(Ends),
    ApplyCustom,
}

pub(super) struct CalViews {
    pub(super) view: CalendarView,
    /// The day the Day, Week and Month views show, or show around.
    pub(super) anchor: NaiveDate,
    pub(super) custom_open: bool,
    pub(super) freq: Freq,
    pub(super) interval: Entity<InputState>,
    pub(super) days: [bool; 7],
    pub(super) ends: Ends,
    pub(super) count: Entity<InputState>,
    pub(super) until: Entity<InputState>,
    pub(super) scroll: ScrollHandle,
}
impl CalViews {
    pub(super) fn new(window: &mut Window, cx: &mut App) -> Self {
        Self {
            view: CalendarView::default(),
            anchor: Local::now().date_naive(),
            custom_open: false,
            freq: Freq::default(),
            interval: input("1", false, window, cx),
            days: [false; 7],
            ends: Ends::default(),
            count: input("10", false, window, cx),
            until: input("2027-06-30", false, window, cx),
            scroll: ScrollHandle::new(),
        }
    }
}

/// An RRULE from the custom repeat form's choices.
pub(super) fn custom_rule(
    freq: Freq,
    interval: u32,
    days: &[bool; 7],
    ends: Ends,
    count: u32,
    until: Option<NaiveDate>,
) -> String {
    let mut rule = format!("FREQ={}", freq.key());
    if interval > 1 {
        rule.push_str(&format!(";INTERVAL={interval}"));
    }
    let byday: Vec<&str> = WEEKDAYS
        .iter()
        .zip(days)
        .filter(|(_, on)| **on)
        .map(|((k, _), _)| *k)
        .collect();
    if freq == Freq::Weekly && !byday.is_empty() {
        rule.push_str(&format!(";BYDAY={}", byday.join(",")));
    }
    match ends {
        Ends::After if count > 0 => rule.push_str(&format!(";COUNT={count}")),
        Ends::On => {
            if let Some(d) = until {
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
        let end = chrono::DateTime::from_timestamp(e.end.max(e.start + 1) - 1, 0).map(|t| t.date_naive());
        return matches!((start, end), (Some(s), Some(t)) if s <= day && day <= t);
    }
    let (from, to) = (day_start(day), day_start(day + Days::days(1)));
    e.start < to && e.end.max(e.start + 1) > from
}

fn time_of(ts: i64) -> String {
    Local
        .timestamp_opt(ts, 0)
        .single()
        .map(|t| t.format("%H:%M").to_string())
        .unwrap_or_default()
}

impl MailApp {
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
            }
            CalAction::Close => self.app_view = None,
            CalAction::View(v) => self.cal.view = v,
            CalAction::Previous => self.cal.anchor = step(self.cal.view, self.cal.anchor, -1),
            CalAction::Next => self.cal.anchor = step(self.cal.view, self.cal.anchor, 1),
            CalAction::Today => self.cal.anchor = Local::now().date_naive(),
            CalAction::OpenDay(d) => {
                self.cal.anchor = d;
                self.cal.view = CalendarView::Day;
            }
            CalAction::Custom(open) => self.cal.custom_open = open,
            CalAction::Freq(f) => self.cal.freq = f,
            CalAction::ToggleDay(i) => {
                if let Some(d) = self.cal.days.get_mut(i) {
                    *d = !*d;
                }
            }
            CalAction::Ends(e) => self.cal.ends = e,
            CalAction::ApplyCustom => {
                let interval = Self::value(&self.cal.interval, cx).trim().parse::<u32>().unwrap_or(1).clamp(1, 999);
                let count = Self::value(&self.cal.count, cx).trim().parse::<u32>().unwrap_or(0);
                let until = NaiveDate::parse_from_str(Self::value(&self.cal.until, cx).trim(), "%Y-%m-%d").ok();
                if self.cal.ends == Ends::After && count == 0 {
                    self.notice("Say how many times it repeats", true, cx);
                    return;
                }
                if self.cal.ends == Ends::On && until.is_none() {
                    self.notice("Type the last date as 2027-06-30", true, cx);
                    return;
                }
                let rule = custom_rule(self.cal.freq, interval, &self.cal.days, self.cal.ends, count, until);
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
                    "Custom repeat…",
                    act(CalAction::Custom(true)),
                    cx,
                ))
                .into_any_element();
        }
        let freqs = self.segmented(
            "custom-freq",
            [Freq::Daily, Freq::Weekly, Freq::Monthly, Freq::Yearly]
                .into_iter()
                .map(|f| {
                    let label = match f {
                        Freq::Daily => "Daily",
                        Freq::Weekly => "Weekly",
                        Freq::Monthly => "Monthly",
                        Freq::Yearly => "Yearly",
                    };
                    (label.to_owned(), act(CalAction::Freq(f)), self.cal.freq == f)
                })
                .collect(),
            cx,
        );
        let mut days = div().flex().flex_wrap().gap_1();
        for (i, (_, label)) in WEEKDAYS.iter().enumerate() {
            let on = self.cal.days[i];
            days = days.child(
                div()
                    .id(SharedString::from(format!("custom-day-{i}")))
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
                    .child(*label)
                    .on_click(cx.listener(move |this, _, w, cx| {
                        this.handle(Action::Calendar(CalAction::ToggleDay(i)), w, cx)
                    })),
            );
        }
        let ends = self.segmented(
            "custom-ends",
            vec![
                ("Never".into(), act(CalAction::Ends(Ends::Never)), self.cal.ends == Ends::Never),
                ("After a number of times".into(), act(CalAction::Ends(Ends::After)), self.cal.ends == Ends::After),
                ("On a date".into(), act(CalAction::Ends(Ends::On)), self.cal.ends == Ends::On),
            ],
            cx,
        );
        div()
            .id("custom-repeat-form")
            .p_3()
            .rounded_lg()
            .border_1()
            .border_color(rgb(c.border))
            .flex()
            .flex_col()
            .gap_2()
            .child(small("Repeats"))
            .child(freqs)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(small("Every"))
                    .child(div().w(px(70.)).child(field(Input::new(&self.cal.interval), c, false)))
                    .child(small(self.cal.freq.unit())),
            )
            .when(self.cal.freq == Freq::Weekly, |d| d.child(small("On")).child(days))
            .child(small("Ends"))
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
                    .child(self.button("custom-apply", "Use This Repeat", "check", act(CalAction::ApplyCustom), true, cx))
                    .child(self.button("custom-cancel", "Cancel", "", act(CalAction::Custom(false)), false, cx)),
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
                    ("Agenda", CalendarView::Agenda),
                    ("Day", CalendarView::Day),
                    ("Week", CalendarView::Week),
                    ("Month", CalendarView::Month),
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
                (first - Days::days(first.weekday().num_days_from_monday() as i64), 42)
            }
        };
        let events = self
            .backend
            .agenda(day_start(from) - 86_400, day_start(from + Days::days(days as i64)) + 86_400);
        let title = match view {
            CalendarView::Month => anchor.format("%B %Y").to_string(),
            CalendarView::Week => format!(
                "{} – {}",
                from.format("%b %-d"),
                (from + Days::days(6)).format("%b %-d, %Y")
            ),
            _ => anchor.format("%A, %B %-d, %Y").to_string(),
        };
        let today = Local::now().date_naive();
        let header = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_2()
            .child(switcher)
            .child(div().flex_1())
            .child(self.button("calendar-prev", "@Previous", "chevron-left", act(CalAction::Previous), false, cx))
            .child(self.button("calendar-today", "Today", "", act(CalAction::Today), false, cx))
            .child(self.button("calendar-next", "@Next", "chevron-right", act(CalAction::Next), false, cx))
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
                let mut grid = div().flex().flex_col().border_1().border_color(rgb(c.border)).rounded_lg().overflow_hidden();
                let mut names = div().flex().bg(rgb(c.bg));
                for (_, n) in WEEKDAYS {
                    names = names.child(
                        div().flex_1().px_2().py_1().text_size(px(11.)).text_color(rgb(c.muted)).child(n),
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
                                    .font_weight(if day == today { FontWeight::BOLD } else { FontWeight::NORMAL })
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
                                div().text_size(px(10.)).text_color(rgb(c.muted)).child(format!("{more} more")),
                            );
                        }
                        row = row.child(cell);
                    }
                    grid = grid.child(row);
                }
                grid.into_any_element()
            }
            _ => {
                // Day and Week: a column per day, all-day events on top, then
                // the day's events in order with their times.
                let mut cols = div().flex().border_1().border_color(rgb(c.border)).rounded_lg().overflow_hidden();
                for i in 0..days {
                    let day = from + Days::days(i as i64);
                    let mut todays: Vec<&crate::sync::ical::Event> =
                        events.iter().filter(|e| on_day(e, day)).collect();
                    todays.sort_by_key(|e| (!e.all_day, e.start));
                    let mut col = div()
                        .flex_1()
                        .min_w_0()
                        .min_h(px(360.))
                        .p_2()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .when(i > 0, |d| d.border_l_1())
                        .border_color(rgb(c.border))
                        .child(
                            div()
                                .pb_1()
                                .text_size(px(12.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(if day == today { c.accent } else { c.text }))
                                .child(day.format(if days == 1 { "%A %-d" } else { "%a %-d" }).to_string()),
                        );
                    if todays.is_empty() {
                        col = col.child(div().text_size(px(11.)).text_color(rgb(c.muted)).child("Nothing"));
                    }
                    for (k, e) in todays.iter().enumerate() {
                        let uid = e.uid.clone();
                        let occurrence = e.recurrence_id;
                        let when = if e.all_day {
                            "All day".to_owned()
                        } else {
                            format!("{} – {}", time_of(e.start), time_of(e.end))
                        };
                        col = col.child(
                            div()
                                .id(SharedString::from(format!("day-event-{day}-{k}")))
                                .p_2()
                                .rounded(px(6.))
                                .bg(rgb(c.soft))
                                .border_l_2()
                                .border_color(rgb(c.accent))
                                .cursor_pointer()
                                .hover(move |s| s.bg(rgb(c.border)))
                                .child(div().text_size(px(12.)).font_weight(FontWeight::MEDIUM).truncate().child(e.summary.clone()))
                                .child(div().text_size(px(11.)).text_color(rgb(c.muted)).truncate().child(if e.location.is_empty() {
                                    when
                                } else {
                                    format!("{when} · {}", e.location)
                                }))
                                .on_click(cx.listener(move |this, _, w, cx| {
                                    this.handle(Action::EditEvent(uid.clone(), occurrence), w, cx)
                                })),
                        );
                    }
                    cols = cols.child(col);
                }
                cols.into_any_element()
            }
        };
        Some(div().flex().flex_col().gap_3().child(header).child(grid).into_any_element())
    }

    /// Calendar or Contacts in place of the list and reader.
    pub(super) fn app_view_page(&self, view: AppView, corners: Corners<Pixels>, cx: &Context<Self>) -> AnyElement {
        let c = self.colors();
        let (title, page) = match view {
            AppView::Calendar => ("Calendar", self.calendar_page(cx)),
            AppView::Contacts => ("Contacts", self.contacts_page(cx)),
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
                    .child(div().flex_1().text_size(px(16.)).font_weight(FontWeight::SEMIBOLD).child(title))
                    .child(self.button(
                        "app-view-close",
                        "@Back to mail",
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

    /// The sidebar's Calendar and Contacts entries.
    pub(super) fn app_view_links(&self, collapsed: bool, cx: &Context<Self>) -> AnyElement {
        let c = self.colors();
        let item = |id: &'static str, label: &'static str, icon: &'static str, view: AppView| {
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
                .when(!collapsed, |d| d.child(label))
                .tooltip(move |w, cx| Tooltip::new(label).build(w, cx))
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
            .child(item("open-calendar", "Calendar", "clock", AppView::Calendar))
            .child(item("open-contacts", "Contacts", "contact", AppView::Contacts))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::prelude::v1::test;

    #[test]
    fn custom_rules_are_written_as_rrules() {
        let mut days = [false; 7];
        days[0] = true;
        days[2] = true;
        let until = NaiveDate::from_ymd_opt(2027, 6, 30);
        assert_eq!(
            custom_rule(Freq::Weekly, 2, &days, Ends::After, 10, None),
            "FREQ=WEEKLY;INTERVAL=2;BYDAY=MO,WE;COUNT=10"
        );
        assert_eq!(
            custom_rule(Freq::Monthly, 1, &days, Ends::On, 0, until),
            "FREQ=MONTHLY;UNTIL=20270630T235959Z"
        );
        // What the editor makes, the calendar can expand.
        assert!(crate::sync::ical::Rule::expandable("FREQ=WEEKLY;INTERVAL=2;BYDAY=MO,WE;COUNT=10"));
    }
}

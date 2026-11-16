//! A date and time picker for choosing when something happens later:
//! when a snoozed message comes back, or when a scheduled message is sent.
//! Quick choices on the left; a month calendar, the time of day (steps and
//! common times) and what was chosen, said in words, on the right. With
//! Local AI's "Understand typed dates" on, a field above them takes a time
//! typed in words ("tmrw at 6", "next fri 9am", "in 2 hrs") and sets the
//! calendar and clock to it.
use super::*;
use crate::manage::local_ai::when;
use chrono::{Datelike, Days, Local, NaiveDate, NaiveTime, TimeZone, Timelike};

/// What the picker reports.
pub(super) enum PickerEvent {
    /// A time was chosen (seconds since the epoch).
    Picked(i64),
    Cancel,
}

/// Minutes the time steps by.
const STEP: u32 = 15;
/// Times of day offered with one click.
const COMMON_TIMES: [(u32, u32); 6] = [(8, 0), (9, 0), (12, 0), (13, 0), (17, 0), (20, 0)];

pub(super) struct DateTimePicker {
    colors: Colors,
    /// The first day of the month shown.
    month: NaiveDate,
    day: NaiveDate,
    hour: u32,
    minute: u32,
    /// Quick choices: (label, when).
    presets: Vec<(String, i64)>,
    /// What confirming does ("Snooze", "Schedule").
    confirm: SharedString,
    /// Ids are prefixed with this, so tests and the development driver can
    /// find a picker's parts.
    key: &'static str,
    error: Option<String>,
    /// Where a time is typed, and what reads it (None while the feature is
    /// off).
    typing: Option<Typing>,
}
impl EventEmitter<PickerEvent> for DateTimePicker {}

struct Typing {
    input: Entity<InputState>,
    parser: Rc<when::Parser>,
    /// What the typed text was read as, when it names a time.
    read: Option<when::When>,
    _subscription: Subscription,
}

impl DateTimePicker {
    pub(super) fn new(
        key: &'static str,
        confirm: impl Into<SharedString>,
        presets: Vec<(String, i64)>,
        colors: Colors,
    ) -> Self {
        let tomorrow = Local::now()
            .date_naive()
            .checked_add_days(Days::new(1))
            .unwrap_or_else(|| Local::now().date_naive());
        Self {
            colors,
            month: tomorrow.with_day(1).unwrap_or(tomorrow),
            day: tomorrow,
            hour: 8,
            minute: 0,
            presets,
            confirm: confirm.into(),
            key,
            error: None,
            typing: None,
        }
    }
    /// Adds the field a time can be typed into, read by `parser`.
    pub(super) fn typed(
        mut self,
        parser: Option<Rc<when::Parser>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let Some(parser) = parser else {
            return self;
        };
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Type a time: tmrw at 6, next fri 9am, in 2 hrs…")
        });
        // Ready to type into as soon as the picker opens.
        input.update(cx, |s, cx| s.focus(window, cx));
        let subscription = cx.subscribe_in(&input, window, |this, input, e, _, cx| match e {
            InputEvent::Change => {
                let text = input.read(cx).value().to_string();
                this.read_typed(&text, cx);
            }
            InputEvent::PressEnter { .. }
                if this.typing.as_ref().is_some_and(|t| t.read.is_some()) =>
            {
                this.confirm(cx)
            }
            _ => {}
        });
        self.typing = Some(Typing {
            input,
            parser,
            read: None,
            _subscription: subscription,
        });
        self
    }
    /// Sets the calendar and clock to the time `text` names, if it names one.
    fn read_typed(&mut self, text: &str, cx: &mut Context<Self>) {
        let Some(typing) = &mut self.typing else {
            return;
        };
        let now = Local::now();
        typing.read = if text.trim().is_empty() {
            None
        } else {
            typing.parser.parse(text, &now)
        };
        if let Some(t) = typing
            .read
            .as_ref()
            .and_then(|w| chrono::DateTime::from_timestamp(w.at, 0))
        {
            let t = t.with_timezone(&Local);
            self.day = t.date_naive();
            self.month = self.day.with_day(1).unwrap_or(self.day);
            self.hour = t.hour();
            self.minute = t.minute();
        }
        self.error = None;
        cx.notify();
    }
    /// Types `text` into the field, as someone typing it would.
    #[cfg(test)]
    pub(super) fn type_time(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(typing) = &self.typing {
            let input = typing.input.clone();
            input.update(cx, |s, cx| s.set_value(text.to_owned(), window, cx));
        }
        self.read_typed(text, cx);
    }
    /// Whether a time can be typed.
    #[cfg(test)]
    pub(super) fn can_type(&self) -> bool {
        self.typing.is_some()
    }

    /// Under the typed field: what it was read as.
    fn typed_hint(&self, typing: &Typing, cx: &Context<Self>) -> Option<Stateful<Div>> {
        let c = self.colors;
        let text = typing.input.read(cx).value();
        if text.trim().is_empty() {
            return None;
        }
        let (line, color) = match &typing.read {
            Some(w) => {
                let when = crate::manage::composing::describe_later(w.at, Local::now());
                let line = match &w.rule {
                    Some(rule) => format!(
                        "{when} · the first time of a repeating time ({})",
                        repeat_words(rule)
                    ),
                    None => when,
                };
                (line, c.accent)
            }
            None => (
                "Not a time Picacho can read. Try “tomorrow at 6”.".to_owned(),
                c.muted,
            ),
        };
        Some(
            div()
                .id(SharedString::from(format!("{}-typed-hint", self.key)))
                .debug_selector({
                    let key = self.key;
                    move || format!("{key}-typed-hint")
                })
                .text_size(px(12.))
                .text_color(rgb(color))
                .child(line),
        )
    }
    /// Starts at an earlier choice (a time already scheduled).
    pub(super) fn at(mut self, when: i64) -> Self {
        if let Some(t) = chrono::DateTime::from_timestamp(when, 0) {
            let t = t.with_timezone(&Local);
            self.day = t.date_naive();
            self.month = self.day.with_day(1).unwrap_or(self.day);
            self.hour = t.hour();
            self.minute = t.minute() / STEP * STEP;
        }
        self
    }
    /// The chosen moment, if it exists in this time zone.
    pub(super) fn chosen(&self) -> Option<i64> {
        let time = NaiveTime::from_hms_opt(self.hour, self.minute, 0)?;
        Local
            .from_local_datetime(&self.day.and_time(time))
            .earliest()
            .map(|t| t.timestamp())
    }
    fn confirm(&mut self, cx: &mut Context<Self>) {
        match self.chosen() {
            Some(at) if at > chrono::Utc::now().timestamp() + 30 => {
                cx.emit(PickerEvent::Picked(at))
            }
            Some(_) => {
                self.error = Some("Choose a time in the future.".into());
                cx.notify();
            }
            None => {
                self.error = Some(
                    "That time doesn't exist here (the clocks change). Choose another.".into(),
                );
                cx.notify();
            }
        }
    }
    fn shift_month(&mut self, months: i32, cx: &mut Context<Self>) {
        let total = self.month.year() * 12 + self.month.month0() as i32 + months;
        if let Some(m) =
            NaiveDate::from_ymd_opt(total.div_euclid(12), total.rem_euclid(12) as u32 + 1, 1)
        {
            // Not before this month.
            let this = Local::now().date_naive().with_day(1).unwrap_or(m);
            self.month = m.max(this);
        }
        cx.notify();
    }
    fn step_time(&mut self, minutes: i32, cx: &mut Context<Self>) {
        let total = (self.hour * 60 + self.minute) as i32 + minutes;
        let total = total.rem_euclid(24 * 60) as u32;
        self.hour = total / 60;
        self.minute = total % 60;
        self.error = None;
        cx.notify();
    }
    fn small_button(
        &self,
        id: String,
        label: impl Into<SharedString>,
        cx: &mut Context<Self>,
        on: impl Fn(&mut Self, &mut Context<Self>) + 'static,
    ) -> Stateful<Div> {
        let c = self.colors;
        let selector = id.clone();
        div()
            .id(SharedString::from(id))
            .debug_selector(move || selector.clone())
            .h(px(26.))
            .min_w(px(26.))
            .px_2()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(6.))
            .border_1()
            .border_color(rgb(c.border))
            .text_size(px(12.))
            .cursor_pointer()
            .hover(move |s| s.bg(rgb(c.soft)))
            .child(label.into())
            .on_click(cx.listener(move |this, _, _, cx| on(this, cx)))
    }
    fn calendar(&self, cx: &mut Context<Self>) -> Div {
        let c = self.colors;
        let key = self.key;
        let today = Local::now().date_naive();
        let lead = self.month.weekday().num_days_from_monday() as u64;
        let start = self
            .month
            .checked_sub_days(Days::new(lead))
            .unwrap_or(self.month);
        let can_go_back = self.month > today.with_day(1).unwrap_or(today);
        let mut grid = div().flex().flex_col().gap(px(2.));
        grid = grid.child(
            div()
                .flex()
                .children(["Mo", "Tu", "We", "Th", "Fr", "Sa", "Su"].map(|d| {
                    div()
                        .w(px(34.))
                        .h(px(22.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(10.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(c.muted))
                        .child(d)
                })),
        );
        for week in 0..6u64 {
            let mut row = div().flex();
            for weekday in 0..7u64 {
                let Some(day) = start.checked_add_days(Days::new(week * 7 + weekday)) else {
                    continue;
                };
                let in_month = day.month() == self.month.month();
                let past = day < today;
                let selected = day == self.day;
                let id = format!("{key}-day-{}", day.format("%Y-%m-%d"));
                let selector = id.clone();
                row = row.child(
                    div()
                        .id(SharedString::from(id))
                        .debug_selector(move || selector.clone())
                        .w(px(34.))
                        .h(px(30.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_full()
                        .text_size(px(12.))
                        .when(day == today && !selected, |d| {
                            d.border_1().border_color(rgb(c.accent))
                        })
                        .when(selected, |d| {
                            d.bg(rgb(c.accent))
                                .text_color(rgb(c.on_accent))
                                .font_weight(FontWeight::SEMIBOLD)
                        })
                        .when(!selected && !in_month, |d| d.text_color(rgb(c.muted)))
                        .when(past, |d| d.opacity(0.35))
                        .when(!past && !selected, |d| {
                            d.cursor_pointer().hover(move |s| s.bg(rgb(c.soft)))
                        })
                        .child(day.day().to_string())
                        .when(!past, |d| {
                            d.on_click(cx.listener(move |this, _, _, cx| {
                                this.day = day;
                                if day.month() != this.month.month() {
                                    this.month = day.with_day(1).unwrap_or(day);
                                }
                                this.error = None;
                                cx.notify();
                            }))
                        }),
                );
            }
            grid = grid.child(row);
        }
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(13.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(self.month.format("%B %Y").to_string()),
                    )
                    .child(
                        self.small_button(format!("{key}-prev-month"), "", cx, |t, cx| {
                            t.shift_month(-1, cx)
                        })
                        .child(icon_el("chevron-left", 13., c.muted))
                        .when(!can_go_back, |d| d.opacity(0.35)),
                    )
                    .child(
                        self.small_button(format!("{key}-next-month"), "", cx, |t, cx| {
                            t.shift_month(1, cx)
                        })
                        .child(icon_el("chevron-right", 13., c.muted)),
                    ),
            )
            .child(grid)
    }
    fn time_row(&self, cx: &mut Context<Self>) -> Div {
        let c = self.colors;
        let key = self.key;
        let time = format!("{}:{:02}", self.hour, self.minute);
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(icon_el("clock", 14., c.muted))
                    .child(
                        self.small_button(format!("{key}-earlier"), "−", cx, |t, cx| {
                            t.step_time(-(STEP as i32), cx)
                        }),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("{key}-time")))
                            .debug_selector(move || format!("{key}-time"))
                            .min_w(px(56.))
                            .text_center()
                            .text_size(px(18.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(time),
                    )
                    .child(self.small_button(format!("{key}-later"), "+", cx, |t, cx| {
                        t.step_time(STEP as i32, cx)
                    }))
                    .child(
                        self.small_button(format!("{key}-hour-later"), "+1h", cx, |t, cx| {
                            t.step_time(60, cx)
                        }),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_1()
                    .children(COMMON_TIMES.map(|(h, m)| {
                        let chosen = self.hour == h && self.minute == m;
                        self.small_button(
                            format!("{key}-at-{h}-{m:02}"),
                            format!("{h}:{m:02}"),
                            cx,
                            move |t, cx| {
                                t.hour = h;
                                t.minute = m;
                                t.error = None;
                                cx.notify();
                            },
                        )
                        .when(chosen, |d| d.bg(rgb(c.soft)).text_color(rgb(c.accent)))
                    })),
            )
    }
}

impl Render for DateTimePicker {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = self.colors;
        let key = self.key;
        let now = Local::now();
        let summary = match self.chosen() {
            Some(at) => crate::manage::composing::describe_later(at, now),
            None => "—".into(),
        };
        let mut presets = div()
            .flex()
            .flex_col()
            .gap(px(2.))
            .w(px(190.))
            .flex_shrink_0();
        for (i, (label, at)) in self.presets.clone().into_iter().enumerate() {
            let when = crate::manage::composing::describe_later(at, now);
            presets = presets.child(
                div()
                    .id((key, i))
                    .debug_selector(move || format!("{key}-preset-{i}"))
                    .px_2()
                    .py(px(6.))
                    .rounded(px(7.))
                    .flex()
                    .flex_col()
                    .cursor_pointer()
                    .hover(move |s| s.bg(rgb(c.soft)))
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_weight(FontWeight::MEDIUM)
                            .child(label),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(rgb(c.muted))
                            .child(when),
                    )
                    .on_click(cx.listener(move |_, _, _, cx| cx.emit(PickerEvent::Picked(at)))),
            );
        }
        div()
            .id(SharedString::from(format!("{key}-picker")))
            .debug_selector(move || format!("{key}-picker"))
            .flex()
            .flex_col()
            .gap_3()
            .when_some(self.typing.as_ref(), |d, typing| {
                d.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            div()
                                .id(SharedString::from(format!("{key}-typed")))
                                .debug_selector(move || format!("{key}-typed"))
                                .child(
                                    Input::new(&typing.input)
                                        .prefix(icon_el("sparkle", 13., c.muted))
                                        .text_size(px(13.)),
                                ),
                        )
                        .children(self.typed_hint(typing, cx)),
                )
            })
            .child(
                div()
                    .flex()
                    .gap_4()
                    .child(presets)
                    .child(div().w(px(1.)).bg(rgb(c.border)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap_3()
                            .child(self.calendar(cx))
                            .child(self.time_row(cx)),
                    ),
            )
            .children(
                self.error
                    .clone()
                    .map(|e| div().text_size(px(12.)).text_color(rgb(c.danger)).child(e)),
            )
            .child(
                div()
                    .pt_3()
                    .border_t_1()
                    .border_color(rgb(c.border))
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(13.))
                            .child(format!("{} {summary}", self.confirm)),
                    )
                    .child(
                        self.small_button(format!("{key}-cancel"), "Cancel", cx, |_, cx| {
                            cx.emit(PickerEvent::Cancel)
                        })
                        .h(px(30.))
                        .px_3(),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("{key}-confirm")))
                            .debug_selector(move || format!("{key}-confirm"))
                            .h(px(30.))
                            .px_3()
                            .flex()
                            .items_center()
                            .rounded(px(6.))
                            .bg(rgb(c.accent))
                            .text_color(rgb(c.on_accent))
                            .text_size(px(12.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .cursor_pointer()
                            .hover(move |s| s.bg(rgb(c.accent_hover)))
                            .child(self.confirm.clone())
                            .on_click(cx.listener(|this, _, _, cx| this.confirm(cx))),
                    ),
            )
    }
}

/// A repeating rule in a few words: "every 2 weeks on MO, WE".
fn repeat_words(rule: &str) -> String {
    let field = |name: &str| {
        rule.split(';')
            .find_map(|p| p.strip_prefix(name)?.strip_prefix('='))
    };
    let unit = match field("FREQ") {
        Some("MINUTELY") => "minute",
        Some("HOURLY") => "hour",
        Some("DAILY") => "day",
        Some("WEEKLY") => "week",
        Some("MONTHLY") => "month",
        _ => "year",
    };
    let mut out = match field("INTERVAL").and_then(|n| n.parse::<u32>().ok()) {
        Some(n) if n > 1 => format!("every {n} {unit}s"),
        _ => format!("every {unit}"),
    };
    if let Some(days) = field("BYDAY") {
        out.push_str(&format!(" on {}", days.replace(',', ", ")));
    }
    out
}

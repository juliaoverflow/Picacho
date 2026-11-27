//! Calendars in Picacho: an invitation's card in the reader (Accept, Maybe,
//! Decline), and Settings → Calendar: the CalDAV server, its calendars, the
//! coming events, and adding and editing events.
use super::*;
use crate::sync::ical::{self, Event, PartStat};

/// When an event is, as people say it: "Tue, Nov 24, 10:00 – 11:00", in
/// this computer's time, with the event's own when its zone keeps
/// another time ("… (16:00 – 17:00 in Asia/Tokyo)").
pub(super) fn event_when(e: &Event) -> String {
    let mut text = event_when_here(e);
    if !e.all_day
        && let Some(zone) = find_zone(&e.tzid)
        && !same_as_here(zone)
    {
        let at = |t: i64| {
            chrono::DateTime::from_timestamp(t, 0)
                .unwrap_or_default()
                .with_timezone(&zone)
        };
        text = t!(
            "calendar-when-in-its-zone",
            when = text,
            start = at(e.start).local(t!("dates-calendar-time")),
            end = at(e.end).local(t!("dates-calendar-time")),
            zone = zone.name()
        );
    }
    text
}

fn event_when_here(e: &Event) -> String {
    let local = |t: i64| {
        chrono::DateTime::from_timestamp(t, 0)
            .unwrap_or_default()
            .with_timezone(&chrono::Local)
    };
    let (start, end) = (local(e.start), local(e.end));
    if e.all_day {
        let last = local(e.end - 1);
        if last.date_naive() > start.date_naive() {
            t!("calendar-all-day", first = start.local(t!("dates-weekday-month-day")), last = last.local(t!("dates-weekday-month-day")))
        } else {
            t!("calendar-all-day-2", day = start.local(t!("dates-weekday-month-day-year")))
        }
    } else if start.date_naive() == end.date_naive() {
        format!(
            "{} – {}",
            start.local(t!("dates-weekday-month-day-year-time")),
            end.local(t!("dates-calendar-time"))
        )
    } else {
        format!(
            "{} – {}",
            start.local(t!("dates-weekday-month-day-time")),
            end.local(t!("dates-weekday-month-day-time"))
        )
    }
}

/// Reads "2026-11-24 10:00" (or "2026-11-24" for a date) as local time.
#[cfg(test)]
pub(super) fn parse_local(text: &str) -> Option<(i64, bool)> {
    parse_in(text, None)
}

/// Reads "2026-11-24 10:00" as a time in `zone` (this computer's when
/// `None`), or "2026-11-24" as a date.
pub(super) fn parse_in(text: &str, zone: Option<chrono_tz::Tz>) -> Option<(i64, bool)> {
    use chrono::TimeZone;
    let text = text.trim();
    if let Ok(t) = chrono::NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M") {
        return match zone {
            Some(z) => z
                .from_local_datetime(&t)
                .earliest()
                .map(|t| (t.timestamp(), false)),
            None => chrono::Local
                .from_local_datetime(&t)
                .earliest()
                .map(|t| (t.timestamp(), false)),
        };
    }
    let d = chrono::NaiveDate::parse_from_str(text, "%Y-%m-%d").ok()?;
    Some((d.and_hms_opt(0, 0, 0)?.and_utc().timestamp(), true))
}

/// A repeating time's rule without the time of day it names (that is the
/// event's start): "FREQ=WEEKLY;BYDAY=MO;BYHOUR=9" is "FREQ=WEEKLY;BYDAY=MO".
fn rule_without_times(rule: &str) -> String {
    rule.split(';')
        .filter(|p| {
            let k = p.split('=').next().unwrap_or("").to_ascii_uppercase();
            !matches!(k.as_str(), "BYHOUR" | "BYMINUTE" | "BYSECOND" | "DTSTART")
        })
        .collect::<Vec<_>>()
        .join(";")
}

fn format_local(t: i64, all_day: bool) -> String {
    format_in(t, all_day, None)
}

/// `t` written as the editor reads it, in `zone` (this computer's when `None`).
fn format_in(t: i64, all_day: bool, zone: Option<chrono_tz::Tz>) -> String {
    let dt = chrono::DateTime::from_timestamp(t, 0).unwrap_or_default();
    if all_day {
        dt.format("%Y-%m-%d").to_string()
    } else {
        match zone {
            Some(z) => dt.with_timezone(&z).format("%Y-%m-%d %H:%M").to_string(),
            None => dt
                .with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M")
                .to_string(),
        }
    }
}

/// A time zone as typed: an IANA name ("Europe/Berlin", any case), or a
/// city alone where only one zone has it ("berlin", "New York").
pub(super) fn find_zone(text: &str) -> Option<chrono_tz::Tz> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    if let Ok(z) = text.parse::<chrono_tz::Tz>() {
        return Some(z);
    }
    let wanted = text.to_ascii_lowercase().replace(' ', "_");
    let matches: Vec<chrono_tz::Tz> = chrono_tz::TZ_VARIANTS
        .iter()
        .copied()
        .filter(|z| {
            let name = z.name().to_ascii_lowercase();
            name == wanted || name.rsplit('/').next() == Some(wanted.as_str())
        })
        .collect();
    (matches.len() == 1).then(|| matches[0])
}

/// Whether `zone` keeps this computer's time (the same offsets all year).
fn same_as_here(zone: chrono_tz::Tz) -> bool {
    use chrono::{Offset, TimeZone};
    let now = chrono::Utc::now();
    [0i64, 182].iter().all(|days| {
        let t = (now + chrono::Duration::days(*days)).naive_utc();
        zone.offset_from_utc_datetime(&t).fix() == chrono::Local.offset_from_utc_datetime(&t).fix()
    })
}

/// How an event being edited repeats.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) enum Repeat {
    #[default]
    Never,
    Daily,
    Weekly,
    Monthly,
    Yearly,
    /// A rule made elsewhere, kept as it was.
    Kept(String),
}

impl Repeat {
    pub(super) fn of(rule: &str) -> Self {
        match rule.trim().to_ascii_uppercase().as_str() {
            "" => Self::Never,
            "FREQ=DAILY" => Self::Daily,
            "FREQ=WEEKLY" => Self::Weekly,
            "FREQ=MONTHLY" => Self::Monthly,
            "FREQ=YEARLY" => Self::Yearly,
            _ => Self::Kept(rule.trim().to_owned()),
        }
    }
    fn rule(&self) -> String {
        match self {
            Self::Never => String::new(),
            Self::Daily => "FREQ=DAILY".into(),
            Self::Weekly => "FREQ=WEEKLY".into(),
            Self::Monthly => "FREQ=MONTHLY".into(),
            Self::Yearly => "FREQ=YEARLY".into(),
            Self::Kept(r) => r.clone(),
        }
    }
}

/// An invitation found in a message, for the reader's card.
#[derive(Clone, Debug, Default)]
pub(super) struct Invitation {
    pub method: String,
    pub event: Event,
}

impl MailApp {
    /// Looks for an invitation in the open message, once.
    pub(super) fn check_invitation(&mut self, m: &Mail, cx: &mut Context<Self>) {
        if self.invitations.contains_key(&m.id) || m.body.is_none() {
            return;
        }
        // What was found per message, for the session: bounded, as the
        // other caches of the reader are.
        if self.invitations.len() >= super::READER_CACHE {
            self.invitations.clear();
        }
        // Calendar parts are attachments, or the message says it's an invitation.
        let likely = m.attachments().iter().any(|a| {
            a.mime.eq_ignore_ascii_case("text/calendar")
                || a.mime.eq_ignore_ascii_case("application/ics")
                || a.name.to_ascii_lowercase().ends_with(".ics")
        }) || m.has_attachments
            || m.body
                .as_deref()
                .is_some_and(|b| b.contains("BEGIN:VCALENDAR"));
        if !likely {
            self.invitations.insert(m.id.clone(), None);
            return;
        }
        let Some(a) = self.account_of(m) else { return };
        self.invitations.insert(m.id.clone(), None);
        let (id, mail) = (m.id.clone(), m.clone());
        self.task(
            cx,
            move |b| b.invitation(&a, &mail),
            move |this, result, cx| {
                if let Ok(Some(cal)) = result
                    && let Some(event) = cal.events.into_iter().next()
                {
                    this.invitations.insert(
                        id,
                        Some(Invitation {
                            method: cal.method,
                            event,
                        }),
                    );
                    cx.notify();
                }
            },
        );
    }

    /// The invitation card above a message's text.
    pub(super) fn invitation_card(&self, m: &Mail, cx: &Context<Self>) -> Option<AnyElement> {
        let c = self.colors();
        let inv = self.invitations.get(&m.id)?.as_ref()?;
        let e = &inv.event;
        let answered = self.backend.invitation_answer(&e.uid);
        let busy = self.invitation_busy.as_deref() == Some(m.id.as_str());
        let mine = |at: &ical::Attendee| {
            self.accounts.iter().any(|a| {
                a.email.eq_ignore_ascii_case(&at.email)
                    || a.identities
                        .iter()
                        .any(|i| i.email.trim().eq_ignore_ascii_case(&at.email))
            })
        };
        // An invitation you already have in your calendar, changed since.
        let changed = (inv.method == "REQUEST")
            .then(|| self.backend.invitation_changed(e))
            .flatten();
        let title = match inv.method.as_str() {
            "CANCEL" => t!("calendar-cancelled"),
            "REPLY" => t!("calendar-answer-to-your-invitation"),
            "COUNTER" => t!("calendar-new-time-proposed"),
            "DECLINECOUNTER" => t!("calendar-your-proposed-time-wasnt-taken"),
            "REFRESH" => t!("calendar-a-guest-asks-for-the-latest"),
            "ADD" => t!("calendar-more-times-added"),
            "PUBLISH" | "" => t!("calendar-event"),
            _ if changed.is_some() => t!("calendar-updated-invitation"),
            _ => t!("calendar-invitation"),
        };
        let row = |label: &'static str, text: String| {
            div()
                .flex()
                .gap_2()
                .text_size(px(12.5))
                .child(
                    div()
                        .w(px(72.))
                        .flex_shrink_0()
                        .text_color(rgb(c.muted))
                        .child(label),
                )
                .child(div().flex_1().min_w_0().child(text))
        };
        let organizer = e.organizer.as_ref().map(|o| {
            if o.name.is_empty() {
                o.email.clone()
            } else {
                format!("{} <{}>", o.name, o.email)
            }
        });
        let people = e
            .attendees
            .iter()
            .map(|at| {
                let name = if at.name.is_empty() {
                    at.email.clone()
                } else {
                    at.name.clone()
                };
                // Optional guests and rooms say so (ROLE, CUTYPE).
                let role = ical::extra_param(&at.extra, "ROLE").unwrap_or_default();
                let kind = ical::extra_param(&at.extra, "CUTYPE").unwrap_or_default();
                let name = if role.eq_ignore_ascii_case("OPT-PARTICIPANT") {
                    t!("calendar-optional", name = name)
                } else if kind.eq_ignore_ascii_case("ROOM") || kind.eq_ignore_ascii_case("RESOURCE") {
                    format!("{name}, {}", kind.to_ascii_lowercase())
                } else {
                    name
                };
                match at.status {
                    Some(PartStat::Accepted) => t!("calendar-yes", name = name),
                    Some(PartStat::Declined) => t!("calendar-no", name = name),
                    Some(PartStat::Tentative) => t!("calendar-maybe", name = name),
                    Some(PartStat::Delegated) => t!("calendar-delegated", name = name),
                    _ => name,
                }
            })
            .collect::<Vec<_>>()
            .join(", ");
        let invited = inv.method == "REQUEST" && !e.cancelled && e.attendees.iter().any(mine);
        let id = m.id.clone();
        let answer = |label: &'static str, icon: &'static str, status: PartStat| {
            let chosen = answered == Some(status);
            self.button(
                format!("invite-{}", status.as_str().to_ascii_lowercase()),
                label,
                icon,
                Action::AnswerInvitation(id.clone(), status),
                chosen,
                cx,
            )
            .aria_toggled(if chosen {
                Toggled::True
            } else {
                Toggled::False
            })
            .when(busy, |d| d.opacity(0.6))
        };
        Some(
            div()
                .id("invitation")
                .role(Role::Region)
                .aria_label(format!("{title}: {}", e.summary))
                .debug_selector(|| "invitation".into())
                .mb_4()
                .p_4()
                .rounded(px(12.))
                .border_1()
                .border_color(rgb(if e.cancelled { c.danger } else { c.border }))
                .bg(rgb(c.raised))
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(icon_el(
                            "clock",
                            16.,
                            if e.cancelled { c.danger } else { c.accent },
                        ))
                        .child(
                            div()
                                .text_size(px(11.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(c.muted))
                                .child(title.to_uppercase()),
                        ),
                )
                .child(
                    div()
                        .text_size(px(16.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .when(e.cancelled, |d| d.line_through())
                        .child(if e.summary.is_empty() {
                            t!("calendar-no-title").to_owned()
                        } else {
                            e.summary.clone()
                        }),
                )
                .child(row(t!("calendar-when"), event_when(e)))
                .when(!e.recurrence.is_empty(), |d| {
                    d.child(row(
                        t!("calendar-repeats"),
                        match ical::Rule::parse(&e.recurrence) {
                            Some(r) if ical::Rule::understood(&e.recurrence) => r.describe(),
                            _ if ical::Rule::expandable(&e.recurrence) => {
                                t!("calendar-yes-on-a-schedule-of-its").into()
                            }
                            _ => t!("calendar-yes-2").into(),
                        },
                    ))
                })
                .when(!e.location.is_empty(), |d| {
                    d.child(row(t!("calendar-where"), e.location.clone()))
                })
                .children(e.details().into_iter().map(|(label, text)| row(label, text)))
                .when_some(organizer, |d, o| d.child(row(t!("calendar-organizer"), o)))
                .when(!people.is_empty(), |d| d.child(row(t!("calendar-guests"), people)))
                .when(invited, |d| {
                    d.child(
                        div()
                            .mt_2()
                            .flex()
                            .flex_wrap()
                            .gap_2()
                            .child(answer(t!("calendar-accept"), "check", PartStat::Accepted))
                            .child(answer(t!("calendar-maybe-2"), "info", PartStat::Tentative))
                            .child(answer(t!("calendar-decline"), "close", PartStat::Declined)),
                    )
                })
                .when(
                    e.cancelled && self.backend.calendar_server().is_some(),
                    |d| {
                        d.child(div().mt_2().flex().child(self.button(
                            "invite-remove",
                            t!("calendar-remove-from-calendar"),
                            "trash",
                            Action::RemoveCancelled(m.id.clone()),
                            false,
                            cx,
                        )))
                    },
                )
                .when(
                    inv.method == "PUBLISH" && self.backend.default_calendar().is_some(),
                    |d| {
                        d.child(div().mt_2().flex().child(self.button(
                            "invite-add",
                            t!("calendar-add-to-calendar"),
                            "plus",
                            Action::AddEventFromMail(m.id.clone()),
                            false,
                            cx,
                        )))
                    },
                )
                // Changed since you answered: what changed, and an update.
                .when_some(changed.filter(|_| invited), |d, before| {
                    let mut what = Vec::new();
                    if before.event.start != e.start || before.event.end != e.end {
                        what.push(t!("calendar-was", when = event_when(&before.event)));
                    }
                    if before.event.location != e.location && !before.event.location.is_empty() {
                        what.push(t!("calendar-was-at", location = before.event.location));
                    }
                    d.child(
                        div()
                            .text_size(px(11.5))
                            .text_color(rgb(c.muted))
                            .child(if what.is_empty() {
                                t!("calendar-changed-since-its-in-your-calendar").to_owned()
                            } else {
                                t!("calendar-changed", changes = what.join("; "))
                            }),
                    )
                    .child(div().flex().child(self.button(
                        "invite-update",
                        t!("calendar-update-my-calendar"),
                        "refresh",
                        Action::TakeInvitationUpdate(m.id.clone()),
                        false,
                        cx,
                    )))
                })
                // A guest proposing another time can say so.
                .when(invited && self.proposing.as_deref() != Some(m.id.as_str()), |d| {
                    d.child(div().flex().child(self.link(
                        "invite-propose",
                        t!("calendar-propose-a-new-time"),
                        Action::ProposeTime(m.id.clone()),
                        cx,
                    )))
                })
                .when(invited && self.proposing.as_deref() == Some(m.id.as_str()), |d| {
                    d.child(self.propose_form(&m.id, cx))
                })
                // As organizer: a guest's answer, or proposed time.
                .when(inv.method == "REPLY" && self.backend.stored_event(&e.uid).is_some(), |d| {
                    d.child(div().mt_2().flex().child(self.button(
                        "invite-apply",
                        t!("calendar-note-their-answer"),
                        "check",
                        Action::ApplyReply(m.id.clone()),
                        false,
                        cx,
                    )))
                })
                .when(inv.method == "COUNTER" && self.backend.stored_event(&e.uid).is_some(), |d| {
                    let before = self.backend.stored_event(&e.uid).map(|s| event_when(&s.event)).unwrap_or_default();
                    d.child(
                        div()
                            .text_size(px(11.5))
                            .text_color(rgb(c.muted))
                            .child(t!("calendar-now", before = before)),
                    )
                    .child(
                        div()
                            .mt_2()
                            .flex()
                            .gap_2()
                            .child(self.button(
                                "counter-accept",
                                t!("calendar-move-the-event"),
                                "check",
                                Action::AcceptCounter(m.id.clone()),
                                true,
                                cx,
                            ))
                            .child(self.button(
                                "counter-decline",
                                t!("calendar-keep-the-time"),
                                "close",
                                Action::DeclineCounter(m.id.clone()),
                                false,
                                cx,
                            )),
                    )
                })
                .when(inv.method == "REFRESH" && self.backend.stored_event(&e.uid).is_some(), |d| {
                    d.child(div().mt_2().flex().child(self.button(
                        "invite-refresh",
                        t!("calendar-send-it-again"),
                        "send",
                        Action::AnswerRefresh(m.id.clone()),
                        true,
                        cx,
                    )))
                })
                .when(inv.method == "ADD" && self.backend.stored_event(&e.uid).is_some(), |d| {
                    d.child(div().mt_2().flex().child(self.button(
                        "invite-add",
                        t!("calendar-add-to-calendar"),
                        "check",
                        Action::TakeAddedOccurrences(m.id.clone()),
                        true,
                        cx,
                    )))
                })
                // A guest's note with a proposed time (COMMENT; older
                // Picachos wrote it as the description).
                .when(inv.method == "COUNTER" && !(e.comment.is_empty() && e.description.is_empty()), |d| {
                    d.child(row(
                        t!("calendar-note"),
                        if e.comment.is_empty() { e.description.clone() } else { e.comment.clone() },
                    ))
                })
                .when_some(answered.filter(|_| invited), |d, s| {
                    d.child(
                        div()
                            .text_size(px(11.5))
                            .text_color(rgb(c.muted))
                            .child(t!("calendar-you-answered", verb = s.verb())),
                    )
                })
                .into_any_element(),
        )
    }

    /// Answers the open message's invitation.
    pub(super) fn answer_invitation(
        &mut self,
        id: String,
        status: PartStat,
        cx: &mut Context<Self>,
    ) {
        let Some(m) = self.messages.iter().find(|m| m.id == id).cloned() else {
            return;
        };
        let Some(a) = self.account_of(&m) else { return };
        if self.invitation_busy.is_some() {
            return;
        }
        self.invitation_busy = Some(id);
        self.task(
            cx,
            move |b| b.answer_invitation(&a, &m, status),
            move |this, result, cx| {
                this.invitation_busy = None;
                match result {
                    Ok(None) => this.notice(
                        t!("calendar-the-organizer-was-told", verb = status.verb()),
                        false,
                        cx,
                    ),
                    Ok(Some(note)) => this.notice(note, true, cx),
                    Err(e) => this.notice(format!("{e:#}"), true, cx),
                }
            },
        );
    }

    pub(super) fn calendar_action(
        &mut self,
        action: Action,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            Action::AnswerInvitation(id, status) => self.answer_invitation(id, status, cx),
            Action::RemoveCancelled(id) => {
                let Some(m) = self.messages.iter().find(|m| m.id == id).cloned() else {
                    return;
                };
                let Some(a) = self.account_of(&m) else { return };
                self.task(
                    cx,
                    move |b| b.remove_cancelled(&a, &m),
                    |this, r, cx| match r {
                        Ok(()) => this.notice(t!("calendar-removed-from-your-calendar"), false, cx),
                        Err(e) => this.notice(format!("{e:#}"), true, cx),
                    },
                );
            }
            Action::ProposeTime(id) => {
                let start = self.invitations.get(&id).cloned().flatten().map(|i| {
                    (
                        format_local(i.event.start, false),
                        format_local(i.event.end, false),
                    )
                });
                if let Some((start, end)) = start {
                    self.event_start
                        .update(cx, |i, cx| i.set_text(start, w, cx));
                    self.event_end.update(cx, |i, cx| i.set_text(end, w, cx));
                    self.event_notes.update(cx, |i, cx| i.set_text("", w, cx));
                    self.event_zone_chosen = None;
                    self.proposing = Some(id);
                }
            }
            Action::CancelProposal => self.proposing = None,
            Action::SendProposal(id) => {
                let Some(m) = self.messages.iter().find(|m| m.id == id).cloned() else {
                    return;
                };
                let Some(a) = self.account_of(&m) else { return };
                let value =
                    |i: &Entity<InputState>, cx: &Context<Self>| i.read(cx).value().to_string();
                let (Some((start, false, _)), Some((end, false, _))) = (
                    self.read_when(&value(&self.event_start, cx)),
                    self.read_when(&value(&self.event_end, cx)),
                ) else {
                    self.notice(t!("calendar-write-the-times-as-2026-11"), true, cx);
                    return;
                };
                let note = value(&self.event_notes, cx);
                self.proposing = None;
                self.task(
                    cx,
                    move |b| b.propose_time(&a, &m, start, end, &note),
                    |this, r, cx| match r {
                        Ok(()) => {
                            this.notice(t!("calendar-your-proposed-time-was-sent-to"), false, cx)
                        }
                        Err(e) => this.notice(format!("{e:#}"), true, cx),
                    },
                );
            }
            Action::ApplyReply(id) => self.on_invitation(&id, cx, |b, a, m| b.apply_reply(a, m)),
            Action::AcceptCounter(id) => self.on_invitation(&id, cx, |b, a, m| {
                b.accept_counter(a, m).map(|n| {
                    t!("calendar-the-event-moved-told", n = n)
                })
            }),
            Action::DeclineCounter(id) => self.on_invitation(&id, cx, |b, a, m| {
                b.decline_counter(a, m)
                    .map(|()| "They were told the time stays".into())
            }),
            Action::AnswerRefresh(id) => self.on_invitation(&id, cx, |b, a, m| {
                b.answer_refresh(a, m).map(|()| "They were sent the event as it is now".into())
            }),
            Action::TakeAddedOccurrences(id) => self.on_invitation(&id, cx, |b, a, m| {
                b.take_added(a, m).map(|()| "The added times are in your calendar".into())
            }),
            Action::TakeInvitationUpdate(id) => self.on_invitation(&id, cx, |b, a, m| {
                b.take_update(a, m)
                    .map(|()| "Your calendar has the new version".into())
            }),
            Action::AddEventFromMail(id) => {
                let Some(event) = self
                    .invitations
                    .get(&id)
                    .cloned()
                    .flatten()
                    .map(|i| i.event)
                else {
                    return;
                };
                let Some(m) = self.messages.iter().find(|m| m.id == id).cloned() else {
                    return;
                };
                let Some(a) = self.account_of(&m) else { return };
                self.task(
                    cx,
                    // One already in the calendar is replaced only by mail
                    // from its organizer, vouched for by their domain.
                    move |b| b.add_event_from_mail(&a, &m, event),
                    |this, r, cx| match r {
                        Ok(_) => this.notice(t!("calendar-added-to-your-calendar"), false, cx),
                        Err(e) => this.notice(format!("{e:#}"), true, cx),
                    },
                );
            }
            Action::ConnectCalendar => {
                let server = crate::sync::caldav::Server {
                    url: self.calendar_url.read(cx).value().trim().to_owned(),
                    user: self.calendar_user.read(cx).value().trim().to_owned(),
                    ..Default::default()
                };
                let password = self.calendar_password.read(cx).value().to_string();
                self.calendar_password
                    .update(cx, |input, cx| input.set_text("", w, cx));
                // With calendars set up already, this one goes beside them.
                let another = self.backend.primary_calendar_server().is_some();
                self.calendar_run(cx, move |b| {
                    let n = if another {
                        b.add_calendar_source(|| b.connect_calendar(server, &password))?
                    } else {
                        b.connect_calendar(server, &password)?
                    };
                    Ok(t!("calendar-read-events", n = n))
                });
            }
            Action::SyncCalendar => {
                self.calendar_run(cx, |b| {
                    let _ = b.sync_tasks();
                    b.sync_calendar().map(|n| t!("calendar-read-events", n = n))
                });
            }
            Action::DismissCalendarNotices => {
                let _ = self.backend.dismiss_calendar_notices();
                cx.notify();
            }
            Action::DisconnectCalendar => {
                self.calendar_run(cx, |b| {
                    b.disconnect_calendar()
                        .map(|()| "Calendar server disconnected".into())
                });
            }
            Action::DefaultCalendar(href) => {
                let _ = self.backend.set_default_calendar(&href);
                cx.notify();
            }
            Action::EventScope(all) => self.event_all_repeats = all,
            Action::EventRepeat(r) => self.event_repeat = r,
            Action::EventAlarm(a) => self.event_alarm = a,
            Action::NewEvent => {
                self.event_alarm = None;
                self.free_busy = None;
                self.event_occurrence = None;
                self.event_repeat = Repeat::Never;
                let here = crate::manage::backend::calendar::local_zone().unwrap_or_default();
                self.event_zone.update(cx, |i, cx| i.set_text(here, w, cx));
                let start = (chrono::Local::now() + chrono::Duration::hours(1))
                    .format("%Y-%m-%d %H:00")
                    .to_string();
                let end = (chrono::Local::now() + chrono::Duration::hours(2))
                    .format("%Y-%m-%d %H:00")
                    .to_string();
                self.edit_event(
                    String::new(),
                    String::new(),
                    "",
                    &start,
                    &end,
                    "",
                    "",
                    w,
                    cx,
                );
            }
            Action::EditEvent(uid, occurrence) => {
                let Some(s) = self
                    .backend
                    .calendar_events()
                    .into_iter()
                    .find(|s| s.event.uid == uid)
                else {
                    return;
                };
                self.event_repeat = Repeat::of(&s.event.recurrence);
                self.event_alarm = s.event.alarms.first().copied();
                self.free_busy = None;
                // One repeat: as it is now (changed on its own, or as the series has it).
                let e = match occurrence {
                    Some(at) => ical::occurrences(&s.event, &s.overrides, at, at + 1)
                        .into_iter()
                        .find(|o| o.recurrence_id == Some(at))
                        .unwrap_or_else(|| s.event.clone()),
                    None => s.event,
                };
                self.event_occurrence = occurrence;
                self.event_all_repeats = false;
                let end = if e.all_day { e.end - 86_400 } else { e.end };
                let zone = find_zone(&e.tzid)
                    .or_else(|| (e.utc && e.tzid.is_empty()).then_some(chrono_tz::UTC));
                let zone_text = match zone {
                    Some(z) => z.name().to_owned(),
                    None => crate::manage::backend::calendar::local_zone().unwrap_or_default(),
                };
                self.event_zone
                    .update(cx, |i, cx| i.set_text(zone_text, w, cx));
                let guests = e
                    .attendees
                    .iter()
                    .map(|g| g.email.clone())
                    .collect::<Vec<_>>()
                    .join(", ");
                self.edit_event(
                    uid,
                    guests,
                    &e.summary,
                    &format_in(e.start, e.all_day, zone),
                    &format_in(end, e.all_day, zone),
                    &e.location,
                    &e.description,
                    w,
                    cx,
                );
            }
            Action::CancelEventEdit => {
                self.event_editing = None;
                cx.notify();
            }
            Action::SaveEvent => {
                let Some(uid) = self.event_editing.clone() else {
                    return;
                };
                let value =
                    |i: &Entity<InputState>, cx: &Context<Self>| i.read(cx).value().to_string();
                let summary = value(&self.event_title, cx);
                let zone_text = value(&self.event_zone, cx);
                let zone = find_zone(&zone_text);
                if !zone_text.trim().is_empty() && zone.is_none() {
                    self.calendar_error = Some(t!("calendar-isnt-a-time-zone-picacho-knows", trim = zone_text.trim()));
                    cx.notify();
                    return;
                }
                self.event_zone_chosen = zone;
                let start_text = value(&self.event_start, cx);
                let (start, all_day) = match self.read_when(&start_text) {
                    Some((t, all_day, rule)) => {
                        // "every monday at 9": it repeats, as the words say.
                        if let Some(rule) = rule
                            && self.event_occurrence.is_none()
                        {
                            self.event_repeat = Repeat::of(&rule_without_times(&rule));
                        }
                        (t, all_day)
                    }
                    None => {
                        self.calendar_error = Some(if self.when.is_some() {
                            t!("calendar-picacho-couldnt-tell-when-it-starts").into()
                        } else {
                            t!("calendar-write-the-start-as-2026-11").into()
                        });
                        cx.notify();
                        return;
                    }
                };
                let end_text = value(&self.event_end, cx);
                let end = match self.read_when(&end_text) {
                    Some((t, true, _)) if all_day => t + 86_400,
                    Some((t, false, _)) if !all_day && t >= start => t,
                    // A time alone ("11:30") or a length ("1 hour") read on the start's day.
                    Some((t, false, _)) if !all_day => {
                        let day = 86_400;
                        let shifted = start - start.rem_euclid(day) + t.rem_euclid(day);
                        if shifted >= start {
                            shifted
                        } else {
                            start + 3600
                        }
                    }
                    _ if end_text.trim().is_empty() => {
                        if all_day {
                            start + 86_400
                        } else {
                            start + 3600
                        }
                    }
                    _ => {
                        self.calendar_error =
                            Some(t!("calendar-write-the-end-the-same-way").into());
                        cx.notify();
                        return;
                    }
                };
                let stored = self
                    .backend
                    .calendar_events()
                    .into_iter()
                    .find(|s| !uid.is_empty() && s.event.uid == uid);
                let location = value(&self.event_location, cx);
                let description = value(&self.event_notes, cx);
                let occurrence = self.event_occurrence.filter(|_| !self.event_all_repeats);
                if let (Some(at), Some(s)) = (occurrence, &stored)
                    && !s.event.recurrence.is_empty()
                {
                    // Only this repeat.
                    let changed = Event {
                        summary,
                        start,
                        end,
                        location,
                        description,
                        organizer: s.event.organizer.clone(),
                        attendees: s.event.attendees.clone(),
                        ..Default::default()
                    };
                    self.calendar_run(cx, move |b| {
                        b.save_occurrence(&uid, at, changed)
                            .map(|()| "This repeat was changed".into())
                    });
                    self.event_editing = None;
                    return;
                }
                let mut event = stored.as_ref().map(|s| s.event.clone()).unwrap_or_default();
                // Every repeat: a move by the time this one moved moves them all.
                let (start, end) = match (self.event_occurrence, &stored) {
                    (Some(at), Some(s)) if !s.event.recurrence.is_empty() => {
                        let shift = start - at;
                        (s.event.start + shift, s.event.start + shift + (end - start))
                    }
                    _ => (start, end),
                };
                event.uid = uid;
                event.summary = summary;
                event.start = start;
                event.end = end;
                event.all_day = all_day;
                if let Some(z) = zone.filter(|_| !all_day) {
                    event.utc = z == chrono_tz::UTC;
                    event.tzid = if event.utc {
                        String::new()
                    } else {
                        z.name().to_owned()
                    };
                }
                event.location = location;
                event.description = description;
                // The first reminder is the one chosen; others stay.
                match (self.event_alarm, event.alarms.is_empty()) {
                    (Some(a), true) => event.alarms.push(a),
                    (Some(a), false) => event.alarms[0] = a,
                    (None, _) => event.alarms.clear(),
                }
                event.recurrence = self.event_repeat.rule();
                if event.recurrence.is_empty() {
                    event.exdates.clear();
                }
                // Guests: invited by mail, from the account they're organized from.
                let guests: Vec<String> = value(&self.event_guests, cx)
                    .split([',', ';', '\n'])
                    .map(|g| g.trim().to_owned())
                    .filter(|g| g.contains('@'))
                    .collect();
                let organizer = self.organizer_account(&event);
                let invite = !guests.is_empty() && organizer.is_some();
                if !guests.is_empty() {
                    let Some(a) = &organizer else {
                        self.calendar_error = Some(
                            t!("calendar-add-an-email-account-first-invitations").into(),
                        );
                        cx.notify();
                        return;
                    };
                    if event.organizer.is_none() {
                        event.organizer = Some(ical::Attendee {
                            email: a.email.clone(),
                            name: a.name.clone(),
                            status: None,
                            ..Default::default()
                        });
                    }
                    let kept = event.attendees.clone();
                    event.attendees = guests
                        .iter()
                        .map(|g| {
                            kept.iter()
                                .find(|k| k.email.eq_ignore_ascii_case(g))
                                .cloned()
                                .unwrap_or(ical::Attendee {
                                    email: g.clone(),
                                    ..Default::default()
                                })
                        })
                        .collect();
                } else {
                    event.attendees.clear();
                }
                self.calendar_run(cx, move |b| {
                    let saved = b.save_event(event, None)?;
                    match organizer.filter(|_| invite) {
                        Some(a) => {
                            let n = b.send_invitations(&a, &saved.event.uid)?;
                            Ok(t!("calendar-event-saved-and-invited", n = n))
                        }
                        None => Ok(t!("calendar-event-saved").into()),
                    }
                });
                self.event_editing = None;
            }
            Action::DeleteEvent(uid) => {
                self.event_editing = None;
                if let Some(at) = self
                    .event_occurrence
                    .filter(|_| !self.event_all_repeats && self.event_repeat != Repeat::Never)
                {
                    self.calendar_run(cx, move |b| {
                        b.delete_occurrence(&uid, at)
                            .map(|()| "This repeat was deleted".into())
                    });
                    return;
                }
                // Guests of an event you organize are told it's off.
                let stored = self
                    .backend
                    .calendar_events()
                    .into_iter()
                    .find(|s| s.event.uid == uid);
                let organizer = stored.as_ref().and_then(|s| {
                    (!s.event.attendees.is_empty())
                        .then(|| self.organizer_account(&s.event))
                        .flatten()
                });
                self.calendar_run(cx, move |b| {
                    let told = match organizer {
                        Some(a) => b.cancel_invitations(&a, &uid).unwrap_or(0),
                        None => 0,
                    };
                    b.delete_event(&uid)?;
                    Ok(if told > 0 {
                        t!("calendar-event-deleted-told-its-cancelled", told = told)
                    } else {
                        t!("calendar-event-deleted").into()
                    })
                });
            }
            // Calendars from a mail account's provider, signed in as it is
            // (Google asks once for calendar access in the browser).
            Action::ConnectJmapCalendar(id) => {
                let Some(a) = self.accounts.iter().find(|a| a.id == id).cloned() else {
                    return;
                };
                let another = self.backend.primary_calendar_server().is_some();
                self.calendar_run(cx, move |b| {
                    let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
                    let n = if another {
                        b.add_calendar_source(|| b.use_mail_account_for_calendar(&a, cancel))?
                    } else {
                        b.use_mail_account_for_calendar(&a, cancel)?
                    };
                    Ok(t!("calendar-read-events-from", n = n, email = a.email))
                });
            }
            Action::SubscribeCalendarFeed => {
                let url = self.calendar_feed_url.read(cx).value().trim().to_owned();
                if url.is_empty() {
                    self.calendar_error = Some(t!("calendar-write-the-feeds-address").into());
                    cx.notify();
                    return;
                }
                self.calendar_feed_url.update(cx, |i, cx| i.set_text("", w, cx));
                let minutes = self.feed_interval;
                self.calendar_run(cx, move |b| {
                    let n = b.subscribe_calendar_feed(&url, "", minutes)?;
                    Ok(t!("calendar-read-events", n = n))
                });
            }
            Action::NewFeedInterval(minutes) => self.feed_interval = minutes,
            Action::FeedInterval(id, minutes) => {
                if let Err(e) = self.backend.set_feed_interval(&id, minutes) {
                    self.calendar_error = Some(format!("{e:#}"));
                }
            }
            Action::SyncCalendarSource(id) => {
                self.calendar_run(cx, move |b| {
                    let n = b.in_calendar_source(&id, || b.sync_calendar())?;
                    Ok(t!("calendar-read-events", n = n))
                });
            }
            Action::RemoveCalendarSource(id) => {
                self.calendar_run(cx, move |b| {
                    b.remove_calendar_source(&id)?;
                    Ok(t!("calendar-calendar-removed").into())
                });
            }
            Action::RenameCalendar(href) => {
                let name = self.collection_name.read(cx).value().trim().to_owned();
                if name.is_empty() {
                    self.calendar_error = Some(t!("calendar-write-the-new-name-in-the").into());
                    cx.notify();
                    return;
                }
                self.collection_name.update(cx, |i, cx| i.set_text("", w, cx));
                self.calendar_run(cx, move |b| {
                    b.rename_calendar(&href, &name, "")?;
                    Ok(t!("calendar-renamed-the-calendar", name = name))
                });
            }
            Action::AttachToEvent(uid) => {
                let picker = cx.prompt_for_paths(PathPromptOptions {
                    files: true,
                    directories: false,
                    multiple: false,
                    prompt: Some(t!("calendar-attach").into()),
                });
                cx.spawn(async move |this, cx| {
                    if let Ok(Ok(Some(paths))) = picker.await
                        && let Some(path) = paths.into_iter().next()
                    {
                        let _ = this.update(cx, |this, cx| {
                            this.calendar_run(cx, move |b| {
                                b.attach_to_event(&uid, &path)?;
                                Ok(t!("calendar-attached-the-file-to-the-event").into())
                            });
                        });
                    }
                })
                .detach();
            }
            Action::RemoveEventAttachment(uid, managed_id) => {
                self.calendar_run(cx, move |b| {
                    b.remove_event_attachment(&uid, &managed_id)?;
                    Ok(t!("calendar-removed-the-attachment").into())
                });
            }
            Action::DeleteCalendar(href) => {
                self.calendar_run(cx, move |b| {
                    b.delete_calendar(&href)?;
                    Ok(t!("calendar-deleted-the-calendar").into())
                });
            }
            Action::CreateCalendar => {
                let name = self.collection_name.read(cx).value().trim().to_owned();
                self.collection_name.update(cx, |i, cx| i.set_text("", w, cx));
                self.calendar_run(cx, move |b| {
                    let made = b.create_calendar(&name, "")?;
                    Ok(t!("calendar-made-the-calendar", name = made.name))
                });
            }
            Action::AddTask => {
                let name = self.collection_name.read(cx).value().trim().to_owned();
                // Due as written in the task editor's Due field; else none.
                let due_text = self.task_due.read(cx).value().trim().to_owned();
                let due = if due_text.is_empty() {
                    0
                } else {
                    match self.read_when(&due_text) {
                        Some((t, ..)) => t,
                        None => {
                            self.calendar_error = Some(t!("calendar-write-the-due-date-as-2026").into());
                            cx.notify();
                            return;
                        }
                    }
                };
                self.collection_name.update(cx, |i, cx| i.set_text("", w, cx));
                self.task_due.update(cx, |i, cx| i.set_text("", w, cx));
                self.calendar_run(cx, move |b| {
                    let t = b.add_task(&name, due)?;
                    Ok(t!("calendar-added-the-task", summary = t.event.summary))
                });
            }
            Action::CompleteTask(uid) => {
                if self.task_editing.as_ref() == Some(&uid) {
                    self.task_editing = None;
                }
                self.calendar_run(cx, move |b| b.complete_task(&uid, true).map(|()| "Task done".into()));
            }
            Action::EditTask(uid) => {
                let Some(t) = self.backend.calendar_tasks().into_iter().find(|t| t.event.uid == uid) else {
                    return;
                };
                let zone = find_zone(&t.event.tzid);
                let due = if t.event.start == 0 {
                    String::new()
                } else {
                    format_in(t.event.start, t.event.all_day, zone)
                };
                for (input, text) in [
                    (&self.task_title, t.event.summary.clone()),
                    (&self.task_due, due),
                    (&self.task_notes, t.event.description.clone()),
                ] {
                    input.update(cx, |i, cx| i.set_text(text, w, cx));
                }
                self.calendar_error = None;
                self.task_editing = Some(uid);
            }
            Action::CancelTaskEdit => {
                self.task_editing = None;
                self.task_due.update(cx, |i, cx| i.set_text("", w, cx));
                cx.notify();
            }
            Action::SaveTask => {
                let Some(uid) = self.task_editing.clone() else {
                    return;
                };
                let title = self.task_title.read(cx).value().trim().to_owned();
                let notes = self.task_notes.read(cx).value().to_string();
                let due_text = self.task_due.read(cx).value().trim().to_owned();
                let (due, all_day) = if due_text.is_empty() {
                    (0, false)
                } else {
                    // In the task's own zone, where it keeps one.
                    let zone = self
                        .backend
                        .calendar_tasks()
                        .into_iter()
                        .find(|t| t.event.uid == uid)
                        .and_then(|t| find_zone(&t.event.tzid));
                    match parse_in(&due_text, zone).or_else(|| self.read_when(&due_text).map(|(t, d, _)| (t, d))) {
                        Some(read) => read,
                        None => {
                            self.calendar_error = Some(t!("calendar-write-the-due-date-as-2026").into());
                            cx.notify();
                            return;
                        }
                    }
                };
                self.task_editing = None;
                self.task_due.update(cx, |i, cx| i.set_text("", w, cx));
                self.calendar_run(cx, move |b| {
                    b.edit_task(&uid, &title, &notes, due, all_day)
                        .map(|t| t!("calendar-saved-the-task", summary = t.event.summary))
                });
            }
            Action::DeleteTask(uid) => {
                self.task_editing = None;
                self.calendar_run(cx, move |b| b.delete_task(&uid).map(|()| "Task deleted".into()));
            }
            Action::ServerScheduling(on) => {
                self.calendar_run(cx, move |b| {
                    b.set_server_scheduling(on).map(|()| {
                        if on {
                            t!("calendar-the-calendar-server-now-sends-your").into()
                        } else {
                            t!("calendar-picacho-emails-your-invitations-again").into()
                        }
                    })
                });
            }
            Action::ResendInvitations(uid) => {
                let Some(stored) = self.backend.stored_event(&uid) else { return };
                let Some(a) = self.organizer_account(&stored.event) else {
                    self.calendar_error = Some(t!("calendar-add-an-email-account-first-invitations").into());
                    cx.notify();
                    return;
                };
                self.calendar_run(cx, move |b| {
                    let n = b.resend_invitations(&a, &uid)?;
                    Ok(t!("calendar-invitations-sent-again-to", n = n))
                });
            }
            Action::CheckFreeBusy => {
                let guests: Vec<String> = self
                    .event_guests
                    .read(cx)
                    .value()
                    .split([',', ';'])
                    .map(|g| g.trim().to_owned())
                    .filter(|g| g.contains('@'))
                    .collect();
                let around = self
                    .read_when(self.event_start.read(cx).value().trim())
                    .map(|(t, ..)| t)
                    .unwrap_or_else(|| chrono::Utc::now().timestamp());
                let Some(organizer) = self.accounts.first().map(|a| a.email.clone()) else {
                    return;
                };
                self.free_busy = Some(t!("calendar-looking").into());
                self.task(
                    cx,
                    move |b| {
                        let from = around - around.rem_euclid(86_400);
                        b.free_busy(&organizer, &guests, from, from + 86_400)
                    },
                    |this, result, cx| {
                        this.free_busy = Some(match result {
                            Ok(found) if found.iter().all(|f| f.busy.is_empty()) => {
                                t!("calendar-everyone-is-free-that-day").into()
                            }
                            Ok(found) => found
                                .iter()
                                .map(|f| {
                                    let times: Vec<String> = f
                                        .busy
                                        .iter()
                                        .map(|(s, e)| {
                                            let at = |t: i64| {
                                                chrono::DateTime::from_timestamp(t, 0)
                                                    .unwrap_or_default()
                                                    .with_timezone(&chrono::Local)
                                                    .local(t!("dates-calendar-time"))
                                            };
                                            format!("{}–{}", at(*s), at(*e))
                                        })
                                        .collect();
                                    if times.is_empty() {
                                        t!("calendar-is-free", attendee = f.attendee)
                                    } else {
                                        t!("calendar-is-busy", attendee = f.attendee, times = times.join(", "))
                                    }
                                })
                                .collect::<Vec<_>>()
                                .join(". "),
                            Err(e) => format!("{e:#}"),
                        });
                        cx.notify();
                    },
                );
            }
            Action::ListHeld(id) => {
                let Some(a) = self.accounts.iter().find(|a| a.id == id).cloned() else {
                    return;
                };
                self.task(
                    cx,
                    move |b| b.held_submissions(&a),
                    move |this, result, cx| {
                        match result {
                            Ok(held) => this.held = Some((id.clone(), held)),
                            Err(e) => this.notice(format!("{e:#}"), true, cx),
                        }
                        cx.notify();
                    },
                );
            }
            Action::CancelHeld(id, submission) => {
                let Some(a) = self.accounts.iter().find(|a| a.id == id).cloned() else {
                    return;
                };
                if let Some((_, held)) = self.held.as_mut() {
                    held.retain(|h| h.id != submission);
                }
                self.calendar_run(cx, move |b| {
                    Ok(if b.cancel_held(&a, &submission)? {
                        t!("calendar-taken-back-its-in-drafts").into()
                    } else {
                        t!("calendar-it-had-already-been-sent").into()
                    })
                });
            }
            _ => {}
        }
        cx.notify();
    }

    /// Shows reminders that are due, as notifications. Called with the
    /// mail checks' timer.
    pub(super) fn show_due_reminders(&mut self, cx: &mut Context<Self>) {
        if self.backend.calendar_server().is_none() {
            return;
        }
        self.task(
            cx,
            |b| Ok(b.due_reminders(chrono::Utc::now().timestamp())),
            |_, result: anyhow::Result<Vec<(Event, i64)>>, _| {
                for (e, _) in result.unwrap_or_default() {
                    let body = if e.location.is_empty() {
                        event_when(&e)
                    } else {
                        format!("{} · {}", event_when(&e), e.location)
                    };
                    crate::display::notify::reminder(&e.uid, &e.summary, &body);
                }
            },
        );
        // Subscribed feeds whose interval has passed.
        if self.backend.calendar_sources().iter().any(|(_, s, _)| s.feed) {
            self.task(cx, |b| Ok(b.sync_due_feeds()), |_, read: anyhow::Result<usize>, cx| {
                if read.is_ok_and(|n| n > 0) {
                    cx.notify();
                }
            });
        }
    }

    /// How often a feed can be read again: (minutes, words).
    fn feed_intervals() -> [(u32, &'static str); 4] {
        [
            (60, t!("calendar-every-hour")),
            (crate::sync::feed::DEFAULT_MINUTES, t!("calendar-every-6-hours")),
            (24 * 60, t!("calendar-every-day")),
            (7 * 24 * 60, t!("calendar-every-week")),
        ]
    }

    /// Settings → Calendar: the sources beside the first (other accounts'
    /// calendars, subscribed feeds), each to read again or remove, and
    /// where more are added.
    fn calendar_sources_group(&self, cx: &Context<Self>) -> Div {
        let c = self.colors();
        let busy = self.calendar_busy;
        let first = self.backend.primary_calendar_server().is_some();
        let mut rows = Rows::new();
        for (id, server, synced) in self.backend.calendar_sources() {
            let when = synced
                .and_then(|t| chrono::DateTime::from_timestamp(t, 0))
                .map(|t| t.with_timezone(&chrono::Local).local(t!("dates-month-day-time")))
                .unwrap_or_else(|| t!("calendar-never-read").to_owned());
            let kind = if server.feed {
                "feed"
            } else if !server.google_account.is_empty() {
                "google"
            } else if !server.proton_account.is_empty() {
                "proton"
            } else if !server.graph_account.is_empty() || !server.ews_account.is_empty() {
                "exchange"
            } else if !server.jmap_account.is_empty() {
                "jmap"
            } else {
                "caldav"
            };
            let caption = t!("calendar-source-caption", kind = kind, synced = when);
            let mut controls = div().flex().flex_wrap().items_center().gap_2();
            if server.feed {
                let every = if server.refresh_minutes == 0 { crate::sync::feed::DEFAULT_MINUTES } else { server.refresh_minutes };
                controls = controls.child(
                    self.segmented(
                        &format!("calendar-feed-every-{id}"),
                        Self::feed_intervals()
                            .into_iter()
                            .map(|(m, words)| (words.to_owned(), Action::FeedInterval(id.clone(), m), every == m))
                            .collect(),
                        cx,
                    )
                    .max_w(px(360.)),
                );
            }
            controls = controls
                .child(self.button(
                    SharedString::from(format!("calendar-source-sync-{id}")),
                    t!("calendar-read-now"),
                    "refresh",
                    Action::SyncCalendarSource(id.clone()),
                    false,
                    cx,
                ))
                .child(self.button(
                    SharedString::from(format!("calendar-source-remove-{id}")),
                    t!("calendar-remove"),
                    "close",
                    Action::RemoveCalendarSource(id.clone()),
                    false,
                    cx,
                ));
            rows = rows.row(self.setting_row(&self.backend.calendar_source_label(&id), &caption, controls));
        }
        // Another account, beside the first: as the first is set up.
        if first {
            let labelled = |text: &'static str, id: &'static str, input: &Entity<InputState>| {
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(div().text_size(px(12.)).text_color(rgb(c.muted)).child(text))
                    .child(div().id(id).debug_selector(move || id.into()).child(field(Input::new(input), c, busy)))
            };
            rows = rows.row(
                div()
                    .id("calendar-add-account")
                    .flex()
                    .flex_col()
                    .gap_3()
                    .max_w(px(420.))
                    .child(div().text_size(px(12.)).text_color(rgb(c.muted)).child(t!("calendar-add-another-account")))
                    .children(self.jmap_sources("calendar-add-jmap", Action::ConnectJmapCalendar, cx))
                    .child(labelled(t!("calendar-server"), "calendar-url", &self.calendar_url))
                    .child(labelled(t!("calendar-user-name"), "calendar-user", &self.calendar_user))
                    .child(labelled(t!("calendar-password"), "calendar-password", &self.calendar_password))
                    .child(div().flex().child(
                        self.button(
                            "calendar-connect",
                            if busy { t!("calendar-connecting") } else { t!("calendar-add-calendars") },
                            "plus",
                            Action::ConnectCalendar,
                            false,
                            cx,
                        )
                        .when(busy, |d| d.opacity(0.6)),
                    )),
            );
        }
        // A feed to subscribe to.
        rows = rows.row(
            div()
                .id("calendar-subscribe")
                .flex()
                .flex_col()
                .gap_2()
                .max_w(px(480.))
                .child(div().text_size(px(12.)).text_color(rgb(c.muted)).child(t!("calendar-subscribe-caption")))
                .child(div().id("calendar-feed-url").debug_selector(|| "calendar-feed-url".into()).child(field(
                    Input::new(&self.calendar_feed_url),
                    c,
                    busy,
                )))
                .child(self.segmented(
                    "calendar-feed-every",
                    Self::feed_intervals()
                        .into_iter()
                        .map(|(m, words)| (words.to_owned(), Action::NewFeedInterval(m), self.feed_interval == m))
                        .collect(),
                    cx,
                ))
                .child(div().flex().child(self.button(
                    "calendar-subscribe-button",
                    t!("calendar-subscribe"),
                    "plus",
                    Action::SubscribeCalendarFeed,
                    false,
                    cx,
                ))),
        );
        self.settings_group(t!("calendar-more-calendars"), rows)
    }

    /// The account an event you organize is sent from: the one whose
    /// address organizes it, or (for a new event) the first.
    /// The event's attached files (ATTACH); those the server keeps (RFC
    /// 8607 MANAGED-ID) can be removed here.
    fn event_attachments(&self, uid: &str, cx: &Context<Self>) -> Option<AnyElement> {
        let c = self.colors();
        let stored = self.backend.stored_event(uid).filter(|_| !uid.is_empty())?;
        let files = stored.event.attachments();
        if files.is_empty() {
            return None;
        }
        let mut list = div().id("event-attachments").flex().flex_col().gap_1();
        for (i, f) in files.into_iter().enumerate() {
            let name = if f.name.is_empty() { t!("calendar-attachment").to_owned() } else { f.name.clone() };
            let named = match f.size {
                Some(s) => t!("calendar-file-and-size", name = name, kb = s.div_ceil(1024)),
                None => name,
            };
            list = list.child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(icon_el("paperclip", 13., c.muted))
                    .child(div().flex_1().min_w_0().truncate().text_size(px(12.)).child(named))
                    .when(!f.managed_id.is_empty(), |d| {
                        d.child(self.button(
                            format!("event-attachment-remove-{i}"),
                            t!("calendar-remove"),
                            "",
                            Action::RemoveEventAttachment(uid.to_owned(), f.managed_id.clone()),
                            false,
                            cx,
                        ))
                    }),
            );
        }
        Some(list.into_any_element())
    }
    /// Under the guests: those the server says it couldn't reach (RFC 6638
    /// §7.3 SCHEDULE-STATUS 3.x or 5.x), and a way to send again.
    fn undelivered_guests(&self, uid: &str, cx: &Context<Self>) -> Option<AnyElement> {
        let c = self.colors();
        let stored = self.backend.stored_event(uid).filter(|_| !uid.is_empty())?;
        let failed: Vec<String> = stored
            .event
            .attendees
            .iter()
            .filter(|g| g.delivery_failed())
            .map(|g| if g.name.is_empty() { g.email.clone() } else { g.name.clone() })
            .collect();
        if failed.is_empty() {
            return None;
        }
        Some(
            div()
                .id("event-undelivered")
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .flex_1()
                        .text_size(px(11.5))
                        .text_color(rgb(c.danger))
                        .child(t!("calendar-the-server-couldnt-deliver-the-invitation", addresses = failed.join(", "))),
                )
                .child(self.button(
                    "event-resend",
                    t!("calendar-send-again"),
                    "send",
                    Action::ResendInvitations(uid.to_owned()),
                    false,
                    cx,
                ))
                .into_any_element(),
        )
    }
    fn organizer_account(&self, e: &Event) -> Option<Account> {
        match &e.organizer {
            Some(o) => self.accounts.iter().find(|a| {
                a.email.eq_ignore_ascii_case(&o.email)
                    || a.identities
                        .iter()
                        .any(|i| i.email.trim().eq_ignore_ascii_case(&o.email))
            }),
            None => self.accounts.first(),
        }
        .cloned()
    }

    /// A start or end as written: the strict format, else (with Settings →
    /// Local AI → Understand dates) words like "tomorrow at 10" or "every
    /// monday at 9". Returns the moment, whether it's a date, and a rule
    /// when the words repeat.
    fn read_when(&self, text: &str) -> Option<(i64, bool, Option<String>)> {
        let zone = self.event_zone_chosen;
        if let Some((t, all_day)) = parse_in(text, zone) {
            return Some((t, all_day, None));
        }
        let parser = self.when.as_ref()?;
        let read = match zone {
            Some(z) => parser.parse(text, &chrono::Utc::now().with_timezone(&z))?,
            None => parser.parse(text, &chrono::Local::now())?,
        };
        Some((read.at, false, read.rule))
    }

    #[allow(clippy::too_many_arguments)]
    fn edit_event(
        &mut self,
        uid: String,
        guests: String,
        title: &str,
        start: &str,
        end: &str,
        location: &str,
        notes: &str,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        for (input, text) in [
            (&self.event_title, title),
            (&self.event_start, start),
            (&self.event_end, end),
            (&self.event_location, location),
            (&self.event_notes, notes),
            (&self.event_guests, guests.as_str()),
        ] {
            input.update(cx, |i, cx| i.set_text(text.to_owned(), w, cx));
        }
        self.calendar_error = None;
        self.event_editing = Some(uid);
    }

    fn calendar_run(
        &mut self,
        cx: &mut Context<Self>,
        job: impl FnOnce(Arc<Backend>) -> anyhow::Result<String> + Send + 'static,
    ) {
        if self.calendar_busy {
            return;
        }
        self.calendar_busy = true;
        self.calendar_error = None;
        self.task(cx, job, |this, result, cx| {
            this.calendar_busy = false;
            match result {
                Ok(done) => this.notice(done, false, cx),
                Err(e) => this.calendar_error = Some(format!("{e:#}")),
            }
        });
    }

    /// Reads the calendar from its server once at startup, quietly.
    pub(super) fn sync_calendar_at_start(&mut self, cx: &mut Context<Self>) {
        if self.backend.calendar_server().is_some() {
            self.task(
                cx,
                |b| {
                    let _ = b.sync_calendar()?;
                    // Tasks and storage aren't worth failing the calendar for.
                    let _ = b.sync_tasks();
                    Ok(b.calendar_quota().ok().flatten())
                },
                |this, result, cx| {
                    if let Ok(quota) = result {
                        this.calendar_quota = quota;
                    }
                    cx.notify()
                },
            );
        }
    }

    /// Under the editor's time zone: what the start is here, when the zone
    /// chosen keeps another time.
    fn zone_note(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let c = self.colors();
        let zone = find_zone(&self.event_zone.read(cx).value())?;
        if same_as_here(zone) {
            return None;
        }
        let (start, all_day) = parse_in(&self.event_start.read(cx).value(), Some(zone))?;
        if all_day {
            return None;
        }
        let here = chrono::DateTime::from_timestamp(start, 0)?
            .with_timezone(&chrono::Local)
            .local(t!("dates-weekday-month-day-time"));
        Some(
            div()
                .id("event-zone-note")
                .debug_selector(|| "event-zone-note".into())
                .text_size(px(11.5))
                .text_color(rgb(c.muted))
                .child(t!("calendar-thats-here", here = here))
                .into_any_element(),
        )
    }

    /// Runs `job` for the invitation in message `id`, and says how it went.
    fn on_invitation(
        &mut self,
        id: &str,
        cx: &mut Context<Self>,
        job: impl FnOnce(&Backend, &Account, &Mail) -> anyhow::Result<String> + Send + 'static,
    ) {
        let Some(m) = self.messages.iter().find(|m| m.id == id).cloned() else {
            return;
        };
        let Some(a) = self.account_of(&m) else { return };
        self.task(
            cx,
            move |b| job(&b, &a, &m),
            |this, r, cx| match r {
                Ok(done) => this.notice(done, false, cx),
                Err(e) => this.notice(format!("{e:#}"), true, cx),
            },
        );
    }

    /// The guest's form for proposing another time.
    fn propose_form(&self, id: &str, cx: &Context<Self>) -> AnyElement {
        let c = self.colors();
        let labelled = |text: &'static str, el_id: &'static str, input: &Entity<InputState>| {
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(rgb(c.muted))
                        .child(text),
                )
                .child(div().id(el_id).child(field(Input::new(input), c, false)))
        };
        div()
            .id("invite-propose-form")
            .debug_selector(|| "invite-propose-form".into())
            .role(Role::Form)
            .aria_label(t!("calendar-propose-a-new-time-2"))
            .mt_2()
            .flex()
            .flex_col()
            .gap_2()
            .max_w(px(420.))
            .child(labelled(t!("calendar-starts"), "propose-start", &self.event_start))
            .child(labelled(t!("calendar-ends"), "propose-end", &self.event_end))
            .child(labelled(
                t!("calendar-note-to-the-organizer-optional"),
                "propose-note",
                &self.event_notes,
            ))
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(self.button(
                        "propose-send",
                        t!("calendar-send-proposal"),
                        "send",
                        Action::SendProposal(id.to_owned()),
                        true,
                        cx,
                    ))
                    .child(self.button(
                        "propose-cancel",
                        t!("calendar-cancel"),
                        "close",
                        Action::CancelProposal,
                        false,
                        cx,
                    )),
            )
            .into_any_element()
    }

    /// Buttons to use the calendars or contacts of each JMAP account whose
    /// server keeps them, instead of signing in to another server.
    pub(super) fn jmap_sources(
        &self,
        id: &'static str,
        action: fn(String) -> Action,
        cx: &Context<Self>,
    ) -> Option<AnyElement> {
        let c = self.colors();
        // Calendars and contacts are kept apart from mail; a mail account's
        // provider can serve them, as an option (see `dav_accounts`).
        let accounts: Vec<&Account> = self
            .accounts
            .iter()
            .filter(|a| crate::manage::backend::dav_accounts::MailProvider::of(a).is_some())
            .collect();
        if accounts.is_empty() {
            return None;
        }
        Some(
            div()
                .id(id)
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(rgb(c.muted))
                        .child(t!("calendar-or-use-your-email-providers-signed")),
                )
                .child(div().flex().flex_wrap().gap_2().children(accounts.into_iter().map(|a| {
                    self.button(
                        SharedString::from(format!("{id}-{}", a.id)),
                        &a.email,
                        "plus",
                        action(a.id.clone()),
                        false,
                        cx,
                    )
                })))
                .into_any_element(),
        )
    }

    pub(super) fn calendar_page(&self, cx: &Context<Self>) -> AnyElement {
        let c = self.colors();
        let busy = self.calendar_busy;
        let label = |text: &'static str| {
            div()
                .text_size(px(12.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(rgb(c.muted))
                .child(text)
        };
        let labelled = |text: &'static str, id: &'static str, input: &Entity<InputState>| {
            div().flex().flex_col().gap_1().child(label(text)).child(
                div().id(id).debug_selector(move || id.into()).child(field(
                    Input::new(input),
                    c,
                    busy,
                )),
            )
        };
        let Some(server) = self.backend.primary_calendar_server() else {
            let form = Rows::new()
                .row(
                    div()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .max_w(px(420.))
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(rgb(c.muted))
                                .child(t!("calendar-fastmail-icloud-nextcloud-radicale-mailbox-org")),
                        )
                        .children(self.jmap_sources("calendar-jmap", Action::ConnectJmapCalendar, cx))
                        .child(labelled(t!("calendar-server"), "calendar-url", &self.calendar_url))
                        .child(labelled(t!("calendar-user-name"), "calendar-user", &self.calendar_user))
                        .child(labelled(t!("calendar-password"), "calendar-password", &self.calendar_password))
                        .child(div().flex().child(
                            self.button(
                                "calendar-connect",
                                if busy { t!("calendar-connecting") } else { t!("calendar-connect") },
                                "plus",
                                Action::ConnectCalendar,
                                true,
                                cx,
                            )
                            .when(busy, |d| d.opacity(0.6)),
                        )),
                )
                .rows(self.calendar_error.clone().map(|e| {
                    div().id("calendar-error").text_size(px(12.)).text_color(rgb(c.danger)).child(e)
                }));
            return self
                .settings_column()
                .child(self.settings_group(t!("calendar-calendar-server-caldav"), form))
                .child(self.calendar_sources_group(cx))
                .into_any_element();
        };
        let synced = self
            .backend
            .calendar_synced()
            .and_then(|t| chrono::DateTime::from_timestamp(t, 0))
            .map(|t| {
                t.with_timezone(&chrono::Local)
                    .local(t!("dates-month-day-time"))
            })
            .unwrap_or_else(|| t!("calendar-never-read").to_owned());
        let over_jmap = !server.jmap_account.is_empty();
        let server_title = if over_jmap {
            t!("calendar-calendar-server-jmap")
        } else if !server.google_account.is_empty() {
            t!("calendar-calendars-google")
        } else if !server.proton_account.is_empty() {
            t!("calendar-calendars-proton")
        } else {
            t!("calendar-calendar-server-caldav")
        };
        let mut server_rows = Rows::new().row(
            self.setting_row(
                &server.url,
                &{
                    let account = self
                        .backend
                        .calendar_mail_account()
                        .and_then(|id| self.accounts.iter().find(|a| a.id == id))
                        .filter(|_| !over_jmap);
                    t!(
                        "calendar-read-read-again-each-time-picacho",
                        how = if over_jmap { "jmap" } else if account.is_some() { "account" } else { "password" },
                        user = server.user,
                        email = account.map(|a| a.email.as_str()).unwrap_or_default(),
                        synced = synced
                    )
                },
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        self.button(
                            "calendar-sync",
                            if busy { t!("calendar-reading") } else { t!("calendar-read-now") },
                            "refresh",
                            Action::SyncCalendar,
                            false,
                            cx,
                        )
                        .when(busy, |d| d.opacity(0.6)),
                    )
                    .child(self.button(
                        "calendar-disconnect",
                        t!("calendar-disconnect"),
                        "close",
                        Action::DisconnectCalendar,
                        false,
                        cx,
                    )),
            ),
        );
        // What the server said about shared calendars (JMAP).
        let notices = self.backend.calendar_notices();
        if !notices.is_empty() {
            server_rows = server_rows.row(
                self.setting(
                    t!("calendar-shared-calendars"),
                    &notices.join("\n"),
                    self.button(
                        "calendar-notices-dismiss",
                        t!("calendar-dismiss"),
                        "close",
                        Action::DismissCalendarNotices,
                        false,
                        cx,
                    ),
                ),
            );
        }
        let calendars: Vec<_> = self
            .backend
            .calendars()
            .into_iter()
            .filter(|c| c.writable)
            .collect();
        let default = self.backend.default_calendar().map(|c| c.href);
        if calendars.len() > 1 {
            server_rows = server_rows.row(
                self.setting(
                    t!("calendar-new-events-go-to"),
                    "",
                    self.segmented(
                        "calendar-default",
                        calendars
                            .iter()
                            .map(|cal| {
                                (
                                    cal.name.clone(),
                                    Action::DefaultCalendar(cal.href.clone()),
                                    default.as_ref() == Some(&cal.href),
                                )
                            })
                            .collect(),
                        cx,
                    )
                    .max_w(px(420.)),
                ),
            );
        }
        // Google sends them itself, always; Proton Calendar's are its own.
        if !over_jmap && server.google_account.is_empty() && server.proton_account.is_empty() {
            server_rows = server_rows.row(self.switch_setting(
                t!("calendar-let-the-server-send-invitations"),
                "",
                "calendar-server-scheduling".into(),
                server.server_schedules,
                Action::ServerScheduling(!server.server_schedules),
                cx,
            ));
        }
        server_rows = server_rows.rows(self.calendar_error.clone().map(|e| {
            div()
                .id("calendar-error")
                .text_size(px(12.))
                .text_color(rgb(c.danger))
                .child(e)
        }));
        // A new calendar, or a task, named in one field.
        server_rows = server_rows.row(
            self.setting(
                t!("calendar-new-calendar-or-task"),
                "",
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .id("collection-name")
                            .w(px(200.))
                            .child(field(Input::new(&self.collection_name), c, busy)),
                    )
                    .child(
                        div()
                            .id("task-new-due")
                            .w(px(180.))
                            .when(self.task_editing.is_none(), |d| {
                                d.child(field(Input::new(&self.task_due), c, busy))
                            }),
                    )
                    .child(self.button("calendar-create", t!("calendar-make-calendar"), "plus", Action::CreateCalendar, false, cx))
                    .child(self.button("task-add", t!("calendar-add-task"), "check", Action::AddTask, false, cx)),
            ),
        );
        // Each calendar, to rename (to the name in the field) or delete.
        let all = self.backend.calendars();
        if !all.is_empty() {
            let mut list = div().id("calendar-list").flex().flex_col().gap_2();
            for cal in all.iter().filter(|c| c.writable) {
                list = list.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(div().flex_1().min_w_0().truncate().text_size(px(13.)).child(cal.name.clone()))
                        .child(self.button(
                            format!("calendar-rename-{}", cal.href),
                            t!("calendar-rename"),
                            "",
                            Action::RenameCalendar(cal.href.clone()),
                            false,
                            cx,
                        ))
                        .child(
                            self.button(
                                format!("calendar-delete-{}", cal.href),
                                t!("calendar-delete"),
                                "",
                                Action::DeleteCalendar(cal.href.clone()),
                                false,
                                cx,
                            )
                            .text_color(rgb(c.danger)),
                        ),
                );
            }
            let storage = self.calendar_quota.map(|(used, left)| {
                let mb = |b: u64| b as f64 / 1_048_576.0;
                t!("calendar-the-server-keeps-mb-of-yours", used = format!("{:.0}", mb(used)), left = format!("{:.0}", mb(left)))
            });
            server_rows = server_rows.row(self.setting(
                t!("calendar-calendars"),
                &{
                    let rename = t!("calendar-rename-uses-the-name-written-in");
                    match storage {
                        Some(storage) => crate::i18n::sentences(rename, &storage),
                        None => rename.to_owned(),
                    }
                },
                list,
            ));
        }
        // Open tasks.
        let tasks = self.backend.calendar_tasks();
        let mut task_section = None;
        if !tasks.is_empty() {
            let count = tasks.len();
            task_section = Some(
                div()
                    .id("calendar-tasks")
                    .role(Role::List)
                    .aria_label(t!("calendar-tasks"))
                    .rounded(px(12.))
                    .border_1()
                    .border_color(rgb(c.border))
                    .bg(rgb(c.raised))
                    .overflow_hidden()
                    .flex()
                    .flex_col()
                    .children(tasks.into_iter().take(50).enumerate().map(|(i, t)| {
                        let due = if t.event.start == 0 {
                            t!("calendar-no-due-date").to_owned()
                        } else if t.event.all_day {
                            chrono::DateTime::from_timestamp(t.event.start, 0)
                                .map(|d| d.local(t!("dates-weekday-month-day")))
                                .unwrap_or_default()
                        } else {
                            chrono::DateTime::from_timestamp(t.event.start, 0)
                                .map(|d| d.with_timezone(&chrono::Local).local(t!("dates-weekday-month-day-time")))
                                .unwrap_or_default()
                        };
                        let overdue = t.event.start != 0 && t.event.start < chrono::Utc::now().timestamp();
                        div()
                            .id(SharedString::from(format!("task-{i}")))
                            .role(Role::ListItem)
                            .aria_label(t!("calendar-due", summary = t.event.summary, due = due))
                            .aria_position_in_set(i + 1)
                            .aria_size_of_set(count)
                            .px_4()
                            .py(px(10.))
                            .border_b_1()
                            .border_color(rgb(c.border))
                            .flex()
                            .items_center()
                            .gap_3()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .child(div().text_size(px(13.)).truncate().child(t.event.summary.clone()))
                                    .child(
                                        div()
                                            .text_size(px(11.5))
                                            .text_color(rgb(if overdue { c.danger } else { c.muted }))
                                            .child(if t.event.start == 0 {
                                                t!("calendar-no-due-date-2").to_owned()
                                            } else if overdue {
                                                t!("calendar-overdue", due = due)
                                            } else {
                                                t!("calendar-due-2", due = due)
                                            }),
                                    ),
                            )
                            .child(div().debug_selector(move || format!("task-edit-{i}")).child(self.button(
                                SharedString::from(format!("task-edit-{i}")),
                                t!("calendar-edit"),
                                "compose",
                                Action::EditTask(t.event.uid.clone()),
                                false,
                                cx,
                            )))
                            .child(self.button(
                                SharedString::from(format!("task-done-{i}")),
                                t!("calendar-done"),
                                "check",
                                Action::CompleteTask(t.event.uid.clone()),
                                false,
                                cx,
                            ))
                    })),
            );
        }
        // The task being edited.
        let mut task_editor = None;
        if let Some(uid) = &self.task_editing {
            task_editor = Some(
                div()
                    .id("task-editor")
                    .debug_selector(|| "task-editor".into())
                    .role(Role::Form)
                    .aria_label(t!("calendar-edit-task"))
                    .p_4()
                    .rounded(px(12.))
                    .border_1()
                    .border_color(rgb(c.border))
                    .bg(rgb(c.raised))
                    .flex()
                    .flex_col()
                    .gap_3()
                    .max_w(px(480.))
                    .child(div().text_size(px(14.)).font_weight(FontWeight::SEMIBOLD).child(t!("calendar-edit-task-2")))
                    .child(labelled(t!("calendar-task"), "task-title", &self.task_title))
                    .child(labelled(t!("calendar-due-2026-11-24-17-00"), "task-due", &self.task_due))
                    .child(labelled(t!("calendar-notes"), "task-notes", &self.task_notes))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap_2()
                            .child(self.button("task-save", t!("calendar-save"), "check", Action::SaveTask, true, cx))
                            .child(self.button("task-cancel", t!("calendar-cancel"), "close", Action::CancelTaskEdit, false, cx))
                            .child(self.button("task-delete", t!("calendar-delete"), "trash", Action::DeleteTask(uid.clone()), false, cx)),
                    ),
            );
        }
        // Messages JMAP servers hold to send later.
        let jmap_accounts: Vec<&Account> = self
            .accounts
            .iter()
            .filter(|a| a.provider == crate::manage::model::Provider::Jmap)
            .collect();
        let mut held_rows = Rows::new();
        if !jmap_accounts.is_empty() {

            for a in jmap_accounts {
                let listed = self.held.as_ref().filter(|(id, _)| *id == a.id);
                let detail = match listed {
                    None => t!("calendar-messages-handed-to-the-server-to").to_owned(),
                    Some((_, held)) if held.is_empty() => t!("calendar-nothing-is-waiting").to_owned(),
                    Some((_, held)) => t!("calendar-waiting", count = held.len()),
                };
                held_rows = held_rows.row(self.setting_row(
                    &a.email,
                    &detail,
                    self.button(
                        SharedString::from(format!("held-{}", a.id)),
                        t!("calendar-show"),
                        "refresh",
                        Action::ListHeld(a.id.clone()),
                        false,
                        cx,
                    ),
                ));
                for h in listed.map(|(_, h)| h.as_slice()).unwrap_or_default() {
                    let when = chrono::DateTime::from_timestamp(h.send_at, 0)
                        .map(|d| d.with_timezone(&chrono::Local).local(t!("dates-weekday-month-day-time")))
                        .unwrap_or_default();
                    held_rows = held_rows.row(self.setting_row(
                        if h.subject.is_empty() { t!("calendar-no-subject") } else { &h.subject },
                        &t!("calendar-goes", when = when),
                        self.button(
                            SharedString::from(format!("held-cancel-{}", h.id)),
                            t!("calendar-take-back"),
                            "undo",
                            Action::CancelHeld(a.id.clone(), h.id.clone()),
                            false,
                            cx,
                        ),
                    ));
                }
            }
        }
        // The event being added or edited.
        let mut event_editor = None;
        if let Some(uid) = &self.event_editing {
            event_editor = Some(
                div()
                    .id("event-editor")
                    .role(Role::Form)
                    .aria_label(if uid.is_empty() {
                        t!("calendar-new-event")
                    } else {
                        t!("calendar-edit-event")
                    })
                    .p_4()
                    .rounded(px(12.))
                    .border_1()
                    .border_color(rgb(c.border))
                    .bg(rgb(c.raised))
                    .flex()
                    .flex_col()
                    .gap_3()
                    .max_w(px(480.))
                    .child(
                        div()
                            .text_size(px(14.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(if uid.is_empty() {
                                t!("calendar-new-event-2")
                            } else {
                                t!("calendar-edit-event-2")
                            }),
                    )
                    .child(labelled(t!("calendar-title"), "event-title", &self.event_title))
                    .child(labelled(
                        t!("calendar-starts-2026-11-24-10-00"),
                        "event-start",
                        &self.event_start,
                    ))
                    .child(labelled(t!("calendar-ends-empty-an-hour-later"), "event-end", &self.event_end))
                    .child(labelled(t!("calendar-time-zone-the-times-above-are"), "event-zone", &self.event_zone))
                    .children(self.zone_note(cx))
                    .child(labelled(t!("calendar-where"), "event-location", &self.event_location))
                    .child(labelled(t!("calendar-notes"), "event-notes", &self.event_notes))
                    .child(labelled(t!("calendar-guests-invited-by-mail-separated-by"), "event-guests", &self.event_guests))
                    .children(self.undelivered_guests(uid, cx))
                    .children(self.event_attachments(uid, cx))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(self.button(
                                "event-free-busy",
                                t!("calendar-when-are-they-free"),
                                "calendar",
                                Action::CheckFreeBusy,
                                false,
                                cx,
                            ))
                            .children(self.free_busy.clone().map(|text| {
                                div()
                                    .id("event-free-busy-result")
                                    .flex_1()
                                    .text_size(px(11.5))
                                    .text_color(rgb(c.muted))
                                    .child(text)
                            })),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(div().text_size(px(12.)).font_weight(FontWeight::MEDIUM).text_color(rgb(c.muted)).child(t!("calendar-remind-me")))
                            .child(self.segmented(
                                "event-alarm",
                                [
                                    (t!("calendar-never"), None),
                                    (t!("calendar-10-minutes-before"), Some(600)),
                                    (t!("calendar-an-hour-before"), Some(3600)),
                                    (t!("calendar-a-day-before"), Some(86_400)),
                                ]
                                .into_iter()
                                .map(|(label, a)| (label.to_owned(), Action::EventAlarm(a), self.event_alarm == a))
                                .collect(),
                                cx,
                            )),
                    )
                    .when(self.event_occurrence.is_some() && self.event_repeat != Repeat::Never, |d| {
                        d.child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .child(div().text_size(px(12.)).font_weight(FontWeight::MEDIUM).text_color(rgb(c.muted)).child(t!("calendar-this-event-repeats-change")))
                                .child(self.segmented(
                                    "event-scope",
                                    vec![
                                        (t!("calendar-only-this-one").into(), Action::EventScope(false), !self.event_all_repeats),
                                        (t!("calendar-every-repeat").into(), Action::EventScope(true), self.event_all_repeats),
                                    ],
                                    cx,
                                )),
                        )
                    })
                    .when(self.event_occurrence.is_none() || self.event_all_repeats || self.event_repeat == Repeat::Never, |d| {
                        let mut choices = vec![
                            (t!("calendar-never").to_owned(), Repeat::Never),
                            (t!("calendar-daily").to_owned(), Repeat::Daily),
                            (t!("calendar-weekly").to_owned(), Repeat::Weekly),
                            (t!("calendar-monthly").to_owned(), Repeat::Monthly),
                            (t!("calendar-yearly").to_owned(), Repeat::Yearly),
                        ];
                        if let Repeat::Kept(rule) = &self.event_repeat {
                            let text = ical::Rule::parse(rule).map(|r| r.describe()).unwrap_or_else(|| t!("calendar-as-it-was").into());
                            choices.push((text, self.event_repeat.clone()));
                        }
                        d.child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .child(div().text_size(px(12.)).font_weight(FontWeight::MEDIUM).text_color(rgb(c.muted)).child(t!("calendar-repeats")))
                                .child(self.segmented(
                                    "event-repeat",
                                    choices
                                        .into_iter()
                                        .map(|(label, r)| {
                                            let on = self.event_repeat == r;
                                            (label, Action::EventRepeat(r), on)
                                        })
                                        .collect(),
                                    cx,
                                ))
                                .child(self.custom_repeat_form(cx)),
                        )
                    })
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap_2()
                            .child(self.button(
                                "event-save",
                                t!("calendar-save"),
                                "check",
                                Action::SaveEvent,
                                true,
                                cx,
                            ))
                            .child(self.button(
                                "event-cancel",
                                t!("calendar-cancel"),
                                "close",
                                Action::CancelEventEdit,
                                false,
                                cx,
                            ))
                            .when(!uid.is_empty(), |d| {
                                d.child(self.button(
                                    "event-delete",
                                    t!("calendar-delete"),
                                    "trash",
                                    Action::DeleteEvent(uid.clone()),
                                    false,
                                    cx,
                                ))
                            })
                            // Kept by the calendar server (RFC 8607), on
                            // CalDAV; the server says whether it can.
                            .when(!uid.is_empty() && !over_jmap, |d| {
                                d.child(self.button(
                                    "event-attach",
                                    t!("calendar-attach-file"),
                                    "paperclip",
                                    Action::AttachToEvent(uid.clone()),
                                    false,
                                    cx,
                                ))
                            }),
                    ),
            );
        }
        // Day, Week and Month grids, in place of the agenda.
        let mut body = div().flex().flex_col().gap_3().children(self.calendar_grid(cx));
        if self.cal.view != super::calendar_views::CalendarView::Agenda {
            body = body
                .when(default.is_some() && self.event_editing.is_none(), |d| {
                    d.child(div().flex().child(self.button(
                        "event-new",
                        t!("calendar-new-event-2"),
                        "plus",
                        Action::NewEvent,
                        false,
                        cx,
                    )))
                });
        } else {
        // Coming events.
        let now = chrono::Utc::now().timestamp();
        let coming: Vec<Event> = self
            .backend
            .agenda(now, now + 366 * 86_400)
            .into_iter()
            .take(60)
            .collect();
        body = body.child(
            div()
                .flex()
                .items_center()
                .child(div().flex_1().child(self.section(t!("calendar-coming-up"))))
                .when(default.is_some() && self.event_editing.is_none(), |d| {
                    d.child(self.button(
                        "event-new",
                        t!("calendar-new-event-2"),
                        "plus",
                        Action::NewEvent,
                        false,
                        cx,
                    ))
                }),
        );
        let list = div()
            .id("calendar-events")
            .role(Role::List)
            .aria_label(t!("calendar-coming-events"))
            .rounded(px(12.))
            .border_1()
            .border_color(rgb(c.border))
            .bg(rgb(c.raised))
            .overflow_hidden()
            .flex()
            .flex_col();
        let list = if coming.is_empty() {
            list.child(
                div()
                    .px_4()
                    .py_3()
                    .text_size(px(12.))
                    .text_color(rgb(c.muted))
                    .child(t!("calendar-nothing-coming-up")),
            )
        } else {
            let count = coming.len();
            list.children(coming.into_iter().enumerate().map(|(i, e)| {
                let when = event_when(&e);
                let spoken = format!(
                    "{}, {when}{}",
                    e.summary,
                    if e.location.is_empty() {
                        String::new()
                    } else {
                        format!(", {}", e.location)
                    }
                );
                div()
                    .id(SharedString::from(format!("event-{i}")))
                    .role(Role::ListItem)
                    .aria_label(spoken)
                    .aria_position_in_set(i + 1)
                    .aria_size_of_set(count)
                    .focusable()
                    .tab_stop(true)
                    .focus_visible(move |s| s.border_2().border_color(rgb(c.accent)))
                    .px_4()
                    .py_3()
                    .border_b_1()
                    .border_color(rgb(c.border))
                    .cursor_pointer()
                    .hover(move |s| s.bg(rgb(c.surface)))
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_weight(FontWeight::MEDIUM)
                            .truncate()
                            .child(e.summary.clone()),
                    )
                    .child(
                        div()
                            .text_size(px(11.5))
                            .text_color(rgb(c.muted))
                            .truncate()
                            .child(if e.location.is_empty() {
                                when
                            } else {
                                format!("{when} · {}", e.location)
                            }),
                    )
                    .on_click({
                        let uid = e.uid.clone();
                        let occurrence = e.recurrence_id;
                        cx.listener(move |this, _, w, cx| {
                            this.handle(Action::EditEvent(uid.clone(), occurrence), w, cx)
                        })
                    })
            }))
        };
        body = body.child(list);
        }
        self.settings_column()
            .children(event_editor)
            .child(body)
            .when(task_section.is_some() || task_editor.is_some(), |d| {
                d.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(self.section(t!("calendar-tasks")))
                        .children(task_section)
                        .children(task_editor),
                )
            })
            .child(self.settings_group(server_title, server_rows))
            .child(self.calendar_sources_group(cx))
            .child(self.settings_group(t!("calendar-held-to-send-later"), held_rows))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::prelude::v1::test;

    #[test]
    fn times_are_written_and_read_back() {
        let (t, all_day) = parse_local("2026-11-24 10:00").unwrap();
        assert!(!all_day);
        assert_eq!(format_local(t, false), "2026-11-24 10:00");
        let (d, all_day) = parse_local("2026-12-25").unwrap();
        assert!(all_day);
        assert_eq!(format_local(d, true), "2026-12-25");
        assert!(parse_local("tomorrow").is_none());
        assert_eq!(
            rule_without_times("FREQ=WEEKLY;BYDAY=MO;BYHOUR=9;BYMINUTE=30"),
            "FREQ=WEEKLY;BYDAY=MO"
        );
        assert_eq!(Repeat::of("FREQ=WEEKLY"), Repeat::Weekly);
        assert_eq!(
            Repeat::of(" freq=weekly;BYDAY=MO"),
            Repeat::Kept("freq=weekly;BYDAY=MO".into())
        );
        assert_eq!(find_zone("berlin"), Some(chrono_tz::Europe::Berlin));
        assert_eq!(find_zone("New York"), Some(chrono_tz::America::New_York));
        assert!(find_zone("nowhere").is_none());
        let (t, _) = parse_in("2026-11-24 10:00", Some(chrono_tz::Asia::Tokyo)).unwrap();
        // 10:00 in Tokyo is 01:00 UTC.
        assert_eq!(t, 1_795_482_000);
        assert_eq!(
            format_in(t, false, Some(chrono_tz::Asia::Tokyo)),
            "2026-11-24 10:00"
        );
        let e = Event {
            start: d,
            end: d + 86_400,
            all_day: true,
            ..Default::default()
        };
        assert!(event_when(&e).ends_with("all day"));
    }

    use super::super::tests::{act, frame, two_accounts};

    #[gpui::test]
    fn new_settings_pages_and_the_invitation_card_draw(cx: &mut gpui::TestAppContext) {
        let (view, cx, _) = two_accounts(cx);
        act(&view, Action::Settings, cx);
        for page in [
            Page::Contacts,
            Page::Calendar,
            Page::Encryption,
            Page::General,
        ] {
            act(&view, Action::SettingsPage(page), cx);
            frame(cx);
        }
        // Contacts: added here, without a server.
        act(&view, Action::SettingsPage(Page::Contacts), cx);
        act(&view, Action::NewContact, cx);
        frame(cx);
        assert!(cx.debug_bounds("contact-save").is_some());
        view.update_in(cx, |this, w, cx| {
            this.contact_name
                .update(cx, |i, cx| i.set_text("Rae Park", w, cx));
            this.contact_emails.update(cx, |i, cx| {
                i.set_text("rae@example.com, rae@work.example", w, cx)
            });
        });
        act(&view, Action::SaveContact, cx);
        let cards = view.read_with(cx, |this, _| this.backend.contacts());
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].emails, ["rae@example.com", "rae@work.example"]);
        act(&view, Action::EditContact(cards[0].uid.clone()), cx);
        view.read_with(cx, |this, cx| {
            assert_eq!(this.contact_name.read(cx).value().to_string(), "Rae Park");
        });
        act(&view, Action::RemoveContact(cards[0].uid.clone()), cx);
        assert!(view.read_with(cx, |this, _| this.backend.contacts().is_empty()));
        // Reduce motion: chosen, and kept.
        act(
            &view,
            Action::SetMotion(super::super::motion::Motion::Reduced),
            cx,
        );
        assert!(cx.update(|_, cx| cx.reduce_motion()));
        act(
            &view,
            Action::SetMotion(super::super::motion::Motion::Full),
            cx,
        );
        assert!(!cx.update(|_, cx| cx.reduce_motion()));
        act(&view, Action::Close, cx);
        // An invitation shows its card, with answers for the invited.
        let (id, me) = view.read_with(cx, |this, _| {
            (this.messages[0].id.clone(), this.accounts[0].email.clone())
        });
        let event = ical::parse(&format!(
            "BEGIN:VCALENDAR\nMETHOD:REQUEST\nBEGIN:VEVENT\nUID:inv-1\nDTSTART:20261124T090000Z\nDTEND:20261124T100000Z\nSUMMARY:Planning\nLOCATION:Room 4\nORGANIZER;CN=Ana:mailto:ana@example.org\nATTENDEE:mailto:{me}\nEND:VEVENT\nEND:VCALENDAR\n"
        ))
        .events
        .remove(0);
        view.update(cx, |this, cx| {
            this.invitations.insert(
                id.clone(),
                Some(Invitation {
                    method: "REQUEST".into(),
                    event,
                }),
            );
            cx.notify();
        });
        act(&view, Action::Select(id), cx);
        frame(cx);
        assert!(cx.debug_bounds("invitation").is_some());
        assert!(cx.debug_bounds("invite-accepted").is_some());
        assert!(cx.debug_bounds("invite-declined").is_some());
        // A guest can propose another time.
        let id = view.read_with(cx, |this, _| this.selected.clone().unwrap());
        act(&view, Action::ProposeTime(id.clone()), cx);
        frame(cx);
        assert!(cx.debug_bounds("propose-send").is_some());
        view.read_with(cx, |this, cx| {
            assert_eq!(this.proposing.as_deref(), Some(id.as_str()));
            assert!(
                !this.event_start.read(cx).value().is_empty(),
                "the invitation's time is filled in"
            );
        });
        act(&view, Action::CancelProposal, cx);
        // An event's editor has its time zone; one elsewhere says what time it is here.
        act(&view, Action::Settings, cx);
        view.update(cx, |this, _| {
            // A calendar server, as far as the page is concerned.
            this.backend
                .store
                .set_preference(
                    "calendar_server",
                    r#"{"url":"https://dav.example","user":"qa"}"#,
                )
                .unwrap();
        });
        act(&view, Action::SettingsPage(Page::Calendar), cx);
        view.update_in(cx, |this, w, cx| {
            this.event_editing = Some(String::new());
            this.event_zone
                .update(cx, |i, cx| i.set_text("Pacific/Kiritimati", w, cx));
            this.event_start
                .update(cx, |i, cx| i.set_text("2026-11-24 10:00", w, cx));
        });
        frame(cx);
        assert!(cx.debug_bounds("event-zone").is_some());
        // Tasks: listed with Edit, opened in their own editor; the server
        // scheduling switch is offered for CalDAV.
        assert!(cx.debug_bounds("calendar-server-scheduling").is_some());
        view.update(cx, |this, _| {
            this.backend
                .store
                .set_preference(
                    "calendar_tasks",
                    r#"[{"event":{"uid":"t1","summary":"File taxes","location":"","description":"Receipts in the drawer","start":0,"end":0,"all_day":false,"organizer":null,"attendees":[],"sequence":0,"recurrence":"","cancelled":false,"task":true},"calendar":"https://dav.example/cal/","href":"https://dav.example/cal/t1.ics","etag":""}]"#,
                )
                .unwrap();
        });
        frame(cx);
        assert!(cx.debug_bounds("task-edit-0").is_some());
        act(&view, Action::EditTask("t1".into()), cx);
        frame(cx);
        assert!(cx.debug_bounds("task-editor").is_some());
        view.read_with(cx, |this, cx| {
            assert_eq!(this.task_title.read(cx).value().to_string(), "File taxes");
            assert!(this.task_due.read(cx).value().is_empty(), "no due date");
            assert_eq!(this.task_notes.read(cx).value().to_string(), "Receipts in the drawer");
        });
        act(&view, Action::CancelTaskEdit, cx);
        frame(cx);
        assert!(cx.debug_bounds("task-editor").is_none());
        assert!(
            cx.debug_bounds("event-zone-note").is_some(),
            "a zone 14 hours ahead differs from here"
        );
        // Checking a key: its fingerprint and a field for theirs.
        let bo = crate::manage::openpgp::test_key("Bo <bo@example.se>", "");
        let fp = view.update(cx, |this, _| {
            use pgp::ser::Serialize as _;
            let mut ring = crate::manage::openpgp::Keyring::default();
            ring.add_found(
                "bo@example.se",
                &pgp::composed::SignedPublicKey::from(bo).to_bytes().unwrap(),
            )
            .unwrap();
            let fp = ring.list()[0].fingerprint.clone();
            this.backend.set_keyring(ring).unwrap();
            fp
        });
        act(&view, Action::SettingsPage(Page::Encryption), cx);
        act(&view, Action::CheckKey(fp.clone()), cx);
        frame(cx);
        assert!(cx.debug_bounds("openpgp-check-panel").is_some());
        view.update_in(cx, |this, w, cx| {
            this.key_check_input
                .update(cx, |i, cx| i.set_text(fp.clone(), w, cx))
        });
        act(&view, Action::MatchFingerprint(fp.clone()), cx);
        assert!(view.read_with(cx, |this, _| this.backend.keyring().list()[0].verified));
        // Autocrypt's prefer-encrypt=mutual: a switch per account with a
        // key, off until turned on (Autocrypt 1.1 §2.2).
        let (id, me) = view.update(cx, |this, _| {
            let a = this.accounts[0].clone();
            let mut ring = crate::manage::openpgp::Keyring::clone(&this.backend.keyring());
            ring.secret.push(
                crate::manage::openpgp::test_key(&format!("Me <{}>", a.email), "")
                    .to_armored_string(Default::default())
                    .unwrap(),
            );
            this.backend.set_keyring(ring).unwrap();
            (a.id, a.email)
        });
        frame(cx);
        let switch: &'static str = format!("autocrypt-mutual-{id}").leak();
        assert!(cx.debug_bounds(switch).is_some());
        assert!(!view.read_with(cx, |this, _| this.backend.keyring().prefers_mutual(&me)));
        act(&view, Action::ToggleAutocryptMutual(me.clone()), cx);
        assert!(view.read_with(cx, |this, _| this.backend.keyring().prefers_mutual(&me)));
        act(&view, Action::ToggleAutocryptMutual(me.clone()), cx);
        assert!(!view.read_with(cx, |this, _| this.backend.keyring().prefers_mutual(&me)));
    }
}

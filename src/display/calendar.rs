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
        text.push_str(&format!(
            " ({} – {} in {})",
            at(e.start).format("%H:%M"),
            at(e.end).format("%H:%M"),
            zone.name()
        ));
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
            format!(
                "{} – {}, all day",
                start.format("%a, %b %-d"),
                last.format("%a, %b %-d")
            )
        } else {
            format!("{}, all day", start.format("%a, %b %-d %Y"))
        }
    } else if start.date_naive() == end.date_naive() {
        format!(
            "{} – {}",
            start.format("%a, %b %-d %Y, %H:%M"),
            end.format("%H:%M")
        )
    } else {
        format!(
            "{} – {}",
            start.format("%a, %b %-d, %H:%M"),
            end.format("%a, %b %-d, %H:%M")
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
    fn of(rule: &str) -> Self {
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
            "CANCEL" => "Cancelled",
            "REPLY" => "Answer to your invitation",
            "COUNTER" => "New time proposed",
            "DECLINECOUNTER" => "Your proposed time wasn't taken",
            "REFRESH" => "A guest asks for the latest version",
            "ADD" => "More times added",
            "PUBLISH" | "" => "Event",
            _ if changed.is_some() => "Updated invitation",
            _ => "Invitation",
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
                    format!("{name}, optional")
                } else if kind.eq_ignore_ascii_case("ROOM") || kind.eq_ignore_ascii_case("RESOURCE") {
                    format!("{name}, {}", kind.to_ascii_lowercase())
                } else {
                    name
                };
                match at.status {
                    Some(PartStat::Accepted) => format!("{name} (yes)"),
                    Some(PartStat::Declined) => format!("{name} (no)"),
                    Some(PartStat::Tentative) => format!("{name} (maybe)"),
                    Some(PartStat::Delegated) => format!("{name} (delegated)"),
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
                            "(No title)".to_owned()
                        } else {
                            e.summary.clone()
                        }),
                )
                .child(row("When", event_when(e)))
                .when(!e.recurrence.is_empty(), |d| {
                    d.child(row(
                        "Repeats",
                        match ical::Rule::parse(&e.recurrence) {
                            Some(r) if ical::Rule::understood(&e.recurrence) => r.describe(),
                            _ if ical::Rule::expandable(&e.recurrence) => {
                                "Yes, on a schedule of its own (each time is listed in your calendar)".into()
                            }
                            _ => "Yes".into(),
                        },
                    ))
                })
                .when(!e.location.is_empty(), |d| {
                    d.child(row("Where", e.location.clone()))
                })
                .children(e.details().into_iter().map(|(label, text)| row(label, text)))
                .when_some(organizer, |d, o| d.child(row("Organizer", o)))
                .when(!people.is_empty(), |d| d.child(row("Guests", people)))
                .when(invited, |d| {
                    d.child(
                        div()
                            .mt_2()
                            .flex()
                            .flex_wrap()
                            .gap_2()
                            .child(answer("Accept", "check", PartStat::Accepted))
                            .child(answer("Maybe", "info", PartStat::Tentative))
                            .child(answer("Decline", "close", PartStat::Declined)),
                    )
                })
                .when(
                    e.cancelled && self.backend.calendar_server().is_some(),
                    |d| {
                        d.child(div().mt_2().flex().child(self.button(
                            "invite-remove",
                            "Remove from Calendar",
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
                            "Add to Calendar",
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
                        what.push(format!("was {}", event_when(&before.event)));
                    }
                    if before.event.location != e.location && !before.event.location.is_empty() {
                        what.push(format!("was at {}", before.event.location));
                    }
                    d.child(
                        div()
                            .text_size(px(11.5))
                            .text_color(rgb(c.muted))
                            .child(if what.is_empty() {
                                "Changed since it's in your calendar.".to_owned()
                            } else {
                                format!("Changed: {}.", what.join("; "))
                            }),
                    )
                    .child(div().flex().child(self.button(
                        "invite-update",
                        "Update My Calendar",
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
                        "Propose a New Time…",
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
                        "Note Their Answer",
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
                            .child(format!("Now: {before}")),
                    )
                    .child(
                        div()
                            .mt_2()
                            .flex()
                            .gap_2()
                            .child(self.button(
                                "counter-accept",
                                "Move the Event",
                                "check",
                                Action::AcceptCounter(m.id.clone()),
                                true,
                                cx,
                            ))
                            .child(self.button(
                                "counter-decline",
                                "Keep the Time",
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
                        "Send It Again",
                        "send",
                        Action::AnswerRefresh(m.id.clone()),
                        true,
                        cx,
                    )))
                })
                .when(inv.method == "ADD" && self.backend.stored_event(&e.uid).is_some(), |d| {
                    d.child(div().mt_2().flex().child(self.button(
                        "invite-add",
                        "Add to Calendar",
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
                        "Note",
                        if e.comment.is_empty() { e.description.clone() } else { e.comment.clone() },
                    ))
                })
                .when_some(answered.filter(|_| invited), |d, s| {
                    d.child(
                        div()
                            .text_size(px(11.5))
                            .text_color(rgb(c.muted))
                            .child(format!("You answered: {}", s.verb())),
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
                        format!("{} — the organizer was told", status.verb()),
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
                        Ok(()) => this.notice("Removed from your calendar", false, cx),
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
                    self.notice("Write the times as 2026-11-24 10:00.", true, cx);
                    return;
                };
                let note = value(&self.event_notes, cx);
                self.proposing = None;
                self.task(
                    cx,
                    move |b| b.propose_time(&a, &m, start, end, &note),
                    |this, r, cx| match r {
                        Ok(()) => {
                            this.notice("Your proposed time was sent to the organizer", false, cx)
                        }
                        Err(e) => this.notice(format!("{e:#}"), true, cx),
                    },
                );
            }
            Action::ApplyReply(id) => self.on_invitation(&id, cx, |b, a, m| b.apply_reply(a, m)),
            Action::AcceptCounter(id) => self.on_invitation(&id, cx, |b, a, m| {
                b.accept_counter(a, m).map(|n| {
                    format!(
                        "The event moved; {n} {} told",
                        if n == 1 { "guest was" } else { "guests were" }
                    )
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
                self.task(
                    cx,
                    move |b| b.save_event(event, None),
                    |this, r, cx| match r {
                        Ok(_) => this.notice("Added to your calendar", false, cx),
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
                self.calendar_run(cx, move |b| {
                    b.connect_calendar(server, &password)
                        .map(|n| format!("Read {n} events"))
                });
            }
            Action::SyncCalendar => {
                self.calendar_run(cx, |b| {
                    let _ = b.sync_tasks();
                    b.sync_calendar().map(|n| format!("Read {n} events"))
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
                    self.calendar_error = Some(format!(
                        "“{}” isn't a time zone Picacho knows. Write it as Europe/Berlin or America/New_York, or a city such as Tokyo.",
                        zone_text.trim()
                    ));
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
                            "Picacho couldn't tell when it starts. Try “tomorrow at 10”, or 2026-11-24 10:00.".into()
                        } else {
                            "Write the start as 2026-11-24 10:00, or 2026-11-24 for all day. (Settings → Local AI → Understand dates reads “tomorrow at 10” too.)".into()
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
                            Some("Write the end the same way as the start, or leave it empty for an hour.".into());
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
                            "Add an email account first: invitations are sent from it.".into(),
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
                            Ok(format!(
                                "Event saved, and {n} {} invited",
                                if n == 1 { "guest" } else { "guests" }
                            ))
                        }
                        None => Ok("Event saved".into()),
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
                        format!("Event deleted; {told} told it's cancelled")
                    } else {
                        "Event deleted".into()
                    })
                });
            }
            Action::ConnectJmapCalendar(id) => {
                let Some(a) = self.accounts.iter().find(|a| a.id == id).cloned() else {
                    return;
                };
                self.calendar_run(cx, move |b| {
                    let n = b.connect_jmap_calendar(&a)?;
                    Ok(format!("Read {n} events from {}", a.email))
                });
            }
            Action::RenameCalendar(href) => {
                let name = self.collection_name.read(cx).value().trim().to_owned();
                if name.is_empty() {
                    self.calendar_error = Some("Write the new name in the field first.".into());
                    cx.notify();
                    return;
                }
                self.collection_name.update(cx, |i, cx| i.set_text("", w, cx));
                self.calendar_run(cx, move |b| {
                    b.rename_calendar(&href, &name, "")?;
                    Ok(format!("Renamed the calendar {name}"))
                });
            }
            Action::AttachToEvent(uid) => {
                let picker = cx.prompt_for_paths(PathPromptOptions {
                    files: true,
                    directories: false,
                    multiple: false,
                    prompt: Some("Attach".into()),
                });
                cx.spawn(async move |this, cx| {
                    if let Ok(Ok(Some(paths))) = picker.await
                        && let Some(path) = paths.into_iter().next()
                    {
                        let _ = this.update(cx, |this, cx| {
                            this.calendar_run(cx, move |b| {
                                b.attach_to_event(&uid, &path)?;
                                Ok("Attached the file to the event".into())
                            });
                        });
                    }
                })
                .detach();
            }
            Action::DeleteCalendar(href) => {
                self.calendar_run(cx, move |b| {
                    b.delete_calendar(&href)?;
                    Ok("Deleted the calendar".into())
                });
            }
            Action::CreateCalendar => {
                let name = self.collection_name.read(cx).value().trim().to_owned();
                self.collection_name.update(cx, |i, cx| i.set_text("", w, cx));
                self.calendar_run(cx, move |b| {
                    let made = b.create_calendar(&name, "")?;
                    Ok(format!("Made the calendar {}", made.name))
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
                            self.calendar_error = Some("Write the due date as 2026-11-24 17:00 or 2026-11-24, or leave it empty.".into());
                            cx.notify();
                            return;
                        }
                    }
                };
                self.collection_name.update(cx, |i, cx| i.set_text("", w, cx));
                self.task_due.update(cx, |i, cx| i.set_text("", w, cx));
                self.calendar_run(cx, move |b| {
                    let t = b.add_task(&name, due)?;
                    Ok(format!("Added the task {}", t.event.summary))
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
                            self.calendar_error = Some("Write the due date as 2026-11-24 17:00 or 2026-11-24, or leave it empty.".into());
                            cx.notify();
                            return;
                        }
                    }
                };
                self.task_editing = None;
                self.task_due.update(cx, |i, cx| i.set_text("", w, cx));
                self.calendar_run(cx, move |b| {
                    b.edit_task(&uid, &title, &notes, due, all_day)
                        .map(|t| format!("Saved the task {}", t.event.summary))
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
                            "The calendar server now sends your invitations".into()
                        } else {
                            "Picacho emails your invitations again".into()
                        }
                    })
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
                self.free_busy = Some("Looking…".into());
                self.task(
                    cx,
                    move |b| {
                        let from = around - around.rem_euclid(86_400);
                        b.free_busy(&organizer, &guests, from, from + 86_400)
                    },
                    |this, result, cx| {
                        this.free_busy = Some(match result {
                            Ok(found) if found.iter().all(|f| f.busy.is_empty()) => {
                                "Everyone is free that day.".into()
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
                                                    .format("%H:%M")
                                                    .to_string()
                                            };
                                            format!("{}–{}", at(*s), at(*e))
                                        })
                                        .collect();
                                    if times.is_empty() {
                                        format!("{} is free", f.attendee)
                                    } else {
                                        format!("{} is busy {}", f.attendee, times.join(", "))
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
                        "Taken back; it's in Drafts".into()
                    } else {
                        "It had already been sent".into()
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
    }

    /// The account an event you organize is sent from: the one whose
    /// address organizes it, or (for a new event) the first.
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
            .format("%a, %b %-d, %H:%M");
        Some(
            div()
                .id("event-zone-note")
                .debug_selector(|| "event-zone-note".into())
                .text_size(px(11.5))
                .text_color(rgb(c.muted))
                .child(format!("That's {here} here."))
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
            .aria_label("Propose a new time")
            .mt_2()
            .flex()
            .flex_col()
            .gap_2()
            .max_w(px(420.))
            .child(labelled("Starts", "propose-start", &self.event_start))
            .child(labelled("Ends", "propose-end", &self.event_end))
            .child(labelled(
                "Note to the organizer (optional)",
                "propose-note",
                &self.event_notes,
            ))
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(self.button(
                        "propose-send",
                        "Send Proposal",
                        "send",
                        Action::SendProposal(id.to_owned()),
                        true,
                        cx,
                    ))
                    .child(self.button(
                        "propose-cancel",
                        "Cancel",
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
        let accounts: Vec<&Account> = self
            .accounts
            .iter()
            .filter(|a| a.provider == crate::manage::model::Provider::Jmap)
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
                        .child("Or use those kept with your mail, signed in as the account already is:"),
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
        let mut page = div()
            .flex()
            .flex_col()
            .gap_5()
            .child(self.section("Calendar"))
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(c.muted))
                    .child("Invitations in mail show what, when and where, with Accept, Maybe and Decline; the organizer is told. With a calendar server, answered invitations go in your calendar, and you can add and change events here."),
            );
        let Some(server) = self.backend.calendar_server() else {
            return page
                .child(self.section("Calendar server (CalDAV)"))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .max_w(px(420.))
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(rgb(c.muted))
                                .child("Fastmail, iCloud, Nextcloud, Radicale, mailbox.org or another CalDAV server. iCloud and Fastmail need an app password."),
                        )
                        .children(self.jmap_sources("calendar-jmap", Action::ConnectJmapCalendar, cx))
                        .child(labelled("Server", "calendar-url", &self.calendar_url))
                        .child(labelled("User name", "calendar-user", &self.calendar_user))
                        .child(labelled("Password", "calendar-password", &self.calendar_password))
                        .child(div().flex().child(
                            self.button(
                                "calendar-connect",
                                if busy { "Connecting…" } else { "Connect" },
                                "plus",
                                Action::ConnectCalendar,
                                true,
                                cx,
                            )
                            .when(busy, |d| d.opacity(0.6)),
                        )),
                )
                .children(self.calendar_error.clone().map(|e| {
                    div().id("calendar-error").text_size(px(12.)).text_color(rgb(c.danger)).child(e)
                }))
                .into_any_element();
        };
        let synced = self
            .backend
            .calendar_synced()
            .and_then(|t| chrono::DateTime::from_timestamp(t, 0))
            .map(|t| {
                t.with_timezone(&chrono::Local)
                    .format("%b %-d, %H:%M")
                    .to_string()
            })
            .unwrap_or_else(|| "never".into());
        let over_jmap = !server.jmap_account.is_empty();
        page = page
            .child(self.section(if over_jmap {
                "Calendar server (JMAP)"
            } else {
                "Calendar server (CalDAV)"
            }))
            .child(
            self.setting(
                &server.url,
                &format!(
                    "{} {}. Read {synced}; read again each time Picacho starts.",
                    if over_jmap { "The calendars of" } else { "Signed in as" },
                    server.user
                ),
                div()
                    .flex()
                    .gap_2()
                    .child(
                        self.button(
                            "calendar-sync",
                            if busy { "Reading…" } else { "Read Now" },
                            "refresh",
                            Action::SyncCalendar,
                            false,
                            cx,
                        )
                        .when(busy, |d| d.opacity(0.6)),
                    )
                    .child(self.button(
                        "calendar-disconnect",
                        "Disconnect",
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
            page = page.child(
                self.setting(
                    "Shared calendars",
                    &notices.join("\n"),
                    self.button(
                        "calendar-notices-dismiss",
                        "Dismiss",
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
            page = page.child(
                self.setting(
                    "New events go to",
                    "Also where invitations you accept are put.",
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
        if !over_jmap {
            page = page.child(self.switch_setting(
                "Let the server send invitations",
                if server.server_schedules {
                    "The calendar server invites guests to events you organize, and tells them about changes and cancellations. Invitations you receive are still answered by mail."
                } else {
                    "Picacho emails invitations, updates and cancellations for events you organize. Turn on to have the calendar server send them, where it can."
                },
                "calendar-server-scheduling".into(),
                server.server_schedules,
                Action::ServerScheduling(!server.server_schedules),
                cx,
            ));
        }
        page = page.children(self.calendar_error.clone().map(|e| {
            div()
                .id("calendar-error")
                .text_size(px(12.))
                .text_color(rgb(c.danger))
                .child(e)
        }));
        // A new calendar, or a task, named in one field.
        page = page.child(
            self.setting(
                "New calendar or task",
                "A calendar is made on the server. A task goes to the calendar new events go to (or the first that keeps tasks), due when written in Due, else with no due date.",
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
                    .child(self.button("calendar-create", "Make Calendar", "plus", Action::CreateCalendar, false, cx))
                    .child(self.button("task-add", "Add Task", "check", Action::AddTask, false, cx)),
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
                            "Rename",
                            "",
                            Action::RenameCalendar(cal.href.clone()),
                            false,
                            cx,
                        ))
                        .child(
                            self.button(
                                format!("calendar-delete-{}", cal.href),
                                "Delete",
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
                format!(" The server keeps {:.0} MB of yours, with {:.0} MB left.", mb(used), mb(left))
            });
            page = page.child(self.setting(
                "Calendars",
                &format!(
                    "Rename uses the name written in the field above. Deleting removes the calendar and its events from the server.{}",
                    storage.unwrap_or_default()
                ),
                list,
            ));
        }
        // Open tasks.
        let tasks = self.backend.calendar_tasks();
        if !tasks.is_empty() {
            let count = tasks.len();
            page = page.child(self.section("Tasks")).child(
                div()
                    .id("calendar-tasks")
                    .role(Role::List)
                    .aria_label("Tasks")
                    .rounded(px(12.))
                    .border_1()
                    .border_color(rgb(c.border))
                    .bg(rgb(c.raised))
                    .overflow_hidden()
                    .flex()
                    .flex_col()
                    .children(tasks.into_iter().take(50).enumerate().map(|(i, t)| {
                        let due = if t.event.start == 0 {
                            "no due date".to_owned()
                        } else if t.event.all_day {
                            chrono::DateTime::from_timestamp(t.event.start, 0)
                                .map(|d| d.format("%a %b %-d").to_string())
                                .unwrap_or_default()
                        } else {
                            chrono::DateTime::from_timestamp(t.event.start, 0)
                                .map(|d| d.with_timezone(&chrono::Local).format("%a %b %-d, %H:%M").to_string())
                                .unwrap_or_default()
                        };
                        let overdue = t.event.start != 0 && t.event.start < chrono::Utc::now().timestamp();
                        div()
                            .id(SharedString::from(format!("task-{i}")))
                            .role(Role::ListItem)
                            .aria_label(format!("{}, due {due}", t.event.summary))
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
                                                "No due date".to_owned()
                                            } else if overdue {
                                                format!("Overdue: {due}")
                                            } else {
                                                format!("Due {due}")
                                            }),
                                    ),
                            )
                            .child(div().debug_selector(move || format!("task-edit-{i}")).child(self.button(
                                SharedString::from(format!("task-edit-{i}")),
                                "Edit",
                                "compose",
                                Action::EditTask(t.event.uid.clone()),
                                false,
                                cx,
                            )))
                            .child(self.button(
                                SharedString::from(format!("task-done-{i}")),
                                "Done",
                                "check",
                                Action::CompleteTask(t.event.uid.clone()),
                                false,
                                cx,
                            ))
                    })),
            );
        }
        // The task being edited.
        if let Some(uid) = &self.task_editing {
            page = page.child(
                div()
                    .id("task-editor")
                    .debug_selector(|| "task-editor".into())
                    .role(Role::Form)
                    .aria_label("Edit task")
                    .p_4()
                    .rounded(px(12.))
                    .border_1()
                    .border_color(rgb(c.border))
                    .bg(rgb(c.raised))
                    .flex()
                    .flex_col()
                    .gap_3()
                    .max_w(px(480.))
                    .child(div().text_size(px(14.)).font_weight(FontWeight::SEMIBOLD).child("Edit Task"))
                    .child(labelled("Task", "task-title", &self.task_title))
                    .child(labelled("Due (2026-11-24 17:00, a date, or empty for none)", "task-due", &self.task_due))
                    .child(labelled("Notes", "task-notes", &self.task_notes))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap_2()
                            .child(self.button("task-save", "Save", "check", Action::SaveTask, true, cx))
                            .child(self.button("task-cancel", "Cancel", "close", Action::CancelTaskEdit, false, cx))
                            .child(self.button("task-delete", "Delete", "trash", Action::DeleteTask(uid.clone()), false, cx)),
                    ),
            );
        }
        // Messages JMAP servers hold to send later.
        let jmap_accounts: Vec<&Account> = self
            .accounts
            .iter()
            .filter(|a| a.provider == crate::manage::model::Provider::Jmap)
            .collect();
        if !jmap_accounts.is_empty() {
            page = page.child(self.section("Held to send later"));
            for a in jmap_accounts {
                let listed = self.held.as_ref().filter(|(id, _)| *id == a.id);
                let detail = match listed {
                    None => "Messages handed to the server to send later can be taken back until they go.".to_owned(),
                    Some((_, held)) if held.is_empty() => "Nothing is waiting.".to_owned(),
                    Some((_, held)) => format!("{} waiting.", held.len()),
                };
                page = page.child(self.setting(
                    &a.email,
                    &detail,
                    self.button(
                        SharedString::from(format!("held-{}", a.id)),
                        "Show",
                        "refresh",
                        Action::ListHeld(a.id.clone()),
                        false,
                        cx,
                    ),
                ));
                for h in listed.map(|(_, h)| h.as_slice()).unwrap_or_default() {
                    let when = chrono::DateTime::from_timestamp(h.send_at, 0)
                        .map(|d| d.with_timezone(&chrono::Local).format("%a %b %-d, %H:%M").to_string())
                        .unwrap_or_default();
                    page = page.child(self.setting(
                        if h.subject.is_empty() { "(No subject)" } else { &h.subject },
                        &format!("Goes {when}"),
                        self.button(
                            SharedString::from(format!("held-cancel-{}", h.id)),
                            "Take Back",
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
        if let Some(uid) = &self.event_editing {
            page = page.child(
                div()
                    .id("event-editor")
                    .role(Role::Form)
                    .aria_label(if uid.is_empty() {
                        "New event"
                    } else {
                        "Edit event"
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
                                "New Event"
                            } else {
                                "Edit Event"
                            }),
                    )
                    .child(labelled("Title", "event-title", &self.event_title))
                    .child(labelled(
                        "Starts (2026-11-24 10:00, a date for all day, or words like “tomorrow at 10” with Understand dates)",
                        "event-start",
                        &self.event_start,
                    ))
                    .child(labelled("Ends (empty: an hour later)", "event-end", &self.event_end))
                    .child(labelled("Time zone (the times above are in it)", "event-zone", &self.event_zone))
                    .children(self.zone_note(cx))
                    .child(labelled("Where", "event-location", &self.event_location))
                    .child(labelled("Notes", "event-notes", &self.event_notes))
                    .child(labelled("Guests (invited by mail; separated by commas)", "event-guests", &self.event_guests))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(self.button(
                                "event-free-busy",
                                "When Are They Free?",
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
                            .child(div().text_size(px(12.)).font_weight(FontWeight::MEDIUM).text_color(rgb(c.muted)).child("Remind me"))
                            .child(self.segmented(
                                "event-alarm",
                                [
                                    ("Never", None),
                                    ("10 minutes before", Some(600)),
                                    ("An hour before", Some(3600)),
                                    ("A day before", Some(86_400)),
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
                                .child(div().text_size(px(12.)).font_weight(FontWeight::MEDIUM).text_color(rgb(c.muted)).child("This event repeats. Change"))
                                .child(self.segmented(
                                    "event-scope",
                                    vec![
                                        ("Only this one".into(), Action::EventScope(false), !self.event_all_repeats),
                                        ("Every repeat".into(), Action::EventScope(true), self.event_all_repeats),
                                    ],
                                    cx,
                                )),
                        )
                    })
                    .when(self.event_occurrence.is_none() || self.event_all_repeats || self.event_repeat == Repeat::Never, |d| {
                        let mut choices = vec![
                            ("Never".to_owned(), Repeat::Never),
                            ("Daily".to_owned(), Repeat::Daily),
                            ("Weekly".to_owned(), Repeat::Weekly),
                            ("Monthly".to_owned(), Repeat::Monthly),
                            ("Yearly".to_owned(), Repeat::Yearly),
                        ];
                        if let Repeat::Kept(rule) = &self.event_repeat {
                            let text = ical::Rule::parse(rule).map(|r| r.describe()).unwrap_or_else(|| "As it was".into());
                            choices.push((text, self.event_repeat.clone()));
                        }
                        d.child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .child(div().text_size(px(12.)).font_weight(FontWeight::MEDIUM).text_color(rgb(c.muted)).child("Repeats"))
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
                                )),
                        )
                    })
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap_2()
                            .child(self.button(
                                "event-save",
                                "Save",
                                "check",
                                Action::SaveEvent,
                                true,
                                cx,
                            ))
                            .child(self.button(
                                "event-cancel",
                                "Cancel",
                                "close",
                                Action::CancelEventEdit,
                                false,
                                cx,
                            ))
                            .when(!uid.is_empty(), |d| {
                                d.child(self.button(
                                    "event-delete",
                                    "Delete",
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
                                    "Attach File",
                                    "paperclip",
                                    Action::AttachToEvent(uid.clone()),
                                    false,
                                    cx,
                                ))
                            }),
                    ),
            );
        }
        // Coming events.
        let now = chrono::Utc::now().timestamp();
        let coming: Vec<Event> = self
            .backend
            .agenda(now, now + 366 * 86_400)
            .into_iter()
            .take(60)
            .collect();
        page = page.child(
            div()
                .flex()
                .items_center()
                .child(div().flex_1().child(self.section("Coming up")))
                .when(default.is_some() && self.event_editing.is_none(), |d| {
                    d.child(self.button(
                        "event-new",
                        "New Event",
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
            .aria_label("Coming events")
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
                    .child("Nothing coming up."),
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
        page.child(list).into_any_element()
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
    }
}

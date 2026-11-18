//! The message list: a large title, filter chips, roomy rows and a bottom
//! bar for search, filters, status and compose.
//!
//! Rows swipe: toward the trailing edge to mark read or unread, toward the
//! leading edge to archive (or delete where archiving makes no sense). A
//! long press opens the row's actions on iOS and starts selecting on
//! Android, as each system's own mail apps do. Pulling the top of the list
//! down draws a box out from above it, which checks for mail when let go
//! of far enough, says Refreshing meanwhile, and goes back up when done.
//! The list itself stays where it is.
use super::chrome::{Tap, act};
use super::*;

/// Each row's height: three lines of text at phone sizes.
pub(super) const ROW: f32 = 98.;
/// How far a row swipes (or the list pulls) before letting go acts.
const COMMIT: f32 = 88.;
/// After a swipe ends, the fling's own momentum must not start another.
const SWIPE_REST: Duration = Duration::from_millis(450);
/// The pull-to-refresh box's height.
const PULL_BOX: f32 = 40.;
/// Where the pull box rests, below the list's top, once pulled far enough.
const PULL_REST: f32 = 12.;
/// Where the pull box waits out of sight, its shadow too.
const PULL_HIDDEN: f32 = -PULL_BOX - 12.;
/// The Refreshing box shows at least this long, so a quick check doesn't
/// flash it.
const REFRESH_MIN: Duration = Duration::from_millis(700);
/// It hides after this long whatever mail is doing.
const REFRESH_MAX: Duration = Duration::from_secs(60);
/// How long the pull box takes to go back up.
const PULL_HIDE: Duration = Duration::from_millis(220);
/// How long the pull box takes to settle into its Refreshing place, its
/// words changing meanwhile.
const PULL_SETTLE: Duration = Duration::from_millis(340);
/// The pull box's width: the same pulling and refreshing, so it doesn't
/// jump as its words change.
const PULL_BOX_WIDTH: f32 = 200.;

/// What a swipe does, by direction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SwipeAct {
    ToggleRead,
    Archive,
    Trash,
    DeleteForever,
    DeleteDraft,
}

impl SwipeAct {
    fn label(self, unread: bool) -> &'static str {
        match self {
            Self::ToggleRead if unread => "Read",
            Self::ToggleRead => "Unread",
            Self::Archive => "Archive",
            Self::Trash => "Trash",
            Self::DeleteForever | Self::DeleteDraft => "Delete",
        }
    }
    fn icon(self, unread: bool) -> &'static str {
        match self {
            Self::ToggleRead if unread => "mail-open",
            Self::ToggleRead => "mail",
            Self::Archive => "archive",
            _ => "trash",
        }
    }
}

impl MailApp {
    pub(super) fn mobile_list(
        &mut self,
        insets: Insets,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = self.colors();
        let split = Self::mobile_split(window);
        let count = self.visible.len();
        // Selecting: nothing to go back to until it ends.
        let back = (!self.selecting).then(|| self.back_icon("mobile-to-mailboxes", cx));
        let trailing = if self.selecting {
            let all = self.selection.len() == self.selectable_count() && !self.selection.is_empty();
            vec![
                self.touch_text(
                    "mobile-select-all",
                    if all { "None" } else { "All" },
                    false,
                    act(Action::SelectAll),
                    cx,
                )
                .into_any_element(),
                self.touch_text(
                    "mobile-select-done",
                    "Done",
                    true,
                    act(Action::StopSelecting),
                    cx,
                )
                .into_any_element(),
            ]
        } else if self.folder == Folder::Drafts || count == 0 {
            vec![]
        } else {
            vec![
                self.touch_text(
                    "mobile-select",
                    "Select",
                    false,
                    act(Action::ToggleSelectMode),
                    cx,
                )
                .into_any_element(),
            ]
        };
        let title = if self.selecting {
            match self.selection.len() {
                0 => "Select Messages".to_owned(),
                n => format!("{} Selected", fmt_count(n)),
            }
        } else {
            self.mailbox_label()
        };
        let subtitle = self
            .current_account()
            .filter(|a| !self.selecting && a.label() != title)
            .map(|a| a.email.clone());
        let list = if count == 0 {
            div()
                .flex_1()
                .min_h_0()
                .child(self.empty_list(cx))
                .into_any_element()
        } else {
            let entity = cx.entity();
            uniform_list("mobile-message-list", count, move |range, _, cx| {
                entity.update(cx, |this, cx| {
                    this.want_rows(range.clone(), cx);
                    range
                        .map(|i| this.mobile_row(this.visible[i], cx))
                        .collect::<Vec<_>>()
                })
            })
            .track_scroll(&self.list_scroll)
            // A sideways pan swipes a row; it must never scroll the list.
            .map(|mut list| {
                list.style().restrict_scroll_to_axis = Some(true);
                list
            })
            .flex_1()
            .min_h_0()
            .map(|list| super::super::panes::scroll_frame(list, &self.list_scroll))
            .into_any_element()
        };
        div()
            .id("mobile-list")
            .debug_selector(|| "mobile-list".into())
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(c.bg))
            .child(self.nav_bar("mobile-list-bar", insets, back, title, subtitle, trailing))
            .when(!self.selecting, |d| {
                d.child(self.mobile_filter_chips(insets, cx))
            })
            .when(self.search_open, |d| {
                d.child(
                    div()
                        .px(px(insets.left + GUTTER))
                        .when(Self::value(&self.search, cx).trim().is_empty(), |d| {
                            d.child(self.search_hints(cx))
                        })
                        .children(self.server_search_bar(cx)),
                )
            })
            .child(
                div()
                    .id("mobile-list-area")
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .bg(rgb(c.surface))
                    .border_t_1()
                    .border_color(rgb(c.border))
                    // The pull box waits above, out of sight.
                    .overflow_hidden()
                    .on_scroll(
                        cx.listener(|this, e: &ScrollEvent, w, cx| this.mobile_pull(e, w, cx)),
                    )
                    .child(div().flex_1().min_h_0().flex().flex_col().child(list))
                    .children(self.pull_box()),
            )
            .when_some(self.catalog_progress(), |d, progress| {
                d.child(
                    div()
                        .flex_shrink_0()
                        .h(px(32.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .gap_2()
                        .bg(rgb(c.surface))
                        .text_size(px(13.))
                        .text_color(rgb(c.muted))
                        .child(spinner(12., c.muted))
                        .child(if progress.total == 0 {
                            "Listing mail…".to_owned()
                        } else {
                            format!(
                                "Listing mail · {} of {}",
                                fmt_count(progress.cataloged),
                                fmt_count(progress.total)
                            )
                        }),
                )
            })
            .child(if self.search_open {
                self.mobile_search_bar(insets, cx).into_any_element()
            } else if self.selecting {
                self.mobile_selection_bar(insets, cx).into_any_element()
            } else {
                self.tool_row(
                    "mobile-list-toolbar",
                    insets,
                    vec![self.bar_icon(
                        "mobile-settings",
                        "settings",
                        "Settings",
                        Rc::new(
                            |this: &mut MailApp, w: &mut Window, cx: &mut Context<MailApp>| {
                                this.mobile_open_settings(w, cx)
                            },
                        ),
                        cx,
                    )],
                    Some(self.search_pill(cx)),
                    vec![self.compose_button(cx)],
                )
                .when(split, |d| d.border_r_0())
                .into_any_element()
            })
            .into_any_element()
    }

    /// The filters as chips under the title, so changing one is a tap.
    fn mobile_filter_chips(&self, insets: Insets, cx: &Context<Self>) -> impl IntoElement {
        let chip = |id: &'static str, filter: Filter, label: &'static str| {
            let on = self.filter_by == filter;
            self.chip(
                id,
                label,
                on,
                act(Action::SetFilter(if on { Filter::All } else { filter })),
                cx,
            )
        };
        div()
            .id("mobile-filters")
            .debug_selector(|| "mobile-filters".into())
            .flex_shrink_0()
            .overflow_x_scroll()
            .pl(px(insets.left + GUTTER))
            .pr(px(insets.right + GUTTER))
            .flex()
            .items_center()
            .gap_2()
            .child(chip("mobile-filter-unread", Filter::Unread, "Unread"))
            .child(chip("mobile-filter-starred", Filter::Starred, "Starred"))
            .child(chip(
                "mobile-filter-attachments",
                Filter::Attachments,
                "Attachments",
            ))
            .child(self.chip(
                "mobile-filter-oldest",
                "Oldest First",
                self.oldest_first,
                act(Action::OldestFirst(!self.oldest_first)),
                cx,
            ))
    }

    /// A message's row, sized for a fingertip, sliding under a swipe.
    fn mobile_row(&self, index: usize, cx: &Context<Self>) -> AnyElement {
        let c = self.colors();
        let m = &self.messages[index];
        if m.pending {
            return div()
                .h(px(ROW))
                .overflow_hidden()
                .child(placeholder_row(&m.id, c))
                .into_any_element();
        }
        let id = m.id.clone();
        let checked = self.selection.contains(&m.id);
        // The open message is marked only beside the list (a wide screen).
        let open = self.mobile.split && self.selected.as_ref() == Some(&m.id);
        let drafts = self.folder == Folder::Drafts;
        let unread = m.unread;
        let offset = match &self.mobile.swipe {
            Some((swiping, dx)) if *swiping == id => *dx,
            _ => 0.,
        };
        let (leading, trailing) = self.swipe_acts(m);
        let sender = match self.mask_for(m) {
            Some(k) if k.sent => k.sender(),
            _ => m.sender.clone(),
        };
        let lead: AnyElement = if self.selecting {
            div()
                .size(px(26.))
                .rounded_full()
                .border_2()
                .border_color(rgb(if checked { c.accent } else { c.muted }))
                .bg(rgb(if checked { c.accent } else { c.surface }))
                .flex()
                .items_center()
                .justify_center()
                .when(checked, |d| d.child(icon_el("check", 15., c.on_accent)))
                .into_any_element()
        } else {
            avatar(&m.initials(), 40., index, c).into_any_element()
        };
        let weight = if unread {
            FontWeight::SEMIBOLD
        } else {
            FontWeight::NORMAL
        };
        let content = div()
            .absolute()
            .top_0()
            .bottom_0()
            .left(px(offset))
            .w_full()
            .pl(px(GUTTER))
            .pr(px(GUTTER))
            .py(px(12.))
            .flex()
            .items_start()
            .gap(px(12.))
            .bg(rgb(if open || checked { c.soft } else { c.surface }))
            .border_b_1()
            .border_color(rgb(c.border))
            .child(
                div()
                    .relative()
                    .w(px(40.))
                    .h(px(40.))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(lead)
                    .when(unread && !self.selecting, |d| {
                        d.child(
                            div()
                                .absolute()
                                .left(px(-12.))
                                .top(px(16.))
                                .size(px(9.))
                                .rounded_full()
                                .bg(rgb(c.accent)),
                        )
                    }),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(px(17.))
                                    .font_weight(weight)
                                    .child(sender),
                            )
                            .when(m.starred, |d| d.child(icon_el("star-filled", 14., STAR)))
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .text_size(px(14.))
                                    .text_color(rgb(if unread { c.accent } else { c.muted }))
                                    .child(date_short(m.date)),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(px(15.))
                                    .font_weight(weight)
                                    .child(self.hide_codes(m, &m.subject).into_owned()),
                            )
                            .when(m.answered, |d| d.child(icon_el("answered", 14., c.muted)))
                            .when(m.forwarded, |d| d.child(icon_el("forwarded", 14., c.muted)))
                            .when(self.snoozed_until(m).is_some(), |d| {
                                d.child(icon_el("snooze", 14., c.accent))
                            })
                            .when(m.has_attachments(), |d| {
                                d.child(icon_el("paperclip", 14., c.muted))
                            }),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_size(px(15.))
                            .text_color(rgb(c.muted))
                            .child(preview(&self.hide_codes(m, &m.snippet))),
                    ),
            );
        // What a swipe reveals behind the row.
        let behind = |act: SwipeAct, leading_edge: bool| {
            let color = match act {
                SwipeAct::ToggleRead => c.accent,
                SwipeAct::Archive => 0x2f9e6e,
                _ => 0xd64545,
            };
            let reached = offset.abs() >= COMMIT;
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .when(leading_edge, |d| d.justify_start().pl(px(GUTTER + 4.)))
                .when(!leading_edge, |d| d.justify_end().pr(px(GUTTER + 4.)))
                .bg(rgb(color))
                .text_color(rgb(0xffffff))
                .text_size(px(14.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap_1()
                        .when(reached, |d| d.opacity(1.))
                        .when(!reached, |d| d.opacity(0.7))
                        .child(icon_el(act.icon(unread), 22., 0xffffff))
                        .child(act.label(unread)),
                )
        };
        let menu_id = id.clone();
        let tap_id = id.clone();
        let swipe_id = id.clone();
        let press_id = id.clone();
        div()
            .id(SharedString::from(format!("mobile-row-{id}")))
            .debug_selector(move || format!("mobile-row-{id}"))
            .relative()
            .h(px(ROW))
            .w_full()
            .overflow_hidden()
            .cursor_pointer()
            .when(offset > 0., |d| {
                d.children(leading.map(|a| behind(a, true)))
            })
            .when(offset < 0., |d| {
                d.children(trailing.map(|a| behind(a, false)))
            })
            .child(content)
            .on_scroll(cx.listener(move |this, e: &ScrollEvent, w, cx| {
                this.mobile_swipe(&swipe_id, e, w, cx)
            }))
            .on_click(cx.listener(move |this, _: &ClickEvent, w, cx| {
                if this.mobile.swipe.is_some() {
                    return;
                }
                if this.selecting {
                    this.handle(Action::ToggleSelected(tap_id.clone()), w, cx)
                } else {
                    this.mobile_push(Action::Select(tap_id.clone()), w, cx)
                }
            }))
            .when(!drafts && cfg!(target_os = "android"), |d| {
                d.child(self.press_select(press_id, cx))
            })
            .when(drafts || !cfg!(target_os = "android"), |d| {
                d.child(self.press_menu(move |this| this.mail_entries(&menu_id), cx))
            })
            .into_any_element()
    }

    /// The actions a swipe toward each edge takes on `m`: (finger moving
    /// toward the trailing edge, toward the leading edge).
    fn swipe_acts(&self, m: &Mail) -> (Option<SwipeAct>, Option<SwipeAct>) {
        match self.folder {
            Folder::Drafts => (None, Some(SwipeAct::DeleteDraft)),
            Folder::Trash | Folder::Junk => {
                (Some(SwipeAct::ToggleRead), Some(SwipeAct::DeleteForever))
            }
            Folder::Archive | Folder::Sent => (Some(SwipeAct::ToggleRead), Some(SwipeAct::Trash)),
            _ if self
                .account_of(m)
                .is_some_and(|a| a.provider == Provider::Pop3) =>
            {
                (Some(SwipeAct::ToggleRead), Some(SwipeAct::Trash))
            }
            _ => (Some(SwipeAct::ToggleRead), Some(SwipeAct::Archive)),
        }
    }

    /// Follows a horizontal pan over a row. Vertical pans go on to scroll
    /// the list.
    fn mobile_swipe(&mut self, id: &str, e: &ScrollEvent, w: &mut Window, cx: &mut Context<Self>) {
        let ScrollDelta::Pixels(d) = e.delta else {
            return;
        };
        let (dx, dy) = (f32::from(d.x), f32::from(d.y));
        let swiping = self.mobile.swipe.as_ref().is_some_and(|(s, _)| s == id);
        match e.touch_phase {
            TouchPhase::Ended | TouchPhase::Cancelled if swiping => {
                cx.stop_propagation();
                let (_, offset) = self.mobile.swipe.take().unwrap_or_default();
                self.mobile.swipe_ended = Some(std::time::Instant::now());
                if offset.abs() >= COMMIT {
                    self.mobile_swiped(id, offset > 0., w, cx);
                }
                cx.notify();
            }
            TouchPhase::Started | TouchPhase::Moved if swiping => {
                cx.stop_propagation();
                if let Some((_, offset)) = &mut self.mobile.swipe {
                    let before = offset.abs() >= COMMIT;
                    *offset += dx;
                    if before != (offset.abs() >= COMMIT) {
                        platform::haptic(platform::Haptic::Selection);
                    }
                }
                cx.notify();
            }
            // A pan starting on this row: undecided until it moves.
            TouchPhase::Started if self.mobile.swipe.is_none() => {
                self.mobile.pan = Some(id.to_owned());
                self.mobile_decide_swipe(id, dx, dy, cx);
            }
            // Only the finger's own pan, decided on its first movement,
            // starts a swipe. Once the finger lifts (`pan` cleared), its
            // fling keeps arriving as Moved events; those never start one.
            TouchPhase::Moved if self.mobile.pan.as_deref() == Some(id) => {
                self.mobile_decide_swipe(id, dx, dy, cx);
            }
            TouchPhase::Ended | TouchPhase::Cancelled => self.mobile.pan = None,
            _ => {}
        }
    }

    /// Settles an undecided pan over row `id` by its first movement:
    /// sideways swipes the row, anything else is left to scroll the list.
    fn mobile_decide_swipe(&mut self, id: &str, dx: f32, dy: f32, cx: &mut Context<Self>) {
        if dx == 0. && dy == 0. {
            return;
        }
        self.mobile.pan = None;
        let rested = self
            .mobile
            .swipe_ended
            .is_none_or(|t| t.elapsed() > SWIPE_REST);
        if self.selecting || !rested || dx.abs() <= dy.abs() {
            return;
        }
        let Some(m) = self.messages.iter().find(|m| m.id == id) else {
            return;
        };
        let (leading, trailing) = self.swipe_acts(m);
        // Only toward an edge that has an action.
        if (dx > 0. && leading.is_none()) || (dx < 0. && trailing.is_none()) {
            return;
        }
        cx.stop_propagation();
        self.mobile.swipe = Some((id.to_owned(), dx));
        cx.notify();
    }

    /// What a swipe on message `id` does when let go past the threshold.
    pub(super) fn swipe_action(&self, id: &str, toward_trailing: bool) -> Option<Action> {
        let m = self.messages.iter().find(|m| m.id == id)?;
        let (leading, trailing) = self.swipe_acts(m);
        let act = if toward_trailing { leading } else { trailing }?;
        let ids = vec![id.to_owned()];
        Some(match act {
            SwipeAct::ToggleRead => Action::Apply(ids, Change::Read(m.unread)),
            SwipeAct::Archive => Action::Apply(ids, Change::Move(Folder::Archive)),
            SwipeAct::Trash => Action::Apply(ids, Change::Move(Folder::Trash)),
            SwipeAct::DeleteForever => Action::DeleteForever(ids),
            SwipeAct::DeleteDraft => Action::DeleteDraft(id.to_owned()),
        })
    }

    /// Carries out a finished swipe on message `id`.
    fn mobile_swiped(
        &mut self,
        id: &str,
        toward_trailing: bool,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(action) = self.swipe_action(id, toward_trailing) {
            platform::haptic(platform::Haptic::Success);
            self.handle(action, w, cx);
        }
    }

    /// Android: a long press starts selecting, with this row picked.
    fn press_select(&self, id: String, cx: &Context<Self>) -> impl IntoElement {
        let this = cx.weak_entity();
        canvas(
            |_, _, _| {},
            move |bounds, _, window, _| {
                let this = this.clone();
                let id = id.clone();
                window.on_mouse_event(move |e: &LongPressEvent, phase, window, cx| {
                    if phase != DispatchPhase::Bubble
                        || e.phase != TouchPhase::Started
                        || !bounds.contains(&e.start_position)
                        || window.default_prevented()
                    {
                        return;
                    }
                    window.prevent_default();
                    cx.stop_propagation();
                    platform::haptic(platform::Haptic::Selection);
                    let _ = this.update(cx, |this, cx| {
                        this.handle(Action::StartSelecting(id.clone()), window, cx)
                    });
                });
            },
        )
        .absolute()
        .inset_0()
    }

    /// Checks for mail once the top of the list is pulled down far enough.
    fn mobile_pull(&mut self, e: &ScrollEvent, w: &mut Window, cx: &mut Context<Self>) {
        // Content scrolling lets go of the keyboard, as iOS lists do.
        if self.mobile.keyboard_for.is_some() {
            self.mobile_dismiss_keyboard(w, cx);
        }
        let ScrollDelta::Pixels(d) = e.delta else {
            return;
        };
        let dy = f32::from(d.y);
        // Scroll events bubble here whether or not the list could take them,
        // so only a touch that starts (and stays) at the top pulls. The list
        // handles the event first, so its offset already includes this one.
        let at_top = self.list_scroll.0.borrow().base_handle.offset().y >= px(0.);
        if e.touch_phase == TouchPhase::Started {
            self.mobile.pull_armed = at_top;
        }
        // While it says Refreshing, the box stays put.
        let pulling = self.mobile.refreshing.is_none()
            && (self.mobile.pull > 0. || (self.mobile.pull_armed && at_top && dy > 0.));
        match e.touch_phase {
            TouchPhase::Started | TouchPhase::Moved if pulling => {
                self.mobile.pull_hide = None;
                let before = self.mobile.pull >= COMMIT;
                // Resistance: the further pulled, the slower it follows.
                self.mobile.pull = (self.mobile.pull + dy * 0.5).clamp(0., COMMIT * 1.5);
                if !before && self.mobile.pull >= COMMIT {
                    platform::haptic(platform::Haptic::Selection);
                }
                cx.notify();
            }
            TouchPhase::Ended | TouchPhase::Cancelled => {
                // Momentum after the finger lifts mustn't start a pull.
                self.mobile.pull_armed = false;
                let pull = std::mem::take(&mut self.mobile.pull);
                if pull >= COMMIT {
                    self.mobile.refreshing = Some(std::time::Instant::now());
                    self.mobile.pull_anim += 1;
                    self.mobile.pull_settle = Some((pull_box_top(pull), self.mobile.pull_anim));
                    self.handle(Action::Refresh, w, cx);
                    self.watch_refresh(cx);
                    cx.notify();
                } else if pull > 0. {
                    self.hide_pull_box(pull_box_top(pull));
                    cx.notify();
                }
            }
            _ => {}
        }
    }

    /// Sends the pull box back up from `from`.
    fn hide_pull_box(&mut self, from: f32) {
        self.mobile.pull_settle = None;
        self.mobile.pull_anim += 1;
        self.mobile.pull_hide = Some((from, self.mobile.pull_anim));
    }

    /// Mail is being checked for the open mailbox, by a sync or a retried
    /// sign-in.
    fn mobile_refreshing(&self) -> bool {
        self.checking_server()
            || self.mailbox_loading()
            || self.retrying.iter().any(|id| self.shows(id))
    }

    /// Hides the Refreshing box once the check it started is done.
    fn watch_refresh(&self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(150))
                    .await;
                let done = this.update(cx, |this, cx| {
                    let Some(since) = this.mobile.refreshing else {
                        return true;
                    };
                    let waited = since.elapsed();
                    let done = waited >= REFRESH_MAX
                        || (waited >= REFRESH_MIN && !this.mobile_refreshing());
                    if done {
                        this.mobile.refreshing = None;
                        this.hide_pull_box(PULL_REST);
                        cx.notify();
                    }
                    done
                });
                if done.unwrap_or(true) {
                    break;
                }
            }
        })
        .detach();
    }

    /// The pull-to-refresh box: drawn out from above the list as it is
    /// pulled, resting there with Refreshing while mail is checked, and
    /// going back up after. `None` while out of sight.
    fn pull_box(&self) -> Option<AnyElement> {
        let c = self.colors();
        let (top, refreshing) = if self.mobile.refreshing.is_some() {
            (PULL_REST, true)
        } else if self.mobile.pull > 0. {
            (pull_box_top(self.mobile.pull), false)
        } else if self.mobile.pull_hide.is_some() {
            (PULL_HIDDEN, false)
        } else {
            return None;
        };
        // Its words, one layer for each state, laid over each other so one
        // can fade into the other.
        let layer = || {
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .gap_2()
        };
        let pulling = |progress: f32| {
            layer()
                .child(
                    div()
                        .size(px(16.))
                        .flex_shrink_0()
                        .child(ring(progress, c.border, c.accent).size_full()),
                )
                .child("Pull down to refresh")
        };
        let busy = || layer().child(spinner(14., c.accent)).child("Refreshing…");
        let settle = self.mobile.pull_settle.filter(|_| refreshing);
        let card = div()
            .relative()
            .w(px(PULL_BOX_WIDTH))
            .h(px(PULL_BOX))
            .rounded_full()
            .bg(rgb(c.raised))
            .border_1()
            .border_color(rgb(c.border))
            .shadow_md()
            .text_size(px(13.))
            .text_color(rgb(c.text))
            .map(|d| match settle {
                // Pulling's words give way to Refreshing's as it settles.
                Some((_, key)) => d
                    .child(pulling(1.).with_animation(
                        ("mobile-pull-out", key),
                        Animation::new(PULL_SETTLE / 2).with_easing(ease_out),
                        |d, t| d.opacity(1. - t),
                    ))
                    .child(busy().with_animation(
                        ("mobile-pull-in", key),
                        Animation::new(PULL_SETTLE).with_easing(|t: f32| {
                            // In once pulling's words are mostly gone.
                            ease_out(((t - 0.35) / 0.65).clamp(0., 1.))
                        }),
                        |d, t| d.opacity(t),
                    )),
                None if refreshing => d.child(busy()),
                None => d.child(pulling((self.mobile.pull / COMMIT).min(1.))),
            });
        let row = div()
            .id("mobile-pull-box")
            .debug_selector(|| "mobile-pull-box".into())
            .absolute()
            .left_0()
            .right_0()
            .top(px(top))
            .flex()
            .justify_center()
            .child(card);
        Some(match (settle, self.mobile.pull_hide) {
            // From where it was let go of into its place, overshooting a
            // little as it springs back.
            (Some((from, key)), _) => row
                .with_animation(
                    ("mobile-pull-settle", key),
                    Animation::new(PULL_SETTLE).with_easing(ease_out_back),
                    move |d, t| d.top(px(from + (PULL_REST - from) * t)),
                )
                .into_any_element(),
            (None, Some((from, key))) if !refreshing && self.mobile.pull <= 0. => row
                .with_animation(
                    ("mobile-pull-hide", key),
                    Animation::new(PULL_HIDE).with_easing(|t: f32| t * t),
                    move |d, t| {
                        d.top(px(from + (PULL_HIDDEN - from) * t))
                            .opacity(1. - t * 0.6)
                    },
                )
                .into_any_element(),
            _ => row.into_any_element(),
        })
    }

    /// The bar's search field, closed: tapping it searches. Until then it
    /// says what mail is doing.
    fn search_pill(&self, cx: &Context<Self>) -> AnyElement {
        let c = self.colors();
        let (busy, status) = self.mobile_status();
        div()
            .id("mobile-search")
            .debug_selector(|| "mobile-search".into())
            .w_full()
            .h(px(TOUCH))
            .px_4()
            .flex()
            .items_center()
            .gap_2()
            .rounded_full()
            .bg(rgb(c.surface))
            .border_1()
            .border_color(rgb(c.border))
            .cursor_pointer()
            .active(move |s| s.bg(rgb(c.border)))
            .child(icon_el("search", 18., c.muted))
            .child(
                div()
                    .flex_shrink_0()
                    .text_size(px(16.))
                    .text_color(rgb(c.muted))
                    .child("Search"),
            )
            .child(div().flex_1())
            .child(
                div()
                    .id("mobile-status")
                    .debug_selector(|| "mobile-status".into())
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap_1()
                    .text_size(px(12.))
                    .text_color(rgb(c.muted))
                    .when(busy, |d| d.child(spinner(12., c.muted)))
                    .child(div().truncate().child(status)),
            )
            .on_click(self.tap_listener(act(Action::OpenSearch), cx))
            .into_any_element()
    }

    /// Searching: the field sits at the bottom, right above the keyboard.
    /// Done lets go of the keyboard to look through what was found;
    /// Cancel, once it is down, ends the search.
    fn mobile_search_bar(&self, insets: Insets, cx: &Context<Self>) -> Stateful<Div> {
        let c = self.colors();
        let typing =
            self.mobile.keyboard_for.as_ref() == Some(&self.search.read(cx).focus_handle(cx));
        let button = if typing {
            self.touch_text(
                "mobile-search-done",
                "Done",
                true,
                Rc::new(
                    |this: &mut MailApp, w: &mut Window, cx: &mut Context<MailApp>| {
                        this.mobile_dismiss_keyboard(w, cx)
                    },
                ),
                cx,
            )
        } else {
            self.touch_text(
                "mobile-search-cancel",
                "Cancel",
                false,
                act(Action::CloseSearch),
                cx,
            )
        };
        // The press must not take focus from the field (which would turn
        // Done into Cancel before the finger lifts).
        let button = button.on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());
        self.bottom_bar("mobile-search-bar", insets).child(
            div()
                .h(px(TOOLBAR))
                .flex()
                .items_center()
                .gap_2()
                .children([
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(
                            field(Input::new(&self.search), c, false)
                                .prefix(icon_el("search", 18., c.muted))
                                .h(px(TOUCH))
                                .text_size(px(17.))
                                .rounded(px(12.)),
                        )
                        .into_any_element(),
                    button.into_any_element(),
                ]),
        )
    }

    /// Selecting: what can be done with the picked messages.
    fn mobile_selection_bar(&self, insets: Insets, cx: &Context<Self>) -> Stateful<Div> {
        let none = self.selection.is_empty();
        let all_read = self.selection_all(|m| !m.unread);
        let all_starred = self.selection_all(|m| m.starred);
        let tool = |id: &'static str, icon: &'static str, label: &'static str, tap: Tap| {
            self.touch_icon(id, icon, label, tap, cx)
                .when(none, |d| d.opacity(0.4))
                .into_any_element()
        };
        let first = self.selection.iter().next().cloned();
        self.action_row(
            "mobile-selection-bar",
            insets,
            vec![
                tool(
                    "mobile-bulk-read",
                    if all_read { "mail" } else { "mail-open" },
                    if all_read {
                        "Mark as unread"
                    } else {
                        "Mark as read"
                    },
                    act(Action::ApplySelection(Change::Read(!all_read))),
                ),
                tool(
                    "mobile-bulk-star",
                    if all_starred { "star-filled" } else { "star" },
                    if all_starred { "Unstar" } else { "Star" },
                    act(Action::ApplySelection(Change::Star(!all_starred))),
                ),
                tool(
                    "mobile-bulk-move",
                    "folder",
                    "Move to folder",
                    act(Action::MoveMenu(MoveTarget::Selection)),
                ),
                tool(
                    "mobile-bulk-archive",
                    "archive",
                    "Archive",
                    act(Action::ApplySelection(Change::Move(Folder::Archive))),
                ),
                tool(
                    "mobile-bulk-trash",
                    "trash",
                    "Move to trash",
                    act(Action::ApplySelection(Change::Move(Folder::Trash))),
                ),
                tool(
                    "mobile-bulk-more",
                    "ellipsis",
                    "More",
                    Rc::new(
                        move |this: &mut MailApp, w: &mut Window, cx: &mut Context<MailApp>| {
                            if let Some(id) = &first {
                                let entries = this.mail_entries(id);
                                this.open_menu_here(entries, w, cx);
                            }
                        },
                    ),
                ),
            ],
        )
    }
}

/// Where the pull box's top is, `pull` points into a pull: out of sight at
/// none, at rest at the point where letting go refreshes, a little further
/// (resisting) past it.
fn pull_box_top(pull: f32) -> f32 {
    if pull <= COMMIT {
        PULL_HIDDEN + (PULL_REST - PULL_HIDDEN) * (pull / COMMIT).max(0.)
    } else {
        PULL_REST + (pull - COMMIT) * 0.25
    }
}

fn ease_out(t: f32) -> f32 {
    1. - (1. - t).powi(3)
}

/// Eases out past the end a little, then back: a soft spring.
fn ease_out_back(t: f32) -> f32 {
    const OVERSHOOT: f32 = 1.2;
    let u = t - 1.;
    1. + (OVERSHOOT + 1.) * u.powi(3) + OVERSHOOT * u.powi(2)
}

//! Built-in desktop widget window for macOS, Linux and Windows: a compact,
//! glanceable window showing Inbox messages, Unread count, and Upcoming
//! Calendar events with live updates. Clicking any item opens it directly in
//! Picacho's main window.
#![cfg(not(any(target_os = "android", target_os = "ios")))]

use super::*;
use crate::manage::widgets::{
    self, Snapshot, WidgetAction, WidgetEvent, WidgetMessage, WidgetPackage, WidgetReceipt,
};
use chrono::{Datelike, Local, TimeZone, Utc};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WidgetTab {
    Inbox,
    Unread,
    Calendar,
    Packages,
    Receipts,
}

pub(super) struct DesktopWidgetWindow {
    app: WeakEntity<MailApp>,
    main: AnyWindowHandle,
    store: Arc<crate::manage::store::Store>,
    snapshot: Snapshot,
    loading: bool,
    tab: WidgetTab,
    /// `None` for All Accounts / All Inboxes, or a chosen account id.
    account: Option<String>,
    scroll: ScrollHandle,
    focus: FocusHandle,
    /// Where the Packages or Receipts list was laid out last, so it shows as
    /// many whole rows as fit (see `fitting`).
    list_bounds: Rc<Cell<Bounds<Pixels>>>,
}

/// A Packages row's height (fixed, so the rows that fit can be counted).
const PACKAGE_ROW: f32 = 60.;
/// A Receipts row's height.
const RECEIPT_ROW: f32 = 46.;
/// The space between rows, and around the list.
const ROW_GAP: f32 = 4.;
const LIST_PAD: f32 = 4.;

impl DesktopWidgetWindow {
    fn new(
        app: WeakEntity<MailApp>,
        main: AnyWindowHandle,
        store: Arc<crate::manage::store::Store>,
        snapshot: Snapshot,
        loading: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut updates = widgets::subscribe();
        cx.spawn(async move |this, cx| {
            use futures::StreamExt;
            while let Some(snap) = updates.next().await {
                if this
                    .update(cx, |this, cx| {
                        this.snapshot = snap;
                        this.loading = false;
                        if let Some(id) = &this.account
                            && !this.snapshot.accounts.iter().any(|a| &a.id == id)
                        {
                            this.account = None;
                        }
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();

        let refresh_store = store.clone();
        std::thread::Builder::new()
            .name("picacho-widget-init".into())
            .spawn(move || {
                widgets::publish_force(&refresh_store);
            })
            .ok();

        Self {
            app,
            main,
            store,
            snapshot,
            loading,
            tab: WidgetTab::Inbox,
            account: None,
            scroll: ScrollHandle::new(),
            focus: cx.focus_handle(),
            list_bounds: Rc::new(Cell::new(Bounds::default())),
        }
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.loading = true;
        cx.notify();
        let store = self.store.clone();
        let _ = std::thread::Builder::new()
            .name("picacho-widget-refresh".into())
            .spawn(move || {
                widgets::publish_force(&store);
            });
    }

    fn colors(&self, cx: &App) -> Colors {
        if let Some(app) = self.app.upgrade() {
            app.read(cx).colors()
        } else {
            Colors::new(false, Accent::from_key(&self.snapshot.accent_key))
        }
    }

    fn dispatch_action(&self, action: WidgetAction, cx: &mut Context<Self>) {
        let (app, main) = (self.app.clone(), self.main);
        let uri = action.to_uri();
        if main
            .update(cx, move |_, w, cx| {
                w.activate_window();
                cx.activate(true);
                let _ = app.update(cx, |app, cx| {
                    app.open_widget_action(action, w, cx);
                });
            })
            .is_err()
        {
            // Main window was closed while the widget stayed open: reopen it
            // with this widget action queued.
            crate::display::launch::push(uri);
            shell::open(cx);
        }
    }

    fn cycle_account(&mut self, cx: &mut Context<Self>) {
        if self.snapshot.accounts.len() <= 1 {
            self.account = None;
            return;
        }
        self.account = match &self.account {
            None => self.snapshot.accounts.first().map(|a| a.id.clone()),
            Some(curr) => {
                let pos = self
                    .snapshot
                    .accounts
                    .iter()
                    .position(|a| &a.id == curr);
                match pos {
                    Some(i) if i + 1 < self.snapshot.accounts.len() => {
                        Some(self.snapshot.accounts[i + 1].id.clone())
                    }
                    _ => None,
                }
            }
        };
        cx.notify();
    }

    fn account_label(&self) -> String {
        if let Some(id) = &self.account
            && let Some(a) = self.snapshot.accounts.iter().find(|a| &a.id == id)
        {
            return a.name.clone();
        }
        if self.snapshot.accounts.len() == 1 {
            return self.snapshot.accounts[0].name.clone();
        }
        match self.tab {
            WidgetTab::Packages | WidgetTab::Receipts => t!("widget-window-all-accounts").to_owned(),
            _ => t!("widget-window-all-inboxes").to_owned(),
        }
    }

    fn render_inbox(&self, c: Colors, unread_only: bool, cx: &mut Context<Self>) -> AnyElement {
        let msgs = widgets::filtered_messages(&self.snapshot, self.account.as_deref(), unread_only, 25);
        if msgs.is_empty() {
            let empty = if self.snapshot.accounts.is_empty() {
                t!("widget-window-open-picacho-to-add-an-account")
            } else if unread_only {
                t!("widget-window-all-caught-up")
            } else {
                t!("widget-window-no-messages-in-inbox")
            };
            return div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .p_6()
                .text_sm()
                .text_color(rgb(c.muted))
                .child(empty)
                .into_any_element();
        }

        let mut list = div()
            .id("desktop-widget-inbox-list")
            .track_scroll(&self.scroll)
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px_2()
            .py_1()
            .flex()
            .flex_col()
            .gap(px(2.));

        for (i, m) in msgs.into_iter().enumerate() {
            list = list.child(self.message_row(i, m, c, cx));
        }
        list.into_any_element()
    }

    fn message_row(
        &self,
        i: usize,
        m: &WidgetMessage,
        c: Colors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let account_id = m.account_id.clone();
        let msg_id = m.id.clone();
        let sender = if m.sender.is_empty() {
            t!("widget-window-unknown-sender").to_owned()
        } else {
            m.sender.clone()
        };
        let subject = if m.subject.is_empty() {
            t!("widget-window-no-subject").to_owned()
        } else {
            m.subject.clone()
        };
        let date_str = format_short_date(m.date);
        let unread = m.unread;

        div()
            .id(SharedString::from(format!("desktop-widget-msg-{i}")))
            .px_2()
            .py(px(7.))
            .rounded(px(8.))
            .cursor_pointer()
            .hover(move |s| s.bg(rgb(c.raised)))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.dispatch_action(
                    WidgetAction::Message {
                        account: account_id.clone(),
                        id: msg_id.clone(),
                    },
                    cx,
                );
            }))
            .flex()
            .items_start()
            .gap_2()
            .child(
                div()
                    .mt(px(6.))
                    .size(px(7.))
                    .rounded_full()
                    .flex_shrink_0()
                    .when(unread, |d| d.bg(rgb(c.accent))),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(1.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_xs()
                                    .text_color(rgb(if unread { c.text } else { c.muted }))
                                    .when(unread, |d| d.font_weight(FontWeight::SEMIBOLD))
                                    .child(sender),
                            )
                            .when(m.has_attachments, |d| {
                                d.child(icon_el("paperclip", 11., c.muted))
                            })
                            .when(m.starred, |d| d.child(icon_el("star", 11., c.accent)))
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .text_xs()
                                    .text_color(rgb(c.muted))
                                    .child(date_str),
                            ),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_xs()
                            .text_color(rgb(if unread { c.text } else { c.muted }))
                            .when(unread, |d| d.font_weight(FontWeight::MEDIUM))
                            .child(subject),
                    )
                    .when(!m.snippet.is_empty(), |d| {
                        d.child(
                            div()
                                .truncate()
                                .text_xs()
                                .text_color(rgb(c.muted))
                                .child(m.snippet.clone()),
                        )
                    }),
            )
            .into_any_element()
    }

    fn render_unread(&self, c: Colors, cx: &mut Context<Self>) -> AnyElement {
        let unread = widgets::unread_for(&self.snapshot, self.account.as_deref());
        let account = self.account.clone();
        let count_text = if unread > 999 {
            "999+".to_owned()
        } else {
            unread.to_string()
        };
        let subtitle = if unread == 0 {
            t!("widget-window-all-caught-up").to_owned()
        } else {
            t!("widget-window-unread-messages", count = unread)
        };

        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(
                div()
                    .id("desktop-widget-unread-hero")
                    .mx_3()
                    .mt_2()
                    .mb_1()
                    .p_3()
                    .rounded(px(10.))
                    .bg(rgb(c.surface))
                    .border_1()
                    .border_color(rgb(c.border))
                    .cursor_pointer()
                    .hover(move |s| s.bg(rgb(c.raised)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.dispatch_action(WidgetAction::Inbox { account: account.clone() }, cx);
                    }))
                    .flex()
                    .items_baseline()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .items_baseline()
                            .gap_2()
                            .child(
                                div()
                                    .text_size(px(30.))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(rgb(if unread > 0 { c.accent } else { c.text }))
                                    .child(count_text),
                            )
                            .child(div().text_xs().text_color(rgb(c.muted)).child(subtitle)),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(c.muted))
                            .child(self.account_label()),
                    ),
            )
            .child(self.render_inbox(c, true, cx))
            .into_any_element()
    }

    fn render_calendar(&self, c: Colors, cx: &mut Context<Self>) -> AnyElement {
        if self.snapshot.events.is_empty() {
            let empty = if self.snapshot.calendar_connected {
                t!("widget-window-no-upcoming-events")
            } else {
                t!("widget-window-connect-a-calendar-in-settings")
            };
            return div()
                .id("desktop-widget-cal-empty")
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .p_6()
                .cursor_pointer()
                .text_sm()
                .text_color(rgb(c.muted))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.dispatch_action(WidgetAction::Calendar, cx);
                }))
                .child(empty)
                .into_any_element();
        }

        let mut list = div()
            .id("desktop-widget-cal-list")
            .track_scroll(&self.scroll)
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px_2()
            .py_1()
            .flex()
            .flex_col()
            .gap(px(4.));

        for (i, ev) in self.snapshot.events.iter().enumerate() {
            let bar_color = parse_hex_color(&ev.calendar_color).unwrap_or(c.accent);
            let time_label = format_event_time(ev);
            list = list.child(
                div()
                    .id(SharedString::from(format!("desktop-widget-evt-{i}")))
                    .px_2()
                    .py(px(7.))
                    .rounded(px(8.))
                    .cursor_pointer()
                    .hover(move |s| s.bg(rgb(c.raised)))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.dispatch_action(WidgetAction::Calendar, cx);
                    }))
                    .flex()
                    .items_stretch()
                    .gap_2()
                    .child(
                        div()
                            .w(px(3.))
                            .rounded(px(2.))
                            .flex_shrink_0()
                            .bg(rgb(bar_color)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(1.))
                            .child(
                                div()
                                    .truncate()
                                    .text_xs()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(c.text))
                                    .child(ev.title.clone()),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .text_xs()
                                    .text_color(rgb(c.muted))
                                    .child(time_label),
                            )
                            .when(!ev.location.is_empty(), |d| {
                                d.child(
                                    div()
                                        .truncate()
                                        .text_xs()
                                        .text_color(rgb(c.muted))
                                        .child(ev.location.clone()),
                                )
                            }),
                    ),
            );
        }
        list.into_any_element()
    }

    /// How many rows of `row` pixels fit the list's height, as laid out
    /// last (all of them before the first layout); and a measure that keeps
    /// that height up to date, asking for a new frame when it changes.
    fn fitting(&self, row: f32) -> (usize, AnyElement) {
        let height = f32::from(self.list_bounds.get().size.height);
        let fit = if height <= 0. {
            usize::MAX
        } else {
            ((height - 2. * LIST_PAD + ROW_GAP) / (row + ROW_GAP)).floor().max(0.) as usize
        };
        let anchor = self.list_bounds.clone();
        let measure = canvas(
            move |bounds, w, _| {
                if anchor.get().size != bounds.size {
                    anchor.set(bounds);
                    w.refresh();
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0()
        .into_any_element();
        (fit, measure)
    }

    /// The bar above Packages or Receipts: "Recent" and "View all N", which
    /// opens them in the Summary.
    fn view_all_bar(
        &self,
        id: &'static str,
        total: usize,
        action: WidgetAction,
        c: Colors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .px_3()
            .pt(px(6.))
            .flex()
            .items_center()
            .child(div().flex_1().text_xs().text_color(rgb(c.muted)).child(t!("widget-window-recent")))
            .when(total > 0, |d| {
                d.child(
                    div()
                        .id(id)
                        .debug_selector(move || id.into())
                        .cursor_pointer()
                        .text_xs()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(rgb(c.accent))
                        .hover(|s| s.opacity(0.75))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.dispatch_action(action.clone(), cx);
                        }))
                        .child(t!("widget-window-view-all", total = total)),
                )
            })
            .into_any_element()
    }

    /// A list of rows that shows as many whole ones as its height fits (no
    /// scrolling, no scaling); "View all" above it has the rest.
    fn fitted_list(id: &'static str, rows: Vec<AnyElement>, measure: AnyElement) -> AnyElement {
        div()
            .id(id)
            .relative()
            .flex_1()
            .min_h_0()
            .overflow_hidden()
            .px_2()
            .py(px(LIST_PAD))
            .flex()
            .flex_col()
            .gap(px(ROW_GAP))
            .child(measure)
            .children(rows)
            .into_any_element()
    }

    fn render_packages(&self, c: Colors, cx: &mut Context<Self>) -> AnyElement {
        let account = self.account.clone();
        let all = widgets::filtered_packages(&self.snapshot, self.account.as_deref(), usize::MAX);
        if all.is_empty() {
            let empty = if self.loading {
                t!("widget-window-loading-packages")
            } else {
                t!("widget-window-no-recent-packages")
            };
            return div()
                .id("desktop-widget-pkg-empty")
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .p_6()
                .cursor_pointer()
                .text_sm()
                .text_color(rgb(c.muted))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.dispatch_action(
                        WidgetAction::Packages {
                            account: account.clone(),
                        },
                        cx,
                    );
                }))
                .child(empty)
                .into_any_element();
        }
        let total = self.snapshot.package_total(self.account.as_deref());
        let (fit, measure) = self.fitting(PACKAGE_ROW);
        let rows = all
            .into_iter()
            .take(fit)
            .enumerate()
            .map(|(i, p)| self.package_row(i, p, c, cx))
            .collect();
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(self.view_all_bar(
                "desktop-widget-pkg-all",
                total,
                WidgetAction::Packages { account },
                c,
                cx,
            ))
            .child(Self::fitted_list("desktop-widget-pkg-list", rows, measure))
            .into_any_element()
    }

    fn package_row(
        &self,
        i: usize,
        p: &WidgetPackage,
        c: Colors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let account_id = p.account_id.clone();
        let fallback_account = self.account.clone();
        let mail_id = p.mail_id.clone();
        let mut detail = vec![p.status.clone()];
        if !p.carrier.is_empty() {
            detail.push(p.carrier.clone());
        }
        if !p.expected.is_empty() {
            detail.push(t!("widget-window-expected", expected = p.expected));
        } else {
            let d = format_short_date(p.updated);
            if !d.is_empty() {
                detail.push(d);
            }
        }
        if self.account.is_none()
            && self.snapshot.accounts.len() > 1
            && !p.account_label.is_empty()
        {
            detail.push(p.account_label.clone());
        }
        let title = if p.title.is_empty() {
            t!("widget-window-package").to_owned()
        } else {
            p.title.clone()
        };
        let stage = p.stage.min(4);
        let delivered = p.delivered;

        let mut bar = div().mt(px(4.)).flex().items_center().gap(px(3.));
        for step in 0..5 {
            bar = bar.child(
                div()
                    .flex_1()
                    .h(px(3.))
                    .rounded_full()
                    .bg(rgb(if step <= stage {
                        if delivered { c.muted } else { c.accent }
                    } else {
                        c.border
                    })),
            );
        }

        div()
            .id(SharedString::from(format!("desktop-widget-pkg-{i}")))
            .h(px(PACKAGE_ROW))
            .flex_shrink_0()
            .overflow_hidden()
            .px_2()
            .py(px(7.))
            .rounded(px(8.))
            .cursor_pointer()
            .hover(move |s| s.bg(rgb(c.raised)))
            .on_click(cx.listener(move |this, _, _, cx| {
                if !account_id.is_empty() && !mail_id.is_empty() {
                    this.dispatch_action(
                        WidgetAction::Message {
                            account: account_id.clone(),
                            id: mail_id.clone(),
                        },
                        cx,
                    );
                } else {
                    this.dispatch_action(
                        WidgetAction::Packages {
                            account: fallback_account.clone(),
                        },
                        cx,
                    );
                }
            }))
            .flex()
            .items_start()
            .gap_2()
            .child(
                div().mt(px(2.)).child(icon_el(
                    if delivered { "package-check" } else { "package" },
                    15.,
                    if delivered { c.muted } else { c.accent },
                )),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(1.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1p5()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_xs()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(c.text))
                                    .child(title),
                            )
                            .when(!p.number.is_empty(), |d| {
                                d.child(
                                    div()
                                        .flex_shrink_0()
                                        .max_w(px(110.))
                                        .truncate()
                                        .text_xs()
                                        .text_color(rgb(c.muted))
                                        .child(p.number.clone()),
                                )
                            }),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_xs()
                            .text_color(rgb(c.muted))
                            .child(detail.join(" · ")),
                    )
                    .child(bar),
            )
            .into_any_element()
    }

    fn render_receipts(&self, c: Colors, cx: &mut Context<Self>) -> AnyElement {
        let account = self.account.clone();
        let all = widgets::filtered_receipts(&self.snapshot, self.account.as_deref(), usize::MAX);
        if all.is_empty() {
            let empty = if self.loading {
                t!("widget-window-loading-receipts")
            } else {
                t!("widget-window-no-recent-receipts")
            };
            return div()
                .id("desktop-widget-rcpt-empty")
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .p_6()
                .cursor_pointer()
                .text_sm()
                .text_color(rgb(c.muted))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.dispatch_action(
                        WidgetAction::Receipts {
                            account: account.clone(),
                        },
                        cx,
                    );
                }))
                .child(empty)
                .into_any_element();
        }
        let total = self.snapshot.receipt_total(self.account.as_deref());
        let (fit, measure) = self.fitting(RECEIPT_ROW);
        let rows = all
            .into_iter()
            .take(fit)
            .enumerate()
            .map(|(i, r)| self.receipt_row(i, r, c, cx))
            .collect();
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(self.view_all_bar(
                "desktop-widget-rcpt-all",
                total,
                WidgetAction::Receipts { account },
                c,
                cx,
            ))
            .child(Self::fitted_list("desktop-widget-rcpt-list", rows, measure))
            .into_any_element()
    }

    fn receipt_row(
        &self,
        i: usize,
        r: &WidgetReceipt,
        c: Colors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let account_id = r.account_id.clone();
        let fallback_account = self.account.clone();
        let mail_id = r.mail_id.clone();
        let mut detail = Vec::new();
        if !r.order.is_empty() {
            detail.push(t!("widget-window-order", order = r.order));
        }
        let date_str = format_short_date(r.updated.max(r.date));
        if !date_str.is_empty() {
            detail.push(date_str);
        }
        if self.account.is_none()
            && self.snapshot.accounts.len() > 1
            && !r.account_label.is_empty()
        {
            detail.push(r.account_label.clone());
        }
        let refund = r.refund;

        div()
            .id(SharedString::from(format!("desktop-widget-rcpt-{i}")))
            .h(px(RECEIPT_ROW))
            .flex_shrink_0()
            .overflow_hidden()
            .px_2()
            .py(px(7.))
            .rounded(px(8.))
            .cursor_pointer()
            .hover(move |s| s.bg(rgb(c.raised)))
            .on_click(cx.listener(move |this, _, _, cx| {
                if !account_id.is_empty() && !mail_id.is_empty() {
                    this.dispatch_action(
                        WidgetAction::Message {
                            account: account_id.clone(),
                            id: mail_id.clone(),
                        },
                        cx,
                    );
                } else {
                    this.dispatch_action(
                        WidgetAction::Receipts {
                            account: fallback_account.clone(),
                        },
                        cx,
                    );
                }
            }))
            .flex()
            .items_center()
            .gap_2()
            .child(icon_el("receipt", 15., c.accent))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(1.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_xs()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(c.text))
                                    .child(r.merchant.clone()),
                            ),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_xs()
                            .text_color(rgb(c.muted))
                            .child(detail.join(" · ")),
                    ),
            )
            .child(
                // The amount, with "Refund" under a refund's.
                div()
                    .flex_shrink_0()
                    .flex()
                    .flex_col()
                    .items_end()
                    .text_color(rgb(if refund { c.accent } else { c.text }))
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(r.amount.clone()),
                    )
                    .when(refund, |d| {
                        d.child(div().text_size(px(10.)).child(t!("widget-window-refund")))
                    }),
            )
            .into_any_element()
    }
}

impl Render for DesktopWidgetWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = self.colors(cx);
        if super::applock::covered(cx) {
            return super::applock::cover(c, window);
        }
        let unread = widgets::unread_for(&self.snapshot, self.account.as_deref());
        let compose_account = self.account.clone();
        let inbox_account = self.account.clone();

        let titlebar = window::detached_titlebar(
            window::detached_bar("desktop-widget-titlebar", c)
                .child(
                    window::no_drag(
                        div()
                            .id("desktop-widget-open-main")
                            .flex()
                            .items_center()
                            .gap_1p5()
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let action = match this.tab {
                                    WidgetTab::Calendar => WidgetAction::Calendar,
                                    WidgetTab::Packages => WidgetAction::Packages {
                                        account: inbox_account.clone(),
                                    },
                                    WidgetTab::Receipts => WidgetAction::Receipts {
                                        account: inbox_account.clone(),
                                    },
                                    _ => WidgetAction::Inbox {
                                        account: inbox_account.clone(),
                                    },
                                };
                                this.dispatch_action(action, cx);
                            }))
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(c.text))
                                    .child("Picacho"),
                            )
                            .when(unread > 0, |d| {
                                d.child(
                                    div()
                                        .px(px(6.))
                                        .py(px(1.))
                                        .rounded_full()
                                        .bg(rgb(c.soft))
                                        .text_xs()
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(rgb(c.accent))
                                        .child(if unread > 999 {
                                            "999+".to_owned()
                                        } else {
                                            unread.to_string()
                                        }),
                                )
                            }),
                    ),
                )
                .child(div().flex_1())
                .child(window::no_drag(
                    div()
                        .id("desktop-widget-refresh")
                        .size(px(24.))
                        .rounded(px(6.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .hover(move |s| s.bg(rgb(c.raised)))
                        .on_click(cx.listener(|this, _, _, cx| this.refresh(cx)))
                        .child(icon_el("refresh", 12., c.muted)),
                ))
                .child(window::no_drag(
                    div()
                        .id("desktop-widget-compose")
                        .px_2()
                        .py(px(4.))
                        .rounded(px(6.))
                        .bg(rgb(c.accent))
                        .text_color(rgb(c.on_accent))
                        .hover(move |s| s.bg(rgb(c.accent_hover)))
                        .cursor_pointer()
                        .text_xs()
                        .font_weight(FontWeight::MEDIUM)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.dispatch_action(
                                WidgetAction::Compose {
                                    account: compose_account.clone(),
                                },
                                cx,
                            );
                        }))
                        .child(t!("widget-window-compose")),
                )),
            c,
            |_, w, _| w.remove_window(),
            window,
            cx,
        );

        let tab_btn = |id: &'static str, label: &'static str, tab: WidgetTab, active: bool| {
            div()
                .id(id)
                .px_2p5()
                .py(px(4.))
                .rounded(px(6.))
                .cursor_pointer()
                .text_xs()
                .font_weight(if active {
                    FontWeight::SEMIBOLD
                } else {
                    FontWeight::NORMAL
                })
                .bg(rgb(if active { c.soft } else { c.bg }))
                .text_color(rgb(if active { c.accent } else { c.muted }))
                .hover(move |s| s.bg(rgb(if active { c.soft } else { c.raised })))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.tab = tab;
                    this.scroll.set_offset(point(px(0.), px(0.)));
                    cx.notify();
                }))
                .child(label)
        };

        let controls = div()
            .px_2p5()
            .py(px(6.))
            .border_b_1()
            .border_color(rgb(c.border))
            .flex()
            .flex_wrap()
            .items_center()
            .gap_1()
            .child(tab_btn("widget-tab-inbox", t!("widget-window-inbox"), WidgetTab::Inbox, self.tab == WidgetTab::Inbox))
            .child(tab_btn("widget-tab-unread", t!("widget-window-unread"), WidgetTab::Unread, self.tab == WidgetTab::Unread))
            .child(tab_btn(
                "widget-tab-calendar",
                t!("widget-window-calendar"),
                WidgetTab::Calendar,
                self.tab == WidgetTab::Calendar,
            ))
            .child(tab_btn(
                "widget-tab-packages",
                t!("widget-window-packages"),
                WidgetTab::Packages,
                self.tab == WidgetTab::Packages,
            ))
            .child(tab_btn(
                "widget-tab-receipts",
                t!("widget-window-receipts"),
                WidgetTab::Receipts,
                self.tab == WidgetTab::Receipts,
            ))
            .child(div().flex_1())
            .when(
                self.tab != WidgetTab::Calendar && self.snapshot.accounts.len() > 1,
                |d| {
                    d.child(
                        div()
                            .id("desktop-widget-account-filter")
                            .px_2()
                            .py(px(3.))
                            .rounded(px(6.))
                            .bg(rgb(c.surface))
                            .hover(move |s| s.bg(rgb(c.raised)))
                            .cursor_pointer()
                            .max_w(px(120.))
                            .truncate()
                            .text_xs()
                            .text_color(rgb(c.muted))
                            .on_click(cx.listener(|this, _, _, cx| this.cycle_account(cx)))
                            .child(self.account_label()),
                    )
                },
            );

        let body = match self.tab {
            WidgetTab::Inbox => self.render_inbox(c, false, cx),
            WidgetTab::Unread => self.render_unread(c, cx),
            WidgetTab::Calendar => self.render_calendar(c, cx),
            WidgetTab::Packages => self.render_packages(c, cx),
            WidgetTab::Receipts => self.render_receipts(c, cx),
        };

        window::window_frame(
            div()
                .id("desktop-widget-window")
                .debug_selector(|| "desktop-widget-window".into())
                .track_focus(&self.focus)
                .size_full()
                .flex()
                .flex_col()
                .bg(rgb(c.bg))
                .text_color(rgb(c.text))
                .child(titlebar)
                .child(controls)
                .child(body),
            c,
            window,
        )
        .into_any_element()
    }
}

fn parse_hex_color(hex: &str) -> Option<u32> {
    let stripped = hex.trim().strip_prefix('#')?;
    if stripped.len() != 6 {
        return None;
    }
    u32::from_str_radix(stripped, 16).ok()
}

fn format_short_date(ts: i64) -> String {
    if ts <= 0 {
        return String::new();
    }
    let Some(dt) = Local.timestamp_opt(ts, 0).single() else {
        return String::new();
    };
    let now = Local::now();
    if dt.date_naive() == now.date_naive() {
        dt.local(t!("dates-calendar-time"))
    } else if (now.date_naive() - dt.date_naive()).num_days().abs() < 6 {
        dt.local("%a")
    } else if dt.year() == now.year() {
        dt.local(t!("dates-month-day"))
    } else {
        dt.local(t!("dates-month-day-year"))
    }
}

fn format_event_time(ev: &WidgetEvent) -> String {
    let today = Local::now().date_naive();
    let tomorrow = today + chrono::Duration::days(1);
    if ev.all_day {
        let day = Utc
            .timestamp_opt(ev.start, 0)
            .single()
            .map(|d| d.date_naive())
            .map(|d| day_label(today, tomorrow, d))
            .unwrap_or_else(|| t!("widget-window-today").to_owned());
        return t!("widget-window-all-day", day = day);
    }
    let Some(start) = Local.timestamp_opt(ev.start, 0).single() else {
        return String::new();
    };
    let day = day_label(today, tomorrow, start.date_naive());
    let start_str = start.local(t!("dates-calendar-time"));
    if ev.end > ev.start
        && let Some(end) = Local.timestamp_opt(ev.end, 0).single()
    {
        return format!("{day} · {start_str} – {}", end.local(t!("dates-calendar-time")));
    }
    format!("{day} · {start_str}")
}

fn day_label(today: chrono::NaiveDate, tomorrow: chrono::NaiveDate, date: chrono::NaiveDate) -> String {
    if date == today {
        t!("widget-window-today").to_owned()
    } else if date == tomorrow {
        t!("widget-window-tomorrow").to_owned()
    } else {
        date.local(t!("dates-weekday-month-day"))
    }
}

impl MailApp {
    /// Opens Picacho's compact Desktop Widget window, or brings it forward if
    /// already open.
    pub(super) fn open_widget_window(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        if let Some(handle) = self.widget_window {
            let alive = cx
                .windows()
                .iter()
                .any(|win| win.window_id() == handle.window_id());
            if alive {
                let _ = handle.update(cx, |_, win, _| win.activate_window());
                return;
            }
            self.widget_window = None;
        }

        let store = self.backend.store.clone();
        let (snap, loading) = match widgets::read_cached(&store.path) {
            Some(cached) => (cached, false),
            None => (
                Snapshot::empty(self.accent.key(), Utc::now().timestamp()),
                true,
            ),
        };
        let app = cx.weak_entity();
        let main = w.window_handle();
        let bounds = Bounds::centered(None, size(px(360.), px(460.)), cx);
        let mut options = window::detached_options(t!("widget-window-picacho-widget").into(), bounds);
        options.window_min_size = Some(size(px(260.), px(240.)));
        match cx.open_window(options, move |window, cx| {
            let view = cx.new(|cx| DesktopWidgetWindow::new(app, main, store, snap, loading, cx));
            {
                let focus = view.read(cx).focus.clone();
                focus.focus(window, cx);
            }
            cx.new(|cx| Root::new(view, window, cx))
        }) {
            Ok(handle) => {
                self.widget_window = Some(handle.into());
            }
            Err(e) => {
                self.notice(t!("widget-window-could-not-open-desktop-widget", e = format!("{:#}", e)), true, cx);
            }
        }
    }
}

//! The Summary's Receipts section: what an account's mail (or every
//! account's) says was paid, newest first (see `manage::receipts`). The
//! Summary's Receipts widget shows the last few and what was spent this
//! month; its Receipts page lists every one by month, each month with its
//! total, and searches them by who was paid, order, amount, subject,
//! sender or what their emails say. Clicking a receipt opens its email;
//! one taken for a receipt wrongly can be hidden.
use super::summary::{WIDGET_WIDTH, tag};
use super::*;
use super::summary::SummaryItem;
use crate::manage::receipts::{self, Amount, Kind, LineKind, Listed};
use chrono::{Datelike, TimeZone};

/// Receipts the Summary's Receipts widget lists; the rest are on its
/// Receipts page.
const WIDGET_RECEIPTS: usize = 5;
/// The preference holding the receipts hidden by hand, by key.
const HIDDEN: &str = receipts::HIDDEN_PREFERENCE;
/// Hidden receipts kept at most; the oldest go first.
const MAX_HIDDEN: usize = 2000;

impl MailApp {
    /// The receipts hidden by hand, as saved.
    fn hidden_receipts(&self) -> Vec<String> {
        self.backend
            .store
            .preference(HIDDEN)
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_default()
    }

    /// Marks the Summary's receipts hidden as saved.
    pub(super) fn apply_hidden_receipts(&mut self) {
        let hidden = self.hidden_receipts();
        receipts::apply_hidden(&mut self.receipts, &hidden);
    }

    /// Hides the Summary's receipt `key` (or shows it again), and saves
    /// that.
    pub(super) fn hide_receipt(&mut self, key: &str, hide: bool, cx: &mut Context<Self>) {
        let mut hidden = self.hidden_receipts();
        hidden.retain(|k| k != key);
        if hide {
            hidden.push(key.to_owned());
        }
        if hidden.len() > MAX_HIDDEN {
            hidden.drain(..hidden.len() - MAX_HIDDEN);
        }
        if let Ok(json) = serde_json::to_string(&hidden) {
            self.backend.store.set_preference_soon(HIDDEN, &json);
        }
        receipts::apply_hidden(&mut self.receipts, &hidden);
        // The search's `has:receipt` follows at once, without working out
        // every kind again.
        if let Some(l) = self.receipts.iter().find(|l| l.key() == key)
            && let Some(kinds) = self.mail_kinds.as_mut()
        {
            let found = &mut Arc::make_mut(kinds).receipts;
            for s in &l.emails {
                if hide {
                    found.remove(&s.mail_id);
                } else {
                    found.insert(s.mail_id.clone());
                }
            }
            self.filter(cx);
        }
        cx.notify();
    }

    /// Offers what can be done with the Summary's receipt `key`, at the
    /// pointer: open its email, copy its order number, hide it.
    pub(super) fn receipt_menu(&mut self, key: &str, w: &mut Window, cx: &mut Context<Self>) {
        let Some(l) = self.receipts.iter().find(|l| l.key() == key) else {
            return;
        };
        let mut entries = vec![entry("Open Email", "mail-open", open_email(l))];
        if !l.receipt.lines.is_empty() {
            let shown = self.summary_expanded.contains(&format!("receipt:{key}"));
            entries.push(entry(
                if shown { "Hide Details" } else { "Show Details" },
                "list",
                Action::ToggleReceipt(key.to_owned()),
            ));
        }
        if let Some(order) = &l.receipt.order {
            entries.push(entry(
                "Copy Order Number",
                "copy",
                Action::CopyText(order.clone()),
            ));
        }
        if let Some(total) = &l.receipt.total {
            entries.push(entry("Copy Amount", "copy", Action::CopyText(total.show())));
        }
        entries.push(Entry::Separator);
        let key = key.to_owned();
        entries.push(if l.hidden {
            entry("Show Receipt", "eye", Action::HideReceipt(key, false))
        } else {
            entry("Hide Receipt", "eye-off", Action::HideReceipt(key, true))
        });
        self.open_menu_here(entries, w, cx);
    }

    /// Opens the Receipts page's search field, focused, or closes it and
    /// clears what was typed.
    pub(super) fn search_receipts(&mut self, open: bool, w: &mut Window, cx: &mut Context<Self>) {
        self.receipts_search = open;
        if open {
            self.summary_receipts = true;
            self.summary_packages = false;
            self.receipt_search.update(cx, |s, cx| s.focus(w, cx));
            self.load_parcel_texts(cx);
        } else {
            self.receipt_search
                .update(cx, |s, cx| s.set_value("", w, cx));
            self.focus.focus(w, cx);
        }
        cx.notify();
    }

    /// What is typed in the Receipts page's search field, when it is open.
    fn receipt_query(&self, cx: &App) -> Option<String> {
        self.receipts_search
            .then(|| self.receipt_search.read(cx).value().trim().to_lowercase())
            .filter(|q| !q.is_empty())
    }

    /// The receipts the Receipts page lists: as `listed_receipts`, and
    /// those matching the search when there is one, each with whether only
    /// what its emails say matched.
    fn shown_receipts<'a>(&'a self, cx: &App) -> Vec<(&'a Listed, bool)> {
        let query = self.receipt_query(cx);
        let listed: Vec<&Listed> = self.listed_receipts(self.receipts_show_hidden).collect();
        let Some(q) = query else {
            return listed.into_iter().map(|l| (l, false)).collect();
        };
        // Search results again only once something they depend on changed
        // (see `search_key`), not on every frame.
        let key = self.search_key(
            &q,
            listed.iter().map(|l| (l.first().mail_id.as_str(), l.emails.len(), l.hidden)),
        );
        let found = self.cached_search("receipts", key, || {
            listed
                .iter()
                .enumerate()
                .filter_map(|(i, l)| {
                    let account = self.account_name(&l.first().account);
                    receipt_matches(l, &q, account.as_deref(), &self.parcel_texts).map(|e| (i, e))
                })
                .collect()
        });
        found.iter().filter_map(|&(i, e)| Some((*listed.get(i)?, e))).collect()
    }

    /// The Receipts page's search button: opens the search field, or
    /// closes it when it is open.
    fn search_receipts_button(&self, cx: &Context<Self>) -> AnyElement {
        let c = self.colors();
        let open = self.receipts_search;
        div()
            .id("summary-receipts-search-button")
            .debug_selector(|| "summary-receipts-search-button".into())
            .size(px(if self.mobile.on {
                super::mobile::TOUCH
            } else {
                26.
            }))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(6.))
            .cursor_pointer()
            .when(open, |d| d.bg(rgb(c.soft)))
            .hover(move |s| s.bg(rgb(c.border)))
            .child(icon_el(
                "search",
                14.,
                if open { c.accent } else { c.muted },
            ))
            .tooltip(|w, cx| Tooltip::new("Search Receipts").build(w, cx))
            .on_click(
                cx.listener(move |this, _, w, cx| {
                    this.handle(Action::SearchReceipts(!open), w, cx)
                }),
            )
            .into_any_element()
    }

    /// The Summary's receipts as listed: without the hidden, unless they
    /// are asked for on the Receipts page.
    pub(super) fn listed_receipts(&self, with_hidden: bool) -> impl Iterator<Item = &Listed> {
        self.receipts
            .iter()
            .filter(move |l| with_hidden || !l.hidden)
    }

    /// "$412.30 this month · 14 in all", or "14 in all".
    fn receipts_count(&self, count: usize) -> Option<Stateful<Div>> {
        let c = self.colors();
        let start = month_start(chrono::Utc::now().timestamp());
        let spent = receipts::totals(self.listed_receipts(false).filter(|l| l.date >= start));
        (count > 0).then(|| {
            div()
                .id("summary-receipts-count")
                .debug_selector(|| "summary-receipts-count".into())
                .text_size(px(12.))
                .text_color(rgb(c.muted))
                .child(if spent.is_empty() {
                    format!("{count} in all")
                } else {
                    format!("{} this month · {count} in all", show_totals(&spent))
                })
        })
    }

    /// The Receipts widget: the last few receipts, and a link to the
    /// Receipts page with every one.
    pub(super) fn receipts_widget(
        &self,
        single: bool,
        all: bool,
        cx: &Context<Self>,
    ) -> AnyElement {
        let c = self.colors();
        let count = self.listed_receipts(false).count();
        let shown = count.min(WIDGET_RECEIPTS);
        div()
            .id("summary-receipts-widget")
            .debug_selector(|| "summary-receipts-widget".into())
            .when(single, |d| d.w_full())
            .when(!single, |d| d.w(px(WIDGET_WIDTH)))
            .max_w_full()
            .flex()
            .flex_col()
            .rounded(px(10.))
            .border_1()
            .border_color(rgb(c.border))
            .bg(rgb(c.surface))
            .child(
                div()
                    .px_3()
                    .py_2()
                    .flex()
                    .items_center()
                    .gap_2()
                    .border_b_1()
                    .border_color(rgb(c.border))
                    .child(icon_el("receipt", 15., c.accent))
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(13.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Receipts"),
                    )
                    .children(self.receipts_count(count)),
            )
            .when(count == 0, |d| {
                d.child(
                    div()
                        .id("summary-receipts-empty")
                        .debug_selector(|| "summary-receipts-empty".into())
                        .px_3()
                        .py_4()
                        .flex()
                        .items_center()
                        .gap_2()
                        .text_size(px(12.))
                        .text_color(rgb(c.muted))
                        .child(icon_el("receipt", 18., c.border))
                        .child(self.receipts_empty_text()),
                )
            })
            .children(
                self.listed_receipts(false)
                    .take(shown)
                    .enumerate()
                    .map(|(i, l)| self.widget_receipt(i, l, all, cx)),
            )
            // Also when every receipt is hidden: the Receipts page shows them.
            .when(!self.receipts.is_empty(), |d| {
                d.child(
                    div()
                        .px_3()
                        .py_2()
                        .border_t_1()
                        .border_color(rgb(c.border))
                        .flex()
                        .items_center()
                        .child(
                            div()
                                .flex_1()
                                .text_size(px(11.))
                                .text_color(rgb(c.muted))
                                .child(if count > shown {
                                    format!("{shown} of {count}")
                                } else {
                                    String::new()
                                }),
                        )
                        .child(self.summary_link(
                            "summary-receipts-more",
                            "View more…",
                            Action::SummaryReceipts(true),
                            cx,
                        )),
                )
            })
            .into_any_element()
    }

    /// What an empty Receipts section says.
    fn receipts_empty_text(&self) -> &'static str {
        if self.summary_loading && self.receipts.is_empty() {
            "Looking through your mail for receipts…"
        } else {
            "No receipts in this mail"
        }
    }

    /// A receipt in the Receipts widget: who was paid, when, and how much;
    /// clicking it opens its email.
    fn widget_receipt(&self, i: usize, l: &Listed, all: bool, cx: &Context<Self>) -> AnyElement {
        let c = self.colors();
        let account = all.then(|| self.account_name(&l.first().account)).flatten();
        let mut detail = Vec::new();
        if let Some(order) = &l.receipt.order {
            detail.push(format!("Order {order}"));
        }
        detail.push(date_day(l.date));
        let key = l.key();
        let open = open_email(l);
        div()
            .id(SharedString::from(format!("summary-widget-receipt-{i}")))
            .debug_selector(move || format!("summary-widget-receipt-{i}"))
            .px_3()
            .py(px(6.))
            .flex()
            .items_center()
            .gap_2()
            .cursor_pointer()
            .when(i > 0, |d| d.border_t_1().border_color(rgb(c.border)))
            .hover(move |s| s.bg(rgb(c.raised)))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, _: &MouseDownEvent, w, cx| {
                    this.handle(Action::ReceiptMenu(key.clone()), w, cx)
                }),
            )
            .on_click(cx.listener(move |this, _, w, cx| this.handle(open.clone(), w, cx)))
            .child(icon_el("receipt", 16., c.accent))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(px(12.))
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(l.merchant.clone()),
                            )
                            .when(l.receipt.kind == Kind::Refund, |d| {
                                d.child(tag("Refund", c))
                            })
                            .when_some(account, |d, name| d.child(tag(&name, c))),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_size(px(11.))
                            .text_color(rgb(c.muted))
                            .child(detail.join(" · ")),
                    ),
            )
            .child(self.receipt_amount(&format!("summary-widget-amount-{i}"), l))
            .into_any_element()
    }

    /// A receipt's amount, at its row's end: a refund's as money back.
    fn receipt_amount(&self, id: &str, l: &Listed) -> AnyElement {
        let c = self.colors();
        let selector = id.to_owned();
        let refund = l.receipt.kind == Kind::Refund;
        div()
            .id(SharedString::from(id.to_owned()))
            .debug_selector(move || selector)
            .flex_shrink_0()
            .text_size(px(12.))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(rgb(if refund { c.accent } else { c.text }))
            .child(match &l.receipt.total {
                Some(total) if refund => format!("+{}", total.show()),
                Some(total) => total.show(),
                None => "—".to_owned(),
            })
            .into_any_element()
    }

    /// The Receipts page: every receipt, by month, newest first.
    /// The Receipts page's top: its title, count, search and notices.
    fn receipts_header(&self, cx: &Context<Self>) -> AnyElement {
        let c = self.colors();
        let count = self.listed_receipts(false).count();
        let hidden = self.receipts.len() - count;
        let show_hidden = self.receipts_show_hidden;
        div()
            .id("summary-receipts-page")
            .debug_selector(|| "summary-receipts-page".into())
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(icon_el("receipt", 16., c.accent))
                    .child(
                        div()
                            .text_size(px(14.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Receipts"),
                    )
                    .children(self.receipts_count(count))
                    .child(div().flex_1())
                    .child(self.search_receipts_button(cx))
                    .when(hidden > 0, |d| {
                        d.child(self.summary_link(
                            "summary-receipts-show-hidden",
                            &if show_hidden {
                                format!("Leave Out {hidden} Hidden")
                            } else {
                                format!("Show {hidden} Hidden")
                            },
                            Action::ShowHiddenReceipts(!show_hidden),
                            cx,
                        ))
                    }),
            )
            .when(self.receipts_search, |d| {
                d.child(
                    div()
                        .id("summary-receipts-search")
                        .debug_selector(|| "summary-receipts-search".into())
                        .w_full()
                        .max_w(px(420.))
                        // Escape typed in the field reaches here as the
                        // field's own action, not the window's Close.
                        .on_action(
                            cx.listener(|this, _: &gpui_component::input::Escape, w, cx| {
                                this.handle(Action::SearchReceipts(false), w, cx)
                            }),
                        )
                        .child(field(
                            Input::new(&self.receipt_search)
                                .prefix(icon_el("search", 13., c.muted))
                                .cleanable(true),
                            c,
                            false,
                        )),
                )
            })
            .when(self.receipts_search && self.parcel_texts_loading, |d| {
                d.child(
                    div()
                        .id("summary-receipts-search-reading")
                        .debug_selector(|| "summary-receipts-search-reading".into())
                        .flex()
                        .items_center()
                        .gap_2()
                        .text_size(px(12.))
                        .text_color(rgb(c.muted))
                        .child(spinner(12., c.muted))
                        .child("Searching emails…"),
                )
            })
            .children(self.summary_download_status(false, cx))
            .into_any_element()
    }

    /// What the Receipts page shows when it lists no receipt.
    fn receipts_empty(&self, cx: &Context<Self>) -> AnyElement {
        let c = self.colors();
        let show_hidden = self.receipts_show_hidden;
        let searched = self.receipt_query(cx).is_some();
        div().pt_3().child(
                    div()
                        .id("summary-receipts-empty")
                        .debug_selector(|| "summary-receipts-empty".into())
                        .py_6()
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap_2()
                        .rounded(px(10.))
                        .border_1()
                        .border_color(rgb(c.border))
                        .text_size(px(13.))
                        .text_color(rgb(c.muted))
                        .child(icon_el("receipt", 28., c.border))
                        .child(
                            if searched && self.listed_receipts(show_hidden).next().is_some() {
                                "No receipts match your search"
                            } else {
                                self.receipts_empty_text()
                            },
                        ),
        )
        .into_any_element()
    }

    /// What the Receipts page says under its receipts.
    fn receipts_footer(&self) -> AnyElement {
        let c = self.colors();
        div().pt_3().child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(c.muted))
                    .child(format!(
                        "Found in {} emails on this device. Amounts are what the \
                         emails say; nothing is looked up online.",
                        fmt_count(self.receipts_scanned),
                    )),
        )
        .into_any_element()
    }

    /// The Receipts page's rows: each month's heading, then its receipts.
    fn receipt_items<'a>(&'a self, cx: &App) -> Vec<ReceiptItem<'a>> {
        let mut items = Vec::new();
        let mut month = None;
        let shown = self.shown_receipts(cx);
        for (i, &(l, in_email)) in shown.iter().enumerate() {
            let start = month_start(l.date);
            if month != Some(start) {
                month = Some(start);
                let list: Vec<&Listed> = shown[i..]
                    .iter()
                    .map(|(l, _)| *l)
                    .take_while(|l| month_start(l.date) == start)
                    .collect();
                items.push(ReceiptItem::Month { start, list });
            }
            items.push(ReceiptItem::Receipt { l, in_email });
        }
        items
    }

    /// The Receipts page, its rows virtualized (see `summary_list`).
    pub(super) fn receipts_page(&self, cx: &Context<Self>) -> AnyElement {
        let items = self.receipt_items(cx);
        // For the rows built this frame (see `summary_rows`): each receipt by
        // its place in `receipts`, a month's heading as `usize::MAX`.
        let rows: Vec<(usize, bool)> = items
            .iter()
            .map(|item| match item {
                ReceiptItem::Month { .. } => (usize::MAX, false),
                ReceiptItem::Receipt { l, in_email, .. } => {
                    (super::summary::index_in(&self.receipts, l).unwrap_or(usize::MAX), *in_email)
                }
            })
            .collect();
        *self.summary_rows.borrow_mut() = Rc::new(rows);
        let key = {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            for item in &items {
                match item {
                    ReceiptItem::Month { start, list, .. } => (0u8, *start, list.len()).hash(&mut h),
                    ReceiptItem::Receipt { l, in_email, .. } => {
                        (1u8, l.key(), l.emails.len(), l.hidden, in_email).hash(&mut h)
                    }
                }
            }
            let mut open: Vec<&String> = self.summary_expanded.iter().collect();
            open.sort();
            open.hash(&mut h);
            (self.receipts_search, self.parcel_texts_loading, self.summary_downloads, self.receipts_scanned).hash(&mut h);
            (self.dark, self.receipts_show_hidden).hash(&mut h);
            h.finish()
        };
        let count = items.len().max(1);
        self.summary_list("receipts", count, key, cx, |this, item, cx| {
            let all = this.account.as_deref() == Some(fwd::ALL_MAIL);
            match item {
                SummaryItem::Header => this.receipts_header(cx),
                SummaryItem::Footer => this.receipts_footer(),
                SummaryItem::Row { i, .. } => this.receipt_list_row(i, all, cx),
            }
        })
    }

    /// Row `i` of the Receipts page's list: a month's heading, or a receipt
    /// in its month's card. From the rows made as the page was drawn
    /// (`summary_rows`); the neighbours say where a month starts and ends.
    fn receipt_list_row(&self, i: usize, all: bool, cx: &Context<Self>) -> AnyElement {
        let rows = self.summary_rows.borrow().clone();
        let Some(&(at, in_email)) = rows.get(i) else {
            return self.receipts_empty(cx);
        };
        let receipt = |r: Option<&(usize, bool)>| r.and_then(|&(at, _)| self.receipts.get(at));
        let Some(l) = receipt(Some(&(at, in_email))) else {
            // A month's heading: its receipts are the rows up to the next.
            let list: Vec<&Listed> = rows[i + 1..]
                .iter()
                .take_while(|(at, _)| *at != usize::MAX)
                .filter_map(|r| receipt(Some(r)))
                .collect();
            let m = rows[..i].iter().filter(|(at, _)| *at == usize::MAX).count();
            let start = list.first().map_or(0, |l| month_start(l.date));
            return self.receipts_month(m, start, &list);
        };
        let first = i == 0 || rows[i - 1].0 == usize::MAX;
        let last = rows.get(i + 1).is_none_or(|(at, _)| *at == usize::MAX);
        // Its place among the receipts shown, as its ids have it.
        let n = rows[..i].iter().filter(|(at, _)| *at != usize::MAX).count();
        let row = self.receipt_row(n, !first, l, all, in_email, cx);
        self.card_edge_below(row, last)
    }

    /// A receipt row in its month's card: the card's sides, and its rounded
    /// bottom under the month's last (the month's heading is its top).
    fn card_edge_below(&self, row: AnyElement, last: bool) -> AnyElement {
        let c = self.colors();
        div()
            .border_l_1()
            .border_r_1()
            .border_color(rgb(c.border))
            .when(last, |d| d.border_b_1().rounded_b(px(10.)))
            .child(row)
            .into_any_element()
    }

    /// A month's heading on the Receipts page: its name and what was spent,
    /// the top of the card its receipts make (see `receipt_items`).
    fn receipts_month(&self, m: usize, start: i64, list: &[&Listed]) -> AnyElement {
        let c = self.colors();
        let spent = receipts::totals(list.iter().copied().filter(|l| !l.hidden));
        let name = chrono::Local
            .timestamp_opt(start, 0)
            .single()
            .map(|d| d.format("%B %Y").to_string())
            .unwrap_or_default();
        div()
            .id(SharedString::from(format!("summary-receipts-month-{m}")))
            .debug_selector(move || format!("summary-receipts-month-{m}"))
            .mt_3()
            .px_3()
            .py_2()
            .flex()
            .items_center()
            .gap_2()
            .border_1()
            .border_color(rgb(c.border))
            .bg(rgb(c.raised))
            .rounded_t(px(10.))
            .child(
                div()
                    .flex_1()
                    .text_size(px(12.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(name),
            )
            .when(!spent.is_empty(), |d| {
                d.child(
                    div()
                        .id(SharedString::from(format!("summary-receipts-month-total-{m}")))
                        .debug_selector(move || format!("summary-receipts-month-total-{m}"))
                        .text_size(px(12.))
                        .text_color(rgb(c.muted))
                        .child(show_totals(&spent)),
                )
            })
            .into_any_element()
    }

    /// A receipt on the Receipts page: who was paid, its email's subject,
    /// its order and date, and how much; clicking it opens its email.
    fn receipt_row(
        &self,
        i: usize,
        border: bool,
        l: &Listed,
        all: bool,
        in_email: bool,
        cx: &Context<Self>,
    ) -> AnyElement {
        let c = self.colors();
        let account = all.then(|| self.account_name(&l.first().account)).flatten();
        let mut detail = Vec::new();
        if let Some(order) = &l.receipt.order {
            detail.push(format!("Order {order}"));
        }
        detail.push(date_full(l.date));
        if l.emails.len() > 1 {
            detail.push(format!("{} emails", l.emails.len()));
        }
        let key = l.key();
        let open = open_email(l);
        let has_lines = !l.receipt.lines.is_empty();
        let expanded = has_lines && self.summary_expanded.contains(&format!("receipt:{key}"));
        let row = div()
            .id(SharedString::from(format!("summary-receipt-{i}")))
            .debug_selector(move || format!("summary-receipt-{i}"))
            .px_3()
            .py_2()
            .flex()
            .items_center()
            .gap_3()
            .cursor_pointer()
            .hover(move |s| s.bg(rgb(c.raised)))
            .on_mouse_down(MouseButton::Right, {
                let key = key.clone();
                cx.listener(move |this, _: &MouseDownEvent, w, cx| {
                    this.handle(Action::ReceiptMenu(key.clone()), w, cx)
                })
            })
            .on_click(cx.listener(move |this, _, w, cx| this.handle(open.clone(), w, cx)))
            .child(icon_el(
                "receipt",
                18.,
                if l.hidden { c.muted } else { c.accent },
            ))
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
                            .gap_2()
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(px(13.))
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(l.merchant.clone()),
                            )
                            .when(l.receipt.kind == Kind::Refund, |d| {
                                d.child(tag("Refund", c))
                            })
                            .when(l.receipt.source == receipts::Source::Markup, |d| {
                                d.child(tag("From sender", c))
                            })
                            .when_some(account, |d, name| d.child(tag(&name, c)))
                            .when(l.hidden, |d| d.child(tag("Hidden", c)))
                            .when(in_email, |d| {
                                d.child(
                                    div()
                                        .id(SharedString::from(format!(
                                            "summary-receipt-in-email-{i}"
                                        )))
                                        .debug_selector(move || {
                                            format!("summary-receipt-in-email-{i}")
                                        })
                                        .child(tag("Matched in an email", c)),
                                )
                            }),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_size(px(12.))
                            .child(l.emails[0].subject.clone()),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_size(px(12.))
                            .text_color(rgb(c.muted))
                            .child(detail.join(" · ")),
                    ),
            )
            .child(self.receipt_amount(&format!("summary-amount-{i}"), l))
            .when(has_lines, |d| {
                d.child(self.summary_link(
                    &format!("summary-receipt-details-{i}"),
                    if expanded { "Less" } else { "Details" },
                    Action::ToggleReceipt(key.clone()),
                    cx,
                ))
            })
            // Phones have no right click: hiding is a link.
            .child(self.summary_link(
                &format!("summary-receipt-hide-{i}"),
                if l.hidden { "Show" } else { "Hide" },
                Action::HideReceipt(key, !l.hidden),
                cx,
            ));
        div()
            .flex()
            .flex_col()
            .when(border, |d| d.border_t_1().border_color(rgb(c.border)))
            .child(row)
            .when(expanded, |d| d.child(self.receipt_lines(i, l)))
            .into_any_element()
    }

    /// What a receipt lists, under its row: each item, tax, discount… with
    /// its amount, the total last and set apart.
    fn receipt_lines(&self, i: usize, l: &Listed) -> AnyElement {
        let c = self.colors();
        div()
            .id(SharedString::from(format!("summary-receipt-lines-{i}")))
            .debug_selector(move || format!("summary-receipt-lines-{i}"))
            .pl(px(42.))
            .pr_3()
            .pb_2()
            .flex()
            .flex_col()
            .gap(px(2.))
            .children(l.receipt.lines.iter().enumerate().map(|(j, line)| {
                let total = line.kind == LineKind::Total;
                let minor = matches!(line.kind, LineKind::Subtotal | LineKind::Total);
                let discount = line.amount.cents < 0;
                div()
                    .id(SharedString::from(format!("summary-receipt-line-{i}-{j}")))
                    .debug_selector(move || format!("summary-receipt-line-{i}-{j}"))
                    .flex()
                    .items_center()
                    .gap_3()
                    .text_size(px(12.))
                    .when(minor, |d| d.pt(px(2.)).border_t_1().border_color(rgb(c.border)))
                    .when(total, |d| d.font_weight(FontWeight::SEMIBOLD))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(rgb(if line.kind == LineKind::Item || total {
                                c.text
                            } else {
                                c.muted
                            }))
                            .child(line.label.clone()),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_color(rgb(if discount { c.accent } else { c.text }))
                            .child(line.amount.show()),
                    )
            }))
            .into_any_element()
    }
}

/// A receipt on the Receipts page: its place in the list, and whether
/// only what its emails say matched the search.
/// A row of the Receipts page's list.
enum ReceiptItem<'a> {
    /// A month's heading, with its receipts (for its total).
    Month { start: i64, list: Vec<&'a Listed> },
    /// A receipt, and whether only what its emails say matched the search.
    Receipt { l: &'a Listed, in_email: bool },
}

/// Whether a receipt matches a search (lowercase): every word of it is in
/// who was paid, its order, amount, kind, account, or one of its emails'
/// subjects or senders; or else in those or what its emails say (`texts`,
/// by message id). None when it doesn't match; Some(true) when only its
/// emails' text made it match.
fn receipt_matches(
    l: &Listed,
    query: &str,
    account: Option<&str>,
    texts: &HashMap<String, String>,
) -> Option<bool> {
    let r = &l.receipt;
    let amount = r.total.as_ref().map(Amount::show);
    let mut fields: Vec<&str> = vec![&l.merchant];
    fields.extend(r.merchant.as_deref());
    fields.extend(r.order.as_deref());
    fields.extend(amount.as_deref());
    fields.extend(r.total.as_ref().map(|t| t.currency.as_str()));
    if r.kind == Kind::Refund {
        fields.push("refund");
    }
    fields.extend(account);
    for line in &r.lines {
        fields.push(&line.label);
    }
    for s in &l.emails {
        fields.push(&s.subject);
        fields.push(&s.sender);
    }
    let fields = fields.join("\n").to_lowercase();
    let words: Vec<&str> = query.split_whitespace().collect();
    if words.iter().all(|w| fields.contains(w)) {
        return Some(false);
    }
    let said = |w: &str| {
        l.emails
            .iter()
            .filter_map(|s| texts.get(&s.mail_id))
            .any(|text| text.contains(w))
    };
    words
        .iter()
        .all(|w| fields.contains(w) || said(w))
        .then_some(true)
}

/// Opens a receipt's newest email.
fn open_email(l: &Listed) -> Action {
    let s = &l.emails[0];
    Action::OpenSummaryMail(s.account.clone(), s.mailbox.clone(), s.mail_id.clone())
}

/// The first second of the local month `t` is in.
fn month_start(t: i64) -> i64 {
    let Some(d) = chrono::Local.timestamp_opt(t, 0).single() else {
        return t;
    };
    chrono::Local
        .with_ymd_and_hms(d.year(), d.month(), 1, 0, 0, 0)
        .earliest()
        .map_or(t, |m| m.timestamp())
}

/// A day as a receipt's row in the widget says it: "Nov 20, 2026".
fn date_day(t: i64) -> String {
    chrono::Local
        .timestamp_opt(t, 0)
        .single()
        .map(|d| d.format("%b %-d, %Y").to_string())
        .unwrap_or_default()
}

/// What was spent in each currency: "$412.30 + €18.00".
fn show_totals(totals: &[Amount]) -> String {
    totals
        .iter()
        .map(Amount::show)
        .collect::<Vec<_>>()
        .join(" + ")
}

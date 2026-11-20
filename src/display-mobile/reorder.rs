//! Sorting accounts on the mailboxes screen. Edit turns each account (and
//! alias shown as one) into a row with a handle; dragging a handle moves the
//! account up or down, as dragging it in the desktop's sidebar does, and the
//! order is the sidebar's too (`tree`). All Accounts stays first.
use super::*;

/// A row's height while sorting.
const ROW: f32 = 56.;
/// From one row to the next: the row and the line between rows.
const PITCH: f32 = ROW + 1.;

impl MailApp {
    /// The accounts and aliases the mailboxes screen lists, in order.
    pub(super) fn mobile_sortable(&self) -> Vec<String> {
        tree::account_order(&self.tree)
            .into_iter()
            .filter(|id| {
                if fwd::is_view(id) {
                    self.view_title(id).is_some()
                } else {
                    self.accounts.iter().any(|a| &a.id == id)
                }
            })
            .collect()
    }

    /// Starts or ends sorting accounts.
    pub(super) fn mobile_set_reordering(&mut self, on: bool, cx: &mut Context<Self>) {
        if on {
            // Every account is in the arrangement before any moves.
            self.sync_tree();
        }
        self.mobile.reordering = on;
        self.mobile.reorder_drag = None;
        cx.notify();
    }

    /// The navigation bar's Edit (or Done) button, when there's anything
    /// to sort.
    pub(super) fn mobile_reorder_button(&self, cx: &Context<Self>) -> Option<AnyElement> {
        if self.mobile_sortable().len() < 2 && !self.mobile.reordering {
            return None;
        }
        let on = self.mobile.reordering;
        Some(
            self.touch_text(
                "mobile-reorder",
                if on { "Done" } else { "Edit" },
                on,
                Rc::new(move |this: &mut MailApp, _, cx: &mut Context<MailApp>| {
                    this.mobile_set_reordering(!on, cx)
                }),
                cx,
            )
            .into_any_element(),
        )
    }

    /// The mailboxes screen while sorting: one row per account.
    pub(super) fn mobile_reorder_list(&self, cx: &Context<Self>) -> AnyElement {
        let c = self.colors();
        let rows = self
            .mobile_sortable()
            .iter()
            .map(|id| self.mobile_reorder_row(id, cx))
            .collect();
        div()
            .relative()
            .flex()
            .flex_col()
            .child(self.reorder_follower(cx))
            .child(self.group_label("Accounts"))
            .child(self.group(rows))
            .child(
                div()
                    .pt_2()
                    .px(px(GUTTER))
                    .text_size(px(13.))
                    .text_color(rgb(c.muted))
                    .child("Drag ≡ to change the order. All Accounts stays first."),
            )
            .into_any_element()
    }

    /// Once a drag has started on a handle, follows the finger wherever it
    /// goes (off the handle, out of the list) until it lifts, ahead of the
    /// list's own scrolling.
    fn reorder_follower(&self, cx: &Context<Self>) -> impl IntoElement {
        let this = cx.weak_entity();
        canvas(
            |_, _, _| {},
            move |_, _, window, _| {
                let this = this.clone();
                window.on_mouse_event(move |e: &ScrollEvent, phase, _, cx| {
                    if phase != DispatchPhase::Capture || e.touch_phase == TouchPhase::Started {
                        return;
                    }
                    let _ = this.update(cx, |this, cx| {
                        let Some(id) = this.mobile.reorder_drag.as_ref().map(|(k, _)| k.clone())
                        else {
                            return;
                        };
                        this.mobile_reorder_pan(&id, e, cx);
                    });
                });
            },
        )
        .absolute()
        .size_0()
    }

    fn mobile_reorder_row(&self, id: &str, cx: &Context<Self>) -> AnyElement {
        let c = self.colors();
        let (lead, title, subtitle) = if fwd::is_view(id) {
            let (title, subtitle, icon) = self.view_title(id).unwrap_or_default();
            let lead = div()
                .size(px(32.))
                .rounded_full()
                .bg(rgb(c.soft))
                .flex()
                .items_center()
                .justify_center()
                .child(icon_el(icon, 18., c.accent))
                .into_any_element();
            (lead, title, Some(subtitle))
        } else {
            let Some((index, a)) = self.accounts.iter().enumerate().find(|(_, a)| a.id == id)
            else {
                return div().into_any_element();
            };
            let subtitle = (a.label() != a.email).then(|| a.email.clone());
            (
                self.account_avatar(a, index, 32.),
                a.label().to_owned(),
                subtitle,
            )
        };
        let offset = self
            .mobile
            .reorder_drag
            .as_ref()
            .filter(|(key, _)| key == id)
            .map(|(_, offset)| *offset);
        let pan_id = id.to_owned();
        let row = div()
            .id(SharedString::from(format!("mobile-reorder-{id}")))
            .debug_selector({
                let id = id.to_owned();
                move || format!("mobile-reorder-{id}")
            })
            .h(px(ROW))
            .w_full()
            .pl(px(GUTTER))
            .flex()
            .items_center()
            .gap_3()
            .bg(rgb(c.surface))
            .when_some(offset, |d, offset| {
                d.relative()
                    .top(px(offset))
                    .bg(rgb(c.soft))
                    .shadow_md()
            })
            .child(lead)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .truncate()
                            .text_size(px(17.))
                            .child(title),
                    )
                    .children(subtitle.map(|s| {
                        div()
                            .truncate()
                            .text_size(px(13.))
                            .text_color(rgb(c.muted))
                            .child(s)
                    })),
            )
            .child(
                div()
                    .id(SharedString::from(format!("mobile-reorder-handle-{id}")))
                    .debug_selector({
                        let id = id.to_owned();
                        move || format!("mobile-reorder-handle-{id}")
                    })
                    .size(px(TOUCH))
                    .mr_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(icon_el("grip", 22., c.muted))
                    .on_scroll(cx.listener(move |this, e: &ScrollEvent, _, cx| {
                        this.mobile_reorder_pan(&pan_id, e, cx)
                    })),
            );
        // The dragged row draws over its neighbours.
        match offset {
            Some(_) => div()
                .h(px(ROW))
                .child(deferred(row).with_priority(1))
                .into_any_element(),
            None => row.into_any_element(),
        }
    }

    /// Follows a finger dragging account `id` by its handle. The list never
    /// scrolls meanwhile.
    pub(super) fn mobile_reorder_pan(&mut self, id: &str, e: &ScrollEvent, cx: &mut Context<Self>) {
        let ScrollDelta::Pixels(d) = e.delta else {
            return;
        };
        let dy = f32::from(d.y);
        let dragging = self
            .mobile
            .reorder_drag
            .as_ref()
            .is_some_and(|(key, _)| key == id);
        match e.touch_phase {
            TouchPhase::Started if self.mobile.reorder_drag.is_none() => {
                cx.stop_propagation();
                platform::haptic(platform::Haptic::Selection);
                self.mobile.reorder_drag = Some((id.to_owned(), 0.));
                self.mobile_reorder_by(dy, cx);
            }
            TouchPhase::Moved if dragging => {
                cx.stop_propagation();
                self.mobile_reorder_by(dy, cx);
            }
            TouchPhase::Ended | TouchPhase::Cancelled if dragging => {
                cx.stop_propagation();
                self.mobile.reorder_drag = None;
                self.save_tree();
                cx.notify();
            }
            _ => {}
        }
    }

    /// Moves the dragged row `dy` further, passing it over its neighbours
    /// once it is more than halfway past them.
    pub(super) fn mobile_reorder_by(&mut self, dy: f32, cx: &mut Context<Self>) {
        let Some((id, mut offset)) = self.mobile.reorder_drag.take() else {
            return;
        };
        offset += dy;
        loop {
            let order = self.mobile_sortable();
            let Some(i) = order.iter().position(|k| *k == id) else {
                break;
            };
            let at = if offset > PITCH / 2. && i + 1 < order.len() {
                tree::DropAt::After(order[i + 1].clone())
            } else if offset < -PITCH / 2. && i > 0 {
                tree::DropAt::Before(order[i - 1].clone())
            } else {
                // At either end it only gives a little.
                let max = if i + 1 < order.len() { f32::MAX } else { PITCH / 3. };
                let min = if i > 0 { f32::MIN } else { -PITCH / 3. };
                offset = offset.clamp(min, max);
                break;
            };
            let down = matches!(at, tree::DropAt::After(_));
            if !tree::move_node(&mut self.tree, &id, &at) {
                break;
            }
            offset += if down { -PITCH } else { PITCH };
            platform::haptic(platform::Haptic::Selection);
        }
        self.mobile.reorder_drag = Some((id, offset));
        cx.notify();
    }
}

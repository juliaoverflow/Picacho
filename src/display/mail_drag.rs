//! Dragging messages onto the sidebar's mailboxes to move them there.
//!
//! With a pointer, a message row drags as GPUI drags anything: the row (or
//! the selection, when the row is in it) follows the pointer as a pill, and
//! a mailbox it can go to lights up under it. With a finger, a pointer's drag
//! never starts, and a press that moves is a scroll: a long press on the row
//! picks it up instead, and moving the finger carries it. A long press let go
//! of without moving opens the row's menu, as it does elsewhere.
//!
//! A message can go to its own account's Inbox, Archive, Spam, Trash or
//! server folders (dropped on Starred, it is starred), not to another
//! account's.
use super::*;
use std::cell::RefCell;

/// How far a finger moves before a picked-up message is being carried,
/// rather than held for its menu.
const CARRY: f32 = 10.;
/// Moved less than this, a pointer's drag was meant as a click.
const CLICK_SLOP: f32 = 5.;

/// Messages being dragged with a pointer (GPUI's drag value).
#[derive(Clone)]
pub(super) struct MailDrag {
    pub(super) ids: Vec<String>,
    /// Where the drag began, to tell a click that shook from a drag.
    start: Rc<Cell<Point<Pixels>>>,
}

/// What follows the pointer while messages are dragged.
struct MailDragPreview {
    label: SharedString,
    c: Colors,
}

impl Render for MailDragPreview {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        drag_pill(self.label.clone(), self.c)
    }
}

/// The pill naming what is being moved.
fn drag_pill(label: SharedString, c: Colors) -> Div {
    div()
        .flex()
        .items_center()
        .gap_2()
        .max_w(px(280.))
        .px_3()
        .py(px(6.))
        .rounded_full()
        .bg(rgb(c.accent))
        .text_color(rgb(c.on_accent))
        .text_size(px(13.))
        .font_weight(FontWeight::MEDIUM)
        .shadow_lg()
        .child(icon_el("mail", 14., c.on_accent))
        .child(div().min_w_0().truncate().child(label))
}

/// Messages carried by a finger (see the module's notes).
pub(super) struct TouchMailDrag {
    ids: Vec<String>,
    /// The row pressed, whose menu a press that doesn't move opens.
    row: String,
    label: SharedString,
    start: Point<Pixels>,
    at: Point<Pixels>,
    /// The finger has moved far enough to be carrying them.
    carrying: bool,
    /// The mailbox under the finger that would take them.
    over: Option<(String, Mailbox)>,
}

/// Where the sidebar's mailboxes were drawn, for a finger to find them.
pub(super) type DropRects = Rc<RefCell<Vec<(Bounds<Pixels>, String, Mailbox)>>>;

impl MailApp {
    /// The messages a drag from row `id` carries: the selection, when the
    /// row is in it, or the row.
    fn drag_ids(&self, id: &str) -> Vec<String> {
        if self.selection.contains(id) {
            self.visible
                .iter()
                .map(|&i| &self.messages[i])
                .filter(|m| self.selection.contains(&m.id) && !m.pending)
                .map(|m| m.id.clone())
                .collect()
        } else {
            vec![id.to_owned()]
        }
    }

    /// What the pill says: the message's subject, or how many there are.
    fn drag_label(&self, ids: &[String]) -> SharedString {
        match ids {
            [id] => self
                .messages
                .iter()
                .find(|m| &m.id == id)
                .map(|m| {
                    if m.subject.trim().is_empty() {
                        "(No subject)".to_owned()
                    } else {
                        self.hide_codes(m, &m.subject).into_owned()
                    }
                })
                .unwrap_or_default()
                .into(),
            ids => format!("{} messages", fmt_count(ids.len())).into(),
        }
    }

    /// What dropping `ids` on `account`'s `mailbox` does, if it can take
    /// them: all from that account, and not all there already.
    pub(super) fn drop_change(
        &self,
        ids: &[String],
        account: &str,
        mailbox: &Mailbox,
    ) -> Option<Change> {
        if ids.is_empty() || fwd::is_view(account) {
            return None;
        }
        let mails: Vec<&Mail> = self
            .messages
            .iter()
            .filter(|m| ids.contains(&m.id) && !m.pending)
            .collect();
        if mails.len() != ids.len() || mails.iter().any(|m| m.account != account) {
            return None;
        }
        match mailbox {
            Mailbox::Standard(Folder::Starred) => mails
                .iter()
                .any(|m| !m.starred)
                .then_some(Change::Star(true)),
            Mailbox::Standard(
                f @ (Folder::Inbox | Folder::Archive | Folder::Junk | Folder::Trash),
            ) => mails
                .iter()
                .any(|m| m.folder != *f)
                .then_some(Change::Move(*f)),
            Mailbox::Remote(name) if self.manages_folders(account) => mails
                .iter()
                .any(|m| m.folder != Folder::Other || &m.remote_folder != name)
                .then(|| Change::MoveTo(name.clone())),
            _ => None,
        }
    }

    /// Moves (or stars) `ids` as dropping them on the mailbox does.
    fn drop_mail(
        &mut self,
        ids: Vec<String>,
        account: &str,
        mailbox: &Mailbox,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(change) = self.drop_change(&ids, account, mailbox) {
            #[cfg(test)]
            {
                self.last_drop = Some((ids.clone(), change.clone()));
            }
            self.handle(Action::Apply(ids, change), w, cx);
        }
    }

    /// Makes message row `id` draggable: with a pointer, and on a touch
    /// device by a long press that moves.
    pub(super) fn draggable_mail(
        &self,
        row: Stateful<Div>,
        id: &str,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let ids = self.drag_ids(id);
        let label = self.drag_label(&ids);
        let c = self.colors();
        let row = row.on_drag(
            MailDrag {
                ids: ids.clone(),
                start: Rc::default(),
            },
            move |d: &MailDrag, _offset, w, cx| {
                d.start.set(w.mouse_position());
                cx.new(|_| MailDragPreview {
                    label: label.clone(),
                    c,
                })
            },
        );
        if !self.mobile.touch || self.mobile.on {
            return row;
        }
        let this = cx.weak_entity();
        let id = id.to_owned();
        row.relative().child(
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
                        let _ = this.update(cx, |this, cx| this.pick_up(&id, e.start_position, cx));
                    });
                },
            )
            .absolute()
            .inset_0(),
        )
    }

    /// A long press on message row `id`: its messages are picked up.
    fn pick_up(&mut self, id: &str, at: Point<Pixels>, cx: &mut Context<Self>) {
        let ids = self.drag_ids(id);
        self.menu = None;
        self.touch_mail_drag = Some(TouchMailDrag {
            label: self.drag_label(&ids),
            ids,
            row: id.to_owned(),
            start: at,
            at,
            carrying: false,
            over: None,
        });
        self.touch_haptic();
        cx.notify();
    }

    /// Follows the finger carrying messages, and drops them where it lifts.
    pub(super) fn carry_mail(
        &mut self,
        e: &LongPressEvent,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(drag) = &mut self.touch_mail_drag else {
            return;
        };
        cx.stop_propagation();
        match e.phase {
            TouchPhase::Started => {}
            TouchPhase::Moved => {
                drag.at = e.position;
                drag.carrying |= (e.position - drag.start).magnitude() as f32 > CARRY;
                let over = self
                    .drop_rects
                    .borrow()
                    .iter()
                    .find(|(b, ..)| b.contains(&e.position))
                    .map(|(_, account, mailbox)| (account.clone(), mailbox.clone()));
                let ids = drag.ids.clone();
                let over = over.filter(|(a, m)| self.drop_change(&ids, a, m).is_some());
                if let Some(drag) = &mut self.touch_mail_drag {
                    drag.over = over;
                }
                cx.notify();
            }
            TouchPhase::Ended => {
                let Some(drag) = self.touch_mail_drag.take() else {
                    return;
                };
                match drag.over {
                    Some((account, mailbox)) if drag.carrying => {
                        self.drop_mail(drag.ids, &account, &mailbox, w, cx)
                    }
                    // Held without moving: the row's menu, as a long press
                    // opens elsewhere.
                    _ if !drag.carrying => {
                        let entries = self.mail_entries(&drag.row);
                        self.open_menu(drag.start, entries, cx);
                    }
                    _ => {}
                }
                cx.notify();
            }
            TouchPhase::Cancelled => {
                self.touch_mail_drag = None;
                cx.notify();
            }
        }
    }

    /// Makes a sidebar mailbox a place messages can be dropped.
    pub(super) fn mail_drop_target(
        &self,
        row: Stateful<Div>,
        account: &str,
        mailbox: &Mailbox,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let c = self.colors();
        let this = cx.weak_entity();
        let (target_account, target_mailbox) = (account.to_owned(), mailbox.clone());
        let (drop_account, drop_mailbox) = (account.to_owned(), mailbox.clone());
        let row = row
            .can_drop(move |value, _, cx| {
                value.downcast_ref::<MailDrag>().is_some_and(|d| {
                    this.upgrade().is_some_and(|this| {
                        this.read(cx)
                            .drop_change(&d.ids, &target_account, &target_mailbox)
                            .is_some()
                    })
                })
            })
            .drag_over::<MailDrag>(move |s, _, _, _| s.bg(rgb(c.soft)))
            .on_drop(cx.listener(move |this, d: &MailDrag, w, cx| {
                cx.stop_propagation();
                this.drop_mail(d.ids.clone(), &drop_account, &drop_mailbox, w, cx);
            }));
        let Some(drag) = &self.touch_mail_drag else {
            return row;
        };
        // Where it is, for the finger to find; lit while it is over it.
        let over = drag.over.as_ref() == Some(&(account.to_owned(), mailbox.clone()));
        let rects = self.drop_rects.clone();
        let (account, mailbox) = (account.to_owned(), mailbox.clone());
        row.when(over, |d| d.bg(rgb(c.soft))).relative().child(
            canvas(
                move |bounds, _, _| {
                    rects
                        .borrow_mut()
                        .push((bounds, account.clone(), mailbox.clone()))
                },
                |_, _, _, _| {},
            )
            .absolute()
            .inset_0(),
        )
    }

    /// A pointer's drag let go of anywhere but a mailbox: one that barely
    /// moved was a click on its row.
    pub(super) fn mail_dropped_elsewhere(
        &mut self,
        d: &MailDrag,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let moved = (w.mouse_position() - d.start.get()).magnitude() as f32;
        if moved < CLICK_SLOP
            && let Some(id) = d.ids.first()
        {
            self.handle(Action::Select(id.clone()), w, cx);
        }
    }

    /// The pill under a finger carrying messages, over everything.
    pub(super) fn touch_mail_drag_layer(&self) -> Option<AnyElement> {
        let drag = self.touch_mail_drag.as_ref()?;
        Some(
            div()
                .id("touch-mail-drag")
                .debug_selector(|| "touch-mail-drag".into())
                .absolute()
                // Beside the finger, not under it.
                .left(drag.at.x + px(16.))
                .top(drag.at.y - px(48.))
                .child(drag_pill(drag.label.clone(), self.colors()))
                .into_any_element(),
        )
    }
}

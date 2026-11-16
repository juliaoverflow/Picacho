//! A message, full screen: the desktop reader's body (the email, its
//! banners, attachments and conversation) between a navigation bar and a
//! bottom bar of the actions used most, within a thumb's reach.
use super::chrome::{Tap, act};
use super::*;

impl MailApp {
    /// The open message. `stacked` when it covers the list (a phone), so
    /// it has a back button; beside the list it has none.
    pub(super) fn mobile_reader(
        &mut self,
        insets: Insets,
        stacked: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = self.colors();
        // Several messages picked beside the list: what can be done with them.
        if self.selection.len() > 1 {
            return div()
                .size_full()
                .flex()
                .flex_col()
                .pt(px(insets.top))
                .child(self.reader(Corners::default(), false, window, cx))
                .into_any_element();
        }
        let Some(m) = self.selected_ref() else {
            return div().into_any_element();
        };
        let id = m.id.clone();
        let unread = m.unread;
        let drafts_or_sent = matches!(m.folder, Folder::Drafts | Folder::Sent);
        let pop = self
            .account_of(m)
            .is_some_and(|a| a.provider == Provider::Pop3);
        let position = self.visible.iter().position(|i| self.messages[*i].id == id);
        let has_prev = position.is_some_and(|p| p > 0);
        let has_next = position.is_some_and(|p| p + 1 < self.visible.len());
        let step = |offset: isize| -> Tap {
            Rc::new(move |this, _, cx| {
                this.next(offset, cx);
                // The next message rises from below, the previous one
                // drops from above: not a new screen sliding in.
                this.mobile.push_anim += 1;
                this.mobile.slide = if offset < 0 {
                    super::Slide::Previous
                } else {
                    super::Slide::Next
                };
            })
        };
        let title = self.mailbox_label();
        let leading = stacked.then(|| self.back_icon("mobile-reader-back", cx));
        // Starring is beside the subject, in the message itself.
        let trailing = vec![
            self.touch_icon(
                "mobile-reader-prev",
                "chevron-up",
                "Previous message",
                step(-1),
                cx,
            )
            .when(!has_prev, |d| d.opacity(0.35))
            .into_any_element(),
            self.touch_icon(
                "mobile-reader-next",
                "chevron-down",
                "Next message",
                step(1),
                cx,
            )
            .when(!has_next, |d| d.opacity(0.35))
            .into_any_element(),
        ];
        let reply_entries = vec![
            entry("Reply", "reply", Action::Reply(false)),
            entry("Reply All", "reply-all", Action::Reply(true)),
            entry("Forward", "forward", Action::Forward),
        ];
        let more_id = id.clone();
        let mut tools = vec![
            self.touch_icon(
                "mobile-reader-archive",
                "archive",
                "Archive",
                act(Action::Move(Folder::Archive)),
                cx,
            )
            .into_any_element(),
            self.touch_icon(
                "mobile-reader-trash",
                "trash",
                "Move to trash",
                act(Action::Move(Folder::Trash)),
                cx,
            )
            .into_any_element(),
            self.touch_icon(
                "mobile-reader-move",
                "folder",
                "Move to folder",
                act(Action::MoveMenu(MoveTarget::Mail(vec![id.clone()]))),
                cx,
            )
            .into_any_element(),
        ];
        if !pop && !drafts_or_sent {
            tools.push(
                self.touch_icon(
                    "mobile-reader-snooze",
                    "snooze",
                    "Snooze",
                    act(Action::SnoozeMenu(vec![id.clone()])),
                    cx,
                )
                .into_any_element(),
            );
        }
        tools.push(
            self.touch_icon(
                "mobile-reader-reply",
                "reply",
                "Reply",
                Rc::new(
                    move |this: &mut MailApp, w: &mut Window, cx: &mut Context<MailApp>| {
                        this.open_menu_here(reply_entries.clone(), w, cx)
                    },
                ),
                cx,
            )
            .into_any_element(),
        );
        tools.push(
            self.touch_icon(
                "mobile-reader-more",
                "ellipsis",
                "More",
                Rc::new(
                    move |this: &mut MailApp, w: &mut Window, cx: &mut Context<MailApp>| {
                        let mut entries = this.mail_entries(&more_id);
                        entries.push(Entry::Separator);
                        entries.push(entry(
                            if unread {
                                "Mark as Read"
                            } else {
                                "Mark as Unread"
                            },
                            if unread { "mail-open" } else { "mail" },
                            Action::Read(more_id.clone()),
                        ));
                        entries.push(entry("Message Details", "info", Action::ToggleDetails));
                        entries.push(entry("View Original", "code", Action::ShowOriginal));
                        this.open_menu_here(entries, w, cx)
                    },
                ),
                cx,
            )
            .into_any_element(),
        );
        let body = self.reader_body(m, window, cx);
        div()
            .id("mobile-reader")
            .debug_selector(|| "mobile-reader".into())
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(c.surface))
            .child(self.nav_bar(
                "mobile-reader-bar",
                insets,
                leading,
                if stacked { title } else { String::new() },
                None,
                trailing,
            ))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .pl(px(insets.left))
                    .pr(px(insets.right))
                    .flex()
                    .flex_col()
                    .text_size(px(16.))
                    .child(body),
            )
            .child(self.action_row("mobile-reader-toolbar", insets, tools))
            .into_any_element()
    }
}

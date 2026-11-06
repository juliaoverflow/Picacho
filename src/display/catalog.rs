//! The message list's catalog: every message in a mailbox, listed in the
//! background in batches, with no "Load more" step.
use super::*;
use crate::manage::backend::Catalog;

/// Headers listed per catalog request.
const BATCH: usize = 500;

impl MailApp {
    /// Lists the rest of a mailbox, a batch at a time, until it is complete.
    /// One catalog runs per mailbox; account work stays serialized.
    pub(super) fn catalog_mailbox(&mut self, a: Account, mailbox: Mailbox, cx: &mut Context<Self>) {
        let key = (a.id.clone(), mailbox.key());
        if self.account_blocked(&a.id) || !self.cataloging.insert(key.clone()) {
            return;
        }
        let backend = self.backend.clone();
        let (account, target) = (a.clone(), mailbox.clone());
        let job = cx
            .background_executor()
            .spawn(async move { backend.catalog(&account, target, BATCH) });
        cx.spawn(async move |this, cx| {
            let result = job.await;
            let _ = this.update(cx, |this, cx| {
                this.cataloging.remove(&key);
                if !this.accounts.iter().any(|x| x.id == a.id) {
                    return;
                }
                this.account_result(&a.id, IssueSource::Sync(key.1.clone()), &result);
                if let Ok(progress) = result {
                    this.catalogs.insert(key.clone(), progress);
                    if this.account.as_ref() == Some(&a.id) && this.mailbox().key() == key.1 {
                        this.reload_cached(cx);
                    }
                    if !progress.complete() {
                        this.catalog_mailbox(a, mailbox, cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
    /// The open mailbox's catalog while it is still being listed.
    pub(super) fn catalog_progress(&self) -> Option<Catalog> {
        let key = (self.account.clone()?, self.mailbox().key());
        let progress = self.catalogs.get(&key).copied();
        (self.cataloging.contains(&key) && !progress.is_some_and(|p| p.complete()))
            .then(|| progress.unwrap_or_default())
    }
    /// Replaces the listed messages, keeping the open message's body and the
    /// list's scroll position on the same message, wherever it moved.
    pub(super) fn set_messages(&mut self, messages: Vec<Mail>, cx: &App) {
        let mut messages = self.backend.with_readable(messages);
        if let Some(open) = self.selected.as_ref().and_then(|id| {
            self.messages
                .iter()
                .find(|m| &m.id == id && m.body.is_some())
        }) && let Some(m) = messages
            .iter_mut()
            .find(|m| m.id == open.id && m.body.is_none())
        {
            let (unread, starred, folder) = (m.unread, m.starred, m.folder);
            *m = Mail {
                unread,
                starred,
                folder,
                ..open.clone()
            };
        }
        let anchor = self.scroll_anchor();
        self.messages = messages;
        self.filter(cx);
        if let Some(anchor) = anchor {
            self.restore_scroll(anchor);
        }
    }
    /// The first visible message and how far it is scrolled past.
    fn scroll_anchor(&self) -> Option<(String, Pixels)> {
        let offset = -self.list_scroll.0.borrow().base_handle.offset().y;
        if offset <= px(0.) {
            return None;
        }
        let index = (offset / px(ROW_HEIGHT)).floor() as usize;
        let id = self.messages.get(*self.visible.get(index)?)?.id.clone();
        Some((id, offset - px(ROW_HEIGHT) * index as f32))
    }
    fn restore_scroll(&self, (id, within): (String, Pixels)) {
        if let Some(index) = self.visible.iter().position(|i| self.messages[*i].id == id) {
            let y = px(ROW_HEIGHT) * index as f32 + within;
            self.list_scroll
                .0
                .borrow()
                .base_handle
                .set_offset(point(px(0.), -y));
        }
    }
}

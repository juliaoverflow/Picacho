//! The message list's catalog: every message in a mailbox, listed in the
//! background in batches, with no "Load more" step.
//!
//! Once the mailbox's layout is known (one search), the list has a row for
//! every message from the start: listed ones, and placeholders for the rest.
//! Placeholders on screen are listed first; the background catalog fills in
//! the remainder, newest first, at a pace the server tolerates.
use super::*;
use crate::manage::backend::Catalog;

/// Headers listed per background catalog request. Small enough that rows
/// scrolled into view are not kept waiting behind a batch for long.
const BATCH: usize = 200;

/// How much of a mailbox has been listed on this computer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Progress {
    pub(super) cataloged: usize,
    pub(super) total: usize,
}
impl Progress {
    pub(super) fn complete(&self) -> bool {
        self.cataloged >= self.total
    }
}

impl MailApp {
    /// Lists the rest of a mailbox, a batch at a time, until it is complete.
    /// One catalog runs per mailbox; account work stays serialized.
    pub(super) fn catalog_mailbox(&mut self, a: Account, mailbox: Mailbox, cx: &mut Context<Self>) {
        let key = (a.id.clone(), mailbox.key());
        if self.account_blocked(&a.id)
            || self.backend.throttled_until(&a.id).is_some()
            || !self.cataloging.insert(key.clone())
        {
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
                if let Ok(catalog) = result {
                    let progress = Progress {
                        cataloged: catalog.cataloged,
                        total: catalog.total,
                    };
                    this.catalogs.insert(key.clone(), progress);
                    if this.account.as_ref() == Some(&a.id) && this.mailbox().key() == key.1 {
                        this.show_catalog(catalog, cx);
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
    /// Puts newly listed messages in their rows. A new layout (the first,
    /// or one changed by new or removed mail) reloads the whole list.
    fn show_catalog(&mut self, catalog: Catalog, cx: &mut Context<Self>) {
        let same = match (&self.layout, &catalog.layout) {
            (Some(open), Some(next)) => {
                open.validity == next.validity && Arc::ptr_eq(&open.uids, &next.uids)
            }
            (_, None) => true,
            (None, Some(_)) => false,
        };
        if !same {
            self.unfillable.clear();
        }
        if !same || !self.place(catalog.added, cx) {
            self.reload_cached(cx);
        }
    }
    /// Replaces placeholders with listed messages. False if any has no row.
    pub(super) fn place(&mut self, listed: Vec<Mail>, cx: &App) -> bool {
        if listed.is_empty() {
            return true;
        }
        let rows: HashMap<&str, usize> = self
            .messages
            .iter()
            .enumerate()
            .map(|(i, m)| (m.id.as_str(), i))
            .collect();
        let places: Vec<Option<usize>> = listed
            .iter()
            .map(|m| rows.get(m.id.as_str()).copied())
            .collect();
        let all = places.iter().all(Option::is_some);
        for (mail, place) in listed.into_iter().zip(places) {
            if let Some(i) = place {
                let mut mail = mail;
                // Lists do not carry bodies; keep the one an open message has.
                mail.body = None;
                mail.html = None;
                self.messages[i] = mail;
            }
        }
        // Rows only change from placeholder to message; a filter or search
        // may now match them.
        self.apply_filter(cx);
        all
    }
    /// Asks for the placeholders around the visible rows to be listed now.
    /// Called while the list draws; only the latest request is kept.
    pub(super) fn want_rows(&mut self, range: std::ops::Range<usize>, cx: &mut Context<Self>) {
        let reach = range.len().max(1);
        let from = range.start.saturating_sub(reach);
        let to = (range.end + reach).min(self.visible.len());
        let wanted: Vec<u32> = self.visible[from..to]
            .iter()
            .map(|i| &self.messages[*i])
            .filter(|m| m.pending && !self.unfillable.contains(&m.uid))
            .map(|m| m.uid)
            .collect();
        if wanted.is_empty() {
            return;
        }
        self.wanted_rows = wanted;
        if !self.filling {
            self.fill_rows(cx);
        }
    }
    fn fill_rows(&mut self, cx: &mut Context<Self>) {
        let wanted = std::mem::take(&mut self.wanted_rows);
        let (Some(a), Some(layout)) = (self.current_account(), self.layout.clone()) else {
            return;
        };
        let still: HashSet<&u32> = wanted.iter().collect();
        let uids: Vec<u32> = self
            .messages
            .iter()
            .filter(|m| m.pending && still.contains(&m.uid))
            .map(|m| m.uid)
            .collect();
        if uids.is_empty() || self.account_blocked(&a.id) {
            return;
        }
        self.filling = true;
        let mailbox = self.mailbox();
        let key = mailbox.key();
        let generation = self.load_generation;
        let backend = self.backend.clone();
        let account = a.clone();
        let asked = uids.clone();
        let job = cx
            .background_executor()
            .spawn(async move { backend.fill(&account, &mailbox, &layout, &uids) });
        cx.spawn(async move |this, cx| {
            let result = job.await;
            let _ = this.update(cx, |this, cx| {
                this.filling = false;
                this.account_result(&a.id, IssueSource::Sync(key), &result);
                if generation != this.load_generation {
                    this.wanted_rows.clear();
                    return;
                }
                match result {
                    Ok(listed) => {
                        // Rows the server did not list (deleted meanwhile)
                        // are not asked for again on every frame; the next
                        // catalog or sync settles them.
                        let got: HashSet<u32> = listed.iter().map(|m| m.uid).collect();
                        this.unfillable
                            .extend(asked.iter().filter(|u| !got.contains(u)));
                        if !this.place(listed, cx) {
                            this.reload_cached(cx);
                        }
                        if !this.wanted_rows.is_empty() {
                            this.fill_rows(cx);
                        }
                    }
                    // Do not retry on every frame; scrolling asks again.
                    Err(_) => this.wanted_rows.clear(),
                }
                cx.notify();
            });
        })
        .detach();
    }
    /// The open mailbox's catalog while it is still being listed.
    pub(super) fn catalog_progress(&self) -> Option<Progress> {
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
        self.apply_filter(cx);
        if let Some(anchor) = anchor {
            self.restore_scroll(anchor);
        }
    }
    /// The first visible message and how far it is scrolled past.
    pub(super) fn scroll_anchor(&self) -> Option<(String, Pixels)> {
        let offset = -self.list_scroll.0.borrow().base_handle.offset().y;
        if offset <= px(0.) {
            return None;
        }
        let index = (offset / px(ROW_HEIGHT)).floor() as usize;
        let id = self.messages.get(*self.visible.get(index)?)?.id.clone();
        Some((id, offset - px(ROW_HEIGHT) * index as f32))
    }
    pub(super) fn restore_scroll(&self, (id, within): (String, Pixels)) {
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

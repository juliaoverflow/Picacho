//! An account's Summary, under its Inbox (and All Accounts', for every
//! account): what its mail adds up to. Its Packages section lists every
//! parcel the mail tells of (see `manage::parcels`), last updated first,
//! each with how far along the way it is (ordered → shipped → in transit →
//! out for delivery → delivered); clicking one shows its whole history,
//! message by message. Shipping mail known only by its headers is
//! downloaded so its tracking numbers can be read. The reader also shows
//! the tracking numbers of the open message. Its Receipts section (see
//! `receipts`) lists what the mail says was paid.
use super::*;
use crate::manage::local_ai::parcel::Confirmer;
use crate::manage::parcels::{self, Event, Override, Parcel, STAGES, Source, Status, Tracked};
use std::sync::atomic::AtomicBool;

/// The parcels found in a message, with the lengths of the subject and text
/// they were found in and whether the model confirmed them.
pub(super) type Found = ((usize, usize, bool), Rc<Vec<Parcel>>);

/// Banners shown above a message at most, one per parcel.
const BANNERS: [&str; 3] = ["parcel-banner-0", "parcel-banner-1", "parcel-banner-2"];
/// Parcels cached per message for the reader; cleared past this many.
const FOUND_CACHE: usize = 4000;
/// Shipping messages downloaded between two looks through the mail.
const DOWNLOAD_BATCH: usize = 150;
/// Parcels the Summary's Packages widget lists; the rest are on its
/// Packages page.
const WIDGET_PACKAGES: usize = 5;
/// The preference holding statuses set by hand on parcels (`Override`s).
const OVERRIDES: &str = "parcel_statuses";
/// Statuses set by hand kept at most; the oldest go first.
const MAX_OVERRIDES: usize = 2000;
/// A parcel not shipped yet with no news for this long was likely sent
/// without an email saying so (or never will be): it isn't counted as on
/// the way. One shipped is taken to have been delivered sooner
/// (`Tracked::assumed`).
const QUIET_DAYS: i64 = 30;
/// The Packages widget's width, where there's room for it (and the
/// Receipts widget's).
pub(super) const WIDGET_WIDTH: f32 = 380.;

impl MailApp {
    /// Shows the Summary of an account (or All Accounts) in place of its
    /// list and reader. The parcels shown before stay while it looks again.
    pub(super) fn open_summary(&mut self, id: String, w: &mut Window, cx: &mut Context<Self>) {
        let same = self
            .summary_read
            .as_ref()
            .is_some_and(|(read, _, _)| *read == id);
        self.open_mailbox(Some(id), Folder::Inbox.into(), w, cx);
        // The Summary shows parcels, not the Inbox it was opened over.
        self.load_generation += 1;
        self.selected = None;
        self.summary_open = true;
        // Opened again, it shows its widgets, and every package.
        self.summary_packages = false;
        self.summary_receipts = false;
        if self.summary_search {
            self.search_parcels(false, w, cx);
        }
        if self.receipts_search {
            self.search_receipts(false, w, cx);
        }
        if !same {
            self.summary.clear();
            self.receipts.clear();
            self.summary_expanded.clear();
            self.forget_parcel_texts();
            self.summary_downloads = None;
        }
        self.summary_error = None;
        self.summary_cancel = Arc::new(AtomicBool::new(false));
        // Looked at again, whatever changed.
        self.summary_read = None;
        // A look at other mail still running is left to finish unheard.
        self.summary_loading = false;
        self.summary_fetching = false;
        self.summary_generation += 1;
        self.load_summary(cx);
        cx.notify();
    }

    /// Leaves the Summary, stopping its downloads.
    pub(super) fn leave_summary(&mut self) {
        if self.summary_open {
            self.summary_open = false;
            self.summary_cancel.store(true, Ordering::Relaxed);
        }
        self.forget_parcel_texts();
    }

    /// Looks through the Summary's accounts' mail for parcels, in the
    /// background, unless nothing changed since the last look. What was
    /// found in each message is kept, so only new or newly downloaded mail
    /// is read.
    pub(super) fn load_summary(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.account.clone().filter(|_| self.summary_open) else {
            return;
        };
        if self.summary_loading {
            self.summary_again = true;
            return;
        }
        let accounts = self.subscription_accounts(&id);
        let use_model = self.find_parcels && self.parcel_model.is_some();
        // Read before the look: mail changing during it is looked at again.
        let revisions: Vec<u64> = accounts
            .iter()
            .map(|a| self.backend.store.list_revision(a))
            .collect();
        let read = (id, revisions, use_model);
        if self.summary_read.as_ref() == Some(&read) {
            return;
        }
        self.summary_generation += 1;
        let generation = self.summary_generation;
        self.summary_loading = true;
        cx.notify();
        self.task(
            cx,
            move |b| {
                // The model is loaded here, off the main thread, as the
                // reader's copy can't leave it.
                let model: Option<Confirmer> = use_model
                    .then(|| crate::manage::local_ai::parcel_confirmer(&b.store).ok())
                    .flatten();
                let parcels = b.store.parcels(&accounts, model.as_ref())?;
                let receipts = b.store.receipts(&accounts)?;
                Ok((parcels, receipts))
            },
            move |this, result, cx| {
                if generation != this.summary_generation {
                    return;
                }
                this.summary_loading = false;
                if !this.summary_open {
                    return;
                }
                match result {
                    Ok((scan, mut receipts)) => {
                        this.receipts = std::mem::take(&mut receipts.listed);
                        this.receipts_scanned = receipts.scanned;
                        this.apply_hidden_receipts();
                        this.summary = scan.tracked;
                        this.apply_parcel_overrides();
                        this.summary_scanned = scan.scanned;
                        this.summary_read = Some(read);
                        // Shipping mail first, then receipts, each once.
                        let mut waiting = scan.to_download;
                        let mut queued: HashSet<String> =
                            waiting.iter().map(|(_, id)| id.clone()).collect();
                        waiting.extend(
                            receipts
                                .to_download
                                .into_iter()
                                .filter(|(_, id)| queued.insert(id.clone())),
                        );
                        this.download_shipping(waiting, cx);
                        // New emails are searched too.
                        this.load_parcel_texts(cx);
                    }
                    Err(e) => this.notice(format!("{e:#}"), true, cx),
                }
                if std::mem::take(&mut this.summary_again) {
                    this.load_summary(cx);
                }
                cx.notify();
            },
        );
    }

    /// Downloads the next few shipping messages known only by their
    /// headers, then looks through the mail again; each message is tried
    /// once.
    fn download_shipping(&mut self, waiting: Vec<(String, String)>, cx: &mut Context<Self>) {
        if self.summary_fetching || self.summary_error.is_some() {
            return;
        }
        let waiting: Vec<(String, String)> = waiting
            .into_iter()
            .filter(|(_, id)| !self.summary_tried.contains(id))
            .collect();
        if waiting.is_empty() {
            self.summary_downloads = None;
            return;
        }
        let done = self.summary_downloads.map_or(0, |(done, _)| done);
        self.summary_downloads = Some((done, done + waiting.len()));
        let batch: Vec<(String, String)> = waiting.into_iter().take(DOWNLOAD_BATCH).collect();
        // Marked tried only once the batch is through: a batch stopped
        // (the Summary left) or failed is tried again next time, rather
        // than its messages never being read.
        let ids: Vec<String> = batch.iter().map(|(_, id)| id.clone()).collect();
        self.summary_fetching = true;
        let cancel = self.summary_cancel.clone();
        let generation = self.summary_generation;
        let count = batch.len();
        self.task(
            cx,
            move |b| b.download_for_parcels(&batch, &cancel),
            move |this, result, cx| {
                this.summary_fetching = false;
                if !this.summary_open || this.summary_cancel.load(Ordering::Relaxed) {
                    this.summary_downloads = None;
                    return;
                }
                match result {
                    Ok(_) => {
                        this.summary_tried.extend(ids);
                        if let Some((done, total)) = &mut this.summary_downloads {
                            *done = (*done + count).min(*total);
                        }
                        // Read what arrived (and download the next ones).
                        if generation == this.summary_generation || !this.summary_loading {
                            this.summary_read = None;
                            this.load_summary(cx);
                        }
                    }
                    Err(e) => {
                        this.summary_downloads = None;
                        this.summary_error =
                            Some(format!("Couldn't download shipping and receipt emails: {e:#}"));
                    }
                }
                cx.notify();
            },
        );
    }

    /// Stops downloading shipping mail for the Summary.
    pub(super) fn stop_summary_downloads(&mut self, cx: &mut Context<Self>) {
        self.summary_cancel.store(true, Ordering::Relaxed);
        self.summary_downloads = None;
        self.summary_error = Some("Downloading shipping and receipt emails was stopped.".into());
        cx.notify();
    }

    /// Forgets what the reader found in mail, and looks through the
    /// Summary's mail again, after the model was turned on or off.
    pub(super) fn forget_parcels(&mut self, cx: &mut Context<Self>) {
        self.parcels_found.borrow_mut().clear();
        if self.summary_open {
            self.summary_read = None;
            self.load_summary(cx);
        }
    }

    /// The statuses set by hand on parcels, as saved.
    pub(super) fn parcel_overrides(&self) -> Vec<Override> {
        self.backend
            .store
            .preference(OVERRIDES)
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_default()
    }

    /// Sets the Summary's parcels' statuses by hand, and hides them, as
    /// saved.
    fn apply_parcel_overrides(&mut self) {
        let overrides = self.parcel_overrides();
        parcels::apply_overrides(&mut self.summary, &overrides);
    }

    /// Changes what is set by hand on the Summary's parcel `key` (its
    /// status, hiding, name or sender), and saves that.
    pub(super) fn override_parcel(
        &mut self,
        key: &str,
        change: impl FnOnce(&mut Override),
        cx: &mut Context<Self>,
    ) {
        let Some(t) = self.summary.iter().find(|t| t.parcel.key() == key) else {
            return;
        };
        let mut overrides = self.parcel_overrides();
        let mut next = overrides
            .iter()
            .rev()
            .find(|o| o.fits(t))
            .cloned()
            .unwrap_or(Override {
                ids: Vec::new(),
                status: None,
                hidden: false,
                name: None,
                sender: None,
                set: 0,
            });
        overrides.retain(|o| !o.fits(t));
        // Known by what it was known by, and what it is known by now.
        for id in &t.ids {
            if !next.ids.contains(id) {
                next.ids.push(id.clone());
            }
        }
        next.set = chrono::Utc::now().timestamp();
        change(&mut next);
        if next.sets_anything() {
            overrides.push(next);
        }
        if overrides.len() > MAX_OVERRIDES {
            overrides.drain(..overrides.len() - MAX_OVERRIDES);
        }
        if let Ok(json) = serde_json::to_string(&overrides) {
            self.backend.store.set_preference_soon(OVERRIDES, &json);
        }
        parcels::apply_overrides(&mut self.summary, &overrides);
        cx.notify();
    }

    /// The statuses that can be set by hand on the Summary's parcel `key`,
    /// the one it has checked, and hiding it.
    fn parcel_entries(&self, key: &str) -> Vec<Entry> {
        let Some(t) = self.summary.iter().find(|t| t.parcel.key() == key) else {
            return Vec::new();
        };
        let key = key.to_owned();
        let mut entries = vec![checked(
            "Automatic (from emails)",
            t.manual.is_none(),
            Action::SetParcelStatus(key.clone(), None),
        )];
        entries.push(Entry::Separator);
        for status in Status::ALL {
            entries.push(checked(
                status.label(),
                t.manual == Some(status),
                Action::SetParcelStatus(key.clone(), Some(status)),
            ));
        }
        entries.push(Entry::Separator);
        entries.push(entry(
            "Rename Package…",
            "select",
            Action::EditParcel(key.clone(), ParcelField::Name),
        ));
        entries.push(entry(
            "Edit Sender…",
            "user",
            Action::EditParcel(key.clone(), ParcelField::Sender),
        ));
        entries.push(Entry::Separator);
        entries.push(if t.hidden {
            entry("Show Package", "eye", Action::HideParcel(key, false))
        } else {
            entry("Hide Package", "eye-off", Action::HideParcel(key, true))
        });
        entries
    }

    /// Offers the statuses to set by hand on the Summary's parcel `key`,
    /// at the pointer.
    pub(super) fn parcel_menu(&mut self, key: &str, w: &mut Window, cx: &mut Context<Self>) {
        let entries = self.parcel_entries(key);
        if !entries.is_empty() {
            self.open_menu_here(entries, w, cx);
        }
    }

    /// Edits the name or sender of the Summary's parcel `key` in place, on
    /// the Packages page.
    pub(super) fn edit_parcel(
        &mut self,
        key: String,
        field: ParcelField,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(t) = self.summary.iter().find(|t| t.parcel.key() == key) else {
            return;
        };
        let (text, placeholder) = match field {
            ParcelField::Name => (
                t.name.clone().unwrap_or_default(),
                "Name, such as \"Graphics card\"".to_owned(),
            ),
            ParcelField::Sender => (t.from().to_owned(), "Sender".to_owned()),
        };
        self.summary_packages = true;
        self.summary_receipts = false;
        // A hidden parcel being edited stays in sight.
        if t.hidden {
            self.summary_show_hidden = true;
        }
        self.parcel_editing = Some((key, field));
        self.parcel_input.update(cx, |s, cx| {
            s.set_placeholder(placeholder, w, cx);
            s.set_text(text, w, cx);
            s.focus(w, cx);
        });
        // The whole text selected once the field has focus, so typing
        // replaces it.
        w.on_next_frame(|w, cx| w.dispatch_action(Box::new(gpui_component::input::SelectAll), cx));
        cx.notify();
    }

    /// Saves the name or sender being edited; empty, it goes back to the
    /// emails' (the order or tracking number alone, the newest sender).
    pub(super) fn commit_parcel_edit(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        let Some((key, field)) = self.parcel_editing.take() else {
            return;
        };
        let value = self.parcel_input.read(cx).value().trim().to_owned();
        let auto_sender = self
            .summary
            .iter()
            .find(|t| t.parcel.key() == key)
            .map(|t| t.latest().sender.clone());
        let value = (!value.is_empty()).then_some(value);
        self.override_parcel(
            &key,
            |o| match field {
                ParcelField::Name => o.name = value,
                // The emails' own sender, typed back, is no change.
                ParcelField::Sender => o.sender = value.filter(|v| Some(v) != auto_sender.as_ref()),
            },
            cx,
        );
        self.focus.focus(w, cx);
        cx.notify();
    }

    /// Escape leaves the name or sender as it was.
    pub(super) fn cancel_parcel_edit(&mut self, w: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.parcel_editing.take().is_none() {
            return false;
        }
        self.focus.focus(w, cx);
        cx.notify();
        true
    }

    /// The field editing a parcel's name or sender, in place: clicks in it
    /// don't open or close the parcel.
    fn parcel_field(&self, id: &str, size: f32, cx: &Context<Self>) -> AnyElement {
        let selector = id.to_owned();
        let c = self.colors();
        div()
            .id(SharedString::from(id.to_owned()))
            .debug_selector(move || selector)
            .w_full()
            .max_w(px(360.))
            .px_1()
            .rounded(px(4.))
            .border_1()
            .border_color(rgb(c.accent))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(|_, _, cx| cx.stop_propagation())
            // Escape typed in the field reaches here as the field's own
            // action, not the window's Close: it leaves the value as it was.
            .on_action(cx.listener(|this, _: &gpui_component::input::Escape, w, cx| {
                this.cancel_parcel_edit(w, cx);
            }))
            .child(
                Input::new(&self.parcel_input)
                    .appearance(false)
                    .text_size(px(size)),
            )
            .into_any_element()
    }

    /// Whether the parcel `key`'s `field` is being edited.
    fn editing(&self, key: &str, field: ParcelField) -> bool {
        self.parcel_editing
            .as_ref()
            .is_some_and(|(k, f)| k == key && *f == field)
    }

    /// Opens the Packages page's search field, focused, or closes it and
    /// clears what was typed.
    pub(super) fn search_parcels(&mut self, open: bool, w: &mut Window, cx: &mut Context<Self>) {
        self.summary_search = open;
        if open {
            self.summary_packages = true;
            self.summary_receipts = false;
            self.parcel_search.update(cx, |s, cx| s.focus(w, cx));
            self.load_parcel_texts(cx);
        } else {
            self.parcel_search.update(cx, |s, cx| s.set_value("", w, cx));
            self.focus.focus(w, cx);
        }
        cx.notify();
    }

    /// What is typed in the Packages page's search field, when it is open.
    fn parcel_query(&self, cx: &App) -> Option<String> {
        self.summary_search
            .then(|| self.parcel_search.read(cx).value().trim().to_lowercase())
            .filter(|q| !q.is_empty())
    }

    /// Reads the text of the packages' (or receipts') emails not read yet,
    /// in the background, while their search is open; the list is filtered
    /// again when it arrives.
    pub(super) fn load_parcel_texts(&mut self, cx: &mut Context<Self>) {
        if !(self.summary_search || self.receipts_search) || self.parcel_texts_loading {
            return;
        }
        // The emails of the packages, or of the receipts, whichever search
        // is open.
        let parcels = self
            .summary
            .iter()
            .flat_map(|t| &t.history)
            .map(|e| &e.sighting.mail_id)
            .filter(|_| self.summary_search);
        let receipts = self
            .receipts
            .iter()
            .flat_map(|l| &l.emails)
            .map(|s| &s.mail_id)
            .filter(|_| self.receipts_search);
        let mut missing: Vec<String> = Vec::new();
        let mut asked = HashSet::new();
        for id in parcels.chain(receipts) {
            if !self.parcel_texts.contains_key(id) && asked.insert(id) {
                missing.push(id.clone());
            }
        }
        if missing.is_empty() {
            return;
        }
        self.parcel_texts_loading = true;
        let generation = self.parcel_texts_generation;
        cx.notify();
        self.task(
            cx,
            move |b| {
                let texts = b.store.parcel_search_texts(&missing)?;
                Ok((missing, texts))
            },
            move |this, result, cx| {
                if generation != this.parcel_texts_generation {
                    return;
                }
                this.parcel_texts_loading = false;
                match result {
                    Ok((asked, mut texts)) => {
                        for id in asked {
                            let text = texts.remove(&id).unwrap_or_default();
                            this.parcel_texts.insert(id, text);
                        }
                        // Emails that came while these were read.
                        this.load_parcel_texts(cx);
                    }
                    Err(e) => this.notice(format!("{e:#}"), true, cx),
                }
                cx.notify();
            },
        );
    }

    /// Lets go of the packages' emails' text, and any reading of it.
    fn forget_parcel_texts(&mut self) {
        self.parcel_texts = HashMap::new();
        self.parcel_texts_loading = false;
        self.parcel_texts_generation += 1;
    }

    /// The parcels the Packages page lists: as `listed_parcels`, and those
    /// matching the search when there is one, each with whether only what
    /// its emails say matched.
    fn shown_parcels<'a>(&'a self, cx: &App) -> Vec<(&'a Tracked, bool)> {
        let query = self.parcel_query(cx);
        self.listed_parcels(self.summary_show_hidden)
            .filter_map(|t| match query.as_deref() {
                None => Some((t, false)),
                Some(q) => {
                    let account = self.account_name(&t.latest().account);
                    parcel_matches(t, q, account.as_deref(), &self.parcel_texts).map(|e| (t, e))
                }
            })
            .collect()
    }

    /// The Summary's parcels as listed: without the hidden, unless they
    /// are asked for on the Packages page.
    pub(super) fn listed_parcels(&self, with_hidden: bool) -> impl Iterator<Item = &Tracked> {
        self.summary.iter().filter(move |t| with_hidden || !t.hidden)
    }

    /// The Summary row under an account's (or All Accounts') Inbox in the
    /// sidebar.
    pub(super) fn summary_nav(&self, id: &str, cx: &Context<Self>) -> Stateful<Div> {
        let c = self.colors();
        let active = self.summary_open && self.account.as_deref() == Some(id);
        let action = Action::OpenSummary(id.to_owned());
        let selector = format!("nav-{id}-summary");
        div()
            .id(SharedString::from(selector.clone()))
            .debug_selector(|| selector)
            .h(px(30.))
            .pl(px(if self.sidebar_collapsed { 10. } else { 30. }))
            .pr_2()
            .rounded(px(7.))
            .flex()
            .items_center()
            .gap_2()
            .cursor_pointer()
            .when(active, |d| d.bg(rgb(c.soft)))
            .hover(move |s| s.bg(rgb(if active { c.soft } else { c.surface })))
            .text_color(rgb(if active { c.text } else { c.muted }))
            .child(icon_el(
                "summary",
                16.,
                if active { c.accent } else { c.muted },
            ))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(px(13.))
                    .font_weight(if active {
                        FontWeight::SEMIBOLD
                    } else {
                        FontWeight::MEDIUM
                    })
                    .child("Summary"),
            )
            .on_click(cx.listener(move |this, _, w, cx| this.handle(action.clone(), w, cx)))
    }
    /// The same in the collapsed sidebar.
    pub(super) fn summary_rail(&self, id: &str, cx: &Context<Self>) -> Stateful<Div> {
        let c = self.colors();
        let active = self.summary_open && self.account.as_deref() == Some(id);
        self.rail_button(
            format!("rail-{id}-summary"),
            "Summary".into(),
            icon_el("summary", 17., if active { c.accent } else { c.muted }).into_any_element(),
            active,
            Action::OpenSummary(id.to_owned()),
            cx,
        )
    }

    /// The name of an account, for parcels in All Accounts' Summary.
    pub(super) fn account_name(&self, id: &str) -> Option<String> {
        self.accounts
            .iter()
            .find(|a| a.id == id)
            .map(|a| a.label().to_owned())
    }

    /// The Summary, in the list's and reader's place (or a phone's screen).
    pub(super) fn summary_page(
        &self,
        single: bool,
        corners: Corners<Pixels>,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let c = self.colors();
        let all = self.account.as_deref() == Some(fwd::ALL_MAIL);
        let title = if all {
            "All Accounts".to_owned()
        } else {
            self.account
                .as_deref()
                .and_then(|id| self.account_name(id))
                .unwrap_or_default()
        };
        let page = self.summary_packages || self.summary_receipts;
        let body = if self.summary_receipts {
            self.receipts_page(all, cx)
        } else if page {
            self.packages_page(all, cx)
        } else {
            self.summary_widgets(single, all, cx)
        };
        div()
            .id("summary-page")
            .debug_selector(|| "summary-page".into())
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .h_full()
            .when(single, |d| d.w_full())
            .bg(rgb(c.surface))
            .rounded_bl(corners.bottom_left)
            .rounded_br(corners.bottom_right)
            .child(
                div()
                    .px_5()
                    .py(px(10.))
                    .border_b_1()
                    .border_color(rgb(c.border))
                    .flex()
                    .items_center()
                    .gap_2()
                    .when(page, |d| {
                        d.child(
                            div()
                                .id("summary-back")
                                .debug_selector(|| "summary-back".into())
                                .flex()
                                .items_center()
                                .gap_1()
                                .cursor_pointer()
                                .text_size(px(13.))
                                .text_color(rgb(c.accent))
                                .hover(|s| s.opacity(0.75))
                                .child(icon_el("arrow-left", 13., c.accent))
                                .child("Summary")
                                .on_click(cx.listener(|this, _, w, cx| {
                                    let back = if this.summary_receipts {
                                        Action::SummaryReceipts(false)
                                    } else {
                                        Action::SummaryPackages(false)
                                    };
                                    this.handle(back, w, cx)
                                })),
                        )
                    })
                    // The page's name, and the account under it.
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .min_w_0()
                            .child(
                                div()
                                    .text_size(px(17.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(if self.summary_receipts {
                                        "Receipts"
                                    } else if page {
                                        "Packages"
                                    } else {
                                        "Summary"
                                    }),
                            )
                            .when(!title.is_empty(), |d| {
                                d.child(
                                    div()
                                        .id("summary-account")
                                        .debug_selector(|| "summary-account".into())
                                        .min_w_0()
                                        .truncate()
                                        .text_size(px(12.))
                                        .text_color(rgb(c.muted))
                                        .child(title),
                                )
                            }),
                    )
                    .when(self.summary_loading || self.summary_fetching, |d| {
                        d.child(spinner(13., c.muted))
                    }),
            )
            .child(
                div()
                    .id("summary-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_5()
                    .py_4()
                    .child(body),
            )
    }

    /// The Summary's widgets, laid out from the top left: Packages, then
    /// Receipts.
    fn summary_widgets(&self, single: bool, all: bool, cx: &Context<Self>) -> AnyElement {
        div()
            .flex()
            .flex_wrap()
            .items_start()
            .gap_4()
            .child(self.packages_widget(single, all, cx))
            .child(self.receipts_widget(single, all, cx))
            .into_any_element()
    }

    /// The Packages widget: the last updated few parcels, and a link to the
    /// Packages page with every one.
    fn packages_widget(&self, single: bool, all: bool, cx: &Context<Self>) -> AnyElement {
        let c = self.colors();
        let packages = self.listed_parcels(false).count();
        let shown = packages.min(WIDGET_PACKAGES);
        div()
            .id("summary-packages-widget")
            .debug_selector(|| "summary-packages-widget".into())
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
                    .child(icon_el("package", 15., c.accent))
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(13.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Packages"),
                    )
                    .children(self.packages_count(packages)),
            )
            .children(self.summary_download_status(true, cx).map(|status| {
                div().px_3().pt_2().child(status).into_any_element()
            }))
            .when(packages == 0, |d| {
                d.child(
                    div()
                        .id("summary-packages-empty")
                        .debug_selector(|| "summary-packages-empty".into())
                        .px_3()
                        .py_4()
                        .flex()
                        .items_center()
                        .gap_2()
                        .text_size(px(12.))
                        .text_color(rgb(c.muted))
                        .child(icon_el("package", 18., c.border))
                        .child(
                            if self.summary_loading || self.summary_downloads.is_some() {
                                "Looking through your mail for packages…"
                            } else {
                                "No packages in this mail"
                            },
                        ),
                )
            })
            .children(
                self.listed_parcels(false)
                    .take(shown)
                    .enumerate()
                    .map(|(i, t)| self.widget_package(i, t, all, cx)),
            )
            // Also when every parcel is hidden: the Packages page shows them.
            .when(!self.summary.is_empty(), |d| {
                d.child(
                    div()
                        .px_3()
                        .py_2()
                        .border_t_1()
                        .border_color(rgb(c.border))
                        .flex()
                        .items_center()
                        .child(div().flex_1().text_size(px(11.)).text_color(rgb(c.muted)).child(
                            if packages > shown {
                                format!("{shown} of {packages}")
                            } else {
                                String::new()
                            },
                        ))
                        .child(self.summary_link(
                            "summary-view-more",
                            "View more…",
                            Action::SummaryPackages(true),
                            cx,
                        )),
                )
            })
            .into_any_element()
    }

    /// "2 on the way · 7 in all", or "7 delivered".
    fn packages_count(&self, packages: usize) -> Option<Div> {
        let c = self.colors();
        let active = self
            .listed_parcels(false)
            .filter(|t| !t.finished() && !quiet(t))
            .count();
        (packages > 0).then(|| {
            div()
                .text_size(px(12.))
                .text_color(rgb(c.muted))
                .child(if active > 0 {
                    format!("{active} on the way · {packages} in all")
                } else {
                    format!("{packages} in all")
                })
        })
    }

    /// A parcel in the Packages widget: its name, where it is, and how far
    /// along; clicking it shows it on the Packages page.
    fn widget_package(&self, i: usize, t: &Tracked, all: bool, cx: &Context<Self>) -> AnyElement {
        let c = self.colors();
        let p = &t.parcel;
        let delivered = t.done();
        let mut detail = vec![status_text(t)];
        // Its sender as set by hand first, then its carrier, then its
        // newest email's sender.
        let from = t
            .sender
            .clone()
            .or_else(|| p.carrier.clone())
            .or_else(|| sender_name(&t.latest().sender));
        if let Some(from) = from {
            detail.push(from);
        }
        if quiet(t) {
            detail.push(format!("No news since {}", date_full(t.updated)));
        } else if !delivered
            && let Some(arrival) = p.expected_arrival.as_deref().and_then(arrival_date)
        {
            detail.push(format!("Expected {arrival}"));
        } else {
            detail.push(date_full(t.updated));
        }
        let account = all.then(|| self.account_name(&t.latest().account)).flatten();
        let key = p.key();
        div()
            .id(SharedString::from(format!("summary-widget-package-{i}")))
            .debug_selector(move || format!("summary-widget-package-{i}"))
            .px_3()
            .py(px(6.))
            .flex()
            .items_center()
            .gap_2()
            .cursor_pointer()
            .when(i > 0, |d| d.border_t_1().border_color(rgb(c.border)))
            .hover(move |s| s.bg(rgb(c.raised)))
            .on_mouse_down(MouseButton::Right, {
                let key = key.clone();
                cx.listener(move |this, _: &MouseDownEvent, w, cx| {
                    this.handle(Action::ParcelMenu(key.clone()), w, cx)
                })
            })
            .on_click(cx.listener(move |this, _, w, cx| {
                this.handle(Action::OpenParcel(key.clone()), w, cx)
            }))
            .child(icon_el(
                if delivered { "package-check" } else { "package" },
                16.,
                if delivered { c.muted } else { c.accent },
            ))
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
                                    .child(t.name.clone().unwrap_or_else(|| p.title())),
                            )
                            // Named, its order or number still shows beside.
                            .when(t.name.is_some(), |d| {
                                d.child(
                                    div()
                                        .min_w_0()
                                        .truncate()
                                        .text_size(px(11.))
                                        .text_color(rgb(c.muted))
                                        .child(p.title()),
                                )
                            })
                            .when_some(account, |d, name| d.child(tag(&name, c))),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_size(px(11.))
                            .text_color(rgb(if t.trouble.is_some() {
                                c.danger
                            } else {
                                c.muted
                            }))
                            .child(detail.join(" · ")),
                    ),
            )
            .child(self.stepper(format!("summary-widget-stepper-{i}"), t, false))
            .into_any_element()
    }

    /// The Packages page: every parcel, last updated first, each opening
    /// to its history.
    fn packages_page(&self, all: bool, cx: &Context<Self>) -> AnyElement {
        let c = self.colors();
        let packages = self.listed_parcels(false).count();
        let hidden = self.summary.len() - packages;
        let show_hidden = self.summary_show_hidden;
        div()
            .id("summary-packages-page")
            .debug_selector(|| "summary-packages-page".into())
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(icon_el("package", 16., c.accent))
                    .child(
                        div()
                            .text_size(px(14.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Packages"),
                    )
                    .children(self.packages_count(packages))
                    .child(div().flex_1())
                    .child(self.search_parcels_button(cx))
                    .when(hidden > 0, |d| {
                        d.child(self.summary_link(
                            "summary-show-hidden",
                            &if show_hidden {
                                format!("Leave Out {hidden} Hidden")
                            } else {
                                format!("Show {hidden} Hidden")
                            },
                            Action::ShowHiddenParcels(!show_hidden),
                            cx,
                        ))
                    }),
            )
            .when(self.summary_search, |d| {
                d.child(
                    div()
                        .id("summary-search")
                        .debug_selector(|| "summary-search".into())
                        .w_full()
                        .max_w(px(420.))
                        // Escape typed in the field reaches here as the
                        // field's own action, not the window's Close.
                        .on_action(cx.listener(
                            |this, _: &gpui_component::input::Escape, w, cx| {
                                this.handle(Action::SearchParcels(false), w, cx)
                            },
                        ))
                        .child(field(
                            Input::new(&self.parcel_search)
                                .prefix(icon_el("search", 13., c.muted))
                                .cleanable(true),
                            c,
                            false,
                        )),
                )
            })
            .when(self.summary_search && self.parcel_texts_loading, |d| {
                d.child(
                    div()
                        .id("summary-search-reading")
                        .debug_selector(|| "summary-search-reading".into())
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
            .child(self.packages(all, cx))
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(c.muted))
                    .child(format!(
                        "Found in {} emails on this device{}. Nothing is looked up \
                         online; Track opens the carrier's page.",
                        fmt_count(self.summary_scanned),
                        if self.find_parcels && self.parcel_model.is_some() {
                            ", with tracking numbers confirmed by the local \
                             tracking number model"
                        } else {
                            " (turn on Find tracking numbers in Settings → \
                             Local AI to find more)"
                        }
                    )),
            )
            .into_any_element()
    }

    /// The Packages page's search button: opens the search field, or
    /// closes it when it is open.
    fn search_parcels_button(&self, cx: &Context<Self>) -> AnyElement {
        let c = self.colors();
        let open = self.summary_search;
        div()
            .id("summary-search-button")
            .debug_selector(|| "summary-search-button".into())
            .size(px(if self.mobile.on { super::mobile::TOUCH } else { 26. }))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(6.))
            .cursor_pointer()
            .when(open, |d| d.bg(rgb(c.soft)))
            .hover(move |s| s.bg(rgb(c.border)))
            .child(icon_el("search", 14., if open { c.accent } else { c.muted }))
            .tooltip(|w, cx| Tooltip::new("Search Packages").build(w, cx))
            .on_click(cx.listener(move |this, _, w, cx| {
                this.handle(Action::SearchParcels(!open), w, cx)
            }))
            .into_any_element()
    }

    /// How far downloading shipping mail has got, or why it stopped.
    /// `compact` in the Packages widget: said shortly, as it is narrow.
    /// Either way the words wrap rather than run past the Stop link.
    pub(super) fn summary_download_status(&self, compact: bool, cx: &Context<Self>) -> Option<AnyElement> {
        let c = self.colors();
        if let Some((done, total)) = self.summary_downloads {
            let done = fmt_count(done);
            let total = fmt_count(total);
            return Some(
                div()
                    .id("summary-downloads")
                    .debug_selector(|| "summary-downloads".into())
                    .w_full()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_size(px(12.))
                    .text_color(rgb(c.muted))
                    .child(div().flex_shrink_0().child(spinner(11., c.muted)))
                    .child(
                        div()
                            .id("summary-downloads-text")
                            .debug_selector(|| "summary-downloads-text".into())
                            .flex_1()
                            .min_w_0()
                            .child(if compact {
                                format!("Downloading emails: {done} of {total}")
                            } else {
                                format!(
                                    "Downloading shipping and receipt emails to read their \
                                     tracking numbers and totals: {done} of {total}"
                                )
                            }),
                    )
                    .child(self.summary_link(
                        "summary-downloads-stop",
                        "Stop",
                        Action::StopSummaryDownloads,
                        cx,
                    ))
                    .into_any_element(),
            );
        }
        let error = self.summary_error.clone()?;
        Some(
            div()
                .id("summary-download-error")
                .debug_selector(|| "summary-download-error".into())
                .w_full()
                .min_w_0()
                .text_size(px(12.))
                .text_color(rgb(c.muted))
                .child(error)
                .into_any_element(),
        )
    }

    /// A link in the Summary: it doesn't also open or close the parcel it
    /// is in.
    pub(super) fn summary_link(
        &self,
        id: &str,
        label: &str,
        action: Action,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let c = self.colors();
        let selector = id.to_owned();
        div()
            .id(SharedString::from(id.to_owned()))
            .debug_selector(move || selector)
            .flex_shrink_0()
            .text_size(px(12.))
            .text_color(rgb(c.accent))
            .cursor_pointer()
            .hover(|s| s.opacity(0.75))
            .child(label.to_owned())
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |this, _, w, cx| {
                cx.stop_propagation();
                this.handle(action.clone(), w, cx)
            }))
    }

    /// The Packages section: a parcel per row, last updated first.
    fn packages(&self, all: bool, cx: &Context<Self>) -> AnyElement {
        let c = self.colors();
        let shown = self.shown_parcels(cx);
        if shown.is_empty() {
            let searched = self.parcel_query(cx).is_some()
                && self.listed_parcels(self.summary_show_hidden).next().is_some();
            return div()
                .id("summary-packages-empty")
                .debug_selector(|| "summary-packages-empty".into())
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
                .child(icon_el("package", 28., c.border))
                .child(
                    if searched {
                        "No packages match your search"
                    } else if self.summary_loading || self.summary_downloads.is_some() {
                        "Looking through your mail for packages…"
                    } else {
                        "No packages in this mail"
                    },
                )
                .into_any_element();
        }
        div()
            .id("summary-packages")
            .debug_selector(|| "summary-packages".into())
            .flex()
            .flex_col()
            .rounded(px(10.))
            .border_1()
            .border_color(rgb(c.border))
            .children(
                shown
                    .into_iter()
                    .enumerate()
                    .map(|(i, (t, in_email))| self.package(i, t, all, in_email, cx)),
            )
            .into_any_element()
    }

    /// How far along the way a parcel is: a dot per stage, filled up to
    /// where it is (●─●─●─○─○), with the stages named when `labels`.
    fn stepper(&self, id: String, t: &Tracked, labels: bool) -> AnyElement {
        let c = self.colors();
        // Probably delivered: filled to the end, muted, as no email says so.
        let current = if t.assumed { STAGES.len() - 1 } else { t.stage };
        let tone = if t.trouble.is_some() {
            c.danger
        } else if t.assumed {
            c.muted
        } else {
            c.accent
        };
        let dot = |i: usize| {
            let reached = i <= current;
            let size = if i == current { 12. } else { 9. };
            div()
                .size(px(size))
                .flex_shrink_0()
                .rounded_full()
                .when(reached, |d| {
                    d.bg(rgb(if i == current { tone } else { c.accent }))
                })
                .when(!reached, |d| d.border_2().border_color(rgb(c.border)))
        };
        let bar = |i: usize| {
            div()
                .h(px(2.))
                .when(labels, |d| d.flex_1())
                .when(!labels, |d| d.w(px(14.)))
                .bg(rgb(if i <= current { c.accent } else { c.border }))
        };
        let mut track = div().flex().items_center().when(labels, |d| d.w_full());
        for i in 0..STAGES.len() {
            if i > 0 {
                track = track.child(bar(i));
            }
            track = track.child(dot(i));
        }
        let mut out = div()
            .id(SharedString::from(id.clone()))
            .debug_selector(move || id)
            .flex()
            .flex_col()
            .gap_1()
            .when(labels, |d| d.w_full())
            .child(track);
        if labels {
            out = out.child(div().flex().justify_between().text_size(px(11.)).children(
                STAGES.iter().enumerate().map(|(i, stage)| {
                    div()
                        .text_color(rgb(if i == current {
                            tone
                        } else if i < current {
                            c.text
                        } else {
                            c.muted
                        }))
                        .when(i == current, |d| d.font_weight(FontWeight::SEMIBOLD))
                        .child(if i == current {
                            t.status_label()
                        } else {
                            stage
                        })
                }),
            ));
        }
        out.into_any_element()
    }

    /// A parcel: its row, and when opened, its history below it.
    fn package(
        &self,
        i: usize,
        t: &Tracked,
        all: bool,
        in_email: bool,
        cx: &Context<Self>,
    ) -> AnyElement {
        let c = self.colors();
        let p = &t.parcel;
        let key = p.key();
        let open = self.summary_expanded.contains(&key);
        let mut detail: Vec<String> = Vec::new();
        if let Some(carrier) = &p.carrier {
            detail.push(carrier.clone());
        }
        detail.push(status_text(t));
        if !t.done()
            && let Some(arrival) = p.expected_arrival.as_deref().and_then(arrival_date)
        {
            detail.push(format!("Expected {arrival}"));
        }
        if quiet(t) {
            detail.push(format!("No news since {}", date_full(t.updated)));
        } else {
            detail.push(format!("Updated {}", date_full(t.updated)));
        }
        let latest = t.latest();
        let mut from = if t.from().is_empty() {
            latest.subject.clone()
        } else {
            format!("{} · {}", t.from(), latest.subject)
        };
        let editing_name = self.editing(&key, ParcelField::Name);
        let editing_sender = self.editing(&key, ParcelField::Sender);
        if t.history.len() > 1 {
            from.push_str(&format!(" · {} emails", t.history.len()));
        }
        let account = all.then(|| self.account_name(&latest.account)).flatten();
        let delivered = t.done();
        let row = div()
            .id(SharedString::from(format!("summary-package-{i}")))
            .debug_selector(move || format!("summary-package-{i}"))
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
                    this.handle(Action::ParcelMenu(key.clone()), w, cx)
                })
            })
            .on_click(cx.listener({
                let key = key.clone();
                move |this, _, w, cx| this.handle(Action::ToggleParcel(key.clone()), w, cx)
            }))
            .child(icon_el(
                if delivered {
                    "package-check"
                } else {
                    "package"
                },
                18.,
                if delivered { c.muted } else { c.accent },
            ))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(1.))
                    // Its name, given by hand, above its order or number.
                    .when(editing_name, |d| {
                        d.child(self.parcel_field(&format!("summary-name-input-{i}"), 13., cx))
                    })
                    .when(!editing_name && t.name.is_some(), |d| {
                        d.child(
                            div()
                                .id(SharedString::from(format!("summary-name-{i}")))
                                .debug_selector(move || format!("summary-name-{i}"))
                                .min_w_0()
                                .truncate()
                                .text_size(px(13.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(t.name.clone().unwrap_or_default()),
                        )
                    })
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .when(t.name.is_some() || editing_name, |d| {
                                        d.text_size(px(12.)).text_color(rgb(c.muted))
                                    })
                                    .when(t.name.is_none() && !editing_name, |d| {
                                        d.text_size(px(13.)).font_weight(FontWeight::MEDIUM)
                                    })
                                    .child(p.title()),
                            )
                            .when(matches!(p.source, Source::Markup), |d| {
                                d.child(tag("From sender", c))
                            })
                            .when_some(account, |d, name| d.child(tag(&name, c)))
                            .when(t.hidden, |d| d.child(tag("Hidden", c)))
                            .when(in_email, |d| {
                                d.child(
                                    div()
                                        .id(SharedString::from(format!("summary-in-email-{i}")))
                                        .debug_selector(move || format!("summary-in-email-{i}"))
                                        .child(tag("Matched in an email", c)),
                                )
                            }),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_size(px(12.))
                            .text_color(rgb(if t.trouble.is_some() {
                                c.danger
                            } else {
                                c.muted
                            }))
                            .child(detail.join(" · ")),
                    )
                    .when(editing_sender, |d| {
                        d.child(self.parcel_field(&format!("summary-sender-input-{i}"), 12., cx))
                    })
                    .when(!editing_sender, |d| {
                        d.child(
                            div()
                                .id(SharedString::from(format!("summary-from-{i}")))
                                .debug_selector(move || format!("summary-from-{i}"))
                                .min_w_0()
                                .truncate()
                                .text_size(px(12.))
                                .text_color(rgb(c.muted))
                                .child(from),
                        )
                    }),
            )
            .child(self.stepper(format!("summary-stepper-{i}"), t, false))
            .child(icon_el(
                if open {
                    "chevron-down"
                } else {
                    "chevron-right"
                },
                12.,
                c.muted,
            ));
        div()
            .flex()
            .flex_col()
            .when(i > 0, |d| d.border_t_1().border_color(rgb(c.border)))
            .child(row)
            .when(open, |d| d.child(self.history(i, t, all, cx)))
            .into_any_element()
    }

    /// A parcel's whole story: how far it is, links to track it, and every
    /// email about it, newest first.
    fn history(&self, i: usize, t: &Tracked, all: bool, cx: &Context<Self>) -> AnyElement {
        let c = self.colors();
        let p = &t.parcel;
        let mut actions: Vec<AnyElement> = Vec::new();
        if let Some(url) = &p.tracking_url {
            actions.push(
                self.summary_link(
                    &format!("summary-track-{i}"),
                    "Track",
                    Action::OpenUrl(url.clone()),
                    cx,
                )
                .into_any_element(),
            );
        }
        if !p.tracking_number.is_empty() {
            actions.push(
                self.summary_link(
                    &format!("summary-copy-{i}"),
                    "Copy Number",
                    Action::CopyText(p.tracking_number.clone()),
                    cx,
                )
                .into_any_element(),
            );
        }
        actions.push(
            self.summary_link(
                &format!("summary-status-{i}"),
                "Set Status",
                Action::ParcelMenu(p.key()),
                cx,
            )
            .into_any_element(),
        );
        let mut facts: Vec<String> = Vec::new();
        if let Some(service) = &p.service {
            facts.push(service.clone());
        }
        if let Some(order) = &p.order
            && !p.tracking_number.is_empty()
        {
            facts.push(format!("Order {order}"));
        }
        if let Some(arrival) = p.expected_arrival.as_deref().and_then(arrival_date) {
            facts.push(format!("Expected {arrival}"));
        }
        div()
            .id(SharedString::from(format!("summary-history-{i}")))
            .debug_selector(move || format!("summary-history-{i}"))
            .px_4()
            .pt_2()
            .pb_3()
            .ml(px(30.))
            .flex()
            .flex_col()
            .gap_3()
            .child(self.stepper(format!("summary-stages-{i}"), t, true))
            .when(!facts.is_empty() || !actions.is_empty(), |d| {
                d.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_3()
                        .text_size(px(12.))
                        .text_color(rgb(c.muted))
                        .when(!facts.is_empty(), |d| {
                            d.child(div().flex_1().min_w_0().truncate().child(facts.join(" · ")))
                        })
                        .when(facts.is_empty(), |d| d.child(div().flex_1()))
                        .children(actions),
                )
            })
            .child(
                div()
                    .text_size(px(12.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("History"),
            )
            .child(
                div().flex().flex_col().children(
                    t.history.iter().enumerate().map(|(j, e)| {
                        self.history_event(i, j, e, j + 1 == t.history.len(), all, cx)
                    }),
                ),
            )
            .into_any_element()
    }

    /// An email in a parcel's history: when, what it said, and a way to
    /// open it.
    fn history_event(
        &self,
        i: usize,
        j: usize,
        e: &Event,
        last: bool,
        all: bool,
        cx: &Context<Self>,
    ) -> AnyElement {
        let c = self.colors();
        let s = &e.sighting;
        let what = e.status.map_or("Update", |s| s.label());
        let mut who = if s.sender.is_empty() {
            String::new()
        } else {
            s.sender.clone()
        };
        if all && let Some(name) = self.account_name(&s.account) {
            if !who.is_empty() {
                who.push_str(" · ");
            }
            who.push_str(&name);
        }
        let open = Action::OpenSummaryMail(s.account.clone(), s.mailbox.clone(), s.mail_id.clone());
        let tone = match e.status {
            Some(st) if st.stage().is_none() => c.danger,
            Some(_) => c.accent,
            None => c.muted,
        };
        div()
            .id(SharedString::from(format!("summary-event-{i}-{j}")))
            .debug_selector(move || format!("summary-event-{i}-{j}"))
            .flex()
            .gap_3()
            .child(
                // The timeline: a dot per email, joined by a line.
                div()
                    .w(px(10.))
                    .flex_shrink_0()
                    .flex()
                    .flex_col()
                    .items_center()
                    .child(div().mt(px(4.)).size(px(8.)).rounded_full().bg(rgb(tone)))
                    .when(!last, |d| {
                        d.child(div().w(px(2.)).flex_1().bg(rgb(c.border)))
                    }),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .pb_3()
                    .flex()
                    .flex_col()
                    .gap(px(1.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .text_size(px(12.))
                            .child(
                                div()
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(rgb(if e.status.is_some() {
                                        c.text
                                    } else {
                                        c.muted
                                    }))
                                    .child(what),
                            )
                            .child(div().text_color(rgb(c.muted)).child(date_full(e.at)))
                            .child(div().flex_1())
                            .child(self.summary_link(
                                &format!("summary-open-{i}-{j}"),
                                "Open Email",
                                open,
                                cx,
                            )),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_size(px(12.))
                            .child(s.subject.clone()),
                    )
                    .when(!who.is_empty(), |d| {
                        d.child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_size(px(11.))
                                .text_color(rgb(c.muted))
                                .child(who),
                        )
                    }),
            )
            .into_any_element()
    }

    /// The parcels the reader finds in received mail `m`: in its markup,
    /// links and, confirmed by the model when it is loaded, its text.
    pub(super) fn found_parcels(&self, m: &Mail) -> Rc<Vec<Parcel>> {
        if matches!(m.folder, Folder::Sent | Folder::Drafts) {
            return Rc::default();
        }
        if m.body.is_none() && m.html.is_none() {
            return Rc::default();
        }
        let body = m.body.as_deref().unwrap_or_default();
        let model = self.parcel_model.as_deref().filter(|_| self.find_parcels);
        let size = body.len() + m.html.as_deref().map_or(0, str::len);
        let key = (m.subject.len(), size, model.is_some());
        if let Some((k, found)) = self.parcels_found.borrow().get(&m.id)
            && *k == key
        {
            return found.clone();
        }
        let found = Rc::new(parcels::find(
            &m.subject,
            body,
            m.html.as_deref(),
            m.date,
            model,
        ));
        let mut cache = self.parcels_found.borrow_mut();
        if cache.len() >= FOUND_CACHE {
            cache.clear();
        }
        cache.insert(m.id.clone(), (key, found.clone()));
        found
    }

    /// Above a message that tells of parcels: each, its carrier and stage,
    /// and links to track it and copy its number.
    pub(super) fn parcel_banners(&self, m: &Mail, cx: &Context<Self>) -> Vec<AnyElement> {
        let found = self.found_parcels(m);
        let c = self.colors();
        found
            .iter()
            .zip(BANNERS)
            .enumerate()
            .map(|(i, (p, id))| {
                let mut text = if p.tracking_number.is_empty() {
                    p.title()
                } else {
                    format!("Tracking number {}", p.tracking_number)
                };
                if let Some(carrier) = &p.carrier {
                    text.push_str(&format!(" · {carrier}"));
                }
                if let Some(status) = p.status {
                    text.push_str(&format!(" · {}", status.label()));
                }
                let mut actions: Vec<AnyElement> = Vec::new();
                if let Some(url) = &p.tracking_url {
                    actions.push(
                        self.link(
                            &format!("parcel-track-{i}"),
                            "Track Package",
                            Action::OpenUrl(url.clone()),
                            cx,
                        )
                        .into_any_element(),
                    );
                }
                if !p.tracking_number.is_empty() {
                    if !actions.is_empty() {
                        actions.push(div().text_color(rgb(c.muted)).child("·").into_any_element());
                    }
                    actions.push(
                        self.link(
                            &format!("parcel-copy-{i}"),
                            "Copy Number",
                            Action::CopyText(p.tracking_number.clone()),
                            cx,
                        )
                        .into_any_element(),
                    );
                }
                self.banner(id, "package", BannerStyle::Outlined, text, actions)
                    .into_any_element()
            })
            .collect()
    }
}

/// Where a parcel is, as its row says it: "Delivered (set by you)" for a
/// status set by hand.
fn status_text(t: &Tracked) -> String {
    if t.manual.is_some() {
        format!("{} (set by you)", t.status_label())
    } else {
        t.status_label().to_owned()
    }
}

/// Whether a parcel matches a search (lowercase): every word of it is in
/// its name, tracking number, orders, carrier, sender, status, account,
/// or one of its emails' subjects or senders; or else in those or what
/// its emails say (`texts`, by message id). None when it doesn't match;
/// Some(true) when only its emails' text made it match.
fn parcel_matches(
    t: &Tracked,
    query: &str,
    account: Option<&str>,
    texts: &HashMap<String, String>,
) -> Option<bool> {
    let p = &t.parcel;
    let mut fields: Vec<&str> = vec![&p.tracking_number, t.from(), t.status_label()];
    fields.extend(t.name.as_deref());
    fields.extend(p.carrier.as_deref());
    fields.extend(p.order.as_deref());
    fields.extend(p.orders.iter().map(String::as_str));
    fields.extend(p.contents.as_deref());
    fields.extend(p.products.iter().map(String::as_str));
    fields.extend(account);
    for e in &t.history {
        fields.push(&e.sighting.subject);
        fields.push(&e.sighting.sender);
    }
    let fields = fields.join("\n").to_lowercase();
    let words: Vec<&str> = query.split_whitespace().collect();
    if words.iter().all(|w| fields.contains(w)) {
        return Some(false);
    }
    let said = |w: &str| {
        t.history
            .iter()
            .filter_map(|e| texts.get(&e.sighting.mail_id))
            .any(|text| text.contains(w))
    };
    words.iter().all(|w| fields.contains(w) || said(w)).then_some(true)
}

/// Whether a parcel not delivered has had no news for `QUIET_DAYS`.
fn quiet(t: &Tracked) -> bool {
    t.manual.is_none()
        && !t.finished()
        && t.trouble.is_none()
        && t.updated < chrono::Utc::now().timestamp() - QUIET_DAYS * 86_400
}

/// A sender as the widget names it: its name, without its address.
fn sender_name(sender: &str) -> Option<String> {
    let name = match sender.find('<') {
        Some(i) => sender[..i].trim(),
        None => sender.trim(),
    }
    .trim_matches('"')
    .trim();
    (!name.is_empty()).then(|| name.to_owned())
}

/// A small label beside a parcel's title.
pub(super) fn tag(text: &str, c: Colors) -> Div {
    div()
        .flex_shrink_0()
        .text_size(px(10.))
        .px_1()
        .rounded(px(4.))
        .bg(rgb(c.soft))
        .text_color(rgb(c.muted))
        .child(text.to_owned())
}

/// An expected arrival as the Summary shows it: its date, from the markup's
/// ISO 8601 date or date and time.
fn arrival_date(raw: &str) -> Option<String> {
    let date = raw.get(..10)?;
    let date = chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").ok()?;
    Some(date.format("%b %-d, %Y").to_string())
}

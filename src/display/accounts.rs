//! Mailbox health, recovery actions, and sidebar warnings.
use super::*;

impl MailApp {
    pub(super) fn retry_account(&mut self, id: String, cx: &mut Context<Self>) {
        let Some(a) = self.accounts.iter().find(|a| a.id == id).cloned() else {
            return;
        };
        if self.retrying.contains(&id)
            || self.sync_requests.iter().any(|(account, _)| account == &id)
        {
            return;
        }
        self.retrying.insert(id.clone());
        let retry = id.clone();
        self.task(
            cx,
            move |b| {
                b.retry_credentials(&retry);
                Ok(())
            },
            move |this, _, cx| {
                this.retrying.remove(&id);
                let visible = this.shows(&id) && this.folder != Folder::Drafts;
                let mailbox = if visible {
                    this.mailbox()
                } else {
                    Folder::Inbox.into()
                };
                let limit = if visible { SYNC_WINDOW } else { 100 };
                this.sync_mailbox(a, mailbox, limit, cx);
            },
        );
    }
    pub(super) fn reconnect(&mut self, id: String, w: &mut Window, cx: &mut Context<Self>) {
        if self.connecting || self.modal != Some(Modal::Reconnect(id.clone())) {
            return;
        }
        let Some(a) = self.accounts.iter().find(|a| a.id == id).cloned() else {
            return;
        };
        let password = Self::value(&self.form[PASSWORD], cx);
        let password = if a.service == Service::Gmail {
            password.replace(' ', "")
        } else {
            password
        };
        if !a.oauth && a.service != Service::Pinal && password.is_empty() {
            self.notice(
                format!("Enter your {}.", a.service.password_label().to_lowercase()),
                true,
                cx,
            );
            return;
        }
        Self::set(&self.form[PASSWORD], "", w, cx);
        self.sign_in_step = SignInStep::Browser;
        self.start_reconnect(id, cx, move |b, cancel| b.reconnect(&a, password, cancel));
    }
    /// Repeats a sign-in-again that failed after the browser.
    pub(super) fn retry_reconnect(&mut self, id: String, cx: &mut Context<Self>) {
        if self.connecting || !self.accounts.iter().any(|a| a.id == id) {
            return;
        }
        self.sign_in_step = SignInStep::Finishing;
        self.start_reconnect(id, cx, |b, cancel| b.retry_sign_in(cancel));
    }
    fn start_reconnect(
        &mut self,
        id: String,
        cx: &mut Context<Self>,
        sign_in: impl FnOnce(&Backend, Arc<AtomicBool>) -> anyhow::Result<Account> + Send + 'static,
    ) {
        self.cancel = Arc::new(AtomicBool::new(false));
        let cancel = self.cancel.clone();
        let cancelled = cancel.clone();
        self.connecting = true;
        self.sign_in_retry = false;
        self.retrying.insert(id.clone());
        self.status.clear();
        self.follow_sign_in(cx);
        self.task(
            cx,
            move |b| {
                let next = sign_in(&b, cancel)?;
                let encrypted = next.service == Service::Pinal && b.encrypted(&next);
                Ok((next, encrypted))
            },
            move |this, result, cx| {
                this.retrying.remove(&id);
                if !Arc::ptr_eq(&this.cancel, &cancelled) {
                    return;
                }
                this.connecting = false;
                if cancelled.load(Ordering::Relaxed) {
                    return;
                }
                match result {
                    Ok((a, encrypted)) => {
                        if let Some(old) = this.accounts.iter_mut().find(|a| a.id == id) {
                            *old = a;
                        }
                        if encrypted {
                            this.encrypted.insert(id.clone());
                        }
                        // Only access errors are resolved by sign-in. A mailbox's
                        // rejected command or SMTP delivery still needs a successful retry.
                        if let Some(issues) = this.account_issues.get_mut(&id) {
                            issues.retain(|source, issue| {
                                *source == IssueSource::Send || issue.access.is_none()
                            });
                        }
                        this.status.clear();
                        this.modal = Some(Modal::Settings);
                        this.page = Page::Account(id.clone());
                        this.retry_account(id, cx);
                    }
                    Err(error) => {
                        this.sign_in_retry = this.backend.can_retry_sign_in();
                        this.notice(format!("{error:#}"), true, cx)
                    }
                }
            },
        );
    }
    pub(super) fn account_warning(&self, id: &str, cx: &Context<Self>) -> Stateful<Div> {
        let c = self.colors();
        let id = id.to_owned();
        let selector = format!("account-warning-{id}");
        div()
            .id(SharedString::from(selector.clone()))
            .debug_selector(move || selector)
            .size(px(22.))
            .flex_shrink_0()
            .rounded(px(4.))
            .bg(rgb(c.bg))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .child(icon_el("warning", 16., c.danger))
            .tooltip(|w, cx| Tooltip::new("Mailbox needs attention · Show details").build(w, cx))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |this, _, w, cx| {
                cx.stop_propagation();
                this.handle(Action::AccountSettings(id.clone()), w, cx);
            }))
    }
    pub(super) fn reconnect_form(&self, id: &str) -> AnyElement {
        let Some(a) = self.accounts.iter().find(|a| a.id == id) else {
            return div().into_any_element();
        };
        let browser = a.oauth || a.service == Service::Pinal;
        div().w_full().flex().flex_col().gap_3()
            .child(div().text_size(px(13.)).child(a.email.clone()))
            .child(div().text_size(px(12.)).text_color(rgb(self.colors().muted)).child(
                if browser { "Sign in with the same email address in your browser. Your cached mail, drafts, and settings are kept." }
                else { "Update the password for this mailbox. Your cached mail, drafts, and settings are kept." }))
            .when_some(a.service.browser_unavailable().filter(|_| !browser), |d, why| d.child(notice_box(why, self.colors())))
            .when(!browser, |d| d.child(self.field(a.service.password_label(), PASSWORD)))
            .when_some(a.service.password_help().filter(|_| !browser), |d, (help, _)| d.child(div().text_size(px(12.)).child(help)))
            .into_any_element()
    }
    pub(super) fn account_blocked(&self, id: &str) -> bool {
        self.retrying.contains(id)
            || self.account_issues.get(id).is_some_and(|issues| {
                issues
                    .iter()
                    .any(|(source, issue)| *source != IssueSource::Send && issue.access.is_some())
            })
    }
    pub(super) fn account_result<T>(
        &mut self,
        id: &str,
        source: IssueSource,
        result: &anyhow::Result<T>,
    ) {
        if !self.accounts.iter().any(|a| a.id == id) {
            return;
        }
        let issues = self.account_issues.entry(id.into()).or_default();
        match result {
            Ok(_) => {
                issues.remove(&source);
                // A successful incoming login also resolves earlier access failures
                // from body downloads/reads. SMTP failures have their own boundary.
                if matches!(source, IssueSource::Sync(_)) {
                    issues.retain(|source, issue| {
                        *source == IssueSource::Send || issue.access.is_none()
                    });
                }
            }
            Err(error) if !error.is::<crate::manage::crypto::Locked>() => {
                issues.insert(source, AccountIssue::new(error));
            }
            Err(_) => {}
        }
    }
}

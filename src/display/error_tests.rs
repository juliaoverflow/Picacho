//! Mailbox failures stay local, survive navigation, and expose explicit recovery.
use super::tests::{act, frame, two_accounts};
use super::*;
use crate::manage::status::AccessFailure;
use std::prelude::v1::test;

#[gpui::test]
fn mailbox_warning_opens_its_details_in_tree_and_rail_without_global_errors(
    cx: &mut TestAppContext,
) {
    let (view, cx, _) = two_accounts(cx);
    let (first, second) = view.read_with(cx, |this, _| {
        (this.accounts[0].id.clone(), this.accounts[1].id.clone())
    });
    view.update_in(cx, |this, _, cx| {
        let error: anyhow::Result<()> =
            Err(anyhow::anyhow!("App password was revoked").context(AccessFailure::Authentication));
        this.account_result(&second, IssueSource::Sync("inbox".into()), &error);
        assert!(this.status.is_empty() && !this.error);
        cx.notify();
    });
    frame(cx);
    assert!(cx.debug_bounds("notice-toast").is_none());
    let selector: &'static str = Box::leak(format!("account-warning-{second}").into_boxed_str());
    let warning = cx
        .debug_bounds(selector)
        .expect("red warning belongs to failing mailbox");
    cx.simulate_click(warning.center(), Modifiers::none());
    view.read_with(cx, |this, _| {
        assert_eq!(this.page, Page::Account(second.clone()));
        assert_eq!(
            this.account.as_ref(),
            Some(&first),
            "opening details does not navigate mail"
        );
        assert!(
            !this.collapsed.contains(&second),
            "warning click does not collapse its parent"
        );
        assert!(
            this.account_issues[&second][&IssueSource::Sync("inbox".into())]
                .details
                .contains("App password was revoked")
        );
    });
    frame(cx);
    assert!(cx.debug_bounds("account-error-details").is_some());
    assert!(cx.debug_bounds("modal-status").is_none());
    act(&view, Action::SettingsPage(Page::General), cx);
    // GPUI keeps a selector's bounds in its frame buffer after the element
    // goes away, so check what General draws rather than stale bounds.
    view.read_with(cx, |this, _| {
        assert_eq!(this.page, Page::General);
        assert!(
            !this.settings_page_has_errors(),
            "General settings has no mailbox errors"
        );
    });
    act(&view, Action::Close, cx);
    act(&view, Action::ToggleSidebar, cx);
    frame(cx);
    let warning = cx
        .debug_bounds(selector)
        .expect("collapsed rail keeps the warning");
    cx.simulate_click(warning.center(), Modifiers::none());
    view.read_with(cx, |this, _| {
        assert_eq!(this.page, Page::Account(second.clone()))
    });
    act(&view, Action::Reconnect(second.clone()), cx);
    act(&view, Action::SubmitReconnect(second), cx);
    view.read_with(cx, |this, _| {
        assert!(!this.connecting);
        assert!(this.status.contains("password"));
        assert_eq!(this.accounts.len(), 2);
        assert_eq!(
            this.messages.len(),
            10,
            "a failed sign-in keeps cached mail"
        );
    });
    act(&view, Action::Close, cx);
    frame(cx);
    assert!(
        cx.debug_bounds("notice-toast").is_none(),
        "sign-in form errors stay in the form"
    );
}

#[gpui::test]
fn recovery_clears_only_resolved_issues_and_ignores_removed_accounts(cx: &mut TestAppContext) {
    let (view, cx, _) = two_accounts(cx);
    view.update_in(cx, |this, _, _| {
        let id = this.accounts[0].id.clone();
        let denied: anyhow::Result<()> =
            Err(anyhow::anyhow!("Keychain denied").context(AccessFailure::Keychain));
        let failed: anyhow::Result<()> = Err(anyhow::anyhow!("Server rejected UID MOVE"));
        this.account_result(&id, IssueSource::Read, &denied);
        this.account_result(&id, IssueSource::Change, &failed);
        this.account_result(&id, IssueSource::Send, &denied);
        assert!(this.account_blocked(&id));
        this.account_result(&id, IssueSource::Sync("inbox".into()), &Ok(()));
        let issues = &this.account_issues[&id];
        assert!(!issues.contains_key(&IssueSource::Read));
        assert!(issues.contains_key(&IssueSource::Change));
        assert!(
            issues.contains_key(&IssueSource::Send),
            "incoming success cannot prove SMTP delivery"
        );
        assert!(
            !this.account_blocked(&id),
            "SMTP failures must not pause incoming mail"
        );
        this.account_result(&id, IssueSource::Change, &Ok(()));
        this.account_result(&id, IssueSource::Send, &Ok(()));
        assert!(this.account_issues[&id].is_empty());
        this.account_result("removed", IssueSource::Download, &failed);
        assert!(!this.account_issues.contains_key("removed"));
    });
}

#[gpui::test]
fn polling_is_independent_of_ui_busy_state_and_coalesces_duplicate_requests(
    cx: &mut TestAppContext,
) {
    let (view, cx, _) = two_accounts(cx);
    view.update_in(cx, |this, _, cx| {
        this.account_issues.clear();
        this.last_check.clear();
        let existing_busy = this.busy;
        this.busy += 7; // unrelated rendering/user work must not starve inbox checks
        this.check_due(cx);
        assert_eq!(this.sync_requests.len(), 2);
        assert_eq!(this.busy, existing_busy + 9);
        this.check_due(cx);
        this.refresh(cx);
        assert_eq!(
            this.sync_requests.len(),
            2,
            "refresh clicks must not queue duplicate syncs"
        );
        assert_eq!(this.busy, existing_busy + 9);
        this.busy -= 7;
    });
    cx.run_until_parked();
    view.read_with(cx, |this, _| {
        assert!(this.sync_requests.is_empty());
        assert!(!this.syncing);
        assert!(
            this.accounts.iter().all(
                |a| this.account_issues[&a.id].contains_key(&IssueSource::Sync("inbox".into()))
            )
        );
        assert!(this.status.is_empty());
    });
}

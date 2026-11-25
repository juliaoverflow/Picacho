//! The app lock: its screen covers Picacho until the passcode unlocks it,
//! Settings → Privacy turns it on, changes and off, and coming back after
//! being away locks again.
use super::applock::{LockAction, LockForm};
use super::tests::{act, frame, open, store, two_accounts};
use super::*;
use crate::manage::applock::{Failures, LockSettings, PasscodeHash};
use std::prelude::v1::test;

fn locked_store(after: u64) -> Arc<Store> {
    let store = store();
    LockSettings {
        passcode: Some(PasscodeHash::new("2468").unwrap()),
        biometrics: false,
        after,
    }
    .save(&store)
    .unwrap();
    store
}

fn type_in(
    view: &Entity<MailApp>,
    field: fn(&MailApp) -> &Entity<InputState>,
    text: &str,
    cx: &mut VisualTestContext,
) {
    view.update_in(cx, |this, w, cx| {
        let state = field(this).clone();
        MailApp::set(&state, text.to_owned(), w, cx);
    });
}

#[gpui::test]
fn picacho_opens_locked_and_only_its_passcode_unlocks_it(cx: &mut TestAppContext) {
    let (view, cx) = open(locked_store(60), cx);
    frame(cx);
    view.read_with(cx, |this, cx| {
        assert!(this.lock.locked);
        assert!(applock::covered(cx), "other windows are covered too");
    });
    assert!(cx.debug_bounds("lock-screen").is_some());
    assert!(cx.debug_bounds("settings-nav").is_none());

    type_in(&view, |this| &this.lock.passcode, "1111", cx);
    act(&view, Action::Lock(LockAction::Unlock), cx);
    frame(cx);
    view.read_with(cx, |this, cx| {
        assert!(this.lock.locked, "a wrong passcode keeps it locked");
        assert_eq!(
            MailApp::value(&this.lock.passcode, cx),
            "",
            "and is cleared"
        );
        assert_eq!(Failures::load(&this.backend.store).count, 1);
    });
    assert!(cx.debug_bounds("lock-error").is_some());

    type_in(&view, |this| &this.lock.passcode, "2468", cx);
    act(&view, Action::Lock(LockAction::Unlock), cx);
    view.read_with(cx, |this, cx| {
        assert!(!this.lock.locked);
        assert!(!applock::covered(cx));
        assert_eq!(
            Failures::load(&this.backend.store).count,
            0,
            "wrong tries are forgotten"
        );
    });
}

#[gpui::test]
fn many_wrong_passcodes_make_the_next_try_wait(cx: &mut TestAppContext) {
    let store = locked_store(60);
    let now = chrono::Utc::now().timestamp();
    for _ in 0..5 {
        Failures::record(&store, now);
    }
    let (view, cx) = open(store, cx);
    // Even the right passcode waits.
    type_in(&view, |this| &this.lock.passcode, "2468", cx);
    act(&view, Action::Lock(LockAction::Unlock), cx);
    view.read_with(cx, |this, _| {
        assert!(this.lock.locked);
        assert!(
            this.lock_error()
                .is_some_and(|e| e.starts_with("Too many wrong passcodes")),
            "{:?}",
            this.lock_error()
        );
    });
}

#[gpui::test]
fn settings_turn_the_lock_on_change_its_passcode_and_turn_it_off(cx: &mut TestAppContext) {
    let (view, cx, _) = two_accounts(cx);
    view.read_with(cx, |this, _| {
        assert!(!this.lock.locked && !this.lock.settings.enabled())
    });
    act(&view, Action::Settings, cx);
    act(&view, Action::SettingsPage(Page::Privacy), cx);
    frame(cx);
    assert!(cx.debug_bounds("app-lock").is_some());
    assert!(cx.debug_bounds("encrypt-database").is_some());

    act(
        &view,
        Action::Lock(LockAction::Form(Some(LockForm::Enable))),
        cx,
    );
    type_in(&view, |this| &this.lock.new_code, "1357", cx);
    type_in(&view, |this| &this.lock.confirm, "1358", cx);
    act(&view, Action::Lock(LockAction::SubmitForm), cx);
    view.read_with(cx, |this, _| {
        assert!(
            !this.lock.settings.enabled(),
            "two different passcodes are refused"
        );
        assert!(this.lock_form_error().is_some());
    });
    type_in(&view, |this| &this.lock.new_code, "12", cx);
    type_in(&view, |this| &this.lock.confirm, "12", cx);
    act(&view, Action::Lock(LockAction::SubmitForm), cx);
    view.read_with(cx, |this, _| {
        assert!(!this.lock.settings.enabled(), "too short")
    });
    type_in(&view, |this| &this.lock.new_code, "1357", cx);
    type_in(&view, |this| &this.lock.confirm, "1357", cx);
    act(&view, Action::Lock(LockAction::SubmitForm), cx);
    view.read_with(cx, |this, _| {
        assert!(this.lock.settings.enabled());
        assert!(!this.lock.locked, "turning it on doesn't lock at once");
        let saved = LockSettings::load(&this.backend.store);
        assert!(saved.verify("1357"));
        assert!(
            crate::manage::applock::hides_contents(&this.backend.store),
            "turning the lock on keeps mail out of notifications and widgets"
        );
    });

    act(&view, Action::Lock(LockAction::SetHideContents(false)), cx);
    view.read_with(cx, |this, _| {
        assert!(!crate::manage::applock::hides_contents(&this.backend.store))
    });
    act(&view, Action::Lock(LockAction::SetAfter(300)), cx);
    view.read_with(cx, |this, _| {
        assert_eq!(LockSettings::load(&this.backend.store).after, 300)
    });

    // Changing needs the current passcode.
    act(
        &view,
        Action::Lock(LockAction::Form(Some(LockForm::Change))),
        cx,
    );
    type_in(&view, |this| &this.lock.current, "0000", cx);
    type_in(&view, |this| &this.lock.new_code, "9753", cx);
    type_in(&view, |this| &this.lock.confirm, "9753", cx);
    act(&view, Action::Lock(LockAction::SubmitForm), cx);
    view.read_with(cx, |this, _| {
        assert!(this.lock.settings.verify("1357"), "unchanged");
        assert!(this.lock_form_error().is_some());
    });
    type_in(&view, |this| &this.lock.current, "1357", cx);
    act(&view, Action::Lock(LockAction::SubmitForm), cx);
    view.read_with(cx, |this, _| {
        assert!(this.lock.settings.verify("9753"));
        assert_eq!(this.lock.settings.after, 300, "kept");
    });

    // Lock Now, then unlock with the new passcode.
    act(&view, Action::Lock(LockAction::LockNow), cx);
    frame(cx);
    assert!(cx.debug_bounds("lock-screen").is_some());
    type_in(&view, |this| &this.lock.passcode, "9753", cx);
    act(&view, Action::Lock(LockAction::Unlock), cx);
    view.read_with(cx, |this, _| assert!(!this.lock.locked));

    act(
        &view,
        Action::Lock(LockAction::Form(Some(LockForm::Disable))),
        cx,
    );
    type_in(&view, |this| &this.lock.current, "9753", cx);
    act(&view, Action::Lock(LockAction::SubmitForm), cx);
    view.read_with(cx, |this, _| {
        assert!(!this.lock.settings.enabled());
        assert!(!LockSettings::load(&this.backend.store).enabled());
    });
    // Off, Lock Now does nothing.
    act(&view, Action::Lock(LockAction::LockNow), cx);
    view.read_with(cx, |this, _| assert!(!this.lock.locked));
}

#[gpui::test]
fn coming_back_after_the_chosen_time_locks_again(cx: &mut TestAppContext) {
    let (view, cx) = open(locked_store(60), cx);
    type_in(&view, |this| &this.lock.passcode, "2468", cx);
    act(&view, Action::Lock(LockAction::Unlock), cx);
    // Away for less than a minute: still open.
    view.update_in(cx, |this, w, cx| {
        this.lock_away_for(Duration::from_secs(20));
        this.lock_activation(true, w, cx);
        assert!(!this.lock.locked);
    });
    // Away for longer: locked.
    view.update_in(cx, |this, w, cx| {
        this.lock_away_for(Duration::from_secs(90));
        this.lock_activation(true, w, cx);
        assert!(this.lock.locked);
        assert!(applock::covered(cx));
    });
}

/// A database whose key is gone opens the recovery window: trying again
/// can't help, Start Over asks once more, then moves it aside and starts
/// Picacho on a new one.
#[gpui::test]
fn a_database_without_its_key_can_be_started_over(cx: &mut TestAppContext) {
    use crate::manage::vault::{self, KeyStore, MemoryKeys};
    cx.update(gpui_component::init);
    let dir = std::env::temp_dir().join(format!("mail-recovery-{}", uuid::Uuid::new_v4()));
    let keys: Arc<MemoryKeys> = Arc::default();
    let store = Store::open_with_keys(&dir, keys.clone()).unwrap();
    store.set_preference("old", "yes").unwrap();
    store.set_encrypted(true).unwrap();
    drop(store);
    let name = std::fs::read_to_string(dir.join(vault::KEY_NAME_FILE)).unwrap();
    keys.delete(&name).unwrap();
    let error = Store::open_with_keys(&dir, keys.clone()).err().unwrap();
    assert_eq!(vault::key_problem(&error), Some(vault::KeyProblem::Missing));

    let started = Rc::new(Cell::new(false));
    let (d, k, s) = (dir.clone(), keys.clone(), started.clone());
    let opener: super::recovery::Opener = Box::new(move || {
        let store = Store::open_with_keys(&d, k.clone())?;
        let s = s.clone();
        Ok(Box::new(move |_: &mut App| {
            assert_eq!(store.preference("old"), None, "a new, empty database");
            s.set(true);
        }) as super::recovery::Start)
    });
    cx.update(|cx| super::recovery::open(dir.clone(), error, opener, cx).unwrap());
    let window = cx.update(|cx| cx.windows()[0]);
    let cx = VisualTestContext::from_window(window, cx).into_mut();
    let click = |cx: &mut VisualTestContext, selector: &'static str| {
        frame(cx);
        let at = cx
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("{selector} was not rendered"))
            .center();
        cx.simulate_click(at, Modifiers::none());
        cx.run_until_parked();
    };
    click(cx, "recovery-try-again");
    assert!(!started.get(), "still no key");
    click(cx, "recovery-start-over");
    assert!(!started.get(), "asks to be sure first");
    click(cx, "recovery-start-over");
    assert!(started.get());
    let aside = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .any(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with("mail.sqlite3.unreadable-")
        });
    assert!(aside, "the old database is kept, aside");
    std::fs::remove_dir_all(dir).unwrap();
}

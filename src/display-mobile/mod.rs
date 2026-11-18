//! Picacho on phones and tablets: the same mail, accounts and settings as
//! the desktop window, presented for touch.
//!
//! This is a presentation, not a second app. It is a child of `display::ui`
//! (declared there with `#[path]`), so it renders the one `MailApp` state,
//! dispatches the same `Action`s and reuses the desktop's views where their
//! content fits a phone: the message body, Settings pages, the setup
//! flow, the composer, subscriptions and dialogs. What changes is the frame
//! around them:
//!
//! - Navigation is a stack (mailboxes → messages → message) with a back
//!   button, the iOS edge swipe and Android's Back, instead of panes. Wide
//!   screens (tablets, large phones in landscape) show the list and the
//!   message side by side.
//! - Frequent actions sit in bars at the bottom, within reach of a thumb;
//!   menus open as sheets from the bottom edge.
//! - Every control is at least `TOUCH` points square.
//! - Content stays inside the safe area and above the software keyboard,
//!   which opens with the layout each field wants (see `platform`).
//! - Rows swipe to archive or mark read, and a long press opens a row's
//!   actions (iOS) or starts selecting (Android).
//!
//! The presentation is on for iOS and Android builds. On a desktop,
//! `PICACHO_MOBILE=1` previews it in a phone-sized window.
use super::*;

mod chrome;
mod compose;
mod list;
mod mailboxes;
mod platform;
mod reader;
mod reorder;
mod settings;
mod sheets;
#[cfg(test)]
mod tests;

pub(super) use platform::Keys;

/// The smallest a touch target may be: Apple's 44 points, Material's 48dp.
pub(super) const TOUCH: f32 = if cfg!(target_os = "android") {
    48.
} else {
    44.
};
/// Content's distance from the screen's sides.
pub(super) const GUTTER: f32 = 16.;
/// A navigation bar's height, below the status bar.
const NAV_BAR: f32 = 52.;
/// A bottom toolbar's height, above the home indicator or navigation bar.
const TOOLBAR: f32 = 56.;
/// Screens at least this wide (and tall) show the list and the open
/// message side by side, as a tablet or a large phone in landscape.
const SPLIT_WIDTH: f32 = 700.;
const SPLIT_HEIGHT: f32 = 480.;
/// The message list's width beside the reader.
const SPLIT_LIST: f32 = 360.;
/// How long screens take to slide in.
const PUSH: Duration = Duration::from_millis(260);

/// Whether this build shows the phone presentation on a desktop.
pub(super) fn desktop_preview() -> bool {
    !cfg!(any(target_os = "android", target_os = "ios"))
        && std::env::var_os("PICACHO_MOBILE").is_some_and(|v| v != "0")
}

/// The touch presentation's own state. Everything about mail, accounts and
/// settings stays in `MailApp`, shared with the desktop.
pub(super) struct Mobile {
    /// The phone presentation is shown.
    pub(super) on: bool,
    /// The mailboxes screen is shown in place of the message list: the
    /// bottom of the navigation stack.
    pub(super) mailboxes: bool,
    /// On a phone, Settings shows a page (true) or its list of pages.
    pub(super) settings_page: bool,
    /// Settings has been on screen since it opened: whether it opened at a
    /// page (an account's, from its menu) was worked out.
    pub(super) settings_seen: bool,
    /// The Settings page last drawn: a page chosen any other way (a menu,
    /// a shortcut) opens over the list too.
    pub(super) settings_last: Option<Page>,
    /// The list and the open message are side by side (a wide screen).
    pub(super) split: bool,
    /// A row being swiped: (message, horizontal offset).
    pub(super) swipe: Option<(String, f32)>,
    /// A pan that started on this row and hasn't moved yet: whether it
    /// swipes the row is decided by its first movement.
    pub(super) pan: Option<String>,
    /// When the last swipe ended; its fling must not start another.
    pub(super) swipe_ended: Option<std::time::Instant>,
    /// How far a pull-to-refresh has been dragged down at the list's top.
    pub(super) pull: f32,
    /// The current touch began with the list at its top, so pulling past
    /// it may refresh. A fling that scrolls up to the top doesn't count.
    pub(super) pull_armed: bool,
    /// A pull let go of past the point: since when its box has said
    /// Refreshing. It hides once mail is no longer being checked.
    pub(super) refreshing: Option<std::time::Instant>,
    /// The pull box going back up out of sight: where it starts, and its
    /// animation's key.
    pub(super) pull_hide: Option<(f32, u64)>,
    /// The pull box settling from where it was let go of into its
    /// Refreshing place: where it starts, and its animation's key.
    pub(super) pull_settle: Option<(f32, u64)>,
    /// Bumped for each time the pull box settles or hides.
    pub(super) pull_anim: u64,
    /// The mailboxes screen is sorting accounts: each is a row with a handle.
    pub(super) reordering: bool,
    /// The account being dragged by its handle, and how far its row has
    /// been dragged from its place.
    pub(super) reorder_drag: Option<(String, f32)>,
    /// Where the person is: the screen, dialog, composer and setup step.
    /// When it changes, a field left focused behind lets go of the keyboard.
    pub(super) place: Option<Place>,
    /// The text field the keyboard was last opened for.
    pub(super) keyboard_for: Option<FocusHandle>,
    /// Status bar contents last set dark (false) or light (true).
    pub(super) chrome_dark: Option<bool>,
    /// Bumped when a screen changes; keys its entrance.
    pub(super) push_anim: u64,
    /// How the screen that just came in enters, and when it started.
    pub(super) slide: Slide,
    /// The screen last drawn.
    pub(super) drawn: Option<Screen>,
    /// The `push_anim` the last change of screen was worked out for.
    pub(super) seen_anim: u64,
    /// On a wide screen, a change between the mailboxes, the list and the
    /// message moves only the column that changed: which one (none when
    /// neither did), for this `push_anim`. The screen stays in place.
    pub(super) pane_slide: Option<(Option<SplitPane>, u64)>,
    /// A pan from the leading edge on a wide screen, where the screen
    /// doesn't follow the finger: how far it has gone.
    pub(super) split_edge: Option<f32>,
    pub(super) slide_at: Option<std::time::Instant>,
    /// A pan that started at the leading edge and hasn't moved yet.
    pub(super) edge_pending: bool,
    /// How far the screen has been dragged from the leading edge to go back.
    pub(super) back_drag: Option<f32>,
    /// A let-go back drag (or back button) finishing its slide: where it
    /// starts, whether it goes back, and its animation's key.
    pub(super) back_release: Option<(f32, bool, u64)>,
    /// The back drag's speed (points a second) and when it last moved,
    /// to tell a flick from a slow drag.
    pub(super) back_flick: f32,
    pub(super) back_moved: Option<std::time::Instant>,
    pub(super) mailboxes_scroll: ScrollHandle,
    pub(super) settings_scroll: ScrollHandle,
    pub(super) sheet_scroll: ScrollHandle,
    /// The filter chips' sideways scroll.
    pub(super) filters_scroll: ScrollHandle,
    /// Stands in for the system's insets in tests.
    #[cfg(test)]
    pub(super) fake_insets: Option<Insets>,
}

impl Mobile {
    pub(super) fn new() -> Self {
        Self {
            on: cfg!(any(target_os = "android", target_os = "ios")) || desktop_preview(),
            mailboxes: false,
            settings_page: false,
            settings_seen: false,
            settings_last: None,
            split: false,
            swipe: None,
            swipe_ended: None,
            pan: None,
            pull: 0.,
            pull_armed: false,
            refreshing: None,
            pull_hide: None,
            pull_settle: None,
            pull_anim: 0,
            reordering: false,
            reorder_drag: None,
            keyboard_for: None,
            place: None,
            chrome_dark: None,
            push_anim: 0,
            drawn: None,
            seen_anim: 0,
            pane_slide: None,
            split_edge: None,
            slide: Slide::None,
            slide_at: None,
            edge_pending: false,
            back_drag: None,
            back_release: None,
            back_flick: 0.,
            back_moved: None,
            mailboxes_scroll: ScrollHandle::new(),
            settings_scroll: ScrollHandle::new(),
            sheet_scroll: ScrollHandle::new(),
            filters_scroll: ScrollHandle::new(),
            #[cfg(test)]
            fake_insets: None,
        }
    }
}

/// Space the system keeps: status bar, notch, home indicator, navigation
/// bar and the software keyboard, in points.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct Insets {
    pub(super) top: f32,
    pub(super) bottom: f32,
    pub(super) left: f32,
    pub(super) right: f32,
    /// The keyboard's height, when it is up (0 otherwise).
    pub(super) keyboard: f32,
}

impl Insets {
    /// The space below content: the keyboard when it is up, else the
    /// home indicator's area.
    pub(super) fn below(&self) -> f32 {
        self.keyboard.max(self.bottom)
    }
}

/// Where the person is: the screen, dialog, composer (and whether its Send
/// Later page is open) and setup step.
pub(super) type Place = (Screen, Option<Modal>, Option<(EntityId, bool)>, Step);

/// How a screen enters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Slide {
    /// Already in place (as after a back drag finished).
    None,
    /// Deeper: from the trailing edge.
    Push,
    /// Back: from just past the leading edge, fading in.
    Pop,
    /// The next or previous message: from below or above.
    Next,
    Previous,
}

/// A column of the wide screen's list and message, side by side.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SplitPane {
    /// The mailboxes or the message list.
    Leading,
    /// The open message, or the empty space for one.
    Reader,
}

/// How far a pan from the leading edge of a wide screen goes to go back.
const SPLIT_EDGE: f32 = 80.;

/// Pans starting this close to the leading edge go back.
const EDGE: f32 = 24.;

/// Which navigation level is on screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Screen {
    Setup,
    Mailboxes,
    List,
    Reader,
    Subscriptions,
    Settings,
}

impl MailApp {
    /// The screen the stack shows now, worked out from the shared state.
    pub(super) fn mobile_screen(&self) -> Screen {
        if self.onboarding {
            Screen::Setup
        } else if self.modal == Some(Modal::Settings) {
            Screen::Settings
        } else if self.subs_open {
            Screen::Subscriptions
        } else if self.selected.is_some() && self.selection.len() <= 1 {
            Screen::Reader
        } else if self.mobile.mailboxes {
            Screen::Mailboxes
        } else {
            Screen::List
        }
    }

    /// Whether the screen is wide enough for the list beside the reader.
    pub(super) fn mobile_split(window: &Window) -> bool {
        let size = window.viewport_size();
        size.width >= px(SPLIT_WIDTH) && size.height >= px(SPLIT_HEIGHT)
    }

    /// The phone's root element, in place of the desktop window's `root`.
    pub(super) fn mobile_root(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let c = self.colors();
        let insets = self.mobile_insets(window);
        // Back is Picacho's while it has somewhere to go back to.
        #[cfg(target_os = "android")]
        crate::android::set_back_enabled(self.mobile_has_back(window, cx));
        // A frame is being drawn: the display runs at its fastest meanwhile.
        #[cfg(target_os = "android")]
        crate::android::drawing();
        if self.mobile_screen() != Screen::Settings {
            self.mobile.settings_seen = false;
            self.mobile.settings_last = None;
        }
        self.sync_system_chrome();
        self.follow_keyboard(window, cx);
        let screen = self.mobile_screen();
        let split = Self::mobile_split(window);
        self.mobile.split = split;
        self.follow_split_slide(screen);
        let content = match screen {
            Screen::Setup => self.mobile_setup(insets, window, cx),
            Screen::Settings => self.mobile_settings(insets, window, cx),
            Screen::Subscriptions => self.mobile_subscriptions(insets, window, cx),
            _ if split => self.mobile_split_view(screen, insets, window, cx),
            Screen::Mailboxes => self.mobile_mailboxes(insets, cx),
            Screen::List => self.mobile_list(insets, window, cx),
            Screen::Reader => self.mobile_reader(insets, true, window, cx),
        };
        // Pushed screens slide in; an edge swipe drags the screen along,
        // over the screen it goes back to.
        let size = window.viewport_size();
        let (width, height) = (f32::from(size.width), f32::from(size.height));
        let dragged = match (self.mobile.back_drag, self.mobile.back_release) {
            (Some(x), _) => Some(x),
            (None, Some((x, _, _))) => Some(x),
            _ => None,
        };
        // A screen pushed just now slides in over the one it came from.
        let pushing = dragged.is_none()
            && self.mobile.slide == Slide::Push
            && self.mobile.slide_at.is_some();
        let beneath = if dragged.is_some() || pushing {
            self.mobile_beneath(screen, insets, window, cx)
        } else {
            None
        };
        // Moved by its corner at a fixed size, so a sliding screen keeps
        // its layout rather than being squeezed.
        let slide = div()
            .id("mobile-screen")
            .debug_selector(|| "mobile-screen".into())
            .absolute()
            .top_0()
            .left_0()
            .w(px(width))
            .h(px(height))
            .flex()
            .flex_col()
            .bg(rgb(c.bg))
            .child(content);
        let ease = |t: f32| 1. - (1. - t).powi(3);
        let slide = match (self.mobile.back_drag, self.mobile.back_release) {
            (Some(x), _) => slide.left(px(x)).shadow_2xl().into_any_element(),
            (None, Some((from, commit, key))) => {
                let to = if commit { width } else { 0. };
                slide
                    .shadow_2xl()
                    .with_animation(
                        ("mobile-back", key),
                        Animation::new(PUSH).with_easing(ease),
                        move |d, t| d.left(px(from + (to - from) * t)),
                    )
                    .into_any_element()
            }
            _ => {
                // A column moving on its own leaves the screen in place.
                let kind = if self.mobile_pane_slide().is_some() {
                    Slide::None
                } else {
                    self.mobile.slide
                };
                slide
                    .with_animation(
                        ("mobile-push", self.mobile.push_anim),
                        Animation::new(PUSH).with_easing(ease),
                        move |d, t| match kind {
                            Slide::None => d,
                            Slide::Push => d.left(px(width * (1. - t))),
                            Slide::Pop => d.left(px(-60. * (1. - t))).opacity(0.5 + 0.5 * t),
                            Slide::Next => d.top(px(48. * (1. - t))).opacity(t),
                            Slide::Previous => d.top(px(-48. * (1. - t))).opacity(t),
                        },
                    )
                    .into_any_element()
            }
        };
        // Beneath a dragged screen: the one it goes back to, slightly
        // behind and dimmed, catching up as the drag goes on.
        let push_key = self.mobile.push_anim;
        let beneath = beneath.map(|under| {
            if pushing {
                // Eases left and dims as the new screen covers it.
                return div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .w(px(width))
                    .h(px(height))
                    .flex()
                    .flex_col()
                    .child(under)
                    .child(
                        div()
                            .id("mobile-push-dim")
                            .absolute()
                            .inset_0()
                            .with_animation(
                                ("mobile-push-dim", push_key),
                                Animation::new(PUSH).with_easing(ease),
                                |d, t| d.bg(rgba(0x0a121c00 | (t * 40.) as u32)),
                            ),
                    )
                    .with_animation(
                        ("mobile-push-under", push_key),
                        Animation::new(PUSH).with_easing(ease),
                        move |d, t| d.left(px(-0.3 * width * t)),
                    )
                    .into_any_element();
            }
            // Let go: it finishes moving along with the screen on top.
            if let Some((from, commit, key)) = self.mobile.back_release {
                let start = from / width.max(1.);
                let end = if commit { 1. } else { 0. };
                return div()
                    .absolute()
                    .top_0()
                    .w(px(width))
                    .h(px(height))
                    .flex()
                    .flex_col()
                    .child(under)
                    .with_animation(
                        ("mobile-back-under", key),
                        Animation::new(PUSH).with_easing(ease),
                        move |d, t| {
                            let p = start + (end - start) * t;
                            d.left(px(-0.3 * width * (1. - p)))
                        },
                    )
                    .into_any_element();
            }
            let progress = dragged.unwrap_or_default() / width.max(1.);
            div()
                .absolute()
                .top_0()
                .left(px(-0.3 * width * (1. - progress)))
                .w(px(width))
                .h(px(height))
                .flex()
                .flex_col()
                .child(under)
                .child(div().absolute().inset_0().bg(rgba(
                    0x0a121c00 | ((1. - progress).clamp(0., 1.) * 40.) as u32,
                )))
                .into_any_element()
        });
        let edge = self.edge_listener(cx);
        div()
            .id("mail-app")
            .key_context("Mail")
            .track_focus(&self.focus)
            .size_full()
            .relative()
            .overflow_hidden()
            .bg(rgb(c.bg))
            .text_color(rgb(c.text))
            .font_family(".SystemUIFont")
            .text_size(px(16.))
            .map(|d| self.app_actions(d, cx))
            .on_back_gesture(
                cx.listener(|this, e: &BackGestureEvent, w, cx| this.mobile_back_gesture(e, w, cx)),
            )
            .child(edge)
            .children(beneath)
            .child(slide)
            .children(self.mobile_composers(insets, cx))
            .when(
                self.modal.is_some() && self.modal != Some(Modal::Settings),
                |d| d.child(self.modal(window, cx)),
            )
            .when(self.menu.is_some(), |d| {
                d.child(self.action_sheet(insets, window, cx))
            })
            .children(self.link_sheet(insets, window, cx))
            .children(self.mobile_toast(insets, screen, cx))
    }

    /// The system's insets: safe area and keyboard.
    pub(super) fn mobile_insets(&self, window: &Window) -> Insets {
        #[cfg(test)]
        if let Some(insets) = self.mobile.fake_insets {
            return insets;
        }
        platform::insets(window)
    }

    /// Goes back one level, as Back or Escape does. Returns false when the
    /// desktop's own `close` should handle it.
    pub(super) fn mobile_back(&mut self, w: &mut Window, cx: &mut Context<Self>) -> bool {
        // The keyboard goes first. One the system already put away (Android
        // closes it on Back itself) lets go of its field on the way back.
        if self.mobile.keyboard_for.is_some() {
            let shown = self.keyboard_shown(w);
            self.mobile.keyboard_for = None;
            self.focus.focus(w, cx);
            platform::hide_keyboard();
            cx.notify();
            if shown {
                return true;
            }
        }
        // The warning before a link: Back is Go Back.
        if self.links.confirm.take().is_some() {
            cx.notify();
            return true;
        }
        // An open full-screen composer closes, keeping its draft.
        if let Some(composer) = self.mobile_composer(cx) {
            composer.update(cx, |_, cx| cx.emit(composer::ComposerEvent::Close));
            return true;
        }
        if self.menu.is_some() || self.presets_for.is_some() || self.model_confirm.is_some() {
            return false;
        }
        // Settings: a page goes back to the list of pages.
        if self.modal == Some(Modal::Settings) {
            if self.mobile.settings_page && !Self::mobile_split(w) {
                if self.fwd_reverse.take().is_none() {
                    self.mobile.settings_page = false;
                }
                self.mobile_popped(cx);
                return true;
            }
            self.mobile.settings_page = false;
            return false;
        }
        if self.modal.is_some() || self.selecting || self.search_open {
            return false;
        }
        match self.mobile_screen() {
            Screen::Subscriptions if self.selected.is_none() && self.subs_sender.is_none() => {
                // Subscriptions were opened from the mailboxes.
                self.open_mailbox(self.account.clone(), Folder::Inbox.into(), w, cx);
                self.mobile.mailboxes = true;
                self.mobile_popped(cx);
                true
            }
            Screen::Reader | Screen::Subscriptions => {
                self.mobile_popped(cx);
                false
            }
            Screen::List => {
                self.mobile.mailboxes = true;
                self.mobile_popped(cx);
                true
            }
            Screen::Setup => {
                if self.setup_has_back() {
                    self.handle(Action::SetupBack, w, cx);
                } else {
                    Self::leave_app();
                }
                true
            }
            Screen::Mailboxes => {
                Self::leave_app();
                true
            }
            Screen::Settings => true,
        }
    }

    /// A text field has the software keyboard, and it is on screen: Android
    /// may have put it away without telling (Back closes it first).
    fn keyboard_shown(&self, w: &Window) -> bool {
        self.mobile.keyboard_for.is_some()
            && (self.mobile_insets(w).keyboard > 0. || !cfg!(target_os = "android"))
    }

    /// Setup's step has a way back, as its own "Back" link offers.
    fn setup_has_back(&self) -> bool {
        matches!(self.step, Step::Provider | Step::Email | Step::Credentials)
    }

    /// Back at the first screen leaves Picacho, as Android's Back does
    /// (where Android doesn't do it itself, see `android::leave`).
    fn leave_app() {
        #[cfg(target_os = "android")]
        crate::android::leave();
    }

    /// Whether Back has somewhere to go here: anything `mobile_back` (or
    /// the desktop's `close` after it) would close, or a screen to go back
    /// to. Android leaves Back to the system where there isn't (home).
    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    pub(super) fn mobile_has_back(&self, w: &Window, cx: &App) -> bool {
        self.keyboard_shown(w)
            || self.links.confirm.is_some()
            || self.mobile_composer(cx).is_some()
            || self.menu.is_some()
            || self.presets_for.is_some()
            || self.model_confirm.is_some()
            || self.modal.is_some()
            || self.selecting
            || self.search_open
            || match self.mobile_screen() {
                Screen::Mailboxes => false,
                Screen::Setup => self.setup_has_back(),
                _ => true,
            }
    }

    /// Android's Back (its back callback, see `PicachoActivity`): a back
    /// swipe drags the screen as the iOS edge swipe does; letting go past
    /// the point, or the back button, goes back.
    #[cfg(target_os = "android")]
    pub(super) fn platform_back(
        &mut self,
        back: crate::android::Back,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use crate::android::Back;
        let gesture = |phase, progress| BackGestureEvent {
            phase,
            progress,
            ..Default::default()
        };
        match back {
            Back::Swiping { started, progress } => {
                let phase = if started {
                    BackGesturePhase::Started
                } else {
                    BackGesturePhase::Progressed
                };
                self.mobile_back_gesture(&gesture(phase, progress), w, cx);
            }
            Back::Go if self.mobile.back_drag.is_some() => {
                self.mobile_back_gesture(&gesture(BackGesturePhase::Committed, 1.), w, cx);
            }
            // A sheet or the keyboard (no drag for those), or the button.
            Back::Go if self.mobile.back_release.is_none() => self.close(w, cx),
            Back::Go => {}
            Back::Cancelled => {
                self.mobile_back_gesture(&gesture(BackGesturePhase::Cancelled, 0.), w, cx);
            }
        }
    }

    /// Follows a platform back gesture (where the platform has one).
    fn mobile_back_gesture(
        &mut self,
        e: &BackGestureEvent,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let width = f32::from(w.viewport_size().width);
        match e.phase {
            BackGesturePhase::Started | BackGesturePhase::Progressed
                if self.mobile_can_go_back(cx) =>
            {
                self.mobile.back_drag = Some(e.progress.clamp(0., 1.) * width);
            }
            BackGesturePhase::Committed if self.mobile.back_drag.is_some() => {
                self.mobile_release_back(true, w, cx)
            }
            _ if self.mobile.back_drag.is_some() => self.mobile_release_back(false, w, cx),
            _ => {}
        }
        cx.notify();
    }

    /// Whether a back drag can start here: a screen to go back to, and
    /// nothing (a sheet, a composer) over it.
    fn mobile_can_go_back(&self, cx: &App) -> bool {
        !matches!(self.mobile_screen(), Screen::Mailboxes | Screen::Setup)
            && !self.mobile_split_frame()
            && self.menu.is_none()
            && (self.modal.is_none() || self.modal == Some(Modal::Settings))
            && self.mobile_composer(cx).is_none()
            && self.mobile.back_release.is_none()
            && self.links.confirm.is_none()
            && !self.selecting
            && !self.search_open
    }

    /// Listens, before anything under the finger, for pans that start at
    /// the leading edge: those drag the screen back. Pans starting anywhere
    /// else go on to what is under the finger (a row's swipe, a list).
    fn edge_listener(&self, cx: &Context<Self>) -> impl IntoElement {
        let this = cx.weak_entity();
        canvas(
            |_, _, _| {},
            move |_, _, window, _| {
                let this = this.clone();
                window.on_mouse_event(move |e: &ScrollEvent, phase, window, cx| {
                    if phase == DispatchPhase::Capture {
                        let _ = this.update(cx, |this, cx| this.mobile_edge_pan(e, window, cx));
                    }
                });
            },
        )
        .absolute()
        .size_0()
    }

    fn mobile_edge_pan(&mut self, e: &ScrollEvent, w: &mut Window, cx: &mut Context<Self>) {
        let ScrollDelta::Pixels(d) = e.delta else {
            return;
        };
        let (dx, dy) = (f32::from(d.x), f32::from(d.y));
        let width = f32::from(w.viewport_size().width);
        // For a touch pan, the position is where it started.
        let at_edge = f32::from(e.position.x) <= EDGE;
        if self.mobile_split_edge_pan(e.touch_phase, at_edge, dx, dy, w, cx) {
            return;
        }
        match e.touch_phase {
            TouchPhase::Started if at_edge && self.mobile_can_go_back(cx) => {
                self.mobile.edge_pending = true;
                self.mobile_decide_back(dx, dy, cx);
            }
            TouchPhase::Started | TouchPhase::Moved if self.mobile.back_drag.is_some() => {
                cx.stop_propagation();
                if let Some(x) = &mut self.mobile.back_drag {
                    *x = (*x + dx).clamp(0., width);
                }
                let now = std::time::Instant::now();
                let dt = self
                    .mobile
                    .back_moved
                    .map_or(0.016, |t| now.duration_since(t).as_secs_f32().max(0.004));
                self.mobile.back_flick = dx / dt;
                self.mobile.back_moved = Some(now);
                cx.notify();
            }
            TouchPhase::Moved if self.mobile.edge_pending => self.mobile_decide_back(dx, dy, cx),
            TouchPhase::Ended | TouchPhase::Cancelled => {
                self.mobile.edge_pending = false;
                if let Some(x) = self.mobile.back_drag {
                    cx.stop_propagation();
                    // Past the middle-ish, or flicked toward it (a fast last
                    // movement, still moving when let go), goes back.
                    let moving = self
                        .mobile
                        .back_moved
                        .is_some_and(|t| t.elapsed() < std::time::Duration::from_millis(80));
                    let commit = x > width * 0.4 || (moving && self.mobile.back_flick > 500.);
                    self.mobile_release_back(commit, w, cx);
                }
            }
            _ => {}
        }
    }

    /// Settles a pan from the edge by its first movement: toward the
    /// trailing edge drags the screen back; anything else is left alone.
    fn mobile_decide_back(&mut self, dx: f32, dy: f32, cx: &mut Context<Self>) {
        if dx == 0. && dy == 0. {
            return;
        }
        self.mobile.edge_pending = false;
        if dx > 0. && dx > dy.abs() {
            cx.stop_propagation();
            // Nothing under the finger may also take this pan.
            self.mobile.pan = None;
            self.mobile.swipe = None;
            self.mobile.back_drag = Some(dx);
            self.mobile.back_flick = 0.;
            self.mobile.back_moved = Some(std::time::Instant::now());
            cx.notify();
        }
    }

    /// Lets go of a back drag: the screen slides the rest of the way (and
    /// then goes back) or returns.
    fn mobile_release_back(&mut self, commit: bool, w: &mut Window, cx: &mut Context<Self>) {
        let from = self.mobile.back_drag.take().unwrap_or_default();
        self.mobile.push_anim += 1;
        let key = self.mobile.push_anim;
        self.mobile.back_release = Some((from, commit, key));
        if commit {
            platform::haptic(platform::Haptic::Selection);
        }
        cx.notify();
        cx.spawn_in(w, async move |this, cx| {
            cx.background_executor().timer(PUSH).await;
            let _ = this.update_in(cx, |this, w, cx| {
                if this.mobile.back_release.map(|(_, _, k)| k) != Some(key) {
                    return;
                }
                this.mobile.back_release = None;
                if commit {
                    this.close(w, cx);
                    // It already slid away: the screen below is in place.
                    this.mobile.slide = Slide::None;
                    this.mobile.push_anim += 1;
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Goes back as a back button does: the screen slides away over the one
    /// below, where there is one to show, else at once.
    pub(super) fn mobile_animate_back(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        let screen = self.mobile_screen();
        if self.mobile_can_go_back(cx) && self.mobile_has_beneath(screen, w) {
            self.mobile.back_drag = Some(0.);
            self.mobile_release_back(true, w, cx);
        } else {
            self.close(w, cx);
        }
    }

    /// Whether the screen below this one can be drawn under it.
    fn mobile_has_beneath(&self, screen: Screen, w: &Window) -> bool {
        if Self::mobile_split(w) {
            return false;
        }
        match screen {
            Screen::Reader => !self.subs_open,
            Screen::List => true,
            Screen::Settings => self.fwd_reverse.is_none(),
            Screen::Subscriptions => self.selected.is_none() && self.subs_sender.is_none(),
            Screen::Mailboxes | Screen::Setup => false,
        }
    }

    /// The screen going back would show, drawn beneath a dragged one.
    fn mobile_beneath(
        &mut self,
        screen: Screen,
        insets: Insets,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !self.mobile_has_beneath(screen, window) {
            return None;
        }
        Some(match screen {
            Screen::Reader => self.mobile_list(insets, window, cx),
            Screen::List | Screen::Subscriptions => self.mobile_mailboxes(insets, cx),
            Screen::Settings if self.mobile.settings_page => {
                self.mobile_settings_list(insets, false, cx)
            }
            Screen::Settings => {
                if self.selected.is_some() {
                    self.mobile_reader(insets, true, window, cx)
                } else if self.mobile.mailboxes {
                    self.mobile_mailboxes(insets, cx)
                } else {
                    self.mobile_list(insets, window, cx)
                }
            }
            Screen::Mailboxes | Screen::Setup => return None,
        })
    }

    /// Slides the screen below in from the leading edge.
    pub(super) fn mobile_popped(&mut self, cx: &mut Context<Self>) {
        self.mobile.push_anim += 1;
        self.mobile.slide = Slide::Pop;
        cx.notify();
    }

    /// Slides the next screen in from the trailing edge.
    pub(super) fn mobile_pushed(&mut self, cx: &mut Context<Self>) {
        // Sorting accounts ends with the mailboxes screen.
        self.mobile.reordering = false;
        self.mobile.reorder_drag = None;
        self.mobile.push_anim += 1;
        self.mobile.slide = Slide::Push;
        self.mobile.slide_at = Some(std::time::Instant::now());
        let key = self.mobile.push_anim;
        // Once it has slid in, the screen it covers is no longer drawn.
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(PUSH).await;
            let _ = this.update(cx, |this, cx| {
                if this.mobile.push_anim == key {
                    this.mobile.slide_at = None;
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    /// Runs a shared action, then slides in what it opened.
    pub(super) fn mobile_push(&mut self, action: Action, w: &mut Window, cx: &mut Context<Self>) {
        let before = self.mobile_screen();
        self.handle(action, w, cx);
        if self.mobile_screen() != before {
            self.mobile_pushed(cx);
        }
    }

    /// The composer covering the screen, if one is open.
    pub(super) fn mobile_composer(&self, cx: &App) -> Option<Entity<Composer>> {
        self.composers
            .iter()
            .rev()
            .find(|c| {
                let c = c.read(cx);
                c.placement != Placement::Window && !c.minimized
            })
            .cloned()
    }

    /// Keeps the status bar's text readable on the app's background.
    fn sync_system_chrome(&mut self) {
        if self.mobile.chrome_dark != Some(self.dark) {
            self.mobile.chrome_dark = Some(self.dark);
            platform::system_chrome(self.dark, self.colors().bg);
        }
    }

    /// Opens the keyboard a newly focused field wants, and closes it once
    /// no field has focus. iOS raises no keyboard by itself.
    fn follow_keyboard(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let place = Some((
            self.mobile_screen(),
            self.modal.clone(),
            // Send Later opening or closing changes the place too.
            self.mobile_composer(cx)
                .map(|c| (c.entity_id(), c.read(cx).later_open())),
            self.step,
        ));
        if place != self.mobile.place {
            self.mobile.place = place;
            // Signing in, sending or going elsewhere left the field focused
            // where it no longer is: let go of it and the keyboard. A field
            // the new place focused itself keeps it.
            if self.mobile.keyboard_for.is_some() && window.focused(cx) == self.mobile.keyboard_for
            {
                self.focus.focus(window, cx);
            }
        }
        let focused = window.focused(cx);
        if focused == self.mobile.keyboard_for {
            return;
        }
        let keys = focused.as_ref().and_then(|f| {
            self.keyboard_fields(cx)
                .into_iter()
                .find(|(field, _)| field == f)
                .map(|(_, keys)| keys)
        });
        match keys {
            Some(keys) => {
                self.mobile.keyboard_for = focused;
                // After the frame, so it wins over the platform's default
                // keyboard for a newly focused field.
                window.on_next_frame(move |_, _| platform::show_keyboard(keys));
            }
            None if self.mobile.keyboard_for.is_some() => {
                self.mobile.keyboard_for = None;
                window.on_next_frame(|_, _| platform::hide_keyboard());
            }
            None => {}
        }
    }

    /// The text fields the app owns, with the keyboard each wants.
    pub(super) fn keyboard_fields(&self, cx: &App) -> Vec<(FocusHandle, Keys)> {
        let mut out = vec![];
        let mut add = |state: &Entity<InputState>, keys: Keys| {
            out.push((state.read(cx).focus_handle(cx), keys));
        };
        for (i, state) in self.form.iter().enumerate() {
            add(
                state,
                match i {
                    EMAIL => Keys::Email,
                    IMAP_HOST | SMTP_HOST => Keys::Url,
                    IMAP_PORT | SMTP_PORT | TOTP => Keys::Number,
                    _ => Keys::Text,
                },
            );
        }
        add(&self.search, Keys::Search);
        add(&self.login_input, Keys::Email);
        add(&self.reply_to_input, Keys::Email);
        for (i, state) in self.identity_inputs.iter().enumerate() {
            add(
                state,
                match i {
                    0 | 2 => Keys::Email,
                    3 => Keys::Url,
                    4 => Keys::Number,
                    _ => Keys::Text,
                },
            );
        }
        for state in [
            &self.rename_input,
            &self.unlock_input,
            &self.label_input,
            &self.nickname_input,
            &self.fwd_key,
            &self.fwd_note,
            &self.fwd_search,
            &self.fwd_prefix,
            &self.fwd_description,
            &self.fwd_contact_search,
        ] {
            add(state, Keys::Text);
        }
        add(&self.fwd_server, Keys::Url);
        add(&self.fwd_contact_new, Keys::Email);
        out.push((self.signature_input.read(cx).focus_handle(cx), Keys::Text));
        if let Some(focus) = self
            .picker
            .as_ref()
            .and_then(|p| p.read(cx).typed_focus(cx))
        {
            out.push((focus, Keys::Text));
        }
        for composer in &self.composers {
            out.extend(composer.read(cx).keyboard_fields(cx));
        }
        out
    }

    /// Lets go of the keyboard when the person scrolls content, as iOS
    /// lists do (keyboardDismissMode onDrag).
    pub(super) fn mobile_dismiss_keyboard(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        if self.mobile.keyboard_for.take().is_some() {
            self.focus.focus(w, cx);
            platform::hide_keyboard();
            cx.notify();
        }
    }

    /// The mailboxes, the list or a message, on a wide screen: the list (or
    /// the mailboxes) and the message side by side.
    fn split_framed(screen: Screen) -> bool {
        matches!(screen, Screen::Mailboxes | Screen::List | Screen::Reader)
    }

    /// Whether the screen shows the list and the message side by side.
    fn mobile_split_frame(&self) -> bool {
        self.mobile.split && Self::split_framed(self.mobile_screen())
    }

    /// Works out, once for each change of screen, whether it stays side by
    /// side, so only a column moves (and which), not the whole screen:
    /// closing the message leaves the list where it is.
    fn follow_split_slide(&mut self, screen: Screen) {
        let key = self.mobile.push_anim;
        if self.mobile.seen_anim != key {
            self.mobile.seen_anim = key;
            let framed = |s: Screen| self.mobile.split && Self::split_framed(s);
            self.mobile.pane_slide = match self.mobile.drawn {
                Some(old) if framed(old) && framed(screen) => Some((
                    match self.mobile.slide {
                        Slide::Next | Slide::Previous => Some(SplitPane::Reader),
                        _ if old == screen => None,
                        _ if (old == Screen::Reader) != (screen == Screen::Reader) => {
                            Some(SplitPane::Reader)
                        }
                        _ => Some(SplitPane::Leading),
                    },
                    key,
                )),
                _ => None,
            };
        }
        self.mobile.drawn = Some(screen);
    }

    /// The column moving on its own for the current change of screen, if
    /// the screen stays side by side (`Some(None)` when neither changed).
    fn mobile_pane_slide(&self) -> Option<Option<SplitPane>> {
        self.mobile
            .pane_slide
            .filter(|(_, key)| *key == self.mobile.push_anim)
            .map(|(pane, _)| pane)
    }

    /// `content` entering as its column of the wide screen, when it is the
    /// one that changed: a short slide and fade within the column.
    fn mobile_pane_motion(&self, pane: SplitPane, content: AnyElement) -> AnyElement {
        if self.mobile_pane_slide() != Some(Some(pane)) {
            return content;
        }
        let kind = self.mobile.slide;
        let name = match pane {
            SplitPane::Leading => "mobile-pane-leading",
            SplitPane::Reader => "mobile-pane-reader",
        };
        div()
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .child(content)
            .with_animation(
                (name, self.mobile.push_anim),
                Animation::new(PUSH).with_easing(|t| 1. - (1. - t).powi(3)),
                move |d, t| match kind {
                    Slide::None => d,
                    Slide::Push => d.left(px(48. * (1. - t))).opacity(t),
                    Slide::Pop => d.left(px(-48. * (1. - t))).opacity(t),
                    Slide::Next => d.top(px(48. * (1. - t))).opacity(t),
                    Slide::Previous => d.top(px(-48. * (1. - t))).opacity(t),
                },
            )
            .into_any_element()
    }

    /// Whether a pan from the leading edge of the wide screen can go back:
    /// closing the message, or from the list to the mailboxes.
    fn mobile_split_can_go_back(&self, cx: &App) -> bool {
        self.mobile_split_frame()
            && self.mobile_screen() != Screen::Mailboxes
            && self.menu.is_none()
            && self.modal.is_none()
            && self.mobile_composer(cx).is_none()
            && self.links.confirm.is_none()
            && !self.selecting
            && !self.search_open
    }

    /// On a wide screen, a pan from the leading edge goes back once it has
    /// gone far enough, without the screen following the finger: only a
    /// column changes. True when the pan is this one's.
    fn mobile_split_edge_pan(
        &mut self,
        phase: TouchPhase,
        at_edge: bool,
        dx: f32,
        dy: f32,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        match phase {
            TouchPhase::Started if at_edge && self.mobile_split_can_go_back(cx) => {
                // Undecided until its first movement.
                self.mobile.split_edge = Some(0.);
                self.mobile_follow_split_edge(dx, dy, cx)
            }
            TouchPhase::Started | TouchPhase::Moved if self.mobile.split_edge.is_some() => {
                self.mobile_follow_split_edge(dx, dy, cx)
            }
            TouchPhase::Ended | TouchPhase::Cancelled => {
                let Some(x) = self.mobile.split_edge.take() else {
                    return false;
                };
                if x <= 0. {
                    return false;
                }
                cx.stop_propagation();
                if phase == TouchPhase::Ended && x >= SPLIT_EDGE {
                    platform::haptic(platform::Haptic::Selection);
                    self.close(w, cx);
                }
                true
            }
            _ => false,
        }
    }

    /// Follows a pan from the wide screen's leading edge. Its first
    /// movement settles it: toward the trailing edge it goes back; anything
    /// else is left to what is under the finger.
    fn mobile_follow_split_edge(&mut self, dx: f32, dy: f32, cx: &mut Context<Self>) -> bool {
        let Some(x) = self.mobile.split_edge else {
            return false;
        };
        if x <= 0. {
            if dx == 0. && dy == 0. {
                return false;
            }
            if !(dx > 0. && dx > dy.abs()) {
                self.mobile.split_edge = None;
                return false;
            }
            // Nothing under the finger may also take this pan.
            self.mobile.pan = None;
            self.mobile.swipe = None;
        }
        cx.stop_propagation();
        self.mobile.split_edge = Some((x + dx).max(f32::MIN_POSITIVE));
        true
    }

    /// The list and the open message side by side, on a wide screen.
    fn mobile_split_view(
        &mut self,
        screen: Screen,
        insets: Insets,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = self.colors();
        let list = if screen == Screen::Mailboxes {
            self.mobile_mailboxes(insets, cx)
        } else {
            self.mobile_list(
                Insets {
                    right: 0.,
                    ..insets
                },
                window,
                cx,
            )
        };
        let list = self.mobile_pane_motion(SplitPane::Leading, list);
        let reader = if self.selected.is_some() {
            self.mobile_reader(Insets { left: 0., ..insets }, false, window, cx)
        } else {
            div()
                .id("mobile-reader-empty")
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_3()
                .bg(rgb(c.surface))
                .text_color(rgb(c.muted))
                .child(icon_el("mail", 44., c.border))
                .child("No message selected")
                .into_any_element()
        };
        let reader = self.mobile_pane_motion(SplitPane::Reader, reader);
        div()
            .size_full()
            .flex()
            .child(
                div()
                    .w(px(SPLIT_LIST + insets.left))
                    .flex_shrink_0()
                    .h_full()
                    .border_r_1()
                    .border_color(rgb(c.border))
                    .overflow_hidden()
                    .child(list),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .overflow_hidden()
                    .child(reader),
            )
            .into_any_element()
    }
}

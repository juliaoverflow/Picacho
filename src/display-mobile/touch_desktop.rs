//! The desktop window on a touch device (Settings → General → Desktop
//! layout), as suits a tablet. The window's content is the desktop's own;
//! this does what the phone presentation's frame otherwise would: keeps it
//! inside the safe area and above the software keyboard, raises the
//! keyboard for a focused field, answers Android's Back, opens context
//! menus on a long press and shows the warning before a link.
use super::*;

impl MailApp {
    /// The desktop window's `root`, fitted to a touch device.
    pub(in crate::display::ui) fn touch_desktop_root(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        // Back is Picacho's while there is something for it to close.
        #[cfg(target_os = "android")]
        crate::android::set_back_enabled(self.touch_desktop_has_back(window, cx));
        // A frame is being drawn: the display runs at its fastest meanwhile.
        #[cfg(target_os = "android")]
        crate::android::drawing();
        self.sync_system_chrome();
        self.follow_keyboard(window, cx);
        let insets = self.mobile_insets(window);
        // Laid out before the window's content, which sizes itself (panes,
        // dialogs, menus) to the frame inside it.
        window::set_safe_area(Edges {
            top: px(insets.top),
            right: px(insets.right),
            bottom: px(insets.below()),
            left: px(insets.left),
        });
        let c = self.colors();
        let root = self.root(window, cx);
        div()
            .id("touch-desktop")
            .debug_selector(|| "touch-desktop".into())
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .bg(rgb(c.bg))
            .pt(px(insets.top))
            .pr(px(insets.right))
            .pb(px(insets.below()))
            .pl(px(insets.left))
            .child(long_press_menus())
            .children(
                self.touch_mail_drag
                    .is_some()
                    .then(|| self.carry_listener(cx)),
            )
            .child(div().relative().flex_1().min_h_0().child(root))
            .children(self.touch_mail_drag_layer())
            .children(self.link_sheet(insets, window, cx))
    }

    /// A control's height (or side): `desktop` under a pointer, and at
    /// least `touch` on a touch device, for a fingertip.
    pub(in crate::display::ui) fn tap(&self, desktop: f32, touch: f32) -> Pixels {
        px(if self.mobile.touch {
            desktop.max(touch)
        } else {
            desktop
        })
    }

    /// The side of the desktop's small square buttons (the top bar's, the
    /// list's): bigger for a fingertip, as far as their bars allow.
    pub(in crate::display::ui) fn small_control(&self) -> f32 {
        if self.mobile.touch { 34. } else { 28. }
    }

    /// An icon-only button's name, shown on a long press in the desktop
    /// window on a touch device, where there is no pointer to hover for
    /// its tooltip. (A long press arrives as a right click.)
    pub(in crate::display::ui) fn long_press_tip(
        &self,
        el: Stateful<Div>,
        tip: impl Into<String>,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        if !self.mobile.touch || self.mobile.on {
            return el;
        }
        let tip = tip.into();
        el.on_mouse_down(
            MouseButton::Right,
            cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                cx.stop_propagation();
                this.notice(tip.clone(), false, cx);
            }),
        )
    }

    /// While a finger carries messages, its long press goes on to
    /// `carry_mail`, before anything under it.
    fn carry_listener(&self, cx: &Context<Self>) -> impl IntoElement {
        let this = cx.weak_entity();
        canvas(
            |_, _, _| {},
            move |_, _, window, _| {
                let this = this.clone();
                window.on_mouse_event(move |e: &LongPressEvent, phase, window, cx| {
                    if phase == DispatchPhase::Capture {
                        let _ = this.update(cx, |this, cx| this.carry_mail(e, window, cx));
                    }
                });
            },
        )
        .absolute()
        .size_0()
    }

    /// The tap a touch device gives as something is picked up.
    pub(in crate::display::ui) fn touch_haptic(&self) {
        platform::haptic(platform::Haptic::Selection);
    }

    /// Whether Back has something to close in the desktop window: what its
    /// `close` (Escape) would, including an open composer (its draft kept)
    /// and the open message however wide the window. Android leaves Back
    /// to the system otherwise.
    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    pub(super) fn touch_desktop_has_back(&self, w: &Window, cx: &App) -> bool {
        self.keyboard_shown(w)
            || self.links.confirm.is_some()
            || self.menu.is_some()
            || self.presets_for.is_some()
            || self.model_confirm.is_some()
            || self.tree_ui.renaming.is_some()
            || self.modal.is_some()
            || self.selecting
            || self.search_open
            || self.mobile_composer(cx).is_some()
            || (self.subs_open && (self.selected.is_some() || self.subs_sender.is_some()))
            || self.selected.is_some()
    }
}

/// A long press nothing else took acts as a right click, which opens the
/// desktop's context menus (a message's, a mailbox's, a link's).
fn long_press_menus() -> impl IntoElement {
    canvas(
        |_, _, _| {},
        |_, _, window, _| {
            // Painted first, so it hears a long press last: anything under
            // the finger that handles one itself prevents this.
            window.on_mouse_event(|e: &LongPressEvent, phase, window, cx| {
                if phase != DispatchPhase::Bubble
                    || e.phase != TouchPhase::Started
                    || window.default_prevented()
                {
                    return;
                }
                window.prevent_default();
                platform::haptic(platform::Haptic::Selection);
                let position = e.start_position;
                // Outside this dispatch, as the press it stands for.
                window.defer(cx, move |window, cx| {
                    window.dispatch_event(
                        PlatformInput::MouseDown(MouseDownEvent {
                            button: MouseButton::Right,
                            position,
                            modifiers: Modifiers::default(),
                            click_count: 1,
                            first_mouse: false,
                        }),
                        cx,
                    );
                    window.dispatch_event(
                        PlatformInput::MouseUp(MouseUpEvent {
                            button: MouseButton::Right,
                            position,
                            modifiers: Modifiers::default(),
                            click_count: 1,
                        }),
                        cx,
                    );
                });
            });
        },
    )
    .absolute()
    .size_0()
}

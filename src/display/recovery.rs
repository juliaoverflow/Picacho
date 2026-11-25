//! What Picacho shows when its local database can't be opened as it starts:
//! why, **Try Again** (a keychain that was locked, or refused, may answer
//! now), and, when the encrypted database's key is gone or wrong, **Start
//! Over**, which moves the database aside (never deleting it) so a new one
//! is made. Quitting leaves everything as it is.
use super::*;
use crate::manage::vault;
use std::path::PathBuf;

/// Starts Picacho once its store is open.
pub(super) type Start = Box<dyn FnOnce(&mut App)>;
/// Tries to open the store; on success, what starts Picacho with it.
pub(super) type Opener = Box<dyn FnMut() -> anyhow::Result<Start>>;

/// How a button looks.
#[derive(Clone, Copy, PartialEq)]
enum Tone {
    Plain,
    Primary,
    Danger,
}

pub(super) struct Recovery {
    path: PathBuf,
    error: String,
    problem: Option<vault::KeyProblem>,
    /// Start Over was pressed once: it asks to be sure.
    confirming: bool,
    trying: bool,
    opener: Opener,
    focus: FocusHandle,
}

/// Opens the recovery window for the store in `path`, which failed to open
/// with `error`.
pub(super) fn open(
    path: PathBuf,
    error: anyhow::Error,
    opener: Opener,
    cx: &mut App,
) -> anyhow::Result<()> {
    let phone = cfg!(any(target_os = "android", target_os = "ios"));
    let bounds = Bounds::centered(None, size(px(560.), px(440.)), cx);
    let mut options = window::detached_options("Picacho".into(), bounds);
    if phone {
        options.window_bounds = None;
    }
    cx.open_window(options, move |window, cx| {
        let view = cx.new(|cx| Recovery {
            path,
            problem: vault::key_problem(&error),
            error: format!("{error:#}"),
            confirming: false,
            trying: false,
            opener,
            focus: cx.focus_handle(),
        });
        view.read(cx).focus.clone().focus(window, cx);
        // Closed without starting: Picacho quits, as nothing else is open.
        window.on_window_should_close(cx, |_, cx| {
            cx.quit();
            true
        });
        cx.new(|cx| Root::new(view, window, cx))
    })?;
    cx.activate(true);
    Ok(())
}

impl Recovery {
    fn try_open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.trying = true;
        match (self.opener)() {
            Ok(start) => {
                // Gone before Picacho starts, so its window is the one it
                // brings forward.
                window.remove_window();
                cx.defer(start);
            }
            Err(e) => {
                self.problem = vault::key_problem(&e);
                self.error = format!("{e:#}");
                self.trying = false;
            }
        }
        cx.notify();
    }

    fn start_over(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.confirming {
            self.confirming = true;
            cx.notify();
            return;
        }
        match vault::move_aside(&self.path) {
            Ok(_) => self.try_open(window, cx),
            Err(e) => {
                self.error = format!("{e:#}");
                cx.notify();
            }
        }
    }

    fn button(
        &self,
        id: &'static str,
        label: &str,
        tone: Tone,
        c: Colors,
        cx: &Context<Self>,
        act: fn(&mut Self, &mut Window, &mut Context<Self>),
    ) -> Stateful<Div> {
        let (bg, fg) = match tone {
            Tone::Danger => (c.danger, 0xffffff),
            Tone::Primary => (c.accent, c.on_accent),
            Tone::Plain => (c.surface, c.text),
        };
        div()
            .id(id)
            .debug_selector(move || id.into())
            .role(Role::Button)
            .aria_label(label.to_owned())
            .focusable()
            .tab_stop(true)
            .h(px(36.))
            .px_4()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(9.))
            .cursor_pointer()
            .bg(rgb(bg))
            .text_color(rgb(fg))
            .font_weight(FontWeight::MEDIUM)
            .border_1()
            .border_color(rgb(if tone == Tone::Plain { c.border } else { bg }))
            .active(|s| s.opacity(0.75))
            .child(label.to_owned())
            .on_click(cx.listener(move |this, _, w, cx| act(this, w, cx)))
    }
}

impl Render for Recovery {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = Colors::new(system_dark(window), Accent::from_key(""));
        let phone = cfg!(any(target_os = "android", target_os = "ios"));
        let start_over = self.problem.is_some();
        let bar = window::detached_bar("recovery-titlebar", c).child(
            div()
                .text_size(px(13.))
                .text_color(rgb(c.muted))
                .child("Picacho"),
        );
        let bar = window::detached_titlebar(bar, c, |_, _, cx| cx.quit(), window, cx);
        let explanation = if start_over {
            t!("recovery-start-over-moves-the-unreadable-database")
        } else {
            t!("recovery-nothing-was-changed-if-the-system")
        };
        let mut buttons = div()
            .flex()
            .flex_wrap()
            .justify_end()
            .gap_2()
            .child(
                self.button("recovery-quit", t!("recovery-quit"), Tone::Plain, c, cx, |_, _, cx| {
                    cx.quit()
                }),
            );
        if start_over {
            buttons = buttons.child(self.button(
                "recovery-start-over",
                if self.confirming {
                    t!("recovery-move-aside-and-start-over")
                } else {
                    t!("recovery-start-over")
                },
                if self.confirming {
                    Tone::Danger
                } else {
                    Tone::Plain
                },
                c,
                cx,
                Self::start_over,
            ));
        }
        buttons = buttons.child(self.button(
            "recovery-try-again",
            if self.trying {
                t!("recovery-trying")
            } else {
                t!("recovery-try-again")
            },
            Tone::Primary,
            c,
            cx,
            Self::try_open,
        ));
        div()
            .id("recovery")
            .debug_selector(|| "recovery".into())
            .key_context("Recovery")
            .track_focus(&self.focus)
            .on_action(|_: &Quit, _, cx| cx.quit())
            .map(|d| window::window_frame(d, c, window))
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(c.bg))
            .text_color(rgb(c.text))
            .font_family(".SystemUIFont")
            .text_size(px(14.))
            .when(!phone, |d| d.child(bar))
            .child(
                div()
                    .id("recovery-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p_6()
                    .when(phone, |d| d.pt(px(64.)))
                    .flex()
                    .flex_col()
                    .gap_4()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .child(icon_el("lock", 28., c.warning))
                            .child(
                                div()
                                    .text_size(px(18.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(t!("recovery-picacho-couldnt-open-its-local-database")),
                            ),
                    )
                    .child(
                        div()
                            .id("recovery-error")
                            .debug_selector(|| "recovery-error".into())
                            .role(Role::Alert)
                            .text_size(px(13.))
                            .child(self.error.clone()),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(rgb(c.muted))
                            .child(explanation),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(rgb(c.muted))
                            .child(t!("recovery-folder", path = self.path.display())),
                    )
                    .child(buttons),
            )
    }
}

//! Pane layout: the collapsible sidebar, its animations and toggle.
use super::*;

/// Ease-out cubic, the curve every pane animation shares.
pub(super) fn ease_out(t: f32) -> f32 {
    1. - (1. - t).powi(3)
}

impl MailApp {
    fn pane_animating(since: Option<std::time::Instant>) -> bool {
        since.is_some_and(|t| t.elapsed() < PANE_ANIMATION)
    }
    /// Re-renders once an animation finishes so its outgoing layer is dropped.
    fn after_animation(cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            Timer::after(PANE_ANIMATION + Duration::from_millis(16)).await;
            let _ = this.update(cx, |_, cx| cx.notify());
        })
        .detach();
    }
    pub(super) fn set_sidebar_collapsed(&mut self, collapsed: bool, cx: &mut Context<Self>) {
        if self.sidebar_collapsed == collapsed {
            return;
        }
        self.sidebar_from = self.sidebar_shown.get();
        self.sidebar_collapsed = collapsed;
        self.sidebar_anim += 1;
        self.sidebar_since = Some(std::time::Instant::now());
        Self::after_animation(cx);
        let _ = self
            .backend
            .store
            .set_preference("sidebar", if collapsed { "collapsed" } else { "expanded" });
        cx.notify();
    }
    /// Rows slide in one after another while the sidebar opens or closes.
    pub(super) fn stagger<E: IntoElement + Styled + 'static>(
        &self,
        el: E,
        index: usize,
    ) -> AnyElement {
        if !Self::pane_animating(self.sidebar_since) {
            return el.into_any_element();
        }
        let delay = index.min(10) as f32 * 0.045;
        el.with_animation(
            ElementId::NamedInteger(format!("sidebar-row-{index}").into(), self.sidebar_anim),
            Animation::new(PANE_ANIMATION + Duration::from_millis(120)),
            move |el, t| {
                let t = ease_out(((t - delay) / (1. - delay)).clamp(0., 1.));
                el.opacity(t).left(px((1. - t) * -10.))
            },
        )
        .into_any_element()
    }
    /// The sidebar toggle, beside Compose (or above it in the rail).
    pub(super) fn sidebar_toggle(&self, collapsed: bool, cx: &Context<Self>) -> Stateful<Div> {
        self.button(
            "toggle-sidebar",
            if collapsed {
                "@Show sidebar · ⌃⌘S"
            } else {
                "@Hide sidebar · ⌃⌘S"
            },
            if collapsed {
                "panel-left-open"
            } else {
                "panel-left-close"
            },
            Action::ToggleSidebar,
            false,
            cx,
        )
    }
    /// The sidebar at `width`. Toggling crossfades the folder tree and the
    /// rail while the width animates, so closing is as smooth as opening.
    pub(super) fn sidebar(&self, width: f32, collapsed: bool, cx: &Context<Self>) -> AnyElement {
        let c = self.colors();
        self.sidebar_shown.set(width);
        let layer = |content: AnyElement, w: f32, collapsed: bool| {
            div()
                .absolute()
                .top_0()
                .left_0()
                .bottom_0()
                .w(px(w))
                .flex()
                .flex_col()
                .child(content)
                .child(self.download_status(collapsed, cx))
        };
        let tree = || {
            layer(
                self.sidebar_tree(cx).into_any_element(),
                self.sidebar_width,
                false,
            )
        };
        let rail = || layer(self.sidebar_rail(cx).into_any_element(), SIDEBAR_RAIL, true);
        let sidebar = div()
            .id("sidebar")
            .debug_selector(|| "sidebar".into())
            .relative()
            .flex_shrink_0()
            .h_full()
            .overflow_hidden()
            .border_r_1()
            .border_color(rgb(c.border));
        if !Self::pane_animating(self.sidebar_since) {
            return sidebar
                .w(px(width))
                .child(if collapsed { rail() } else { tree() })
                .into_any_element();
        }
        let (outgoing, incoming) = if collapsed {
            (tree(), rail())
        } else {
            (rail(), tree())
        };
        let from = self.sidebar_from;
        let animation = || Animation::new(PANE_ANIMATION).with_easing(ease_out);
        sidebar
            .w(px(width))
            .child(outgoing.with_animation(
                ("sidebar-out", self.sidebar_anim),
                animation(),
                |d, t| d.opacity(1. - t),
            ))
            .child(incoming.with_animation(
                ("sidebar-in", self.sidebar_anim),
                animation(),
                |d, t| d.opacity(t),
            ))
            .with_animation(("sidebar", self.sidebar_anim), animation(), move |d, t| {
                d.w(px(from + (width - from) * t))
            })
            .into_any_element()
    }
}

impl MailApp {
    /// An account's picture, or its initials.
    pub(super) fn account_avatar(&self, a: &Account, index: usize, size: f32) -> AnyElement {
        avatar(&initials(&a.name), size, index, self.colors()).into_any_element()
    }
}

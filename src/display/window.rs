//! The window frame when Picacho draws its own decorations (Linux and BSD).
//!
//! GNOME never offers server-side decorations, and KDE honors a request for
//! client-side ones, so on those platforms the topbar becomes the title bar:
//! it moves the window, opens the compositor's window menu, and carries the
//! minimize, maximize and close controls. The root surface is rounded and
//! outlined to match Adwaita and Breeze. GPUI-component's `Root` supplies the
//! shadow and resize edges. macOS and Windows main windows are left unchanged.
//!
//! Message and compose windows (`detached_*`) draw their own title bar on
//! every platform: under macOS's window buttons, as Linux client-side
//! decorations, and with hit-tested caption controls on Windows.
use super::{Colors, icon_el};
use gpui::{prelude::*, *};
use gpui_component::tooltip::Tooltip;

/// Corner radius of a client-decorated window, matching Breeze and Adwaita.
const WINDOW_RADIUS: f32 = 10.;

/// Whether this window draws its own title bar.
pub(super) fn is_client_decorated(w: &Window) -> bool {
    cfg!(any(target_os = "linux", target_os = "freebsd"))
        && matches!(w.window_decorations(), Decorations::Client { .. })
}

/// Radii for the window's outer corners: rounded while Picacho draws its own
/// decorations, square on edges the compositor has tiled or maximized.
pub(super) fn window_corners(w: &Window) -> Corners<Pixels> {
    if !cfg!(any(target_os = "linux", target_os = "freebsd")) {
        return Corners::default();
    }
    let Decorations::Client { tiling } = w.window_decorations() else {
        return Corners::default();
    };
    let r = |a: bool, b: bool| px(if a || b { 0. } else { WINDOW_RADIUS });
    Corners {
        top_left: r(tiling.top, tiling.left),
        top_right: r(tiling.top, tiling.right),
        bottom_left: r(tiling.bottom, tiling.left),
        bottom_right: r(tiling.bottom, tiling.right),
    }
}

thread_local! {
    /// The system's reserved edges (status bar, notch, navigation bar, the
    /// software keyboard) while a touch device shows the desktop window,
    /// which keeps out of them; zero otherwise.
    static SAFE_AREA: std::cell::Cell<Edges<Pixels>> = std::cell::Cell::new(Edges::default());
}

thread_local! {
    /// The window is on a touch device (see `Mobile::touch`), for drawing
    /// that has no `MailApp` to ask. Set as each frame is drawn.
    static TOUCH: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Sets whether the window is on a touch device (see `TOUCH`).
pub(super) fn set_touch(on: bool) {
    TOUCH.with(|t| t.set(on));
}

/// Whether the window is on a touch device (see `TOUCH`).
pub(super) fn touch() -> bool {
    TOUCH.with(|t| t.get())
}

/// Sets the safe area the desktop window keeps out of (see `SAFE_AREA`).
pub(super) fn set_safe_area(edges: Edges<Pixels>) {
    SAFE_AREA.with(|s| s.set(edges));
}

fn safe_area() -> Edges<Pixels> {
    SAFE_AREA.with(|s| s.get())
}

/// How far the visible frame sits inside the window, each side. While
/// Picacho draws its own decorations, a transparent shadow band surrounds
/// the frame (none on edges that are tiled or maximized), and anything
/// drawn there is clipped; with the system's decorations it is zero. A
/// touch device's safe area counts too.
pub(super) fn frame_insets(w: &Window) -> Edges<Pixels> {
    let band = gpui_component::window_paddings(w);
    let safe = safe_area();
    Edges {
        top: band.top + safe.top,
        right: band.right + safe.right,
        bottom: band.bottom + safe.bottom,
        left: band.left + safe.left,
    }
}

/// The space inside the window's visible frame, where the app is laid out,
/// in window coordinates. While Picacho draws its own decorations this is
/// smaller than the window (`viewport_size`): a shadow band and two borders
/// surround it. Layout decisions (pane widths, breakpoints, dialog and
/// composer sizes) use it, so nothing sized to the window runs past the
/// frame and gets cut off.
pub(super) fn content_bounds(w: &Window) -> Bounds<Pixels> {
    let viewport = w.viewport_size();
    let Decorations::Client { tiling } = w.window_decorations() else {
        let safe = safe_area();
        return Bounds::new(
            point(safe.left, safe.top),
            size(
                (viewport.width - safe.left - safe.right).max(px(0.)),
                (viewport.height - safe.top - safe.bottom).max(px(0.)),
            ),
        );
    };
    inner_bounds(viewport, frame_insets(w), tiling, is_client_decorated(w))
}

/// The size of [`content_bounds`].
pub(super) fn content_size(w: &Window) -> Size<Pixels> {
    content_bounds(w).size
}

/// `content_bounds` for a window `viewport` big with this shadow band and
/// tiling; `outlined` when Picacho outlines the frame (`window_frame`).
fn inner_bounds(
    viewport: Size<Pixels>,
    shadow: Edges<Pixels>,
    tiling: Tiling,
    outlined: bool,
) -> Bounds<Pixels> {
    // gpui-component's frame border, and Picacho's outline inside it; both
    // are left off edges that are tiled.
    let border = if outlined { px(2.) } else { px(1.) };
    let side = |inset: Pixels, tiled: bool| inset + if tiled { px(0.) } else { border };
    let (left, right) = (side(shadow.left, tiling.left), side(shadow.right, tiling.right));
    let (top, bottom) = (side(shadow.top, tiling.top), side(shadow.bottom, tiling.bottom));
    Bounds::new(
        point(left, top),
        size(
            (viewport.width - left - right).max(px(0.)),
            (viewport.height - top - bottom).max(px(0.)),
        ),
    )
}

/// Round and outline the root surface when it is the window frame. GPUI clips
/// children only to rectangles, so a full-bleed child that paints its own
/// background into a corner must apply the matching [`window_corners`] radius.
pub(super) fn window_frame<E: Styled>(root: E, c: Colors, w: &Window) -> E {
    if !is_client_decorated(w) {
        return root;
    }
    let Decorations::Client { tiling } = w.window_decorations() else {
        return root;
    };
    let corners = window_corners(w);
    let mut root = root
        .rounded_tl(corners.top_left)
        .rounded_tr(corners.top_right)
        .rounded_bl(corners.bottom_left)
        .rounded_br(corners.bottom_right)
        .border_color(rgb(c.border));
    if !tiling.top {
        root = root.border_t_1();
    }
    if !tiling.bottom {
        root = root.border_b_1();
    }
    if !tiling.left {
        root = root.border_l_1();
    }
    if !tiling.right {
        root = root.border_r_1();
    }
    root
}

/// A double-click on the bar maximizes, as the system title bar would.
pub(super) fn on_titlebar_double_click(event: &ClickEvent, w: &mut Window, _: &mut App) {
    if event.click_count() == 2 {
        if is_client_decorated(w) {
            w.zoom_window();
        } else {
            w.titlebar_double_click();
        }
    }
}

/// Make a finished bar the window's title bar when Picacho draws its own
/// decorations; return it unchanged otherwise. With `controls`, minimize,
/// maximize and close are appended at the trailing edge.
///
/// Buttons in the bar must stop left mouse-down propagation (see
/// [`no_drag`]) so clicking them never moves the window. `on_close` runs for
/// the close button, so the app can refuse while work is in flight.
pub(super) fn client_titlebar(
    bar: Stateful<Div>,
    c: Colors,
    controls: bool,
    on_close: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    w: &mut Window,
    cx: &mut App,
) -> Stateful<Div> {
    if !is_client_decorated(w) {
        return bar;
    }
    // Start the compositor move only once the pointer moves while pressed, so
    // plain clicks and double-clicks on the bar still arrive.
    let pressed = w.use_keyed_state("picacho-titlebar-pressed", cx, |_, _| false);
    let set = |value: bool| {
        let pressed = pressed.clone();
        move |_: &MouseDownEvent, _: &mut Window, cx: &mut App| {
            pressed.update(cx, |p, _| *p = value)
        }
    };
    let release = pressed.clone();
    let moving = pressed.clone();
    let bar = bar
        .on_mouse_down(MouseButton::Left, set(true))
        .on_mouse_down_out(set(false))
        .on_mouse_up(MouseButton::Left, move |_, _, cx| {
            release.update(cx, |p, _| *p = false)
        })
        .on_mouse_move(move |_, w, cx| {
            if *moving.read(cx) {
                moving.update(cx, |p, _| *p = false);
                w.start_window_move();
            }
        })
        .on_mouse_down(MouseButton::Right, |event, w, _| {
            w.show_window_menu(event.position)
        });
    if !controls {
        return bar;
    }
    let available = w.window_controls();
    let maximized = w.is_maximized();
    bar.when(available.minimize, |bar| {
        bar.child(
            control("window-minimize", "window-minimize", "Minimize", c)
                .on_click(|_, w, _| w.minimize_window()),
        )
    })
    .when(available.maximize, |bar| {
        bar.child(
            control(
                "window-maximize",
                if maximized {
                    "window-restore"
                } else {
                    "window-maximize"
                },
                if maximized { "Restore" } else { "Maximize" },
                c,
            )
            .on_click(|_, w, _| w.zoom_window()),
        )
    })
    .child(control("window-close", "window-close", "Close", c).on_click(on_close))
}

/// Keep a press on a title bar button from starting a window move.
pub(super) fn no_drag<E: InteractiveElement>(el: E) -> E {
    el.on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
}

/// A window control the size of the topbar's small icon buttons.
fn control(id: &'static str, icon: &str, label: &'static str, c: Colors) -> Stateful<Div> {
    no_drag(
        div()
            .id(id)
            .debug_selector(move || id.into())
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .size(px(28.))
            .rounded(px(7.))
            .cursor_pointer()
            .hover(move |s| s.bg(rgb(c.raised)))
            .active(|s| s.opacity(0.75))
            .tooltip(move |w, cx| Tooltip::new(label).build(w, cx))
            .child(icon_el(icon, 16., c.muted)),
    )
}

/// Options for a message or compose window: Picacho draws its title bar.
pub(super) fn detached_options(title: SharedString, bounds: Bounds<Pixels>) -> WindowOptions {
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        window_min_size: Some(size(px(420.), px(360.))),
        titlebar: Some(TitlebarOptions {
            title: Some(title),
            appears_transparent: true,
            traffic_light_position: Some(point(px(13.), px(12.))),
        }),
        window_decorations: cfg!(any(target_os = "linux", target_os = "freebsd"))
            .then_some(WindowDecorations::Client),
        app_id: Some(crate::display::desktop::APP_ID.into()),
        ..Default::default()
    }
}

/// The start of a message or compose window's title bar. Add the title
/// and buttons (stopping mouse-down, see [`no_drag`]), then finish it with
/// [`detached_titlebar`].
pub(super) fn detached_bar(id: &'static str, c: Colors) -> Stateful<Div> {
    div()
        .id(id)
        .debug_selector(move || id.into())
        .h(px(super::TITLEBAR_HEIGHT))
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap_1()
        // Room for the macOS window buttons.
        .pl(px(if cfg!(target_os = "macos") { 80. } else { 12. }))
        .pr_2()
        .bg(rgb(c.bg))
        .border_b_1()
        .border_color(rgb(c.border))
        .on_click(on_titlebar_double_click)
}

/// Makes a finished bar the window's title bar: it moves the window and,
/// where the system draws none, carries minimize, maximize and close.
pub(super) fn detached_titlebar(
    bar: Stateful<Div>,
    c: Colors,
    on_close: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    w: &mut Window,
    cx: &mut App,
) -> Stateful<Div> {
    if is_client_decorated(w) {
        return client_titlebar(bar, c, true, on_close, w, cx);
    }
    if cfg!(target_os = "windows") {
        // Windows hit-tests these areas itself: dragging, snapping and the
        // caption buttons behave like a native title bar.
        return bar
            .window_control_area(WindowControlArea::Drag)
            .child(
                control("window-minimize", "window-minimize", "Minimize", c)
                    .window_control_area(WindowControlArea::Min),
            )
            .child(
                control(
                    "window-maximize",
                    if w.is_maximized() {
                        "window-restore"
                    } else {
                        "window-maximize"
                    },
                    "Maximize",
                    c,
                )
                .window_control_area(WindowControlArea::Max),
            )
            .child(
                control("window-close", "window-close", "Close", c)
                    .window_control_area(WindowControlArea::Close),
            );
    }
    bar
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::prelude::v1::test;

    #[test]
    fn content_is_laid_out_inside_the_shadow_and_borders() {
        let viewport = size(px(1200.), px(800.));
        let floating = Tiling::default();
        // A floating window: a 20px band and two 1px borders on each side.
        let band = Edges::all(px(20.));
        assert_eq!(
            inner_bounds(viewport, band, floating, true),
            Bounds::new(point(px(22.), px(22.)), size(px(1156.), px(756.)))
        );
        // Without Picacho's outline, only gpui-component's border.
        assert_eq!(
            inner_bounds(viewport, band, floating, false),
            Bounds::new(point(px(21.), px(21.)), size(px(1158.), px(758.)))
        );
        // Maximized: no band and no borders.
        let tiled = Tiling::tiled();
        let none = Edges::all(px(0.));
        assert_eq!(
            inner_bounds(viewport, none, tiled, true),
            Bounds::new(Point::default(), viewport)
        );
        // Tiled to the left half: the left, top and bottom edges are tiled,
        // so only the right keeps its band and borders.
        let left = Tiling {
            left: true,
            top: true,
            bottom: true,
            right: false,
        };
        let right_band = Edges {
            right: px(20.),
            ..Edges::default()
        };
        assert_eq!(
            inner_bounds(viewport, right_band, left, true),
            Bounds::new(Point::default(), size(px(1178.), px(800.)))
        );
    }
}

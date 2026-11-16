//! What the phone presentation asks of the operating system: the safe
//! area, the software keyboard (and which layout it shows), haptics and the
//! status bar. Each call does nothing where the system has no such thing,
//! so the presentation also runs, as a preview, on a desktop.
use super::Insets;
use gpui::{Pixels, Window};

/// The software keyboard layout a text field wants.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::display) enum Keys {
    Text,
    /// `@` and `.` on the main layer; no autocorrection.
    Email,
    /// Server names: `.` and `/` on the main layer.
    Url,
    /// Digits: ports and sign-in codes.
    Number,
    /// Text, with Search on the return key (set by the field itself).
    Search,
}

/// The system's reserved edges and the keyboard, in points.
pub(super) fn insets(window: &Window) -> Insets {
    let reported = window.insets();
    let px = |p: Pixels| f32::from(p);
    let mut insets = Insets {
        top: px(reported.safe_area.top),
        bottom: px(reported.safe_area.bottom),
        left: px(reported.safe_area.left),
        right: px(reported.safe_area.right),
        keyboard: px(reported.ime.bottom),
    };
    // gpui-mobile reports these itself rather than through GPUI's
    // insets; take whichever is larger.
    #[cfg(target_os = "ios")]
    {
        let (top, bottom, left, right) = gpui_mobile::safe_area_insets();
        insets.top = insets.top.max(top);
        insets.bottom = insets.bottom.max(bottom);
        insets.left = insets.left.max(left);
        insets.right = insets.right.max(right);
        insets.keyboard = insets.keyboard.max(gpui_mobile::keyboard_height());
    }
    #[cfg(target_os = "android")]
    {
        // The keyboard resizes Android's content area, so it arrives here
        // as part of the bottom inset.
        if let Some(window) = gpui_mobile::android::jni::platform().and_then(|p| p.primary_window())
        {
            let safe = window.safe_area_insets_logical();
            insets.top = insets.top.max(safe.top);
            insets.bottom = insets.bottom.max(safe.bottom);
            insets.left = insets.left.max(safe.left);
            insets.right = insets.right.max(safe.right);
        }
        insets.keyboard = insets.keyboard.max(gpui_mobile::keyboard_height());
    }
    // A desktop preview leaves room where a notched phone would.
    if super::desktop_preview() {
        insets.top = insets.top.max(preview_inset("PICACHO_MOBILE_TOP", 47.));
        insets.bottom = insets
            .bottom
            .max(preview_inset("PICACHO_MOBILE_BOTTOM", 34.));
    }
    insets
}

fn preview_inset(name: &str, default: f32) -> f32 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// Raises the software keyboard with the layout for `keys`.
pub(super) fn show_keyboard(keys: Keys) {
    #[cfg(any(target_os = "ios", target_os = "android"))]
    {
        use gpui_mobile::KeyboardType;
        gpui_mobile::show_keyboard_with_type(match keys {
            Keys::Text | Keys::Search => KeyboardType::Default,
            Keys::Email => KeyboardType::EmailAddress,
            Keys::Url => KeyboardType::URL,
            Keys::Number => KeyboardType::NumberPad,
        });
    }
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    let _ = keys;
}

pub(super) fn hide_keyboard() {
    #[cfg(any(target_os = "ios", target_os = "android"))]
    gpui_mobile::hide_keyboard();
}

/// A tap of feedback that confirms a gesture did something.
#[derive(Clone, Copy, Debug)]
pub(super) enum Haptic {
    /// A swipe or long press reached the point where it acts.
    Selection,
    /// Something was done (a swipe's action, a refresh).
    Success,
}

pub(super) fn haptic(kind: Haptic) {
    #[cfg(any(target_os = "ios", target_os = "android"))]
    {
        use gpui_mobile::packages::vibration::{HapticFeedback, haptic_feedback};
        let _ = haptic_feedback(match kind {
            Haptic::Selection => HapticFeedback::Selection,
            Haptic::Success => HapticFeedback::Success,
        });
    }
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    let _ = kind;
}

/// Draws the status bar's text light on dark backgrounds and dark on
/// light ones, and colors Android's navigation bar like the app.
pub(super) fn system_chrome(dark: bool, background: u32) {
    #[cfg(any(target_os = "ios", target_os = "android"))]
    gpui_mobile::set_system_chrome(&gpui_mobile::SystemChromeStyle {
        status_bar_color: Some(background),
        status_bar_style: if dark {
            gpui_mobile::StatusBarContentStyle::Light
        } else {
            gpui_mobile::StatusBarContentStyle::Dark
        },
        navigation_bar_color: Some(background),
    });
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    let _ = (dark, background);
}

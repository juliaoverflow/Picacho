//! Reduce motion: the system's own setting (macOS's Reduce motion,
//! Windows' "Show animations in Windows", GNOME's and KDE's animation
//! settings), or Picacho's own choice in Settings → General. GPUI's
//! animations then show their end state at once (`App::reduce_motion`), and
//! spinners stand still.

/// What Settings → General → Reduce motion says.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Motion {
    /// Follow the system.
    #[default]
    System,
    /// Always reduce motion.
    Reduced,
    /// Always animate.
    Full,
}

impl Motion {
    pub const PREFERENCE: &str = "reduce_motion";
    pub fn key(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Reduced => "on",
            Self::Full => "off",
        }
    }
    pub fn from_key(key: &str) -> Self {
        match key {
            "on" => Self::Reduced,
            "off" => Self::Full,
            _ => Self::System,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::System => t!("motion-automatic"),
            Self::Reduced => t!("motion-on"),
            Self::Full => t!("motion-off"),
        }
    }
    /// Whether motion is reduced with this choice.
    pub fn reduced(self) -> bool {
        match self {
            Self::System => system_reduces_motion(),
            Self::Reduced => true,
            Self::Full => false,
        }
    }
}

/// Runs a command, giving its output when it succeeds.
#[cfg(not(any(target_os = "android", target_os = "ios", windows, test)))]
fn output(program: &str, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new(program)
        .args(args)
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

/// Whether the system asks apps to reduce motion.
#[cfg(not(any(target_os = "android", target_os = "ios", test)))]
pub fn system_reduces_motion() -> bool {
    #[cfg(target_os = "macos")]
    {
        output(
            "defaults",
            &["read", "com.apple.universalaccess", "reduceMotion"],
        )
        .is_some_and(|v| v == "1")
    }
    // "Show animations in Windows" (Settings → Accessibility → Visual
    // effects), asked of the system: no `reg` process (and console) each
    // time the window comes to the front.
    #[cfg(windows)]
    {
        crate::display::windows::animations_off()
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        // GNOME (and others using its settings), then KDE Plasma.
        if let Some(v) = output(
            "gsettings",
            &["get", "org.gnome.desktop.interface", "enable-animations"],
        ) {
            return v == "false";
        }
        output(
            "kreadconfig5",
            &["--group", "KDE", "--key", "AnimationDurationFactor"],
        )
        .or_else(|| {
            output(
                "kreadconfig6",
                &["--group", "KDE", "--key", "AnimationDurationFactor"],
            )
        })
        .and_then(|v| v.parse::<f32>().ok())
        .is_some_and(|f| f == 0.)
    }
}

/// Android: Settings → Accessibility → Remove animations.
#[cfg(all(target_os = "android", not(test)))]
pub fn system_reduces_motion() -> bool {
    crate::android::reduce_motion()
}

/// iOS: Settings → Accessibility → Motion → Reduce Motion.
#[cfg(all(target_os = "ios", not(test)))]
pub fn system_reduces_motion() -> bool {
    #[link(name = "UIKit", kind = "framework")]
    unsafe extern "C" {
        fn UIAccessibilityIsReduceMotionEnabled() -> bool;
    }
    // Safety: a UIKit function without arguments, safe from any thread.
    unsafe { UIAccessibilityIsReduceMotionEnabled() }
}

#[cfg(test)]
pub fn system_reduces_motion() -> bool {
    false
}

/// What Settings → General → Contrast in emails says: an email's own
/// `prefers-contrast` styles (Media Queries 5 §11.4) follow it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Contrast {
    /// Follow the system's high-contrast setting.
    #[default]
    System,
    /// Always ask emails for more contrast.
    More,
    /// Never: emails show their usual styles.
    Standard,
}

impl Contrast {
    pub const PREFERENCE: &str = "email_contrast";
    pub fn key(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::More => "more",
            Self::Standard => "standard",
        }
    }
    pub fn from_key(key: &str) -> Self {
        match key {
            "more" => Self::More,
            "standard" => Self::Standard,
            _ => Self::System,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::System => t!("motion-automatic"),
            Self::More => t!("motion-more"),
            Self::Standard => t!("motion-standard"),
        }
    }
    /// Whether emails are asked for more contrast with this choice.
    pub fn more(self) -> bool {
        match self {
            Self::System => system_prefers_contrast(),
            Self::More => true,
            Self::Standard => false,
        }
    }
}

/// Whether the system asks for more contrast: macOS's Increase contrast,
/// Windows' contrast themes, GNOME's high contrast. Elsewhere, not known.
#[cfg(not(any(target_os = "android", target_os = "ios", test)))]
pub fn system_prefers_contrast() -> bool {
    #[cfg(target_os = "macos")]
    {
        output(
            "defaults",
            &["read", "com.apple.universalaccess", "increaseContrast"],
        )
        .is_some_and(|v| v == "1")
    }
    #[cfg(windows)]
    {
        crate::display::windows::high_contrast()
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        output(
            "gsettings",
            &["get", "org.gnome.desktop.a11y.interface", "high-contrast"],
        )
        .is_some_and(|v| v == "true")
    }
}

/// Not known on phones yet (Android's and iOS's own settings would need
/// platform calls): emails show their usual styles unless asked.
#[cfg(any(target_os = "android", target_os = "ios", test))]
pub fn system_prefers_contrast() -> bool {
    false
}

/// What emails' media queries see (`prefers-reduced-motion`,
/// `prefers-contrast`): set by the window as its settings change, read by
/// the HTML renderer's worker threads.
static EMAIL_REDUCED_MOTION: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static EMAIL_MORE_CONTRAST: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn set_email_preferences(reduced_motion: bool, more_contrast: bool) {
    use std::sync::atomic::Ordering::Relaxed;
    EMAIL_REDUCED_MOTION.store(reduced_motion, Relaxed);
    EMAIL_MORE_CONTRAST.store(more_contrast, Relaxed);
}

/// Media Queries 5 §11.3, §11.4: what an email's `prefers-reduced-motion`
/// and `prefers-contrast` queries see.
pub fn email_preferences() -> blitz_dom::MediaPreferences {
    use std::sync::atomic::Ordering::Relaxed;
    blitz_dom::MediaPreferences {
        reduced_motion: EMAIL_REDUCED_MOTION.load(Relaxed),
        contrast: if EMAIL_MORE_CONTRAST.load(Relaxed) {
            blitz_dom::Contrast::More
        } else {
            blitz_dom::Contrast::NoPreference
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choices_round_trip() {
        for m in [Motion::System, Motion::Reduced, Motion::Full] {
            assert_eq!(Motion::from_key(m.key()), m);
        }
        assert!(Motion::Reduced.reduced());
        assert!(!Motion::Full.reduced());
        assert_eq!(Motion::from_key(""), Motion::System);
    }

    #[test]
    fn contrast_choices_round_trip() {
        for c in [Contrast::System, Contrast::More, Contrast::Standard] {
            assert_eq!(Contrast::from_key(c.key()), c);
        }
        assert!(Contrast::More.more() && !Contrast::Standard.more());
        assert_eq!(Contrast::from_key("junk"), Contrast::System);
    }
}

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
            Self::System => "Automatic",
            Self::Reduced => "On",
            Self::Full => "Off",
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
#[cfg(not(any(target_os = "android", target_os = "ios", test)))]
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
    #[cfg(windows)]
    {
        // "Show animations in Windows" (Settings → Accessibility → Visual effects).
        output(
            "reg",
            &[
                "query",
                r"HKCU\Control Panel\Desktop\WindowMetrics",
                "/v",
                "MinAnimate",
            ],
        )
        .is_some_and(|v| v.split_whitespace().last() == Some("0"))
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
}

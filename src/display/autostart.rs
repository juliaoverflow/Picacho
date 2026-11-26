//! Starting Picacho in the background when the person signs in to their
//! computer (Settings → General). The system starts the background process
//! (`picacho --background <data directory>`, see `resident`), which shows
//! the tray icon and checks mail without opening a window.
//!
//! - Linux and the BSDs: a desktop entry in `$XDG_CONFIG_HOME/autostart`,
//!   or inside a Flatpak, the desktop portal's Background request.
//! - macOS: a launch agent in `~/Library/LaunchAgents`.
//! - Windows: the current user's `Run` key.
use std::path::Path;

/// The name the login item goes by: the desktop entry's, the launch
/// agent's label, the `Run` value.
#[cfg_attr(windows, allow(dead_code))]
const NAME: &str = "tech.pinal.picacho.background";

/// Starts Picacho in the background for the data directory `dir` at each
/// sign-in (`on`), or no longer. Blocks: the portal may ask the person.
pub fn set(dir: &Path, on: bool) -> std::io::Result<()> {
    // Tests never touch the computer's login items.
    if cfg!(test) {
        return Ok(());
    }
    imp::set(dir, on)
}

/// Brings the login item up to date with this build (the app may have
/// moved since it was registered). Does nothing where the system keeps it
/// (Flatpak's portal) or when it isn't wanted.
pub fn refresh(dir: &Path) {
    if cfg!(test) {
        return;
    }
    if let Err(e) = imp::refresh(dir) {
        eprintln!("Picacho: could not update starting at sign-in: {e}");
    }
}

/// A path that can't be written into the login item as it is.
#[allow(dead_code)]
fn unusable(what: &str) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidInput, what.to_owned())
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
mod imp {
    use super::{NAME, unusable};
    use crate::display::{desktop, resident};
    use std::path::{Path, PathBuf};

    fn in_flatpak() -> bool {
        std::env::var_os("FLATPAK_ID").is_some()
    }

    fn entry_path() -> std::io::Result<PathBuf> {
        let base = directories::BaseDirs::new()
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "no home folder"))?;
        Ok(base
            .config_dir()
            .join("autostart")
            .join(format!("{NAME}.desktop")))
    }

    fn entry(dir: &Path) -> std::io::Result<String> {
        let exe = std::env::current_exe()?;
        let exe = exe.to_string_lossy();
        let dir = dir.to_string_lossy();
        // A line break would end the Exec key and start keys of its own.
        if exe.contains(['\n', '\r']) || dir.contains(['\n', '\r']) {
            return Err(unusable("a path with a line break"));
        }
        Ok(format!(
            "[Desktop Entry]\n\
             Type=Application\n\
             Name=Picacho\n\
             Exec={} {} {}\n\
             Icon={}\n\
             Terminal=false\n\
             X-GNOME-Autostart-enabled=true\n",
            desktop::exec_quote(&exe),
            resident::FLAG,
            desktop::exec_quote(&dir),
            desktop::APP_ID,
        ))
    }

    pub(super) fn set(dir: &Path, on: bool) -> std::io::Result<()> {
        if in_flatpak() {
            return portal(dir, on);
        }
        let path = entry_path()?;
        if on {
            desktop::write_if_changed(&path, entry(dir)?.as_bytes())
        } else {
            match std::fs::remove_file(&path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
                _ => Ok(()),
            }
        }
    }

    pub(super) fn refresh(dir: &Path) -> std::io::Result<()> {
        if in_flatpak() {
            return Ok(());
        }
        desktop::write_if_changed(&entry_path()?, entry(dir)?.as_bytes())
    }

    /// Inside a Flatpak the sandbox can't write the session's autostart
    /// folder: the Background portal does, with the command run inside the
    /// sandbox. The desktop may ask the person first.
    fn portal(dir: &Path, on: bool) -> std::io::Result<()> {
        use ashpd::desktop::background::Background;
        let dir = dir.to_string_lossy().into_owned();
        let command = ["picacho".to_owned(), resident::FLAG.to_owned(), dir];
        let reason = t!("settings-autostart-reason");
        let response = futures::executor::block_on(async move {
            Background::request()
                .reason(reason)
                .auto_start(on)
                .command(&command)
                .dbus_activatable(false)
                .send()
                .await?
                .response()
        })
        .map_err(std::io::Error::other)?;
        if on && !response.auto_start() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                t!("settings-autostart-not-allowed"),
            ));
        }
        Ok(())
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use super::{NAME, unusable};
    use crate::display::resident;
    use std::path::{Path, PathBuf};

    fn agent_path() -> std::io::Result<PathBuf> {
        let base = directories::BaseDirs::new()
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "no home folder"))?;
        Ok(base
            .home_dir()
            .join("Library/LaunchAgents")
            .join(format!("{NAME}.plist")))
    }

    /// Escapes text for a property list's `<string>`.
    fn xml(text: &str) -> String {
        text.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    }

    fn agent(dir: &Path) -> std::io::Result<String> {
        let exe = std::env::current_exe()?;
        let (Some(exe), Some(dir)) = (exe.to_str(), dir.to_str()) else {
            return Err(unusable("a path that isn't Unicode"));
        };
        // Started once at sign-in; the background process starts the
        // window's in a process group of its own, which launchd leaves be.
        Ok(format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
             <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
             <plist version=\"1.0\">\n\
             <dict>\n\
             \t<key>Label</key>\n\
             \t<string>{NAME}</string>\n\
             \t<key>ProgramArguments</key>\n\
             \t<array>\n\
             \t\t<string>{}</string>\n\
             \t\t<string>{}</string>\n\
             \t\t<string>{}</string>\n\
             \t</array>\n\
             \t<key>RunAtLoad</key>\n\
             \t<true/>\n\
             \t<key>AbandonProcessGroup</key>\n\
             \t<true/>\n\
             \t<key>LimitLoadToSessionType</key>\n\
             \t<string>Aqua</string>\n\
             \t<key>ProcessType</key>\n\
             \t<string>Interactive</string>\n\
             </dict>\n\
             </plist>\n",
            xml(exe),
            xml(resident::FLAG),
            xml(dir),
        ))
    }

    fn write_if_changed(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
        if std::fs::read(path).is_ok_and(|old| old == bytes) {
            return Ok(());
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let partial = path.with_extension("partial");
        std::fs::write(&partial, bytes)?;
        std::fs::rename(&partial, path)
    }

    pub(super) fn set(dir: &Path, on: bool) -> std::io::Result<()> {
        let path = agent_path()?;
        if on {
            write_if_changed(&path, agent(dir)?.as_bytes())
        } else {
            match std::fs::remove_file(&path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
                _ => Ok(()),
            }
        }
    }

    pub(super) fn refresh(dir: &Path) -> std::io::Result<()> {
        write_if_changed(&agent_path()?, agent(dir)?.as_bytes())
    }
}

#[cfg(windows)]
mod imp {
    use super::unusable;
    use crate::display::resident;
    use std::path::Path;

    const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    const VALUE: &str = "Picacho";

    fn command(dir: &Path) -> std::io::Result<String> {
        let exe = std::env::current_exe()?;
        let (Some(exe), Some(dir)) = (exe.to_str(), dir.to_str()) else {
            return Err(unusable("a path that isn't Unicode"));
        };
        // Windows paths can't hold a quote.
        if exe.contains('"') || dir.contains('"') {
            return Err(unusable("a path with a quote"));
        }
        // Backslashes before a quote are halved when the command line is
        // read: a trailing one (a drive's root) is doubled to stay one.
        let tail = if dir.ends_with('\\') { "\\" } else { "" };
        Ok(format!("\"{exe}\" {} \"{dir}{tail}\"", resident::FLAG))
    }

    pub(super) fn set(dir: &Path, on: bool) -> std::io::Result<()> {
        let key = windows_registry::CURRENT_USER
            .create(RUN)
            .map_err(std::io::Error::other)?;
        if on {
            key.set_string(VALUE, command(dir)?)
                .map_err(std::io::Error::other)
        } else {
            match key.remove_value(VALUE) {
                Ok(()) => Ok(()),
                // Not there: nothing to remove.
                // HRESULT_FROM_WIN32(ERROR_FILE_NOT_FOUND).
                Err(e) if e.code().0 == 0x8007_0002_u32 as i32 => Ok(()),
                Err(e) => Err(std::io::Error::other(e)),
            }
        }
    }

    pub(super) fn refresh(dir: &Path) -> std::io::Result<()> {
        let wanted = command(dir)?;
        let key = windows_registry::CURRENT_USER
            .create(RUN)
            .map_err(std::io::Error::other)?;
        if key.get_string(VALUE).ok().as_deref() != Some(wanted.as_str()) {
            key.set_string(VALUE, wanted)
                .map_err(std::io::Error::other)?;
        }
        Ok(())
    }
}

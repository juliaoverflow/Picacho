//! The app's identity on the Linux desktop. GPUI cannot give a window its own
//! icon there: Wayland compositors and X11 docks look the window's app ID up
//! in a desktop entry and take the icon from it. So every window carries
//! [`APP_ID`], and [`register`] installs a desktop entry and icon under that
//! name for the current user. Failures are ignored: a missing icon must never
//! stop the app.

/// The window app ID (Wayland) and WM_CLASS (X11), and the desktop entry's
/// and icon's name. Matches the macOS bundle identifier.
pub const APP_ID: &str = "tech.pinal.picacho";

/// Installs the desktop entry and icon in `$XDG_DATA_HOME` (by default
/// `~/.local/share`), or updates them when this build differs. Skipped when
/// a system package already provides the entry, or inside a sandbox that
/// ships its own.
pub fn register() {
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    std::thread::spawn(|| {
        let _ = install();
    });
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
fn install() -> std::io::Result<()> {
    use std::path::PathBuf;
    const ICON: &[u8] = include_bytes!("../../assets/brand/icon.png");
    if std::env::var_os("FLATPAK_ID").is_some() || std::env::var_os("SNAP").is_some() {
        return Ok(());
    }
    let entry = format!("applications/{APP_ID}.desktop");
    let system = std::env::var("XDG_DATA_DIRS")
        .ok()
        .filter(|dirs| !dirs.is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    if system
        .split(':')
        .any(|dir| PathBuf::from(dir).join(&entry).exists())
    {
        return Ok(());
    }
    let Some(base) = directories::BaseDirs::new() else {
        return Ok(());
    };
    let data = base.data_dir();
    let exe = std::env::current_exe()?;
    let desktop = format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=Picacho\n\
         GenericName=Email Client\n\
         Comment=Read and send email\n\
         Exec={}\n\
         Icon={APP_ID}\n\
         StartupWMClass={APP_ID}\n\
         Terminal=false\n\
         Categories=Network;Email;\n\
         Keywords=mail;email;inbox;imap;\n",
        exec_quote(&exe.to_string_lossy()),
    );
    write_if_changed(
        &data.join(format!("icons/hicolor/256x256/apps/{APP_ID}.png")),
        ICON,
    )?;
    write_if_changed(&data.join(entry), desktop.as_bytes())
}

/// Writes `bytes` to `path` unless it already holds them, so launchers do
/// not reload on every start.
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
fn write_if_changed(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    if std::fs::read(path).is_ok_and(|old| old == bytes) {
        return Ok(());
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    // Write beside it and rename, so a launcher never reads half a file.
    let partial = path.with_extension("partial");
    std::fs::write(&partial, bytes)?;
    std::fs::rename(&partial, path)
}

/// Quotes a path for a desktop entry's Exec key.
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
fn exec_quote(path: &str) -> String {
    let mut out = String::from("\"");
    for ch in path.chars() {
        match ch {
            // Escaped for the Exec quoting, then the backslash itself is
            // escaped for the desktop entry's string value.
            '"' | '`' | '$' | '\\' => {
                out.push_str("\\\\");
                out.push(ch);
            }
            '%' => out.push_str("%%"),
            _ => out.push(ch),
        }
    }
    out.push('"');
    out
}

#[cfg(all(test, any(target_os = "linux", target_os = "freebsd")))]
mod tests {
    #[test]
    fn exec_quote_escapes_reserved_characters() {
        assert_eq!(
            super::exec_quote("/opt/Pi cacho/picacho"),
            "\"/opt/Pi cacho/picacho\""
        );
        assert_eq!(super::exec_quote("/a$b%c"), "\"/a\\\\$b%%c\"");
    }
}

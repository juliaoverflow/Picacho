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

/// Ensures the desktop entry is written synchronously before asking `xdg-mime`
/// or `xdg-settings` to make Picacho the default email app.
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
pub fn register_sync() {
    let _ = install();
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
fn install() -> std::io::Result<()> {
    use std::path::PathBuf;
    const ICON: &[u8] = include_bytes!("../../assets/brand/icon.png");
    if std::env::var_os("FLATPAK_ID").is_some() || std::env::var_os("SNAP").is_some() {
        return Ok(());
    }
    let Some(base) = directories::BaseDirs::new() else {
        return Ok(());
    };
    let data = base.data_dir();
    let _ = install_plasmoids(data);

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
    let exe = std::env::current_exe()?;
    // A line break in the path would end the Exec key and start keys of
    // its own; such a path gets no entry.
    if exe.to_string_lossy().contains(['\n', '\r']) {
        return Ok(());
    }
    let desktop = format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=Picacho\n\
         {}\
         Exec={} %U\n\
         Icon={APP_ID}\n\
         StartupNotify=true\n\
         StartupWMClass={APP_ID}\n\
         Terminal=false\n\
         Categories=Network;Email;\n\
         MimeType=x-scheme-handler/mailto;x-scheme-handler/picacho;message/rfc822;application/vnd.ms-outlook;\n",
        localized_keys(),
        exec_quote(&exe.to_string_lossy()),
    );
    write_if_changed(
        &data.join(format!("icons/hicolor/256x256/apps/{APP_ID}.png")),
        ICON,
    )?;
    write_if_changed(&data.join(entry), desktop.as_bytes())
}

/// The desktop entry's GenericName, Comment and Keywords lines: in English,
/// then in each language Picacho has that words them otherwise (`Key[ll_CC]`),
/// for app menus to show in their reader's language.
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
fn localized_keys() -> String {
    use std::fmt::Write;
    let words = || {
        [
            ("GenericName", t!("desktop-generic-name")),
            ("Comment", t!("desktop-comment")),
            ("Keywords", t!("desktop-keywords")),
        ]
    };
    // A line break would end the key and start keys of its own.
    let one_line = |text: &str| text.replace(['\n', '\r'], " ");
    let english = crate::i18n::in_source(words);
    let mut lines = String::new();
    for (key, text) in english {
        let _ = writeln!(lines, "{key}={}", one_line(text));
    }
    for &(code, _) in crate::i18n::languages() {
        // Desktop entries name locales as POSIX does: pt_BR, not pt-BR.
        let locale = code.replace('-', "_");
        for ((key, text), (_, source)) in crate::i18n::in_language(code, words)
            .into_iter()
            .zip(english)
        {
            if text != source {
                let _ = writeln!(lines, "{key}[{locale}]={}", one_line(text));
            }
        }
    }
    lines
}

/// Installs Picacho's KDE Plasma 6 desktop & panel widgets into
/// `$XDG_DATA_HOME/plasma/plasmoids/` so Plasma users can add them directly
/// from "Add Widgets…". Their words come with the snapshot they show
/// (`widgets::labels`), in the language Picacho shows; their names and
/// descriptions get each language's in `metadata.json` (`named`).
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
fn install_plasmoids(data: &std::path::Path) -> std::io::Result<()> {
    let files: &[(&str, &[u8])] = &[
        (
            "plasma/plasmoids/tech.pinal.picacho.inbox/metadata.json",
            include_bytes!("../../packaging/linux/plasmoids/tech.pinal.picacho.inbox/metadata.json"),
        ),
        (
            "plasma/plasmoids/tech.pinal.picacho.inbox/contents/config/main.xml",
            include_bytes!("../../packaging/linux/plasmoids/tech.pinal.picacho.inbox/contents/config/main.xml"),
        ),
        (
            "plasma/plasmoids/tech.pinal.picacho.inbox/contents/config/config.qml",
            include_bytes!("../../packaging/linux/plasmoids/tech.pinal.picacho.inbox/contents/config/config.qml"),
        ),
        (
            "plasma/plasmoids/tech.pinal.picacho.inbox/contents/ui/configGeneral.qml",
            include_bytes!("../../packaging/linux/plasmoids/tech.pinal.picacho.inbox/contents/ui/configGeneral.qml"),
        ),
        (
            "plasma/plasmoids/tech.pinal.picacho.inbox/contents/ui/main.qml",
            include_bytes!("../../packaging/linux/plasmoids/tech.pinal.picacho.inbox/contents/ui/main.qml"),
        ),
        (
            "plasma/plasmoids/tech.pinal.picacho.calendar/metadata.json",
            include_bytes!("../../packaging/linux/plasmoids/tech.pinal.picacho.calendar/metadata.json"),
        ),
        (
            "plasma/plasmoids/tech.pinal.picacho.calendar/contents/ui/main.qml",
            include_bytes!("../../packaging/linux/plasmoids/tech.pinal.picacho.calendar/contents/ui/main.qml"),
        ),
        (
            "plasma/plasmoids/tech.pinal.picacho.packages/metadata.json",
            include_bytes!("../../packaging/linux/plasmoids/tech.pinal.picacho.packages/metadata.json"),
        ),
        (
            "plasma/plasmoids/tech.pinal.picacho.packages/contents/config/main.xml",
            include_bytes!("../../packaging/linux/plasmoids/tech.pinal.picacho.packages/contents/config/main.xml"),
        ),
        (
            "plasma/plasmoids/tech.pinal.picacho.packages/contents/config/config.qml",
            include_bytes!("../../packaging/linux/plasmoids/tech.pinal.picacho.packages/contents/config/config.qml"),
        ),
        (
            "plasma/plasmoids/tech.pinal.picacho.packages/contents/ui/configGeneral.qml",
            include_bytes!("../../packaging/linux/plasmoids/tech.pinal.picacho.packages/contents/ui/configGeneral.qml"),
        ),
        (
            "plasma/plasmoids/tech.pinal.picacho.packages/contents/ui/main.qml",
            include_bytes!("../../packaging/linux/plasmoids/tech.pinal.picacho.packages/contents/ui/main.qml"),
        ),
        (
            "plasma/plasmoids/tech.pinal.picacho.receipts/metadata.json",
            include_bytes!("../../packaging/linux/plasmoids/tech.pinal.picacho.receipts/metadata.json"),
        ),
        (
            "plasma/plasmoids/tech.pinal.picacho.receipts/contents/config/main.xml",
            include_bytes!("../../packaging/linux/plasmoids/tech.pinal.picacho.receipts/contents/config/main.xml"),
        ),
        (
            "plasma/plasmoids/tech.pinal.picacho.receipts/contents/config/config.qml",
            include_bytes!("../../packaging/linux/plasmoids/tech.pinal.picacho.receipts/contents/config/config.qml"),
        ),
        (
            "plasma/plasmoids/tech.pinal.picacho.receipts/contents/ui/configGeneral.qml",
            include_bytes!("../../packaging/linux/plasmoids/tech.pinal.picacho.receipts/contents/ui/configGeneral.qml"),
        ),
        (
            "plasma/plasmoids/tech.pinal.picacho.receipts/contents/ui/main.qml",
            include_bytes!("../../packaging/linux/plasmoids/tech.pinal.picacho.receipts/contents/ui/main.qml"),
        ),
    ];
    for (rel, bytes) in files {
        if rel.ends_with("/metadata.json") {
            write_if_changed(&data.join(rel), &named(bytes))?;
        } else {
            write_if_changed(&data.join(rel), bytes)?;
        }
    }
    Ok(())
}

/// A widget's metadata.json with its name and description in each language
/// Picacho has (`Name[de]`), for Plasma's widget list.
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
fn named(metadata: &[u8]) -> Vec<u8> {
    let Ok(mut json) = serde_json::from_slice::<serde_json::Value>(metadata) else {
        return metadata.to_vec();
    };
    let id = json["KPlugin"]["Id"].as_str().unwrap_or_default().to_owned();
    let Some(words) = plasmoid_words(&id) else {
        return metadata.to_vec();
    };
    let english = crate::i18n::in_source(words);
    let plugin = &mut json["KPlugin"];
    for &(code, _) in crate::i18n::languages() {
        let locale = code.replace('-', "_");
        let (name, description) = crate::i18n::in_language(code, words);
        if name != english.0 {
            plugin[format!("Name[{locale}]")] = name.into();
        }
        if description != english.1 {
            plugin[format!("Description[{locale}]")] = description.into();
        }
    }
    serde_json::to_vec_pretty(&json).unwrap_or_else(|_| metadata.to_vec())
}

/// A widget's name and description, by its id.
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
fn plasmoid_words(id: &str) -> Option<fn() -> (&'static str, &'static str)> {
    Some(match id {
        "tech.pinal.picacho.inbox" => || (t!("widgets-plasmoid-inbox"), t!("widgets-plasmoid-inbox-about")),
        "tech.pinal.picacho.calendar" => || (t!("widgets-plasmoid-calendar"), t!("widgets-plasmoid-calendar-about")),
        "tech.pinal.picacho.packages" => || (t!("widgets-plasmoid-packages"), t!("widgets-plasmoid-packages-about")),
        "tech.pinal.picacho.receipts" => || (t!("widgets-plasmoid-receipts"), t!("widgets-plasmoid-receipts-about")),
        _ => return None,
    })
}

/// Writes `bytes` to `path` unless it already holds them, so launchers do
/// not reload on every start.
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
pub(super) fn write_if_changed(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
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
pub(super) fn exec_quote(path: &str) -> String {
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

    #[test]
    fn plasmoids_are_named_as_their_metadata_names_them() {
        for widget in ["inbox", "calendar", "packages", "receipts"] {
            let path = format!("packaging/linux/plasmoids/tech.pinal.picacho.{widget}/metadata.json");
            let file = std::fs::read(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(&path)).unwrap();
            let json: serde_json::Value = serde_json::from_slice(&super::named(&file)).unwrap();
            let id = json["KPlugin"]["Id"].as_str().unwrap();
            let (name, about) = crate::i18n::in_source(super::plasmoid_words(id).unwrap());
            assert_eq!(json["KPlugin"]["Name"], name, "{path}");
            assert_eq!(json["KPlugin"]["Description"], about, "{path}");
        }
    }

    #[test]
    fn localized_keys_are_one_line_each() {
        let keys = super::localized_keys();
        assert!(
            keys.starts_with("GenericName=Email Client\nComment=Fast, native email for every inbox you have\n"),
            "{keys}"
        );
        assert!(
            keys.contains("\nKeywords=mail;email;inbox;imap;\n"),
            "{keys}"
        );
        assert!(
            keys.lines()
                .all(|line| line.split_once('=').is_some_and(|(key, _)| !key.is_empty()))
        );
    }
}

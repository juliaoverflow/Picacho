//! Open With… for attachments: the apps that open a file, to choose one.
//!
//! - Windows shows its own Open With dialog.
//! - macOS lists the apps Launch Services has for the file (the default
//!   first), and opens it in the one chosen.
//! - Linux asks the desktop's portal (xdg-desktop-portal's OpenURI with
//!   `ask`), whose dialog lists the apps, also inside Flatpak; without a
//!   portal, the apps whose desktop entries take the file's type are
//!   listed (the default first, from `mimeapps.list`).
use std::path::{Path, PathBuf};

/// An app that opens a file: its name, and how to start it (an app
/// bundle's path on macOS, a desktop entry's file on Linux).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct OpenWithApp {
    pub name: String,
    pub path: PathBuf,
}

/// The apps that open `file` (of type `mime`), the default first.
#[allow(unused_variables)]
pub(crate) fn apps_for(file: &Path, mime: &str) -> Vec<OpenWithApp> {
    #[cfg(target_os = "macos")]
    {
        mac::apps_for(file)
    }
    #[cfg(all(unix, not(any(target_os = "macos", target_os = "android", target_os = "ios"))))]
    {
        desktop::apps_for(mime)
    }
    #[cfg(not(any(
        target_os = "macos",
        all(unix, not(any(target_os = "macos", target_os = "android", target_os = "ios")))
    )))]
    {
        Vec::new()
    }
}

/// Opens `file` in `app`.
#[allow(unused_variables)]
pub(crate) fn open_in(app: &OpenWithApp, file: &Path) -> anyhow::Result<()> {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("/usr/bin/open")
            .arg("-a")
            .arg(&app.path)
            .arg(file)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()?;
        Ok(())
    }
    #[cfg(all(unix, not(any(target_os = "macos", target_os = "android", target_os = "ios"))))]
    {
        desktop::launch(&app.path, file)
    }
    #[cfg(not(any(
        target_os = "macos",
        all(unix, not(any(target_os = "macos", target_os = "android", target_os = "ios")))
    )))]
    {
        anyhow::bail!("Open With isn't available here.")
    }
}

/// Linux: the desktop portal's own Open With dialog for `file`. An error
/// when there is no portal to ask.
#[cfg(all(unix, not(any(target_os = "macos", target_os = "android", target_os = "ios"))))]
pub(crate) async fn portal_open_with(file: PathBuf) -> anyhow::Result<()> {
    let handle = std::fs::File::open(&file)?;
    let request = ashpd::desktop::open_uri::OpenFileRequest::default()
        .ask(true)
        .send_file(&handle)
        .await?;
    // Cancelled in the dialog is no error.
    match request.response() {
        Ok(()) | Err(ashpd::Error::Response(ashpd::desktop::ResponseError::Cancelled)) => Ok(()),
        Err(e) => Err(e.into()),
    }
}

#[cfg(target_os = "macos")]
mod mac {
    use super::OpenWithApp;
    use objc2::rc::autoreleasepool;
    use objc2_app_kit::NSWorkspace;
    use objc2_foundation::{NSString, NSURL};
    use std::path::{Path, PathBuf};

    pub(super) fn apps_for(file: &Path) -> Vec<OpenWithApp> {
        autoreleasepool(|_| {
            let url = NSURL::fileURLWithPath(&NSString::from_str(&file.to_string_lossy()));
            let workspace = NSWorkspace::sharedWorkspace();
            let default = workspace
                .URLForApplicationToOpenURL(&url)
                .and_then(|u| u.path())
                .map(|p| PathBuf::from(p.to_string()));
            let mut apps: Vec<OpenWithApp> = workspace
                .URLsForApplicationsToOpenURL(&url)
                .iter()
                .filter_map(|u| u.path())
                .map(|p| PathBuf::from(p.to_string()))
                .map(|path| OpenWithApp {
                    name: path
                        .file_stem()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    path,
                })
                .collect();
            super::order(&mut apps, default.as_deref());
            apps
        })
    }
}

/// The default first, then by name, each name once.
#[cfg_attr(any(target_os = "android", target_os = "ios", windows), allow(dead_code))]
fn order(apps: &mut Vec<OpenWithApp>, default: Option<&Path>) {
    apps.retain(|a| !a.name.trim().is_empty());
    apps.sort_by(|a, b| {
        let first = |x: &OpenWithApp| Some(x.path.as_path()) != default;
        (first(a), a.name.to_lowercase()).cmp(&(first(b), b.name.to_lowercase()))
    });
    let mut seen = std::collections::HashSet::new();
    apps.retain(|a| seen.insert(a.name.to_lowercase()));
}

/// Desktop entries (the freedesktop.org Desktop Entry and MIME
/// Applications specifications).
#[cfg_attr(not(all(unix, not(any(target_os = "macos", target_os = "android", target_os = "ios")))), allow(dead_code))]
mod desktop {
    use super::OpenWithApp;
    use std::path::{Path, PathBuf};

    /// The folders desktop entries are in, the most important first.
    fn data_dirs() -> Vec<PathBuf> {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let data_home = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| home.as_ref().map(|h| h.join(".local/share")));
        let dirs = std::env::var("XDG_DATA_DIRS")
            .ok()
            .filter(|d| !d.trim().is_empty())
            .unwrap_or_else(|| "/usr/local/share:/usr/share".into());
        data_home
            .into_iter()
            .chain(dirs.split(':').map(PathBuf::from).filter(|p| p.is_absolute()))
            .collect()
    }

    /// The file of desktop entry `id` (`org.gnome.TextEditor.desktop`).
    fn entry_file(dirs: &[PathBuf], id: &str) -> Option<PathBuf> {
        dirs.iter().map(|d| d.join("applications").join(id)).find(|p| p.is_file())
    }

    /// One entry's keys of its `[Desktop Entry]` group.
    #[derive(Debug, Default, PartialEq)]
    pub(super) struct Entry {
        pub name: String,
        pub exec: String,
        pub mime: Vec<String>,
        pub shown: bool,
    }

    pub(super) fn parse(text: &str) -> Option<Entry> {
        let mut entry = Entry {
            shown: true,
            ..Entry::default()
        };
        let mut in_group = false;
        let mut application = false;
        let mut localized_name = None;
        let lang = std::env::var("LANG").unwrap_or_default();
        let lang = lang.split(['.', '@']).next().unwrap_or("").to_owned();
        let short = lang.split('_').next().unwrap_or("").to_owned();
        for line in text.lines() {
            let line = line.trim();
            if line.starts_with('[') {
                in_group = line == "[Desktop Entry]";
                continue;
            }
            if !in_group || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else { continue };
            let (key, value) = (key.trim(), value.trim());
            match key {
                "Type" => application = value == "Application",
                "Name" => entry.name = value.to_owned(),
                "Exec" => entry.exec = value.to_owned(),
                "MimeType" => {
                    entry.mime = value
                        .split(';')
                        .map(|m| m.trim().to_ascii_lowercase())
                        .filter(|m| !m.is_empty())
                        .collect()
                }
                "NoDisplay" | "Hidden" if value == "true" => entry.shown = false,
                _ if !lang.is_empty() && (key == format!("Name[{lang}]")) => localized_name = Some(value.to_owned()),
                _ if !short.is_empty() && key == format!("Name[{short}]") && localized_name.is_none() => {
                    localized_name = Some(value.to_owned())
                }
                _ => {}
            }
        }
        if let Some(name) = localized_name {
            entry.name = name;
        }
        (application && !entry.exec.is_empty()).then_some(entry)
    }

    /// The default app for `mime` in `mimeapps.list`, by entry id.
    fn default_for(mime: &str) -> Option<String> {
        let config = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
        let lists = [config.join("mimeapps.list")]
            .into_iter()
            .chain(data_dirs().into_iter().map(|d| d.join("applications/mimeapps.list")));
        for list in lists {
            let Ok(text) = std::fs::read_to_string(list) else { continue };
            let mut in_defaults = false;
            for line in text.lines() {
                let line = line.trim();
                if line.starts_with('[') {
                    in_defaults = line == "[Default Applications]";
                    continue;
                }
                if in_defaults
                    && let Some((key, value)) = line.split_once('=')
                    && key.trim().eq_ignore_ascii_case(mime)
                    && let Some(first) = value.split(';').map(str::trim).find(|v| !v.is_empty())
                {
                    return Some(first.to_owned());
                }
            }
        }
        None
    }

    pub(super) fn apps_for(mime: &str) -> Vec<OpenWithApp> {
        let mime = mime.to_ascii_lowercase();
        let wildcard = format!("{}/*", mime.split('/').next().unwrap_or(""));
        let dirs = data_dirs();
        let mut seen = std::collections::HashSet::new();
        let mut apps = Vec::new();
        for dir in &dirs {
            let Ok(files) = std::fs::read_dir(dir.join("applications")) else { continue };
            for file in files.flatten() {
                let path = file.path();
                let id = file.file_name().to_string_lossy().into_owned();
                // An entry in an earlier folder hides one of its id after.
                if !id.ends_with(".desktop") || !seen.insert(id) {
                    continue;
                }
                let Some(entry) = std::fs::read_to_string(&path).ok().and_then(|t| parse(&t)) else {
                    continue;
                };
                if entry.shown && entry.mime.iter().any(|m| *m == mime || *m == wildcard) {
                    apps.push(OpenWithApp { name: entry.name, path });
                }
            }
        }
        let default = default_for(&mime).and_then(|id| entry_file(&dirs, &id));
        super::order(&mut apps, default.as_deref());
        apps
    }

    /// The arguments of an `Exec` value (quoted with double quotes, `\`
    /// escaping `"`, `` ` ``, `$` and `\` inside them), with the file in
    /// place of its field code. Without one, the file goes last.
    pub(super) fn command(exec: &str, name: &str, file: &Path) -> Vec<String> {
        let mut words = Vec::new();
        let mut word = String::new();
        let mut quoted = false;
        let mut started = false;
        let mut chars = exec.chars();
        while let Some(c) = chars.next() {
            match c {
                '"' => {
                    quoted = !quoted;
                    started = true;
                }
                '\\' if quoted => {
                    if let Some(next) = chars.next() {
                        word.push(next);
                    }
                }
                ' ' | '\t' if !quoted => {
                    if started || !word.is_empty() {
                        words.push(std::mem::take(&mut word));
                    }
                    started = false;
                }
                _ => word.push(c),
            }
        }
        if started || !word.is_empty() {
            words.push(word);
        }
        let path = file.to_string_lossy().into_owned();
        let url = url::Url::from_file_path(file).map(|u| u.to_string()).unwrap_or_else(|_| path.clone());
        let mut placed = false;
        let mut out = Vec::new();
        for w in words {
            match w.as_str() {
                "%f" | "%F" => {
                    out.push(path.clone());
                    placed = true;
                }
                "%u" | "%U" => {
                    out.push(url.clone());
                    placed = true;
                }
                "%i" | "%k" => {}
                "%c" => out.push(name.to_owned()),
                _ => {
                    let mut s = String::new();
                    let mut it = w.chars().peekable();
                    while let Some(c) = it.next() {
                        if c == '%' {
                            match it.next() {
                                Some('%') => s.push('%'),
                                Some('f' | 'F') => {
                                    s.push_str(&path);
                                    placed = true;
                                }
                                Some('u' | 'U') => {
                                    s.push_str(&url);
                                    placed = true;
                                }
                                Some('c') => s.push_str(name),
                                _ => {}
                            }
                        } else {
                            s.push(c);
                        }
                    }
                    out.push(s);
                }
            }
        }
        if !placed {
            out.push(path);
        }
        out
    }

    pub(super) fn launch(entry: &Path, file: &Path) -> anyhow::Result<()> {
        let text = std::fs::read_to_string(entry)?;
        let parsed = parse(&text).ok_or_else(|| anyhow::anyhow!("That app can't be started."))?;
        let args = command(&parsed.exec, &parsed.name, file);
        let (program, rest) = args.split_first().ok_or_else(|| anyhow::anyhow!("That app can't be started."))?;
        std::process::Command::new(program)
            .args(rest)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()?;
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn desktop_entries_give_their_command_with_the_file() {
            let e = parse(
                "[Desktop Entry]\nType=Application\nName=Text Editor\nExec=\"/opt/My Apps/edit\" --new-window %U\nMimeType=text/plain;text/markdown;\n\n[Desktop Action new]\nName=New\nExec=edit --other\n",
            )
            .unwrap();
            assert_eq!(e.name, "Text Editor");
            assert_eq!(e.mime, ["text/plain", "text/markdown"]);
            let file = Path::new("/tmp/a b.txt");
            assert_eq!(
                command(&e.exec, &e.name, file),
                ["/opt/My Apps/edit", "--new-window", "file:///tmp/a%20b.txt"]
            );
            assert_eq!(command("viewer --title=%c 100%%", "View", file), ["viewer", "--title=View", "100%", "/tmp/a b.txt"]);
            assert_eq!(command("gimp-2.10 %f %i", "GIMP", file), ["gimp-2.10", "/tmp/a b.txt"]);
            assert!(parse("[Desktop Entry]\nType=Link\nName=x\nExec=y\n").is_none());
            assert!(!parse("[Desktop Entry]\nType=Application\nName=x\nExec=y\nNoDisplay=true\n").unwrap().shown);
        }
    }
}

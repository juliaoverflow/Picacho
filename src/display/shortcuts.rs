//! Keyboard shortcuts the person can change: a text file in Picacho's
//! settings folder, a line per action (`reply = secondary-shift-r`), read
//! when Picacho starts. `secondary` is ⌘ on macOS and Ctrl elsewhere;
//! several shortcuts for one action are separated by commas, and `none`
//! takes an action's shortcut away. Lines that don't parse keep the
//! default.
use super::*;

const FILE: &str = "shortcuts.txt";

/// Where the shortcuts file is.
pub(super) fn file_path() -> std::path::PathBuf {
    directories::ProjectDirs::from("tech", "pinal", "picacho")
        .map(|p| p.config_dir().join(FILE))
        .unwrap_or_else(|| std::env::temp_dir().join(FILE))
}

/// The actions shortcuts can be given to: (name, default keys, action,
/// key context).
pub(super) fn bindable() -> Vec<(&'static str, String, Box<dyn gpui::Action>, &'static str)> {
    let b = |name: &'static str, keys: &str, action: Box<dyn gpui::Action>| {
        (name, keys.to_owned(), action, "Mail")
    };
    vec![
        b("quit", "secondary-q", Box::new(Quit)),
        b("close-window", "secondary-w", Box::new(CloseWindow)),
        b("settings", "secondary-,", Box::new(ShowAccounts)),
        b("new-message", "secondary-n", Box::new(Compose)),
        b("find-in-message", "secondary-f", Box::new(FindInMessage)),
        b("search-mailbox", "secondary-alt-f", Box::new(Search)),
        b("zoom-in", "secondary-=, secondary-+", Box::new(ZoomIn)),
        b("zoom-out", "secondary--", Box::new(ZoomOut)),
        b("actual-size", "secondary-0", Box::new(ActualSize)),
        b("refresh", "secondary-r", Box::new(Refresh)),
        b("close", "escape", Box::new(Close)),
        b("next-message", "alt-down", Box::new(Next)),
        b("previous-message", "alt-up", Box::new(Previous)),
        b("archive", "secondary-shift-a", Box::new(Archive)),
        b("delete", "secondary-backspace", Box::new(DeleteMail)),
        b("reply", "secondary-shift-r", Box::new(Reply)),
        b("reply-all", "secondary-alt-r", Box::new(ReplyAll)),
        b("forward", "secondary-shift-f", Box::new(ForwardMail)),
        b("toggle-read", "secondary-shift-u", Box::new(ToggleRead)),
        b("move-to-spam", "secondary-shift-j", Box::new(MoveToSpam)),
        b("toggle-star", "secondary-shift-s", Box::new(ToggleStar)),
        b("toggle-dark-mode", "secondary-shift-l", Box::new(ToggleTheme)),
        b("send", "secondary-enter", Box::new(SendMail)),
        b(
            "toggle-sidebar",
            if cfg!(target_os = "macos") { "ctrl-cmd-s" } else { "ctrl-alt-s" },
            Box::new(ToggleSidebar),
        ),
        b("select-all", "secondary-a", Box::new(SelectAllMail)),
        b("undo", "secondary-z", Box::new(gpui_component::input::Undo)),
        b("go-inbox", "secondary-1", Box::new(GoInbox)),
        b("go-starred", "secondary-2", Box::new(GoStarred)),
        b("go-sent", "secondary-3", Box::new(GoSent)),
        b("go-drafts", "secondary-4", Box::new(GoDrafts)),
        b("go-archive", "secondary-5", Box::new(GoArchive)),
        b("go-spam", "secondary-6", Box::new(GoSpam)),
        b("go-trash", "secondary-7", Box::new(GoTrash)),
        b("go-all-mail", "secondary-8", Box::new(GoAllMail)),
        b("minimize", "secondary-m", Box::new(Minimize)),
        b("open-in-window", "secondary-o", Box::new(OpenInWindow)),
        b("print", "secondary-p", Box::new(PrintMessage)),
        b("group-by-conversation", "secondary-shift-t", Box::new(ToggleThreads)),
    ]
}

/// The file's changes: action name → keys.
fn parse(text: &str) -> HashMap<String, String> {
    text.lines()
        .map(|l| l.split('#').next().unwrap_or("").trim())
        .filter_map(|l| l.split_once('='))
        .map(|(name, keys)| (name.trim().to_lowercase(), keys.trim().to_lowercase()))
        .filter(|(name, _)| !name.is_empty())
        .collect()
}

/// The shortcuts as the file changes them.
pub(super) fn overrides() -> HashMap<String, String> {
    std::fs::read_to_string(file_path())
        .map(|t| parse(&t))
        .unwrap_or_default()
}

/// The file, written with every action and its default (commented out)
/// if there is none yet.
pub(super) fn ensure_file() -> std::io::Result<std::path::PathBuf> {
    let path = file_path();
    if !path.exists() {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut text = String::from(
            "# Picacho's keyboard shortcuts. Take the # off a line and change its keys;\n\
             # Picacho reads this file when it starts.\n\
             # `secondary` is Cmd on macOS and Ctrl elsewhere; other modifiers are ctrl,\n\
             # alt, shift and cmd. Separate several shortcuts with commas; `none` takes\n\
             # an action's shortcut away.\n\n",
        );
        for (name, keys, _, _) in bindable() {
            text.push_str(&format!("# {name} = {keys}\n"));
        }
        std::fs::write(&path, text)?;
    }
    Ok(path)
}

/// The key bindings of the actions above, as the file changes them.
pub(super) fn bindings(cx: &App) -> Vec<KeyBinding> {
    let custom = overrides();
    let mut out = Vec::new();
    for (name, default, action, context) in bindable() {
        let load = |keys: &str| -> Option<Vec<KeyBinding>> {
            keys.split(',')
                .map(str::trim)
                .filter(|k| !k.is_empty())
                .map(|k| {
                    let context = Some(std::rc::Rc::new(
                        KeyBindingContextPredicate::parse(context).ok()?,
                    ));
                    KeyBinding::load(k, action.boxed_clone(), context, false, None, cx).ok()
                })
                .collect()
        };
        match custom.get(name).map(String::as_str) {
            Some("none") => {}
            Some(keys) => match load(keys) {
                Some(b) => out.extend(b),
                None => {
                    eprintln!("Picacho: the shortcut “{keys}” for {name} isn't understood; keeping {default}");
                    out.extend(load(&default).unwrap_or_default());
                }
            },
            None => out.extend(load(&default).unwrap_or_default()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::prelude::v1::test;

    #[test]
    fn lines_name_actions_and_keys() {
        let m = parse("# comment\nreply = Ctrl-R # mine\n  archive=none\nbad line\n");
        assert_eq!(m.get("reply").map(String::as_str), Some("ctrl-r"));
        assert_eq!(m.get("archive").map(String::as_str), Some("none"));
        assert_eq!(m.len(), 2);
    }
}

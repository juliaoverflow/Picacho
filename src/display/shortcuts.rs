//! Keyboard shortcuts the person can change: a text file in Picacho's
//! settings folder, a line per action (`reply = secondary-shift-r`), read
//! when Picacho starts and reloaded automatically whenever the file changes.
//! `secondary` is ⌘ on macOS and Ctrl elsewhere; several shortcuts for one
//! action are separated by commas, and `none` takes an action's shortcut away.
//! Lines that don't parse keep the default.
use super::*;

const FILE: &str = "shortcuts.txt";

/// Key bindings registered by GPUI components before Picacho adds its own,
/// kept so reloading `shortcuts.txt` clears and rebuilds Picacho's bindings
/// without losing text-input or component bindings.
struct BaseBindings(Vec<KeyBinding>);
impl Global for BaseBindings {}

/// Snapshots the keymap right after `gpui_component::init` and `prompt::install`
/// so [`reload`] can rebuild Picacho's bindings cleanly.
pub(super) fn save_base(cx: &mut App) {
    let base: Vec<KeyBinding> = cx.key_bindings().borrow().bindings().cloned().collect();
    cx.set_global(BaseBindings(base));
}

/// Where the shortcuts file is.
pub(super) fn file_path() -> std::path::PathBuf {
    // Tests keep away from the person's own shortcuts.
    #[cfg(test)]
    return std::env::temp_dir()
        .join(format!("picacho-test-shortcuts-{}", std::process::id()))
        .join(FILE);
    #[allow(unreachable_code)]
    directories::ProjectDirs::from("tech", "pinal", "picacho")
        .map(|p| p.config_dir().join(FILE))
        .unwrap_or_else(|| std::env::temp_dir().join(FILE))
}

/// Which part of Settings → Keyboard Shortcuts an action is listed in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Group {
    Messages,
    Mailbox,
    Writing,
    App,
}
impl Group {
    pub(super) const ALL: [Self; 4] = [Self::Messages, Self::Mailbox, Self::Writing, Self::App];
    pub(super) fn title(self) -> &'static str {
        match self {
            Self::Messages => "Messages",
            Self::Mailbox => "Mailbox",
            Self::Writing => "Writing",
            Self::App => "Window & App",
        }
    }
}

/// An action a shortcut can be given to.
pub(super) struct Bindable {
    /// Its name in `shortcuts.txt`.
    pub(super) name: &'static str,
    /// Its name in Settings.
    pub(super) label: &'static str,
    pub(super) group: Group,
    /// Its keys unless the file changes them.
    pub(super) default: String,
    /// What the keys do, and where (key context): an action may be one
    /// command in the main window and another in the composer.
    pub(super) targets: Vec<(Box<dyn gpui::Action>, &'static str)>,
}

/// Every action shortcuts can be given to, in the order Settings lists them.
pub(super) fn bindable() -> Vec<Bindable> {
    use Group::*;
    use composer::{
        Bold, Bullets, Heading, InlineCode, InsertLink, Italic, Numbers, QuoteLines, SaveDraft,
        SendDraft, Strike, TogglePreview,
    };
    fn b(
        group: Group,
        name: &'static str,
        label: &'static str,
        keys: &str,
        action: impl gpui::Action,
    ) -> Bindable {
        entry(group, name, label, keys, vec![(Box::new(action), "Mail")])
    }
    fn composing(
        name: &'static str,
        label: &'static str,
        keys: &str,
        action: impl gpui::Action,
    ) -> Bindable {
        entry(Writing, name, label, keys, vec![(Box::new(action), "Composer")])
    }
    fn entry(
        group: Group,
        name: &'static str,
        label: &'static str,
        keys: &str,
        targets: Vec<(Box<dyn gpui::Action>, &'static str)>,
    ) -> Bindable {
        Bindable { name, label, group, default: keys.to_owned(), targets }
    }
    // Send in the main window (its menu) and in a composer is one shortcut.
    let send = entry(
        Writing,
        "send",
        "Send",
        "secondary-enter",
        vec![(Box::new(SendMail), "Mail"), (Box::new(SendDraft), "Composer")],
    );
    vec![
        b(Messages, "reply", "Reply", "secondary-shift-r", Reply),
        b(Messages, "reply-all", "Reply All", "secondary-alt-r", ReplyAll),
        b(Messages, "forward", "Forward", "secondary-shift-f", ForwardMail),
        b(Messages, "archive", "Archive", "secondary-shift-a", Archive),
        b(Messages, "delete", "Move to Trash", "secondary-backspace", DeleteMail),
        b(Messages, "move-to-spam", "Move to Spam", "secondary-shift-j", MoveToSpam),
        b(Messages, "toggle-read", "Mark as Read / Unread", "secondary-shift-u", ToggleRead),
        b(Messages, "toggle-star", "Star / Unstar", "secondary-shift-s", ToggleStar),
        b(Messages, "next-message", "Next Message", "alt-down", Next),
        b(Messages, "previous-message", "Previous Message", "alt-up", Previous),
        b(Messages, "open-in-window", "Open in New Window", "secondary-o", OpenInWindow),
        b(Messages, "find-in-message", "Find in Message", "secondary-f", FindInMessage),
        b(Messages, "print", "Print", "secondary-p", PrintMessage),
        b(Mailbox, "refresh", "Refresh", "secondary-r", Refresh),
        b(Mailbox, "search-mailbox", "Search Mailbox", "secondary-alt-f", Search),
        b(Mailbox, "select-all", "Select All Messages", "secondary-a", SelectAllMail),
        b(Mailbox, "group-by-conversation", "Group by Conversation", "secondary-shift-t", ToggleThreads),
        b(Mailbox, "go-inbox", "Go to Inbox", "secondary-1", GoInbox),
        b(Mailbox, "go-starred", "Go to Starred", "secondary-2", GoStarred),
        b(Mailbox, "go-sent", "Go to Sent", "secondary-3", GoSent),
        b(Mailbox, "go-drafts", "Go to Drafts", "secondary-4", GoDrafts),
        b(Mailbox, "go-archive", "Go to Archive", "secondary-5", GoArchive),
        b(Mailbox, "go-spam", "Go to Spam", "secondary-6", GoSpam),
        b(Mailbox, "go-trash", "Go to Trash", "secondary-7", GoTrash),
        b(Mailbox, "go-all-mail", "Go to All Mail", "secondary-8", GoAllMail),
        b(Writing, "new-message", "New Message", "secondary-n", Compose),
        send,
        composing("save-draft", "Save Draft", "secondary-s", SaveDraft),
        composing("bold", "Bold", "secondary-b", Bold),
        composing("italic", "Italic", "secondary-i", Italic),
        composing("strikethrough", "Strikethrough", "secondary-shift-x", Strike),
        composing("code", "Code", "secondary-e", InlineCode),
        composing("link", "Insert Link", "secondary-k", InsertLink),
        composing("bulleted-list", "Bulleted List", "secondary-shift-8", Bullets),
        composing("numbered-list", "Numbered List", "secondary-shift-7", Numbers),
        composing("quote", "Quote", "secondary-shift-9", QuoteLines),
        composing("heading", "Heading", "secondary-alt-1", Heading),
        composing("preview-formatting", "Preview Formatting", "secondary-shift-p", TogglePreview),
        b(Writing, "undo", "Undo", "secondary-z", gpui_component::input::Undo),
        b(App, "settings", "Settings", "secondary-,", ShowAccounts),
        b(
            App,
            "toggle-sidebar",
            "Show / Hide Sidebar",
            if cfg!(target_os = "macos") { "ctrl-cmd-s" } else { "ctrl-alt-s" },
            ToggleSidebar,
        ),
        b(App, "toggle-dark-mode", "Light / Dark Appearance", "secondary-shift-l", ToggleTheme),
        b(App, "zoom-in", "Zoom In", "secondary-=, secondary-+", ZoomIn),
        b(App, "zoom-out", "Zoom Out", "secondary--", ZoomOut),
        b(App, "actual-size", "Actual Size", "secondary-0", ActualSize),
        b(App, "close", "Close / Cancel", "escape", Close),
        b(App, "minimize", "Minimize", "secondary-m", Minimize),
        b(App, "close-window", "Close Window", "secondary-w", CloseWindow),
        b(App, "lock", "Lock Picacho", "secondary-alt-l", LockApp),
        b(App, "quit", "Quit Picacho", "secondary-q", Quit),
    ]
}

/// What the mouse does that isn't a shortcut to change, shown with them.
pub(super) fn gestures() -> [(&'static str, String); 2] {
    let secondary = if cfg!(target_os = "macos") { "⌘" } else { "Ctrl+" };
    let shift = if cfg!(target_os = "macos") { "⇧" } else { "Shift+" };
    [
        ("Select a Range of Messages", format!("{shift}Click")),
        ("Add to or Take from the Selection", format!("{secondary}Click")),
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

/// The shortcuts as the file at `path` changes them.
pub(super) fn overrides_from(path: &std::path::Path) -> HashMap<String, String> {
    std::fs::read_to_string(path)
        .map(|t| parse(&t))
        .unwrap_or_default()
}

/// The file's header, above a line per action.
const HEADER: &str = "# Picacho's keyboard shortcuts, also changed in Settings → Keyboard Shortcuts.\n\
     # Take the # off a line and change its keys; changes apply without restarting.\n\
     # `secondary` is Cmd on macOS and Ctrl elsewhere; other modifiers are ctrl,\n\
     # alt, shift and cmd. Separate several shortcuts with commas; `none` takes\n\
     # an action's shortcut away.\n";

/// The file, written with every action and its default (commented out)
/// if there is none yet.
pub(super) fn ensure_file() -> std::io::Result<std::path::PathBuf> {
    let path = file_path();
    ensure_file_at(&path)?;
    Ok(path)
}

fn ensure_file_at(path: &std::path::Path) -> std::io::Result<()> {
    if path.exists() {
        return Ok(());
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut text = String::from(HEADER);
    for group in Group::ALL {
        text.push_str(&format!("\n# {}\n", group.title()));
        for b in bindable().into_iter().filter(|b| b.group == group) {
            text.push_str(&format!("# {} = {}\n", b.name, b.default));
        }
    }
    std::fs::write(path, text)
}

/// `text` with `name`'s line, set or commented out, replaced by `line`: in
/// the same place, or at the end when the file doesn't have one.
fn with_line(text: &str, name: &str, line: &str) -> String {
    let names = |l: &str| {
        let l = l.trim_start().trim_start_matches('#');
        l.split('#')
            .next()
            .and_then(|l| l.split_once('='))
            .is_some_and(|(n, _)| n.trim().eq_ignore_ascii_case(name))
    };
    let mut out: Vec<&str> = Vec::new();
    let mut placed = false;
    for l in text.lines() {
        if names(l) {
            if !placed {
                out.push(line);
                placed = true;
            }
        } else {
            out.push(l);
        }
    }
    if !placed {
        out.push(line);
    }
    let mut out = out.join("\n");
    out.push('\n');
    out
}

/// Gives `name` these keys in the file at `path` (`none` for no shortcut),
/// or its default again with `None`.
fn set_at(path: &std::path::Path, name: &str, keys: Option<&str>) -> std::io::Result<()> {
    ensure_file_at(path)?;
    let line = match keys {
        Some(keys) => format!("{name} = {keys}"),
        None => {
            let default = bindable()
                .into_iter()
                .find(|b| b.name == name)
                .map(|b| b.default)
                .unwrap_or_default();
            format!("# {name} = {default}")
        }
    };
    let text = std::fs::read_to_string(path)?;
    std::fs::write(path, with_line(&text, name, &line))
}

/// The host's operating system, for reading `secondary`.
fn os() -> gpui::OperatingSystem {
    if cfg!(target_os = "macos") {
        gpui::OperatingSystem::Mac
    } else if cfg!(windows) {
        gpui::OperatingSystem::Windows
    } else {
        gpui::OperatingSystem::Linux
    }
}

/// One shortcut as its keystrokes: one (`secondary-shift-r`), or several
/// pressed one after another (`secondary-k secondary-s`).
fn strokes(keys: &str) -> Option<Vec<Keystroke>> {
    let strokes = keys
        .split_whitespace()
        .map(|k| Keystroke::parse_with_os(k, os()).ok())
        .collect::<Option<Vec<_>>>()?;
    (!strokes.is_empty()).then_some(strokes)
}

/// The shortcuts in a comma-separated list, each as written.
fn split(keys: &str) -> Vec<String> {
    keys.split(',')
        .map(str::trim)
        .filter(|k| !k.is_empty())
        .map(str::to_owned)
        .collect()
}

/// The shortcuts an action has: the file's when they can be read, its
/// default otherwise, none for `none`.
pub(super) fn effective(b: &Bindable, custom: &HashMap<String, String>) -> Vec<String> {
    match custom.get(b.name).map(String::as_str) {
        Some("none") => Vec::new(),
        Some(keys) if !split(keys).is_empty() && split(keys).iter().all(|k| strokes(k).is_some()) => {
            split(keys)
        }
        _ => split(&b.default),
    }
}

/// Two shortcuts' keystrokes, when both can be read.
fn both(a: &str, b: &str) -> Option<(Vec<Keystroke>, Vec<Keystroke>)> {
    Some((strokes(a)?, strokes(b)?))
}

fn one(a: &Keystroke, b: &Keystroke) -> bool {
    a.modifiers == b.modifiers && a.key == b.key
}

/// Whether two shortcuts are the same keys, however they're written.
fn same(a: &str, b: &str) -> bool {
    match both(a, b) {
        Some((a, b)) => a.len() == b.len() && a.iter().zip(&b).all(|(a, b)| one(a, b)),
        None => a.eq_ignore_ascii_case(b),
    }
}

/// Whether one shortcut begins the other (`secondary-k` and
/// `secondary-k secondary-s`), or they're the same: pressing the shorter
/// waits to see whether the longer is coming.
fn competes(a: &str, b: &str) -> bool {
    match both(a, b) {
        Some((a, b)) => a.iter().zip(&b).all(|(a, b)| one(a, b)),
        None => a.eq_ignore_ascii_case(b),
    }
}

/// Whether an action has its default shortcuts.
pub(super) fn is_default(b: &Bindable, custom: &HashMap<String, String>) -> bool {
    let now = effective(b, custom);
    let default = split(&b.default);
    now.len() == default.len() && now.iter().zip(&default).all(|(a, b)| same(a, b))
}

/// Whether two actions' shortcuts would be pressed in the same place.
fn overlap(a: &Bindable, b: &Bindable) -> bool {
    a.targets
        .iter()
        .any(|(_, ca)| b.targets.iter().any(|(_, cb)| ca == cb))
}

/// The other actions given one of `keys`, or one beginning or begun by
/// one of them, where they'd compete.
pub(super) fn clashes<'a>(
    b: &Bindable,
    keys: &[String],
    all: &'a [Bindable],
    custom: &HashMap<String, String>,
) -> Vec<&'a Bindable> {
    all.iter()
        .filter(|o| o.name != b.name && overlap(b, o))
        .filter(|o| {
            effective(o, custom)
                .iter()
                .any(|k| keys.iter().any(|mine| competes(k, mine)))
        })
        .collect()
}

/// Keystrokes pressed one after another as the file writes them.
pub(super) fn written(strokes: &[Keystroke]) -> String {
    strokes.iter().map(written_one).collect::<Vec<_>>().join(" ")
}

/// A pressed keystroke as the file writes it, `secondary` for ⌘ on macOS
/// and Ctrl elsewhere.
fn written_one(k: &Keystroke) -> String {
    let m = &k.modifiers;
    let mac = cfg!(target_os = "macos");
    let mut out = String::new();
    if (mac && m.platform) || (!mac && m.control) {
        out.push_str("secondary-");
    }
    if mac && m.control {
        out.push_str("ctrl-");
    }
    if !mac && m.platform {
        out.push_str("super-");
    }
    if m.alt {
        out.push_str("alt-");
    }
    if m.shift {
        out.push_str("shift-");
    }
    if m.function {
        out.push_str("fn-");
    }
    out.push_str(&k.key);
    out
}

/// A shortcut as Settings shows it: ⌃⌥⇧⌘R on macOS, Ctrl+Shift+R
/// elsewhere, keystrokes pressed one after another apart.
pub(super) fn shown(keys: &str) -> String {
    match strokes(keys) {
        Some(strokes) => shown_strokes(&strokes),
        None => keys.to_owned(),
    }
}

/// Keystrokes as Settings shows them.
pub(super) fn shown_strokes(strokes: &[Keystroke]) -> String {
    strokes.iter().map(shown_one).collect::<Vec<_>>().join(" ")
}

fn shown_one(k: &Keystroke) -> String {
    let m = &k.modifiers;
    let mac = cfg!(target_os = "macos");
    let key = match k.key.as_str() {
        "enter" if mac => "↩".to_owned(),
        "enter" => "Enter".to_owned(),
        "backspace" if mac => "⌫".to_owned(),
        "delete" if mac => "⌦".to_owned(),
        "tab" if mac => "⇥".to_owned(),
        "escape" => "Esc".to_owned(),
        "up" => "↑".to_owned(),
        "down" => "↓".to_owned(),
        "left" => "←".to_owned(),
        "right" => "→".to_owned(),
        "pageup" => "Page Up".to_owned(),
        "pagedown" => "Page Down".to_owned(),
        "space" => "Space".to_owned(),
        key if key.chars().count() == 1 || key.starts_with('f') && key[1..].parse::<u8>().is_ok() => {
            key.to_uppercase()
        }
        key => {
            let mut c = key.chars();
            c.next()
                .map(|f| f.to_uppercase().chain(c).collect())
                .unwrap_or_default()
        }
    };
    if mac {
        let mut out = String::new();
        for (on, sign) in [(m.function, "fn "), (m.control, "⌃"), (m.alt, "⌥"), (m.shift, "⇧"), (m.platform, "⌘")] {
            if on {
                out.push_str(sign);
            }
        }
        out + &key
    } else {
        let super_key = if cfg!(windows) { "Win" } else { "Super" };
        [(m.control, "Ctrl"), (m.platform, super_key), (m.alt, "Alt"), (m.shift, "Shift"), (m.function, "Fn")]
            .into_iter()
            .filter(|(on, _)| *on)
            .map(|(_, name)| name.to_owned())
            .chain([key])
            .collect::<Vec<_>>()
            .join("+")
    }
}

/// The file's changes as last loaded, for Settings to show, and each
/// action's shortcuts with them, for tooltips.
struct Custom {
    changes: HashMap<String, String>,
    current: HashMap<&'static str, Vec<String>>,
}
impl Global for Custom {}
impl Custom {
    fn new(changes: HashMap<String, String>) -> Self {
        let current = bindable()
            .iter()
            .map(|b| (b.name, effective(b, &changes)))
            .collect();
        Self { changes, current }
    }
}

/// The file's changes as last loaded.
pub(super) fn custom(cx: &App) -> HashMap<String, String> {
    cx.try_global::<Custom>().map(|c| c.changes.clone()).unwrap_or_default()
}

/// The first shortcut `name` has now, as shown, if it has one.
pub(super) fn current(name: &str, cx: &App) -> Option<String> {
    let first = match cx.try_global::<Custom>() {
        Some(c) => c.current.get(name)?.first().cloned(),
        // Not loaded yet (tests): the default.
        None => bindable()
            .into_iter()
            .find(|b| b.name == name)
            .and_then(|b| split(&b.default).into_iter().next()),
    };
    first.map(|k| shown(&k))
}

/// A tooltip naming `name`'s shortcut as it is now ("Reply · ⇧⌘R"), or
/// just the label when it has none.
pub(super) fn tip(label: &str, name: &str, cx: &App) -> String {
    tip_then(label, name, "", cx)
}

/// [`tip`], with words after the shortcut ("· ⌘A selects all").
pub(super) fn tip_then(label: &str, name: &str, after: &str, cx: &App) -> String {
    match current(name, cx) {
        Some(keys) => format!("{label} · {keys}{after}"),
        None => label.to_owned(),
    }
}

/// The key bindings of the actions above, as the file at `path` changes them,
/// along with any lines that could not be parsed.
pub(super) fn bindings_from(path: &std::path::Path, cx: &App) -> (Vec<KeyBinding>, Vec<String>) {
    bindings_for(&overrides_from(path), cx)
}

fn bindings_for(custom: &HashMap<String, String>, cx: &App) -> (Vec<KeyBinding>, Vec<String>) {
    let mut out = Vec::new();
    let mut warnings = Vec::new();
    for b in bindable() {
        let load = |keys: &str| -> Option<Vec<KeyBinding>> {
            let mut bindings = Vec::new();
            for k in split(keys) {
                for (action, context) in &b.targets {
                    let context = Some(std::rc::Rc::new(
                        KeyBindingContextPredicate::parse(context).ok()?,
                    ));
                    bindings.push(
                        KeyBinding::load(&k, action.boxed_clone(), context, false, None, cx).ok()?,
                    );
                }
            }
            Some(bindings)
        };
        match custom.get(b.name).map(String::as_str) {
            Some("none") => {}
            Some(keys) => match load(keys) {
                Some(bindings) => out.extend(bindings),
                None => {
                    let msg = format!(
                        "the shortcut “{keys}” for {} isn't understood; keeping {}",
                        b.name, b.default
                    );
                    eprintln!("Picacho: {msg}");
                    warnings.push(msg);
                    out.extend(load(&b.default).unwrap_or_default());
                }
            },
            None => out.extend(load(&b.default).unwrap_or_default()),
        }
    }
    (out, warnings)
}

/// Every action's default keys are bound, in every place they work.
#[cfg(test)]
pub(super) fn defaults_all_load(cx: &App) {
    let (bindings, warnings) = bindings_for(&HashMap::new(), cx);
    assert!(warnings.is_empty());
    let expected: usize = bindable()
        .iter()
        .map(|b| split(&b.default).len() * b.targets.len())
        .sum();
    assert_eq!(bindings.len(), expected, "a default shortcut didn't load");
}

/// Clears and rebuilds the active keymap from `path`, preserving the base
/// component bindings snapshotted at startup. Returns any parse warnings.
pub(super) fn reload_from(path: &std::path::Path, cx: &mut App) -> Vec<String> {
    let base = cx
        .try_global::<BaseBindings>()
        .map(|b| b.0.clone())
        .unwrap_or_default();
    cx.clear_key_bindings();
    if !base.is_empty() {
        cx.bind_keys(base);
    }
    let (app, warnings) = app_bindings_from(path, cx);
    cx.bind_keys(app);
    cx.set_global(Custom::new(overrides_from(path)));
    warnings
}

/// Reloads `shortcuts.txt` into the live keymap without restarting.
pub(super) fn reload(cx: &mut App) -> Vec<String> {
    reload_from(&file_path(), cx)
}

/// Watches `shortcuts.txt` for edits (mtime/size changes) and reloads it
/// automatically while Picacho runs.
#[cfg_attr(any(target_os = "android", target_os = "ios"), allow(dead_code))]
pub(super) fn watch(cx: &mut App) {
    type Stamp = Option<(Option<std::time::SystemTime>, u64)>;
    fn stamp(path: &std::path::Path) -> Stamp {
        let meta = std::fs::metadata(path).ok()?;
        Some((meta.modified().ok(), meta.len()))
    }
    let path = file_path();
    let mut last = stamp(&path);
    cx.spawn(async move |cx| {
        loop {
            cx.background_executor()
                .timer(Duration::from_millis(1500))
                .await;
            let now = stamp(&path);
            if now != last {
                last = now;
                cx.update(|cx| {
                    reload(cx);
                    cx.refresh_windows();
                });
            }
        }
    })
    .detach();
}

/// Settings → Keyboard Shortcuts: the action waiting for keys, if any.
#[derive(Default)]
pub(super) struct KeysUi {
    recording: Option<&'static str>,
    /// Takes every keystroke while recording, before shortcuts see it.
    intercept: Option<Subscription>,
    /// Why the last keys pressed weren't taken.
    hint: Option<String>,
    /// Keys pressed so far: another may follow, for a sequence.
    pressed: Vec<Keystroke>,
    /// Counts keys pressed, so a pause saves only after the last.
    presses: u64,
}

/// How long recording waits for another key of a sequence, as long as
/// pressing a sequence waits between its keys.
const SEQUENCE_PAUSE: Duration = Duration::from_millis(1000);
/// The most keys in a sequence recorded.
const SEQUENCE_MAX: usize = 3;

#[derive(Clone)]
pub(super) enum KeyAction {
    /// Waits for keys to give this action.
    Record(&'static str),
    /// Stops waiting for keys.
    Cancel,
    /// Gives this action its default shortcut again.
    Reset(&'static str),
    /// Takes this action's shortcut away.
    Remove(&'static str),
    /// Every action's default shortcut again.
    ResetAll,
    /// Opens `shortcuts.txt` in the system's text editor.
    OpenFile,
}

impl MailApp {
    pub(super) fn key_action(&mut self, action: KeyAction, cx: &mut Context<Self>) {
        match action {
            KeyAction::Record(name) => {
                let this = cx.weak_entity();
                let intercept = cx.intercept_keystrokes(move |e, _, cx| {
                    let k = e.keystroke.clone();
                    let taken = this
                        .update(cx, |this, cx| {
                            // Settings closed or went to another page meanwhile.
                            let open = this.modal == Some(Modal::Settings)
                                || this.settings_window.is_some();
                            if !open || this.page != Page::Shortcuts {
                                this.stop_recording(cx);
                                return false;
                            }
                            // Modifiers alone are waited past, for the key they go with.
                            if !k.is_modifier_key() {
                                this.recorded(k, cx);
                            }
                            true
                        })
                        .unwrap_or(false);
                    if taken {
                        cx.stop_propagation();
                    }
                });
                self.keys_ui = KeysUi {
                    recording: Some(name),
                    intercept: Some(intercept),
                    ..Default::default()
                };
            }
            KeyAction::Cancel => self.stop_recording(cx),
            KeyAction::Reset(name) => {
                self.stop_recording(cx);
                self.save_shortcut(name, None, cx);
            }
            KeyAction::Remove(name) => {
                self.stop_recording(cx);
                self.save_shortcut(name, Some("none"), cx);
            }
            KeyAction::ResetAll => {
                self.stop_recording(cx);
                let path = file_path();
                let result = (|| -> std::io::Result<()> {
                    for b in bindable() {
                        set_at(&path, b.name, None)?;
                    }
                    Ok(())
                })();
                match result {
                    Ok(()) => {
                        self.apply_shortcuts(cx);
                        self.notice("Every shortcut is back to its default", false, cx);
                    }
                    Err(e) => self.notice(format!("Couldn't save the shortcuts: {e}"), true, cx),
                }
            }
            KeyAction::OpenFile => match ensure_file() {
                Ok(path) => cx.open_with_system(&path),
                Err(e) => self.notice(format!("Couldn't write the shortcuts file: {e}"), true, cx),
            },
        }
        cx.notify();
    }

    fn stop_recording(&mut self, cx: &mut Context<Self>) {
        let ui = std::mem::take(&mut self.keys_ui);
        // Dropped once the keystroke that ended it is through.
        if let Some(intercept) = ui.intercept {
            cx.defer(move |_| drop(intercept));
        }
    }

    /// Keys pressed while an action waits for them. They're saved after
    /// a pause, unless another follows to make a sequence.
    fn recorded(&mut self, k: Keystroke, cx: &mut Context<Self>) {
        if self.keys_ui.recording.is_none() {
            return;
        }
        let m = &k.modifiers;
        if k.key == "escape" && !m.modified() {
            self.stop_recording(cx);
            cx.notify();
            return;
        }
        // A letter, a digit or the space bar alone would be typed instead;
        // after the first key of a sequence, it's fine.
        let typed = k.key.chars().count() == 1 || matches!(k.key.as_str(), "space" | "enter" | "tab");
        if self.keys_ui.pressed.is_empty() && typed && !(m.control || m.alt || m.platform || m.function) {
            self.keys_ui.hint = Some(if cfg!(target_os = "macos") {
                "Add ⌘, ⌃ or ⌥ to that key".into()
            } else {
                "Add Ctrl or Alt to that key".into()
            });
            cx.notify();
            return;
        }
        self.keys_ui.hint = None;
        self.keys_ui.pressed.push(k);
        self.keys_ui.presses += 1;
        if self.keys_ui.pressed.len() >= SEQUENCE_MAX {
            self.save_recorded(cx);
            return;
        }
        let presses = self.keys_ui.presses;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SEQUENCE_PAUSE).await;
            let _ = this.update(cx, |this, cx| {
                if this.keys_ui.presses == presses && this.keys_ui.recording.is_some() {
                    this.save_recorded(cx);
                }
            });
        })
        .detach();
        cx.notify();
    }

    /// Gives the keys recorded to the action waiting for them.
    fn save_recorded(&mut self, cx: &mut Context<Self>) {
        let Some(name) = self.keys_ui.recording else {
            return;
        };
        let keys = written(&self.keys_ui.pressed);
        self.stop_recording(cx);
        if keys.is_empty() {
            return;
        }
        // The keys are this action's now; others with the very same keys
        // where they'd compete give them up.
        let all = bindable();
        let custom = custom(cx);
        let mut taken = Vec::new();
        if let Some(me) = all.iter().find(|b| b.name == name) {
            for other in clashes(me, std::slice::from_ref(&keys), &all, &custom) {
                let theirs = effective(other, &custom);
                if !theirs.iter().any(|o| same(o, &keys)) {
                    // Only begins or is begun by them: both keep theirs.
                    continue;
                }
                let left: Vec<String> = theirs.into_iter().filter(|o| !same(o, &keys)).collect();
                let left = if left.is_empty() { "none".to_owned() } else { left.join(", ") };
                if let Err(e) = set_at(&file_path(), other.name, Some(&left)) {
                    self.notice(format!("Couldn't save the shortcuts: {e}"), true, cx);
                    return;
                }
                taken.push(other.label);
            }
        }
        self.save_shortcut(name, Some(&keys), cx);
        if !taken.is_empty() {
            self.notice(
                format!("{} no longer has {}", taken.join(", "), shown(&keys)),
                false,
                cx,
            );
        }
        cx.notify();
    }

    fn save_shortcut(&mut self, name: &str, keys: Option<&str>, cx: &mut Context<Self>) {
        match set_at(&file_path(), name, keys) {
            Ok(()) => self.apply_shortcuts(cx),
            Err(e) => self.notice(format!("Couldn't save the shortcuts: {e}"), true, cx),
        }
    }

    fn apply_shortcuts(&mut self, cx: &mut Context<Self>) {
        reload(cx);
        cx.refresh_windows();
    }

    /// Settings → Keyboard Shortcuts.
    pub(super) fn shortcuts_page(&self, cx: &Context<Self>) -> AnyElement {
        let c = self.colors();
        let all = bindable();
        let custom = custom(cx);
        let changed = all.iter().any(|b| !is_default(b, &custom));
        let chip = move |text: String| {
            div()
                .px_2()
                .py(px(3.))
                .rounded(px(6.))
                .bg(rgb(c.bg))
                .border_1()
                .border_color(rgb(c.border))
                .text_size(px(12.))
                .text_color(rgb(c.text))
                .child(text)
        };
        let icon_button = move |id: String, icon: &'static str, tip: &'static str, action: KeyAction| {
            div()
                .id(SharedString::from(id.clone()))
                .debug_selector(move || id)
                .role(Role::Button)
                .aria_label(tip)
                .focusable()
                .tab_stop(true)
                .size(px(24.))
                .flex()
                .flex_shrink_0()
                .items_center()
                .justify_center()
                .rounded(px(6.))
                .cursor_pointer()
                .hover(move |s| s.bg(rgb(c.raised)))
                .child(icon_el(icon, 13., c.muted))
                .tooltip(move |w, cx| Tooltip::new(tip).build(w, cx))
                .on_click(cx.listener(move |this, _, w, cx| {
                    this.handle(Action::Keys(action.clone()), w, cx)
                }))
        };
        let mut page = div()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(c.muted))
                    .child("Click a shortcut and press the keys you'd like for it. For a sequence, press the next key within a second. Keys another action had are taken from it."),
            );
        for group in Group::ALL {
            page = page.child(self.section(group.title()));
            let mut rows = div().flex().flex_col();
            for b in all.iter().filter(|b| b.group == group) {
                let keys = effective(b, &custom);
                let recording = self.keys_ui.recording == Some(b.name);
                let clash = clashes(b, &keys, &all, &custom);
                let name = b.name;
                let label = b.label;
                let id = format!("shortcut-{name}");
                let keys_el = div()
                    .id(SharedString::from(id.clone()))
                    .debug_selector(move || id)
                    .role(Role::Button)
                    .aria_label(format!("Change the shortcut for {label}"))
                    .focusable()
                    .tab_stop(true)
                    .focus_visible(move |s| s.border_color(rgb(c.accent)))
                    .min_w(px(120.))
                    .min_h(px(30.))
                    .px_1()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .justify_end()
                    .gap_1()
                    .rounded(px(8.))
                    .border_1()
                    .cursor_pointer()
                    .map(|d| {
                        if recording {
                            let pressed = &self.keys_ui.pressed;
                            let note = match &self.keys_ui.hint {
                                Some(hint) => hint.clone(),
                                None if pressed.is_empty() => "Press keys… (Esc cancels)".into(),
                                // A pause saves them; another key makes a sequence.
                                None => "…".into(),
                            };
                            d.border_color(rgb(c.accent))
                                .bg(rgb(c.soft))
                                .when(!pressed.is_empty(), |d| d.child(chip(shown_strokes(pressed))))
                                .child(
                                    div()
                                        .px_1()
                                        .text_size(px(12.))
                                        .text_color(rgb(c.accent))
                                        .child(note),
                                )
                        } else if keys.is_empty() {
                            d.border_color(rgb(c.surface))
                                .hover(move |s| s.border_color(rgb(c.border)))
                                .child(
                                    div()
                                        .px_1()
                                        .text_size(px(12.))
                                        .text_color(rgb(c.muted))
                                        .child("None"),
                                )
                        } else {
                            d.border_color(rgb(c.surface))
                                .hover(move |s| s.border_color(rgb(c.border)))
                                .children(keys.iter().map(|k| chip(shown(k))))
                        }
                    })
                    .on_click(cx.listener(move |this, _, w, cx| {
                        let action = if recording { KeyAction::Cancel } else { KeyAction::Record(name) };
                        this.handle(Action::Keys(action), w, cx)
                    }));
                let row = div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .py_1()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(div().text_size(px(13.)).child(label))
                            .when(!clash.is_empty(), |d| {
                                let names: Vec<&str> = clash.iter().map(|o| o.label).collect();
                                d.child(
                                    div()
                                        .text_size(px(11.))
                                        .text_color(rgb(c.warning))
                                        .child(format!("Also used by {}", names.join(", "))),
                                )
                            }),
                    )
                    .child(keys_el)
                    // Kept in place when hidden, so the shortcuts line up.
                    .child(div().size(px(24.)).flex_shrink_0().when(!is_default(b, &custom), |d| {
                        d.child(icon_button(
                            format!("shortcut-reset-{name}"),
                            "undo",
                            "Use the default shortcut",
                            KeyAction::Reset(name),
                        ))
                    }))
                    .child(div().size(px(24.)).flex_shrink_0().when(!keys.is_empty(), |d| {
                        d.child(icon_button(
                            format!("shortcut-remove-{name}"),
                            "close",
                            "No shortcut",
                            KeyAction::Remove(name),
                        ))
                    }));
                rows = rows.child(row);
            }
            if group == Group::Mailbox {
                for (label, keys) in gestures() {
                    rows = rows.child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .py_1()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_size(px(13.))
                                    .text_color(rgb(c.muted))
                                    .child(label),
                            )
                            .child(
                                div()
                                    .min_w(px(120.))
                                    .px_1()
                                    .flex()
                                    .justify_end()
                                    .child(chip(keys).text_color(rgb(c.muted))),
                            )
                            .child(div().w(px(56.)).flex_shrink_0()),
                    );
                }
            }
            page = page.child(rows);
        }
        page.child(self.section("Shortcuts File"))
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(c.muted))
                    .child(format!(
                        "Shortcuts are kept in {}, a line per action such as “reply = secondary-shift-r”. Edits to it apply as soon as it's saved.",
                        file_path().display()
                    )),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .child(self.button(
                        "open-shortcuts",
                        "Edit as Text",
                        "external",
                        Action::Keys(KeyAction::OpenFile),
                        false,
                        cx,
                    ))
                    .when(changed, |d| {
                        d.child(self.button(
                            "reset-shortcuts",
                            "Reset All Shortcuts",
                            "undo",
                            Action::Keys(KeyAction::ResetAll),
                            false,
                            cx,
                        ))
                    }),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::Action as _;
    use std::prelude::v1::test;

    #[test]
    fn lines_name_actions_and_keys() {
        let m = parse("# comment\nreply = Ctrl-R # mine\n  archive=none\nbad line\n");
        assert_eq!(m.get("reply").map(String::as_str), Some("ctrl-r"));
        assert_eq!(m.get("archive").map(String::as_str), Some("none"));
        assert_eq!(m.len(), 2);
    }

    #[test]
    fn a_changed_shortcut_takes_the_place_of_its_line() {
        let text = "# header\n# reply = secondary-shift-r\n# archive = secondary-shift-a\n";
        let text = with_line(text, "reply", "reply = ctrl-y");
        assert_eq!(text, "# header\nreply = ctrl-y\n# archive = secondary-shift-a\n");
        // Set again, and back to the default: still one line, in place.
        let text = with_line(&text, "reply", "# reply = secondary-shift-r");
        assert_eq!(text, "# header\n# reply = secondary-shift-r\n# archive = secondary-shift-a\n");
        // An action the file doesn't list yet goes at the end.
        let text = with_line(&text, "bold", "bold = none");
        assert!(text.ends_with("# archive = secondary-shift-a\nbold = none\n"));
    }

    #[test]
    fn settings_saves_shortcuts_to_the_file() {
        let dir = std::env::temp_dir().join(format!("picacho-shortcuts-{}", uuid::Uuid::new_v4()));
        let path = dir.join("shortcuts.txt");
        set_at(&path, "reply", Some("secondary-y")).unwrap();
        set_at(&path, "archive", Some("none")).unwrap();
        let custom = overrides_from(&path);
        assert_eq!(custom.get("reply").map(String::as_str), Some("secondary-y"));
        assert_eq!(custom.get("archive").map(String::as_str), Some("none"));
        let all = bindable();
        let reply = all.iter().find(|b| b.name == "reply").unwrap();
        let archive = all.iter().find(|b| b.name == "archive").unwrap();
        assert!(!is_default(reply, &custom));
        assert!(effective(archive, &custom).is_empty());
        set_at(&path, "reply", None).unwrap();
        let custom = overrides_from(&path);
        assert!(is_default(reply, &custom));
        // The file still lists every action once.
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text.matches("reply =").count(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pressed_keys_are_written_and_shown_for_this_platform() {
        let k = Keystroke::parse_with_os("secondary-shift-r", os()).unwrap();
        assert_eq!(written(std::slice::from_ref(&k)), "secondary-shift-r");
        let shown = shown("secondary-shift-r");
        if cfg!(target_os = "macos") {
            assert_eq!(shown, "⇧⌘R");
        } else {
            assert_eq!(shown, "Ctrl+Shift+R");
        }
        assert!(same("secondary-alt-r", &written(&[Keystroke::parse_with_os("secondary-alt-r", os()).unwrap()])));
    }

    #[test]
    fn sequences_are_read_shown_and_compete_with_what_begins_them() {
        let all = bindable();
        let mut custom = HashMap::new();
        custom.insert("archive".to_owned(), "secondary-k secondary-a".to_owned());
        let archive = all.iter().find(|b| b.name == "archive").unwrap();
        assert_eq!(effective(archive, &custom), vec!["secondary-k secondary-a"]);
        assert!(same("secondary-k secondary-a", "SECONDARY-k   secondary-a"));
        assert!(!same("secondary-k", "secondary-k secondary-a"));
        assert!(competes("secondary-k", "secondary-k secondary-a"));
        assert!(!competes("secondary-j", "secondary-k secondary-a"));
        let both = shown("secondary-k secondary-a");
        assert_eq!(both, format!("{} {}", shown("secondary-k"), shown("secondary-a")));
        // Code (⌘E) doesn't compete; Insert Link (⌘K) waits on it, but
        // that's the composer, not the main window.
        assert!(clashes(archive, &effective(archive, &custom), &all, &custom).is_empty());
        let refresh = all.iter().find(|b| b.name == "refresh").unwrap();
        let clash: Vec<_> = clashes(refresh, &["secondary-k".into()], &all, &custom)
            .iter()
            .map(|b| b.name)
            .collect();
        assert_eq!(clash, vec!["archive"]);
    }

    #[test]
    fn shortcuts_clash_only_where_both_work() {
        let all = bindable();
        let custom = HashMap::new();
        let find = |n: &str| all.iter().find(|b| b.name == n).unwrap();
        // Save Draft (composer) and Star (main window) don't compete...
        let star = find("toggle-star");
        assert!(clashes(star, &["secondary-s".into()], &all, &custom).is_empty());
        // ...but Reply's keys given to Forward do.
        let forward = find("forward");
        let clash: Vec<_> = clashes(forward, &["secondary-shift-r".into()], &all, &custom)
            .iter()
            .map(|b| b.name)
            .collect();
        assert_eq!(clash, vec!["reply"]);
        // No default competes with another.
        for b in &all {
            assert!(clashes(b, &effective(b, &custom), &all, &custom).is_empty(), "{}", b.name);
        }
    }

    #[gpui::test]
    fn keys_pressed_in_settings_become_the_shortcut(cx: &mut gpui::TestAppContext) {
        use super::super::tests::{act, frame, open, store};
        let (view, cx) = open(store(), cx);
        // Help → Keyboard Shortcuts is Settings' page.
        act(&view, Action::Help, cx);
        frame(cx);
        view.read_with(cx, |this, _| {
            assert_eq!(this.page, Page::Shortcuts);
            assert!(this.modal == Some(Modal::Settings) || this.settings_window.is_some());
        });
        assert!(cx.debug_bounds("shortcut-forward").is_some());

        // Escape stops waiting without changing anything.
        act(&view, Action::Keys(KeyAction::Record("forward")), cx);
        cx.simulate_keystrokes("escape");
        view.read_with(cx, |this, _| assert!(this.keys_ui.recording.is_none()));
        assert!(!overrides_from(&file_path()).contains_key("forward"));

        // A letter alone isn't taken; with the secondary key it is, and
        // Reply, which had it, gives it up.
        act(&view, Action::Keys(KeyAction::Record("forward")), cx);
        cx.simulate_keystrokes("r");
        view.read_with(cx, |this, _| assert_eq!(this.keys_ui.recording, Some("forward")));
        let secondary = if cfg!(target_os = "macos") { "cmd" } else { "ctrl" };
        cx.simulate_keystrokes(&format!("{secondary}-shift-r"));
        // Waiting for another key of a sequence, until a pause.
        view.read_with(cx, |this, _| assert_eq!(this.keys_ui.pressed.len(), 1));
        assert!(!overrides_from(&file_path()).contains_key("forward"));
        cx.executor().advance_clock(SEQUENCE_PAUSE);
        cx.run_until_parked();
        frame(cx);
        let custom = overrides_from(&file_path());
        assert_eq!(custom.get("forward").map(String::as_str), Some("secondary-shift-r"));
        assert_eq!(custom.get("reply").map(String::as_str), Some("none"));
        view.read_with(cx, |this, cx| {
            assert!(this.keys_ui.recording.is_none());
            assert_eq!(super::custom(cx), custom);
        });

        // Tooltips name the shortcut as it is now.
        view.read_with(cx, |_, cx| {
            assert_eq!(tip("@Reply", "reply", cx), "@Reply");
            assert_eq!(tip("@Forward", "forward", cx), format!("@Forward · {}", shown("secondary-shift-r")));
        });

        // Keys pressed one after another make a sequence; a plain letter is
        // fine after the first.
        act(&view, Action::Keys(KeyAction::Record("archive")), cx);
        cx.simulate_keystrokes(&format!("{secondary}-k"));
        cx.simulate_keystrokes("a");
        cx.executor().advance_clock(SEQUENCE_PAUSE);
        cx.run_until_parked();
        let custom = overrides_from(&file_path());
        assert_eq!(custom.get("archive").map(String::as_str), Some("secondary-k a"));
        cx.update(|_, cx| {
            let keymap = cx.key_bindings();
            let keymap = keymap.borrow();
            let archive: Vec<usize> = keymap
                .bindings()
                .filter(|b| b.action().name() == Archive.name())
                .map(|b| b.keystrokes().len())
                .collect();
            assert_eq!(archive, vec![2], "the sequence is bound");
        });
        view.read_with(cx, |_, cx| {
            assert_eq!(tip("@Archive", "archive", cx), format!("@Archive · {}", shown("secondary-k a")));
        });

        act(&view, Action::Keys(KeyAction::ResetAll), cx);
        let custom = overrides_from(&file_path());
        assert!(bindable().iter().all(|b| is_default(b, &custom)));
        let _ = std::fs::remove_dir_all(file_path().parent().unwrap());
    }

    #[gpui::test]
    fn reloading_shortcuts_updates_active_keybindings(cx: &mut gpui::TestAppContext) {
        let dir = std::env::temp_dir().join(format!("picacho-shortcuts-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("shortcuts.txt");

        cx.update(|cx| {
            // Seed a base binding (like gpui_component::init does) and snapshot it.
            let base = key(cx, "ctrl-shift-b", Hide, Some("TestBase"));
            cx.bind_keys([base]);
            save_base(cx);

            std::fs::write(&path, "reply = ctrl-alt-r\narchive = none\n").unwrap();
            let warnings = reload_from(&path, cx);
            assert!(warnings.is_empty());

            let active = cx.key_bindings();
            let active = active.borrow();
            let reply_keys: Vec<String> = active
                .bindings()
                .filter(|b| b.action().name() == Reply.name())
                .map(|b| {
                    b.keystrokes()
                        .iter()
                        .map(|k| k.unparse(gpui::OperatingSystem::Linux))
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .collect();
            assert_eq!(reply_keys, vec!["ctrl-alt-r"]);
            assert!(
                !active.bindings().any(|b| b.action().name() == Archive.name()),
                "archive = none should remove the Archive binding"
            );
            assert!(
                active.bindings().any(|b| {
                    b.action().name() == Hide.name()
                        && b.predicate().is_some_and(|p| format!("{p}").contains("TestBase"))
                }),
                "base bindings snapshotted before reload should be preserved"
            );
            drop(active);

            // Edit the file again: live reload replaces the custom Reply key and restores Archive.
            std::fs::write(&path, "reply = ctrl-shift-y\n").unwrap();
            let warnings = reload_from(&path, cx);
            assert!(warnings.is_empty());

            let active = cx.key_bindings();
            let active = active.borrow();
            let reply_keys: Vec<String> = active
                .bindings()
                .filter(|b| b.action().name() == Reply.name())
                .map(|b| {
                    b.keystrokes()
                        .iter()
                        .map(|k| k.unparse(gpui::OperatingSystem::Linux))
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .collect();
            assert_eq!(reply_keys, vec!["ctrl-shift-y"]);
            assert!(
                active.bindings().any(|b| b.action().name() == Archive.name()),
                "removing archive = none should restore Archive's default binding"
            );
        });

        let _ = std::fs::remove_dir_all(&dir);
    }
}

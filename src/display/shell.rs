//! The desktop app's life beyond its windows.
//!
//! Closing Picacho's last window quits this process. When Picacho keeps
//! running in the background (Settings → General), it then starts the
//! small background process (`resident`), which checks mail without any of
//! the window's memory, shows the tray icon, and starts this process again
//! to open the window.
//!
//! The shell owns what outlives a window here: the backend (accounts'
//! connections, sessions and locks, shared with each new window), the tray
//! icon when Settings shows it with a window open, and the socket later
//! launches reach (`instance`).
use super::*;
use crate::display::{
    instance, resident,
    tray::{self, Tray, TrayCommand},
};
use crate::manage::background;
use futures::{StreamExt, channel::mpsc::UnboundedSender};

pub(super) struct Shell {
    backend: Arc<Backend>,
    /// The main window, while it is open.
    main: Option<AnyWindowHandle>,
    /// The latest main window's view. It may outlive its window while a
    /// detached window (Settings, a message) still draws from it.
    view: Option<WeakEntity<MailApp>>,
    /// The last window closed and the background process takes over once
    /// this one has quit.
    hand_over: bool,
    tray: TrayState,
    /// Bumped for each tray being created, so a creation that finishes
    /// after the tray was no longer wanted is dropped.
    tray_generation: u64,
    /// Where the tray and later launches send their requests.
    commands: UnboundedSender<TrayCommand>,
    /// The single-instance socket, removed at quitting.
    instance: Option<std::path::PathBuf>,
}

enum TrayState {
    Hidden,
    /// Being registered off the main thread (Linux's session bus).
    #[cfg_attr(not(any(target_os = "linux", target_os = "freebsd")), allow(dead_code))]
    Creating(u64),
    /// Kept for its `Drop`, which removes the icon.
    Shown(#[allow(dead_code)] Tray),
    /// This desktop has no tray; asked again once the icon was not wanted
    /// for a while (a tray may have appeared since).
    Unavailable,
}

impl Global for Shell {}

/// Sets up the shell and opens the main window. `listener` answers later
/// launches of Picacho.
pub(super) fn launch(store: Arc<Store>, listener: Option<instance::Listener>, cx: &mut App) {
    let backend = new_backend(store);
    let (commands, mut requests) = futures::channel::mpsc::unbounded::<TrayCommand>();
    let instance = listener.map(|listener| {
        let commands = commands.clone();
        listener.serve(move || commands.unbounded_send(TrayCommand::Open).is_ok())
    });
    // The login item follows the app should it have moved (an update, an
    // AppImage elsewhere).
    if background::starts_at_login(&backend.store) {
        let dir = backend.store.path.clone();
        cx.background_executor()
            .spawn(async move { crate::display::autostart::refresh(&dir) })
            .detach();
    }
    cx.set_global(Shell {
        backend,
        main: None,
        view: None,
        hand_over: false,
        tray: TrayState::Hidden,
        tray_generation: 0,
        commands,
        instance,
    });
    // Lives as long as the app: the shell keeps a sender.
    cx.spawn(async move |cx| {
        while let Some(command) = requests.next().await {
            cx.update(|cx| match command {
                TrayCommand::Open => open(cx),
                TrayCommand::Widget => {
                    crate::display::launch::push("picacho://widget/panel".to_owned());
                    open(cx);
                }
                TrayCommand::Quit => quit(cx),
            });
        }
    })
    .detach();
    cx.on_window_closed(|cx, _| window_closed(cx)).detach();
    cx.on_app_quit(|cx| {
        shut_down(cx);
        std::future::ready(())
    })
    .detach();
    open(cx);
}

/// Whether closing the last window leaves Picacho running in the
/// background (as `window_closed` decides) rather than quitting.
pub(super) fn hands_over(cx: &App) -> bool {
    cx.try_global::<Shell>()
        .is_some_and(|shell| background::enabled(&shell.backend.store) && shell.instance.is_some())
}

/// Brings Picacho's window forward, opening the main window if none is.
pub(super) fn open(cx: &mut App) {
    let windows = cx.windows();
    let main = cx.global::<Shell>().main;
    let shown = main
        .filter(|main| windows.iter().any(|w| w.window_id() == main.window_id()))
        .or_else(|| windows.first().copied());
    if let Some(window) = shown {
        let _ = window.update(cx, |_, w, _| w.activate_window());
        cx.activate(true);
        // Drawn again, it takes what a later launch asked to open.
        if let Some(view) = cx.global::<Shell>().view.as_ref().and_then(|v| v.upgrade()) {
            view.update(cx, |_, cx| cx.notify());
        }
        return;
    }
    let backend = cx.global::<Shell>().backend.clone();
    match open_main_window(backend, cx) {
        Ok((window, view)) => {
            let shell = cx.global_mut::<Shell>();
            shell.main = Some(window);
            shell.view = Some(view);
        }
        Err(e) => {
            eprintln!("Picacho: could not open the mail window: {e:#}");
            // Back to the background, or out if there's none to go to.
            window_closed(cx);
            return;
        }
    }
    update_tray(cx);
    cx.activate(true);
}

/// Quits from the tray: as the Quit command does, refusing while a message
/// is being sent.
fn quit(cx: &mut App) {
    let app = cx
        .global::<Shell>()
        .view
        .as_ref()
        .and_then(|view| view.upgrade());
    match app {
        Some(app) => app.update(cx, |this, cx| this.quit_app(cx)),
        None => cx.quit(),
    }
}

fn window_closed(cx: &mut App) {
    let windows = cx.windows();
    let shell = cx.global_mut::<Shell>();
    if shell
        .main
        .is_some_and(|main| !windows.iter().any(|w| w.window_id() == main.window_id()))
    {
        shell.main = None;
    }
    if !windows.is_empty() {
        return;
    }
    // The background process needs the socket to be reached by later
    // launches; without one, Picacho just quits.
    shell.hand_over = background::enabled(&shell.backend.store) && shell.instance.is_some();
    if let Some(view) = shell.view.take().and_then(|view| view.upgrade()) {
        view.update(cx, |this, _| this.retire());
    }
    // Quitting at once left the window on screen, frozen, until the process
    // had ended: on Wayland its removal is only sent to the compositor as
    // the event loop next goes to sleep, which a loop told to stop never
    // does. A moment's wait lets it go first; the saving and handing over
    // that quitting does then happen with nothing showing.
    cx.spawn(async move |cx| {
        cx.background_executor().timer(QUIT_AFTER_CLOSE).await;
        cx.update(|cx| {
            // Opened again meanwhile (a launch, the tray): it stays.
            if cx.windows().is_empty() {
                cx.quit();
            }
        });
    })
    .detach();
}

/// How long quitting waits once the last window has closed, so the system
/// takes the window away first (see `window_closed`).
const QUIT_AFTER_CLOSE: std::time::Duration = std::time::Duration::from_millis(50);

/// Shows or hides the tray icon with a window open, as Settings asks. (The
/// background process shows its own.)
pub(super) fn update_tray(cx: &mut App) {
    let Some(shell) = cx.try_global::<Shell>() else {
        return;
    };
    let wanted = background::tray_while_open(&shell.backend.store);
    let shell = cx.global_mut::<Shell>();
    match (&shell.tray, wanted) {
        (TrayState::Hidden, true) => {}
        (TrayState::Hidden | TrayState::Unavailable, false) => {
            shell.tray = TrayState::Hidden;
            return;
        }
        (_, false) => {
            // Dropping it removes the icon.
            shell.tray = TrayState::Hidden;
            return;
        }
        (_, true) => return,
    }
    shell.tray_generation += 1;
    let generation = shell.tray_generation;
    let commands = shell.commands.clone();
    // Linux registers with the tray over the session bus, which must never
    // hold up the window; the other systems add it on the main thread.
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    {
        shell.tray = TrayState::Creating(generation);
        let created = cx
            .background_executor()
            .spawn(async move { tray::create(commands) });
        cx.spawn(async move |cx| {
            let tray = created.await;
            cx.update(|cx| {
                let Some(shell) = cx.try_global::<Shell>() else {
                    return;
                };
                if !matches!(shell.tray, TrayState::Creating(g) if g == generation) {
                    // No longer wanted: dropping `tray` removes it again.
                    return;
                }
                cx.global_mut::<Shell>().tray = match tray {
                    Some(tray) => TrayState::Shown(tray),
                    None => TrayState::Unavailable,
                };
            });
        })
        .detach();
    }
    #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
    {
        let _ = generation;
        shell.tray = match tray::create(commands) {
            Some(tray) => TrayState::Shown(tray),
            None => TrayState::Unavailable,
        };
    }
}

/// As the app quits: the tray ends, what was set to be saved is, and the
/// background process starts when it takes over.
fn shut_down(cx: &mut App) {
    let window_open = cx
        .global::<Shell>()
        .view
        .as_ref()
        .is_some_and(|view| view.upgrade().is_some());
    let shell = cx.global_mut::<Shell>();
    // First, so a launch meanwhile starts afresh rather than reaching an
    // app that is going away.
    let socket = shell.instance.take();
    if let Some(path) = &socket {
        instance::release(path);
    }
    shell.tray = TrayState::Hidden;
    shell.backend.store.flush();
    // The window does this itself as it quits (`MailApp::new`).
    if !window_open {
        let _ = shell.backend.store.compact_on_quit();
    }
    // Last, once the cache is saved and the socket free for it to take.
    if shell.hand_over
        && let Some(dir) = socket.as_deref().and_then(|path| path.parent())
        && let Err(e) = resident::start(dir)
    {
        eprintln!("Picacho: could not keep running in the background: {e}");
    }
}

//! Picacho in the background, as a small process of its own.
//!
//! When the last window closes and Picacho keeps running in the background
//! (Settings → General), the window's process quits and starts this one
//! (`picacho --background <data directory>`). It has no GPUI and none of the
//! window's state: only the store, the backend, the monitor that checks
//! mail and announces it (`manage::background`), the tray icon, and the
//! socket later launches reach (`instance`). Its memory stays near what
//! those need, rather than what a window used.
//!
//! Opening Picacho (the tray, launching it again, macOS's Dock) hands back:
//! this process stops the monitor and lets it finish what it was doing (a
//! scheduled send in full), releases the socket, starts the window's process
//! and exits. Exactly one of the two runs at a time, so they never check
//! mail, or send, side by side.
use super::{
    instance,
    tray::{self, TrayCommand},
};
use crate::manage::{
    background::{self, Monitor},
    store::Store,
};
use futures::{StreamExt, channel::mpsc::UnboundedSender};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

/// The argument that starts this process; the data directory follows.
pub const FLAG: &str = "--background";
/// The argument that opens the window on a data directory (it follows), as
/// this process does when handing over.
pub const DATA_DIR_FLAG: &str = "--data-dir";

/// How long handing over waits for a check under way to finish.
const CHECK_WAIT: Duration = Duration::from_secs(10);

/// Runs Picacho in the background for the data directory `dir` until it is
/// opened or quit. Returns at once if another Picacho already runs there or
/// running in the background has been turned off.
pub fn run(dir: &Path) -> anyhow::Result<()> {
    let Some(listener) = instance::claim_resident(dir) else {
        return Ok(());
    };
    let store = match Store::open(dir) {
        Ok(store) => Arc::new(store),
        Err(e) => {
            listener.abandon();
            return Err(e);
        }
    };
    if !background::enabled(&store) {
        listener.abandon();
        return Ok(());
    }
    let backend = super::ui::new_backend(store.clone());
    let backend_for_notifications = backend.clone();
    let announce: background::Announce = Box::new(super::notify::announce);
    let monitor = match Monitor::start(backend, announce) {
        Ok(monitor) => monitor,
        Err(e) => {
            listener.abandon();
            return Err(e.into());
        }
    };
    let (commands, requests) = futures::channel::mpsc::unbounded::<TrayCommand>();
    // Notifications shown from here: a tap, or Reply, opens the window with
    // it; their other buttons act here (`notify::Background`).
    let opens = commands.clone();
    super::notify::set_background(super::notify::Background {
        backend: backend_for_notifications,
        open: Some(Box::new(move |target| {
            if let Ok(json) = serde_json::to_string(&target) {
                super::launch::push(format!("{}{json}", super::launch::NOTIFICATION));
            }
            let _ = opens.unbounded_send(TrayCommand::Open);
        })),
    });
    let launches = commands.clone();
    let socket = listener.serve(move || launches.unbounded_send(TrayCommand::Open).is_ok());
    let state = State {
        dir: dir.to_owned(),
        store,
        monitor,
        socket,
    };
    event_loop(state, commands, requests)
}

/// What handing over or quitting ends.
struct State {
    dir: PathBuf,
    store: Arc<Store>,
    monitor: Monitor,
    socket: PathBuf,
}

impl State {
    /// Stops checking, lets go of the socket and, for `Open`, starts the
    /// window's process, calling `before_spawn` just before (or once done,
    /// without one). The caller exits after.
    fn finish(self, command: TrayCommand, before_spawn: impl FnOnce()) {
        self.monitor.stop_and_wait(CHECK_WAIT);
        self.store.flush();
        instance::release(&self.socket);
        // What later launches (or tapped notifications) asked to open goes to
        // the window's process through a private handover file inside the data
        // directory rather than command-line arguments.
        if matches!(command, TrayCommand::Open | TrayCommand::Widget) {
            if command == TrayCommand::Widget {
                super::launch::push("picacho://widget/panel".to_owned());
            }
            let pending = super::launch::take();
            if let Err(e) = super::launch::save_pending(&self.dir, &pending) {
                eprintln!("Picacho: could not hand over pending requests: {e}");
            }
            let args: Vec<std::ffi::OsString> =
                vec![DATA_DIR_FLAG.into(), self.dir.into_os_string()];
            // A launch's token, so the window comes forward and the
            // launcher stops showing Picacho as starting once it's open.
            let activation = instance::take_activation();
            before_spawn();
            if let Err(e) = spawn(&args, activation.as_deref()) {
                eprintln!("Picacho: could not open the mail window: {e}");
            }
        } else {
            before_spawn();
        }
    }
}

/// Linux and BSD: the tray and the socket have threads of their own, so the
/// main thread only waits for a request.
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
fn event_loop(
    state: State,
    commands: UnboundedSender<TrayCommand>,
    mut requests: futures::channel::mpsc::UnboundedReceiver<TrayCommand>,
) -> anyhow::Result<()> {
    // Started at sign-in, this may come before the panel's tray.
    let tray = tray::create_patient(commands);
    let command = futures::executor::block_on(requests.next()).unwrap_or(TrayCommand::Quit);
    // Opening can take a moment (a check under way finishes first): the
    // icon pulses meanwhile, so the click is seen to have done something.
    if command != TrayCommand::Quit
        && let Some(tray) = &tray
    {
        tray.opening();
    }
    // The icon goes before the window starts: it may show one of its own.
    state.finish(command, move || drop(tray));
    Ok(())
}

/// macOS and Windows: the tray icon needs the system's event loop on the
/// main thread. On macOS the process has no Dock icon, and opening Picacho
/// from Finder or the Dock while it runs reaches it as a reopen.
#[cfg(any(target_os = "macos", windows))]
fn event_loop(
    state: State,
    commands: UnboundedSender<TrayCommand>,
    mut requests: futures::channel::mpsc::UnboundedReceiver<TrayCommand>,
) -> anyhow::Result<()> {
    use tao::{
        event::{Event, StartCause},
        event_loop::{ControlFlow, EventLoopBuilder},
    };
    #[allow(unused_mut)]
    let mut event_loop = EventLoopBuilder::<TrayCommand>::with_user_event().build();
    #[cfg(target_os = "macos")]
    {
        use tao::platform::macos::{ActivationPolicy, EventLoopExtMacOS};
        event_loop.set_activation_policy(ActivationPolicy::Accessory);
    }
    let proxy = event_loop.create_proxy();
    std::thread::Builder::new()
        .name("picacho-requests".into())
        .spawn(move || {
            while let Some(command) = futures::executor::block_on(requests.next()) {
                if proxy.send_event(command).is_err() {
                    break;
                }
            }
        })?;
    let mut state = Some(state);
    // Kept for its `Drop`, which removes the icon.
    let mut tray: Option<tray::Tray> = None;
    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        let command = match event {
            Event::NewEvents(StartCause::Init) => {
                tray = tray::create(commands.clone());
                None
            }
            #[cfg(target_os = "macos")]
            Event::Reopen { .. } => Some(TrayCommand::Open),
            #[cfg(target_os = "macos")]
            Event::Opened { urls } => {
                for url in urls {
                    if let Some(req) = super::launch::request(url.as_str()) {
                        super::launch::push(req);
                    }
                }
                Some(TrayCommand::Open)
            }
            Event::UserEvent(command) => Some(command),
            _ => None,
        };
        if let Some(command) = command {
            // The icon goes first: the window may show one of its own.
            drop(tray.take());
            if let Some(state) = state.take() {
                state.finish(command, || {});
            }
            std::process::exit(0);
        }
    })
}

/// Starts Picacho again as a process of its own with `args`: the window's
/// or this one's. It outlives the process starting it. `activation` is the
/// desktop's activation token for the window it opens (see `instance`).
fn spawn(args: &[OsString], activation: Option<&str>) -> std::io::Result<()> {
    use std::process::{Command, Stdio};
    const TOKEN: &str = "XDG_ACTIVATION_TOKEN";
    let exe = std::env::current_exe()?;
    // In a Flatpak, a child ends with the sandbox of the process that
    // started it; the portal starts it in a sandbox of its own instead.
    let mut command = if std::env::var_os("FLATPAK_ID").is_some() {
        let mut command = Command::new("flatpak-spawn");
        if let Some(token) = activation {
            command.arg(format!("--env={TOKEN}={token}"));
        }
        command.arg(exe);
        command
    } else {
        let mut command = Command::new(exe);
        if let Some(token) = activation {
            command.env(TOKEN, token);
        }
        command
    };
    // Startup tokens this process inherited belong to a launch long done.
    if activation.is_none() {
        command.env_remove(TOKEN);
    }
    command
        .env_remove("DESKTOP_STARTUP_ID")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null());
    #[cfg(unix)]
    {
        // Not ended with the terminal (or session) that started Picacho.
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
    command.spawn().map(drop)
}

/// Starts the background process for `dir`, as the window's process quits.
pub(super) fn start(dir: &Path) -> std::io::Result<()> {
    spawn(&[FLAG.into(), dir.as_os_str().to_owned()], None)
}

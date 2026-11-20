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
    let announce: background::Announce = Box::new(super::notify::announce);
    let monitor = match Monitor::start(backend, announce) {
        Ok(monitor) => monitor,
        Err(e) => {
            listener.abandon();
            return Err(e.into());
        }
    };
    let (commands, requests) = futures::channel::mpsc::unbounded::<TrayCommand>();
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
    /// window's process. The caller exits after.
    fn finish(self, command: TrayCommand) {
        self.monitor.stop_and_wait(CHECK_WAIT);
        self.store.flush();
        instance::release(&self.socket);
        if command == TrayCommand::Open
            && let Err(e) = spawn(&[DATA_DIR_FLAG.into(), self.dir.into_os_string()])
        {
            eprintln!("Picacho: could not open the mail window: {e}");
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
    let tray = tray::create(commands);
    let command = futures::executor::block_on(requests.next()).unwrap_or(TrayCommand::Quit);
    // The icon goes first: the window may show one of its own.
    drop(tray);
    state.finish(command);
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
            Event::UserEvent(command) => Some(command),
            _ => None,
        };
        if let Some(command) = command {
            // The icon goes first: the window may show one of its own.
            drop(tray.take());
            if let Some(state) = state.take() {
                state.finish(command);
            }
            std::process::exit(0);
        }
    })
}

/// Starts Picacho again as a process of its own with `args`: the window's
/// or this one's. It outlives the process starting it.
fn spawn(args: &[OsString]) -> std::io::Result<()> {
    use std::process::{Command, Stdio};
    let exe = std::env::current_exe()?;
    // In a Flatpak, a child ends with the sandbox of the process that
    // started it; the portal starts it in a sandbox of its own instead.
    let mut command = if std::env::var_os("FLATPAK_ID").is_some() {
        let mut command = Command::new("flatpak-spawn");
        command.arg(exe);
        command
    } else {
        Command::new(exe)
    };
    command
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
    spawn(&[FLAG.into(), dir.as_os_str().to_owned()])
}

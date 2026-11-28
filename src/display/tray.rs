//! The tray icon: in the notification area (Windows), the menu bar (macOS),
//! or as a StatusNotifierItem (Linux and BSD: KDE, and GNOME with the
//! AppIndicator extension). It shows while Picacho runs in the background,
//! and while a window is open too when Settings asks for it (see `shell`).
//! Its menu opens Picacho's window or quits. On Linux and BSD it also says
//! how much unread inbox mail there is (the workspace's, `workspaces`) and
//! switches workspaces, from the widget snapshots Picacho publishes.
//!
//! A tray that can't be shown (no tray on this desktop) is no error:
//! `create` returns `None` and Picacho runs on without one. Dropping the
//! returned `Tray` removes the icon.
use futures::channel::mpsc::UnboundedSender;

/// What the tray asks of the app.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum TrayCommand {
    /// Show Picacho's window, opening it if it is closed.
    Open,
    /// Open or focus Picacho's compact Desktop Widget window.
    Widget,
    /// Turn a workspace on (`None`: everything).
    #[cfg_attr(not(any(target_os = "linux", target_os = "freebsd")), allow(dead_code))]
    Workspace(Option<String>),
    Quit,
}

const TOOLTIP: &str = "Picacho";

/// What the tray shows of the mail, from the latest widget snapshot.
#[derive(Clone, Debug, Default, PartialEq)]
#[cfg_attr(not(any(target_os = "linux", target_os = "freebsd")), allow(dead_code))]
struct Status {
    /// Unread inbox mail (the workspace's).
    unread: usize,
    /// The workspace on; empty for everything.
    workspace: String,
    /// Every workspace: (id, name, on).
    workspaces: Vec<(String, String, bool)>,
}
impl Status {
    fn of(snapshot: &crate::manage::widgets::Snapshot) -> Self {
        Self {
            unread: snapshot.total_unread,
            workspace: snapshot.workspace.clone(),
            workspaces: snapshot
                .workspaces
                .iter()
                .map(|w| (w.id.clone(), w.name.clone(), w.on))
                .collect(),
        }
    }
    /// The tooltip's words: how much is unread, and in which workspace.
    #[cfg_attr(not(any(target_os = "linux", target_os = "freebsd")), allow(dead_code))]
    fn description(&self) -> String {
        let unread = if self.unread == 0 {
            t!("tray-no-unread").to_owned()
        } else {
            t!("tray-unread", count = self.unread)
        };
        if self.workspace.is_empty() {
            unread
        } else {
            t!("tray-in-workspace", unread = unread, name = self.workspace.clone())
        }
    }
}

static STATUS: std::sync::Mutex<Option<Status>> = std::sync::Mutex::new(None);

/// What the tray shows now (Linux's; the others show no counts).
#[cfg_attr(not(any(target_os = "linux", target_os = "freebsd")), allow(dead_code))]
fn status() -> Status {
    STATUS.lock().unwrap_or_else(|e| e.into_inner()).clone().unwrap_or_default()
}

/// Shows `snapshot`'s counts and workspaces in the tray.
fn show(snapshot: &crate::manage::widgets::Snapshot) {
    let next = Status::of(snapshot);
    let mut status = STATUS.lock().unwrap_or_else(|e| e.into_inner());
    if status.as_ref() != Some(&next) {
        *status = Some(next);
        drop(status);
        platform::refresh();
    }
}

/// Keeps the tray's counts up to date with the snapshots `store`'s
/// process publishes, starting from the one last written. Once per process.
pub(super) fn follow(store: &crate::manage::store::Store) {
    use futures::StreamExt;
    static FOLLOWING: std::sync::Once = std::sync::Once::new();
    if let Some(snapshot) = crate::manage::widgets::read_cached(&store.path) {
        show(&snapshot);
    }
    FOLLOWING.call_once(|| {
        let mut updates = crate::manage::widgets::subscribe();
        let _ = std::thread::Builder::new()
            .name("picacho-tray-status".into())
            .spawn(move || {
                while let Some(snapshot) = futures::executor::block_on(updates.next()) {
                    show(&snapshot);
                }
            });
    });
}

/// The app icon as RGBA pixels, `size` pixels square.
fn icon_rgba(size: u32) -> Option<Vec<u8>> {
    const ICON: &[u8] = include_bytes!("../../assets/brand/icon.png");
    let image = image::load_from_memory_with_format(ICON, image::ImageFormat::Png).ok()?;
    let image = image.resize_exact(size, size, image::imageops::FilterType::Lanczos3);
    Some(image.into_rgba8().into_raw())
}

pub(super) use platform::{Tray, create, relabel};
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
pub(super) use platform::create_patient;

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
mod platform {
    use super::*;
    use ksni::blocking::TrayMethods;

    /// The icon's D-Bus service runs on a thread of ksni's; this handle
    /// ends it when dropped.
    pub(in crate::display) struct Tray {
        handle: ksni::blocking::Handle<Sni>,
    }

    impl Tray {
        /// Shows that Picacho's window is opening: the icon pulses (KDE
        /// Plasma draws items that need attention so) until it goes. Off the
        /// caller's thread: the bus must never hold up opening the window.
        pub(in crate::display) fn opening(&self) {
            let handle = self.handle.clone();
            std::thread::spawn(move || {
                handle.update(|sni| sni.opening = true);
            });
        }
    }

    impl Drop for Tray {
        fn drop(&mut self) {
            // Asks the service to end without waiting for it: the bus must
            // never hold up the window.
            drop(self.handle.shutdown());
            *SHOWN.lock().unwrap_or_else(|e| e.into_inner()) = None;
        }
    }

    /// The icon showing, for `relabel`.
    static SHOWN: std::sync::Mutex<Option<ksni::blocking::Handle<Sni>>> =
        std::sync::Mutex::new(None);

    /// Sends the menu again, in the language now shown (`menu` words it as
    /// it's asked). Off the main thread: the bus must never hold it up.
    pub(in crate::display) fn relabel() {
        if let Some(handle) = SHOWN.lock().unwrap_or_else(|e| e.into_inner()).clone() {
            std::thread::spawn(move || {
                handle.update(|_| {});
            });
        }
    }

    /// Sends the tooltip and menu again, with the counts now known.
    pub(super) fn refresh() {
        relabel();
    }

    pub(in crate::display) struct Sni {
        commands: UnboundedSender<TrayCommand>,
        icons: Vec<ksni::Icon>,
        /// The window is opening (see `Tray::opening`).
        opening: bool,
    }

    impl Sni {
        fn send(&self, command: TrayCommand) {
            let _ = self.commands.unbounded_send(command);
        }
        /// Opens the window (or the widget) with the activation token the
        /// tray gave this click, so it comes to the front on Wayland.
        fn open(&self, command: TrayCommand) {
            if let Some(token) = click_token() {
                crate::display::instance::offer_activation(token);
            }
            self.send(command);
        }
    }

    /// The tray's activation token for the click being handled. KDE Plasma
    /// sends it just before the click; others may send it a moment after,
    /// or never.
    fn click_token() -> Option<String> {
        for _ in 0..25 {
            if let Some(token) = ksni::current_xdg_activation_token() {
                return Some(token);
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        None
    }

    impl ksni::Tray for Sni {
        fn id(&self) -> String {
            crate::display::desktop::APP_ID.into()
        }
        fn title(&self) -> String {
            TOOLTIP.into()
        }
        fn icon_pixmap(&self) -> Vec<ksni::Icon> {
            self.icons.clone()
        }
        fn status(&self) -> ksni::Status {
            if self.opening {
                ksni::Status::NeedsAttention
            } else {
                ksni::Status::Active
            }
        }
        fn attention_icon_pixmap(&self) -> Vec<ksni::Icon> {
            self.icons.clone()
        }
        fn tool_tip(&self) -> ksni::ToolTip {
            ksni::ToolTip {
                title: TOOLTIP.into(),
                description: status().description(),
                ..Default::default()
            }
        }
        fn activate(&mut self, _x: i32, _y: i32) {
            self.open(TrayCommand::Open);
        }
        fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
            use ksni::menu::{MenuItem, RadioGroup, RadioItem, StandardItem, SubMenu};
            let status = status();
            // The workspaces, to switch between, once there are any.
            let workspaces: Vec<MenuItem<Self>> = if status.workspaces.is_empty() {
                vec![]
            } else {
                let mut ids: Vec<Option<String>> = vec![None];
                let mut options = vec![RadioItem {
                    label: t!("workspaces-everything").replace('_', "__"),
                    ..Default::default()
                }];
                let mut selected = 0;
                for (i, (id, name, on)) in status.workspaces.iter().enumerate() {
                    ids.push(Some(id.clone()));
                    options.push(RadioItem {
                        label: name.replace('_', "__"),
                        ..Default::default()
                    });
                    if *on {
                        selected = i + 1;
                    }
                }
                vec![
                    SubMenu {
                        label: t!("tray-workspace").into(),
                        submenu: vec![
                            RadioGroup {
                                selected,
                                select: Box::new(move |this: &mut Self, i: usize| {
                                    if let Some(id) = ids.get(i) {
                                        this.send(TrayCommand::Workspace(id.clone()));
                                    }
                                }),
                                options,
                            }
                            .into(),
                        ],
                        ..Default::default()
                    }
                    .into(),
                    MenuItem::Separator,
                ]
            };
            workspaces.into_iter().chain([
                StandardItem {
                    label: t!("tray-open-picacho").into(),
                    activate: Box::new(|this: &mut Self| this.open(TrayCommand::Open)),
                    ..Default::default()
                }
                .into(),
                StandardItem {
                    label: t!("tray-desktop-widget").into(),
                    activate: Box::new(|this: &mut Self| this.open(TrayCommand::Widget)),
                    ..Default::default()
                }
                .into(),
                MenuItem::Separator,
                StandardItem {
                    label: t!("tray-quit-picacho").into(),
                    icon_name: "application-exit".into(),
                    activate: Box::new(|this: &mut Self| this.send(TrayCommand::Quit)),
                    ..Default::default()
                }
                .into(),
            ])
            .collect()
        }
    }

    /// Registers the icon with the desktop's tray. Talks to the session
    /// bus, so it runs off the main thread (see `shell`).
    pub(in crate::display) fn create(commands: UnboundedSender<TrayCommand>) -> Option<Tray> {
        spawn(commands, false)
    }

    /// As `create`, for the background process, which may start with the
    /// session (Settings → General), before the panel's tray is up: the
    /// icon then appears once a tray does, rather than never.
    pub(in crate::display) fn create_patient(
        commands: UnboundedSender<TrayCommand>,
    ) -> Option<Tray> {
        spawn(commands, true)
    }

    fn spawn(commands: UnboundedSender<TrayCommand>, patient: bool) -> Option<Tray> {
        let icons = [32, 64]
            .into_iter()
            .filter_map(|size| {
                let mut data = icon_rgba(size)?;
                // StatusNotifierItem wants ARGB, most significant byte first.
                for pixel in data.as_chunks_mut::<4>().0 {
                    pixel.rotate_right(1);
                }
                Some(ksni::Icon {
                    width: size as i32,
                    height: size as i32,
                    data,
                })
            })
            .collect();
        let tray = Sni {
            commands,
            icons,
            opening: false,
        };
        // A sandbox may not own the well-known name the specification
        // asks for; trays find the icon without it.
        let sandboxed =
            std::env::var_os("FLATPAK_ID").is_some() || std::env::var_os("SNAP").is_some();
        match tray
            .disable_dbus_name(sandboxed)
            .assume_sni_available(patient)
            .spawn()
        {
            Ok(handle) => {
                *SHOWN.lock().unwrap_or_else(|e| e.into_inner()) = Some(handle.clone());
                Some(Tray { handle })
            }
            Err(e) => {
                eprintln!("Picacho: no tray icon: {e}");
                None
            }
        }
    }
}

#[cfg(any(target_os = "macos", windows))]
mod platform {
    use super::*;
    use std::sync::{Mutex, Once};
    use tray_icon::{
        Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent,
        menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
    };

    const OPEN_ID: &str = "picacho-open";
    const WIDGET_ID: &str = "picacho-widget";
    const QUIT_ID: &str = "picacho-quit";

    /// Where the tray's events go. tray-icon takes its event handlers only
    /// once per process, so they read the current sender from here.
    static COMMANDS: Mutex<Option<UnboundedSender<TrayCommand>>> = Mutex::new(None);

    fn send(command: TrayCommand) {
        if let Some(commands) = COMMANDS.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            let _ = commands.unbounded_send(command);
        }
    }

    /// Owns the system's icon, which `TrayIcon` removes when dropped.
    pub(in crate::display) struct Tray {
        _icon: TrayIcon,
    }

    thread_local! {
        /// The menu's items and their words, for `relabel` (on the main
        /// thread, where the menu lives).
        static ITEMS: std::cell::RefCell<Vec<(MenuItem, fn() -> &'static str)>> =
            const { std::cell::RefCell::new(Vec::new()) };
    }

    /// The counts aren't shown here: the icon's tooltip stays "Picacho".
    pub(super) fn refresh() {}

    /// Words the menu in the language now shown.
    pub(in crate::display) fn relabel() {
        ITEMS.with_borrow(|items| {
            for (item, words) in items {
                item.set_text(words());
            }
        });
    }

    /// Adds the icon. On the main thread, with the app's event loop running.
    pub(in crate::display) fn create(commands: UnboundedSender<TrayCommand>) -> Option<Tray> {
        static HANDLERS: Once = Once::new();
        *COMMANDS.lock().unwrap_or_else(|e| e.into_inner()) = Some(commands);
        HANDLERS.call_once(|| {
            MenuEvent::set_event_handler(Some(|event: MenuEvent| {
                if event.id == OPEN_ID {
                    send(TrayCommand::Open);
                } else if event.id == WIDGET_ID {
                    send(TrayCommand::Widget);
                } else if event.id == QUIT_ID {
                    send(TrayCommand::Quit);
                }
            }));
            // Windows: a click on the icon opens the window; the menu is on
            // the right button. macOS shows the menu for any click.
            TrayIconEvent::set_event_handler(Some(|event: TrayIconEvent| {
                if cfg!(windows)
                    && let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                {
                    send(TrayCommand::Open);
                }
            }));
        });
        let menu = Menu::new();
        let open = MenuItem::with_id(OPEN_ID, t!("tray-open-picacho"), true, None);
        let widget = MenuItem::with_id(WIDGET_ID, t!("tray-desktop-widget"), true, None);
        let quit = MenuItem::with_id(QUIT_ID, t!("tray-quit-picacho"), true, None);
        ITEMS.set(vec![
            (open.clone(), || t!("tray-open-picacho")),
            (widget.clone(), || t!("tray-desktop-widget")),
            (quit.clone(), || t!("tray-quit-picacho")),
        ]);
        let built = menu
            .append(&open)
            .and_then(|_| menu.append(&widget))
            .and_then(|_| menu.append(&PredefinedMenuItem::separator()))
            .and_then(|_| menu.append(&quit));
        if let Err(e) = built {
            eprintln!("Picacho: no tray menu: {e}");
            return None;
        }
        let size = 64;
        let icon = icon_rgba(size).and_then(|rgba| Icon::from_rgba(rgba, size, size).ok())?;
        let built = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_icon(icon)
            .with_tooltip(TOOLTIP)
            .with_menu_on_left_click(cfg!(target_os = "macos"))
            .build();
        match built {
            Ok(icon) => Some(Tray { _icon: icon }),
            Err(e) => {
                eprintln!("Picacho: no tray icon: {e}");
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manage::widgets::{Snapshot, WidgetWorkspace};

    #[test]
    fn the_tooltip_counts_the_workspaces_unread_mail() {
        let mut snap = Snapshot::empty("green", 0);
        assert_eq!(Status::of(&snap).description(), "No unread mail");
        snap.total_unread = 3;
        assert_eq!(Status::of(&snap).description(), "3 unread messages");
        snap.workspace = "Office".into();
        snap.workspaces = vec![WidgetWorkspace {
            id: "w".into(),
            name: "Office".into(),
            on: true,
            unread: 3,
        }];
        let status = Status::of(&snap);
        assert_eq!(status.description(), "3 unread messages in Office");
        assert_eq!(status.workspaces, [("w".to_owned(), "Office".to_owned(), true)]);
    }
}

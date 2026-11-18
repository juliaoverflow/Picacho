//! The tray icon: in the notification area (Windows), the menu bar (macOS),
//! or as a StatusNotifierItem (Linux and BSD: KDE, and GNOME with the
//! AppIndicator extension). It shows while Picacho runs in the background,
//! and while a window is open too when Settings asks for it (see `shell`).
//! Its menu opens Picacho's window or quits.
//!
//! A tray that can't be shown (no tray on this desktop) is no error:
//! `create` returns `None` and Picacho runs on without one. Dropping the
//! returned `Tray` removes the icon.
use futures::channel::mpsc::UnboundedSender;

/// What the tray asks of the app.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TrayCommand {
    /// Show Picacho's window, opening it if it is closed.
    Open,
    Quit,
}

const TOOLTIP: &str = "Picacho";
const OPEN_LABEL: &str = "Open Picacho";
const QUIT_LABEL: &str = "Quit Picacho";

/// The app icon as RGBA pixels, `size` pixels square.
fn icon_rgba(size: u32) -> Option<Vec<u8>> {
    const ICON: &[u8] = include_bytes!("../../assets/brand/icon.png");
    let image = image::load_from_memory_with_format(ICON, image::ImageFormat::Png).ok()?;
    let image = image.resize_exact(size, size, image::imageops::FilterType::Lanczos3);
    Some(image.into_rgba8().into_raw())
}

pub(super) use platform::{Tray, create};

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
mod platform {
    use super::*;
    use ksni::blocking::TrayMethods;

    /// The icon's D-Bus service runs on a thread of ksni's; this handle
    /// ends it when dropped.
    pub(in crate::display) struct Tray {
        handle: ksni::blocking::Handle<Sni>,
    }

    impl Drop for Tray {
        fn drop(&mut self) {
            // Asks the service to end without waiting for it: the bus must
            // never hold up the window.
            drop(self.handle.shutdown());
        }
    }

    pub(in crate::display) struct Sni {
        commands: UnboundedSender<TrayCommand>,
        icons: Vec<ksni::Icon>,
    }

    impl Sni {
        fn send(&self, command: TrayCommand) {
            let _ = self.commands.unbounded_send(command);
        }
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
        fn tool_tip(&self) -> ksni::ToolTip {
            ksni::ToolTip {
                title: TOOLTIP.into(),
                ..Default::default()
            }
        }
        fn activate(&mut self, _x: i32, _y: i32) {
            self.send(TrayCommand::Open);
        }
        fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
            use ksni::menu::{MenuItem, StandardItem};
            vec![
                StandardItem {
                    label: OPEN_LABEL.into(),
                    activate: Box::new(|this: &mut Self| this.send(TrayCommand::Open)),
                    ..Default::default()
                }
                .into(),
                MenuItem::Separator,
                StandardItem {
                    label: QUIT_LABEL.into(),
                    icon_name: "application-exit".into(),
                    activate: Box::new(|this: &mut Self| this.send(TrayCommand::Quit)),
                    ..Default::default()
                }
                .into(),
            ]
        }
    }

    /// Registers the icon with the desktop's tray. Talks to the session
    /// bus, so it runs off the main thread (see `shell`).
    pub(in crate::display) fn create(commands: UnboundedSender<TrayCommand>) -> Option<Tray> {
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
        let tray = Sni { commands, icons };
        // A sandbox may not own the well-known name the specification
        // asks for; trays find the icon without it.
        let sandboxed =
            std::env::var_os("FLATPAK_ID").is_some() || std::env::var_os("SNAP").is_some();
        match tray.disable_dbus_name(sandboxed).spawn() {
            Ok(handle) => Some(Tray { handle }),
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

    /// Adds the icon. On the main thread, with the app's event loop running.
    pub(in crate::display) fn create(commands: UnboundedSender<TrayCommand>) -> Option<Tray> {
        static HANDLERS: Once = Once::new();
        *COMMANDS.lock().unwrap_or_else(|e| e.into_inner()) = Some(commands);
        HANDLERS.call_once(|| {
            MenuEvent::set_event_handler(Some(|event: MenuEvent| {
                if event.id == OPEN_ID {
                    send(TrayCommand::Open);
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
        let open = MenuItem::with_id(OPEN_ID, OPEN_LABEL, true, None);
        let quit = MenuItem::with_id(QUIT_ID, QUIT_LABEL, true, None);
        let built = menu
            .append(&open)
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

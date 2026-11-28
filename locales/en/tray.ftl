# src/display/tray.rs: the menu of Picacho's icon in the system tray.

tray-open-picacho = Open Picacho

tray-desktop-widget = Desktop Widget

tray-quit-picacho = Quit Picacho

tray-workspace = Workspace

tray-no-unread = No unread mail

tray-unread =
    { $count ->
        [one] { $count } unread message
       *[other] { $count } unread messages
    }

tray-in-workspace = { $unread } in { $name }

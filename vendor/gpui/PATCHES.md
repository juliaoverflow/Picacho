# GPUI patches

Source: `gpui` 0.2.2 from crates.io, https://github.com/zed-industries/zed.
License: Apache-2.0. The examples and their assets are left out.

## Copying from a click on Wayland

`write_to_clipboard` and `write_to_primary` in
`src/platform/linux/wayland/client.rs`. Upstream set the selection with the
serial of the last key press. A copy made by clicking (a context menu's
"Copy Link Address") then carried an old serial, or 0 when no key had been
pressed, and compositors such as Mutter ignore a selection whose serial is
older than the current selection's: nothing was copied. The serial is now
that of the latest key press or mouse button (`SerialTracker::latest` in
`src/platform/linux/wayland/serial.rs`).

Remove when upstream uses the latest input serial for selections.

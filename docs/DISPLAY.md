# Display development

Display contains GPUI events and views. Shared state/actions live in `ui.rs`; settings and reader rendering live in `settings.rs` and `reader.rs`. Keep protocol behavior in Sync and persisted application state in Manage.

## Supported window sizes

The native window minimum is 360 × 320 logical pixels. Layout adapts to the window's actual viewport, without writing automatic layout decisions into the user's sidebar preference:

- At 1180 pixels and wider, show the normal sidebar, message list and reader.
- Below 1180, use the sidebar rail and a narrower message list; reader actions use icons with tooltips.
- Below 900, show the list or the selected reader. The reader has Back navigation; Escape also returns to the list. Below 480, hide the rail while reading to give the message full width.
- Below 700, settings uses a horizontal, scrollable account-navigation row. Wider settings keeps a vertical sidebar. The settings page scrolls independently.

Dialogs stay inside the viewport, with a fixed close/header region and scrollable content. Sync choices wrap into rows, while bulk controls and compose actions wrap when needed. Onboarding uses a full-width card with a maximum width and scrolls on short windows.

## Locked messages

Locked and readable messages share the same reader shell: toolbar, flex sizing, scroll handle and background. Add content inside that shell rather than returning early from a locked-message branch. The password panel has an explicit width bounded by the reader, an input label, inline errors and a per-account unlock-in-progress state. Enter submits; blank input gives feedback and focuses the field. Duplicate submissions are ignored while an unlock runs. Clear the input on submission, mailbox navigation and Back navigation, and zeroize the worker's password string when it is finished.

User-facing failures show the top-level actionable message. The underlying error chain remains available to callers in Manage/Sync; database driver paths and platform error chains should not crowd a dialog.

## Verification

`responsive_tests.rs` runs GPUI's layout engine at 1440×1000, 1180×700, 960×600, 899×600, 700×480, 640×480, 480×420 and 360×320. It checks viewport bounds, wrapping choices, scroll reachability, locked-reader controls, focus feedback and narrow-window navigation. Separate checks cover bulk selection, compose, onboarding and Add Account.

These are layout/interaction checks rather than pixel snapshots. Native Linux settings were also inspected under Xvfb with Mesa software rendering at 1440×1000, 640×480 and 360×320, using synthetic accounts in an isolated profile. The encrypted-reader layout is covered by GPUI tests; live Pinal sign-in and native password unlocking require a real account/keychain and were not performed. macOS/Windows rendering needs platform-specific follow-up when changing GPUI or fonts.

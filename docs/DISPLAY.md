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

Background mailbox failures appear as a red sidebar warning; the account settings
page contains operation-specific details and recovery actions. General settings
and global notices do not repeat those failures. Explicit sign-in/send validation
stays in its form, and closing a form clears its feedback. The mailbox details
view exposes the underlying error chain for diagnosis; ordinary controls keep
their short actionable messages.

## Verification

`responsive_tests.rs` runs GPUI's layout engine at 1440×1000, 1180×700, 960×600, 899×600, 700×480, 640×480, 480×420 and 360×320. It checks viewport bounds, wrapping choices, scroll reachability, locked-reader controls, focus feedback and narrow-window navigation. Separate checks cover bulk selection, compose, onboarding and Add Account.

These are layout/interaction checks rather than pixel snapshots. Native Linux settings were also inspected under Xvfb with Mesa software rendering at 1440×1000, 640×480 and 360×320, using synthetic accounts in an isolated profile. The encrypted-reader layout is covered by GPUI tests; live Pinal sign-in and native password unlocking require a real account/keychain and were not performed. macOS/Windows rendering needs platform-specific follow-up when changing GPUI or fonts.

### HTML email component

The reader calls `html::HtmlView` with decoded HTML and a remote-content policy.
Blitz owns CSS layout and DOM interaction; GPUI owns the surrounding reader,
scrolling, controls and settings. The renderer has no mailbox or Store dependency.
It paints only the visible strip, applies DPI scaling to vector commands, caches
the resulting tile and retires replaced atlas images. Leaving a message, turning
HTML off or blocking remote content discards the document and cancels queued
resource work. Clicks, hover, drag selection/copy, Tab and Enter/Space are bridged
to the document; scripts, forms and embedded frames are disabled.

The wrapping controls above each body save choices for that account/message.
Global Reading settings supply HTML-on/remote-blocked defaults to emails without
an override. Messages without an HTML part retain the selectable plain reader.

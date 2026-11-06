# Display development

Display contains GPUI events and views. Shared state/actions live in `ui.rs`; settings and reader rendering live in `settings.rs` and `reader.rs`. Keep protocol behavior in Sync and persisted application state in Manage.

## Supported window sizes

The native window minimum is 360 × 320 logical pixels. Layout adapts to the window's actual viewport, without writing automatic layout decisions into the user's sidebar preference:

- At 1180 pixels and wider, show the normal sidebar, message list and reader.
- Below 1180, use the sidebar rail and a narrower message list; reader actions use icons with tooltips.
- Below 900, show the list or the selected reader. The reader has Back navigation; Escape also returns to the list. Below 480, hide the rail while reading to give the message full width.
- Below 700, settings uses a horizontal, scrollable account-navigation row. Wider settings keeps a vertical sidebar. The settings page scrolls independently.

Dialogs stay inside the viewport, with a fixed close/header region and scrollable content. Sync choices wrap into rows, while bulk controls and compose actions wrap when needed. Onboarding uses a full-width card with a maximum width and scrolls on short windows.

## Panes

From 900 pixels up, the sidebar and reader edges can be dragged. The message
list keeps at least 300 pixels; widths are saved. Dragging a pane past half
of its smallest useful width (190 for the sidebar, 528 for the reader: enough for the email's controls row on one line) snaps
it into a 60-pixel rail, and dragging back out reopens it. The sidebar toggle
sits beside Compose (above it in the rail); the reader's toggle is at the
left of its toolbar. Collapsing crossfades the full pane and its rail while
the width animates. The open message's entrance fade runs only when a
message opens, never when the reader is rebuilt at the end of that animation.
The reader's ribbon offers message actions above a preview of the open
message: an HTML email is rendered in miniature, its content column scaled
to the ribbon's width (a long one fades out at the bottom); a plain text one
is a minimap of its subject and lines. Clicking a place in the preview opens
the reader scrolled there. The preview loads the email, so the reader opens
with it already rendered.

Debug macOS builds include a driver for visual checks: run with
`PICACHO_DEV_DIR=<dir>` and append commands to `<dir>/cmd` (see
`devdriver.rs`). It captures only the app's own window.

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
Each document lives on its own thread (`html/worker.rs`): parsing, styling,
layout, input and rasterization never run on the UI thread. The view caches
512-pixel strips per document revision, so scrolling only draws cached images;
an older strip stays up until its replacement arrives, and strips far from the
viewport are released. A spinner shows until the first layout and paint.
Leaving a message, turning HTML off or blocking remote content ends the thread
and cancels queued resource work. Clicks, hover (including author `:hover`
styles and CSS cursors), drag selection/copy, Tab and Enter/Space are bridged
to the document; a secondary click offers Copy, link and image actions, Copy
Message Text and Show Original. Scripts, forms and embedded frames are disabled.
Development builds optimize the rendering crates (see `Cargo.toml`), since
unoptimized they are roughly 50 times slower.

One row above each body, never wrapping, holds a light/dark switch on the
left and the Formatted / Plain text and Remote images switches on the right;
they save choices for that account/message. A narrow window's full-width
reader shows them as icons. Global Reading settings supply HTML-on,
remote-blocked and email-color (Light, Dark or Match app) defaults to emails
without an override; choices saved before email colors existed follow the
default color. Messages without an HTML part retain the selectable plain
reader, on a page of the email's color when it differs from the app's.

Blocked remote images keep their size as a dashed outline, so the sender's
layout holds; images without a size get a small one, and tracking pixels stay
invisible. Dark email colors use the email's own `prefers-color-scheme: dark`
styles when it has them. Otherwise the painted scene is recolored before
rasterizing (OKLab lightness, hue and chroma kept): grey and pale surfaces
darken, dark text lightens, saturated brand surfaces and the light text on
them are kept slightly dimmed, and images are untouched.

The "to …" line under the sender opens the message's details: addresses,
mailed-by (the SPF envelope domain), signed-by (DKIM domains), transport
security (TLS on the last hop, or end-to-end encryption) and SPF/DKIM/DMARC
verdicts from the receiving server's Authentication-Results. Show Original
opens the raw source with those facts, Copy and Download (.eml).

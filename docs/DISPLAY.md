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

## Sidebar arrangement

The sidebar is a tree of accounts, folders and separators (`tree.rs`), saved
as the `sidebar_tree` preference and reconciled with the saved accounts on
start and whenever one is added or removed. Each account and folder keeps its
own open/closed state (`collapsed_accounts`); the rail uses the same state,
so collapsing the sidebar never opens or closes an account. Clicking an
account or folder toggles it; its chevron turns and its contents slide in.

Add… (the + row, or + in the rail) and a right-click on empty sidebar space
offer Add Account, New Folder and New Separator. A new folder starts with
its name selected; double-click or Rename… edits it, Enter or clicking away
saves and Escape cancels. Deleting a folder keeps its contents in its place.

Every entry can be dragged. The upper or lower half of a row places it
before or after (a line with a dot marks the spot); the middle of a folder
drops into it (the folder is outlined), and holding there opens a closed
folder. To the left of a folder's last entry, a drop leaves the folder. The
dragged entry dims, a pill follows the pointer, the list scrolls near its
edges and the moved entry flashes where it landed. A folder never goes
inside itself.

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

## Frame budget

GPUI redraws the whole window whenever the app view is notified, and every
`with_animation` frame notifies it. Keep the app quiet when nothing changes:

- Never use repeating animations. `spinner()` turns in steps on a slow
  clock (`SPIN_STEP`) and schedules its own next tick only while one is drawn.
- Rendering must not touch SQLite or copy message bodies: use
  `selected_ref()`, the cached `reading_options`, and `loaded_unread`.
- Background loops coalesce their UI updates (`refresh_counts` runs at most
  one follow-up per interval and notifies only when counts change).
- Placeholder rows the server did not list are remembered in `unfillable`
  so the list does not request them again on every frame.
- GPUI's `debug_bounds` keeps a selector's last bounds after its element is
  gone; tests that assert absence should check state instead.

## Writing mail

`Composer` (`composer.rs`) is one editor entity shown four ways: a floating
panel at the lower right (drag its header to move it; minimize it to a bar;
full size fills the main window), an inline reply below the message it
answers, or a window of its own. Header buttons, top right: Save draft,
Minimize, Full size, Open in new window, Close (saves). Moving to or from a
window reopens the editor there with the same draft; within the main window
it only changes placement. `compose.rs` holds the app's side: what a new
message, reply, reply all or forward starts with, placement from the
Composing settings, and sending.

Recipients have To, Cc and Bcc (Cc and Bcc appear on demand) and suggest
people from loaded mail. With several accounts, From picks the sender.
Formatted messages are Markdown underneath: the toolbar and shortcuts (⌘B,
⌘I, ⌘⇧X, ⌘E, ⌘K, ⌘⇧8, ⌘⇧7, ⌘⇧9, ⌘⌥1) wrap or prefix the selection, ⌘⇧P
previews, and sending adds an HTML version (`smtp::html_body`, raw HTML
escaped). Files can be dropped on the editor. Drafts save on their own
after a pause, off the main thread, and at once on ⌘S, close and quit.

Sending checks recipients, then optionally asks about a missing subject or
an attachment the text mentions. The message then waits out the Undo delay
(Composing settings) in the outbox with a "Sending… Undo" toast; Undo or a
failure brings the editor back with its draft.

Settings → Composing: reply below the message or in a compose window, where
new messages open, formatted or plain, reply all by default, quoting, undo
delay, sending checks and a signature.

## Conversations

The reader shows the open message's conversation from the loaded list
(`thread.rs`): messages join by References, or by subject (prefixes like
Re:/Fwd:/AW: removed) when at least one of them is a reply, so newsletters
that reuse a subject stay apart. Earlier messages are cards above the
message, later ones below; a long history folds its middle into "N more".
The sender line reads "to Ana, Bo, me · cc Cy +2". A reply's quoted history
(plain text after "On … wrote:" and similar, or Gmail/Apple/Outlook/
Thunderbird quote markup in HTML) is folded behind a "•••" toggle. All of
this is cached per message and list revision, never recomputed per frame.

## Windows

A message opens in a window of its own from its context menu, the reader's
toolbar or ⌘O (`message_window.rs`); it reads like the reader, and its
Reply, Reply all and Forward open compose windows. Composers move to a
window with "Open in new window". Detached windows use the "Detached" key
context: ⌘W closes them (a composer saves first) and Escape closes a
message window.

## Account pictures

Clicking an account's picture (or Change Picture in its settings, or
Change Picture… in its sidebar menu) opens a menu: Load from Google (Gmail
browser sign-ins) or Gravatar (only the SHA-256 of the address is sent),
Choose from Presets…, Upload Your Own…, and Remove Picture. Presets are
CC0 photographs bundled in `assets/avatars` (sources in its README) and
listed in `assets.rs`; the picker returns to Settings when opened from it.

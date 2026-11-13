# Using Picacho

## Run

The ready-to-open macOS application is **`dist/Picacho.app`**.

```sh
open dist/Picacho.app
```

Or run from source:

```sh
cargo run --release --locked
```

With no accounts, Picacho opens a short setup: choose your provider, sign in, and your mail starts syncing. **Add Account** (in the sidebar, Settings, or the File menu) shows the same setup in a dialog.

A fictional sample inbox is available for development and screenshots. It is compiled out by default; enable it with `cargo run --features sample-data` and choose **Explore a sample inbox** on the welcome screen.

## Sign-in and connection security

Browser sign-in finds out which address was chosen from the provider, so the address is never typed.

For another provider, settings discovery follows Thunderbird's order: the domain's autoconfig file, Mozilla's ISPDB, the domain's MX records (which detect Google Workspace and Microsoft 365 custom domains, then use those services' sign-in), and common hostnames such as `imap.<domain>`.

Incoming connections use implicit TLS with certificate and hostname verification. The one exception is Proton Mail Bridge, which only runs on this computer: Picacho upgrades its connection with STARTTLS and still verifies the certificate, so Bridge's certificate must be trusted (Bridge offers to install it on macOS). SMTP requires STARTTLS, or implicit TLS on port 465. Plaintext connections and certificate bypasses are not offered.

## Settings

**Settings** (⌘, on macOS, Ctrl+, elsewhere) has a General page for light, dark, or automatic appearance and the theme color, a global **Reading** page for HTML rendering and remote-content defaults, and a page per mailbox for:

- **Keep mail from**: 2 weeks, 1 month, 3 months, 1 year, or all mail. Older mail stays on the server and leaves the local cache at the next sync. (IMAP accounts.)
- **Check for new mail**: every 1, 5, 15, 30, or 60 minutes, or manually.
- **Notifications**: a system notification for new inbox mail while Picacho is in the background.

A red warning beside a mailbox (also in the collapsed sidebar) means it needs
attention. Click it, or open that mailbox's settings, for the operation, time,
underlying cause, and recovery actions. Background failures keep cached mail
available and do not create global error toasts or appear in unrelated settings.
Authentication or Keychain failures pause automatic work for that mailbox until
you retry or sign in again. Connection failures follow the normal polling
schedule. Sign in again preserves the mailbox's identity, cached mail, drafts,
and synchronization preferences. Sending errors also stay in the compose form
so the unsent draft can be corrected.

The window adapts down to 360 × 320 logical pixels. At narrow widths, opening a message shows a full reader with Back navigation; settings moves account navigation above the page and wraps synchronization choices. See [DISPLAY.md](DISPLAY.md) for layout rules and checks.

## POP3 behavior

POP3 downloads are identified by stable UIDL values, cached, and deduplicated. Read/star flags and folder moves are local. Refresh preserves these local changes. Originals remain on the server; the app never issues POP3 `DELE`. Sent messages are saved locally after SMTP acceptance.

## Implemented interactions

- A sidebar listing every account as a collapsible group with its standard mailboxes and server folders (nested beneath the inbox). It collapses to an icon rail with ⌃⌘S.
- Right-click menus for accounts, mailboxes, and messages.
- Multi-select with the list's Select button, ⌘-click, ⇧-click, ⌘A, the avatar checkboxes, or a message's right-click menu (Select, Select All From Sender), with a bulk toolbar for read/unread, star, archive, spam, and trash. While several messages are selected, the reader pane summarizes them and offers the same actions; ⌘⇧U and ⌘⇧S mark or star the whole selection.
- A filter menu for unread, starred, and attachment-bearing mail, plus newest/oldest ordering.
- Star, read, and move changes apply instantly and roll back if the server rejects them.
- Background download of full messages into the local cache, with progress at the bottom of the sidebar (a ring when collapsed).
- Search and unread filtering within the currently loaded mailbox. Every word must match. Search terms: `from:` (sender name or address), `reply-to:`, `to:` (To or Cc), `cc:`, `subject:`, `is:unread`, `is:read`, `is:starred`, `is:unstarred`, `has:attachment`; a leading `-` excludes (`-from:noreply`) and quotes keep spaces (`from:"Ana López"`). A message's right-click menu searches for its sender or its Reply-To address. Reply-To and Cc come with the list headers of mail synced from now on; older cached mail has them once downloaded in full.
- Native selectable message text, interactive HTML rendered with Blitz, MIME decoding, attachment listing and saving.
- Compose, reply, reply all, forward, and file attachments.
- SMTP delivery; failed sends retain the draft.
- Automatic local draft saving and reopening; save on window close and quit.
- Server read/star flags and atomic IMAP moves where the server supports `MOVE`.
- Light, dark, and automatic appearance with seven theme colors; tooltips, keyboard shortcuts, and animated reader/dialog entrances.
- A full macOS menu bar: the Picacho menu (About, Settings ⌘,, Services, Hide, Quit), File, Edit, View, Mailbox (⌘1–⌘7), Message, Window, and Help. Shortcuts use Ctrl in place of ⌘ on Windows and Linux.
- Per-mailbox refresh schedules and new-mail notifications.

## Performance and storage

`uniform_list` draws only visible message rows. Filtering runs on preloaded summaries. DNS, TLS, authentication, sync, MIME parsing, downloads, and delivery run on GPUI's background executor. Per-account serialization prevents concurrent network requests from overwriting each other's state while keeping rendering and input responsive.

IMAP downloads headers in batches of 100 first, then downloads full MIME messages in the background in small batches (so other operations on the account are not blocked), newest first. Opening a message that is not downloaded yet fetches it immediately. The initial view loads 300 messages, with **Load more** up to 5,000. SQLite uses WAL mode and a mailbox/date index. Credentials and OAuth tokens are stored in the system keychain, not SQLite. The local message cache and drafts are ordinary, unencrypted SQLite data in a directory restricted to the current user.

Data lives in the platform directory selected by `directories::ProjectDirs` for `tech.pinal.picacho` (on macOS, `~/Library/Application Support/tech.pinal.picacho`); data from earlier builds is moved there on first launch. For development or isolated tests, set `PICACHO_DATA_DIR` to another directory. Account removal clears that account's local cache, drafts, and Keychain entry; it leaves server mail intact.

## Current boundaries

- Drafts are local to this device. They do not synchronize with the provider's Drafts folder.
- Search covers loaded message summaries, not a server-wide search index. Starred refresh searches the standard inbox/sent/archive/spam/trash mailboxes; Gmail uses its All Mail mailbox. Other server folders are listed under each account's inbox; messages can be moved between the standard mailboxes but not yet into custom folders.
- HTML email uses Blitz for CSS layouts, links, text selection, and disclosures. **HTML on/off** and **Remote blocked/allowed** controls above each message are saved per email. HTML is on and remote content is blocked by default; Settings → Reading changes the defaults for all accounts. Remote images, CSS and fonts only load when allowed. Scripts, forms, embedded frames and filesystem resources are disabled. CID attachment images and original-message forwarding attachments are not reproduced.
- Attachment forwarding is manual: save an attachment and attach it to the new message. Downloaded messages are limited to 25 MB; outgoing content and attachments to 18 MB before MIME encoding.
- IMAP archive/trash moves require the server's `MOVE` capability. The app avoids broad `EXPUNGE` operations that could remove unrelated messages.
- Gmail/Outlook save their own sent mail. For custom IMAP, the app attempts to append a sent copy; an append failure does not report an already accepted SMTP message as unsent.
- New mail is found by polling on each mailbox's schedule while Picacho is open; there is no background daemon or IMAP IDLE push yet.
- The app is verified on macOS. The code builds for Windows and Linux (native title bar, Ctrl shortcuts, platform keychains, notifications), but those platforms are not yet packaged or tested.

## Verification

```sh
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
```

The integration tests use local loopback servers and freshly generated, properly verified TLS certificates. They require `openssl` (included with the macOS developer environment) and permission to open local sockets. They do not contact live providers or send email to external recipients. `cargo test -- --ignored live` runs an opt-in discovery check against public autoconfig servers.

The tests cover IMAP sync/read/flags/atomic moves/cache reconciliation, SMTP STARTTLS authentication and MIME attachment delivery, POP3 TLS downloading and deduplication/local-folder preservation, MIME decoding, POP3 dot unstuffing and command validation, cache/account isolation, local drafts, PKCE generation, autoconfig/MX parsing, and headless UI tests of onboarding, settings discovery, and the account menu. OAuth against live Gmail and Microsoft accounts requires a user to sign in with their own application registration; it cannot be validated without that setup.

See [VERIFICATION.md](VERIFICATION.md) for the recorded checks and [SYNCHRONIZATION.md](SYNCHRONIZATION.md) for failure contracts, reference-client review and the reproducible Dovecot comparison.

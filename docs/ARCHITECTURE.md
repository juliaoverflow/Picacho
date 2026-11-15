# Developing Picacho

Picacho has three units. `main.rs` only opens the data directory and starts the app.

| Unit | Responsibility | Entry points |
| --- | --- | --- |
| **Sync** (`src/sync/`) | Provider APIs, discovery, OAuth, verified TLS, IMAP/POP3/SMTP exchanges, MIME decoding | `imap`, `pop`, `smtp`, `discover`, `oauth` |
| **Manage** (`src/manage/`) | Account operations, per-account serialization, cache reconciliation, SQLite, local drafts, encryption and readable in-memory messages | `Backend`, `Store`, domain types in `model` |
| **Display** (`src/display/`) | GPUI events and views, settings, message reader, assets and notifications | `run`, `ui`, `settings`, `reader` |

Display invokes Manage on GPUI's background executor. Manage coordinates Sync and persists completed results. Sync's protocol helpers have no database or UI access. Domain types are shared from Manage. OAuth currently saves refreshed credentials through Manage's keychain helper; credentials never enter SQLite. The internal `transport` facade keeps the coordinator's imports small, while protocol implementations remain in separate files.

## Where to make changes

- New provider: start with Sync discovery/OAuth and Manage's `Service`/`Account`. Keep server-specific behavior out of views.
- Incoming protocol: change Sync's `imap.rs` or `pop.rs`; update the coordinator in `manage/backend.rs` for persistence policy.
- IMAP extensions and the session stream (COMPRESS, IDLE waits): `sync/imap.rs` and `sync/stream.rs`; Manage's `connection.rs` caches capabilities, namespace and rights per connection. Push for new mail lives in `manage/watch.rs`; draft sync in `Backend::sync_drafts`. See [SYNCHRONIZATION.md](SYNCHRONIZATION.md).
- Database/cache: change `manage/store.rs`. Existing profiles open without a migration or data-directory rename in this reorganization.
- Encryption: change `manage/crypto.rs`; plaintext from encrypted mail must stay in memory.
- HTML renderer: `display/html/` is a reusable component with no mailbox, database, or MailApp dependencies. `document.rs` owns Blitz layout and vector painting, `view.rs` adapts GPUI input and visible-tile painting, and `resources.rs` is the only resource-loading boundary. The reader supplies HTML and a per-email remote-content policy. Reading defaults live in `manage/reading.rs`; both defaults and overrides use Store preferences. See [Picacho-Patches](https://github.com/pinaltech/Picacho-Patches/tree/HEAD/stylo_derive) for the scoped upstream compatibility patch.
- Settings or reader layout: use `display/settings.rs` or `display/reader.rs`. Shared view state and event handling live in `display/ui.rs`.

## Verification and commits

Run `cargo fmt --check`, `cargo clippy --all-targets --locked -- -D warnings`, `cargo test --locked`, and `cargo build --locked` before committing a phase. Also test `--features sample-data` when changing onboarding or views. Protocol tests run production code over loopback TLS with test-only trust roots. They need OpenSSL and local sockets, but no real accounts.

On Linux, GPUI needs the development libraries for ALSA, X11/XCB, XKB, Wayland, fontconfig, FreeType, Vulkan, EGL, OpenSSL, DBus and Secret Service. Headless view tests use GPUI's test platform. The lockfile keeps `command-fds` 0.3.1 and `libc` 0.2.183 because GPUI's transitive `xattr` 0.2.3 still refers to Linux's removed `ENOATTR` alias; revisit this compatibility constraint when upgrading GPUI.

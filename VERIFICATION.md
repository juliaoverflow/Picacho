# Verification record

Environment: macOS on Apple Silicon, Rust 1.96.0, GPUI 0.2.2, GPUI Component 0.5.1.

Result: all nine automated tests passed; formatting and strict Clippy checks passed; the optimized `.app` built and launched successfully. The bundle is approximately 16 MB. An idle UI sample showed 0.0% CPU and approximately 92 MB resident memory; this is a point-in-time observation, not a frame-rate or load benchmark.

## Automated coverage

The suite exercises real TCP/TLS protocol exchanges with local servers. Test roots are injected only in `#[cfg(test)]` builds. Production uses the operating system's normal trust store; no certificate verification bypass is compiled into the application.

| Test | What it verifies |
| --- | --- |
| `imap_tls_sync_read_flags_move_and_reconcile` | Authenticated IMAP over TLS, header-only sync, lazy body download, read/star flag updates, atomic UID MOVE, UIDVALIDITY-aware IDs, and removal of messages no longer in a mailbox |
| `smtp_starttls_auth_delivery_and_attachment` | STARTTLS upgrade, authentication, sender and recipient envelopes, MIME delivery with an attachment, draft removal after acceptance, and local POP3 Sent copy |
| `pop3_tls_download_is_deduplicated_and_local_moves_survive_sync` | TLS POP3, stable UIDL deduplication, cached MIME reads, and preservation of local archive/read changes across sync |
| `pop3_loopback_uidl_and_dot_unstuffing` | UIDL parsing and POP3 dot unstuffing |
| `parses_mime_body_and_attachment` | Encoded headers, UTF-8 subject, plain text, attachment filename and decoded size |
| `html_does_not_display_script` | HTML-to-text conversion excludes script/style contents |
| `rejects_pop_command_injection` | Newline-bearing POP3 commands are rejected |
| `cache_roundtrip_and_account_isolation` | SQLite cache and raw MIME round trips, folder projections, account isolation, reconciliation, and draft persistence |
| `pkce_is_url_safe_and_correct_length` | OAuth PKCE verifier/challenge encoding |

## Native UI walkthrough

The application is launched as a macOS `.app` and inspected through its actual rendered window. The walkthrough checks initial layout, typing/search, message selection, compose fields, draft saving and reopening, folder navigation, account forms, validation, light/dark appearance, and keyboard shortcuts. It uses a separate data directory (now `PICACHO_DATA_DIR`) so test drafts and preferences do not enter the user's normal profile.

The saved draft was confirmed after restarting the application, with its recipient, subject, and body preserved. The account form rejected a missing email address before attempting any network connection. Both appearance modes were inspected in the running release app.

## Limits of verification

Local protocol tests cannot validate a particular provider's tenant policies, consent configuration, password, or OAuth registration. Live Gmail and Outlook OAuth consent/token refresh require the user's credentials and registered client. No real mailbox was connected and no external email was sent during development.

## Picacho update (October 24, 2026)

Rename to Picacho, provider list, Pinal sign-in, Settings, and menu bar. `cargo fmt --check`, `cargo clippy --all-targets --locked -- -D warnings`, and `cargo test --locked` pass (18 tests, 1 ignored live test). New tests cover the provider list and discovery fallback, the Add Account dialog, theme color and per-mailbox settings persistence, ID-token addresses, the loopback callback rejecting a forged state, and IMAP STARTTLS with a date-limited sync window. The release `.app` builds and launches. The redesigned screens were not inspected visually in this pass, and browser sign-in was not run against live Google, Microsoft, or Pinal servers.

## Three-unit reorganization (October 26, 2026)

Verified on Linux x86_64 with Rust 1.96.0. `cargo fmt --check`, strict Clippy for all targets, `cargo build --locked`, and `cargo test --locked -- --test-threads=1` pass: 27 passed, 1 opt-in discovery test ignored. The protocol tests used real loopback sockets and verified TLS certificates. The Linux GPUI dependency compatibility fix is recorded in ARCHITECTURE.md; no third-party source files were patched. The refactor preserves the existing database format, cache identities, and GPUI action namespace.

## Synchronization reliability (October 26, 2026)

Linux x86_64, Rust 1.96.0: formatting, strict all-target Clippy, and the full suite pass (41 tests passed, 2 opt-in checks ignored). Local fault coverage includes incomplete/failed IMAP snapshots, transient reconnect, expunge races, missing/reset UIDVALIDITY, unsolicited flags, cleanup disconnects, partially accepted bulk changes, invalid/duplicate POP3 UIDL rows, interrupted RETR, repair of a summary without raw data, transactional database failures, dot unstuffing, response-size limits and command deadlines.

The opt-in Dovecot comparison also passes: Dovecot 2.4.1-4 (7d8c0e5759), verified loopback TLS, isolated Maildir, Picacho versus Python imaplib/poplib UID/flag/body-hash results. IMAP moves, POP3 deduplication and local-folder preservation were exercised. Thunderbird and NeoMutt source reviews and exact reproduction instructions are recorded in [docs/SYNCHRONIZATION.md](docs/SYNCHRONIZATION.md). Full desktop clients were not run against real user accounts.

## Responsive Display (October 26, 2026)

Formatting, strict all-target/all-feature Clippy, normal and sample-data builds pass. Both default and `sample-data` full test configurations pass with 45 tests and 2 opt-in checks ignored. The separate Dovecot comparison passes as well.

Four new GPUI tests cover eight viewport sizes from 1440×1000 to 360×320: settings bounds, wrapping synchronization choices, scrolling to account controls, locked-reader toolbar/password sizing, scrolling to Unlock, inline blank-input feedback and focus, narrow-reader Back navigation, bulk-selection controls, compose footer actions, onboarding and Add Account. Existing UI interaction tests also pass.

The native Linux GPUI application launched under Xvfb/Mesa; general and account settings were visually inspected at 1440×1000, 640×480 and 360×320. Long account names, horizontal account navigation and wrapping controls were checked using an isolated profile with synthetic accounts. Native encrypted-message unlocking was not exercised against a live Pinal account; its pane layout/interaction is covered by the headless GPUI tests. See [docs/DISPLAY.md](docs/DISPLAY.md).

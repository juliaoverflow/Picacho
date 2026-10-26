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

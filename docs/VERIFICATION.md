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

The opt-in Dovecot comparison also passes: Dovecot 2.4.1-4 (7d8c0e5759), verified loopback TLS, isolated Maildir, Picacho versus Python imaplib/poplib UID/flag/body-hash results. IMAP moves, POP3 deduplication and local-folder preservation were exercised. Thunderbird and NeoMutt source reviews and exact reproduction instructions are recorded in [docs/SYNCHRONIZATION.md](SYNCHRONIZATION.md). Full desktop clients were not run against real user accounts.

## Responsive Display (October 26, 2026)

Formatting, strict all-target/all-feature Clippy, normal and sample-data builds pass. Both default and `sample-data` full test configurations pass with 45 tests and 2 opt-in checks ignored. The separate Dovecot comparison passes as well.

Four new GPUI tests cover eight viewport sizes from 1440×1000 to 360×320: settings bounds, wrapping synchronization choices, scrolling to account controls, locked-reader toolbar/password sizing, scrolling to Unlock, inline blank-input feedback and focus, narrow-reader Back navigation, bulk-selection controls, compose footer actions, onboarding and Add Account. Existing UI interaction tests also pass.

The native Linux GPUI application launched under Xvfb/Mesa; general and account settings were visually inspected at 1440×1000, 640×480 and 360×320. Long account names, horizontal account navigation and wrapping controls were checked using an isolated profile with synthetic accounts. Native encrypted-message unlocking was not exercised against a live Pinal account; its pane layout/interaction is covered by the headless GPUI tests. See [docs/DISPLAY.md](DISPLAY.md).

## Mailbox errors and independent synchronization (October 26, 2026)

Linux x86_64, Rust 1.96.0: formatting, strict all-target/all-feature Clippy,
default and `sample-data` builds pass. Both complete test configurations pass:
50 passed, 2 existing opt-in checks ignored. Protocol tests require local socket
access; they use verified loopback TLS and do not contact real mail providers.

New tests cover cached Keychain denial and explicit retry, account isolation,
actionable IMAP authentication rejection without automatic replay, preservation
of cached mail/drafts after failed reauthentication, sidebar warning clicks in
expanded/collapsed modes, account-only details, clearing resolved failures without
losing unrelated SMTP/change failures, and coalesced polling while other UI work
is busy. Existing snapshot, protocol, encryption, responsive-layout and bulk
rollback checks continue to pass.

The native client was built and launched under Xvfb/Mesa with isolated profiles.
First-run onboarding, a mailbox with unavailable Keychain credentials, its red
sidebar warning and account-details/recovery panel, and General settings were
inspected. No real account was connected or external email sent. macOS signing,
Keychain ACL prompts, and live browser reauthentication cannot be exercised in
this Linux environment. The bundle script accepts a stable signing identity;
the README documents why ad-hoc rebuilds can change Keychain access identity and
how to verify the remaining macOS behavior.

## Interactive HTML email (October 26, 2026)

Linux x86_64, Rust 1.96.0: default and `sample-data` builds and full suites pass
(57 passed, 2 existing opt-in checks ignored in each configuration). Formatting
and strict all-target/all-feature Clippy pass. The unchanged ignored checks need
live discovery or a separate Dovecot installation.

New coverage exercises decoded HTML/plain alternatives, attachment exclusion,
old-cache compatibility, actual Blitz CSS painting, pointer-driven disclosure
expansion, long-message/HiDPI visible-tile rendering, per-email toggle clicks,
independent account/message choices, persisted defaults and overrides after
reopening, and narrow-window settings/controls. A loopback HTTP server verifies
that blocked HTML makes zero requests, enabling remote content loads CSS imports,
fonts/background/image resources, and requests carry no cookies or referrer.
Filesystem/unsafe URL schemes are refused; embedded data resources remain local.
The existing Pinal encryption test now also encrypts and opens HTML, verifies that
SQLite retains only encrypted raw mail and no readable HTML, restores HTML from
memory on reload, and discards it on locking even with a denied Keychain entry.

The native GPUI client ran under Xvfb/Mesa with an isolated synthetic profile.
HTML formatting/tables, disclosure expansion, plain-text switching, per-message
remote loading and the global Reading tab were visually inspected. The loopback
server received no request while blocked, then received the SVG request when
remote content was enabled. Reading settings and reader scrolling were checked
at 1440×1000, 640×480 and 360×320. No real mail account was connected. Native
macOS/Windows builds and OS browser/Keychain integration were not exercised here.

The renderer embeds Blitz 0.3.0-beta.4's DOM/HTML/paint crates without its standalone
shell or unrestricted network client. The scoped Stylo derive patch needed to
compile alongside GPUI is documented in `vendor/stylo_derive/README.md`; Linux's
Fontconfig API features are aligned in Cargo.toml.

## Incremental sync and Gmail limits (November 5, 2026)

Verified on macOS (Apple Silicon). `cargo fmt --check`, `cargo clippy --all-targets --locked -- -D warnings` and `cargo test` pass: 70 passed, 2 ignored (live discovery, Dovecot not installed). New tests use a scripted CONDSTORE + X-GM-EXT-1 server over local TLS: an unchanged mailbox costs one SELECT; new mail and flag changes use `UID FETCH n:*` and `CHANGEDSINCE` with no SEARCH; a removal falls back to one SEARCH; everything uses one sign-in. Placeholder rows fill on demand; each Gmail message downloads once across labels; a `[THROTTLED]` reply pauses background work without contacting the server. A GPUI test draws placeholder rows and checks that they cannot be opened, selected or matched by filters. Eight reliability tests had been failing on macOS because their fixture's accepted sockets inherited non-blocking mode; the fixture now clears it, and they pass. Gmail's pre-login CAPABILITY was checked live (it lists `X-GM-EXT-1`); CONDSTORE is detected after sign-in at runtime and was not checked against a live Gmail account. The debug app launched with an empty profile; the new rows were not inspected against a real mailbox.

## Review fixes for subscriptions and sender checks (November 12, 2026)

Rust 1.98 is now the minimum: mail-auth 0.14 and later depend on mail-parser 1.0, which requires it. Older mail-auth releases need the same mail-parser, so pinning one would not help; `rust-version` in Cargo.toml and the README say so. Verified on macOS (Apple Silicon) with Rust 1.98.0: `cargo build --locked --all-targets` and `cargo test --locked -- --test-threads=1` pass: 143 passed, 2 ignored. Strict Clippy reports three lints in code this change does not touch (`forwarding.rs`, `ui.rs` nickname input, `html/document.rs`), and `cargo fmt --check` was already failing before it.

Unsubscribing trusts List-Unsubscribe only as far as a DKIM signature from the From address's domain covers it (RFC 8058). The newest of a sender's last five messages with a signed header is used, so a forged message can't redirect it; one click is used only when List-Unsubscribe-Post is signed too. Everything else asks first, naming the address or page. A mailto goes to one plain address with a fixed body; extra recipients, `cc`/`bcc` and the body are ignored. Junk and Trash are never offered, and one-click redirects must stay on HTTPS. Tests sign messages with RFC 8463's example key and check forged, replayed, unsigned and other-domain headers, and a GPUI test checks that Cancel sends nothing.

Sender checks (SPF, DKIM, DMARC) now wait for Check sender in a message's details, since each lookup reaches the sender's DNS when it is made. A message over a week old whose signature no longer verifies, or whose key is gone or revoked, reads "couldn't verify (key may have changed)" rather than failed; a changed body still fails. Exchange Online's internal hops (2603:10a6, 2603:10b6 and 2603:1096, named in Microsoft's domains at both ends) are skipped when finding where mail arrived, so SPF checks the sender's server. An unclosed `<head>` ends at `<body>` or block content, and previews decode entities once. The one-time subscription pass works in batches of 100.

Not verified here: keyring 4 finding Secret Service passwords saved by keyring 3 on Linux (the backend moved from dbus-secret-service to zbus); this needs a check on a Linux desktop.

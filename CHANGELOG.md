# Changelog

All notable changes to Picacho are recorded here. Each release's section is
its release notes: on GitHub, in the F-Droid repository and in the Flatpak's
metainfo. Versions follow [Semantic Versioning](https://semver.org). The
rolling beta has no version or section of its own.

Add changes under **Unreleased**; in the pull request that releases `X.Y.Z`
into the stable branch, rename it to `## [X.Y.Z] - YYYY-MM-DD` and set the
same version in `Cargo.toml`.

## [Unreleased]

### Added

- Phones and tablets: tapping a link in an email first shows where it goes,
  warns of known tricks (look-alike letters, bare IP addresses, unencrypted
  pages), and offers Go Back, Open Anyway or Always Trust the site. Turn it
  off, or remove trusted sites, in Settings → Reading.
- Desktop: hovering a link in an email shows its address at the bottom left
  of the mail view.
- Phones and tablets: Edit on the Mailboxes screen sorts accounts by dragging
  their handles, in the same order as the desktop's sidebar.
- Pasting copied rich text into a message pastes it as plain text, with a
  “Paste with formatting” button to keep its bold, links and lists instead.
  Turn on Settings → Composing → Paste with formatting to make that the
  default, with “Paste without formatting” offered instead.
- Picacho keeps running in the background when its window closes, checking
  for new mail and notifying you of it, with a tray icon to open it again or
  quit. Turn it off, or show the tray icon while the window is open too, in
  Settings → General. Launching Picacho again opens the running one. In the
  background Picacho is a small process of its own, using a fraction of the
  window's memory.
- Scheduled mail (Send Later) is sent, and snoozed mail comes back to the
  inbox, while Picacho runs in the background (desktop and Android), each
  with a notification.
- Android: new-mail notifications, and checking for mail in the background
  (with an ongoing notification) after Picacho is closed, also after a
  restart. Turn it off in Settings → General.
- Android: signing in to Gmail, Outlook and Pinal opens the provider's page
  over Picacho (a Custom Tab), and its last page brings Picacho back.
- Android: Proton's human verification (its CAPTCHA) shows in Picacho.
- Android: Proton's second factor with a security key, held to the phone
  (NFC) or plugged in (USB), or with a passkey from your password manager
  (Android 14 and later, where it lets Picacho ask).
- Android: attaching files, and choosing an account picture, with the
  system's file picker.
- Android: Back goes back through Picacho's screens (a back swipe drags the
  screen as it goes), and from the first screen home, keeping Picacho where
  it was.

### Fixed

- Tapping a notification opens what it tells of: new mail opens the message
  in its mailbox, mail back from snooze the inbox (or the message), a sent or
  failed scheduled message Sent or Drafts. On Android it did nothing; on Linux
  clicking one now does this too while Picacho's window is open.
- Android: the keyboard shows for text fields, and Paste pastes what other
  apps copied (and Copy reaches them).
- Android: Picacho scrolls and animates at the display's full refresh rate
  (such as 120 Hz) rather than 60, also where the system caps apps it
  doesn't know (ColorOS), and lets the display slow down again when still.
- Android 15 and later: the top of the app no longer runs under the status
  bar, nor the bottom under the navigation bar, and the keyboard no longer
  covers the field being typed in.
- Selecting text in an email starts and ends under the pointer. In columns
  and centered layouts it ran well to the right of it, and in vertically
  centered cells a line away; links there are clicked where they are shown.
- Phones and tablets: text in an email can be selected and copied. A long
  press selects a word, moving the finger before lifting it selects more,
  and lifting it offers Copy, with the link or image pressed.

## [0.1.0] - 2026-11-16

### Added

- First release for Android (APK, through Picacho's F-Droid repository) and
  Linux (Flatpak, through Picacho's Flatpak repository).

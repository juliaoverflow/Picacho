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

- Windows: an installer for the current user (`scripts\bundle-windows.ps1`,
  unsigned for now), with Picacho's icon, version information and a Start
  menu shortcut. No console window opens. Notifications are Picacho's and
  open the message when clicked. The data folder is limited to your
  account. Picacho starts without WebView2 or Windows Hello: it says when
  Proton's check needs WebView2, and offers security keys and phone
  passkeys itself where Windows Hello isn't available.
- Events have a time zone: times are written in it, and Picacho says what
  time that is for you. Events elsewhere show both times.
- Guests' answers and the times they propose come back to the organizer:
  note the answer, move the event, or keep the time. Guests can propose
  another time, and updated invitations offer to update your calendar.
- Every repeating rule other apps make is expanded (such as "the last
  weekday of the month"), and extra dates (RDATE) are shown.
- Checking a found key: its fingerprint and QR code, and a field to paste
  the fingerprint its owner gave you, which Picacho compares. Your own key
  shows yours, for others to check.
- Repeating events show once for each time they happen, and one repeat
  can be changed or deleted on its own. Events can be made to repeat
  daily, weekly, monthly or yearly.
- Events can have guests: they're sent an invitation from your account
  when the event is saved, and a cancellation when it's deleted.
- With Local AI → Understand dates, event times can be written as
  "tomorrow at 10" or "every monday at 9"; an empty end is an hour later.
- Keys Picacho finds for people are marked "not checked" until you
  compare the fingerprint and mark them checked, and sending says when
  one wasn't checked.
- Encrypting the subject also hides the thread it's part of (In-Reply-To,
  References), Reply-To and Keywords.
- S/MIME drafts sync with the server's Drafts, sealed to your certificate.
- Reduce motion's Automatic follows Android's and iOS's settings, and
  notices changes whenever Picacho's window comes to the front.
- Windows builds include OpenSSL, and the Android build script finds
  Android Studio's SDK and NDK by itself.
- Invitations in mail show what, when, where and who above the message,
  with Accept, Maybe and Decline; the organizer is told (iMIP), from the
  address that was invited.
- Settings → Calendar: calendars on a CalDAV server (Fastmail, iCloud,
  Nextcloud, Radicale…). Accepted invitations go in, declined and cancelled
  ones come out, and events can be added, edited and deleted.
- Settings → Contacts: contacts can be added, edited and deleted, on the
  CardDAV server when there is one; what other apps keep on a card stays.
- OpenPGP keys are found automatically: on the address's domain (WKD), on
  keys.openpgp.org, and in Autocrypt headers; signed mail sends yours the
  same way. Settings → Encryption → Find Key looks one up.
- Encrypt the subject too: with OpenPGP, the composer can hide the subject
  inside the encryption (others see "...").
- Drafts to be sent encrypted sync with the server's Drafts, sealed to your
  own OpenPGP key, instead of staying on one device.
- S/MIME: import your certificate (.p12/.pfx), read and check S/MIME mail,
  keep senders' certificates, and sign and encrypt with S/MIME instead of
  OpenPGP.
- Accessibility: menus are menus to screen readers and take the keyboard
  when they open; contacts, events, invitations, keys and certificates are
  named. Settings → General → Reduce motion follows the system's setting
  (or yours): nothing slides or fades, and spinners stand still.
- Settings → Encryption: OpenPGP for every account. Import your key and
  others' public keys (armored `.asc` files from GnuPG, Thunderbird,
  Proton…); they're kept in the system keychain. Mail encrypted to you
  (PGP/MIME) opens when read and stays encrypted on disk; signatures are
  checked, and the message's details say who signed it, or that it was
  changed. The composer's Sign and Encrypt buttons sign with your key and
  encrypt to each recipient's and yours. Drafts to be sent encrypted aren't
  uploaded to the server.
- Settings → Contacts: address books read from a CardDAV server (Fastmail,
  iCloud, Nextcloud, Radicale…), and vCard files imported and exported.
  The composer suggests these people first, then people from your mail.
- A mailbox's menu has Export as Mbox… (all its messages in one mbox file,
  for other mail apps or a backup) and Import Messages… (`.mbox` files and
  `.eml` messages, added to it on the server).
- Screen readers: buttons, links, switches, choices, settings pages,
  folders (with unread counts) and messages (unread, sender, subject, date,
  preview) say what they are and whether they're on or selected, and the
  sidebar, message list, reader and composer are named regions. Tab and
  Shift-Tab move between controls, with a visible focus ring.
- The Summary's Receipts page shows what each receipt lists, under
  Details: its items, subtotal, shipping, fees, tip, taxes and discounts,
  and its total. They're read from the email's text or the sender's
  markup. Receipts can be searched by what they list too.
- Settings → Local AI → Suggest replies: a small model on this computer
  reads received mail and offers a few short replies under it (and in the
  Reply menu on phones and tablets), chosen for what the message says and
  worded in its language, in 37 languages: "Let me check my calendar." for
  a meeting request, "Das passt mir." for a German one. Choosing one starts
  a reply with it written in. Replies you choose are offered more, and
  right-clicking one stops it (Forget, beside the setting, starts over).
  Nothing is offered for mailing lists, no-reply senders, sign-in codes,
  receipts or package updates.
- JMAP: delivery receipts are asked for where the server can (DSN), the
  away reply works on servers that have only JMAP's own (VacationResponse,
  such as Fastmail), mail accounts shared with your sign-in can be added
  from the account's settings (Shared with you), search results show the
  part of each message that matched, and servers that push changes only
  over a WebSocket are followed too. Picacho reads the session again when
  the server says it changed, and keeps to its request limits.
- Settings → Sending → Let the server send scheduled messages: a message
  sent later goes to the server at once, which holds it and sends it at
  its time even with Picacho closed (SMTP FUTURERELEASE, JMAP HOLDUNTIL).
- Outgoing servers: TLS from the start on any port (not only 465), chosen
  in setup and for sending addresses' own servers, and found by
  autoconfiguration; relays that take mail without signing in work too.
- IMAP: Only subscribed folders (account settings) shows the folders
  subscribed to on the server; other users' and shared folders (NAMESPACE)
  are listed; servers speaking only IMAP4rev2 are understood (with
  Picacho-Patches' imap-proto patch 0005); moved and saved messages show in
  their new folder at once (COPYUID, APPENDUID); folders the listing shows
  unchanged aren't opened to check (LIST-STATUS); and server searches ask
  only for the newest page of results where the server can (PARTIAL).
- IMAP: on servers offering UIDONLY, messages are named only by their UIDs,
  sparing the server its sequence numbers (with Picacho-Patches' imap-proto
  patch 0006, which reads its UIDFETCH replies; until then it stays off).

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

- HTML email: a newsletter inside a form shows instead of nothing; content
  that fades in at first shows; wide emails are scaled to fit rather than cut
  off; absolutely positioned content and fixed bars are shown once, in place;
  sliced images in table cells meet without gaps; old attributes (centered
  tables, images aligned left or right, cell background images, `<font>`
  colors and sizes, named `bgcolor`s) apply; a body background reaches the
  edges; short words in narrow cells no longer break letter by letter.
- HTML email: blocked images show their alt text, and tracking pixels sized
  in CSS stay invisible. Images load several at a time, and long newsletters
  load all of theirs.
- HTML email in dark colors: an email whose dark styles only swap a logo is
  no longer shown as light text on a white page; dark text on a pastel
  color stays dark; black borders and rules turn light; links are readable;
  SVG images keep their colors.
- HTML email: `#` links and Tab scroll to their target; the email's own
  scrolling boxes scroll; double- and triple-click, shift-click and Ctrl/⌘A
  select text; Chinese, Japanese and Thai words are selected and wrapped as
  words.
- Sign-in in the browser no longer fails when a stray connection reaches
  Picacho's sign-in page before the browser does.
- A message encrypted with OpenPGP in an account other than Pinal Mail no
  longer asks to unlock Pinal Mail: it opens with your own key (Settings →
  Encryption), or says it is encrypted.
- Proton (Direct): a refused sign-in no longer says Proton ended the
  session; it says which step was refused and what Proton answered.
- JMAP: Require encryption in transit was ignored; messages now go only
  where the server can ask for it (REQUIRETLS), and are refused otherwise,
  as over SMTP.
- Phones and tablets: the filter chips over the message list fade out at an
  edge with more chips scrolled past it.
- Tablets: with the list and a message side by side, going back (Back, the
  back button or an edge swipe) only moves the column that changes; closing
  the message no longer slides the whole screen. Opening a message, and the
  next or previous one, likewise moves only the message's column.
- Tablets: in Settings beside its list, choosing a page or going back from
  an alias's reverse aliases moves only the page's column, and a back swipe
  no longer drags the whole screen. Back from reverse aliases returns to
  Forwarding instead of closing Settings.
- Phones and tablets: All Accounts (or an alias's view) names itself under
  the mailbox's name, rather than one of its accounts.
- Tapping a new mail notification for a message not synced yet (such as one
  just pushed) no longer leaves the message open before on screen; the new
  one opens once it arrives.
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

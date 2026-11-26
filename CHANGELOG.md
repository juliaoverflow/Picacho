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

- Start in the background when you sign in (Settings → General): Picacho
  starts with your computer without a window, showing its tray icon and
  checking for mail. Turning off running in the background turns it off too.
- Closing Picacho blurs its window behind "Closing Picacho…" or "Moving
  Picacho to the background…" until it's gone, while it saves and tidies
  its cache. (On macOS and Windows the window is covered without the blur.)
- On Linux, the dock or task manager shows Picacho as starting from the
  moment it's launched until its window opens, also when it was running in
  the background.
- Opening Picacho from its tray icon brings the window to the front on
  Wayland, and the icon pulses until the window opens.
- Tasks on JMAP servers (JMAP for Task Lists): with calendars from a JMAP
  account, its open tasks are listed in Settings → Calendar and can be
  added, edited, marked done and deleted, as on CalDAV servers. A repeating
  task is done one occurrence at a time, and changes others make to shared
  task lists are told with calendar notices.
- Task reminders, from CalDAV and JMAP servers alike, are shown with the
  calendar's, before each task is due.
- Formatted messages are written as they will look, no longer as Markdown:
  bold, italic, underline and the other formats show as such, lists,
  quotes and headings as they're sent, links as links. Underline (⌘U/Ctrl+U),
  alignment and Clear formatting are new in the toolbar. Pictures pasted or
  dropped on the text show where they go, in the line when they fit, and
  are sent there; drag a selected picture's corner to resize it. Tables
  show as tables. Copying keeps the formatting (and pictures) in other apps.
- Spelling in formatted messages: misspelled words are underlined, with
  spellings, Learn and Ignore on a right click (Settings → Composing →
  Check spelling). macOS and Windows use the system's spellchecker, Linux
  the Hunspell dictionaries installed.
- Phones and tablets: a long press in a formatted message selects a word,
  with handles and a Cut, Copy, Paste menu; dragging scrolls.
- Drafts written in other mail apps keep their HTML formatting: fonts,
  colors, alignment, tables and styles open as written and are sent as
  they were, and their pictures show in the text. Picacho's own formatted
  drafts are saved as HTML too, so other apps show their formatting.

- Gmail and Outlook accounts can use their provider's own API instead of
  IMAP and SMTP: **Use the Gmail API** or **Use Microsoft Graph** when
  adding the account. Microsoft 365 organizations that turn IMAP off can be
  added this way. Mail, folders and labels, search, drafts, sending and
  snoozing work as for other accounts; Gmail's filters and Outlook's inbox
  rules, and each one's away reply, are edited in Settings. With Microsoft
  Graph, Exchange's calendars and contacts and the organization's directory
  are offered in Calendar and Contacts: repeating events as their series,
  with occurrences changed or cancelled on their own. Exchange holds
  scheduled mail and sends it at its time; imported mail arrives as mail,
  not drafts; new mail is noticed within half a minute. An existing
  account switches either way in its Settings.
- Exchange Server in an organization's own data center, through Exchange
  Web Services: found by Autodiscover, signed in with NTLMv2 or Kerberos,
  or through Microsoft Entra where the organization uses hybrid modern
  authentication. A server that offers only Basic sign-in, which sends the
  password itself, is used only once you turn on **Allow Basic sign-in**
  (offered when adding the account, and in its Settings), and only over
  HTTPS. Mail, folders, drafts, sending and scheduled
  sending, inbox rules and Out of Office, calendars (with availability),
  contacts and the organization's directory. New mail is noticed as it
  arrives (Exchange's streaming notifications). Servers requiring Extended
  Protection for Authentication are signed in to. Autodiscover also
  follows Exchange's redirects and SRV records within the domain, and asks
  SOAP Autodiscover where the older form is off.
- Exchange Server and Microsoft Graph accounts show how much space their
  mail takes.

### Fixed

- Mobile view tests no longer take one another's launch requests (they
  failed now and then when run together).
- Inline PGP can be sent, not only read: with OpenPGP, the composer's
  inline PGP button signs the text in the clear or replaces it with an
  armored encrypted message, and encrypts (`name.pgp`) or signs
  (`name.sig`) each attachment on its own, for apps without PGP/MIME. Plain
  text only. Received inline PGP mail now keeps its attachments, and
  decrypts encrypted ones.
- The interface is ready for translation: its text is in Fluent message
  files (`locales/`), shown in the system's language where Picacho has it
  (English for now), with each language's plural rules.
- iOS catches up with Android: system notifications for new mail with Mark
  as Read, Archive, Delete and Reply; checking for mail in the background
  through background app refresh (scheduled mail and snoozes too); Proton
  sign-in with a security key held to the iPhone (NFC) or a passkey on
  another device by QR code, and Proton's human verification (CAPTCHA); a
  Share extension that starts a message with what was shared; and app icons
  at every iPhone and iPad size.
- App lock (Settings → Privacy): a passcode, Touch ID, Face ID or Android's
  fingerprint or face unlock when Picacho opens and after it has been away
  for a chosen time, Lock Picacho (⌥⌘L / Ctrl+Alt+L), covers over its other
  windows and, on phones, the app switcher's picture (Android 13 and later
  also keep it out of Recents), and slower tries after wrong passcodes.
- Hide mail outside Picacho (Settings → Privacy, on with the app lock or
  encryption): notifications and widgets say how much new mail there is,
  not who it's from or what it says.
- An optional encrypted local database (SQLCipher, AES-256) with its key in
  the system keychain, turned on and off in place (Settings → Privacy). A
  database that can't be opened at launch (its key gone, the keychain
  locked) gets a window offering Try Again or moving it aside to start over,
  rather than Picacho quitting silently.

- Group by Conversation in the message list (grouped as the reader gathers
  a conversation), compact rows, and the reader below the list; Pin to Top
  and Mute Conversation; dragging messages onto sidebar folders (within an
  account, across accounts, onto All Mail to archive, or onto Drafts to
  start new drafts).
- Find in Message (⌘F / Ctrl+F) with highlighted matches; ⌥⌘F searches the
  mailbox.
- Print without a browser (the system print dialog on Linux and macOS; on
  Windows, and on Linux without a print portal, the message opens as a PDF to
  print from the viewer) and vector Save as PDF with full formatting, images
  and embedded Unicode fonts.
- Attachments open from their saved file or a single cached copy beside the
  mail store, with the system's app, Quick Look (macOS), Open With…
  (Windows, macOS and Linux) and Show in Folder; PNG, JPEG, GIF, WebP, SVG,
  BMP, ICO, TIFF, TGA, PNM, QOI and HDR images (plus HEIC/HEIF/AVIF on
  macOS) preview in the reader.
- A local full-text index of downloaded mail (SQLite FTS5) covering whole
  messages and their attachments' text (text files, HTML, attached emails,
  PDFs, Word, Excel, PowerPoint and OpenDocument files): words find
  message text also beside `from:`-style operators, servers are asked about
  mail downloaded only in part, and Search All Mail on This Computer lists
  every account's matches as results of their own; Saved Searches in the
  sidebar.
- Blocked senders and rules applied by Picacho to new inbox mail, for every
  service, by the window and in the background; Apply Rules Now for mail
  already in the inboxes (Settings → Rules & Layout).
- Signatures per mailbox and per sending address (Markdown or raw HTML,
  swapped automatically when changing From in an open draft), and templates
  with placeholders (`{name}`, `{first_name}`, `{date}`, `{subject}`…) in
  the composer.
- Calendar and Contacts as views of their own on desktops and phones, the
  calendar's hour-by-hour Day and Week time grids and Month grid, and custom
  repeat rules across daily, weekly, monthly and yearly schedules.
- Publishing your OpenPGP key on keys.openpgp.org or, once confirmed, on an
  HKP keyserver (`keyserver.ubuntu.com` or one typed in; these list every
  address at once and for good), or exporting Web Key Directory (WKD) files
  for your own domains. Keys are never looked up on HKP keyservers, which
  don't check the addresses keys are listed under.
- Picacho as the default email app (`mailto:`) and for `.eml` and Outlook
  `.msg` files on macOS, Linux, Windows and Android; Settings → General
  reports default-app status and jumps directly to Picacho's page in Windows
  Default apps (or sets it directly via `xdg-mime`/`xdg-settings` on Linux);
  requests forwarded through the background process hand over cleanly through
  a private handover file inside the data directory.
- Mark as Read, Archive, Delete and Reply buttons on new-mail notifications,
  and Mark All as Read, Archive All and Delete All on ones for several
  messages (Linux, Windows, macOS, Android), also while Picacho runs in the
  background.
- Home-screen and desktop widgets across Android, iOS, macOS, Linux and
  Windows: a resizable **Inbox** widget (recent messages, unread badge,
  optional per-account and unread-only filter, and quick Compose), a compact
  **Unread Mail** widget (also on the iOS Lock Screen), an upcoming
  **Calendar** agenda widget, and **Packages** and **Receipts** widgets
  showing as many of your recent deliveries and receipts/refunds as their
  size fits (whole rows, never scaled), with **View all N** at the top, following
  Picacho's theme accent color and Local AI's sign-in code hiding — natively
  via Android AppWidgets, iOS and macOS WidgetKit, KDE Plasma 6 Plasmoids,
  Windows 11 Widgets Board Adaptive Cards, `--widget-json`/`--widget-waybar`
  CLI output, and a built-in **Desktop Widget** window (Window menu, tray
  menu, or `--widget`).
- Keyboard shortcuts changed in Settings → **Keyboard Shortcuts**: click a
  shortcut and press new keys, or keys one after another for a sequence (keys
  another action had are taken from it), reset one or all, or take one away.
  Tooltips name the shortcuts as they are now. Every shortcut is listed there, the
  composer's formatting ones too, in place of the old Keyboard Shortcuts
  dialog and Rules & Layout's shortcuts section. They're kept in
  `shortcuts.txt`, whose edits apply live without restarting Picacho.

- IMAP and POP3 servers that only offer STARTTLS (ports 143 and 110): the
  connection is upgraded before anything is sent, and never used plain.
  Found by autoconfig and port probing, or chosen in manual setup.
- POP3: older mail is listed by its headers (TOP) and downloads whole when
  opened; servers asking to wait (LOGIN-DELAY, IN-USE, SYS/TEMP) pause
  background checks instead of being taken for a wrong password; UTF-8
  (RFC 6856) where offered.
- Filters (ManageSieve): room is checked first (HAVESPACE), Picacho's
  inactive script is replaced whole (RENAMESCRIPT), and QUOTA, TRYLATER and
  REFERRAL refusals say what they mean.
- SMTP: a message's priority (composer button; MT-PRIORITY, Importance and
  X-Priority), and **Give up on delivery after** a few hours (DELIVERBY).
- IMAP: IMAP4rev2 is turned on where offered beside rev1; searches use
  `newer:`/`older:` (WITHIN), approximate words (SEARCH=FUZZY), one sorted
  page (ESORT), one search across All Mail (MULTISEARCH); long commands
  reporting progress (INPROGRESS) aren't timed out; Storage shows the
  fullest quota root, and suggests JMAP where the server offers it
  (JMAPACCESS).
- Calendars and contacts over JMAP sync only what changed
  (CalendarEvent/changes, ContactCard/changes), page through every event and
  card, and refuse to overwrite one changed elsewhere. Free/busy and your
  invitation addresses come from JMAP Principals, and shared-calendar
  notices show in Settings → Calendar.
- CalDAV and CardDAV: calendars and address books can be renamed and
  deleted; edits use Schedule-Tag where the server schedules, and its
  scheduling inbox is read; events and cards too large for the server are
  refused before sending; files can be attached to events where the
  calendar keeps them (RFC 8607); new cards go as vCard 4.0 to books that
  take only that; the calendar server's storage is shown.
- Events in a time zone carry its VTIMEZONE, so Outlook and Exchange show
  them at the right time.

- Single sign-on for IMAP and POP3 accounts with an organization's
  identity provider (OpenID Connect / OAuth 2.0: Okta, Entra ID, Keycloak,
  Authentik, ADFS…), including those that sign people in with SAML 2.0
  behind it. Found from the domain's autoconfig file where it says so, or
  set up under Server settings with the sign-in service's address. Picacho
  registers itself where the provider allows, or uses a client id from the
  administrator; the mail servers get the token with OAUTHBEARER or XOAUTH2.
- Kerberos sign-in (SASL GSSAPI, RFC 4752) for IMAP and POP3 accounts, and
  their outgoing and filter servers, with the ticket the computer already
  has. Uses the system's Kerberos (GSS-API on Linux, the BSDs and macOS,
  SSPI on Windows); no password is kept or sent, and a server without
  GSSAPI is never tried with a password.
- POP3: **Remove from the server** after download, after a week or after a
  month, and **Delete on the server too** for mail deleted here for good.
  Both off by default; only mail saved here completely is removed, once
  the server confirms.
- Tasks can be edited and deleted, have no due date, and keep what other
  apps set on them (priority, categories, alarms).
- **Let the server send invitations**: a CalDAV server that schedules (RFC
  6638) can send the invitations, updates and cancellations for events you
  organize, instead of Picacho mailing them.

- Calendars and address books read only what changed since the last sync
  (WebDAV sync-collection, RFC 6578; else the collection's ctag), rather
  than everything each time. Calendars for tasks only are left out, and
  calendars keep their colour.
- CalDAV and CardDAV servers are found from a domain's `_caldavs._tcp` and
  `_carddavs._tcp` records (RFC 6764), where the server is in the domain.
- Calendars and contacts from Fastmail and other JMAP servers, signed in as
  the mail account already is (Settings → Calendar / Contacts).
- Tasks (open VTODOs) in Settings → Calendar, added and marked done there;
  reminders on events, shown as notifications; and "When Are They Free?"
  for an event's guests, where the calendar server schedules (RFC 6638).
- New calendars and address books can be made on the server.
- Contacts keep and edit postal addresses, birthdays and photos, and
  groups with their members.
- JMAP: messages handed to the server to send later are listed and can be
  taken back (Settings → Calendar → Held to send later). Sent messages say
  what became of each recipient's copy (Delivery). The server's own sending
  addresses are shown and can be renamed, added or removed. Conversations
  include messages in other mailboxes, and messages can be copied to
  another account the same sign-in reaches. Attached messages can be
  previewed without downloading them, and read receipts are sent by the
  server where it can (RFC 9007).
- JMAP mailboxes that changed are brought up to date from what changed in
  their listing (Email/queryChanges), rather than listed again.
- IMAP: drafts are replaced in one step (REPLACE), imported mail goes in
  batches (MULTIAPPEND), sign-in takes one round trip (SASL-IR), a missing
  Sent or Archive folder is made where the server can mark it
  (CREATE-SPECIAL-USE), storage used is shown on servers without QUOTA
  (STATUS=SIZE), and a snoozed message is woken by the server's own search
  result (SEARCHRES).
- Large messages on IMAP servers with BURL and URLAUTH are filed in Sent
  first and sent from there, crossing the network once; a forward with the
  original attached has the server put the original in itself (CATENATE).
- SCRAM-SHA-256-PLUS and SCRAM-SHA-1-PLUS: sign-in is tied to the TLS
  connection (tls-server-end-point), for IMAP, SMTP, POP3 and ManageSieve.
- SMTP: BINARYMIME for messages with binary content, and HELO for servers
  from before ESMTP.
- Sending to more recipients than the server takes at once (RCPTMAX, or a
  452 "too many recipients") sends to the rest in another round. Refusals
  say what the server's code means ("That mailbox doesn't exist").
  Delivery reports name each recipient as written (ORCPT).
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

### Changed

- Settings is reorganized: every page groups its settings into titled
  cards, with controls beside their names where they fit and long
  explanations trimmed to a few words. A mailbox's page starts with its
  picture (Change Picture is beside it), then Account, Sync, Sending,
  Delivery, Storage and Filters; Contacts and Calendar show your contacts,
  events and tasks before their server settings.

### Fixed

- Tasks no longer show as events in calendars that hold both, tasks
  without a due date are no longer dropped, and a task's due date is its
  DUE rather than its start.
- Usage notes no longer describe a "Load more" step the message list
  doesn't have.
- Saving an event with guests to iCloud, Fastmail, Nextcloud or Google no
  longer has the server email them a second invitation: Picacho sends
  invitations itself, and now tells the server so (SCHEDULE-AGENT=CLIENT).
- Editing a contact keeps its email and phone labels (work, home, Apple's
  custom labels), full name parts (prefixes, middle names, suffixes) and
  organization units, unless those were what changed.
- Saving an invitation with a reminder to a calendar no longer writes it
  without the reminder's start, which some servers refused.
- Leaving an IMAP folder (to delete it) uses UNSELECT where offered, so
  messages another app marked deleted aren't removed.
- JMAP servers that are limiting requests (429, 503) pause background sync,
  as IMAP servers that say so do.
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

### Security

- Turning the local database's encryption on or off converts the drafts'
  files kept beside it as a migration that lasts failures and crashes:
  Picacho reports success only once every file is converted and read back,
  keeps the old key until then, and carries on when the switch is asked for
  again or Picacho starts. Converting no longer writes over an attachment
  named like its temporary file.
- While the local database is encrypted, the pieces of a large message
  kept on disk so its download can resume are encrypted too, each checked
  when the download resumes; unencrypted pieces left from before are
  deleted.
- With a proxy set for the system, email images and other requests to
  addresses strangers chose (one-click unsubscribing, MTA-STS policies, key
  lookups, revocation checks) are no longer sent through it, since a proxy
  looks names up itself and could reach your local network. If your proxy
  refuses such requests itself, turn on **Proxy blocks local network** in
  Settings → Reading (shown when a proxy is set), or set
  `PICACHO_PROXY_BLOCKS_LOCAL_NETWORK=1` for the whole computer.
- Check sender no longer says a certificate isn't revoked on the strength
  of a revocation list that doesn't cover it: another partition of its
  issuer's lists, a list for some reasons only, a delta list, or one not yet
  issued or without an end date. Only lists that cover it are kept.
- Exchange accounts no longer fall back to Basic sign-in on their own,
  which sent the password itself to any server that left out NTLM. Where
  your Exchange server offers only Basic, Picacho now says so and syncing
  stops until you turn on **Allow Basic sign-in** in the account's
  Settings. Autodiscover never sends Basic.
- An Autocrypt header, which anyone can forge, no longer replaces a
  contact's OpenPGP key found on their domain or a keyserver, so a forged
  message can't have mail you encrypt go to someone else's key. Keys from
  Autocrypt headers are limited in size and number.
- An inline PGP message's attachments that its signature or encryption
  doesn't cover are named in the message details, which no longer show the
  message as fully signed or encrypted.
- CardDAV and CalDAV send your password only to the server's own site,
  even when the server names an address elsewhere.
- Release builds ignore environment variables that would point sign-in and
  Pinal Mail at other servers.
- A received message's Message-ID, read-receipt address or invitation
  organizer can no longer add headers to the read receipts, replies and
  calendar answers Picacho sends.
- CardDAV and CalDAV never send your password over plain http, even when
  the server points elsewhere.
- An S/MIME certificate is kept only for the address its message is from,
  and a self-signed one never replaces one already kept, so a stranger
  can't have mail you encrypt go to their certificate.
- Email images are never loaded from your computer or local network, and
  an email's images can't run Picacho out of memory. A proxy set for the
  system (also one on the local network) is used to load them.
- Connections use TLS 1.2 or later.
- Copied sign-in codes are marked sensitive on Android, so they don't show
  in the clipboard preview.
- Key lookups (WKD) only go to plain domain names, and read at most 1 MB.
- Mail sent over SMTP has every line ending made CRLF before it is sent,
  so nothing in a forwarded message can end the data early (SMTP
  smuggling).
- An email whose start looks almost like a DOCTYPE no longer fails to
  render.
- Importing a file larger than an import may be, or one without line
  breaks, no longer reads it all into memory.
- Release builds' signing key is only given to builds that publish, never
  to a pull request's.

### Performance

- Widgets are refreshed after a background check without looking through
  every account's mail again unless it changed in ways packages and
  receipts depend on, and then once for both.
- Reading a message of thousands of parts no longer takes time growing
  with the square of their number, and at most 4096 parts are looked at.
- Opening a large email file checks its size before reading it, so an
  oversized `.eml` or `.msg` can't run Picacho out of memory.
- Reading and downloading mail no longer reads every OpenPGP key again
  for each message, and other people's keys are kept in the database
  rather than rewritten whole into the system keychain for each new one.
- Verification results are kept for recently read messages only, and
  pictures in quoted replies are decoded within the same limits as
  attachment previews.
- An open email's thread, and Proton and JMAP accounts' watches, sleep
  instead of waking several times a second.
- Syncing Proton and JMAP accounts, marking a mailbox read and emptying a
  folder no longer read every cached message's text; All Mail and Starred
  read only the messages they show.
- Deleting or labelling thousands of chosen messages no longer stalls the
  window.
- Exporting a mailbox and renaming a Proton or JMAP folder no longer read
  every message's text at once; listing more of a large IMAP mailbox looks
  up only its own messages.
- Less memory: the message list keeps only the open message's text, not
  every message read since the mailbox was opened; only the latest 32
  decrypted messages keep their text in memory; previews are at most 400
  characters; a message's raw source isn't kept once another opens; and on
  Linux memory an email or a long list used goes back to the system.
- Counting subscriptions and code mail, and finding parcels and receipts,
  no longer hold every message of an account at once.
- Marking a Proton folder read keeps only the unread messages' ids, not
  the whole folder's listing.

## [0.1.0] - 2026-11-16

### Added

- First release for Android (APK, through Picacho's F-Droid repository) and
  Linux (Flatpak, through Picacho's Flatpak repository).

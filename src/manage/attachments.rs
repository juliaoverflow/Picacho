//! Attachments across mail: the attachment browser's list of every
//! attachment in a mailbox, an account or All Accounts, filtered by type,
//! name, sender and what the attachments say (the full-text index of
//! `attachment_text`), sorted, previewed and saved together
//! (`display/attachments.rs`); and what is shared with one person, for
//! the reader's contact panel (`display/contact_panel.rs`): their mail,
//! its attachments and the events they are invited to.
use crate::manage::{backend::Backend, model::*};
use crate::sync::{carddav::Card, ical::Event};
use anyhow::{Context as _, Result};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
};

/// The most messages with attachments the browser lists at once, newest
/// first (each account's, and then all of them together).
pub const LIBRARY_LIMIT: usize = 5000;
/// The most messages whose attachments' text a search finds.
pub const CONTENT_LIMIT: usize = 5000;

/// What sort of file an attachment is, as the browser's filters group them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Image,
    Pdf,
    Document,
    Spreadsheet,
    Presentation,
    Archive,
    Audio,
    Video,
    /// Invitations, events and contact cards.
    Calendar,
    /// Attached emails.
    Email,
    Other,
}

impl Kind {
    pub const ALL: [Self; 11] = [
        Self::Image,
        Self::Pdf,
        Self::Document,
        Self::Spreadsheet,
        Self::Presentation,
        Self::Archive,
        Self::Audio,
        Self::Video,
        Self::Calendar,
        Self::Email,
        Self::Other,
    ];

    /// An attachment's kind, by its type, else its name's extension.
    pub fn of(a: &Attachment) -> Self {
        let mime = a.mime.to_ascii_lowercase();
        let ext = a
            .name
            .rsplit_once('.')
            .map(|(_, e)| e.to_ascii_lowercase())
            .unwrap_or_default();
        let by_ext = match ext.as_str() {
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "bmp" | "ico" | "tif" | "tiff" | "heic" | "heif"
            | "avif" | "tga" | "qoi" | "hdr" | "pbm" | "pgm" | "ppm" | "pnm" => Some(Self::Image),
            "pdf" => Some(Self::Pdf),
            "doc" | "docx" | "docm" | "dotx" | "odt" | "ott" | "rtf" | "txt" | "text" | "md" | "markdown"
            | "pages" | "html" | "htm" => Some(Self::Document),
            "xls" | "xlsx" | "xlsm" | "ods" | "csv" | "tsv" | "numbers" => Some(Self::Spreadsheet),
            "ppt" | "pptx" | "pptm" | "odp" | "key" => Some(Self::Presentation),
            "zip" | "rar" | "7z" | "tar" | "gz" | "tgz" | "bz2" | "xz" | "zst" => Some(Self::Archive),
            "mp3" | "m4a" | "wav" | "ogg" | "oga" | "opus" | "flac" | "aac" | "amr" => Some(Self::Audio),
            "mp4" | "m4v" | "mov" | "webm" | "mkv" | "avi" | "3gp" => Some(Self::Video),
            "ics" | "ical" | "vcs" | "vcf" | "vcard" => Some(Self::Calendar),
            "eml" | "msg" => Some(Self::Email),
            _ => None,
        };
        // A generic type says nothing; the name does.
        if matches!(mime.as_str(), "" | "application/octet-stream" | "application/x-download" | "binary/octet-stream") {
            return by_ext.unwrap_or(Self::Other);
        }
        let by_mime = match mime.as_str() {
            m if m.starts_with("image/") => Some(Self::Image),
            m if m.starts_with("audio/") => Some(Self::Audio),
            m if m.starts_with("video/") => Some(Self::Video),
            "application/pdf" => Some(Self::Pdf),
            "message/rfc822" | "message/global" | "application/vnd.ms-outlook" => Some(Self::Email),
            "text/calendar" | "application/ics" | "text/vcard" | "text/x-vcard" | "text/directory" => {
                Some(Self::Calendar)
            }
            "text/csv" | "text/tab-separated-values" | "application/vnd.ms-excel" => Some(Self::Spreadsheet),
            "application/vnd.ms-powerpoint" => Some(Self::Presentation),
            "application/msword" | "application/rtf" | "text/rtf" => Some(Self::Document),
            m if m.contains("spreadsheet") => Some(Self::Spreadsheet),
            m if m.contains("presentation") => Some(Self::Presentation),
            m if m.contains("wordprocessing") || m.contains("opendocument.text") => Some(Self::Document),
            m if m.contains("zip") || m.contains("compressed") || m.contains("x-tar") || m.contains("x-7z")
                || m.contains("x-rar") || m.contains("gzip") =>
            {
                Some(Self::Archive)
            }
            _ => None,
        };
        // `text/plain` names a CSV or a calendar as often as a note.
        match (by_mime, by_ext) {
            (Some(k), _) => k,
            (None, Some(k)) => k,
            (None, None) if mime.starts_with("text/") => Self::Document,
            (None, None) => Self::Other,
        }
    }

    /// Its icon (Lucide's, from gpui-component's icons).
    pub fn icon(self) -> &'static str {
        match self {
            Self::Image => "file-image",
            Self::Pdf => "file-text",
            Self::Document => "file-pen-line",
            Self::Spreadsheet => "file-spreadsheet",
            Self::Presentation => "presentation",
            Self::Archive => "file-archive",
            Self::Audio => "file-music",
            Self::Video => "file-video-camera",
            Self::Calendar => "calendar",
            Self::Email => "mail",
            Self::Other => "file",
        }
    }
}

/// One attachment of one message.
#[derive(Clone, Debug)]
pub struct Item {
    /// Its message, without its text; shared by its attachments.
    pub mail: Arc<Mail>,
    /// Which of `mail.attachments()` it is.
    pub index: usize,
}

impl Item {
    pub fn attachment(&self) -> &Attachment {
        &self.mail.attachments()[self.index]
    }
    /// What tells it apart from every other attachment: its message and part.
    pub fn key(&self) -> String {
        format!("{}\u{1f}{}", self.mail.id, self.attachment().part)
    }
    pub fn kind(&self) -> Kind {
        Kind::of(self.attachment())
    }
    /// Whether the full-text index reads its text.
    pub fn readable(&self) -> bool {
        let a = self.attachment();
        crate::manage::attachment_text::readable(&a.mime, &a.name)
    }
}

/// Every attachment of `mails`, message by message in their order.
pub fn items(mails: Vec<Mail>) -> Vec<Item> {
    mails
        .into_iter()
        .flat_map(|m| {
            let mail = Arc::new(m);
            (0..mail.attachments().len()).map(move |index| Item {
                mail: mail.clone(),
                index,
            })
        })
        .collect()
}

/// How the browser orders attachments.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Sort {
    #[default]
    Newest,
    Oldest,
    Largest,
    Name,
}

impl Sort {
    pub const ALL: [Self; 4] = [Self::Newest, Self::Oldest, Self::Largest, Self::Name];
}

/// `items` in the `sort` order; ties keep their message's date order.
pub fn sort(items: &mut [Item], sort: Sort) {
    match sort {
        Sort::Newest => items.sort_by_key(|i| std::cmp::Reverse(i.mail.date)),
        Sort::Oldest => items.sort_by_key(|i| i.mail.date),
        Sort::Largest => items.sort_by_key(|i| std::cmp::Reverse(i.attachment().size)),
        Sort::Name => items.sort_by_cached_key(|i| i.attachment().name.to_lowercase()),
    }
}

/// What the browser keeps of its attachments.
#[derive(Clone, Debug, Default)]
pub struct Filter {
    pub kind: Option<Kind>,
    /// Words typed, lowercase: each found in the attachment's name, its
    /// message's subject or sender, or (with `content`) in what it says.
    pub words: Vec<String>,
    /// The messages whose attachments' text holds every word
    /// (`Store::attachment_text_matches`), once looked up.
    pub content: Option<Arc<HashSet<String>>>,
    /// Only attachments from or to this address (lowercase).
    pub person: Option<String>,
}

impl Filter {
    pub fn keeps(&self, item: &Item) -> bool {
        if self.kind.is_some_and(|k| item.kind() != k) {
            return false;
        }
        if let Some(person) = &self.person
            && !involves(&item.mail, person)
        {
            return false;
        }
        if self.words.is_empty() {
            return true;
        }
        let a = item.attachment();
        let m = &item.mail;
        let named = self.words.iter().all(|w| {
            a.name.to_lowercase().contains(w)
                || m.subject.to_lowercase().contains(w)
                || m.sender.to_lowercase().contains(w)
                || m.from.to_lowercase().contains(w)
        });
        // What its message's attachments say, matched as a whole: only
        // those whose text the index reads can have said it.
        named || (item.readable() && self.content.as_ref().is_some_and(|ids| ids.contains(&m.id)))
    }
}

/// The words of a search, lowercase, quoted phrases kept together.
pub fn words(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for (i, part) in text.split('"').enumerate() {
        if i % 2 == 1 {
            let phrase = part.trim().to_lowercase();
            if !phrase.is_empty() {
                out.push(phrase);
            }
        } else {
            out.extend(part.split_whitespace().map(str::to_lowercase));
        }
    }
    out
}

/// Whether `address` (lowercase) sent `m`, or it was sent to or copied to it.
pub fn involves(m: &Mail, address: &str) -> bool {
    m.from.trim().eq_ignore_ascii_case(address)
        || [m.to.as_str(), m.cc()]
            .iter()
            .any(|list| crate::manage::categories::addresses_in(list).any(|a| a == address))
}

/// Where each of `names` is saved in `dir`: its safe name, numbered
/// ("report (2).pdf") apart from the others and from files there already.
pub fn file_names<'a>(names: impl IntoIterator<Item = &'a str>, dir: &Path) -> Vec<PathBuf> {
    let mut taken: HashSet<String> = HashSet::new();
    names
        .into_iter()
        .map(|name| {
            let safe = crate::manage::backend::opened::safe_file_name(name);
            let (stem, ext) = match safe.rsplit_once('.') {
                Some((s, e)) if !s.is_empty() => (s.to_owned(), format!(".{e}")),
                _ => (safe.clone(), String::new()),
            };
            let mut candidate = safe;
            let mut n = 1;
            // Names compared without case: on macOS's and Windows' disks
            // "Report.pdf" is "report.pdf", and would replace it.
            while taken.contains(&candidate.to_lowercase()) || dir.join(&candidate).exists() {
                taken.insert(candidate.to_lowercase());
                n += 1;
                candidate = format!("{stem} ({n}){ext}");
            }
            taken.insert(candidate.to_lowercase());
            dir.join(candidate)
        })
        .collect()
}

/// How saving several attachments went.
#[derive(Debug, Default)]
pub struct Saved {
    pub saved: usize,
    /// Those that couldn't be: their names and why.
    pub failed: Vec<(String, String)>,
}

/// The other person of a message: who sent it, or for mail you sent (from
/// one of `own` addresses, lowercase), the first person it went to. Their
/// name (empty when the header gives none) and address, lowercase.
pub fn counterpart(m: &Mail, own: &HashSet<String>) -> Option<(String, String)> {
    let from = m.from.trim().to_lowercase();
    let mine = m.folder == Folder::Sent || own.contains(&from);
    if !mine && !from.is_empty() {
        return Some((m.sender.trim().to_owned(), from));
    }
    let people = mailparse::addrparse(&m.to).ok()?;
    people.iter().find_map(|addr| {
        let infos: Vec<&mailparse::SingleInfo> = match addr {
            mailparse::MailAddr::Single(info) => vec![info],
            mailparse::MailAddr::Group(group) => group.addrs.iter().collect(),
        };
        infos.into_iter().find_map(|info| {
            let email = info.addr.trim().to_lowercase();
            (!email.is_empty() && !own.contains(&email))
                .then(|| (info.display_name.clone().unwrap_or_default().trim().to_owned(), email))
        })
    })
}

/// The address book's card for `address` (lowercase): a person's, not a
/// group's.
pub fn card_for<'a>(cards: &'a [Card], address: &str) -> Option<&'a Card> {
    cards
        .iter()
        .find(|c| !c.group && c.emails.iter().any(|e| e.trim().eq_ignore_ascii_case(address)))
}

/// The events of `agenda` (soonest first) that `address` (lowercase)
/// organizes or is invited to, cancelled ones and tasks aside: up to `limit`.
pub fn events_with(agenda: Vec<Event>, address: &str, limit: usize) -> Vec<Event> {
    agenda
        .into_iter()
        .filter(|e| !e.cancelled && !e.task)
        .filter(|e| {
            e.organizer.as_ref().is_some_and(|o| o.email.eq_ignore_ascii_case(address))
                || e.attendees.iter().any(|a| a.email.eq_ignore_ascii_case(address))
        })
        .take(limit)
        .collect()
}

/// What the reader's contact panel shows of the person a message is with.
#[derive(Clone, Debug, Default)]
pub struct Person {
    /// Their address, lowercase.
    pub address: String,
    /// Their name: the card's, else the message's.
    pub name: String,
    pub card: Option<Card>,
    /// Their latest mail, but the message open.
    pub recent: Vec<Mail>,
    /// The attachments of their latest mail, newest first.
    pub attachments: Vec<Item>,
    /// Events with them from now on, soonest first.
    pub events: Vec<Event>,
}

/// The most of each the contact panel lists.
pub const PANEL_RECENT: usize = 8;
pub const PANEL_ATTACHMENTS: usize = 8;
pub const PANEL_EVENTS: usize = 5;
/// How far ahead the contact panel looks for events.
pub const PANEL_EVENT_DAYS: i64 = 120;

impl Person {
    /// The panel's lists from `mail` with them (newest first), but the
    /// message `open`.
    pub fn from_mail(address: String, name: String, card: Option<Card>, mail: Vec<Mail>, open: &str, events: Vec<Event>) -> Self {
        let others: Vec<Mail> = mail.into_iter().filter(|m| m.id != open).collect();
        let recent = others.iter().take(PANEL_RECENT).cloned().collect();
        let attachments = items(others.into_iter().filter(|m| !m.attachments().is_empty()).collect())
            .into_iter()
            .take(PANEL_ATTACHMENTS)
            .collect();
        let name = card
            .as_ref()
            .map(|c| c.name.trim().to_owned())
            .filter(|n| !n.is_empty())
            .unwrap_or(name);
        Self {
            address,
            name,
            card,
            recent,
            attachments,
            events,
        }
    }
}

impl Backend {
    /// `m` as its attachments can be read: as it is when its source is
    /// here or its attachments download one at a time, else read (and so
    /// downloaded) first.
    pub fn attachments_ready(&self, a: &Account, m: &Mail) -> Result<Mail> {
        if m.attachments().iter().any(|x| !x.section.is_empty()) || self.store.has_raw(&m.id)? {
            return Ok(m.clone());
        }
        self.read(a, m)
    }

    /// One attachment's bytes, downloading its message first if needed.
    pub fn library_attachment(&self, a: &Account, item: &Item) -> Result<Vec<u8>> {
        let m = self.attachments_ready(a, &item.mail)?;
        let part = same_part(&m, item.attachment())?;
        self.attachment(a, &m, part)
    }

    /// One attachment in a file of its own (Picacho's copy, as the
    /// reader's Open uses), downloading its message first if needed.
    pub fn library_attachment_file(&self, a: &Account, item: &Item) -> Result<PathBuf> {
        let m = self.attachments_ready(a, &item.mail)?;
        let attachment = item.attachment();
        let part = same_part(&m, attachment)?;
        self.attachment_file(a, &m, part, &attachment.name)
    }

    /// Saves one attachment as `path`, downloading its message first if
    /// needed, as the reader's Save does.
    pub fn save_library_attachment(&self, a: &Account, item: &Item, path: &Path) -> Result<()> {
        let m = self.attachments_ready(a, &item.mail)?;
        let part = same_part(&m, item.attachment())?;
        let written = self.attachment_to(a, &m, part, path)?;
        crate::manage::backend::opened::mark_from_internet(path);
        self.note_saved_attachment(&m, part, path, &written);
        Ok(())
    }

    /// The text of an attachment, as the index reads it, for a preview.
    pub fn attachment_preview_text(&self, a: &Account, item: &Item) -> Result<String> {
        let bytes = self.library_attachment(a, item)?;
        let attachment = item.attachment();
        std::panic::catch_unwind(|| {
            crate::manage::attachment_text::part_text(&bytes, &attachment.mime, &attachment.name)
        })
        .map_err(|_| anyhow::anyhow!(t!("manage-attachments-couldnt-read")))
    }

    /// Saves `items` (each with its account) into `dir`, each under its own
    /// name, numbered apart from the others and from what is there. A
    /// message is downloaded once for all its attachments; one that fails
    /// leaves the others to be saved.
    pub fn save_attachments(&self, items: &[(Account, Item)], dir: &Path) -> Saved {
        let paths = file_names(items.iter().map(|(_, i)| i.attachment().name.as_str()), dir);
        let mut ready: HashMap<String, Result<Mail, String>> = HashMap::new();
        let mut out = Saved::default();
        for ((account, item), path) in items.iter().zip(paths) {
            let name = item.attachment().name.clone();
            let m = ready
                .entry(item.mail.id.clone())
                .or_insert_with(|| self.attachments_ready(account, &item.mail).map_err(|e| format!("{e:#}")))
                .clone();
            let result = m.map_err(anyhow::Error::msg).and_then(|m| {
                let part = same_part(&m, item.attachment())?;
                let written = self.attachment_to(account, &m, part, &path)?;
                crate::manage::backend::opened::mark_from_internet(&path);
                self.note_saved_attachment(&m, part, &path, &written);
                Ok(())
            });
            match result {
                Ok(()) => out.saved += 1,
                Err(e) => out.failed.push((name, format!("{e:#}"))),
            }
        }
        out
    }
}

/// The part of `m` (read again, perhaps) that is the attachment listed as
/// `listed`: the same part, if it still has that name, else the one with
/// its name.
fn same_part(m: &Mail, listed: &Attachment) -> Result<usize> {
    let all = m.attachments();
    all.iter()
        .find(|x| x.part == listed.part && x.name == listed.name)
        .or_else(|| all.iter().find(|x| x.name == listed.name))
        .or_else(|| all.iter().find(|x| x.part == listed.part))
        .map(|x| x.part)
        .with_context(|| t!("manage-backend-attachment-not-found"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manage::store::Store;

    fn attachment(name: &str, mime: &str, size: usize, part: usize) -> Attachment {
        Attachment {
            name: name.into(),
            size,
            mime: mime.into(),
            part,
            section: String::new(),
            encoding: String::new(),
        }
    }

    fn mail(id: &str, account: &str, from: &str, date: i64, files: Vec<Attachment>) -> Mail {
        let mut m = Mail {
            id: id.into(),
            account: account.into(),
            from: from.into(),
            sender: from.split('@').next().unwrap_or_default().into(),
            to: "me@example.com".into(),
            subject: format!("Mail {id}"),
            message_id: format!("<{id}@example.com>"),
            date,
            has_attachments: true,
            ..Mail::default()
        };
        m.set_attachments(files);
        m
    }

    #[test]
    fn kinds_by_type_then_name() {
        let kind = |name: &str, mime: &str| Kind::of(&attachment(name, mime, 1, 1));
        assert_eq!(kind("photo.JPG", "image/jpeg"), Kind::Image);
        assert_eq!(kind("scan", "application/pdf"), Kind::Pdf);
        assert_eq!(kind("scan.pdf", "application/octet-stream"), Kind::Pdf);
        assert_eq!(
            kind("q3.xlsx", "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"),
            Kind::Spreadsheet
        );
        assert_eq!(kind("deck.pptx", ""), Kind::Presentation);
        assert_eq!(kind("figures.csv", "text/plain"), Kind::Spreadsheet);
        assert_eq!(kind("invite.ics", "text/calendar"), Kind::Calendar);
        assert_eq!(kind("notes.txt", "text/plain"), Kind::Document);
        assert_eq!(kind("fwd.eml", "message/rfc822"), Kind::Email);
        assert_eq!(kind("src.tar.gz", "application/gzip"), Kind::Archive);
        assert_eq!(kind("blob", "application/x-unknown"), Kind::Other);
    }

    #[test]
    fn filters_by_kind_words_content_and_person() {
        let all = items(vec![
            mail("1", "a", "ana@example.com", 30, vec![
                attachment("Invoice-0042.pdf", "application/pdf", 10, 1),
                attachment("logo.png", "image/png", 5, 2),
            ]),
            mail("2", "a", "bo@example.com", 20, vec![attachment("minutes.docx", "", 20, 1)]),
        ]);
        let keys = |f: &Filter| all.iter().filter(|i| f.keeps(i)).map(|i| i.attachment().name.clone()).collect::<Vec<_>>();
        assert_eq!(keys(&Filter::default()).len(), 3);
        assert_eq!(keys(&Filter { kind: Some(Kind::Image), ..Filter::default() }), ["logo.png"]);
        assert_eq!(keys(&Filter { words: words("invoice"), ..Filter::default() }), ["Invoice-0042.pdf"]);
        // By sender, or by subject: every attachment of the message.
        assert_eq!(keys(&Filter { words: words("bo"), ..Filter::default() }), ["minutes.docx"]);
        // What attachments say: only those whose text is read.
        let content = Some(Arc::new(HashSet::from(["1".to_string()])));
        assert_eq!(
            keys(&Filter { words: words("tender"), content, ..Filter::default() }),
            ["Invoice-0042.pdf"]
        );
        assert_eq!(
            keys(&Filter { person: Some("ana@example.com".into()), ..Filter::default() }),
            ["Invoice-0042.pdf", "logo.png"]
        );
        assert_eq!(words(r#"Q3 "Board Pack" draft"#), ["q3", "board pack", "draft"]);
    }

    #[test]
    fn sorts_and_names_saved_files_apart() {
        let mut all = items(vec![
            mail("1", "a", "ana@example.com", 10, vec![attachment("b.pdf", "", 5, 1)]),
            mail("2", "a", "ana@example.com", 30, vec![attachment("A.pdf", "", 1, 1)]),
            mail("3", "a", "ana@example.com", 20, vec![attachment("c.pdf", "", 9, 1)]),
        ]);
        let names = |all: &[Item]| all.iter().map(|i| i.attachment().name.clone()).collect::<Vec<_>>();
        sort(&mut all, Sort::Newest);
        assert_eq!(names(&all), ["A.pdf", "c.pdf", "b.pdf"]);
        sort(&mut all, Sort::Largest);
        assert_eq!(names(&all), ["c.pdf", "b.pdf", "A.pdf"]);
        sort(&mut all, Sort::Name);
        assert_eq!(names(&all), ["A.pdf", "b.pdf", "c.pdf"]);
        sort(&mut all, Sort::Oldest);
        assert_eq!(names(&all), ["b.pdf", "c.pdf", "A.pdf"]);

        let dir = std::env::temp_dir().join(format!("picacho-names-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("report.pdf"), b"there already").unwrap();
        let paths = file_names(["report.pdf", "Report.pdf", "../notes", ".hidden.txt", "notes"], &dir);
        let names: Vec<String> = paths.iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(names, ["report (2).pdf", "Report (3).pdf", "notes", "hidden.txt", "notes (2)"]);
        assert!(paths.iter().all(|p| p.parent() == Some(dir.as_path())));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_other_person_of_a_message() {
        let own = HashSet::from(["me@example.com".to_string()]);
        let received = mail("1", "a", "Ana@Example.com", 1, vec![]);
        assert_eq!(counterpart(&received, &own), Some(("Ana".into(), "ana@example.com".into())));
        let mut sent = mail("2", "a", "me@example.com", 1, vec![]);
        sent.to = "Me <me@example.com>, \"Bo Diaz\" <Bo@example.com>".into();
        assert_eq!(counterpart(&sent, &own), Some(("Bo Diaz".into(), "bo@example.com".into())));
        sent.to = "me@example.com".into();
        assert_eq!(counterpart(&sent, &own), None);
    }

    #[test]
    fn cards_and_events_with_a_person() {
        let cards = vec![
            Card { uid: "g".into(), name: "Team".into(), emails: vec!["ana@example.com".into()], group: true, ..Card::default() },
            Card { uid: "1".into(), name: "Ana Silva".into(), emails: vec!["Ana@Example.com".into()], ..Card::default() },
        ];
        assert_eq!(card_for(&cards, "ana@example.com").map(|c| c.uid.as_str()), Some("1"));
        assert!(card_for(&cards, "bo@example.com").is_none());
        let event = |uid: &str, who: &str, cancelled: bool| Event {
            uid: uid.into(),
            summary: uid.into(),
            attendees: vec![crate::sync::ical::Attendee { email: who.into(), ..Default::default() }],
            cancelled,
            ..Event::default()
        };
        let found = events_with(
            vec![event("a", "ANA@example.com", false), event("b", "bo@example.com", false), event("c", "ana@example.com", true)],
            "ana@example.com",
            5,
        );
        assert_eq!(found.iter().map(|e| e.uid.as_str()).collect::<Vec<_>>(), ["a"]);
        let person = Person::from_mail(
            "ana@example.com".into(),
            "Ana".into(),
            card_for(&cards, "ana@example.com").cloned(),
            vec![
                mail("open", "a", "ana@example.com", 3, vec![attachment("x.pdf", "", 1, 1)]),
                mail("2", "a", "ana@example.com", 2, vec![]),
                mail("3", "a", "ana@example.com", 1, vec![attachment("y.pdf", "", 1, 1)]),
            ],
            "open",
            found,
        );
        assert_eq!(person.name, "Ana Silva");
        assert_eq!(person.recent.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["2", "3"]);
        assert_eq!(person.attachments.iter().map(|i| i.attachment().name.as_str()).collect::<Vec<_>>(), ["y.pdf"]);
    }

    /// The store lists mail with attachments by mailbox, account or every
    /// one, one copy each; finds attachments by what they say; and finds
    /// the mail with a person, address by address.
    #[test]
    fn store_lists_attachments_and_mail_with_a_person() -> Result<()> {
        let p = std::env::temp_dir().join(format!("picacho-attachments-{}", uuid::Uuid::new_v4()));
        let store = Store::open(&p)?;
        let pdf = || vec![attachment("plan.pdf", "application/pdf", 10, 1)];
        let mut archived = mail("a:2", "a", "ana@example.com", 20, pdf());
        archived.folder = Folder::Archive;
        // Gmail's copy of the same message under another label.
        let mut copy = archived.clone();
        copy.id = "a:2b".into();
        copy.folder = Folder::Other;
        copy.remote_folder = "Work".into();
        let mut trashed = mail("a:3", "a", "ana@example.com", 30, pdf());
        trashed.folder = Folder::Trash;
        // Only its headers: it has attachments, not yet listed.
        let mut headers_only = mail("a:4", "a", "bo@example.com", 40, vec![]);
        headers_only.has_attachments = true;
        let mut diana = mail("b:1", "b", "diana@example.com", 50, pdf());
        diana.to = "me@example.com".into();
        let mut to_ana = mail("b:2", "b", "me@example.com", 60, vec![]);
        to_ana.to = "Ana <ana@example.com>".into();
        to_ana.has_attachments = false;
        store.save_messages(&[
            mail("a:1", "a", "ana@example.com", 10, pdf()),
            archived,
            copy,
            trashed,
            headers_only,
            diana,
            to_ana,
        ])?;
        let accounts = ["a".to_string(), "b".to_string()];
        let (all, unlisted) = store.attachment_mail(&accounts, None, 100)?;
        // Of the two copies, the label's is kept over the archive's.
        assert_eq!(all.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["b:1", "a:2b", "a:1"]);
        assert_eq!(unlisted, 1);
        let (inbox, _) = store.attachment_mail(&accounts[..1], Some(&Folder::Inbox.into()), 100)?;
        assert_eq!(inbox.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["a:1"]);
        let (trash, _) = store.attachment_mail(&accounts[..1], Some(&Folder::Trash.into()), 100)?;
        assert_eq!(trash.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["a:3"]);
        let (work, _) = store.attachment_mail(&accounts[..1], Some(&Mailbox::Remote("Work".into())), 100)?;
        assert_eq!(work.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["a:2b"]);
        let (limited, _) = store.attachment_mail(&accounts, None, 2)?;
        assert_eq!(limited.len(), 2);

        // Mail with Ana: from her, and to her; not Diana's, nor Trash's.
        let with = store.mail_with("Ana@example.com", &accounts, 10)?;
        assert_eq!(with.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["b:2", "a:2b", "a:1"]);
        assert!(store.mail_with("ana@example.com", &accounts[..1], 10)?.iter().all(|m| m.account == "a"));
        assert!(store.mail_with("", &accounts, 10)?.is_empty());

        // What attachments say, once their message is downloaded.
        let raw = "From: ana@example.com\r\nSubject: Plan\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=b\r\n\r\n--b\r\nContent-Type: text/plain\r\n\r\nSee attached.\r\n--b\r\nContent-Type: text/csv; name=\"q3.csv\"\r\nContent-Disposition: attachment; filename=\"q3.csv\"\r\n\r\nregion,revenue\r\nPatagonia,1200\r\n--b--\r\n";
        store.save_raw("a:1", raw.as_bytes())?;
        while store.index_attachments(10)? > 0 {}
        let words = |w: &[&str]| w.iter().map(|w| w.to_string()).collect::<Vec<_>>();
        assert_eq!(store.attachment_text_matches(&words(&["patag"]), 10)?, HashSet::from(["a:1".to_string()]));
        assert!(store.attachment_text_matches(&words(&["patagonia", "nowhere"]), 10)?.is_empty());
        // Words of the message's own text aren't the attachments'.
        assert!(store.attachment_text_matches(&words(&["see"]), 10)?.is_empty());
        assert!(store.attachment_text_matches(&[], 10)?.is_empty());
        drop(store);
        std::fs::remove_dir_all(p)?;
        Ok(())
    }
}

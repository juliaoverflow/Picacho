//! The address book: people from a CardDAV server, from imported vCard
//! files and added here, offered in the composer's address fields before
//! people seen in mail. Kept in the store's preferences; the CardDAV
//! password is kept in the system keychain.
//!
//! Contacts added or edited here go to the server when one is set up
//! (and the card came from it, or is new), and stay on this computer
//! otherwise.
use super::*;
use crate::sync::carddav::{self, Card};

/// The cards, as JSON.
const CARDS: &str = "contacts";
/// The CardDAV server (address, user name and address book), as JSON.
const SERVER: &str = "contacts_server";
/// When the address book was last read from the server (Unix seconds).
const SYNCED: &str = "contacts_synced";
/// The keychain entry with the CardDAV password.
#[cfg_attr(test, allow(dead_code))]
const PASSWORD: &str = "contacts:carddav";

/// Whether a card came from the server (rather than a file or this computer).
pub fn from_server(card: &Card) -> bool {
    !card.href.is_empty()
}

/// Where a JMAP server's ContactCard state is kept in `Server::states`.
const JMAP_STATE_KEY: &str = "jmap";

impl Backend {
    /// Every card, the server's and this computer's.
    pub fn contacts(&self) -> Vec<Card> {
        self.store
            .preference(CARDS)
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_default()
    }
    fn save_contacts(&self, cards: &[Card]) -> Result<()> {
        self.store
            .set_preference(CARDS, &serde_json::to_string(cards)?)
    }
    /// The CardDAV server address book sync uses, if one is set up.
    pub fn contacts_server(&self) -> Option<carddav::Server> {
        self.store
            .preference(SERVER)
            .and_then(|json| serde_json::from_str(&json).ok())
            .filter(|s: &carddav::Server| !s.url.is_empty())
    }
    /// When the address book was last read from the server.
    pub fn contacts_synced(&self) -> Option<i64> {
        self.store.preference(SYNCED)?.parse().ok()
    }
    fn contacts_password(&self) -> Result<String> {
        #[cfg(test)]
        if let Some(s) = &self.test_secret {
            return Ok(s.password.clone());
        }
        store::read_secret_text(PASSWORD)?
            .context("The contacts server's password is missing. Set it up again.")
    }
    /// Signs in to a CardDAV server and reads its address books; the server
    /// and password are kept only once that works.
    pub fn connect_contacts(&self, mut server: carddav::Server, password: &str) -> Result<usize> {
        let fetched = carddav::fetch_changes(&server, password, self.tls_connector()?, &[])?;
        let cards = fetched.cards;
        server.book = fetched.book;
        server.states = fetched.states;
        server.vcard4 = fetched.vcard4;
        server.max_size = fetched.max_size;
        server.books = fetched.books;
        #[cfg(not(test))]
        store::write_secret_text(PASSWORD, password)?;
        self.store
            .set_preference(SERVER, &serde_json::to_string(&server)?)?;
        self.replace_server_cards(cards)
    }
    /// Uses the address books of a JMAP account's server (JMAP for
    /// Contacts) instead of a CardDAV server: its sign-in is the account's.
    pub fn connect_jmap_contacts(&self, a: &Account) -> Result<usize> {
        ensure!(a.provider == Provider::Jmap, "Only JMAP accounts keep contacts this way.");
        self.with_jmap_contacts(a, crate::sync::jscal::ensure_contacts)?;
        let server = carddav::Server {
            url: a.incoming_host.clone(),
            user: a.email.clone(),
            jmap_account: a.id.clone(),
            ..Default::default()
        };
        self.store
            .set_preference(SERVER, &serde_json::to_string(&server)?)?;
        self.sync_contacts()
    }
    fn with_jmap_contacts<T>(
        &self,
        a: &Account,
        work: impl FnOnce(&crate::sync::jmap::Session) -> Result<T>,
    ) -> Result<T> {
        let lock = self.account_lock(&a.id);
        let _operation = lock.lock().unwrap();
        self.with_jmap(a, work)
    }
    /// The JMAP account contacts come from, if they come from one.
    fn contacts_jmap(&self, server: &carddav::Server) -> Result<Option<Account>> {
        if server.jmap_account.is_empty() {
            return Ok(None);
        }
        Ok(Some(
            self.store
                .accounts()?
                .into_iter()
                .find(|a| a.id == server.jmap_account)
                .context("The account your contacts come from was removed. Set up contacts again.")?,
        ))
    }
    /// Makes a new address book on the server (MKCOL, or AddressBook/set),
    /// which new contacts then go to.
    pub fn create_address_book(&self, name: &str) -> Result<()> {
        ensure!(!name.trim().is_empty(), "Give the address book a name.");
        let mut server = self
            .contacts_server()
            .context("Set up a contacts server in Settings → Contacts first.")?;
        server.book = match self.contacts_jmap(&server)? {
            Some(a) => format!(
                "jmap:{}",
                self.with_jmap_contacts(&a, |s| crate::sync::jscal::create_address_book(s, name.trim()))?
            ),
            None => carddav::create_book(
                &server,
                &self.contacts_password()?,
                self.tls_connector()?,
                name.trim(),
            )?,
        };
        self.store
            .set_preference(SERVER, &serde_json::to_string(&server)?)
    }
    /// The server's address books: (href, name, whether it can be written).
    pub fn address_books(&self) -> Result<Vec<(String, String, bool)>> {
        let server = self
            .contacts_server()
            .context("No contacts server is set up.")?;
        if let Some(a) = self.contacts_jmap(&server)? {
            return self.with_jmap_contacts(&a, |s| {
                Ok(crate::sync::jscal::address_books(s)?
                    .into_iter()
                    .map(|b| (format!("jmap:{}", b.id), b.name, b.writable))
                    .collect())
            });
        }
        carddav::books(&server, &self.contacts_password()?, self.tls_connector()?)
    }
    /// Asks the server for its contacts whose name, nickname or address
    /// contains `text` (RFC 6352 §8.6), at most 50, and adds those not
    /// here yet: for address books and directories that don't list all
    /// they hold. Returns how many were new.
    pub fn search_server_contacts(&self, text: &str) -> Result<usize> {
        let server = self.contacts_server().context("No contacts server is set up.")?;
        ensure!(self.contacts_jmap(&server)?.is_none(), "This server's contacts are all read already.");
        let found = carddav::search(&server, &self.contacts_password()?, self.tls_connector()?, text, 50)?;
        let mut all = self.contacts();
        let before = all.len();
        for card in found {
            if !all.iter().any(|c| c.href == card.href && !c.href.is_empty()) {
                all.push(card);
            }
        }
        let added = all.len() - before;
        if added > 0 {
            self.save_contacts(&all)?;
        }
        Ok(added)
    }
    /// The account's storage on the contacts server (used, left), where
    /// it says (RFC 4331); JMAP servers don't.
    pub fn contacts_quota(&self) -> Result<Option<(u64, u64)>> {
        let server = self.contacts_server().context("No contacts server is set up.")?;
        if self.contacts_jmap(&server)?.is_some() {
            return Ok(None);
        }
        carddav::quota(&server, &self.contacts_password()?, self.tls_connector()?)
    }
    /// Renames an address book on the server.
    pub fn rename_address_book(&self, href: &str, name: &str) -> Result<()> {
        ensure!(!name.trim().is_empty(), "Give the address book a name.");
        let server = self
            .contacts_server()
            .context("No contacts server is set up.")?;
        match self.contacts_jmap(&server)? {
            Some(a) => {
                let id = href.strip_prefix("jmap:").context("Not a JMAP address book.")?;
                self.with_jmap_contacts(&a, |s| {
                    crate::sync::jscal::rename_address_book(s, id, name.trim())
                })
            }
            None => carddav::rename_book(
                &server,
                &self.contacts_password()?,
                self.tls_connector()?,
                href,
                name,
            ),
        }
    }
    /// Deletes an address book and its cards from the server, then reads
    /// what's left.
    pub fn delete_address_book(&self, href: &str) -> Result<usize> {
        let mut server = self
            .contacts_server()
            .context("No contacts server is set up.")?;
        match self.contacts_jmap(&server)? {
            Some(a) => {
                let id = href.strip_prefix("jmap:").context("Not a JMAP address book.")?;
                self.with_jmap_contacts(&a, |s| crate::sync::jscal::delete_address_book(s, id))?;
                server.states.remove(JMAP_STATE_KEY);
            }
            None => {
                carddav::delete_book(
                    &server,
                    &self.contacts_password()?,
                    self.tls_connector()?,
                    href,
                )?;
                server.states.remove(href);
            }
        }
        if server.book == href {
            server.book.clear();
        }
        self.store
            .set_preference(SERVER, &serde_json::to_string(&server)?)?;
        // Its cards go from here too.
        let kept: Vec<Card> = self
            .contacts()
            .into_iter()
            .filter(|c| !(c.href.starts_with(href.trim_end_matches('/')) && !href.starts_with("jmap:")))
            .collect();
        self.save_contacts(&kept)?;
        self.sync_contacts()
    }
    /// Reads the address books again from the server set up before.
    pub fn sync_contacts(&self) -> Result<usize> {
        let mut server = self
            .contacts_server()
            .context("No contacts server is set up.")?;
        if let Some(a) = self.contacts_jmap(&server)? {
            // Once read, only what changed since (ContactCard/changes).
            let since = server
                .states
                .get(JMAP_STATE_KEY)
                .map(|s| s.sync_token.clone())
                .filter(|t| !t.is_empty());
            let known: Vec<Card> = self.contacts().into_iter().filter(from_server).collect();
            let (cards, book, state) = self.with_jmap_contacts(&a, |s| {
                if let Some(since) = &since
                    && let Some((changed, gone, state)) = crate::sync::jscal::card_changes(s, since)?
                {
                    let gone: std::collections::HashSet<&str> = gone.iter().map(String::as_str).collect();
                    let replaced: std::collections::HashSet<&str> =
                        changed.iter().map(|c| c.href.as_str()).collect();
                    let mut cards: Vec<Card> = known
                        .into_iter()
                        .filter(|k| !gone.contains(k.href.as_str()) && !replaced.contains(k.href.as_str()))
                        .collect();
                    cards.extend(changed);
                    return Ok((cards, String::new(), state));
                }
                crate::sync::jscal::cards_in(s)
            })?;
            let mut changed = false;
            if !book.is_empty() && (server.book.is_empty() || !server.book.starts_with("jmap:")) {
                server.book = book;
                changed = true;
            }
            let entry = server.states.entry(JMAP_STATE_KEY.to_owned()).or_default();
            if entry.sync_token != state {
                entry.sync_token = state;
                changed = true;
            }
            if changed {
                self.store
                    .set_preference(SERVER, &serde_json::to_string(&server)?)?;
            }
            return self.replace_server_cards(cards);
        }
        // Only what changed since the last sync is downloaded, where the
        // server can say (sync token or ctag).
        let known: Vec<Card> = self.contacts().into_iter().filter(from_server).collect();
        let fetched = carddav::fetch_changes(
            &server,
            &self.contacts_password()?,
            self.tls_connector()?,
            &known,
        )?;
        if server.book != fetched.book
            || server.states != fetched.states
            || server.vcard4 != fetched.vcard4
            || server.max_size != fetched.max_size
            || server.books != fetched.books
        {
            server.book = fetched.book;
            server.states = fetched.states;
            server.vcard4 = fetched.vcard4;
            server.max_size = fetched.max_size;
            server.books = fetched.books;
            self.store
                .set_preference(SERVER, &serde_json::to_string(&server)?)?;
        }
        self.replace_server_cards(fetched.cards)
    }
    fn replace_server_cards(&self, cards: Vec<Card>) -> Result<usize> {
        let count = cards.len();
        let mut all: Vec<Card> = self
            .contacts()
            .into_iter()
            .filter(|c| !from_server(c))
            .collect();
        all.extend(cards);
        self.save_contacts(&all)?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        self.store.set_preference(SYNCED, &now.to_string())?;
        Ok(count)
    }
    /// Stops reading contacts from the server, forgetting its cards.
    pub fn disconnect_contacts(&self) -> Result<()> {
        let kept: Vec<Card> = self
            .contacts()
            .into_iter()
            .filter(|c| !from_server(c))
            .collect();
        self.save_contacts(&kept)?;
        self.store.set_preference(SERVER, "")?;
        self.store.set_preference(SYNCED, "")?;
        #[cfg(not(test))]
        store::delete_secret(PASSWORD)?;
        Ok(())
    }
    /// Adds a contact, or saves changes to one (found by UID): on the
    /// server when one is set up and can take it, else on this computer.
    /// Returns the card as saved.
    pub fn save_contact(&self, mut card: Card) -> Result<Card> {
        card.name = card.name.trim().to_owned();
        card.emails = card
            .emails
            .iter()
            .map(|e| e.trim().to_owned())
            .filter(|e| !e.is_empty())
            .collect();
        card.phones = card
            .phones
            .iter()
            .map(|p| p.trim().to_owned())
            .filter(|p| !p.is_empty())
            .collect();
        ensure!(
            !card.name.is_empty() || !card.emails.is_empty(),
            "Give the contact a name or an email address."
        );
        for email in &card.emails {
            ensure!(
                crate::sync::smtp::recipients(email).is_ok_and(|r| r.len() == 1),
                "“{email}” isn't a valid email address."
            );
        }
        if card.uid.is_empty() {
            card.uid = uuid::Uuid::new_v4().to_string();
        }
        let saved = match self.contacts_server() {
            Some(server) if from_server(&card) || !server.book.is_empty() => {
                match self.contacts_jmap(&server)? {
                    Some(a) => self.with_jmap_contacts(&a, |s| {
                        crate::sync::jscal::save_card(s, &card, &server.book)
                    })?,
                    None => carddav::save(
                        &server,
                        &self.contacts_password()?,
                        self.tls_connector()?,
                        &card,
                    )?,
                }
            }
            _ => card,
        };
        let mut all = self.contacts();
        all.retain(|c| c.uid != saved.uid);
        all.push(saved.clone());
        self.save_contacts(&all)?;
        Ok(saved)
    }
    /// Removes a contact, from the server too when it's there.
    pub fn delete_contact(&self, uid: &str) -> Result<()> {
        let mut all = self.contacts();
        let Some(card) = all.iter().find(|c| c.uid == uid).cloned() else {
            return Ok(());
        };
        if from_server(&card)
            && let Some(server) = self.contacts_server()
        {
            match self.contacts_jmap(&server)? {
                Some(a) => self.with_jmap_contacts(&a, |s| crate::sync::jscal::delete_card(s, &card))?,
                None => carddav::delete(
                    &server,
                    &self.contacts_password()?,
                    self.tls_connector()?,
                    &card,
                )?,
            }
        }
        all.retain(|c| c.uid != uid);
        self.save_contacts(&all)
    }
    /// Adds the cards in vCard files; a card with the same UID is replaced.
    pub fn import_contacts(&self, paths: &[std::path::PathBuf]) -> Result<usize> {
        let mut added = Vec::new();
        for path in paths {
            // Address books are megabytes at most (photos included): a file
            // past this isn't one, and isn't read into memory whole.
            ensure!(
                std::fs::metadata(path)?.len() <= 256 * 1024 * 1024,
                "{} is too large to be an address book.",
                path.display()
            );
            let bytes = std::fs::read(path)?;
            added.extend(
                carddav::parse_vcards(&String::from_utf8_lossy(&bytes))
                    .into_iter()
                    // People with an address or number, and groups.
                    .filter(|c| !c.emails.is_empty() || !c.phones.is_empty() || c.group)
                    .map(|mut c| {
                        // Imported cards are this computer's, not the server's.
                        c.href.clear();
                        c.etag.clear();
                        c
                    }),
            );
        }
        ensure!(
            !added.is_empty(),
            "No contacts were found in that file."
        );
        let count = added.len();
        let mut all = self.contacts();
        for card in added {
            all.retain(|c| c.uid != card.uid);
            all.push(card);
        }
        self.save_contacts(&all)?;
        Ok(count)
    }
    /// Writes every card to one vCard file.
    pub fn export_contacts(&self, path: &std::path::Path) -> Result<usize> {
        let cards = self.contacts();
        std::fs::write(path, carddav::write_vcards(&cards))?;
        Ok(cards.len())
    }
    /// Forgets the contacts kept on this computer (the server's stay).
    pub fn clear_imported_contacts(&self) -> Result<()> {
        let kept: Vec<Card> = self.contacts().into_iter().filter(from_server).collect();
        self.save_contacts(&kept)
    }
}

/// The address book as (name, address) pairs, one per address, for
/// suggestions.
pub fn people(cards: &[Card]) -> Vec<(String, String)> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for card in cards {
        for email in &card.emails {
            if seen.insert(email.to_lowercase()) {
                let name = if card.name.is_empty() {
                    email.clone()
                } else {
                    card.name.clone()
                };
                out.push((name, email.clone()));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imported_added_and_server_cards_are_kept_apart() -> Result<()> {
        let path = std::env::temp_dir().join(format!("picacho-contacts-{}", uuid::Uuid::new_v4()));
        let b = Backend::new(Arc::new(Store::open(&path)?));
        let file = path.join("people.vcf");
        std::fs::write(
            &file,
            "BEGIN:VCARD\nFN:Ana\nEMAIL:ana@example.org\nEMAIL:ANA@example.org\nUID:a\nEND:VCARD\n",
        )?;
        assert_eq!(b.import_contacts(std::slice::from_ref(&file))?, 1);
        assert_eq!(b.import_contacts(std::slice::from_ref(&file))?, 1);
        b.replace_server_cards(vec![Card {
            uid: "1".into(),
            name: "Bo".into(),
            emails: vec!["bo@example.se".into()],
            href: "https://dav.example/1.vcf".into(),
            ..Default::default()
        }])?;
        // Added here without a server: kept on this computer.
        let cy = b.save_contact(Card {
            name: " Cy ".into(),
            emails: vec!["cy@x.net".into(), " ".into()],
            ..Default::default()
        })?;
        assert_eq!(cy.name, "Cy");
        assert!(!cy.uid.is_empty() && cy.href.is_empty());
        assert!(
            b.save_contact(Card {
                emails: vec!["not an address".into()],
                ..Default::default()
            })
            .is_err()
        );
        assert!(b.save_contact(Card::default()).is_err());
        let mut edited = cy.clone();
        edited.phones = vec!["555".into()];
        b.save_contact(edited)?;
        assert_eq!(b.contacts().len(), 3);
        assert_eq!(
            b.contacts()
                .iter()
                .find(|c| c.uid == cy.uid)
                .unwrap()
                .phones,
            ["555"]
        );
        assert_eq!(people(&b.contacts()).len(), 3);
        let out = path.join("out.vcf");
        assert_eq!(b.export_contacts(&out)?, 3);
        b.delete_contact(&cy.uid)?;
        b.disconnect_contacts()?;
        assert_eq!(b.contacts().len(), 1);
        b.clear_imported_contacts()?;
        assert!(b.contacts().is_empty());
        drop(b);
        std::fs::remove_dir_all(path)?;
        Ok(())
    }
}

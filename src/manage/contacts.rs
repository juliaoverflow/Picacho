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
        let (cards, book) = carddav::fetch(&server, password, self.tls_connector()?)?;
        server.book = book;
        #[cfg(not(test))]
        store::write_secret_text(PASSWORD, password)?;
        self.store
            .set_preference(SERVER, &serde_json::to_string(&server)?)?;
        self.replace_server_cards(cards)
    }
    /// Reads the address books again from the server set up before.
    pub fn sync_contacts(&self) -> Result<usize> {
        let mut server = self
            .contacts_server()
            .context("No contacts server is set up.")?;
        let (cards, book) =
            carddav::fetch(&server, &self.contacts_password()?, self.tls_connector()?)?;
        if server.book != book {
            server.book = book;
            self.store
                .set_preference(SERVER, &serde_json::to_string(&server)?)?;
        }
        self.replace_server_cards(cards)
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
            Some(server) if from_server(&card) || !server.book.is_empty() => carddav::save(
                &server,
                &self.contacts_password()?,
                self.tls_connector()?,
                &card,
            )?,
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
            carddav::delete(
                &server,
                &self.contacts_password()?,
                self.tls_connector()?,
                &card,
            )?;
        }
        all.retain(|c| c.uid != uid);
        self.save_contacts(&all)
    }
    /// Adds the cards in vCard files; a card with the same UID is replaced.
    pub fn import_contacts(&self, paths: &[std::path::PathBuf]) -> Result<usize> {
        let mut added = Vec::new();
        for path in paths {
            let bytes = std::fs::read(path)?;
            added.extend(
                carddav::parse_vcards(&String::from_utf8_lossy(&bytes))
                    .into_iter()
                    .filter(|c| !c.emails.is_empty())
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
            "No contacts with email addresses were found in that file."
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

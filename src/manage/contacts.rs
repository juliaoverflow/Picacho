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
use super::pim_sources::{self, Pim};

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

/// `base`'s key in the contacts source worked within (`pim_sources`).
fn key(base: &str) -> String {
    pim_sources::key(Pim::Contacts, base)
}
/// Whether this thread works within one contacts source, rather than
/// outside all of them (where reading puts them together).
fn within_one() -> bool {
    pim_sources::inside(Pim::Contacts)
}

impl Backend {
    fn card_pref(&self, base: &str) -> Option<String> {
        self.store.preference(&key(base))
    }
    fn set_card_pref(&self, base: &str, value: &str) -> Result<()> {
        self.store.set_preference(&key(base), value)
    }
    /// Runs `work` within contacts source `id`.
    pub fn in_contacts_source<T>(&self, id: &str, work: impl FnOnce() -> T) -> T {
        pim_sources::within(Pim::Contacts, id, work)
    }
    /// Every contacts source's id: the first (""), then the others.
    pub fn contacts_source_ids(&self) -> Vec<String> {
        pim_sources::source_ids(&self.store, Pim::Contacts)
    }
    /// The sources set up besides the first: (id, server, when last read).
    pub fn contacts_sources(&self) -> Vec<(String, carddav::Server, Option<i64>)> {
        pim_sources::extra_sources(&self.store, Pim::Contacts)
            .into_iter()
            .filter_map(|id| {
                let (server, synced) = self.in_contacts_source(&id, || (self.contacts_server(), self.contacts_synced()));
                Some((id, server?, synced))
            })
            .collect()
    }
    /// The first contacts source's server, as Settings shows it.
    pub fn primary_contacts_server(&self) -> Option<carddav::Server> {
        self.in_contacts_source("", || self.contacts_server())
    }
    /// What a contacts source is called: its account's address, or its server.
    pub fn contacts_source_label(&self, id: &str) -> String {
        self.in_contacts_source(id, || {
            let Some(server) = self.contacts_server() else {
                return String::new();
            };
            if let Some(a) = server
                .account()
                .and_then(|id| self.store.accounts().ok()?.into_iter().find(|a| a.id == id))
            {
                return a.email;
            }
            url::Url::parse(&server.url)
                .ok()
                .and_then(|u| u.host_str().map(str::to_owned))
                .unwrap_or(server.url)
        })
    }
    /// Outside any source: the one that has card `uid`, else where new
    /// contacts go. `None` within one already.
    fn route_card(&self, uid: &str) -> Option<String> {
        if within_one() {
            return None;
        }
        let holder = (!uid.is_empty())
            .then(|| {
                self.contacts_source_ids()
                    .into_iter()
                    .find(|id| self.in_contacts_source(id, || self.contacts().iter().any(|c| c.uid == uid)))
            })
            .flatten();
        Some(holder.unwrap_or_else(|| self.default_contacts_source()))
    }
    /// Outside any source: the first one (set up in Settings as always, and
    /// where contacts kept on this computer are).
    fn route_first_book(&self) -> Option<String> {
        (!within_one()).then(String::new)
    }
    /// Where new contacts go: the first source's server where it has one,
    /// else another source's, else this computer.
    pub fn default_contacts_source(&self) -> String {
        self.contacts_source_ids()
            .into_iter()
            .find(|id| self.in_contacts_source(id, || self.contacts_server().is_some()))
            .unwrap_or_default()
    }
    /// Adds another contacts source beside those set up, set up by `work`
    /// (one of the `connect_…` functions) within it.
    pub fn add_contacts_source<T>(&self, work: impl FnOnce() -> Result<T>) -> Result<T> {
        self.add_source(Pim::Contacts, work)
    }
    /// Disconnects one of the sources added besides the first.
    pub fn remove_contacts_source(&self, id: &str) -> Result<()> {
        ensure!(!id.is_empty(), "the first contacts source is disconnected with disconnect_contacts");
        self.in_contacts_source(id, || self.disconnect_contacts())?;
        self.forget_source(Pim::Contacts, id)
    }

    /// Every card, the server's and this computer's (every source's,
    /// outside any).
    pub fn contacts(&self) -> Vec<Card> {
        if !within_one() {
            return self.contacts_source_ids().into_iter().flat_map(|id| self.in_contacts_source(&id, || self.contacts())).collect();
        }
        self.card_pref(CARDS)
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_default()
    }
    fn save_contacts(&self, cards: &[Card]) -> Result<()> {
        // Outside any source, every source's cards would be read together
        // and written into the first.
        ensure!(
            within_one() || pim_sources::extra_sources(&self.store, Pim::Contacts).is_empty(),
            "contacts were saved outside any contacts source"
        );
        self.set_card_pref(CARDS, &serde_json::to_string(cards)?)
    }
    /// The CardDAV server address book sync uses, if one is set up
    /// (outside any source, the first one set up).
    pub fn contacts_server(&self) -> Option<carddav::Server> {
        if !within_one() {
            return self
                .contacts_source_ids()
                .into_iter()
                .find_map(|id| self.in_contacts_source(&id, || self.contacts_server()));
        }
        self.card_pref(SERVER)
            .and_then(|json| serde_json::from_str(&json).ok())
            .filter(|s: &carddav::Server| !s.url.is_empty())
    }
    /// When the address book was last read from the server.
    pub fn contacts_synced(&self) -> Option<i64> {
        self.card_pref(SYNCED)?.parse().ok()
    }
    fn contacts_password(&self) -> Result<String> {
        #[cfg(test)]
        if let Some(s) = &self.test_secret {
            return Ok(s.password.clone());
        }
        // Signed in as a mail account is, as for calendars.
        if let Some(server) = self.contacts_server()
            && !server.mail_account.is_empty()
        {
            return self.mail_account_secret(&server.mail_account);
        }
        store::read_secret_text(&key(PASSWORD))?
            .with_context(|| t!("manage-contacts-the-contacts-servers-password-is-missing"))
    }
    /// Signs in to a CardDAV server and reads its address books; the server
    /// and password are kept only once that works.
    pub fn connect_contacts(&self, mut server: carddav::Server, password: &str) -> Result<usize> {
        if let Some(id) = self.route_first_book() {
            return self.in_contacts_source(&id, || self.connect_contacts(server, password));
        }
        let fetched = carddav::fetch_changes(&server, password, self.tls_connector()?, &[])?;
        let cards = fetched.cards;
        server.book = fetched.book;
        server.states = fetched.states;
        server.vcard4 = fetched.vcard4;
        server.max_size = fetched.max_size;
        server.books = fetched.books;
        #[cfg(not(test))]
        if server.mail_account.is_empty() {
            store::write_secret_text(&key(PASSWORD), password)?;
        }
        self.set_card_pref(SERVER, &serde_json::to_string(&server)?)?;
        self.replace_server_cards(cards)
    }
    /// Uses the address books of a JMAP account's server (JMAP for
    /// Contacts) instead of a CardDAV server: its sign-in is the account's.
    pub fn connect_jmap_contacts(&self, a: &Account) -> Result<usize> {
        if let Some(id) = self.route_first_book() {
            return self.in_contacts_source(&id, || self.connect_jmap_contacts(a));
        }
        ensure!(a.provider == Provider::Jmap, t!("manage-contacts-only-jmap-accounts-keep-contacts-this"));
        self.with_jmap_contacts(a, crate::sync::jscal::ensure_contacts)?;
        let server = carddav::Server {
            url: a.incoming_host.clone(),
            user: a.email.clone(),
            jmap_account: a.id.clone(),
            ..Default::default()
        };
        self.set_card_pref(SERVER, &serde_json::to_string(&server)?)?;
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
    /// Uses the Exchange contacts of an Outlook account that reaches
    /// Microsoft 365 through Microsoft Graph, and its organization's
    /// directory for finding people (`search_server_contacts`).
    pub fn connect_graph_contacts(&self, a: &Account) -> Result<usize> {
        if let Some(id) = self.route_first_book() {
            return self.in_contacts_source(&id, || self.connect_graph_contacts(a));
        }
        ensure!(a.graph(), t!("manage-contacts-only-accounts-that-use-microsoft"));
        let server = carddav::Server {
            url: crate::sync::graph::BASE.to_owned(),
            user: a.email.clone(),
            graph_account: a.id.clone(),
            book: "graph:contacts".into(),
            ..Default::default()
        };
        self.set_card_pref(SERVER, &serde_json::to_string(&server)?)?;
        self.sync_contacts()
    }
    /// Uses the contacts of an Exchange Server account's mailbox (EWS),
    /// and its organization's directory for finding people.
    pub fn connect_ews_contacts(&self, a: &Account) -> Result<usize> {
        if let Some(id) = self.route_first_book() {
            return self.in_contacts_source(&id, || self.connect_ews_contacts(a));
        }
        ensure!(a.ews(), t!("manage-calendar-only-exchange-accounts"));
        let server = carddav::Server {
            url: a.ews_endpoint(),
            user: a.email.clone(),
            ews_account: a.id.clone(),
            book: "ews:contacts".into(),
            ..Default::default()
        };
        self.set_card_pref(SERVER, &serde_json::to_string(&server)?)?;
        self.sync_contacts()
    }
    /// The Exchange account contacts come from, if they come from one.
    fn contacts_ews(&self, server: &carddav::Server) -> Result<Option<Account>> {
        if server.ews_account.is_empty() {
            return Ok(None);
        }
        Ok(Some(
            self.store
                .accounts()?
                .into_iter()
                .find(|a| a.id == server.ews_account)
                .with_context(|| t!("manage-contacts-the-account-your-contacts-come-from"))?,
        ))
    }
    /// The Graph account contacts come from, if they come from one.
    fn contacts_graph(&self, server: &carddav::Server) -> Result<Option<Account>> {
        if server.graph_account.is_empty() {
            return Ok(None);
        }
        Ok(Some(
            self.store
                .accounts()?
                .into_iter()
                .find(|a| a.id == server.graph_account)
                .with_context(|| t!("manage-contacts-the-account-your-contacts-come-from"))?,
        ))
    }
    fn with_graph_contacts<T>(&self, a: &Account, work: impl FnOnce(&crate::sync::graph::Session) -> Result<T>) -> Result<T> {
        let lock = self.account_lock(&a.id);
        let _operation = lock.lock().unwrap();
        self.with_graph(a, work)
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
                .with_context(|| t!("manage-contacts-the-account-your-contacts-come-from"))?,
        ))
    }
    /// The mail account `id` names, for contacts that sign in as it.
    fn contacts_account(&self, id: &str) -> Result<Option<Account>> {
        if id.is_empty() {
            return Ok(None);
        }
        Ok(Some(
            self.store
                .accounts()?
                .into_iter()
                .find(|a| a.id == id)
                .with_context(|| t!("manage-contacts-the-account-your-contacts-come-from"))?,
        ))
    }
    /// The Google account contacts come from (People API), if any.
    fn contacts_google(&self, server: &carddav::Server) -> Result<Option<Account>> {
        self.contacts_account(&server.google_account)
    }
    /// The Proton account contacts come from (Proton Contacts), if any.
    fn contacts_proton(&self, server: &carddav::Server) -> Result<Option<Account>> {
        self.contacts_account(&server.proton_account)
    }
    /// Why `server` has one address book only (Exchange's, Google's,
    /// Proton's), if it does.
    fn single_book(&self, server: &carddav::Server) -> Option<()> {
        (!server.graph_account.is_empty()
            || !server.ews_account.is_empty()
            || !server.google_account.is_empty()
            || !server.proton_account.is_empty())
        .then_some(())
    }
    fn with_proton_contacts<T>(&self, a: &Account, work: impl FnOnce(&crate::sync::proton::Session) -> Result<T>) -> Result<T> {
        let lock = self.account_lock(&a.id);
        let _operation = lock.lock().unwrap();
        self.with_proton(a, work)
    }
    /// Uses the contacts of a Google account signed in with OAuth, through
    /// the People API: its sign-in is the account's.
    pub fn connect_google_contacts(&self, a: &Account) -> Result<usize> {
        if let Some(id) = self.route_first_book() {
            return self.in_contacts_source(&id, || self.connect_google_contacts(a));
        }
        ensure!(
            crate::manage::backend::dav_accounts::MailProvider::of(a) == Some(crate::manage::backend::dav_accounts::MailProvider::Google),
            t!("manage-calendar-only-google-accounts")
        );
        let server = carddav::Server {
            url: crate::sync::google::PEOPLE_BASE.to_owned(),
            user: a.email.clone(),
            google_account: a.id.clone(),
            book: "google:contacts".into(),
            ..Default::default()
        };
        self.set_card_pref(SERVER, &serde_json::to_string(&server)?)?;
        self.sync_contacts()
    }
    /// Uses the Proton Contacts of a Proton account signed in without
    /// Bridge: end-to-end encrypted with the account's own keys.
    pub fn connect_proton_contacts(&self, a: &Account) -> Result<usize> {
        if let Some(id) = self.route_first_book() {
            return self.in_contacts_source(&id, || self.connect_proton_contacts(a));
        }
        ensure!(a.provider == Provider::Proton, t!("manage-calendar-only-proton-accounts"));
        let server = carddav::Server {
            url: "https://mail.proton.me".to_owned(),
            user: a.email.clone(),
            proton_account: a.id.clone(),
            book: "proton:contacts".into(),
            ..Default::default()
        };
        self.set_card_pref(SERVER, &serde_json::to_string(&server)?)?;
        self.sync_contacts()
    }
    /// Makes a new address book on the server (MKCOL, or AddressBook/set),
    /// which new contacts then go to.
    pub fn create_address_book(&self, name: &str) -> Result<()> {
        ensure!(!name.trim().is_empty(), t!("manage-contacts-give-the-address-book-a-name"));
        if let Some(id) = self.route_first_book() {
            return self.in_contacts_source(&id, || self.create_address_book(name));
        }
        let mut server = self
            .contacts_server()
            .with_context(|| t!("manage-contacts-set-up-a-contacts-server-in"))?;
        ensure!(self.single_book(&server).is_none(), t!("manage-contacts-exchange-contacts-have-one-address-book"));
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
        self.set_card_pref(SERVER, &serde_json::to_string(&server)?)
    }
    /// The server's address books: (href, name, whether it can be written).
    pub fn address_books(&self) -> Result<Vec<(String, String, bool)>> {
        if let Some(id) = self.route_first_book() {
            return self.in_contacts_source(&id, || self.address_books());
        }
        let server = self
            .contacts_server()
            .with_context(|| t!("manage-contacts-no-contacts-server-is-set-up"))?;
        if !server.google_account.is_empty() || !server.proton_account.is_empty() {
            return Ok(vec![(format!("{}:contacts", if server.google_account.is_empty() { "proton" } else { "google" }), t!("manage-contacts-contacts").into(), true)]);
        }
        if self.contacts_graph(&server)?.is_some() || self.contacts_ews(&server)?.is_some() {
            return Ok(vec![("graph:contacts".into(), t!("manage-contacts-contacts").into(), true)]);
        }
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
        if !within_one() {
            // Each source that searches (JMAP's, Google's and Proton's are
            // read whole already).
            let mut added = 0;
            let mut searched = false;
            for id in self.contacts_source_ids() {
                let searches = self.in_contacts_source(&id, || {
                    self.contacts_server().is_some_and(|s| s.jmap_account.is_empty() && s.google_account.is_empty() && s.proton_account.is_empty())
                });
                if searches {
                    searched = true;
                    added += self.in_contacts_source(&id, || self.search_server_contacts(text))?;
                }
            }
            ensure!(searched, t!("manage-contacts-this-servers-contacts-are-all-read"));
            return Ok(added);
        }
        let server = self.contacts_server().with_context(|| t!("manage-contacts-no-contacts-server-is-set-up"))?;
        ensure!(
            self.contacts_jmap(&server)?.is_none() && server.google_account.is_empty() && server.proton_account.is_empty(),
            t!("manage-contacts-this-servers-contacts-are-all-read")
        );
        let found = match self.contacts_graph(&server)? {
            // The organization's directory, and people you work with.
            Some(a) => self.with_graph_contacts(&a, |s| s.search_directory(text.trim(), 50))?,
            None if let Some(a) = self.contacts_ews(&server)? => self.ews_directory(&a, text.trim())?,
            None => carddav::search(&server, &self.contacts_password()?, self.tls_connector()?, text, 50)?,
        };
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
        if let Some(id) = self.route_first_book() {
            return self.in_contacts_source(&id, || self.contacts_quota());
        }
        let server = self.contacts_server().with_context(|| t!("manage-contacts-no-contacts-server-is-set-up"))?;
        // Only CardDAV servers say (RFC 4331).
        let api = [&server.jmap_account, &server.graph_account, &server.ews_account, &server.google_account, &server.proton_account];
        if api.iter().any(|a| !a.is_empty()) {
            return Ok(None);
        }
        carddav::quota(&server, &self.contacts_password()?, self.tls_connector()?)
    }
    /// Renames an address book on the server.
    pub fn rename_address_book(&self, href: &str, name: &str) -> Result<()> {
        ensure!(!name.trim().is_empty(), t!("manage-contacts-give-the-address-book-a-name"));
        if let Some(id) = self.route_first_book() {
            return self.in_contacts_source(&id, || self.rename_address_book(href, name));
        }
        let server = self
            .contacts_server()
            .with_context(|| t!("manage-contacts-no-contacts-server-is-set-up"))?;
        ensure!(self.single_book(&server).is_none(), t!("manage-contacts-exchange-contacts-have-one-address-book"));
        match self.contacts_jmap(&server)? {
            Some(a) => {
                let id = href.strip_prefix("jmap:").with_context(|| t!("manage-contacts-not-a-jmap-address-book"))?;
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
        if let Some(id) = self.route_first_book() {
            return self.in_contacts_source(&id, || self.delete_address_book(href));
        }
        let mut server = self
            .contacts_server()
            .with_context(|| t!("manage-contacts-no-contacts-server-is-set-up"))?;
        ensure!(self.single_book(&server).is_none(), t!("manage-contacts-exchange-contacts-have-one-address-book"));
        match self.contacts_jmap(&server)? {
            Some(a) => {
                let id = href.strip_prefix("jmap:").with_context(|| t!("manage-contacts-not-a-jmap-address-book"))?;
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
        self.set_card_pref(SERVER, &serde_json::to_string(&server)?)?;
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
        if !within_one() {
            let results: Vec<(String, Result<usize>)> = self
                .contacts_source_ids()
                .into_iter()
                .filter_map(|id| {
                    self.in_contacts_source(&id, || self.contacts_server().is_some().then(|| self.sync_contacts())).map(|r| (id, r))
                })
                .collect();
            ensure!(!results.is_empty(), t!("manage-contacts-no-contacts-server-is-set-up"));
            return super::calendar::together(results, |id| self.contacts_source_label(id));
        }
        let mut server = self
            .contacts_server()
            .with_context(|| t!("manage-contacts-no-contacts-server-is-set-up"))?;
        if let Some(a) = self.contacts_google(&server)? {
            let cards = self.with_google(&a, |s| s.contacts())?;
            return self.replace_server_cards(cards);
        }
        if let Some(a) = self.contacts_proton(&server)? {
            let known: Vec<Card> = self.contacts().into_iter().filter(from_server).collect();
            let cards = self.with_proton_contacts(&a, |s| s.contacts(&known))?;
            return self.replace_server_cards(cards);
        }
        if let Some(a) = self.contacts_ews(&server)? {
            let cards = self.ews_contacts(&a)?;
            return self.replace_server_cards(cards);
        }
        if let Some(a) = self.contacts_graph(&server)? {
            let cards = self.with_graph_contacts(&a, |s| s.contacts())?;
            // Found in the directory: kept until the next search.
            return self.replace_server_cards(cards);
        }
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
                    && let Some((changed, gone, state)) = crate::sync::jscal::card_changes(s, since, &known)?
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
                crate::sync::jscal::cards_in(s, &known)
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
                self.set_card_pref(SERVER, &serde_json::to_string(&server)?)?;
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
            self.set_card_pref(SERVER, &serde_json::to_string(&server)?)?;
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
        self.set_card_pref(SYNCED, &now.to_string())?;
        Ok(count)
    }
    /// Stops reading contacts from the server, forgetting its cards.
    pub fn disconnect_contacts(&self) -> Result<()> {
        if let Some(id) = self.route_first_book() {
            return self.in_contacts_source(&id, || self.disconnect_contacts());
        }
        let kept: Vec<Card> = self
            .contacts()
            .into_iter()
            .filter(|c| !from_server(c))
            .collect();
        self.save_contacts(&kept)?;
        self.set_card_pref(SERVER, "")?;
        self.set_card_pref(SYNCED, "")?;
        #[cfg(not(test))]
        store::delete_secret(&key(PASSWORD))?;
        // Another source's keys aren't read again: gone.
        let id = pim_sources::current(Pim::Contacts).unwrap_or_default();
        if !id.is_empty() {
            self.clear_source_keys(&id, &[CARDS, SERVER, SYNCED])?;
        }
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
            t!("manage-contacts-give-the-contact-a-name-or")
        );
        for email in &card.emails {
            ensure!(
                crate::sync::smtp::recipients(email).is_ok_and(|r| r.len() == 1),
                t!("manage-contacts-isnt-a-valid-email-address", email = email)
            );
        }
        if let Some(id) = self.route_card(&card.uid) {
            return self.in_contacts_source(&id, || self.save_contact(card));
        }
        if card.uid.is_empty() {
            card.uid = uuid::Uuid::new_v4().to_string();
        }
        let card_uid = card.uid.clone();
        let saved = match self.contacts_server() {
            // Found in the directory: kept here, not written to it.
            Some(_) if card.href.starts_with("graph-directory:") || card.href.starts_with("ews-directory:") => Card { href: String::new(), ..card },
            Some(server) if from_server(&card) || !server.book.is_empty() => {
                if let Some(a) = self.contacts_google(&server)? {
                    self.with_google(&a, |s| s.save_contact(&card))?
                } else if let Some(a) = self.contacts_proton(&server)? {
                    self.with_proton_contacts(&a, |s| s.save_contact(&card))?
                } else if let Some(a) = self.contacts_ews(&server)? {
                    self.ews_save_contact(&a, &card)?
                } else if let Some(a) = self.contacts_graph(&server)? {
                    self.with_graph_contacts(&a, |s| s.save_contact(&card))?
                } else { match self.contacts_jmap(&server)? {
                    Some(a) => self.with_jmap_contacts(&a, |s| {
                        crate::sync::jscal::save_card(s, &card, &server.book)
                    })?,
                    None => carddav::save(
                        &server,
                        &self.contacts_password()?,
                        self.tls_connector()?,
                        &card,
                    )?,
                } }
            }
            _ => card,
        };
        let mut all = self.contacts();
        // Google and Proton name a new card themselves.
        all.retain(|c| c.uid != saved.uid && c.uid != card_uid);
        all.push(saved.clone());
        self.save_contacts(&all)?;
        Ok(saved)
    }
    /// Removes a contact, from the server too when it's there.
    pub fn delete_contact(&self, uid: &str) -> Result<()> {
        if let Some(id) = self.route_card(uid) {
            return self.in_contacts_source(&id, || self.delete_contact(uid));
        }
        let mut all = self.contacts();
        let Some(card) = all.iter().find(|c| c.uid == uid).cloned() else {
            return Ok(());
        };
        if from_server(&card)
            && !card.href.starts_with("graph-directory:")
            && !card.href.starts_with("ews-directory:")
            && let Some(server) = self.contacts_server()
        {
            if let Some(a) = self.contacts_google(&server)? {
                self.with_google(&a, |s| s.delete_contact(&card))?;
                all.retain(|c| c.uid != uid);
                return self.save_contacts(&all);
            }
            if let Some(a) = self.contacts_proton(&server)? {
                self.with_proton_contacts(&a, |s| s.delete_contact(&card))?;
                all.retain(|c| c.uid != uid);
                return self.save_contacts(&all);
            }
            if let Some(a) = self.contacts_ews(&server)? {
                self.ews_delete_contact(&a, &card)?;
                all.retain(|c| c.uid != uid);
                return self.save_contacts(&all);
            }
            if let Some(a) = self.contacts_graph(&server)? {
                self.with_graph_contacts(&a, |s| s.delete_contact(&card))?;
                all.retain(|c| c.uid != uid);
                return self.save_contacts(&all);
            }
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
        // Kept on this computer, with the first source's cards.
        if let Some(id) = self.route_first_book() {
            return self.in_contacts_source(&id, || self.import_contacts(paths));
        }
        let mut added = Vec::new();
        for path in paths {
            // Address books are megabytes at most (photos included): a file
            // past this isn't one, and isn't read into memory whole.
            ensure!(
                std::fs::metadata(path)?.len() <= 256 * 1024 * 1024,
                t!("manage-contacts-is-too-large-to-be-an", path = path.display())
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
            t!("manage-contacts-no-contacts-were-found-in-that")
        );
        let count = added.len();
        let mut all = self.contacts();
        // As if each card in turn replaced any of its uid (the last of a
        // uid imported twice standing): by an index, not by going through
        // every contact again for each card imported.
        let last: HashMap<String, usize> = added.iter().enumerate().map(|(i, c)| (c.uid.clone(), i)).collect();
        all.retain(|c| !last.contains_key(&c.uid));
        all.extend(
            added
                .into_iter()
                .enumerate()
                .filter(|(i, c)| last.get(&c.uid) == Some(i))
                .map(|(_, c)| c),
        );
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
        if let Some(id) = self.route_first_book() {
            return self.in_contacts_source(&id, || self.clear_imported_contacts());
        }
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

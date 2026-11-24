//! Organizing mail on this computer, whatever the server offers: the list
//! grouped by conversation, pinned messages, muted conversations, saved
//! searches (smart folders), blocked senders and rules applied here as new
//! mail arrives (for providers without server filters: Gmail, Outlook,
//! iCloud, Proton, POP3), and how the list and reader are laid out.
//!
//! Saved as one Store preference, as `composing` is.
use crate::manage::{
    backend::{Backend, Change},
    model::{Account, Folder, Mail},
    store::Store,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const PREFERENCE: &str = "organizing";

/// How tall the message list's rows are.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Density {
    /// Sender, subject and a line of preview.
    #[default]
    Comfortable,
    /// Sender and subject only, closer together.
    Compact,
}

/// Where the reader goes beside the list.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReaderLayout {
    /// To the right of the list.
    #[default]
    Beside,
    /// Below the list (a horizontal split).
    Below,
}

/// A search kept in the sidebar: a smart folder of the mail it finds.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedSearch {
    pub name: String,
    pub query: String,
}

/// Which part of a message a rule looks at.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleField {
    #[default]
    From,
    To,
    Subject,
    /// Sender, recipients, subject and preview.
    Anywhere,
}
impl RuleField {
    pub const ALL: [Self; 4] = [Self::From, Self::To, Self::Subject, Self::Anywhere];
    pub fn label(self) -> &'static str {
        match self {
            Self::From => "From",
            Self::To => "To or Cc",
            Self::Subject => "Subject",
            Self::Anywhere => "Anywhere",
        }
    }
}

/// What a rule does with the mail it matches.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleAction {
    #[default]
    MarkRead,
    Star,
    Archive,
    Spam,
    Trash,
    /// Into one of the account's server folders, by name.
    MoveTo(String),
    /// Adds a label (Gmail) or keyword.
    Label(String),
}
impl RuleAction {
    pub fn label(&self) -> String {
        match self {
            Self::MarkRead => "Mark as read".into(),
            Self::Star => "Star".into(),
            Self::Archive => "Archive".into(),
            Self::Spam => "Move to Spam".into(),
            Self::Trash => "Move to Trash".into(),
            Self::MoveTo(f) => format!("Move to “{f}”"),
            Self::Label(l) => format!("Label “{l}”"),
        }
    }
    /// The change it makes on the server.
    pub fn change(&self) -> Change {
        match self {
            Self::MarkRead => Change::Read(true),
            Self::Star => Change::Star(true),
            Self::Archive => Change::Move(Folder::Archive),
            Self::Spam => Change::Move(Folder::Junk),
            Self::Trash => Change::Move(Folder::Trash),
            Self::MoveTo(f) => Change::MoveTo(f.clone()),
            Self::Label(l) => Change::Label(l.clone(), true),
        }
    }
    fn moves(&self) -> bool {
        matches!(self, Self::Archive | Self::Spam | Self::Trash | Self::MoveTo(_))
    }
}

/// A rule applied on this computer to new inbox mail.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Rule {
    pub field: RuleField,
    /// Lowercase text the field must contain.
    pub contains: String,
    pub action: RuleAction,
    /// Only for this account (its id); every account when empty.
    pub account: String,
    pub enabled: bool,
}
impl Rule {
    pub fn matches(&self, m: &Mail) -> bool {
        if !self.enabled || self.contains.trim().is_empty() {
            return false;
        }
        if !self.account.is_empty() && self.account != m.account {
            return false;
        }
        let needle = self.contains.trim().to_lowercase();
        let has = |s: &str| s.to_lowercase().contains(&needle);
        match self.field {
            RuleField::From => has(&m.from) || has(&m.sender),
            RuleField::To => has(&m.to) || has(m.cc()),
            RuleField::Subject => has(&m.subject),
            RuleField::Anywhere => {
                has(&m.from)
                    || has(&m.sender)
                    || has(&m.to)
                    || has(m.cc())
                    || has(&m.subject)
                    || has(&m.snippet)
            }
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Organizing {
    /// The list shows a conversation as one row, its newest message.
    pub threaded: bool,
    /// Pinned messages, by `mail_key`: listed first.
    pub pinned: BTreeSet<String>,
    /// Muted conversations, by `conversation_key`: new mail in them is
    /// marked read and not announced.
    pub muted: BTreeSet<String>,
    pub saved_searches: Vec<SavedSearch>,
    /// Blocked senders: addresses, or `@domain` for a whole domain. New
    /// mail from them goes to Trash.
    pub blocked: Vec<String>,
    pub rules: Vec<Rule>,
    pub density: Density,
    pub layout: ReaderLayout,
}

impl Organizing {
    pub fn load(store: &Store) -> Self {
        store
            .preference(PREFERENCE)
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }
    pub fn save(&self, store: &Store) {
        if let Ok(json) = serde_json::to_string(self) {
            store.set_preference_soon(PREFERENCE, &json);
        }
    }
    pub fn is_pinned(&self, m: &Mail) -> bool {
        !self.pinned.is_empty() && self.pinned.contains(&mail_key(m))
    }
    pub fn is_muted(&self, m: &Mail) -> bool {
        !self.muted.is_empty()
            && (self.muted.contains(&conversation_key(m))
                || referenced_keys(m).any(|k| self.muted.contains(&k)))
    }
    /// Whether mail from `address` is blocked.
    pub fn is_blocked(&self, address: &str) -> bool {
        let address = address.trim().to_lowercase();
        if address.is_empty() {
            return false;
        }
        self.blocked.iter().any(|b| {
            let b = b.trim().to_lowercase();
            if let Some(domain) = b.strip_prefix('@') {
                address
                    .rsplit_once('@')
                    .is_some_and(|(_, d)| d == domain || d.ends_with(&format!(".{domain}")))
            } else {
                !b.is_empty() && b == address
            }
        })
    }
    /// Blocks `address`, or unblocks it when already blocked. Returns
    /// whether it is blocked now.
    pub fn toggle_block(&mut self, address: &str) -> bool {
        let address = address.trim().to_lowercase();
        let before = self.blocked.len();
        self.blocked.retain(|b| b.trim().to_lowercase() != address);
        if self.blocked.len() == before && !address.is_empty() {
            self.blocked.push(address);
            true
        } else {
            false
        }
    }
    /// What happens on this computer to newly arrived `m`: the changes to
    /// make, in order. Blocked senders go to Trash; muted conversations are
    /// marked read; then the first rule that moves the message, after
    /// every rule that only marks it.
    #[cfg(test)]
    pub fn arrival_changes(&self, m: &Mail) -> Vec<Change> {
        self.changes(m, true)
    }
    /// What blocked senders and rules do to `m`, mail already here (Apply
    /// Rules Now): as for new mail, but muted conversations are left, as
    /// muting one marked what it held read then.
    #[cfg(test)]
    pub fn existing_changes(&self, m: &Mail) -> Vec<Change> {
        self.changes(m, false)
    }
    fn changes(&self, m: &Mail, arriving: bool) -> Vec<Change> {
        if self.is_blocked(&m.from) {
            return vec![Change::Read(true), Change::Move(Folder::Trash)];
        }
        let mut changes = Vec::new();
        if arriving && self.is_muted(m) {
            changes.push(Change::Read(true));
        }
        let mut moved = false;
        for rule in self.rules.iter().filter(|r| r.matches(m)) {
            if rule.action.moves() {
                if moved {
                    continue;
                }
                moved = true;
            }
            let change = rule.action.change();
            if !changes.contains(&change) {
                changes.push(change);
            }
        }
        // Moves last: the marks apply to the message where it was.
        changes.sort_by_key(|c| matches!(c, Change::Move(_) | Change::MoveTo(_)));
        changes
    }
    /// Whether there are blocked senders, muted conversations or rules to
    /// apply at all.
    pub fn applies_to_arrivals(&self) -> bool {
        !self.blocked.is_empty() || !self.muted.is_empty() || self.rules.iter().any(|r| r.enabled)
    }
    /// `mail`'s changes (`arrival_changes`, or `existing_changes` when not
    /// `arriving`), gathered into one request per change, marks before
    /// moves (after which messages have new identities). Changes already
    /// made (read mail marked read, mail starred already) are left out.
    pub fn grouped_changes(&self, mail: &[Mail], arriving: bool) -> Vec<(Change, Vec<Mail>)> {
        let mut groups: Vec<(Change, Vec<Mail>)> = Vec::new();
        for m in mail {
            for change in self.changes(m, arriving) {
                let made = match &change {
                    Change::Read(true) => !m.unread,
                    Change::Star(true) => m.starred,
                    Change::Label(l, true) => m.labels().iter().any(|x| x == l),
                    other => other.moved_already(m),
                };
                if made {
                    continue;
                }
                match groups.iter_mut().find(|(c, _)| *c == change) {
                    Some((_, ms)) => ms.push(m.clone()),
                    None => groups.push((change, vec![m.clone()])),
                }
            }
        }
        groups.sort_by_key(|(c, _)| c.moves());
        groups
    }
    /// Whether new mail like `m` should not be announced.
    pub fn silenced(&self, m: &Mail) -> bool {
        self.is_blocked(&m.from)
            || self.is_muted(m)
            || self
                .rules
                .iter()
                .any(|r| r.matches(m) && (r.action.moves() || r.action == RuleAction::MarkRead))
    }
}

/// Makes `groups` (`Organizing::grouped_changes`) on account `a`'s server
/// and in the cache. Every group is tried; the first failure is returned.
pub fn apply_changes(backend: &Backend, a: &Account, groups: Vec<(Change, Vec<Mail>)>) -> anyhow::Result<()> {
    let mut failed = None;
    for (change, mails) in groups {
        let mails: Vec<Mail> = mails.into_iter().filter(|m| m.account == a.id).collect();
        if mails.is_empty() {
            continue;
        }
        if let Err(e) = backend.change_many(a, &mails, change)
            && failed.is_none()
        {
            failed = Some(e);
        }
    }
    failed.map_or(Ok(()), Err)
}

/// Applies blocked senders, muted conversations and rules to mail that
/// just arrived in `a`'s inbox: by the window as it checks, and by the
/// background monitor while there is no window.
pub fn apply_to_arrivals(backend: &Backend, a: &Account, fresh: &[Mail]) -> anyhow::Result<()> {
    let org = Organizing::load(&backend.store);
    if fresh.is_empty() || !org.applies_to_arrivals() {
        return Ok(());
    }
    apply_changes(backend, a, org.grouped_changes(fresh, true))
}

/// Applies blocked senders and rules to the mail in every account's inbox
/// now (as cached), not only to mail as it arrives. Returns how many
/// messages were changed; an account whose server refused is reported
/// after the others are done.
pub fn apply_to_inboxes(backend: &Backend, accounts: &[Account]) -> anyhow::Result<usize> {
    let org = Organizing::load(&backend.store);
    let mut changed = 0;
    let mut failed = None;
    for a in accounts {
        let inbox = backend.store.summaries(&a.id, Folder::Inbox)?;
        let groups = org.grouped_changes(&inbox, false);
        let ids: std::collections::HashSet<&str> = groups
            .iter()
            .flat_map(|(_, ms)| ms.iter().map(|m| m.id.as_str()))
            .collect();
        let count = ids.len();
        drop(ids);
        match apply_changes(backend, a, groups) {
            Ok(()) => changed += count,
            Err(e) => {
                failed.get_or_insert_with(|| e.context(a.email.clone()));
            }
        }
    }
    match failed {
        Some(e) => Err(e),
        None => Ok(changed),
    }
}

/// A message's identity across folders and labels: its Message-ID, or the
/// cache's id for mail without one.
pub fn mail_key(m: &Mail) -> String {
    if m.message_id.is_empty() {
        m.id.clone()
    } else {
        m.message_id.clone()
    }
}

/// The Message-IDs a message refers to (References and In-Reply-To as
/// cached), each as a conversation key.
fn referenced_keys(m: &Mail) -> impl Iterator<Item = String> + '_ {
    m.references()
        .split_whitespace()
        .map(|r| format!("m:{}", r.trim_matches(|c| c == '<' || c == '>')))
}

/// A conversation's identity for muting it, worked out from one message
/// alone (new mail is checked against it as it arrives): the server's
/// thread id where it gave one, otherwise the first message the
/// conversation refers to (its start), otherwise the message itself.
pub fn conversation_key(m: &Mail) -> String {
    if !m.thread_id().is_empty() {
        return format!("t:{}:{}", m.account, m.thread_id());
    }
    if let Some(root) = m.references().split_whitespace().next() {
        return format!("m:{}", root.trim_matches(|c| c == '<' || c == '>'));
    }
    if !m.message_id.is_empty() {
        return format!("m:{}", m.message_id.trim_matches(|c| c == '<' || c == '>'));
    }
    format!("id:{}", m.id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mail(id: &str, from: &str, subject: &str, refs: &str) -> Mail {
        let mut m = Mail {
            id: id.into(),
            account: "a".into(),
            from: from.into(),
            subject: subject.into(),
            message_id: format!("<{id}@x>"),
            ..Mail::default()
        };
        m.set_references(refs);
        m
    }

    #[test]
    fn blocked_addresses_and_domains() {
        let mut o = Organizing::default();
        assert!(o.toggle_block("Spam@Example.com"));
        o.blocked.push("@junk.test".into());
        assert!(o.is_blocked("spam@example.com"));
        assert!(o.is_blocked("x@mail.junk.test"));
        assert!(!o.is_blocked("x@notjunk.test"));
        assert!(!o.toggle_block("spam@example.com"));
        assert!(!o.is_blocked("spam@example.com"));
    }

    #[test]
    fn rules_mark_before_they_move() {
        let o = Organizing {
            rules: vec![
                Rule {
                    field: RuleField::Subject,
                    contains: "invoice".into(),
                    action: RuleAction::MoveTo("Bills".into()),
                    enabled: true,
                    ..Rule::default()
                },
                Rule {
                    field: RuleField::From,
                    contains: "shop".into(),
                    action: RuleAction::Star,
                    enabled: true,
                    ..Rule::default()
                },
            ],
            ..Organizing::default()
        };
        let m = mail("1", "billing@shop.test", "Your invoice", "");
        assert_eq!(
            o.arrival_changes(&m),
            vec![Change::Star(true), Change::MoveTo("Bills".into())]
        );
    }

    #[test]
    fn existing_mail_takes_rules_but_not_mutes() {
        let mut o = Organizing {
            rules: vec![Rule {
                field: RuleField::From,
                contains: "shop".into(),
                action: RuleAction::Archive,
                enabled: true,
                ..Rule::default()
            }],
            ..Organizing::default()
        };
        let mut m = mail("1", "news@shop.test", "Sale", "");
        m.unread = true;
        o.muted.insert(conversation_key(&m));
        assert_eq!(
            o.arrival_changes(&m),
            vec![Change::Read(true), Change::Move(Folder::Archive)]
        );
        assert_eq!(o.existing_changes(&m), vec![Change::Move(Folder::Archive)]);
        // Grouped, one request per change; what's done already is left.
        let mut read = mail("2", "news@shop.test", "Sale", "");
        read.unread = false;
        let mut archived = mail("3", "news@shop.test", "Sale", "");
        archived.folder = Folder::Archive;
        let groups = o.grouped_changes(&[m.clone(), read, archived], true);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].0, Change::Read(true));
        assert_eq!(groups[0].1.len(), 1, "read mail isn't marked again");
        assert_eq!(groups[1].0, Change::Move(Folder::Archive));
        assert_eq!(groups[1].1.len(), 2, "archived mail isn't moved again");
        o.rules[0].enabled = false;
        o.muted.clear();
        assert!(!o.applies_to_arrivals());
    }
}

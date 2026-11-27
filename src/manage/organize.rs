//! Organizing mail on this computer, whatever the server offers: the list
//! grouped by conversation, pinned messages, muted conversations, saved
//! searches (smart folders), blocked senders and rules applied here as new
//! mail arrives (for providers without server filters: Gmail, Outlook,
//! iCloud, Proton, POP3), and how the list and reader are laid out.
//!
//! Saved as one Store preference, as `composing` is.
use crate::manage::{
    backend::{Backend, Change},
    categories::Category,
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

/// Which part of a message a rule's condition looks at.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleField {
    #[default]
    From,
    /// To or Cc.
    To,
    Subject,
    /// Sender, recipients, subject and preview.
    Anywhere,
    /// The message's text: its preview, and all of it once downloaded.
    Body,
    /// A header named in the condition's `header`. Those the list keeps
    /// (List-Id, Reply-To…) are always there; others once the message
    /// is downloaded.
    Header,
    /// The mailing list it came through (List-Id).
    List,
    /// Its size in kilobytes.
    Size,
    /// Whether it has attachments (no value).
    Attachment,
    /// The inbox category it is sorted into (a `Category::key`).
    Category,
}
impl RuleField {
    pub const ALL: [Self; 10] = [
        Self::From,
        Self::To,
        Self::Subject,
        Self::Anywhere,
        Self::Body,
        Self::Header,
        Self::List,
        Self::Size,
        Self::Attachment,
        Self::Category,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::From => t!("manage-organize-from"),
            Self::To => t!("manage-organize-to-or-cc"),
            Self::Subject => t!("manage-organize-subject"),
            Self::Anywhere => t!("manage-organize-anywhere"),
            Self::Body => t!("manage-organize-text"),
            Self::Header => t!("manage-organize-header"),
            Self::List => t!("manage-organize-mailing-list"),
            Self::Size => t!("manage-organize-size-kb"),
            Self::Attachment => t!("manage-organize-attachments"),
            Self::Category => t!("manage-organize-category"),
        }
    }
    /// The comparisons it offers, the first its default.
    pub fn ops(self) -> &'static [RuleOp] {
        match self {
            Self::Size => &[RuleOp::Over, RuleOp::Under],
            Self::Attachment => &[],
            Self::Category => &[RuleOp::Is],
            _ => &[RuleOp::Contains, RuleOp::Is, RuleOp::StartsWith, RuleOp::EndsWith, RuleOp::Regex],
        }
    }
}

/// How a condition compares its value.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleOp {
    #[default]
    Contains,
    /// The whole of it (an address, for From and To).
    Is,
    StartsWith,
    EndsWith,
    /// A regular expression, matched without regard to case.
    Regex,
    /// Sizes: larger than.
    Over,
    /// Sizes: smaller than.
    Under,
}
impl RuleOp {
    pub fn label(self) -> &'static str {
        match self {
            Self::Contains => t!("manage-organize-op-contains"),
            Self::Is => t!("manage-organize-op-is"),
            Self::StartsWith => t!("manage-organize-op-starts-with"),
            Self::EndsWith => t!("manage-organize-op-ends-with"),
            Self::Regex => t!("manage-organize-op-matches-pattern"),
            Self::Over => t!("manage-organize-op-over"),
            Self::Under => t!("manage-organize-op-under"),
        }
    }
}

/// One test of a rule.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Condition {
    pub field: RuleField,
    pub op: RuleOp,
    pub value: String,
    /// For `RuleField::Header`: the header's name.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub header: String,
    /// The condition holds when the test doesn't.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub negate: bool,
}

/// Regular expressions compiled once each (case-insensitive); `None` for
/// one that doesn't compile.
fn regex(pattern: &str) -> Option<std::sync::Arc<regex::Regex>> {
    use std::collections::HashMap;
    use std::sync::{Arc, LazyLock, Mutex};
    static CACHE: LazyLock<Mutex<HashMap<String, Option<Arc<regex::Regex>>>>> = LazyLock::new(Mutex::default);
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if cache.len() > 256 {
        cache.clear();
    }
    cache
        .entry(pattern.to_owned())
        .or_insert_with(|| {
            regex::RegexBuilder::new(pattern)
                .case_insensitive(true)
                .size_limit(1 << 20)
                .build()
                .ok()
                .map(Arc::new)
        })
        .clone()
}

/// What rules may ask of a message beyond its row: the inbox category it
/// is sorted into, and headers the row doesn't keep (from the downloaded
/// message). Without them, conditions on them don't hold.
#[derive(Clone, Copy, Default)]
pub struct Facts<'a> {
    pub category: Option<&'a dyn Fn(&Mail) -> Category>,
    pub header: Option<&'a HeaderLookup<'a>>,
}
/// A message's header by name (lowercase), where it can be read.
pub type HeaderLookup<'a> = dyn Fn(&Mail, &str) -> Option<String> + 'a;

impl Condition {
    /// A condition on `field` that `value` is contained in it.
    pub fn contains(field: RuleField, value: &str) -> Self {
        Self { field, value: value.into(), ..Self::default() }
    }
    fn compare(&self, text: &str) -> bool {
        let needle = self.value.trim();
        if self.op == RuleOp::Regex {
            return regex(needle).is_some_and(|r| r.is_match(text));
        }
        let (text, needle) = (text.trim().to_lowercase(), needle.to_lowercase());
        match self.op {
            RuleOp::Contains => text.contains(&needle),
            RuleOp::Is => text == needle,
            RuleOp::StartsWith => text.starts_with(&needle),
            RuleOp::EndsWith => text.ends_with(&needle),
            _ => false,
        }
    }
    /// Compares each address of a list of people, or else the whole text
    /// (`Is` holds for an address; `Contains` for the names too).
    fn people(&self, list: &str) -> bool {
        if self.compare(list) {
            return true;
        }
        matches!(self.op, RuleOp::Is | RuleOp::StartsWith | RuleOp::EndsWith)
            && super::categories::addresses_in(list).any(|a| self.compare(&a))
    }
    /// A header's value: those the row keeps, or else from `facts`.
    fn header_value(m: &Mail, name: &str, facts: &Facts) -> Option<String> {
        let name = name.trim().to_ascii_lowercase();
        let kept = match name.as_str() {
            "from" => Some(m.from.clone()),
            "sender" => Some(m.sender.clone()),
            "to" => Some(m.to.clone()),
            "cc" => Some(m.cc().to_owned()),
            "subject" => Some(m.subject.clone()),
            "reply-to" => Some(m.reply_to.clone()),
            "list-id" => Some(m.list_id().to_owned()),
            "list-unsubscribe" => Some(m.list_unsubscribe.clone()),
            "message-id" => Some(m.message_id.clone()),
            "references" => Some(m.references().to_owned()),
            _ => None,
        };
        kept.or_else(|| facts.header.and_then(|h| h(m, &name)))
    }
    pub fn holds(&self, m: &Mail, facts: &Facts) -> bool {
        let test = match self.field {
            RuleField::From => self.people(&m.from) || self.people(&m.sender),
            RuleField::To => self.people(&m.to) || self.people(m.cc()),
            RuleField::Subject => self.compare(&m.subject),
            RuleField::Anywhere => [m.from.as_str(), &m.sender, &m.to, m.cc(), &m.subject, &m.snippet]
                .iter()
                .any(|t| self.compare(t)),
            RuleField::Body => {
                self.compare(&m.snippet) || m.body.as_deref().is_some_and(|b| self.compare(b))
            }
            RuleField::List => self.compare(m.list_id()),
            RuleField::Header => {
                // A header that isn't there holds no value: only a negated
                // test passes.
                match Self::header_value(m, &self.header, facts) {
                    Some(v) => self.compare(&v),
                    None => false,
                }
            }
            RuleField::Size => {
                let kb: u64 = self.value.trim().trim_end_matches(|c: char| c.is_alphabetic() || c.is_whitespace()).parse().unwrap_or(0);
                let size = u64::from(m.size) / 1024;
                match self.op {
                    RuleOp::Under => size < kb,
                    _ => kb > 0 && size > kb,
                }
            }
            RuleField::Attachment => m.has_attachments(),
            RuleField::Category => match (facts.category, Category::parse(&self.value)) {
                (Some(category), Some(wanted)) => category(m) == wanted,
                _ => return false,
            },
        };
        test != self.negate
    }
    /// What's wrong with it, if anything: a missing value, a pattern that
    /// doesn't compile, a size that isn't one.
    pub fn problem(&self) -> Option<&'static str> {
        let value = self.value.trim();
        if self.field == RuleField::Attachment {
            return None;
        }
        if value.is_empty() {
            return Some(t!("manage-organize-each-condition-needs-a-value"));
        }
        match self.field {
            RuleField::Header if self.header.trim().is_empty() || self.header.contains([':', ' ']) => {
                Some(t!("manage-organize-name-the-header"))
            }
            RuleField::Size
                if value
                    .trim_end_matches(|c: char| c.is_alphabetic() || c.is_whitespace())
                    .parse::<u64>()
                    .is_err() =>
            {
                Some(t!("manage-organize-enter-a-size-in-kilobytes"))
            }
            RuleField::Category if Category::parse(value).is_none() => Some(t!("manage-organize-choose-a-category")),
            _ if self.op == RuleOp::Regex && regex(value).is_none() => Some(t!("manage-organize-the-pattern-isnt-valid")),
            _ => None,
        }
    }
    /// In words: "Subject contains “invoice”".
    pub fn describe(&self) -> String {
        if self.field == RuleField::Attachment {
            return if self.negate {
                t!("manage-organize-has-no-attachments").into()
            } else {
                t!("manage-organize-has-attachments").into()
            };
        }
        let field = match self.field {
            RuleField::Header => self.header.trim().to_owned(),
            f => f.label().to_owned(),
        };
        let value = match (self.field, Category::parse(&self.value)) {
            (RuleField::Category, Some(c)) => c.label().to_owned(),
            _ => self.value.trim().to_owned(),
        };
        if self.negate {
            t!("manage-organize-condition-not", field = field, op = self.op.label(), value = value)
        } else {
            t!("manage-organize-condition", field = field, op = self.op.label(), value = value)
        }
    }
    /// Whether it needs the message's inbox category.
    fn wants_category(&self) -> bool {
        self.field == RuleField::Category
    }
    /// Whether it may need headers the row doesn't keep.
    fn wants_headers(&self) -> bool {
        self.field == RuleField::Header && Self::header_value(&Mail::default(), &self.header, &Facts::default()).is_none()
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
    /// Isn't announced (no notification), and changes nothing else.
    Silence,
}
impl RuleAction {
    pub fn label(&self) -> String {
        match self {
            Self::MarkRead => t!("manage-organize-mark-as-read").into(),
            Self::Star => t!("manage-organize-star").into(),
            Self::Archive => t!("manage-organize-archive").into(),
            Self::Spam => t!("manage-organize-move-to-spam").into(),
            Self::Trash => t!("manage-organize-move-to-trash").into(),
            Self::MoveTo(f) => t!("manage-organize-move-to", folder = f),
            Self::Label(l) => t!("manage-organize-label", label = l),
            Self::Silence => t!("manage-organize-dont-notify").into(),
        }
    }
    /// The change it makes on the server, if any.
    pub fn change(&self) -> Option<Change> {
        Some(match self {
            Self::MarkRead => Change::Read(true),
            Self::Star => Change::Star(true),
            Self::Archive => Change::Move(Folder::Archive),
            Self::Spam => Change::Move(Folder::Junk),
            Self::Trash => Change::Move(Folder::Trash),
            Self::MoveTo(f) => Change::MoveTo(f.clone()),
            Self::Label(l) => Change::Label(l.clone(), true),
            Self::Silence => return None,
        })
    }
    pub fn moves(&self) -> bool {
        matches!(self, Self::Archive | Self::Spam | Self::Trash | Self::MoveTo(_))
    }
    /// What's wrong with it: a folder or label without a name.
    pub fn problem(&self) -> Option<&'static str> {
        match self {
            Self::MoveTo(n) | Self::Label(n) if n.trim().is_empty() => {
                Some(t!("manage-organize-name-the-folder-or-label"))
            }
            _ => None,
        }
    }
}

/// A rule applied on this computer to new inbox mail: when all (or, with
/// `any`, one) of its conditions hold, each of its actions, in order.
///
/// Older rules, of one `field`, `contains` and `action`, read as a rule
/// of one condition and one action.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "RuleRepr")]
pub struct Rule {
    /// What the person calls it; described by its conditions when empty.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub name: String,
    pub conditions: Vec<Condition>,
    /// One condition holding is enough (OR), rather than all (AND).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub any: bool,
    pub actions: Vec<RuleAction>,
    /// Only for this account (its id); every account when empty.
    pub account: String,
    pub enabled: bool,
    /// No later rule applies to mail this one matched.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub stop: bool,
}

/// A rule as saved, by this version or an older one.
#[derive(Deserialize, Default)]
#[serde(default)]
struct RuleRepr {
    name: String,
    conditions: Vec<Condition>,
    any: bool,
    actions: Vec<RuleAction>,
    account: String,
    enabled: bool,
    stop: bool,
    // Before several conditions and actions.
    field: Option<RuleField>,
    contains: Option<String>,
    action: Option<RuleAction>,
}
impl From<RuleRepr> for Rule {
    fn from(r: RuleRepr) -> Self {
        let mut conditions = r.conditions;
        if conditions.is_empty()
            && let Some(contains) = r.contains.filter(|c| !c.trim().is_empty())
        {
            conditions.push(Condition::contains(r.field.unwrap_or_default(), contains.trim()));
        }
        let mut actions = r.actions;
        if actions.is_empty()
            && let Some(action) = r.action
        {
            actions.push(action);
        }
        Self { name: r.name, conditions, any: r.any, actions, account: r.account, enabled: r.enabled, stop: r.stop }
    }
}

impl Rule {
    /// A rule of one condition and one action, as rules once were.
    #[cfg(test)]
    pub fn simple(field: RuleField, contains: &str, action: RuleAction) -> Self {
        Self {
            conditions: vec![Condition::contains(field, contains)],
            actions: vec![action],
            enabled: true,
            ..Self::default()
        }
    }
    #[cfg(test)]
    pub fn matches(&self, m: &Mail) -> bool {
        self.matches_with(m, &Facts::default())
    }
    pub fn matches_with(&self, m: &Mail, facts: &Facts) -> bool {
        if !self.enabled || self.conditions.is_empty() || self.actions.is_empty() {
            return false;
        }
        if !self.account.is_empty() && self.account != m.account {
            return false;
        }
        // A condition that can't be tested (no value) never holds.
        let holds = |c: &Condition| c.problem().is_none() && c.holds(m, facts);
        if self.any {
            self.conditions.iter().any(holds)
        } else {
            self.conditions.iter().all(holds)
        }
    }
    /// What keeps it from being saved, if anything.
    pub fn problem(&self) -> Option<&'static str> {
        if self.conditions.is_empty() {
            return Some(t!("manage-organize-add-a-condition"));
        }
        if self.actions.is_empty() {
            return Some(t!("manage-organize-add-an-action"));
        }
        if self.actions.iter().filter(|a| a.moves()).count() > 1 {
            return Some(t!("manage-organize-a-rule-moves-mail-once"));
        }
        self.conditions
            .iter()
            .find_map(Condition::problem)
            .or_else(|| self.actions.iter().find_map(RuleAction::problem))
    }
    /// Its conditions in words, joined by "and" or "or".
    pub fn describe_conditions(&self) -> String {
        let join = if self.any { t!("manage-organize-or") } else { t!("manage-organize-and") };
        self.conditions
            .iter()
            .map(Condition::describe)
            .collect::<Vec<_>>()
            .join(&format!(" {join} "))
    }
    /// Its actions in words, in order.
    pub fn describe_actions(&self) -> String {
        self.actions.iter().map(RuleAction::label).collect::<Vec<_>>().join(", ")
    }
    /// The whole rule in words.
    pub fn describe(&self) -> String {
        t!("manage-organize-rule", conditions = self.describe_conditions(), actions = self.describe_actions())
    }
    pub fn wants_category(&self) -> bool {
        self.enabled && self.conditions.iter().any(Condition::wants_category)
    }
    pub fn wants_headers(&self) -> bool {
        self.enabled && self.conditions.iter().any(Condition::wants_headers)
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
    /// marked read; then each matching rule's actions, in order, until a
    /// rule that stops the rest: the marks, and the first move.
    #[cfg(test)]
    pub fn arrival_changes(&self, m: &Mail) -> Vec<Change> {
        self.changes(m, true, &Facts::default())
    }
    /// What blocked senders and rules do to `m`, mail already here (Apply
    /// Rules Now): as for new mail, but muted conversations are left, as
    /// muting one marked what it held read then.
    #[cfg(test)]
    pub fn existing_changes(&self, m: &Mail) -> Vec<Change> {
        self.changes(m, false, &Facts::default())
    }
    /// The rules' actions for `m`, in order: each matching rule's, until
    /// one that stops the rest.
    fn actions<'a>(&'a self, m: &Mail, facts: &Facts) -> Vec<&'a RuleAction> {
        let mut actions = Vec::new();
        for rule in &self.rules {
            if !rule.matches_with(m, facts) {
                continue;
            }
            actions.extend(rule.actions.iter());
            if rule.stop {
                break;
            }
        }
        actions
    }
    fn changes(&self, m: &Mail, arriving: bool, facts: &Facts) -> Vec<Change> {
        if self.is_blocked(&m.from) {
            return vec![Change::Read(true), Change::Move(Folder::Trash)];
        }
        let mut changes = Vec::new();
        if arriving && self.is_muted(m) {
            changes.push(Change::Read(true));
        }
        let mut moved = false;
        for action in self.actions(m, facts) {
            if action.moves() {
                if moved {
                    continue;
                }
                moved = true;
            }
            let Some(change) = action.change() else { continue };
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
    pub fn grouped_changes(&self, mail: &[Mail], arriving: bool, facts: &Facts) -> Vec<(Change, Vec<Mail>)> {
        let mut groups: Vec<(Change, Vec<Mail>)> = Vec::new();
        for m in mail {
            for change in self.changes(m, arriving, facts) {
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
        self.silenced_with(m, &Facts::default())
    }
    /// `silenced`, with what rules may ask beyond the row.
    pub fn silenced_with(&self, m: &Mail, facts: &Facts) -> bool {
        self.is_blocked(&m.from)
            || self.is_muted(m)
            || self
                .actions(m, facts)
                .iter()
                .any(|a| a.moves() || matches!(a, RuleAction::MarkRead | RuleAction::Silence))
    }
    /// Whether any rule in use asks for the inbox category.
    pub fn wants_category(&self) -> bool {
        self.rules.iter().any(Rule::wants_category)
    }
    /// Whether any rule in use asks for headers the list doesn't keep.
    pub fn wants_headers(&self) -> bool {
        self.rules.iter().any(Rule::wants_headers)
    }
}

/// What rules applied away from the window ask beyond a message's row:
/// its inbox category (sorted as the window would, without the model) and
/// its headers (from its source, where it was downloaded). Each is read
/// only when some rule asks.
pub struct BackendFacts<'a> {
    backend: &'a Backend,
    sorting: Option<(super::categories::Categorizing, super::categories::Known)>,
    headers: bool,
}
impl<'a> BackendFacts<'a> {
    pub fn new(backend: &'a Backend, org: &Organizing, accounts: &[Account]) -> Self {
        let sorting = org.wants_category().then(|| {
            (
                super::categories::Categorizing::load(&backend.store),
                super::categories::Known::load(backend, accounts),
            )
        });
        Self { backend, sorting, headers: org.wants_headers() }
    }
    fn category(&self, m: &Mail) -> Category {
        match &self.sorting {
            Some((prefs, known)) => prefs.fold(super::categories::decide(
                m,
                super::categories::Content::of_with(m, None),
                prefs,
                known,
            )),
            None => Category::Primary,
        }
    }
    fn header(&self, m: &Mail, name: &str) -> Option<String> {
        if !self.headers {
            return None;
        }
        let raw = self.backend.store.raw(&m.id).ok().flatten()?;
        header_in(&raw, name)
    }
    /// Runs `f` with these as rules' `Facts`.
    pub fn with<T>(&self, f: impl FnOnce(&Facts) -> T) -> T {
        let category = |m: &Mail| self.category(m);
        let header = |m: &Mail, name: &str| self.header(m, name);
        f(&Facts { category: Some(&category), header: Some(&header) })
    }
}

/// Header `name`'s value in a message's source (its first such header,
/// unfolded and decoded), if it has one.
pub fn header_in(raw: &[u8], name: &str) -> Option<String> {
    use mailparse::MailHeaderMap;
    let (headers, _) = mailparse::parse_headers(raw).ok()?;
    headers.get_first_value(name)
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
    let facts = BackendFacts::new(backend, &org, std::slice::from_ref(a));
    let groups = facts.with(|facts| org.grouped_changes(fresh, true, facts));
    apply_changes(backend, a, groups)
}

/// Applies blocked senders and rules to the mail in every account's inbox
/// now (as cached), not only to mail as it arrives. Returns how many
/// messages were changed; an account whose server refused is reported
/// after the others are done.
pub fn apply_to_inboxes(backend: &Backend, accounts: &[Account]) -> anyhow::Result<usize> {
    let org = Organizing::load(&backend.store);
    let facts = BackendFacts::new(backend, &org, accounts);
    let mut changed = 0;
    let mut failed = None;
    for a in accounts {
        let inbox = backend.store.summaries(&a.id, Folder::Inbox)?;
        let groups = facts.with(|facts| org.grouped_changes(&inbox, false, facts));
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
                Rule::simple(RuleField::Subject, "invoice", RuleAction::MoveTo("Bills".into())),
                Rule::simple(RuleField::From, "shop", RuleAction::Star),
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
            rules: vec![Rule::simple(RuleField::From, "shop", RuleAction::Archive)],
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
        let groups = o.grouped_changes(&[m.clone(), read, archived], true, &Facts::default());
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].0, Change::Read(true));
        assert_eq!(groups[0].1.len(), 1, "read mail isn't marked again");
        assert_eq!(groups[1].0, Change::Move(Folder::Archive));
        assert_eq!(groups[1].1.len(), 2, "archived mail isn't moved again");
        o.rules[0].enabled = false;
        o.muted.clear();
        assert!(!o.applies_to_arrivals());
    }

    #[test]
    fn older_rules_read_as_one_condition_and_one_action() {
        let old = r#"{"field":"subject","contains":"Invoice","action":{"move_to":"Bills"},"account":"","enabled":true}"#;
        let rule: Rule = serde_json::from_str(old).unwrap();
        assert_eq!(rule, Rule::simple(RuleField::Subject, "Invoice", RuleAction::MoveTo("Bills".into())));
        // Saved again in the newer shape, and read back the same.
        let json = serde_json::to_string(&rule).unwrap();
        assert!(json.contains("conditions") && !json.contains("contains\":\"Invoice"));
        assert_eq!(serde_json::from_str::<Rule>(&json).unwrap(), rule);
        let org: Organizing = serde_json::from_str(&format!(r#"{{"rules":[{old}]}}"#)).unwrap();
        assert_eq!(org.rules.len(), 1);
    }

    #[test]
    fn several_conditions_and_actions() {
        let mut big = mail("1", "Billing <billing@shop.test>", "Your invoice #42", "");
        big.size = 900 * 1024;
        big.has_attachments = true;
        big.unread = true;
        let mut rule = Rule {
            conditions: vec![
                Condition { field: RuleField::From, op: RuleOp::EndsWith, value: "@shop.test".into(), ..Condition::default() },
                Condition { field: RuleField::Subject, op: RuleOp::Regex, value: r"invoice #\d+".into(), ..Condition::default() },
                Condition { field: RuleField::Size, op: RuleOp::Over, value: "500".into(), ..Condition::default() },
                Condition { field: RuleField::Attachment, ..Condition::default() },
            ],
            actions: vec![RuleAction::Label("Bills".into()), RuleAction::MarkRead, RuleAction::Archive],
            enabled: true,
            ..Rule::default()
        };
        assert_eq!(rule.problem(), None);
        let o = Organizing { rules: vec![rule.clone()], ..Organizing::default() };
        assert_eq!(
            o.arrival_changes(&big),
            vec![Change::Label("Bills".into(), true), Change::Read(true), Change::Move(Folder::Archive)]
        );
        // All must hold…
        let small = Mail { size: 10 * 1024, ..big.clone() };
        assert!(!rule.matches(&small));
        // …or one, with `any`.
        rule.any = true;
        assert!(rule.matches(&small));
        assert!(!rule.matches(&mail("2", "x@other.test", "hello", "")));
        // Negated: without attachments.
        let none = Condition { field: RuleField::Attachment, negate: true, ..Condition::default() };
        assert!(none.holds(&mail("3", "a@b.test", "s", ""), &Facts::default()));
        assert!(!none.holds(&big, &Facts::default()));
        assert!(rule.describe().contains("Bills"));
    }

    #[test]
    fn headers_and_categories_come_from_facts() {
        let m = mail("1", "news@shop.test", "Sale", "");
        let header = Condition {
            field: RuleField::Header,
            header: "X-Mailer".into(),
            op: RuleOp::Contains,
            value: "campaign".into(),
            ..Condition::default()
        };
        assert!(!header.holds(&m, &Facts::default()), "not downloaded: no header");
        let lookup = |_: &Mail, name: &str| (name == "x-mailer").then(|| "Campaign Monitor".to_owned());
        assert!(header.holds(&m, &Facts { header: Some(&lookup), ..Facts::default() }));
        let category = Condition { field: RuleField::Category, op: RuleOp::Is, value: "promotions".into(), ..Condition::default() };
        assert!(!category.holds(&m, &Facts::default()));
        let promo = |_: &Mail| Category::Promotions;
        assert!(category.holds(&m, &Facts { category: Some(&promo), ..Facts::default() }));
        let o = Organizing {
            rules: vec![Rule { conditions: vec![category], actions: vec![RuleAction::Silence], enabled: true, ..Rule::default() }],
            ..Organizing::default()
        };
        assert!(o.wants_category());
        let facts = Facts { category: Some(&promo), ..Facts::default() };
        assert!(o.silenced_with(&m, &facts));
        assert!(o.changes(&m, true, &facts).is_empty(), "silence changes nothing on the server");
        assert_eq!(
            header_in(b"From: a@b.test\r\nX-Mailer: Campaign\r\n Monitor\r\n\r\nbody", "x-mailer").as_deref(),
            Some("Campaign Monitor")
        );
    }

    #[test]
    fn stop_and_one_move() {
        let m = mail("1", "news@shop.test", "Sale", "");
        let mut o = Organizing {
            rules: vec![
                Rule { stop: true, ..Rule::simple(RuleField::From, "shop", RuleAction::MoveTo("Shops".into())) },
                Rule::simple(RuleField::Subject, "sale", RuleAction::Star),
            ],
            ..Organizing::default()
        };
        assert_eq!(o.arrival_changes(&m), vec![Change::MoveTo("Shops".into())]);
        o.rules[0].stop = false;
        o.rules.push(Rule::simple(RuleField::Anywhere, "sale", RuleAction::Trash));
        assert_eq!(o.arrival_changes(&m), vec![Change::Star(true), Change::MoveTo("Shops".into())]);
        let two = Rule { actions: vec![RuleAction::Archive, RuleAction::Trash], ..Rule::simple(RuleField::From, "x", RuleAction::Star) };
        assert!(two.problem().is_some());
        let bad = Rule::simple(RuleField::Subject, "(", RuleAction::Star);
        let bad = Rule { conditions: vec![Condition { op: RuleOp::Regex, ..bad.conditions[0].clone() }], ..bad };
        assert!(bad.problem().is_some());
        assert!(!bad.matches(&m));
    }
}

//! Inbox categories: the inbox sorted into Primary, Transactions, Travel,
//! Finance, Updates, Newsletters, Promotions, Social and Forums, shown as
//! tabs above it, as Gmail's and Apple Mail's inboxes are.
//!
//! Which category a message is in is worked out here, on this computer,
//! from what the list already holds of it (sender, subject, preview and
//! its list headers), so it works the same for every kind of account:
//!
//! 1. A sender the person sorted (`Categorizing::senders`), by address or
//!    `@domain`, always goes where they put it.
//! 2. Mail from the person's own addresses, and mail they replied to,
//!    is Primary.
//! 3. Receipts and package updates are Transactions (`mail_kinds`' rules),
//!    or Travel where they are a trip's.
//! 4. Sign-in codes are Primary: someone is waiting for them.
//! 5. Where the server sorted it (Gmail's categories through its API,
//!    Outlook's Focused Inbox), its choice (`Mail::server_category`), made
//!    more precise where its category lumps several of these together
//!    (Gmail's Updates: travel, finance, newsletters…; `refines`).
//! 6. Mail from people the person writes to (`Known`) is Primary, unless
//!    it came through a mailing list.
//! 7. With Settings → Local AI → Sort the inbox, the inbox category model
//!    (`local_ai::categories`), where it is sure enough to differ from the
//!    rules below: surer still to take mail out of Primary (`AI_*`).
//! 8. Otherwise the message's look: social networks' senders are Social;
//!    discussion lists Forums; bulk or automated mail with offers in it
//!    Promotions; without them, Travel from airlines, railways, hotels and
//!    booking sites or about a trip, Finance from banks and payment
//!    services or about money, Newsletters from a list that is one, and
//!    Updates otherwise. Everything else is Primary.
//!
//! Nothing but bulk or automated mail (list headers, a no-reply sender, a
//! marketing service) leaves Primary by its words alone, so a person's
//! mail isn't sorted away for mentioning a sale.
//!
//! A category whose tab is turned off (`Categorizing::hidden`) is shown in
//! the one it belongs to (`Category::parent`: Travel in Transactions,
//! Finance and Newsletters in Updates, the rest in Primary, as Gmail does).
//! Which categories' new mail is announced is `inbox_control`'s.
//! Saved as one Store preference.
use super::local_ai::categories::Ai;
use super::model::{Folder, Mail};
use super::store::Store;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::Arc;

pub const PREFERENCE: &str = "categories";

/// One of the inbox's categories, in the order its tabs show.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    /// Mail from people, and anything not sorted elsewhere.
    #[default]
    Primary,
    /// Receipts, orders and package updates.
    Transactions,
    /// Flights, trains, hotels and other bookings (in Transactions without
    /// a tab of its own).
    Travel,
    /// Banks, cards, bills, statements and payments (in Updates without a
    /// tab of its own).
    Finance,
    /// Notifications, alerts and reminders.
    Updates,
    /// Newsletters and digests to read (in Updates without a tab of its
    /// own).
    Newsletters,
    /// Offers, deals and marketing.
    Promotions,
    /// Social networks and dating, media sharing and the like.
    Social,
    /// Discussion lists and online groups.
    Forums,
}

impl Category {
    pub const ALL: [Self; 9] = [
        Self::Primary,
        Self::Transactions,
        Self::Travel,
        Self::Finance,
        Self::Updates,
        Self::Newsletters,
        Self::Promotions,
        Self::Social,
        Self::Forums,
    ];
    /// The category its mail shows in when it has no tab: the broader one
    /// it is part of (Travel's bookings are transactions too), or Primary.
    pub fn parent(self) -> Option<Self> {
        match self {
            Self::Primary => None,
            Self::Travel => Some(Self::Transactions),
            Self::Finance | Self::Newsletters => Some(Self::Updates),
            _ => Some(Self::Primary),
        }
    }
    /// Its name in searches (`category:promotions`) and in the cache.
    pub fn key(self) -> &'static str {
        match self {
            Self::Primary => "primary",
            Self::Transactions => "transactions",
            Self::Travel => "travel",
            Self::Finance => "finance",
            Self::Updates => "updates",
            Self::Newsletters => "newsletters",
            Self::Promotions => "promotions",
            Self::Social => "social",
            Self::Forums => "forums",
        }
    }
    /// The category a search names, by its key or another common name
    /// (Gmail's `personal`, Apple's `purchases`…); a prefix of three
    /// letters or more is enough (`category:promo`).
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim().to_lowercase();
        let named = match text.as_str() {
            "personal" | "main" | "focused" => Some(Self::Primary),
            "purchases" | "orders" | "receipts" | "shopping" => Some(Self::Transactions),
            "trips" | "flights" | "bookings" | "reservations" => Some(Self::Travel),
            "bank" | "banking" | "money" | "bills" | "statements" | "payments" => {
                Some(Self::Finance)
            }
            "notifications" | "alerts" => Some(Self::Updates),
            "news" | "digests" | "reading" => Some(Self::Newsletters),
            "deals" | "offers" | "marketing" | "promotional" | "promos" => Some(Self::Promotions),
            "groups" | "lists" => Some(Self::Forums),
            _ => None,
        };
        named.or_else(|| {
            (text.chars().count() >= 3)
                .then(|| Self::ALL.into_iter().find(|c| c.key().starts_with(&text)))
                .flatten()
        })
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Primary => t!("categories-primary"),
            Self::Transactions => t!("categories-transactions"),
            Self::Travel => t!("categories-travel"),
            Self::Finance => t!("categories-finance"),
            Self::Updates => t!("categories-updates"),
            Self::Newsletters => t!("categories-newsletters"),
            Self::Promotions => t!("categories-promotions"),
            Self::Social => t!("categories-social"),
            Self::Forums => t!("categories-forums"),
        }
    }
    /// What goes in it, in a few words.
    pub fn about(self) -> &'static str {
        match self {
            Self::Primary => t!("categories-primary-about"),
            Self::Transactions => t!("categories-transactions-about"),
            Self::Travel => t!("categories-travel-about"),
            Self::Finance => t!("categories-finance-about"),
            Self::Updates => t!("categories-updates-about"),
            Self::Newsletters => t!("categories-newsletters-about"),
            Self::Promotions => t!("categories-promotions-about"),
            Self::Social => t!("categories-social-about"),
            Self::Forums => t!("categories-forums-about"),
        }
    }
    pub fn icon(self) -> &'static str {
        match self {
            Self::Primary => "inbox",
            Self::Transactions => "receipt",
            Self::Travel => "plane",
            Self::Finance => "bank",
            Self::Updates => "info",
            Self::Newsletters => "subscriptions",
            Self::Promotions => "tag",
            Self::Social => "people",
            Self::Forums => "thread",
        }
    }
    /// The category a server's own sorting names (`Mail::server_category`):
    /// Gmail's `CATEGORY_*` labels and Outlook's Focused Inbox.
    fn of_server(hint: &str) -> Option<Server> {
        Some(match hint {
            "primary" | "personal" | "focused" => Server::Is(Self::Primary),
            "social" => Server::Is(Self::Social),
            "promotions" => Server::Is(Self::Promotions),
            "updates" => Server::Is(Self::Updates),
            "forums" => Server::Is(Self::Forums),
            // Outlook's Other: anything but Primary.
            "other" => Server::NotPrimary,
            _ => return None,
        })
    }
}

/// What a server said of a message's category.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Server {
    Is(Category),
    /// Not Primary, but not which other category (Outlook's Other).
    NotPrimary,
}

/// The server category Gmail's labels give a message (`CATEGORY_SOCIAL`…),
/// as `Mail::server_category` keeps it; empty for none.
pub fn gmail_category<'a>(label_ids: impl IntoIterator<Item = &'a str>) -> &'static str {
    for id in label_ids {
        let hint = match id {
            "CATEGORY_PERSONAL" => "personal",
            "CATEGORY_SOCIAL" => "social",
            "CATEGORY_PROMOTIONS" => "promotions",
            "CATEGORY_UPDATES" => "updates",
            "CATEGORY_FORUMS" => "forums",
            _ => continue,
        };
        return hint;
    }
    ""
}

/// The server category Outlook's Focused Inbox gives a message
/// (`inferenceClassification`), as `Mail::server_category` keeps it.
pub fn outlook_category(classification: &str) -> &'static str {
    match classification.to_ascii_lowercase().as_str() {
        "focused" => "focused",
        "other" => "other",
        _ => "",
    }
}

/// How the inbox is sorted into categories.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Categorizing {
    /// The inbox shows its categories as tabs.
    pub enabled: bool,
    /// Categories without a tab: their mail shows in the nearest category
    /// above them that has one (`Category::parent`).
    pub hidden: BTreeSet<Category>,
    /// Where the person sorted senders: a lowercase address, or `@domain`
    /// for a whole domain (and its subdomains).
    pub senders: BTreeMap<String, Category>,
    /// New mail was announced only when it is in Primary: read once, to
    /// carry it over to Settings → Inbox Control
    /// (`InboxControl::from_categories`), which decides it now.
    pub notify_primary_only: bool,
    /// All Mail is the first tab, and the inbox opens on it (rather than
    /// on Primary, with All Mail last).
    pub all_mail_first: bool,
}

impl Default for Categorizing {
    fn default() -> Self {
        Self {
            enabled: true,
            // As Gmail starts: forums are few, and fold into Primary.
            hidden: BTreeSet::from([Category::Forums]),
            senders: BTreeMap::new(),
            notify_primary_only: false,
            all_mail_first: false,
        }
    }
}

impl Categorizing {
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
    /// The tabs the inbox shows, in order: Primary and those not hidden.
    pub fn tabs(&self) -> Vec<Category> {
        Category::ALL
            .into_iter()
            .filter(|c| *c == Category::Primary || !self.hidden.contains(c))
            .collect()
    }
    /// Whether the inbox is sorted at all: on, with a tab besides Primary.
    pub fn sorting(&self) -> bool {
        self.enabled && self.tabs().len() > 1
    }
    /// Turns a category's tab on or off. Primary always has one.
    pub fn toggle_tab(&mut self, category: Category) {
        if category != Category::Primary && !self.hidden.remove(&category) {
            self.hidden.insert(category);
        }
    }
    /// Where the person sorted mail from `address`: the address itself, or
    /// its domain or a domain above it (`@example.com` for
    /// `news@mail.example.com`).
    pub fn sender_choice(&self, address: &str) -> Option<Category> {
        if self.senders.is_empty() {
            return None;
        }
        let address = address.trim().to_lowercase();
        if address.is_empty() {
            return None;
        }
        if let Some(c) = self.senders.get(&address) {
            return Some(*c);
        }
        let mut domain = address.rsplit_once('@')?.1;
        loop {
            if let Some(c) = self.senders.get(&format!("@{domain}")) {
                return Some(*c);
            }
            domain = domain.split_once('.')?.1;
            if !domain.contains('.') {
                return None;
            }
        }
    }
    /// Sorts mail from `address` (an address, or `@domain`) into
    /// `category` from now on; `None` forgets the choice.
    pub fn set_sender(&mut self, address: &str, category: Option<Category>) {
        let address = address.trim().to_lowercase();
        if address.is_empty() {
            return;
        }
        match category {
            Some(c) => {
                self.senders.insert(address, c);
            }
            None => {
                self.senders.remove(&address);
            }
        }
    }
    /// The tab `m` shows under: its category, or Primary when that has no
    /// tab.
    #[cfg(test)]
    pub fn category(&self, m: &Mail, known: &Known) -> Category {
        self.fold(classify(m, self, known))
    }
    /// The tab mail of `category` shows under: its own, or else the
    /// nearest category above it with one (Primary at the top).
    pub fn fold(&self, category: Category) -> Category {
        let mut c = category;
        while self.hidden.contains(&c) {
            match c.parent() {
                Some(up) => c = up,
                None => break,
            }
        }
        c
    }
}

/// Who the person writes to and is: mail from them is Primary.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Known {
    /// Addresses written to (sent mail's recipients) and those in the
    /// address book, lowercase.
    pub people: HashSet<String>,
    /// The person's own addresses (accounts and their other addresses),
    /// lowercase.
    pub own: HashSet<String>,
}

impl Known {
    /// Who `accounts` have written to, the address book, and the
    /// accounts' own addresses.
    pub fn load(backend: &super::backend::Backend, accounts: &[super::model::Account]) -> Self {
        let ids: Vec<String> = accounts.iter().map(|a| a.id.clone()).collect();
        let mut people = backend.store.correspondents(&ids).unwrap_or_default();
        for card in backend.contacts() {
            people.extend(card.emails.iter().map(|e| e.trim().to_lowercase()));
        }
        let own: HashSet<String> = accounts
            .iter()
            .flat_map(|a| {
                std::iter::once(a.email.trim().to_lowercase())
                    .chain(a.identities.iter().map(|i| i.email.trim().to_lowercase()))
            })
            .filter(|e| !e.is_empty())
            .collect();
        people.retain(|p| !own.contains(p));
        Self { people, own }
    }
}

/// The addresses in a header's list of people (`Ana <ana@x.org>, bo@y.org`),
/// lowercase.
pub fn addresses_in(list: &str) -> impl Iterator<Item = String> + '_ {
    list.split(|c: char| c.is_whitespace() || matches!(c, ',' | ';' | '<' | '>' | '"' | '(' | ')'))
        .filter(|t| {
            t.split_once('@').is_some_and(|(local, domain)| {
                !local.is_empty() && domain.contains('.') && !domain.ends_with('.')
            })
        })
        .map(|t| {
            t.trim_matches(|c: char| c == '.' || c == '\'')
                .to_lowercase()
        })
}

/// The category `m` belongs in (whether or not it has a tab); see the
/// module's comment for the order things are decided in.
#[cfg(test)]
pub fn classify(m: &Mail, prefs: &Categorizing, known: &Known) -> Category {
    decide(m, Content::of(m), prefs, known)
}

/// What a message's own sender, headers and words say of its category:
/// the costly part of sorting it, which depends on nothing else, so it is
/// worked out once per message (and off the window's thread) and kept.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Content {
    /// A receipt or a package update.
    transaction: bool,
    /// A sign-in or verification code.
    code: bool,
    /// What its look says (`Look::category`).
    look: Category,
    /// Whether receipts, packages and codes were looked for. A quick
    /// look (`quick`) leaves them out until the whole one is done.
    pub complete: bool,
    /// The inbox category model's likeliest category, and how likely (in
    /// thousandths), where it was asked.
    ai: Option<(Category, u16)>,
    /// The model's generation (`Ai::generation`) that was asked; 0 for
    /// none.
    generation: u64,
}

impl Content {
    /// The whole look at `m`.
    #[cfg(test)]
    pub fn of(m: &Mail) -> Self {
        Self::of_with(m, None)
    }
    /// The whole look at `m`, asking the inbox category model too where
    /// there is one.
    pub fn of_with(m: &Mail, ai: Option<&Ai>) -> Self {
        let from = m.from.trim().to_ascii_lowercase();
        let look = Look::of(m, &from);
        Self {
            transaction: look.transaction(m),
            code: look.code(m),
            look: look.category(m),
            complete: true,
            ai: ai.map(|ai| {
                let (c, p) = ai.verdict(m);
                (c, (p.clamp(0., 1.) * 1000.).round() as u16)
            }),
            generation: ai.map_or(0, |ai| ai.generation),
        }
    }
    /// A quick look at `m`, without looking for receipts, packages and
    /// codes, which take longest: what a long list shows until the whole
    /// look is done.
    pub fn quick(m: &Mail) -> Self {
        let from = m.from.trim().to_ascii_lowercase();
        let look = Look::of(m, &from);
        Self {
            transaction: look.order_sender(),
            code: false,
            look: look.category(m),
            complete: false,
            ai: None,
            generation: 0,
        }
    }
    /// What the look at `m` depends on, hashed: a message whose
    /// fingerprint changed is looked at again.
    pub fn fingerprint(m: &Mail) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (
            &m.from,
            &m.sender,
            &m.subject,
            m.snippet.len(),
            m.list_unsubscribe.len(),
        )
            .hash(&mut h);
        (m.list_id(), m.list_links().len(), m.folder).hash(&mut h);
        h.finish()
    }
}

/// Sorts listed mail into its tabs: the preferences, the people known, and
/// what each message's content was found to say (`Content`), kept by
/// message so each is looked at once.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Sorter {
    pub prefs: Categorizing,
    pub known: Arc<Known>,
    /// The inbox category model, where Settings → Local AI turned it on.
    pub ai: Option<SortingAi>,
    /// By message id: the fingerprint it had when looked at, and what was
    /// found.
    contents: HashMap<String, (u64, Content)>,
}

/// The inbox category model as a `Sorter` holds it: one is another while
/// it is the same generation.
#[derive(Clone, Debug)]
pub struct SortingAi(pub Arc<Ai>);
impl PartialEq for SortingAi {
    fn eq(&self, other: &Self) -> bool {
        self.0.generation == other.0.generation
    }
}
impl Eq for SortingAi {}

impl Sorter {
    pub fn new(prefs: Categorizing) -> Self {
        Self {
            prefs,
            ..Self::default()
        }
    }
    /// What was found in `m`, if it hasn't changed since.
    pub fn content(&self, m: &Mail) -> Option<Content> {
        self.contents
            .get(&m.id)
            .filter(|(fingerprint, _)| *fingerprint == Content::fingerprint(m))
            .map(|(_, c)| *c)
    }
    /// The category `m` belongs in (`classify`), from what was found in
    /// it, or a quick look where nothing was yet. Without the model (turned
    /// off since), what it said is left out.
    pub fn category(&self, m: &Mail) -> Category {
        let mut content = self.content(m).unwrap_or_else(|| Content::quick(m));
        if self.ai.is_none() {
            content.ai = None;
        }
        decide(m, content, &self.prefs, &self.known)
    }
    /// Whether `m` still wants a whole look: not looked at closely yet, or
    /// not by the current model.
    pub fn wants_close_look(&self, m: &Mail) -> bool {
        !self.content(m).is_some_and(|c| self.looked_closely(&c))
    }
    /// Whether `c` is a whole look as good as one taken now: with the
    /// model, by this generation of it; without, any whole look (what the
    /// model said is left out then). Turning the model off used to have
    /// every message looked at again.
    fn looked_closely(&self, c: &Content) -> bool {
        c.complete && (self.ai.is_none() || c.generation == self.generation())
    }
    /// The tab `m` shows under, from what was found in it, without looking
    /// at it now: one not looked at yet (it will be, off the window's
    /// thread) shows in Primary unless its sender was put elsewhere.
    pub fn tab_known(&self, m: &Mail) -> Category {
        let mut content = self.content(m).unwrap_or_default();
        if self.ai.is_none() {
            content.ai = None;
        }
        self.prefs.fold(decide(m, content, &self.prefs, &self.known))
    }
    /// The model's generation looks are wanted from (0 without one).
    fn generation(&self) -> u64 {
        self.ai.as_ref().map_or(0, |ai| ai.0.generation)
    }
    /// The model, for looking closely (`look_closely`).
    pub fn model(&self) -> Option<Arc<Ai>> {
        self.ai.as_ref().map(|ai| ai.0.clone())
    }
    /// The tab `m` shows under, looking at it quickly if it wasn't yet.
    #[cfg(test)]
    pub fn tab(&self, m: &Mail) -> Category {
        self.prefs.fold(self.category(m))
    }
    /// Looks quickly at those of `messages` not looked at yet (or changed
    /// since), and returns every one a whole look is still wanted for.
    ///
    /// The window asks this of every listed message each time its list is
    /// filtered, on its own thread: a list first shown (tens of thousands
    /// of messages at launch) was looked at one message after another,
    /// holding up the window for seconds. Long lists are looked at on
    /// several threads.
    #[cfg(test)]
    pub fn look_quickly<'a>(
        &mut self,
        messages: impl IntoIterator<Item = &'a Mail>,
    ) -> Vec<&'a Mail> {
        self.look_quickly_at_most(messages, usize::MAX)
    }
    /// `look_quickly`, at no more than `most` messages not looked at yet
    /// (the first ones, the list's newest); the others are left for the
    /// whole look (and returned with those wanting it). On the window's
    /// thread a quick look costs about half a millisecond a message: tens
    /// of seconds of work for a large mailbox, which held up the window.
    pub fn look_quickly_at_most<'a>(
        &mut self,
        messages: impl IntoIterator<Item = &'a Mail>,
        most: usize,
    ) -> Vec<&'a Mail> {
        /// What a message needs: nothing, a whole look, or a first look
        /// (with its fingerprint).
        #[derive(Clone, Copy)]
        enum Need {
            Nothing,
            Whole,
            First(u64),
        }
        let messages: Vec<&'a Mail> = messages.into_iter().collect();
        let this = &*self;
        let needs: Vec<Need> = in_parts(&messages, |m| {
            let fingerprint = Content::fingerprint(m);
            match this.contents.get(&m.id).filter(|(f, _)| *f == fingerprint) {
                Some((_, c)) if this.looked_closely(c) => Need::Nothing,
                Some(_) => Need::Whole,
                None => Need::First(fingerprint),
            }
        });
        let first: Vec<(&'a Mail, u64)> = messages
            .iter()
            .zip(&needs)
            .filter_map(|(m, need)| match need {
                Need::First(fingerprint) => Some((*m, *fingerprint)),
                _ => None,
            })
            .take(most)
            .collect();
        let looks = in_parts(&first, |(m, _)| Content::quick(m));
        for ((m, fingerprint), look) in first.into_iter().zip(looks) {
            self.contents.insert(m.id.clone(), (fingerprint, look));
        }
        messages
            .into_iter()
            .zip(needs)
            .filter(|(_, need)| !matches!(need, Need::Nothing))
            .map(|(m, _)| m)
            .collect()
    }
    /// Keeps what whole looks found: (message id, fingerprint, content).
    pub fn keep(&mut self, found: Vec<(String, u64, Content)>) {
        for (id, fingerprint, content) in found {
            self.contents.insert(id, (fingerprint, content));
        }
    }
    /// Forgets messages no longer listed, once there are many more of them
    /// than listed ones.
    pub fn forget_unlisted(&mut self, listed: &[Mail]) {
        if self.contents.len() <= 2 * listed.len() + 1000 {
            return;
        }
        let ids: HashSet<&str> = listed.iter().map(|m| m.id.as_str()).collect();
        self.contents.retain(|id, _| ids.contains(id.as_str()));
    }
}

/// How many threads a quick look at a long list may use (the window waits
/// for it): one core is left free (a phone has few), and more than eight
/// gain little.
fn look_threads() -> usize {
    std::thread::available_parallelism().map_or(1, |n| n.get().saturating_sub(1).clamp(1, 8))
}

/// `look` of each of `items`, in order: on several threads when there are
/// enough of them to be worth it, otherwise here.
fn in_parts<T: Sync, R: Send>(items: &[T], look: impl Fn(&T) -> R + Sync) -> Vec<R> {
    const PART: usize = 1024;
    let serial = || items.iter().map(&look).collect::<Vec<R>>();
    let threads = look_threads();
    if threads < 2 || items.len() < 2 * PART {
        return serial();
    }
    let size = items.len().div_ceil(threads).max(PART);
    let look = &look;
    std::thread::scope(|scope| {
        let parts: Vec<_> = items
            .chunks(size)
            .map(|part| scope.spawn(move || part.iter().map(look).collect::<Vec<R>>()))
            .collect();
        let mut done = Vec::with_capacity(items.len());
        for part in parts {
            done.extend(part.join().ok()?);
        }
        Some(done)
    })
    // A part that panicked: looked at again here.
    .unwrap_or_else(serial)
}

/// Whole looks at `messages` (`Content::of_with`, asking `ai` too where
/// given), with their fingerprints, on as many threads as are useful: what
/// the window asks for in the background as new mail is listed.
pub fn look_closely(messages: &[Mail], ai: Option<&Ai>) -> Vec<(String, u64, Content)> {
    const PART: usize = 256;
    let look = |part: &[Mail]| -> Vec<(String, u64, Content)> {
        part.iter()
            .map(|m| {
                (
                    m.id.clone(),
                    Content::fingerprint(m),
                    Content::of_with(m, ai),
                )
            })
            .collect()
    };
    // In the background, beside a window that's checking mail too: a
    // quarter of the cores at most, each at a lower priority. On every
    // core, the window waited on them (and on the allocator they kept
    // busy) for seconds at launch.
    let threads = std::thread::available_parallelism().map_or(1, |n| (n.get() / 4).clamp(1, 4));
    if messages.len() < 2 * PART {
        return look(messages);
    }
    let size = messages.len().div_ceil(threads).max(PART);
    std::thread::scope(|scope| {
        let parts: Vec<_> = messages
            .chunks(size)
            .map(|part| {
                scope.spawn(move || {
                    crate::manage::memory::lower_priority();
                    look(part)
                })
            })
            .collect();
        let mut found = Vec::with_capacity(messages.len());
        for part in parts {
            // A part that panicked is looked at again here.
            match part.join() {
                Ok(done) => found.extend(done),
                Err(_) => return None,
            }
        }
        Some(found)
    })
    .unwrap_or_else(|| look(messages))
}

/// The category `m` belongs in, given what its content says (`Content`):
/// `classify`, with the costly part done already.
pub fn decide(m: &Mail, content: Content, prefs: &Categorizing, known: &Known) -> Category {
    let from = m.from.trim().to_ascii_lowercase();
    if let Some(c) = prefs.sender_choice(&from) {
        return c;
    }
    if matches!(m.folder, Folder::Sent | Folder::Drafts) || known.own.contains(&from) || m.answered
    {
        return Category::Primary;
    }
    if content.transaction {
        // A booking's receipt is travel's.
        return if content.look == Category::Travel {
            Category::Travel
        } else {
            Category::Transactions
        };
    }
    if content.code {
        return Category::Primary;
    }
    let server = Category::of_server(m.server_category());
    if let Some(Server::Is(c)) = server {
        // The server's categories are fewer: where what was found here is
        // one of those it lumps together, that is more precise.
        let ours = match content.ai {
            Some((said, thousandths)) if thousandths >= 700 => said,
            _ => content.look,
        };
        return if refines(c, ours) { ours } else { c };
    }
    if known.people.contains(&from) && m.list_id().is_empty() && server.is_none() {
        return Category::Primary;
    }
    let rules = match content.look {
        // Outlook put it in Other: not Primary, though nothing here says
        // what it is.
        Category::Primary if server == Some(Server::NotPrimary) => Category::Updates,
        c => c,
    };
    if let Some((said, thousandths)) = content.ai {
        let sure = f32::from(thousandths) / 1000.;
        let needed = if said == rules {
            0.
        } else if rules == Category::Primary {
            AI_OUT_OF_PRIMARY
        } else if said == Category::Primary {
            AI_INTO_PRIMARY
        } else {
            AI_OVER_RULES
        };
        let refused = said == Category::Primary && server == Some(Server::NotPrimary);
        if sure >= needed && !refused {
            return said;
        }
    }
    rules
}

/// Whether `ours` is a more precise category than the server's `theirs`:
/// Gmail's Updates holds receipts, bookings, bank mail and newsletters, and
/// its Promotions newsletters too.
fn refines(theirs: Category, ours: Category) -> bool {
    use Category::*;
    matches!(
        (theirs, ours),
        (Updates, Transactions | Travel | Finance | Newsletters) | (Promotions, Newsletters)
    )
}

/// How sure the inbox category model has to be to sort mail elsewhere than
/// the rules would: to take it out of Primary (a person's mail must stay
/// there), to put it in Primary, and otherwise.
const AI_OUT_OF_PRIMARY: f32 = 0.9;
const AI_INTO_PRIMARY: f32 = 0.8;
const AI_OVER_RULES: f32 = 0.7;

/// What a message's sender, headers and words say of it.
struct Look<'a> {
    /// The sender's address before the `@`, and its domain.
    local: &'a str,
    domain: &'a str,
    /// From a list, a newsletter, or a marketing service: sent in bulk.
    bulk: bool,
    /// From an address no one reads (`noreply@`, `notifications@`…).
    automated: bool,
    /// Sent through a service only marketing mail goes through.
    marketing_service: bool,
}

impl<'a> Look<'a> {
    fn of(m: &Mail, from: &'a str) -> Self {
        let (local, domain) = from.rsplit_once('@').unwrap_or((from, ""));
        let unsubscribe = m.list_unsubscribe.to_ascii_lowercase();
        let marketing_service = MARKETING_SERVICES
            .iter()
            .any(|s| unsubscribe.contains(s) || domain.contains(s));
        // Shops' order and receipt addresses are no one's either, nor are
        // a sending service's subdomains (`e.shop.example`).
        let automated = automated_sender(local)
            || transactional_sender(local)
            || sending_subdomain(domain)
            || in_domains(domain, TRAVEL_DOMAINS)
            || in_domains(domain, FINANCE_DOMAINS);
        let bulk = !m.list_unsubscribe.trim().is_empty()
            || !m.list_id().is_empty()
            || marketing_service
            || marketing_sender(local);
        Self {
            local,
            domain,
            bulk,
            automated,
            marketing_service,
        }
    }

    /// Mail one-to-one from an address only orders and receipts come
    /// from (`orders@`, `receipts@`…): a transaction, whatever the
    /// receipt finder makes of its words.
    fn order_sender(&self) -> bool {
        !self.bulk && ORDER_WORDS.contains(&first_word(self.local))
    }

    /// A receipt or a package update.
    fn transaction(&self, m: &Mail) -> bool {
        if self.order_sender() {
            return true;
        }
        // People don't send receipts; shops and carriers do, from
        // addresses like these, or in bulk.
        if !(self.bulk || self.automated) {
            return false;
        }
        super::mail_kinds::receipt_by_rule(&m.subject, &m.snippet, &m.sender, &m.from)
            || super::mail_kinds::package_by_rule(&m.subject, &m.snippet, &m.from)
    }

    /// A sign-in or verification code.
    fn code(&self, m: &Mail) -> bool {
        let digits = |s: &str| s.bytes().any(|b| b.is_ascii_digit());
        // A code is digits (in ASCII here, mostly); without any, there is
        // nothing to tokenize for.
        (digits(&m.subject) || digits(&m.snippet))
            && super::mail_kinds::code_by_rule(&m.subject, &m.snippet)
    }

    /// The category the message's look gives it.
    fn category(&self, m: &Mail) -> Category {
        if in_domains(self.domain, SOCIAL_DOMAINS) {
            return Category::Social;
        }
        if self.forum(m) {
            return Category::Forums;
        }
        if self.bulk || self.automated {
            let words = Words::of(&m.subject, &m.snippet);
            let travel = in_domains(self.domain, TRAVEL_DOMAINS);
            let finance = in_domains(self.domain, FINANCE_DOMAINS);
            // A bank's or an airline's amounts aren't offers; their words
            // for one are.
            let priced = words.priced() && !travel && !finance;
            if self.marketing_service || words.offers() || priced {
                return Category::Promotions;
            }
            if travel || words.any(TRAVEL_PHRASES) {
                return Category::Travel;
            }
            if finance || words.any(FINANCE_PHRASES) {
                return Category::Finance;
            }
            if self.newsletter(m, &words) {
                return Category::Newsletters;
            }
            return Category::Updates;
        }
        Category::Primary
    }

    /// A newsletter: from a list or with a way to unsubscribe, and from a
    /// newsletter's address, a newsletter service, or saying it is one.
    fn newsletter(&self, m: &Mail, words: &Words) -> bool {
        let listed = !m.list_unsubscribe.trim().is_empty() || !m.list_id().is_empty();
        listed
            && (NEWSLETTER_WORDS.contains(&first_word(self.local))
                || in_domains(self.domain, NEWSLETTER_DOMAINS)
                || words.any(NEWSLETTER_PHRASES))
    }

    /// A discussion list's mail: people writing to a list that takes
    /// posts, or a known home of groups.
    fn forum(&self, m: &Mail) -> bool {
        let list = m.list_id();
        if list.is_empty() || self.marketing_service {
            return false;
        }
        let posts = m
            .list_links()
            .iter()
            .any(|l| l.kind == super::list_headers::ListLinkKind::Post);
        posts
            || FORUM_HOSTS
                .iter()
                .any(|h| list.ends_with(h) || self.domain.ends_with(h))
    }
}

/// Whether `domain` is one of `domains` or under one.
fn in_domains(domain: &str, domains: &[&str]) -> bool {
    !domain.is_empty()
        && domains.iter().any(|d| {
            domain == *d
                || domain
                    .strip_suffix(d)
                    .is_some_and(|rest| rest.ends_with('.'))
        })
}

/// The first word of an address's local part (`news` of `news.letter+x`,
/// `alerts` of `alerts-123`), without trailing digits.
fn first_word(local: &str) -> &str {
    let word = local.split(['.', '-', '_', '+']).next().unwrap_or(local);
    word.trim_end_matches(|c: char| c.is_ascii_digit())
}

/// An address no one reads replies at.
fn automated_sender(local: &str) -> bool {
    let squeezed: String = local
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    [
        "noreply",
        "donotreply",
        "dontreply",
        "noresponse",
        "mailerdaemon",
        "postmaster",
    ]
    .iter()
    .any(|w| squeezed.contains(w))
        || AUTOMATED_WORDS.contains(&first_word(local))
}

/// A domain whose subdomain is one sending services use for automated or
/// marketing mail (`e.delta.com`, `alerts.bank.example`). `mail.` and
/// `email.` aren't among them: universities give people addresses there.
fn sending_subdomain(domain: &str) -> bool {
    let labels: Vec<&str> = domain.split('.').collect();
    // Only what's left of the last two labels (`shop.example`).
    labels.len() > 2
        && labels[..labels.len() - 2]
            .iter()
            .any(|l| SENDING_SUBDOMAINS.contains(&first_word(l)))
}

/// An address that sends marketing.
fn marketing_sender(local: &str) -> bool {
    MARKETING_WORDS.contains(&first_word(local))
}

/// An address that sends receipts and orders.
fn transactional_sender(local: &str) -> bool {
    TRANSACTIONAL_WORDS.contains(&first_word(local))
}

/// A message's subject and the start of its preview, as words: lowercase,
/// with punctuation as spaces (but `%`, `$`, `€`, `£` kept) and a space
/// at each end, so a word is found as ` word `.
struct Words {
    text: String,
    subject: String,
}

impl Words {
    fn of(subject: &str, preview: &str) -> Self {
        let preview: String = preview.chars().take(400).collect();
        Self {
            text: words(&format!("{subject} {preview}")),
            subject: subject.to_owned(),
        }
    }
    /// Whether any of `phrases` is in the text as whole words.
    fn any(&self, phrases: &[&str]) -> bool {
        phrases.iter().any(|p| self.has(p))
    }
    /// Whether `phrase` is in the text: as whole words; at a word's start
    /// where it ends in `*` (a stem: "скидк*" for скидка, скидки…); and
    /// anywhere where its script attaches words to it or writes without
    /// spaces (`attached`).
    fn has(&self, phrase: &str) -> bool {
        if phrase.chars().any(attached) {
            return self.text.contains(phrase);
        }
        let (phrase, stem) = match phrase.strip_suffix('*') {
            Some(stem) => (stem, true),
            None => (phrase, false),
        };
        let bytes = self.text.as_bytes();
        self.text.match_indices(phrase).any(|(i, _)| {
            i > 0 && bytes[i - 1] == b' ' && (stem || bytes.get(i + phrase.len()) == Some(&b' '))
        })
    }
    /// Offers, sales and the like, or shopping's emoji.
    fn offers(&self) -> bool {
        // "70% off" anywhere (its number and sign are one word here).
        PROMO_PHRASES.iter().any(|p| self.has(p))
            || self.text.contains("% off ")
            || PROMO_EMOJI.iter().any(|e| self.subject.contains(e))
    }
    /// A percentage or a price in the subject: an offer's, mostly, though
    /// banks and airlines give amounts too.
    fn priced(&self) -> bool {
        // "Up to 70% off", "Save $20", "€10 voucher".
        let s = &self.subject;
        let after_digit = |sign: char| {
            s.char_indices().any(|(i, c)| {
                c == sign && s[..i].chars().last().is_some_and(|p| p.is_ascii_digit())
            })
        };
        let before_digit = |sign: char| {
            s.char_indices().any(|(i, c)| {
                c == sign
                    && s[i + c.len_utf8()..]
                        .chars()
                        .next()
                        .is_some_and(|n| n.is_ascii_digit())
            })
        };
        after_digit('%') || before_digit('$') || before_digit('€') || before_digit('£')
    }
}

/// Whether `c` is in a script whose words take others attached to them
/// (Arabic's and Hebrew's articles and prepositions, Korean's particles) or
/// are written without spaces (Thai, Japanese, Chinese): there a phrase is
/// looked for anywhere, not as whole words.
fn attached(c: char) -> bool {
    matches!(u32::from(c),
        0x0590..=0x06FF // Hebrew, Arabic (and Persian)
        | 0x0750..=0x077F | 0xFB1D..=0xFDFF | 0xFE70..=0xFEFF
        | 0x0E00..=0x0E7F // Thai
        | 0x1100..=0x11FF | 0x3130..=0x318F | 0xAC00..=0xD7AF // Hangul
        | 0x3040..=0x30FF // Hiragana, Katakana
        | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF // Han
    )
}

/// `text` lowercased, with punctuation turned into single spaces, and a
/// space at each end.
fn words(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push(' ');
    let mut space = true;
    for c in text.chars() {
        if c.is_alphanumeric() || in_word(c) || matches!(c, '%' | '$' | '€' | '£') {
            out.extend(c.to_lowercase());
            space = false;
        } else if c == '\'' || c == '’' {
            // "don't" stays one word.
        } else if !space {
            out.push(' ');
            space = true;
        }
    }
    if !space {
        out.push(' ');
    }
    out
}

/// Whether `c` belongs in a word though it is no letter or digit:
/// combining marks (Thai's and Indic scripts' vowel signs, Arabic's and
/// Hebrew's points, accents typed apart) and Persian's zero-width joiners,
/// which break a word apart if taken for spaces.
fn in_word(c: char) -> bool {
    matches!(u32::from(c),
        0x0300..=0x036F | 0x0483..=0x0489 | 0x0591..=0x05C7 | 0x0610..=0x061A
        | 0x064B..=0x065F | 0x0670 | 0x06D6..=0x06ED | 0x0900..=0x0DFF | 0x0E31..=0x0E3A
        | 0x0E47..=0x0E4E | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF | 0x200C | 0x200D
        | 0x20D0..=0x20FF | 0x3099..=0x309A | 0xFE20..=0xFE2F
    ) && !c.is_whitespace()
}

/// Words (and phrases) of offers, in every language the other local
/// models read (the reply model's 37), as `Words` writes them; a phrase
/// ending in `*` is a stem, matched at a word's start (`Words::has`).
/// Only bulk or automated mail is looked at for them.
#[rustfmt::skip]
const PROMO_PHRASES: &[&str] = &[
    // English.
    "sale", "sales event", "on sale", "deal", "deals", "discount", "discounts", "coupon", "coupons",
    "promo", "promo code", "promotion", "offer", "offers", "special offer", "save up to",
    "free shipping", "free delivery", "free gift", "limited time", "last chance", "ends tonight",
    "ends today", "ends soon", "ending soon", "final hours", "shop now", "shop the", "buy now",
    "new arrivals", "just dropped", "back in stock", "black friday", "cyber monday", "flash sale",
    "clearance", "bogo", "buy one", "exclusive", "early access", "members only", "dont miss",
    "hurry", "gift guide", "gift ideas", "voucher", "rewards", "bonus points", "upgrade now",
    "subscribe and save", "best sellers", "bestsellers", "trending now", "price drop",
    "lowest price", "lowest prices", "big savings", "save big", "treat yourself", "giveaway",
    "sweepstakes",
    // Spanish.
    "oferta", "ofertas", "descuento", "descuentos", "rebajas", "envío gratis", "envio gratis",
    "promoción", "promocion", "cupón", "cupon", "liquidación", "compra ahora",
    // German.
    "angebot", "angebote", "rabatt", "gutschein", "versandkostenfrei", "jetzt kaufen",
    "schnäppchen", "sonderangebot", "ausverkauf",
    // French.
    "soldes", "réduction", "reduction", "offre", "offres", "livraison gratuite", "bon plan",
    "code promo", "profitez", "promos",
    // Italian.
    "sconto", "sconti", "offerta", "offerte", "saldi", "spedizione gratuita", "promozione",
    // Portuguese.
    "desconto", "descontos", "promoção", "frete grátis", "frete gratis", "liquidação",
    // Dutch.
    "korting", "aanbieding", "aanbiedingen", "uitverkoop", "gratis verzending",
    // Polish.
    "promocja", "rabat", "wyprzedaż", "okazja", "darmowa dostawa",
    // The reply model's other 30 languages (a stem ending in "*" is a word's start).
    "rea", "rean", "rabatt*", "rabat*", "udsalg", "tilbud", "fri frakt", "gratis fragt",
    "gratis frakt", "bara i dag", "kun i dag", "bare i dag", "rabattkod", "rabatkode", "rabattkode",
    "rebaixes", "descompte", "enviament gratuït", "només avui", "alennus*", "alennusmyynti",
    "ilmainen toimitus", "vain tänään", "alennuskoodi", "скидк*", "знижк*", "распродаж*",
    "розпродаж*", "промокод", "бесплатная доставка", "безкоштовна доставка", "только сегодня",
    "лише сьогодні", "намален*", "отстъпк*", "безплатна доставка", "само днес", "slev*", "výprodej",
    "doprava zdarma", "jen dnes", "zľav*", "výpredaj", "doprava zadarmo", "len dnes", "popust*",
    "rasprodaj*", "besplatna dostava", "samo danas", "reducere*", "reduceri", "livrare gratuită",
    "doar azi", "kedvezmény*", "leárazás", "ingyenes szállítás", "csak ma", "έκπτωσ*", "εκπτώσ*",
    "δωρεάν αποστολή", "μόνο σήμερα", "indirim*", "ücretsiz kargo", "sadece bugün", "kupon", "छूट",
    "सेल", "मुफ़्त डिलीवरी", "सिर्फ़ आज", "कूपन", "ছাড়", "সেল", "ফ্রি ডেলিভারি", "শুধু আজ",
    "diskon", "obral", "gratis ongkir", "hanya hari ini", "kode promo", "diskaun", "jualan",
    "penghantaran percuma", "hari ini sahaja", "giảm giá", "khuyến mãi", "miễn phí vận chuyển",
    "chỉ hôm nay", "mã giảm giá", "libreng shipping", "ngayong araw lang", "punguzo", "ofa",
    "usafirishaji bure", "leo tu", "خصم", "تخفيضات", "شحن مجاني", "اليوم فقط", "رمز الخصم", "تخفیف",
    "حراج", "ارسال رایگان", "فقط امروز", "کد تخفیف", "הנחה", "מבצע", "משלוח חינם", "רק היום",
    "קוד קופון", "ลดราคา", "ส่วนลด", "ส่งฟรี", "วันนี้วันเดียว", "โปรโมชัน", "セール", "割引", "送料無料",
    "本日限り", "クーポン", "促销", "特价", "折扣", "包邮", "优惠券", "优惠码", "仅限今天", "特賣", "免運", "優惠碼", "僅限今天", "할인",
    "세일", "무료 배송", "쿠폰", "오늘 하루만",
];

/// Emoji that shops put in subjects.
const PROMO_EMOJI: &[&str] = &["🛍", "🎁", "💸", "💰", "🏷", "🤑", "🛒"];

/// Services that send marketing mail (seen in a sender's domain or the
/// List-Unsubscribe address). Those that send receipts and alerts too
/// (Amazon SES, SendGrid, Mailgun, Postmark) aren't among them.
#[rustfmt::skip]
const MARKETING_SERVICES: &[&str] = &[
    "list-manage.com", "mcsv.net", "mcdlv.net", "rsgsv.net", "mailchimp", "klaviyo", "klclick",
    "exacttarget", "exct.net", "marketingcloud", "mktomail", "marketo", "hubspotemail",
    "hubspotlinks", "constantcontact", "ccsend.com", "createsend", "cmail19", "cmail20",
    "sendinblue", "sendibm", "brevo", "omnisend", "attentivemobile", "attn.tv", "emarsys", "rsys",
    "responsys", "sailthru", "bronto", "cheetahmail", "mailerlite", "convertkit", "ck.page",
    "iterable", "braze", "shopifyemail", "activehosted", "acemsrvc", "getresponse", "aweber",
    "drip.com", "moosend", "dotdigital", "dmdelivery", "listrak", "zetaglobal", "e.customeriomail",
    "campaign-archive",
];

/// Local parts of addresses that send notifications rather than people's
/// mail.
#[rustfmt::skip]
const AUTOMATED_WORDS: &[&str] = &[
    "notifications", "notification", "notify", "notifier", "alerts", "alert", "updates", "update",
    "info", "information", "news", "newsletter", "newsletters", "digest", "mailer", "mailbot",
    "bounce", "bounces", "automated", "auto", "system", "account", "accounts", "security",
    "billing", "invoice", "invoices", "statements", "statement", "reminder", "reminders",
    "calendar", "jobs", "jobalerts", "careers", "feedback", "survey", "surveys", "service",
    "services", "support", "team", "community", "members", "member", "admin", "robot", "bot",
    "confirmation", "confirm", "verify", "verification", "welcome", "customercare",
    "customerservice",
];

/// Local parts of addresses only orders, receipts and their deliveries
/// come from.
#[rustfmt::skip]
const ORDER_WORDS: &[&str] = &[
    "orders", "order", "receipts", "receipt", "purchases", "purchase", "payments", "transaction",
    "transactions", "shipping", "shipment", "shipments", "tracking", "invoice", "invoices",
];

/// Subdomains sending services send automated and marketing mail from.
#[rustfmt::skip]
const SENDING_SUBDOMAINS: &[&str] = &[
    "e", "em", "ealerts", "alerts", "alert", "notifications", "notification", "notify", "news",
    "newsletter", "updates", "marketing", "mkt", "promo", "offers", "reply", "bounce", "bounces",
    "mailer", "comms", "communications", "engage", "txn", "transactional", "info",
];

/// Local parts of addresses that send marketing.
#[rustfmt::skip]
const MARKETING_WORDS: &[&str] = &[
    "marketing", "offers", "offer", "deals", "deal", "promo", "promos", "promotions", "promotion",
    "sale", "sales", "shop", "store", "specials", "rewards", "loyalty", "campaign", "campaigns",
    "edm", "crm", "mkt", "newsletter", "newsletters", "news", "insider", "vip",
];

/// Local parts of addresses that send receipts and orders.
#[rustfmt::skip]
const TRANSACTIONAL_WORDS: &[&str] = &[
    "orders", "order", "receipts", "receipt", "purchases", "purchase", "payments", "payment",
    "ship", "shipping", "shipment", "shipments", "delivery", "tracking", "auto", "transaction",
    "transactions", "store", "shop", "sales", "ups", "fedex", "usps", "dhl", "mcinfo",
];

/// Airlines, railways, hotels and booking sites: their mail (but offers)
/// is Travel.
#[rustfmt::skip]
const TRAVEL_DOMAINS: &[&str] = &[
    "booking.com", "airbnb.com", "expedia.com", "hotels.com", "kayak.com", "trivago.com", "agoda.com",
    "tripadvisor.com", "skyscanner.net", "ryanair.com", "easyjet.com", "lufthansa.com", "delta.com",
    "united.com", "aa.com", "britishairways.com", "ba.com", "airfrance.fr", "klm.com", "iberia.com",
    "emirates.com", "qatarairways.com", "southwest.com", "jetblue.com", "aircanada.ca", "vueling.com",
    "wizzair.com", "turkishairlines.com", "amtrak.com", "trainline.com", "sncf.fr", "sncf-connect.com",
    "renfe.com", "bahn.de", "trenitalia.com", "ns.nl", "flixbus.com", "marriott.com", "hilton.com",
    "ihg.com", "accor.com", "hyatt.com", "vrbo.com", "hertz.com", "avis.com", "sixt.com", "europcar.com",
];

/// Words of trips, in the same languages and form as `PROMO_PHRASES` (no
/// apostrophes or hyphens: `Words` drops them).
#[rustfmt::skip]
const TRAVEL_PHRASES: &[&str] = &[
    "flight", "flights", "boarding pass", "online check in", "check in now", "check in for",
    "checkin", "itinerary", "e ticket", "eticket", "your trip", "your stay", "hotel",
    "booking confirmation", "your booking", "car rental", "rental car", "departure", "layover",
    "baggage", "boarding",
    "vuelo", "tarjeta de embarque", "itinerario", "tu viaje", "tu estancia",
    "flug", "bordkarte", "reiseplan", "ihre reise", "fahrkarte", "zugticket", "buchungsbestätigung",
    "votre vol", "carte dembarquement", "votre voyage", "votre séjour",
    "voo", "cartão de embarque", "sua viagem", "hospedagem",
    "volo", "carta dimbarco", "il tuo viaggio", "soggiorno",
    "vlucht", "instapkaart", "je reis", "reisschema",
    "twój lot", "karta pokładowa", "twoja podróż",
    // The reply model's other 30 languages (a stem ending in "*" is a word's start).
    "flyg", "flyget", "boardingkort*", "incheckning*", "flyet", "indtjekning", "innsjekking",
    "hotell", "targeta dembarcament", "lentosi", "lähtöselvity*", "tarkastuskort*",
    "hotelli*", "рейс*", "посадочн*", "авиабилет*", "полет*", "бордна*", "letenk*", "palubní",
    "palubný", "odbavení", "odbavenie", "ukrcajn*", "zbor*", "îmbarcare", "járat*",
    "beszállókárty*", "πτήση*", "επιβίβασης", "uçuş*", "biniş kart*", "otel", "फ़्लाइट",
    "बोर्डिंग पास", "होटल", "ফ্লাইট", "বোর্ডিং পাস", "হোটেল", "penerbangan", "boarding pass",
    "pas masuk", "chuyến bay", "thẻ lên máy bay", "khách sạn", "safari ya ndege",
    "pasi ya kupanda ndege", "hoteli", "رحلة", "بطاقة الصعود", "فندق", "پرواز", "کارت پرواز", "هتل",
    "טיסה", "כרטיס העלייה", "מלון", "เที่ยวบิน", "บัตรขึ้นเครื่อง", "โรงแรม", "フライト", "搭乗", "ホテル",
    "航班", "登机牌", "酒店", "班機", "登機證", "飯店", "항공편", "탑승권", "호텔",
];

/// Banks, cards, payment and investing services: their mail (but offers
/// and receipts) is Finance.
#[rustfmt::skip]
const FINANCE_DOMAINS: &[&str] = &[
    "paypal.com", "chase.com", "bankofamerica.com", "wellsfargo.com", "citi.com", "citibank.com",
    "capitalone.com", "americanexpress.com", "aexp.com", "discover.com", "usbank.com", "schwab.com",
    "fidelity.com", "vanguard.com", "robinhood.com", "coinbase.com", "venmo.com", "cash.app",
    "wise.com", "revolut.com", "n26.com", "monzo.com", "starlingbank.com", "ing.com", "ing.de",
    "ing.nl", "bbva.com", "bbva.es", "santander.com", "santander.es", "caixabank.es", "hsbc.com",
    "barclays.co.uk", "lloydsbank.co.uk", "natwest.com", "deutsche-bank.de", "commerzbank.de",
    "sparkasse.de", "dkb.de", "bnpparibas.com", "societegenerale.fr", "credit-agricole.fr",
    "intesasanpaolo.com", "unicredit.it", "nubank.com.br", "itau.com.br", "bradesco.com.br",
    "rabobank.nl", "abnamro.nl", "pkobp.pl", "mbank.pl", "klarna.com", "affirm.com", "intuit.com",
];

/// Words of money (in the same languages as `TRAVEL_PHRASES`).
#[rustfmt::skip]
const FINANCE_PHRASES: &[&str] = &[
    "your statement", "account statement", "monthly statement", "statement is ready",
    "statement is available", "estatement", "your balance", "available balance", "account balance",
    "low balance", "direct deposit", "deposit received", "money transfer", "bank transfer",
    "transfer received", "transfer sent", "wire transfer", "payment received", "payment due",
    "your bill", "credit card", "debit card", "card ending", "account ending",
    "transaction alert", "overdraft", "tax", "tax return", "autopay", "minimum payment",
    "credit score", "portfolio", "dividend", "interest rate",
    "extracto", "saldo", "transferencia", "tarjeta terminada", "impuestos",
    "kontoauszug", "kontostand", "überweisung", "lastschrift", "steuer", "karte endend",
    "relevé", "solde", "virement", "prélèvement", "impôts",
    "extrato", "transferência", "fatura", "boleto", "imposto", "pix",
    "estratto conto", "bonifico", "bolletta", "addebito",
    "afschrift", "overboeking", "incasso", "belasting",
    "wyciąg", "przelew", "podatek",
    // The reply model's other 30 languages (a stem ending in "*" is a word's start).
    "kontoutdrag*", "kontoudskrift", "kontoutskrift", "saldot", "saldoen", "överföring",
    "overførsel", "overføring", "swish", "mobilepay", "vipps", "extracte", "transferència", "bizum",
    "tiliot*", "tilisiirto*", "verkkopank*", "выписк*", "виписк*", "баланс*", "перевод*",
    "переказ*", "извлечение", "превод*", "výpis*", "zůstatek", "zostatok", "převod*", "prevod*",
    "izvadak", "prijenos*", "stanje računa", "extras de cont", "soldul", "számlakivonat*",
    "egyenleg*", "átutalás*", "αντίγραφο κίνησης", "υπόλοιπο", "μεταφορά χρημάτων", "hesap özet*",
    "bakiye*", "havale*", "खाता विवरण", "बैलेंस", "ट्रांसफ़र", "upi", "স্টেটমেন্ট", "ব্যালেন্স",
    "ট্রান্সফার", "rekening koran", "e statement", "penyata akaun", "duitnow", "sao kê", "số dư",
    "chuyển tiền", "statement of account", "gcash", "taarifa ya akaunti", "salio", "m pesa",
    "كشف حساب", "رصيد", "تحويل", "صورت‌حساب", "موجودی", "انتقال وجه", "דף חשבון", "יתרה", "העברה",
    "รายการเดินบัญชี", "ยอดเงิน", "โอนเงิน", "พร้อมเพย์", "明細", "残高", "振込", "送金", "对账单", "余额", "转账",
    "對帳單", "餘額", "轉帳", "거래내역", "잔액", "송금", "입금",
];

/// Local parts of newsletters' addresses.
#[rustfmt::skip]
const NEWSLETTER_WORDS: &[&str] = &[
    "newsletter", "newsletters", "news", "digest", "weekly", "editor", "editors", "daily",
    "briefing", "bulletin", "letters", "dispatch",
];

/// Services newsletters are sent through.
#[rustfmt::skip]
const NEWSLETTER_DOMAINS: &[&str] = &[
    "substack.com", "beehiiv.com", "buttondown.email", "ghost.io", "mailerlite.com", "revue.email",
    "tinyletter.com", "medium.com", "letterdrop.com",
];

/// Words of newsletters (in the same languages as `TRAVEL_PHRASES`).
#[rustfmt::skip]
const NEWSLETTER_PHRASES: &[&str] = &[
    "newsletter", "weekly", "digest", "in this issue", "this issue", "edition", "this week",
    "briefing", "roundup", "round up", "the week in", "read more", "read online",
    "boletín", "semanal", "esta semana", "en este número",
    "wöchentlich", "ausgabe", "diese woche", "in dieser ausgabe",
    "lettre", "hebdo", "édition", "cette semaine", "dans ce numéro",
    "boletim", "edição", "nesta edição", "nesta semana",
    "settimanale", "edizione", "questa settimana", "in questo numero",
    "nieuwsbrief", "wekelijks", "editie", "deze week",
    "tygodnik", "wydanie", "w tym tygodniu", "w tym wydaniu",
    // The reply model's other 30 languages (a stem ending in "*" is a word's start).
    "nyhetsbrev", "nyhedsbrev", "i det här numret", "i dette nummer", "butlletí", "uutiskirje*",
    "tässä numerossa", "рассылк*", "розсилк*", "в этом выпуске", "у цьому випуску", "в този брой",
    "v tomto čísle", "u ovom broju", "în acest număr", "ebben a számban", "σε αυτό το τεύχος",
    "bu sayıda", "इस अंक में", "এই সংখ্যায়", "di edisi ini", "dalam edisi ini", "trong số này",
    "sa isyung ito", "katika toleo hili", "في هذا العدد", "در این شماره", "בגיליון הזה",
    "ในฉบับนี้", "今号", "ニュースレター", "本期", "电子报", "電子報", "简报", "이번 호", "뉴스레터",
];

/// Social networks, dating, and sharing sites: their mail is Social.
#[rustfmt::skip]
const SOCIAL_DOMAINS: &[&str] = &[
    "facebookmail.com", "facebook.com", "instagram.com", "linkedin.com", "x.com", "twitter.com",
    "tiktok.com", "pinterest.com", "reddit.com", "redditmail.com", "nextdoor.com", "discord.com",
    "discordapp.com", "snapchat.com", "tumblr.com", "quora.com", "meetup.com", "strava.com",
    "goodreads.com", "bsky.app", "threads.net", "youtube.com", "whatsapp.com", "telegram.org",
    "vk.com", "xing.com", "flickr.com", "gotinder.com", "tinder.com", "bumble.com", "hinge.co",
    "match.com", "okcupid.com", "myspace.com", "mastodon.social", "letterboxd.com", "untappd.com",
    "deviantart.com", "twitch.tv", "mewe.com", "skype.com",
];

/// Where discussion groups live: their lists' mail is Forums.
#[rustfmt::skip]
const FORUM_HOSTS: &[&str] = &[
    "googlegroups.com", "groups.io", "freelists.org", "lists.sourceforge.net", "discoursemail.com",
    "groups.google.com", "simplelists.com", "listserv.net", "topicbox.com",
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manage::list_headers::{ListLink, ListLinkKind};

    fn mail(from: &str, subject: &str, snippet: &str) -> Mail {
        Mail {
            id: format!("{from}:{subject}"),
            from: from.into(),
            sender: from.split('@').next().unwrap_or_default().into(),
            subject: subject.into(),
            snippet: snippet.into(),
            ..Mail::default()
        }
    }
    fn bulk(from: &str, subject: &str, snippet: &str) -> Mail {
        Mail {
            list_unsubscribe: "<https://example.com/unsubscribe>".into(),
            ..mail(from, subject, snippet)
        }
    }
    fn of(m: &Mail) -> Category {
        classify(m, &Categorizing::default(), &Known::default())
    }

    #[test]
    fn people_stay_in_primary() {
        assert_eq!(
            of(&mail(
                "ana@example.com",
                "Lunch on Friday?",
                "Are you free?"
            )),
            Category::Primary
        );
        // Words of an offer from a person are still a person's mail.
        assert_eq!(
            of(&mail(
                "ana@example.com",
                "Garage sale this weekend",
                "50% off everything, come by!"
            )),
            Category::Primary
        );
        // Replied to, or sent: Primary whatever it looks like.
        let mut newsletter = bulk("news@shop.example", "Black Friday deals", "");
        newsletter.answered = true;
        assert_eq!(of(&newsletter), Category::Primary);
    }

    #[test]
    fn offers_are_promotions_and_other_bulk_mail_updates() {
        assert_eq!(
            of(&bulk("news@shop.example", "Up to 70% off ends tonight", "")),
            Category::Promotions
        );
        assert_eq!(
            of(&bulk(
                "hello@brand.example",
                "New arrivals just dropped",
                "Shop now"
            )),
            Category::Promotions
        );
        assert_eq!(
            of(&bulk("info@tienda.example", "Rebajas de invierno", "")),
            Category::Promotions
        );
        assert_eq!(
            of(&mail("deals@shop.example", "Save $20 today", "")),
            Category::Promotions
        );
        // A marketing service's mail is marketing.
        let mut via = mail("store@brand.example", "Our story", "");
        via.list_unsubscribe = "<https://brand.us1.list-manage.com/unsubscribe?u=1>".into();
        assert_eq!(of(&via), Category::Promotions);
        // Bulk without an offer: an update.
        assert_eq!(
            of(&bulk(
                "hello@thebrowser.com",
                "Five things worth your time",
                ""
            )),
            Category::Updates
        );
        assert_eq!(
            of(&mail(
                "notifications@github.com",
                "All checks have passed",
                "Your pull request is ready"
            )),
            Category::Updates
        );
        assert_eq!(
            of(&mail(
                "no-reply@accounts.example",
                "Your account was updated",
                ""
            )),
            Category::Updates
        );
        // Words inside other words don't count: "wholesale", "offline".
        assert_eq!(
            of(&bulk(
                "news@example.org",
                "Wholesale prices report, offline edition",
                ""
            )),
            Category::Newsletters
        );
    }

    #[test]
    fn the_model_decides_only_where_sure_enough() {
        let prefs = Categorizing::default();
        let known = Known::default();
        let with = |m: &Mail, said: Category, sure: f32| {
            let mut c = Content::of(m);
            c.ai = Some((said, (sure * 1000.) as u16));
            decide(m, c, &prefs, &known)
        };
        let person = mail("ana@example.com", "Lunch on Friday?", "");
        let newsletter = bulk("alerts@shop.example", "Five things worth your time", "");
        // A person's mail leaves Primary only where the model is very sure.
        assert_eq!(with(&person, Category::Promotions, 0.85), Category::Primary);
        assert_eq!(
            with(&person, Category::Promotions, 0.95),
            Category::Promotions
        );
        // Between other categories it needs less; into Primary, more.
        assert_eq!(
            with(&newsletter, Category::Promotions, 0.6),
            Category::Updates
        );
        assert_eq!(
            with(&newsletter, Category::Promotions, 0.75),
            Category::Promotions
        );
        assert_eq!(
            with(&newsletter, Category::Primary, 0.75),
            Category::Updates
        );
        assert_eq!(
            with(&newsletter, Category::Primary, 0.85),
            Category::Primary
        );
        // What comes before it still decides first: a sorted sender, mail
        // replied to, the server's own sorting.
        let mut prefs = Categorizing::default();
        prefs.set_sender("alerts@shop.example", Some(Category::Social));
        let mut c = Content::of(&newsletter);
        c.ai = Some((Category::Promotions, 999));
        assert_eq!(decide(&newsletter, c, &prefs, &known), Category::Social);
        let mut answered = person.clone();
        answered.answered = true;
        assert_eq!(
            with(&answered, Category::Promotions, 0.99),
            Category::Primary
        );
        let mut gmail = newsletter.clone();
        gmail.set_server_category("updates");
        assert_eq!(with(&gmail, Category::Promotions, 0.99), Category::Updates);
        // Outlook's Other is never Primary, however sure the model is.
        let mut other = person.clone();
        other.set_server_category("other");
        assert_eq!(with(&other, Category::Primary, 0.99), Category::Updates);
        // Without the model (turned off), what it said is left out.
        let mut sorter = Sorter::new(Categorizing::default());
        let mut c = Content::of(&person);
        c.ai = Some((Category::Promotions, 990));
        sorter.keep(vec![(person.id.clone(), Content::fingerprint(&person), c)]);
        assert_eq!(sorter.category(&person), Category::Primary);
    }

    #[test]
    fn travel_finance_and_newsletters_have_their_own() {
        // Travel: booking sites and airlines, and trips' words.
        assert_eq!(
            of(&mail(
                "noreply@booking.com",
                "Your stay in Lisbon",
                "Check-in Friday"
            )),
            Category::Travel
        );
        assert_eq!(
            of(&mail(
                "no-reply@flyair.example",
                "Your boarding pass",
                "Gate closes 30 minutes before departure"
            )),
            Category::Travel
        );
        assert_eq!(
            of(&mail(
                "info@reisen.example",
                "Ihr Reiseplan",
                "Flug am Montag"
            )),
            Category::Travel
        );
        // A booking's receipt is travel's; a shop's receipt isn't.
        assert_eq!(
            of(&mail(
                "receipts@flyair.example",
                "Your receipt from FlyAir",
                "Flight to Rome"
            )),
            Category::Travel
        );
        // Offers from airlines are promotions.
        assert_eq!(
            of(&bulk(
                "deals@flyair.example",
                "Fly to Rome from $49",
                "Book your flight today"
            )),
            Category::Promotions
        );
        // Finance: banks, cards, bills and statements.
        assert_eq!(
            of(&mail(
                "alerts@chase.com",
                "Purchase over $100",
                "Your card was used"
            )),
            Category::Finance
        );
        assert_eq!(
            of(&mail("noreply@bank.example", "Your statement is ready", "")),
            Category::Finance
        );
        assert_eq!(
            of(&mail(
                "service@banco.example",
                "Transferencia recibida",
                "Has recibido una transferencia"
            )),
            Category::Finance
        );
        assert_eq!(
            of(&mail(
                "service@banco.example",
                "Tu extracto",
                "Saldo disponible"
            )),
            Category::Finance
        );
        // Words alone in a person's mail don't move it.
        assert_eq!(
            of(&mail(
                "ana@example.com",
                "Hotel and flight for the offsite",
                "Your statement is ready?"
            )),
            Category::Primary
        );
        // Common English isn't Polish or French trips' words.
        assert_eq!(
            of(&mail(
                "alerts@app.example",
                "A lot of new activity",
                "Vol. 2 of the guide"
            )),
            Category::Updates
        );
        // Newsletters: listed, and a newsletter's address, service or words.
        assert_eq!(
            of(&bulk(
                "author@substack.com",
                "On writing",
                "Thoughts on craft"
            )),
            Category::Newsletters
        );
        assert_eq!(
            of(&bulk(
                "hello@studio.example",
                "The week in design",
                "In this issue: type"
            )),
            Category::Newsletters
        );
        // Unlisted, or a notification: not one.
        assert_eq!(
            of(&mail("digest@app.example", "The week in design", "")),
            Category::Updates
        );
        assert_eq!(
            of(&bulk(
                "notifications@github.com",
                "[repo] Issue #12 opened",
                ""
            )),
            Category::Updates
        );
    }

    /// The rules read every language the other models do: offers, trips,
    /// money and newsletters, in scripts with spaces, with words attached
    /// (Arabic, Korean) and without spaces (Japanese, Thai), and stems that
    /// take endings (Russian).
    #[test]
    fn the_rules_read_every_language() {
        let cases = [
            (
                "news@shop.example",
                "Скидки до 70% – только сегодня",
                "",
                Category::Promotions,
            ),
            (
                "news@shop.example",
                "最大50%OFF 本日限り",
                "全品送料無料",
                Category::Promotions,
            ),
            (
                "news@shop.example",
                "خصومات تصل إلى 50%",
                "تسوّق الآن",
                Category::Promotions,
            ),
            (
                "news@shop.example",
                "Sonderaktion",
                "Hasta luego",
                Category::Updates,
            ),
            (
                "no-reply@air.example",
                "Το check-in για την πτήση σας άνοιξε",
                "",
                Category::Travel,
            ),
            (
                "no-reply@air.example",
                "تسجيل الوصول لرحلتك مفتوح",
                "حمّل بطاقة الصعود",
                Category::Travel,
            ),
            (
                "no-reply@air.example",
                "Odbavení na let do Prahy je otevřeno",
                "palubní vstupenka",
                Category::Travel,
            ),
            (
                "alerts@bank.example",
                "₩50,000이 입금되었습니다",
                "",
                Category::Finance,
            ),
            (
                "alerts@bank.example",
                "Выписка по счёту готова",
                "",
                Category::Finance,
            ),
            (
                "alerts@bank.example",
                "รายการเดินบัญชีของคุณพร้อมแล้ว",
                "",
                Category::Finance,
            ),
            (
                "alerts@bank.example",
                "Hesap özetiniz hazır",
                "",
                Category::Finance,
            ),
        ];
        for (from, subject, preview, want) in cases {
            assert_eq!(of(&mail(from, subject, preview)), want, "{subject}");
        }
        let letter = |subject: &str, preview: &str| bulk("hello@studio.example", subject, preview);
        assert_eq!(
            of(&letter("第12号", "今号の内容：最近読んでいるもの")),
            Category::Newsletters
        );
        assert_eq!(
            of(&letter("Wydanie 12", "W tym wydaniu: co czytam")),
            Category::Newsletters
        );
        assert_eq!(
            of(&letter("ฉบับที่ 12", "ในฉบับนี้: สิ่งที่ฉันอ่าน")),
            Category::Newsletters
        );
        // Words of one language aren't taken for another's: English "Vol."
        // isn't a flight, Italian "lento" (slow) neither, nor a pale ale a
        // sale.
        assert_eq!(
            of(&mail(
                "alerts@app.example",
                "Vol. 2 of the guide is out",
                "A pale ale tasting"
            )),
            Category::Updates
        );
        assert_eq!(
            of(&mail("alerts@app.example", "Il caricamento è lento", "")),
            Category::Updates
        );
    }

    /// Every phrase of the rules' lists is found in a text of itself: one
    /// written with what the text loses (an apostrophe, a hyphen) would
    /// never be.
    #[test]
    fn every_phrase_can_be_found() {
        for list in [
            PROMO_PHRASES,
            TRAVEL_PHRASES,
            FINANCE_PHRASES,
            NEWSLETTER_PHRASES,
        ] {
            let lost: Vec<&str> = list
                .iter()
                .copied()
                .filter(|phrase| !Words::of(phrase.trim_end_matches('*'), "").has(phrase))
                .collect();
            assert!(lost.is_empty(), "these can't be found: {lost:?}");
        }
    }

    #[test]
    fn hidden_tabs_fold_into_the_category_above() {
        let mut prefs = Categorizing::default();
        let booking = mail("noreply@booking.com", "Your stay in Lisbon", "");
        let statement = mail("noreply@bank.example", "Your statement is ready", "");
        let letter = bulk("author@substack.com", "On writing", "");
        assert_eq!(
            prefs.category(&booking, &Known::default()),
            Category::Travel
        );
        prefs.toggle_tab(Category::Travel);
        prefs.toggle_tab(Category::Finance);
        prefs.toggle_tab(Category::Newsletters);
        assert_eq!(
            prefs.category(&booking, &Known::default()),
            Category::Transactions
        );
        assert_eq!(
            prefs.category(&statement, &Known::default()),
            Category::Updates
        );
        assert_eq!(
            prefs.category(&letter, &Known::default()),
            Category::Updates
        );
        // And on up, to Primary.
        prefs.toggle_tab(Category::Updates);
        assert_eq!(
            prefs.category(&letter, &Known::default()),
            Category::Primary
        );
        assert_eq!(prefs.fold(Category::Primary), Category::Primary);
        for c in Category::ALL {
            assert!(c == Category::Primary || c.parent().is_some());
        }
    }

    #[test]
    fn gmails_broader_categories_are_refined_here() {
        let mut m = mail("noreply@booking.com", "Your stay in Lisbon", "");
        m.set_server_category("updates");
        assert_eq!(of(&m), Category::Travel);
        let mut letter = bulk("author@substack.com", "On writing", "");
        letter.set_server_category("promotions");
        assert_eq!(of(&letter), Category::Newsletters);
        // Not where Gmail's category is another kind.
        letter.set_server_category("social");
        assert_eq!(of(&letter), Category::Social);
        let mut personal = mail("noreply@booking.com", "Your stay in Lisbon", "");
        personal.set_server_category("personal");
        assert_eq!(of(&personal), Category::Primary);
    }

    #[test]
    fn senders_say_whether_they_are_automated() {
        // A sending service's subdomain.
        assert_eq!(
            of(&mail(
                "onlinebanking@ealerts.bankofamerica.com",
                "Your available balance",
                ""
            )),
            Category::Finance
        );
        assert_eq!(
            of(&mail(
                "deltaairlines@e.delta.com",
                "Last chance: double miles",
                ""
            )),
            Category::Promotions
        );
        // But people's addresses at a university's mail domain aren't.
        assert_eq!(
            of(&mail("ana@mail.university.edu", "Notes from class", "")),
            Category::Primary
        );
        assert_eq!(
            of(&mail("bo@email.arizona.edu", "Lab meeting", "")),
            Category::Primary
        );
        assert_eq!(of(&mail("cy@mail.ru", "Привет", "")), Category::Primary);
        // An order address's mail is a transaction, even unrecognized, and
        // already at a quick look.
        let etsy = mail(
            "transaction@etsy.com",
            "Your Etsy Purchase from ShopName (1234567890)",
            "",
        );
        assert_eq!(of(&etsy), Category::Transactions);
        let quick = decide(
            &etsy,
            Content::quick(&etsy),
            &Categorizing::default(),
            &Known::default(),
        );
        assert_eq!(quick, Category::Transactions);
        // Not when it's a list's: an order address's newsletter.
        assert_eq!(
            of(&bulk("orders@shop.example", "New arrivals", "")),
            Category::Promotions
        );
    }

    #[test]
    fn social_networks_are_social() {
        assert_eq!(
            of(&mail(
                "notification@facebookmail.com",
                "Ana commented on your post",
                ""
            )),
            Category::Social
        );
        assert_eq!(
            of(&bulk(
                "messages-noreply@linkedin.com",
                "You appeared in 9 searches",
                ""
            )),
            Category::Social
        );
        assert_eq!(
            of(&mail("noreply@mail.instagram.com", "New login", "")),
            Category::Social
        );
        // Not every domain ending in x.com.
        assert_eq!(of(&mail("ana@fedex.com", "Hi", "")), Category::Primary);
    }

    #[test]
    fn discussion_lists_are_forums() {
        let mut post = mail("bo@example.org", "Re: [rust-users] Lifetimes question", "");
        post.set_list_id("rust-users.lists.example.org");
        post.set_list_links(vec![ListLink {
            kind: ListLinkKind::Post,
            uri: "mailto:rust-users@lists.example.org".into(),
        }]);
        assert_eq!(of(&post), Category::Forums);
        let mut group = mail("ana@example.com", "Meeting notes", "");
        group.set_list_id("club.googlegroups.com");
        assert_eq!(of(&group), Category::Forums);
        // A known person writing through the list is still the list's.
        let known = Known {
            people: HashSet::from(["bo@example.org".to_owned()]),
            own: HashSet::new(),
        };
        assert_eq!(
            classify(&post, &Categorizing::default(), &known),
            Category::Forums
        );
        // Its tab is off at first: shown in Primary.
        assert_eq!(
            Categorizing::default().category(&post, &known),
            Category::Primary
        );
    }

    #[test]
    fn receipts_and_parcels_are_transactions_and_codes_primary() {
        assert_eq!(
            of(&mail(
                "receipts@store.example",
                "Your receipt from Example Store.",
                ""
            )),
            Category::Transactions
        );
        assert_eq!(
            of(&mail(
                "ship-confirm@amazon.com",
                "Shipped: \"Kettle\"",
                "Your package is on the way"
            )),
            Category::Transactions
        );
        // A person's mail about an order isn't a receipt.
        assert_eq!(
            of(&mail("ana@example.com", "Order #58201946 confirmed", "")),
            Category::Primary
        );
        assert_eq!(
            of(&mail(
                "no-reply@accounts.example",
                "Your verification code",
                "Your code is 482913. It expires in 10 minutes."
            )),
            Category::Primary
        );
    }

    #[test]
    fn the_servers_sorting_and_known_people() {
        let mut m = mail("ana@shop.example", "Hello", "");
        m.set_server_category("promotions");
        assert_eq!(of(&m), Category::Promotions);
        // Outlook's Other: not Primary.
        m.set_server_category("other");
        assert_eq!(of(&m), Category::Updates);
        m.set_server_category("focused");
        let mut automated = bulk("news@shop.example", "Your weekly digest", "");
        automated.set_server_category("focused");
        assert_eq!(of(&automated), Category::Primary);
        // People written to are Primary, even from an automated-looking
        // address.
        let known = Known {
            people: HashSet::from(["info@smallshop.example".to_owned()]),
            own: HashSet::from(["me@example.com".to_owned()]),
        };
        let prefs = Categorizing::default();
        assert_eq!(
            classify(
                &mail("info@smallshop.example", "Your quote", ""),
                &prefs,
                &known
            ),
            Category::Primary
        );
        assert_eq!(
            classify(&mail("me@example.com", "Note to self", ""), &prefs, &known),
            Category::Primary
        );
        assert_eq!(
            gmail_category(["INBOX", "CATEGORY_SOCIAL", "UNREAD"]),
            "social"
        );
        assert_eq!(gmail_category(["INBOX"]), "");
        assert_eq!(outlook_category("Other"), "other");
    }

    #[test]
    fn senders_go_where_they_were_sorted() {
        let mut prefs = Categorizing::default();
        let m = mail("news@mail.shop.example", "Up to 70% off", "");
        prefs.set_sender("@shop.example", Some(Category::Primary));
        assert_eq!(
            prefs.sender_choice("NEWS@mail.shop.example"),
            Some(Category::Primary)
        );
        assert_eq!(classify(&m, &prefs, &Known::default()), Category::Primary);
        prefs.set_sender("news@mail.shop.example", Some(Category::Updates));
        assert_eq!(classify(&m, &prefs, &Known::default()), Category::Updates);
        prefs.set_sender("news@mail.shop.example", None);
        prefs.set_sender("@shop.example", None);
        assert_eq!(
            classify(&m, &prefs, &Known::default()),
            Category::Promotions
        );
        // Not a domain merely ending the same way.
        prefs.set_sender("@shop.example", Some(Category::Social));
        assert_eq!(prefs.sender_choice("a@workshop.example"), None);
        assert_eq!(prefs.sender_choice("a@example"), None);
    }

    #[test]
    fn hidden_tabs_fold_into_primary() {
        let mut prefs = Categorizing::default();
        let m = bulk("news@shop.example", "Up to 70% off", "");
        assert_eq!(prefs.category(&m, &Known::default()), Category::Promotions);
        prefs.toggle_tab(Category::Promotions);
        assert_eq!(prefs.category(&m, &Known::default()), Category::Primary);
        assert!(!prefs.tabs().contains(&Category::Promotions));
        prefs.toggle_tab(Category::Primary);
        assert!(prefs.tabs().contains(&Category::Primary));
    }

    #[test]
    fn categories_are_named_in_searches() {
        assert_eq!(Category::parse("Promotions"), Some(Category::Promotions));
        assert_eq!(Category::parse("promo"), Some(Category::Promotions));
        assert_eq!(Category::parse("personal"), Some(Category::Primary));
        assert_eq!(Category::parse("purchases"), Some(Category::Transactions));
        assert_eq!(Category::parse("so"), None);
        assert_eq!(Category::parse("nothing"), None);
        for c in Category::ALL {
            assert_eq!(Category::parse(c.key()), Some(c));
        }
    }

    #[test]
    fn addresses_are_found_in_lists() {
        let found: Vec<String> = addresses_in(
            "\"Doe, Jane\" <Jane@Example.com>, bo@y.org; (Team) team@lists.example.org",
        )
        .collect();
        assert_eq!(
            found,
            ["jane@example.com", "bo@y.org", "team@lists.example.org"]
        );
        assert_eq!(addresses_in("undisclosed-recipients:;").count(), 0);
    }

    #[test]
    fn preferences_read_back_and_default() {
        let prefs = Categorizing::default();
        assert!(prefs.sorting());
        assert_eq!(
            prefs.tabs(),
            [
                Category::Primary,
                Category::Transactions,
                Category::Travel,
                Category::Finance,
                Category::Updates,
                Category::Newsletters,
                Category::Promotions,
                Category::Social
            ]
        );
        let mut changed = prefs.clone();
        changed.set_sender("A@B.example", Some(Category::Social));
        let json = serde_json::to_string(&changed).unwrap();
        assert_eq!(
            serde_json::from_str::<Categorizing>(&json).unwrap(),
            changed
        );
        // Older or partial preferences keep the defaults for what they lack.
        let partial: Categorizing = serde_json::from_str(r#"{"enabled":false}"#).unwrap();
        assert!(!partial.all_mail_first);
        assert!(!partial.enabled && partial.hidden.contains(&Category::Forums));
    }

    /// Sorting is done for every listed inbox message as the list is
    /// filtered: it must stay cheap.
    #[test]
    fn sorting_a_large_inbox_is_quick() {
        let samples = [
            bulk(
                "news@shop.example",
                "Up to 70% off ends tonight",
                "Shop the sale now",
            ),
            mail(
                "ana@example.com",
                "Lunch on Friday?",
                "Are you free around noon? We could try the new place.",
            ),
            mail(
                "notifications@github.com",
                "All checks have passed",
                "Your pull request is ready for review.",
            ),
            mail(
                "receipts@store.example",
                "Your receipt from Example Store.",
                "Total $42.10",
            ),
            mail(
                "no-reply@accounts.example",
                "Your verification code",
                "Your code is 482913.",
            ),
        ];
        let prefs = Categorizing::default();
        let known = Known::default();
        // How long sorting a message takes in one round of all the samples.
        let round = |sort: &dyn Fn(&Mail) -> Category| {
            let start = std::time::Instant::now();
            for _ in 0..40 {
                for m in &samples {
                    std::hint::black_box(sort(m));
                }
            }
            start.elapsed() / (40 * samples.len() as u32)
        };
        // A whole look, and what a list does on its thread: a quick look
        // and the decision. Rounds of each in turn, the best of each: other
        // tests running at once mustn't count against either, nor more
        // against one than the other. Unoptimized test builds on a busy
        // machine are slow, so the bounds are generous; what matters is
        // that the quick look leaves out what costs.
        let (mut whole, mut quick) = (std::time::Duration::MAX, std::time::Duration::MAX);
        for _ in 0..8 {
            whole = whole.min(round(&|m| classify(m, &prefs, &known)));
            quick = quick.min(round(&|m| decide(m, Content::quick(m), &prefs, &known)));
        }
        assert!(
            whole < std::time::Duration::from_micros(3000),
            "{whole:?} per message"
        );
        assert!(
            quick < std::time::Duration::from_micros(500),
            "{quick:?} per message, quickly"
        );
        assert!(
            quick * 2 < whole,
            "a quick look ({quick:?}) costs nearly a whole one ({whole:?})"
        );
    }

    #[test]
    fn the_sorter_looks_quickly_then_closely_and_once() {
        let receipt = mail(
            "no-reply@store.example",
            "Your receipt from Example Store.",
            "",
        );
        let person = mail("ana@example.com", "Lunch?", "");
        let listed = vec![receipt.clone(), person.clone()];
        let mut sorter = Sorter::new(Categorizing::default());
        // Not looked at: quickly, until the whole look is in.
        assert_eq!(sorter.tab(&receipt), Category::Updates);
        let wanted: Vec<Mail> = sorter.look_quickly(&listed).into_iter().cloned().collect();
        assert_eq!(wanted.len(), 2);
        sorter.keep(look_closely(&wanted, None));
        assert_eq!(sorter.tab(&receipt), Category::Transactions);
        assert_eq!(sorter.tab(&person), Category::Primary);
        assert!(sorter.look_quickly(&listed).is_empty());
        // Changed since: looked at again.
        let mut changed = receipt.clone();
        changed.subject = "Up to 70% off".into();
        assert_eq!(sorter.look_quickly([&changed]).len(), 1);
        // A long list's looks match the short one's.
        let many: Vec<Mail> = (0..1200)
            .map(|i| Mail {
                id: format!("m{i}"),
                ..listed[i % 2].clone()
            })
            .collect();
        let found = look_closely(&many, None);
        assert_eq!(found.len(), many.len());
        assert!(found.iter().all(|(id, _, c)| {
            let i: usize = id[1..].parse().unwrap();
            c.complete && (c.transaction == i.is_multiple_of(2))
        }));
        sorter.keep(found);
        sorter.forget_unlisted(&listed);
        assert!(sorter.content(&person).is_some() && sorter.contents.len() == 2);
    }

    /// Whole looks are kept in the cache and read back as they were; a
    /// deleted message's goes with it. Without the model, any whole look
    /// will do, so turning it off doesn't have everything looked at again.
    #[test]
    fn whole_looks_are_kept_and_good_without_the_model() {
        let path = std::env::temp_dir().join(format!("picacho-looks-{}", uuid::Uuid::new_v4()));
        let store = crate::manage::store::Store::open(&path).unwrap();
        let mut receipt = mail("no-reply@store.example", "Your receipt from Example Store.", "");
        receipt.id = "r1".into();
        receipt.account = "a".into();
        let mut other = receipt.clone();
        other.id = "gone".into();
        store.save_messages(&[receipt.clone(), other.clone()]).unwrap();
        let found = look_closely(&[receipt.clone(), other.clone()], None);
        store.save_looks(&found).unwrap();
        // Not a message: not kept.
        store.save_looks(&[("nowhere".into(), 1, Content::default())]).unwrap();
        store.delete_ids("a", &["gone".into()]).unwrap();
        let kept = store.looks().unwrap();
        assert_eq!(kept.len(), 1, "{kept:?}");
        let mut sorter = Sorter::new(Categorizing::default());
        sorter.keep(kept);
        assert!(!sorter.wants_close_look(&receipt));
        assert_eq!(sorter.tab_known(&receipt), Category::Transactions);
        // Not looked at yet: Primary, without looking now.
        let mut fresh = receipt.clone();
        fresh.id = "new".into();
        assert_eq!(sorter.tab_known(&fresh), Category::Primary);
        assert!(sorter.wants_close_look(&fresh));
        let _ = std::fs::remove_dir_all(&path);
    }

    /// A long list is looked at quickly on several threads, as a short one
    /// is here: every message once, in order, and what was looked at
    /// closely already isn't wanted again.
    #[test]
    fn a_long_list_is_looked_at_quickly_as_a_short_one() {
        let receipt = mail("no-reply@store.example", "Your receipt from Example Store.", "");
        let deal = mail("news@shop.example", "Up to 70% off everything", "");
        let person = mail("ana@example.com", "Lunch?", "");
        let kinds = [receipt, deal, person];
        let many: Vec<Mail> = (0..5000)
            .map(|i| Mail {
                id: format!("m{i}"),
                ..kinds[i % 3].clone()
            })
            .collect();
        // One of each, one at a time.
        let mut serial = Sorter::new(Categorizing::default());
        for m in &many[..3] {
            serial.look_quickly([m]);
        }
        let mut sorter = Sorter::new(Categorizing::default());
        // Some looked at closely before: not wanted again.
        sorter.keep(look_closely(&many[..100], None));
        let wanted = sorter.look_quickly(&many);
        assert_eq!(wanted.len(), many.len() - 100);
        assert!(wanted.iter().zip(&many[100..]).all(|(a, b)| a.id == b.id), "in order");
        assert_eq!(sorter.contents.len(), many.len());
        for (i, m) in many.iter().enumerate().skip(100) {
            let same = &many[i % 3];
            assert_eq!(sorter.content(m), serial.content(same), "{}", m.id);
            assert_eq!(sorter.tab(m), serial.tab(same), "{}", m.id);
        }
        // Still wanted closely, and not looked at quickly again.
        assert_eq!(sorter.look_quickly(&many).len(), many.len() - 100);
    }

    #[test]
    fn a_quick_look_leaves_out_only_receipts_parcels_and_codes() {
        let prefs = Categorizing::default();
        let known = Known::default();
        let receipt = mail(
            "no-reply@store.example",
            "Your receipt from Example Store.",
            "",
        );
        assert!(!Content::quick(&receipt).complete && Content::of(&receipt).complete);
        assert_eq!(
            decide(&receipt, Content::quick(&receipt), &prefs, &known),
            Category::Updates
        );
        assert_eq!(
            decide(&receipt, Content::of(&receipt), &prefs, &known),
            Category::Transactions
        );
        let promo = bulk("news@shop.example", "Up to 70% off", "");
        assert_eq!(
            decide(&promo, Content::quick(&promo), &prefs, &known),
            Category::Promotions
        );
        // What it depends on changing, it is looked at again.
        let mut changed = promo.clone();
        changed.unread = false;
        changed.starred = true;
        assert_eq!(Content::fingerprint(&changed), Content::fingerprint(&promo));
        changed.subject = "Your weekly digest".into();
        assert_ne!(Content::fingerprint(&changed), Content::fingerprint(&promo));
    }
}

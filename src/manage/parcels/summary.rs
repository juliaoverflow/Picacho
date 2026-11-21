//! Every message about a parcel gathered into one entry with its history,
//! as a Summary lists them.
//!
//! Messages are gathered, in order of trust:
//!
//! 1. by tracking number;
//! 2. a message with no tracking number, by its orders: with the parcel of
//!    that order when the order has one, with each of them when it has
//!    several, or else with the order's other messages that say the same is
//!    in the shipment ("Shipped: 1 Luggage item", "Delivered: 1 Luggage
//!    item"), as one order may ship in parts;
//! 3. a parcel not yet delivered with a later one from the same account
//!    listing the same product within a few weeks (a shop's "order
//!    shipped" and its "package delivered", which share no number); where
//!    it could continue several, the one expected that day, or else the
//!    oldest;
//! 4. a carrier's parcel without an order with a shop's delivery known only
//!    by its order, when each is the only one that fits the other's dates.
use super::{Parcel, Source, Status};
use crate::manage::model::Mailbox;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// The stages of the way a Summary shows for each parcel, in order.
pub const STAGES: [&str; 5] = [
    "Ordered",
    "Shipped",
    "In transit",
    "Out for delivery",
    "Delivered",
];

/// How long after a parcel's last news another message about the same
/// product may continue it.
const SAME_PRODUCT: i64 = 30 * 86_400;
/// How long after a carrier's last news a shop's delivery may be its end.
const CARRIER_TO_SHOP: i64 = 10 * 86_400;
/// How long after the day it was expected (or out for delivery) a parcel
/// with no more news is taken to have been delivered.
const ASSUME_AFTER: i64 = 3 * 86_400;
/// How long a parcel on its way (shipped or further) may go without news
/// before it is taken to have been delivered: carriers such as USPS and
/// FedEx often send no email when they deliver, and even a parcel from
/// abroad is rarely on the way this long without one.
const QUIET_AFTER: i64 = 21 * 86_400;
/// How near the day a parcel was expected another message's news must be
/// to be taken as its arrival, when it could continue several parcels.
const EXPECTED_NEAR: i64 = 86_400;

/// A message a parcel was found in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sighting {
    pub account: String,
    pub mail_id: String,
    /// Its Message-ID, which its copies under other labels share.
    pub message_id: String,
    pub mailbox: Mailbox,
    pub subject: String,
    pub sender: String,
    /// When it was sent (Unix seconds).
    pub sent: i64,
}

/// A step of a parcel's history: a message about it, and what it said.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Event {
    pub sighting: Sighting,
    pub status: Option<Status>,
    /// When it happened: the update time the message gives, or when it was
    /// sent.
    pub at: i64,
}

/// A parcel as an account's (or every account's) mail tells of it.
#[derive(Clone, Debug)]
pub struct Tracked {
    /// Its tracking number (or order) and what the messages say of it, the
    /// newest first.
    pub parcel: Parcel,
    /// Its last update (Unix seconds).
    pub updated: i64,
    /// Every message about it, newest first, each once; none after the
    /// first saying it was delivered.
    pub history: Vec<Event>,
    /// How far along `STAGES` it has got.
    pub stage: usize,
    /// The newest message says it is delayed, was returned or cancelled.
    pub trouble: Option<Status>,
    /// No message says it was delivered, but the day it was expected (or
    /// was out for delivery) is days past with no news since.
    pub assumed: bool,
    /// What it is known by, for statuses set by hand (`Override`): its
    /// tracking numbers, and its orders as "account:order:number".
    pub ids: Vec<String>,
    /// The status set by hand, in place of what its emails say.
    pub manual: Option<Status>,
    /// Hidden from the Summary by hand.
    pub hidden: bool,
    /// The name given it by hand ("Graphics card"), shown above its order
    /// or tracking number.
    pub name: Option<String>,
    /// Who it is from, as set by hand, in place of its emails' sender.
    pub sender: Option<String>,
    /// Its stage, trouble and assumption as its emails have them, for when
    /// a status set by hand is taken back.
    auto: (usize, Option<Status>, bool),
}

/// A status set by hand on a parcel, or its hiding, as saved: it holds
/// while new emails about the parcel arrive, until it is taken back.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Override {
    /// The parcel's `Tracked::ids` when it was set: it is found again by
    /// any of them, as a parcel's number or orders join it later.
    pub ids: Vec<String>,
    #[serde(default)]
    pub status: Option<Status>,
    #[serde(default)]
    pub hidden: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sender: Option<String>,
    /// When it was set (Unix seconds).
    #[serde(default)]
    pub set: i64,
}

impl Override {
    /// Whether it was set on `t`.
    pub fn fits(&self, t: &Tracked) -> bool {
        self.ids.iter().any(|id| t.ids.contains(id))
    }
    /// Whether it sets anything: else it needn't be kept.
    pub fn sets_anything(&self) -> bool {
        self.status.is_some() || self.hidden || self.name.is_some() || self.sender.is_some()
    }
}

/// Sets each parcel's status by hand and hides it as `overrides` say, or
/// gives it back what its emails say.
pub fn apply_overrides(tracked: &mut [Tracked], overrides: &[Override]) {
    for t in tracked {
        let o = overrides.iter().rev().find(|o| o.fits(t));
        t.set_manual(o.and_then(|o| o.status), o.is_some_and(|o| o.hidden));
        t.name = o.and_then(|o| o.name.clone());
        t.sender = o.and_then(|o| o.sender.clone());
    }
}

impl Tracked {
    /// Sets its status by hand (or, with None, back to its emails'), and
    /// whether it is hidden.
    pub fn set_manual(&mut self, status: Option<Status>, hidden: bool) {
        (self.stage, self.trouble, self.assumed) = self.auto;
        self.manual = status;
        self.hidden = hidden;
        match status.map(|s| (s, s.stage())) {
            Some((_, Some(stage))) => {
                self.stage = stage;
                self.trouble = None;
                self.assumed = false;
            }
            Some((s, None)) => {
                self.trouble = Some(s);
                self.assumed = false;
            }
            None => {}
        }
    }
    /// Who it is from, as the Summary says: as set by hand, or the newest
    /// email's sender.
    pub fn from(&self) -> &str {
        self.sender.as_deref().unwrap_or(&self.latest().sender)
    }
    /// The message with the last update.
    pub fn latest(&self) -> &Sighting {
        &self.history[0].sighting
    }
    /// Where it is, as the Summary says it: its trouble, or its stage, as
    /// the newest email at that stage names it ("Ready for pickup" rather
    /// than "Out for delivery").
    pub fn status_label(&self) -> &'static str {
        if let Some(s) = self.manual {
            return s.label();
        }
        if let Some(s) = self.trouble {
            return s.label();
        }
        if self.assumed {
            return "Probably delivered";
        }
        self.history
            .iter()
            .filter_map(|e| e.status)
            .find(|s| s.stage() == Some(self.stage))
            .map_or(STAGES[self.stage], Status::label)
    }
    /// An email says it was delivered.
    pub fn delivered(&self) -> bool {
        self.stage == STAGES.len() - 1
    }
    /// Delivered, or taken to have been.
    pub fn done(&self) -> bool {
        self.delivered() || self.assumed
    }
    /// Nothing more is coming: delivered (or taken to have been),
    /// returned or cancelled.
    pub fn finished(&self) -> bool {
        self.done() || matches!(self.trouble, Some(Status::Returned | Status::Cancelled))
    }
}

/// Sets of sightings joined into one parcel.
struct Joins(Vec<usize>);

impl Joins {
    fn root(&mut self, i: usize) -> usize {
        let mut r = i;
        while self.0[r] != r {
            r = self.0[r];
        }
        let mut i = i;
        while self.0[i] != r {
            let next = self.0[i];
            self.0[i] = r;
            i = next;
        }
        r
    }
    fn join(&mut self, a: usize, b: usize) {
        let (a, b) = (self.root(a), self.root(b));
        if a != b {
            self.0[b.max(a)] = a.min(b);
        }
    }
}

/// What a group of sightings is, for joining groups.
struct Group {
    members: Vec<usize>,
    account: String,
    first: i64,
    last: i64,
    /// When the first message saying it was delivered was sent.
    delivered: Option<i64>,
    numbers: Vec<String>,
    /// The furthest stage its messages reach.
    stage: Option<usize>,
    has_order: bool,
    /// Every order its messages name.
    orders: Vec<String>,
    products: Vec<String>,
    /// The latest day it was expected by (Unix seconds, the end of that
    /// day), as its messages say.
    expected: Option<i64>,
}

/// Every parcel the sightings tell of, once each, last updated first.
pub fn summarize(found: Vec<(Parcel, Sighting)>) -> Vec<Tracked> {
    summarize_at(found, chrono::Utc::now().timestamp())
}

/// The same, as of `now` (Unix seconds).
pub fn summarize_at(found: Vec<(Parcel, Sighting)>, now: i64) -> Vec<Tracked> {
    let at = |(p, s): &(Parcel, Sighting)| p.updated.unwrap_or(s.sent);
    let orders_of = |p: &Parcel| -> Vec<String> {
        let mut o = p.orders.clone();
        if let Some(own) = &p.order
            && !o.contains(own)
        {
            o.insert(0, own.clone());
        }
        o
    };
    let mut joins = Joins((0..found.len()).collect());
    // 1. By tracking number; and the tracking numbers of each order.
    let mut by_number: HashMap<&str, usize> = HashMap::new();
    let mut numbers_of_order: HashMap<(String, String), Vec<String>> = HashMap::new();
    for (i, (p, s)) in found.iter().enumerate() {
        if p.tracking_number.is_empty() {
            continue;
        }
        match by_number.get(p.tracking_number.as_str()) {
            Some(&j) => joins.join(i, j),
            None => {
                by_number.insert(&p.tracking_number, i);
            }
        }
        for order in orders_of(p) {
            let numbers = numbers_of_order
                .entry((s.account.clone(), order))
                .or_default();
            if !numbers.contains(&p.tracking_number) {
                numbers.push(p.tracking_number.clone());
            }
        }
    }
    // 2. Messages without a tracking number, by their orders.
    // Sightings that go into several parcels: (sighting, parcels' members).
    let mut copies: Vec<(usize, Vec<usize>)> = Vec::new();
    // Order-only messages that say what is in the shipment, or none.
    let mut by_contents: HashMap<(String, String, Option<String>), usize> = HashMap::new();
    let mut later: Vec<usize> = Vec::new();
    for (i, (p, s)) in found.iter().enumerate() {
        if !p.tracking_number.is_empty() {
            continue;
        }
        let orders = orders_of(p);
        let mut numbers: Vec<&String> = Vec::new();
        for order in &orders {
            if let Some(list) = numbers_of_order.get(&(s.account.clone(), order.clone())) {
                for n in list {
                    if !numbers.contains(&n) {
                        numbers.push(n);
                    }
                }
            }
        }
        match numbers.as_slice() {
            [] => {}
            [only] => {
                joins.join(i, by_number[only.as_str()]);
                continue;
            }
            // An order shipped as several parcels: what is said of the
            // order is said of each.
            several => {
                copies.push((i, several.iter().map(|n| by_number[n.as_str()]).collect()));
                continue;
            }
        }
        if p.status == Some(Status::Processing) || p.contents.is_none() {
            later.push(i);
            continue;
        }
        for order in &orders {
            let key = (s.account.clone(), order.clone(), p.contents.clone());
            match by_contents.get(&key) {
                Some(&j) => joins.join(i, j),
                None => {
                    by_contents.insert(key, i);
                }
            }
        }
    }
    // An order's confirmation, and its messages that don't say what is in
    // the shipment: with its one shipment, or each of several; or alone.
    for &i in &later {
        let (p, s) = &found[i];
        let mut shipments: Vec<usize> = Vec::new();
        for order in orders_of(p) {
            for ((account, o, contents), &j) in &by_contents {
                if *account == s.account && *o == order && contents.is_some() {
                    let r = joins.root(j);
                    if !shipments.iter().any(|&k| joins.root(k) == r) {
                        shipments.push(j);
                    }
                }
            }
        }
        match shipments.as_slice() {
            [] => {
                // With the order's other such messages.
                for order in orders_of(p) {
                    let key = (s.account.clone(), order, None);
                    match by_contents.get(&key) {
                        Some(&j) => joins.join(i, j),
                        None => {
                            by_contents.insert(key, i);
                        }
                    }
                }
            }
            [one] => joins.join(i, *one),
            several => copies.push((i, several.to_vec())),
        }
    }
    let copied: Vec<usize> = copies.iter().map(|(i, _)| *i).collect();
    let mut members: HashMap<usize, Vec<usize>> = HashMap::new();
    for i in 0..found.len() {
        if !copied.contains(&i) {
            members.entry(joins.root(i)).or_default().push(i);
        }
    }
    for (i, targets) in copies {
        let mut roots: Vec<usize> = targets.iter().map(|&t| joins.root(t)).collect();
        roots.sort_unstable();
        roots.dedup();
        // A message naming what is in one of the shipments ("Your return of
        // GIGABYTE B850…") is about that one alone; not an order's
        // confirmation, nor one naming the first of several ("… and 3 more
        // items").
        let subject = found[i].1.subject.to_lowercase();
        let whole_order = found[i].0.status == Some(Status::Processing)
            || subject.contains(" more item");
        let named: Vec<usize> = roots
            .iter()
            .copied()
            .filter(|r| {
                members.get(r).is_some_and(|m| {
                    m.iter().any(|&j| names(&found[j].0).iter().any(|n| subject.contains(n.as_str())))
                })
            })
            .collect();
        if !whole_order
            && let [one] = named.as_slice()
        {
            roots = vec![*one];
        }
        for r in roots {
            members.entry(r).or_default().push(i);
        }
    }
    let mut groups: Vec<Group> = members
        .into_values()
        .map(|members| describe(&found, members, at))
        .collect();
    // In a fixed order, the oldest first: what joins what doesn't change
    // from one look to the next.
    groups.sort_by_key(|g| (g.first, g.members.iter().min().copied()));
    // 3. The same product, continued by a later message: the oldest
    // message first, with the parcel it fits that was expected the day it
    // came, or else the oldest it fits (first ordered, first delivered).
    loop {
        let mut pair = None;
        for b in 0..groups.len() {
            let fits: Vec<usize> = (0..groups.len())
                .filter(|&a| a != b && continues(&groups[a], &groups[b]))
                .collect();
            let came = groups[b].delivered.unwrap_or(groups[b].first);
            let on_the_day = fits.iter().copied().find(|&a| {
                groups[a]
                    .expected
                    .is_some_and(|e| came >= e - 86_400 - EXPECTED_NEAR && came <= e + EXPECTED_NEAR)
            });
            if let Some(a) = on_the_day.or(fits.first().copied()) {
                pair = Some((a, b));
                break;
            }
        }
        let Some((a, b)) = pair else {
            break;
        };
        // Removed in place, so the groups stay oldest first.
        let taken = groups.remove(b);
        let a = if b < a { a - 1 } else { a };
        let mut all = std::mem::take(&mut groups[a].members);
        all.extend(taken.members);
        groups[a] = describe(&found, all, at);
    }
    // 4. A carrier's parcel, and a shop's delivery of an order, that only
    // fit each other.
    let fits = |t: &Group, o: &Group| {
        t.account == o.account
            && !t.numbers.is_empty()
            && !t.has_order
            && t.delivered.is_none()
            && o.numbers.is_empty()
            && o.delivered.is_some_and(|d| {
                d >= t.last - 86_400 && d <= t.last + CARRIER_TO_SHOP
            })
    };
    let mut pairs: Vec<(usize, usize)> = Vec::new();
    for t in 0..groups.len() {
        let mut fit = (0..groups.len()).filter(|&o| fits(&groups[t], &groups[o]));
        if let (Some(o), None) = (fit.next(), fit.next())
            && (0..groups.len())
                .filter(|&u| fits(&groups[u], &groups[o]))
                .count()
                == 1
        {
            pairs.push((t, o));
        }
    }
    let mut gone: Vec<usize> = Vec::new();
    for (t, o) in pairs {
        let taken = std::mem::take(&mut groups[o].members);
        groups[t].members.extend(taken);
        gone.push(o);
    }
    let mut out: Vec<Tracked> = groups
        .into_iter()
        .enumerate()
        .filter(|(i, g)| !gone.contains(i) && !g.members.is_empty())
        .filter_map(|(_, g)| {
            gather(
                g.members.iter().map(|&i| found[i].clone()).collect(),
                now,
            )
        })
        .collect();
    out.sort_by(|a, b| {
        b.updated
            .cmp(&a.updated)
            .then_with(|| a.parcel.key().cmp(&b.parcel.key()))
    });
    out
}

/// What a group of sightings is.
fn describe(
    found: &[(Parcel, Sighting)],
    members: Vec<usize>,
    at: impl Fn(&(Parcel, Sighting)) -> i64,
) -> Group {
    let mut g = Group {
        account: found[members[0]].1.account.clone(),
        first: i64::MAX,
        last: i64::MIN,
        delivered: None,
        numbers: Vec::new(),
        stage: None,
        has_order: false,
        orders: Vec::new(),
        products: Vec::new(),
        expected: None,
        members: Vec::new(),
    };
    for &i in &members {
        let x = &found[i];
        let (p, _) = x;
        let t = at(x);
        g.first = g.first.min(t);
        g.last = g.last.max(t);
        if p.status == Some(Status::Delivered) {
            g.delivered = Some(g.delivered.map_or(t, |d| d.min(t)));
        }
        if !p.tracking_number.is_empty() && !g.numbers.contains(&p.tracking_number) {
            g.numbers.push(p.tracking_number.clone());
        }
        g.has_order |= p.order.is_some();
        for o in p.order.iter().chain(&p.orders) {
            if !g.orders.contains(o) {
                g.orders.push(o.clone());
            }
        }
        g.stage = g.stage.max(p.status.and_then(Status::stage));
        g.expected = g.expected.max(expected_by(p));
        let specific = p.contents.iter().filter(|c| specific(c));
        for name in p.products.iter().chain(specific) {
            if !g.products.contains(name) {
                g.products.push(name.clone());
            }
        }
    }
    g.members = members;
    g
}

/// The end of the day a parcel is expected by, as a message says.
fn expected_by(p: &Parcel) -> Option<i64> {
    let day = p.expected_arrival.as_deref()?.get(..10)?;
    let day = chrono::NaiveDate::parse_from_str(day, "%Y-%m-%d").ok()?;
    Some(day.and_hms_opt(23, 59, 0)?.and_utc().timestamp())
}

/// What a message says is in its shipment (its products and contents,
/// already lowercase and without quotes or a cut name's "..."), where
/// long enough to tell one shipment from another.
fn names(p: &Parcel) -> Vec<&String> {
    p.products
        .iter()
        .chain(&p.contents)
        .filter(|n| n.chars().filter(|c| c.is_alphanumeric()).count() >= 12 && n.contains(' '))
        .collect()
}

/// Whether what a subject says is in a shipment names it well enough to
/// find it again: "2 items: video games, light bulbs", not "1 electronics
/// item".
fn specific(contents: &str) -> bool {
    let generic = contents
        .trim_start_matches(|c: char| c.is_ascii_digit() || c == ' ')
        .split([' ', ','])
        .filter(|w| !w.is_empty())
        .count()
        <= 3
        && (contents.ends_with(" item") || contents.ends_with(" items"));
    !generic && contents.chars().filter(|c| c.is_alphabetic()).count() >= 8
}

/// Whether `b` goes on where `a` left off: the same account and product,
/// `a` not delivered, `b`'s news going on past `a`'s, begun within weeks
/// of `a`'s last news,
/// and further along (two orders of the same thing both "shipped" are two
/// parcels). Its tracking number may differ: a parcel may change carrier
/// on the way ("collected by the carrier" AP…, "delivered" UUSC…).
fn continues(a: &Group, b: &Group) -> bool {
    a.account == b.account
        && a.delivered.is_none()
        && b.last >= a.last
        && b.first <= a.last + SAME_PRODUCT
        && b.stage > a.stage
        // Two orders are two parcels, even of the same product.
        && (a.orders.is_empty()
            || b.orders.is_empty()
            || a.orders.iter().any(|o| b.orders.contains(o)))
        && a
            .products
            .iter()
            .any(|p| b.products.iter().any(|q| same_product(p, q)))
}

/// Whether two shortened names are of one product: one begins the other
/// (shops cut a name at different lengths: "hard shell travel case for mous",
/// "hard shell travel case for mouse m7/m"), and the shorter says enough.
fn same_product(a: &str, b: &str) -> bool {
    let (short, long) = if a.len() <= b.len() { (a, b) } else { (b, a) };
    long.starts_with(short) && short.chars().count() >= 16 && short.contains(' ')
}

/// One parcel's messages as one entry.
fn gather(mut sightings: Vec<(Parcel, Sighting)>, now: i64) -> Option<Tracked> {
    let at = |(p, s): &(Parcel, Sighting)| p.updated.unwrap_or(s.sent);
    sightings.sort_by_key(|x| std::cmp::Reverse((at(x), x.1.sent)));
    // Copies of one message (Gmail's labels, a message in two folders)
    // count once.
    let mut seen: Vec<String> = Vec::new();
    sightings.retain(|(_, s)| {
        let identity = if s.message_id.is_empty() {
            format!("{}\u{0}{}", s.subject, s.sent)
        } else {
            s.message_id.clone()
        };
        if seen.contains(&identity) {
            false
        } else {
            seen.push(identity);
            true
        }
    });
    // Ready for pickup, then "picked up": by its buyer, so delivered.
    let mut ready = false;
    for (p, _) in sightings.iter_mut().rev() {
        match p.status {
            Some(Status::PickupAvailable) => ready = true,
            Some(Status::Shipped) if ready => p.status = Some(Status::Delivered),
            _ => {}
        }
    }
    // Only the first email saying it was delivered counts: later ones
    // (another "Delivered", a review request, a reminder) are left out.
    if let Some(first_delivered) = sightings
        .iter()
        .rposition(|(p, _)| p.status == Some(Status::Delivered))
    {
        sightings.drain(..first_delivered);
    }
    let updated = at(sightings.first()?);
    // The tracking number's own messages first: an order-only delivery
    // that joined it has none.
    let numbered = sightings
        .iter()
        .position(|(p, _)| !p.tracking_number.is_empty())
        .unwrap_or(0);
    let mut merged = sightings[numbered].0.clone();
    for (p, _) in &sightings {
        merged.order = merged.order.or_else(|| p.order.clone());
        merged.carrier = merged.carrier.or_else(|| p.carrier.clone());
        merged.service = merged.service.or_else(|| p.service.clone());
        merged.expected_arrival = merged
            .expected_arrival
            .or_else(|| p.expected_arrival.clone());
        if matches!(p.source, Source::Markup | Source::Link)
            && !matches!(merged.source, Source::Markup | Source::Link)
        {
            merged.source = p.source;
            merged.tracking_url = p.tracking_url.clone().or(merged.tracking_url);
        }
        merged.tracking_url = merged.tracking_url.or_else(|| p.tracking_url.clone());
    }
    let history: Vec<Event> = sightings
        .iter()
        .map(|(p, s)| Event {
            sighting: s.clone(),
            status: p.status,
            at: p.updated.unwrap_or(s.sent),
        })
        .collect();
    // The furthest stage any message reached: a later reminder that it
    // shipped doesn't take a delivered parcel back. With a tracking number
    // it has at least shipped.
    let reached = history.iter().filter_map(|e| e.status?.stage()).max();
    let floor = usize::from(!merged.tracking_number.is_empty());
    let stage = reached.unwrap_or(0).max(floor);
    let newest = history.iter().find_map(|e| e.status);
    merged.status = newest;
    merged.updated = Some(updated);
    let trouble = newest.filter(|s| s.stage().is_none() && stage < STAGES.len() - 1);
    let expected = expected_by(&merged);
    let mut ids: Vec<String> = Vec::new();
    for (p, s) in &sightings {
        let numbers = (!p.tracking_number.is_empty()).then(|| p.tracking_number.clone());
        let orders = p
            .order
            .iter()
            .chain(&p.orders)
            .map(|o| format!("{}:order:{o}", s.account));
        for id in numbers.into_iter().chain(orders) {
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
    }
    let out_for_delivery = stage == 3 && newest.and_then(Status::stage) == Some(3);
    let assumed = stage < STAGES.len() - 1
        && trouble.is_none()
        && stage >= 1
        && (expected.is_some_and(|e| e.max(updated) + ASSUME_AFTER < now)
            || (out_for_delivery && updated + ASSUME_AFTER < now)
            || updated + QUIET_AFTER < now);
    Some(Tracked {
        parcel: merged,
        updated,
        history,
        stage,
        trouble,
        assumed,
        ids,
        manual: None,
        hidden: false,
        name: None,
        sender: None,
        auto: (stage, trouble, assumed),
    })
}

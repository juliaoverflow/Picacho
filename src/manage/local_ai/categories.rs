//! Sift Categories, the inbox category model: it reads what the message
//! list holds of a message (the sender's name and address, the subject, the
//! preview and the list headers) and scores the inbox's categories
//! (`manage::categories::Category`), in `models/categories/classes.json`'s
//! order. `manage::categories` asks it where its rules are unsure.
//!
//! Features are built exactly as `models/categories/features.py` builds
//! them for training; `models/categories/parity.json` checks the two
//! agree.
//!
//! On top of it, `Learned` is what this computer learned from the messages
//! the person moved to another category (Move to Category…, or dragged onto
//! a tab): a small linear layer over the same features, added to the
//! model's scores, so mail like what they moved goes where they put it,
//! whoever sends it. It stays on this device, and can be forgotten.
use super::otp::{vector_in, words};
use super::replies::concepts_in;
use super::runtime::NativeTagger;
use crate::manage::categories::Category;
use crate::manage::list_headers::ListLinkKind;
use crate::manage::model::Mail;
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::{Arc, LazyLock};

/// The hashed feature space (a power of two).
pub const DIM: usize = 1 << 15;
const SUBJECT_WORDS: usize = 16;
const PREVIEW_CHARS: usize = 400;
const PREVIEW_WORDS: usize = 60;
const PREVIEW_PAIRS: usize = 30;
const NAME_WORDS: usize = 4;
const LOCAL_TOKENS: usize = 4;
const PROMO_EMOJI: &str = "🛍🎁💸💰🏷🤑🛒🔥⚡🎉⏰💥";
const CURRENCY: &str = "$€£¥₹₽₩₺₫฿₪₴৳₱";
const REPLY_PREFIXES: [&str; 11] = [
    "re:", "aw:", "sv:", "fw:", "fwd:", "wg:", "tr:", "rv:", "odp:", "回复:", "答复:",
];
/// How many classes the model scores: one per category.
pub const CLASSES: usize = Category::ALL.len();

static LEXICON: LazyLock<Vec<(String, Vec<String>)>> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../../../models/categories/lexicon.json")).expect("lexicon")
});

#[derive(Deserialize)]
struct Senders {
    services: Vec<String>,
    automated: Vec<String>,
}
static SENDERS: LazyLock<Senders> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../../../models/categories/senders.json")).expect("senders")
});

/// The model's outputs, as categories.
static OUTPUTS: LazyLock<Vec<Category>> = LazyLock::new(|| {
    let names: Vec<String> =
        serde_json::from_str(include_str!("../../../models/categories/classes.json"))
            .expect("classes");
    names
        .iter()
        .map(|n| Category::parse(n).expect("a category"))
        .collect()
});

/// What the model reads of a message.
#[derive(Clone, Copy, Debug, Default)]
pub struct Input<'a> {
    pub name: &'a str,
    pub address: &'a str,
    pub subject: &'a str,
    pub preview: &'a str,
    pub unsubscribe: &'a str,
    pub list_id: &'a str,
    pub list_post: bool,
}

impl<'a> Input<'a> {
    pub fn of(m: &'a Mail) -> Self {
        Self {
            name: &m.sender,
            address: &m.from,
            subject: &m.subject,
            preview: &m.snippet,
            unsubscribe: &m.list_unsubscribe,
            list_id: m.list_id(),
            list_post: m.list_links().iter().any(|l| l.kind == ListLinkKind::Post),
        }
    }
}

/// Runs of ASCII letters and digits, without trailing digits, leaving out
/// those that were only digits.
fn tokens(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut run = String::new();
    for c in text.chars().chain(std::iter::once(' ')) {
        if c.is_ascii_alphanumeric() {
            run.push(c);
        } else {
            let t = run.trim_end_matches(|c: char| c.is_ascii_digit());
            if !t.is_empty() {
                out.push(t.to_owned());
            }
            run.clear();
        }
    }
    out
}

/// A currency sign beside a digit.
fn priced(text: &[char]) -> bool {
    text.iter().enumerate().any(|(i, c)| {
        CURRENCY.contains(*c)
            && ((i > 0 && text[i - 1].is_ascii_digit())
                || text.get(i + 1).is_some_and(char::is_ascii_digit))
    })
}

fn percent(text: &[char]) -> bool {
    text.iter()
        .enumerate()
        .any(|(i, c)| (*c == '%' || *c == '％') && i > 0 && text[i - 1].is_ascii_digit())
}

/// A name like a person's: two or three words, each starting with an
/// uppercase letter, without digits or an "@".
fn person(name: &str) -> bool {
    let parts: Vec<&str> = name.split_whitespace().collect();
    (2..=3).contains(&parts.len())
        && !name.contains('@')
        && !name.chars().any(|c| c.is_ascii_digit())
        && parts
            .iter()
            .all(|p| p.chars().next().is_some_and(char::is_uppercase))
}

/// The feature strings of a message.
pub(crate) fn features(i: &Input) -> Vec<String> {
    let preview: String = i.preview.chars().take(PREVIEW_CHARS).collect();
    let name = i.name.trim();
    let take = |mut v: Vec<String>, n: usize| {
        v.truncate(n);
        v
    };
    let sw = take(words(i.subject, true), SUBJECT_WORDS);
    let pw = take(words(&preview, true), PREVIEW_WORDS);
    let nw = take(words(name, true), NAME_WORDS);
    let address = i.address.trim().to_lowercase();
    let (local, domain) = match address.rsplit_once('@') {
        Some((l, d)) => (l.to_owned(), d.to_owned()),
        None => (address.clone(), String::new()),
    };
    let lt = take(tokens(&local), LOCAL_TOKENS);
    let mut f: Vec<String> = sw.iter().map(|w| format!("S:{w}")).collect();
    f.extend(sw.windows(2).map(|p| format!("SB:{}_{}", p[0], p[1])));
    f.extend(pw.iter().map(|w| format!("P:{w}")));
    let pp = &pw[..pw.len().min(PREVIEW_PAIRS)];
    f.extend(pp.windows(2).map(|p| format!("PB:{}_{}", p[0], p[1])));
    f.extend(nw.iter().map(|w| format!("N:{w}")));
    f.extend(lt.iter().map(|t| format!("L:{t}")));
    if let Some(first) = lt.first() {
        f.push(format!("LW:{first}"));
    }
    let labels: Vec<&str> = domain.split('.').filter(|l| !l.is_empty()).collect();
    if labels.len() >= 2 {
        let (name_label, tld) = (labels[labels.len() - 2], labels[labels.len() - 1]);
        f.push(format!("D:{name_label}"));
        f.push(format!("DT:{tld}"));
        f.push(format!("R:{name_label}.{tld}"));
        for sub in &labels[..labels.len() - 2] {
            if let Some(t) = tokens(sub).into_iter().next() {
                f.push(format!("DS:{t}"));
            }
        }
        f.push(format!("sub:{}", (labels.len() - 2).min(2)));
    }
    let text = format!("{}\n{preview}", i.subject);
    f.extend(
        concepts_in(&LEXICON, &words(&text, false))
            .into_iter()
            .map(|c| format!("C:{c}")),
    );
    f.extend(
        concepts_in(&LEXICON, &words(i.subject, false))
            .into_iter()
            .map(|c| format!("CS:{c}")),
    );
    let unsub = i.unsubscribe.to_lowercase();
    if !i.unsubscribe.trim().is_empty() {
        f.push("unsub".into());
        if unsub.contains("mailto:") {
            f.push("unsub:mailto".into());
        }
        if unsub.contains("http") {
            f.push("unsub:http".into());
        }
    }
    if SENDERS
        .services
        .iter()
        .any(|s| unsub.contains(s.as_str()) || domain.contains(s.as_str()))
    {
        f.push("esp".into());
    }
    if !i.list_id.is_empty() {
        f.push("listid".into());
        let lower = i.list_id.to_lowercase();
        let parts: Vec<&str> = lower.split('.').collect();
        for l in &parts[..parts.len() - 1] {
            if let Some(t) = tokens(l).into_iter().next() {
                f.push(format!("LI:{t}"));
            }
        }
    }
    if i.list_post {
        f.push("listpost".into());
    }
    let squeezed: String = local.chars().filter(char::is_ascii_alphanumeric).collect();
    if SENDERS
        .automated
        .iter()
        .any(|a| squeezed.contains(a.as_str()))
    {
        f.push("auto".into());
    }
    let chars: Vec<char> = text.chars().collect();
    if percent(&chars) {
        f.push("pct".into());
    }
    if priced(&chars) {
        f.push("money".into());
    }
    if PROMO_EMOJI.chars().any(|e| i.subject.contains(e)) {
        f.push("emoji".into());
    }
    let s = i.subject.trim().to_lowercase();
    if REPLY_PREFIXES.iter().any(|p| s.starts_with(p)) {
        f.push("re".into());
    }
    if s.starts_with('[') && s.contains(']') {
        f.push("tag".into());
    }
    if i.subject
        .split_whitespace()
        .any(|w| w.chars().count() >= 3 && w.chars().all(|c| c.is_ascii_uppercase()))
    {
        f.push("caps".into());
    }
    let digits = i.subject.chars().filter(char::is_ascii_digit).count();
    f.push(format!("digits:{}", digits.min(9) / 3));
    let brand = if labels.len() >= 2 {
        labels[labels.len() - 2].replace('-', "")
    } else {
        String::new()
    };
    let squashed: String = name.to_lowercase().split_whitespace().collect();
    if brand.chars().count() >= 3
        && !squashed.is_empty()
        && (squashed.contains(&brand) || brand.contains(&squashed))
    {
        f.push("brand".into());
    }
    if person(name) {
        f.push("person".into());
    }
    let lower_name = name.to_lowercase();
    if name.is_empty() || lower_name == address || lower_name == local {
        f.push("noname".into());
    }
    f.push(format!("slen:{}", (sw.len() / 4).min(4)));
    f
}

/// The hashed, normalized features of a message.
pub(crate) fn vector(features: &[String]) -> Vec<(usize, f32)> {
    vector_in(features, DIM)
}

/// The loaded model.
pub struct Model {
    tagger: NativeTagger,
}

impl Model {
    pub fn new(tagger: NativeTagger) -> Result<Self> {
        ensure!(
            tagger.classes() == CLASSES && OUTPUTS.len() == CLASSES,
            t!(
                "manage-local-ai-categories-the-model-has-outputs-but",
                classes = tagger.classes(),
                count = CLASSES
            )
        );
        Ok(Self { tagger })
    }
    /// The model's probability of each category (`classes.json`'s order)
    /// for features `x`.
    fn probabilities(&self, x: &[(usize, f32)]) -> Vec<f32> {
        self.tagger.probabilities(x)
    }
}

/// What this computer learned from the mail the person moved: a weight per
/// category for each feature seen in it, and a bias, added to the model's
/// log-probabilities. Kept small: past `MAX_FEATURES` the weakest go.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Learned {
    weights: BTreeMap<u32, [f32; CLASSES]>,
    bias: [f32; CLASSES],
    /// How many messages it learned from.
    pub lessons: u32,
}

/// The most features `Learned` keeps.
const MAX_FEATURES: usize = 6000;
/// How far each lesson moves the weights, how many passes it takes, and
/// the largest a weight may grow.
const RATE: f32 = 0.6;
const PASSES: usize = 4;
const LIMIT: f32 = 4.;

impl Learned {
    pub fn is_empty(&self) -> bool {
        self.lessons == 0
    }
    /// What it adds to each category's log-probability for features `x`.
    fn adjust(&self, x: &[(usize, f32)]) -> [f32; CLASSES] {
        let mut out = self.bias;
        if self.weights.is_empty() {
            return out;
        }
        for &(i, v) in x {
            if let Some(w) = self.weights.get(&(i as u32)) {
                for (o, w) in out.iter_mut().zip(w) {
                    *o += w * v;
                }
            }
        }
        out
    }
}

/// The model with what was learned here: what sorts mail.
#[derive(Clone)]
pub struct Ai {
    model: Arc<Model>,
    pub learned: Learned,
    /// Changes whenever what it says may have: a message's verdict from an
    /// older generation is worked out again.
    pub generation: u64,
}

impl std::fmt::Debug for Ai {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Ai")
            .field("lessons", &self.learned.lessons)
            .field("generation", &self.generation)
            .finish()
    }
}

/// The generation of the model with `learned`: the same for the same model
/// and learning, from one launch to the next (the looks kept in the cache,
/// `Store::save_looks`, stay good), and another once either changes. Never 0,
/// which stands for no model.
fn generation_of(learned: &Learned) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    super::CATEGORIES.installed.hash(&mut h);
    serde_json::to_string(learned).unwrap_or_default().hash(&mut h);
    h.finish().max(1)
}

impl Ai {
    pub fn new(model: Model, learned: Learned) -> Self {
        Self {
            model: Arc::new(model),
            generation: generation_of(&learned),
            learned,
        }
    }

    /// The probability of each category for features `x`: the model's,
    /// adjusted by what was learned.
    fn combined(&self, x: &[(usize, f32)]) -> Vec<f32> {
        let p = self.model.probabilities(x);
        if self.learned.is_empty() {
            return p;
        }
        let adjust = self.learned.adjust(x);
        let mut z: Vec<f32> = p
            .iter()
            .zip(adjust)
            .map(|(p, a)| p.max(1e-6).ln() + a)
            .collect();
        let max = z.iter().copied().fold(f32::MIN, f32::max);
        let mut total = 0.;
        for z in &mut z {
            *z = (*z - max).exp();
            total += *z;
        }
        z.iter_mut().for_each(|z| *z /= total);
        z
    }

    /// The probability of each category (in `Category::ALL`'s order) for
    /// a message.
    pub fn probabilities(&self, m: &Mail) -> [f32; CLASSES] {
        let p = self.combined(&vector(&features(&Input::of(m))));
        let mut out = [0.; CLASSES];
        for (k, c) in OUTPUTS.iter().enumerate() {
            out[*c as usize] = p[k];
        }
        out
    }

    /// The likeliest category for a message, and how likely.
    pub fn verdict(&self, m: &Mail) -> (Category, f32) {
        let p = self.probabilities(m);
        let (k, best) =
            p.iter().enumerate().fold(
                (0, f32::MIN),
                |b, (k, p)| if *p > b.1 { (k, *p) } else { b },
            );
        (Category::ALL[k], best)
    }

    /// Learns that `messages` belong in `category`: a few passes of
    /// gradient steps on the learned layer, the model's own weights left
    /// as they are. Returns the new generation of it.
    pub fn learn(&self, messages: &[&Mail], category: Category) -> Self {
        let mut next = self.clone();
        let target = OUTPUTS.iter().position(|c| *c == category).unwrap_or(0);
        let xs: Vec<Vec<(usize, f32)>> = messages
            .iter()
            .map(|m| vector(&features(&Input::of(m))))
            .collect();
        for _ in 0..PASSES {
            for x in &xs {
                let p = next.combined(x);
                let learned = &mut next.learned;
                for k in 0..CLASSES {
                    let g = p[k] - if k == target { 1. } else { 0. };
                    learned.bias[k] = (learned.bias[k] - RATE * 0.05 * g).clamp(-LIMIT, LIMIT);
                    for &(i, v) in x {
                        let w = learned.weights.entry(i as u32).or_insert([0.; CLASSES]);
                        w[k] = (w[k] - RATE * g * v).clamp(-LIMIT, LIMIT);
                    }
                }
            }
        }
        next.learned.lessons = next.learned.lessons.saturating_add(xs.len() as u32);
        if next.learned.weights.len() > MAX_FEATURES {
            let mut strength: Vec<(u32, f32)> = next
                .learned
                .weights
                .iter()
                .map(|(i, w)| (*i, w.iter().map(|w| w.abs()).sum()))
                .collect();
            strength.sort_by(|a, b| b.1.total_cmp(&a.1));
            let keep: std::collections::HashSet<u32> = strength
                .into_iter()
                .take(MAX_FEATURES)
                .map(|(i, _)| i)
                .collect();
            next.learned.weights.retain(|i, _| keep.contains(i));
        }
        next.generation = generation_of(&next.learned);
        next
    }

    /// The same model, having forgotten what it learned here.
    pub fn forgetting(&self) -> Self {
        Self {
            model: self.model.clone(),
            learned: Learned::default(),
            generation: generation_of(&Learned::default()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The trained model, where `models/categories/train.py` has written
    /// it (it isn't published yet, nor kept in git): None, and the test
    /// skipped, where it hasn't.
    pub(crate) fn trained() -> Option<Model> {
        let path =
            super::super::local_dir(&super::super::CATEGORIES).join(super::super::CATEGORIES.file);
        if !path.exists() {
            eprintln!(
                "skipping: {} isn't there (run models/categories/train.py)",
                path.display()
            );
            return None;
        }
        let path = super::super::test_model(super::super::CATEGORIES.file);
        Some(
            Model::new(NativeTagger::load(&path).expect("the model loads"))
                .expect("model and classes agree"),
        )
    }

    fn mail(name: &str, from: &str, subject: &str, preview: &str) -> Mail {
        Mail {
            id: format!("{from}:{subject}"),
            sender: name.into(),
            from: from.into(),
            subject: subject.into(),
            snippet: preview.into(),
            ..Mail::default()
        }
    }

    #[test]
    fn the_outputs_are_the_categories_in_order() {
        assert_eq!(OUTPUTS.as_slice(), Category::ALL.as_slice());
    }

    #[test]
    fn features_read_the_sender_headers_and_words() {
        let f = features(&Input {
            name: "Kestrel Outfitters",
            address: "News@E.Kestrel.co",
            subject: "Up to 70% off 🔥",
            preview: "Shop now: $20 off",
            unsubscribe: "<https://kestrel.us4.list-manage.com/u>",
            list_id: "4f2a.list-id.mcsv.net",
            list_post: false,
        });
        for want in [
            "S:70",
            "L:news",
            "LW:news",
            "D:kestrel",
            "DT:co",
            "R:kestrel.co",
            "DS:e",
            "sub:1",
            "C:discount",
            "unsub",
            "unsub:http",
            "esp",
            "listid",
            "LI:list",
            "pct",
            "money",
            "emoji",
            "brand",
        ] {
            assert!(f.iter().any(|x| x == want), "{want} in {f:?}");
        }
        // A shop's name can look like a person's; its domain says it's the
        // shop's ("brand").
        assert!(!f.iter().any(|x| x == "auto" || x == "re"));
        let f = features(&Input {
            name: "Ana Silva",
            address: "ana.silva@gmail.com",
            subject: "Re: dinner",
            ..Input::default()
        });
        assert!(f.iter().any(|x| x == "person") && f.iter().any(|x| x == "re"));
        assert!(features(&Input::default()).iter().any(|x| x == "noname"));
    }

    /// The features, vectors and probabilities the training code computed
    /// for its examples come out the same here.
    #[test]
    fn matches_the_training_code() {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("models/categories/parity.json");
        let Ok(text) = std::fs::read_to_string(&path) else {
            eprintln!(
                "skipping: {} isn't there (run models/categories/train.py)",
                path.display()
            );
            return;
        };
        let parity: serde_json::Value = serde_json::from_str(&text).unwrap();
        let model = trained();
        for case in parity["cases"].as_array().unwrap() {
            let s = |k: &str| case[k].as_str().unwrap();
            let input = Input {
                name: s("name"),
                address: s("address"),
                subject: s("subject"),
                preview: s("preview"),
                unsubscribe: s("unsubscribe"),
                list_id: s("list_id"),
                list_post: case["list_post"].as_bool().unwrap(),
            };
            let f = features(&input);
            let want: Vec<String> = serde_json::from_value(case["features"].clone()).unwrap();
            assert_eq!(f, want, "features of {:?}", input.subject);
            let x = vector(&f);
            let want: Vec<(usize, f32)> = case["vector"]
                .as_array()
                .unwrap()
                .iter()
                .map(|p| {
                    (
                        p[0].as_u64().unwrap() as usize,
                        p[1].as_f64().unwrap() as f32,
                    )
                })
                .collect();
            assert_eq!(x.len(), want.len());
            for ((i, v), (j, u)) in x.iter().zip(&want) {
                assert_eq!(i, j);
                assert!((v - u).abs() < 1e-5, "{v} vs {u}");
            }
            if let Some(model) = &model {
                let p = model.probabilities(&x);
                for (k, want) in case["probabilities"].as_array().unwrap().iter().enumerate() {
                    let want = want.as_f64().unwrap() as f32;
                    assert!((p[k] - want).abs() < 1e-4, "{k}: {} vs {want}", p[k]);
                }
            }
        }
    }

    /// Held-out wordings (`eval.json`): the model's category is right for
    /// nearly all, and people's mail stays in Primary.
    #[test]
    fn sorts_held_out_mail() {
        let Some(model) = trained() else { return };
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("models/categories/eval.json");
        let cases: Vec<serde_json::Value> =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let ai = Ai::new(model, Learned::default());
        let (mut right, mut people, mut people_kept) = (0, 0, 0);
        // By language: (right, all) of people's mail and of the rest.
        let mut by_language: BTreeMap<String, [(usize, usize); 2]> = BTreeMap::new();
        for c in &cases {
            let s = |k: &str| c[k].as_str().unwrap().to_owned();
            let mut m = mail(&s("name"), &s("address"), &s("subject"), &s("preview"));
            m.list_unsubscribe = s("unsub");
            m.set_list_id(s("list_id"));
            if c["list_post"].as_bool().unwrap() {
                m.set_list_links(vec![crate::manage::list_headers::ListLink {
                    kind: ListLinkKind::Post,
                    uri: "mailto:list@example.org".into(),
                }]);
            }
            let (guess, _) = ai.verdict(&m);
            let gold = Category::parse(&s("category")).unwrap();
            right += usize::from(guess == gold);
            if gold == Category::Primary {
                people += 1;
                people_kept += usize::from(guess == Category::Primary);
            }
            let tally = &mut by_language.entry(s("lang")).or_default()
                [usize::from(gold != Category::Primary)];
            tally.0 += usize::from(guess == gold);
            tally.1 += 1;
        }
        let share = right as f32 / cases.len() as f32;
        assert!(share > 0.9, "right for {share}");
        assert!(
            people_kept as f32 / people as f32 > 0.995,
            "{people_kept} of {people} kept in Primary"
        );
        // Every language the other models have (the reply model's catalog
        // words its replies in each), each doing well on its own.
        let catalog: serde_json::Value =
            serde_json::from_str(include_str!("../../../models/replies/catalog.json")).unwrap();
        let languages: Vec<&String> = catalog["replies"].as_object().unwrap().keys().collect();
        assert_eq!(languages.len(), 38);
        for lang in languages {
            let [(p_ok, p_all), (o_ok, o_all)] = by_language
                .get(lang)
                .copied()
                .unwrap_or_else(|| panic!("no held-out mail in {lang}"));
            assert!(
                o_all > 0 && p_all > 0,
                "{lang}: {o_all} sorted, {p_all} people's"
            );
            let (people_share, other_share) =
                (p_ok as f32 / p_all as f32, o_ok as f32 / o_all as f32);
            assert!(
                people_share > 0.97,
                "{lang}: people's mail right for {people_share}"
            );
            assert!(
                other_share > 0.8,
                "{lang}: the rest right for {other_share}"
            );
        }
    }

    /// Moving a few messages teaches mail like them to go there, from
    /// other senders too; forgetting undoes it.
    #[test]
    fn learns_from_messages_moved() {
        let Some(model) = trained() else { return };
        let ai = Ai::new(model, Learned::default());
        let webinar = |n: u32| {
            mail(
                "Northwind",
                &format!("events{n}@northwind{n}.io"),
                "Join our free webinar on cloud costs",
                "Register now: Thursday at 11 AM. Seats are limited.",
            )
        };
        let before = ai.probabilities(&webinar(9))[Category::Updates as usize];
        let moved: Vec<Mail> = (0..3).map(webinar).collect();
        let refs: Vec<&Mail> = moved.iter().collect();
        let taught = ai.learn(&refs, Category::Updates);
        assert!(taught.generation != ai.generation && taught.learned.lessons == 3);
        let after = taught.probabilities(&webinar(9))[Category::Updates as usize];
        assert!(after > before && after > 0.5, "{before} → {after}");
        assert_eq!(taught.verdict(&webinar(9)).0, Category::Updates);
        // Unrelated mail is left as it was.
        let person = mail(
            "Ana Silva",
            "ana@gmail.com",
            "Lunch on Friday?",
            "Are you free around noon?",
        );
        assert_eq!(taught.verdict(&person).0, Category::Primary);
        // Kept as JSON, and forgotten.
        let json = serde_json::to_string(&taught.learned).unwrap();
        assert_eq!(
            serde_json::from_str::<Learned>(&json).unwrap(),
            taught.learned
        );
        let forgot = taught.forgetting();
        assert!(forgot.learned.is_empty());
        // A generation is the model's and its learning's: the same from one
        // launch to the next (kept looks stay good), another once learning
        // changes, and back once it's forgotten.
        assert_eq!(forgot.generation, ai.generation);
        assert_eq!(generation_of(&serde_json::from_str(&json).unwrap()), taught.generation);
        assert!(
            (forgot.probabilities(&webinar(9))[Category::Updates as usize] - before).abs() < 1e-6
        );
    }
}

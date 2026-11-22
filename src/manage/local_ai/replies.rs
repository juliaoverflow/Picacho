//! Reply suggestions, with Echo Replies: the model reads a received message
//! and scores every reply in `models/replies/catalog.json` ("Thanks for
//! letting me know.", "Let me check my calendar."…), and "none" for mail
//! that wants no reply. The best few are offered under the message, worded
//! in the message's language, which is told from its script and common
//! words (`language`).
//!
//! Messages are cleaned and their features built exactly as
//! `models/replies/features.py` does for training; `models/replies/
//! parity.json` checks the two agree. The model's outputs are in
//! `classes.json`'s order; `catalog.json`'s wordings can change without
//! retraining. What a person chooses, and the suggestions they turn away,
//! adjust what is offered next (`History`), on this device only.
use super::otp::{vector_in, words};
use super::runtime::NativeTagger;
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::LazyLock;

/// The hashed feature space (a power of two).
pub const DIM: usize = 1 << 15;
const MAX_TEXT: usize = 4000;
const SUBJECT_WORDS: usize = 16;
const BODY_WORDS: usize = 160;
const QUESTION_LINES: usize = 4;
const QUESTION_WORDS: usize = 24;
const QUESTION_MARKS: [char; 3] = ['?', '？', '؟'];
/// The class for mail that wants no reply.
pub const NONE: &str = "none";
/// How many suggestions are offered at most, and the least probability one
/// needs.
pub const MAX_SUGGESTIONS: usize = 4;
const MIN_SCORE: f32 = 0.03;

/// Concepts that carry across languages (invite, sorry, sick, deadline…),
/// each with stems in every language: "=" marks a stem that must be a whole
/// word, "^" one a word must start with, and a stem with a space is looked
/// for across words. The model was trained with this same file.
static LEXICON: LazyLock<Vec<(String, Vec<String>)>> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../../../models/replies/lexicon.json")).expect("lexicon")
});

#[derive(Deserialize)]
struct Catalog {
    intents: Vec<String>,
    replies: HashMap<String, HashMap<String, String>>,
}

/// The replies and their wording in each language.
static CATALOG: LazyLock<Catalog> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../../../models/replies/catalog.json")).expect("catalog")
});

/// The model's classes, in the order of its outputs.
static CLASSES: LazyLock<Vec<String>> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../../../models/replies/classes.json")).expect("classes")
});

/// Common words of the languages that share a script, for telling them
/// apart.
static STOPWORDS: LazyLock<Vec<(String, HashSet<String>)>> = LazyLock::new(|| {
    let map: BTreeMap<String, Vec<String>> =
        serde_json::from_str(include_str!("../../../models/replies/stopwords.json"))
            .expect("stopwords");
    map.into_iter()
        .map(|(lang, ws)| (lang, ws.into_iter().collect()))
        .collect()
});

/// A reply's wording in `language`, if the catalog has one.
pub fn wording(intent: &str, language: &str) -> Option<&'static str> {
    CATALOG
        .replies
        .get(language)?
        .get(intent)
        .map(String::as_str)
}

/// The concepts named by the words, in lexicon order.
pub(crate) fn concepts(words: &[String]) -> Vec<&'static str> {
    let joined = format!(" {} ", words.join(" "));
    LEXICON
        .iter()
        .filter(|(_, stems)| {
            stems.iter().any(|st| {
                if let Some(whole) = st.strip_prefix('=') {
                    words.iter().any(|w| w == whole)
                } else if let Some(start) = st.strip_prefix('^') {
                    if start.contains(' ') {
                        joined.contains(&format!(" {start}"))
                    } else {
                        words.iter().any(|w| w.starts_with(start))
                    }
                } else if st.contains(' ') {
                    joined.contains(st.as_str())
                } else {
                    words.iter().any(|w| w.contains(st.as_str()))
                }
            })
        })
        .map(|(name, _)| name.as_str())
        .collect()
}

/// The part of a body its sender wrote: up to the signature ("--") or a
/// forwarded or quoted original ("-----", "_____"), without quoted lines.
pub(crate) fn clean(body: &str) -> String {
    let body: String = body.chars().take(MAX_TEXT).collect();
    let mut out: Vec<&str> = Vec::new();
    for line in body.split('\n') {
        let t = line.trim();
        if t == "--" || t.starts_with("-----") || t.starts_with("_____") {
            break;
        }
        if t.starts_with('>') {
            continue;
        }
        out.push(line);
    }
    out.join("\n")
}

/// The feature strings of a message.
pub(crate) fn features(subject: &str, body: &str) -> Vec<String> {
    let text = clean(body);
    let sw = words(subject, true);
    let sw = &sw[..sw.len().min(SUBJECT_WORDS)];
    let bw = words(&text, true);
    let bw = &bw[..bw.len().min(BODY_WORDS)];
    let mut f: Vec<String> = sw.iter().map(|w| format!("S:{w}")).collect();
    f.extend(bw.iter().map(|w| format!("B:{w}")));
    f.extend(bw.windows(2).map(|p| format!("BB:{}_{}", p[0], p[1])));
    let mut asked = 0;
    for line in text.split('\n') {
        if asked == QUESTION_LINES {
            break;
        }
        if line.contains(&QUESTION_MARKS[..]) {
            asked += 1;
            let lw = words(line, true);
            f.extend(lw.iter().take(QUESTION_WORDS).map(|w| format!("QW:{w}")));
            f.extend(
                concepts(&words(line, false))
                    .into_iter()
                    .map(|c| format!("QC:{c}")),
            );
        }
    }
    let whole = words(&text, false);
    f.extend(
        concepts(&whole[..whole.len().min(BODY_WORDS)])
            .into_iter()
            .map(|c| format!("C:{c}")),
    );
    f.extend(
        concepts(&words(subject, false))
            .into_iter()
            .map(|c| format!("CS:{c}")),
    );
    let q = text.matches(&QUESTION_MARKS[..]).count();
    f.push(format!("q:{}", q.min(3)));
    if text.contains('!') || text.contains('！') {
        f.push("ex".into());
    }
    f.push(format!("len:{}", (bw.len() / 20).min(6)));
    f
}

/// The hashed, normalized features of a message.
pub(crate) fn vector(features: &[String]) -> Vec<(usize, f32)> {
    vector_in(features, DIM)
}

/// The script a character is written in, for telling languages apart.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Script {
    Latin,
    Cyrillic,
    Greek,
    Arabic,
    Hebrew,
    Devanagari,
    Bengali,
    Thai,
    Hangul,
    Kana,
    Han,
}

fn script(c: char) -> Option<Script> {
    let u = u32::from(c);
    Some(match u {
        _ if c.is_ascii_alphabetic() => Script::Latin,
        0x00C0..=0x024F | 0x1E00..=0x1EFF => Script::Latin,
        0x0370..=0x03FF | 0x1F00..=0x1FFF => Script::Greek,
        0x0400..=0x04FF => Script::Cyrillic,
        0x0590..=0x05FF => Script::Hebrew,
        0x0600..=0x06FF | 0x0750..=0x077F | 0xFB50..=0xFDFF | 0xFE70..=0xFEFF => Script::Arabic,
        0x0900..=0x097F => Script::Devanagari,
        0x0980..=0x09FF => Script::Bengali,
        0x0E00..=0x0E7F => Script::Thai,
        0x1100..=0x11FF | 0x3130..=0x318F | 0xAC00..=0xD7AF => Script::Hangul,
        0x3040..=0x30FF => Script::Kana,
        0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF => Script::Han,
        _ => return None,
    })
}

/// Characters written only in traditional Chinese, and their simplified
/// forms, among the commonest in mail.
const TRADITIONAL: &str = "們這個來說時會對為發後過國學還麼開關點經電話謝請問題見嗎們於與從區實現機郵週認識業務幫準備確覆議應時間總進給讓聽體營臺灣";
const SIMPLIFIED: &str = "们这个来说时会对为发后过国学还么开关点经电话谢请问题见吗们于与从区实现机邮周认识业务帮准备确复议应时间总进给让听体营台湾";

/// Letters that point to one language among those sharing a script.
const HINTS: &[(&str, &str)] = &[
    ("pt", "ãõ"),
    ("es", "ñ¿¡"),
    ("ro", "șțăşţ"),
    ("pl", "ąęłśźżńć"),
    ("cs", "řůě"),
    ("sk", "ľĺŕô"),
    ("hr", "đ"),
    ("hu", "őű"),
    ("tr", "ğış"),
    ("vi", "ơưạảấầẩẫậắằẳẵặẹẻẽếềểễệỉịọỏốồổỗộớờởỡợụủứừửữựỳỵỷỹđ"),
    ("de", "ß"),
    ("ca", "·èò"),
    ("uk", "іїєґ"),
    ("ru", "ыэё"),
    ("bg", "ъ"),
];

/// The language a message is written in, among those replies are worded
/// in, told from the script of its letters and, where languages share a
/// script, its common words and particular letters. None when there's too
/// little to tell.
pub fn language(subject: &str, body: &str) -> Option<&'static str> {
    let text: String = format!("{subject}\n{}", clean(body))
        .chars()
        .take(2000)
        .collect::<String>()
        .to_lowercase();
    let mut counts: HashMap<Script, usize> = HashMap::new();
    for c in text.chars() {
        if let Some(s) = script(c) {
            *counts.entry(s).or_default() += 1;
        }
    }
    let kana = counts.get(&Script::Kana).copied().unwrap_or(0);
    let han = counts.get(&Script::Han).copied().unwrap_or(0);
    // Japanese mixes kana into its kanji; a little kana is enough.
    if kana > 0 && kana + han >= counts.values().copied().max().unwrap_or(0) / 2 {
        return Some("ja");
    }
    let (&top, _) = counts.iter().max_by_key(|(s, n)| (**n, script_rank(**s)))?;
    let count = |chars: &str| text.chars().filter(|c| chars.contains(*c)).count();
    let picked = match top {
        Script::Hangul => "ko",
        Script::Kana => "ja",
        Script::Han => {
            if count(TRADITIONAL) > count(SIMPLIFIED) {
                "zh-hant"
            } else {
                "zh"
            }
        }
        Script::Thai => "th",
        Script::Devanagari => "hi",
        Script::Bengali => "bn",
        Script::Hebrew => "he",
        Script::Greek => "el",
        Script::Arabic => {
            // Persian's own letters (پ چ ژ گ ک ی) against Arabic's (ة ي ك ى).
            if count("پچژگکی") > count("ةيكى") {
                "fa"
            } else {
                "ar"
            }
        }
        // Without words to tell them apart, Ukrainian's and Bulgarian's own
        // letters do, else Russian.
        Script::Cyrillic => {
            by_words(&text, &["ru", "uk", "bg"]).unwrap_or(if count("іїєґ") > 0 {
                "uk"
            } else if count("ъ") > 0 && count("ыэё") == 0 {
                "bg"
            } else {
                "ru"
            })
        }
        Script::Latin => by_words(
            &text,
            &[
                "en", "es", "pt", "ca", "it", "fr", "de", "nl", "sv", "da", "no", "pl", "cs", "sk",
                "hr", "ro", "hu", "fi", "tr", "id", "ms", "vi", "tl", "sw",
            ],
        )?,
    };
    CATALOG
        .replies
        .get_key_value(picked)
        .map(|(k, _)| k.as_str())
}

/// Ties between scripts with as many letters go to the one listed first.
fn script_rank(s: Script) -> u8 {
    match s {
        Script::Latin => 0,
        _ => 1,
    }
}

/// The likeliest of `candidates` for `text` (lowercase): how many of its
/// words are each language's common words, and its particular letters.
fn by_words(text: &str, candidates: &[&'static str]) -> Option<&'static str> {
    let tokens = words(text, false);
    let mut best: Option<(&'static str, f32)> = None;
    for &lang in candidates {
        let mut score = 0.;
        if let Some((_, stop)) = STOPWORDS.iter().find(|(l, _)| l == lang) {
            score += tokens.iter().filter(|w| stop.contains(w.as_str())).count() as f32;
        }
        if let Some((_, letters)) = HINTS.iter().find(|(l, _)| *l == lang) {
            score += 0.5 * text.chars().filter(|c| letters.contains(*c)).count() as f32;
        }
        if score > best.map_or(0., |(_, s)| s) {
            best = Some((lang, score));
        }
    }
    best.filter(|(_, s)| *s >= 1.).map(|(l, _)| l)
}

/// The language replies are worded in when a message's can't be told: the
/// computer's, if replies are worded in it, else English.
pub fn fallback_language() -> &'static str {
    let locale = sys_locale::get_locale().unwrap_or_default();
    for_locale(&locale)
}

fn for_locale(locale: &str) -> &'static str {
    let lower = locale.to_ascii_lowercase().replace('_', "-");
    let mut parts = lower.split('-');
    let base = parts.next().unwrap_or("");
    let rest: Vec<&str> = parts.collect();
    let code = match base {
        "zh" if rest
            .iter()
            .any(|p| matches!(*p, "hant" | "tw" | "hk" | "mo")) =>
        {
            "zh-hant"
        }
        "nb" | "nn" => "no",
        "fil" => "tl",
        "iw" => "he",
        "in" => "id",
        other => other,
    };
    CATALOG
        .replies
        .get_key_value(code)
        .map_or("en", |(k, _)| k.as_str())
}

/// Mail its sender doesn't read replies to, by its address or a list's
/// unsubscribe header: no suggestions for it.
pub fn wants_no_reply(from: &str, list_unsubscribe: &str) -> bool {
    if !list_unsubscribe.trim().is_empty() {
        return true;
    }
    let address = from
        .rsplit('<')
        .next()
        .unwrap_or(from)
        .trim_end_matches('>')
        .to_ascii_lowercase();
    let local = address.split('@').next().unwrap_or("");
    [
        "noreply",
        "no-reply",
        "no_reply",
        "donotreply",
        "do-not-reply",
        "do_not_reply",
        "mailer-daemon",
        "postmaster",
        "notification",
        "newsletter",
        "bounce",
        "alerts",
    ]
    .iter()
    .any(|p| local.contains(p))
}

/// How a person has used each reply: chosen from the suggestions, or
/// turned away ("Don't Suggest This"). Kept on this device.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct History {
    pub uses: BTreeMap<String, Use>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Use {
    pub chosen: u32,
    pub dismissed: u32,
}

impl History {
    /// How much more (or less) likely to offer `intent`: chosen replies
    /// rise, up to three times as likely; None for one turned away since it
    /// was last chosen.
    pub fn factor(&self, intent: &str) -> Option<f32> {
        let u = self.uses.get(intent).copied().unwrap_or_default();
        if u.dismissed > u.chosen {
            return None;
        }
        Some(1. + 0.25 * u.chosen.min(8) as f32)
    }

    /// `intent` was chosen.
    pub fn chose(&mut self, intent: &str) {
        let u = self.uses.entry(intent.to_owned()).or_default();
        u.chosen = u.chosen.saturating_add(1);
    }

    /// `intent` isn't to be offered again (until chosen from elsewhere or
    /// the history is forgotten).
    pub fn dismiss(&mut self, intent: &str) {
        let u = self.uses.entry(intent.to_owned()).or_default();
        u.dismissed = u.chosen.saturating_add(1);
    }

    /// The replies turned away, in catalog order.
    pub fn dismissed(&self) -> Vec<&str> {
        CATALOG
            .intents
            .iter()
            .map(String::as_str)
            .filter(|i| self.factor(i).is_none())
            .collect()
    }

    pub fn is_empty(&self) -> bool {
        self.uses.is_empty()
    }
}

/// A suggested reply.
#[derive(Clone, Debug, PartialEq)]
pub struct Suggestion {
    pub intent: &'static str,
    pub text: &'static str,
    pub score: f32,
}

/// The replies suggested for a message, and the language they're worded in.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Suggestions {
    pub language: &'static str,
    pub replies: Vec<Suggestion>,
}

/// Suggests replies with a loaded model.
pub struct Suggester {
    tagger: NativeTagger,
}

impl Suggester {
    pub fn new(tagger: NativeTagger) -> Result<Self> {
        ensure!(
            tagger.classes() == CLASSES.len(),
            "The reply model has {} outputs, but {} replies are expected",
            tagger.classes(),
            CLASSES.len()
        );
        Ok(Self { tagger })
    }

    /// The probability of each class (classes.json's order) for a message.
    pub fn probabilities(&self, subject: &str, body: &str) -> Vec<f32> {
        self.tagger.probabilities(&vector(&features(subject, body)))
    }

    /// The replies to suggest for a message: none where the model finds it
    /// wants none, else the likeliest (adjusted by `history`, and only
    /// those worded in the message's language, or `fallback` when that
    /// can't be told), at most `limit`.
    pub fn suggest(
        &self,
        subject: &str,
        body: &str,
        history: &History,
        fallback: &'static str,
        limit: usize,
    ) -> Suggestions {
        let language = language(subject, body).unwrap_or(fallback);
        let p = self.probabilities(subject, body);
        let none = CLASSES.iter().position(|c| c == NONE);
        let top = p
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .map(|(k, _)| k);
        if top.is_none() || top == none {
            return Suggestions {
                language,
                replies: Vec::new(),
            };
        }
        let mut scored: Vec<Suggestion> = CLASSES
            .iter()
            .zip(&p)
            .filter(|(c, _)| c.as_str() != NONE)
            .filter_map(|(c, &prob)| {
                let factor = history.factor(c)?;
                let text = wording(c, language)?;
                let intent = CATALOG.intents.iter().find(|i| *i == c)?.as_str();
                Some(Suggestion {
                    intent,
                    text,
                    score: prob * factor,
                })
            })
            .filter(|s| s.score >= MIN_SCORE)
            .collect();
        scored.sort_by(|a, b| b.score.total_cmp(&a.score));
        scored.truncate(limit);
        Suggestions {
            language,
            replies: scored,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The published model (the trained file when it is that one,
    /// otherwise downloaded once; see `test_model`).
    pub(crate) fn trained() -> Suggester {
        let path = super::super::test_model(super::super::REPLIES.file);
        Suggester::new(NativeTagger::load(&path).expect("the published model loads"))
            .expect("model and classes agree")
    }

    #[test]
    fn the_catalog_words_every_class_in_every_language() {
        let classes: Vec<&str> = CLASSES.iter().map(String::as_str).collect();
        assert_eq!(classes.last(), Some(&NONE));
        assert_eq!(&classes[..classes.len() - 1], CATALOG.intents.as_slice());
        assert_eq!(CATALOG.replies.len(), 38);
        for (lang, replies) in &CATALOG.replies {
            for intent in &CATALOG.intents {
                assert!(replies.contains_key(intent), "{lang} has no {intent}");
            }
        }
        assert_eq!(wording("thanks", "de"), Some("Danke!"));
        assert_eq!(wording("thanks", "xx"), None);
    }

    #[test]
    fn concepts_match_stems_whole_words_prefixes_and_phrases() {
        let w = |s: &str| words(s, false);
        assert!(concepts(&w("Thanks a lot!")).contains(&"thank"));
        assert!(concepts(&w("How are you doing?")).contains(&"howareyou"));
        // "=late" is a whole word: not in "latest".
        assert!(!concepts(&w("the latest figures")).contains(&"late"));
        assert!(concepts(&w("I'll be late")).contains(&"late"));
        assert!(concepts(&w("明天下午可以开会吗")).contains(&"meet"));
        assert!(concepts(&[]).is_empty());
    }

    #[test]
    fn cleaning_keeps_what_the_sender_wrote() {
        let body = "Thanks!\n> quoted\nmore\n-- \nSam\nSales";
        assert_eq!(clean(body), "Thanks!\nmore");
        assert_eq!(clean("Hi\n-----Original Message-----\nold"), "Hi");
        let f = features("Lunch?", "Are you free on Friday?\n> Old question?");
        assert!(f.contains(&"q:1".to_owned()), "{f:?}");
        assert!(f.contains(&"CS:free".to_owned()) || f.contains(&"C:free".to_owned()));
        assert!(f.iter().any(|x| x == "QW:free"));
    }

    #[test]
    fn tells_languages_apart() {
        let cases = [
            (
                "en",
                "Can we meet on Friday? Let me know what works for you.",
            ),
            (
                "es",
                "¿Podemos reunirnos el viernes? Avísame si te viene bien.",
            ),
            (
                "pt",
                "Podemos nos reunir na sexta? Me avise se está tudo bem para você.",
            ),
            (
                "ca",
                "Podem quedar divendres? Digues-me si et va bé, gràcies.",
            ),
            (
                "it",
                "Possiamo vederci venerdì? Fammi sapere se per te va bene.",
            ),
            (
                "fr",
                "Peut-on se voir vendredi ? Dis-moi si ça te va, merci.",
            ),
            (
                "de",
                "Können wir uns am Freitag treffen? Sag mir bitte, ob es dir passt.",
            ),
            (
                "nl",
                "Kunnen we vrijdag afspreken? Laat me weten of het je uitkomt.",
            ),
            (
                "sv",
                "Kan vi ses på fredag? Säg till om det passar dig, tack.",
            ),
            (
                "da",
                "Kan vi mødes på fredag? Sig til, hvis det passer dig, tak.",
            ),
            (
                "no",
                "Kan vi møtes på fredag? Si fra om det passer for deg, takk.",
            ),
            (
                "pl",
                "Czy możemy się spotkać w piątek? Daj znać, czy ci pasuje.",
            ),
            (
                "cs",
                "Můžeme se sejít v pátek? Dej vědět, jestli ti to vyhovuje.",
            ),
            (
                "sk",
                "Môžeme sa stretnúť v piatok? Daj vedieť, či ti to vyhovuje.",
            ),
            (
                "hr",
                "Možemo li se naći u petak? Javi mi odgovara li ti, hvala.",
            ),
            (
                "ro",
                "Ne putem vedea vineri? Spune-mi dacă îți convine, mulțumesc.",
            ),
            (
                "hu",
                "Találkozhatnánk pénteken? Szólj, ha neked is jó, köszi.",
            ),
            (
                "fi",
                "Voisimmeko tavata perjantaina? Kerro, sopiiko se sinulle, kiitos.",
            ),
            (
                "tr",
                "Cuma günü buluşabilir miyiz? Sana uyarsa haber ver, teşekkürler.",
            ),
            (
                "id",
                "Bisa kita bertemu hari Jumat? Kabari saya kalau kamu bisa.",
            ),
            (
                "ms",
                "Boleh kita berjumpa hari Jumaat? Beritahu saya kalau awak boleh.",
            ),
            (
                "vi",
                "Mình gặp nhau vào thứ Sáu được không? Bạn báo mình nhé.",
            ),
            (
                "tl",
                "Puwede ba tayong magkita sa Biyernes? Sabihan mo ako, salamat.",
            ),
            (
                "sw",
                "Tunaweza kukutana Ijumaa? Niambie kama inakufaa, asante sana.",
            ),
            (
                "ru",
                "Можем встретиться в пятницу? Дайте знать, если вам удобно.",
            ),
            (
                "uk",
                "Можемо зустрітися в п'ятницю? Дайте знати, чи вам зручно.",
            ),
            (
                "bg",
                "Може ли да се видим в петък? Кажи ми дали ти е удобно.",
            ),
            (
                "el",
                "Μπορούμε να βρεθούμε την Παρασκευή; Πες μου αν σε βολεύει.",
            ),
            (
                "ar",
                "هل يمكننا أن نلتقي يوم الجمعة؟ أخبرني إن كان ذلك يناسبك.",
            ),
            (
                "fa",
                "می‌توانیم جمعه همدیگر را ببینیم؟ به من خبر بده که برایت مناسب است یا نه.",
            ),
            ("he", "אפשר להיפגש ביום שישי? תגיד לי אם זה מתאים לך."),
            ("hi", "क्या हम शुक्रवार को मिल सकते हैं? मुझे बताइए।"),
            ("bn", "আমরা কি শুক্রবার দেখা করতে পারি? আমাকে জানাবেন।"),
            ("th", "เราเจอกันวันศุกร์ได้ไหม บอกฉันด้วยนะ"),
            ("ja", "金曜日にお会いできますか？ご都合を教えてください。"),
            ("zh", "我们周五见面可以吗？请告诉我你是否方便。"),
            ("zh-hant", "我們週五見面可以嗎？請告訴我你是否方便。"),
            ("ko", "금요일에 만날 수 있을까요? 괜찮으신지 알려 주세요."),
        ];
        let wrong: Vec<String> = cases
            .iter()
            .filter(|(want, text)| language("", text) != Some(want))
            .map(|(want, text)| format!("{want}: {:?} for {text}", language("", text)))
            .collect();
        assert!(wrong.is_empty(), "{wrong:#?}");
        assert_eq!(language("", "12345 !!!"), None);
        assert_eq!(for_locale("de_DE.UTF-8"), "de");
        assert_eq!(for_locale("zh-Hant-TW"), "zh-hant");
        assert_eq!(for_locale("nb-NO"), "no");
        assert_eq!(for_locale("xx"), "en");
    }

    #[test]
    fn automated_senders_want_no_reply() {
        assert!(wants_no_reply("Acme <no-reply@acme.test>", ""));
        assert!(wants_no_reply("noreply@github.com", ""));
        assert!(wants_no_reply("notifications@service.test", ""));
        assert!(wants_no_reply(
            "ana@example.com",
            "<mailto:leave@list.test>"
        ));
        assert!(!wants_no_reply("Ana <ana@example.com>", ""));
        assert!(!wants_no_reply("Sam <sam.reply@example.com>", ""));
    }

    #[test]
    fn history_raises_chosen_replies_and_hides_dismissed_ones() {
        let mut h = History::default();
        assert_eq!(h.factor("thanks"), Some(1.));
        h.chose("thanks");
        h.chose("thanks");
        assert_eq!(h.factor("thanks"), Some(1.5));
        h.dismiss("thanks");
        assert_eq!(h.factor("thanks"), None);
        assert_eq!(h.dismissed(), ["thanks"]);
        // Chosen again after being turned away, it's offered.
        h.chose("thanks");
        h.chose("thanks");
        assert!(h.factor("thanks").is_some());
        for _ in 0..20 {
            h.chose("sounds_good");
        }
        assert_eq!(h.factor("sounds_good"), Some(3.));
        let json = serde_json::to_string(&h).unwrap();
        assert_eq!(serde_json::from_str::<History>(&json).unwrap(), h);
        assert_eq!(
            serde_json::from_str::<History>("{}").unwrap(),
            History::default()
        );
    }

    /// The features, vectors and scores the training code computed for its
    /// examples come out the same here.
    #[test]
    fn matches_the_training_code() {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("models/replies/parity.json");
        let Ok(text) = std::fs::read_to_string(&path) else {
            eprintln!(
                "skipping: {} isn't there (run models/replies/train.py)",
                path.display()
            );
            return;
        };
        let parity: serde_json::Value = serde_json::from_str(&text).unwrap();
        let classes: Vec<String> = serde_json::from_value(parity["classes"].clone()).unwrap();
        assert_eq!(&classes, &*CLASSES);
        let model = trained();
        for case in parity["cases"].as_array().unwrap() {
            let subject = case["subject"].as_str().unwrap();
            let body = case["body"].as_str().unwrap();
            let f = features(subject, body);
            let expected: Vec<String> = serde_json::from_value(case["features"].clone()).unwrap();
            assert_eq!(f, expected, "features of {subject:?} / {body:?}");
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
            let p = model.probabilities(subject, body);
            for t in case["top"].as_array().unwrap() {
                let k = CLASSES
                    .iter()
                    .position(|c| c == t[0].as_str().unwrap())
                    .unwrap();
                let want = t[1].as_f64().unwrap() as f32;
                assert!((p[k] - want).abs() < 1e-4, "{}: {} vs {want}", t[0], p[k]);
            }
        }
    }

    /// Fresh messages in every language (new combinations, names, days and
    /// slips, of wordings the published model was trained on; how it does on
    /// wording it never saw is what `train.py` reports first): the language
    /// is told, and the best suggestion fits the message.
    #[test]
    fn suggests_fitting_replies_in_the_language_of_the_message() {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("models/replies/eval.json");
        let Ok(text) = std::fs::read_to_string(&path) else {
            eprintln!(
                "skipping: {} isn't there (run models/replies/train.py)",
                path.display()
            );
            return;
        };
        let cases: Vec<serde_json::Value> = serde_json::from_str(&text).unwrap();
        let model = trained();
        // Per language: [told, messages, best fits, replied to].
        let mut by: BTreeMap<String, [usize; 4]> = BTreeMap::new();
        let mut quiet_wrongly = 0;
        let mut automated = [0, 0];
        for case in &cases {
            let subject = case["subject"].as_str().unwrap();
            let body = case["body"].as_str().unwrap();
            let lang = case["language"].as_str().unwrap();
            let e = by.entry(lang.to_owned()).or_default();
            e[1] += 1;
            let told = language(subject, body);
            e[0] += usize::from(told == Some(lang));
            if told != Some(lang) && std::env::var_os("PICACHO_SHOW_MISSES").is_some() {
                eprintln!("{lang} told as {told:?}: {subject:?} {body:?}");
            }
            let s = model.suggest(subject, body, &History::default(), "en", MAX_SUGGESTIONS);
            if case["none"].as_bool().unwrap() {
                automated[1] += 1;
                automated[0] += usize::from(s.replies.is_empty());
                continue;
            }
            e[3] += 1;
            let fits: Vec<&str> = case["fits"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap())
                .collect();
            match s.replies.first() {
                Some(best) => e[2] += usize::from(fits.contains(&best.intent)),
                None => quiet_wrongly += 1,
            }
        }
        let share = |a: usize, b: usize| a as f64 / b.max(1) as f64;
        let mut told = [0, 0];
        let mut fits = [0, 0];
        for (lang, e) in &by {
            eprintln!(
                "  {lang}: language {:.3} of {}, best reply fits {:.3}",
                share(e[0], e[1]),
                e[1],
                share(e[2], e[3])
            );
            told[0] += e[0];
            told[1] += e[1];
            fits[0] += e[2];
            fits[1] += e[3];
            assert!(
                share(e[0], e[1]) > 0.75,
                "{lang}'s language is told too rarely"
            );
        }
        assert!(share(told[0], told[1]) > 0.9, "languages told {told:?}");
        eprintln!(
            "best reply fits {:.3}; automated mail left alone {:.3}; replied-to mail left without suggestions {quiet_wrongly}",
            share(fits[0], fits[1]),
            share(automated[0], automated[1])
        );
        assert!(share(fits[0], fits[1]) > 0.85);
        assert!(share(automated[0], automated[1]) > 0.8);
        assert!(quiet_wrongly * 50 < fits[1]);
    }
}

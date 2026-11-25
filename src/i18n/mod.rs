//! The interface's text, in the reader's language (i18n).
//!
//! Every sentence Picacho shows is a Fluent message (projectfluent.org) in
//! `locales/<language>/*.ftl`, named by an id: `t!("settings-section-general")`
//! is its text, and `t!("notify-new-messages", count = n)` fills in its
//! variables (and picks its plural form, by the language's own rules).
//! English (`locales/en`) is the source and the fallback: a message a
//! translation lacks is shown in English, never as its id.
//!
//! The language is the system's, unless Settings → General → Language
//! chooses one (`set_preference`, the `language` preference). Mail Picacho
//! writes (quotes, invitations, read receipts) is in the interface's
//! language too, unless Settings chooses another for it (`in_outgoing`, the
//! `outgoing-language` preference). Text without
//! variables is kept once per language, so `t!("id")` is a `&'static str`
//! and fits wherever a literal did.
//!
//! Adding a language: a folder in `locales/` with the same files, listed
//! in `catalog::LANGUAGES`. `tests` check that every id the code names
//! exists in English, that every English message is used, and that each
//! translation's messages exist in English with the same variables.

/// The text of message `id`, or, with `name = value` pairs, the message
/// with its variables filled in (a `String`).
macro_rules! t {
    ($id:literal) => {
        $crate::i18n::text($id)
    };
    ($id:literal, $($name:ident = $value:expr),+ $(,)?) => {
        $crate::i18n::format(
            $id,
            &[$((stringify!($name), $crate::i18n::Arg::arg(&$value))),+],
        )
    };
}

mod catalog;
pub mod dates;
#[cfg(test)]
mod tests;

use fluent_bundle::{FluentArgs, FluentResource, FluentValue, concurrent::FluentBundle};
use std::{
    borrow::Cow,
    cell::RefCell,
    collections::HashMap,
    sync::{Arc, LazyLock, Mutex, RwLock},
};
use unic_langid::LanguageIdentifier;

/// The preference naming the chosen language ("" or none: the system's).
pub const PREFERENCE: &str = "language";

/// The preference naming the language of mail Picacho writes ("" or none:
/// the interface's).
pub const OUTGOING_PREFERENCE: &str = "outgoing-language";

/// One language's messages, with English behind them.
struct Catalog {
    /// The language shown.
    language: &'static str,
    bundle: FluentBundle<FluentResource>,
    /// English, for messages the language lacks (`None` when it's English).
    english: Option<FluentBundle<FluentResource>>,
    /// Text without variables, kept for the process's life (one per
    /// message shown, per language chosen).
    kept: Mutex<HashMap<&'static str, &'static str>>,
}

static CURRENT: LazyLock<RwLock<Arc<Catalog>>> =
    LazyLock::new(|| RwLock::new(loaded(system_language())));

/// The language of mail Picacho writes, when Settings chose one.
static OUTGOING: RwLock<Option<&'static str>> = RwLock::new(None);

/// The languages loaded so far, each once for the process's life.
static LOADED: LazyLock<Mutex<HashMap<&'static str, Arc<Catalog>>>> = LazyLock::new(Mutex::default);

/// `language`'s catalog, loaded the first time it's asked for.
fn loaded(language: &'static str) -> Arc<Catalog> {
    let mut all = LOADED.lock().unwrap_or_else(|e| e.into_inner());
    all.entry(language)
        .or_insert_with(|| Arc::new(Catalog::load(language)))
        .clone()
}

thread_local! {
    /// The language this thread shows instead of the chosen one, inside
    /// `in_source` or `in_language`.
    static OVERRIDE: RefCell<Option<Arc<Catalog>>> = const { RefCell::new(None) };
}

fn current() -> Arc<Catalog> {
    if let Some(catalog) = OVERRIDE.with_borrow(Option::clone) {
        return catalog;
    }
    CURRENT.read().unwrap_or_else(|e| e.into_inner()).clone()
}

/// `f`'s result with every message in English (the source), whatever the
/// language shown: for text that leaves Picacho, like the words of an
/// invitation sent by mail, which stay in English for now.
#[cfg_attr(not(any(target_os = "linux", target_os = "freebsd")), allow(dead_code))]
pub fn in_source<R>(f: impl FnOnce() -> R) -> R {
    with_catalog(loaded(catalog::SOURCE), f)
}

/// `f`'s result with every message in `language` (one of `languages()`'
/// codes; English where it lacks one): for text the system shows in its
/// reader's language, like the desktop entry's.
pub fn in_language<R>(language: &'static str, f: impl FnOnce() -> R) -> R {
    with_catalog(loaded(language), f)
}

/// `f`'s result in each language Picacho has, English first: for reading
/// back text Picacho may have written in any of them (a reply's
/// "On …, … wrote:" line).
pub fn in_every_language<R>(f: impl Fn() -> R) -> Vec<R> {
    catalog::LANGUAGES
        .iter()
        .map(|&(code, _)| in_language(code, &f))
        .collect()
}

/// `f`'s result with every message in the language of mail Picacho writes:
/// what Settings chose for it, else the interface's.
pub fn in_outgoing<R>(f: impl FnOnce() -> R) -> R {
    let chosen = *OUTGOING.read().unwrap_or_else(|e| e.into_inner());
    match chosen {
        Some(language) => in_language(language, f),
        None => f(),
    }
}

/// Writes mail in `chosen` (a language code; empty or `None`: the
/// interface's), from now on.
pub fn set_outgoing(chosen: Option<&str>) {
    let language = chosen
        .map(str::trim)
        .filter(|c| !c.is_empty())
        .and_then(|c| negotiate([c]));
    *OUTGOING.write().unwrap_or_else(|e| e.into_inner()) = language;
}

fn with_catalog<R>(catalog: Arc<Catalog>, f: impl FnOnce() -> R) -> R {
    struct Restore(Option<Arc<Catalog>>);
    impl Drop for Restore {
        fn drop(&mut self) {
            OVERRIDE.set(self.0.take());
        }
    }
    let _restore = Restore(OVERRIDE.replace(Some(catalog)));
    f()
}

/// The language shown: one of `catalog::LANGUAGES`' codes.
#[allow(dead_code)]
pub fn language() -> &'static str {
    current().language
}

/// The languages Picacho can show: code and name (in that language).
#[allow(dead_code)]
pub fn languages() -> &'static [(&'static str, &'static str)] {
    &catalog::NAMES
}

/// Shows `chosen` (a language code; empty or `None` for the system's), from
/// now on. Views redraw with it at their next frame.
pub fn set_preference(chosen: Option<&str>) {
    let language = chosen
        .map(str::trim)
        .filter(|c| !c.is_empty())
        .and_then(|c| negotiate([c]))
        .unwrap_or_else(system_language);
    if language == current().language {
        return;
    }
    *CURRENT.write().unwrap_or_else(|e| e.into_inner()) = loaded(language);
}

/// The system's preferred language Picacho has, or English.
fn system_language() -> &'static str {
    // Tests read English, whatever the machine's language.
    if cfg!(test) {
        return catalog::SOURCE;
    }
    negotiate(sys_locale::get_locales()).unwrap_or(catalog::SOURCE)
}

/// The first of `wanted` (BCP 47 tags, most preferred first) that Picacho
/// has, matching by language with region and script where they decide
/// (pt-BR, zh-Hant).
fn negotiate<S: AsRef<str>>(wanted: impl IntoIterator<Item = S>) -> Option<&'static str> {
    use fluent_langneg::{NegotiationStrategy, negotiate_languages};
    let wanted: Vec<LanguageIdentifier> = wanted
        .into_iter()
        .filter_map(|w| w.as_ref().replace('_', "-").split('.').next()?.parse().ok())
        .collect();
    let available: Vec<LanguageIdentifier> = catalog::LANGUAGES
        .iter()
        .filter_map(|(code, _)| code.parse().ok())
        .collect();
    let chosen = negotiate_languages(&wanted, &available, None, NegotiationStrategy::Lookup);
    let chosen = chosen.first()?.to_string();
    catalog::LANGUAGES
        .iter()
        .map(|(code, _)| *code)
        .find(|code| *code == chosen)
}

impl Catalog {
    fn load(language: &'static str) -> Self {
        let english = (language != catalog::SOURCE).then(|| bundle(catalog::SOURCE));
        Self {
            language,
            bundle: bundle(language),
            english,
            kept: Mutex::default(),
        }
    }

    /// The bundle that has `id`: the language's, else English's.
    fn having(&self, id: &str) -> Option<&FluentBundle<FluentResource>> {
        std::iter::once(&self.bundle)
            .chain(self.english.as_ref())
            .find(|b| b.has_message(id))
    }

    fn format(&self, id: &str, args: Option<&FluentArgs>) -> String {
        let Some(bundle) = self.having(id) else {
            debug_assert!(false, "no message {id}");
            return id.to_owned();
        };
        let Some(pattern) = bundle.get_message(id).and_then(|m| m.value()) else {
            return id.to_owned();
        };
        let mut errors = Vec::new();
        let text = bundle.format_pattern(pattern, args, &mut errors);
        if !errors.is_empty() {
            eprintln!("Picacho: message {id} in {}: {errors:?}", self.language);
        }
        text.into_owned()
    }
}

/// One language's bundle, from its files. A file that doesn't parse loses
/// the messages that don't (the tests keep that from shipping).
fn bundle(language: &'static str) -> FluentBundle<FluentResource> {
    let id: LanguageIdentifier = language.parse().unwrap_or_default();
    let mut bundle = FluentBundle::new_concurrent(vec![id]);
    // No Unicode isolation marks around variables: GPUI lays out
    // bidirectional text itself, and the marks would show in some fonts.
    bundle.set_use_isolating(false);
    for source in catalog::files(language) {
        let resource = FluentResource::try_new(source.to_owned()).unwrap_or_else(|(r, errors)| {
            eprintln!("Picacho: locales/{language}: {errors:?}");
            r
        });
        if let Err(errors) = bundle.add_resource(resource) {
            eprintln!("Picacho: locales/{language}: {errors:?}");
        }
    }
    bundle
}

/// Message `id`'s text, without variables.
pub fn text(id: &'static str) -> &'static str {
    let catalog = current();
    let mut kept = catalog.kept.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(text) = kept.get(id) {
        return text;
    }
    let text: &'static str = Box::leak(catalog.format(id, None).into_boxed_str());
    kept.insert(id, text);
    text
}

/// `text` (a message without variables, as a language showed it) in the
/// language shown now: for words set once and kept, like a text field's
/// placeholder, when the language changes. `None` when it isn't such a
/// message (or wasn't shown).
pub fn reword(text: &str) -> Option<&'static str> {
    let catalogs: Vec<Arc<Catalog>> = LOADED.lock().unwrap_or_else(|e| e.into_inner()).values().cloned().collect();
    let id = catalogs.iter().find_map(|catalog| {
        let kept = catalog.kept.lock().unwrap_or_else(|e| e.into_inner());
        kept.iter().find(|(_, shown)| **shown == text).map(|(id, _)| *id)
    })?;
    Some(self::text(id))
}

/// Two sentences, one after the other, as the language writes them.
pub fn sentences(first: &str, second: &str) -> String {
    t!("app-sentences", first = first, second = second)
}

/// Message `id` with its variables (`name`, value) filled in.
pub fn format(id: &'static str, args: &[(&'static str, FluentValue<'static>)]) -> String {
    let mut fluent = FluentArgs::with_capacity(args.len());
    for (name, value) in args {
        fluent.set(*name, value.clone());
    }
    current().format(id, Some(&fluent))
}

/// What a message's variable can be: text or a number. Text is copied,
/// so the value needn't outlive the call.
pub trait Arg {
    fn arg(&self) -> FluentValue<'static>;
}
impl Arg for str {
    fn arg(&self) -> FluentValue<'static> {
        FluentValue::from(self.to_owned())
    }
}
impl Arg for String {
    fn arg(&self) -> FluentValue<'static> {
        FluentValue::from(self.clone())
    }
}
impl Arg for Cow<'_, str> {
    fn arg(&self) -> FluentValue<'static> {
        FluentValue::from(self.to_string())
    }
}
impl Arg for gpui::SharedString {
    fn arg(&self) -> FluentValue<'static> {
        FluentValue::from(self.to_string())
    }
}
impl Arg for char {
    fn arg(&self) -> FluentValue<'static> {
        FluentValue::from(self.to_string())
    }
}
impl<T: Arg + ?Sized> Arg for &T {
    fn arg(&self) -> FluentValue<'static> {
        (**self).arg()
    }
}
macro_rules! number_args {
    ($($t:ty)+) => {
        $(impl Arg for $t {
            fn arg(&self) -> FluentValue<'static> {
                FluentValue::from(*self)
            }
        })+
    };
}
number_args!(i8 i16 i32 i64 i128 isize u8 u16 u32 u64 u128 usize);
number_args!(f32 f64);
/// Displayed values a message often names: shown as the system writes them.
macro_rules! display_args {
    ($($t:ty),+ $(,)?) => {
        $(impl Arg for $t {
            fn arg(&self) -> FluentValue<'static> {
                FluentValue::from(self.to_string())
            }
        })+
    };
}
display_args!(
    std::path::Display<'_>,
    std::io::Error,
    anyhow::Error,
    reqwest::StatusCode,
    url::Url,
    lettre::Address,
);
impl<'a, I> Arg for chrono::format::DelayedFormat<I>
where
    I: Iterator + Clone,
    I::Item: std::borrow::Borrow<chrono::format::Item<'a>>,
{
    fn arg(&self) -> FluentValue<'static> {
        FluentValue::from(self.to_string())
    }
}

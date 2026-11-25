//! Spelling for the formatted composer: the system's spellchecker where
//! there is one (macOS's NSSpellChecker, Windows' spell checking API), and
//! on Linux and the BSDs the Hunspell dictionaries installed for desktop
//! apps (LibreOffice, Firefox), read with `spellbook`. Phones and tablets
//! leave spelling to their keyboards. Words learned are kept in a list of
//! Picacho's own (and, on macOS, in the system's).

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};

/// A spellchecker.
pub trait Speller: Send + Sync {
    /// Whether `word` is spelled right (or known).
    fn correct(&self, word: &str) -> bool;
    /// Spellings `word` may have meant, best first.
    fn suggest(&self, word: &str) -> Vec<String>;
    /// Accepts `word` from now on.
    fn learn(&self, word: &str) {
        let _ = word;
    }
}

/// The checker and what it said, shared by every composer.
pub struct Spelling {
    speller: Option<Arc<dyn Speller>>,
    known: Mutex<HashMap<String, bool>>,
    /// Words learned here, and where they are kept.
    personal: Mutex<HashSet<String>>,
    personal_path: Mutex<Option<std::path::PathBuf>>,
    /// Words to let be, this session.
    ignored: Mutex<HashSet<String>>,
}

static SPELLING: OnceLock<Spelling> = OnceLock::new();

/// The shared spelling, its checker made on first use.
pub fn spelling() -> &'static Spelling {
    SPELLING.get_or_init(|| Spelling {
        speller: platform(),
        known: Mutex::new(HashMap::new()),
        personal: Mutex::new(HashSet::new()),
        personal_path: Mutex::new(None),
        ignored: Mutex::new(HashSet::new()),
    })
}

impl Spelling {
    /// Whether there's a spellchecker here at all.
    pub fn available(&self) -> bool {
        self.speller.is_some()
    }
    /// Keeps learned words in `path` (one per line), reading those there.
    pub fn use_personal(&self, path: std::path::PathBuf) {
        let mut current = self.personal_path.lock().unwrap();
        if current.as_ref() == Some(&path) {
            return;
        }
        if let Ok(text) = std::fs::read_to_string(&path) {
            let mut personal = self.personal.lock().unwrap();
            personal.extend(text.lines().map(str::trim).filter(|w| !w.is_empty()).map(str::to_owned));
        }
        *current = Some(path);
    }
    /// Whether `word` reads as a mistake: unknown to the checker, and not
    /// learned or ignored. Never for what isn't a word to check (numbers,
    /// addresses, all capitals).
    pub fn misspelled(&self, word: &str) -> bool {
        let Some(speller) = &self.speller else { return false };
        if !checkable(word) {
            return false;
        }
        if self.personal.lock().unwrap().contains(word) || self.ignored.lock().unwrap().contains(word) {
            return false;
        }
        if let Some(&ok) = self.known.lock().unwrap().get(word) {
            return !ok;
        }
        let ok = speller.correct(word);
        let mut known = self.known.lock().unwrap();
        if known.len() > 50_000 {
            known.clear();
        }
        known.insert(word.to_owned(), ok);
        !ok
    }
    pub fn suggest(&self, word: &str) -> Vec<String> {
        let Some(speller) = &self.speller else { return vec![] };
        let mut out = speller.suggest(word);
        out.dedup();
        out.truncate(6);
        out
    }
    /// Accepts `word` from now on, here and in later sessions.
    pub fn learn(&self, word: &str) {
        if let Some(speller) = &self.speller {
            speller.learn(word);
        }
        self.known.lock().unwrap().insert(word.to_owned(), true);
        let mut personal = self.personal.lock().unwrap();
        if personal.insert(word.to_owned())
            && let Some(path) = self.personal_path.lock().unwrap().as_ref()
        {
            let mut words: Vec<&String> = personal.iter().collect();
            words.sort();
            let text: String = words.iter().map(|w| format!("{w}\n")).collect();
            if let Err(e) = std::fs::write(path, text) {
                eprintln!("The learned word couldn't be kept: {e:#}");
            }
        }
    }
    /// Lets `word` be, until Picacho quits.
    pub fn ignore(&self, word: &str) {
        self.ignored.lock().unwrap().insert(word.to_owned());
    }
}

/// Whether `word` is something to check: letters (an apostrophe or hyphen
/// inside), not all capitals (acronyms), not one letter.
pub fn checkable(word: &str) -> bool {
    let letters = word.chars().filter(|c| c.is_alphabetic()).count();
    letters > 1
        && word.chars().all(|c| c.is_alphabetic() || matches!(c, '\'' | '’' | '-'))
        && !(word.chars().filter(|c| c.is_alphabetic()).all(char::is_uppercase) && letters <= 6)
}

/// The words of `text` worth checking, as byte ranges.
pub fn words(text: &str) -> Vec<std::ops::Range<usize>> {
    use unicode_segmentation::UnicodeSegmentation as _;
    let mut out = Vec::new();
    for (i, w) in text.unicode_word_indices() {
        let range = i..i + w.len();
        // Inside an address or a path, it isn't a word.
        let before = text[..i].chars().next_back();
        let after = text[range.end..].chars().next();
        let joined = |c: Option<char>| c.is_some_and(|c| matches!(c, '@' | '/' | '\\' | '.' | '_' | ':' | '#') || c.is_ascii_digit());
        if joined(before) && !matches!(before, Some('.')) || joined(after) && after != Some('.') {
            continue;
        }
        // "e.g." and file names: a dot followed by a letter.
        if after == Some('.') && text[range.end + 1..].chars().next().is_some_and(char::is_alphanumeric) {
            continue;
        }
        out.push(range);
    }
    out
}

#[cfg(any(target_os = "android", target_os = "ios"))]
fn platform() -> Option<Arc<dyn Speller>> {
    None
}

/// Tests spell with a checker that knows one mistake ("helo"), so they
/// don't depend on the dictionaries installed.
#[cfg(test)]
fn platform() -> Option<Arc<dyn Speller>> {
    struct Test;
    impl Speller for Test {
        fn correct(&self, word: &str) -> bool {
            word != "helo"
        }
        fn suggest(&self, _: &str) -> Vec<String> {
            vec!["hello".into(), "help".into()]
        }
    }
    Some(Arc::new(Test))
}

#[cfg(all(target_os = "macos", not(test)))]
fn platform() -> Option<Arc<dyn Speller>> {
    Some(Arc::new(mac::System))
}

#[cfg(all(windows, not(test)))]
fn platform() -> Option<Arc<dyn Speller>> {
    windows_speller::System::new().map(|s| Arc::new(s) as Arc<dyn Speller>)
}

#[cfg(all(unix, not(any(target_os = "macos", target_os = "android", target_os = "ios")), not(test)))]
fn platform() -> Option<Arc<dyn Speller>> {
    hunspell::Dictionaries::installed().map(|d| Arc::new(d) as Arc<dyn Speller>)
}

/// macOS's own spellchecker, which follows the languages set in System
/// Settings → Keyboard → Text Input.
#[cfg(target_os = "macos")]
mod mac {
    use objc2_app_kit::NSSpellChecker;
    use objc2_foundation::{NSRange, NSString};

    /// NSSpellChecker's shared checker; checking and guessing work from
    /// any thread (its panel is the main thread's, and isn't used).
    pub struct System;

    impl super::Speller for System {
        fn correct(&self, word: &str) -> bool {
            let checker = NSSpellChecker::sharedSpellChecker();
            let text = NSString::from_str(word);
            checker.checkSpellingOfString_startingAt(&text, 0).length == 0
        }
        fn suggest(&self, word: &str) -> Vec<String> {
            let checker = NSSpellChecker::sharedSpellChecker();
            let text = NSString::from_str(word);
            let range = NSRange::new(0, text.length());
            let Some(guesses) = checker.guessesForWordRange_inString_language_inSpellDocumentWithTag(range, &text, None, 0) else {
                return vec![];
            };
            (0..guesses.count()).map(|i| guesses.objectAtIndex(i).to_string()).collect()
        }
        fn learn(&self, word: &str) {
            NSSpellChecker::sharedSpellChecker().learnWord(&NSString::from_str(word));
        }
    }
}

/// Windows' spell checking API (Windows 8 and later), in the languages
/// set in Settings → Time & language.
#[cfg(windows)]
mod windows_speller {
    use windows::Win32::Globalization::{ISpellChecker, ISpellCheckerFactory, SpellCheckerFactory};
    use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx};
    use windows::core::{HSTRING, PCWSTR};

    pub struct System {
        checkers: Vec<ISpellChecker>,
    }

    // The checkers are free-threaded COM objects.
    unsafe impl Send for System {}
    unsafe impl Sync for System {}

    impl System {
        pub fn new() -> Option<Self> {
            unsafe {
                let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
                let factory: ISpellCheckerFactory = CoCreateInstance(&SpellCheckerFactory, None, CLSCTX_INPROC_SERVER).ok()?;
                let mut languages = Vec::new();
                for tag in super::languages() {
                    let tag = HSTRING::from(tag.replace('_', "-"));
                    if factory.IsSupported(PCWSTR(tag.as_ptr())).is_ok_and(|b| b.as_bool()) {
                        languages.push(tag);
                    }
                }
                if languages.is_empty() {
                    languages.push(HSTRING::from("en-US"));
                }
                let checkers: Vec<ISpellChecker> = languages
                    .iter()
                    .filter_map(|tag| factory.CreateSpellChecker(PCWSTR(tag.as_ptr())).ok())
                    .collect();
                (!checkers.is_empty()).then_some(System { checkers })
            }
        }
    }

    impl super::Speller for System {
        fn correct(&self, word: &str) -> bool {
            let text = HSTRING::from(word);
            self.checkers.iter().any(|c| unsafe {
                // No errors listed: right in this language.
                match c.Check(PCWSTR(text.as_ptr())) {
                    Ok(errors) => {
                        let mut error = None;
                        errors.Next(&mut error).is_err() || error.is_none()
                    }
                    Err(_) => true,
                }
            })
        }
        fn suggest(&self, word: &str) -> Vec<String> {
            let text = HSTRING::from(word);
            let mut out = Vec::new();
            for c in &self.checkers {
                unsafe {
                    let Ok(list) = c.Suggest(PCWSTR(text.as_ptr())) else { continue };
                    loop {
                        let mut item = [windows::core::PWSTR::null()];
                        let mut fetched = 0;
                        if list.Next(&mut item, Some(&mut fetched)).is_err() || fetched == 0 {
                            break;
                        }
                        if let Ok(s) = item[0].to_string() {
                            out.push(s);
                        }
                        windows::Win32::System::Com::CoTaskMemFree(Some(item[0].0 as _));
                    }
                }
            }
            out
        }
        fn learn(&self, word: &str) {
            let text = HSTRING::from(word);
            if let Some(c) = self.checkers.first() {
                unsafe {
                    let _ = c.Add(PCWSTR(text.as_ptr()));
                }
            }
        }
    }
}

/// The languages to check in: the interface's, then the system's.
#[cfg_attr(any(test, target_os = "macos", target_os = "android", target_os = "ios"), allow(dead_code))]
fn languages() -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut push = |tag: String| {
        let tag = tag.split(['.', '@']).next().unwrap_or("").replace('-', "_");
        if !tag.is_empty() && tag != "C" && tag != "POSIX" && !out.contains(&tag) {
            out.push(tag);
        }
    };
    push(crate::i18n::language().to_owned());
    for var in ["LC_ALL", "LC_MESSAGES", "LANG", "LANGUAGE"] {
        if let Ok(v) = std::env::var(var) {
            for part in v.split(':') {
                push(part.to_owned());
            }
        }
    }
    if let Some(l) = sys_locale() {
        push(l);
    }
    out
}

#[cfg(windows)]
fn sys_locale() -> Option<String> {
    use windows::Win32::Globalization::GetUserDefaultLocaleName;
    let mut buf = [0u16; 85];
    let n = unsafe { GetUserDefaultLocaleName(&mut buf) };
    (n > 1).then(|| String::from_utf16_lossy(&buf[..n as usize - 1]))
}

#[cfg(not(windows))]
#[cfg_attr(any(test, target_os = "macos", target_os = "android", target_os = "ios"), allow(dead_code))]
fn sys_locale() -> Option<String> {
    None
}

/// Hunspell dictionaries (`.aff` and `.dic`), as desktop apps install them.
#[cfg(all(unix, not(any(target_os = "macos", target_os = "android", target_os = "ios"))))]
#[cfg_attr(test, allow(dead_code))]
mod hunspell {
    pub struct Dictionaries {
        dicts: Vec<spellbook::Dictionary>,
    }

    /// Where distributions and Flatpak runtimes put Hunspell dictionaries.
    fn folders() -> Vec<std::path::PathBuf> {
        let mut out = Vec::new();
        if let Some(home) = std::env::var_os("HOME") {
            out.push(std::path::Path::new(&home).join(".local/share/hunspell"));
        }
        let data = std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
        for d in data.split(':').filter(|d| !d.is_empty()) {
            out.push(std::path::Path::new(d).join("hunspell"));
            out.push(std::path::Path::new(d).join("myspell/dicts"));
            out.push(std::path::Path::new(d).join("myspell"));
        }
        for d in ["/app/share/hunspell", "/usr/share/hunspell", "/usr/share/myspell/dicts", "/usr/local/share/hunspell"] {
            out.push(d.into());
        }
        out.dedup();
        out
    }

    impl Dictionaries {
        /// The installed dictionaries for the languages in use (`en_US` or,
        /// failing that, any `en_*` for "en"), or English's when none is.
        pub fn installed() -> Option<Self> {
            let mut available: Vec<(String, std::path::PathBuf)> = Vec::new();
            for folder in folders() {
                let Ok(entries) = std::fs::read_dir(&folder) else { continue };
                for e in entries.flatten() {
                    let path = e.path();
                    if path.extension().is_some_and(|x| x == "dic")
                        && path.with_extension("aff").is_file()
                        && let Some(stem) = path.file_stem().map(|s| s.to_string_lossy().into_owned())
                        && !available.iter().any(|(s, _)| *s == stem)
                    {
                        available.push((stem, path));
                    }
                }
            }
            let mut wanted = super::languages();
            wanted.push("en_US".into());
            let mut chosen: Vec<&std::path::PathBuf> = Vec::new();
            for tag in &wanted {
                let lang = tag.split('_').next().unwrap_or(tag);
                let found = available
                    .iter()
                    .find(|(s, _)| s.eq_ignore_ascii_case(tag))
                    .or_else(|| available.iter().find(|(s, _)| s.split(['_', '-']).next() == Some(lang)));
                if let Some((_, p)) = found
                    && !chosen.contains(&p)
                {
                    chosen.push(p);
                }
                if chosen.len() >= 2 {
                    break;
                }
            }
            let dicts: Vec<spellbook::Dictionary> = chosen
                .into_iter()
                .filter_map(|dic| {
                    let aff = std::fs::read_to_string(dic.with_extension("aff")).ok()?;
                    let words = std::fs::read_to_string(dic).ok()?;
                    spellbook::Dictionary::new(&aff, &words).ok()
                })
                .collect();
            (!dicts.is_empty()).then_some(Dictionaries { dicts })
        }
        #[cfg(test)]
        pub fn from_parts(aff: &str, dic: &str) -> Self {
            Dictionaries { dicts: vec![spellbook::Dictionary::new(aff, dic).unwrap()] }
        }
    }

    impl super::Speller for Dictionaries {
        fn correct(&self, word: &str) -> bool {
            let word = word.replace('’', "'");
            self.dicts.iter().any(|d| d.check(&word))
        }
        fn suggest(&self, word: &str) -> Vec<String> {
            let mut out = Vec::new();
            for d in &self.dicts {
                d.suggest(word, &mut out);
            }
            out
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_words_are_checked() {
        let text = "Helo ana@example.com https://x.org/path NASA e.g. file.txt it's 2024 x";
        let found: Vec<&str> = words(text).into_iter().map(|r| &text[r]).filter(|w| checkable(w)).collect();
        assert_eq!(found, ["Helo", "it's"]);
    }

    #[cfg(all(unix, not(any(target_os = "macos", target_os = "android", target_os = "ios"))))]
    #[test]
    fn hunspell_dictionaries_check_and_suggest() {
        let aff = "SET UTF-8\nTRY esianrtolcdugmphbyfvkwz\n";
        let dic = "3\nhello\nworld\nhelp\n";
        let d = hunspell::Dictionaries::from_parts(aff, dic);
        assert!(d.correct("hello"));
        assert!(!d.correct("helo"));
        assert!(d.suggest("helo").iter().any(|s| s == "hello"));
    }
}

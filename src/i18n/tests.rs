use super::*;
use fluent_bundle::FluentResource;
use fluent_syntax::ast;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Every message in `language`'s files: its id and the variables it uses.
fn messages(language: &str) -> BTreeMap<String, BTreeSet<String>> {
    let mut all = BTreeMap::new();
    for (name, text) in catalog::LANGUAGES.iter().find(|(c, _)| *c == language).unwrap().1 {
        let resource = FluentResource::try_new((*text).to_owned())
            .unwrap_or_else(|(_, e)| panic!("locales/{language}/{name} doesn't parse: {e:?}"));
        for entry in resource.entries() {
            if let ast::Entry::Message(m) = entry {
                let mut vars = BTreeSet::new();
                if let Some(value) = &m.value {
                    variables(value, &mut vars);
                }
                let id = m.id.name.to_owned();
                assert!(m.value.is_some(), "{language}/{name}: {id} has no text");
                assert!(all.insert(id.clone(), vars).is_none(), "{language}: {id} is defined twice");
            }
        }
    }
    all
}

fn variables(pattern: &ast::Pattern<&str>, out: &mut BTreeSet<String>) {
    for element in &pattern.elements {
        if let ast::PatternElement::Placeable { expression } = element {
            expression_variables(expression, out);
        }
    }
}

fn expression_variables(expression: &ast::Expression<&str>, out: &mut BTreeSet<String>) {
    match expression {
        ast::Expression::Select { selector, variants } => {
            inline_variables(selector, out);
            for v in variants {
                variables(&v.value, out);
            }
        }
        ast::Expression::Inline(inline) => inline_variables(inline, out),
    }
}

fn inline_variables(inline: &ast::InlineExpression<&str>, out: &mut BTreeSet<String>) {
    match inline {
        ast::InlineExpression::VariableReference { id } => {
            out.insert(id.name.to_owned());
        }
        ast::InlineExpression::Placeable { expression } => expression_variables(expression, out),
        ast::InlineExpression::FunctionReference { arguments, .. } => {
            for a in &arguments.positional {
                inline_variables(a, out);
            }
        }
        _ => {}
    }
}

/// Every Rust file under `dir`.
fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            sources(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// The message ids the code names (`t!("id"`), and where.
/// Each `t!` call in `text`: where it starts, and what follows the opening
/// quote of its id (which may be on the next line, as rustfmt writes it).
fn t_calls(text: &str) -> impl Iterator<Item = (usize, &str)> {
    text.match_indices("t!(").filter_map(|(i, _)| {
        // `t!` itself, not the end of another macro's name (`set!`), nor
        // text in quotes (as here).
        if text[..i].chars().last().is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '"') {
            return None;
        }
        let rest = text[i + 3..].trim_start();
        Some((i, rest.strip_prefix('"')?))
    })
}

fn used() -> BTreeMap<String, PathBuf> {
    let mut files = Vec::new();
    sources(&root().join("src"), &mut files);
    let mut used = BTreeMap::new();
    for file in files {
        // Comments (examples in documentation) aside.
        let text: String = std::fs::read_to_string(&file)
            .unwrap()
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .flat_map(|l| [l, "\n"])
            .collect();
        for (_, rest) in t_calls(&text) {
            let id = &rest[..rest.find('"').unwrap()];
            used.entry(id.to_owned()).or_insert_with(|| file.clone());
        }
    }
    used
}

#[test]
fn every_file_in_locales_is_built_in() {
    for (code, files) in catalog::LANGUAGES {
        let mut listed: Vec<&str> = files.iter().map(|(name, _)| *name).collect();
        let mut on_disk: Vec<String> = std::fs::read_dir(root().join("locales").join(code))
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".ftl"))
            .collect();
        listed.sort_unstable();
        on_disk.sort_unstable();
        assert_eq!(listed, on_disk, "catalog::LANGUAGES lists locales/{code} as it is");
    }
    assert_eq!(catalog::LANGUAGES.len(), catalog::NAMES.len());
}

#[test]
fn every_message_the_code_names_is_in_english_and_every_english_one_is_used() {
    let english = messages(catalog::SOURCE);
    let used = used();
    let missing: Vec<String> = used
        .iter()
        .filter(|(id, _)| !english.contains_key(*id))
        .map(|(id, file)| format!("{id} ({})", file.display()))
        .collect();
    assert!(missing.is_empty(), "not in locales/en: {missing:#?}");
    let unused: Vec<&String> = english
        .keys()
        .filter(|id| !used.contains_key(*id) && *id != "app-name")
        .collect();
    assert!(unused.is_empty(), "in locales/en but never shown: {unused:#?}");
}

/// The variables each `t!("id", name = value, …)` in `src/` names: id,
/// names, and where.
fn calls() -> Vec<(String, BTreeSet<String>, String)> {
    let mut files = Vec::new();
    sources(&root().join("src"), &mut files);
    let mut out = Vec::new();
    for file in files {
        let text = std::fs::read_to_string(&file).unwrap();
        for (i, rest) in t_calls(&text) {
            let id = &rest[..rest.find('"').unwrap()];
            // The arguments after the id, split at the commas outside
            // brackets and strings.
            let mut args = vec![String::new()];
            let mut depth = 0;
            let mut chars = rest[id.len() + 1..].chars();
            while let Some(c) = chars.next() {
                match c {
                    '"' => {
                        let current = args.last_mut().unwrap();
                        current.push(c);
                        while let Some(c) = chars.next() {
                            current.push(c);
                            if c == '\\' {
                                current.extend(chars.next());
                            } else if c == '"' {
                                break;
                            }
                        }
                        continue;
                    }
                    '(' | '[' | '{' => depth += 1,
                    ')' | ']' | '}' if depth == 0 => break,
                    ')' | ']' | '}' => depth -= 1,
                    ',' if depth == 0 => {
                        args.push(String::new());
                        continue;
                    }
                    _ => {}
                }
                args.last_mut().unwrap().push(c);
            }
            let names = args
                .iter()
                .filter_map(|a| {
                    let (name, value) = a.split_once('=')?;
                    let name = name.trim();
                    (!value.starts_with('=') && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
                        .then(|| name.to_owned())
                })
                .collect();
            let line = text[..i].lines().count();
            out.push((id.to_owned(), names, format!("{}:{line}", file.display())));
        }
    }
    out
}

#[test]
fn every_call_names_its_messages_variables() {
    let english = messages(catalog::SOURCE);
    let wrong: Vec<String> = calls()
        .into_iter()
        .filter_map(|(id, names, at)| {
            let vars = english.get(&id)?;
            (names != *vars).then(|| format!("{at}: {id} is given {names:?}, has {vars:?}"))
        })
        .collect();
    assert!(wrong.is_empty(), "{wrong:#?}");
}

#[test]
fn translations_have_only_englishs_messages_and_variables() {
    let english = messages(catalog::SOURCE);
    for (code, _) in catalog::LANGUAGES.iter().filter(|(c, _)| *c != catalog::SOURCE) {
        for (id, vars) in messages(code) {
            let source = english
                .get(&id)
                .unwrap_or_else(|| panic!("{code}: {id} isn't an English message"));
            assert!(
                vars.is_subset(source),
                "{code}: {id} uses {vars:?}, English has {source:?}"
            );
        }
    }
}

#[test]
fn messages_read_in_english_with_their_variables() {
    assert_eq!(language(), "en");
    assert_eq!(t!("app-name"), "Picacho");
    // Kept once: the same text each time.
    assert!(std::ptr::eq(t!("app-name"), t!("app-name")));
    assert_eq!(negotiate(["en-US"]), Some("en"));
    assert_eq!(negotiate(["en_GB.UTF-8"]), Some("en"));
    assert_eq!(negotiate(["tlh"]), None);
    // Plurals and variables, without isolation marks around them.
    let bundle = {
        let mut b = FluentBundle::new_concurrent(vec!["en".parse().unwrap()]);
        b.set_use_isolating(false);
        b.add_resource(
            FluentResource::try_new(
                "n = { $count ->\n [one] One message\n *[other] { $count } messages\n} for { $who }\n"
                    .into(),
            )
            .unwrap(),
        )
        .unwrap();
        b
    };
    let say = |count: usize| {
        let mut args = FluentArgs::new();
        args.set("count", Arg::arg(&count));
        args.set("who", Arg::arg(&"Ana"));
        let m = bundle.get_message("n").unwrap();
        bundle.format_pattern(m.value().unwrap(), Some(&args), &mut vec![]).into_owned()
    };
    assert_eq!(say(1), "One message for Ana");
    assert_eq!(say(3), "3 messages for Ana");
}

#[test]
fn in_source_and_in_language_last_only_for_their_closure() {
    let outer = current().language;
    let (inner, nested) = in_language("en", || (language(), in_source(language)));
    assert_eq!((inner, nested), ("en", "en"));
    // Back to the chosen language, even after a panic inside.
    assert!(std::panic::catch_unwind(|| in_source(|| panic!("inside"))).is_err());
    assert!(OVERRIDE.with_borrow(Option::is_none));
    assert_eq!(current().language, outer);
}

#[test]
fn the_platforms_list_the_languages_picacho_has() {
    let ours: BTreeSet<String> = catalog::LANGUAGES.iter().map(|(code, _)| (*code).to_owned()).collect();
    let read = |path: &str| std::fs::read_to_string(root().join(path)).unwrap();
    let between = |text: &str, start: &str, end: &str| -> String {
        let from = text.find(start).unwrap_or_else(|| panic!("no {start}")) + start.len();
        text[from..][..text[from..].find(end).unwrap()].to_owned()
    };
    // Android 13's per-app language setting.
    let android = read("android/app/src/main/res/xml/locales_config.xml");
    let android: BTreeSet<String> = android
        .split("android:name=\"")
        .skip(1)
        .map(|rest| rest[..rest.find('"').unwrap()].to_owned())
        .collect();
    assert_eq!(android, ours, "res/xml/locales_config.xml");
    // macOS's and iOS's.
    let mac = between(&read("packaging/Info.plist"), "<key>CFBundleLocalizations</key><array>", "</array>");
    let mac: BTreeSet<String> = mac
        .split("<string>")
        .skip(1)
        .map(|rest| rest[..rest.find('<').unwrap()].to_owned())
        .collect();
    assert_eq!(mac, ours, "packaging/Info.plist");
    let ios = between(&read("packaging/ios/project.yml"), "CFBundleLocalizations: [", "]");
    let ios: BTreeSet<String> = ios.split(',').map(|code| code.trim().to_owned()).collect();
    assert_eq!(ios, ours, "packaging/ios/project.yml");
}

#[test]
fn outgoing_mail_follows_its_own_choice() {
    assert_eq!(in_outgoing(language), "en");
    set_outgoing(Some("en"));
    assert_eq!(in_outgoing(language), "en");
    set_outgoing(Some("tlh"));
    assert_eq!(in_outgoing(language), current().language);
    set_outgoing(None);
    assert_eq!(in_every_language(language), ["en"]);
}

#[test]
fn words_kept_from_before_are_worded_again() {
    let shown = t!("settings-language");
    assert_eq!(reword(shown), Some(shown));
    assert_eq!(reword("Not one of Picacho's messages"), None);
}

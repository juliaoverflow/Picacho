#!/bin/sh
# Type-checks src/manage/system_search/spotlight.rs for macOS and iOS
# without the whole app, whose C dependencies (aws-lc-sys and others) need
# Xcode's toolchain. The module needs only objc2, block2, objc2-foundation
# and libloading, at the versions Cargo.lock pins, and a few items of
# system_search/mod.rs and the store, stubbed here.
# Needs: rustup target add aarch64-apple-darwin aarch64-apple-ios
set -eu
root=$(cd "$(dirname "$0")/.." && pwd)
dir=$(mktemp -d)
trap 'rm -rf "$dir"' EXIT
mkdir "$dir/src"
# A locked version of `name` on the line starting with `line` (0.6., 0.3.…).
locked() {
    awk -v name="$1" -v line="$2" '$0 == "name = \"" name "\"" { getline; gsub(/"/, "", $3); if (index($3, line) == 1) v = $3 } END { print v }' "$root/Cargo.lock"
}
cat >"$dir/Cargo.toml" <<EOF
[package]
name = "spotlight-check"
version = "0.0.0"
edition = "2024"

[dependencies]
anyhow = "1"
objc2 = "=$(locked objc2 0.6.)"
block2 = "=$(locked block2 0.6.)"
libloading = "=$(locked libloading 0.8.)"
objc2-foundation = { version = "=$(locked objc2-foundation 0.3.)", default-features = false, features = ["std", "NSArray", "NSError", "NSString"] }
EOF
cat >"$dir/src/lib.rs" <<EOF
#![allow(dead_code)]
macro_rules! t {
    (\$id:literal) => { \$id };
    (\$id:literal, \$(\$n:ident = \$v:expr),+ \$(,)?) => {{ \$(let _ = &\$v;)+ format!("{}", \$id) }};
}
pub mod store {
    #[derive(Clone, Default)]
    pub struct SystemSearchMessage {
        pub account: String, pub mailbox: String, pub date: i64, pub subject: String, pub from: String,
        pub sender: String, pub to: String, pub cc: String, pub text: String, pub message_id: String,
    }
}
pub mod system_search {
    pub struct Entry { pub link: String, pub id: String, pub message: crate::store::SystemSearchMessage }
    pub struct Change { pub id: String, pub entry: Option<Entry>, pub listed: Option<(String, String)> }
    pub fn person(a: &str, b: &str) -> String { format!("{a}{b}") }
    pub fn people(_: &str) -> (Vec<String>, Vec<String>) { (Vec::new(), Vec::new()) }
    #[path = "$root/src/manage/system_search/spotlight.rs"]
    pub mod spotlight;
}
pub fn use_all() {
    let _ = system_search::spotlight::prepare();
    let _ = system_search::spotlight::apply(&[]);
    let _ = system_search::spotlight::clear();
    let _ = (system_search::spotlight::has_leftovers(), system_search::spotlight::warning());
    #[cfg(target_os = "macos")]
    system_search::spotlight::handle_activities(c"GPUIApplicationDelegate", |_| {});
}
EOF
cp "$root/Cargo.lock" "$dir/Cargo.lock"
for target in aarch64-apple-darwin aarch64-apple-ios; do
    echo "Checking spotlight.rs for $target"
    cargo check --quiet --manifest-path "$dir/Cargo.toml" --target "$target" --target-dir "$root/target/spotlight-check"
done

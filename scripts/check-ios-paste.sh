#!/bin/sh
# Type-checks src/display/paste_ios.rs for iOS without the whole app, whose
# C dependencies (aws-lc-sys and others) need Xcode's toolchain. The module
# needs only objc2, at the version Cargo.lock pins, and two items from
# paste.rs, stubbed here.
# Needs: rustup target add aarch64-apple-ios
set -eu
root=$(cd "$(dirname "$0")/.." && pwd)
dir=$(mktemp -d)
trap 'rm -rf "$dir"' EXIT
mkdir "$dir/src"
# The 0.6 line, which the app (and gpui-mobile) uses; 0.5 is also locked for others.
objc2=$(awk '/^name = "objc2"$/ { getline; gsub(/"/, "", $3); if ($3 ~ /^0\.6\./) v = $3 } END { print v }' "$root/Cargo.lock")
cat >"$dir/Cargo.toml" <<EOF
[package]
name = "ios-paste-check"
version = "0.0.0"
edition = "2024"

[dependencies]
objc2 = "=$objc2"
EOF
cat >"$dir/src/lib.rs" <<EOF
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Run {
    pub(crate) text: String,
    pub(crate) bold: bool,
    pub(crate) italic: bool,
    pub(crate) strike: bool,
    pub(crate) code: bool,
    pub(crate) link: Option<String>,
}
pub(crate) fn runs_to_html(_: &[Run]) -> String {
    String::new()
}
#[path = "$root/src/display/paste_ios.rs"]
mod ios;
pub fn clipboard_html() -> Option<String> {
    ios::clipboard_html()
}
EOF
cp "$root/Cargo.lock" "$dir/"
cd "$dir"
cargo clippy --offline --target aarch64-apple-ios -- -D warnings

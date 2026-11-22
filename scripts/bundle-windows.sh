#!/bin/sh
# Cross-builds Picacho for Windows from Linux (the x86_64-pc-windows-gnu
# target) and its installer, dist/Picacho-<version>-x64-debug.msi, for
# trying Picacho on Windows or under Wine:
#
#   sh scripts/bundle-windows.sh
#
# Needs rustup's x86_64-pc-windows-gnu target, MinGW-w64 (its gcc and
# windres), lld (build.rs links with it, to delay-load webauthn.dll and
# WebView2Loader.dll) and, for the MSI, msitools' wixl.
#
# Debug builds only: GPUI compiles its shaders with the Windows SDK's
# fxc.exe in release builds, which runs only on Windows, while debug builds
# compile them at run time, from the shader source in this computer's Cargo
# registry. The result runs on this computer (under Wine, or on Windows
# with the registry's path at hand), not elsewhere. Releases are built on
# Windows with scripts/bundle-windows.ps1.
set -eu
cd "$(dirname "$0")/.."
if [ -f .env ]; then
    set -a
    . ./.env
    set +a
fi
target=x86_64-pc-windows-gnu
cargo build --locked --target "$target"
out="target/$target/debug"
# The GNU build loads WebView2's loader from beside picacho.exe; it comes
# with the webview2-com-sys crate.
loader=$(find "$out/build" -path '*webview2-com-sys*/out/x64/WebView2Loader.dll' | head -n 1)
if [ -z "$loader" ]; then
    echo "WebView2Loader.dll wasn't found under $out/build" >&2
    exit 1
fi
cp "$loader" "$out/WebView2Loader.dll"
version=$(sed -n 's/^version = "\([^"]*\)"/\1/p' Cargo.toml | head -n 1)
mkdir -p dist
msi="dist/Picacho-$version-x64-debug.msi"
if ! command -v wixl >/dev/null 2>&1; then
    echo "Built $out/picacho.exe (install msitools for the MSI)"
    exit 0
fi
wixl --arch x64 \
    -D Version="${version%%[-+]*}" \
    -D ExeDir="$out" \
    -D WebView2Loader=yes \
    -o "$msi" packaging/windows/picacho.wxs
echo "Built $msi"

#!/bin/sh
# Cross-builds Picacho for Windows from Linux (the x86_64-pc-windows-gnu
# target) and its installer, dist/Picacho-<version>-x64-debug.msi, for
# trying Picacho on Windows or under Wine:
#
#   sh scripts/bundle-windows.sh
#
# Needs rustup's x86_64-pc-windows-gnu target, MinGW-w64 (its gcc and
# windres) and lld (build.rs links with it, to delay-load webauthn.dll
# and WebView2Loader.dll). The MSI, made with the WiX Toolset as on
# Windows, needs Wine with the Windows .NET runtime: set PICACHO_WIX to a
# command that runs WiX's wix.dll there, such as
#   PICACHO_WIX="wine /path/to/dotnet.exe Z:/path/to/wix.dll"
# (wix.dll from the `wix` 5.0.2 NuGet package's tools/net6.0/any). WiX's
# native Linux build misreads paths.
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
if [ -z "${PICACHO_WIX:-}" ]; then
    echo "Built $out/picacho.exe (set PICACHO_WIX for the MSI)"
    exit 0
fi
# Wine's Z: drive is this computer's root; WiX takes Windows paths.
win() { printf 'Z:%s' "$(realpath -m "$1")" | tr / '\\'; }
# The installer's dialogs: WiX's UI extension, from NuGet.
ui="target/wix/wixtoolset.ui.wixext.5.0.2"
if [ ! -f "$ui/wixext5/WixToolset.UI.wixext.dll" ]; then
    mkdir -p "$ui"
    curl -sSfL --proto '=https' -o "$ui/package.zip" \
        https://api.nuget.org/v3-flatcontainer/wixtoolset.ui.wixext/5.0.2/wixtoolset.ui.wixext.5.0.2.nupkg
    # Its code runs in the build: checked against the package as published.
    echo "5ef2c707614b9f70b6bbadd2d4abcb4124efee215e9b16bfbc80113079a604c7  $ui/package.zip" | sha256sum -c - \
        || { rm -f "$ui/package.zip"; exit 1; }
    (cd "$ui" && unzip -oq package.zip)
fi
# English's installer (picacho.en-US.wxl has its words; the release build,
# scripts/bundle-windows.ps1, makes one for each language).
$PICACHO_WIX build -arch x64 \
    -ext "$(win "$ui/wixext5/WixToolset.UI.wixext.dll")" \
    -culture en-US -loc "$(win packaging/windows/picacho.en-US.wxl)" \
    -d Version="${version%%[-+]*}" \
    -d ExeDir="$(win "$out")" \
    -d WebView2Loader=yes \
    -o "$(win "$msi")" "$(win packaging/windows/picacho.wxs)"
echo "Built $msi"

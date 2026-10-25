#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
profile="${1:-release}"
if [ "$profile" = release ]; then
    cargo build --release --locked
else
    cargo build --locked
fi
bundle="dist/Pinal Mail.app"
mkdir -p "$bundle/Contents/MacOS" "$bundle/Contents/Resources"
cp "target/$profile/pinal-mail" "$bundle/Contents/MacOS/pinal-mail"
cp packaging/Info.plist "$bundle/Contents/Info.plist"
if [ -f assets/AppIcon.icns ]; then
    cp assets/AppIcon.icns "$bundle/Contents/Resources/AppIcon.icns"
fi
codesign --force --deep --sign - "$bundle"
echo "Built $bundle"

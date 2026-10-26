#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
profile="${1:-release}"
# Optional built-in sign-in registrations (see README, "Connect email"):
# PICACHO_GOOGLE_CLIENT_ID, PICACHO_GOOGLE_CLIENT_SECRET,
# PICACHO_MICROSOFT_CLIENT_ID, PICACHO_PINAL_ACCOUNTS, PICACHO_PINAL_CLIENT_ID.
if [ "$profile" = release ]; then
    cargo build --release --locked
else
    cargo build --locked
fi
bundle="dist/Picacho.app"
mkdir -p "$bundle/Contents/MacOS" "$bundle/Contents/Resources"
cp "target/$profile/picacho" "$bundle/Contents/MacOS/picacho"
cp packaging/Info.plist "$bundle/Contents/Info.plist"
if [ -f assets/AppIcon.icns ]; then
    cp assets/AppIcon.icns "$bundle/Contents/Resources/AppIcon.icns"
fi
codesign --force --deep --sign - "$bundle"
echo "Built $bundle"

#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
profile="${1:-release}"
# Optional built-in sign-in registrations (see README, "Connect email"):
# PICACHO_GOOGLE_CLIENT_ID, PICACHO_GOOGLE_CLIENT_SECRET,
# PICACHO_MICROSOFT_CLIENT_ID, PICACHO_PINAL_ACCOUNTS, PICACHO_PINAL_CLIENT_ID.
# They are compiled in, so load them from .env (if present) before building.
if [ -f .env ]; then
    set -a
    . ./.env
    set +a
fi
if [ "$profile" = release ]; then
    cargo build --release --locked
else
    cargo build --locked
fi
if [ "$(uname -s)" != Darwin ]; then
    # Linux (and other Unix): no app bundle or code signing, just the binary.
    mkdir -p dist
    cp "target/$profile/picacho" dist/picacho
    echo "Built dist/picacho"
    exit 0
fi
bundle="dist/Picacho.app"
mkdir -p "$bundle/Contents/MacOS" "$bundle/Contents/Resources"
cp "target/$profile/picacho" "$bundle/Contents/MacOS/picacho"
cp packaging/Info.plist "$bundle/Contents/Info.plist"
if [ -f assets/AppIcon.icns ]; then
    cp assets/AppIcon.icns "$bundle/Contents/Resources/AppIcon.icns"
fi
# Ad-hoc signatures change identity on rebuild and can invalidate Keychain ACLs.
# A stable Apple Development/Developer ID identity preserves access across builds.
identity="${PICACHO_CODESIGN_IDENTITY:--}"
if [ "$identity" = - ]; then
    echo "Using ad-hoc signing; macOS may request Keychain access again after rebuilds."
    echo "Set PICACHO_CODESIGN_IDENTITY to a stable signing identity to preserve access."
    codesign --force --sign - "$bundle"
else
    codesign --force --options runtime --timestamp --sign "$identity" "$bundle"
fi
codesign --verify --strict "$bundle"
echo "Built $bundle"

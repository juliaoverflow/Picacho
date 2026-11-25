#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
profile="${1:-release}"
# Optional built-in sign-in registrations (see README, "Connect email"):
# PICACHO_GOOGLE_CLIENT_ID, PICACHO_GOOGLE_CLIENT_SECRET,
# PICACHO_MICROSOFT_CLIENT_ID, PICACHO_FASTMAIL_CLIENT_ID, PICACHO_PINAL_ACCOUNTS,
# PICACHO_PINAL_CLIENT_ID, PICACHO_PINAL_MAIL_DOMAINS, PICACHO_PINAL_IMAP,
# PICACHO_PINAL_SMTP.
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
rm -rf "$bundle/Contents/Helpers" "$bundle/Contents/PlugIns" "$bundle/Contents/Frameworks"
mkdir -p "$bundle/Contents/MacOS" "$bundle/Contents/Resources"
cp "target/$profile/picacho" "$bundle/Contents/MacOS/picacho"
cp packaging/Info.plist "$bundle/Contents/Info.plist"
if [ -f assets/AppIcon.icns ]; then
    cp assets/AppIcon.icns "$bundle/Contents/Resources/AppIcon.icns"
fi
# Ad-hoc signatures change identity on rebuild and can invalidate Keychain ACLs.
# A stable Apple Development/Developer ID identity preserves access across builds.
identity="${PICACHO_CODESIGN_IDENTITY:--}"
# macOS Notification Center & Desktop widgets (WidgetKit). They read the
# snapshot Picacho writes from an App Group container, which macOS grants
# only to apps signed by a team: the group is "<TEAM ID>.tech.pinal.picacho"
# (a team-prefixed group needs no provisioning profile, unlike "group.…").
# So they are built only with a signing identity (the team ID is taken from
# its name, "… (TEAMID)", or PICACHO_TEAM_ID), and with Xcode and XcodeGen.
team="${PICACHO_TEAM_ID:-}"
if [ -z "$team" ] && [ "$identity" != - ]; then
    team=$(printf '%s\n' "$identity" | sed -n 's/.*(\([A-Z0-9]\{10\}\)).*/\1/p')
fi
entitlements=""
widgets=no
if [ -n "$team" ] && [ "$identity" != - ] \
    && command -v xcodegen >/dev/null 2>&1 && command -v xcodebuild >/dev/null 2>&1; then
    group="$team.tech.pinal.picacho"
    mkdir -p target/macos-widgets
    (cd packaging/macos && xcodegen generate --quiet)
    widget_cfg=Release
    [ "$profile" = release ] || widget_cfg=Debug
    if xcodebuild -project packaging/macos/PicachoMacWidgets.xcodeproj \
        -scheme PicachoWidgets \
        -configuration "$widget_cfg" \
        -derivedDataPath target/macos-widgets \
        PICACHO_APP_GROUP="$group" \
        CODE_SIGNING_ALLOWED=NO \
        -quiet \
        build; then
        mkdir -p "$bundle/Contents/PlugIns" "$bundle/Contents/Frameworks"
        cp -R "target/macos-widgets/Build/Products/$widget_cfg/PicachoWidgets.appex" \
            "$bundle/Contents/PlugIns/PicachoWidgets.appex"
        # Loaded by Picacho itself (src/manage/widgets.rs) to reload the
        # widgets' timelines: WidgetKit reloads the calling app's widgets, so
        # it must run in Picacho's process, not a helper's.
        swiftc -O -emit-library -module-name PicachoWidgetReload \
            packaging/macos/reload-widgets.swift \
            -o "$bundle/Contents/Frameworks/libpicacho_widget_reload.dylib"
        # Where both find the snapshot.
        /usr/libexec/PlistBuddy -c "Add :PicachoAppGroup string $group" "$bundle/Contents/Info.plist"
        sed "s/\$(PICACHO_APP_GROUP)/$group/" packaging/macos/Picacho.entitlements \
            > target/macos-widgets/Picacho.entitlements
        sed "s/\$(PICACHO_APP_GROUP)/$group/" packaging/macos/PicachoWidgets.entitlements \
            > target/macos-widgets/PicachoWidgets.entitlements
        entitlements=target/macos-widgets/Picacho.entitlements
        widgets=yes
    fi
else
    echo "Leaving out the macOS widgets: they need PICACHO_CODESIGN_IDENTITY (a team's"
    echo "signing identity, or PICACHO_TEAM_ID), Xcode and XcodeGen."
fi
if [ "$identity" = - ]; then
    echo "Using ad-hoc signing; macOS may request Keychain access again after rebuilds."
    echo "Set PICACHO_CODESIGN_IDENTITY to a stable signing identity to preserve access."
    codesign --force --sign - "$bundle"
else
    if [ "$widgets" = yes ]; then
        codesign --force --options runtime --timestamp --sign "$identity" \
            "$bundle/Contents/Frameworks/libpicacho_widget_reload.dylib"
        codesign --force --options runtime --timestamp \
            --entitlements target/macos-widgets/PicachoWidgets.entitlements \
            --sign "$identity" "$bundle/Contents/PlugIns/PicachoWidgets.appex"
        codesign --force --options runtime --timestamp --entitlements "$entitlements" \
            --sign "$identity" "$bundle"
    else
        codesign --force --options runtime --timestamp --sign "$identity" "$bundle"
    fi
fi
codesign --verify --strict "$bundle"
echo "Built $bundle"

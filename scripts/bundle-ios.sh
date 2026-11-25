#!/bin/sh
# Builds Picacho for iPhone and iPad, and installs it on a connected device.
#
#   sh scripts/bundle-ios.sh [--debug] [--no-install] [--simulator]
#
# --simulator builds for the iOS Simulator (`rustup target add
# aarch64-apple-ios-sim`) and runs it there instead, booting
# PICACHO_IOS_SIMULATOR (default: the first available iPhone) if needed.
#
# Needs Xcode signed in to an Apple account (Settings > Accounts), XcodeGen
# (`brew install xcodegen`) and `rustup target add aarch64-apple-ios`. The
# device must be connected or paired, with Developer Mode on.
#
# Optional settings:
#   PICACHO_IOS_TEAM       signing team ID (default: the first team in Xcode)
#   PICACHO_IOS_BUNDLE_ID  bundle identifier (default: tech.pinal.picacho);
#                          a free Apple account may need one of its own
#   PICACHO_IOS_DEVICE     device name or UDID (default: the first iPhone or
#                          iPad that is connected)
set -eu
cd "$(dirname "$0")/.."

profile=release
install=1
simulator=0
for arg in "$@"; do
    case "$arg" in
        --debug) profile=debug ;;
        --no-install) install=0 ;;
        --simulator) simulator=1 ;;
        *) echo "usage: $0 [--debug] [--no-install] [--simulator]" >&2; exit 2 ;;
    esac
done

# Sign-in registrations are compiled in, as in scripts/bundle.sh.
if [ -f .env ]; then
    set -a
    . ./.env
    set +a
fi

export IPHONEOS_DEPLOYMENT_TARGET=17.0
if [ "$simulator" = 1 ]; then
    target=aarch64-apple-ios-sim
    sdk=iphonesimulator
    destination="generic/platform=iOS Simulator"
else
    target=aarch64-apple-ios
    sdk=iphoneos
    destination=generic/platform=iOS
fi
if [ "$profile" = release ]; then
    cargo rustc --lib --release --locked --target "$target" --crate-type staticlib
    configuration=Release
else
    cargo rustc --lib --locked --target "$target" --crate-type staticlib
    configuration=Debug
fi

team="${PICACHO_IOS_TEAM:-$(defaults read com.apple.dt.Xcode IDEProvisioningTeamByIdentifier 2>/dev/null \
    | sed -n 's/.*teamID = \([A-Z0-9]*\);.*/\1/p' | head -n 1)}"
if [ -z "$team" ]; then
    echo "No signing team: sign in to Xcode (Settings > Accounts) or set PICACHO_IOS_TEAM." >&2
    exit 1
fi
bundle_id="${PICACHO_IOS_BUNDLE_ID:-tech.pinal.picacho}"

(cd packaging/ios && xcodegen generate --quiet)
xcodebuild \
    -project packaging/ios/Picacho.xcodeproj \
    -scheme Picacho \
    -configuration "$configuration" \
    -destination "$destination" \
    -derivedDataPath target/ios-build \
    -allowProvisioningUpdates \
    -quiet \
    DEVELOPMENT_TEAM="$team" \
    PICACHO_BUNDLE_ID="$bundle_id" \
    RUST_PROFILE="$profile" \
    build
app="target/ios-build/Build/Products/$configuration-$sdk/Picacho.app"
echo "Built $app"
[ "$install" = 1 ] || exit 0

if [ "$simulator" = 1 ]; then
    sim="${PICACHO_IOS_SIMULATOR:-$(xcrun simctl list devices available | sed -n 's/^ *\(iPhone[^(]*\) (\([0-9A-F-]*\)).*/\2/p' | head -n 1)}"
    if [ -z "$sim" ]; then
        echo "No iPhone simulator; install one in Xcode > Settings > Components." >&2
        exit 1
    fi
    xcrun simctl boot "$sim" 2>/dev/null || true
    open "$(xcode-select -p)/Applications/Simulator.app" || true
    xcrun simctl install "$sim" "$app"
    xcrun simctl launch --terminate-running-process "$sim" "$bundle_id"
    exit 0
fi

device="${PICACHO_IOS_DEVICE:-}"
if [ -z "$device" ]; then
    devices="$(mktemp)"
    xcrun devicectl list devices --json-output "$devices" >/dev/null
    device="$(python3 -I - "$devices" <<'EOF'
import json, sys
for d in json.load(open(sys.argv[1]))["result"]["devices"]:
    hardware = d.get("hardwareProperties", {})
    if hardware.get("deviceType") in ("iPhone", "iPad") and d.get("connectionProperties", {}).get("tunnelState") != "unavailable":
        print(hardware.get("udid") or d["identifier"])
        break
EOF
)"
    rm -f "$devices"
fi
if [ -z "$device" ]; then
    echo "No iPhone or iPad connected; connect one or set PICACHO_IOS_DEVICE." >&2
    exit 1
fi
xcrun devicectl device install app --device "$device" "$app"
xcrun devicectl device process launch --device "$device" --terminate-existing "$bundle_id"

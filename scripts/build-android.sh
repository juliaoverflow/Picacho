#!/bin/sh
# Builds Picacho's release APKs (one per ABI) into android/app/build/outputs/apk/release.
#
# Needs the Android SDK (ANDROID_HOME), the NDK (ANDROID_NDK_HOME), cargo-ndk,
# a JDK 17 or later, and the Rust targets for each ABI:
#   rustup target add aarch64-linux-android x86_64-linux-android
#   cargo install cargo-ndk
#
# PICACHO_ABIS picks the ABIs (default: arm64-v8a,x86_64). Release signing
# reads PICACHO_KEYSTORE, PICACHO_KEYSTORE_PASSWORD, PICACHO_KEY_ALIAS and
# PICACHO_KEY_PASSWORD; without them the APKs are signed with the debug key.
set -eu
cd "$(dirname "$0")/.."
abis="${PICACHO_ABIS:-arm64-v8a,x86_64}"
# Where Android Studio puts the SDK and NDK, when they aren't set.
if [ -z "${ANDROID_HOME:-}" ]; then
    for sdk in "$HOME/Android/Sdk" "$HOME/Library/Android/sdk"; do
        if [ -d "$sdk" ]; then
            export ANDROID_HOME="$sdk"
            break
        fi
    done
fi
if [ -z "${ANDROID_NDK_HOME:-}" ] && [ -n "${ANDROID_HOME:-}" ] && [ -d "$ANDROID_HOME/ndk" ]; then
    # The newest NDK installed.
    ndk=$(ls -1 "$ANDROID_HOME/ndk" | sort -V | tail -n 1)
    if [ -n "$ndk" ]; then
        export ANDROID_NDK_HOME="$ANDROID_HOME/ndk/$ndk"
    fi
fi
if [ -f .env ]; then
    set -a
    . ./.env
    set +a
fi
targets=""
for abi in $(echo "$abis" | tr ',' ' '); do
    targets="$targets -t $abi"
done
# shellcheck disable=SC2086
cargo ndk $targets -P 26 -o android/app/src/main/jniLibs build --release --lib --locked
cd android
./gradlew --no-daemon assembleRelease -Ppicacho.abis="$abis"
ls -1 app/build/outputs/apk/release/*.apk

#!/bin/sh
# Adds the given APKs to Picacho's F-Droid repository on R2 (bucket
# android-repository, served at https://android.repo.pinal.tech) and re-signs
# its index. Run from the repository root in CI.
#
#   scripts/release/publish-fdroid.sh stable X.Y.Z APK...    a release
#   scripts/release/publish-fdroid.sh beta BUILD APK...      the rolling beta
#
# Needs rclone, fdroidserver, rsvg-convert, the Android SDK (ANDROID_HOME, for
# apksigner), the R2 variables of scripts/release/r2-env.sh, and
# FDROID_KEYSTORE_BASE64 / FDROID_KEYSTORE_PASSWORD.
set -eu
channel=$1
version=$2
shift 2
. scripts/release/r2-env.sh
bucket=android-repository

cd fdroid
rm -rf repo archive
mkdir -p repo archive
rclone copy r2:$bucket/repo repo
rclone copy r2:$bucket/archive archive
for apk in "$@"; do
    cp "../$apk" repo/
done
# The signing key: readable by this user alone from the moment it exists,
# and removed however the script ends.
trap 'rm -f "$PWD/fdroid-repo.jks"' EXIT
(umask 077 && printf '%s' "$FDROID_KEYSTORE_BASE64" | base64 -d > fdroid-repo.jks)
chmod 600 config.yml fdroid-repo.jks
rsvg-convert -w 512 -h 512 ../packaging/linux/tech.pinal.picacho.svg -o icon.png
for app in tech.pinal.picacho tech.pinal.picacho.beta; do
    shots=metadata/$app/en-US/tenInchScreenshots
    cp icon.png metadata/$app/en-US/icon.png
    mkdir -p $shots
    cp ../docs/screenshots/inbox-light.png $shots/1-inbox-light.png
    cp ../docs/screenshots/inbox-dark.png $shots/2-inbox-dark.png
    cp ../docs/screenshots/compose.png $shots/3-compose.png
done
cd ..
python3 scripts/release/fdroid-metadata.py "$channel" "$version"

cd fdroid
fdroid update --verbose --pretty
rm -f fdroid-repo.jks
cd ..
r2_publish fdroid/archive $bucket/archive 'index*' 'entry*' 'diff/**'
r2_publish fdroid/repo $bucket/repo 'index*' 'entry*' 'diff/**'

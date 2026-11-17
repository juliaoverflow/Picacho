#!/bin/sh
# Adds the given APKs to Picacho's F-Droid repository on R2 (bucket
# android-repository, served at https://android.repo.pinal.tech) and re-signs
# its index. Run from the repository root in CI.
#
#   scripts/release/publish-fdroid.sh VERSION APK...
#
# Needs rclone, fdroidserver, rsvg-convert, the Android SDK (ANDROID_HOME, for
# apksigner), the R2 variables of scripts/release/r2-env.sh, and
# FDROID_KEYSTORE_BASE64 / FDROID_KEYSTORE_PASSWORD.
set -eu
version=$1
shift
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
printf '%s' "$FDROID_KEYSTORE_BASE64" | base64 -d > fdroid-repo.jks
chmod 600 config.yml fdroid-repo.jks
rsvg-convert -w 512 -h 512 ../packaging/linux/tech.pinal.picacho.svg -o icon.png
cp icon.png metadata/tech.pinal.picacho/en-US/icon.png
mkdir -p metadata/tech.pinal.picacho/en-US/tenInchScreenshots
cp ../docs/screenshots/inbox-light.png metadata/tech.pinal.picacho/en-US/tenInchScreenshots/1-inbox-light.png
cp ../docs/screenshots/inbox-dark.png metadata/tech.pinal.picacho/en-US/tenInchScreenshots/2-inbox-dark.png
cp ../docs/screenshots/compose.png metadata/tech.pinal.picacho/en-US/tenInchScreenshots/3-compose.png
cd ..
python3 scripts/release/fdroid-metadata.py "$version"

cd fdroid
fdroid update --verbose --pretty
rm -f fdroid-repo.jks
cd ..
r2_publish fdroid/archive $bucket/archive 'index*' 'entry*' 'diff/**'
r2_publish fdroid/repo $bucket/repo 'index*' 'entry*' 'diff/**'

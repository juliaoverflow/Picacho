#!/bin/sh
# Renders the Android launcher icon layers (android/icon/*.svg) into the
# app's mipmap folders. Needs rsvg-convert (librsvg).
set -eu
cd "$(dirname "$0")/../android"
for density in mdpi:108 hdpi:162 xhdpi:216 xxhdpi:324 xxxhdpi:432; do
    name=${density%%:*}
    size=${density##*:}
    mkdir -p "app/src/main/res/mipmap-$name"
    for layer in foreground monochrome; do
        rsvg-convert -w "$size" -h "$size" "icon/$layer.svg" \
            -o "app/src/main/res/mipmap-$name/ic_launcher_$layer.png"
    done
done

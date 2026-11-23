#!/bin/sh
# Adds built Flatpak commits to Picacho's Flatpak repository on R2 (bucket
# linux-repository, served at https://linux.repo.pinal.tech/repo/), signs them,
# and regenerates the signed summary and static deltas. Run from the
# repository root in CI.
#
#   scripts/release/publish-flatpak.sh BUILD_REPO...
#
# Each BUILD_REPO is an OSTree repository from flatpak-builder --repo (one per
# architecture). Needs flatpak, ostree, rclone, gpg, the R2 variables of
# scripts/release/r2-env.sh, FLATPAK_GPG_PRIVATE_KEY and FLATPAK_GPG_KEY_ID.
set -eu
. scripts/release/r2-env.sh
bucket=linux-repository
work=build/flatpak-publish
rm -rf "$work"
mkdir -p "$work"

export GNUPGHOME="$PWD/$work/gnupg"
mkdir -m 700 "$GNUPGHOME"
# The imported private key goes however the script ends.
trap 'gpgconf --kill gpg-agent 2>/dev/null; rm -rf "$GNUPGHOME"' EXIT
printf '%s\n' "$FLATPAK_GPG_PRIVATE_KEY" | gpg --batch --quiet --import
sign="--gpg-sign=$FLATPAK_GPG_KEY_ID --gpg-homedir=$GNUPGHOME"

repo="$work/repo"
rclone copy r2:$bucket/repo "$repo"
if [ ! -f "$repo/config" ]; then
    ostree init --mode=archive-z2 --repo="$repo"
fi
# R2 keeps no empty directories, so the downloaded repository lacks the ones
# OSTree expects (refs/remotes and the like) until something is in them.
mkdir -p "$repo/objects" "$repo/refs/heads" "$repo/refs/mirrors" "$repo/refs/remotes" \
    "$repo/state" "$repo/tmp" "$repo/extensions"

for build in "$@"; do
    # Sign each new commit in its build repository, then bring it over.
    for ref in $(ostree --repo="$build" refs); do
        ostree --repo="$build" gpg-sign --gpg-homedir="$GNUPGHOME" \
            "$(ostree --repo="$build" rev-parse "$ref")" "$FLATPAK_GPG_KEY_ID"
    done
    ostree --repo="$repo" pull-local "$build"
done

# shellcheck disable=SC2086
flatpak build-update-repo $sign \
    --title=Picacho \
    --default-branch=stable \
    --generate-static-deltas \
    --prune --prune-depth=5 \
    "$repo"

r2_publish "$repo" $bucket/repo 'summary*' 'summaries/**' 'config'

# The repository and install files, the icon and the metainfo screenshots.
site="$work/site"
mkdir -p "$site/screenshots"
cp packaging/flatpak/picacho.flatpakrepo packaging/flatpak/tech.pinal.picacho.*.flatpakref "$site/"
cp packaging/linux/tech.pinal.picacho.svg "$site/"
cp docs/screenshots/*.png "$site/screenshots/"
rclone copy --checksum "$site" r2:$bucket

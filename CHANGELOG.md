# Changelog

All notable changes to Picacho are recorded here. Each release's section is
its release notes: on GitHub, in the F-Droid repository and in the Flatpak's
metainfo. Versions follow [Semantic Versioning](https://semver.org). The
rolling beta has no version or section of its own.

Add changes under **Unreleased**; in the pull request that releases `X.Y.Z`
into the stable branch, rename it to `## [X.Y.Z] - YYYY-MM-DD` and set the
same version in `Cargo.toml`.

## [Unreleased]

### Added

- Pasting copied rich text into a message pastes it as plain text, with a
  “Paste with formatting” button to keep its bold, links and lists instead.
  Turn on Settings → Composing → Paste with formatting to make that the
  default, with “Paste without formatting” offered instead.

### Fixed

- Selecting text in an email starts and ends under the pointer. In columns
  and centered layouts it ran well to the right of it, and in vertically
  centered cells a line away; links there are clicked where they are shown.
- Phones and tablets: text in an email can be selected and copied. A long
  press selects a word, moving the finger before lifting it selects more,
  and lifting it offers Copy, with the link or image pressed.

## [0.1.0] - 2026-11-16

### Added

- First release for Android (APK, through Picacho's F-Droid repository) and
  Linux (Flatpak, through Picacho's Flatpak repository).

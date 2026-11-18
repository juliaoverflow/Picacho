#!/usr/bin/env python3
"""Prepares fdroid/ before `fdroid update` (run from the repository root).

  fdroid-metadata.py stable X.Y.Z     a release of tech.pinal.picacho
  fdroid-metadata.py beta BUILD       a rolling beta of tech.pinal.picacho.beta

Writes a changelog for every release version code in CHANGELOG.md, and sets
the published app's CurrentVersionCode to its newest build among the APKs
already in fdroid/repo and fdroid/archive plus the new ones.

The version codes follow android/app/build.gradle.kts: a release's comes from
its version, a beta's from the CI run number.
"""
import json
import re
import sys
import zipfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
import changelog  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
FDROID = ROOT / "fdroid"
APP = "tech.pinal.picacho"
APPS = {"stable": APP, "beta": APP + ".beta"}
ABI_DIGITS = {"armeabi-v7a": 1, "arm64-v8a": 2, "x86": 3, "x86_64": 4}


def base_code(version):
    match = re.match(r"^(\d+)\.(\d+)\.(\d+)(?:-[A-Za-z]+\.?(\d+))?", version)
    if not match:
        sys.exit(f"Cannot parse the version {version}")
    major, minor, patch, pre = match.groups()
    slot = 99 if pre is None else min(max(int(pre), 1), 98)
    return ((int(major) * 100 + int(minor)) * 100 + int(patch)) * 100 + slot


def published(app):
    """Version codes of the app's APKs in the downloaded repository."""
    found = []
    for index in (FDROID / "repo" / "index-v2.json", FDROID / "archive" / "index-v2.json"):
        if not index.exists():
            continue
        package = json.loads(index.read_text()).get("packages", {}).get(app, {})
        for entry in package.get("versions", {}).values():
            found.append(entry["manifest"]["versionCode"])
    return found


def main():
    channel, version = sys.argv[1], sys.argv[2]
    if channel not in APPS:
        sys.exit(f"Unknown channel {channel}")
    app = APPS[channel]
    base = base_code(version) if channel == "stable" else int(version)
    codes = published(app) + [base * 10 + digit for digit in ABI_DIGITS.values()]

    changelogs = FDROID / "metadata" / APP / "en-US" / "changelogs"
    changelogs.mkdir(parents=True, exist_ok=True)
    for released, _, _ in changelog.sections():
        text = changelog.plain(changelog.section(released)[1])
        for digit in ABI_DIGITS.values():
            (changelogs / f"{base_code(released) * 10 + digit}.txt").write_text(text[:500])

    metadata = FDROID / "metadata" / f"{app}.yml"
    lines = [l for l in metadata.read_text().splitlines() if not l.startswith("CurrentVersion")]
    lines.append(f"CurrentVersionCode: {max(codes)}")
    metadata.write_text("\n".join(lines) + "\n")
    print(f"{app} CurrentVersionCode: {max(codes)}")


if __name__ == "__main__":
    main()

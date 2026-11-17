#!/usr/bin/env python3
"""Prepares fdroid/ before `fdroid update` (run from the repository root).

Writes a changelog for every version code in CHANGELOG.md, and sets the app's
CurrentVersionCode to the newest release (no pre-release suffix) among the
APKs already in fdroid/repo and fdroid/archive plus the new ones, so F-Droid
clients offer betas only to people who allow unstable updates.

The version codes follow android/app/build.gradle.kts.
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
ABI_DIGITS = {"armeabi-v7a": 1, "arm64-v8a": 2, "x86": 3, "x86_64": 4}


def base_code(version):
    match = re.match(r"^(\d+)\.(\d+)\.(\d+)(?:-[A-Za-z]+\.?(\d+))?", version)
    if not match:
        sys.exit(f"Cannot parse the version {version}")
    major, minor, patch, pre = match.groups()
    slot = 99 if pre is None else min(max(int(pre), 1), 98)
    return ((int(major) * 100 + int(minor)) * 100 + int(patch)) * 100 + slot


def published():
    """(versionCode, versionName) of every APK in the downloaded repository."""
    found = []
    for index in (FDROID / "repo" / "index-v2.json", FDROID / "archive" / "index-v2.json"):
        if not index.exists():
            continue
        package = json.loads(index.read_text()).get("packages", {}).get(APP, {})
        for entry in package.get("versions", {}).values():
            manifest = entry["manifest"]
            found.append((manifest["versionCode"], manifest["versionName"]))
    return found


def main():
    version = sys.argv[1]
    codes = [(base_code(version) * 10 + digit, version) for digit in ABI_DIGITS.values()]
    stable = [code for code, name in published() + codes if "-" not in name]

    changelogs = FDROID / "metadata" / APP / "en-US" / "changelogs"
    changelogs.mkdir(parents=True, exist_ok=True)
    for released, _, _ in changelog.sections():
        text = changelog.plain(changelog.section(released)[1])
        for digit in ABI_DIGITS.values():
            (changelogs / f"{base_code(released) * 10 + digit}.txt").write_text(text[:500])

    metadata = FDROID / "metadata" / f"{APP}.yml"
    lines = [l for l in metadata.read_text().splitlines() if not l.startswith("CurrentVersion")]
    if stable:
        lines.append(f"CurrentVersionCode: {max(stable)}")
    metadata.write_text("\n".join(lines) + "\n")
    print(f"CurrentVersionCode: {max(stable) if stable else 'none (no release yet)'}")


if __name__ == "__main__":
    main()

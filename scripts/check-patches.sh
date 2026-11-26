#!/bin/sh
# Fails when Cargo.toml takes a crate from a folder outside this repository
# (`path = "../…"`, or an absolute path). CI checks out this repository
# alone, so such a build can't be reproduced there, and a release built
# from it would come from an unpublished local tree. Patched crates come
# from git, pinned (see [patch] in Cargo.toml).
set -eu
cd "$(dirname "$0")/.."
found=$(grep -nE 'path[[:space:]]*=[[:space:]]*"(\.\./|/|~)' Cargo.toml || true)
if [ -n "$found" ]; then
    echo "Cargo.toml takes crates from outside the repository:" >&2
    echo "$found" >&2
    echo "Point them at a published git commit instead." >&2
    exit 1
fi
# And the lockfile names no crate without a source but this one.
outside=$(awk '
    /^\[\[package\]\]/ { if (name != "" && !source) print name; name = ""; source = 0 }
    /^name = / { name = $3 }
    /^source = / { source = 1 }
    END { if (name != "" && !source) print name }
' Cargo.lock | tr -d '"' | grep -vx 'picacho' || true)
if [ -n "$outside" ]; then
    echo "Cargo.lock has crates with no source (local folders):" >&2
    echo "$outside" >&2
    exit 1
fi

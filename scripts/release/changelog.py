#!/usr/bin/env python3
"""Release notes from CHANGELOG.md.

  changelog.py notes VERSION                 the version's section, as Markdown
  changelog.py text VERSION                  the same as plain text (F-Droid)
  changelog.py metainfo IN OUT               IN with <releases/> filled from every released version
  changelog.py check VERSION                 fails unless CHANGELOG.md has a section for VERSION
"""
import html
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
HEADING = re.compile(r"^## \[(?P<version>[^\]]+)\](?: - (?P<date>\d{4}-\d{2}-\d{2}))?\s*$")


def sections():
    """(version, date, body lines) for each released version, newest first."""
    out, current = [], None
    for line in (ROOT / "CHANGELOG.md").read_text().splitlines():
        match = HEADING.match(line)
        if match:
            current = None
            if match["version"].lower() != "unreleased":
                current = (match["version"], match["date"], [])
                out.append(current)
        elif line.startswith("## "):
            current = None
        elif current is not None:
            current[2].append(line)
    return out


def section(version):
    for found, date, body in sections():
        if found == version:
            return date, "\n".join(body).strip()
    sys.exit(f"CHANGELOG.md has no section for {version}")


def plain(markdown):
    lines = []
    for line in markdown.splitlines():
        line = re.sub(r"^#+\s*", "", line)
        line = re.sub(r"\[([^\]]+)\]\([^)]+\)", r"\1", line)
        line = line.replace("**", "").replace("`", "")
        lines.append(line)
    return "\n".join(lines).strip() + "\n"


def metainfo_releases(indent="  "):
    """<releases> for the metainfo: headings become paragraphs, items lists."""
    rows = [f"{indent}<releases>"]
    for version, date, body in sections():
        kind = ' type="development"' if "-" in version else ""
        rows.append(f'{indent}  <release version="{version}" date="{date}"{kind}>')
        rows.append(f"{indent}    <description>")
        items = []

        def flush():
            if items:
                rows.append(f"{indent}      <ul>")
                rows.extend(f"{indent}        <li>{html.escape(item, quote=False)}</li>" for item in items)
                rows.append(f"{indent}      </ul>")
                items.clear()

        for line in body:
            text = plain(line).strip()
            if line.startswith("- "):
                items.append(text[2:].strip())
            elif line.startswith("  ") and items:
                items[-1] += " " + text
            elif text:
                flush()
                rows.append(f"{indent}      <p>{html.escape(text, quote=False)}</p>")
        flush()
        rows.append(f"{indent}    </description>")
        rows.append(f"{indent}  </release>")
    rows.append(f"{indent}</releases>")
    return "\n".join(rows)


def main(argv):
    if len(argv) == 3 and argv[1] == "notes":
        print(section(argv[2])[1])
    elif len(argv) == 3 and argv[1] == "text":
        sys.stdout.write(plain(section(argv[2])[1]))
    elif len(argv) == 3 and argv[1] == "check":
        date, _ = section(argv[2])
        if not date:
            sys.exit(f"CHANGELOG.md's section for {argv[2]} has no date")
    elif len(argv) == 4 and argv[1] == "metainfo":
        source = Path(argv[2]).read_text()
        filled = re.sub(r"^\s*<releases\s*/>", lambda _: metainfo_releases(), source, count=1, flags=re.M)
        out = Path(argv[3])
        out.parent.mkdir(parents=True, exist_ok=True)
        out.write_text(filled)
    else:
        sys.exit(__doc__)


if __name__ == "__main__":
    main(sys.argv)

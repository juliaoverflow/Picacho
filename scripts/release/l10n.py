#!/usr/bin/env python3
"""Writes Picacho's translations into the Linux desktop entry and the app
store listing (run from the repository root).

  l10n.py desktop IN OUT     IN with GenericName[xx], Comment[xx] and
                             Keywords[xx] from locales/xx/desktop.ftl
  l10n.py metainfo IN OUT    IN with its summary, description and captions
                             in each language (xml:lang) from
                             locales/xx/store/listing.ftl

English stays as IN has it. A language lacking a message is left out of
that key. The messages read are plain text (no { } placeholders).
"""
import re
import sys
from pathlib import Path
from xml.sax.saxutils import escape

ROOT = Path(__file__).resolve().parents[2]
LOCALES = ROOT / "locales"


def messages(path):
    """A Fluent file's plain messages: id -> one-line text."""
    if not path.is_file():
        return {}
    out, current = {}, None
    for line in path.read_text(encoding="utf-8").splitlines():
        if line.startswith("#") or not line.strip():
            current = None
            continue
        m = re.match(r"([a-z0-9-]+) =\s?(.*)$", line)
        if m:
            current = m.group(1)
            out[current] = m.group(2).strip()
        elif current and line[:1].isspace():
            out[current] = (out[current] + " " + line.strip()).strip()
    for key, text in out.items():
        if "{" in text:
            raise SystemExit(f"{path}: {key} has a placeable; write it as plain text")
    return out


def languages():
    return sorted(d.name for d in LOCALES.iterdir() if d.is_dir() and d.name != "en")


def desktop(source):
    keys = {"GenericName": "desktop-generic-name", "Comment": "desktop-comment", "Keywords": "desktop-keywords"}
    lines = source.splitlines()
    for lang in languages():
        words = messages(LOCALES / lang / "desktop.ftl")
        locale = lang.replace("-", "_")
        for key, mid in keys.items():
            if mid not in words:
                continue
            at = next(i for i, l in enumerate(lines) if l.startswith(key + "="))
            # After the key's English and the languages added before.
            while at + 1 < len(lines) and lines[at + 1].startswith(key + "["):
                at += 1
            lines.insert(at + 1, f"{key}[{locale}]={words[mid]}")
    return "\n".join(lines) + "\n"


def metainfo(source):
    for lang in languages():
        words = messages(LOCALES / lang / "store" / "listing.ftl")
        attr = f' xml:lang="{lang}"'

        def after(pattern, element, mid, text):
            nonlocal source
            if mid not in words:
                return
            m = re.search(pattern, source, re.S)
            if not m:
                raise SystemExit(f"metainfo: no {element} for {mid}")
            indent = re.search(r"([ \t]*)\S", source[source.rfind("\n", 0, m.start()) + 1:]).group(1)
            added = f"\n{indent}<{element}{attr}>{escape(words[mid])}</{element}>"
            source = source[: m.end()] + added + source[m.end():]

        after(r"<summary>[^<]*</summary>(?:\s*<summary xml:lang=\"[^\"]+\">[^<]*</summary>)*", "summary", "listing-summary", None)
        after(r"<description>\s*<p>[^<]*</p>(?:\s*<p xml:lang=\"[^\"]+\">[^<]*</p>)*", "p", "listing-description", None)
        for image, mid in (
            ("inbox-light", "listing-caption-inbox-light"),
            ("inbox-dark", "listing-caption-inbox-dark"),
            ("compose", "listing-caption-compose"),
        ):
            after(
                r"<caption>[^<]*</caption>(?:\s*<caption xml:lang=\"[^\"]+\">[^<]*</caption>)*(?=\s*<image>[^<]*/" + image + r"\.png)",
                "caption",
                mid,
                None,
            )
    return source


def main(argv):
    if len(argv) != 4 or argv[1] not in ("desktop", "metainfo"):
        raise SystemExit(__doc__)
    source = Path(argv[2]).read_text(encoding="utf-8")
    out = desktop(source) if argv[1] == "desktop" else metainfo(source)
    Path(argv[3]).parent.mkdir(parents=True, exist_ok=True)
    Path(argv[3]).write_text(out, encoding="utf-8")


if __name__ == "__main__":
    main(sys.argv)

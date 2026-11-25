#!/usr/bin/env python3
# SPDX-FileCopyrightText: 2026 Gundu Labs
# SPDX-License-Identifier: GPL-3.0-or-later

"""Copy packaging/po into the desktop, metainfo, polkit and KCM metadata; --check only verifies."""

import ast
import json
import re
import sys
from pathlib import Path
from xml.sax.saxutils import escape, unescape

ROOT = Path(__file__).resolve().parent.parent
KEYWORDS = "face;auth;login;biometric;"
DESKTOP = [
    "packaging/gui/com.gundulabs.Gaze.desktop",
    "integrations/kde/gaze-face-unlock.desktop",
    "integrations/kde/kcm/com.gundulabs.Gaze.Settings.desktop",
]
XML = {
    "packaging/config/com.gundulabs.gaze.policy": ("description", "message"),
    "packaging/gui/com.gundulabs.Gaze.metainfo.xml": ("summary", "p"),
}
KCM_JSON = "integrations/kde/kcm/kcm_gaze.json"
CATALOGS = [
    "crates/gaze-gui/po/*.po",
    "integrations/kde/kcm/po/*/*.po",
    "integrations/gnome-shell/po/*.po",
    "integrations/cinnamon/po/*.po",
    "packaging/po/*.po",
]


def parse_po(path):
    """Yield (source forms, translated forms) for every translated, non-fuzzy entry."""
    for block in path.read_text(encoding="utf-8").split("\n\n"):
        fields, name = {}, None
        for line in block.splitlines():
            if match := re.match(r'(msgid(?:_plural)?|msgstr(?:\[\d\])?) (".*")$', line):
                name = match[1]
                fields[name] = ast.literal_eval(match[2])
            elif line.startswith('"') and name:
                fields[name] += ast.literal_eval(line)
        fuzzy = re.search(r"^#,.*\bfuzzy\b", block, re.M)
        translated = [v for k, v in fields.items() if k.startswith("msgstr") and v]
        if fields.get("msgid") and translated and not fuzzy:
            yield [v for k, v in fields.items() if k.startswith("msgid")], translated


def placeholders(text):
    return set(re.findall(r"\{\w+\}|%\d", text))


def catalog_errors(path):
    for sources, translated in parse_po(path):
        allowed = set().union(*map(placeholders, sources))
        for text in translated:
            if placeholders(text) != allowed:
                yield f"{path.relative_to(ROOT)}: placeholders differ for {sources[0]!r}"
            if text.endswith("\n") != sources[0].endswith("\n"):
                yield f"{path.relative_to(ROOT)}: trailing newline differs for {sources[0]!r}"


def sync_desktop(text, catalogs):
    out = []
    for line in text.splitlines():
        if re.match(r"(Name|Comment|Keywords)\[", line):
            continue
        out.append(line)
        key, _, value = line.partition("=")
        if key not in ("Name", "Comment", "Keywords"):
            continue
        source = KEYWORDS if key == "Keywords" else value
        out += [f"{key}[{lang}]={c[source]}" for lang, c in catalogs.items() if source in c]
    return "\n".join(out) + "\n"


def sync_xml(text, catalogs, tags):
    names = "|".join(tags)
    text = re.sub(rf"^ *<(?:{names}) xml:lang=.*\n", "", text, flags=re.M)

    def expand(match):
        indent, tag, body = match.groups()
        source = unescape(body)
        return match[0] + "".join(
            f'{indent}<{tag} xml:lang="{lang}">{escape(c[source])}</{tag}>\n'
            for lang, c in catalogs.items()
            if source in c
        )

    return re.sub(rf"^( *)<({names})>(.*)</\2>\n", expand, text, flags=re.M)


def sync_json(text, catalogs):
    data = json.loads(text)
    plugin = {k: v for k, v in data.pop("KPlugin").items() if "[" not in k}
    data = {k: v for k, v in data.items() if "[" not in k}
    for lang, c in catalogs.items():
        for key in ("Description", "Name"):
            if plugin[key] in c:
                plugin[f"{key}[{lang}]"] = c[plugin[key]]
        if KEYWORDS in c:
            data[f"X-KDE-Keywords[{lang}]"] = ",".join(filter(None, c[KEYWORDS].split(";")))
    data = {"KPlugin": dict(sorted(plugin.items())), **dict(sorted(data.items()))}
    return json.dumps(data, ensure_ascii=False, indent=4) + "\n"


def main():
    check = sys.argv[1:] == ["--check"]
    if sys.argv[1:] and not check:
        sys.exit(f"usage: {sys.argv[0]} [--check]")

    errors = [e for glob in CATALOGS for po in sorted(ROOT.glob(glob)) for e in catalog_errors(po)]
    catalogs = {
        po.stem: {sources[0]: translated[0] for sources, translated in parse_po(po)}
        for po in sorted((ROOT / "packaging/po").glob("*.po"))
    }

    synced = {p: sync_desktop((ROOT / p).read_text(encoding="utf-8"), catalogs) for p in DESKTOP}
    synced |= {p: sync_xml((ROOT / p).read_text(encoding="utf-8"), catalogs, t) for p, t in XML.items()}
    synced[KCM_JSON] = sync_json((ROOT / KCM_JSON).read_text(encoding="utf-8"), catalogs)

    for name, text in synced.items():
        path = ROOT / name
        if path.read_text(encoding="utf-8") == text:
            continue
        if check:
            errors.append(f"{name}: out of date, run just sync-translations")
        else:
            path.write_text(text, encoding="utf-8")
            print(f"updated {name}")

    if errors:
        sys.exit("\n".join(errors))


if __name__ == "__main__":
    main()

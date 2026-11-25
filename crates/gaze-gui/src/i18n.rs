// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

//! Interface translations for `gaze-gui`.
//!
//! Catalogs are standard gettext `.po` files under `crates/gaze-gui/po/`, embedded into the binary
//! at build time, so packages need no locale files and every packaging format ships the same
//! translations. The language follows the usual gettext variables (`LANGUAGE`, `LC_ALL`,
//! `LC_MESSAGES`, `LANG`); `GAZE_GUI_LANG` overrides them for this app only. Strings without a
//! translation, and unknown languages, fall back to English.

use std::collections::HashMap;
use std::sync::OnceLock;

/// Embedded catalogs, keyed by POSIX locale name (`ll` or `ll_CC`).
const CATALOGS: &[(&str, &str)] = &[
    ("cs", include_str!("../po/cs.po")),
    ("de", include_str!("../po/de.po")),
    ("es", include_str!("../po/es.po")),
    ("fr", include_str!("../po/fr.po")),
    ("id", include_str!("../po/id.po")),
    ("it", include_str!("../po/it.po")),
    ("ja", include_str!("../po/ja.po")),
    ("ko", include_str!("../po/ko.po")),
    ("pl", include_str!("../po/pl.po")),
    ("pt_BR", include_str!("../po/pt_BR.po")),
    ("ru", include_str!("../po/ru.po")),
    ("tr", include_str!("../po/tr.po")),
    ("vi", include_str!("../po/vi.po")),
    ("zh_CN", include_str!("../po/zh_CN.po")),
    ("zh_TW", include_str!("../po/zh_TW.po")),
];

type Catalog = HashMap<String, String>;

fn active_catalog() -> Option<&'static Catalog> {
    static ACTIVE: OnceLock<Option<Catalog>> = OnceLock::new();
    ACTIVE
        .get_or_init(|| {
            // Unit tests assert on the English source strings whatever the developer's locale.
            if cfg!(test) {
                return None;
            }
            let wanted = requested_locales(|name| std::env::var(name).ok());
            pick_catalog(&wanted).map(parse_po)
        })
        .as_ref()
}

/// Translates `msgid`, or returns it unchanged when the active language has no translation.
///
/// Literal arguments are what `xgettext --keyword=tr --keyword=trf` extracts; runtime strings,
/// such as enrollment prompts from `gaze-core`, also work but must be added to the catalogs by hand.
pub fn tr(msgid: &str) -> &str {
    active_catalog()
        .and_then(|catalog| catalog.get(msgid))
        .map_or(msgid, String::as_str)
}

/// Translates `msgid` and replaces each `{name}` placeholder with its value.
///
/// Placeholders are named so that translations can reorder them.
pub fn trf(msgid: &str, args: &[(&str, &str)]) -> String {
    fill(tr(msgid), args)
}

/// Translates each label of a fixed option list, keeping its order.
pub fn tr_all<'a>(labels: &[&'a str]) -> Vec<&'a str> {
    labels.iter().map(|&label| tr(label)).collect()
}

fn fill(template: &str, args: &[(&str, &str)]) -> String {
    let mut out = template.to_string();
    for (name, value) in args {
        out = out.replace(&format!("{{{name}}}"), value);
    }
    out
}

/// Returns locale names in priority order, following gettext's lookup rules: `LANGUAGE` is a
/// colon-separated list that applies only when the message locale is not `C`/`POSIX`.
fn requested_locales(var: impl Fn(&str) -> Option<String>) -> Vec<String> {
    let non_empty = |name: &str| var(name).filter(|value| !value.trim().is_empty());

    if let Some(own) = non_empty("GAZE_GUI_LANG") {
        return own.split(':').map(str::to_string).collect();
    }

    let message_locale = non_empty("LC_ALL")
        .or_else(|| non_empty("LC_MESSAGES"))
        .or_else(|| non_empty("LANG"));
    let is_c = |locale: &str| {
        let base = locale.split(['.', '@']).next().unwrap_or_default();
        base == "C" || base == "POSIX"
    };
    if message_locale.as_deref().is_none_or(is_c) {
        return Vec::new();
    }

    let mut wanted = Vec::new();
    if let Some(list) = non_empty("LANGUAGE") {
        wanted.extend(
            list.split(':')
                .filter(|l| !l.is_empty())
                .map(str::to_string),
        );
    }
    wanted.extend(message_locale);
    wanted
}

/// Picks the first catalog matching a requested locale, trying `ll_CC` and then `ll`. Chinese
/// locales match by script: Traditional uses `zh_TW`, everything else `zh_CN`.
fn pick_catalog(wanted: &[String]) -> Option<&'static str> {
    for locale in wanted {
        let base = locale.split(['.', '@']).next().unwrap_or_default();
        if base.is_empty() || base == "en" || base.starts_with("en_") {
            return None;
        }
        if let Some((_, po)) = CATALOGS.iter().find(|(name, _)| *name == base) {
            return Some(po);
        }
        let language = match base {
            "zh_HK" | "zh_MO" | "zh_Hant" | "zh_Hant_TW" | "zh_Hant_HK" => "zh_TW",
            _ => base.split('_').next().unwrap_or_default(),
        };
        if let Some((_, po)) = CATALOGS
            .iter()
            .find(|(name, _)| *name == language || name.split('_').next() == Some(language))
        {
            return Some(po);
        }
    }
    None
}

#[derive(Default)]
struct Entry {
    msgid: String,
    msgstr: String,
    fuzzy: bool,
    field: Field,
}

#[derive(Default, PartialEq)]
enum Field {
    #[default]
    None,
    Id,
    Str,
    Other,
}

/// Parses the subset of the `.po` format the GUI catalogs use: `msgid`/`msgstr` pairs with
/// multi-line strings and C escapes. Fuzzy and untranslated entries are skipped so that a stale
/// translation is never shown.
fn parse_po(po: &str) -> Catalog {
    let mut catalog = Catalog::new();
    let mut entry = Entry::default();

    let mut finish = |entry: &mut Entry| {
        let done = std::mem::take(entry);
        if !done.msgid.is_empty() && !done.msgstr.is_empty() && !done.fuzzy {
            catalog.insert(done.msgid, done.msgstr);
        }
    };

    for line in po.lines() {
        let line = line.trim();
        if line.is_empty() {
            finish(&mut entry);
        } else if let Some(flags) = line.strip_prefix("#,") {
            if entry.field != Field::None {
                finish(&mut entry);
            }
            entry.fuzzy |= flags.split(',').any(|flag| flag.trim() == "fuzzy");
        } else if line.starts_with('#') {
            continue;
        } else if let Some(rest) = line.strip_prefix("msgid ") {
            if entry.field != Field::None {
                finish(&mut entry);
            }
            entry.field = Field::Id;
            entry.msgid.push_str(&unquote(rest));
        } else if let Some(rest) = line.strip_prefix("msgstr ") {
            entry.field = Field::Str;
            entry.msgstr.push_str(&unquote(rest));
        } else if line.starts_with('"') {
            match entry.field {
                Field::Id => entry.msgid.push_str(&unquote(line)),
                Field::Str => entry.msgstr.push_str(&unquote(line)),
                Field::None | Field::Other => {}
            }
        } else {
            // msgctxt, msgid_plural, msgstr[n]: not used by these catalogs.
            entry.field = Field::Other;
        }
    }
    finish(&mut entry);
    catalog
}

fn unquote(quoted: &str) -> String {
    let inner = quoted
        .trim()
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .unwrap_or_default();
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('"') => out.push('"'),
            Some('\\') => out.push('\\'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |name| {
            pairs
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| value.to_string())
        }
    }

    #[test]
    fn locale_follows_gettext_precedence() {
        assert_eq!(
            requested_locales(env(&[("LANG", "zh_CN.UTF-8")])),
            vec!["zh_CN.UTF-8"]
        );
        assert_eq!(
            requested_locales(env(&[("LANG", "en_US.UTF-8"), ("LC_ALL", "zh_CN.UTF-8")])),
            vec!["zh_CN.UTF-8"]
        );
        assert_eq!(
            requested_locales(env(&[("LANGUAGE", "zh_CN:en"), ("LANG", "de_DE.UTF-8")])),
            vec!["zh_CN", "en", "de_DE.UTF-8"]
        );
        // LANGUAGE is ignored for the C locale, as gettext does.
        assert!(requested_locales(env(&[("LANGUAGE", "zh_CN"), ("LANG", "C.UTF-8")])).is_empty());
        assert!(requested_locales(env(&[])).is_empty());
        assert_eq!(
            requested_locales(env(&[("GAZE_GUI_LANG", "zh_CN"), ("LANG", "C")])),
            vec!["zh_CN"]
        );
    }

    #[test]
    fn catalog_matches_region_then_language_and_stops_at_english() {
        let pick = |locales: &[&str]| {
            pick_catalog(&locales.iter().map(|l| l.to_string()).collect::<Vec<_>>()).is_some()
        };
        assert!(pick(&["zh_CN.UTF-8"]));
        assert!(pick(&["zh"]));
        assert!(pick(&["zh_SG@latin"]));
        assert!(!pick(&["en_US.UTF-8", "zh_CN"]));
        assert!(!pick(&["nl_NL", "sv"]));
        assert!(pick(&["nl_NL", "zh_CN"]));
    }

    #[test]
    fn catalog_matches_the_right_regional_variant() {
        let pick = |locale: &str| {
            let po = pick_catalog(&[locale.to_string()]);
            CATALOGS
                .iter()
                .find(|(_, candidate)| Some(*candidate) == po)
                .map(|(name, _)| *name)
        };
        assert_eq!(pick("de_AT.UTF-8"), Some("de"));
        assert_eq!(pick("pt_BR.UTF-8"), Some("pt_BR"));
        assert_eq!(pick("zh_CN.UTF-8"), Some("zh_CN"));
        assert_eq!(pick("zh"), Some("zh_CN"));
        assert_eq!(pick("zh_TW.UTF-8"), Some("zh_TW"));
        assert_eq!(pick("zh_HK.UTF-8"), Some("zh_TW"));
    }

    #[test]
    fn po_parser_handles_multiline_escapes_and_skips_fuzzy_or_empty() {
        let catalog = parse_po(concat!(
            "msgid \"\"\n",
            "msgstr \"Content-Type: text/plain; charset=UTF-8\\n\"\n\n",
            "#: src/window.rs:1\n",
            "msgid \"Hello\"\n",
            "msgstr \"你好\"\n\n",
            "msgid \"\"\n",
            "\"Two \\\"lines\\\"\\n\"\n",
            "\"here\"\n",
            "msgstr \"两行\\n\"\n",
            "\"这里\"\n\n",
            "#, fuzzy\n",
            "msgid \"Stale\"\n",
            "msgstr \"过期\"\n\n",
            "msgid \"Untranslated\"\n",
            "msgstr \"\"\n",
        ));
        assert_eq!(catalog.get("Hello").map(String::as_str), Some("你好"));
        assert_eq!(
            catalog.get("Two \"lines\"\nhere").map(String::as_str),
            Some("两行\n这里")
        );
        assert!(!catalog.contains_key("Stale"));
        assert!(!catalog.contains_key("Untranslated"));
        assert!(!catalog.contains_key(""));
    }

    #[test]
    fn fill_replaces_named_placeholders_in_any_order() {
        assert_eq!(
            fill("将“{old}”重命名为“{new}”", &[("new", "b"), ("old", "a")]),
            "将“a”重命名为“b”"
        );
    }

    fn placeholders(text: &str) -> BTreeSet<&str> {
        text.match_indices('{')
            .filter_map(|(start, _)| {
                let end = text[start..].find('}')?;
                Some(&text[start..start + end + 1])
            })
            .collect()
    }

    #[test]
    fn every_catalog_parses_and_keeps_placeholders() {
        for (name, po) in CATALOGS {
            let catalog = parse_po(po);
            assert!(catalog.len() > 50, "{name} looks empty");
            for (msgid, msgstr) in &catalog {
                assert_eq!(
                    placeholders(msgid),
                    placeholders(msgstr),
                    "{name}: placeholders differ for {msgid:?}"
                );
                assert_eq!(
                    msgid.ends_with('\n'),
                    msgstr.ends_with('\n'),
                    "{name}: trailing newline differs for {msgid:?}"
                );
            }
        }
    }

    /// Every translated string should still exist in the GUI sources, so a rewording in English
    /// cannot leave a stale translation behind unnoticed.
    #[test]
    fn catalog_entries_are_still_used() {
        let sources = [
            include_str!("window.rs"),
            include_str!("camera.rs"),
            include_str!("dialog/capture.rs"),
            include_str!("dialog/config.rs"),
            include_str!("../../gaze-core/src/config/mod.rs"),
            include_str!("../../gaze-core/src/dbus.rs"),
            include_str!("../../gaze-vision/src/camera/mod.rs"),
        ]
        .concat();
        for (name, po) in CATALOGS {
            for msgid in parse_po(po).keys() {
                let literal = format!("{msgid:?}");
                let literal = literal.trim_matches('"');
                assert!(
                    sources.contains(literal) || sources.contains(msgid.as_str()),
                    "{name}: {msgid:?} is no longer in the GUI sources"
                );
            }
        }
    }
}

//! Translated interface text (`localization` → Interface translation).
//!
//! The English text is the key: `tr(locale, "switch model")` returns the catalog's
//! translation, or the English text itself for English and for a missing entry. A
//! `{name}` placeholder is filled by [`trf`] after translating.

use std::collections::HashMap;
use std::sync::OnceLock;

use cyber_core::locale::Locale;

type Catalog = HashMap<String, String>;

const SOURCES: [(Locale, &str); 3] = [
    (Locale::PtBr, include_str!("../locales/pt-BR.json")),
    (Locale::Es, include_str!("../locales/es.json")),
    (Locale::Fr, include_str!("../locales/fr.json")),
];

fn catalogs() -> &'static HashMap<Locale, Catalog> {
    static CATALOGS: OnceLock<HashMap<Locale, Catalog>> = OnceLock::new();
    CATALOGS.get_or_init(|| {
        SOURCES
            .iter()
            .map(|(locale, json)| {
                let catalog = serde_json::from_str(json).unwrap_or_else(|e| {
                    panic!("locales/{}.json does not parse: {e}", locale.tag())
                });
                (*locale, catalog)
            })
            .collect()
    })
}

/// `english` in `locale`, falling back to `english`.
pub fn tr(locale: Locale, english: &str) -> &str {
    catalogs()
        .get(&locale)
        .and_then(|catalog| catalog.get(english))
        .map_or(english, String::as_str)
}

/// [`tr`], then each `{name}` replaced by its value.
pub fn trf(locale: Locale, english: &str, args: &[(&str, &dyn std::fmt::Display)]) -> String {
    let mut text = tr(locale, english).to_string();
    for (name, value) in args {
        text = text.replace(&format!("{{{name}}}"), &value.to_string());
    }
    text
}

impl crate::app::App {
    /// [`tr`] in the interface language.
    pub fn tr<'a>(&self, english: &'a str) -> &'a str {
        tr(self.locale, english)
    }

    /// [`trf`] in the interface language.
    pub fn trf(&self, english: &str, args: &[(&str, &dyn std::fmt::Display)]) -> String {
        trf(self.locale, english, args)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// The string literals passed to `tr`/`trf` (`app.tr("…")`, `tr(locale, "…")`) in
    /// the crate's sources.
    fn literal_keys() -> BTreeSet<String> {
        let mut keys = BTreeSet::new();
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut dirs = vec![src];
        while let Some(dir) = dirs.pop() {
            for entry in std::fs::read_dir(dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    dirs.push(path);
                } else if path.extension().is_some_and(|e| e == "rs") && !path.ends_with("i18n.rs")
                {
                    let text = std::fs::read_to_string(&path).unwrap();
                    keys.extend(calls(&text));
                }
            }
        }
        keys
    }

    /// The first argument literal of each `tr(`/`trf(` call, skipping a leading
    /// `locale,` argument.
    fn calls(text: &str) -> Vec<String> {
        let mut out = Vec::new();
        for (at, _) in text.match_indices("tr") {
            let before = text[..at].chars().next_back();
            if before.is_some_and(|c| c.is_alphanumeric() || c == '_') {
                continue;
            }
            let rest = text[at + 2..].strip_prefix('f').unwrap_or(&text[at + 2..]);
            let Some(rest) = rest.strip_prefix('(') else {
                continue;
            };
            let mut rest = rest.trim_start();
            if !rest.starts_with('"')
                && let Some((head, tail)) = rest.split_once(',')
                && head
                    .chars()
                    .all(|c| c.is_alphanumeric() || "_.".contains(c))
            {
                rest = tail.trim_start();
            }
            if let Some(literal) = rest.strip_prefix('"').and_then(string_body) {
                out.push(literal);
            }
        }
        out
    }

    /// The body of a string literal whose opening quote was consumed.
    fn string_body(text: &str) -> Option<String> {
        let mut body = String::new();
        let mut chars = text.chars();
        while let Some(c) = chars.next() {
            match c {
                '"' => return Some(body),
                '\\' => body.push(chars.next()?),
                c => body.push(c),
            }
        }
        None
    }

    /// Texts translated from tables rather than literals at the call.
    fn table_keys() -> BTreeSet<String> {
        let mut keys: BTreeSet<String> = crate::keys::descriptions().map(str::to_owned).collect();
        keys.extend(crate::palette::texts().into_iter().map(str::to_owned));
        keys.extend(
            crate::view::overlays::HELP
                .iter()
                .map(|(_, v)| (*v).to_owned()),
        );
        keys.extend(crate::view::overlays::PERMISSION_CHOICES.map(str::to_owned));
        keys.extend(crate::view::welcome::LABELS.map(str::to_owned));
        keys.insert(crate::view::welcome::TAGLINE.into());
        keys.insert(crate::view::settings::TITLE.into());
        keys.extend(crate::settings::Tab::ALL.map(|tab| tab.label().to_owned()));
        keys.insert(crate::view::composer::PLACEHOLDER.into());
        keys.insert(crate::language::CHOICES[0].1.into());
        keys
    }

    fn placeholders(text: &str) -> BTreeSet<&str> {
        text.split('{')
            .skip(1)
            .filter_map(|part| part.split_once('}').map(|(name, _)| name))
            .collect()
    }

    /// Every translated text is in every catalog with the same `{placeholders}`, and no
    /// catalog keeps an entry nothing translates.
    #[test]
    fn every_catalog_translates_exactly_the_interface_texts() {
        let mut wanted = literal_keys();
        wanted.extend(table_keys());
        assert!(wanted.len() > 100, "{} keys", wanted.len());
        for (locale, _) in SOURCES {
            let catalog = &catalogs()[&locale];
            let missing: Vec<_> = wanted
                .iter()
                .filter(|k| !catalog.contains_key(*k))
                .collect();
            let unused: Vec<_> = catalog.keys().filter(|k| !wanted.contains(*k)).collect();
            assert!(missing.is_empty(), "{} lacks {missing:#?}", locale.tag());
            assert!(unused.is_empty(), "{} has unused {unused:#?}", locale.tag());
            for (english, text) in catalog {
                assert_eq!(
                    placeholders(english),
                    placeholders(text),
                    "{}: {english:?}",
                    locale.tag()
                );
                assert!(!text.trim().is_empty(), "{}: {english:?}", locale.tag());
            }
        }
    }

    /// Writes the texts to translate when `CYBER_I18N_KEYS` names a file.
    #[test]
    fn export_keys_on_request() {
        if let Ok(path) = std::env::var("CYBER_I18N_KEYS") {
            let mut wanted = literal_keys();
            wanted.extend(table_keys());
            std::fs::write(path, serde_json::to_string_pretty(&wanted).unwrap()).unwrap();
        }
    }

    #[test]
    fn calls_are_found_with_and_without_a_locale_argument() {
        let text = r#"app.tr("one"); tr(locale, "two"); app.trf(
            "three {n}", &[]); attr("no"); tr(self.locale, "four")"#;
        assert_eq!(calls(text), ["one", "two", "three {n}", "four"]);
    }

    #[test]
    fn english_and_missing_entries_fall_back_to_the_key() {
        assert_eq!(tr(Locale::En, "switch model"), "switch model");
        assert_eq!(
            tr(Locale::Fr, "no such text anywhere"),
            "no such text anywhere"
        );
        assert_ne!(tr(Locale::PtBr, "switch model"), "switch model");
    }

    #[test]
    fn placeholders_are_filled_after_translation() {
        let args: &[(&str, &dyn std::fmt::Display)] = &[("tool", &"bash")];
        assert_eq!(trf(Locale::En, "Tool: {tool}", args), "Tool: bash");
        let text = trf(Locale::Es, "Tool: {tool}", args);
        assert!(text.ends_with("bash") && !text.contains('{'), "{text}");
    }
}

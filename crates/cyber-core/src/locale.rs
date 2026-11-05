//! Interface and reply language (`localization`).
//!
//! `language` in the configuration is `auto` (the default) or one of the supported
//! tags. `auto` takes the interface language from `LC_ALL`, `LC_MESSAGES` or `LANG`
//! and leaves the model to answer in the language it is written to; a set language
//! also asks the model to reply in it.

use crate::env::EnvSource;

/// A supported interface language.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum Locale {
    #[default]
    En,
    PtBr,
    Es,
    Fr,
}

impl Locale {
    pub const ALL: [Locale; 4] = [Locale::En, Locale::PtBr, Locale::Es, Locale::Fr];

    /// The BCP 47 tag: `en`, `pt-BR`, `es`, `fr`.
    pub fn tag(self) -> &'static str {
        match self {
            Locale::En => "en",
            Locale::PtBr => "pt-BR",
            Locale::Es => "es",
            Locale::Fr => "fr",
        }
    }

    /// The language's English name, as the model is told it.
    pub fn english_name(self) -> &'static str {
        match self {
            Locale::En => "English",
            Locale::PtBr => "Brazilian Portuguese",
            Locale::Es => "Spanish",
            Locale::Fr => "French",
        }
    }

    /// A language tag or POSIX locale name: `pt-BR`, `pt_BR.UTF-8`, `es_MX`, `fr`.
    /// Matching is on the language alone, so every Portuguese variant is `pt-BR`.
    /// `C`, `POSIX` and unsupported languages are `None`.
    pub fn parse(value: &str) -> Option<Locale> {
        let language = value
            .split(['_', '-', '.', '@'])
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        match language.as_str() {
            "en" => Some(Locale::En),
            "pt" => Some(Locale::PtBr),
            "es" => Some(Locale::Es),
            "fr" => Some(Locale::Fr),
            _ => None,
        }
    }

    /// The first of `LC_ALL`, `LC_MESSAGES` and `LANG` that is set, as POSIX reads
    /// them; English when it names no supported language.
    pub fn from_env(env: &dyn EnvSource) -> Locale {
        ["LC_ALL", "LC_MESSAGES", "LANG"]
            .iter()
            .find_map(|name| env.get(name).filter(|v| !v.is_empty()))
            .and_then(|value| Locale::parse(&value))
            .unwrap_or_default()
    }
}

/// The configured `language`: `None` for `auto`, a missing key or an unsupported value.
pub fn configured(config: &serde_json::Value) -> Option<Locale> {
    let value = config.get("language")?.as_str()?;
    if value.eq_ignore_ascii_case("auto") {
        return None;
    }
    Locale::parse(value).filter(|_| valid(value))
}

/// The interface language: the configured one, else the environment's.
pub fn resolve(config: &serde_json::Value, env: &dyn EnvSource) -> Locale {
    configured(config).unwrap_or_else(|| Locale::from_env(env))
}

/// Whether `value` is an accepted `language` setting.
pub fn valid(value: &str) -> bool {
    value.eq_ignore_ascii_case("auto")
        || Locale::ALL
            .iter()
            .any(|locale| locale.tag().eq_ignore_ascii_case(value))
}

/// The `core/language` context text for a set language. Code and anything the user
/// or a tool must type stays as it is.
pub fn reply_instruction(locale: Locale) -> String {
    format!(
        "Language: write your replies, questions, plans and summaries for the user in {} ({}). \
         Keep code, identifiers, commands, file paths, configuration keys, tool arguments and \
         quoted output exactly as they are, and follow the project's own conventions for \
         commit messages and code comments.",
        locale.english_name(),
        locale.tag(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::HashMap;

    struct Env(HashMap<&'static str, &'static str>);
    impl EnvSource for Env {
        fn get(&self, key: &str) -> Option<String> {
            self.0.get(key).map(|v| (*v).to_string())
        }
    }

    #[test]
    fn parses_tags_and_posix_names_by_language() {
        assert_eq!(Locale::parse("pt-BR"), Some(Locale::PtBr));
        assert_eq!(Locale::parse("pt_PT.UTF-8"), Some(Locale::PtBr));
        assert_eq!(Locale::parse("es_MX.utf8"), Some(Locale::Es));
        assert_eq!(Locale::parse("fr_CA@euro"), Some(Locale::Fr));
        assert_eq!(Locale::parse("en_GB"), Some(Locale::En));
        assert_eq!(Locale::parse("C.UTF-8"), None);
        assert_eq!(Locale::parse("de_DE"), None);
    }

    #[test]
    fn environment_follows_posix_precedence() {
        let env = Env(HashMap::from([
            ("LC_ALL", ""),
            ("LC_MESSAGES", "fr_FR.UTF-8"),
            ("LANG", "pt_BR.UTF-8"),
        ]));
        assert_eq!(Locale::from_env(&env), Locale::Fr);
        let env = Env(HashMap::from([("LANG", "pt_BR.UTF-8")]));
        assert_eq!(Locale::from_env(&env), Locale::PtBr);
        let env = Env(HashMap::from([("LANG", "C")]));
        assert_eq!(Locale::from_env(&env), Locale::En);
        assert_eq!(Locale::from_env(&Env(HashMap::new())), Locale::En);
    }

    #[test]
    fn configuration_wins_over_the_environment_and_auto_defers() {
        let env = Env(HashMap::from([("LANG", "es_ES.UTF-8")]));
        assert_eq!(resolve(&json!({"language": "fr"}), &env), Locale::Fr);
        assert_eq!(resolve(&json!({"language": "PT-br"}), &env), Locale::PtBr);
        assert_eq!(resolve(&json!({"language": "auto"}), &env), Locale::Es);
        assert_eq!(resolve(&json!({}), &env), Locale::Es);
        assert_eq!(
            configured(&json!({"language": "pt_BR"})),
            None,
            "a tag, not a POSIX name"
        );
        assert_eq!(configured(&json!({"language": "de"})), None);
    }

    #[test]
    fn valid_values() {
        for value in ["auto", "AUTO", "en", "pt-BR", "pt-br", "es", "fr"] {
            assert!(valid(value), "{value}");
        }
        for value in ["", "pt", "de", "en-US", "pt_BR"] {
            assert!(!valid(value), "{value}");
        }
    }

    #[test]
    fn the_reply_instruction_names_the_language_and_keeps_code() {
        let text = reply_instruction(Locale::PtBr);
        assert!(text.contains("Brazilian Portuguese (pt-BR)"));
        assert!(text.contains("Keep code, identifiers, commands"));
    }
}

//! Embedded prompt texts (`system-context` → Provider base prompts).
//!
//! The base prompt is the shared core text followed by a short addendum for the model
//! family, whose advice is about tool use and editing, where harnesses differ.

const CORE: &str = include_str!("../prompts/base/core.md");
const OPENAI: &str = include_str!("../prompts/base/openai.md");
const ANTHROPIC: &str = include_str!("../prompts/base/anthropic.md");
const GEMINI: &str = include_str!("../prompts/base/gemini.md");
const LOCAL: &str = include_str!("../prompts/base/local.md");

/// The model family a base prompt is written for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ModelFamily {
    Openai,
    Anthropic,
    Gemini,
    /// Open-weights models, or any model behind a local endpoint.
    Local,
    /// Unknown: the core text alone.
    #[default]
    Default,
}

impl ModelFamily {
    pub const ALL: [Self; 5] = [
        Self::Openai,
        Self::Anthropic,
        Self::Gemini,
        Self::Local,
        Self::Default,
    ];

    /// The durable name, as recorded on a Context Epoch.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Openai => "openai",
            Self::Anthropic => "anthropic",
            Self::Gemini => "gemini",
            Self::Local => "local",
            Self::Default => "default",
        }
    }

    fn addendum(self) -> Option<&'static str> {
        match self {
            Self::Openai => Some(OPENAI),
            Self::Anthropic => Some(ANTHROPIC),
            Self::Gemini => Some(GEMINI),
            Self::Local => Some(LOCAL),
            Self::Default => None,
        }
    }
}

/// The base prompt for a model family: the core text, then a blank line and the addendum.
pub fn base_prompt(family: ModelFamily) -> String {
    let core = CORE.trim_end();
    match family.addendum() {
        Some(addendum) => format!("{core}\n\n{}", addendum.trim_end()),
        None => core.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_family_is_the_core_text_plus_its_addendum() {
        let core = CORE.trim_end();
        assert!(core.starts_with("You are Cyber Code"));
        for family in ModelFamily::ALL {
            let prompt = base_prompt(family);
            match family.addendum() {
                Some(addendum) => {
                    let rest = prompt.strip_prefix(core).unwrap();
                    assert_eq!(rest, format!("\n\n{}", addendum.trim_end()), "{family:?}");
                    assert!(!addendum.trim().is_empty() && !addendum.contains(core));
                }
                None => assert_eq!(prompt, core),
            }
        }
    }

    #[test]
    fn default_is_the_core_text_alone_and_families_differ() {
        assert_eq!(base_prompt(ModelFamily::Default), CORE.trim_end());
        assert!(base_prompt(ModelFamily::Openai).contains("apply_patch"));
        assert!(base_prompt(ModelFamily::Anthropic).contains("parallel"));
        assert!(base_prompt(ModelFamily::Gemini).contains("schema"));
        assert!(base_prompt(ModelFamily::Local).contains("valid JSON"));
        let names: std::collections::BTreeSet<_> =
            ModelFamily::ALL.iter().map(|f| f.as_str()).collect();
        assert_eq!(names.len(), ModelFamily::ALL.len());
    }
}

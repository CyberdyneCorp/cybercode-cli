//! The Workflows embedded in the binary (`workflows` → Saved and bundled workflows, Bundled
//! workflow contracts): `review`, `audit`, `migrate`, `research`, `plan`, `solve` and `integrate`.
//!
//! Modules cannot import, so each embedded source is the module followed by the shared
//! helpers of `bundled/common.js`, joined at compile time. The digest covers both, so a
//! release that changes a helper is a new revision of every bundled Workflow on resume.

use std::sync::OnceLock;

use crate::module::{Origin, WorkflowModule};

/// `CYBER_DISABLE_BUNDLED_WORKFLOWS=1` hides the bundled Workflows, like
/// `workflows.bundled: false`.
pub const DISABLE_ENV: &str = "CYBER_DISABLE_BUNDLED_WORKFLOWS";

macro_rules! bundled {
    ($name:literal) => {
        (
            $name,
            concat!(
                include_str!(concat!("bundled/", $name, ".js")),
                "\n",
                include_str!("bundled/common.js")
            ),
        )
    };
}

/// `(name, source)` of every bundled Workflow, in listing order.
const SOURCES: [(&str, &str); 7] = [
    bundled!("audit"),
    bundled!("integrate"),
    bundled!("migrate"),
    bundled!("plan"),
    bundled!("research"),
    bundled!("review"),
    bundled!("solve"),
];

/// The bundled Workflows, parsed once.
pub fn modules() -> &'static [WorkflowModule] {
    static MODULES: OnceLock<Vec<WorkflowModule>> = OnceLock::new();
    MODULES.get_or_init(|| {
        SOURCES
            .iter()
            .map(|(name, source)| {
                WorkflowModule::parse(
                    *source,
                    Origin::Bundled {
                        name: (*name).to_owned(),
                    },
                )
                .unwrap_or_else(|error| panic!("bundled Workflow {name} does not parse: {error}"))
            })
            .collect()
    })
}

/// The bundled Workflow `name`.
pub fn module(name: &str) -> Option<&'static WorkflowModule> {
    modules().iter().find(|module| module.meta.name == name)
}

/// Whether bundled Workflows are offered: `workflows.bundled` and not disabled by
/// [`DISABLE_ENV`].
pub fn enabled(config_setting: bool, disabled_by_env: bool) -> bool {
    config_setting && !disabled_by_env
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_bundled_module_parses_and_is_named_after_its_file() {
        let names: Vec<&str> = modules().iter().map(|m| m.meta.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "audit",
                "integrate",
                "migrate",
                "plan",
                "research",
                "review",
                "solve"
            ]
        );
        for (module, (file, _)) in modules().iter().zip(SOURCES) {
            assert_eq!(module.meta.name, file);
            assert_eq!(
                module.origin,
                Origin::Bundled {
                    name: file.to_owned()
                }
            );
            let raw = &module.meta.raw;
            for key in ["budget", "estimate", "args"] {
                assert!(raw.get(key).is_some(), "{file}: meta.{key} is missing");
            }
            assert!(!module.meta.phases.is_empty(), "{file}: no phases");
        }
    }

    #[test]
    fn args_defaults_validate_against_their_schema() {
        for module in modules() {
            let schema = module.meta.args.as_ref().unwrap();
            let validator = jsonschema::validator_for(schema)
                .unwrap_or_else(|e| panic!("{}: invalid args schema: {e}", module.meta.name));
            let properties = schema["properties"].as_object().unwrap();
            for (key, property) in properties {
                let Some(default) = property.get("default") else {
                    continue;
                };
                let sub = jsonschema::validator_for(property).unwrap();
                assert!(
                    sub.is_valid(default),
                    "{}: default of {key} does not validate",
                    module.meta.name
                );
            }
            // Required fields aside, the defaults alone make a valid invocation.
            let mut sample = serde_json::Map::new();
            for (key, property) in properties {
                if let Some(default) = property.get("default") {
                    sample.insert(key.clone(), default.clone());
                }
            }
            for (key, value) in required_samples(&module.meta.name) {
                sample.insert(key.into(), value);
            }
            let sample = serde_json::Value::Object(sample);
            assert!(
                validator.is_valid(&sample),
                "{}: defaults do not validate: {sample}",
                module.meta.name
            );
        }
    }

    fn required_samples(name: &str) -> Vec<(&'static str, serde_json::Value)> {
        match name {
            "integrate" => vec![
                (
                    "contributions",
                    serde_json::json!([{"label":"api","commit":"a".repeat(40)}]),
                ),
                ("checks", serde_json::json!([{"argv":["cargo","test"]}])),
            ],
            "migrate" => vec![
                ("instructions", "rename foo to bar".into()),
                ("pattern", "src/**/*.rs".into()),
            ],
            "research" => vec![("question", "how does it work?".into())],
            "plan" => vec![("task", "add a cache".into())],
            "solve" => vec![
                ("task", "fix issue".into()),
                ("checks", serde_json::json!([{"argv":["cargo","test"]}])),
            ],
            _ => Vec::new(),
        }
    }

    #[test]
    fn the_disable_switches_hide_bundled_workflows() {
        assert!(enabled(true, false));
        assert!(!enabled(true, true));
        assert!(!enabled(false, false));
    }

    #[test]
    fn the_digest_covers_the_shared_helpers() {
        let review = module("review").unwrap();
        assert!(review.source.contains("function dedupFindings"));
        assert_eq!(review.digest, crate::module::digest(&review.source));
    }
}

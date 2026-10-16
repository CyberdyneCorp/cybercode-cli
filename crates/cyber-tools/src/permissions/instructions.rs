//! The read gate on project instruction files (`system-context` → Instruction file
//! discovery): the Location's `read` rules decide, and only an explicit Allow loads a
//! file, so a checkout cannot pull a protected file such as `.env` into privileged context
//! through an `@.env` import or a symlink without the prompt a `read` of it would raise.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use cyber_server::runtime::instructions::ReadGate;
use serde_json::Value;

use super::engine::{Effect, Rule, defaults, evaluate_scoped, parse_rules, slash};

/// The gate for `location` from its resolved config (`None` when it failed to load, which
/// refuses every project file: a broken config must not widen what reaches the model).
pub fn instruction_read_gate(
    config: Option<(&Value, &BTreeMap<String, String>)>,
    location: &Path,
) -> ReadGate {
    let Some((config, sources)) = config else {
        return ReadGate::new(|_, _| false);
    };
    let mut rules = defaults(&[], true);
    rules.extend(parse_rules(&config["permissions"], sources));
    let bases = bases(location);
    ReadGate::new(move |path, canonical| {
        resources(&bases, path, canonical)
            .iter()
            .all(|resource| read_effect(&rules, resource) == Effect::Allow)
    })
}

fn read_effect(rules: &[Rule], resource: &str) -> Effect {
    evaluate_scoped(rules, "read", resource, Some("read")).0
}

/// The Location and its project root, as given and canonical: a file is checked relative
/// to each, so a rule written for either (`Read(./.env)` → `.env`) applies.
fn bases(location: &Path) -> Vec<PathBuf> {
    let root = cyber_core::config::project_root(location);
    [location.to_path_buf(), root]
        .into_iter()
        .flat_map(|base| {
            let canonical = base.canonicalize().unwrap_or_else(|_| base.clone());
            [base, canonical]
        })
        .collect()
}

/// Every resource a `read` of the file could be checked as: the discovered and canonical
/// paths, each absolute and relative to every base. The strictest effect wins.
fn resources(bases: &[PathBuf], path: &Path, canonical: &Path) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for file in [path, canonical] {
        out.push(slash(file));
        out.extend(
            bases
                .iter()
                .filter_map(|base| file.strip_prefix(base).ok())
                .map(slash),
        );
    }
    out.sort();
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn gate(config: Value, location: &Path) -> ReadGate {
        instruction_read_gate(Some((&config, &BTreeMap::new())), location)
    }

    #[test]
    fn env_files_ask_by_default_and_a_deny_rule_refuses() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        let web = root.join("web");
        let defaults = gate(json!({}), &web);
        assert!(defaults.allows(&root.join("CYBER.md"), &root.join("CYBER.md")));
        assert!(!defaults.allows(&web.join("CYBER.md"), &root.join(".env")));
        assert!(defaults.allows(&root.join(".env.example"), &root.join(".env.example")));
        let denied = gate(
            json!({"permissions": {"read": {"secrets/*": "deny"}}}),
            &web,
        );
        let secret = root.join("secrets/rules.md");
        assert!(!denied.allows(&secret, &secret));
        assert!(denied.allows(&root.join("CYBER.md"), &root.join("CYBER.md")));
    }

    #[test]
    fn a_config_that_failed_to_load_refuses_every_file() {
        let path = Path::new("CYBER.md");
        assert!(!instruction_read_gate(None, Path::new(".")).allows(path, path));
    }
}

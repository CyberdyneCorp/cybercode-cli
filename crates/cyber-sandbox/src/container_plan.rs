//! The confinement of one Windows AppContainer launch (`sandbox` → Windows enforcement).
//!
//! [`crate::wrap`] builds a plan from a [`Launch`] and hands it to
//! `cyber-sandbox-exec --appcontainer`, which grants exactly these paths to a fresh
//! less-privileged AppContainer identity and revokes them when the command exits. The plan
//! is plain data so its construction and encoding are checked on every platform.
//!
//! The helper also records each launch's grants in the plan's `journal` directory until
//! they are revoked, so a helper killed with its process tree (a timeout, a cancel, a
//! host shutdown) leaves a record the next launch uses to revoke them.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::{Launch, NetworkMode, Policy, SandboxError};

/// The helper option that selects AppContainer confinement.
pub const APPCONTAINER_FLAG: &str = "--appcontainer";

/// The longest encoded plan; Windows command lines are limited to 32 767 UTF-16 units.
const MAX_ENCODED: usize = 16 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerPlan {
    /// Roots granted recursively: read-write under `workspace-write`, read-only otherwise.
    pub roots: Vec<PathBuf>,
    pub writable: bool,
    /// Extra roots granted read-only (`sandbox.readable_paths`).
    pub readable: Vec<PathBuf>,
    /// Protected paths inside the roots that stay read-only.
    pub read_only: Vec<PathBuf>,
    /// Paths inside the roots that must not be read (credential files, `deny_read`).
    pub unreadable: Vec<PathBuf>,
    /// `sandbox.network: "on"`. Otherwise the identity holds no network capability.
    pub network: bool,
    /// Where launches record their grants until they are revoked.
    pub journal: PathBuf,
}

/// `%LOCALAPPDATA%\cyber\sandbox-grants`, else the same under the temporary directory.
pub fn journal_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .unwrap_or_else(std::env::temp_dir)
        .join("cyber")
        .join("sandbox-grants")
}

impl ContainerPlan {
    /// The plan for `launch`. `deny_read` globs are expanded to the paths that exist now.
    pub fn from_launch(launch: &Launch) -> Result<Self, SandboxError> {
        let denied = launch
            .deny_read
            .expand()
            .map_err(SandboxError::Unavailable)?;
        Ok(Self {
            roots: launch.writable.clone(),
            writable: launch.config.policy == Policy::WorkspaceWrite,
            readable: launch.config.readable_paths.clone(),
            read_only: launch.read_only.clone(),
            unreadable: launch.unreadable.iter().cloned().chain(denied).collect(),
            network: launch.config.network == NetworkMode::On,
            journal: journal_dir(),
        })
    }

    pub fn encode(&self) -> Result<String, SandboxError> {
        let text = json!({
            "roots": paths(&self.roots)?,
            "writable": self.writable,
            "readable": paths(&self.readable)?,
            "read_only": paths(&self.read_only)?,
            "unreadable": paths(&self.unreadable)?,
            "network": self.network,
            "journal": paths(std::slice::from_ref(&self.journal))?[0],
        })
        .to_string();
        if text.len() > MAX_ENCODED {
            return Err(SandboxError::Unavailable(format!(
                "the Windows sandbox plan exceeds {MAX_ENCODED} bytes; narrow sandbox.deny_read or the writable roots"
            )));
        }
        Ok(text)
    }

    /// Parse an encoded plan. Unknown keys, missing keys and relative paths are refused.
    pub fn decode(text: &str) -> Result<Self, String> {
        let value: Value = serde_json::from_str(text).map_err(|e| format!("invalid plan: {e}"))?;
        let object = value.as_object().ok_or("invalid plan: not an object")?;
        const KEYS: [&str; 7] = [
            "roots",
            "writable",
            "readable",
            "read_only",
            "unreadable",
            "network",
            "journal",
        ];
        if let Some(key) = object.keys().find(|key| !KEYS.contains(&key.as_str())) {
            return Err(format!("invalid plan: unknown key {key}"));
        }
        let flag = |key: &str| {
            object
                .get(key)
                .and_then(Value::as_bool)
                .ok_or_else(|| format!("invalid plan: {key} must be a boolean"))
        };
        let list = |key: &str| decode_paths(object.get(key), key);
        Ok(Self {
            roots: list("roots")?,
            writable: flag("writable")?,
            readable: list("readable")?,
            read_only: list("read_only")?,
            unreadable: list("unreadable")?,
            network: flag("network")?,
            journal: journal(object.get("journal"))?,
        })
    }
}

fn paths(list: &[PathBuf]) -> Result<Vec<&str>, SandboxError> {
    list.iter()
        .map(|path| {
            path.to_str().ok_or_else(|| {
                SandboxError::Unavailable(format!(
                    "the Windows sandbox cannot grant a non-Unicode path: {}",
                    path.display()
                ))
            })
        })
        .collect()
}

fn journal(value: Option<&Value>) -> Result<PathBuf, String> {
    let text = value
        .and_then(Value::as_str)
        .ok_or("invalid plan: journal must be a path")?;
    let path = PathBuf::from(text);
    if !path.is_absolute() {
        return Err(format!("invalid plan: journal is a relative path {text}"));
    }
    Ok(path)
}

fn decode_paths(value: Option<&Value>, key: &str) -> Result<Vec<PathBuf>, String> {
    let items = value
        .and_then(Value::as_array)
        .ok_or_else(|| format!("invalid plan: {key} must be a list"))?;
    items
        .iter()
        .map(|item| {
            let text = item
                .as_str()
                .ok_or_else(|| format!("invalid plan: {key} holds a non-string"))?;
            let path = PathBuf::from(text);
            if !path.is_absolute() {
                return Err(format!("invalid plan: {key} holds a relative path {text}"));
            }
            Ok(path)
        })
        .collect()
}

/// Whether `path` lies inside `root`, comparing canonical forms when both exist.
pub fn within(path: &Path, root: &Path) -> bool {
    let canonical = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    path.starts_with(root) || canonical(path).starts_with(canonical(root))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DenyRead, SandboxConfig};
    use std::collections::BTreeMap;

    fn launch(config: serde_json::Value, root: &Path) -> Launch {
        let home = root.join("home");
        let config = SandboxConfig::resolve(&config, &BTreeMap::new(), None, &home);
        Launch {
            writable: vec![root.join("repo"), root.join("tmp")],
            read_only: crate::protected_in(&root.join("repo")),
            unreadable: config.unreadable(&home),
            deny_read: DenyRead::new(&config.deny_read, &root.join("repo"), &home),
            proxy: None,
            helper: None,
            config,
        }
    }

    #[test]
    fn workspace_write_plan_grants_roots_and_keeps_exclusions() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join("repo")).unwrap();
        std::fs::write(root.join("repo/.env"), "SECRET=1").unwrap();
        let launch = launch(
            json!({"sandbox": {"deny_read": [".env"], "network": "off"}}),
            &root,
        );
        let plan = ContainerPlan::from_launch(&launch).unwrap();
        assert!(plan.writable);
        assert!(!plan.network);
        assert_eq!(plan.roots, launch.writable);
        assert!(plan.read_only.contains(&root.join("repo/.git")));
        assert!(plan.unreadable.contains(&root.join("home/.ssh")));
        assert!(plan.unreadable.contains(&root.join("repo/.env")));
        assert_eq!(ContainerPlan::decode(&plan.encode().unwrap()), Ok(plan));
    }

    #[test]
    fn read_only_policy_and_network_modes_select_the_identity() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let plan = ContainerPlan::from_launch(&launch(
            json!({"sandbox": {"policy": "read-only", "network": "on"}}),
            &root,
        ))
        .unwrap();
        assert!(!plan.writable);
        assert!(plan.network);
        // The proxy is unreachable from an identity without network capability, so
        // `proxy` confines like `off` rather than allowing everything.
        let plan = ContainerPlan::from_launch(&launch(json!({}), &root)).unwrap();
        assert!(!plan.network);
    }

    #[test]
    fn decoding_refuses_unknown_keys_wrong_types_and_relative_paths() {
        let journal = journal_dir();
        let valid = json!({"roots": [], "writable": true, "readable": [], "read_only": [],
            "unreadable": [], "network": false, "journal": journal});
        assert!(ContainerPlan::decode(&valid.to_string()).is_ok());
        let mut extra = valid.clone();
        extra["full_access"] = json!(true);
        assert!(
            ContainerPlan::decode(&extra.to_string())
                .unwrap_err()
                .contains("unknown key full_access")
        );
        let mut missing = valid.clone();
        missing.as_object_mut().unwrap().remove("network");
        assert!(ContainerPlan::decode(&missing.to_string()).is_err());
        let mut unjournaled = valid.clone();
        unjournaled.as_object_mut().unwrap().remove("journal");
        assert!(ContainerPlan::decode(&unjournaled.to_string()).is_err());
        let mut relative = valid.clone();
        relative["roots"] = json!(["repo"]);
        assert!(
            ContainerPlan::decode(&relative.to_string())
                .unwrap_err()
                .contains("relative path")
        );
        assert!(ContainerPlan::decode("not json").is_err());
    }

    #[test]
    fn oversized_plans_are_refused_before_launch() {
        let plan = ContainerPlan {
            roots: vec![],
            writable: true,
            readable: vec![],
            read_only: vec![],
            unreadable: (0..2000)
                .map(|i| PathBuf::from(format!("/deny/path/number/{i}")))
                .collect(),
            network: false,
            journal: journal_dir(),
        };
        assert!(matches!(plan.encode(), Err(SandboxError::Unavailable(_))));
    }
}

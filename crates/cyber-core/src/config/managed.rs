//! Read-only OS-managed policy (`org-policy` → Policy sources, the OS directory only).
//!
//! The policy document is never part of the user/project layer chain: `load` strips
//! `policy` from every other layer, and this loader is the only place `managed:` hook
//! origins and `policy.hooks.managed_only` come from. A present but unusable document
//! fails closed: no managed hooks are added and every other hook is refused.

use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::{Map, Value};

use super::jsonc;
use crate::env::EnvSource;

#[cfg(windows)]
#[allow(unsafe_code)] // Bounded read-only owner/DACL and known-folder queries are isolated here.
mod windows;

/// Test and packaging override for the OS-managed directory, like `CYBER_HOME`. It is
/// ignored while the OS-managed directory itself holds a policy document, so it can
/// never replace or relax a real managed policy.
pub const MANAGED_POLICY_DIR_ENV: &str = "CYBER_MANAGED_POLICY_DIR";

const FILES: [&str; 2] = ["cyber-policy.json", "cyber-policy.jsonc"];

/// Every top-level key a managed document may hold. Anything else, including a typo of
/// `policy`, makes the document malformed so it fails closed instead of being ignored.
const DOCUMENT_KEYS: [&str; 3] = ["$schema", "hooks", "policy"];

/// The hook-related state of the managed policy for one configuration load.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, schemars::JsonSchema)]
pub struct ManagedPolicy {
    /// `managed:<file>` labels of the documents read, lowest priority first.
    pub sources: Vec<String>,
    /// `policy.hooks.managed_only`: only managed-scope hooks run.
    pub managed_only: bool,
    /// Why a present policy could not be used. Every non-managed hook is refused.
    pub error: Option<String>,
}

impl ManagedPolicy {
    /// Whether non-managed hooks are refused, by policy or because it is malformed.
    pub fn restricts_hooks(&self) -> bool {
        self.managed_only || self.error.is_some()
    }

    /// The diagnostic shown when non-managed hooks are refused.
    pub fn skip_reason(&self) -> String {
        match &self.error {
            Some(error) => format!("managed policy is malformed ({error})"),
            None => "managed-only policy".into(),
        }
    }
}

/// The managed policy and the `hooks` documents it contributes, as `(label, hooks)`.
pub(super) struct ManagedRead {
    pub policy: ManagedPolicy,
    pub hooks: Vec<(String, Value)>,
}

/// The OS-managed directory: `/etc/cyber` on Linux and other Unix systems,
/// `/Library/Application Support/cyber` on macOS, `%ProgramData%\cyber` on Windows, where
/// `%ProgramData%` comes from the known-folder store rather than the environment.
#[cfg(windows)]
pub fn os_managed_dir() -> PathBuf {
    windows::program_data().join("cyber")
}

#[cfg(not(windows))]
pub fn os_managed_dir() -> PathBuf {
    if cfg!(target_os = "macos") {
        return PathBuf::from("/Library/Application Support/cyber");
    }
    PathBuf::from("/etc/cyber")
}

pub(super) fn read(env: &dyn EnvSource, warnings: &mut Vec<String>) -> ManagedRead {
    let os = os_managed_dir();
    let (dir, owner_checked) = match env.get(MANAGED_POLICY_DIR_ENV) {
        Some(_) if has_document(&os) => {
            warnings.push(format!(
                "{MANAGED_POLICY_DIR_ENV} ignored: {} holds the managed policy",
                os.display()
            ));
            (os, true)
        }
        Some(dir) => (PathBuf::from(dir), false),
        None => (os, true),
    };
    let mut read = ManagedRead {
        policy: ManagedPolicy::default(),
        hooks: Vec::new(),
    };
    for path in FILES.iter().map(|name| dir.join(name)) {
        match document(&path, owner_checked, warnings) {
            Ok(None) => {}
            Ok(Some(value)) => {
                let label = format!("managed:{}", path.display());
                if let Err(error) = absorb(&mut read, &label, value) {
                    return malformed(read.policy.sources, format!("{}: {error}", path.display()));
                }
            }
            Err(error) => return malformed(read.policy.sources, error),
        }
    }
    read
}

fn malformed(sources: Vec<String>, error: String) -> ManagedRead {
    ManagedRead {
        policy: ManagedPolicy {
            sources,
            managed_only: false,
            error: Some(error),
        },
        hooks: Vec::new(),
    }
}

fn has_document(dir: &Path) -> bool {
    FILES.iter().any(|name| dir.join(name).exists())
}

/// One policy document; `Ok(None)` when it is absent or ignored for its owner.
fn document(
    path: &Path,
    owner_checked: bool,
    warnings: &mut Vec<String>,
) -> Result<Option<Value>, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("{}: {error}", path.display())),
    };
    if owner_checked && let Some(reason) = not_admin_controlled(path) {
        warnings.push(format!("{reason}; ignored: {}", path.display()));
        return Ok(None);
    }
    let label = path.display().to_string();
    jsonc::parse(&label, &text)
        .map(Some)
        .map_err(|error| error.to_string())
}

/// Only an administrator-controlled file in the OS directory is managed
/// (`org-policy` → Policy sources). On Unix it must be owned by root and not writable by
/// group or others.
#[cfg(unix)]
fn not_admin_controlled(path: &Path) -> Option<&'static str> {
    use std::os::unix::fs::MetadataExt;
    match std::fs::metadata(path) {
        Ok(metadata) if metadata.uid() != 0 => Some("managed policy file not root-owned"),
        Ok(metadata) if metadata.mode() & 0o022 != 0 => {
            Some("managed policy file writable by non-root users")
        }
        Ok(_) => None,
        Err(_) => Some("managed policy file not root-owned"),
    }
}

/// On Windows the file and its directory must be owned by SYSTEM, Administrators or
/// TrustedInstaller, and no other principal may change, remove or take them over.
#[cfg(windows)]
fn not_admin_controlled(path: &Path) -> Option<&'static str> {
    (!windows::admin_controlled(path)).then_some("managed policy file not admin-owned")
}

#[cfg(not(any(unix, windows)))]
fn not_admin_controlled(_path: &Path) -> Option<&'static str> {
    Some("managed policy file ownership cannot be verified")
}

/// Validate one document and add its hooks and `policy.hooks.managed_only`.
fn absorb(read: &mut ManagedRead, label: &str, value: Value) -> Result<(), String> {
    let Value::Object(document) = value else {
        return Err("the document must be a JSON object".into());
    };
    if let Some(key) = document
        .keys()
        .find(|key| !DOCUMENT_KEYS.contains(&key.as_str()))
    {
        return Err(format!("{key}: unsupported managed policy key"));
    }
    if let Some(policy) = document.get("policy") {
        read.policy.managed_only |= managed_only(policy)?;
    }
    if let Some(hooks) = document.get("hooks") {
        read.hooks.push((label.to_owned(), managed_hooks(hooks)?));
    }
    read.policy.sources.push(label.to_owned());
    Ok(())
}

fn managed_only(policy: &Value) -> Result<bool, String> {
    let policy = policy.as_object().ok_or("policy: expected an object")?;
    if let Some(key) = policy.keys().find(|key| *key != "hooks") {
        return Err(format!("policy.{key}: unsupported managed policy key"));
    }
    let hooks = match policy.get("hooks") {
        None => return Ok(false),
        Some(hooks) => hooks
            .as_object()
            .ok_or("policy.hooks: expected an object")?,
    };
    if let Some(key) = hooks.keys().find(|key| *key != "managed_only") {
        return Err(format!(
            "policy.hooks.{key}: unsupported managed policy key"
        ));
    }
    match hooks.get("managed_only") {
        None => Ok(false),
        Some(Value::Bool(value)) => Ok(*value),
        Some(_) => Err("policy.hooks.managed_only: expected a boolean".into()),
    }
}

/// Managed hooks are event arrays only, validated like any other hook definition and
/// never substituted.
fn managed_hooks(hooks: &Value) -> Result<Value, String> {
    let events = hooks.as_object().ok_or("hooks: expected an object")?;
    if let Some(key) = events.keys().find(|key| !super::is_event_name(key)) {
        return Err(format!(
            "hooks.{key}: managed hooks support only event arrays"
        ));
    }
    let mut wrapped = Map::new();
    wrapped.insert("hooks".into(), hooks.clone());
    super::HookSettings::from_config(&Value::Object(wrapped))?;
    Ok(hooks.clone())
}

//! Secret plugin settings (`plugins-marketplace` → User configuration): `userConfig`
//! properties marked `secret` are kept in the OS keyring (macOS Keychain, Windows
//! Credential Manager) under the service [`SERVICE`], never in configuration files.
//! Linux has no keyring backend in release builds, so there they live in an owner-only
//! file beside the other state, like MCP OAuth credentials, and every sandboxed launch
//! lists that file as unreadable. Accounts are `<namespace>.<owner>.<key>`, where the
//! namespace is derived from the data directory, so two `CYBER_HOME`s never share
//! entries, and the owner ([`cyber_core::plugins::secret_owner`]) names the plugin, its
//! scope and its checkout, so a same-named plugin elsewhere never shares them either.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use sha2::{Digest, Sha256};

use crate::mcp::oauth::SecretVault;

/// The keyring service of every plugin secret.
pub const SERVICE: &str = "cyber-plugin-config";

/// Plugin secrets by owner ([`cyber_core::plugins::secret_owner`]) and `userConfig`
/// property.
pub trait PluginSecrets: Send + Sync {
    fn get(&self, owner: &str, key: &str) -> Result<Option<String>, String>;
    fn put(&self, owner: &str, key: &str, value: &str) -> Result<(), String>;
    fn delete(&self, owner: &str, key: &str) -> Result<(), String>;
}

/// Plugin secrets in a [`SecretVault`]: the OS keyring, or a test double.
pub struct VaultSecrets {
    vault: Arc<dyn SecretVault>,
    namespace: String,
}

impl VaultSecrets {
    pub fn new(vault: Arc<dyn SecretVault>, data_dir: &Path) -> Self {
        let canonical = data_dir
            .canonicalize()
            .unwrap_or_else(|_| data_dir.to_path_buf());
        let digest = Sha256::digest(canonical.to_string_lossy().as_bytes());
        Self {
            vault,
            namespace: format!("{digest:x}")[..16].to_owned(),
        }
    }

    fn account(&self, plugin: &str, key: &str) -> String {
        format!("{}.{plugin}.{key}", self.namespace)
    }
}

impl PluginSecrets for VaultSecrets {
    fn get(&self, plugin: &str, key: &str) -> Result<Option<String>, String> {
        let bytes = self
            .vault
            .get(&self.account(plugin, key))
            .map_err(|error| error.to_string())?;
        bytes
            .map(|bytes| String::from_utf8(bytes).map_err(|_| "stored secret is not UTF-8".into()))
            .transpose()
    }

    fn put(&self, plugin: &str, key: &str, value: &str) -> Result<(), String> {
        self.vault
            .put(&self.account(plugin, key), value.as_bytes())
            .map_err(|error| error.to_string())
    }

    fn delete(&self, plugin: &str, key: &str) -> Result<(), String> {
        let account = self.account(plugin, key);
        self.vault
            .delete(&account)
            .map_err(|error| error.to_string())?;
        // A deletion the keyring reported but did not make is not claimed.
        match self.vault.get(&account) {
            Ok(None) => Ok(()),
            _ => Err("OS keyring delete failed".into()),
        }
    }
}

/// Plugin secrets in an owner-only JSON file (`<data>/plugin-secrets.json`).
#[cfg(unix)]
pub struct FileSecrets {
    path: PathBuf,
}

#[cfg(unix)]
impl FileSecrets {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    fn read(&self) -> Result<serde_json::Map<String, serde_json::Value>, String> {
        use std::os::unix::fs::MetadataExt;
        let metadata = match std::fs::symlink_metadata(&self.path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Default::default());
            }
            Err(_) => return Err("plugin secret file is unreadable".into()),
        };
        if !metadata.is_file() || metadata.mode() & 0o077 != 0 {
            return Err("plugin secret file must be a regular owner-only file".into());
        }
        let text =
            std::fs::read_to_string(&self.path).map_err(|_| "plugin secret file is unreadable")?;
        serde_json::from_str(&text).map_err(|_| "plugin secret file is invalid".into())
    }

    fn write(&self, map: &serde_json::Map<String, serde_json::Value>) -> Result<(), String> {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let parent = self
            .path
            .parent()
            .ok_or("plugin secret file has no directory")?;
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        let temp = parent.join(format!(".plugin-secrets-{}", ulid::Ulid::new()));
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temp)
            .map_err(|e| e.to_string())?;
        let text = serde_json::to_vec(map).map_err(|e| e.to_string())?;
        let written = file.write_all(&text).and_then(|()| file.sync_all());
        if let Err(error) = written.and_then(|()| std::fs::rename(&temp, &self.path)) {
            let _ = std::fs::remove_file(&temp);
            return Err(error.to_string());
        }
        Ok(())
    }
}

#[cfg(unix)]
impl PluginSecrets for FileSecrets {
    fn get(&self, plugin: &str, key: &str) -> Result<Option<String>, String> {
        Ok(self.read()?[plugin][key].as_str().map(str::to_owned))
    }

    fn put(&self, plugin: &str, key: &str, value: &str) -> Result<(), String> {
        let mut map = self.read()?;
        let entry = map.entry(plugin).or_insert_with(|| serde_json::json!({}));
        if !entry.is_object() {
            *entry = serde_json::json!({});
        }
        entry[key] = value.into();
        self.write(&map)
    }

    fn delete(&self, plugin: &str, key: &str) -> Result<(), String> {
        let mut map = self.read()?;
        if let Some(entry) = map.get_mut(plugin).and_then(|e| e.as_object_mut()) {
            entry.remove(key);
            if entry.is_empty() {
                map.remove(plugin);
            }
        }
        self.write(&map)
    }
}

/// Hide cyber's credential files in the data directory from every sandboxed launch of
/// this process. On the file backend the secret file is created (empty, owner-only)
/// first: a sandbox hides only paths that exist when it starts.
pub fn hide_credential_files(paths: &cyber_core::paths::Paths) {
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let store = FileSecrets::new(secret_file(paths));
        if !store.path.exists() {
            let _ = store.write(&Default::default());
        }
    }
    cyber_sandbox::hide_credential_file(secret_file(paths));
    cyber_sandbox::hide_credential_file(paths.mcp_auth_file());
}

/// This platform's plugin secret store, chosen at compile time and never by
/// configuration: the OS keyring on macOS and Windows, the owner-only file elsewhere.
pub fn open_secrets(paths: &cyber_core::paths::Paths) -> Arc<dyn PluginSecrets> {
    #[cfg(any(target_os = "macos", windows))]
    {
        Arc::new(VaultSecrets::new(
            Arc::new(crate::mcp::oauth::OsVault::for_service(SERVICE)),
            &paths.data,
        ))
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        Arc::new(FileSecrets::new(secret_file(paths)))
    }
}

/// The file backend's path.
pub fn secret_file(paths: &cyber_core::paths::Paths) -> PathBuf {
    paths.data.join("plugin-secrets.json")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::oauth::MemoryVault;

    #[test]
    fn vault_secrets_are_namespaced_by_data_directory() {
        let root = tempfile::tempdir().unwrap();
        let vault = Arc::new(MemoryVault::default());
        let one = VaultSecrets::new(vault.clone(), &root.path().join("a"));
        let two = VaultSecrets::new(vault.clone(), &root.path().join("b"));
        one.put("jira", "api_token", "s3cret").unwrap();
        assert_eq!(
            one.get("jira", "api_token").unwrap().as_deref(),
            Some("s3cret")
        );
        assert_eq!(two.get("jira", "api_token").unwrap(), None);
        one.delete("jira", "api_token").unwrap();
        assert!(vault.accounts().is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn file_secrets_are_owner_only_and_refuse_a_shared_file() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("plugin-secrets.json");
        let store = FileSecrets::new(path.clone());
        store.put("jira", "api_token", "s3cret").unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        assert_eq!(
            store.get("jira", "api_token").unwrap().as_deref(),
            Some("s3cret")
        );
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(store.get("jira", "api_token").is_err());
    }
}

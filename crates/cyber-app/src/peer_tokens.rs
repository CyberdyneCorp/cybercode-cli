//! Peer tokens on the sending machine (`cross-session-messaging` → Peer pairing and
//! grants): the `cyp_` token `cyber peers add` received in a pairing code, by remote name.
//!
//! Tokens go to the OS keyring (macOS Keychain, Windows Credential Manager) under the
//! service [`SERVICE`] and the account `peer:<name>`. Where there is none (Linux release
//! builds, or a keyring that refuses the write) they go to an owner-only file,
//! `<data>/peers-secrets.json`, and the caller warns ([`TokenStorage::warning`]); every
//! sandboxed launch lists that file as unreadable. Windows has no owner-only file, so there
//! a refused keyring write fails instead. A remote's `token` in user config (normally
//! `{env:NAME}`, for headless machines) takes precedence over both stores.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use cyber_core::config::{PeerRemote, PeersSettings};
use cyber_core::paths::Paths;
use cyber_server::runtime::messaging::PeerTarget;
use cyber_tools::mcp::oauth::SecretVault;
use serde_json::{Map, Value};

/// The keyring service of every peer token.
pub const SERVICE: &str = "cyber-code";

/// The keyring account of the token of remote `name`.
pub fn account(name: &str) -> String {
    format!("peer:{name}")
}

/// Where a token was stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenStorage {
    Keyring,
    File(PathBuf),
}

impl TokenStorage {
    /// The warning for the file fallback.
    pub fn warning(&self) -> Option<String> {
        match self {
            Self::Keyring => None,
            Self::File(path) => Some(format!(
                "peer tokens stored in file (no OS keyring): {}",
                path.display()
            )),
        }
    }
}

/// Peer tokens by remote name.
pub trait PeerTokens: Send + Sync {
    fn get(&self, name: &str) -> Result<Option<String>, String>;
    fn put(&self, name: &str, token: &str) -> Result<TokenStorage, String>;
    /// Removing an absent token succeeds.
    fn delete(&self, name: &str) -> Result<(), String>;
}

/// The keyring when there is one, the owner-only file otherwise.
pub struct PeerTokenStore {
    vault: Option<Arc<dyn SecretVault>>,
    file: PathBuf,
}

impl PeerTokenStore {
    pub fn new(vault: Option<Arc<dyn SecretVault>>, file: PathBuf) -> Self {
        Self { vault, file }
    }

    /// This platform's store, chosen at compile time and never by configuration.
    pub fn open(paths: &Paths) -> Self {
        Self::new(platform_vault(), paths.peer_tokens_file())
    }
}

#[cfg(any(target_os = "macos", windows))]
fn platform_vault() -> Option<Arc<dyn SecretVault>> {
    Some(Arc::new(cyber_tools::mcp::oauth::OsVault::for_service(
        SERVICE,
    )))
}

#[cfg(not(any(target_os = "macos", windows)))]
fn platform_vault() -> Option<Arc<dyn SecretVault>> {
    None
}

impl PeerTokens for PeerTokenStore {
    fn get(&self, name: &str) -> Result<Option<String>, String> {
        if let Some(vault) = &self.vault
            && let Ok(Some(bytes)) = vault.get(&account(name))
        {
            return String::from_utf8(bytes)
                .map(Some)
                .map_err(|_| "the stored peer token is not UTF-8".into());
        }
        Ok(read_file(&self.file)?
            .get(&account(name))
            .and_then(Value::as_str)
            .map(str::to_owned))
    }

    fn put(&self, name: &str, token: &str) -> Result<TokenStorage, String> {
        if let Some(vault) = &self.vault
            && vault.put(&account(name), token.as_bytes()).is_ok()
        {
            // A token left in the file by an earlier fallback would shadow nothing but
            // still sit on disk.
            remove_from_file(&self.file, name)?;
            return Ok(TokenStorage::Keyring);
        }
        let mut map = read_file(&self.file)?;
        map.insert(account(name), Value::from(token));
        write_file(&self.file, &map)?;
        Ok(TokenStorage::File(self.file.clone()))
    }

    fn delete(&self, name: &str) -> Result<(), String> {
        if let Some(vault) = &self.vault {
            vault
                .delete(&account(name))
                .map_err(|error| error.to_string())?;
            // A deletion the keyring reported but did not make is not claimed.
            if !matches!(vault.get(&account(name)), Ok(None)) {
                return Err("OS keyring delete failed".into());
            }
        }
        remove_from_file(&self.file, name)
    }
}

/// The token of `remote`: its `token` from user config, else the stored one.
pub fn resolve(remote: &PeerRemote, tokens: &dyn PeerTokens) -> Result<String, String> {
    if let Some(token) = remote.token.as_deref().filter(|t| !t.trim().is_empty()) {
        return Ok(token.to_string());
    }
    tokens.get(&remote.name)?.ok_or_else(|| {
        format!(
            "no token is stored for peer `{0}`; pair it again with `cyber peers add {0} '<code>'`",
            remote.name
        )
    })
}

/// The send target of the enabled remote `name` in the user configuration `config`, with
/// its token; `None` when no enabled remote has that name. Invalid `peers` settings or a
/// missing token are errors, so nothing is sent on a guess.
pub fn target(
    config: &Value,
    name: &str,
    tokens: &dyn PeerTokens,
) -> Result<Option<PeerTarget>, String> {
    let settings = PeersSettings::from_config(config)?;
    let Some(remote) = settings
        .remotes
        .into_iter()
        .find(|remote| remote.enabled && remote.name == name)
    else {
        return Ok(None);
    };
    let token = resolve(&remote, tokens)?;
    Ok(Some(PeerTarget {
        name: remote.name,
        url: remote.url.as_str().trim_end_matches('/').to_owned(),
        fingerprint: remote.fingerprint,
        token,
    }))
}

/// The names of the enabled remotes in the user configuration `config`, in config order.
pub fn enabled_names(config: &Value) -> Result<Vec<String>, String> {
    let settings = PeersSettings::from_config(config)?;
    Ok(settings
        .remotes
        .into_iter()
        .filter(|remote| remote.enabled)
        .map(|remote| remote.name)
        .collect())
}

/// The remotes whose token is in the file fallback, by name (`cyber doctor`). Windows
/// keeps no such file, so it lists none there.
pub fn file_token_names(paths: &Paths) -> Result<Vec<String>, String> {
    Ok(read_file(&paths.peer_tokens_file())?
        .keys()
        .filter_map(|account| account.strip_prefix("peer:"))
        .map(str::to_owned)
        .collect())
}

fn remove_from_file(path: &Path, name: &str) -> Result<(), String> {
    let mut map = read_file(path)?;
    if map.remove(&account(name)).is_some() {
        write_file(path, &map)?;
    }
    Ok(())
}

#[cfg(unix)]
fn read_file(path: &Path) -> Result<Map<String, Value>, String> {
    use std::os::unix::fs::MetadataExt;
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Map::new()),
        Err(_) => return Err("the peer token file is unreadable".into()),
    };
    if !metadata.is_file() || metadata.mode() & 0o077 != 0 {
        return Err(format!(
            "{} must be a regular owner-only file (mode 0600)",
            path.display()
        ));
    }
    let text = std::fs::read_to_string(path).map_err(|_| "the peer token file is unreadable")?;
    if text.trim().is_empty() {
        return Ok(Map::new());
    }
    serde_json::from_str(&text).map_err(|_| "the peer token file is invalid".into())
}

#[cfg(unix)]
fn write_file(path: &Path, map: &Map<String, Value>) -> Result<(), String> {
    let text = serde_json::to_vec(map).map_err(|error| error.to_string())?;
    cyber_core::trust::write_private_atomic(path, &text)
        .map_err(|error| format!("cannot write {}: {error}", path.display()))
}

/// No owner-only file without Unix modes: nothing is read from it.
#[cfg(not(unix))]
fn read_file(_path: &Path) -> Result<Map<String, Value>, String> {
    Ok(Map::new())
}

#[cfg(not(unix))]
fn write_file(_path: &Path, map: &Map<String, Value>) -> Result<(), String> {
    if map.is_empty() {
        return Ok(());
    }
    Err(
        "the OS keyring refused the peer token; give it as `{env:NAME}` with \
         `cyber peers add <name> '<code>' --token-env NAME`"
            .into(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use cyber_tools::mcp::oauth::MemoryVault;

    /// A keyring that refuses every write, as a locked or absent Secret Service does.
    struct RefusingVault;

    impl SecretVault for RefusingVault {
        fn put(&self, _: &str, _: &[u8]) -> Result<(), cyber_tools::mcp::oauth::OAuthError> {
            Err(cyber_tools::mcp::oauth::OAuthError::Storage(
                "refused".into(),
            ))
        }
        fn get(&self, _: &str) -> Result<Option<Vec<u8>>, cyber_tools::mcp::oauth::OAuthError> {
            Ok(None)
        }
        fn delete(&self, _: &str) -> Result<(), cyber_tools::mcp::oauth::OAuthError> {
            Ok(())
        }
    }

    fn remote(token: Option<&str>) -> PeerRemote {
        PeerRemote {
            name: "desk".into(),
            url: url::Url::parse("https://desk.local:4748").unwrap(),
            fingerprint: format!("sha256:{}", "0".repeat(64)),
            enabled: true,
            token: token.map(str::to_owned),
        }
    }

    #[test]
    fn the_keyring_holds_tokens_under_peer_accounts() {
        let dir = tempfile::tempdir().unwrap();
        let vault = Arc::new(MemoryVault::default());
        let store = PeerTokenStore::new(Some(vault.clone()), dir.path().join("peers-secrets.json"));
        assert_eq!(store.put("desk", "cyp_one").unwrap(), TokenStorage::Keyring);
        assert_eq!(TokenStorage::Keyring.warning(), None);
        assert_eq!(vault.accounts(), vec!["peer:desk".to_string()]);
        assert!(!dir.path().join("peers-secrets.json").exists());
        assert_eq!(store.get("desk").unwrap().as_deref(), Some("cyp_one"));
        assert_eq!(resolve(&remote(None), &store).unwrap(), "cyp_one");
        // A token from config (`{env:NAME}`, already substituted) wins.
        assert_eq!(
            resolve(&remote(Some("cyp_env")), &store).unwrap(),
            "cyp_env"
        );
        store.delete("desk").unwrap();
        assert!(vault.accounts().is_empty());
        let error = resolve(&remote(None), &store).unwrap_err();
        assert!(error.contains("cyber peers add desk"), "{error}");
    }

    #[cfg(unix)]
    #[test]
    fn without_a_keyring_tokens_live_in_an_owner_only_file() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("peers-secrets.json");
        for vault in [None, Some(Arc::new(RefusingVault) as Arc<dyn SecretVault>)] {
            let store = PeerTokenStore::new(vault, path.clone());
            let stored = store.put("desk", "cyp_one").unwrap();
            assert_eq!(stored, TokenStorage::File(path.clone()));
            let warning = stored.warning().unwrap();
            assert!(warning.starts_with("peer tokens stored in file (no OS keyring)"));
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
            assert_eq!(store.get("desk").unwrap().as_deref(), Some("cyp_one"));
            assert_eq!(store.get("nas").unwrap(), None);
            store.delete("desk").unwrap();
            assert_eq!(store.get("desk").unwrap(), None);
        }
        // A file others can read is refused, not used.
        let store = PeerTokenStore::new(None, path.clone());
        store.put("desk", "cyp_one").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(store.get("desk").unwrap_err().contains("owner-only"));
    }

    /// Without owner-only files, a refused keyring write fails and names `--token-env`.
    #[cfg(not(unix))]
    #[test]
    fn without_owner_only_files_a_refused_keyring_write_fails() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("peers-secrets.json");
        let store = PeerTokenStore::new(Some(Arc::new(RefusingVault)), path.clone());
        let error = store.put("desk", "cyp_one").unwrap_err();
        assert!(error.contains("--token-env"), "{error}");
        assert!(!path.exists());
        assert_eq!(store.get("desk").unwrap(), None);
        store.delete("desk").unwrap();
    }

    /// A token stored in the file by an earlier fallback is removed once the keyring
    /// takes it.
    #[cfg(unix)]
    #[test]
    fn a_keyring_write_clears_the_file_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("peers-secrets.json");
        PeerTokenStore::new(None, path.clone())
            .put("desk", "cyp_old")
            .unwrap();
        let store = PeerTokenStore::new(Some(Arc::new(MemoryVault::default())), path.clone());
        assert_eq!(store.put("desk", "cyp_new").unwrap(), TokenStorage::Keyring);
        assert!(!std::fs::read_to_string(&path).unwrap().contains("cyp_old"));
        assert_eq!(store.get("desk").unwrap().as_deref(), Some("cyp_new"));
    }

    /// The send target of a remote: only an enabled one, with the config token over the
    /// stored one; a remote without a token is an error, not a guess.
    #[test]
    fn send_targets_are_enabled_remotes_with_their_token() {
        let dir = tempfile::tempdir().unwrap();
        let store = PeerTokenStore::new(
            Some(Arc::new(MemoryVault::default())),
            dir.path().join("peers-secrets.json"),
        );
        store.put("desk", "cyp_stored").unwrap();
        let fingerprint = format!("sha256:{}", "0".repeat(64));
        let config = serde_json::json!({"peers": {"remotes": [
            {"name": "desk", "url": "https://desk.local:4748/", "fingerprint": fingerprint},
            {"name": "nas", "url": "https://nas.local:4748", "fingerprint": fingerprint,
                "enabled": false, "token": "cyp_nas"},
            {"name": "lab", "url": "http://127.0.0.1:4748", "fingerprint": fingerprint,
                "token": "cyp_config"},
            {"name": "ghost", "url": "https://ghost.local:4748", "fingerprint": fingerprint},
        ]}});
        let desk = target(&config, "desk", &store).unwrap().unwrap();
        assert_eq!(
            (desk.url.as_str(), desk.token.as_str()),
            ("https://desk.local:4748", "cyp_stored")
        );
        let lab = target(&config, "lab", &store).unwrap().unwrap();
        assert_eq!(lab.token, "cyp_config");
        assert!(target(&config, "nas", &store).unwrap().is_none());
        assert!(target(&config, "other", &store).unwrap().is_none());
        let missing = target(&config, "ghost", &store).unwrap_err();
        assert!(missing.contains("cyber peers add ghost"), "{missing}");
        let invalid = serde_json::json!({"peers": [{"name": "desk"}]});
        assert!(target(&invalid, "desk", &store).is_err());
    }
}

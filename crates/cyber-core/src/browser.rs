//! Optional browser verification (`browser-verification` → Supported optional browser
//! integration, Verification artifacts and lifecycle): the `browser` settings, the
//! version-pinned browser MCP definition `cyber browser install` writes, and the
//! workspace content hash that evidence is linked to.
//!
//! The browser is an ordinary MCP server definition under `mcp.<server>`, so trust, the
//! per-server digest approval and the managed MCP policy apply to it unchanged. With
//! `browser.enabled` the shared MCP pool leaves that server alone: each Session starts
//! its own process (a fresh profile), and the Session's end stops it.

use std::io::Read as _;
use std::path::Path;

use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};

/// The MCP server name `cyber browser install` writes and `browser.server` defaults to.
pub const DEFAULT_SERVER: &str = "browser";
/// The pinned browser MCP package. Upgrading it is a reviewed change of this constant.
pub const PINNED_PACKAGE: &str = "@playwright/mcp@0.0.41";
/// The tools a browser server must offer: navigation, DOM inspection, input,
/// screenshots, console messages and network requests.
pub const REQUIRED_TOOLS: &[&str] = &[
    "browser_navigate",
    "browser_snapshot",
    "browser_click",
    "browser_type",
    "browser_take_screenshot",
    "browser_console_messages",
    "browser_network_requests",
];
/// Every browser tool a Session is offered: the required ones plus vetted tools that
/// neither navigate by themselves nor reach files. Anything else the server publishes
/// (`browser_evaluate`, `browser_file_upload`, `browser_tabs`, `browser_navigate_back`,
/// …) is never offered or called: those bypass the per-navigation network check or
/// read files outside the sandbox's `deny_read`.
pub const EXPOSED_TOOLS: &[&str] = &[
    "browser_navigate",
    "browser_snapshot",
    "browser_click",
    "browser_type",
    "browser_take_screenshot",
    "browser_console_messages",
    "browser_network_requests",
    "browser_resize",
    "browser_hover",
    "browser_press_key",
    "browser_select_option",
    "browser_wait_for",
    "browser_handle_dialog",
];
/// Files hashed for one workspace hash at most; a larger tree has no hash.
const MAX_FILES: usize = 100_000;

/// `browser.{enabled, server}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserSettings {
    /// The `mcp` server that is the browser.
    pub server: String,
}

impl BrowserSettings {
    /// `None` unless `browser.enabled` is `true`: browser support stays off for projects
    /// that never ask for it.
    pub fn from_config(config: &Value) -> Result<Option<Self>, String> {
        let browser = match config.get("browser") {
            None | Some(Value::Null) => return Ok(None),
            Some(Value::Object(browser)) => browser,
            Some(_) => return Err("browser must be an object".into()),
        };
        for key in browser.keys() {
            if !matches!(key.as_str(), "enabled" | "server") {
                return Err(format!("browser.{key} is not a browser setting"));
            }
        }
        let enabled = match browser.get("enabled") {
            None | Some(Value::Null) => false,
            Some(Value::Bool(enabled)) => *enabled,
            Some(_) => return Err("browser.enabled must be a boolean".into()),
        };
        let server = match browser.get("server") {
            None | Some(Value::Null) => DEFAULT_SERVER.to_string(),
            Some(Value::String(name)) if valid_server_name(name) => name.clone(),
            Some(_) => {
                return Err(
                    "browser.server must name an MCP server (letters, digits, `-` and `_`)".into(),
                );
            }
        };
        Ok(enabled.then_some(Self { server }))
    }
}

fn valid_server_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// The pinned local definition: headless, and `--isolated` so the browser profile lives
/// in memory and never touches the user's default profile or cookies.
pub fn pinned_definition() -> Value {
    let npx = if cfg!(windows) { "npx.cmd" } else { "npx" };
    json!({
        "type": "local",
        "command": npx,
        "args": ["-y", PINNED_PACKAGE, "--headless", "--isolated"],
    })
}

/// A digest of the workspace content under `root`: every file Git would not ignore,
/// `.git` excluded, by `/`-separated relative path and contents (a link by its target).
/// Evidence recorded under one hash is stale once the hash differs.
pub fn workspace_hash(root: &Path) -> Result<String, String> {
    let mut files = Vec::new();
    let walk = ignore::WalkBuilder::new(root)
        .hidden(false)
        .git_global(false)
        .require_git(false)
        .follow_links(false)
        .filter_entry(|entry| entry.file_name() != ".git")
        .build();
    for entry in walk {
        let entry = entry.map_err(|error| format!("workspace hash: {error}"))?;
        if entry.file_type().is_some_and(|kind| !kind.is_dir()) {
            files.push(entry.into_path());
        }
        if files.len() > MAX_FILES {
            return Err(format!(
                "workspace hash: more than {MAX_FILES} files under {}",
                root.display()
            ));
        }
    }
    let mut named: Vec<(String, std::path::PathBuf)> = files
        .into_iter()
        .filter_map(|path| Some((relative(root, &path)?, path)))
        .collect();
    named.sort();
    let mut hasher = Sha256::new();
    for (name, path) in named {
        hasher.update(name.as_bytes());
        hasher.update([0]);
        hasher.update(file_digest(&path)?);
        hasher.update([0]);
    }
    Ok(format!("sha256:{:x}", hasher.finalize()))
}

fn relative(root: &Path, path: &Path) -> Option<String> {
    let parts: Vec<String> = path
        .strip_prefix(root)
        .ok()?
        .components()
        .map(|part| part.as_os_str().to_string_lossy().into_owned())
        .collect();
    Some(parts.join("/"))
}

fn file_digest(path: &Path) -> Result<[u8; 32], String> {
    let unreadable = |error: std::io::Error| format!("workspace hash: {}: {error}", path.display());
    let meta = std::fs::symlink_metadata(path).map_err(unreadable)?;
    let mut hasher = Sha256::new();
    if meta.file_type().is_symlink() {
        let target = std::fs::read_link(path).map_err(unreadable)?;
        hasher.update(b"link:");
        hasher.update(target.to_string_lossy().as_bytes());
        return Ok(hasher.finalize().into());
    }
    let mut file = std::fs::File::open(path).map_err(unreadable)?;
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let n = file.read(&mut buffer).map_err(unreadable)?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }
    Ok(hasher.finalize().into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_are_off_unless_enabled_and_reject_unknown_keys() {
        assert_eq!(BrowserSettings::from_config(&json!({})).unwrap(), None);
        assert_eq!(
            BrowserSettings::from_config(&json!({"browser": {"server": "web"}})).unwrap(),
            None
        );
        let on = BrowserSettings::from_config(&json!({"browser": {"enabled": true}})).unwrap();
        assert_eq!(on.unwrap().server, "browser");
        assert!(BrowserSettings::from_config(&json!({"browser": {"enabled": "yes"}})).is_err());
        assert!(BrowserSettings::from_config(&json!({"browser": {"profile": "x"}})).is_err());
        assert!(
            BrowserSettings::from_config(&json!({"browser": {"enabled": true, "server": "a b"}}))
                .is_err()
        );
    }

    #[test]
    fn pinned_definition_names_an_exact_version_and_an_isolated_profile() {
        let definition = pinned_definition();
        let args: Vec<&str> = definition["args"]
            .as_array()
            .unwrap()
            .iter()
            .map(|arg| arg.as_str().unwrap())
            .collect();
        assert!(args.contains(&PINNED_PACKAGE) && args.contains(&"--isolated"));
        let version = PINNED_PACKAGE.rsplit_once('@').unwrap().1;
        assert!(semver::Version::parse(version).is_ok(), "{version}");
    }

    #[test]
    fn workspace_hash_changes_with_content_and_ignores_git_and_ignored_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("index.html"), "<h1>v1</h1>").unwrap();
        std::fs::write(dir.path().join(".gitignore"), "dist/\n").unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/app.js"), "1").unwrap();
        let first = workspace_hash(dir.path()).unwrap();
        assert!(first.starts_with("sha256:"));
        std::fs::create_dir_all(dir.path().join(".git")).unwrap();
        std::fs::write(dir.path().join(".git/HEAD"), "ref").unwrap();
        std::fs::create_dir_all(dir.path().join("dist")).unwrap();
        std::fs::write(dir.path().join("dist/out.js"), "built").unwrap();
        assert_eq!(workspace_hash(dir.path()).unwrap(), first);
        std::fs::write(dir.path().join("src/app.js"), "2").unwrap();
        assert_ne!(workspace_hash(dir.path()).unwrap(), first);
    }
}

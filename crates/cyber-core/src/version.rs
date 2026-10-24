//! Build information embedded at compile time.

use std::fmt;
use std::path::PathBuf;

use serde::Serialize;

use crate::env::EnvSource;

/// Release channel of the running build.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Channel {
    Stable,
    Beta,
    Nightly,
    Dev,
}

impl Channel {
    pub fn parse(value: &str) -> Self {
        match value {
            "stable" => Self::Stable,
            "beta" => Self::Beta,
            "nightly" => Self::Nightly,
            _ => Self::Dev,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stable => "stable",
            Self::Beta => "beta",
            Self::Nightly => "nightly",
            Self::Dev => "dev",
        }
    }

    /// Every channel other than `stable` is a preview build.
    pub fn is_preview(self) -> bool {
        self != Self::Stable
    }
}

/// Version, channel, git SHA, build identity and target triple of this binary.
#[derive(Debug, Clone, Serialize)]
pub struct BuildInfo {
    pub version: &'static str,
    pub channel: Channel,
    pub git_sha: &'static str,
    /// What a client compares with a running server: the commit (`.dirty` when tracked
    /// files differed), `CYBER_BUILD_ID`, or the build time without git.
    pub build: &'static str,
    pub target: &'static str,
}

pub fn build_info() -> BuildInfo {
    BuildInfo {
        version: env!("CARGO_PKG_VERSION"),
        channel: Channel::parse(env!("CYBER_BUILD_CHANNEL")),
        git_sha: env!("CYBER_BUILD_GIT_SHA"),
        build: env!("CYBER_BUILD_ID"),
        target: env!("CYBER_BUILD_TARGET"),
    }
}

/// Where the build identity came from: `git`, `env` or `time`.
pub fn build_id_source() -> &'static str {
    env!("CYBER_BUILD_ID_SOURCE")
}

/// The executable this process runs, read once: its canonical path and a stamp of its
/// length and modification time. A rebuild in place keeps the path and changes the stamp,
/// which the commit alone misses for uncommitted edits.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct Binary {
    pub path: Option<String>,
    pub stamp: Option<String>,
}

pub fn binary() -> &'static Binary {
    static BINARY: std::sync::OnceLock<Binary> = std::sync::OnceLock::new();
    BINARY.get_or_init(|| {
        let Ok(exe) = std::env::current_exe() else {
            return Binary::default();
        };
        let path = std::fs::canonicalize(&exe).unwrap_or(exe);
        let stamp = std::fs::metadata(&path).ok().map(|m| {
            let modified = m
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_nanos());
            format!("{:x}-{modified:x}", m.len())
        });
        Binary {
            path: Some(path.display().to_string()),
            stamp,
        }
    })
}

/// The warning `exec` and the TUI show on every start of a version older than the
/// release manifest's `min_supported` (`installation-upgrade` → Minimum supported version).
pub fn unsupported_warning(current: &str, min_supported: &str) -> String {
    format!(
        "warning: cyber {current} is no longer supported (minimum {min_supported}); run \"cyber upgrade\""
    )
}

/// Where releases come from and whom they must be signed by (`installation-upgrade` →
/// Version and channel constants). Only build variables (`CYBER_RELEASE_BASE_URL`,
/// `CYBER_RELEASE_IDENTITY`, `CYBER_RELEASE_ISSUER`) change it in a release build.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReleaseSource {
    /// `v<version>/` below it holds the archives and `SHA256SUMS`;
    /// `channel-<channel>/<channel>.json` holds the channel manifest.
    pub base_url: String,
    /// The expected Sigstore certificate identity, before `v<version>`.
    pub identity_prefix: String,
    /// The expected OIDC issuer of that certificate.
    pub issuer: String,
    /// Debug builds only (`CYBER_TEST_RELEASE_TRUST`): a trusted root replacing the one
    /// embedded in the binary.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub test_trusted_root: Option<PathBuf>,
}

impl ReleaseSource {
    /// The source this binary was built with.
    pub fn built_in() -> Self {
        Self {
            base_url: env!("CYBER_BUILD_RELEASE_BASE_URL").into(),
            identity_prefix: env!("CYBER_BUILD_RELEASE_IDENTITY").into(),
            issuer: env!("CYBER_BUILD_RELEASE_ISSUER").into(),
            test_trusted_root: None,
        }
    }

    /// The source in effect: the built-in one, or in a debug build the test hook.
    pub fn resolve(env: &dyn EnvSource) -> Result<Self, String> {
        #[cfg(debug_assertions)]
        return test_hook(Self::built_in(), env);
        #[cfg(not(debug_assertions))]
        {
            let _ = env;
            Ok(Self::built_in())
        }
    }

    fn base(&self) -> &str {
        self.base_url.trim_end_matches('/')
    }

    /// `<base>/channel-<channel>/<channel>.json`.
    pub fn manifest_url(&self, channel: Channel) -> String {
        let channel = channel.as_str();
        format!("{}/channel-{channel}/{channel}.json", self.base())
    }

    /// `<base>/v<version>/<name>`.
    pub fn asset_url(&self, version: &str, name: &str) -> String {
        format!("{}/v{version}/{name}", self.base())
    }

    /// `cyber-<version>-<target>.zip` on Windows, `.tar.gz` elsewhere.
    pub fn archive_name(version: &str, target: &str) -> String {
        let ext = if target.contains("windows") {
            "zip"
        } else {
            "tar.gz"
        };
        format!("cyber-{version}-{target}.{ext}")
    }

    /// The certificate identity a release of `version` must be signed with.
    pub fn identity(&self, version: &str) -> String {
        format!("{}v{version}", self.identity_prefix)
    }
}

/// The documented test hook (`docs/local-core-guide.md` → Testing), compiled into debug
/// builds only: `CYBER_TEST_RELEASE_BASE_URL` replaces the base URL, and
/// `CYBER_TEST_RELEASE_TRUST=<dir>` replaces the trusted root and identity with
/// `<dir>/trusted_root.json` and `<dir>/identity.json` (`{identity_prefix, issuer}`).
#[cfg(debug_assertions)]
fn test_hook(mut source: ReleaseSource, env: &dyn EnvSource) -> Result<ReleaseSource, String> {
    if let Some(base) = env.get("CYBER_TEST_RELEASE_BASE_URL") {
        source.base_url = base;
    }
    if let Some(dir) = env.get("CYBER_TEST_RELEASE_TRUST") {
        let dir = PathBuf::from(dir);
        let path = dir.join("identity.json");
        let text = std::fs::read_to_string(&path)
            .map_err(|e| format!("CYBER_TEST_RELEASE_TRUST: {}: {e}", path.display()))?;
        let identity: serde_json::Value = serde_json::from_str(&text)
            .map_err(|e| format!("CYBER_TEST_RELEASE_TRUST: {}: {e}", path.display()))?;
        let field = |key: &str| {
            identity[key].as_str().map(str::to_string).ok_or_else(|| {
                format!(
                    "CYBER_TEST_RELEASE_TRUST: {}: missing \"{key}\"",
                    path.display()
                )
            })
        };
        source.identity_prefix = field("identity_prefix")?;
        source.issuer = field("issuer")?;
        source.test_trusted_root = Some(dir.join("trusted_root.json"));
    }
    Ok(source)
}

impl fmt::Display for BuildInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "cyber {} ({}, {}, {})",
            self.version,
            self.channel.as_str(),
            self.git_sha,
            self.target
        )
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    #[test]
    fn built_in_release_source_is_the_github_releases_of_the_repository() {
        let source = ReleaseSource::built_in();
        assert_eq!(
            source.base_url,
            "https://github.com/CyberdyneCorp/cybercode-cli/releases/download"
        );
        assert_eq!(
            source.identity("1.5.0"),
            "https://github.com/CyberdyneCorp/cybercode-cli/.github/workflows/release.yml@refs/tags/v1.5.0"
        );
        assert_eq!(source.issuer, "https://token.actions.githubusercontent.com");
        assert_eq!(source.test_trusted_root, None);
    }

    #[test]
    fn asset_urls_follow_the_release_layout() {
        let source = ReleaseSource {
            base_url: "http://127.0.0.1:8000/dl/".into(),
            ..ReleaseSource::built_in()
        };
        assert_eq!(
            source.manifest_url(Channel::Beta),
            "http://127.0.0.1:8000/dl/channel-beta/beta.json"
        );
        assert_eq!(
            source.asset_url("1.4.0", "SHA256SUMS"),
            "http://127.0.0.1:8000/dl/v1.4.0/SHA256SUMS"
        );
        assert_eq!(
            ReleaseSource::archive_name("1.4.0", "aarch64-apple-darwin"),
            "cyber-1.4.0-aarch64-apple-darwin.tar.gz"
        );
        assert_eq!(
            ReleaseSource::archive_name("1.4.0", "x86_64-pc-windows-msvc"),
            "cyber-1.4.0-x86_64-pc-windows-msvc.zip"
        );
    }

    #[test]
    fn without_the_hook_the_built_in_source_is_used() {
        let env = HashMap::<String, String>::new();
        assert_eq!(
            ReleaseSource::resolve(&env).unwrap(),
            ReleaseSource::built_in()
        );
    }

    #[cfg(debug_assertions)]
    #[test]
    fn debug_hook_replaces_the_base_url_and_trust() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("identity.json"),
            r#"{"identity_prefix": "https://example.test/release@refs/tags/", "issuer": "https://issuer.test"}"#,
        )
        .unwrap();
        let env = HashMap::from([
            (
                "CYBER_TEST_RELEASE_BASE_URL".to_string(),
                "http://127.0.0.1:9".to_string(),
            ),
            (
                "CYBER_TEST_RELEASE_TRUST".to_string(),
                dir.path().display().to_string(),
            ),
        ]);
        let source = ReleaseSource::resolve(&env).unwrap();
        assert_eq!(source.base_url, "http://127.0.0.1:9");
        assert_eq!(
            source.identity("2.0.0"),
            "https://example.test/release@refs/tags/v2.0.0"
        );
        assert_eq!(source.issuer, "https://issuer.test");
        assert_eq!(
            source.test_trusted_root,
            Some(dir.path().join("trusted_root.json"))
        );
    }

    #[cfg(debug_assertions)]
    #[test]
    fn debug_hook_without_an_identity_file_fails_closed() {
        let dir = tempfile::tempdir().unwrap();
        let env = HashMap::from([(
            "CYBER_TEST_RELEASE_TRUST".to_string(),
            dir.path().display().to_string(),
        )]);
        let error = ReleaseSource::resolve(&env).unwrap_err();
        assert!(error.contains("identity.json"), "{error}");
    }
}

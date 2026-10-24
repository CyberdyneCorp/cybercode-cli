//! Upgrade planning and the verified self-update (`installation-upgrade` → Upgrade
//! command, Self-update integrity). A plan is pure: skip, replace the binary, run the
//! package manager's argv, or print the manual command. The self-update downloads and
//! verifies everything before the first rename, so any failure leaves the installed
//! binary byte-identical.

use std::path::Path;
use std::time::Duration;

use cyber_core::env::EnvSource;
use cyber_core::version::ReleaseSource;
use serde::Serialize;

use crate::archive::{self, Members};
use crate::fetch::Fetcher;
use crate::method::InstallMethod;
use crate::replace::{self, Swap};
use crate::verify::{Sums, Trust};

/// The repository `cargo install` builds from until a crate is published.
pub const CARGO_GIT: &str = "https://github.com/CyberdyneCorp/cybercode-cli";

/// What `cyber upgrade` does for one method and target version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Plan {
    /// The target is the running version.
    Current,
    /// Download, verify and replace the binary (`script`).
    Replace,
    /// Run the package manager, as an argv.
    Manager { argv: Vec<String> },
    /// Print the command for the user to run (`nix`, `aur`, `docker`, `unknown`).
    Manual { command: String },
}

impl Plan {
    /// The plan for a version the user named: anything but the running version is
    /// installed, an older one as a deliberate downgrade.
    pub fn new(method: InstallMethod, from: &str, to: &str) -> Self {
        if from == to {
            return Self::Current;
        }
        if method == InstallMethod::Script {
            return Self::Replace;
        }
        match manager_argv(method, to) {
            Some(argv) => Self::Manager { argv },
            None => Self::Manual {
                command: manual_command(method, to),
            },
        }
    }

    /// The plan for a channel manifest's version, which is unsigned: only a newer one by
    /// semver is installed, so an older or replayed manifest never downgrades.
    pub fn to_latest(method: InstallMethod, from: &str, latest: &str) -> Self {
        if crate::policy::bump(from, latest).is_none() {
            return Self::Current;
        }
        Self::new(method, from, latest)
    }

    /// `current`, `replaced`, `manager` or `manual`, as `--format json` reports it.
    pub fn action(&self) -> &'static str {
        match self {
            Self::Current => "current",
            Self::Replace => "replaced",
            Self::Manager { .. } => "manager",
            Self::Manual { .. } => "manual",
        }
    }
}

/// The upgrade command of a package manager, as an argv run without a shell.
fn manager_argv(method: InstallMethod, version: &str) -> Option<Vec<String>> {
    let package = format!("@cyber-code/cli@{version}");
    let tag = format!("v{version}");
    let argv = match method {
        InstallMethod::Brew => vec!["brew", "upgrade", "cyber"],
        InstallMethod::Npm => vec!["npm", "i", "-g", &package],
        InstallMethod::Pnpm => vec!["pnpm", "add", "-g", &package],
        InstallMethod::Bun => vec!["bun", "add", "-g", &package],
        InstallMethod::Cargo => vec![
            "cargo",
            "install",
            "--locked",
            "--git",
            CARGO_GIT,
            "--tag",
            &tag,
            "cyber-cli",
        ],
        InstallMethod::Scoop => vec!["scoop", "update", "cyber"],
        InstallMethod::Winget => vec!["winget", "upgrade", "cyber-code.cyber"],
        _ => return None,
    };
    Some(argv.into_iter().map(str::to_string).collect())
}

/// What the user runs for a method cyber does not drive itself.
fn manual_command(method: InstallMethod, version: &str) -> String {
    match method {
        InstallMethod::Nix => "nix profile upgrade cyber-code".into(),
        InstallMethod::Aur => "yay -Syu cyber-code".into(),
        InstallMethod::Docker => format!("docker pull ghcr.io/cyber-code/cyber:{version}"),
        _ => format!("curl -fsSL {CARGO_GIT}/releases/latest/download/install.sh | sh"),
    }
}

/// Run a package manager's `argv` without a shell, the program resolved on `PATH`; the
/// error names the command so the user can run it.
pub fn run_manager(argv: &[String], env: &dyn EnvSource) -> Result<(), String> {
    let command = argv.join(" ");
    let failed = |reason: String| format!("{command}: {reason}");
    let (program, args) = argv
        .split_first()
        .ok_or_else(|| failed("empty command".into()))?;
    let path = env.get("PATH").unwrap_or_default();
    let pathext = env.get("PATHEXT");
    let program = crate::method::which(program, std::ffi::OsStr::new(&path), pathext.as_deref())
        .ok_or_else(|| failed("not found on PATH".into()))?;
    let status = std::process::Command::new(program)
        .args(args)
        .status()
        .map_err(|e| failed(e.to_string()))?;
    if !status.success() {
        return Err(failed(format!("exited with {status}")));
    }
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum UpgradeError {
    #[error(transparent)]
    Fetch(#[from] crate::fetch::FetchError),
    #[error(transparent)]
    Verify(#[from] crate::verify::VerifyError),
    #[error(transparent)]
    Archive(#[from] crate::archive::ArchiveError),
    #[error("replacing {path}: {source}")]
    Replace {
        path: String,
        #[source]
        source: std::io::Error,
    },
}

/// One verified self-update of the binary at `exe` to `version` for `target`.
pub struct SelfUpdate<'a> {
    pub source: &'a ReleaseSource,
    pub version: &'a str,
    /// The target triple of the running build.
    pub target: &'a str,
    /// The installed binary, as `current_exe` reports it.
    pub exe: &'a Path,
    pub timeout: Duration,
}

impl SelfUpdate<'_> {
    /// Verify `SHA256SUMS`, download and check the archive, extract the binaries beside
    /// `exe`, then replace the helper and `cyber`.
    pub async fn run(&self) -> Result<(), UpgradeError> {
        let trust = Trust::for_source(self.source)?;
        let fetcher = Fetcher::new(self.timeout)?;
        let url = |name: &str| self.source.asset_url(self.version, name);
        let sums_bytes = fetcher.bytes(&url("SHA256SUMS")).await?;
        let bundle = fetcher.bytes(&url("SHA256SUMS.sigstore.json")).await?;
        trust.verify(
            &sums_bytes,
            &bundle,
            &self.source.identity(self.version),
            &self.source.issuer,
        )?;
        let sums = Sums::parse(&String::from_utf8_lossy(&sums_bytes))?;
        let name = ReleaseSource::archive_name(self.version, self.target);
        if sums.get(&name).is_none() {
            return Err(crate::verify::VerifyError::Missing(self.target.to_string()).into());
        }
        let dir = self.exe.parent().unwrap_or(Path::new("."));
        let download = fetcher.to_file(&url(&name), dir).await?;
        sums.check_file(&name, &download)?;
        let extracted = archive::extract(&download, &name, Members::for_target(self.target), dir)?;
        drop(download);
        self.install(extracted)
    }

    fn install(&self, extracted: archive::Extracted) -> Result<(), UpgradeError> {
        let cyber = Swap {
            target: self.exe.to_path_buf(),
            new: extracted.cyber.to_path_buf(),
        };
        let helper = extracted.helper.as_ref().map(|new| Swap {
            target: replace::helper_path(self.exe),
            new: new.to_path_buf(),
        });
        replace::install(&cyber, helper.as_ref()).map_err(|source| UpgradeError::Replace {
            path: self.exe.display().to_string(),
            source,
        })
        // The staged paths drop here; after a rename they no longer exist.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plans_per_method() {
        assert_eq!(
            Plan::new(InstallMethod::Brew, "1.4.0", "1.4.0"),
            Plan::Current
        );
        assert_eq!(
            Plan::new(InstallMethod::Script, "1.4.0", "1.5.0"),
            Plan::Replace
        );
        let argv = |method| match Plan::new(method, "1.4.0", "1.5.0") {
            Plan::Manager { argv } => argv.join(" "),
            other => panic!("{other:?}"),
        };
        assert_eq!(argv(InstallMethod::Brew), "brew upgrade cyber");
        assert_eq!(argv(InstallMethod::Npm), "npm i -g @cyber-code/cli@1.5.0");
        assert_eq!(
            argv(InstallMethod::Pnpm),
            "pnpm add -g @cyber-code/cli@1.5.0"
        );
        assert_eq!(argv(InstallMethod::Bun), "bun add -g @cyber-code/cli@1.5.0");
        assert_eq!(
            argv(InstallMethod::Cargo),
            format!("cargo install --locked --git {CARGO_GIT} --tag v1.5.0 cyber-cli")
        );
        assert_eq!(argv(InstallMethod::Scoop), "scoop update cyber");
        assert_eq!(
            argv(InstallMethod::Winget),
            "winget upgrade cyber-code.cyber"
        );
        let manual = |method| match Plan::new(method, "1.4.0", "1.5.0") {
            Plan::Manual { command } => command,
            other => panic!("{other:?}"),
        };
        assert_eq!(manual(InstallMethod::Nix), "nix profile upgrade cyber-code");
        assert_eq!(manual(InstallMethod::Aur), "yay -Syu cyber-code");
        assert!(manual(InstallMethod::Docker).ends_with(":1.5.0"));
        assert!(manual(InstallMethod::Unknown).contains("install.sh"));
    }

    /// Regression: a manifest naming an older release (a backport tag, a replayed or
    /// overwritten manifest, a pre-release build reading `stable`) is not a downgrade.
    #[test]
    fn latest_plans_only_newer_versions() {
        let script = InstallMethod::Script;
        assert_eq!(Plan::to_latest(script, "1.4.0", "1.3.5"), Plan::Current);
        assert_eq!(Plan::to_latest(script, "1.4.0", "1.4.0"), Plan::Current);
        assert_eq!(
            Plan::to_latest(InstallMethod::Npm, "1.5.0-beta.1", "1.4.0"),
            Plan::Current
        );
        assert_eq!(
            Plan::to_latest(script, "1.4.0", "not-semver"),
            Plan::Current
        );
        assert_eq!(Plan::to_latest(script, "1.4.0", "1.4.1"), Plan::Replace);
        assert_eq!(
            Plan::to_latest(script, "1.5.0-beta.1", "1.5.0-beta.2"),
            Plan::Replace
        );
        // A version the user names is installed even when older.
        assert_eq!(Plan::new(script, "1.4.0", "1.3.5"), Plan::Replace);
    }
}

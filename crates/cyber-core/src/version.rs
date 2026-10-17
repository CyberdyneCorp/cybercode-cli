//! Build information embedded at compile time.

use std::fmt;

use serde::Serialize;

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

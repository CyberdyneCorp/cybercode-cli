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

/// Version, channel, git SHA and target triple of this binary.
#[derive(Debug, Clone, Serialize)]
pub struct BuildInfo {
    pub version: &'static str,
    pub channel: Channel,
    pub git_sha: &'static str,
    pub target: &'static str,
}

pub fn build_info() -> BuildInfo {
    BuildInfo {
        version: env!("CARGO_PKG_VERSION"),
        channel: Channel::parse(env!("CYBER_BUILD_CHANNEL")),
        git_sha: env!("CYBER_BUILD_GIT_SHA"),
        target: env!("CYBER_BUILD_TARGET"),
    }
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

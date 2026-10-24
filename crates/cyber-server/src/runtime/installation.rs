//! The last background update check (`installation-upgrade` → Background update policy):
//! what `GET /service/update` answers, and the live-only `installation.update_available.1`
//! and `installation.updated.1` events. The check itself runs in the application.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{LiveEvent, Runtime};

/// The last update check, or the running build before the first one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UpdateStatus {
    /// The running server's version.
    pub current: String,
    /// The channel looked up (`dev` builds look up `stable`).
    pub channel: String,
    /// How the server binary was installed, once a check detected it.
    pub method: Option<String>,
    /// The channel's latest version; `null` before the first check.
    pub latest: Option<String>,
    /// `major`, `minor` or `patch` when `latest` is newer than `current`.
    pub kind: Option<String>,
    pub notes_url: Option<String>,
    /// The oldest supported version, from the cached or fetched manifest.
    pub min_supported: Option<String>,
    /// `current` is older than `min_supported`.
    pub unsupported: bool,
    /// A silent patch upgrade installed `latest`; clients restart to use it.
    pub updated: bool,
    pub checked_at_ms: Option<u64>,
}

impl UpdateStatus {
    /// The running build before any manifest was read.
    pub fn initial(current: &str, channel: &str) -> Self {
        Self {
            current: current.into(),
            channel: channel.into(),
            method: None,
            latest: None,
            kind: None,
            notes_url: None,
            min_supported: None,
            unsupported: false,
            updated: false,
            checked_at_ms: None,
        }
    }
}

/// The data of `installation.update_available.1` and `installation.updated.1`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct InstallationNotice {
    pub current: String,
    pub latest: String,
    /// `major`, `minor` or `patch`.
    pub kind: String,
    pub notes_url: Option<String>,
}

impl Runtime {
    /// The last recorded update check, if any.
    pub fn update_status(&self) -> Option<UpdateStatus> {
        self.inner
            .update_status
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub fn set_update_status(&self, status: UpdateStatus) {
        *self
            .inner
            .update_status
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(status);
    }

    /// Publish `installation.update_available.1`, or `installation.updated.1` after a
    /// silent upgrade; never durable.
    pub fn publish_installation(&self, notice: InstallationNotice, updated: bool) {
        self.inner.bus.publish(if updated {
            LiveEvent::InstallationUpdated { notice }
        } else {
            LiveEvent::InstallationUpdateAvailable { notice }
        });
    }
}

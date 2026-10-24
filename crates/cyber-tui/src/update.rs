//! Update notices (`tui` → Update notices; `installation-upgrade` → Background update
//! policy, Minimum supported version): a toast when the server's check found a newer
//! version or installed one silently, from the live `installation.*` event or, for a
//! TUI that attaches after the check, from `GET /service/update`; and a banner while the
//! running version is older than the release manifest's `min_supported`. Nothing here
//! restarts or interrupts a Session.

use cyber_client::Client;
use serde_json::Value;

use crate::app::{Action, App};
use crate::perform::Msg;

/// What this TUI already showed.
#[derive(Debug, Default)]
pub struct Notices {
    /// The last update toast, so a reconnect does not repeat it.
    shown: Option<String>,
    /// The unsupported-version warning, shown above the transcript.
    pub banner: Option<String>,
}

/// `GET /service/update`; a server without it (an older build) shows nothing.
pub(crate) async fn perform(client: &Client) -> Result<Msg, String> {
    Ok(Msg::Update(client.get("/service/update").await.ok()))
}

impl App {
    /// `installation.update_available.1` or `installation.updated.1`.
    pub(crate) fn installation_event(&mut self, event: &cyber_client::Event) -> Vec<Action> {
        if let Some(latest) = event.data["latest"].as_str() {
            self.update_notice(latest, event.kind == "installation.updated.1");
        }
        Vec::new()
    }

    /// The last check, read on attach: the toast when it found a newer version, and the
    /// banner when the running version is unsupported.
    pub(crate) fn update_status(&mut self, status: Option<Value>) {
        let Some(status) = status else {
            return;
        };
        if status["unsupported"] == true
            && let (Some(current), Some(minimum)) =
                (status["current"].as_str(), status["min_supported"].as_str())
        {
            self.update.banner = Some(cyber_core::version::unsupported_warning(current, minimum));
        }
        if status["kind"].is_string()
            && let Some(latest) = status["latest"].as_str()
        {
            self.update_notice(latest, status["updated"] == true);
        }
    }

    fn update_notice(&mut self, latest: &str, updated: bool) {
        let latest = crate::sidebar::clean(latest);
        let text = if updated {
            format!("cyber updated to {latest}; restart cyber to use it")
        } else {
            format!("cyber {latest} is available — run \"cyber upgrade\"")
        };
        if self.update.shown.as_ref() != Some(&text) {
            self.toast(text.clone());
            self.update.shown = Some(text);
        }
    }
}

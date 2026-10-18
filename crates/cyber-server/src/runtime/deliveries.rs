//! What the agent delivers to the user (`background-tasks` → Notify tool, Send file to
//! user): notifications, rate-limited per Session, and files copied into managed storage
//! so clients can download exactly what was sent. Both are durable Session events every
//! client sees; neither enters the model-visible history.

use serde::{Deserialize, Serialize};

use super::events::event;
use super::{Runtime, RuntimeError};

/// `{ call_id, title, body }`.
pub const NOTIFICATION_SENT: &str = "session.notification.sent.1";
/// `{ call_id, message_id, caption, files }`.
pub const FILES_SENT: &str = "session.files.sent.1";
/// Notifications a Session may send per [`NOTIFY_WINDOW_MS`].
pub const NOTIFY_LIMIT: usize = 10;
pub const NOTIFY_WINDOW_MS: i64 = 3_600_000;
/// Files a Session may send over its lifetime, and their total size: each is a copy in
/// managed storage until the Session is deleted or retention sweeps it.
pub const SEND_LIMIT_FILES: usize = 100;
pub const SEND_LIMIT_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Notification {
    pub call_id: String,
    pub title: String,
    pub body: String,
}

/// One delivered file. `stored_path` is the server's copy; clients download it with
/// `GET /sessions/{id}/files/{file_id}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SentFile {
    pub id: String,
    /// The file name shown to the user.
    pub name: String,
    /// The path in the Location it was sent from.
    pub source: String,
    pub size: u64,
    pub media_type: String,
    pub sha256: String,
    pub stored_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct FilesSent {
    pub call_id: String,
    /// The assistant message whose call sent them.
    pub message_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caption: Option<String>,
    pub files: Vec<SentFile>,
}

/// The newest notification times kept per Session, enough to apply the limit.
pub(super) fn remember_notification(times: &mut Vec<i64>, time_ms: i64) {
    times.push(time_ms);
    let over = times.len().saturating_sub(NOTIFY_LIMIT);
    times.drain(..over);
}

/// Whether one more notification at `now_ms` stays within the limit.
fn within_limit(times: &[i64], now_ms: i64) -> bool {
    times
        .iter()
        .filter(|time| now_ms - **time < NOTIFY_WINDOW_MS)
        .count()
        < NOTIFY_LIMIT
}

fn within_send_limit(
    sent: &std::collections::BTreeMap<String, SentFile>,
    files: usize,
    bytes: u64,
) -> Result<(), RuntimeError> {
    let total: u64 = sent.values().map(|file| file.size).sum();
    if sent.len() + files > SEND_LIMIT_FILES || total.saturating_add(bytes) > SEND_LIMIT_BYTES {
        return Err(RuntimeError::Invalid(format!(
            "Send limit reached: a session sends at most {SEND_LIMIT_FILES} files and {} MB",
            SEND_LIMIT_BYTES / (1024 * 1024)
        )));
    }
    Ok(())
}

impl Runtime {
    /// Record a notification; past [`NOTIFY_LIMIT`] in the last hour it is refused.
    pub async fn record_notification(
        &self,
        session_id: &str,
        notification: Notification,
    ) -> Result<(), RuntimeError> {
        let handle = self.inner.handle(session_id).await?;
        let mut state = handle.state.lock().await;
        let now = chrono::Utc::now().timestamp_millis();
        if !within_limit(&state.notifications, now) {
            return Err(RuntimeError::Invalid(format!(
                "Notification limit reached ({NOTIFY_LIMIT} per hour)"
            )));
        }
        self.inner
            .commit_locked(&mut state, vec![event(NOTIFICATION_SENT, &notification)])?;
        Ok(())
    }

    /// Whether the Session may send `files` more files of `bytes` in total within
    /// [`SEND_LIMIT_FILES`] and [`SEND_LIMIT_BYTES`]; checked before anything is copied.
    pub async fn admit_sent_files(
        &self,
        session_id: &str,
        files: usize,
        bytes: u64,
    ) -> Result<(), RuntimeError> {
        let handle = self.inner.handle(session_id).await?;
        let state = handle.state.lock().await;
        within_send_limit(&state.sent_files, files, bytes)
    }

    /// Record files delivered to the user, within the Session's send limit.
    pub async fn record_sent_files(
        &self,
        session_id: &str,
        sent: FilesSent,
    ) -> Result<(), RuntimeError> {
        let handle = self.inner.handle(session_id).await?;
        let mut state = handle.state.lock().await;
        let bytes = sent.files.iter().map(|file| file.size).sum();
        within_send_limit(&state.sent_files, sent.files.len(), bytes)?;
        self.inner
            .commit_locked(&mut state, vec![event(FILES_SENT, &sent)])?;
        Ok(())
    }

    /// A file the Session delivered.
    pub async fn sent_file(
        &self,
        session_id: &str,
        file_id: &str,
    ) -> Result<Option<SentFile>, RuntimeError> {
        let handle = self.inner.handle(session_id).await?;
        let state = handle.state.lock().await;
        Ok(state.sent_files.get(file_id).cloned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_eleventh_notification_within_an_hour_is_over_the_limit() {
        let mut times = Vec::new();
        for minute in 0..10 {
            assert!(within_limit(&times, minute * 60_000));
            remember_notification(&mut times, minute * 60_000);
        }
        assert!(!within_limit(&times, 30 * 60_000));
        // The first one leaves the window an hour after it was sent.
        assert!(within_limit(&times, NOTIFY_WINDOW_MS));
        remember_notification(&mut times, NOTIFY_WINDOW_MS);
        assert_eq!(times.len(), NOTIFY_LIMIT);
    }
}

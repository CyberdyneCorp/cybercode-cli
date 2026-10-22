//! Unattended asks (`goals` → Goal mode safety): a permission request raised while the
//! Session works on a Goal, that no client watches for `goals.unattended_ask_seconds`,
//! pauses the Goal with `Waiting for approval` and notifies. The request stays pending,
//! so a client that attaches later can still answer it; the paused Goal does not
//! continue after the Turn ends until it is resumed.

use std::time::{Duration, Instant};

use serde_json::json;

use super::records::{self, Paused};
use crate::runtime::events::event;
use crate::runtime::presence::Watching;
use crate::runtime::{PendingKind, PendingRequest, Runtime, RuntimeError};

/// The pause reason (`goals` → Goal mode safety).
pub const WAITING_FOR_APPROVAL: &str = "Waiting for approval";

impl Runtime {
    /// Watch a just-opened permission request of a Session with an active Goal.
    pub(crate) fn watch_unattended_ask(&self, request: &PendingRequest) {
        if !matches!(request.kind, PendingKind::Permission(_)) {
            return;
        }
        let weak = self.downgrade();
        let closed = self.inner.closed.clone();
        let request = request.clone();
        let task = tokio::spawn(async move {
            let Some(runtime) = weak.upgrade() else {
                return;
            };
            tokio::select! {
                () = closed.cancelled() => {}
                result = runtime.await_attention(request) => {
                    if let Err(error) = result {
                        cyber_core::log::warn("goals", "unattended ask watch failed",
                            json!({"error": error.to_string()}));
                    }
                }
            }
        });
        let mut owners = self
            .inner
            .background
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        owners.retain(|owner| !owner.is_finished());
        owners.push(task);
    }

    /// Wait until `request` is answered, the Goal is no longer active, or nobody has
    /// watched the Session for the configured time; then pause the Goal.
    async fn await_attention(&self, request: PendingRequest) -> Result<(), RuntimeError> {
        let handle = self.inner.handle(&request.session_id).await?;
        let (directory, has_goal) = {
            let state = handle.state.lock().await;
            (state.info.directory.clone(), state.goals.active().is_some())
        };
        if !has_goal {
            return Ok(());
        }
        let (settings, _) = self.goal_settings(&directory)?;
        let wait = Duration::from_secs(settings.unattended_ask_seconds);
        let scopes = watchers(&request, &directory);
        let opened = Instant::now();
        loop {
            let seen = self
                .inner
                .presence
                .last_seen(&scopes)
                .map_or(opened, |seen| seen.max(opened));
            let deadline = seen + wait;
            if Instant::now() < deadline {
                tokio::time::sleep_until(deadline.into()).await;
            }
            if !self.still_pending(&request) {
                return Ok(());
            }
            let seen = self.inner.presence.last_seen(&scopes);
            if seen.is_none_or(|seen| seen.max(opened) + wait <= Instant::now()) {
                return self.pause_for_approval(&request).await;
            }
        }
    }

    fn still_pending(&self, request: &PendingRequest) -> bool {
        self.inner
            .pending(Some(&request.session_id))
            .iter()
            .any(|pending| pending.id == request.id)
    }

    async fn pause_for_approval(&self, request: &PendingRequest) -> Result<(), RuntimeError> {
        let handle = self.inner.handle(&request.session_id).await?;
        let Some(goal) = handle.state.lock().await.goals.active().cloned() else {
            return Ok(());
        };
        let paused = Paused {
            goal_id: goal.goal_id.clone(),
            reason: WAITING_FOR_APPROVAL.into(),
        };
        self.inner
            .commit(&handle, vec![event(records::PAUSED, &paused)])
            .await?;
        self.goal_notify(
            &request.session_id,
            &goal,
            "Goal paused: waiting for approval",
            &describe(request),
        )
        .await;
        Ok(())
    }
}

/// Who could answer: the asking Session, the Sessions it is routed to, their Locations.
fn watchers(request: &PendingRequest, directory: &str) -> Vec<Watching> {
    let mut scopes = vec![
        Watching::Session(request.session_id.clone()),
        Watching::Location(directory.to_owned()),
    ];
    for route in &request.routed_to {
        scopes.push(Watching::Session(route.session_id.clone()));
        scopes.push(Watching::Location(route.directory.clone()));
    }
    scopes
}

/// `<action> <resources> (request per_…)`.
fn describe(request: &PendingRequest) -> String {
    match &request.kind {
        PendingKind::Permission(ask) => format!(
            "{} {} (request {})",
            ask.action,
            ask.resources.join(", "),
            request.id
        ),
        _ => format!("request {}", request.id),
    }
}

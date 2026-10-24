//! Browser presence has an explicit lifetime, independent of data streams. The
//! single watcher owns expiry and only settles requests that lost all approvers.
use super::{
    Runtime,
    presence::{InteractiveClient, InteractiveClientError},
};
use std::sync::PoisonError;

impl Runtime {
    pub async fn add_interactive_client(
        &self,
        session: &str,
        directory: &str,
    ) -> Result<InteractiveClient, InteractiveClientError> {
        let directory = canonical(directory);
        let handle = self
            .inner
            .handle(session)
            .await
            .map_err(|_| InteractiveClientError::Unavailable)?;
        let mut state = handle.state.lock().await;
        if canonical(&state.info.directory) != directory {
            return Err(InteractiveClientError::Missing);
        }
        if !self.inner.presence.managed(session) {
            self.inner.presence.manage(session, &directory)?;
            let recorded = self.inner.record_locked(
                &mut state,
                vec![super::events::event(
                    super::events::INTERACTIVE_MANAGED,
                    &serde_json::json!({}),
                )],
            );
            match recorded {
                Ok(events) => self.inner.publish(&events),
                Err(_) => {
                    self.inner.presence.unmanage(session);
                    return Err(InteractiveClientError::Unavailable);
                }
            }
        }
        drop(state);
        self.watch_interactive_clients();
        match self.inner.presence.create(session, &directory) {
            Ok(lease) => Ok(lease),
            Err(error) => {
                self.inner
                    .settle_browser_requests()
                    .await
                    .map_err(|_| InteractiveClientError::Unavailable)?;
                Err(error)
            }
        }
    }
    pub fn renew_interactive_client(
        &self,
        session: &str,
        directory: &str,
        client: &str,
    ) -> Result<InteractiveClient, InteractiveClientError> {
        self.inner
            .presence
            .renew(session, &canonical(directory), client)
    }
    pub async fn release_interactive_client(
        &self,
        session: &str,
        directory: &str,
        client: &str,
    ) -> Result<(), InteractiveClientError> {
        self.inner
            .presence
            .release(session, &canonical(directory), client)?;
        // Release acknowledgement follows normal non-interactive settlement.
        self.inner
            .settle_browser_requests()
            .await
            .map_err(|_| InteractiveClientError::Unavailable)?;
        Ok(())
    }
    fn watch_interactive_clients(&self) {
        if !self.inner.presence.start_watch() {
            return;
        }
        let presence = self.inner.presence.clone();
        let weak = self.downgrade();
        let closed = self.inner.closed.clone();
        let task = tokio::spawn(async move {
            loop {
                let changed = presence.changed();
                let deadline = presence.next_expiry();
                tokio::select! {
                    ()=closed.cancelled()=>break,
                    ()=changed=>{},
                    ()=async {match deadline {Some(deadline)=>tokio::time::sleep_until(deadline).await,None=>std::future::pending().await}}=>{},
                }
                presence.expire();
                let Some(runtime) = weak.upgrade() else {
                    break;
                };
                if let Err(error) = runtime.inner.settle_browser_requests().await {
                    cyber_core::log::warn(
                        "interactive",
                        "request settlement failed",
                        serde_json::json!({"error":error.to_string()}),
                    );
                }
            }
        });
        let mut owners = self
            .inner
            .background
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        owners.retain(|owner| !owner.is_finished());
        owners.push(task);
    }
}

fn canonical(directory: &str) -> String {
    std::fs::canonicalize(directory)
        .map_or_else(|_| directory.to_owned(), |path| path.display().to_string())
}

use super::{Computer, ComputerError, Preview, Receipt, Result, Status};
use crate::runtime::Runtime;
use cyber_computer::{
    Action,
    backend::{Backend, Native, Selector},
};
use serde::{Deserialize, Serialize};
use std::{path::PathBuf, sync::Arc, time::Duration};

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ComputerPolicy {
    pub enabled: bool,
    #[serde(default = "retention")]
    pub retention_days: u64,
}
fn retention() -> u64 {
    1
}
impl Default for ComputerPolicy {
    fn default() -> Self {
        Self {
            enabled: false,
            retention_days: 1,
        }
    }
}

impl Runtime {
    pub fn configure_computer(
        &self,
        path: PathBuf,
        policy: Arc<dyn Fn() -> ComputerPolicy + Send + Sync>,
    ) -> Result<()> {
        self.configure_computer_backend(path, policy, Arc::new(Native))
    }
    pub fn configure_computer_backend(
        &self,
        path: PathBuf,
        policy: Arc<dyn Fn() -> ComputerPolicy + Send + Sync>,
        backend: Arc<dyn Backend>,
    ) -> Result<()> {
        let weak = self.inner.me.clone();
        let attendance = Arc::new(move |session: &str| {
            weak.upgrade()
                .is_some_and(|inner| inner.presence.live_session(session))
        });
        let computer = Arc::new(Computer::new(
            Arc::clone(&self.inner.store),
            path,
            policy,
            attendance,
            backend,
        )?);
        self.inner
            .computer
            .set(Arc::clone(&computer))
            .map_err(|_| ComputerError::Invalid("computer runtime is already configured"))?;
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            let weak = Arc::downgrade(&computer);
            let close = self.inner.closed.child_token();
            let task = handle.spawn(async move {
                let mut delay = Duration::from_secs(120);
                loop {
                    tokio::select! {_=close.cancelled()=>break,_=tokio::time::sleep(delay)=>{}}
                    let Some(computer) = weak.upgrade() else {
                        break;
                    };
                    let _ = tokio::task::spawn_blocking(move || computer.prune()).await;
                    delay = Duration::from_secs(6 * 3600);
                }
            });
            self.inner
                .background
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(task);
        }
        Ok(())
    }
    pub fn computer(&self) -> Result<Arc<Computer>> {
        self.inner
            .computer
            .get()
            .cloned()
            .ok_or(ComputerError::Invalid(
                "native computer runtime is unavailable in this host",
            ))
    }
    pub fn computer_stop(&self, session: &str) {
        if let Some(computer) = self.inner.computer.get() {
            computer.stop(session);
        }
    }
    pub async fn computer_status(&self, session: &str) -> Result<Status> {
        self.state(session)
            .await
            .map_err(|_| ComputerError::Invalid("Session is unavailable"))?;
        let computer = self.computer()?;
        let session = session.to_owned();
        blocking(move || computer.status(&session)).await
    }
    pub async fn computer_preview(&self, session: &str, selector: Selector) -> Result<Preview> {
        self.state(session)
            .await
            .map_err(|_| ComputerError::Invalid("Session is unavailable"))?;
        let computer = self.computer()?;
        let session = session.to_owned();
        blocking(move || computer.preview(&session, selector)).await
    }
    pub async fn computer_call(
        &self,
        session: &str,
        operation: &str,
        frame: Option<&str>,
        action: Option<Action>,
    ) -> Result<Receipt> {
        let state = self
            .state(session)
            .await
            .map_err(|_| ComputerError::Invalid("Session is unavailable"))?;
        let permitted = state.effective_mode(true) != "plan";
        if action.is_none()
            && !self
                .inner
                .resolve_selection(&state.info, &state.model_selection)
                .map_err(|_| ComputerError::Invalid("Session model is unavailable"))?
                .model
                .vision
        {
            return Err(ComputerError::Invalid(
                "selected model does not support image input",
            ));
        }
        let computer = self.computer()?;
        let (session, operation, frame) = (
            session.to_owned(),
            operation.to_owned(),
            frame.map(str::to_owned),
        );
        blocking(move || match action {
            Some(action) => computer.input(
                &session,
                &operation,
                frame
                    .as_deref()
                    .ok_or(ComputerError::Invalid("input requires a frame"))?,
                action,
                permitted,
            ),
            None => computer.screenshot(&session, &operation),
        })
        .await
    }
}

pub async fn blocking<T: Send + 'static>(
    run: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    tokio::task::spawn_blocking(run)
        .await
        .map_err(|_| ComputerError::Native(cyber_computer::Error::Native("native worker failed")))?
}

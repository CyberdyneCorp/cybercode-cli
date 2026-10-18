//! Managed worktree lifecycle notices (`worktrees` → Worktree events) and the
//! Session relocation of `enter_worktree`/`exit_worktree` (`worktrees` → Enter and
//! exit tools).

use std::path::Path;
use std::sync::atomic::Ordering;

use cyber_core::worktrees::Managed;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use super::{CallStatus, LiveEvent, Runtime, RuntimeError, SessionState};

pub const ENTERED: &str = "session.worktree.entered.1";
pub const EXITED: &str = "session.worktree.exited.1";

/// Event kinds that move a Session to another Location; each carries `to.path`
/// and `from.path`.
pub fn relocates(kind: &str) -> bool {
    matches!(kind, "session.worktree.rebound.1" | ENTERED | EXITED)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorktreeChange {
    Created,
    Removed,
    Kept,
}

impl WorktreeChange {
    pub fn kind(self) -> &'static str {
        match self {
            Self::Created => "worktree.created.1",
            Self::Removed => "worktree.removed.1",
            Self::Kept => "worktree.kept.1",
        }
    }
}

/// `{ name, path, branch, session_id? }` of `worktree.created/removed/kept.1`.
#[derive(Debug, Clone, PartialEq, Serialize, schemars::JsonSchema)]
pub struct WorktreeNotice {
    pub name: String,
    pub path: String,
    pub branch: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

/// An unmanaged Location an entered Session returns to.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(super) struct Origin {
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(super) struct Entered {
    pub from: Origin,
    pub to: Managed,
    /// This Session created the checkout, so its exit may remove it. Absent in older
    /// events, which then keep the checkout.
    #[serde(default)]
    pub created: bool,
}

/// Where an exited Session returned to, and whether it had created the checkout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeExit {
    pub origin: String,
    pub created: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(super) struct Exited {
    pub from: Managed,
    pub to: Origin,
}

impl Runtime {
    /// Live only, on the instance stream of `directory`, the Location that asked.
    pub fn publish_worktree(
        &self,
        change: WorktreeChange,
        directory: &Path,
        managed: &Managed,
        session_id: Option<&str>,
    ) {
        let directory = directory
            .canonicalize()
            .unwrap_or_else(|_| directory.to_path_buf());
        self.inner.bus.publish(LiveEvent::Worktree {
            directory: directory.display().to_string(),
            kind: change.kind().into(),
            worktree: WorktreeNotice {
                name: managed.name.clone(),
                path: managed.path.display().to_string(),
                branch: managed.branch.clone(),
                session_id: session_id.map(str::to_owned),
            },
        });
    }

    /// Whether `enter_worktree`'s `call_id` may move the Session at all, checked before
    /// any worktree is discovered or created: the Session runs a Turn in an unmanaged
    /// Location and the call runs alone. [`Self::enter_worktree`] checks again.
    pub async fn check_enterable(&self, id: &str, call_id: &str) -> Result<(), RuntimeError> {
        let handle = self.inner.handle(id).await?;
        if handle.location_uncertain.load(Ordering::SeqCst) {
            return Err(RuntimeError::Invalid(
                "Location outcome unknown; recovery is required".into(),
            ));
        }
        enterable(&*handle.state.lock().await, call_id)?;
        if !handle.holds_lease() {
            return Err(RuntimeError::Invalid(
                "Worktree changes need a running Turn".into(),
            ));
        }
        Ok(())
    }

    /// Move a running Session from its unmanaged Location into `to`, which it
    /// `created` or found existing. The Drain's Location lease is replaced by one on
    /// `to`, so removal stays fenced until the Drain settles; `call_id` must be the only
    /// call in flight.
    pub async fn enter_worktree(
        &self,
        id: &str,
        call_id: &str,
        to: &Managed,
        created: bool,
        cancel: CancellationToken,
    ) -> Result<(), RuntimeError> {
        let check = |state: &SessionState| -> Result<Origin, RuntimeError> {
            enterable(state, call_id)?;
            Ok(Origin {
                path: state.info.directory.clone(),
            })
        };
        self.relocate(
            id,
            cancel,
            to.path.display().to_string(),
            Some(to.id.clone()),
            |state| {
                let from = check(state)?;
                Ok(vec![super::events::event(
                    ENTERED,
                    &Entered {
                        from,
                        to: to.clone(),
                        created,
                    },
                )])
            },
        )
        .await
    }

    /// Return an entered Session to the Location it entered from.
    pub async fn exit_worktree(
        &self,
        id: &str,
        call_id: &str,
        from: &Managed,
        cancel: CancellationToken,
    ) -> Result<WorktreeExit, RuntimeError> {
        let (origin, created) = {
            let handle = self.inner.handle(id).await?;
            let state = handle.state.lock().await;
            (exit_origin(&state, call_id, from)?, state.entered_created)
        };
        self.relocate(id, cancel, origin.clone(), None, |state| {
            let to = exit_origin(state, call_id, from)?;
            Ok(vec![super::events::event(
                EXITED,
                &Exited {
                    from: from.clone(),
                    to: Origin { path: to },
                },
            )])
        })
        .await?;
        Ok(WorktreeExit { origin, created })
    }

    /// Sessions that entered `managed` with `enter_worktree` and have not exited, idle
    /// and archived ones included: removing the checkout would strand them.
    pub async fn entered_sessions(&self, managed: &Managed) -> Result<Vec<String>, RuntimeError> {
        Ok(self
            .bound_sessions(managed)
            .await?
            .into_iter()
            .filter(|info| info.worktree_origin.is_some())
            .map(|info| info.id)
            .collect())
    }

    /// Sessions, archived ones included, whose Location is the `managed` checkout
    /// incarnation: those started in it, isolated children and entered Sessions.
    pub async fn bound_sessions(
        &self,
        managed: &Managed,
    ) -> Result<Vec<super::SessionInfo>, RuntimeError> {
        let path = managed
            .path
            .canonicalize()
            .unwrap_or_else(|_| managed.path.clone());
        let mut filter = super::ListFilter {
            limit: Some(200),
            include_archived: true,
            ..Default::default()
        };
        let mut bound = Vec::new();
        loop {
            let page = self.list(&filter)?;
            for row in page.sessions {
                let directory = Path::new(&row.directory);
                if directory
                    .canonicalize()
                    .unwrap_or_else(|_| directory.to_path_buf())
                    != path
                {
                    continue;
                }
                let handle = self.inner.handle(&row.id).await?;
                let state = handle.state.lock().await;
                if state.info.worktree_id.as_deref() == Some(managed.id.as_str()) {
                    bound.push(state.info.clone());
                }
            }
            filter.cursor = page.next;
            if filter.cursor.is_none() {
                return Ok(bound);
            }
        }
    }

    /// Return an idle entered Session to the Location it entered from, without a Turn:
    /// the exit of a Session that ends entered, a forced removal of the checkout it
    /// entered, and the recovery of one whose checkout is gone. The Session is fenced
    /// like a deletion meanwhile, so no Drain starts; a prompt that arrives waits for
    /// the exit. None when the Session did not enter a worktree.
    pub async fn exit_idle_worktree(
        &self,
        id: &str,
    ) -> Result<Option<(Managed, WorktreeExit)>, RuntimeError> {
        let _admission = self.inner.open().await?;
        let handle = self.inner.handle(id).await?;
        if handle.location_uncertain.load(Ordering::SeqCst) {
            return Err(RuntimeError::Invalid(
                "Location outcome unknown; recovery is required".into(),
            ));
        }
        let ids = [id.to_owned()];
        self.fence_deletion(id, &ids)?;
        let exited = {
            let mut state = handle.state.lock().await;
            match state.entered_worktree.as_deref().cloned() {
                None => Ok(None),
                Some(from) => exit_origin(&state, "", &from).and_then(|origin| {
                    let created = state.entered_created;
                    let event = super::events::event(
                        EXITED,
                        &Exited {
                            from: from.clone(),
                            to: Origin {
                                path: origin.clone(),
                            },
                        },
                    );
                    self.inner.commit_locked(&mut state, vec![event])?;
                    Ok(Some((from, WorktreeExit { origin, created })))
                }),
            }
        };
        self.lift_deletion_fence(&ids);
        exited
    }

    async fn relocate(
        &self,
        id: &str,
        cancel: CancellationToken,
        directory: String,
        worktree_id: Option<String>,
        events: impl Fn(&SessionState) -> Result<Vec<cyber_store::NewEvent>, RuntimeError>,
    ) -> Result<(), RuntimeError> {
        let _admission = self.inner.open().await?;
        let handle = self.inner.handle(id).await?;
        if handle.location_uncertain.load(Ordering::SeqCst) {
            return Err(RuntimeError::Invalid(
                "Location outcome unknown; recovery is required".into(),
            ));
        }
        let mut info = {
            let state = handle.state.lock().await;
            events(&state)?;
            state.info.clone()
        };
        if !handle.holds_lease() {
            return Err(RuntimeError::Invalid(
                "Worktree changes need a running Turn".into(),
            ));
        }
        info.directory = directory;
        info.worktree_id = worktree_id;
        let lease = self.inner.claim_location(&info, false, cancel).await?;
        if lease.worktree_id != info.worktree_id {
            let _ = lease.settle();
            return Err(RuntimeError::Invalid(
                "Managed checkout identity changed".into(),
            ));
        }
        let committed = {
            let mut state = handle.state.lock().await;
            events(&state)
                .and_then(|events| self.inner.commit_locked(&mut state, events).map(|_| ()))
        };
        if let Err(error) = committed {
            let _ = lease.settle();
            return Err(error);
        }
        // The earlier Location's work is settled: this call runs alone.
        if let Some(previous) = handle.replace_lease(lease)
            && let Err(error) = previous.settle()
        {
            handle.location_uncertain.store(true, Ordering::SeqCst);
            return Err(RuntimeError::Invalid(error));
        }
        Ok(())
    }
}

/// Only the relocating call may be in flight.
fn alone(state: &SessionState, call_id: &str) -> Result<(), RuntimeError> {
    let busy = state.calls.iter().any(|(id, call)| {
        id != call_id
            && matches!(
                call.status,
                CallStatus::Dispatched | CallStatus::OutcomeUnknown
            )
    });
    if busy {
        return Err(RuntimeError::Invalid(
            "Worktree changes need the call to run alone".into(),
        ));
    }
    Ok(())
}

/// Only a Session in an unmanaged Location may enter, with the call running alone.
fn enterable(state: &SessionState, call_id: &str) -> Result<(), RuntimeError> {
    alone(state, call_id)?;
    if state.info.worktree_id.is_some() || state.info.worktree_origin.is_some() {
        return Err(RuntimeError::Invalid(
            "The Session already runs in a managed worktree; exit it first".into(),
        ));
    }
    Ok(())
}

fn exit_origin(
    state: &SessionState,
    call_id: &str,
    from: &Managed,
) -> Result<String, RuntimeError> {
    alone(state, call_id)?;
    match &state.info.worktree_origin {
        Some(origin) if state.info.worktree_id.as_deref() == Some(from.id.as_str()) => {
            Ok(origin.clone())
        }
        Some(_) => Err(RuntimeError::Invalid(
            "The entered worktree changed; recovery is required".into(),
        )),
        None => Err(RuntimeError::Invalid(
            "The Session did not enter a worktree".into(),
        )),
    }
}

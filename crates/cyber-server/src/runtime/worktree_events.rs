//! Managed worktree lifecycle notices (`worktrees` → Worktree events) and the
//! Session relocation of `enter_worktree`/`exit_worktree` (`worktrees` → Enter and
//! exit tools).

use std::cell::RefCell;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use cyber_core::worktrees::Managed;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use super::{CallStatus, LiveEvent, Runtime, RuntimeError, SessionState};

pub const ENTERED: &str = "session.worktree.entered.1";
pub const EXITED: &str = "session.worktree.exited.1";

tokio::task_local! {
    /// A management operation can move idle Sessions while holding shutdown
    /// admission. Its observations run only after that outer owner settles.
    static DEFERRED_CWD: RefCell<DeferredCwd>;
}

struct DeferredCwd {
    owner: Arc<tokio::sync::RwLock<()>>,
    hooks: Vec<super::host::LifecycleHook>,
}

impl super::Inner {
    pub(super) fn after_worktree_owner<T>(
        &self,
        work: impl std::future::Future<Output = T>,
    ) -> impl std::future::Future<Output = T> {
        // Box before constructing the returned future: generic worktree flows
        // otherwise retain their large state machine in every ownership frame.
        let work = Box::pin(work);
        async move {
            if DEFERRED_CWD
                .try_with(|pending| Arc::ptr_eq(&pending.borrow().owner, &self.lifecycle))
                .unwrap_or(false)
            {
                return work.await;
            }
            let pending = RefCell::new(DeferredCwd {
                owner: Arc::clone(&self.lifecycle),
                hooks: Vec::new(),
            });
            let (result, hooks) = DEFERRED_CWD
                .scope(pending, async {
                    let result = work.await;
                    let hooks = DEFERRED_CWD
                        .with(|pending| std::mem::take(&mut pending.borrow_mut().hooks));
                    (result, hooks)
                })
                .await;
            for hook in hooks {
                self.observe_lifecycle(hook).await;
            }
            result
        }
    }

    pub(super) async fn observe_cwd(&self, hook: super::host::LifecycleHook) {
        let mut hook = Some(hook);
        let _ = DEFERRED_CWD.try_with(|pending| {
            let mut pending = pending.borrow_mut();
            if Arc::ptr_eq(&pending.owner, &self.lifecycle) {
                pending.hooks.push(hook.take().unwrap());
            }
        });
        if let Some(hook) = hook {
            self.observe_lifecycle(hook).await;
        }
    }
}

/// Event kinds that move a Session to another Location; each carries `to.path`
/// and `from.path`.
pub fn relocates(kind: &str) -> bool {
    matches!(kind, "session.worktree.rebound.1" | ENTERED | EXITED)
}

/// Capture only an acknowledged effective path transition, from its committed
/// record and the resulting Session identity. Replay never invokes this producer.
pub(super) fn cwd_hook(
    state: &SessionState,
    stored: &[cyber_store::StoredEvent],
) -> Option<super::host::LifecycleHook> {
    let transition = stored.iter().find(|event| relocates(&event.kind))?;
    let from = transition.data["from"]["path"].as_str()?;
    let to = transition.data["to"]["path"].as_str()?;
    if from == to {
        return None;
    }
    Some(super::lifecycle::lifecycle(
        state,
        "CwdChanged",
        to.into(),
        serde_json::json!({
            "previous_directory": from,
            "directory": to,
            "transition_id": transition.id,
            "transition_sequence": transition.seq,
            "transition": transition.kind,
        }),
    ))
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
        let mut hook = None;
        let mut observed = None;
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
                    let stored = self.inner.commit_locked(&mut state, vec![event])?;
                    hook = cwd_hook(&state, &stored);
                    observed = Some(state.info.clone());
                    Ok(Some((from, WorktreeExit { origin, created })))
                }),
            }
        };
        self.lift_deletion_fence(&ids);
        if let Some(info) = observed {
            // Refresh file-observer membership before CwdChanged handlers can
            // write at the committed destination. Neither holds the Session lock.
            self.inner.tools.refresh_location(&info);
        }
        drop(_admission);
        if let Some(hook) = hook {
            self.inner.observe_cwd(hook).await;
        }
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
            events(&state).and_then(|events| {
                let stored = self.inner.commit_locked(&mut state, events)?;
                Ok(cwd_hook(&state, &stored))
            })
        };
        let hook = match committed {
            Ok(hook) => hook,
            Err(error) => {
                let _ = lease.settle();
                return Err(error);
            }
        };
        // The earlier Location's work is settled: this call runs alone.
        if let Some(previous) = handle.replace_lease(lease)
            && let Err(error) = previous.settle()
        {
            handle.location_uncertain.store(true, Ordering::SeqCst);
            return Err(RuntimeError::Invalid(error));
        }
        drop(_admission);
        if let Some(hook) = hook {
            self.inner.observe_cwd(hook).await;
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

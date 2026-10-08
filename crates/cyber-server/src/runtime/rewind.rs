//! Per-turn snapshots, diff summaries and three-phase revert (`snapshots-checkpoints`).

use super::bus::LiveEvent;
use super::events::*;
use super::host::{FileDiff, RestoreError};
use super::model::{Entry, RevertState, RevertTarget, SessionState};
use super::{Handle, Inner, Runtime, RuntimeError};

impl Inner {
    /// Snapshot the Session's working tree; failures are reported and never fail the Turn.
    pub(crate) async fn take_snapshot(&self, handle: &Handle) -> Option<String> {
        let (session_id, directory) = {
            let state = handle.state.lock().await;
            (state.info.id.clone(), state.info.directory.clone())
        };
        match self.options.snapshots.track(&directory).await {
            Ok(snapshot) => snapshot.map(|s| s.tree),
            Err(message) => {
                self.bus.publish(LiveEvent::Error {
                    session_id,
                    kind: "snapshot".into(),
                    message,
                });
                None
            }
        }
    }

    /// After a Turn's tool settlements: record the post-Turn tree and the user message's diff.
    pub(crate) async fn after_turn(&self, handle: &Handle, pre: Option<String>) {
        let Some(post) = self.take_snapshot(handle).await else {
            return;
        };
        let state = handle.state.lock().await.clone();
        let Some(step) = state.steps.last().filter(|s| s.pre == pre) else {
            return;
        };
        let directory = &state.info.directory;
        let changed = match &pre {
            Some(pre) => self
                .options
                .snapshots
                .changed(directory, pre, &post)
                .await
                .unwrap_or_default(),
            None => Vec::new(),
        };
        let taken = SnapshotTaken {
            session_id: state.info.id.clone(),
            step_id: step.step_id.clone(),
            tree: post.clone(),
            changed: changed.clone(),
            skipped: Vec::new(),
        };
        let mut events = vec![event(SNAPSHOT_TAKEN, &taken)];
        if let Some(diff) = self
            .message_diff(
                &state,
                step.user_message_id.as_deref(),
                &post,
                !changed.is_empty(),
            )
            .await
        {
            events.push(event(DIFF_COMPUTED, &diff));
        }
        if let Err(e) = self.commit(handle, events).await {
            self.bus.publish(LiveEvent::Error {
                session_id: state.info.id.clone(),
                kind: "snapshot".into(),
                message: e.to_string(),
            });
        }
    }

    /// The diff from the message's first pre-Turn snapshot to `post`, when anything changed.
    async fn message_diff(
        &self,
        state: &SessionState,
        message: Option<&str>,
        post: &str,
        changed: bool,
    ) -> Option<DiffComputed> {
        let message = message?;
        if !changed && !state.diffs.contains_key(message) {
            return None;
        }
        let first = state
            .steps
            .iter()
            .find(|s| s.user_message_id.as_deref() == Some(message))
            .and_then(|s| s.pre.clone())?;
        let diffs = self
            .options
            .snapshots
            .diff(&state.info.directory, &first, post)
            .await
            .ok()?;
        Some(DiffComputed {
            message_id: message.to_string(),
            diffs,
        })
    }

    /// A staged revert is committed by the next prompt or compaction.
    pub(crate) async fn commit_staged_revert(&self, handle: &Handle) -> Result<(), RuntimeError> {
        let staged = handle.state.lock().await.revert.clone();
        match staged {
            Some(revert) => self
                .commit(
                    handle,
                    vec![revert_event(
                        &handle_id(handle).await,
                        &revert,
                        RevertPhase::Commit,
                    )],
                )
                .await
                .map(|_| ()),
            None => Ok(()),
        }
    }

    fn ensure_idle(&self, session_id: &str) -> Result<(), RuntimeError> {
        let running = self
            .drains
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains_key(session_id);
        if running {
            Err(RuntimeError::Busy(session_id.into()))
        } else {
            Ok(())
        }
    }

    async fn restore(
        &self,
        directory: &str,
        target: &str,
        recorded: &str,
    ) -> Result<(), RuntimeError> {
        match self
            .options
            .snapshots
            .restore(directory, target, recorded)
            .await
        {
            Ok(_) => Ok(()),
            Err(RestoreError::Conflict(paths)) => Err(RuntimeError::RewindConflict(paths)),
            Err(RestoreError::Failed(message)) => Err(RuntimeError::Invalid(message)),
        }
    }

    /// Restore code for a stage; returns `(baseline, applied, diff)`.
    async fn stage_code(
        &self,
        handle: &Handle,
        state: &SessionState,
        message_id: &str,
    ) -> Result<(Option<String>, Option<String>, String), RuntimeError> {
        let unavailable = || {
            RuntimeError::Invalid(
                "Snapshots are unavailable for this Location; only the conversation can be rewound"
                    .into(),
            )
        };
        let last_post = state.steps.iter().rev().find_map(|s| s.post.clone());
        let recorded = state
            .revert
            .as_ref()
            .and_then(|r| r.applied.clone())
            .or(last_post)
            .ok_or_else(unavailable)?;
        let baseline = match state.revert.as_ref().and_then(|r| r.baseline.clone()) {
            Some(tree) => tree,
            None => self.take_snapshot(handle).await.ok_or_else(unavailable)?,
        };
        // Files return to their state before the message's first Turn.
        let target = state
            .steps
            .iter()
            .find(|s| s.user_message_id.as_deref() == Some(message_id))
            .and_then(|s| s.pre.clone())
            .unwrap_or_else(|| recorded.clone());
        let directory = &state.info.directory;
        self.restore(directory, &target, &recorded).await?;
        let diff = match self
            .options
            .snapshots
            .diff(directory, &baseline, &target)
            .await
        {
            Ok(files) => files
                .into_iter()
                .map(|f| f.patch)
                .collect::<Vec<_>>()
                .join(""),
            Err(_) => String::new(),
        };
        Ok((Some(baseline), Some(target), diff))
    }
}

async fn handle_id(handle: &Handle) -> String {
    handle.state.lock().await.info.id.clone()
}

fn revert_event(
    session_id: &str,
    revert: &RevertState,
    phase: RevertPhase,
) -> cyber_store::NewEvent {
    event(
        REVERTED,
        &Reverted {
            session_id: session_id.into(),
            message_id: revert.message_id.clone(),
            target: revert.target,
            phase,
            baseline: revert.baseline.clone(),
            applied: revert.applied.clone(),
            diff: revert.diff.clone(),
        },
    )
}

impl Runtime {
    /// Run a user shell command (`session-runtime` → User shell commands) and record the
    /// command and its output as a user message the model sees next Turn, without a Turn.
    pub async fn shell(&self, session_id: &str, command: &str) -> Result<String, RuntimeError> {
        self.inner.ensure_idle(session_id)?;
        let handle = self.inner.handle(session_id).await?;
        self.inner
            .with_shell_location(&handle, async {
                self.inner.commit_staged_revert(&handle).await?;
                let directory = handle.state.lock().await.info.directory.clone();
                let output = self
                    .inner
                    .tools
                    .shell_owned(
                        &directory,
                        session_id,
                        command,
                        self.inner.closed.child_token(),
                    )
                    .await
                    .map_err(RuntimeError::Invalid)?;
                let message_id = cyber_core::ids::new_id("msg");
                let parts = vec![
                    cyber_llm::Content::Text {
                        text: format!("!{command}"),
                    },
                    cyber_llm::Content::Text {
                        text: format!(
                            "<shell-output command=\"{}\">\n{output}\n</shell-output>",
                            command.replace('"', "'")
                        ),
                    },
                ];
                let admitted = Admitted {
                    wake: false,
                    message_id: message_id.clone(),
                    digest: format!("shell:{message_id}"),
                    parts,
                    delivery: super::model::Delivery::Queue,
                    source: "shell".into(),
                };
                let promoted = Promoted { message_id };
                self.inner
                    .commit(
                        &handle,
                        vec![event(ADMITTED, &admitted), event(PROMOTED, &promoted)],
                    )
                    .await?;
                Ok(output)
            })
            .await
    }

    /// File diffs recorded for a user message (`snapshots-checkpoints` → Per-turn diff summary).
    pub async fn diff(
        &self,
        session_id: &str,
        message_id: &str,
    ) -> Result<Vec<FileDiff>, RuntimeError> {
        let handle = self.inner.handle(session_id).await?;
        let state = handle.state.lock().await;
        Ok(state.diffs.get(message_id).cloned().unwrap_or_default())
    }

    /// Stage a rewind to just before `message_id`: restore files when the target includes
    /// code and record the revert. Repeated stages reuse the first baseline.
    pub async fn revert_stage(
        &self,
        session_id: &str,
        message_id: &str,
        target: RevertTarget,
    ) -> Result<RevertState, RuntimeError> {
        self.inner.ensure_idle(session_id)?;
        let handle = self.inner.handle(session_id).await?;
        self.inner
            .with_idle_location(&handle, async {
                let state = handle.state.lock().await.clone();
                if !state
                    .entries
                    .iter()
                    .any(|e| matches!(e, Entry::User { id, .. } if id == message_id))
                {
                    return Err(RuntimeError::Invalid(format!(
                        "No user message {message_id} in this session"
                    )));
                }
                let previous = state.revert.clone();
                let (baseline, applied, diff) = if target.code() {
                    self.inner.stage_code(&handle, &state, message_id).await?
                } else {
                    previous
                        .map(|r| (r.baseline, r.applied, r.diff))
                        .unwrap_or_default()
                };
                let revert = RevertState {
                    message_id: message_id.into(),
                    target,
                    baseline,
                    applied,
                    diff,
                };
                self.inner
                    .commit(
                        &handle,
                        vec![revert_event(session_id, &revert, RevertPhase::Stage)],
                    )
                    .await?;
                Ok(revert)
            })
            .await
    }

    /// Undo a staged revert: restore the pre-stage working tree and keep the conversation.
    pub async fn revert_clear(&self, session_id: &str) -> Result<(), RuntimeError> {
        self.inner.ensure_idle(session_id)?;
        let handle = self.inner.handle(session_id).await?;
        self.inner
            .with_idle_location(&handle, async {
                let state = handle.state.lock().await.clone();
                let revert = state
                    .revert
                    .clone()
                    .ok_or_else(|| RuntimeError::Invalid("No revert is staged".into()))?;
                if let (Some(baseline), Some(applied)) = (&revert.baseline, &revert.applied) {
                    self.inner
                        .restore(&state.info.directory, baseline, applied)
                        .await?;
                }
                self.inner
                    .commit(
                        &handle,
                        vec![revert_event(session_id, &revert, RevertPhase::Clear)],
                    )
                    .await?;
                Ok(())
            })
            .await
    }

    /// Make a staged revert final: the boundary message and everything after it are removed.
    pub async fn revert_commit(&self, session_id: &str) -> Result<(), RuntimeError> {
        self.inner.ensure_idle(session_id)?;
        let handle = self.inner.handle(session_id).await?;
        if handle.state.lock().await.revert.is_none() {
            return Err(RuntimeError::Invalid("No revert is staged".into()));
        }
        self.inner.commit_staged_revert(&handle).await
    }
}

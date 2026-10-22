//! Branch checkout and labels (`session-runtime` → Session branches). A checkout needs an
//! idle Session and, with `code`, restores files through the three-phase revert: it stages
//! a `code` revert, so user edits are merged, the diff is previewed and a clear undoes it.

use serde::{Deserialize, Serialize};

use super::branches::{CHECKED_OUT, CheckedOut, LABELED, Labeled, valid_label};
use super::events::{RevertPhase, event};
use super::jobs::JobStatus;
use super::model::{Delivery, RevertState, RevertTarget, SessionState};
use super::rewind::revert_event;
use super::{Handle, Runtime, RuntimeError};

/// What to check out: a message ID or a label, and whether to restore code.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BranchCheckout {
    #[serde(default)]
    pub message_id: Option<String>,
    #[serde(default)]
    pub label: Option<String>,
    /// Make the message itself the tip instead of the newest tip under it.
    #[serde(default)]
    pub exact: bool,
    /// Restore the working tree to the snapshot after the tip's last step.
    #[serde(default)]
    pub code: bool,
}

/// The active tip after a checkout and, when code was restored, the staged revert.
#[derive(Debug, Clone, PartialEq, Serialize, schemars::JsonSchema)]
pub struct CheckedOutBranch {
    pub tip_message_id: String,
    pub code_restored: bool,
    /// The files the code restore changed.
    pub restored_files: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revert: Option<RevertState>,
}

/// A code restore a checkout staged: the revert it records, the files it changed and the
/// working tree before it, which a refused checkout restores.
struct BranchCode {
    revert: RevertState,
    files: Vec<String>,
    before: String,
}

impl Runtime {
    /// Make another branch active. The Session must be idle: no Drain, no promotable
    /// input and no running background work; held input stays held.
    pub async fn checkout(
        &self,
        session_id: &str,
        request: &BranchCheckout,
    ) -> Result<CheckedOutBranch, RuntimeError> {
        self.inner.ensure_idle(session_id)?;
        let handle = self.inner.handle(session_id).await?;
        self.inner
            .with_idle_location(&handle, self.checkout_idle(&handle, request))
            .await
    }

    async fn checkout_idle(
        &self,
        handle: &Handle,
        request: &BranchCheckout,
    ) -> Result<CheckedOutBranch, RuntimeError> {
        let state = handle.state.lock().await.clone();
        self.ensure_no_background(&state.info.id)?;
        ensure_checkout_allowed(handle, &state)?;
        let tip = resolve_tip(&state, request)?;
        let code = match request.code {
            true => Some(self.stage_branch_code(handle, &state, &tip).await?),
            false => None,
        };
        let revert = code.as_ref().map(|c| c.revert.clone());
        let checked_out = CheckedOut {
            tip_message_id: tip.clone(),
            code_restored: revert.is_some(),
            snapshot_id: revert.as_ref().and_then(|r| r.applied.clone()),
        };
        let mut events = vec![event(CHECKED_OUT, &checked_out)];
        if let Some(revert) = &revert {
            events.push(revert_event(&state.info.id, revert, RevertPhase::Stage));
        }
        // The restore awaited without the lock, so input may have arrived meanwhile: the
        // checks run again with the commit, and a refusal puts the files back.
        let committed = {
            let mut current = handle.state.lock().await;
            self.ensure_unchanged(handle, (&state, &current), request, &tip)
                .and_then(|()| self.inner.commit_locked(&mut current, events))
        };
        if let Err(error) = committed {
            if let Some(code) = &code {
                self.unstage_branch_code(&state, code).await?;
            }
            return Err(error);
        }
        Ok(CheckedOutBranch {
            tip_message_id: tip,
            code_restored: revert.is_some(),
            restored_files: code.map(|c| c.files).unwrap_or_default(),
            revert,
        })
    }

    /// The Session is still idle and in the state the checkout was prepared from.
    fn ensure_unchanged(
        &self,
        handle: &Handle,
        (prepared, current): (&SessionState, &SessionState),
        request: &BranchCheckout,
        tip: &str,
    ) -> Result<(), RuntimeError> {
        self.inner.ensure_idle(&current.info.id)?;
        self.ensure_no_background(&current.info.id)?;
        ensure_checkout_allowed(handle, current)?;
        let moved = resolve_tip(current, request)? != tip
            || current.active_tip() != prepared.active_tip()
            || current.revert != prepared.revert;
        match moved {
            true => Err(RuntimeError::Busy(current.info.id.clone())),
            false => Ok(()),
        }
    }

    /// A running Job (a process, a Monitor, a subagent or a workflow) or delegation hands
    /// its result back into the Session's active path when it ends. What started it may
    /// be leaving that path, so a checkout fails closed until none runs.
    fn ensure_no_background(&self, session_id: &str) -> Result<(), RuntimeError> {
        let jobs = self
            .jobs(Some(session_id))?
            .iter()
            .any(|job| job.status == JobStatus::Running);
        let delegations = self
            .inner
            .delegations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
            .any(|control| control.source == session_id && !control.done.is_cancelled());
        match jobs || delegations {
            true => Err(RuntimeError::Busy(session_id.to_owned())),
            false => Ok(()),
        }
    }

    /// Restore files to the snapshot after the last step on the path to `tip`. Nothing is
    /// recorded here; the caller commits the stage with the checkout.
    async fn stage_branch_code(
        &self,
        handle: &Handle,
        state: &SessionState,
        tip: &str,
    ) -> Result<BranchCode, RuntimeError> {
        let unavailable = |what: &str| {
            RuntimeError::Invalid(format!("{what}; check out without restoring code instead"))
        };
        let no_snapshots = || unavailable("Snapshots are unavailable for this Location");
        let mut projected = state.clone();
        projected.checkout(tip);
        let target =
            last_snapshot(&projected).ok_or_else(|| unavailable("The branch has no snapshot"))?;
        let message_id = projected
            .current_user_message()
            .ok_or_else(|| unavailable("The branch has no user message"))?
            .to_owned();
        let staged = state.revert.as_ref();
        let recorded = staged
            .and_then(|r| r.applied.clone())
            .or_else(|| last_snapshot(state))
            .ok_or_else(no_snapshots)?;
        let before = self
            .inner
            .take_snapshot(handle)
            .await
            .ok_or_else(no_snapshots)?;
        let baseline = staged
            .and_then(|r| r.baseline.clone())
            .unwrap_or_else(|| before.clone());
        let directory = &state.info.directory;
        self.inner.restore(directory, &target, &recorded).await?;
        // A three-way restore keeps user edits the target lacks, so the preview compares
        // the baseline with the restored working tree, not with the target.
        let restored = self.inner.take_snapshot(handle).await;
        let files = self
            .inner
            .preview_files(directory, &baseline, restored.as_deref().unwrap_or(&target))
            .await;
        let diff = files.iter().map(|f| f.patch.as_str()).collect();
        let revert = RevertState {
            message_id,
            target: RevertTarget::Code,
            baseline: Some(baseline),
            applied: Some(target),
            diff,
        };
        Ok(BranchCode {
            revert,
            files: files.into_iter().map(|f| f.file).collect(),
            before,
        })
    }

    /// Put the files a refused checkout restored back to the tree before it.
    async fn unstage_branch_code(
        &self,
        state: &SessionState,
        code: &BranchCode,
    ) -> Result<(), RuntimeError> {
        let applied = code.revert.applied.as_deref().unwrap_or(&code.before);
        self.inner
            .restore(&state.info.directory, &code.before, applied)
            .await
    }

    /// Set label `name` on `message_id`, or clear it when `None`. Names are unique per
    /// Session: a name already on another message must be cleared first.
    pub async fn label_branch(
        &self,
        session_id: &str,
        name: &str,
        message_id: Option<&str>,
    ) -> Result<(), RuntimeError> {
        if !valid_label(name) {
            return Err(RuntimeError::Invalid(format!(
                "Branch label {name:?} must match [A-Za-z0-9._-]{{1,48}}"
            )));
        }
        let handle = self.inner.handle(session_id).await?;
        let mut state = handle.state.lock().await;
        let current = state.branches().labels().get(name).cloned();
        match (message_id, current) {
            (Some(message), _) if !state.branches().contains(message) => {
                return Err(RuntimeError::MessageNotFound(format!(
                    "No message {message} in this Session"
                )));
            }
            (Some(message), Some(current)) if current == message => return Ok(()),
            (Some(_), Some(current)) => {
                return Err(RuntimeError::Conflict(format!(
                    "Branch label {name} is already on message {current}; remove it first"
                )));
            }
            (None, None) => {
                return Err(RuntimeError::MessageNotFound(format!(
                    "No branch label {name} in this Session"
                )));
            }
            _ => {}
        }
        let labeled = Labeled {
            name: name.to_owned(),
            message_id: message_id.map(str::to_owned),
        };
        self.inner
            .commit_locked(&mut state, vec![event(LABELED, &labeled)])
            .map(drop)
    }
}

/// Promotable input would start a Drain, a running compaction is summarizing the active
/// path, and a staged conversation rewind would move the tip on the next prompt, so a
/// checkout waits for all three.
fn ensure_checkout_allowed(handle: &Handle, state: &SessionState) -> Result<(), RuntimeError> {
    let promotable = state.pending(Delivery::Steer).next().is_some()
        || state.pending(Delivery::Queue).next().is_some();
    // A running compaction is summarizing the active path; its summary must stay there.
    if promotable || handle.is_compacting() {
        return Err(RuntimeError::Busy(state.info.id.clone()));
    }
    if state
        .revert
        .as_ref()
        .is_some_and(|r| r.target != RevertTarget::Code)
    {
        return Err(RuntimeError::Invalid(
            "A rewind is staged; commit or clear it before checking out a branch".into(),
        ));
    }
    Ok(())
}

/// The message a request names, then its newest tip unless `exact`.
fn resolve_tip(state: &SessionState, request: &BranchCheckout) -> Result<String, RuntimeError> {
    let tree = state.branches();
    let target = match (&request.message_id, &request.label) {
        (Some(message), None) if tree.contains(message) => message.clone(),
        (Some(message), None) => {
            return Err(RuntimeError::MessageNotFound(format!(
                "No message {message} in this Session"
            )));
        }
        (None, Some(label)) => tree.labels().get(label).cloned().ok_or_else(|| {
            RuntimeError::MessageNotFound(format!("No branch label {label} in this Session"))
        })?,
        _ => {
            return Err(RuntimeError::Invalid(
                "Name exactly one of message_id and label".into(),
            ));
        }
    };
    if request.exact {
        return Ok(target);
    }
    Ok(tree.newest_tip(&target).unwrap_or(&target).to_owned())
}

fn last_snapshot(state: &SessionState) -> Option<String> {
    state.steps.iter().rev().find_map(|s| s.post.clone())
}

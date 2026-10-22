//! The OpenAPI 3.1 document (`server-api` → OpenAPI document and versioning).
//!
//! One table lists every operation; request and response schemas come from the Rust types
//! the handlers use, so the document and the SDK generated from it cannot drift silently.

use schemars::generate::{SchemaGenerator, SchemaSettings};
use schemars::{JsonSchema, Schema};
use serde_json::{Map, Value, json};

use super::catalog::{Health, ToolInfo};
use super::envelope::{Data, Located, LocationInfo, Page};
use super::error::ErrorBody;
use super::events::{EventEnvelope, HistoryPage};
use super::service::{ServiceInfo, StopAccepted, StopService};
use super::sessions::*;
use super::{AgentInfo, CommandInfo, ModelInfo, SkillCatalog};
use crate::runtime::{FileDiff, InboxRow, PendingRequest, Receipt, RevertState, SessionRow};

/// One HTTP operation.
pub struct Op {
    pub method: &'static str,
    /// Path under `/api/v1`, with `{param}` placeholders.
    pub path: &'static str,
    pub id: &'static str,
    summary: &'static str,
    status: u16,
    request: Option<Schema>,
    optional_body: bool,
    response: Option<Schema>,
    query: Vec<&'static str>,
    located: bool,
    stream: bool,
    websocket: bool,
    /// Declared error responses beyond the default tagged error.
    errors: Vec<(u16, &'static str, Schema)>,
    /// Served by the peer listener with a grant's bearer token, not by the main API.
    peer: bool,
}

fn op(method: &'static str, path: &'static str, id: &'static str, summary: &'static str) -> Op {
    Op {
        method,
        path,
        id,
        summary,
        status: 200,
        request: None,
        optional_body: false,
        response: None,
        query: Vec::new(),
        located: false,
        stream: false,
        websocket: false,
        errors: Vec::new(),
        peer: false,
    }
}

/// An operation of the peer listener (`/api/v1/peer/*`), under group `peer`.
fn peer_op(
    method: &'static str,
    path: &'static str,
    id: &'static str,
    summary: &'static str,
) -> Op {
    Op {
        peer: true,
        ..op(method, path, id, summary)
    }
}

impl Op {
    fn body<T: JsonSchema>(mut self, g: &mut SchemaGenerator) -> Self {
        self.request = Some(g.subschema_for::<T>());
        self
    }

    /// A request body that may be omitted.
    fn optional_body<T: JsonSchema>(mut self, g: &mut SchemaGenerator) -> Self {
        self.optional_body = true;
        self.body::<T>(g)
    }

    fn ok<T: JsonSchema>(mut self, g: &mut SchemaGenerator) -> Self {
        self.response = Some(g.subschema_for::<T>());
        self
    }

    fn status(mut self, status: u16) -> Self {
        self.status = status;
        self
    }

    fn query(mut self, names: &[&'static str]) -> Self {
        self.query.extend_from_slice(names);
        self
    }

    /// Location-scoped: accepts `location[directory]` and `x-cyber-directory`.
    fn located(mut self) -> Self {
        self.located = true;
        self
    }

    fn websocket(mut self) -> Self {
        self.websocket = true;
        self
    }

    fn stream(mut self) -> Self {
        self.stream = true;
        self
    }

    fn error<T: JsonSchema>(
        mut self,
        g: &mut SchemaGenerator,
        status: u16,
        description: &'static str,
    ) -> Self {
        self.errors
            .push((status, description, g.subschema_for::<T>()));
        self
    }
}

pub fn operations(g: &mut SchemaGenerator) -> Vec<Op> {
    vec![
        op(
            "post",
            "/git/commit-message",
            "v1.git.commitMessage",
            "Generate a conventional commit message for the staged diff",
        )
        .located()
        .ok::<Located<super::vcs::CommitMessage>>(g),
        op(
            "post",
            "/git/pr-description",
            "v1.git.prDescription",
            "Draft a pull request title and body for base..HEAD",
        )
        .located()
        .optional_body::<super::vcs::PrDescriptionRequest>(g)
        .ok::<Located<super::vcs::PrDescription>>(g),
        op(
            "get",
            "/memory/requests",
            "v1.memory.requestStatus",
            "Look up durable save/delete request evidence without execution",
        )
        .located()
        .query(&["scope", "key"])
        .ok::<Located<Option<crate::runtime::MemoryRequestStatus>>>(g),
        op(
            "get",
            "/memory/settings",
            "v1.memory.settings",
            "Read the Location's effective memory toggle",
        )
        .located()
        .ok::<Located<super::memory::MemorySettingsView>>(g),
        op(
            "put",
            "/memory/settings",
            "v1.memory.setSettings",
            "Persist /memory on, off or readonly for the next Safe Boundary",
        )
        .located()
        .body::<super::memory::SetMemorySettings>(g)
        .ok::<Located<super::memory::MemorySettingsView>>(g),
        op(
            "get",
            "/memory/edit/{scope}/{name}",
            "v1.memory.editReview",
            "Review original Markdown and a fingerprint for a conditional edit",
        )
        .located()
        .ok::<Located<cyber_core::memory::MemoryEditReview>>(g),
        op(
            "get",
            "/memory/recovery/requests",
            "v1.memory.recoveryRequest",
            "Look up retained recovery request evidence without execution",
        )
        .located()
        .query(&["scope", "key"])
        .ok::<Located<Option<crate::runtime::MemoryRecoveryRequestStatus>>>(g),
        op(
            "get",
            "/memory/recovery/{scope}",
            "v1.memory.recovery",
            "Review matching interrupted memory storage and admission",
        )
        .located()
        .ok::<Located<Option<super::memory::MemoryRecoveryView>>>(g),
        op(
            "post",
            "/memory/recovery/{scope}",
            "v1.memory.recover",
            "Confirm both current memory recovery fingerprints",
        )
        .located()
        .body::<super::memory::RecoverMemory>(g)
        .ok::<Located<crate::runtime::MemoryChange>>(g),
        op(
            "put",
            "/memory/{scope}/{name}",
            "v1.memory.put",
            "Write a validated memory note and publish its durable receipt",
        )
        .located()
        .body::<super::memory::PutMemory>(g)
        .ok::<Located<crate::runtime::MemoryChange>>(g),
        op(
            "delete",
            "/memory/{scope}/{name}",
            "v1.memory.delete",
            "Delete a memory note and publish its durable receipt",
        )
        .located()
        .ok::<Located<crate::runtime::MemoryChange>>(g),
        op(
            "get",
            "/memory",
            "v1.memory.list",
            "Review memory metadata and invalid notes",
        )
        .query(&["scope"])
        .located()
        .ok::<Located<cyber_core::memory::MemoryCatalog>>(g),
        op(
            "get",
            "/memory/{scope}/{name}",
            "v1.memory.get",
            "Read a memory note",
        )
        .located()
        .ok::<Located<cyber_core::memory::MemoryDocument>>(g),
        op(
            "get",
            "/usage",
            "v1.usage.get",
            "Read durable Session subtree or Workflow Run usage",
        )
        .query(&["scope", "id"])
        .ok::<Data<crate::runtime::ScopeUsage>>(g),
        op(
            "get",
            "/usage/stats",
            "v1.usage.stats",
            "Activity per local day, streaks and per-model totals over a period",
        )
        .query(&["period", "directory", "tz"])
        .ok::<Data<crate::runtime::UsageStats>>(g),
        op(
            "get",
            "/usage/insights",
            "v1.usage.insights",
            "Contributing factors and subagent shares of the last 24 hours or 7 days",
        )
        .query(&["window", "directory"])
        .ok::<Data<crate::runtime::UsageInsights>>(g),
        op(
            "get",
            "/usage/limits",
            "v1.usage.limits",
            "The latest usage window readings providers reported in their responses",
        )
        .ok::<Data<crate::runtime::UsageLimits>>(g),
        op(
            "get",
            "/health",
            "v1.health.get",
            "Health and version (unauthenticated)",
        )
        .ok::<Health>(g),
        op(
            "post",
            "/service/stop",
            "v1.service.stop",
            "Stop the server matching this registration identity",
        )
        .body::<StopService>(g)
        .ok::<StopAccepted>(g),
        op(
            "get",
            "/service",
            "v1.service.get",
            "The listening server's identity, build and running work",
        )
        .ok::<ServiceInfo>(g),
        op(
            "get",
            "/jobs",
            "v1.job.list",
            "List durable background tasks",
        )
        .query(&["session_id", "limit", "cursor"])
        .ok::<Page<crate::runtime::Job>>(g),
        op("get", "/jobs/{id}", "v1.job.get", "Read a background task")
            .ok::<Data<crate::runtime::Job>>(g),
        op(
            "post",
            "/jobs/{id}/stop",
            "v1.job.stop",
            "Cancel a background task and await settlement",
        )
        .ok::<Data<crate::runtime::Job>>(g),
        op(
            "get",
            "/jobs/{id}/output",
            "v1.job.output",
            "Read the end of a background task's log",
        )
        .query(&["tail_bytes"])
        .ok::<Data<super::jobs::JobOutput>>(g),
        op(
            "post",
            "/jobs/{id}/pty/ticket",
            "v1.job.ptyTicket",
            "Issue a single-use ticket, valid for 60 seconds, to attach to a running PTY",
        )
        .ok::<Data<super::pty::PtyTicket>>(g),
        op(
            "get",
            "/jobs/{id}/pty/connect",
            "v1.job.ptyConnect",
            "Attach to a running PTY over WebSocket with a ticket: replays retained output from cursor (omitted: all, -1: none), then live output; client frames are input",
        )
        .query(&["ticket", "cursor"])
        .websocket(),
        op(
            "post",
            "/workflows/runs",
            "v1.workflow.run",
            "Start a Workflow Run from a saved name or an inline script; it executes in the server",
        )
        .status(201)
        .body::<super::workflows::StartBody>(g)
        .ok::<Data<crate::runtime::WorkflowRun>>(g),
        op(
            "get",
            "/workflows/runs",
            "v1.workflow.list",
            "List Workflow Run summaries of the Location (directory=all for every Location), active runs first, then the newest",
        )
        .located()
        .query(&["status", "session_id", "directory", "limit", "cursor"])
        .ok::<Page<crate::runtime::WorkflowRunSummary>>(g),
        op(
            "post",
            "/workflows/runs/resume_interrupted",
            "v1.workflow.resumeInterrupted",
            "Resume every run a server restart interrupted, of the Location or of all (directory=all)",
        )
        .located()
        .query(&["directory"])
        .ok::<Data<crate::runtime::InterruptedResume>>(g),
        op(
            "get",
            "/workflows/runs/{id}",
            "v1.workflow.get",
            "Read a Workflow Run's summary with its agents (agents_detail)",
        )
        .ok::<Data<super::workflows::RunDetail>>(g),
        op(
            "post",
            "/workflows/runs/{id}/pause",
            "v1.workflow.pause",
            "Pause a run: in-flight agents finish, no new agent starts",
        )
        .ok::<Data<crate::runtime::WorkflowRun>>(g),
        op(
            "post",
            "/workflows/runs/{id}/resume",
            "v1.workflow.resume",
            "Relaunch a paused, failed, stopped or budget-exceeded run through replay",
        )
        .optional_body::<super::workflows::ResumeBody>(g)
        .ok::<Data<crate::runtime::WorkflowRun>>(g),
        op(
            "post",
            "/workflows/runs/{id}/stop",
            "v1.workflow.stop",
            "Stop a run and cancel its in-flight agents",
        )
        .ok::<Data<crate::runtime::WorkflowRun>>(g),
        op(
            "get",
            "/workflows/runs/{id}/logs",
            "v1.workflow.logs",
            "Read a run's log() entries",
        )
        .ok::<Page<crate::runtime::WorkflowLogEntry>>(g),
        op(
            "get",
            "/sessions/{sessionID}/hook-executions",
            "v1.session.hooks",
            "List durable hook executions; running receipts do not establish a live process",
        )
        .query(&["limit", "cursor"])
        .ok::<Page<crate::runtime::HookExecutionRecord>>(g),
        op("get", "/openapi.json", "v1.health.openapi", "This document"),
        op("get", "/location", "v1.location.get", "Resolve a Location")
            .located()
            .ok::<Data<LocationInfo>>(g),
        op(
            "get",
            "/ws",
            "v1.event.websocket",
            "JSON-RPC 2.0 over WebSocket: operation IDs as methods, events as notifications",
        )
        .websocket(),
        op(
            "get",
            "/event",
            "v1.event.subscribe",
            "Live events for a Location (scope=all for every Location)",
        )
        .located()
        .query(&["scope"])
        .stream(),
        op(
            "get",
            "/sessions",
            "v1.session.list",
            "List root Sessions of a Location, newest first",
        )
        .located()
        .query(&["limit", "cursor", "search", "children", "archived", "pr"])
        .ok::<Located<Page<SessionRow>>>(g),
        op(
            "get",
            "/sessions/{id}/children",
            "v1.session.children",
            "List direct child threads across Locations with observed state",
        )
        .query(&["limit", "cursor"])
        .ok::<Data<super::children::ChildThreads>>(g),
        op(
            "post",
            "/worktrees",
            "v1.worktree.create",
            "Create a managed worktree Session and run setup",
        )
        .located()
        .status(201)
        .body::<super::worktrees::CreateWorktreeBody>(g)
        .ok::<Located<super::worktrees::CreatedWorktree>>(g),
        op(
            "get",
            "/worktrees",
            "v1.worktree.list",
            "List managed worktrees, Git status and owning Sessions",
        )
        .located()
        .ok::<Located<Vec<super::worktrees::WorktreeEntry>>>(g),
        op(
            "delete",
            "/worktrees/{name}",
            "v1.worktree.remove",
            "Remove a managed worktree and its branch; dirty ones need force",
        )
        .located()
        .query(&["force"])
        .ok::<Located<super::worktrees::RemovedWorktree>>(g),
        op(
            "post",
            "/sessions/{id}/worktree/cleanup",
            "v1.worktree.cleanup",
            "Remove a Session's clean worktree on exit, or keep it and say why",
        )
        .located()
        .ok::<Located<super::worktrees::WorktreeCleanup>>(g),
        op(
            "get",
            "/sessions/{id}/children/{child}/setup",
            "v1.worktree.inspectChildSetup",
            "Inspect a direct child's setup journal",
        )
        .ok::<Data<crate::worktrees::ChildSetupInspection>>(g),
        op(
            "post",
            "/sessions/{id}/children/{child}/setup",
            "v1.worktree.recoverChildSetup",
            "Explicitly retry a settled child setup failure or continue undispatched steps",
        )
        .body::<crate::worktrees::SetupRecoveryRequest>(g)
        .ok::<Data<super::worktrees::CreatedWorktree>>(g),
        op("post", "/sessions", "v1.session.create", "Create a Session")
            .located()
            .status(201)
            .optional_body::<CreateBody>(g)
            .ok::<Located<Session>>(g),
        op(
            "get",
            "/sessions/{sessionID}",
            "v1.session.get",
            "Get a Session",
        )
        .ok::<Data<Session>>(g),
        op(
            "patch",
            "/sessions/{sessionID}",
            "v1.session.update",
            "Rename or archive a Session",
        )
        .body::<UpdateBody>(g)
        .ok::<Data<Session>>(g),
        op(
            "delete",
            "/sessions/{sessionID}",
            "v1.session.delete",
            "Delete a Session and its children",
        )
        .status(204),
        op(
            "post",
            "/sessions/{sessionID}/fork",
            "v1.session.fork",
            "Fork a Session",
        )
        .status(201)
        .optional_body::<ForkBody>(g)
        .ok::<Data<Session>>(g),
        op(
            "post",
            "/sessions/{sessionID}/subtask",
            "v1.session.subtask",
            "Start an explicit named background child or fork the current context",
        )
        .status(202)
        .body::<SubtaskBody>(g)
        .ok::<Data<crate::runtime::Job>>(g),
        op(
            "post",
            "/sessions/{sessionID}/delegations/{requestID}",
            "v1.session.startDelegation",
            "Durably reserve a named user delegation request",
        )
        .status(202)
        .body::<SubtaskBody>(g)
        .ok::<Data<crate::runtime::Delegation>>(g),
        op(
            "get",
            "/sessions/{sessionID}/delegations/{requestID}",
            "v1.session.delegation",
            "Inspect owned delegation admission or recovery uncertainty",
        )
        .ok::<Data<crate::runtime::Delegation>>(g),
        op(
            "post",
            "/sessions/{sessionID}/delegations/{requestID}/reconcile",
            "v1.session.reconcileDelegation",
            "Link a lost admission receipt to an exactly bound existing Job without dispatch",
        )
        .ok::<Data<crate::runtime::Delegation>>(g),
        op(
            "post",
            "/sessions/{sessionID}/delegations/{requestID}/stop",
            "v1.session.stopDelegation",
            "Cancel one admission request or its recorded Job",
        )
        .ok::<Data<crate::runtime::Delegation>>(g),
        op(
            "post",
            "/sessions/{sessionID}/prompt",
            "v1.session.prompt",
            "Durably admit a prompt",
        )
        .status(202)
        .body::<PromptBody>(g)
        .ok::<Data<Receipt>>(g),
        op(
            "post",
            "/sessions/{sessionID}/command",
            "v1.command.run",
            "Run a skill or command as the prompt",
        )
        .status(202)
        .body::<CommandBody>(g)
        .ok::<Data<Receipt>>(g)
        .error::<super::error::ReviewTargetError>(
            g,
            422,
            "The bundled review target could not be resolved; nothing was admitted",
        ),
        op(
            "post",
            "/sessions/{sessionID}/approve",
            "v1.session.approve",
            "Confirm and replay the latest classifier-blocked call once",
        )
        .status(202)
        .ok::<Data<crate::runtime::AutoOverrideReceipt>>(g),
        op(
            "post",
            "/sessions/{sessionID}/reopen-subtree",
            "v1.session.reopenSubtree",
            "Reopen a reviewed acknowledged scope with fresh durable and native proof",
        )
        .body::<ReopenSubtreeBody>(g)
        .ok::<Data<crate::runtime::SubtreeReopenReport>>(g),
        op(
            "post",
            "/sessions/{sessionID}/stop-subtree",
            "v1.session.stopSubtree",
            "Close subtree admission and report bounded local stop acknowledgement",
        )
        .ok::<Data<crate::runtime::SubtreeStopReport>>(g),
        op(
            "post",
            "/sessions/{sessionID}/interrupt",
            "v1.session.interrupt",
            "Interrupt the running Drain",
        )
        .status(204),
        op(
            "post",
            "/sessions/{sessionID}/agent",
            "v1.session.agent",
            "Switch the agent",
        )
        .body::<AgentBody>(g)
        .ok::<Data<Session>>(g),
        op(
            "post",
            "/sessions/{sessionID}/model",
            "v1.session.model",
            "Switch the model",
        )
        .body::<ModelBody>(g)
        .ok::<Data<Session>>(g),
        op(
            "post",
            "/sessions/{sessionID}/mode",
            "v1.session.mode",
            "Switch the permission mode",
        )
        .body::<ModeBody>(g)
        .ok::<Data<Session>>(g),
        op(
            "post",
            "/sessions/{sessionID}/wake",
            "v1.session.wake",
            "Wake existing promotable Session input without admitting a new prompt",
        )
        .status(204),
        op(
            "post",
            "/sessions/{sessionID}/compact",
            "v1.session.compact",
            "Compact now or at the next Safe Boundary",
        )
        .status(202)
        .optional_body::<CompactBody>(g),
        op(
            "get",
            "/sessions/{sessionID}/recap",
            "v1.session.recap",
            "The latest Session recap, or null",
        )
        .ok::<Data<Option<crate::runtime::Recap>>>(g),
        op(
            "post",
            "/sessions/{sessionID}/recap",
            "v1.session.generateRecap",
            "Return the recap covering the latest finished Turn, generating it when stale",
        )
        .ok::<Data<crate::runtime::Recap>>(g),
        op(
            "post",
            "/sessions/{sessionID}/rewind",
            "v1.session.rewind",
            "Stage and commit a rewind",
        )
        .body::<RevertBody>(g)
        .ok::<Data<RevertState>>(g),
        op(
            "post",
            "/sessions/{sessionID}/revert/stage",
            "v1.session.revertStage",
            "Stage a revert",
        )
        .body::<RevertBody>(g)
        .ok::<Data<RevertState>>(g),
        op(
            "post",
            "/sessions/{sessionID}/revert/clear",
            "v1.session.revertClear",
            "Undo a staged revert",
        )
        .status(204),
        op(
            "post",
            "/sessions/{sessionID}/revert/commit",
            "v1.session.revertCommit",
            "Commit a staged revert",
        )
        .status(204),
        op(
            "post",
            "/sessions/{sessionID}/shell",
            "v1.session.shell",
            "Run a user shell command",
        )
        .body::<ShellBody>(g)
        .ok::<Data<ShellResult>>(g),
        op(
            "post",
            "/sessions/{sessionID}/background",
            "v1.session.background",
            "Move the Session's running foreground command to a background Job",
        )
        .ok::<Data<super::sessions::BackgroundResult>>(g),
        op(
            "post",
            "/sessions/{sessionID}/side",
            "v1.session.side",
            "Ask a side question (/btw) with a read-only copy of the Session's context and read-only tools; the Session's history is unchanged",
        )
        .body::<super::deliveries::SideBody>(g)
        .ok::<Data<crate::runtime::SideAnswer>>(g),
        op(
            "get",
            "/sessions/{sessionID}/files/{fileID}",
            "v1.session.sentFile",
            "Download a file the Session delivered with send_file",
        ),
        op(
            "get",
            "/sessions/{sessionID}/browser/evidence",
            "v1.session.browserEvidence",
            "Browser verification evidence of the Session, each record marked stale when the workspace changed since",
        )
        .ok::<Data<crate::runtime::BrowserEvidenceReport>>(g),
        op(
            "get",
            "/sessions/{sessionID}/messages",
            "v1.message.list",
            "List the active path's messages, or every branch's with branches=all",
        )
        .query(&["limit", "cursor", "order", "branches"])
        .ok::<Page<Message>>(g),
        op(
            "get",
            "/sessions/{sessionID}/tree",
            "v1.session.tree",
            "The user-message tree of every branch, with the active path and labels",
        )
        .ok::<Data<Vec<crate::runtime::BranchNode>>>(g),
        op(
            "post",
            "/sessions/{sessionID}/checkout",
            "v1.session.checkout",
            "Make another branch active on an idle Session, optionally restoring its code",
        )
        .body::<crate::runtime::BranchCheckout>(g)
        .ok::<Data<crate::runtime::CheckedOutBranch>>(g),
        op(
            "put",
            "/sessions/{sessionID}/labels/{name}",
            "v1.session.label",
            "Label a message of the branch tree",
        )
        .body::<super::branches::LabelBody>(g)
        .status(204),
        op(
            "delete",
            "/sessions/{sessionID}/labels/{name}",
            "v1.session.unlabel",
            "Remove a branch label",
        )
        .status(204),
        op(
            "get",
            "/sessions/{sessionID}/messages/{messageID}",
            "v1.message.get",
            "Get a message",
        )
        .ok::<Data<Message>>(g),
        op(
            "get",
            "/sessions/{sessionID}/inbox",
            "v1.session.inbox",
            "List inbox rows",
        )
        .ok::<Data<Vec<InboxRow>>>(g),
        op(
            "patch",
            "/sessions/{sessionID}/inbox/{messageID}",
            "v1.session.inboxEdit",
            "Edit an unpromoted input",
        )
        .status(204)
        .body::<InboxEditBody>(g),
        op(
            "delete",
            "/sessions/{sessionID}/inbox/{messageID}",
            "v1.session.inboxRemove",
            "Remove an unpromoted input",
        )
        .status(204),
        op(
            "post",
            "/sessions/{sessionID}/inbox/{messageID}/release",
            "v1.session.inboxRelease",
            "Release a held input",
        )
        .status(204)
        .optional_body::<ReleaseBody>(g),
        op(
            "post",
            "/sessions/{sessionID}/inbox/{messageID}/drop",
            "v1.session.inboxDrop",
            "Drop a held or queued input",
        )
        .status(204),
        op(
            "get",
            "/sessions/{sessionID}/context",
            "v1.session.context",
            "Context baseline, summary and task state",
        )
        .ok::<Data<SessionContext>>(g),
        op(
            "get",
            "/sessions/{sessionID}/diff",
            "v1.session.diff",
            "File diffs for a user message, or the whole Session without `message_id`",
        )
        .query(&["message_id"])
        .ok::<Data<Vec<FileDiff>>>(g),
        op(
            "get",
            "/sessions/{sessionID}/history",
            "v1.session.history",
            "Durable events after a sequence",
        )
        .query(&["after", "limit"])
        .ok::<HistoryPage>(g),
        op(
            "get",
            "/sessions/{sessionID}/events",
            "v1.session.events",
            "Replay durable events, then follow",
        )
        .query(&["after"])
        .stream(),
        op(
            "post",
            "/sessions/{sessionID}/permissions/{requestID}/reply",
            "v1.permission.reply",
            "Reply to a permission request",
        )
        .status(204)
        .body::<PermissionReplyBody>(g),
        op(
            "post",
            "/sessions/{sessionID}/questions/{requestID}/reply",
            "v1.question.reply",
            "Answer or dismiss a question",
        )
        .status(204)
        .body::<QuestionReplyBody>(g),
        op(
            "get",
            "/permissions/requests",
            "v1.permission.list",
            "Pending permission requests",
        )
        .query(&["session_id"])
        .ok::<Data<Vec<PendingRequest>>>(g),
        op(
            "get",
            "/permissions/test",
            "v1.permission.test",
            "Evaluate a tool call or shell command against the effective ruleset without a Session",
        )
        .located()
        .query(&["agent", "mode", "tool", "resource", "command"])
        .ok::<Located<serde_json::Value>>(g),
        op(
            "get",
            "/questions/requests",
            "v1.question.list",
            "Pending questions",
        )
        .query(&["session_id"])
        .ok::<Data<Vec<PendingRequest>>>(g),
        op("get", "/models", "v1.model.list", "Models, available first")
            .located()
            .ok::<Located<Vec<ModelInfo>>>(g),
        op(
            "get",
            "/plugins",
            "v1.plugin.list",
            "List the Location's plugins with scope, state and trust without loading them",
        )
        .located()
        .ok::<Located<cyber_core::plugins::PluginList>>(g),
        op(
            "get",
            "/hooks",
            "v1.hook.list",
            "Inspect resolved hook definitions and current trust without execution",
        )
        .located()
        .ok::<Located<cyber_core::hooks::HookReview>>(g),
        op(
            "post",
            "/hooks/trust",
            "v1.hook.trust",
            "Approve a current project/local hook digest",
        )
        .located()
        .body::<super::hooks::HookTrustBody>(g)
        .ok::<Located<super::hooks::HookApproval>>(g),
        op(
            "post",
            "/hooks/untrust",
            "v1.hook.untrust",
            "Revoke a checkout hook digest without loading configuration",
        )
        .located()
        .body::<super::hooks::HookTrustBody>(g)
        .ok::<Located<super::hooks::HookRevocation>>(g),
        op(
            "get",
            "/formatters",
            "v1.formatter.status",
            "Inspect local formatter detection without execution",
        )
        .located()
        .ok::<Located<Vec<super::FormatterStatus>>>(g),
        op(
            "get",
            "/lsp",
            "v1.lsp.status",
            "Inspect actual Location language-server state without startup",
        )
        .located()
        .ok::<Located<Vec<super::LspStatus>>>(g),
        op(
            "post",
            "/lsp/close",
            "v1.lsp.close",
            "Close and fence this Location's language servers until reload",
        )
        .located()
        .ok::<Located<super::LspTransition>>(g),
        op(
            "post",
            "/lsp/reload",
            "v1.lsp.reload",
            "Settle this Location's language servers and reopen lazy discovery",
        )
        .located()
        .ok::<Located<super::LspTransition>>(g),
        op(
            "get",
            "/vcs",
            "v1.vcs.status",
            "The Location's git branch, upstream, ahead/behind, worktree and dirty flag",
        )
        .located()
        .ok::<Located<super::vcs::Vcs>>(g),
        op(
            "get",
            "/system",
            "v1.system.stats",
            "CPU idle, cores, load, memory and the Location's disk on the server's machine",
        )
        .located()
        .ok::<Located<super::system::Machine>>(g),
        op(
            "get",
            "/features",
            "v1.features.list",
            "Every registered feature with its maturity, default, effective value and source for the Location",
        )
        .located()
        .ok::<Located<Vec<cyber_core::features::FeatureStatus>>>(g)
        .error::<ErrorBody>(g, 400, "InvalidRequestError, such as `ConfigInvalidError: unknown feature \"telepathy\"`"),
        op(
            "get",
            "/team-definitions",
            "v1.teams.definitions",
            "Team definitions of the Location (project ones only from a trusted checkout) and the files that failed to load, naming the field",
        )
        .located()
        .ok::<Located<cyber_core::teams::Definitions>>(g)
        .error::<ErrorBody>(g, 404, "FeatureDisabledError: the `teams` feature is off"),
        op(
            "get",
            "/teams",
            "v1.teams.list",
            "Teams led from the Location, newest first (`status`: active, stopping or stopped), with teammate and task counts and cost",
        )
        .located()
        .query(&["status"])
        .ok::<Located<Vec<crate::runtime::teams::TeamListing>>>(g)
        .error::<ErrorBody>(g, 404, "FeatureDisabledError: the `teams` feature is off"),
        op(
            "post",
            "/teams",
            "v1.teams.create",
            "Start a team led by `lead_session_id` from a definition of the lead's Location, from teammates, or both",
        )
        .body::<super::teams::CreateTeam>(g)
        .status(201)
        .ok::<Data<super::teams::CreatedTeam>>(g)
        .error::<ErrorBody>(g, 400, "InvalidRequestError, such as `Teammates cannot lead a team` or an unknown agent")
        .error::<ErrorBody>(g, 404, "SessionNotFoundError, or FeatureDisabledError: the `teams` feature is off")
        .error::<ErrorBody>(g, 409, "TeamLimitError: Team size limit reached (n), or ConflictError when the lead is already in a team"),
        op(
            "get",
            "/teams/{teamID}",
            "v1.teams.get",
            "A team: members with status, current task, cost and last message, the task board, budget and spend",
        )
        .ok::<Data<crate::runtime::teams::TeamView>>(g)
        .error::<ErrorBody>(g, 404, "TeamNotFoundError, or FeatureDisabledError"),
        op(
            "get",
            "/teams/{teamID}/tasks",
            "v1.teams.tasks",
            "The team's task board in creation order, optionally only one `status` or `owner`",
        )
        .query(&["status", "owner"])
        .ok::<Data<Vec<crate::runtime::teams::Task>>>(g)
        .error::<ErrorBody>(g, 404, "TeamNotFoundError, or FeatureDisabledError"),
        op(
            "post",
            "/teams/{teamID}/stop",
            "v1.teams.stop",
            "Stop a team: stopping while teammates finish their current Turn, then stopped; queued input stays queued",
        )
        .ok::<Data<crate::runtime::teams::Team>>(g)
        .error::<ErrorBody>(g, 404, "TeamNotFoundError, or FeatureDisabledError"),
        op(
            "post",
            "/teams/{teamID}/resume",
            "v1.teams.resume",
            "Resume a stopped team; after lead_deleted `lead` names the new lead",
        )
        .optional_body::<super::teams::ResumeTeam>(g)
        .ok::<Data<crate::runtime::teams::Team>>(g)
        .error::<ErrorBody>(g, 400, "InvalidRequestError, such as a new lead that is a teammate")
        .error::<ErrorBody>(g, 404, "TeamNotFoundError, or FeatureDisabledError")
        .error::<ErrorBody>(g, 409, "ConflictError: only a stopped team resumes"),
        op(
            "get",
            "/teams/{teamID}/events",
            "v1.teams.events",
            "Live `team.*` events of the team and its members' permission and question events, for the team view",
        )
        .stream()
        .error::<ErrorBody>(g, 404, "TeamNotFoundError, or FeatureDisabledError"),
        op(
            "get",
            "/mcp",
            "v1.mcp.status",
            "Inspect configured MCP state and retained ownership observations without startup",
        )
        .located()
        .ok::<Located<Vec<crate::runtime::McpServerStatus>>>(g),
        op(
            "post",
            "/mcp/close",
            "v1.mcp.close",
            "Close this Location's MCP connections; unresolved ownership returns conflict",
        )
        .located()
        .ok::<Located<super::mcp::McpClosed>>(g),
        op(
            "post",
            "/mcp/{name}/reconnect",
            "v1.mcp.reconnect",
            "Reconnect a server waiting for authentication, or a failed one that retained no owner, with a fresh owner; returns its status",
        )
        .located()
        .ok::<Located<crate::runtime::McpServerStatus>>(g),
        op(
            "get",
            "/messaging/sessions",
            "v1.messaging.sessions",
            "Sessions the caller (`from`, else a client) can message, newest first",
        )
        .query(&["from", "directory"])
        .ok::<Data<Vec<crate::runtime::messaging::ReachableSession>>>(g),
        op(
            "post",
            "/messaging/send",
            "v1.messaging.send",
            "Send a message to a Session as `from`, else as the client; a refusal is a 200 outcome",
        )
        .body::<super::messaging::SendMessage>(g)
        .ok::<Data<crate::runtime::messaging::SendResult>>(g)
        .error::<ErrorBody>(
            g,
            404,
            "AddressNotFoundError, SessionNotFoundError, or PeerNotConfiguredError with peer",
        )
        .error::<ErrorBody>(g, 409, "AmbiguousAddressError with candidates, or ConflictError")
        .error::<ErrorBody>(g, 413, "MessageTooLargeError with size and limit")
        .error::<ErrorBody>(g, 403, "AccountRequiredError")
        .error::<super::error::PeerErrorBody>(
            g,
            502,
            "PeerUnreachableError with peer and cause, or PeerAuthError with peer and reason",
        ),
        op(
            "post",
            "/messaging/watch",
            "v1.messaging.watch",
            "Subscribe `from` to one notice when the target next goes idle or exits; an idle target fires at once",
        )
        .body::<super::messaging::WatchSession>(g)
        .status(201)
        .ok::<Data<crate::runtime::messaging::Watch>>(g)
        .error::<ErrorBody>(
            g,
            404,
            "AddressNotFoundError, SessionNotFoundError, or PeerNotConfiguredError with peer",
        )
        .error::<ErrorBody>(g, 409, "AmbiguousAddressError with candidates")
        .error::<ErrorBody>(g, 429, "RateLimitedError with reason watch_quota")
        .error::<super::error::PeerErrorBody>(
            g,
            502,
            "PeerUnreachableError with peer and cause, or PeerAuthError with peer and reason",
        )
        .error::<ErrorBody>(
            g,
            403,
            "AccountRequiredError, or ForbiddenError when the target refuses the subscriber's messages (reason non_interactive, inbound_rule or disabled)",
        ),
        op(
            "get",
            "/sessions/{sessionID}/messaging",
            "v1.session.messaging",
            "Page the Session's sent and received cross-session messages with their outcomes; refused messages carry no text",
        )
        .query(&["cursor", "limit", "direction", "order"])
        .ok::<Page<crate::runtime::messaging::MessageRecord>>(g)
        .error::<ErrorBody>(g, 404, "SessionNotFoundError"),
        op(
            "get",
            "/sessions/{sessionID}/loops",
            "v1.session.loops",
            "The Session's active loops, then its 20 most recent ended ones, with their last 50 iterations",
        )
        .ok::<Data<Vec<crate::runtime::loops::LoopState>>>(g)
        .error::<ErrorBody>(g, 404, "SessionNotFoundError"),
        op(
            "post",
            "/sessions/{sessionID}/loops",
            "v1.session.createLoop",
            "Create a loop from `/loop` arguments (`command`) or parsed fields; its first iteration is due now",
        )
        .body::<super::loops::CreateLoop>(g)
        .status(201)
        .ok::<Data<crate::runtime::loops::LoopState>>(g)
        .error::<ErrorBody>(g, 400, "InvalidRequestError, such as `Loop interval must be at least 60s`")
        .error::<ErrorBody>(g, 404, "SessionNotFoundError")
        .error::<ErrorBody>(g, 409, "LoopLimitError"),
        op(
            "delete",
            "/sessions/{sessionID}/loops",
            "v1.session.stopLoops",
            "Stop every active loop of the Session; returns the stopped loop IDs",
        )
        .ok::<Data<Vec<String>>>(g)
        .error::<ErrorBody>(g, 404, "SessionNotFoundError"),
        op(
            "delete",
            "/sessions/{sessionID}/loops/{loopID}",
            "v1.session.stopLoop",
            "Stop one active loop; an iteration already admitted still runs",
        )
        .ok::<Data<Vec<String>>>(g)
        .error::<ErrorBody>(g, 404, "SessionNotFoundError or LoopNotFoundError"),
        op(
            "post",
            "/providers/{provider}/login",
            "v1.provider.login",
            "Start a subscription sign-in (only `chatgpt`); cancels a pending attempt. Open `authorization_url`; `callback` says whether the server waits for the browser's redirect",
        )
        .status(201)
        .ok::<Data<super::providers::LoginAttempt>>(g)
        .error::<ErrorBody>(g, 400, "ProviderLoginUnsupportedError, or InvalidRequestError on Windows")
        .error::<ErrorBody>(g, 503, "ServiceUnavailableError"),
        op(
            "get",
            "/providers/{provider}/login/{attemptID}",
            "v1.provider.loginAttempt",
            "A sign-in attempt: `pending`, `complete` with the account and models, or `failed` with the error",
        )
        .ok::<Data<super::providers::LoginAttempt>>(g)
        .error::<ErrorBody>(g, 400, "ProviderLoginUnsupportedError")
        .error::<ErrorBody>(g, 404, "ProviderLoginNotFoundError"),
        op(
            "post",
            "/providers/{provider}/login/{attemptID}/code",
            "v1.provider.loginCode",
            "Complete a pending attempt with the redirect URL pasted from the browser; a wrong `state` is refused and the attempt stays pending",
        )
        .body::<super::providers::LoginCode>(g)
        .ok::<Data<super::providers::LoginAttempt>>(g)
        .error::<ErrorBody>(g, 400, "InvalidRequestError or ProviderLoginUnsupportedError")
        .error::<ErrorBody>(g, 404, "ProviderLoginNotFoundError")
        .error::<ErrorBody>(g, 409, "ConflictError when the attempt is no longer pending"),
        op(
            "delete",
            "/providers/{provider}/login/{attemptID}",
            "v1.provider.cancelLogin",
            "Cancel a pending sign-in attempt and stop waiting for the browser",
        )
        .status(204)
        .error::<ErrorBody>(g, 404, "ProviderLoginNotFoundError"),
        op(
            "delete",
            "/providers/{provider}/login",
            "v1.provider.logout",
            "Forget the stored login and its model list; tokens are not revoked remotely",
        )
        .ok::<Data<super::providers::Logout>>(g)
        .error::<ErrorBody>(g, 400, "ProviderLoginUnsupportedError"),
        op(
            "get",
            "/peers",
            "v1.peers.list",
            "The running peer listener and every peer grant (never a token)",
        )
        .ok::<Data<super::peers::PeersOverview>>(g),
        op(
            "get",
            "/peers/remotes",
            "v1.peers.remotes",
            "The enabled remotes of user config and whether each answered its last listing (asked at most every 15 s, 5 s timeout)",
        )
        .ok::<Data<Vec<crate::runtime::messaging::RemoteReach>>>(g),
        op(
            "post",
            "/peers/grants",
            "v1.peers.invite",
            "Create a peer grant and return its pairing code, the only time its token is shown",
        )
        .body::<super::peers::InvitePeer>(g)
        .status(201)
        .ok::<Data<super::peers::PeerInvitation>>(g)
        .error::<ErrorBody>(
            g,
            409,
            "ConflictError: no peer listener runs, or an active grant has the name",
        ),
        op(
            "delete",
            "/peers/grants/{name}",
            "v1.peers.revoke",
            "Revoke a peer grant; its next request is refused",
        )
        .ok::<Data<super::peers::PeerRevoked>>(g),
        op("get", "/agents", "v1.agent.list", "Selectable agents")
            .located()
            .ok::<Located<Vec<AgentInfo>>>(g),
        op(
            "get",
            "/tools",
            "v1.tool.list",
            "Tools offered for an agent and mode",
        )
        .located()
        .query(&["agent", "mode"])
        .ok::<Located<Vec<ToolInfo>>>(g),
        op(
            "get",
            "/tools/schema",
            "v1.tool.schema",
            "Built-in tool schema bundle",
        )
        .located()
        .ok::<Located<Vec<ToolInfo>>>(g),
        op(
            "get",
            "/commands",
            "v1.command.list",
            "Commands and skills for autocomplete",
        )
        .located()
        .ok::<Located<Vec<CommandInfo>>>(g),
        op(
            "get",
            "/skills",
            "v1.skill.list",
            "Skills with source, scope, the agent's permission effect and diagnostics",
        )
        .located()
        .query(&["agent"])
        .ok::<Located<SkillCatalog>>(g),
        op("get", "/fs/find", "v1.fs.find", "Find files for @ mentions")
            .located()
            .query(&["query", "limit"])
            .ok::<Located<Vec<String>>>(g),
        op(
            "get",
            "/files",
            "v1.fs.list",
            "One project directory level with git status, ignore flags and read decisions",
        )
        .located()
        .query(&["path", "hidden", "session_id"])
        .ok::<Located<super::files::FileListing>>(g),
        op(
            "get",
            "/files/search",
            "v1.fs.search",
            "Fuzzy path search over the project's non-ignored files",
        )
        .located()
        .query(&["query", "limit", "session_id"])
        .ok::<Located<super::files::FileSearch>>(g),
        op(
            "get",
            "/files/content",
            "v1.fs.content",
            "A project file's text (at most 1 MiB) when the Session's rules allow read",
        )
        .located()
        .query(&["path", "session_id"])
        .ok::<Located<cyber_core::files::Content>>(g),
    ]
}

/// The peer listener's operations (`server-api` → Peer listener): served only on
/// `peers.listen`, never on the main API, each with an active grant's
/// `Authorization: Bearer cyp_…`. Group `peer`.
pub fn peer_operations(g: &mut SchemaGenerator) -> Vec<Op> {
    use super::error::PeerErrorBody;
    use crate::runtime::messaging::{
        PeerMessage, PeerOutcome, PeerSendResult, PeerSessionStatus, listing::PeerSessions,
    };
    const ATTACH: &str = "/peer/attach/{sessionID}";
    let scope = "PeerScopeError: the grant lacks the route's scope";
    let attached = "PermissionDeniedError: no approved attach ID in X-Cyber-Attach";
    vec![
        peer_op(
            "get",
            "/peer/hello",
            "v1.peer.hello",
            "This machine's name, version and key fingerprint, and the calling grant's name and scopes",
        )
        .ok::<super::peer::Hello>(g),
        peer_op(
            "post",
            "/peer/messages",
            "v1.peer.send",
            "Receive a message from a Session of the calling machine (scope message); the sender is named after the grant; idempotent on the grant and message ID",
        )
        .body::<PeerMessage>(g)
        .ok::<PeerSendResult>(g)
        .error::<PeerErrorBody>(g, 403, scope)
        .error::<ErrorBody>(g, 404, "AddressNotFoundError")
        .error::<ErrorBody>(g, 413, "MessageTooLargeError with size and limit")
        .error::<ErrorBody>(g, 429, "RateLimitedError with retryAfterMs"),
        peer_op(
            "get",
            "/peer/messages/{messageID}",
            "v1.peer.outcome",
            "The current outcome of a message the calling grant sent: delivered, held, released, dropped or refused",
        )
        .ok::<PeerOutcome>(g)
        .error::<PeerErrorBody>(g, 403, scope)
        .error::<ErrorBody>(g, 404, "NotFoundError: the grant sent no such message here"),
        peer_op(
            "get",
            "/peer/sessions",
            "v1.peer.sessions",
            "Root Sessions the calling grant could message, newest first, at most 50; Locations by workspace name unless peers.share_paths",
        )
        .query(&["limit"])
        .ok::<PeerSessions>(g)
        .error::<PeerErrorBody>(g, 403, scope),
        peer_op(
            "get",
            "/peer/sessions/{sessionID}/status",
            "v1.peer.status",
            "What one Session the grant can see is doing, for watches",
        )
        .ok::<PeerSessionStatus>(g)
        .error::<PeerErrorBody>(g, 403, scope)
        .error::<ErrorBody>(g, 404, "NotFoundError: no such Session is shown to this grant"),
        peer_op(
            "post",
            ATTACH,
            "v1.peer.attachOpen",
            "Ask the Session's user for remote.attach (resource peer:<grant>, up to 2 minutes); an approval returns the attach ID (scope attach)",
        )
        .ok::<super::peer::attach::Attach>(g)
        .error::<PeerErrorBody>(g, 403, "PeerScopeError, or PermissionDeniedError when refused or unanswered")
        .error::<ErrorBody>(g, 404, "NotFoundError: no such Session is shown to this grant"),
        peer_op("delete", ATTACH, "v1.peer.attachClose", "End this attach")
            .status(204)
            .error::<ErrorBody>(g, 403, attached),
        peer_op(
            "get",
            "/peer/attach/{sessionID}/session",
            "v1.peer.attachSession",
            "The attached Session, as GET /sessions/:id shows it",
        )
        .ok::<Data<Session>>(g)
        .error::<ErrorBody>(g, 403, attached),
        peer_op(
            "get",
            "/peer/attach/{sessionID}/messages",
            "v1.peer.attachMessages",
            "The attached Session's transcript, as GET /sessions/:id/messages",
        )
        .query(&["limit", "cursor", "order"])
        .ok::<Page<Message>>(g)
        .error::<ErrorBody>(g, 403, attached),
        peer_op(
            "get",
            "/peer/attach/{sessionID}/events",
            "v1.peer.attachEvents",
            "The attached Session's durable events after `after`; without it, its durable and live events from now on, as the instance stream carries them",
        )
        .query(&["after"])
        .stream()
        .error::<ErrorBody>(g, 403, attached),
        peer_op(
            "post",
            "/peer/attach/{sessionID}/prompt",
            "v1.peer.attachPrompt",
            "Prompt the attached Session as its user with text and image parts",
        )
        .status(202)
        .body::<super::peer::attach::PeerPrompt>(g)
        .ok::<Data<Receipt>>(g)
        .error::<ErrorBody>(g, 403, attached),
        peer_op(
            "post",
            "/peer/attach/{sessionID}/interrupt",
            "v1.peer.attachInterrupt",
            "Interrupt the attached Session's running Drain",
        )
        .status(204)
        .error::<ErrorBody>(g, 403, attached),
        peer_op(
            "get",
            "/peer/attach/{sessionID}/permissions",
            "v1.peer.attachPermissions",
            "The attached Session's pending permission requests, attach requests excluded",
        )
        .ok::<Data<Vec<PendingRequest>>>(g)
        .error::<ErrorBody>(g, 403, attached),
        peer_op(
            "post",
            "/peer/attach/{sessionID}/permissions/{requestID}/reply",
            "v1.peer.attachReply",
            "Reply once or reject to a permission request the attach lists (`always` is refused); never to an attach request",
        )
        .status(204)
        .body::<PermissionReplyBody>(g)
        .error::<ErrorBody>(g, 400, "InvalidRequestError")
        .error::<ErrorBody>(g, 403, attached)
        .error::<ErrorBody>(g, 404, "RequestNotFoundError"),
    ]
}

fn generator() -> SchemaGenerator {
    let mut settings = SchemaSettings::draft2020_12();
    settings.definitions_path = "/components/schemas".into();
    settings.into_generator()
}

pub fn document(version: &str) -> Value {
    let mut g = generator();
    let mut ops = operations(&mut g);
    ops.extend(peer_operations(&mut g));
    let error = g.subschema_for::<ErrorBody>();
    let envelope = g.subschema_for::<EventEnvelope>();
    let _ = g.subschema_for::<crate::runtime::McpStatusUpdate>();
    let _ = g.subschema_for::<crate::runtime::McpToolsChanged>();
    let _ = g.subschema_for::<crate::runtime::MemoryChange>();
    let _ = g.subschema_for::<crate::runtime::WorkflowRunProgress>();
    let mut paths = Map::new();
    for o in &ops {
        let entry = paths
            .entry(format!("/api/v1{}", o.path))
            .or_insert_with(|| json!({}));
        entry[o.method] = operation(o, &error, &envelope);
    }
    let mut schemas = g.take_definitions(true);
    schemas.insert(
        "ErrorBody".into(),
        serde_json::to_value(schemars::schema_for!(ErrorBody)).unwrap_or_default(),
    );
    json!({
        "openapi": "3.1.0",
        "info": { "title": "Cyber Code API", "version": version },
        "servers": [{ "url": "http://127.0.0.1:4747" }],
        "security": [{ "basic": [] }],
        "components": {
            "securitySchemes": {
                "basic": { "type": "http", "scheme": "basic" },
                "peerBearer": { "type": "http", "scheme": "bearer", "description": "A peer grant's cyp_ token, on the peer listener only" },
            },
            "schemas": schemas,
        },
        "paths": paths,
    })
}

fn operation(o: &Op, error: &Schema, envelope: &Schema) -> Value {
    let group = o.id.split('.').nth(1).unwrap_or("misc");
    let mut parameters: Vec<Value> = o
        .path
        .split('/')
        .filter_map(|s| s.strip_prefix('{').and_then(|s| s.strip_suffix('}')))
        .map(|name| json!({ "name": name, "in": "path", "required": true, "schema": { "type": "string" } }))
        .collect();
    if o.id == "v1.memory.delete" {
        parameters.push(json!({ "name": "x-cyber-memory-review", "in": "header", "required": false, "schema": { "type": "string", "pattern": "^[0-9a-f]{64}$" } }));
    }
    parameters.extend(o.query.iter().map(|name| json!({ "name": name, "in": "query", "required": false, "schema": { "type": "string" } })));
    if o.located {
        parameters.push(json!({ "name": "location[directory]", "in": "query", "required": false, "schema": { "type": "string" } }));
        parameters.push(json!({ "name": "x-cyber-directory", "in": "header", "required": false, "schema": { "type": "string" } }));
    }
    let success = if o.stream {
        json!({ "description": "Server-sent events; each `data` is an EventEnvelope", "content": { "text/event-stream": { "schema": envelope } } })
    } else {
        match &o.response {
            Some(schema) => {
                json!({ "description": "OK", "content": { "application/json": { "schema": schema } } })
            }
            None => json!({ "description": "OK" }),
        }
    };
    let mut responses = Map::new();
    responses.insert(o.status.to_string(), success);
    for (status, description, schema) in &o.errors {
        responses.insert(status.to_string(), json!({ "description": description, "content": { "application/json": { "schema": schema } } }));
    }
    responses.insert("default".into(), json!({ "description": "Tagged error", "content": { "application/json": { "schema": error } } }));
    if o.peer && o.path.starts_with("/peer/attach/{sessionID}/") {
        parameters.push(json!({ "name": "X-Cyber-Attach", "in": "header", "required": true, "schema": { "type": "string" } }));
    }
    let mut out = json!({ "operationId": o.id, "summary": o.summary, "tags": [group], "parameters": parameters, "responses": responses });
    if o.peer {
        out["security"] = json!([{ "peerBearer": [] }]);
        out["x-cyber-listener"] = json!("peer");
    } else if o.method != "get"
        && let Some(params) = out["parameters"].as_array_mut()
    {
        params.push(json!({ "name": "Idempotency-Key", "in": "header", "required": false, "schema": { "type": "string" } }));
    }
    if let Some(body) = &o.request {
        out["requestBody"] = json!({ "required": !o.optional_body, "content": { "application/json": { "schema": body } } });
    }
    if o.websocket {
        out["x-websocket"] = json!(true);
    }
    if o.id == "v1.health.get" {
        out["security"] = json!([]);
    }
    out
}

/// `(method, path)` of an operation ID, for `cyber api`.
pub fn lookup(id: &str) -> Option<(&'static str, &'static str)> {
    operations(&mut generator())
        .into_iter()
        .find(|o| o.id == id)
        .map(|o| (o.method, o.path))
}

/// Every operation as `(method, path)`, for route coverage tests.
pub fn routes() -> Vec<(&'static str, &'static str)> {
    operations(&mut generator())
        .into_iter()
        .map(|o| (o.method, o.path))
        .collect()
}

/// Every peer listener operation as `(method, path)`, for route coverage tests.
pub fn peer_routes() -> Vec<(&'static str, &'static str)> {
    peer_operations(&mut generator())
        .into_iter()
        .map(|o| (o.method, o.path))
        .collect()
}

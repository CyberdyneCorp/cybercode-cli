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
use super::service::{StopAccepted, StopService};
use super::sessions::*;
use super::{AgentInfo, CommandInfo, ModelInfo};
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
}

pub fn operations(g: &mut SchemaGenerator) -> Vec<Op> {
    vec![
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
            "Read durable Session subtree usage",
        )
        .query(&["scope", "id"])
        .ok::<Data<crate::runtime::UsageReport>>(g),
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
        .query(&["limit", "cursor", "search", "children", "archived"])
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
        .ok::<Data<Receipt>>(g),
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
            "get",
            "/sessions/{sessionID}/messages",
            "v1.message.list",
            "List messages",
        )
        .query(&["limit", "cursor", "order"])
        .ok::<Page<Message>>(g),
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
            "File diffs for a user message",
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
        op("get", "/fs/find", "v1.fs.find", "Find files for @ mentions")
            .located()
            .query(&["query", "limit"])
            .ok::<Located<Vec<String>>>(g),
    ]
}

fn generator() -> SchemaGenerator {
    let mut settings = SchemaSettings::draft2020_12();
    settings.definitions_path = "/components/schemas".into();
    settings.into_generator()
}

pub fn document(version: &str) -> Value {
    let mut g = generator();
    let ops = operations(&mut g);
    let error = g.subschema_for::<ErrorBody>();
    let envelope = g.subschema_for::<EventEnvelope>();
    let _ = g.subschema_for::<crate::runtime::McpStatusUpdate>();
    let _ = g.subschema_for::<crate::runtime::MemoryChange>();
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
            "securitySchemes": { "basic": { "type": "http", "scheme": "basic" } },
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
    responses.insert("default".into(), json!({ "description": "Tagged error", "content": { "application/json": { "schema": error } } }));
    let mut out = json!({ "operationId": o.id, "summary": o.summary, "tags": [group], "parameters": parameters, "responses": responses });
    if o.method != "get"
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

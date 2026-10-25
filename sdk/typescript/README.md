# @cyber-code/sdk

TypeScript client for the Cyber Code server API (`/api/v1`). ESM, Node ≥ 20, no runtime
dependencies (uses the global `fetch`).

```ts
import { Cyber } from "@cyber-code/sdk";

const client = Cyber.connect();                 // no I/O until the first call
const { data: session } = await client.session.create({ title: "demo" });
const result = await client.sessions.prompt(session.id, "fix the failing test", { wait: true });
console.log(result.text, result.stopReason, result.usage);
```

## Connecting

`Cyber.connect({ baseUrl?, auth?, fetch?, headers?, directory?, timeoutMs?, retry?, reconnect? })`

- With no `baseUrl` the client reads `<state>/server.json` on the first request. `<state>` is
  `$XDG_STATE_HOME/cyber`, else `$CYBER_HOME/state`, else `~/.local/state/cyber`.
- `auth` defaults to the local password file (`<state>/password`, HTTP Basic as `cyber`).
  Use `{ type: "password", password }` or `{ type: "account", token | tokenProvider }` (Bearer;
  the provider is called before every request and stream reconnect).
- `directory` is sent as `x-cyber-directory` on Location-scoped routes. `client.at(dir)` returns a
  scoped client and leaves the parent unchanged.

`Cyber.start({ mode, binary?, db?, directory?, signal? })` returns `{ client, registration?, close() }`:

- `attach` uses the registered server; `close()` is a no-op.
- `spawn` runs `cyber service start` first; `close()` stops the service only when this handle
  started it.
- `embedded` spawns a private `cyber serve --stdio` (in-memory database unless `db` is given,
  which sets `CYBER_DB`; no auth) and talks JSON-RPC 2.0 over its stdin/stdout. The client has
  the same surface as over HTTP (typed methods, `request()`, streams as subscriptions, prompt
  wait, `onRequest`). `close()` or aborting `signal` closes stdin and kills the child if it has
  not exited within 2 s. If the child exits, pending calls and streams fail with a permanent
  `CyberClientError` (`Transport`) instead of reconnecting.

```ts
const server = await Cyber.start({ mode: "embedded", directory: "/path/to/repo" });
const { data: session } = await server.client.session.create();
await server.close();
```

## Methods and results

Every operation in `sdk/openapi.json` is a method grouped by its OpenAPI tag:
`client.session.list(query?)`, `client.session.prompt(sessionID, body)`,
`client.message.list(sessionID, query?)`, `client.permission.reply(sessionID, requestID, body)`, …
Path parameters come first, then the body (or the query for GET routes), then
`{ idempotencyKey?, signal?, headers? }`. `client.request("v1.session.get", { path: { sessionID } })`
calls any operation by ID.

What a method resolves with:

| Response body                        | Resolves with                              |
| ------------------------------------ | ------------------------------------------ |
| `{ data }`                           | `data` (unwrapped)                         |
| `{ location, data }` (Location-scoped) | the whole envelope, so `location` stays available |
| `{ data: [...], cursor }` (pages)    | the whole page, so `cursor` stays available |
| `204` / `202` with no body           | `undefined`                                |

So `client.session.get(id)` gives a `Session`, while `client.session.create()` gives
`{ location, data: Session }` and `client.session.list()` gives `{ location, data: { data, cursor } }`.

Queue a prompt with `client.session.prompt(id, { parts: [{ type: "text", text: "follow up" }],
delivery: "queue", resume: false })`, then call `client.session.wake(id)` to dispatch the
existing inbox row. Wake takes no prompt body and resolves to `undefined`. Held input needs
`client.session.inboxRelease(id, messageID, { delivery: "queue" })` instead. An idle child keeps
its structured schema and starts a fresh attempt after verified checkout preparation;
preparation refusal leaves the row pending or held. A completed child with no promotable
input keeps its result without starting inference. Explicit wake dispatches one
deferred structured attempt. Prompts queued with `resume: true` during active child execution
hand off automatically after result collection or Job settlement. Interruption preserves their
rows and clears automatic wake intent; use an explicit wake to resume later. Interruption also
cancels owned checkout preparation, and late completion cannot admit or release input.
Unacknowledged preparation requires recovery before execution can resume.

## Reviewed memory edits

On supported Unix storage, review the original note before saving a draft:

```ts
const { data: review } = await client.memory.editReview("global", "policy");
await client.memory.put("global", "policy", {
  content: editedMarkdown,
  review_fingerprint: review.fingerprint,
}, { idempotencyKey: retainedSaveKey });
```

A changed note, index, scope or valid catalog entry returns a conflict. Preserve the draft and
obtain a fresh review before confirming another save. For a conditional delete, send
`headers: { "x-cyber-memory-review": review.fingerprint }` in the options to
`client.memory.delete(scope, name, options)`. Retain the exact key for an uncertain outcome;
identical acknowledged retries return the original receipt. Review is read-only and does not
create absent scopes. Windows memory mutations remain unavailable pending native acceptance.

For an uncertain save/delete response, use `client.memory.requestStatus({ scope, key })`
at the original directory. It reads durable admission, HTTP fingerprint, journal and
completion evidence even after response-cache loss or when memory is disabled.
`null` means no retained admission was found; it does not authorize a retry.
Recovery keys use `client.memory.recoveryRequest({ scope, key })` instead.

## Session budgets

`client.session.create({ budget: { max_tokens: 100000, max_cost_usd: 2, enforcement: "soft" } })`
persists a Session cap. All canonical fields are optional: `max_turns`, `max_tokens`,
`max_cost_usd`, `max_wall_seconds`, `enforcement` (default `soft`). Trusted `budgets.session`
provides creation defaults. Limits cover descendants and hidden model calls and survive restart;
wall time starts on the first gated dispatch and includes idle time. Soft limits allow in-flight
overshoot and prevent subsequent provider dispatch. Session responses retain the Budget;
`budget.warned.1` and `budget.exceeded.1` events identify `scope_id` and the limiting dimension.
Independent forks copy caps with fresh spending. Reserved enforcement is refused until reservations
are implemented; daily caps, budget-triggered subtree cancellation and full client displays remain open. The explicit local subtree-stop API is available; exec/TUI adoption remains open.

## Descendant usage

Session detail and list responses expose `children_cost`, `children_tokens`, `children_unpriced_steps` and `children_usage_complete` separately from own usage. Descendant totals include nested children and hidden title, compaction and evaluator calls, and persist across restart and child deletion. Retained billing receipts contain IDs and usage only, without prompt content; deleting an ancestor removes its receipts. Older databases are backfilled from surviving history and report `children_usage_complete: false` because previously deleted child billing cannot be reconstructed. New Sessions start with complete attribution. Soft subtree budget enforcement is implemented: ancestor caps include descendant spending and block subsequent provider dispatch, including after child deletion. Budget-triggered subtree cancellation and complete live client counters remain open.

## Durable delegation admission

Choose a unique `op_` request ID before sending the request:

```ts
const requestID = "op_" + crypto.randomUUID();
await client.session.startDelegation(session.id, requestID, {
  prompt: "find retry logic", agent: "explore", max_steps: 4,
});
const admission = await client.session.delegation(session.id, requestID);
await client.session.stopDelegation(session.id, requestID);
```

Keep that ID across transport retries. Identical input replays the admission; changed input conflicts. A stop before submission creates a tombstone preventing delayed dispatch. Poll `session.delegation` until admission settles. `admitted` provides `job_id` for task status/output through `client.job`; `unknown` prevents automatic redispatch. Call `session.reconcileDelegation(session.id, requestID)` to link a lost launching receipt to exactly one existing Job with matching historical operation/Session authority. Missing or ambiguous proof stays unknown; this does not settle uncertain native execution. Explicit stop can cancel an abandoned `reserved` record through the launch fence; `launching` uncertainty remains unknown. Cancellation targets the recorded Job only. Existing `session.subtask` returns the Job after admission; exec uses the durable protocol with request-scoped cancellation before a Job is returned; the TUI also uses saved IDs with `/admissions` lookup/cancellation and reconnect recovery.

## Errors

Tagged server errors reject with `CyberApiError` (`tag`, `status`, `body`). Generated guards
narrow them: `isSessionBusyError(err)`, `isSessionNotFoundError(err)`, … Requests that never got a
usable answer reject with `CyberClientError` whose `reason` is `Transport`, `UnexpectedStatus`,
`MalformedResponse` or `Timeout`.

Every non-GET call sends an `Idempotency-Key` (UUIDv7 unless you pass `idempotencyKey`). Over
HTTP it is retried up to 3 times on transport failures and timeouts with the same key, so the
server replays the first result instead of repeating the write. JSON-RPC errors map the same way:
`error.data` is the tagged body and the HTTP status comes from the code (`-32000 - status`).

## Events

```ts
for await (const event of client.sessions.events(sessionID, { after: lastSeq })) {
  lastSeq = event.durable?.seq ?? lastSeq;
}
for await (const event of client.events.subscribe()) { /* live events of the Location */ }
```

Streams reconnect with exponential backoff (500 ms → 15 s). Session streams resume from the last
`durable.seq` they yielded, so no durable event is lost or repeated. `break` (or aborting
`signal`) closes the connection.

## Permissions and questions

```ts
const stop = client.permissions.onRequest((req) => (req.action === "read" ? "once" : "reject"));
const stopQuestions = client.questions.onRequest((req) => req.questions.map((q) => [q.options[0]!.label]));
```

Handlers return `"once" | "always" | "reject"` (or `{ reply, message }`) for permissions, and
answer labels per question (or `undefined` to dismiss) for questions. On every (re)connect the
SDK first fetches the pending requests and handles those of the client's Location it has not
handled yet, so requests asked during a disconnect are not missed; each request is handled once.
This includes child requests routed to an ancestor in the client's Location, even when the child
runs in another worktree. `req.origin` identifies its title, agent and Location; `req.session_id`
remains the child owner used for replies. Live notifications and reconnect catch-up are deduplicated.

## Application tools

```ts
const unregister = await client.tools.register({
  name: "lookup_ticket",                      // ^[A-Za-z][A-Za-z0-9_-]{0,63}$, not a built-in name
  description: "Look up a ticket by ID",
  input: { type: "object", properties: { id: { type: "string" } }, required: ["id"] },
  execute: async ({ id }, { sessionID, callID }) => `ticket ${id}`,   // a throw is the tool's failure
});
await unregister();
```

The tool runs in your process: the server sends `tool.execute` over a JSON-RPC channel and
the result (a string, or any JSON value) goes back to the model. Over HTTP the channel is a
WebSocket to `/api/v1/ws` (global `WebSocket`, Node ≥ 22, or pass `webSocket` to
`Cyber.connect`) that opens on the first registration, re-registers every tool after a
reconnect (500 ms → 15 s backoff), and closes after the last tool is unregistered. In embedded
mode the stdio channel is reused. Registrations disappear when the channel disconnects.
Registration replies and execution requests carry `registration_id`; the SDK checks it against
the exact channel and handler before execution, so stale calls cannot reach a replacement.
Client tools use ordinary permission and hook checks and default to ask; plan mode hides them.
Configure an allow rule for the exposed tool name or answer the current permission request.

## Hook callbacks

```ts
const off = await client.hooks.on(
  "PreToolUse",
  (input) => {
    const path = (input.tool_input as { path?: string } | undefined)?.path ?? "";
    return path.startsWith("docs/") ? { decision: "allow", reason: "docs edits are pre-approved" } : undefined;
  },
  { matcher: "edit", timeout: 10 },     // configured matcher syntax; 1-600 s (default 60)
);
await off();
```

The handler runs in your process for every Session of the server while the channel is open:
the server sends `hook/execute` with the event envelope (the same JSON configured hooks get),
treats the returned object as a hook decision (`hooks` decision schema; nothing returned means
no decision) and merges it after every configured handler. Each execution is recorded with
scope `client` (`client.session.hooks(id)`). A throw, an invalid answer or a timeout is a
non-blocking error. Registrations share the tool channel: they are restored after a reconnect
and dropped when it closes. A managed-only hooks policy skips client handlers.

## Browser-safe types

`@cyber-code/sdk/schema` exports the generated types, the operation table and the error guards
with no Node built-ins.

## Development

```sh
npm run generate    # python3 ../../scripts/generate_sdk.py (regenerates src/generated)
npm run typecheck   # src, tests and examples
npm test
npm run build       # dist/
```

`src/generated/` is generated from `sdk/openapi.json`; do not edit it by hand.
`python3 scripts/generate_sdk.py --check` exits 1 when it is out of date. WebSocket operations
(`x-websocket: true`, e.g. `GET /api/v1/ws`) get no typed method. Examples live in
`examples/` (drive a session, stream events with resume, approve permissions, register an app
tool, run a workflow, run several workflows at once).

## Workflow runs

`client.workflows.run({ name | script, args?, budget?, max_concurrent?, session_id? })` starts a
Workflow Run in the server (`POST /api/v1/workflows/runs`) and returns a handle. Without
`session_id` it first creates an owner Session in the client's Location. `max_concurrent` is the
run's share of the server-wide agent slots. The handle offers:

- `status()`: the run's summary with its agents (`v1.workflow.get`)
- `events()`: this run's durable `workflow.*` events and its live `workflow.run.progress` events
  from the Location's event stream
- `pause()`, `resume({ revision? })` and `cancel()` (stop)
- `result()`: polls until the run ends; a `queued` run is still waiting, as a `running` one is. It
  resolves the script's return value when the run completed (`{ result_file, result_preview }`
  above 1 MiB) and rejects with `WorkflowRunError` (carrying the run) when it failed, exceeded its
  budget, stopped or paused

`client.workflows.list({ status?, session_id?, directory?, limit? })` yields a handle for each run
`GET /api/v1/workflows/runs` lists (active runs first, then the newest; `directory: "all"` for every
Location), each carrying its `summary` (agent counts, tokens, cost, active time). It reads further
pages lazily as the iteration reaches them. `client.workflows.get(id)` returns the handle of an
existing run, so a script can attach to a run another client started.

```ts
const run = await client.workflows.run({ name: "audit", args: { base: "main" }, budget: { max_cost_usd: 10 } });
console.log(await run.result());

for await (const active of client.workflows.list({ status: ["running", "queued"] })) {
  console.log(active.id, active.summary?.agents, await active.result());
}
```

## Cross-session messaging

`client.messaging` holds the generated `sessions({ from?, directory? })`, `send({ id?, from?, to,
text, notify_when_idle? })` and `watch({ from, target })`, plus `history(sessionId, { direction?,
order?, limit? })`, which yields a Session's sent and received messages
(`GET /api/v1/sessions/:id/messaging`) as `MessageRecord`s, reading further pages lazily. Without
`from`, `send()` sends as the client (`client:api`, or the `X-Cyber-Client` label); a refused
message is a result with `outcome: "refused"`, not an error, and its records carry no `text`. A
repeated `id` returns the first result with `replayed: true`. `MessageRecord` and `SendResult` are
also exported from `@cyber-code/sdk/schema`.

```ts
const sent = await client.messaging.send({ to: "backend", text: "deploy at noon" });
for await (const record of client.messaging.history(sent.to.session_id!, { direction: "in" })) {
  console.log(record.created_at, record.from, record.outcome, record.text ?? "");
}
```


`await client.mcp.close()` stops shared MCP connections for the client's Location and returns a Location envelope with `{ closed: true }` after verified settlement. A later Session open may establish fresh connections. Unresolved ownership returns `ConflictError` and keeps the Location fenced for retry or recovery; the operation does not clear unknown effects. Requests use the ordinary authentication, Location routing and idempotency handling.

MCP ownership changes arrive through `client.events.subscribe()` as `mcp.status.changed.1`, with the exported `McpStatusUpdate` payload and an independent connection aggregate. The payload reports committed status, phase and settlement acknowledgement; it does not prove current process liveness. Unknown settlement requires recovery, and private launch details remain in local logs. When a connected server's tools or prompts are re-listed after its `list_changed` notification, the live (non-durable) `mcp.tools.changed.1` event carries the exported `McpToolsChanged` payload with the server's current tool names and `<server>:<prompt>` commands; MCP prompts appear in `GET /commands` with `source: "mcp"`.

`await client.mcp.status()` reads fresh configured state and retained ownership observations without starting servers. It reports disabled/changed definitions and unresolved owners, including removed definitions that still need recovery. A persisted Running record without a retained actor reports failed. Discovery names still require ordinary tool authorization, and connected observations do not replace a fresh process health check.

Idle local connections are monitored for leader exit and ready stdio EOF; observed loss removes discovery and attempts durable native settlement. Local reconnect uses exponential 1–60 second backoff with at most ten attempts per loss sequence, after verified native/durable settlement. Bounded idle processing handles ready notifications and EOF behind them while retaining incomplete frames.

Local MCP catalogs refresh automatically after server list-change notifications. Discovery is temporarily unavailable during refresh; changed tool metadata invalidates old registration identities. The status API remains read-only. Local loss reconnects with fresh authorization and registration identities; unsafe ownership remains fenced, and interrupted tool calls are never replayed. Remote reconnect and manual retry controls remain pending.

## Project collaboration

`client.collaborations.forSession(sessionId, projectId?)` binds helper requests to one local Session and this client's checkout. It provides `status()`, `snapshot()`, abortable polling `watch()`, human `send()`, `createTask()`, `claim(task)` and `update(task, changes)`. Task claims carry the observed revision; updates also carry claim generation. Conflicts reject without taking another worker's assignment. Mutation transport retries keep the same operation ID.

Mutation results are `HubTask | CollaborationQueuedReceipt` (or `HubMessage | CollaborationQueuedReceipt` for sends). Use `isCollaborationQueued(result)` before treating a result as applied. A queued receipt preserves `operation_id`; inspect `status.receipts` rather than resending new intent. Supply `operationId` in mutation options to retry an uncertain operation explicitly. Authorization errors stop `watch()` without yielding stale snapshots.

`client.collaboration` retains the generated local owner operations. Use a separate bearer-authenticated client pointed at the explicit hub URL for the generated typed `client.hub` operations. Hub metadata requests never send a local Basic password, and protected hub calls require a separate bearer client before any request is sent. Hub URLs require HTTPS outside literal `127.0.0.0/8` or IPv6 `[::1]` loopback; HTTP `localhost` is not accepted. User information, query strings and fragments in the hub base URL are rejected before a token provider or network request runs. Hub HTTP requests reject redirects so credentials and content stay at the explicitly selected endpoint. Hub mutation results retain `{ data, replayed }`; issuance secrets are null on operation replay. The local owner API never returns access tokens or publication capabilities. Incoming agent messages remain untrusted and held until the receiver approves through the ordinary inbox.

See the [deployment and usage guide](../../docs/collaboration.md). The feature is experimental and off by default; live issuer provisioning and multi-host acceptance are separate from local signed-fixture tests.


## Experimental native computer controls

On an opted-in macOS14+ server, `client.tool.computerHealth()` reports native OS consent and the current server's grant without capturing pixels. Owner-only `client.session.computerPreview(sessionID, { app: "com.apple.TextEdit" })` resolves an exact target; after a human reviews it, `computerAllow(sessionID, { preview_id, confirm: true })` arms a finite grant. `client.tool.computerStopAll()` revokes this server's control independently of model work. These owner methods are separate from the model tool and are not hub-agent capabilities.

`client.session.computerStatus(sessionID)` includes unknown operations and the latest retained frame metadata. Explicit `computerFrame(sessionID, frameID)` returns an authenticated, bounded PNG as `Uint8Array` over HTTP; stdio refuses binary frame reads. Captures remain private and may be unavailable after retention. See [Native computer use](../../docs/computer-use.md) for setup, input limits, privacy and the still-pending physical-device acceptance.

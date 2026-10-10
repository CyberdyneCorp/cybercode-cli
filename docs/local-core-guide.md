# Local core guide

Detailed commands, client behavior, limits and terminology for the implemented local core and partial P1 features. Start with the [README](../README.md) or [documentation index](README.md); the [P1 status](implementation/p1-status.md) records acceptance evidence and remaining work.

## Build and development

Common tasks are in the [`justfile`](../justfile) (install [`just`](https://github.com/casey/just)); run `just` to list them:

```bash
just build          # debug build: target/debug/cyber
just ci             # local checks: lint, tests, specs, SDK (platform builds run in CI)
just test -p cyber-server        # one crate, or `just test <name filter>`
just test-linux     # the test suite on Linux in Docker (bubblewrap sandbox)
just tui            # the TUI here;  just exec "fix the failing test"
just sandboxed exec --ephemeral "hello"   # run with a throwaway CYBER_HOME
just sdk-generate   # regenerate sdk/openapi.json and the TypeScript SDK
```

The raw commands:

Requires Rust 1.89 or newer (edition 2024). SQLite is bundled.

```bash
cargo build                 # target/debug/cyber
cargo test --workspace      # unit, integration, crash-recovery and CLI tests
cargo clippy --workspace --all-targets -- -D warnings

cargo run -p cyber-cli --                   # the TUI in the current directory (starts the background server)
cargo run -p cyber-cli -- exec "fix the failing test"            # one non-interactive run
cargo run -p cyber-cli -- exec --format stream-json --ephemeral "explain this repo"
cargo run -p cyber-cli -- --worktree fix-login                   # TUI in a managed checkout
cargo run -p cyber-cli -- exec --worktree fix-login "fix login"  # reuse checkout, new Session
cargo run -p cyber-cli -- exec --worktree -- "fix login"         # generated worktree name
cargo run -p cyber-cli -- service status   # the background server; also start|stop|restart|password
cargo run -p cyber-cli -- api v1.session.list                    # any API operation
cargo run -p cyber-cli -- doctor           # config, keys, catalog, database, sandbox, tools, server
cargo run -p cyber-cli -- db backup ~/cyber-backup --artifacts   # online backup bundle with a manifest
cargo run -p cyber-cli -- debug info       # build, paths, database, config layers, trust
cargo run -p cyber-cli -- trust inspect    # repository-controlled definitions awaiting approval
cargo run -p cyber-cli -- hooks list       # resolved hooks, file origins, digests and handler trust
cargo run -p cyber-cli -- models openai    # catalog: available models first

# one real turn with a tool call (uses OPENAI_API_KEY)
cargo run -p cyber-llm --example turn -- openai/gpt-6-luna "What time is it in UTC?"
CYBER_LIVE_TESTS=1 cargo test -p cyber-llm --test live   # live provider checks

# a durable session with a tool loop, title and cost (uses OPENAI_API_KEY)
cargo run -p cyber-server --example session -- openai/gpt-6-luna

# a coding task with the built-in tools, the OS sandbox and snapshots (uses OPENAI_API_KEY)
cargo run -p cyber-tools --example agent -- openai/gpt-6-luna path/to/repo "Fix the failing test" accept-edits
```

## Sessions and subagents

Worktree startup runs trusted `worktrees.setup` commands inside the sandbox and reports output on stderr. Setup failure keeps the Session and files.

In the TUI, `/subtask <prompt>` forks a background child with the current context and keeps the parent open. API clients use POST `/sessions/{id}/subtask` with `{ "prompt": "try another approach" }`; TypeScript clients use `client.session.subtask(id, { prompt })`. `/tasks` (alias `/ps`) lists background subagents with status, elapsed time and final output. Enter opens the child thread; Ctrl+S stops the selected task. `/stop` asks for confirmation before stopping all running tasks of the Session. Interrupting the parent Turn leaves background tasks running. Server restart records unfinished tasks as interrupted and queues their handback without redispatch. The model can stop its own tasks with `task_stop`; API/SDK clients can list, read and stop jobs through `/jobs`. Bash, monitor, PTY and workflow Jobs remain unimplemented.

`/agent` (alias `/subagents`) browses direct child threads across worktree directories, showing observed running, waiting, completed or failed status. Enter opens an existing thread; Ctrl+R refreshes, and child threads include a parent navigation entry. Active-thread prompts retain the selected Session's ID, directory and steer delivery. API clients use GET `/sessions/{id}/children` with `limit` and an opaque `cursor`; SDK clients use `client.session.children(id, options)`. Idle child prompts start a fresh structured attempt under exclusive execution ownership, preserving the schema and child name. A tool still collecting its result refuses continuation; active-thread input remains steering. Held or non-waking input preserves the completed result. Acknowledged clean-removed checkouts now recreate through the same child identity and original base, with trusted source setup before inference. Successful continuations apply cleanup policy and record file summaries; failures and interruption retain the checkout. Cleanup confirmation blocks new input until it settles. Missing/replaced ownership, current deny rules, read-only recreation and incomplete setup refuse continuation. Complete queued/held dispatch, unknown-outcome resolution and native acceptance of this client path remain pending.

A leading TUI mention such as `@explore where is retry logic?` starts a fresh background child using that profile. The `@` menu includes visible subagent-capable agents beside files and quotes names containing spaces. Embedded/file mentions remain ordinary prompt text; use `@./path` for a file whose name matches an agent. Results return through the parent handback and `/tasks`. Before a Job is assigned, `/admissions` lists saved request IDs; Enter looks up a request and Ctrl+S cancels it. Esc cancels the current Session's pending admission before interrupting its conversation. IDs are saved before submission and looked up on reconnect without resubmitting prompts. Pending requests survive TUI exit; use `/admissions` to inspect or cancel them after reconnecting. API/SDK callers can pass an optional `agent` to the Session subtask endpoint; omitting it preserves the forked current-profile behavior. Use Enter to start a mentioned child; queued/held delegation and generic prompt admission are still being implemented.

`cyber exec '@explore find retry logic' --file notes.txt --max-turns 4` starts a fresh background child using a visible subagent-capable profile. Exec prints the child's complete durable text and usage and waits for its Job to settle, including cleanup. JSON results retain a client-generated `delegation_id` and parent Session before submission, then identify the child Session and Job after admission. Timeout and interruption cover request submission, queued admission and task following; they cancel only that request or its recorded Job. Local token/cost checks include the child’s own and nested descendant usage and request stop of the owned Job at the recorded limit. JSON reports combined `cost_usd` and `total_tokens` alongside `own_cost_usd`, own token classes and the four descendant billing fields. Budgeted delegation requires complete descendant billing from the attached server. Client polling can overshoot while calls remain in flight; server-side subtree scheduling and cancellation remain open. Unacknowledged cancellation is reported explicitly. Attachments retain their Content type, and the requested step limit is capped by the profile. Attached servers must advertise the durable admission routes and delegation fields. Session setup, catalogue discovery, file reads and capability negotiation precede the delegation timeout. Parent handback remains enabled; queued/held mention delivery and durable server-side subtree budget enforcement remain open.


## Auto-mode controls

In auto mode, approval-required tool actions use the configured evaluator (or small model). Decisions are recorded before execution; three consecutive blocks or an unavailable evaluator fall back to manual approval, or denial without an attached user.

Configure controls in trusted global or project configuration:

```json
{
  "permissions": {
    "auto_mode": {
      "rules": {
        "always_block": [{ "action": "bash", "resource": "kubectl apply *" }],
        "always_allow": [{ "action": "edit", "resource": "src/*.rs" }]
      },
      "policy": "Only make changes for the current ticket.",
      "classify_read_only": false,
      "fallback": "ask"
    }
  }
}
```

Block rules take precedence. Allow rules cover every resource and preserve hard denies, protected paths and parent manual approval. Read-only tools skip classification by default; set `classify_read_only` to true to review their approval-required requests. Set `fallback` to `deny` to refuse fallback even with a user attached. Project settings require trust. Inspect or clear recorded decision counters for the current checkout:

```bash
cyber permissions auto show
cyber permissions auto reset
cyber --format json permissions auto show
```

Counters include allows, blocks, fallbacks and classifier/policy decisions. They start when checkout scope was first recorded (or at reset); historical unscoped events are excluded. Reset preserves decision history and spending records and leaves other checkouts unchanged.

After a classifier block, use `/approve` in the TUI to review the original tool call and reason, then confirm one replay. It uses the stored input without another coding-model turn, saves no permanent approval and preserves current denies, protected paths and parent manual approval. Rejected or unattended confirmation has no effects. A consumed replay remains spent even after restart or failure; unresolved effects require recovery. Configuration block rules cannot be overridden this way. API clients can POST `/api/v1/sessions/:id/approve` or call `client.session.approve(id)`. The returned receipt identifies the original decision and replay call; subscribe to Session events and answer the `auto_override` permission request to proceed.

Children also retain every effective auto-mode ancestor's gate. Eligible requests use each parent's policy and task context, and all gates must allow before execution. Any configured block or deny fallback prevents effects; independent manual approvals still prompt. Review decisions and usage are recorded on the executing child with the reviewed ancestor's identity.

## Budgets and deferred input

New Sessions accept a `budget` object through POST `/sessions`: `{ "max_turns": 20, "max_tokens": 100000, "max_cost_usd": 2, "max_wall_seconds": 600, "enforcement": "soft" }`. Omit fields to leave those dimensions unlimited; a zero cap blocks dispatch. Trusted `budgets.session` configuration supplies creation defaults. Caps persist across restart and new prompts, count the Session and descendants, and apply to visible Turns, title, evaluator, compaction and web-page summary calls. Wall time starts at the first gated dispatch and continues while idle. Soft limits publish durable 80% warnings and exhaustion events, let in-flight calls settle and block new calls at the recorded limit; concurrent calls can overshoot. Independent forks copy the cap with fresh spending and activation. Project budget overrides require workspace trust. Reserved enforcement is currently refused; reservations, daily caps, budget-triggered subtree cancellation and complete client budget displays remain open. The runtime has bounded local subtree-stop reporting; exec/TUI adoption remains open.

Deferred prompts (`resume: false`) can be dispatched through authenticated POST `/api/v1/sessions/{id}/wake` or SDK `session.wake(id)`, without resubmitting their text. Held input needs explicit inbox release. For idle children, both paths prepare the verified checkout and reset the structured attempt while preserving the original inbox identity, admission sequence and edited content. Preparation failure keeps the input pending or held. Waking a completed child without promotable input preserves its result without checkout recreation or inference. Requested child prompts queued during an active attempt now hand off automatically after the foreground collector or background Job settles its original result and billing. `resume: false` prompts remain deferred; interruption clears automatic wake intent while retaining their rows. Interruption cancels and awaits owned checkout preparation; late success cannot admit or release input. Unacknowledged preparation requires recovery after restart. Durable source/target admission fences and subtree closure are implemented in the runtime. Reviewed unknown-effect recovery and native acceptance remain open.

Session detail and list responses expose `children_cost`, `children_tokens`, `children_unpriced_steps` and `children_usage_complete` separately from own usage. Descendant totals include nested children and hidden title, compaction and evaluator calls, and persist across restart and child deletion. Retained billing receipts contain IDs and usage only, without prompt content; deleting an ancestor removes its receipts. Older databases are backfilled from surviving history and report `children_usage_complete: false` because previously deleted child billing cannot be reconstructed. New Sessions start with complete attribution. Subtree budget enforcement and live client counters remain open.

For durable API admission, POST `/sessions/{id}/delegations/{requestID}` with the subtask body and a client-selected `op_` ID. GET that path to inspect admission, and POST `/stop` beneath it to cancel that request or its recorded Job. Repeating the same ID/input returns the existing record; different input conflicts. A stop received before submission prevents a delayed POST from dispatching. `admitted` exposes a Job ID to follow through `/jobs`; `unknown` preserves recovery uncertainty and prevents automatic redispatch. The SDK methods are `session.startDelegation`, `session.delegation` and `session.stopDelegation`. Exec uses this protocol and recovers a lost submission response by looking up the same request without redispatch. The TUI uses saved IDs for named mentions and `/subtask`, with bounded cancellation and explicit unknown outcomes. An explicit stop can resolve an abandoned `reserved` admission as cancelled because no launch marker was committed; `launching` uncertainty remains fenced. Automatic resolution of unknown native effects remains open.


## Explicit subtree stop

POST `/api/v1/sessions/{id}/stop-subtree` or SDK `client.session.stopSubtree(id)` closes admission and requests a bounded local sweep of the Session and descendants. The response includes `session_id`, `scope_id`, `status` (`acknowledged` or `unknown`), `problems` and `persisted`. HTTP 200 means a report is available; inspect its status and persistence before treating cancellation as acknowledged. Queued/held input stays intact and unrelated Jobs continue.

Admission remains closed after either report. Repeating a new request reassesses the same closure; replaying an idempotency key returns the original observation. Ordinary interruption keeps its existing background-child independence.

For a persisted `acknowledged` report with a `receipt_id`, explicitly review and submit POST `/api/v1/sessions/{id}/reopen-subtree` with `{ "scope_id": "op_...", "stop_receipt_id": "evt_..." }`, or SDK `client.session.reopenSubtree(id, review)`. The server replays fresh Session state, checks scoped actor evidence and retains native checkout ownership through the writer commit. Stale, Unknown, changed or orphaned evidence refuses reopening. The response includes `reopen_receipt_id`; reopening preserves input and starts no inference. Independently closed descendant scopes stay closed. Use an explicit wake or held-input release afterward when needed.

Reviewed recovery for unknown native effects and orphaned sweep/operation owners remains required. Exec/TUI and budget-triggered adoption remain open.

## Worktrees and setup recovery

`cyber worktree list` shows path, branch, dirty state, ahead/behind counts relative to the creation base and associated Sessions; use `--format json` for structured output. Pending or invalid ownership records remain visible for recovery. Startup worktrees remain on disk; public removal/prune, automatic exit cleanup and enter/exit commands are still being implemented.

The model's `agent` tool accepts `isolation: "worktree"` for fresh foreground/background children, including forked and structured tasks. Each child gets a branch `cyber/<parent-short-id>/<name>` and a durable checkout binding. Isolated child names may use the Session naming limit of 128 bytes, subject to Git branch-name validation; managed storage names are generated separately. Its result includes `worktree.path`, `branch`, `kept` and changed files with line counts; binary/large-file counts can be unknown. Clean checkouts are removed under the default cleanup policy. Edits, commits ahead of the base, setup failures and cancellation retain the checkout. Set `worktrees.keep: "always"` to retain clean checkouts too. `cleanup: "ask"` requests child-owned confirmation through the parent approval route; rejection or an unattended session retains the checkout. New edits, keep policy and deny rules are rechecked after approval. Interrupt/stop clears pending cleanup requests and preserves the checkout. Retained children resume against the same verified checkout. Clean-removed children resume with the same Session/branch and a new checkout identity, pinned to the original base; current trusted source setup must complete before inference. Missing, force-removed, incomplete or replaced removal evidence requires recovery. Setup failure stays fenced across restart. Session streams and history preserve Location across rebinding/replay, and old/new Location listeners receive the rebound notice before later events route to the new Location. The TUI updates its directory immediately and scopes later actions to it. Existing primary-profile forks and user subtasks can resume, while fresh primary subagent spawns retain their eligibility checks. An explicit `/subtask` uses the current profile’s isolation setting and authorizes checkout creation and trusted source setup in every Mode. Child tools keep their inherited Mode, approval requests and deny ceilings; read-only sandbox policy still refuses creation. Native platform acceptance is pending.

The APIs are `GET /api/v1/worktrees` for listing and `POST /api/v1/worktrees` for startup, with `{ "name": "fix-login", "session": { "model": "provider/model" } }`; omit `name` to generate one.

For a setup-pending isolated child, inspect `GET /api/v1/sessions/{parent}/children/{child}/setup`, then submit `POST` to the same path with the reviewed `revision`, `digest`, optional `retry_index` and a nonempty `reason`. The SDK methods are `worktree.inspectChildSetup(parent, child)` and `worktree.recoverChildSetup(parent, child, review)`. An explicit retry preserves earlier successful steps and the original failure; use an idempotency key to replay the same request safely. Omit `retry_index` to continue undispatched steps or acknowledge fully completed setup. A command preparation failure recorded as `not_dispatched` can also be retried after review because it never reached process launch. Changed recipes, stale reviews, active/unknown ownership, pending command outcomes and generic execution or launch errors without an acknowledged exit cannot authorize retry. Recovery retains current deny/sandbox rules and starts no inference.

## Usage and cost

`GET /api/v1/usage?scope=session&id=<session>` returns own, descendant and combined billing from one database snapshot, including retained charges after descendant deletion. Token classes, combined known tokens, unpriced calls and attribution completeness remain separate. The SDK method is `client.usage.get({ scope: "session", id })`; other scopes currently return a tagged unavailable error.

In the TUI, `/cost` reads the atomic usage API and opens a Session-and-descendants snapshot with all five token classes, cost and cache hit rate. Press `R` to refresh and `Esc` to close. Failed refreshes retain the last observed amounts with an error. Unpriced calls and incomplete historical attribution are shown explicitly; the view labels known spending as a lower bound when necessary.

On Linux the sandbox needs bubblewrap (`apt install bubblewrap`); without it commands fail closed unless you pass `--sandbox full-access`.

## Workspace and specifications

| Crate | Role |
|---|---|
| `cyber-core` | Build info, XDG paths, IDs, config layering and trust gating, evaluation manifests |
| `cyber-store` | SQLite WAL/FULL, migrations, event store, writer queue, ownership lock |
| `cyber-cli` | The `cyber` binary |
| `cyber-llm` | Provider-neutral LLM layer: adapters, retry, catalog, credentials, cost |
| `cyber-server` | Durable session runtime and the `/api/v1` HTTP, SSE, WebSocket and stdio JSON-RPC API |
| `cyber-tools` | Built-in tools, permission engine, bash analysis, skills, web tools, tool host |
| `cyber-sandbox` | OS sandbox: Seatbelt (macOS), bubblewrap (Linux), credential masking, network allowlist proxy |
| `cyber-snapshot` | Shadow-git working-tree snapshots and conflict-aware restore |
| `cyber-tui` | Terminal UI: composer, streaming rendering, permission and question prompts, pickers, themes |
| `cyber-app` | Server assembly and lifecycle: store, catalog, tools, sandbox, snapshots, listeners, registration |
| `cyber-client` | Rust client for the API (HTTP and in-process), used by `cyber exec` and the TUI |

The TypeScript SDK `@cyber-code/sdk` lives in [`sdk/typescript`](../sdk/typescript/README.md). It is generated from [`sdk/openapi.json`](../sdk/openapi.json) by `scripts/generate_sdk.py`; regenerate the document with `UPDATE_OPENAPI=1 cargo test -p cyber-server --test http openapi_document_is_current`.

Evaluation fixtures and their manifests live in [`eval/`](../eval/README.md).

CI and local specification checks use OpenSpec 1.13.2. Pin the validator so upstream rule changes can be reviewed separately from product changes.

```bash
npm install -g @fission-ai/openspec@1.13.2
openspec list --specs
openspec validate --all --strict
python3 scripts/spec_lint.py      # cross-spec registries: commands, routes, prefixes, phases
python3 scripts/spec_inventory.py # refresh the ROADMAP requirement table
```

---

## Design principles

1. **Provider independence.** The local core supports OpenAI Responses, Anthropic Messages and compatible Chat endpoints, including local servers. Additional native adapters and mixed-model workflows are planned.
2. **Local-first.** Local state and local-model execution do not require a Cyber account. Hosted model calls require connectivity and provider credentials. Cyber Account and hosted services are planned.
3. **Server-first.** The TUI, `exec` and TypeScript SDK share the public server API. IDE, web and mobile clients are planned.
4. **Durable execution.** Prompt admission, tool calls and context changes are recorded before execution proceeds. Recovery handles interrupted streams and uncertain tool outcomes explicitly; file-backed storage uses FULL durability under the documented filesystem assumptions.
5. **Controlled execution.** Permission rules, protected paths, an OS sandbox and snapshots constrain repository work. Production auto-mode permission requests now use durable classifier allow/block/fallback decisions; protected/manual ceilings remain. Windows enforcement and complete auto-mode controls remain M1.1 work.
6. **Familiar conventions.** Repository instructions and skills use familiar `AGENTS.md`/`CLAUDE.md` and `SKILL.md` conventions. MCP integration and setup import are later roadmap work.

## Architecture

See the architecture diagram in the [README](../README.md#architecture) and the [storage decision](../docs/decisions/0001-storage-architecture.md).

## Naming

| Thing | Value |
|---|---|
| Binary | `cyber` |
| Project config | `cyber.jsonc` / `cyber.json` at any level, and `.cyber/` directories |
| Global config | `$XDG_CONFIG_HOME/cyber` (`~/.config/cyber`), overridable by `CYBER_CONFIG_DIR` |
| Data / state / cache | `~/.local/share/cyber` · `~/.local/state/cyber` · `~/.cache/cyber` |
| Database | `<data>/cyber.db` |
| Env prefix | `CYBER_` |

## Specification glossary

These terms describe the product contracts. Workflows, goals, loops, runners, Relay, devices, channels and Cyber Account are planned capabilities. Subagent execution is partially implemented; auto-mode tool classification is implemented locally while its full configuration/override contract remains open. This table is not a list of shipped features.

| Term | Meaning |
|---|---|
| **Location** | `{ directory, workspace? }`. A working directory resolved to a Project; it scopes config, tools and permissions. |
| **Project** | Identified from git (hash of `origin` → cached ID → first root commit), else `global`. |
| **Session** | A durable conversation bound to one Location, with an agent, a model and a permission mode. |
| **Admitted Prompt** | Input durably recorded in the session inbox before execution, with `delivery` = `steer` (next safe boundary), `queue` (when idle) or `hold` (needs approval). |
| **Turn** | One provider request/stream plus the settlement of its tool calls. |
| **Drain** | The process-local loop that runs Turns for a Session until nothing is eligible. Only one Drain runs per Session. |
| **Safe Boundary** | The point between Turns where input, context updates, messages and goal checks are applied. |
| **Context Epoch** | A stable, cacheable system-prompt baseline. Changes arrive as Mid-Conversation System Messages until the next compaction. |
| **Agent** | A named profile (system prompt, model, mode, tools, permissions, step limit). Built-ins: `build` (primary), `explore` and `general` (subagents). **Subagent**: an Agent run in a child Session spawned by a tool. |
| **Workflow** | A JS/TS script, run by the Workflow runtime, that orchestrates many subagents. **Workflow Run**: one execution of it, resumable. |
| **Goal** | A completion condition attached to a Session and checked by an evaluator after each Turn. A Session has one active Goal plus an ordered goal queue. |
| **Loop** | A prompt re-run on a fixed interval or self-paced inside a Session. |
| **Routine** | A saved prompt + repos + triggers (schedule / API / GitHub / webhook) executed on a Runner. |
| **Runner** | A machine that executes Sessions for a remote client: the user's own machine, a self-hosted runner, or Cyber Cloud. |
| **Relay** | The rendezvous service (Cyber Cloud or self-hosted) that connects remote Devices to a local server. It carries end-to-end encrypted traffic only. |
| **Peer** | Another `cyber` server the user controls, reached directly over its API (LAN, VPN, SSH tunnel) without the Relay or an account. |
| **Device** | A client installation (phone, browser, other machine) paired to a Cyber Account. |
| **Channel** | An inbound event source (webhook, chat bridge, CI, MCP channel server) that admits messages into a running Session. |
| **Mode** | A permission mode: `default`, `accept-edits`, `plan`, `auto`, `dont-ask`, `bypass`. Planning is a Mode, not an agent. |
| **Cyber Account** | An identity from CyberdyneAuth. It carries `sub`, `orgs`, `entitlements` and `roles`. |

## Specification capability map

This is the full product scope, spanning P0–P4. See the [roadmap](../ROADMAP.md) for each capability’s delivery phase; it is not an availability matrix.

| Area | Capabilities |
|---|---|
| Foundation | `cli-commands`, `configuration`, `storage-events`, `installation-upgrade` |
| Models & identity | `provider-catalog`, `provider-credentials`, `cyber-account` |
| Agent runtime | `session-runtime`, `system-context`, `memory`, `compaction`, `agents-subagents` |
| Tools & safety | `workspace-trust`, `tool-registry`, `builtin-tools`, `permissions-modes`, `sandbox`, `snapshots-checkpoints`, `worktrees`, `code-intelligence` |
| Autonomy & orchestration | `workflows`, `goals`, `loops-scheduling`, `background-tasks`, `agent-teams` |
| Connectivity | `server-api`, `client-sdk`, `cross-session-messaging`, `remote-control`, `runners-cloud`, `routines`, `channels` |
| Extensibility | `hooks`, `plugins-marketplace`, `mcp`, `skills-commands` |
| Surfaces | `tui`, `web-client`, `exec-mode`, `editor-integration`, `vcs-integration`, `session-sharing` |
| Operations | `harness-evaluation`, `browser-verification`, `observability-costs`, `org-policy`, `compat-import` |

## Product focus

The first release targets dependable local coding: inspect and edit a repository, verify the result, preserve user changes, and resume after interruption. Its release gates measure coding success, cost, recovery and trust behavior.

OpenCode, Codex and Claude Code are design references. Provider independence, a public server API, durable execution and later multi-model workflows define Cyber Code's direction. Competitor parity claims require dated, reproducible measurements rather than an undated feature matrix.

The local storage decision is SQLite WAL with FULL synchronization and one writer owner. PostgreSQL supports later hosted services with multiple active instances. See the [storage decision](../docs/decisions/0001-storage-architecture.md) for alternatives and benchmark criteria.

## Hook configuration and trust review

`cyber hooks list --format json` shows resolved hook definitions, file origins, scope, SHA-256 digests and individual trust state, with credentials redacted. Checkout definitions remain withheld until configuration approval with `cyber trust inspect|approve`; the list reports those withheld paths. After reviewing a resolved project/local handler, use `cyber hooks trust --digest <digest>`. A changed handler needs its new digest approved. `cyber hooks untrust --digest <digest>` also revokes obsolete digests when current configuration cannot be parsed.

GET `/api/v1/hooks` inspects the current Location’s loaded definitions and uncached trust state without running handlers. The generated SDK exposes `client.hook.list()` and preserves the Location envelope. Rows retain original digests and file origins, report mandatory sandboxing, and redact headers and recognized secret fields. Withheld project paths and original top-level/profile hook sections are reported as redacted literal JSON. Environment/file placeholders remain unexpanded; invalid handler schemas remain inspectable. These inactive sections have no executable handler digest or approval controls. POST `/api/v1/hooks/trust` accepts `{ "digest": "…" }` only for a currently resolved project/local handler in a trusted checkout. POST `/api/v1/hooks/untrust` revokes an obsolete digest even if configuration is malformed, returning whether an approval existed. Both preserve the Location envelope; SDK clients expose `hook.trust` and `hook.untrust`. The TUI `/hooks` viewer shows loaded definitions and withheld paths. Use ↑/↓ to select, PgUp/PgDn to scroll, R to refresh, T to review approval and U to review revocation; Y confirms the displayed exact digest, N/Esc cancels. Global/managed/plugin/invocation rows do not offer checkout approval. Switching Session or Location dismisses the review. The CLI list and TUI viewer also display raw withheld sections. Loaded rows include a last-run summary for the same effective digest/event/scope and canonical checkout, using the receipt’s captured Location. Summaries include admission time, outcome, duration and stop acknowledgement; raw IO and decision content are omitted even for opted-in receipts. Running is an unverified observation and unknown requires recovery. CLI review opens only an existing database. `cyber hooks test <event> [--payload FILE]` exercises matching command, HTTP, prompt and local MCP-tool handlers with fresh synthetic identity and no Session startup.

`cyber hooks test PreToolUse --payload event.json --format json` accepts an object such as `{"tool_name":"edit","tool_input":{"path":"src/main.rs"}}`. Omit `--payload` to use empty event fields. Files must be regular JSON files at most 1 MiB; common envelope fields and the synthetic marker cannot be supplied. Tests print each selected handler and merged decisions, keep project configuration/handler trust gates, and retain real command process/checkout ownership through cancellation. Fixed-input events run with bounded concurrency and ordered merging; rewrite-dependent pre events currently chain sequentially. Unsupported transports/async handlers and incomplete termination exit nonzero with explicit outcomes. Synthetic receipts are separate from Session history and last-run summaries. They exercise real handler effects while leaving Session state unchanged; Remote MCP handlers, custom-adapter acknowledgement and full native acceptance remain open.

Synchronous HTTP handlers now POST the protected event JSON with configured headers and application/json content type, parse a bounded JSON object decision, and use the same durable Session/synthetic receipts and once claims. Project/local/sandbox-all scopes honor network off/on/allowlist configuration. Every request uses an explicit owned proxy; redirects, automatic retries, ambient proxies and transport-header overrides are refused. Non-2xx, malformed/oversized bodies and transport errors are nonblocking unless `fail_closed` is enabled. Timeouts and cancellation close and join local request/proxy sockets; this acknowledges local transport disposal and cannot prove remote POST rollback. Raw event/response IO is omitted by default and bounded when explicitly enabled; header credentials are never recorded as IO. Remote MCP handlers, async scheduling and complete native acceptance remain open.

On Unix, matching command handlers now run around built-in tools: `PreToolUse` before permission evaluation, followed by `PostToolUse` or `PostToolUseFailure` after settlement. A pre-hook denial blocks even in bypass mode; rewritten inputs are schema checked, and allow decisions retain permission denies and Mode ceilings. Project/local handlers require current checkout and individual digest approval. Errors and changes produce transient TUI/exec notices; durable execution receipts omit raw IO by default.

Matching command `PermissionRequest` hooks now receive the final tool input and the current permission ask immediately before a user prompt. `allow` approves that request once without saving a rule; `deny` blocks without a prompt. Input rewrites are ignored for this event. Requests already allowed or denied by hard policy never reach this boundary; `PermissionDenied` retries remain open.

Command handlers with `once: true` claim their effective digest once per Session through durable receipt admission. Restart, cancellation and unknown outcomes retain the claim; changed definitions need fresh trust and have a new digest. `status_message` and `system_message` appear as transient user notices while running and after acknowledged settlement, without entering model context.

Command handlers for `PermissionRequest`, `PostToolUse` and `PostToolUseFailure` now share a per-event pool bounded by `hooks.concurrency`. Finished handlers free their slots immediately; decisions still merge in declared order. A mandatory stop or fail-closed admission error cancels siblings and waits for launched owners to settle. Pre-tool handlers remain sequential so rewrites can drive later input and selectors; complete pre-tool concurrency remains open.

GET `/api/v1/sessions/{sessionID}/hook-executions` lists durable hook receipts with `limit` (1–500, default 50) and a Session-bound `cursor`. The SDK exposes `client.session.hooks(sessionID, { limit, cursor })`. Rows include captured call/tool identifiers when available, outcome, decision and stop acknowledgement. Raw IO is omitted by default; explicitly enabled, bounded IO remains visible to authenticated clients. A `running` receipt means admission was recorded, not that a process has been reconciled as live after restart; unresolved recovery remains open.

`cyber hooks history --session <id> [--limit 50] [--cursor <cursor>]` reads these committed receipts directly from the local database without starting a server, loading executable configuration or reconciling owners. `--format json` preserves the receipt fields and next cursor, including explicitly recorded IO; text output summarizes status and acknowledgement without raw IO. Running observations remain unverified as live, and unknown observations require recovery.

In the TUI, `/hooks history` pages the current Session’s receipts. Use `N` for the next page, `R` to refresh from the first page, arrows to scroll and Escape to close. The viewer shows recorded status, outcome, timing, call/tool correlation and stop acknowledgement. Raw IO is excluded from viewer state and display; running observations remain unverified as live. Definition review and trust controls remain under implementation.

On Windows, eligible global full-access command hooks now use PowerShell with an explicit parent-owned event-input route. The helper consumes its private launch permit before command input; required sandbox hooks still refuse until Windows confinement is integrated. Native acceptance of this route remains pending.

Other lifecycle events, remote tools and MCP hooks, async scheduling, pre-tool concurrency, context admission, required Windows hook confinement, complete execution-history controls and managed/plugin collection remain under implementation. Definition and history review commands execute no handlers.


Synchronous prompt handlers use `model_roles.evaluator` with `small_model` fallback. The trusted handler policy stays separate from protected event evidence; requests force tools disabled after overlays and require a complete strict allow/deny/ask judgement. Input and response are bounded to 1 MiB. Recorded calls check Session/ancestor budgets before inference, bill observed usage as hidden calls and check budgets again before proceeding. Calls without usage retain an unpriced marker. Synthetic CLI tests load pinned/cached/bundled model data only when trusted prompt handlers exist for the event; they can call the configured provider without creating a Session. The native OpenAI Responses, OpenAI-compatible and Anthropic adapters use an owned explicit proxy with ambient proxies, redirects and retries disabled. Raw SSE and error bodies are bounded before decoder buffering. Timeout closes and joins local provider sockets before applying nonblocking/fail-closed policy; cancellation retains execution through that acknowledgement and stops continuation. This cannot prove remote processing stopped or final usage was reported. Custom adapters without the owned HTTP capability and disposed callers retain unknown outcomes through the ownership fence. Raw IO follows the same opt-in bounded receipt policy.



Synchronous `mcp_tool` handlers now call configured local servers through a dedicated connection owned by the hook receipt. For example, `{"type":"mcp_tool","server":"audit","tool":"record"}` sends the event JSON to `record`; optional `arguments` support typed `${field}` substitutions. The tool must be present in filtered discovery and its arguments must match the discovered schema. Results must contain one unambiguous textual JSON decision; failed, missing or ambiguous decisions follow `fail_closed`. Hook and server authorization are rechecked before launch, before the call and after its result. Required hook sandboxing applies independently of `sandbox.apply_to` for ordinary MCP connections. One absolute handler deadline includes initialization, discovery and the call. Cancellation retains the native process/proxy owner through shutdown acknowledgement, durable receipt settlement and scratch cleanup. Dedicated connections allow permission hooks to call the same server while its shared connection is waiting on a callback. Hook connections do not borrow a Session's elicitation or sampling callback authority. Synthetic CLI tests create independent receipts and no Sessions. Remote transport, asynchronous handlers and full platform acceptance remain open.


MCP definitions in `mcp` now validate typed local `{type:"local",command,args?,env?,cwd?,enabled?,timeout?,tools?}` and remote `{type:"remote",url,headers?,oauth?,enabled?,timeout?,tools?}` settings after workspace trust and substitution. Connection timeout defaults to 30 seconds; `mcp.tool_timeout` defaults to 300 seconds. Tool deny globs take precedence over allow globs. Configured local servers now have owned concurrent startup, paginated discovery, shared tool registration and bounded reconnect. Remote transports, OAuth, resources/prompts and remote MCP hook transport remain incomplete.

MCP launch authorization now checks all contributing field origins and separate per-server approval of the effective definition, keyed by checkout. Workspace/hook approvals do not grant server approval, and a project override of global arguments remains project-controlled. Use `cyber mcp definitions --format json` to review loaded server definitions, their digests, field origins and approval state. Approve a currently loaded project server with `cyber mcp trust NAME --digest DIGEST`, after checkout configuration approval, and revoke an individual digest with `cyber mcp untrust --digest DIGEST`. Revocation works even when the current configuration is invalid. Environment/header values, OAuth settings and URL queries are redacted; approval binds the unredacted effective definition. These commands start no servers and create no Sessions. MCP confinement requires adding `mcp` to `sandbox.apply_to`; default scope excludes MCP, independently of project approval. Configured local servers start when their Location opens; tools remain subject to agent, Mode, permission and hook controls.


## Local MCP startup waiting

Configured local MCP servers start concurrently when a Location opens. The model can call `wait_for_mcp` with `{"servers":["shared"],"timeout":30}` to wait for named servers still connecting; timeout is in seconds, defaults to 60 and must be between 0 and 60. Zero reads current status immediately. The result includes each requested server's redacted status/error and a `timed_out` flag. Disabled/failed servers return without waiting; unknown names fail. Waiting does not start servers, invoke tools or approve discoveries, and cancellation leaves independent startup ownership intact.

The wait tool is read-only and allowed by default, subject to `wait_for_mcp` permission rules and ordinary hooks/agent controls. Connected tools are materialized on subsequent model steps. Local loss reconnects after verified settlement with bounded backoff; remote transports, resources/prompts and full native acceptance remain incomplete.


## Deferred tool schemas

When the effective catalog exceeds `tool_output.deferred_threshold_tokens` (default 10000 estimated tokens, four characters per token), the model receives MCP/plugin names and one-line descriptions without their parameter schemas. Builtin and Session/client tools retain full schemas. The threshold is shared; zero forces eligible tools to defer.

The model can call `tool_search` with `{"query":"jira issue"}` or `{"select":["mcp__github__create_issue"]}`. Search matches case-insensitive words in names/descriptions, returns full schemas sorted by name and loads them for subsequent steps in the same Session. `limit` defaults to five and accepts integers from zero to 1000. Exact selections reject unavailable names before loading anything; when both query and select are supplied, select takes precedence.

Selections survive restart and compaction, while new Sessions and forks start independently. Calling an unloaded tool returns `Tool <name> is deferred. Load it with tool_search first.` Search is allowed by default, subject to `tool_search` permission rules, agent controls, hooks and ordinary output budgets. Loading a schema does not approve execution: fresh permissions, server authorization and registration identity still apply. Search neither starts servers nor invokes their tools. Plugin registration delivery remains incomplete.


## MCP workspace roots

Configured local connections advertise MCP roots: the canonical Location and existing files or directories in `sandbox.writable_roots`, resolved relative to the Location and deduplicated. Read-only policy advertises the Location alone. Roots use percent-encoded `file://` URIs and describe a connection snapshot (`listChanged: false`); close and reopen the Location to capture configuration changes. An unavailable configured writable path refuses startup before native launch.

Servers can request `roots/list` during initialization, tool execution or idle processing. The list describes the workspace and leaves process sandbox grants and tool permissions unchanged. Private scratch, unrelated read-only paths and sibling worktrees are not added implicitly. Sampling remains unadvertised and unsupported until its model, permission and billing path is implemented.


## MCP server guidance

Connected local servers can supply usage instructions during initialization. Eligible instructions appear in a `<mcp_instructions>` context block, ordered by server name and attributed to each server. Server text is escaped to preserve block boundaries. Guidance remains available when schemas are deferred, while agent, Mode, permission and server-authorization controls determine eligibility. Higher client registrations that shadow every MCP tool also remove that server's guidance.

Instructions use the `mcp/instructions` Context Source. Reconnection changes, connection loss, Location close and visibility restrictions update its applicability through mid-conversation system messages. The Epoch baseline remains byte-stable. Reading this source uses cached discovery metadata and starts no servers or RPCs. Remote MCP instruction delivery remains incomplete.

### Required local MCP startup

Set `required: true` on a server entry to hold a Session's first Turn until the server connects and publishes discovery, up to its configured `timeout` (default 30 seconds). A disabled, failed or unavailable required server reports `McpRequiredError: <name>` before prompt promotion or a model call; the original prompt remains pending for retry. Cancelling the wait preserves independently owned Location startup. Retry after recovery by waking the Session; unresolved native ownership remains fenced.

The requirement applies once per Session and survives replay and conversation rewind. A fork begins its own first Turn. The default is false and preserves existing server approval digests; enabling it changes the definition and requires matching authorization. Remote transports, dynamic header commands, per-server output caps and resolved-option CLI display remain incomplete.

### MCP output budgets and resolved options

Set a positive `output_token_limit` on an MCP server to cap its model-facing text using the four-Unicode-character token estimate. The global `tool_output.max_lines` and `max_bytes` ceilings still apply; an omitted cap inherits that global budget. The standard truncation notice follows the bounded payload and points to the managed file containing the full original text. Structured data is retained. This is an estimate rather than provider tokenizer accounting. Local success, error and structured results share this settlement path.

Run `cyber mcp get <name>` to review one loaded server's definition, provenance, approval digest, authorization observations and output budget without connecting or creating a database. Header/environment values, OAuth details and URL query values are redacted; numeric output limits remain visible. Unknown or withheld names fail review. Dynamic header commands/refresh and remote MCP delivery remain incomplete.

### MCP call timeouts and progress

An explicit server `timeout` applies to both connection startup and tool-call inactivity. When omitted, connection startup defaults to 30 seconds and calls use `mcp.tool_timeout` (default 300 seconds). `cyber mcp get <name>` reports the resolved `call_timeout_seconds`. Definitions with an explicit timeout require fresh matching approval under these corrected call semantics; definitions with omitted defaults preserve existing approval digests.

Valid progress for the active request resets inactivity. Values must advance, including fractional values; duplicate/regressive progress, foreign tokens, malformed optional fields and messages carrying a request identity do not extend the timer. A timeout leaves remote completion unverified until native ownership settles. Calls are not replayed automatically.

### Graceful local MCP close

Explicit Location close and runtime shutdown withdraw MCP tools before stopping shared servers. On POSIX, client IO closes and the owned group receives SIGTERM. The leader remains unreaped for the full five-second grace period so descendants can finish cleanup after the leader exits. SIGKILL then stops remaining group members, followed by native acknowledgement, local proxy shutdown and durable settlement before scratch/checkout cleanup. Windows uses owned Job Object termination.

Independent servers close concurrently. An interrupted close retains its original grace deadline and Location admission fence; retry resumes that deadline. Unknown ownership or failed settlement remains fenced for recovery. Failed startup, cancelled calls, lost connections and owner disposal retain immediate cleanup, with disposal providing no durable acknowledgement. Remote shutdown and full native platform acceptance remain incomplete.

### Local MCP form elicitation

Configured local stdio servers advertise form elicitation. During an interactive tool call, `elicitation/create` opens the existing question flow, names the requesting server, and asks for explicit Accept, Decline or Cancel before returning values. Flat strings, numbers, integers, booleans and string-enum arrays retain their JSON types; supported constraints and formats are checked before sending. Optional fields can be omitted and declared defaults require an explicit choice. Decline, Cancel and invalid answers send no form values.

Authorization is checked before asking and again before sending answers. Startup, idle and unattended requests decline without creating a Session question; logs omit form text and answers. Human question time pauses call inactivity, while interrupt or Location close stops the native call and dismisses its own pending questions. URL elicitation is unadvertised and refused. Remote transports and full native acceptance remain pending.

### Local MCP sampling

Set `mcp.sampling.enabled` to `true` to advertise basic sampling on newly connected local stdio servers. It defaults off. Close and reopen the Location after enabling it; disabling it also refuses requests on existing connections. Requests during startup or idle have no active call owner and are refused. Sampling advertises no tools or ambient-context support and accepts bounded text/image messages; audio and tool-enabled requests are refused.

Each active-call request checks `mcp_sampling` permission for the server name, selects `model_roles.small`, and checks Session and ancestor budgets after approval. Model hints cannot select another provider. Sampling receives only the server's input, with tools disabled and no Session history or cache identity. Usage records use `mcp_sampling:<server>` and the originating call ID. Soft budget overrun records usage and withholds the response; it is not a token reservation.

Human approval and nested sampling pause outer MCP call inactivity; the provider call has its own configured call timeout. Cancellation retains the nested task until local provider transport shutdown and observed billing settle. Native OpenAI-compatible, OpenAI Responses and Anthropic socket tests cover timeout and Location close. Local socket closure does not prove remote rollback or final usage; unsupported custom transport cancellation remains unverified. Sampling input and credentials are excluded from Session events. Remote MCP sampling and complete native platform acceptance remain open.

## Memory management (partial P1)

```bash
cyber memory path                 # this project's directory; global outside Git
cyber memory list --global        # metadata and invalid-file diagnostics
cyber memory show coding-policy
EDITOR='code --wait' cyber memory edit coding-policy
cyber memory delete coding-policy
cyber debug memory --global
```

`edit` opens a private draft using EDITOR, or VISUAL when EDITOR is unset. It validates the note and updates its index through the memory journal after the editor exits successfully. Changes to the original note or index during editing refuse the commit. Drafts remain in private `<data>/.memory-edit-*/note.md` directories, including after success; errors identify the retained draft. An unchanged new template creates no note. The draft must remain a private regular file without symlink or hard-link aliases. List/show/path/edit/delete open no database and start no model work. Recovery starts no model work and only checks an existing database read-only.

Disabled or read-only settings refuse mutations; list/show/path/debug remain explicit manual review. Unix mutation support is implemented locally. Windows privacy/durability, multi-client retention and complete native acceptance remain open. API mutations and durable event delivery are implemented locally. Sessions receive committed index changes at their next Safe Boundary.

Authenticated clients can list metadata with `GET /api/v1/memory?scope=project|global` (default project) and read a note with `GET /api/v1/memory/{scope}/{name}`. Responses carry the standard Location envelope. Listing returns `memories` and safe `invalid` diagnostics; reading returns `metadata` and `body`. The generated TypeScript client exposes `client.memory.list({scope})` and `client.memory.get(scope, name)`. These explicit user reads remain available when automatic memory is disabled. Missing notes return MemoryNotFoundError; busy, pending or unsafe storage refuses without automatic recovery. On Unix, PUT /api/v1/memory/{scope}/{name} accepts a strict JSON object containing `content` (the complete Markdown document), and DELETE accepts no body. Both return a durable MemoryChange and publish memory.updated.1. The SDK exposes memory.put(scope, name, {content}, options) and memory.delete(scope, name, options). Idempotency-Key protects completed replay and refuses conflicting or unresolved requests; pre-admission contention returns retryable 503. Disabled/read-only settings refuse fresh mutations. Windows mutations and complete legacy/client recovery controls remain open.

Before an API edit, call `GET /api/v1/memory/edit/{scope}/{name}` (SDK `client.memory.editReview(scope, name)`). Its Location envelope contains `name`, `original` Markdown (null for an absent note in an existing scope), and `fingerprint`. Missing scopes return 404 without creation. PUT may include `review_fingerprint`; DELETE may send `X-Cyber-Memory-Review`. These conditional mutations check the scope, target/index content and file identities, and valid catalog contents before durable admission and again before journal preparation. Changed files return 409 and preserve the user's edits. Fresh disabled/read-only mutations remain forbidden. An identical acknowledged request with its retained key replays even after its review is consumed; a changed review under that key conflicts. Reviewed TUI editing is implemented locally; private Unix client checkpoints are implemented; multi-client retention and Windows mutations remain open.

Memory writes through the running model-tool host now publish `memory.updated.1` with a durable independent aggregate and a public `MemoryChange` receipt. Notifications contain the note name and mutation identity rather than the note body. An unresolved mutation refuses further writes of that scope until reviewed recovery; ordinary storage reads also refuse while a completed journal awaits durable acknowledgement. CLI writes retain their existing local storage behavior. Reviewed database/file reconciliation controls and complete native acceptance remain required.

The shared core now exposes read-only `MemoryScope::inspect_recovery()` and fingerprint-bound `recover_reviewed()` / `recover_reviewed_with_acknowledgement()`. Inspection returns the validated proposed note, original mutation identity and completion status. The fingerprint binds the scope/journal identities, known journal artifacts, current target/index and catalog. Recovery compares that state before normalization or installation; changed content or replaced target/staging inodes require a new review. Unsafe aliases and corrupt evidence refuse without cleanup. Failed acknowledgement preserves the completed journal and requires a new completed-state review. The CLI exposes these interfaces through `memory recovery` and `memory recover --review`; HTTP pinned-journal reconciliation is available; CLI server reconciliation is available; TUI paired reconciliation is available; legacy resolution remains open. A fingerprint grants no authority to clear a pending database admission.

Run `cyber memory recovery [--global]` to inspect an interrupted local transaction, including proposed note content, completion status and its review fingerprint. With `--format json`, absent evidence is `null`; inspection creates no memory scope or database. After reviewing the proposed content, run `cyber memory recover --review <fingerprint> [--global]`. Recovery requires enabled/writable settings, retained scope ownership and unchanged evidence. It checks an existing database read-only without following a database symlink; a pending admission, corrupt/unknown schema, unreadable database or `CYBER_DB=:memory:` refuses before file effects. A database predating memory admissions is supported. File-only recovery never clears a database admission; unresolved durable writes still require database/file reconciliation controls.

New model-tool and HTTP memory mutations durably bind the prepared journal before installing files. The binding records its exact mutation receipt and complete intent digest (before/after note/index hashes and catalog), then rotates the live owner nonce atomically. Persisted binding evidence contains the superseded nonce and the new nonce hash, so it cannot authorize completion after owner loss. Recovery inspection exposes `journal`, whose immutable `intent_fingerprint` stays stable across partial installation, separately from the fingerprint of current reviewed files. New completion requires the exact bound receipt; historical acknowledged writes remain replayable. This correlation does not clear pending admission or grant recovery authority; database/file reconciliation remains required.

The shared runtime now provides `review_memory_recovery` and `recover_memory_write` for pinned admissions. The database review binds the request, result and aggregate sequence in one snapshot; reconciliation compares that review and current storage before effects and durably grants a fresh recovery nonce. Disposal retains unknown fencing and requires another review. A matching acknowledged journal finishes archival with its original receipt and no duplicate notification. These are host-controlled interfaces: user-facing legacy database reconciliation is still pending, and hosts must retain the configured scope claim, lifecycle lease and fresh mutation settings. Legacy unbound admissions remain refused rather than inferring original intent or ownership.

Authenticated `GET /api/v1/memory/recovery/{project|global}` returns the Location envelope with null for absent evidence or `{storage, admission}` for a matching pinned journal. Inspect proposed content, transaction identity, immutable intent digest and completion state before confirming. `POST` to the same route accepts only `{storage_fingerprint, admission_fingerprint}` from that review. Recovery repeats writable/enabled settings and retains runtime shutdown and scope ownership through acknowledgement and archival. Missing, malformed, stale, foreign or unbound evidence refuses; manual inspection remains available when generation is disabled. The SDK exposes `client.memory.recovery(scope)` and `client.memory.recover(scope, fingerprints, options)`. With a retained Idempotency-Key, completed retries return the durable original receipt even after response-cache loss. Unkeyed retries retain stale-review refusal. Local-only/legacy HTTP recovery, TUI editing and Windows writes remain open.

Retain a unique key before confirming recovery and pass it as the SDK `idempotencyKey` option. `GET /api/v1/memory/recovery/requests?scope=project|global&key=<retained-key>` (SDK `client.memory.recoveryRequest({scope, key})`) returns null for an absent identity or its `mutation_id`, pinned journal and nullable `completed` change. Null completion means unresolved evidence, not permission to restart an attempt. Lookup checks the original canonical Location/project. Request-key and digest correlation is projected atomically with durable recovery ownership; different body/endpoint/Location reuse refuses after cache loss. A later explicit review resolving that same mutation makes its receipt available to earlier retained attempt keys. Already acknowledged journal archival can link a fresh request without another completion or notification.

For a retained save/delete key, use authenticated `GET /api/v1/memory/requests?scope=project|global&key=<retained-key>` (SDK `client.memory.requestStatus({scope, key})`) at the original Location. It reads the durable admission independently of the response cache and enabled/writable memory settings. Evidence includes the note name, deletion flag, original HTTP `request_fingerprint`, nullable pinned journal and nullable completed change. Compare the fingerprint against the exact retained request before reconciling client state. Null evidence means no matching admission was found; null completion means unresolved admission. Neither authorizes replay. Foreign Location/scope refuses, inconsistent ledger evidence refuses, and lookup creates no memory files or notifications. The TUI uses the same fingerprint and key identity checks before reconciling retained records.

For paired database/file reconciliation, start or use the registered TCP server and run `cyber memory recovery --server [--global]`. JSON output preserves the Location envelope and paired `data.storage` / `data.admission` reviews; text output prints the same review and proposed content. After inspecting both fingerprints, confirm with `cyber memory recover --server --review <storage-fingerprint> --admission-review <database-fingerprint> --key <retained-key> [--global]`. The CLI sends one authenticated request with that exact key and repeats no request automatically. After an uncertain response, inspect `cyber memory recovery-request --server --key <retained-key> [--global]`; unresolved evidence requires a fresh review. Completed identical retries return the original durable receipt. These commands require existing registration/credentials and never start a service, create a database or run a model locally; a socket-only server currently requires a TCP listener for this client. Default local recovery continues to refuse unresolved database admissions.

Open `/memory` in the TUI for project notes, or `/memory global` for global notes. Use Up/Down to select metadata, Enter to read the note, G to switch scope and R to refresh. D requests deletion of an opened note; Y confirms and N/Esc cancels. V inspects pinned recovery even when ordinary reads are fenced. Review the proposed content, completion state and both fingerprints; C opens confirmation and Y submits it once. K inspects the latest retained request in the current Session/Location/scope without repeating effects. U opens all retained requests; Up/Down selects a record, Enter/K looks up its outcome in its original context, and U returns. F opens explicit forgetting confirmation; Y removes only that record, N/Esc cancels. Forgetting does not resolve unknown server effects and retains draft text. The panel displays the key; copy it for CLI/SDK receipt lookup after exiting or changing Session/Location. Failed requests clear actionable reviews, and late responses cannot replace a newer scope/Location. PgUp/PgDn scroll content. E edits an opened note or resumes the retained draft. Type and paste multiline Markdown; Ctrl-S reviews the save and Y confirms. Ctrl-R obtains a fresh original/fingerprint while retaining draft text. A rejected save retains the draft and requires a fresh review before another confirmation. Esc first pauses editing and retains the draft; a second Esc closes the panel. Session/Location/scope changes retain the draft without granting foreign save authority. X/Y explicitly discards it. On Unix, drafts and confirmed save/delete/recovery intents persist privately under `<state>/memory-client/state.json`. The runner checkpoints edits and confirms durable intent before dispatch. Restart restores text and keys without review fingerprints or confirmations; E resumes in the original Session/Location/scope, then Ctrl-R obtains a fresh review. Stored save/delete/recovery keys support read-only lookup in their original context. Completed save/delete status must match the exact retained HTTP fingerprint, key, scope and journal/receipt before its record retires. A matching acknowledged save clears unchanged draft text; newer local edits stay and require fresh review. No retained request is sent automatically. Checkpoint errors withhold fresh memory effects and orderly exit while unsaved work remains; repair storage and retry, or explicitly export and discard the draft. Malformed or concurrently owned state requires a fresh TUI start after manual repair/ownership release. Acknowledged saves/deletes remove their pending intents, and acknowledged recovery keys remain available. Pending records remain bounded. Unknown or unresolved lookup preserves them; no automatic retry is sent. Multi-client retention and complete native acceptance remain open. Unsupported native privacy refuses memory effects without creating client state. Complete Windows/native acceptance remains open.

## Code intelligence status (partial P1)

`cyber lsp status` lists local server enablement and installation, then queries an existing registered server for this Location. It starts no service, model or language server and creates no database. `--format json` returns rows with `id`, nullable `enabled`, `installed`, `running` and `roots`. Each observed root carries `id`, `root` and `status: starting|connected|broken`.

A connected root makes `running` true. An authenticated empty root set makes it false. Without registration, `running` and `roots` are null (human output: unknown). Starting/broken entries without a connected root also keep running unknown; those protocol states do not prove process termination. Retained server IDs absent from current discovery remain visible with null enabled/installed metadata. An invalid, refused or unreachable registered service fails explicitly instead of reporting stopped servers. Requests use existing credentials, bypass ambient proxies, refuse redirects and enforce a five-second deadline and one-MiB response limit.

`cyber fmt status` reports detected formatter definitions and project markers without executing them. Formatter execution after edits and LSP navigation/error feedback are implemented pieces of P1; complete platform and lifecycle acceptance remains open. See [feature limits](features.md) and [P1 evidence](implementation/p1-status.md).

## Migration detection

Run `cyber import --detect [--format json]` to inventory known Claude, Codex and OpenCode files for the current checkout and home. `--cwd` chooses the Location; `CODEX_HOME` adds an explicit Codex home. This command bypasses config/bootstrap/log creation, does not open a database and never executes source commands or plugins. Existing home and Location directories are required.

The report counts raw agent/command/skill files and inline JSON/JSONC/TOML definitions, MCP servers and hook handlers. Counts include repeated definitions in separate layers, not an effective merged configuration. Parse/read failures are static issues without configuration values. Session counts and effective read-time coverage are `null` in JSON (`unknown` in text); `complete` and `counts_complete` remain false. Referenced sources, complete source/session detection, per-item compatibility, the first-run TUI offer, complete conversion/review/write flows and editor integrations remain open.

`cyber import <claude|codex|opencode|auto> --dry-run [--scope project|global] [--format json]` now previews supported configuration, permission, provider/model and static MCP mappings. It preserves existing native keys and fills missing keys in canonical auto order. Project previews include source home defaults; global previews exclude project sources. No configuration, database, log or directory is created. Literal credentials become required environment references. Sources and existing targets are reverified before output; changes invalidate review.

Diffs currently normalize JSON and redact known raw/escaped secret values; they do not preserve original bytes/comments. Reports list imported/merged supported leaves and their contributing source files/JSON pointers, including inherited provider selection, per-selector permission sources and environment requirements. Unsupported fields retain indexed descriptors and contributing layer references; constant-rule fields are labeled as converted pointers. No credential values are included. Basic explicit Codex approval/sandbox pairs propose native mode and sandbox policy; retired `untrusted` emits a deprecation note. Granular/on-failure/unknown approvals and conflicting named permissions refuse the proposal pending their adapter. Workspace/environment/network/profile policy mappings remain open. `complete` remains false. Explicit native global, ancestor and `.cyber` file layers now use the runtime loader’s path order and merge rules. JSON `native_layers` lists the files influencing the proposal. All existing/missing native candidates are held under review. Native and source bytes share the sixteen MiB parse budget. Profiles, substitutions, transient flags and trust filtering are not evaluated by this raw file-layer preview. Complete adapters, runtime-effective configuration handling, per-leaf provenance, exact diffs, reviewed writes and idempotent annotations remain required.


### MCP migration previews

Read-only imports accept static Claude `mcpServers` (including `.mcp.json`), Codex `mcp_servers`, and OpenCode flat or `mcp.servers` declarations. Local commands/arguments and HTTP endpoints become native server proposals. Literal environment/header values become required environment references; JSON setup records identify the contributing source fields and never contain the original values. No source server starts during an import. Native `disabled: true` has the same server state and approval identity as `enabled: false`; conflicting flags refuse.

Unsupported server options appear as pending fields. Advanced authentication, additional transports, timeout conversion, source-relative working-directory semantics and Claude per-project state associations still need adapters. Global Claude state contributes only top-level server declarations. These previews remain incomplete and do not provide confirmed writes.

# Local core guide

Detailed commands, client behavior, limits and terminology for the implemented local core and partial P1 features. Start with the [README](../README.md); the [P1 status](implementation/p1-status.md) records acceptance evidence and remaining work.

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

On Unix, matching command handlers now run around built-in tools: `PreToolUse` before permission evaluation, followed by `PostToolUse` or `PostToolUseFailure` after settlement. A pre-hook denial blocks even in bypass mode; rewritten inputs are schema checked, and allow decisions retain permission denies and Mode ceilings. Project/local handlers require current checkout and individual digest approval. Errors and changes produce transient TUI/exec notices; durable execution receipts omit raw IO by default.

Matching command `PermissionRequest` hooks now receive the final tool input and the current permission ask immediately before a user prompt. `allow` approves that request once without saving a rule; `deny` blocks without a prompt. Input rewrites are ignored for this event. Requests already allowed or denied by hard policy never reach this boundary; `PermissionDenied` retries remain open.

Command handlers with `once: true` claim their effective digest once per Session through durable receipt admission. Restart, cancellation and unknown outcomes retain the claim; changed definitions need fresh trust and have a new digest. `status_message` and `system_message` appear as transient user notices while running and after acknowledged settlement, without entering model context.

Command handlers for `PermissionRequest`, `PostToolUse` and `PostToolUseFailure` now share a per-event pool bounded by `hooks.concurrency`. Finished handlers free their slots immediately; decisions still merge in declared order. A mandatory stop or fail-closed admission error cancels siblings and waits for launched owners to settle. Pre-tool handlers remain sequential so rewrites can drive later input and selectors; complete pre-tool concurrency remains open.

Other lifecycle events, remote tools, HTTP/prompt/MCP handlers, async scheduling, pre-tool concurrency, context admission, Windows command launch, `cyber hooks test`, the TUI viewer, public execution history and managed/plugin collection remain under implementation. These review commands execute no handlers.

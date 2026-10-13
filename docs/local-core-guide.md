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

A leading TUI mention such as `@explore where is retry logic?` starts a fresh background child using that profile. The `@` menu includes visible subagent-capable agents beside files and quotes names containing spaces. Embedded/file mentions remain ordinary prompt text; use `@./path` for a file whose name matches an agent. Results return through the parent handback and `/tasks`. Before a Job is assigned, `/admissions` lists saved request IDs; Enter looks up a request, Ctrl+R reconciles an idle unknown receipt and Ctrl+S cancels it. Esc cancels the current Session's pending admission before interrupting its conversation. IDs are saved before submission and looked up on reconnect without resubmitting prompts. Pending requests survive TUI exit; use `/admissions` to inspect or cancel them after reconnecting. API/SDK callers can pass an optional `agent` to the Session subtask endpoint; omitting it preserves the forked current-profile behavior. Use Enter to start a mentioned child; queued/held delegation and generic prompt admission are still being implemented.

`cyber exec '@explore find retry logic' --file notes.txt --max-turns 4` starts a fresh background child using a visible subagent-capable profile. Exec prints the child's complete durable text and usage and waits for its Job to settle, including cleanup. JSON results retain a client-generated `delegation_id` and parent Session before submission, then identify the child Session and Job after admission. Timeout and interruption cover request submission, queued admission and task following; they cancel only that request or its recorded Job. Local token/cost checks include the child’s own and nested descendant usage and request stop of the owned Job at the recorded limit. JSON reports combined `cost_usd` and `total_tokens` alongside `own_cost_usd`, own token classes and the four descendant billing fields. Budgeted delegation requires complete descendant billing from the attached server. Client polling can overshoot while calls remain in flight; server-side subtree scheduling and cancellation remain open. Unacknowledged cancellation is reported explicitly. Attachments retain their Content type, and the requested step limit is capped by the profile. Attached servers must advertise the durable admission routes and delegation fields. Session setup, catalogue discovery, file reads and capability negotiation precede the delegation timeout. Parent handback remains enabled; queued/held mention delivery and durable server-side subtree budget enforcement remain open.

`cyber review [target]` reviews uncommitted changes (the default), `<base>..<head>`, one commit, or `pr <number>`. A PR review needs `gh`, and its commits must already be local. The target is checked before any server contact. Put `--` before a target that starts with `-`. The server then resolves it to commits, withholds paths the reviewer could not read without asking, and runs the bundled `review` skill in the read-only `reviewer` subagent. Like `cyber exec --command review`, the command waits for that Job and its handback, and on `--timeout`, a budget stop or SIGINT it cancels only that Job. Text output lists the findings most severe first, followed by the limitations, or prints `No findings.`. A clean tree prints `No changes to review` and creates no Job. With `--format json`, the output gives `target`, `target_kind`, `findings` (re-validated and sorted by severity, file, line and summary), `limitations`, `counts`, the handback `result`, the reviewer `session_id`/`job_id`, the `delegation_id` and usage. Exit codes:
- 0: the review completed, or there was nothing to review.
- 1: the reviewer failed, could not start (for example an `agent` rule denied `reviewer`), or returned an invalid result (`SchemaMismatch`). JSON reports `reviewed: false`.
- 1, or 5 with `--fail-on-deny`: a permission rule refused the review before anything ran (for example a `bash` deny of `gh *` for `pr <number>`). JSON still prints the review object with `findings: null` and the denial in `error`.
- 2: the target is invalid or cannot be resolved; JSON names its `code`.
- Timeout, budget and denial stops use the exec exit codes.

A project or user `review` command or skill replaces the bundled one. It runs as written, and JSON reports `findings: null`. `/review` in the TUI uses the same server path. The model cannot invoke the bundled `review` skill itself, because only `/review` and `cyber review` supply the resolved diff.

Target grammar: nothing (staged, unstaged and untracked changes against `HEAD`, or the empty tree when `HEAD` is unborn), `<base>..<head>` (both sides required; `...` and open-ended ranges are rejected), one revision (that commit against its first parent), or `pr <number>`. Untracked files hidden by your global excludes file (`core.excludesFile` in `~/.gitconfig` or `~/.config/git/config`, else `~/.config/git/ignore`) are skipped, as in `git status`; a repository's own `core.excludesFile` replaces the global one. A global excludes file over 64 KiB refuses the review (it is not truncated, which would unhide ignored files). A revision may contain only `A-Z a-z 0-9 . _ / ~ ^ -`, cannot start with `-` or `/`, and cannot contain whitespace, `:`, `@{` or `..`. Revisions are resolved to commit IDs by a library engine in the sandboxed inspection helper, which never runs git hooks, external diff, textconv, filters or a pager, and never passes your text as a process argument. Permissions are checked before anything is admitted: an `external_directory` deny for a repository outside the Location refuses the review (403), and each changed path whose `read` decision (or `external_directory` decision outside the Location) is not allow is withheld from the reviewer and named in `limitations`. An `ask` withholds like a deny, so under the default rules modified `.env` and `.env.*` files (other than `.env.example`) are never sent; no prompt is raised. These are the reviewer's own decisions under its read-only ceiling, so running `/review` from a `bypass` Session does not send more. Withheld files do not count against the patch limit. If every changed file is withheld, or withholding leaves no patch text at all, the review is refused with `all_withheld` (`cyber review` exits 2) rather than reported as a clean tree. `PreToolUse` hooks do not run for this host step, so protect secrets with `read` rules. `pr <number>` runs the permission-checked `gh pr view <number> --json baseRefOid,headRefOid,url` in the command sandbox and reviews the PR's own changes: from the merge base of the base and head commits to the head, as GitHub's PR diff does, so later base-branch commits are not shown reversed. A plain `<base>..<head>` target diffs the two commits directly, like `git diff A..B`. It never fetches, checks out or comments, so fetch the base branch and the PR commits first. `gh` is looked up on your `PATH` outside the project (relative `PATH` entries are ignored), runs with that same filtered `PATH` so a project cannot plant the `git` it calls, and `GH_TOKEN`/`GITHUB_TOKEN` reach it only when `sandbox.env_allow` lists them. The patch sent to the reviewer is capped at 256 KiB; truncated and binary files are listed in `limitations`. Resolution failures are reported before admission as HTTP 422 `ReviewTargetError` with a `code`: `invalid_target`, `invalid_ref`, `unresolved_ref`, `not_a_repository`, `empty_diff`, `all_withheld`, `gh_unavailable`, `pr_unresolved` or `pr_commits_missing`. The reviewer must return `{findings: [{file, line, severity, summary, failure_scenario}], limitations: []}` (`severity` is `critical`, `high`, `medium` or `low`; `file` is repository-relative with no leading `/`, drive or `..` segment; `summary` and `failure_scenario` are not blank). The schema enforces exactly what the CLI and TUI re-check, so an invalid result gets one corrective retry and then fails with `SchemaMismatch`. Retrying POST `/command` with the same message ID, name, arguments and delivery returns the original receipt without resolving the target again.

In the TUI, `/review [target]` checks the target locally when the bundled skill is active and shows an invalid target in the status line without contacting the server. An overriding `review` receives its arguments unchanged. A refused review (`No changes to review`, any other `ReviewTargetError`, a permission denial or any other failed post) shows the server message in the status line and starts no task. The TUI chooses the command's message ID, so it knows the reviewer's delegation and treats only that delegation's Job as the review; other Jobs that end meanwhile are not reported as it. When the reviewer Job completes, the findings view opens with severity counts and the re-validated findings, most severe first, with control characters escaped; ↑/↓ and PageUp/PageDown scroll and any other key closes it. A failed reviewer, an invalid result or a reviewer that could not start shows the error instead.


For a lost launching admission receipt, API callers can use `POST /api/v1/sessions/<session>/delegations/<request>/reconcile`; SDK callers can use `session.reconcileDelegation(session, request)`. The server links exactly one existing Job only when its captured operation and Session authority match. Repeating reconciliation does not launch work. Missing or ambiguous proof remains `unknown`; a reserved admission can still be stopped explicitly. Recovering the receipt does not resolve an uncertain native Job or its cancellation. In `/admissions`, Ctrl+R contacts this endpoint for the selected idle unknown receipt using its saved Session and Location. A lost response is looked up, never resubmitted as a task; unresolved records remain saved for reconnect. CLI users can run `cyber api v1.session.reconcileDelegation --param sessionID=ses_... --param requestID=op_...`. Full native/lifecycle acceptance and native-effect recovery remain open.


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

`SubagentStart`/`SubagentStop` now run around every child launch (model `agent` calls, forked skills, `@agent`, `cyber review` and `/review`): a `SubagentStart` deny refuses the launch before any child Session, Job or worktree, even in bypass, and `SubagentStop` observes the handback. `Stop` hooks can `block` to continue a Drain with their reason, up to `hooks.max_stop_continuations`, and `StopFailure` observes failed Turns and Drains. `UserPromptSubmit` can deny a user prompt or delegation before admission or add `additional_context` immediately before it; `SessionStart` runs once per root Session and may add context; `JobEnded` observes terminal background Jobs. These cases are verified locally on Unix; native Windows acceptance remains pending.

Other lifecycle events, remote tools and MCP hooks, async scheduling, pre-tool concurrency, context admission for tool events, `continue: false`, required Windows hook confinement, complete execution-history controls and managed/plugin collection remain under implementation. Definition and history review commands execute no handlers.


Synchronous prompt handlers use `model_roles.evaluator` with `small_model` fallback. The trusted handler policy stays separate from protected event evidence; requests force tools disabled after overlays and require a complete strict allow/deny/ask judgement. Input and response are bounded to 1 MiB. Recorded calls check Session/ancestor budgets before inference, bill observed usage as hidden calls and check budgets again before proceeding. Calls without usage retain an unpriced marker. Synthetic CLI tests load pinned/cached/bundled model data only when trusted prompt handlers exist for the event; they can call the configured provider without creating a Session. The native OpenAI Responses, OpenAI-compatible and Anthropic adapters use an owned explicit proxy with ambient proxies, redirects and retries disabled. Raw SSE and error bodies are bounded before decoder buffering. Timeout closes and joins local provider sockets before applying nonblocking/fail-closed policy; cancellation retains execution through that acknowledgement and stops continuation. This cannot prove remote processing stopped or final usage was reported. Custom adapters without the owned HTTP capability and disposed callers retain unknown outcomes through the ownership fence. Raw IO follows the same opt-in bounded receipt policy.



Synchronous `mcp_tool` handlers now call configured local servers through a dedicated connection owned by the hook receipt. For example, `{"type":"mcp_tool","server":"audit","tool":"record"}` sends the event JSON to `record`; optional `arguments` support typed `${field}` substitutions. The tool must be present in filtered discovery and its arguments must match the discovered schema. Results must contain one unambiguous textual JSON decision; failed, missing or ambiguous decisions follow `fail_closed`. Hook and server authorization are rechecked before launch, before the call and after its result. Required hook sandboxing applies independently of `sandbox.apply_to` for ordinary MCP connections. One absolute handler deadline includes initialization, discovery and the call. Cancellation retains the native process/proxy owner through shutdown acknowledgement, durable receipt settlement and scratch cleanup. Dedicated connections allow permission hooks to call the same server while its shared connection is waiting on a callback. Hook connections do not borrow a Session's elicitation or sampling callback authority. Synthetic CLI tests create independent receipts and no Sessions. Remote transport, asynchronous handlers and full platform acceptance remain open.


MCP definitions in `mcp` now validate typed local `{type:"local",command,args?,env?,cwd?,enabled?,timeout?,tools?}` and remote `{type:"remote",url,headers?,headers_command?,headers_refresh_seconds?,oauth?,enabled?,timeout?,tools?,required?,output_token_limit?}` settings after workspace trust and substitution. Connection timeout defaults to 30 seconds; `mcp.tool_timeout` defaults to 300 seconds. Tool deny globs take precedence over allow globs. Configured local and remote servers have owned concurrent startup, paginated discovery, shared tool registration and bounded reconnect. Remote OAuth sign-in is described under [Remote MCP OAuth](#remote-mcp-oauth); resources/prompts and remote MCP hook transport remain incomplete.

#### Remote servers

A remote entry connects over Streamable HTTP first and falls back to the legacy HTTP+SSE transport only when that first initialize fails for a non-authentication reason (HTTP 400/404/405, a non-MCP content type or a transport error), inside the same connect `timeout`; reconnects try the transport that last worked. HTTP 401 or 403 never falls back or retries (apart from the single OAuth token refresh described below): the server settles as `needs_auth`, whether the refusal comes at connect or on an established connection, and its tools are withdrawn. `headers` values may use `{env:NAME}` substitution; the substituted values are part of the approval digest. Each request carries the configured headers plus `Mcp-Session-Id` and `MCP-Protocol-Version`. The optional GET event stream delivers server notifications such as `tools/list_changed`; when it ends cleanly it is reopened on the same session with `Last-Event-ID`, and a tool call whose response stream closes early is resumed the same way. Without a stream the client pings the server every 10 seconds. A failed reopen, transport error, unanswered ping or expired session (404) is loss: tools are withdrawn and reconnection uses the shared 1 to 60 second backoff with at most ten attempts. Closing a Location sends `DELETE` for the session (405 is accepted).

Remote servers are not processes, so `sandbox.apply_to` governs only their network: when it selects MCP with a policy other than full access, network `off` refuses before contact and network `proxy` refuses a URL host outside `allowed_domains`. Every remote request, restricted or not, leaves through an owned loopback proxy that ignores ambient proxy variables, follows no redirects and never retries. Project-defined remote servers need the same workspace trust and individual `cyber mcp trust <name> --digest` approval as local ones before any request is sent, and approval is rechecked immediately before contact. Diagnostics, logs, status and durable records never contain the URL query or header values.

MCP launch authorization now checks all contributing field origins and separate per-server approval of the effective definition, keyed by checkout. Workspace/hook approvals do not grant server approval, and a project override of global arguments remains project-controlled. Use `cyber mcp definitions --format json` to review loaded server definitions, their digests, field origins and approval state. Approve a currently loaded project server with `cyber mcp trust NAME --digest DIGEST`, after checkout configuration approval, and revoke an individual digest with `cyber mcp untrust --digest DIGEST`. Revocation works even when the current configuration is invalid. Environment/header values, OAuth settings and URL queries are redacted; approval binds the unredacted effective definition. These commands start no servers and create no Sessions. MCP confinement requires adding `mcp` to `sandbox.apply_to`; default scope excludes MCP, independently of project approval. Configured local servers start when their Location opens; tools remain subject to agent, Mode, permission and hook controls.


## Local MCP startup waiting

Configured local MCP servers start concurrently when a Location opens. The model can call `wait_for_mcp` with `{"servers":["shared"],"timeout":30}` to wait for named servers still connecting; timeout is in seconds, defaults to 60 and must be between 0 and 60. Zero reads current status immediately. The result includes each requested server's redacted status/error and a `timed_out` flag. Disabled/failed servers return without waiting; unknown names fail. Waiting does not start servers, invoke tools or approve discoveries, and cancellation leaves independent startup ownership intact.

The wait tool is read-only and allowed by default, subject to `wait_for_mcp` permission rules and ordinary hooks/agent controls. Connected tools are materialized on subsequent model steps. Local and remote loss reconnects after verified settlement with bounded backoff; full native acceptance remains incomplete.

## MCP resources

When a connected, authorized server in the Location advertises the `resources` capability, the model gets three read-only tools, also offered in plan mode:

- `mcp_list_resources` with `{"server":"docs","cursor":"..."}` returns one page of the server's resources (`uri`, `name`, and `title`, `description`, `mimeType`, `size` when the server sends them) plus `nextCursor` when more pages exist; pass it back as `cursor` for the next page.
- `mcp_list_resource_templates` takes the same input and returns `resourceTemplates` (`uriTemplate`, `name`, ...).
- `mcp_read_resource` with `{"server":"docs","uri":"file:///guide.md"}` reads one resource.

The client only sends `resources/list`, `resources/templates/list` and `resources/read` to a server that advertised `resources`; it never subscribes. Results are untrusted data: listings are wrapped in `<mcp_resources server="...">` and text contents in `<mcp_resource server="..." uri="..." mime="...">`, with `&`, `<` and `>` escaped so content cannot close or forge the frame; URI userinfo is dropped from displayed URIs. Binary (`blob`) contents are decoded, stored as a managed file next to other tool output and summarized as `[binary resource: N bytes stored at PATH]`. Large text follows the ordinary tool output budget and overflow files.

URIs must have an RFC 3986 scheme, contain no control characters and be at most 2048 bytes; they are opaque, never mapped to local paths. A server that returns the cursor it was given is refused. A read whose decoded contents exceed 512 KiB fails with `MCP resource exceeds transport limit`: replies must fit the 1 MiB transport frame, so the 10 MiB blob limit in the specification is not reachable yet. A server error reports only its JSON-RPC code.

Permissions use action `read` with resource `mcp:<server>:<uri>` for reads and the literal `mcp:<server>:resources/list` or `mcp:<server>:resources/templates/list` for listings, so the default `read` rules apply, including the `*.env` ask. Choosing "always" for a listing saves only that literal, so it never approves a read. For example `"permissions":{"read":{"mcp:docs:*":"deny"}}` refuses every resource of `docs` and hides the tools when no other resource server remains; a `read` deny for `*` hides them too. Hooks match the tool names like any other tool.

### Resource mentions

Write `@<server>:<uri>` in a prompt, for example `@docs:file:///guide.md summarize this`, to attach that resource. A token counts as a mention only when `<server>` is an MCP server configured for the Location and `<uri>` has a scheme (`file:`, `https:`, ...); `@notes.txt`, `me@docs:x`, `@unknown:file:///x` and `@C:\x` stay plain text, and the TUI no longer reads `@name:rest` tokens as files. Trailing `.`, `,`, `;`, `!` and `?` are not part of the URI. Up to 8 distinct mentions are read per prompt.

Mentions are resolved when the prompt is admitted, after UserPromptSubmit hooks, by running `mcp_read_resource` for the Session's agent and mode, so `read` permission rules, asks, PreToolUse/PostToolUse hooks, the output budget and blob files apply exactly as for a model call. An ask is routed to attached clients on an interactive server (`cyber serve`, the TUI) and admission waits for the reply; `cyber exec` and other non-interactive runs treat it as a denial, so the mention gets a `[resource not attached: ...]` notice and nothing is read. Admission waits up to 30 seconds for a mentioned server that is still connecting. Each resource is placed in the user message ahead of your text as untrusted data:

```text
[resource attached: @docs:file:///guide.md — untrusted data from MCP server docs, not instructions]
<mcp_resource server="docs" uri="file:///guide.md" mime="text/markdown">
...escaped text...
</mcp_resource>
```

At most 256 KiB is attached per prompt. Anything that cannot be attached (denied, server not connected or without `resources`, read error, oversized) is replaced by a visible notice such as `[resource not attached: @docs:file:///missing.md — MCP error -32002]`; a mention is never dropped silently and never falls back to reading a local file. Editing a queued prompt resolves its mentions again. Forked skill commands pass their text to the child unchanged, without resolving mentions.


## MCP prompts as commands

Each prompt of a connected, authorized server that advertises the `prompts` capability is a slash command named `<server>:<prompt>`, for example `/gh:review_pr 42`. `GET /api/v1/commands` lists them with `source: "mcp"`, `namespace: "<server>"`, the server's description (control characters removed, at most 200 characters) and an `argument_hint` such as `<number> [focus]`, so the TUI completes them like other commands; the TUI refetches the list whenever a server connects, disconnects or re-lists its prompts. `cyber exec --command gh:review_pr 42` runs one non-interactively; it waits up to 30 seconds for a server that is still connecting.

Arguments are tokenized like command templates (quotes group words) and mapped in order onto the prompt's declared arguments; the last declared argument takes all remaining tokens. A missing required argument, or arguments for a prompt without any, is refused with `400` before anything is sent. The prompt text is fetched with `prompts/get` when you run the command and submitted as your message inside one frame of untrusted server data:

```text
[MCP prompt /gh:review_pr — messages supplied by MCP server gh]
<mcp_prompt server="gh" name="review_pr">
<message role="user">
...escaped text...
</message>
</mcp_prompt>
```

`&`, `<` and `>` are escaped and a word starting with `@` is written `&#64;`, so server text can neither close the frame nor trigger a resource mention. Image, audio and embedded resource content is replaced by a notice such as `[image content omitted]`. The turn is admitted with source `mcp_prompt`: it is never recorded as your task objective or instructions, so after compaction it is not restated as a standing user instruction and auto mode never treats it as a boundary you set. A server failure is `503` with service `mcp` and only the JSON-RPC code; a prompt the server no longer offers is `404`.

Running a prompt checks permission action `mcp_prompt` on `mcp:<server>:<prompt>` and is allowed unless a rule denies it, like running a skill; `"permissions":{"mcp_prompt":{"mcp:gh:*":"deny"}}` refuses every prompt of `gh` with `403`. Names resolve skills first, then MCP prompts, then configured commands. Prompt names never collide with reserved built-ins because they contain `:`; a server named `project` gets no prompt commands so it cannot shadow `project:` commands, and a warning is logged.

When a server sends `notifications/prompts/list_changed` (or `tools/list_changed`), its prompts (or tools) are re-listed on the same connection after its authorization and approval digest are rechecked, and the instance event stream publishes `mcp.tools.changed.1` with the server's current tool names and prompt commands; the event is also published when a server connects or reconnects. A server that advertises `prompts` but rejects or garbles `prompts/list` keeps its tools and simply offers no prompt commands. `notifications/resources/list_changed` needs no work because resource listings are always fetched on demand.

This works the same for local and remote servers. A tool the server removed is withdrawn from the model, and a call to it settles as `Stale tool call: <name>` without reaching the server. A `list_changed` that a remote server sends on a tool call's event stream is applied after that call finishes, on the same session, without reconnecting.

## Deferred tool schemas

When the effective catalog exceeds `tool_output.deferred_threshold_tokens` (default 10000 estimated tokens, four characters per token), the model receives MCP/plugin names and one-line descriptions without their parameter schemas. Builtin and Session/client tools retain full schemas. The threshold is shared; zero forces eligible tools to defer.

The model can call `tool_search` with `{"query":"jira issue"}` or `{"select":["mcp__github__create_issue"]}`. Search matches case-insensitive words in names/descriptions, returns full schemas sorted by name and loads them for subsequent steps in the same Session. `limit` defaults to five and accepts integers from zero to 1000. Exact selections reject unavailable names before loading anything; when both query and select are supplied, select takes precedence.

Selections survive restart and compaction, while new Sessions and forks start independently. Calling an unloaded tool returns `Tool <name> is deferred. Load it with tool_search first.` Search is allowed by default, subject to `tool_search` permission rules, agent controls, hooks and ordinary output budgets. Loading a schema does not approve execution: fresh permissions, server authorization and registration identity still apply. Search neither starts servers nor invokes their tools. Plugin registration delivery remains incomplete.


## MCP workspace roots

Configured local connections advertise MCP roots: the canonical Location and existing files or directories in `sandbox.writable_roots`, resolved relative to the Location and deduplicated. Read-only policy advertises the Location alone. Roots use percent-encoded `file://` URIs and describe a connection snapshot (`listChanged: false`), so no `notifications/roots/list_changed` is sent. Each Location, including a worktree opened as its own Location, gets its own connection and sees only its own roots. An unavailable configured writable path refuses startup before native launch.

Changing `sandbox.policy`, `sandbox.writable_roots`, network isolation or allowed domains, the credential grant (`sandbox.readable_paths`, `sandbox.env.allow`) or the `sandbox.apply_to` selection replaces the running local server instead of leaving it on its launch-time roots and grants. Its tools, prompts, resources and instructions are withdrawn at once, a call to it fails with `MCP sandbox configuration changed; reconnecting` without reaching the server, and on the next refresh the old process is stopped and a fresh one starts with the current roots and sandbox. A writable root that no longer exists counts as a change, and the server stays down until the path returns or the configuration is edited. Remote servers receive no roots and are not affected.

Servers can request `roots/list` during initialization, tool execution or idle processing. The list describes the workspace and leaves process sandbox grants and tool permissions unchanged. Private scratch, unrelated read-only paths and sibling worktrees are not added implicitly. Sampling is advertised only when enabled (see [Local MCP sampling](#local-mcp-sampling)).


## MCP server guidance

Connected local servers can supply usage instructions during initialization. Eligible instructions appear in a `<mcp_instructions>` context block, ordered by server name and attributed to each server. Server text is escaped to preserve block boundaries. Guidance remains available when schemas are deferred, while agent, Mode, permission and server-authorization controls determine eligibility. Higher client registrations that shadow every MCP tool also remove that server's guidance.

Instructions use the `mcp/instructions` Context Source. Reconnection changes, connection loss, Location close and visibility restrictions update its applicability through mid-conversation system messages. The Epoch baseline remains byte-stable. Reading this source uses cached discovery metadata and starts no servers or RPCs. Remote MCP instruction delivery remains incomplete.

### Required local MCP startup

Set `required: true` on a server entry to hold a Session's first Turn until the server connects and publishes discovery, up to its configured `timeout` (default 30 seconds). A disabled, failed or unavailable required server reports `McpRequiredError: <name>` before prompt promotion or a model call; the original prompt remains pending for retry. Cancelling the wait preserves independently owned Location startup. Retry after recovery by waking the Session; unresolved native ownership remains fenced.

The requirement applies once per Session and survives replay and conversation rewind. A fork begins its own first Turn. The default is false and preserves existing server approval digests; enabling it changes the definition and requires matching authorization. Remote servers honor `required` the same way; a `needs_auth` server fails the first Turn with `McpRequiredError`.

### MCP output budgets and resolved options

Set a positive `output_token_limit` on an MCP server to cap its model-facing text using the four-Unicode-character token estimate. The global `tool_output.max_lines` and `max_bytes` ceilings still apply; an omitted cap inherits that global budget. The standard truncation notice follows the bounded payload and points to the managed file containing the full original text. Structured data is retained. This is an estimate rather than provider tokenizer accounting. Local success, error and structured results share this settlement path.

Run `cyber mcp get <name>` to review one loaded server's definition, provenance, approval digest, authorization observations and output budget without connecting or creating a database. Header/environment values, OAuth details and URL query values are redacted; numeric output limits and the effective `headers_refresh_seconds` remain visible. A project-defined `headers_command` is shown verbatim so it can be reviewed before `cyber mcp trust` approves its digest; a command from your own global configuration is redacted. For a project-defined server, any `url`, header or `env` value written with an `{env:NAME}` or `{file:path}` placeholder is shown as that template, for example `X-Trace: {env:GITHUB_TOKEN}`, so you can see which of your local values the server would receive. The resolved value is never printed. Unknown or withheld names fail review.

A remote server may set `headers_command`, a shell command run outside the sandbox (only after the definition is approved) whose stdout must be a JSON object of string headers. Its headers override static `headers`, transport-owned names are refused and the command is re-run every `headers_refresh_seconds` (default 3600, at most 86400) on the live session, before the next request once the interval has elapsed. A failed run fails the connect. During a refresh it is treated as connection loss, so the reconnect re-runs the command. Diagnostics never contain the command output. A 401/403 on a live session of such a server also triggers a reconnect, and the server becomes `needs_auth` only if the freshly run command's headers are refused too. A server selected by `sandbox.apply_to` (or connected by a sandboxed MCP-tool hook) cannot use `headers_command`: the connect fails before the command runs.

### Remote MCP OAuth

Remote servers that answer 401 can be signed in with OAuth 2.1. Sign-in is always a user action: a Session, `cyber exec` or the background service never starts one and never opens a browser. A refused server settles `needs_auth` with ``MCP server <name> needs authentication; run `cyber mcp auth <name>` ``, its tools are withdrawn, it is not retried, and a `required: true` server fails the first Turn.

The optional `oauth` field of a remote entry controls this:

```json
{
  "mcp": {
    "github": {
      "type": "remote",
      "url": "https://mcp.example.com/mcp",
      "oauth": {
        "client_id": "cyber-cli",
        "client_secret": "{env:GITHUB_MCP_CLIENT_SECRET}",
        "scopes": ["repo.read", "issues.write"],
        "redirect_port": 53682
      }
    }
  }
}
```

Omit `oauth` or set it to `true` to use defaults (dynamic client registration, the scope from the server's challenge or metadata, an ephemeral callback port). Set it to `false` to disable OAuth: stored tokens are then neither sent nor refreshed. In the object form every field is optional and unknown fields are refused; `client_secret` requires `client_id` and should use `{env:NAME}`; scopes are nonempty tokens without whitespace; `redirect_port` is 1024 to 65535. The `oauth` value is part of the server's approval digest, so a project-defined server needs `cyber mcp trust` again after it changes. `cyber mcp get`/`definitions` show `client_id`, `scopes` and `redirect_port`, never the secret (a project-defined secret shows only its `{env:}` template).

`cyber mcp auth <name> [--port N]` signs a server in:

1. It checks workspace trust and, for a project-defined server, its approved digest before sending anything.
2. It sends one unauthenticated `initialize` to read the `WWW-Authenticate` challenge, then fetches protected-resource metadata (the challenge's `resource_metadata`, then `/.well-known/oauth-protected-resource<path>`, then the root well-known URL) and the authorization server's RFC 8414 or OpenID Connect metadata. The metadata must name exactly the configured URL and issuer, every endpoint must be HTTPS (plain HTTP only to a loopback host, and a local address only when the server, or for endpoints the issuer, is itself local) and PKCE S256 must be offered, or the command fails without an authorization request.
3. Without `oauth.client_id` it registers a client dynamically. If the server offers no registration endpoint it fails with a `needs_client_registration` diagnostic naming `oauth.client_id`; configure one and run `cyber mcp auth` again.
4. It prints the authorization URL to stderr. Open it in a browser; the server redirects to `http://127.0.0.1:<port>/mcp/oauth/callback`, which accepts one callback with the matching `state` and times out after 5 minutes. The code is exchanged with the PKCE verifier and the RFC 8707 `resource` (the server URL without its query, so a key in the query is never sent to the authorization server or printed).
5. It stores the tokens and, if the background service is running, asks it to reconnect the server (`POST /api/v1/mcp/{name}/reconnect`), reporting the result as `reconnect` in its JSON output. No token is printed.

All sign-in traffic goes through the same network policy and owned proxy as the server itself, and follows no redirects. OAuth needs an `https://` server URL (plain `http://` only to a loopback address or `localhost`): `cyber mcp auth` refuses any other server before contacting it, and a stored token is never sent to one.

Tokens are stored for the server name at its exact URL and issuer. Where they go depends on the platform and is fixed when Cyber Code is built; no environment variable, configuration or project file changes it:

- **macOS and Windows:** the OS keyring (Keychain, Credential Manager) holds the access token, refresh token and client secret under the service `cyber-mcp-oauth`, split into entries of at most 2560 bytes. Entry names start with a random id of the data directory, so two data directories (two `CYBER_HOME`s) of one user never share or delete each other's tokens. A secret too large for 8 entries is refused with `credential too large for the OS keyring`, and nothing is stored. Everything else (issuer, client id, scopes, expiry, remembered conditions) is in `<data>/mcp-auth-index.json`, which never contains a token or client secret and never records the server URL itself (an issuer or token endpoint may share its origin). The keyring also holds a digest of each index record, so a record edited by someone without keyring access is refused rather than used. A missing or altered keyring entry, or an edited index record, is treated as no valid credential, never as a shorter token.
- **macOS upgrades:** tokens an earlier release saved in `<data>/mcp-auth.json` keep working. They move to the Keychain the next time they are written (`cyber mcp auth`, or a refresh), and `cyber mcp logout` removes them from both places. If the Keychain refuses a deletion (for example a declined prompt after an upgrade), logout fails instead of reporting success; run it again to finish.
- **Windows:** a file left at `<data>/mcp-auth.json` is never used. While one exists, every sign-in, stored-token lookup and logout fails with a storage error, so remove it.
- **Linux and other platforms:** `<data>/mcp-auth.json`, created with mode 0600. Linux has no keyring backend that works without libdbus, so this file is the fallback. A file that group or others can read is refused.

Changing a server's `url` means its old token is never sent; sign in again. Tokens never appear in configuration, logs, events, status or command output.

Every request to the server carries headers in this order, later layers replacing the same name: static `headers`, then `Authorization: Bearer <token>`, then `headers_command` output. A token that expires within 5 minutes (or within half its lifetime, for tokens issued for less than 10 minutes) is refreshed before connecting and before each call, catalog refresh or idle ping on a live connection, and the rotated refresh token is saved before use. Connections and processes sharing a stored credential refresh it one at a time (other servers' refreshes do not wait) and pick up each other's rotated tokens, a refresh never writes back a credential that `cyber mcp logout` removed, and a refresh that does not finish within the server's `timeout` counts as failed. A 401 to a token that has a refresh token is retried once after one refresh; if that is refused too, or the refresh fails, the server settles `needs_auth`. A 403 with `error="insufficient_scope"` settles `needs_auth` at once and remembers the extra scopes, which the next `cyber mcp auth` requests.

`cyber mcp logout <name>` removes every stored token and remembered condition for the name (at any URL or issuer) without reading configuration, and, if it removed anything, asks a running service to reconnect the server, which then settles `needs_auth`. A connection that reconnect does not reach stops using the removed token at its next call or idle ping and settles `needs_auth` too. MCP-tool hooks send the stored token like any other connection. Tokens are not revoked at the authorization server.

To reconnect a `needs_auth` or `needs_client_registration` server after signing in from another tool, call `POST /api/v1/mcp/{name}/reconnect` (SDK `mcp.reconnect`). It returns `connecting`, then publishes the usual `mcp.status.changed.1` transitions; an unknown server is 404, an unapproved one 403, and a server in any other state, with a changed definition (except a `needs_client_registration` server whose `oauth.client_id` was just configured) or in a closing Location 409.

### MCP call timeouts and progress

An explicit server `timeout` applies to both connection startup and tool-call inactivity. When omitted, connection startup defaults to 30 seconds and calls use `mcp.tool_timeout` (default 300 seconds). `cyber mcp get <name>` reports the resolved `call_timeout_seconds`. Definitions with an explicit timeout require fresh matching approval under these corrected call semantics; definitions with omitted defaults preserve existing approval digests.

Valid progress for the active request resets inactivity. Values must advance, including fractional values; duplicate/regressive progress, foreign tokens, malformed optional fields and messages carrying a request identity do not extend the timer. A timeout leaves remote completion unverified until native ownership settles. Calls are not replayed automatically.

When a call is interrupted, times out or its Location closes before the server answers, Cyber Code sends the server one `notifications/cancelled` for that request (on a remote session, before the session is closed), waiting at most one second. It is skipped when the definition is no longer approved or its stored OAuth credential was removed. A failure is logged without header values and never delays shutdown.

### Graceful local MCP close

Explicit Location close and runtime shutdown withdraw MCP tools before stopping shared servers. On POSIX, client IO closes and the owned group receives SIGTERM. The leader remains unreaped for the full five-second grace period so descendants can finish cleanup after the leader exits. SIGKILL then stops remaining group members, followed by native acknowledgement, local proxy shutdown and durable settlement before scratch/checkout cleanup. Windows uses owned Job Object termination.

Independent servers close concurrently. An interrupted close retains its original grace deadline and Location admission fence; retry resumes that deadline. Unknown ownership or failed settlement remains fenced for recovery. Failed startup, cancelled calls, lost connections and owner disposal retain immediate cleanup, with disposal providing no durable acknowledgement. Remote close joins the HTTP bridge and its proxy and sends the session `DELETE`; settling MCP owners when an embedded `cyber exec` exits and full native platform acceptance remain incomplete.

### MCP form elicitation

Local stdio and remote (Streamable HTTP and SSE) servers advertise form elicitation. During an interactive tool call, `elicitation/create` opens the existing question flow, names the requesting server, and asks for explicit Accept, Decline or Cancel before returning values. A remote server may send the form on the call's own event stream or on its GET event stream; the answer is posted back on the same session. Flat strings, numbers, integers, booleans and string-enum arrays retain their JSON types; supported constraints and formats are checked before sending. Optional fields can be omitted and declared defaults require an explicit choice. Decline, Cancel and invalid answers send no form values, and an invalid form schema is refused without asking you anything.

Authorization is checked before asking and again before sending answers: a changed or untrusted definition, or a `cyber mcp logout` while the question is open, sends Cancel instead of your answers. `cyber exec`, other non-interactive sessions, and requests during startup or idle decline at once without creating a question; logs omit form text and answers. Human question time pauses call inactivity. Interrupting the call or closing the Location dismisses its pending questions and sends the server a best-effort `notifications/cancelled`; if the server cancels the form itself, or the call finishes while you are still answering, the question is dismissed and no answer is sent.

Only form mode is supported. A URL-mode request is refused: Cyber Code never opens a browser or fetches a link for a server, and the URL is not shown or logged. Native Windows acceptance is still pending.

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

Following the [v1 provider format](https://opencode.ai/docs/providers/) and [v2 provider format](https://opencode.ai/v2/docs/providers/), read-only provider imports also accept bounded OpenCode v1 `provider` entries with `npm/options`, and v2 `providers` entries with `package/settings`. Explicit supported OpenAI-compatible chat, OpenAI Responses and Anthropic packages select the native protocol; arbitrary packages are never loaded. Provider endpoints, credential/header environment references, ordered credential environment names and model aliases/names/disabled flags/limits are converted. Model IDs containing `/` retain escaped JSON-pointer provenance. Setup records cover accepted credential/header references; the ordered `env` list remains a native credential fallback, rather than a requirement to set every variable. Unsupported package/auth/account stores, advanced settings and package-specific variants remain pending. Unsafe substitutions, endpoint credentials/query strings and ambiguous or colliding bindings refuse the preview. Literal credentials never enter the proposal or report.

OpenCode v2 request `body` objects and model/variant `headers` now map to native request overlays. V2 `variants` arrays require unique explicit IDs and become native named variants. Native resolution keeps provider → model → selected variant order: nested body objects merge, later arrays/scalars replace earlier values and headers replace case-insensitively. Setup uses exact raw variant-array indices and excludes credentials hidden by native conflicts. Bodies are bounded to depth 16 and 4,096 nodes; null fields, credential keys/echoes and source substitutions require an additional adapter and refuse the current proposal. V1 package-specific model/variant options and v2 `settings` remain pending. [Source request-layer semantics](https://opencode.ai/v2/docs/models/).

Custom inline OpenCode agents now map source `agent`/`agents` to native profiles, including prompt/system text, description, mode, qualified model/variant, hidden/disabled flags, color, steps and strict permission conversion. Custom v1 profiles default to `all`; v2 profiles default to `primary`. V2 global and agent permission arrays append across source layers with original-index attribution. Native permission maps accept an ordered `rules` array alongside action shorthands, expanding it at the written position. Global and agent wrappers enforce ordinary deny ceilings, including under bypass. Project grants still need native checkout trust. Built-in agent defaults, prompt file references and advanced fields remain pending. [V1 agents](https://opencode.ai/docs/agents/) · [V2 agents](https://opencode.ai/v2/docs/agents/).

Explicit OpenCode compaction `auto`, `buffer` and `preserve_recent_tokens`/`keep.tokens` now map to native runtime settings. The validated numeric keep count is visible in previews; credential strings and header contexts remain redacted. Ambiguous token aliases and invalid integers refuse conversion; omitted source defaults and unsupported pruning/tail behavior are not inferred. Commands and `instructions` entries remain reported as pending until their complete native consumers and source resolution are implemented. [V2 compaction settings](https://opencode.ai/v2/docs/compaction/).

Read-only imports accept static Claude `mcpServers` (including `.mcp.json`), Codex `mcp_servers`, and OpenCode flat or `mcp.servers` declarations. Local commands/arguments and HTTP endpoints become native server proposals. Literal environment/header values become required environment references; JSON setup records identify the contributing source fields and never contain the original values. No source server starts during an import. Native `disabled: true` has the same server state and approval identity as `enabled: false`; conflicting flags refuse.

Unsupported server options appear as pending fields. Advanced authentication, additional transports, timeout conversion, source-relative working-directory semantics and Claude per-project state associations still need adapters. Global Claude state contributes only top-level server declarations. These previews remain incomplete and do not provide confirmed writes.

Codex HTTP server previews map `http_headers` and `env_http_headers` to native headers and preserve `required`. Static values become environment setup references; existing environment bindings remain references with source provenance. Conflicting header names/aliases and generated credential names colliding with another source binding refuse the whole preview. Command header helpers are not executed or imported.

### Kept-field comparisons

When a supported source field is ignored because a native or earlier auto-source key already exists, its `merged` report includes a `comparison` object with `kept` and `ignored` values. Text output prints both below the field. Values use the normalized native proposal shape, not original source formatting. Credential/header fields and known credential echoes are redacted, including echoes discovered in later source layers. Existing configuration remains unchanged. Complete native winning-layer provenance, runtime-effective values, remaining adapters and report-file writes still need implementation.

Setup records now include only environment bindings used by newly proposed fields. Kept credentials produce no setup instruction. Shared variables appear once with every accepted source field; ignored fields are excluded. Generated names cannot reuse environment references in existing native configuration, including embedded references with defaults. Collision checks run before shared records are combined. Adapter validation still refuses unsafe or ambiguous source declarations.


### Static configured commands

The public command catalogue and Session command endpoint now consume `commands` entries with literal `template`, optional `description` and `argument_hint`. Arguments use the same `$ARGUMENTS` and positional substitution as skills. A user-invocable skill replaces a same-name command. Reserved names (`help`, `exit`, `goal`, `loop`, `workflows`, `mode`) remain protected; configured definitions and skills with those names use `project:<name>`.

OpenCode dry-run previews convert static inline `command`/`commands` entries into this consumed native shape, retaining exact source-field provenance. Definitions containing overrides (`agent`, `model`, `variant`, `subtask`), shell-output injection, `@file` references, source substitutions or unknown fields stay pending as whole definitions. Read-time native Markdown discovery, complete invocation behavior, unavailable-definition inspection, CLI command management and reviewed migration writes remain open.


Discovered `.claude/commands/**/*.md` and OpenCode command Markdown now become static native command proposals. Optional YAML frontmatter supplies `description` and either argument-hint spelling; the body supplies the template. Reports use `body` and `frontmatter:<key>` descriptors for these source fields. Names retain nested paths and remove only the final `.md` suffix. Project and nearer directory definitions override global/ancestor definitions; same-scope Markdown replaces the inline definition as a whole. Existing native keys stay preserved. Unsupported headers, shell injection and file references remain pending without exposing body values or executing them. An unsupported winning definition suppresses older static fallbacks. Legacy `.opencode/command` files are supported; same-scope singular/plural name collisions refuse pending alias equivalence. Every read file, including overridden or unsupported commands, remains part of review verification.

At runtime, the application catalogue and command expansion also read static Markdown directly from global `command`/`commands` folders under the configured Cyber directory, and project `.cyber/command`/`.cyber/commands` folders from the checkout root to the current Location. Claude-compatible `.claude/commands` folders are read from the configured home and the same project chain. No import is required. Project definitions override global definitions; nearer project directories override ancestors. Within a scope, native Markdown overrides native inline configuration, which overrides compatibility Markdown. Explicit runtime/profile inline overrides without filesystem attribution retain highest precedence. The plural native folder wins over the singular folder when both declare the same name. Later skills retain precedence; reserved names use `project:<name>`.

Each catalogue or expansion request reads the files again; returned templates keep no filesystem handles. Discovery refuses linked directory traversal and hard-linked files, bounds directory entries to 4,096, recursion to 32 levels, files to one MiB each, aggregate reads to sixteen MiB and the registry to 128 names. Unsupported individual definitions suppress lower-priority fallbacks; an incomplete or oversized directory scan withholds the entire static registry. `CYBER_DISABLE_PROJECT_CONFIG` also disables project command discovery. Advanced execution fields, source-path/diagnostic inspection, plugin/MCP command sources and full invocation semantics remain under implementation.


`GET /api/v1/commands` also includes `namespace` for namespaced entries and `provenance` for available static commands. Provenance contains the winning and shadowed origins, each with a scope (`global`, `project` or `runtime`), kind and source paths. It contains no templates or raw loader labels. Native inline configuration can combine leaves from several files, so its paths list includes all contributing file layers. Logical profile/CLI/environment overrides retain runtime precedence without inventing a filesystem source. The catalogue still lists available entries only; unavailable-definition diagnostics, complete skill/plugin/MCP provenance, CLI inspection and current-agent permission effects remain open.


Skills may declare project-relative path globs:

```yaml
paths: ["db/migrations/**", "src/*.{rs,sql}"]
```

Successful file reads, writes, edits and patches append a `<system-reminder>` suggesting matching skills. Directory listings, failed reads, model-disabled skills and effective skill/tool denials do not produce suggestions. Reminders include names and shortened descriptions. The model can load the body through the normal skill tool. Each skill is suggested once per Context Epoch. The seen names commit with the tool result, survive runtime recreation and reset when a new Epoch starts, including after compaction or context repair. Concurrent file calls share that durable decision. Relative declarations are bounded to 64 globs of 256 bytes each. Known completed file operations also produce reminders when a later patch operation or post-hook fails; the tool retains its error status. Aborted/unresolved operations do not produce reminders. Reminders share the configured line/byte payload budget with tool text. Overflow preserves complete text in a managed artifact and keeps the reminder visible. If the reminder itself cannot fit or overflow storage fails, settlement fails without marking the skill seen; a later matching operation can retry after repair.

Inline model-loaded skills can declare bounded `allowed-tools` and `disallowed-tools` lists or comma-separated strings. Patterns use `tool[:resource]`, with `*` and `?` wildcard matching; a bare tool covers all resources. Captured grants apply to later tool groups in the same model Turn, preserve explicit permission denials and expire before the next Turn. Skill loading is a batch barrier. Disallowed patterns override grants and bypass; descendants inherit denials. Instruction text cannot create a grant. Inline user commands capture those declarations with the expanded prompt. Their scopes apply to the first model Turn; an optional skill `model` applies through its tool loop without replacing the Session model. Captured values survive replay, and changed declarations conflict with reuse of an existing request ID. Explicit skill denials refuse admission even in bypass; ordinary prompt text cannot supply declarations. Normal model-loaded inline skills do not switch models. Complete lifecycle and native acceptance remain open.

Skills can declare `context: fork` to run in a background child with the caller's history, using the skill's model when specified. An optional `agent` names the child profile; without it the caller's profile is retained. Agent selectors require fork context and are captured in retry/replay identity. The built-in `reviewer` permits inspection and has an immutable filesystem read-only ceiling, including for descendants: bypass, configured grants and skill grants cannot authorize file edits or mutating shell commands. Use a different custom profile name for coding subtasks. The caller receives the queued summary; instruction bodies and child intermediate output stay in the child. The captured child instruction frame includes the skill base directory and a bounded list of supporting files. User commands preserve held input, release and queue delivery, and bind captured declarations to their retry identity. Child creation shares ordinary subagent admission, Mode/ancestor ceilings and cancellation ownership. Refusal or cancellation before child admission queues a durable status handback with the skill name and delegation ID. Setup diagnostics remain available through the delegation API; replay delivers a missing handback once. Unknown admission ownership requires reconciliation rather than another launch; full native and lifecycle acceptance remains open.


Five bundled skills ship inside the binary and appear in the command catalogue without installation:

| Command | Purpose | Execution |
|---|---|---|
| `/review [target]` | Find actionable defects and request structured findings | Background reviewer |
| `/simplify [scope]` | Find reuse and simplification opportunities | Background reviewer |
| `/security-review [scope]` | Trace pending changes for exploitable defects | Background reviewer |
| `/batch <change>` | Coordinate 5-30 independently checked tasks | Caller, delegating separate worktrees |
| `/customize-cyber <request>` | Explain or update configuration and integrations | Caller |

Embedded instruction frames use `builtin:<name>` as their source; this is not a readable filesystem directory and grants no external-directory access. A discovered `SKILL.md` of the same name replaces the package. Configured static commands take precedence over the embedded defaults; discovered skills still take precedence over those commands. Unsupported winning command definitions remain unavailable. File-backed replacements keep their ordinary directory and supporting-file permissions. These are instruction packages using existing tool authority, not permission exemptions. Full review target resolution, the dedicated `cyber review` CLI and enforced finding validation remain under implementation.

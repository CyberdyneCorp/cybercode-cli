# Cyber Code Roadmap

Every requirement in `openspec/specs/**/spec.md` starts with a phase tag `(P0)`–`(P4)`. A phase is done when all of its tagged requirements are implemented, have passing scenario tests, and the exit criteria below hold.

```bash
# list requirements for a phase
grep -rn "^### Requirement" -A1 openspec/specs | grep -B1 "(P2)"
```

Delivery rule: each phase ships as one or more OpenSpec **changes** (`openspec/changes/<name>/`) that implement existing requirements. A change that alters behavior updates the spec in the same PR, and `openspec validate --all --strict` plus `python3 scripts/spec_lint.py` gate CI.

---

## Phase overview

| Phase | Theme | Outcome | Rough size* |
|---|---|---|---|
| **P0** | Local core | A usable single-machine agent with any LLM: TUI + `exec`, durable sessions, core tools, permission rules, open API | ~12 eng-weeks |
| **P1** | Table stakes | Parity with Codex / OpenCode for daily coding: sandbox, modes, subagents, hooks, MCP, skills, rewind, LSP, memory | ~14 eng-weeks |
| **P2** | Autonomy & orchestration | Better than Claude Code locally: workflows (multi-model), goals with queues, loops/cron, teams, agent view | ~12 eng-weeks |
| **P3** | Connectivity | Cyber Account, remote control, cross-machine messaging, runners with two-way handoff, routines, channels, sharing, org policy | ~16 eng-weeks |
| **P4** | Ecosystem | Marketplace + evals, mobile apps, JetBrains, mods UI, analytics, Python SDK | ongoing |

*The original size figures are unvalidated planning placeholders for a team of 4–6. Re-estimate after M0.3 using coding-quality, recovery and storage measurements; they are not delivery commitments.

The first release prioritizes reliable editing, recovery and measurable coding quality. P0 adapters are OpenAI Responses, Anthropic Messages and OpenAI-compatible endpoints including local servers. Additional native providers arrive in P1. Advanced orchestration and hosted services retain their later phases.

---

## P0: Local core

**Capabilities:** `workspace-trust`, `harness-evaluation`, `sandbox` (macOS/Linux enforcement, network and credential boundaries), `observability-costs` (usage and budgets), `cli-commands`, `configuration`, `storage-events`, `installation-upgrade`, `provider-catalog`, `provider-credentials`, `session-runtime`, `system-context`, `compaction`, `tool-registry`, `builtin-tools`, `permissions-modes` (rules + `default`/`plan` modes), `snapshots-checkpoints` (snapshot + revert), `agents-subagents` (built-in & custom agents, step limits), `skills-commands` (basics), `mcp` (stdio + HTTP tools), `server-api`, `client-sdk` (TS), `tui` (basics), `exec-mode`.

**Milestones**
1. **M0.1 Skeleton and evaluation fixtures:** Rust workspace (`cyber-core`, `cyber-llm`, `cyber-store`, `cyber-server`, `cyber-tui`, `cyber-cli`), XDG paths, SQLite WAL/FULL + event store and bounded writer queue, config loader with trust-gated JSONC layering, reproducible coding and recovery fixtures. **Status: implemented** (change `implement-m0-1-skeleton`). Backup/restore, retention GC and logging were completed in the later P0 operations change.
2. **M0.2 First turn:** catalog from models.dev with an offline snapshot; adapters for OpenAI Responses, Anthropic Messages and OpenAI-compatible Chat (including local servers such as Ollama); streaming Turn with tool calls. **Status: implemented** (change `implement-m0-2-first-turn`); live-verified against OpenAI. Keyring-stored connections, `cyber providers login`, background catalog refresh and `catalog.updated` events remain deferred; environment/config credentials and startup/on-demand catalog loading are implemented.
3. **M0.3 Durable runtime:** inbox admission (steer/queue), Drains, interrupt, retry and error classification, usage/cost, compaction, Context Epochs. **Status: implemented** (change `implement-m0-3-durable-runtime`); live-verified against OpenAI. User shell commands and model-proposed task-state updates arrive with the M0.4 tools; retention GC, backup bundles, logging and `cyber doctor` are implemented (change `implement-p0-operations`).
4. **M0.4 Tools & safety:** read/write/edit/apply_patch/bash/glob/grep/webfetch/websearch/todo/question/skill; output budget; permission engine; checkout trust; macOS/Linux sandbox; conflict-aware git snapshots + revert; unknown-outcome recovery. **Status: implemented** (change `implement-m0-4-tools-safety`); live-verified against OpenAI, sandbox verified on macOS and in a Linux container. Native provider web search, media parts, background bash and a Landlock fallback for Linux hosts without bubblewrap are deferred; the HTTP routes for permissions, questions, diffs and revert arrive with M0.5.
5. **M0.5 Surfaces:** OpenAPI server + SSE replay, TS SDK, TUI (prompt, rendering, permission/question prompts, session picker), `cyber exec` with JSON stream and exit codes. **Status: implemented** (change `implement-m0-5-surfaces`); `cyber exec`, the background service and the TUI live-verified against OpenAI. PTY routes, mDNS, the generated Rust SDK and parts of the TUI (inline mode, mouse, OSC 52, setup wizard, update prompt, split diffs, syntax highlighting, keybinding config) are deferred; live OpenAI and Anthropic baselines (including 50 turns), full backup/verify/restore, retention and logs are complete. Tool goldens and storage/startup probes are available; default service cold startup meets the 150 ms target on the M2 Max reference machine; the embedded probe retains one outlier. All six macOS/Linux platform builds pass CI. The local-model coding and long-task baseline remains. See [P0 exit evidence](docs/measurements/p0-exit-evidence.md).

**Exit criteria**
- Runs a 50-turn coding task end-to-end with OpenAI, Anthropic and a local model through the compatible adapter. Each P0 adapter also passes the published live coding baseline in `harness-evaluation`; turn count alone is insufficient.
- `kill -9` during a Turn loses no acknowledged prompt; unknown external outcomes reconcile without blind mutation retries. Storage fault cases distinguish process crashes from power-loss simulation.
- Untrusted project settings cannot start integrations, read host secrets or redirect provider traffic. Rewind preserves conflicting user edits; repeated compaction preserves user constraints.
- A full database-plus-artifact backup restores and verifies its manifest. Publish admission latency and writer contention measurements under the workload in the storage decision.
- Golden tests for every built-in tool; permission scenarios are table-driven tests.
- Cold start under 150 ms to the TUI first frame on a named reference machine; P0 supported execution platforms are macOS (arm64/x64) and Linux (x64/arm64, gnu+musl). Windows builds may be preview-only until P1 sandbox parity; unsupported enforcement fails closed unless the user explicitly selects full access.

## P1: Table stakes

**Capabilities:** `browser-verification`, `web-client` (local web UI), `provider-catalog` (additional native providers, fallback chain), `sandbox` (Windows, escalation and advanced policies), `permissions-modes` (`accept-edits`, `auto` classifier, `dont-ask`, `bypass`, protected/critical paths, saved approvals), `agents-subagents` (`agent` tool with `output_schema`, background, fork, worktree isolation), `worktrees`, `background-tasks`, `hooks`, `plugins-marketplace` (plugin host only), `mcp` (OAuth, tool search, resources, prompts, `cyber mcp serve`), `skills-commands` (full, compat locations), `memory`, `code-intelligence`, `snapshots-checkpoints` (rewind UI: code / conversation / both), `cross-session-messaging` (same machine), `observability-costs` (cost, budgets, OTel), `editor-integration` (ACP + VS Code), `compat-import`, `configuration` (features registry, `-c` overrides, references), `permissions-modes` (session ruleset API, auto-mode rules, rule dry run), `sandbox` (profiles, env policy), `exec-mode` (streaming input).

**Milestones**
1. **M1.1 Sandbox + modes:** **Status: in progress** (change `implement-m1-1-sandbox-modes`; implementation authorized while P0 local-model validation continues). Extend the P0 Seatbelt / bubblewrap baseline with Windows restricted tokens and Windows network enforcement through the existing allowlist proxy; mode cycling; the auto-mode classifier backed by `model_roles.evaluator`. The evaluator foundation, Bash and initial Python/JavaScript removal guards and four-mode TUI/effective-mode state are implemented; Production auto-mode tool dispatch is implemented locally with durable allow/block/fallback decisions and preserved manual ceilings; Validated trusted auto-mode rules/policy/read-only/fallback controls are implemented locally with project trust gating and durable policy decisions; Checkout-scoped statistics/reset, confirmed one-shot replay and ancestor auto block ceilings are implemented locally. Ancestor classifier/config gates are implemented locally with parent-specific context and child billing. Literal filesystem path analysis covers file operands, directory copies/moves and implicit destinations locally, including descendant, alias and inventory checks; native acceptance and remaining option/context semantics remain open. Native AppContainer asynchronous streams and the foreground process adapter pass at `fcebb4f`; remaining interpreter proofs and complete Windows enforcement remain open.
2. **M1.2 Subagents done right:** **Status: in progress** (change `implement-m1-2-subagents-worktrees`). Durable child result ownership is exclusive across runtimes and retains original target boundaries through callback and writer admission. Owned local subtree stop and idle-native cancellation are implemented with bounded acknowledgement and durable Unknown reports; Matched reopening is implemented locally with native acceptance pending; reviewed recovery, complete cross-process actor cancellation and client adoption remain open. Managed creation/reuse, primary-checkout inclusion, repository locking and journaled setup preserve user edits and interruption evidence. TUI/exec `--worktree` and API POST start fresh Sessions with live setup output. `cyber worktree list`, API GET and SDK listing report verified Git status and associated Sessions, with pending/invalid recovery diagnostics. Session creation, Drains, user shell, rewind, compaction, context repair and Session setup now claim cross-process checkout leases and persist exact creation bindings; crashed/disposed work retains unknown activity. Native Windows startup admission, runtime capability verification and low-level LPAC launch/isolation steps pass at `44f95be`; complete enforced execution remains unaccepted. Windows long-root status, summaries and ignored enumeration route through an owned read-only helper; repository, helper and actual isolated-child native steps and the full CI matrix pass at 3d2ea39. Completed Windows CI at `af9d018` passes the corrected shell fixture and model-selection coverage and fails only the mandatory long-root status assertion. All other jobs pass; newer Mode/step and foreground/structured child work still requires native acceptance. Agent model/variant defaults, starting Mode, step limits and identity pinning through preparation/tool settlement are implemented locally; fresh foreground and background children now execute with permission-before-creation, cancellation ownership, depth checks and in-process FIFO admission, including schema-governed results with one correction attempt. Foreground/background children share durable parent-scoped names. Background tasks have durable records, restart interruption, queue handback and API/SDK/TUI listing/stop controls; local children can resume by name/ID with exclusive result ownership and fresh structured attempts. Local forked children inherit history, Epoch and task context without copied execution ownership, and /fork creates an independent top-level Session. The /subtask command and authenticated API/SDK start explicit user-requested background forks while keeping the parent open. The endpoint also accepts visible named subagent profiles for fresh background children; leading TUI @mentions and agent/file completion use it. Exec leading mentions now use typed delegation with attachments, profile-capped step limits, durable child history, terminal Job following and owned cancellation. Caller-owned runtime cancellation preserves queued noninterference and settles a racing owned Job handoff. An input-bound durable admission ledger now exposes authenticated API/SDK start, lookup and stop, with pre-submission tombstones, a pre-effect launch marker and fenced unknown outcomes. Exec now retains a client request ID before submission, recovers lost responses through scoped lookup and applies its timeout/interruption to queued admission; 815 local Rust tests and workspace Clippy pass. The TUI now saves request IDs before submission, restores lookup on reconnect and offers pending-request cancellation through Esc and `/admissions`; local validation passes 57 TUI tests and workspace Clippy. Explicit cancellation now resolves abandoned reserved admissions through the durable launch fence and preserves tombstones against late host errors; 835 local Rust test executions and workspace Clippy pass, including abrupt owner-death coverage. Complete native-effect reconciliation remains required. Descendant billing now persists nested and hidden usage separately from own totals, retains surviving ancestor charges after child deletion and discloses incomplete historical backfill. Local validation passes 843 Rust executions and workspace Clippy; native acceptance is pending. Delegated exec now checks combined own/descendant cost and tokens at the recorded limit, negotiates billing capability before submission and preserves separate totals in JSON; 52 CLI test executions and workspace Clippy pass locally. Soft Session budgets now persist trusted creation defaults, activation and warning/exhaustion markers; visible and hidden provider calls, retries and web summaries check every ancestor against durable own/descendant spending. Local validation passes 870 Rust executions, followed by 57 budget/compaction/HTTP tests and workspace Clippy after error/type refinements, plus 47 SDK tests and strict specs. Native acceptance is pending. Typed budget/storage refusals now survive compaction preparation and retries; two unchanged-main regressions and all 59 focused tests plus workspace Clippy pass locally. Background Job settlement now includes durable descendant deltas with resume baselines and legacy uncertainty; 65 focused tests and workspace Clippy pass locally. Ordinary exec now observes durable subtree deltas, excludes prior history, reports attribution uncertainty and bounds interruption acknowledgement; 59 CLI tests and workspace Clippy pass locally. Descendant token-class receipts now survive deletion and expose separate class completeness in the API/SDK; 88 focused server/storage tests and workspace Clippy pass locally, with native acceptance and full cost presentation pending. TUI `/cost` now displays a refreshable subtree snapshot with token classes, cache hit rate and attribution uncertainty; all 61 TUI tests and workspace Clippy pass locally. The Session usage API and SDK now read own/descendant/combined billing from one SQL snapshot with corruption refusal and completeness flags; 43 focused server tests and 48 SDK tests pass locally. TUI cost refresh now uses the atomic usage API with strict report validation and stale-response fencing; 64 TUI tests and workspace Clippy pass locally. Completed native diagnostic CI at `553f718` still fails mandatory long-root status: staged diff succeeds, while unstaged/untracked controls fail, so a composed-command replacement is not accepted. A test-only library status candidate now covers long roots and read-only repository evidence; native results and owned sandbox integration remain pending, with production status unchanged. Generic durable prompt admission, reserved budgets, daily caps, explicit subtree cancellation with racing-admission fences, complete live client monitoring and queued/held delegation remain open. Fresh isolated foreground/background children, including forks and structured results, now use durable managed bindings, separate branches, source-authorized setup and file/diff summaries. Clean child removal uses checkout activity fencing; edits, ahead commits, keep policies and failed/cancelled work are retained. Retained children resume by identity after restart. Cleanup confirmation now uses child-owned routed requests, with late-policy/edit checks and cancellation cleanup. Explicit user isolated profiles now create and set up checkouts in all six Modes without widening child tool permissions; source/ancestor denies and read-only sandbox policy still refuse creation. Git-valid child names use the Session naming limit independently of storage names. Acknowledged clean removals now recreate with a new durable child binding and setup-ready gate, preserving the original base, history and branch; missing/foreign/force/incomplete evidence refuses recreation. Existing primary forks and user subtasks resume without granting fresh primary spawning. Parent-scoped API/SDK setup inspection and explicit retry now preserve successful steps, require a reviewed journal revision/recipe and fence late acknowledgements; completed journals can reconcile missing readiness without inference. Verified pre-launch preparation failures are recorded distinctly and support reviewed retry; generic/launch errors and missing acknowledgements remain unknown. Session/history streams now follow the durable Location timeline; old/new listeners receive rebound notices and the TUI applies the new action scope without allowing stale snapshots to restore the old path. Native acceptance, unknown-outcome recovery resolution and full profile configuration snapshots remain open. Child-thread API/SDK listing and TUI `/agent`/`/subagents` browsing now have local pagination, status, parent navigation and active-child steer coverage; idle child prompts now reset structured attempts atomically under exclusive execution ownership, with result-owner refusal and replay coverage; public continuation now recreates acknowledged clean removals, gates inference on source setup and owns cleanup/reporting through approval, interruption and shutdown. Local tests cover all Modes, denies, read-only refusal, edit preservation and durable unknown-settlement refusal. Explicit child inbox wake and held release now prepare verified checkouts and reset attempts atomically while preserving original input identity, edits and queue order; API/SDK wake is available. Requested child input now hands off between structured attempts after foreground collection or durable Job result/billing settlement; interruption pauses automatic intent without deleting rows. Existing-child preparation is now owned through cancellation and admission, with a durable interruption boundary, no late input/release and replay refusal for unacknowledged effects; native tests cover interrupt, disposal and shutdown locally. Fresh child launches now capture source/ancestor authority before approval/FIFO, verify native creation retries and recheck durable bindings in the writer; source interruption and explicit ancestor fences reject stale creation without child history. Delegation and owned child-preparation callbacks now retain captured authority through native awaits; writer fences cover reservation/launch, scoped input/reset and Job handoff, including request cancellation during Location admission. Missing owners cannot advance Reserved launch markers, and registered Jobs preserve ordinary parent-interrupt independence. Direct subtask callbacks now share tracked durable ownership, retain native cancellation acknowledgement after caller disposal and require reply acceptance before transferring Job ownership. Rejected handoffs stop verified Jobs; missing cancellation ownership records durable Unknown and refuses successful cancellation. Durable scope closure now refuses fresh capture and unbound writer admission across closed source/target ancestry, survives restart and later generation fences, and gates native Location waits while preserving terminal Job and shell receipts. The complete bounded actor sweep, tracked idle-native cancellation, matched reopening, detached/cross-process ownership, generic prompt admission, recovery resolution and native acceptance remain open. Public removal/prune, client attachment and process-death recovery, automatic cleanup, snapshot-safe enter/exit, recovery resolution, TUI output/artifacts and complete subagent thread continuation remain open.
3. **M1.3 Extensibility:** **Status: in progress** (change `implement-m1-3-extensibility`). Typed hook configuration/event/handler validation is implemented locally with selector, timeout and checkout-trust tests. Loaded global/project/local hook groups now accumulate at top level and within selected profiles, retaining indexed file origins; managed/plugin collection, final execution scope order and managed-only authority remain open. Canonical handler digests, separate durable approvals in the shared checkout trust store and temporary invocation approval sets are implemented locally with cross-process storage tests. The scope-aware catalog orders resolved contributions and exposes uncached trust/sandbox requirements; CLI list/trust/untrust supports exact-digest review and obsolete revocation. Compiled subject/path/condition selectors, event-aware ordered decision parsing/merging and immutable envelope assembly are implemented locally; built-in tool command dispatch now captures invocation identity, chains rewrites and validates final input before permissions. Remaining lifecycle identity/emission, full target extraction, effectful scheduling and other handler types remain open. Owned command stream capture now supports event stdin, bounded output and explicit stop acknowledgement locally. Command result interpretation has local contract and real-process tests for JSON decisions, exit codes, timeout policy and unknown termination; Noninteractive Unix command launch has local trust-revocation, event-input/environment, mandatory sandbox and scratch-ownership tests. Recorded Unix command execution now has local durable admission/receipt, privacy and tracked subtree cancellation tests. Unix built-in PreToolUse/PostToolUse/PostToolUseFailure command dispatch now has local denial, rewrite, permission, trust and receipt tests, plus transient TUI/exec notices. Command PermissionRequest hooks now answer individual asks without saved approvals and retain hard denials/Mode limits. Fixed-input request/post events now use bounded command concurrency with immediate slot reuse, declared-order merging and acknowledged sibling cancellation; rewrite-dependent pre-tool scheduling remains open. Command once-per-Session claims now persist atomically with receipt admission and survive concurrent runtimes, restart and unknown settlement; status/completion messages are user-only notices. Windows event-input launch, other lifecycle/remote-tool dispatch, recovery reconciliation and complete client observability remain open. Withheld-definition/TUI review and exec trust integration, all remaining lifecycle events and client controls, JSON-RPC plugin host + `@cyber-code/plugin`, MCP OAuth + deferred tool search remain open.
4. **M1.4 Memory & intelligence:** auto-memory, LSP diagnostics feedback, formatters, isolated browser verification with revision-linked artifacts.
5. **M1.5 Migration path:** `cyber import claude|codex|opencode`, ACP for Zed/JetBrains, VS Code extension.

Matched subtree reopening now verifies the reviewed close scope and latest acknowledged stop receipt, fresh Session/Job/operation evidence and retained native Location ownership in the writer transaction. Stop sweeps have durable start/settlement ownership; pending or orphaned sweeps refuse reopening. HTTP and the generated SDK expose the reviewed operation. Nested closures and old-ticket revocation survive; preserved input is not dispatched automatically. Reviewed unknown-effect recovery, complete cross-process actor cancellation, exec/TUI and budget-triggered adoption and native acceptance remain required.

Retained native settlement now keeps checkout ownership through a consuming commit and refuses hosts without explicit proof support. Local tests cover native lock retention, failure, guard disposal and production-host removal fencing. The retained-settlement primitive supplies native proof to the matched reopening transaction.

The explicit local subtree-stop report is now available through authenticated HTTP and the generated TypeScript SDK. Unknown observations remain explicit, idempotency preserves the original report and admission stays closed. Reviewed recovery, complete cross-process actor cancellation, exec/TUI and budget-triggered adoption remain open; see [P1 status](docs/implementation/p1-status.md#explicit-subtree-stop-http-and-sdk).

Current M1.1 work adds invocation-specific Windows AppContainer profiles, direct-object ACL leases and suspended launch with exact identity verification. Native profile/ACL and launch test steps pass at `043bbb1`, including scoped file access, private Temp data, explicit standard streams, loopback denial and live descendant cleanup. The full CI matrix passes at `043bbb1`; owned async wait cancellation and exit-code preservation also pass native Windows and the full CI matrix at `48b5b3f`. Single-use identities and post-start grant sealing pass native Windows and the full CI matrix at `177694c`. Ancestor-pinned ACL preparation passes all eight native profile/ACL tests and all ten launch tests, with the full CI matrix passing at `4c86024`. Relocatable file-identity ACL leases and stored-descriptor preservation pass twelve native profile/ACL tests, ten launch tests and the full CI matrix at `86d2a3a`. Catalog/current-location credential environment filtering passes native tool tests and the full CI matrix at `b467b03`. Recursive preflight and existing-object grant rollback/retry pass twenty native profile/ACL tests, eleven launch tests and the full CI matrix at `9ee3d5b`. Existing-object exclusion rules pass twenty-two native profile/ACL tests, twelve launch tests and the full CI matrix at `3cfee75`; the broad package-allowance regression fails at `d3803b4` because an excluded operation was allowed. At `ca06249`, twenty-five native profile/ACL tests and twelve launch tests pass, including overlapping roots; diagnostics identify the broad-permission bypass as a protected-file write. LPAC opt-out passes that regression at `5dc5c50`, but capability-free Winsock startup and descendant creation fail. Fixed registryRead controls pass at `f57d9c9`; the default fixed-capability policy and exact suspended-token verification pass their native capability and launch/isolation steps at `44f95be`. Its Windows job is still running, with long-root Git status failing; full Windows scope enforcement remains unaccepted. Built-in tool integration, complete recursive roots/exclusions, credential-file isolation, proxy-only networking and crash recovery remain open. The runtime/tool-host ownership-cycle regression passes the full native Windows job at `2bfca16`.

**Exit criteria**
- With the sandbox enabled, a `bypass`-mode session cannot write outside writable roots or reach non-allowlisted hosts (escape test suite).
- `cyber import claude` reproduces a reference Claude Code setup (agents, skills, MCP, hooks) with no manual edits.
- Publish SWE-bench Verified, Terminal-Bench and internal-suite comparisons with exact harness/model versions, budgets and trial counts. Meet the versioned internal quality gate; comparative parity remains a measured product objective, not an unqualified claim.

The [P1 acceptance audit](docs/implementation/p1-acceptance-audit.md) lists the 225 canonical P1 requirements and remaining delivery areas. No P1 milestone is fully accepted. Production auto-mode tool classification is implemented locally; ancestor review acceptance and complete permission/native enforcement remain priorities.

## P2: Autonomy & orchestration (flagship)

**Capabilities:** `workflows` (including automatic orchestration), `goals`, `loops-scheduling`, `agent-teams` (behind feature `teams`), `cross-session-messaging` (direct peers), `tui` (agent view, workflow monitor, goal panel), `background-tasks` (`notify`, `send_file`), `skills-commands` (bundled `batch`, `review`, `audit`), `exec-mode` (`--goal`, `--workflow`).

**Milestones**
1. **M2.1 Workflow runtime:** QuickJS sandbox, `agent()/parallel()/pipeline()/phase()`, determinism guards, event-sourced resume, budgets, model tiers (`fast` / `smart` / `verify`).
2. **M2.2 Bundled workflows:** review, audit (find → verify), migrate (fan-out with worktrees), research (cross-checked), plan-from-angles.
3. **M2.3 Goals:** active goal + goal queue, evaluator verdicts, `check` commands, budgets, `exec --goal` exit codes.
4. **M2.4 Loops & cron:** `/loop` fixed and self-paced, in-session cron, daemon-backed persistent loops.
5. **M2.5 Agent view & teams:** `cyber agents` dashboard; experimental lead/teammates with a shared task list.

**Exit criteria**
- A 300-file migration workflow finishes unattended, with each agent in its own worktree, under a declared reserved cost budget with uncertainty and any billing overrun disclosed. A killed run replays recorded host results and resumes incomplete child Sessions, reconciling unknown tool outcomes before further mutations.
- A queue of 3 goals runs to completion with `check: cargo test`, and the queue advances automatically.
- Mixed-model workflow: the fan-out runs on a local/cheap model and verification on a frontier model, with cost reported per tier.

## P3: Connectivity

**Capabilities:** `cyber-account` (including automation tokens), `remote-control`, `web-client` (Relay bundle), `cross-session-messaging` (cross-machine), `runners-cloud` (including `cyber apply`, best-of-N attempts), `workflows` and `agents-subagents` (remote agents), `agent-teams` (distributed teammates), `routines`, `channels`, `session-sharing`, `org-policy`, `vcs-integration` (GitHub App/Action, auto-fix PRs), `server-api` (Bearer account tokens, remote rate limits), `client-sdk` (Python).

**Milestones**
1. **M3.1 Cyber Account:** `cyber login` via CyberdyneAuth (OIDC code + PKCE, loopback redirect), keyring storage, refresh serialized across processes, `whoami`, entitlements and org claims.
2. **M3.2 Relay + remote control:** outbound WebSocket relay, E2E encryption (X25519), QR device pairing, device list/revoke, push notifications, web client; `cyber relay serve` for self-hosting.
3. **M3.3 Cross-machine messaging:** relay transport, approval by default, org opt-in.
4. **M3.4 Runners:** orchestrator + self-hosted runner pools, environments, session identity tokens; `--cloud`, `teleport` (remote → local) and **`handoff` (live local → remote)**.
5. **M3.5 Automation:** routines (schedule / API / GitHub / webhook triggers), channels (webhook, MCP channel, chat bridges), GitHub App/Action + auto-fix PRs.
6. **M3.6 Sharing & policy:** share service (private / org / public), redaction, server-managed org policy and spend limits.

**Exit criteria**
- A phone can approve a permission request for a laptop session through a relay that only ever sees ciphertext (verified by a relay-side capture test).
- A live session hands off to a self-hosted runner and continues with verified history and working-tree state, then teleports back. Lost acknowledgements and network partitions never authorize two owners; in-flight effects are reconciled before failover.
- Every hosted service verifies tokens via discovery + JWKS. Token rotation and an issuer change in staging cause zero downtime.

## P4: Ecosystem

**Capabilities and extras:** `plugins-marketplace` (marketplaces, dependencies, evals, signing, mods UI panes), `web-client` (desktop application), `editor-integration` (JetBrains plugin, deep links), `session-sharing` (comments, artifact pages), `tui` (voice dictation, mods panes), `agent-teams` (tmux/iTerm panes), mobile apps (iOS/Android) on the remote-control API, `observability-costs` (org analytics dashboards), workflow sharing in the marketplace.

---

## Cross-cutting dependencies

### CyberdyneAuth (identity provider)

These items need changes or configuration in [CyberdyneCorp/CyberdyneAuth](https://github.com/CyberdyneCorp/CyberdyneAuth):

| # | Item | Needed by | Status |
|---|---|---|---|
| A1 | Register public OAuth client `cyber-cli` (grant types `authorization_code`, `refresh_token`; loopback redirect `http://127.0.0.1/callback`, any port) | P3 M3.1 | config only (supported today) |
| A2 | Add scopes `cyber:relay`, `cyber:runner`, `cyber:share`, `cyber:messaging` to the scope catalog | P3 M3.1 | small change |
| A3 | Register entitlement product `cyber-code` (plans `free`, `pro`, `team`) and Stripe mapping | P3 hosted services | config + billing |
| A4 | Confidential clients for hosted services (relay, share, orchestrator) with `access_token_audience`; client-credentials service tokens for introspection | P3 M3.2–M3.6 | config only |
| A5 | **RFC 8628 device authorization grant** for SSH/headless logins | P3 nice-to-have; `--no-browser` paste flow is the fallback | **upstream feature request** |
| A6 | Optional token-exchange (RFC 8693) so runners get per-session down-scoped tokens | P3 M3.4 | upstream feature request |
| A7 | Long-lived automation tokens (`cyber tokens create`): non-rotating access tokens with selectable scopes and a 90-day maximum lifetime, revocable, excluded from step-up actions | P3 M3.1 | **upstream feature request** |

Local use never depends on CyberdyneAuth. Self-hosters can point `account.issuer` at their own CyberdyneAuth deployment.

### Cyber Cloud services (all self-hostable, same binary)

`cyber relay serve`, `cyber share serve`, `cyber orchestrator serve`, and the routine scheduler (part of the orchestrator). Each can run in Docker/Kubernetes with PostgreSQL for multi-instance control-plane state (SQLite for a single instance). Session execution remains on its owning runtime. See [storage architecture](docs/decisions/0001-storage-architecture.md) for transactional contracts, durability and the boundary between these stores.

---

## Risks & mitigations

| Risk | Mitigation |
|---|---|
| Quality gap vs vendor-tuned harnesses (Claude Code, Codex) on their own models | Per-provider base prompts, tool variants (`apply_patch` vs `edit`), and a public eval suite run on every release |
| Sandbox portability (Landlock kernel versions, Windows) | Tiered enforcement with an explicit `cyber sandbox explain`; refuse `bypass` mode without a sandbox or container |
| Workflow token blow-up | Mandatory per-run budgets, prefix-warming of fan-outs, model tiers |
| Relay trust | E2E encryption, self-hostable relay, device revocation, step-up auth (`auth_time` ≤ 15 min) for destructive remote approvals |
| Refresh-token rotation races (replay defense revokes the chain) | A cross-process lock around refresh plus a single token broker in the `cyber` service |
| Scope creep | Phase gates; the features registry (`features.*`, e.g. `teams`) for anything not yet specified as stable |

## Open questions

1. Hosted offering pricing: which features does the `cyber-code` entitlement gate on Cyber Cloud? Self-hosted is always ungated.
2. Should workflows also be authorable in Python (Pyodide/RustPython), or stay JS/TS-only?
3. Mobile: native apps or a PWA on the relay web client first?
4. Plugin signing: is sigstore required for the official marketplace?
5. Should CyberdyneAuth add the device grant (A5) before P3, or ship P3 with the paste flow only?

---

## Requirement inventory

The table counts requirements in `openspec/specs` by phase tag (47 capabilities, 0 untagged). It is generated from the first phase tag of each requirement; use `python3 scripts/spec_inventory.py` to refresh it. Later-phase extensions inside a requirement do not add another count.

| Capability | P0 | P1 | P2 | P3 | P4 | Total |
|---|--:|--:|--:|--:|--:|--:|
| `agent-teams` |  |  | 15 | 1 | 1 | 17 |
| `agents-subagents` | 8 | 14 |  | 1 |  | 23 |
| `background-tasks` |  | 15 |  |  |  | 15 |
| `browser-verification` |  | 2 |  |  |  | 2 |
| `builtin-tools` | 16 | 4 | 1 |  |  | 21 |
| `channels` |  |  |  | 16 |  | 16 |
| `cli-commands` | 18 |  |  | 2 |  | 20 |
| `client-sdk` | 14 | 1 | 2 | 1 |  | 18 |
| `code-intelligence` |  | 12 |  |  | 1 | 13 |
| `compaction` | 15 | 3 | 1 |  |  | 19 |
| `compat-import` | 1 | 10 | 1 |  |  | 12 |
| `configuration` | 18 | 2 |  |  |  | 20 |
| `cross-session-messaging` |  | 18 | 1 | 1 |  | 20 |
| `cyber-account` |  |  |  | 17 |  | 17 |
| `editor-integration` |  | 13 |  | 1 | 1 | 15 |
| `exec-mode` | 11 | 4 | 2 | 1 |  | 18 |
| `goals` |  |  | 21 |  |  | 21 |
| `harness-evaluation` | 4 |  |  |  |  | 4 |
| `hooks` |  | 20 | 2 |  |  | 22 |
| `installation-upgrade` | 12 | 1 |  |  |  | 13 |
| `loops-scheduling` |  |  | 14 | 1 |  | 15 |
| `mcp` | 14 | 6 | 2 |  |  | 22 |
| `memory` |  | 13 |  | 1 |  | 14 |
| `observability-costs` | 8 | 10 |  | 1 |  | 19 |
| `org-policy` |  |  |  | 15 |  | 15 |
| `permissions-modes` | 15 | 7 |  |  |  | 22 |
| `plugins-marketplace` |  | 14 | 2 |  | 6 | 22 |
| `provider-catalog` | 17 | 4 | 2 |  |  | 23 |
| `provider-credentials` | 12 | 3 |  | 2 |  | 17 |
| `remote-control` |  |  |  | 17 | 1 | 18 |
| `routines` |  |  |  | 16 |  | 16 |
| `runners-cloud` |  |  |  | 22 |  | 22 |
| `sandbox` | 7 | 8 |  | 1 |  | 16 |
| `server-api` | 22 |  |  | 2 |  | 24 |
| `session-runtime` | 23 | 3 |  |  |  | 26 |
| `session-sharing` |  |  |  | 10 | 2 | 12 |
| `skills-commands` | 14 | 5 | 1 |  |  | 20 |
| `snapshots-checkpoints` | 12 | 3 | 1 |  |  | 16 |
| `storage-events` | 17 | 2 |  | 2 |  | 21 |
| `system-context` | 12 | 5 | 1 |  |  | 18 |
| `tool-registry` | 15 | 3 |  |  |  | 18 |
| `tui` | 19 | 7 | 3 | 1 | 1 | 31 |
| `vcs-integration` | 2 | 3 |  | 10 |  | 15 |
| `web-client` |  | 1 |  | 1 | 1 | 3 |
| `workflows` |  |  | 26 | 2 | 1 | 29 |
| `workspace-trust` | 4 |  |  |  |  | 4 |
| `worktrees` |  | 9 | 4 |  |  | 13 |
| **Total** | **330** | **225** | **102** | **145** | **15** | **817** |


Most capabilities span phases. A spec is a product contract, and each phase implements the slice tagged for it. For example, `tui` has its basics in P0, rewind and modes in P1, the agent view and workflow monitor in P2, remote indicators in P3 and mods panes in P4.

### Cross-spec contracts reconciled

These were fixed after the parallel authoring pass. Keep them consistent in future changes:

- **Storage and recovery**: SQLite WAL/FULL locally, one writer owner, explicit unknown tool outcomes; PostgreSQL for multi-instance hosted coordination.
- **Trust**: checkout-scoped definition digests; project configuration and saved approvals cannot widen user/global deny ceilings.
- **Modes and models**: mode switches take effect next Turn with `session.mode.switched.1`; role defaults come from `configuration`; unknown prices are `null`/`unpriced`.
- **Exit codes** (every command): 0 ok · 1 runtime · 2 usage/config · 3 goal impossible · 4 budget/turn/timeout · 5 permission denied or approval needed non-interactively · 6 Cyber Account/entitlement required · 130 SIGINT.
- **REST paths** use plural collections (`/api/v1/sessions/{id}`, `/permissions`, `/workflows/runs`, `/channels`, `/routines`, `/runners`, `/agents`). Singletons and streams stay singular (`/health`, `/config`, `/account`, `/event`, `/fs`, `/messaging`, `/ws`).
- **Default server port** 4747 (avoids OpenCode's 4096); `0` = random.
- **Step-up authentication**: `auth_time` must be ≤ 15 min (`prompt=login`, `max_age=900`) for device pairing, runner registration, org routines and remote approval of destructive actions.
- **Model-facing tools** owned by other capabilities are indexed in `builtin-tools` → "Capability-owned tool catalog".
- **Hook events** include worktree, job, goal, schedule and teammate events (`hooks` spec).
- **Config top-level keys** include `background`, `tools`, `git`, `ide`, `autofix`, `network`, `storage`, `compat`, `services`, `budgets`, `profiles` and `default_profile` (`configuration` spec).
- **Process model**: service-first. The TUI, `cyber exec` and SDKs talk to the one registered server; `--embedded` runs a private server on a private database and never touches the shared writer lock.
- **Planning is a Mode** (`plan`), entered and left with the `plan_enter`/`plan_exit` tools; there is no `plan` agent. Built-in agents are `build`, `explore`, `general` plus hidden system agents.
- **Protected `.cyber/` paths** are the configuration documents (`cyber.jsonc`, `hooks.jsonc`, `mcp.json`, plugin manifests and lockfile). Content directories (`plans/`, `workflows/`, `agents/`, `skills/`, `commands/`, `teams/`, `output-styles/`) are writable.
- **Registries**: the command tree (`cli-commands`), route groups (`server-api`) and model-facing tool names (`builtin-tools`) are authoritative; `scripts/spec_lint.py` fails on undeclared entries.
- **One Budget object** `{ max_turns, max_tokens, max_cost_usd, max_wall_seconds, enforcement }` and flags `--max-turns/--max-tokens/--max-cost/--timeout` everywhere (`observability-costs`); defaults under `budgets.<scope>`.
- **Flags**: `--format` is the only output-format flag and `--cwd` the only working-directory flag. Session commands live under `cyber sessions ...`; `cyber import` imports other tools' setups only.
- **Held items** of any origin are released or dropped through `POST /api/v1/sessions/{id}/inbox/{message_id}/release|drop`.
- **Org mode ladder** `plan < default < dont-ask < accept-edits < auto < bypass`; unpriced usage counts as zero toward spend unless `spend.block_unpriced`.
- **ID prefixes**: `ses_` Session, `per_` permission request, `msgx_` cross-session message, `job_` background job, `agt_` workflow agent call, `run_` Workflow Run, `rrn_` Routine run, `rtn_` Routine, `gol_` Goal, `lop_` Loop, `cron_` scheduled task, `team_`/`task_` teams, `dev_` Device, `rnr_` Runner, `chn_` Channel.

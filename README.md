# Cyber Code (`cyber`)

> A model-independent coding agent with durable local sessions, repository tools, a terminal UI and a public server API.

Cyber Code is in **P0 (Local core)**. All five implementation milestones, M0.1–M0.5, are built; **P0 is not closed**. The remaining exit gate is a complete local-model coding baseline, including the long task. A Qwen3.5 9B pilot has passed one small coding task through Ollama's OpenAI-compatible endpoint; that is not a full baseline.

Implemented today:

- OpenAI Responses, Anthropic Messages and OpenAI-compatible Chat adapters.
- Durable prompt admission, streaming tool loops, interrupt/resume, crash recovery and compaction.
- Built-in repository and web tools, permission rules, macOS/Linux sandboxing, snapshots and conflict-aware restore.
- A background server, HTTP/SSE/WebSocket/stdio API, `cyber exec`, the TUI and a generated TypeScript SDK.
- Database-plus-artifact backup, verification, restore, retention and logs.

Recovery/trust tests, per-tool goldens, storage measurements and all six macOS/Linux build targets have passing evidence. Default-service startup meets the 150 ms first-frame target on the named M2 Max machine; the embedded-mode measurements retain an outlier. See [P0 exit evidence](docs/measurements/p0-exit-evidence.md) and [evaluation results and local-model setup](eval/README.md).

**In progress:** P1 implementation is authorized and starts with M1.1, covering Windows sandbox enforcement, network-policy integration and mode cycling with an auto-mode classifier. The evaluator runtime foundation, Bash removal guards, four-mode TUI cycling and effective/pending mode display are implemented. Classifier approval integration and native Windows enforcement remain open while P0 local-model validation continues. See [P1 implementation status](docs/implementation/p1-status.md). Workflows, goals, loops, remote control, cloud runners and additional clients are later roadmap work. Deferred PTY routes, Landlock fallback and parts of the TUI are listed in the roadmap and are not required to exit P0.

The [roadmap](ROADMAP.md) tracks delivery and deferrals. The [OpenSpec contracts](openspec/specs) describe both implemented and planned behavior, tagged by phase. [Architecture decisions](docs/decisions/0001-storage-architecture.md) record design rationale.

## Building

Common tasks are in the [`justfile`](justfile) (install [`just`](https://github.com/casey/just)); run `just` to list them:

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
cargo run -p cyber-cli -- service status   # the background server; also start|stop|restart|password
cargo run -p cyber-cli -- api v1.session.list                    # any API operation
cargo run -p cyber-cli -- doctor           # config, keys, catalog, database, sandbox, tools, server
cargo run -p cyber-cli -- db backup ~/cyber-backup --artifacts   # online backup bundle with a manifest
cargo run -p cyber-cli -- debug info       # build, paths, database, config layers, trust
cargo run -p cyber-cli -- trust inspect    # repository-controlled definitions awaiting approval
cargo run -p cyber-cli -- models openai    # catalog: available models first

# one real turn with a tool call (uses OPENAI_API_KEY)
cargo run -p cyber-llm --example turn -- openai/gpt-6-luna "What time is it in UTC?"
CYBER_LIVE_TESTS=1 cargo test -p cyber-llm --test live   # live provider checks

# a durable session with a tool loop, title and cost (uses OPENAI_API_KEY)
cargo run -p cyber-server --example session -- openai/gpt-6-luna

# a coding task with the built-in tools, the OS sandbox and snapshots (uses OPENAI_API_KEY)
cargo run -p cyber-tools --example agent -- openai/gpt-6-luna path/to/repo "Fix the failing test" accept-edits
```

On Linux the sandbox needs bubblewrap (`apt install bubblewrap`); without it commands fail closed unless you pass `--sandbox full-access`.

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

The TypeScript SDK `@cyber-code/sdk` lives in [`sdk/typescript`](sdk/typescript/README.md). It is generated from [`sdk/openapi.json`](sdk/openapi.json) by `scripts/generate_sdk.py`; regenerate the document with `UPDATE_OPENAPI=1 cargo test -p cyber-server --test http openapi_document_is_current`.

Evaluation fixtures and their manifests live in [`eval/`](eval/README.md).

```bash
openspec list --specs
openspec validate --specs --strict
python3 scripts/spec_lint.py      # cross-spec registries: commands, routes, prefixes, phases
python3 scripts/spec_inventory.py # refresh the ROADMAP requirement table
```

---

## Design principles

1. **Provider independence.** The local core supports OpenAI Responses, Anthropic Messages and compatible Chat endpoints, including local servers. Additional native adapters and mixed-model workflows are planned.
2. **Local-first.** Local state and local-model execution do not require a Cyber account. Hosted model calls require connectivity and provider credentials. Cyber Account and hosted services are planned.
3. **Server-first.** The TUI, `exec` and TypeScript SDK share the public server API. IDE, web and mobile clients are planned.
4. **Durable execution.** Prompt admission, tool calls and context changes are recorded before execution proceeds. Recovery handles interrupted streams and uncertain tool outcomes explicitly; file-backed storage uses FULL durability under the documented filesystem assumptions.
5. **Controlled execution.** Permission rules, protected paths, an OS sandbox and snapshots constrain repository work. Windows enforcement and the auto-mode classifier are planned for M1.1.
6. **Familiar conventions.** Repository instructions and skills use familiar `AGENTS.md`/`CLAUDE.md` and `SKILL.md` conventions. MCP integration and setup import are later roadmap work.

## Current architecture

```text
 TUI · cyber exec · TypeScript SDK
                  │ HTTP / SSE / WebSocket / stdio JSON-RPC
                  ▼
 cyber server (Rust, one background service per user)
   config · trust · model catalog · tools · permissions
   durable inbox → Drains → Turns · compaction · snapshots
                  │
         ┌────────┼──────────────────┐
         ▼        ▼                  ▼
    SQLite WAL  sandbox +        model adapters
    event store shadow-git       OpenAI · Anthropic ·
                snapshots        OpenAI-compatible endpoints
```

**Stack:** Rust with tokio, axum, rusqlite and ratatui. SQLite is bundled. Linux sandbox builds also ship the `cyber-sandbox-exec` helper; see the [platform build matrix](.github/workflows/ci.yml). The generated TypeScript SDK is available now; plugin kits and generated Rust/Python SDKs are planned.

SQLite owns local execution state. PostgreSQL is a design choice for later hosted control-plane services, not a dependency of the local core. See [Storage architecture](docs/decisions/0001-storage-architecture.md).

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

These terms describe the product contracts. Workflows, goals, loops, runners, Relay, devices, channels and Cyber Account are planned capabilities. Subagent execution and the auto-mode classifier are also future work; this table is not a list of shipped features.

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

This is the full product scope, spanning P0–P4. See the [roadmap](ROADMAP.md) for each capability’s delivery phase; it is not an availability matrix.

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

The local storage decision is SQLite WAL with FULL synchronization and one writer owner. PostgreSQL supports later hosted services with multiple active instances. See the [storage decision](docs/decisions/0001-storage-architecture.md) for alternatives and benchmark criteria.

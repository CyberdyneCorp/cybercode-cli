# Cyber Code (`cyber`)

> A dependable, model-independent coding agent you can interrupt, inspect and resume. It brings the best of **OpenCode** (open server, any provider, durable runtime), **Codex** (fast Rust binary, strong sandbox, ergonomic CLI) and **Claude Code** (workflows, goals, loops, remote control, cross-session messaging, cloud runners) into one tool.

This folder is the **OpenSpec** for the product: `openspec/specs/<capability>/spec.md` is the source of truth. Delivery order is in [`ROADMAP.md`](ROADMAP.md). Architecture decisions are in [`docs/decisions`](docs/decisions/0001-storage-architecture.md). This repository specifies a target product; runtime implementation is pending.

```bash
openspec list --specs
openspec validate --specs --strict
python3 scripts/spec_lint.py      # cross-spec registries: commands, routes, prefixes, phases
python3 scripts/spec_inventory.py # refresh the ROADMAP requirement table
```

---

## Principles

1. **Any model, no lock-in.** Native adapters for OpenAI Responses, Chat Completions-compatible endpoints, Anthropic Messages, Gemini, Bedrock, Vertex, Azure and local servers (Ollama, llama.cpp, vLLM). A single workflow can mix models.
2. **Local-first, account-optional.** Everything that runs on one machine works offline and without login. A Cyber Account (CyberdyneAuth) is required only for networked features.
3. **Server-first.** One `cyber` server per user serves many projects. TUI, `exec`, IDE, web, mobile and SDK clients all use the same public OpenAPI.
4. **Durable by default.** Every model-visible fact is persisted before it is acted on: prompt admission, tool calls, context changes, workflow agent results. Acknowledged state survives process crashes; file-backed storage uses FULL durability under documented filesystem assumptions. Unrecorded stream fragments and uncertain external side effects follow explicit recovery rules.
5. **Safe to leave unattended.** Permission modes, an OS sandbox, protected paths and budgets make background, looped and workflow work safe.
6. **Compatible.** Reads `AGENTS.md`/`CLAUDE.md`, `SKILL.md` folders and MCP config, and can import Claude Code, Codex and OpenCode setups.

## Architecture

```
 clients:  TUI · cyber exec · Web (local, Relay, desktop) · IDE (ACP / VS Code) · Mobile · SDK (TS/Rust/Py) · GitHub App
              │  HTTP + SSE + WebSocket (OpenAPI)          ▲ remote clients via Relay (E2E encrypted)
 ┌────────────▼──────────────────────────────────────────────────────────────────────────────┐
 │ cyber server (Rust, one per user; `cyber service`)                                         │
 │  Location services: config · agents · skills · commands · catalog · tools · permissions    │
 │  Session runtime: durable inbox → Drains → Turns · Context Epochs · compaction · snapshots   │
 │  Orchestration: subagents · Workflow Runs (QuickJS) · Goals · Loops · Teams · messaging     │
 │  Execution: sandbox · PTY · background tasks · LSP · formatters · MCP clients              │
 │  Extensibility: hooks · plugin host (JSON-RPC, out-of-process) · channels                  │
 │  Storage: SQLite (WAL) · append-only event store · git snapshot repos                       │
 └───────┬──────────────────────────────┬────────────────────────────────┬───────────────────┘
         │ LLM providers (any)           │ CyberdyneAuth (OIDC)            │ Cyber Cloud (optional,
         ▼                               ▼                                 ▼  self-hostable)
   OpenAI · Anthropic · Gemini ·   login · entitlements · orgs ·      Relay · Share service ·
   Bedrock · Vertex · Ollama · …   roles · managed policy             Runner orchestrator · Routines
```

Storage rationale and alternatives: [Storage architecture](docs/decisions/0001-storage-architecture.md). SQLite owns local execution state; PostgreSQL supports shared hosted control-plane state. The roadmap describes target behavior, not shipped capabilities.

**Stack:** Rust (tokio, axum, rusqlite, rquickjs) in one static binary `cyber`. Plugins are out-of-process over JSON-RPC 2.0 on stdio, with a TypeScript kit `@cyber-code/plugin`. SDKs are `@cyber-code/sdk` (TS) and the `cyber-sdk` crate (Rust), both generated from OpenAPI.

## Naming

| Thing | Value |
|---|---|
| Binary | `cyber` |
| Project config | `cyber.jsonc` / `cyber.json` at any level, and `.cyber/` directories |
| Global config | `$XDG_CONFIG_HOME/cyber` (`~/.config/cyber`), overridable by `CYBER_CONFIG_DIR` |
| Data / state / cache | `~/.local/share/cyber` · `~/.local/state/cyber` · `~/.cache/cyber` |
| Database | `<data>/cyber.db` |
| Env prefix | `CYBER_` |
| OAuth client | public client `cyber-cli` registered in CyberdyneAuth |
| Entitlement product key | `cyber-code` (plans e.g. `cyber-code:pro`, `cyber-code:team`) |

## Glossary

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

## Capability map

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

# Cyber Code (`cyber`)

A model-independent coding agent with a terminal UI, local web dashboard and durable background server. Run one task, coordinate agents, or compare patches with a verified workflow—all against the same local Sessions.

**Development status:** the local core and first web dashboard are implemented. Broader platform, integration and release acceptance is still in progress; see [Known gaps](#known-gaps).

## Quickstart

Requires **Rust 1.89+**; SQLite is bundled. On Linux, install **bubblewrap** for sandboxed commands (`sudo apt install bubblewrap`).

```bash
cargo build --locked -p cyber-cli
export OPENAI_API_KEY="your-key"
./target/debug/cyber
```

OpenAI, Anthropic, GitHub Copilot and OpenAI-compatible endpoints are supported, including local model servers. Instead of an API key, you can use `cyber providers login chatgpt` or `cyber providers login copilot` with a supported subscription. See [provider setup](docs/local-core-guide.md#signing-in-with-a-chatgpt-subscription) and [Copilot limits](docs/local-core-guide.md#signing-in-with-a-github-copilot-subscription).

Examples below use the built binary. If you installed it on your PATH, use `cyber` instead of `./target/debug/cyber`.

## Run the web dashboard

```bash
./target/debug/cyber service password  # copy the printed login password
./target/debug/cyber web               # start/reuse the server and open your browser
```

Enter the password on the login page. The dashboard shares Sessions with the terminal: read transcripts, send prompts, approve permissions, answer questions, and monitor or control Workflows, Jobs and Goals.

To open it manually, follow the URL printed by:

```bash
./target/debug/cyber web --no-open
```

The password grants **owner access** and stays in page memory. Off-loopback access requires HTTPS; a user-managed SSH forward is also supported. See the [dashboard guide](docs/web-dashboard.md) for remote setup, disconnect behavior and desktop/mobile screenshots. Full Settings, attachment previews and terminal emulation are outside this first browser slice.

## Common commands

```bash
./target/debug/cyber exec "fix the failing test"    # one task
./target/debug/cyber review main..HEAD             # read-only code review
./target/debug/cyber --continue                    # resume the latest Session
./target/debug/cyber exec --file shot.png "explain this screenshot"
./target/debug/cyber workflows list                # bundled and saved Workflows
./target/debug/cyber workflows run audit --wait    # run a bundled audit
./target/debug/cyber service status                # inspect the shared server
./target/debug/cyber doctor                        # check your setup
```

The server owns execution and stores state in SQLite, so work can outlive a terminal window. After a crash, Sessions recover interrupted state and Workflows can replay; see [server lifecycle and recovery](docs/local-core-guide.md#background-server-and-rebuilds).

For independent patch attempts, host-recorded tests and a separate read-only judge, start with the [verified `solve` workflow](docs/verified-workflows.md). Candidate patches remain available for human review; nothing is merged automatically.

## Sessions and people working together

Cyber supports communication between Sessions and between different people's agents working on the same Git project:

- **On one server:** Sessions and agents can discover, message and watch each other. See [Session messaging](docs/local-core-guide.md#cross-session-messaging).
- **Across your machines:** [direct peers](docs/peers.md) connect Sessions without an account or relay.
- **With teammates or friends:** a [self-hosted project hub](docs/collaboration.md) connects verified members and their explicitly published agents, with conversations and shared tasks. Each person binds their own checkout to the project; tools, working trees and execution approvals stay local. Incoming agent messages are held for receiver consent by default. The hub is **experimental and off by default**.

## Feature mindmap

This is a map of available capabilities, including partial and experimental areas. The [feature reference](docs/features.md) records their limits and acceptance status.

```mermaid
mindmap
  root((Cyber Code))
    Clients
      Terminal UI
      Exec and public API
      Local web dashboard
      TypeScript SDK
    Agents and automation
      Subagents and worktrees partial
      Workflows and verified solve
      Goals and loops
      Agent teams experimental
    Safety and recovery
      Permission modes
      macOS and Linux sandbox
      Git snapshots
      Durable Sessions and replay
    Tools and integrations
      Providers and local endpoints
      MCP client and server
      Hooks and skills
      Image and PDF input
    Collaboration
      Local Session messages
      Direct peers
      Project hub experimental
```

## Known gaps

Cyber Code has working features in each area below, but it is not at full feature parity with every alternative. Checked **2026-10-24** against official documentation; this compares capabilities, not coding quality or benchmark performance. Related desktop, IDE and hosted products are distinguished from each terminal CLI.

| Compared with | What their documentation provides | What remains in Cyber Code |
|---|---|---|
| **OpenCode** | A broad [provider catalog](https://opencode.ai/docs/providers/), [IDE integration](https://opencode.ai/docs/ide/) and [web UI](https://opencode.ai/docs/web/). | Native provider coverage is narrower; dedicated IDE/desktop clients are planned. The web dashboard is implemented, but full TUI-equivalent browser controls remain unfinished. |
| **Claude Code CLI** | The CLI is part of a [terminal/IDE/desktop/web offering](https://code.claude.com/docs/en/overview), with [plugin marketplaces](https://code.claude.com/docs/en/discover-plugins). | IDE/desktop clients, plugin marketplaces and complete cross-tool configuration/plugin compatibility remain unfinished. A plugin host, SDK, skills and hooks already exist. |
| **Codex CLI** | A [terminal coding agent](https://learn.chatgpt.com/docs/codex/cli), related IDE/cloud surfaces, and a [native Windows sandbox](https://learn.chatgpt.com/docs/windows/windows-sandbox). | Windows confinement remains partial. Dedicated editor/desktop clients, hosted runners and paired-device Relay access are planned. Private HTTPS/SSH dashboard access already works. |
| **Pi** | A deliberately small [terminal harness with TypeScript extensions, packages and an SDK](https://github.com/earendil-works/pi/tree/main/packages/coding-agent). | Cyber ships its own plugin host and SDK; package marketplaces and broader extension compatibility remain unfinished. Pi intentionally leaves features such as subagents and plan mode to extensions/packages, so its smaller core is a design choice. |

Other limits to consider:

- **Acceptance is ongoing:** the full local-model baseline, Windows enforcement and several P1 migration, memory and code-intelligence paths remain open.
- **Automation has edges:** reserved workflow budgets, finished-agent transcript retention and persistent cron-style scheduling remain unfinished; agent teams and the project hub are experimental.
- **Remote access is private:** physical-device certificate trust is an operator check. Relay, paired Devices and mobile apps remain roadmap work.

Use the [feature reference](docs/features.md), [P1 evidence](docs/implementation/p1-status.md), [P2 evidence](docs/implementation/p2-status.md) and [roadmap](ROADMAP.md) for exact scope. A planned OpenSpec requirement is not proof of implementation.

## Architecture

The TUI, web dashboard, CLI, SDK and MCP clients call one server. That server enforces permissions, schedules agents, streams events and owns durable state. See the [architecture diagrams](docs/architecture.md) and [storage decision](docs/decisions/0001-storage-architecture.md).

## Guides and development

| I want to… | Read |
|---|---|
| Configure providers, permissions, MCP or Sessions | [Local core guide](docs/local-core-guide.md) |
| Use the browser dashboard and remote approvals | [Dashboard guide](docs/web-dashboard.md) |
| Run agents across machines or a group project | [Direct peers](docs/peers.md) · [Project hub](docs/collaboration.md) |
| Build or customize a workflow | [Workflow guide](docs/local-core-guide.md#workflows-p2-m21) · [Verified solve](docs/verified-workflows.md) |
| See the interface | [Screenshots](docs/screenshots/README.md) · [Dashboard screenshots](docs/web-dashboard.md#browser-views) |
| Integrate another program | [TypeScript SDK](sdk/typescript/README.md) · [Plugin SDK](sdk/plugin/README.md) · [OpenAPI](sdk/openapi.json) |
| Find a command or planned capability | [CLI reference](docs/cli.md) · [Roadmap](ROADMAP.md) · [OpenSpec](openspec/specs) |
| Check builds, tests or evaluations | [Development guide](docs/local-core-guide.md#build-and-development) · [Evaluation guide](eval/README.md) |

Run `just` for available development tasks and `just ci` for local checks. Browse the [documentation index](docs/README.md) for the full guide set.

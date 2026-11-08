# Changelog

All notable changes to Cyber Code are recorded here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html). Before 1.0, a minor version may change behavior or configuration; such changes are listed under **Changed**.

[`docs/features.md`](docs/features.md) is the detailed reference for what each area does and what remains.

## [Unreleased]

The first release is in preparation. It will contain:

### Added

- **Agent core:** durable Sessions with steer and queue input, interrupt, retry with error classification, compaction and Context Epochs, on SQLite with backup, restore and retention.
- **Models:** OpenAI Responses, Anthropic Messages and OpenAI-compatible endpoints (including local servers such as Ollama), a models.dev catalog with an offline snapshot, usage and cost. Sign in with a ChatGPT Plus/Pro or GitHub Copilot subscription instead of an API key.
- **Reliability:** retry of interrupted streams before any tool runs, and unattended runs that wait for a subscription usage limit to reset.
- **Tools:** read, write, edit, `apply_patch`, bash (with background commands), glob, grep, web fetch and search, todo, question and skills, with output budgets.
- **Safety:** permission rules and modes (`default`, `plan`, `accept-edits`, `auto`, `dont-ask`, `bypass`), workspace trust, a sandbox on macOS and Linux (preview on Windows), secrets redacted from tool output, and framing of untrusted content against prompt injection.
- **Recovery:** Git snapshots, rewind of code, conversation or both, and staged undo/redo.
- **Surfaces:** a terminal UI, `cyber exec` for scripts and CI, a local web dashboard, an OpenAPI server with a TypeScript SDK, and an MCP server mode.
- **TUI:** command palette, themes, sidebar, file explorer, binary viewer and editor, rewind, Ctrl+R prompt history search, sound alerts, mouse support, accessibility and `NO_COLOR` modes.
- **Extensibility:** MCP (stdio and HTTP, OAuth, tool search, resources and prompts), skills and custom commands, hooks, plugins, output styles, memory, code intelligence through LSP, and import of Claude Code, Codex and OpenCode settings.
- **Orchestration:** subagents with worktree isolation, workflows with verified solve and integration queues, goals, session loops, and messaging between Sessions on one machine and between your own machines (direct peers).
- **Model fallback:** `fallback_models` re-sends a Turn that failed before any output (overload, rate limit, quota, authentication) on the next model; the Session stays on it for `fallback.sticky_minutes`.
- **Structured results:** `cyber exec --output-schema <file>` and a per-prompt JSON Schema; the validated object is the result, with up to two corrections.
- **Driving exec over stdin:** `cyber exec --input-format stream-json` for programs that send several prompts, interrupt and answer permission requests in one run.
- **Prompt history search:** Ctrl+R in the TUI composer.
- **Languages:** the TUI in English, Brazilian Portuguese, Spanish and French, with model replies in the chosen language (`language`, `/language`).
- **Install:** signed release archives for 8 targets, `install.sh`, verified `cyber upgrade` and `cyber uninstall`.

### Fixed

- A first run with no API key no longer picks an unreachable local server from the model catalog and fails with a connection error; it explains how to set up a provider.

### Experimental (off by default)

- Agent teams, the self-hosted project collaboration hub, and native computer use on macOS.

### Known limits

- Windows is a preview: sandboxed shells cannot start yet ([#9](https://github.com/CyberdyneCorp/cybercode-cli/issues/9)).
- Package managers (Homebrew, npm, Scoop, WinGet, AUR, Nix) are not published yet, and binaries are not notarized for macOS or signed for Windows.
- The hosted Relay, remote control, session sharing and IDE clients are planned, not built.

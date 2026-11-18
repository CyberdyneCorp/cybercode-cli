# Changelog

All notable changes to Cyber Code are recorded here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html). Before 1.0, a minor version may change behavior or configuration; such changes are listed under **Changed**.

[`docs/features.md`](docs/features.md) is the detailed reference for what each area does and what remains.

## [Unreleased]

### Added

- Computer use reports a platform-neutral capability report (schema version 2): a derived mode (`unsupported`, `capture_only`, `input_enabled`), stable reason codes, authority kind, honored selectors and the support evidence level, alongside the existing macOS fields. Receipts add platform, grant, target and frame references, a reason, cleanup state and timestamps; HTTP errors add a `reason` code. `cyber computer allow <session> --preview <id> --yes` confirms a printed review by its identifier.
- `cyber exec --format stream-json --include-hook-events` adds a `hook_event` line for each hook that ran for the run's Session (hook ID, event, scope, outcome, decision, duration, and the tool call it guarded), before the line of what it gated. Hook input, output, reasons and environment are never included. The instance event stream carries the underlying `session.hook.receipt` summaries when asked with `include=hook_receipts`, and so does a Session's event stream (`/sessions/{id}/events`) for that Session's own receipts, which `--input-format stream-json` runs follow.

### Changed

- A key chord that computer use refuses (for example Command+C under an application grant) is now refused with `key_chord_refused` before anything is recorded or sent, and the grant stays armed. Before, it was recorded, failed in the native adapter, settled as an unknown operation and stopped the grant. The set of allowed and refused chords on macOS is unchanged.

### Fixed

- Model-run commands no longer wait for an editor, pager or Git credential prompt: `git commit` without `-m` aborted only at the command timeout, and `git log` in a PTY waited in the pager. Shell tools, `monitor`, PTYs, background commands, worktree setup and solve checks now get `GIT_EDITOR`/`JJ_EDITOR`/`HGEDITOR`/`EDITOR`/`VISUAL=true`, `PAGER`/`GIT_PAGER`/`JJ_PAGER`/`MANPAGER=cat` and `GIT_TERMINAL_PROMPT=0`; a value set in `sandbox.env.set` is kept.

### Security

- A hook's block reason (stderr of an exit-2 command), JSON `reason`/`stop_reason` and failure stderr are redacted like tool output and cut to 4 KiB before they reach live hook notices and hook receipts; the tool result was already redacted but was unbounded.

## [0.1.0-beta.2] - 2026-11-12

The second pre-release, for macOS and Linux. Besides testing `cyber upgrade` from 0.1.0-beta.1, it adds:

### Added

- `cyber doctor --network` checks whether each usable provider and the update channel can be reached, through the same proxy and client settings as model requests, without sending credentials.

### Changed

- The permission prompt shows an edit's whole diff. When it does not fit, PgDn, PgUp and Home scroll it while the choices stay in view; before, only the first 12 lines were shown.

### Fixed

- OpenAI refusals were dropped, so a refused prompt looked like an empty answer. The refusal text is now shown, the TUI says when the provider's safety filter or the output limit stopped an answer, and `cyber exec` reports `stop_reason` `refusal` or `max_tokens`.

## [0.1.0-beta.1] - 2026-11-11

The first public pre-release, for macOS and Linux. It contains:

### Added

- **Agent core:** durable Sessions with steer and queue input, interrupt, retry with error classification, compaction and Context Epochs, on SQLite with backup, restore and retention.
- **Models:** OpenAI Responses, Anthropic Messages and OpenAI-compatible endpoints (including local servers such as Ollama), a models.dev catalog with an offline snapshot, usage and cost. Sign in with a ChatGPT Plus/Pro or GitHub Copilot subscription instead of an API key.
- **Reliability:** retry of interrupted streams before any tool runs, and unattended runs that wait for a subscription usage limit to reset.
- **Tools:** read, write, edit, `apply_patch`, bash (with background commands), glob, grep, web fetch and search, todo, question and skills, with output budgets.
- **Safety:** permission rules and modes (`default`, `plan`, `accept-edits`, `auto`, `dont-ask`, `bypass`), workspace trust, a sandbox on macOS and Linux, secrets redacted from tool output, and framing of untrusted content against prompt injection.
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
- **Install:** signed release archives for macOS and Linux (6 targets), `install.sh`, verified `cyber upgrade` and `cyber uninstall`. Windows is not part of 0.1.0: CI builds it, but no Windows archive, npm package, Scoop or WinGet manifest is published until the Windows sandbox is complete.

### Fixed

- Security hardening from the pre-release review:
  - a repository's skills no longer pre-approve tools before the checkout is trusted;
  - file tools check a symbolic link's target;
  - shell redirections with descriptors (`1>>`) count as writes;
  - the model catalog can no longer redirect OpenAI and Anthropic keys;
  - provider requests don't follow redirects;
  - read-only detection and "always" approvals are stricter;
  - the protected-path list is complete and case-insensitive on macOS and Windows.
- A first run with no API key no longer picks an unreachable local server from the model catalog and fails with a connection error; it explains how to set up a provider.

### Experimental (off by default)

- Agent teams, the self-hosted project collaboration hub, and native computer use on macOS.

### Known limits

- Windows is a preview: sandboxed shells cannot start yet ([#9](https://github.com/CyberdyneCorp/cybercode-cli/issues/9)).
- Package managers (Homebrew, npm, Scoop, WinGet, AUR, Nix) are not published yet, and binaries are not notarized for macOS or signed for Windows.
- The hosted Relay, remote control, session sharing and IDE clients are planned, not built.

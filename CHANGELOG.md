# Changelog

All notable changes to Cyber Code are recorded here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html). Before 1.0, a minor version may change behavior or configuration; such changes are listed under **Changed**.

[`docs/features.md`](docs/features.md) is the detailed reference for what each area does and what remains.

## [Unreleased]

## [0.1.0] - 2026-11-22

The first stable release, for macOS and Linux. Since 0.1.0-beta.2 it adds:

### Added

- An isolated subagent's worktree now starts from the parent checkout's uncommitted work: staged, unstaged and untracked non-ignored files, deletions and renames are committed in the child's checkout as its base, so its result still shows only its own changes (and now reports `base`). Files the Session may not read (`.env` by default) and ignored files stay behind, `.worktreeinclude` is unchanged, and `worktrees.carry_limit_mb` (default 100, `0` off) caps the total.
- Bundled skill `/grill-me <task>`: before starting, the agent interviews you one question at a time with the `question` tool, recommending an answer for each, then summarizes the agreed scope and waits for your confirmation.
- `title.command` in the global `tui.jsonc` sets the terminal window title from your own command: it receives the Session state as JSON on stdin and its first output line is the title. It runs at most every 300 ms, only when the state changed, with a one-second limit; a project `tui.jsonc` cannot set it.
- `/model <provider/model> <prompt>` in the TUI sends one prompt to another model; the Session's model answers the next prompt again. `POST /api/v1/sessions/{id}/prompt` accepts `model` for the same one-prompt override, and the prompt's inbox row reports it.
- Computer use reports a platform-neutral capability report (schema version 2): a derived mode (`unsupported`, `capture_only`, `input_enabled`), stable reason codes, authority kind, honored selectors and the support evidence level, alongside the existing macOS fields. Receipts add platform, grant, target and frame references, a reason, cleanup state and timestamps; HTTP errors add a `reason` code. `cyber computer allow <session> --preview <id> --yes` confirms a printed review by its identifier.
- `cyber exec --format stream-json --include-hook-events` adds a `hook_event` line for each hook that ran for the run's Session (hook ID, event, scope, outcome, decision, duration, and the tool call it guarded), before the line of what it gated. Hook input, output, reasons and environment are never included. The instance event stream carries the underlying `session.hook.receipt` summaries when asked with `include=hook_receipts`, and so does a Session's event stream (`/sessions/{id}/events`) for that Session's own receipts, which `--input-format stream-json` runs follow.
- The `auto` reviewer can check context before deciding with read-only tools: `transcript_search` over the Session's history, and `read_file`/`grep_files` over the Location under the Session's own read rules (`.env`, `deny_read` globs, secret files and paths outside the Location stay unreadable). Results arrive as framed untrusted data; budgets are 8 calls, 16 KiB per result, 64 KiB per review and 5 s per call. A bare "yes"/"go ahead" reaches the reviewer together with the assistant question it answers. `permission.auto_decided.1` records `reviewer_tools` and `confirmation`; `cyber permissions auto show` adds `reviewer tool calls`.
- Repository map: a ranked, token-budgeted map of the repository's files and definitions built with tree-sitter (Rust, Python, JavaScript, TypeScript, Go, C, C++, Java, Solidity and Bash) and cached per repository. The read-only `repo_map` tool is offered in plan mode and untrusted checkouts too, and hides read-denied files; in trusted checkouts the map is pinned into the baseline context as the `core/repo-map` source (marked `pinned` in `/context`). `repo_map` in config and profiles sets the token budgets; `cyber lsp map`, `GET /api/v1/lsp/map`, `POST /api/v1/lsp/map/rebuild` and TUI `/map` inspect it. The index is never included in backups or exports.
- Native Gemini API adapter behind the experimental `features.gemini` (off by default): `google/*` models stream from `streamGenerateContent` with the key in `x-goog-api-key` (`GEMINI_API_KEY`, then `GOOGLE_GENERATIVE_AI_API_KEY`, then `GOOGLE_API_KEY`), thought signatures are replayed byte-exact, `thinkingLevel`/`thinkingBudget` follow the model, and safety blocks and `google.rpc` quota details map to content-filter, rate-limit and quota errors. Custom providers can use it with `api.type: "gemini"`. `/login google` points to an API key; subscription login is not supported.
- Planner and executor models: `model_roles.plan` names a planner that runs a top-level Session's Turns while it is in plan mode; once the plan is approved (or the Mode changes) the next Turn runs on the Session's own model with the approved plan text, never the planner's reasoning. `/model` while planning overrides the planner for that phase. The TUI footer shows `planner: <model>`, and steps record `model_role: "plan"`. Unset, nothing changes.
- VS Code extension (`clients/vscode`): a panel bound to a Session per workspace folder with streaming, prompt, interrupt, Mode and model; the active file or selection sent as a ranged `file` part; approvals answered from a read-only diff review in the editor; and terminal handoff. It connects only to the local server over its Unix socket or loopback and authenticates with an IDE token only. Each release attaches an unsigned `cyber-<version>.vsix` and its `.sha256`; the extension is not on the Marketplace yet.
- Local IDE tokens: `cyber ide token create --client vscode [--name] [--expires <days>]`, `list` and `revoke`, and `POST|GET|DELETE /api/v1/ide/tokens`. A token (`cyi_…`, 30-day default expiry, one per client, stored hashed) is accepted only from loopback or the Unix socket and only on an allowlist of operations; it cannot create a `bypass` or `dont-ask` Session, mint tokens or reach schedules. `cyber service password` revokes every token.
- Ranged `file` prompt parts: `file { path, range: { start_line, end_line } }` admits only those lines after the usual read checks, and UTF-8 text files are now admitted as `<file>` blocks (before, a text `file` part was refused as unsupported media). The TUI sends `@path#Lx-y` mentions as ranged parts to a local server.
- In-session cron tasks: the model's `cron_create` (a 5-field cron expression in local time, or a one-shot `at`), `cron_list` and `cron_delete`, offered only to root Sessions outside Workflow Runs. Tasks are Session-scoped `cron` loops that share `loops.max_per_session` and the loop budgets, follow `missed: skip|run-once` on resume, and never create a persistent schedule.
- Loop additions: `/loop --until-goal` stops the loop once the Session's current Goal ends, `--notify always|on_change|never` (default `on_change`) sends Session notifications for changed or failed iterations, the observational `LoopIteration` hook fires after each iteration, and `/loop list` shows the pace, next run and stop conditions.
- Persistent schedules: `cyber loops add --cron "<expr>" "<prompt>"` with `list`, `show`, `pause`, `resume`, `run` and `rm`, run by `cyber service` with no client open, in a new or given Session. Runs default to `dont-ask` (an ask the run cannot answer is denied and the run continues), `bypass` needs `loops.allow_bypass` from user or managed config, directories must exist and be trusted (checked again before every run), and the model cannot create schedules. Each schedule gets a deterministic jitter (`loops.jitter_seconds`, default 60), budgets across runs, `--missed skip|run-once`, desktop notifications (macOS and Linux), a `schedule.run.1` record and the `ScheduleRun` hook.
- Path-scoped project rules: `.cyber/rules` and `.claude/rules` definitions (held until the checkout is trusted) are delivered once per Context Epoch after a matching read, edit or patch, and withdrawn when their approval is revoked or the file changes. `/context` and `GET /sessions/{id}/context` inspect every context source with token estimates, pending hook context and compaction, delivered and withheld rules and a next-request estimate; instruction files refused as `outside_project` or `read_denied` are listed there and in `cyber doctor`.

### Changed

- A key chord that computer use refuses (for example Command+C under an application grant) is now refused with `key_chord_refused` before anything is recorded or sent, and the grant stays armed. Before, it was recorded, failed in the native adapter, settled as an unknown operation and stopped the grant. The set of allowed and refused chords on macOS is unchanged.
- In `auto`, a plain read under the enforced OS sandbox (`ls`, `pwd`, `echo`, `cat`, `head`, `tail` or `wc` (never `git`), no shell expansion, only operands the Session may read without asking, untainted run, not escalated or excluded) is a recorded policy allow instead of a reviewer call. Every other command is still reviewed; `classify_read_only: true` reviews these too.
- The longest server-advised retry delay is now set per provider (`providers.<id>.retry.max_wait_ms`, 60 s, 240 s for `gemini`). A longer delay fails at once in an attended Session and is waited out in unattended runs within `usage_limit.max_wait`; before, it was shortened to the cap, so the retry came before the window opened.
- `cyber exec --input-format stream-json` drives a foreground subagent's permission and question requests over stdin like the parent's, bounds owned requests to 64 (a further one gets a `capacity` error line), and adds `input_message_ids` to step lines.

### Fixed

- Model-run commands no longer wait for an editor, pager or Git credential prompt: `git commit` without `-m` aborted only at the command timeout, and `git log` in a PTY waited in the pager. Shell tools, `monitor`, PTYs, background commands, worktree setup and solve checks now get `GIT_EDITOR`/`JJ_EDITOR`/`HGEDITOR`/`EDITOR`/`VISUAL=true`, `PAGER`/`GIT_PAGER`/`JJ_PAGER`/`MANPAGER=cat` and `GIT_TERMINAL_PROMPT=0`; a value set in `sandbox.env.set` is kept.
- Tool loops on Gemini 4 models through an OpenAI-compatible endpoint failed after the first call because thought signatures were not replayed; Gemini 4 now uses the `gemini_signature` carrier like Gemini 3.
- A per-minute Gemini rate limit through the OpenAI-compatible endpoint was treated as an exhausted quota and ended the Turn unretried; `google.rpc` error bodies now go through the Gemini rules.
- Interrupting a `cyber exec --input-format stream-json` run while a subagent waited for permission ended with "Input interruption could not be reconciled"; the held call is now refused without running and the interruption is acknowledged.
- `releases/latest/download/install.sh` redirected to the rolling `channel-beta` release, which has no install script. Channel releases are now prereleases, so only a stable `v<version>` release can be GitHub's latest.

### Security

- Any reviewer tool failure (unknown tool, invalid arguments, a refused read, a timeout or an exhausted budget) ends the `auto` review as a fallback, never an allow.
- A hook's block reason (stderr of an exit-2 command), JSON `reason`/`stop_reason` and failure stderr are redacted like tool output and cut to 4 KiB before they reach live hook notices and hook receipts; the tool result was already redacted but was unbounded.
- `cyber exec` error lines, the `--format json` `error` and stream-json errors now apply the configured `tool_output.redact` patterns and the exact values of known secrets (secret-named environment variables, secret config entries and `--password`), not only the built-in detectors.

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

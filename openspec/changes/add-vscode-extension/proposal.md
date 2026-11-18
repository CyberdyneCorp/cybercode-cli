## Why

Claude Code and Codex both ship a VS Code extension. Users get a chat panel next to their code, the editor's selection and open file as context, proposed edits in the native diff editor with Accept/Reject, and approvals without switching to a terminal. Cyber Code has no IDE client: the README's Known gaps lists "IDE/desktop clients" for every comparison, and `editor-integration` (milestone M3.7) is a target spec with no implementation.

The pieces such a client needs already exist and are covered by tests: one background server per user, the owner API with HTTP Basic authentication, durable Session event replay, interactive presence leases, permission and question requests, and the TypeScript SDK (`@cyber-code/sdk`) that the web dashboard's security model was designed around. A first extension can be a thin client of those contracts. It needs no runtime fork and only one server change.

The current `editor-integration` requirements describe the finished product: Marketplace and Open VSX publication, per-hunk review, plan documents, the `ide/context` source with diagnostics, `cyber ide install` and deep links. They do not say how the extension authenticates, what the webview may do, or how it is built and tested. This change defines a shippable first slice (MVP), states its security contract precisely, and splits the remaining parts into separate requirements so they can follow without blocking it.

## What Changes

- **MVP extension, in-repo at `clients/vscode/`** (extension ID `cyber-code.cyber`), built on `@cyber-code/sdk`:
  - **Sidebar chat panel** (webview view) bound to one Session in the workspace folder's Location. It streams the Session's durable events with `durable.seq` resume, and supports prompt, interrupt, Session list, new Session and resume, plus Mode and model selectors that use the existing routes.
  - **Editor context**: an "active file / selection" chip sent as a server-read `file` part with an optional line range. Server-side `read` checks therefore apply, so deny rules (for example `.env`) withhold the content. Alt+K inserts `@path#L<start>-<end>`.
  - **Native diff review of pending edits**: a `permission.asked.1` edit request opens VS Code's diff (or multi-file diff) editor between a read-only base snapshot and the proposed content. Accept, Reject and Reject with feedback answer the exact pending request. Completed Session changes can be reviewed read-only.
  - **In-editor approvals**: permission and question cards in the panel, plus a notification when the panel is hidden. Replies are idempotent and bound to the exact request. Stale cards are disabled, and `always` shows the scope it saves. `plan_exit` arrives as an ordinary question.
  - **Server start/attach**: reuse the registered server, or run `cyber service start` (argv, no shell) in a trusted workspace. The executable path is a machine-scoped setting only.
  - **Terminal handoff**: "Open Session in Terminal" runs `cyber --resume <id>` in an integrated terminal.
- **Security contract for the extension** (new requirements):
  - The extension connects over the Unix socket on macOS and Linux, and over TCP only to a loopback literal.
  - The owner password is read from the 0600 state file at connect time and held only in extension-host memory. It never goes into settings, SecretStorage, globalState, logs, URLs or the webview.
  - The webview has no credentials and no network access. It has a strict nonce CSP, a schema-checked message bridge and inert rendering.
  - No change to the server's Origin/CORS policy is needed or allowed.
  - VS Code Restricted Mode is honored.
- **Packaging and CI**: an esbuild bundle and an unsigned `.vsix` from `@vscode/vsce`. Unit tests, plus `@vscode/test-electron` integration tests against the real `cyber` binary and the scripted provider used by the dashboard's browser tests, run on Linux (xvfb) and macOS. The `.vsix` is a CI artifact.
- **Server**: the `file` prompt part gains an optional `range { start_line, end_line }`, read through the same `read` checks.
- **Re-scoping of `editor-integration`**:
  - "VS Code extension" now defines the MVP surface and `.vsix` distribution.
  - Marketplace and Open VSX publication moves to a new requirement, "Extension marketplace publication".
  - "Inline diff review" is split. Per-request review becomes "Pending edit diff review", and per-hunk actions and post-hoc revert become "Per-hunk diff review".
  - "Selection mentions and plan review" is split into "Selection mentions" and "Plan review in the editor".
  - Every existing scenario is kept in one of the new requirements.
  - ACP, "IDE context source", "IDE detection and extension install", "Deep links" and "JetBrains plugin" are unchanged.

### ACP recommendation

Build the VS Code extension on the owner API through the SDK, not on ACP. Keep `cyber acp` (already specified, unchanged) as the complement for Zed, JetBrains, Neovim and third-party ACP clients, and schedule it right after this MVP. VS Code has no built-in ACP client; ACP support there comes from community extensions. An ACP process is also a single stdio session, while the owner API gives shared Sessions with the TUI and dashboard, durable replay, presence leases and the full approval model. See design.md D1.

## Non-goals

- Publishing to the VS Code Marketplace or Open VSX, and signing or verifying the `.vsix` (the "Extension marketplace publication" requirement, follow-up).
- JetBrains, Cursor/Windsurf-specific behavior beyond what plain `.vsix` install gives, and `cyber acp` itself.
- Remote use: VS Code Remote-SSH, WSL, Dev Containers, Codespaces, `vscode.dev`, or a server that is not on the extension host's own machine. The extension refuses non-loopback registrations.
- Per-hunk accept/reject, reverting already-applied edits from the diff view, and rendering plan documents.
- `POST /api/v1/ide/context`, diagnostics sharing, `/ide`, `cyber ide install`, IDE detection, the `vscode://` URI handler and `cyber://` deep links.
- Windows: no Windows CLI release ships yet. The TCP loopback path is specified but only exercised on Linux and macOS CI.
- A scoped, non-owner IDE credential (open decision 1).

## Capabilities

### New Capabilities

None. The extension belongs to the existing `editor-integration` capability.

### Modified Capabilities

- `editor-integration`:
  - MODIFIED "VS Code extension".
  - REMOVED (split, scenarios carried over) "Inline diff review" and "Selection mentions and plan review".
  - ADDED "Pending edit diff review", "Selection mentions", "Extension server connection", "Extension owner authentication", "Extension webview isolation", "Extension workspace trust", "Chat panel Session binding", "Editor context attachment", "In-editor approvals", "Session terminal handoff", "Extension packaging and CI", "Extension marketplace publication", "Per-hunk diff review", "Plan review in the editor".
- `server-api`: MODIFIED "Prompt attachment parts" adds the optional `range` to `file` parts. No other line changes.

No requirement in `permissions-modes`, `client-sdk`, `web-client`, `workspace-trust` or `cli-commands` changes. The extension uses their existing contracts: replies and approvals, interactive presence leases, durable replay, `cyber service start` and `cyber --resume`.

## Impact

- **Code**:
  - new `clients/vscode/` (TypeScript extension, webview UI, tests);
  - `crates/cyber-server`: the `file` part range, plus OpenAPI and regenerated SDK schema.
- **CI**: a new `vscode-extension` job (build, typecheck, lint, unit tests, `@vscode/test-electron` on Linux and macOS, `.vsix` artifact). The existing `ci.yml` path filter already treats `clients/` as code.
- **Security**: a new local client holding owner authority. design.md D3–D6 cover the threat model, and open decisions 1–2 cover scoped tokens and server proof.
- **Docs**: `docs/vscode-extension.md` (new), README Known gaps, `docs/features.md`, and the ROADMAP M3.7 line. These are updated at implementation time. This change adds only the ROADMAP line.
- **Phase**: requirements stay (P3) under M3.7. Pulling the MVP earlier is open decision 7.

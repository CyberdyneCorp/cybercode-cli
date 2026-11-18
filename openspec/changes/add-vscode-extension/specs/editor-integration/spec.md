## MODIFIED Requirements

### Requirement: VS Code extension
(P3) The system SHALL provide the VS Code extension `cyber-code.cyber`, built from `clients/vscode/` and distributed as a `.vsix` file. It SHALL declare `engines.vscode` `^1.95.0` and run in the local extension host only (`extensionKind: ["workspace"]`, with no web-extension entry point). It SHALL provide:
- a sidebar chat panel (webview view `cyber.chat` in its own activity-bar container) bound to one Session, as "Chat panel Session binding" defines;
- Session history and resume for the workspace folder's Location;
- Mode and model selectors;
- editor context, diff review, in-editor approvals and terminal handoff, as the other requirements of this capability define.

It SHALL connect to the user's background server through the public owner API and `@cyber-code/sdk`, starting it with `cyber service start` when needed, as "Extension server connection" defines. It SHALL NOT bundle a server, open a listener, or use any private or in-process interface. Marketplace and Open VSX publication is specified by "Extension marketplace publication".

#### Scenario: Panel attaches to the service
- **WHEN** the user opens the Cyber Code panel in a trusted VS Code workspace and a healthy server is registered
- **THEN** the extension attaches to that server without starting another, and lists the Sessions of the workspace folder's Location, newest first

#### Scenario: Install from a vsix
- **WHEN** a user runs `code --install-extension cyber-0.1.0.vsix` and opens a repository
- **THEN** the Cyber Code activity-bar icon appears and the panel offers to attach to or start the server

## REMOVED Requirements

### Requirement: Inline diff review
**Reason**: Split so the first extension slice can ship per-request review while per-hunk actions and post-hoc revert follow.
**Migration**: Per-request review is "Pending edit diff review"; per-hunk actions, the "Reject a hunk" scenario and the `accept-edits` revert are "Per-hunk diff review".

### Requirement: Selection mentions and plan review
**Reason**: Split so selection mentions ship in the first extension slice while plan documents follow.
**Migration**: Selection mentions and the "Selection reference" scenario are "Selection mentions"; plan documents and the "Approve a plan" scenario are "Plan review in the editor".

## ADDED Requirements

### Requirement: Pending edit diff review
(P3) For each pending `edit` permission request that carries `metadata.diff`, the VS Code extension SHALL offer a review in VS Code's native diff editor. One changed file SHALL open `vscode.diff`, and several files SHALL open the multi-file diff editor. The left side SHALL be a read-only snapshot of the on-disk content taken when the request arrived, and the right side SHALL be that snapshot with the request's hunks applied by exact context match. Both SHALL be served read-only from `cyber-review:` URIs that name the request but contain no file content. The review SHALL offer:
- **Accept**: reply `once`.
- **Accept always**: reply `always`, offered only when `always_patterns` is non-empty and only after showing those patterns.
- **Reject**: reply `reject` without a message, which halts the Drain.
- **Reject with feedback**: reply `reject` with the entered `message`.

The actions SHALL apply to the whole request. When the hunks do not apply to the current disk content, the review SHALL show the raw unified diff read-only with the notice `The file changed since Cyber proposed this edit`. The extension SHALL NOT write proposed content to the real file. The server SHALL remain the only writer. The review SHALL close when the server acknowledges a reply, or when `permission.replied.1` shows the request was answered elsewhere. The command `Cyber: Review Session Changes` SHALL open the Session's `GET .../diff` read-only in the multi-file diff editor.

#### Scenario: Accept a proposed edit
- **WHEN** the agent proposes an edit to `src/app.ts` in `default` Mode and the user selects Accept in the diff editor's title bar
- **THEN** the extension replies `once` to that exact request, the server writes the file, and the review closes after the acknowledgement

#### Scenario: Reject with feedback
- **WHEN** the user selects Reject with feedback on a pending three-file `apply_patch` review and enters `use the existing helper`
- **THEN** the reply is `reject` with that message, no file changes, and the model receives `Rejected by user: use the existing helper`

#### Scenario: File changed before review
- **WHEN** `src/app.ts` is modified on disk after the request arrived, so the hunks no longer apply
- **THEN** the review shows the raw diff with `The file changed since Cyber proposed this edit` and still offers the reply actions

#### Scenario: Answered in another client
- **WHEN** a diff review is open and the user approves the same request in the TUI
- **THEN** the extension closes the review on `permission.replied.1` and shows that it was answered elsewhere, without sending a reply

### Requirement: Selection mentions
(P3) The VS Code extension SHALL provide the command `Cyber: Insert Selection Mention`, bound to Alt+K (Option+K on macOS) when an editor has focus. It SHALL insert `@<Location-relative path>#L<start>-<end>` for the current selection into the panel composer, or `@<path>#L<line>` for a single line, and focus the composer. A file outside the bound Location SHALL be inserted as an absolute path and remains subject to the server's `external_directory` rule.

#### Scenario: Selection reference
- **WHEN** the user selects lines 12-30 of `src/app.ts` and presses Alt+K
- **THEN** the composer receives `@src/app.ts#L12-30`

### Requirement: Extension server connection
(P3) The VS Code extension SHALL find the server through `<state>/server.json`, using the same state-directory rules as `@cyber-code/sdk`. It SHALL choose the transport as follows:
- On macOS and Linux, when the registration names a `socket`, it SHALL use that Unix socket.
- Otherwise it SHALL use TCP only when the registered `url` is `http://127.0.0.1:<port>`, `http://[::1]:<port>` or `http://localhost:<port>`.
- Any other registration SHALL be refused with `Cyber Code server is not on loopback; the VS Code extension supports local servers only`, and no credential SHALL be sent.

Before sending credentials over TCP, the extension SHALL check that the registration `pid` is alive and that unauthenticated `GET /api/v1/health` reports `apiVersion` `v1` and the registration's `version` and `build`. After the first authenticated call it SHALL check that `GET /api/v1/service` reports the registration `id`, and disconnect on mismatch.

With no healthy registration, in a trusted workspace with `cyber.autoStartServer` `true` (the default), the extension SHALL run `<executable> service start` as an argument vector without a shell, with a 10-second timeout, and then read the registration again. `<executable>` SHALL be the `cyber.executablePath` setting, which SHALL be machine-scoped so that workspace settings cannot set it, or else `cyber` on `PATH`. A server whose `apiVersion` is not `v1`, or that answers `404` or `503` for a route the extension needs, SHALL be shown as incompatible with its version, and the extension SHALL stop.

#### Scenario: Socket preferred
- **WHEN** `server.json` names both `url` `http://127.0.0.1:4747` and `socket` `<state>/cyber.sock` on Linux
- **THEN** every extension request uses the Unix socket

#### Scenario: LAN registration refused
- **WHEN** `server.json` names `url` `https://dashboard.example:4747` and no socket
- **THEN** the panel shows the loopback-only error and no request carrying credentials is sent

#### Scenario: Workspace cannot choose the executable
- **WHEN** a repository's `.vscode/settings.json` sets `cyber.executablePath` to `./tools/cyber`
- **THEN** the extension ignores that value and starts the server with the machine setting or `cyber` on `PATH`

#### Scenario: Auto-start
- **WHEN** no server is registered and the user opens the panel in a trusted workspace
- **THEN** the extension runs `cyber service start`, attaches to the new registration and lists Sessions

### Requirement: Extension owner authentication
(P3) The VS Code extension SHALL authenticate to the owner API with HTTP Basic `cyber:<password>`. It SHALL read the password from `<state>/password` only in the extension host, when a connection is created, and keep it only in that connection's memory. It SHALL discard the password on disconnect, deactivation and every `401`. After a `401` it SHALL read the file again once, and a second `401` SHALL show `Run \`cyber service password\` or restart the server`. The extension SHALL NOT write the password, or anything derived from it, to:
- VS Code settings, `SecretStorage`, `globalState` or `workspaceState`;
- files, the output channel, error messages or telemetry;
- any URL, including the `auth_token` query parameter;
- any message to the webview.

It SHALL contribute no setting that accepts a password or token. Its output channel SHALL log only method, route template, status and request ID, never headers or bodies.

#### Scenario: No persisted credential
- **WHEN** the extension has attached, streamed a reply and been deactivated
- **THEN** the password appears in none of the user and workspace settings, extension storage, the output channel or the webview's messages

#### Scenario: Password rotated
- **WHEN** the user runs `cyber service password <new>` while the panel is attached
- **THEN** the next request gets `401`, the extension reads the file once more, and it continues with the new password without user action

### Requirement: Extension webview isolation
(P3) The chat panel webview SHALL hold no credentials and make no network requests. All server traffic SHALL go from the extension host. The extension SHALL NOT add `vscode-webview://` or any other origin to `remote.cors_origins`, and SHALL NOT require any server Origin or CORS change. The webview SHALL:
- use a Content Security Policy of `default-src 'none'`, with scripts and styles allowed only by a per-render random nonce and the webview resource source, and with no `connect-src`;
- set `enableCommandUris: false`, `enableForms: false`, and `localResourceRoots` limited to the extension's `dist/` and `media/` directories;
- render model, tool and file text inert: text nodes, a fixed Markdown element allowlist, no remote images, and links opened only by the extension host after confirmation.

Messages between the webview and the extension host SHALL be a validated discriminated union. Unknown types, extra fields, payloads over 1 MiB and malformed IDs SHALL be dropped. A reply message from the webview SHALL be honored only if its request is currently pending for the panel's bound Session in the extension host's own table.

#### Scenario: Webview cannot reach the server
- **WHEN** script in the webview calls `fetch("http://127.0.0.1:4747/api/v1/sessions")`
- **THEN** the CSP blocks the request; if one were sent, the server would reject its `vscode-webview://` Origin with `403`

#### Scenario: Forged approval refused
- **WHEN** the webview posts a reply for `per_123`, which is not pending for the bound Session
- **THEN** the extension host drops the message and sends nothing to the server

#### Scenario: Injected HTML rendered inert
- **WHEN** a tool result contains `<img src="https://attacker.example/x.png" onerror="...">`
- **THEN** the panel shows that text literally and makes no network request

### Requirement: Extension workspace trust
(P3) The VS Code extension SHALL declare limited support for untrusted workspaces, with `cyber.autoStartServer` as a restricted setting. In VS Code Restricted Mode it SHALL NOT start the server, create Sessions, send prompts or reply to permissions or questions. The panel SHALL say why and offer `Manage Workspace Trust`. VS Code workspace trust SHALL be an additional gate and SHALL NOT replace or change Cyber Code's own checkout trust (`workspace-trust`).

#### Scenario: Restricted Mode
- **WHEN** the user opens an untrusted folder in Restricted Mode and opens the panel
- **THEN** no server is started, the composer is disabled, and the panel offers `Manage Workspace Trust`

### Requirement: Chat panel Session binding
(P3) The chat panel SHALL bind to one Session in one Location: the workspace folder of the active editor, or the first folder. In a multi-root workspace the panel header SHALL offer a folder picker. Requests SHALL carry that Location as `x-cyber-directory`. The user SHALL be able to pick an existing Session of the Location (newest first, pages of 50) or create one. The bound Session ID SHALL be remembered per workspace in `workspaceState`, with no Session content. The panel SHALL:
- load history with `message.list`;
- follow `GET .../events`, resuming from the last `durable.seq` after reconnects and panel reloads, so no durable event is lost or repeated;
- send prompts with `POST .../prompt`;
- interrupt with `POST .../interrupt`;
- change Mode and model through the existing routes, showing a change as pending until it takes effect.

While the panel is visible and bound, the extension SHALL hold an interactive presence lease for the Session (`POST .../interactive-clients`, renewed every 15 seconds) and SHALL release it when the panel is hidden or unbound, or the extension deactivates. Live deltas SHALL reach the webview batched to at most 60 messages per second.

#### Scenario: Shared with the TUI
- **WHEN** the user sends a prompt from the panel to a Session that is also open in the TUI
- **THEN** both clients show the same admitted prompt and streamed reply

#### Scenario: Reload resumes
- **WHEN** VS Code reloads the window during a streaming reply
- **THEN** the panel rebinds to the remembered Session, loads its messages and resumes events with no duplicated or missing durable event

#### Scenario: Panel hidden
- **WHEN** the user hides the panel and no other client is attached
- **THEN** the lease is released, and new permission asks follow the Session's non-interactive policy and are never approved automatically

### Requirement: Editor context attachment
(P3) The composer SHALL show a removable chip for the active editor's file and, when there is a non-empty selection, its line range. The chip SHALL be on by default and controlled by `cyber.includeEditorContext` (default `true`). On send, the extension SHALL add a `file` prompt part with the Location-relative `path` and, for a selection, `range { start_line, end_line }` (1-based, inclusive). It SHALL NOT inline file or buffer content itself. The server reads the part through the `read` tool's checks (`server-api` → Prompt attachment parts). When the document has unsaved changes, the chip SHALL say `unsaved changes not included` and offer Save. Untitled and non-`file:` documents SHALL produce no chip.

#### Scenario: Selection sent as a ranged file part
- **WHEN** lines 12-30 of `src/app.ts` are selected and the user sends `explain this`
- **THEN** the prompt carries a text part `explain this` and `file { path: "src/app.ts", range: { start_line: 12, end_line: 30 } }`

#### Scenario: Denied file withheld
- **WHEN** the active editor shows `.env`, the `read` rule for `*.env` is `deny`, and the user sends a prompt with the chip on
- **THEN** the server refuses the part with a permission denial naming the rule, nothing from `.env` is admitted, and the panel shows the denial

### Requirement: In-editor approvals
(P3) The extension SHALL show each pending permission request and question of the bound Session, including requests routed from child Sessions, as a card in the panel. Each card SHALL show the original Session, the action, the resources and the command or diff. When the panel is hidden, the extension SHALL also show a VS Code notification with `Review`. It SHALL fetch pending requests on every (re)connect before following events, so that requests raised while disconnected are shown.

A permission reply SHALL be bound to the exact `session_id` and request ID, and SHALL use one idempotency key per decision that is reused on transport retries. The card SHALL show the decision as applied only after the server acknowledges it. It SHALL be disabled when the request is answered elsewhere, when its Session or Location changes, or while the connection is recovering. An `always` choice SHALL show the `always_patterns` it saves before it is sent. Because any `reject` declines the Session's other pending requests, the card SHALL say so. Questions, including the `plan_exit` approval question, SHALL be answered with the offered choices or dismissed. A taint notice (`metadata.taint`) SHALL be shown verbatim on the card.

#### Scenario: Requests from a disconnect
- **WHEN** a permission was asked while VS Code was closed and the user opens the panel
- **THEN** the pending request appears as a card before any newer event is shown

#### Scenario: Stale card cannot approve a later request
- **WHEN** the TUI answers `per_1` and the agent then asks `per_2` while the panel still shows `per_1`
- **THEN** `per_1`'s card is disabled, and selecting it sends nothing for `per_2`

#### Scenario: Uncertain reply retried
- **WHEN** the connection drops after the user selects Accept and before the acknowledgement arrives
- **THEN** the card shows the reply as unconfirmed, and the retry sends the same request ID, reply and idempotency key

#### Scenario: Plan approval as a question
- **WHEN** the agent calls `plan_exit` in `plan` Mode
- **THEN** the panel shows the question with `Approve and build`, `Approve with accept-edits` and `Request changes`, and the chosen answer is sent to that question

### Requirement: Session terminal handoff
(P3) The command `Cyber: Open Session in Terminal` SHALL open a VS Code integrated terminal that runs the configured executable directly (no shell string) with the arguments `--resume <sessionID>`, in the bound Location. The command `Cyber: New Terminal Session` SHALL do the same without `--resume`. Both SHALL be unavailable in Restricted Mode.

#### Scenario: Continue in the TUI
- **WHEN** the panel is bound to `ses_abc` and the user runs `Cyber: Open Session in Terminal`
- **THEN** an integrated terminal runs `cyber --resume ses_abc` in the Location, and the TUI and panel show the same live Session

### Requirement: Extension packaging and CI
(P3) The extension SHALL be a private npm package in `clients/vscode/` that uses `@cyber-code/sdk` from `sdk/typescript`. It SHALL be bundled with esbuild into `dist/extension.js` (Node, CommonJS) and `dist/webview.js` (browser), with no runtime code loaded from a network. `npm run package` SHALL produce `cyber-<version>.vsix` with `@vscode/vsce`. CI SHALL have a `vscode-extension` job, on every change outside the documentation-only paths, that runs on Linux (under `xvfb-run`) and macOS. The job SHALL:
- install with `npm ci`;
- typecheck, lint and run unit tests;
- run `@vscode/test-electron` integration tests against the `cyber` binary built in the same workflow and a loopback scripted OpenAI-compatible provider, with a temporary `CYBER_HOME` and no real credentials;
- upload the `.vsix` as a workflow artifact.

The integration tests SHALL cover the scenarios of this capability's extension requirements, and SHALL assert that the owner password never appears in settings, extension storage, logs or webview messages.

#### Scenario: Pull request builds the extension
- **WHEN** a pull request changes `clients/vscode/src/extension/approvals.ts`
- **THEN** the `vscode-extension` job runs unit and integration tests on Linux and macOS and uploads `cyber-<version>.vsix`

#### Scenario: Test fixture isolation
- **WHEN** the integration tests run
- **THEN** they use a temporary home, repository, password and server process, and remove them afterwards

### Requirement: Extension marketplace publication
(P3) The system SHALL publish the VS Code extension `cyber-code.cyber` to the Visual Studio Marketplace and to Open VSX (for Cursor, Windsurf and VSCodium). Releases SHALL be built only by the release workflow from a tagged commit, signed through the Marketplace signing flow, and published with a SHA-256 for the `.vsix` in the release manifest. The extension version SHALL match the `cyber` release version.

#### Scenario: Install from Open VSX
- **WHEN** a VSCodium user searches the extension view for `Cyber Code`
- **THEN** `cyber-code.cyber` at the current release version is offered and installs

### Requirement: Per-hunk diff review
(P3) The VS Code extension SHALL extend "Pending edit diff review" with per-hunk Accept and Reject actions. Rejecting some hunks of a request awaiting approval SHALL reply `reject` with feedback naming the rejected hunks by file and line range. In `accept-edits` Mode, Reject on an already-applied edit SHALL stage a code-only revert of that file through `snapshots-checkpoints`.

#### Scenario: Reject a hunk
- **WHEN** the user rejects one hunk of a three-hunk edit awaiting approval
- **THEN** the permission reply is `reject` with feedback naming the rejected hunk, and the model receives it as a tool error

### Requirement: Plan review in the editor
(P3) In `plan` Mode, the VS Code extension SHALL render the plan presented by `plan_exit` as a read-only Markdown document with Approve and Request changes actions. Approve SHALL answer the `plan_exit` question with the chosen approval, and Request changes SHALL send the entered feedback.

#### Scenario: Approve a plan
- **WHEN** the agent calls `plan_exit` with a plan
- **THEN** the extension opens the plan with Approve and Request changes actions, and Approve switches the Session out of plan mode

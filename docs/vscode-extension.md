# VS Code extension

The Cyber Code extension adds a chat panel to VS Code. It is a client of the background server, like the TUI and the web dashboard, so the same Session stays live in all three. From the panel you can:

- chat with a Session of the workspace folder, interrupt it, and change its Mode and model;
- attach the active file or selection as context, or insert `@path#L12-30` with Alt+K (Option+K on macOS);
- review a proposed edit in VS Code's diff editor and accept or reject it;
- answer permission requests and questions, including the `plan_exit` approval;
- continue the Session in the TUI in an integrated terminal.

The extension is in this repository at `clients/vscode/`. The source of truth for its behavior is the `editor-integration` capability in `openspec/`.

## Install

The extension is not on the Visual Studio Marketplace or Open VSX yet. Each [GitHub release](https://github.com/CyberdyneCorp/cybercode-cli/releases) carries `cyber-<version>.vsix` and `cyber-<version>.vsix.sha256`. The `.vsix` is **unsigned** in this phase, so verify its checksum before installing it:

```sh
sha256sum -c cyber-0.2.0.vsix.sha256      # macOS: shasum -a 256 -c cyber-0.2.0.vsix.sha256
code --install-extension cyber-0.2.0.vsix
```

The extension version always equals the `cyber` version it was released with.

**Requirements:**
- VS Code 1.95 or newer, on macOS or Linux. Windows waits for a Windows CLI release.
- The `cyber` CLI installed separately, on `PATH` or set in the user setting `cyber.executablePath`. The extension does not bundle a binary. Without one, the panel says `Cyber Code CLI not found` and links to the install instructions.
- A local server. Remote SSH, WSL, Dev Containers, Codespaces and `vscode.dev` are not supported.

The manifest uses the placeholder publisher `cyber-code` (extension ID `cyber-code.cyber`) until the organization reserves a Marketplace publisher ID.

## First run

1. Open a folder and select the Cyber Code icon in the activity bar.
2. **Server.** If a server is registered, the panel attaches to it. Otherwise it asks once whether to start the background server: **Always start** and **Don't start** are saved in `cyber.autoStartServer`, while **Start this time** saves nothing. Starting runs `cyber service start`. The server keeps running after VS Code closes.
3. **IDE token.** The panel offers **Create IDE token**. This opens an integrated terminal running `cyber ide token create --client vscode`, so you see exactly what runs. The panel attaches as soon as the token file appears.

## IDE tokens

The extension never uses the owner password. It authenticates with a **local IDE token**, a narrower credential that only the owner can issue:

```sh
cyber ide token create --client vscode   # writes <state>/ide/vscode.token (0600); prints only the ID and expiry
cyber ide token list                     # ID, client, name, status, expiry, last use
cyber ide token revoke itk_…             # the panel's next request gets 401
```

- **Expiry and rotation.** A token expires after 30 days by default (`--expires 1` to `90`). Creating another token for `vscode` revokes the previous one. When the token expires within 7 days, the panel says so and offers **Create IDE token** again.
- **Revocation.** Revoking a token closes its event streams within a second and ends its presence lease. The extension re-reads the token file once; if that token is refused too, it offers **Create IDE token**. `cyber service password <value>` revokes every IDE token.
- **Scope.** A token can list, create and show Sessions, prompt, interrupt, change Mode and model, read messages, events and the diff, hold a presence lease, and answer permissions and questions. Anything else answers `403` with `reason: "ide_token_scope"`, which the panel shows as `This action needs the Cyber CLI or dashboard`. A token cannot create tokens, switch to `bypass` or `dont-ask`, or create a Session with those Modes or its own rules. See [IDE tokens in the dashboard guide](web-dashboard.md#local-ide-tokens) for the server side.

## Security model

- **Local only.** On macOS and Linux the extension talks to the server over the Unix socket in the private state directory. Otherwise it uses TCP only to `127.0.0.1`, `[::1]` or `localhost`. Any other registration is refused with `Cyber Code server is not on loopback; the VS Code extension supports local servers only`, before a credential is sent. Over TCP, it first checks that the registered process is alive and that the unauthenticated health route reports the registered version and build. After the first authenticated call it checks the server's ID. The server also refuses IDE tokens from non-loopback peers.
- **The token stays in memory.** The extension reads `<state>/ide/vscode.token` when it connects and keeps the token only in that connection. It never writes the token to settings, `SecretStorage`, extension state, files, the output channel, error messages, URLs or the webview, and no setting accepts a credential. It never reads `<state>/password`. The output channel logs method, route template, status and request ID only.
- **The webview has no credentials and no network.** The panel's Content Security Policy is `default-src 'none'` with per-render nonces and no `connect-src`. Command URIs and forms are disabled. All server traffic leaves the extension host. Messages between the panel and the host are validated: unknown types, extra fields, payloads over 1 MiB and malformed IDs are dropped. A reply is honored only for a request the host itself holds as pending for the bound Session.
- **Inert rendering.** Model, tool and file text is shown as text, with a fixed set of Markdown elements. Remote images are never loaded. A link shows its URL, and it opens in your browser only after you confirm.
- **No server change.** The server's Origin and CORS policy is unchanged. Do not add `vscode-webview://` to `remote.cors_origins`.
- **Workspace trust.** In Restricted Mode the extension does not start the server, create Sessions, send prompts, answer approvals or open terminals. The panel offers **Manage Workspace Trust**. VS Code trust is an extra gate: Cyber's own checkout trust still applies.

## Using the panel

- **Sessions.** The panel binds to one Session of the workspace folder that holds the active editor (multi-root workspaces show a folder picker). It remembers the Session per workspace and resumes the event stream after a reload without losing or repeating events. **New** creates a Session. While the panel is visible, it holds an interactive presence lease (renewed every 15 seconds). When you hide the panel the lease is released, and new asks follow the Session's non-interactive policy.
- **Modes.** The Mode selector offers `default`, `accept-edits`, `plan` and `auto`. A change shows as pending until it takes effect.
- **Editor context.** The composer shows a chip for the active file and selection. On send, the extension adds a `file` part with the Location-relative path and line range. The server reads it through the `read` tool's checks, so deny rules such as `*.env` withhold the content and the panel shows the denial. Unsaved changes are not included; the chip says so and offers **Save**. Remove the chip with `×` or turn it off with `cyber.includeEditorContext`.
- **Diff review.** A pending edit with a diff shows **Review diff**. One file opens in the diff editor, several in the multi-file diff editor. The left side is the file as it is on disk, the right side the proposal. Use **Accept**, **Accept Always** (it shows the patterns it saves first), **Reject** or **Reject with Feedback** in the editor title bar or on the card. If the file changed on disk since the proposal, the review shows the raw diff with `The file changed since Cyber proposed this edit`. The extension never writes the file: the server does, after your reply. **Cyber: Review Session Changes** shows the Session's completed changes read-only.
- **Approvals.** Each pending permission or question of the Session, including those from its subagents, is a card. When the panel is hidden, a notification offers **Review**. A card shows its reply only after the server confirms it. If the connection drops first, the card says the reply is unconfirmed, and choosing the same answer retries it with the same idempotency key. A request answered in the TUI or the dashboard disables its card and closes its review. Rejecting also declines the Session's other pending requests.
- **Terminal handoff.** **Cyber: Open Session in Terminal** runs `cyber --resume <session>` in an integrated terminal. **Cyber: New Terminal Session** starts a new one.

## Settings

| Setting | Scope | Default | Meaning |
|---|---|---|---|
| `cyber.executablePath` | machine | empty | The `cyber` executable. Workspace settings cannot set it. |
| `cyber.autoStartServer` | machine, restricted | `ask` | `ask`, `always` or `never`: whether to start the server when none runs. |
| `cyber.includeEditorContext` | any | `true` | Attach the active file and selection to prompts. |

## Limits

- No Marketplace or Open VSX publication, and the `.vsix` is unsigned.
- Edits are accepted or rejected per request, not per hunk. Applied edits cannot be reverted from the diff view, and plan documents are not rendered.
- No ACP, IDE diagnostics sharing, `cyber ide install` or deep links.
- Over TCP (Windows, once a CLI ships there), a squatter on a stale loopback port could receive the IDE token before the server-ID check. It would get only the IDE scope until you revoke the token. The socket is used on macOS and Linux for this reason.

## Development

```sh
cd clients/vscode
npm ci
npm run typecheck && npm run lint && npm test     # unit tests
CYBER_E2E_BINARY=../../target/debug/cyber npm run test:integration   # VS Code + real server
npm run package                                    # cyber-<version>.vsix
```

The integration tests download VS Code into `clients/vscode/.vscode-test/`, start a real `cyber serve --register` with a temporary `CYBER_HOME` and a scripted loopback model, and issue an IDE token through the real CLI. Afterwards they check that the token appears nowhere in the VS Code profile. On Linux, run them under `xvfb-run -a`.

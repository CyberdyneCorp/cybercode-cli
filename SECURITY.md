# Security policy

Cyber Code runs model-proposed commands and edits on your machine, so we treat sandbox, permission and credential issues as the highest priority.

## Reporting a vulnerability

**Please do not open a public issue for a security problem.**

Report it privately through GitHub: open the repository's **Security** tab and choose **Report a vulnerability**. This creates a private advisory that only maintainers can see.

Include what you can of:

- the affected version (`cyber --version`) or commit, and your OS;
- the configuration involved (mode, sandbox, `permissions`, trust state), with secrets removed;
- steps to reproduce, or a proof of concept;
- what an attacker gains: a file read, a command run, a credential, a prompt they control, and so on.

We aim to acknowledge a report within **3 business days** and to send a first assessment within **10 business days**. We'll keep you updated until a fix ships, and credit you in the advisory unless you ask us not to.

## Supported versions

Cyber Code has not had its first release yet. Until it does, fixes land on `main` only. After the first release, security fixes go into the latest release and `main`.

## In scope

- Sandbox escapes, or commands that run with more access than the selected sandbox and mode allow.
- Bypasses of permission rules, protected paths, workspace trust or org policy (`policy`), including through hooks, plugins, MCP servers, skills or project configuration.
- Credential exposure: API keys, subscription logins, MCP tokens or secrets reaching the model, logs, exports, other Sessions or other machines. This includes failures of tool-output redaction.
- Untrusted content (web pages, repository files, MCP and tool output) causing actions the user did not approve, beyond the documented limits of the untrusted-content defense.
- Authentication or authorization flaws in the local server, the web dashboard, direct peers, `cyber attach` or the project collaboration hub.
- Supply-chain issues in our release artifacts, install script or upgrade verification.

## Out of scope

- Behavior the user explicitly allowed: `bypass` mode, `--dangerously-skip-permissions`, a full-access sandbox, or a rule that allows the action.
- The quality or truthfulness of model output, and prompt injection that only changes what the model says without causing an unapproved action.
- Vulnerabilities in third-party models, providers or MCP servers themselves. Please report those to their owners.
- Denial of service against your own local server.

When in doubt, report it. We'd rather read a report that turns out to be out of scope than miss a real one.

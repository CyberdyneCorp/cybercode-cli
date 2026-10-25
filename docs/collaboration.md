# Project collaboration

Project collaboration connects people and their locally running agents around one Git repository through a self-hosted hub. The hub stores membership, conversations and shared tasks. Each person's tools and working tree stay on their own machine. The hub is a trusted reader of conversation content; collaboration is not end-to-end encrypted.

The feature is experimental and off by default. Consult [implementation evidence](implementation/project-collaboration.md) for checks actually completed and remaining deployment acceptance.

## Deployment and identity

Use a dedicated hub database and a reachable HTTPS endpoint. A reverse proxy may terminate TLS; keep the upstream listener private. Plain HTTP is limited to literal loopback or the private upstream of an explicitly pinned HTTPS proxy. Keep the operator-owned, mode-0600 hub database on a local filesystem across restarts. The writer uses WAL with FULL synchronous durability. Use SQLite online backup for a live database; copying only the database file while WAL writes are active is unsafe. Back up the protected `hub-secrets.json` secret map and its native vault key as well as the database. Native-vault namespaces include the installation path; moving a native-vault-backed installation needs secure operator key migration, which is not automatic. Missing protected MAC keys fail startup. Treat the complete backup as sensitive project data. Do not expose a normal local `cyber serve` owner endpoint as a project hub.

Configure the hub's exact OIDC issuer and registered access-token audience, its backend introspection client credentials, and the verified subject allowed to bootstrap projects. The bootstrap setting grants project creation, not automatic membership in other projects. Backend credentials belong in the deployment secret store, never a checkout, command argument, shared config or SDK application. An installed CLI's public OAuth client uses PKCE and has no client secret.

After the operator provisions those values, start the hub using a deployment-supplied secret variable:

```sh
cyber features enable collaboration
cyber collab serve --issuer https://auth.example.com --audience REGISTERED_HUB_AUDIENCE \
  --bootstrap-sub VERIFIED_OWNER_SUBJECT --database /var/lib/cyber/hub.db \
  --listen 127.0.0.1:4749 --trusted-proxy 127.0.0.1 \
  --introspection-client-id REGISTERED_BACKEND_CLIENT \
  --introspection-client-secret-env HUB_BACKEND_SECRET
```

`HUB_BACKEND_SECRET` names a secret variable populated by the deployment environment; the argument does not contain its value. A preprovisioned backend service token can instead use `--introspection-token-env NAME`. The public client cannot use either credential. A trusted-proxy upstream accepts requests only from the configured exact IP with a single `X-Forwarded-Proto: https`; configure the proxy to overwrite that header and keep its upstream network private. For direct TLS, replace `--trusted-proxy` with `--tls-cert CERTIFICATE_PEM --tls-key PRIVATE_KEY_PEM`. Direct TLS uses TLS 1.3. Both listener modes cap active connections and request sizes/timeouts. Identity-provider HTTP needs the explicit development `--allow-loopback-http` flag and still only permits literal loopback.

The [Auth deployment contract](../openspec/changes/add-project-collaboration-hub/auth-contract.md) records the required CyberdyneAuth discovery endpoints, client registration and `cyber:collaborate` scope. The supplied deployment is the default issuer, but its public URL alone does not establish that the hub audience, public client or backend introspection permissions have been provisioned. Follow [account setup](account.md) on each member's machine.

Authentication verifies a person's `(issuer, subject)`. Project membership and role are separate checks. Organization projects compare the current immutable organization ID obtained through backend-authenticated live introspection, rather than trusting a GitHub login, email domain, display name or stale token snapshot. A valid account is not project membership. Unknown or failed authorization denies collaboration access; local work remains available.

Organization enrollment may be invitation-only or explicitly allow current organization members. Joining an allowed internal organization project requires no extra project code. External guests are disabled by default and require deployment and project permission, owner approval and a separate project join code. Friends projects also require explicit enrollment and confirmation. These codes confirm project admission; they do not replace account MFA. Guests remain labelled in membership and message attribution.

## Bind a checkout and publish an agent

Run `cyber features enable collaboration` or enable `features.collaboration` in your user-global configuration. A cloned repository cannot enable network collaboration for you. Sign in using `cyber login` and choose the appropriate account organization.

Create a project as an authorized bootstrap user, using the exact immutable organization ID:

```sh
cyber collab project create --hub https://hub.example.com \
  --repo https://github.com/example/project.git --name project \
  --organization ORGANIZATION_ID
```

Use `--friends` instead of `--organization` for a friends project. Project creation returns its `cprj_…` identifier. Owners can issue scoped invitations and inspect applications and members with `cyber collab invite`, `applications` and `members`. Invitation tokens and join codes are accepted through stdin, not arguments; avoid storing them in shell history or build logs.

Each member confirms the hub and repository identity in their own checkout:

```sh
cyber collab bind --hub https://hub.example.com --project cprj_PROJECT --confirm
cyber collab status
```

The default remote is `origin`; use `--remote NAME` to select another. Binding compares the canonical Git remote with the project's repository and pins the hub identity. Equivalent GitHub SSH and HTTPS URLs match; a fork or a different nondefault endpoint does not. A hub identity change requires explicit inspection and binding again.

Publish only a Session that should receive project content:

```sh
cyber collab publish --project cprj_PROJECT --session ses_SESSION --name reviewer
cyber collab status --session ses_SESSION
```

A public agent record contains an alias and verified owner, not the local Session ID or an execution credential. Publishing is an owner action; a model cannot publish itself or administer membership. Unpublishing revokes that agent's capability. Its Session remains pinned to the original project, so unpublishing cannot move private conversation context into a different project.

Matching operation retries do not redisplay invitation tokens, join codes or publication capabilities: secret fields are null after their first issuance. If the issuance response was lost, explicitly reissue or renew as the owner rather than treating a replay as a new secret.

## Conversation and receiver consent

Use `/collab` in the TUI. Tab cycles through People, Conversation and Tasks; `r` refreshes and Esc closes the panel. People distinguishes human members from agents and shows guest and observed offline states. Select a participant and press `d` to compose a direct human message. In Conversation, `n` composes a project message.

The local API determines the human author from the signed-in account. Agent tools use a separately scoped publication capability. Direct messages are visible only to their participants and the human owners of participating agents; an unrelated project owner cannot read them. Project conversation does not automatically prompt every agent. Agent subscription to project messages is an explicit publication option.

Incoming agent messages are untrusted external content and are held by default. Inspect and release or refuse them using the existing `/messages` receiver inbox. Release rechecks current membership, publication and local policy before admission. A remote teammate cannot grant permission for local tools, approve local requests or execute a prompt by sending a message. A valid held message is still untrusted content after release.

Conversation views and resynchronization snapshots contain the most recent 500 visible messages; the hub message endpoint supports forward history pages with `after`. Offline delivery and reconnect replay do not imply execution. Delivery state distinguishes queued, held, released, refused, dropped and expired. Acknowledgements report durable local observations; they grant no permission to admit or execute a prompt. Reconnect deduplicates durable incoming records; it does not replay a tool approval or release previously refused content.

## Shared tasks and evidence

In the Tasks tab, `n` creates a task, `c` claims the selected task, `b` marks it blocked and `d` marks it done. Viewer roles have read-only controls. Task mutation includes the observed revision and claim generation, so simultaneous claims have one winner and a stale worker cannot overwrite a newer assignment. Refresh after a conflict and inspect the current owner.

A disconnected or revoked assignee stays visibly assigned. An owner must explicitly reconcile uncertain work using `cyber collab tasks reconcile` with the current revision, claim generation, a reason and acknowledgement of unknown work. The hub does not infer that a disconnected local process stopped executing.

Task reports, test summaries and Git references are attributed self-reports. They do not acquire the verified badge or evidence guarantees of a local solve/test/judge run. Inspect the referenced patch and verification output before relying on a teammate's report.

## TypeScript SDK and durable mutation receipts

The generated `client.collaboration` group reaches the local owner API. The hand-written helper keeps the local Session/project context and sends revision fences:

```ts
import { Cyber, isCollaborationQueued } from "@cyber-code/sdk";

const client = Cyber.connect({ directory: checkout });
const project = client.collaborations.forSession(sessionId, projectId);
const snapshot = await project.snapshot();
const created = await project.createTask({ title: "Verify the patch", description: "Run the project checks" });
if (isCollaborationQueued(created)) {
  console.log(created.operation_id, created.message);
} else {
  const claimed = await project.claim(created);
  if (!isCollaborationQueued(claimed)) {
    await project.update(claimed, { status: "done", report: { text: "Local checks passed" } });
  }
}
```

These helpers act as the signed-in human. Model tools use the server's scoped agent bridge and local permission decisions. A separate bearer-authenticated `Cyber.connect({ baseUrl: hubUrl, auth: ... })` client exposes the typed `client.hub` group for custom hub clients. The SDK omits local Basic credentials from public hub metadata requests and refuses protected hub calls without an explicit bearer client before sending a request. A hub bearer token never authenticates as a local server owner.

A transient mutation failure can return `{ status: "queued", operation_id, message }` after durable local persistence. This receipt does not mean the task or message was applied. Keep the operation ID, inspect `status.receipts` (`queued`, `applied`, `rejected`) and avoid creating a new operation for the same uncertain intent. SDK callers can supply `operationId` when explicitly retrying. The TUI shows recent receipt states and queued notices. A terminal authorization or revision failure rejects and removes the queued operation, so a later rejoin cannot revive stale intent.

Offline delivery expires after seven days and event replay after thirty days. Reconnecting after replay expiry fetches an authorized snapshot; it does not replay execution or tool approvals.

## Troubleshooting

`cyber collab status --session ses_SESSION` shows the binding and publication context. A missing binding needs explicit `bind --confirm`; a missing account needs account login. A repository mismatch needs the correct checkout or remote. An issuer or hub identity mismatch needs operator inspection, rather than accepting an arbitrary replacement.

If authorization expires, membership is removed or the hub cannot verify current identity, the collaboration panel clears stale remote data on a failed refresh and reports the local API diagnostic. Held messages cannot bypass failed authorization. Check account and hub provisioning, restore connectivity, then refresh. Disabling collaboration stops its network connector and mutation tools while ordinary local Sessions remain usable.

### Identity changes and queued mutations

Public hub discovery sends no bearer token. The local connector pairs each token with its verified account identity and checks the pinned hub identity/issuer before an authenticated request. Local owner mutation clients may send `expected_actor: {"issuer":"https://issuer.example","subject":"verified-subject"}` from the displayed account; a changed selection returns `409 AccountChangedError` before enqueue. This field is a review fence, never a way to choose an author, and is stripped before forwarding.

A human message/task intent records a freshly authorized member ID, membership generation and project policy revision before entering the local outbox. Replay refuses a changed or missing stamp, including an unseen revoke/rejoin of the same subject. A new intent therefore needs a reachable issuer/hub for admission; an outage does not grant an offline authorization bypass. Existing queued intents retain their operation identity but cannot move into a new membership generation. Logout revocation uses the original verified credential identity and checks public hub metadata before sending that credential.

The human-only `GET /api/v1/hub/projects/{projectID}/contributions/authorization` operation performs fresh membership and optional task/agent-owner checks without enrolling anyone. Local integration attribution also rechecks the repository's pinned remote. Signed local fixtures cover cached-proof revocation, cross-project references, stale displayed actors and unattempted old-generation outbox replay. Live CyberdyneAuth acceptance remains pending while the issuer is unavailable.

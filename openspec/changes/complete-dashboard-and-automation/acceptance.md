# Live acceptance prerequisites and evidence

This is the execution checklist for the live portions of the goal, not a completed acceptance report. Fixture-backed tests remain useful implementation evidence and cannot satisfy these live rows.

## Observations on 2026-10-24

- The user subsequently confirmed CyberdyneAuth is unavailable due to payment issues and authorized continuing local implementation. Further live Auth probes and authenticated acceptance are on hold until service restoration; this is a user-reported outage, not a diagnosis of the observed TLS alert.
- The initial read-only public Auth metadata check succeeded. Discovery/OpenAPI hashes matched [the earlier contract](../add-project-collaboration-hub/auth-contract.md); `cyber:collaborate` was absent from advertised scopes and device authorization was not advertised.
- A follow-up near **16:02 UTC** failed before HTTP on both discovery and OpenAPI: Python reported `TLSV1_UNRECOGNIZED_NAME`; curl independently reported TLS alert `unrecognized name` (exit 35). This establishes a TLS/SNI failure from this environment at that time, not its cause or permanent deployment state. TLS verification was not disabled.
- No Coolify API URL/token pair or `CYBER_ACCOUNT_*` / `CYBER_COLLAB_*` variables were configured in the current process environment. This does not establish that no credentials exist in another operator session or a native vault. No credential vault was enumerated or printed.
- PR #32 contains planning artifacts only. No user login, refresh, protected introspection, enrollment, invitation, code redemption, membership change, account deactivation or physical-device check has been exercised for this goal.

## Prerequisites to resolve

| Prerequisite | Required input or check | Current evidence |
|---|---|---|
| Artifact review | Review proposal/design/tasks before implementation | Requested; pending user response |
| Issuer transport | Reachable HTTPS issuer with a certificate/virtual host valid for its hostname | Initial fetch succeeded; follow-up TLS alert above requires recheck/operator diagnosis |
| Public CLI client | Registered client ID and allowed loopback `/callback` redirects for PKCE S256 | Local default is `cyber-cli`; live registration unverified |
| API audience and scope | Correct hub access-token audience and `cyber:collaborate` enabled for the client/resource | Local account audience defaults to client ID unless configured; live resource registration unverified; scope absent in successful public snapshot |
| Hub backend identity | Secure service bearer or client-credentials identity authorized to introspect user tokens | Not supplied/verified in this process; never substitute a user or local owner password |
| Hub deployment | Test HTTPS URL, pinned identity, dedicated durable database, bootstrap subject and intended guest/MFA policy | Not designated |
| Test organization | Immutable organization ID and permission to mutate designated test memberships | Not designated |
| Test people | Owner/bootstrap principal, another same-org member and an unrelated valid external account | Not designated; do not repurpose unrelated real memberships |
| Account revocation test | Disposable account/session/token and operator ability to revoke it | Not designated; hub membership removal alone does not prove issuer revocation |
| Second device | Actual phone/tablet/other computer with a trusted certificate path to the owner's dashboard | Not designated; responsive browser emulation does not satisfy this check |

Backend configuration accepts a protected service token or a client ID/secret plus optional scope. Keep these outside the repository and command arguments. Optional `directory:read` is for an organization member picker, not a substitute for live caller introspection. Account and hub audiences must agree with actual issued access tokens; the local default is not evidence of the server's registered audience.

## Live execution matrix

All rows are **not run** until supported by a redacted report containing commit/build, issuer/hub identity, timestamps, test principal/resource identifiers and observed server outcomes. Never include authorization codes, JWTs, refresh tokens, invitation tokens, approval codes, owner passwords or backend secrets in the report.

| Check | What proves success |
|---|---|
| PKCE login and refresh | Real browser authorization returns to the owned loopback callback; validated identity is recorded; an actual refresh rotates/updates credentials without leaking them |
| Current organization authorization | Backend-authenticated introspection returns the active subject and immutable organization set; the hub admits the designated permitted member |
| Unrelated valid user | Valid identity alone cannot read/join the organization project contrary to policy |
| Same-org enrollment | Configured internal enrollment policy works without an extra project approval code; account MFA still applies as configured by Auth |
| External guest admission | The intended external account can join only under allowed guest policy and the bound invitation/application plus one-time approval code |
| Code expiry and replay | Expired, wrong-principal/project and replayed codes cannot activate membership; retry/idempotency does not produce a second admission |
| Project revocation | Removing the designated member prevents new access/delivery/release within the documented authorization window; already held messages cannot bypass it |
| Agent revocation | Unpublishing retires that publication and blocks queued/held admission; republishing cannot revive the old identity |
| Issuer revocation | Revoked test token/session or deactivated disposable user is refused through actual introspection; record cache timing separately from uncached sensitive operations |
| Cleanup | Only test-owned resources are removed/restored, with unknown work preserved for explicit reconciliation rather than assumed stopped |

The existing hub design requires uncached validation for sensitive enrollment/publication/role/policy changes and permits at most 60 seconds of positive reuse for ordinary authorization, bounded by token expiry. Measure the applicable behavior; a successful unrelated request is not revocation evidence.

## Physical-device HTTPS acceptance

Once a suitable deployment is ready:

1. Record actual device model/OS/browser, server build and the user-facing hostname. Keep private keys and owner passwords out of captures.
2. Establish ordinary browser certificate trust on that device and connect over HTTPS without bypassing a certificate warning. A deliberately trusted private CA is acceptable; record that trust setup.
3. Log in to the local owner dashboard, observe a real pending request, approve it and verify completed work from another browser or terminal client.
4. Exercise logout/reload and a disconnect/reconnect; verify stale cards do not approve later requests and no owner password appears in URLs or persistent browser storage.
5. Record result and exact limits. Only then close original dashboard task 5.2; an automated TLS endpoint or SSH-style forward remains separate evidence.

If the device or Auth prerequisites remain unavailable, keep their tasks open and report the dependency. Do not replace live acceptance with a fixture pass or mark the overall goal complete.

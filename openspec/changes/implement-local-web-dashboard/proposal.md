## Why

The local web client is already a P1 target; Relay remote control is P3. Neither has a concrete local dashboard delivery plan. Existing Sessions, pending permissions/questions and workflow state should be accessible from a browser, including another device that can securely reach the user's server, without making the local release depend on an account service or Relay.

## What Changes

- Deliver a responsive browser dashboard in the `cyber` distribution, opened by `cyber web` and driven exclusively by public server APIs.
- First slice: Session list/transcript, steer/queue composer and interrupt, pending permissions/questions, and workflow/Job/Goal status and existing controls. Show child-request origin and the selected Location.
- Support loopback, a user-managed SSH forward, or a TLS-protected LAN/VPN listener. Do not open router ports, create tunnels or require a Cyber Account. Paired Devices, public internet Relay access, mobile apps and push remain P3/P4.
- Reuse local-password Basic authentication with an explicit login form; keep credentials only in page memory and stream through authenticated fetch, never URL tokens or browser storage. Static shell assets may be public; Session data and mutations stay protected.
- Specify browser interactive-presence leases so a live dashboard can serve as an approver and a lost browser cannot leave work waiting forever. Reuse ordinary permission/question reply APIs and idempotency.
- Make pending/stale/offline states explicit. A click is not shown as applied until the server acknowledges it; terminal and browser races resolve through the existing request owner.
- Require TLS off loopback; remove the not-yet-implemented `--insecure-lan` exception from the target web contract. The CLI refuses to silently widen an existing shared listener.

## Capabilities

- MODIFIED `web-client` → Local web client: retain the full eventual TUI-equivalent target, define the first dashboard slice and secure access policy.
- ADDED `web-client`: browser auth, approval semantics, reconnect and responsive/accessibility contracts.
- MODIFIED `server-api` → Local password authentication: allow only the embedded public static shell as a P1 exception; API authentication remains required.
- ADDED `server-api`: expiring interactive browser attachment leases under Session routes.

## Scope and Dependencies

Use the current server, Location routing, pending-request APIs, durable streams and workflow monitoring. The dashboard works before `solve` ships; once it does, candidate evidence is displayed from the same public run report. Work out interactive-client accounting against `permissions-modes` Non-interactive behavior before implementing the lease.

No new account identity, permission-grant format, peer trust model or Relay is introduced. A browser using the local owner password is an owner client, not a scoped peer or paired P3 Device; the UI must state this authority during login. Existing server policy/Mode ceilings still apply.

## Status

Implementation authorized by the owner on 2026-10-24 and delivered in PR #29. Automated browser acceptance and all 14 CI checks pass for implementation commit `81837568`; see `docs/implementation/web-dashboard.md`. Physical second-device certificate trust remains an operator acceptance check. Archive after implementation merge; the eventual full TUI-equivalent target remains outside this first slice.

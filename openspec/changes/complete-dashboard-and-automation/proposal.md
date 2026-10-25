# Complete the dashboard and durable automation

## Why

The first dashboard and project hub are implemented, but teammates still need the terminal for collaboration, configuration and interactive process access. Workflow reservations, finished-agent transcript retention and persistent cron schedules are specified but incomplete. Fixture-backed Auth acceptance does not establish that the live deployment can issue and authorize collaboration credentials.

The user requested one goal covering all four areas on 2026-10-24. This change divides that goal into four independently verifiable increments rather than treating a new screen or a green fixture suite as completion of the entire goal.

## What Changes

1. **Live collaboration acceptance:** recheck the deployment contract, verify required client/audience/scope and backend introspection provisioning, and exercise real login/refresh, organization authorization, guest admission and revocation using designated test principals. Record results without credentials. Fix demonstrated integration defects without weakening authorization.
2. **Browser collaboration:** expose People, Conversation, shared Tasks and held-message inspection/release/refusal through the existing local owner API. Preserve project bindings, verified human/agent attribution, revision fences and receiver consent.
3. **Automation controls:** implement durable, atomic pre-dispatch budget reservations; configurable retention of finished workflow-agent transcripts; in-session cron tools and persistent local schedules with restart, overlap, unattended approval and missed-run policies.
4. **Browser completion:** add schema-backed settings, supported image/PDF attachment upload and previews, and interactive existing PTY Job attachment with input/resize. Validate private HTTPS from a physical second device.

## Capabilities

- Add browser-specific requirements to `web-client` for collaboration, configuration, media and PTY Jobs.
- Add implementation-level reservation lifecycle requirements to `observability-costs` without changing the existing Budget object or its soft default.
- Add transcript retention safeguards to `storage-events`, preserving the existing 30-day default.
- Add durable scheduling occurrence and timezone requirements to `loops-scheduling`, building on its existing CLI/tool contracts.
- Reuse `cyber-account` and `project-collaboration` contracts; live evidence is an acceptance obligation, not a replacement authentication protocol.

## Delivery and scope

Work on `feat/dashboard-automation-completion`, based on main `78c60566`. Each increment must include its relevant tests, generated API/SDK/browser artifacts and user documentation. Review proposal, design and tasks before implementation. Feature availability and acceptance status remain separate.

This goal does not introduce a hosted Relay, cloud runners, automatic remote execution/approvals, automatic merges, GitHub permission synchronization, or a new identity provider. Terminal functionality means an interactive browser emulator for existing local PTY Jobs; it does not silently create a new privileged shell service. Windows PTY availability follows server capabilities.

## External acceptance dependencies

Live acceptance requires designated test users (including an external guest), a test organization, the registered CLI/hub clients and securely supplied backend credentials. Organization removal/deactivation tests must use disposable principals or explicitly designated memberships. A physical second-device trust check requires the user's actual device and reachable TLS deployment.

The successful public check on 2026-10-24 omitted `cyber:collaborate`; its discovery and OpenAPI hashes matched the previous recorded contract. A follow-up near 16:02 UTC failed TLS negotiation from this environment, independently reproduced with Python and curl. See [acceptance prerequisites and evidence](acceptance.md). These are deployment dependencies, not proof that authenticated issuance was attempted. No live authentication, membership changes or device checks have been performed for this change.

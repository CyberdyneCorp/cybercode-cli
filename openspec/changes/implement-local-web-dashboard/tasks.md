## 1. Review and bundle foundation

- [x] 1.1 Review proposal/design, owner-password authority, TLS-only remote binding and the P1/P3 boundary before implementation.
- [x] 1.2 Identify current interactive-client accounting and integrate the lease with Non-interactive behavior without affecting TUI/SDK attachments.
- [x] 1.3 Add TypeScript client build, browser-safe API schemas, embedded artifacts and CI freshness/build checks; no Node runtime dependency in the distributed CLI.
- [x] 1.4 Public shell and assets only; authenticated APIs, no-store data responses, strict CSP and inert Markdown/diff/log rendering.

## 2. Secure access and read-only dashboard

- [x] 2.1 Login with explicit Basic headers, page-memory credentials, credentials-omit fetch/SSE; logout/reload/auth failure clears state and no credential storage/query tokens.
- [x] 2.2 Implement `cyber web` startup, registered URL, `--no-open`, requested bind checks, TLS refusal and actionable mismatch errors without widening a running listener.
- [ ] 2.3 Session/Location navigation, paginated transcript/diffs, existing workflow/Job/Goal status, feature probes and unavailable-state handling.
- [x] 2.4 Production-path/browser tests: unauthenticated data refused, login accepted, no secrets in URLs/storage/logs, hostile Markdown inert, no static `/api` fallback and denied Origin rejected.

## 3. Human presence and requests

- [x] 3.1 Authenticated idempotent lease create/renew/delete, 15-second renewal/45-second expiry, Session/Location authority and restart invalidation.
- [x] 3.2 Browser Session attach/switch/logout, prompt steer/queue and interrupt under existing ceilings.
- [x] 3.3 Pending-request inbox with child origin and command/diff/scope; permission and question replies only through their current public APIs.
- [x] 3.4 Tests: two browsers race with a terminal reply, lost acknowledgement retries same key, stale request never approves a new one, rejected headless asks stay rejected, expiry preserves other clients and applies Mode policy.

## 4. Recovery and responsive UX

- [x] 4.1 Durable event cursors, live-state refetch, 500 ms–15 s reconnect backoff, visible offline/auth states and disabled stale actions.
- [x] 4.2 Existing run/task/goal controls and `solve` evidence when the server exposes it; no automatic candidate application or merge.
- [x] 4.3 Accessible keyboard flows, labelled decisions, approval count, focus handling and 360/1280 CSS-pixel layouts.
- [x] 4.4 Browser screenshots/tests for mobile/desktop, reconnect, server restart, lease expiration and long command/diff rendering.

## 5. Acceptance and docs

- [x] 5.1 Real `cyber web` plus scripted model: prompt → permission → browser approval → completed tool, observed by the terminal and second browser.
- [ ] 5.2 Physical second-device certificate trust remains a manual deployment acceptance check. Automated production-browser approval through an SSH-style TCP forward and an actual TLS endpoint, plus off-loopback plaintext refusal, pass; these do not establish physical-device trust.
- [ ] 5.3 Confirm the peer listener never serves the dashboard or accepts owner-password auth, and existing API/SDK/TUI contracts still pass.
- [ ] 5.4 Publish setup, password authority, TLS/tunnel requirements, disconnect behavior, limitations and exact browser/platform evidence. Update feature/roadmap status only after acceptance; archive after review.

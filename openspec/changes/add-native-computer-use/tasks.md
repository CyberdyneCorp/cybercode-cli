## 1. Review and native spike

- [x] 1.1 Inspect browser/native scope and official macOS capture/input/OS-consent APIs; user selected macOS first with explicit unsupported-platform behavior.
- [x] 1.2 Draft proposal, design and contract deltas, separating native automation from the desktop wrapper.
- [x] 1.3 Review artifacts, owner-control boundaries and initial platform/version with the user before implementation. On 2026-10-24 the user explicitly answered “Approve both designs and implement” after receiving both design links in PR #32; macOS first was selected separately.
- [ ] 1.4 Verify maintained native bindings, build/signing requirements and ScreenCaptureKit/CoreGraphics adapter packaging on macOS without requiring a runtime compiler.

## 2. Capability and authority

- [x] 2.1 Add global-only experimental feature/config/schema validation and typed native capability/OS-consent health.
- [x] 2.2 Add owner-armed Session/target/process-bound expiring grants, one-host-input lease, human-presence checks and non-inheritance by children/hub agents.
- [x] 2.3 Integrate `computer.capture`/`computer.control` with Modes/permissions, exact target review and denial behavior; prevent bypass from arming/widening a grant.
- [x] 2.4 Add independent emergency Stop and expiry/interrupt/logout/restart/OS-revocation fencing with bounded native calls and key/button cleanup.

## 3. Native actions and evidence

- [x] 3.1 Implement bounded ScreenCaptureKit capture and server-owned frame metadata, coordinates, scale/rotation/target identity and image modality handling.
- [x] 3.2 Implement move/click/drag/scroll/Unicode text/key/wait actions through the adapter with frame freshness, target/focus/bounds checks and action limits.
- [x] 3.3 Persist native intent/settlement/idempotency, preserve unknown outcomes and prohibit automatic replay after uncertain input.
- [x] 3.4 Add private artifact retention and authenticated previews; redact raw typed text from durable tool arguments/logs/exports and document image/provider privacy.

## 4. Client surfaces

- [x] 4.1 Register `cyber computer status|setup|allow|stop`, owner-only typed API and generated SDK operations with explicit unsupported-platform/version messages.
- [x] 4.2 Add model tool and guidance, TUI/dashboard active-control banners, target/expiry review, native result evidence and immediate Stop.
- [x] 4.3 Update permissions/config/doctor/setup guides, README mindmap/roadmap and platform/signing documentation.

## 5. Verification

- [x] 5.1 Test fake-adapter grant/Mode/consent/frame/geometry/focus checks with zero input on rejection, concurrency/rate limits and stale owner actions.
- [x] 5.2 Test interruption before/after native dispatch, exactly-once receipt settlement, unknown recovery, revocation and emergency stop under a busy model/tool queue.
- [ ] 5.3 Test capture limits, malformed keys/text, artifact reference isolation, typed-text redaction and unsupported platform absence of side effects.
- [ ] 5.4 With explicit controlled-test consent and OS grants, verify real macOS screenshot, move/click/drag/scroll/text/key editing, coordinate transforms and Stop in a disposable test window; report evidence separately from fixtures.
- [ ] 5.5 Validate OpenSpec, generated contracts/bundles, relevant Rust/browser/SDK suites and native/cross-platform CI; perform code/security review before ready-to-merge.

## Implementation evidence and remaining acceptance

- Compiled native Objective-C adapter uses public Apple SDK APIs, links into the server, and dynamically loads ScreenCaptureKit for explicit unsupported-version behavior. Native syntax/build checks passed; signed/notarized distribution and physical-device behavior remain unaccepted (1.4/5.4).
- Deterministic local checks: computer domain18, global-only configuration2, runtime15, exact-Session presence3, real TCP owner/privacy2, model schema1, TUI1, CLI/doctor2 and owned Session-presence stream1 passed. A separate core settings unit test passed. Strict Clippy passed across cyber-computer/core/server/tools/app/client/cli/tui with all targets and warnings denied.
- Browser modules/transport44 tests and typecheck passed; SDK117 tests passed with8 process tests skipped because CYBER_BIN was not supplied. OpenAPI export and SDK generation/typecheck passed. Only aarch64-apple-darwin is installed locally; unsupported-platform execution and the wider workspace suites await CI (5.3/5.5).
- Dashboard constructor/view/logout and CSP/blob preview are integrated. Browser module/transport checks and three focused Chromium scenarios against the frozen built CLI with mocked native HTTP responses verify global cross-Session authority, explicit review/arming, private PNG preview and logout Stop without invoking the OS adapter.
- No real desktop capture, input injection, OS permission prompt or native device acceptance has been performed. Task5.4 remains blocked on explicit controlled-test participation and OS grants; fixtures must not be described as physical acceptance.

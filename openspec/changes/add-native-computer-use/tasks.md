## 1. Review and native spike

- [x] 1.1 Inspect browser/native scope and official macOS capture/input/OS-consent APIs; user selected macOS first with explicit unsupported-platform behavior.
- [x] 1.2 Draft proposal, design and contract deltas, separating native automation from the desktop wrapper.
- [x] 1.3 Review artifacts, owner-control boundaries and initial platform/version with the user before implementation. On 2026-10-24 the user explicitly answered “Approve both designs and implement” after receiving both design links in PR #32; macOS first was selected separately.
- [ ] 1.4 Verify maintained native bindings, build/signing requirements and ScreenCaptureKit/CoreGraphics adapter packaging on macOS without requiring a runtime compiler.

## 2. Capability and authority

- [ ] 2.1 Add global-only experimental feature/config/schema validation and typed native capability/OS-consent health.
- [ ] 2.2 Add owner-armed Session/target/process-bound expiring grants, one-host-input lease, human-presence checks and non-inheritance by children/hub agents.
- [ ] 2.3 Integrate `computer.capture`/`computer.control` with Modes/permissions, exact target review and denial behavior; prevent bypass from arming/widening a grant.
- [ ] 2.4 Add independent emergency Stop and expiry/interrupt/logout/restart/OS-revocation fencing with bounded native calls and key/button cleanup.

## 3. Native actions and evidence

- [ ] 3.1 Implement bounded ScreenCaptureKit capture and server-owned frame metadata, coordinates, scale/rotation/target identity and image modality handling.
- [ ] 3.2 Implement move/click/drag/scroll/Unicode text/key/wait actions through the adapter with frame freshness, target/focus/bounds checks and action limits.
- [ ] 3.3 Persist native intent/settlement/idempotency, preserve unknown outcomes and prohibit automatic replay after uncertain input.
- [ ] 3.4 Add private artifact retention and authenticated previews; redact raw typed text from durable tool arguments/logs/exports and document image/provider privacy.

## 4. Client surfaces

- [ ] 4.1 Register `cyber computer status|setup|allow|stop`, owner-only typed API and generated SDK operations with explicit unsupported-platform/version messages.
- [ ] 4.2 Add model tool and guidance, TUI/dashboard active-control banners, target/expiry review, native result evidence and immediate Stop.
- [ ] 4.3 Update permissions/config/doctor/setup guides, README mindmap/roadmap and platform/signing documentation.

## 5. Verification

- [ ] 5.1 Test fake-adapter grant/Mode/consent/frame/geometry/focus checks with zero input on rejection, concurrency/rate limits and stale owner actions.
- [ ] 5.2 Test interruption before/after native dispatch, exactly-once receipt settlement, unknown recovery, revocation and emergency stop under a busy model/tool queue.
- [ ] 5.3 Test capture limits, malformed keys/text, artifact reference isolation, typed-text redaction and unsupported platform absence of side effects.
- [ ] 5.4 With explicit controlled-test consent and OS grants, verify real macOS screenshot, move/click/drag/scroll/text/key editing, coordinate transforms and Stop in a disposable test window; report evidence separately from fixtures.
- [ ] 5.5 Validate OpenSpec, generated contracts/bundles, relevant Rust/browser/SDK suites and native/cross-platform CI; perform code/security review before ready-to-merge.

# Native computer acceptance

Status: automated implementation checks passed; **controlled live application capture passed, mouse/keyboard acceptance pending Accessibility permission**. The owner explicitly authorized the disposable-window test on 2026-10-25. OS setup was deliberately requested; restarting only the isolated server enabled Screen Recording. No native input has been injected.

## Local evidence

| Check | Result |
|---|---|
| `cargo test -p cyber-computer --lib` | 18 passed: geometry, input bounds/redaction, expiring grants, rate limits, cross-controller/session fencing, supervisor and independent Stop races |
| `cargo test -p cyber-core --test config native` | 2 passed: global-only authority and managed feature locks |
| Server `runtime::computer::tests` | 16 passed: private artifacts, incomplete writes, retention, durable intent/settlement/replay/unknown acknowledgement, cross-Runtime host lease and busy-worker Stop |
| `cargo test -p cyber-server --test computer` | 3 passed with fake native adapter: Session deletion with retention disabled, real TCP auth/Location/target review/global Stop, self-approval and model/agent/Mode widening refusal, durable typed-text privacy |
| Exact Session presence | 3 passed, including disconnect/reconnect and lease expiry before reattachment; grants and outstanding reviews cannot revive |
| Tool schema, TUI control state, CLI/doctor, client-owned presence stream | 5 focused tests passed; one core settings unit also passed |
| Clippy | Passed for all targets of cyber-computer/core/server/tools/app/client/cli/tui with warnings denied; final TUI/helper changes checked again |
| macOS adapter | Compiled with local SDK; standalone Objective-C syntax check passed. This does not prove native capture/input behavior. |
| OpenAPI / SDK generation | Export test and generator consistency passed; TypeScript typecheck passed |
| SDK suite | 124 passed against the frozen CLI; 2 account/hub cases skip macOS to avoid native Keychain writes; no live account/provider claims |
| Browser modules / private transport | 44 passed; typecheck and generated bundle check passed |

Three focused Chromium dashboard scenarios passed against the frozen built CLI and its embedded assets, including global cross-Session banner, target review/arming, authenticated private PNG rendering under the actual CSP and logout Stop. Native HTTP responses were simulated; the fixture did not call an OS adapter or request consent. The three doctor/feature-list regressions exposed by the first CI run also pass locally.

Independent review identified and fixed reconnecting grants or target reviews, a missing final drag Stop checkpoint, model/agent authority switching during control, and screenshot cleanup that released quota before deleting retained bytes. Regression tests cover stale reviews during blocked target resolution, observed OS revocation/restoration, failed cleanup and a cleanup backlog exceeding 500 rows. Final platform CI results remain a separate merge gate.

Only `aarch64-apple-darwin` is installed locally. Unsupported-platform execution, other architectures, signed distribution and full workspace verification rely on CI/release acceptance; they are not claimed as locally tested. Test counts are for this change's recorded checks, not current repository-wide totals.

## Controlled live capture evidence (2026-10-25)

- Host: macOS 26.7, arm64. Opaque disposable Cocoa application only; no personal applications or whole-display capture.
- Frozen executable SHA-256: `0fa107ad9ea6db0736979bf1ac955065c6ce52f8b047a76288452e23e7f10556`. It remained unchanged through OS setup and the isolated-server restart. Later Session-deletion changes are covered by separate regression tests; these captures do not claim to exercise a subsequently rebuilt executable.
- Two real `computer` tool screenshots passed through the production runtime and ScreenCaptureKit adapter, followed by acknowledged global Stop. Frame dimensions were 1200×1144 pixels for a 600×572-point window (2× scale, rotation 0).
- First frame: `cuf_01M5RSYVXM576NP22H2KBSEVF5`, 201274 bytes, SHA-256 `93b698905badcbd125797fe663d3d0f10cc26e44d16647c0364ba19a5011d711`.
- Visible-canary capture after a fixture-only appearance correction: `cuf_01M5RT7C2YY1GVP1CWT1KGM9M8`, 218961 bytes, SHA-256 `3bfdbcfaa942cd0b7e9d69ab87209996b659c881f286fa3340cfb74b35f0f78a`.
- Both images were inspected privately and contained only the fixture. Button/counter, text field, scroll rows and drag canvas were visible in the second image. Screenshot bytes are not published as evidence.
- The scripted loopback vision provider received an authorized PNG projection. No external provider or paid-model request was used.
- Read-only native health after restart: Screen Recording granted, Accessibility denied. Mouse/key injection, occlusion/focus/geometry negatives and Stop during actual input remain unperformed; permission settings alone are not recorded as a passing input test.

## Controlled device plan for owner review

Before running this plan, the owner must explicitly agree to the controlled test, keep the computer available and grant macOS consent to the exact built server binary. Use a disposable application window with fixture text/buttons, no credentials and no unrelated private content. Prefer an isolated browser profile or a new TextEdit document; never automate an unrelated open application. Capture only that app/window.

1. Build the reviewed branch, start its server and inspect `cyber computer status`. Request OS consent only through deliberate `cyber computer setup --yes`, after owner agreement.
2. Keep the exact test Session attached. Review and arm the disposable application's bundle ID with `cyber computer allow <session> --app <bundle-id> --yes`. Bring its test window to the front before capture/input.
3. Capture the test window. Verify dimensions, hash, app/PID/window/display identity and coordinate mapping using known visible fixture targets. Inspect the image through its authenticated private-frame endpoint, not an external upload.
4. On fresh frames, verify move, left/right/double-click, drag, scroll, Unicode typing, symbolic keys and bounded wait. Check the actual fixture state after each action and retain the matching receipts.
5. Cover part of the captured application with another app's floating window (and then a different window from the same app); verify move/click/drag/scroll refuse the covered hit target. Use only disposable fixture overlays; no whole-display captures. Open a simulated fixture sheet/dialog with an ordinary button and verify keyboard/pointer refusal through its restricted ancestor chain. Change foreground app/window or geometry and try the old frame. Confirm no input occurs. Separately test Screen Recording/Accessibility denial and revocation; regranting OS consent must not replay an old operation.
6. During a bounded drag/type/wait, invoke this server's global Stop API/CLI from an independent owner client. Confirm the native worker exits, releases keys/buttons where possible and cannot transfer its host lease while still alive. Test the focused TUI shortcut separately; it is not an OS-global hotkey.
7. Attempt direct self-approval against the harness: pending Bash/permission/question requests fence native events; Mode/admin widening requires Stop; known Cyber TUI/dashboard windows are refused as focused native targets. Inspect actual behavior rather than trusting browser `isTrusted`.
8. Test a second display, Retina/non-Retina scale, negative origins and rotation only on displays the owner explicitly includes in the controlled test. Record unsupported/untested layouts honestly.
9. Stop all test grants and close the disposable window/profile. Check private retention/expiry and that ordinary exports do not include screenshot bytes. Do not publish private captures as acceptance evidence without separate owner review.

Record actual binary identity, OS/display configuration, permission decisions, fixture results and operation IDs here after execution. Fixture tests must remain distinct from these physical results.

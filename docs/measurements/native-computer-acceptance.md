# Native computer acceptance

Status: implementation checks passed; **physical macOS acceptance pending**. No real desktop capture, input injection or operating-system consent prompt has been performed for this change.

## Local evidence

| Check | Result |
|---|---|
| `cargo test -p cyber-computer --lib` | 15 passed: geometry, input bounds/redaction, expiring grants, rate limits, cross-controller/session fencing, supervisor and independent Stop races |
| `cargo test -p cyber-core --test config native` | 2 passed: global-only authority and managed feature locks |
| Server `runtime::computer::tests` | 15 passed: private artifacts, incomplete writes, retention, durable intent/settlement/replay/unknown acknowledgement, cross-Runtime host lease and busy-worker Stop |
| `cargo test -p cyber-server --test computer` | 2 passed with fake native adapter: real TCP auth/Location/target review/global Stop, self-approval and model/agent/Mode widening refusal, durable typed-text privacy |
| Exact Session presence | 3 passed, including disconnect/reconnect and lease expiry before reattachment; grants and outstanding reviews cannot revive |
| Tool schema, TUI control state, CLI/doctor, client-owned presence stream | 5 focused tests passed; one core settings unit also passed |
| Clippy | Passed for all targets of cyber-computer/core/server/tools/app/client/cli/tui with warnings denied; final TUI/helper changes checked again |
| macOS adapter | Compiled with local SDK; standalone Objective-C syntax check passed. This does not prove native capture/input behavior. |
| OpenAPI / SDK generation | Export test and generator consistency passed; TypeScript typecheck passed |
| SDK suite | 117 passed, 8 skipped because `CYBER_BIN` was unset; no live account/provider claims |
| Browser modules / private transport | 40 passed; typecheck and generated bundle check passed |

Three focused Chromium dashboard scenarios passed against the frozen built CLI and its embedded assets, including global cross-Session banner, target review/arming, authenticated private PNG rendering under the actual CSP and logout Stop. Native HTTP responses were simulated; the fixture did not call an OS adapter or request consent. The three doctor/feature-list regressions exposed by the first CI run also pass locally.

Independent review identified and fixed reconnecting grants or target reviews, a missing final drag Stop checkpoint, model/agent authority switching during control, and screenshot cleanup that released quota before deleting retained bytes. Regression tests cover stale reviews during blocked target resolution, observed OS revocation/restoration, failed cleanup and a cleanup backlog exceeding 500 rows. Final platform CI results remain a separate merge gate.

Only `aarch64-apple-darwin` is installed locally. Unsupported-platform execution, other architectures, signed distribution and full workspace verification rely on CI/release acceptance; they are not claimed as locally tested. Test counts are for this change's recorded checks, not current repository-wide totals.

## Controlled device plan for owner review

Before running this plan, the owner must explicitly agree to the controlled test, keep the computer available and grant macOS consent to the exact built server binary. Use a disposable application window with fixture text/buttons, no credentials and no unrelated private content. Prefer an isolated browser profile or a new TextEdit document; never automate an unrelated open application. Capture only that app/window.

1. Build the reviewed branch, start its server and inspect `cyber computer status`. Request OS consent only through deliberate `cyber computer setup --yes`, after owner agreement.
2. Keep the exact test Session attached. Review and arm the disposable application's bundle ID with `cyber computer allow <session> --app <bundle-id> --yes`. Bring its test window to the front before capture/input.
3. Capture the test window. Verify dimensions, hash, app/PID/window/display identity and coordinate mapping using known visible fixture targets. Inspect the image through its authenticated private-frame endpoint, not an external upload.
4. On fresh frames, verify move, left/right/double-click, drag, scroll, Unicode typing, symbolic keys and bounded wait. Check the actual fixture state after each action and retain the matching receipts.
5. Change foreground app/window or geometry and try the old frame. Confirm no input occurs. Separately test Screen Recording/Accessibility denial and revocation; regranting OS consent must not replay an old operation.
6. During a bounded drag/type/wait, invoke this server's global Stop API/CLI from an independent owner client. Confirm the native worker exits, releases keys/buttons where possible and cannot transfer its host lease while still alive. Test the focused TUI shortcut separately; it is not an OS-global hotkey.
7. Attempt direct self-approval against the harness: pending Bash/permission/question requests fence native events; Mode/admin widening requires Stop; known Cyber TUI/dashboard windows are refused as focused native targets. Inspect actual behavior rather than trusting browser `isTrusted`.
8. Test a second display, Retina/non-Retina scale, negative origins and rotation only on displays the owner explicitly includes in the controlled test. Record unsupported/untested layouts honestly.
9. Stop all test grants and close the disposable window/profile. Check private retention/expiry and that ordinary exports do not include screenshot bytes. Do not publish private captures as acceptance evidence without separate owner review.

Record actual binary identity, OS/display configuration, permission decisions, fixture results and operation IDs here after execution. Fixture tests must remain distinct from these physical results.

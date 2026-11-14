## 0. Review gate

- [x] 0.1 Read issue #44, the macOS design/specs, the computer crates, runtime, HTTP routes, guide and acceptance record. Draft proposal, design, threat model, spec deltas and this plan. No code.
- [ ] 0.2 Owner reviews this change and answers decisions D1–D12 in design §9. Record each answer and its date in this file. Implementation does not start before this.
- [ ] 0.3 Agree the shared foundation (design §4) with the Linux #43 design owners; record the agreed field names here.

## 1. Spikes (Windows 11 x64 device, disposable fixture only, results recorded in design §10)

- [ ] 1.1 S1 capture authority: `CreateForWindow` from an unpackaged medium-IL process on 25H2 and 26H2. Record prompts, privacy settings, policy denial and border. Update design §5.1 and D2.
- [ ] 1.2 S2 geometry: WGC `ContentSize` vs DWM extended frame bounds for Win32/WinUI/WPF windows at 100/125/150/200 %, mixed-DPI, negative-origin and rotated layouts.
- [ ] 1.3 S3 input mapping: absolute virtual-desktop normalization error and `GetCursorPos` agreement on the same layouts under a PMv2 thread.
- [ ] 1.4 S4 UIA bounds: response and timeout behavior against a deliberately hung provider fixture. Confirm Stop latency is independent of it.

## 2. Shared foundation PR (portable, all OSes)

- [ ] 2.1 Health v2 fields and reason codes (additive), with OpenAPI/SDK regeneration. macOS fields unchanged. Test: schema snapshot, and macOS health serialization unchanged.
- [ ] 2.2 Opaque `target_id` and deprecated numeric fields; `Selector::Window(candidate)`; platform-neutral `UnsupportedPlatform` message with reason. Tests: caller-supplied handle/PID/path rejected; candidate expiry; JS-safe serialization (no 64-bit numbers); macOS selector behavior unchanged.
- [ ] 2.3 Owner-only `GET /api/v1/tools/computer/targets` and `cyber computer targets`, `allow --window`. `--app`/`--display` give an unsupported-selector error on Windows. Tests: route needs owner credential and is absent from model tool, hub and peer surfaces; output not persisted.
- [ ] 2.4 Update the cli-commands registry and help text. Update `docs/computer-use.md` for the new fields; no Windows support claim yet.

## 3. Windows adapter: portable policy (safe Rust, unit-tested on every OS)

- [ ] 3.1 Integrity/elevation/uiAccess/session/user classification functions. Tests: elevated, uiAccess, AppContainer, low IL, Session 0, unknown IL → refuse; target token user differs from the owner or is unreadable → `target_other_user`.
- [ ] 3.2 Target-binding comparison (session, desktop, user SID, PID, creation time, HWND, class, unowned, image-path hash). Tests: PID reuse, HWND reuse across processes, same title/new process, class change, different user SID, target gaining an owner.
- [ ] 3.3 UIA control-type/ancestry allowlist and password/dialog classifier over recorded property tuples (no live UIA). Tests: password at any ancestor depth, dialog, unknown type, mixed PIDs, cycle, depth > 32, timeout; an element hosted in a popup window owned by the target (same or other process) is refused.
- [ ] 3.4 Chord policy (Win/Alt/clipboard refusal, Control editing allowlist) and UTF-16 chunking with surrogate pairs. Tests: supplementary characters never split across chunks; refused chords produce zero events.
- [ ] 3.5 Physical-pixel normalization for absolute virtual-desktop input, and topology epoch. Tests: negative origins, multi-monitor extents, rounding bounds, epoch change makes frames stale without revoking the grant.

## 4. Windows adapter: native (cfg(windows), behind the existing Backend trait)

- [ ] 4.1 Thread model: MTA capture and assurance threads, message-only session-watch window, bounded request channels, PMv2 thread context. Tests: deterministic fault injection for a hung request; Stop returns without waiting; the cancelled worker keeps its lease until return.
- [ ] 4.2 Health (read-only): build/arch matrix, `IsSupported`, controller token, session/WTS state, remote metrics, input desktop name, watcher liveness, advanced-color detection. Test: `health(false)` performs no capture, title enumeration, input or OS UI (instrumented adapter).
- [ ] 4.3 Target listing and resolve with the full binding; preview fields (title, image, publisher, IL, DPI, affinity/cloak/minimized).
- [ ] 4.4 WGC capture: single frame, `ContentSize` crop, row pitch, BGRA→RGBA, downscale, bounded PNG; refusal table (affinity, minimized, cloaked, HDR, device loss, resize, item closed); release on the owning thread; late-callback safety. Tests: synthetic texture padding never appears in output; late `FrameArrived` after cancel is dropped.
- [ ] 4.5 Per-event assurance sequence (design §5.3) and `SendInput` dispatch with count checks, cursor verification, text chunk focus rechecks, release tracker and drop guard.
- [ ] 4.6 Session latch: WTS notifications, desktop-switch WinEvent hook, remote metrics, watcher-loss handling; latch stops all grants and never resurrects.
- [ ] 4.7 Cross-process lease: `Local\` named mutex with an owner-only DACL on a dedicated lease thread; abandoned-mutex handling (record `previous_holder_abandoned`, owner-visible cleanup-uncertain warning, no access to another home's journal). Tests: two processes with different homes; crash of home A while held, then home B arms (warning shown, A's journal untouched) and A restarts (its intents settle unknown); squatted name → Busy; health does not acquire or wait on the mutex.
- [ ] 4.8 Sandbox regression: a Windows AppContainer child cannot read the capture root; the capability set never includes capture capabilities.

## 5. Runtime, surfaces and docs

- [ ] 5.1 Runtime `selector_for_identity` Windows branch; journal and receipts carry `target_id`; reason codes in errors. Existing fake-adapter suites run unchanged on Windows CI.
- [ ] 5.2 TUI/dashboard target review shows the Windows preview fields; health reasons rendered; Stop and banners unchanged.
- [ ] 5.3 Docs: Windows section in `docs/computer-use.md` (supported matrix, owner-review authority, refusals, residual races, no global hotkey); README known gaps; roadmap; `docs/measurements/windows-computer-acceptance.md` template.

## 6. Verification and acceptance

- [ ] 6.1 Portable/domain tests (fake adapter) on Linux/macOS/Windows CI: zero events on rejected grant/presence/target/frame; Stop races; per-event fencing; partial input; crash/replay/unknown receipts; handle reuse and pointer-width serialization; private bytes/redaction/retention/deletion; safe callback lifetimes.
- [ ] 6.2 Windows CI (`windows-2025` x64): build, strict Clippy, unit and fault-injection tests for the adapter's policy modules, plus the existing AppContainer/sandbox suites unchanged. `windows-11-arm`: build and the same unit tests. ARM64 support stays unclaimed (D1).
- [ ] 6.3 Controlled Windows acceptance (section below), recorded in the acceptance document with private evidence kept local.
- [ ] 6.4 OpenSpec strict validation, spec lint/inventory, OpenAPI/SDK checks, browser tests, security review on the final merge candidate.
- [ ] 6.5 Release gate (D11): Authenticode signing and Windows packaging verified, or experimental delivery explicitly approved and documented. Until then health on release builds reports Windows as unaccepted.

## Acceptance plan: evidence each requirement needs

What CI can and cannot prove:
- Hosted `windows-2025` runners do provide an auto-logon interactive desktop, and UI-automation suites run on them. They still do not count as acceptance: the runner account's token is an elevated administrator, so the controller check (design §5.2) refuses it as `controller_elevated`, and the image is Windows Server 2025, not a supported Windows 11 client build.
- CI therefore proves compilation, policy logic, fault injection, schema and sandbox regressions, and the absence of side effects on refusal paths.
- CI does **not** prove capture, input delivery, UIPI behavior, DPI mapping, UIA behavior on real apps, session latching or Stop on a desktop.
- No hosted-CI result is cited as desktop acceptance.

| Requirement | CI evidence | Controlled real-desktop evidence |
|---|---|---|
| Native computer capability and consent | Unsupported-platform/arch/build results with no side effects (all OSes) | Health on the accepted build shows owner-review authority and opens no OS UI |
| Platform-neutral computer health | Schema snapshot; reason-code unit tests; instrumented `health(false)` | Locked session, elevated-server and remote-session health readings |
| Pointer-safe native target identity | Serialization and rejection tests; candidate expiry at preview and allow | Candidate listing excludes Cyber/elevated/other-user windows; expired candidate refused |
| Windows target binding | Binding comparison unit tests | `runas` window of another account refused; display topology change makes frames stale without revoking; lock/unlock, logoff, fast-user-switch, disconnect, target restart with the same title, window destroy/recreate |
| Windows window capture | Readback/crop/encode tests on synthetic textures; late-callback test | Fixture capture at 100/125/150/200 %; capture-excluded fixture window refused; minimized/cloaked refused; HDR monitor refused (if available, else recorded pending); device reset if reproducible, else pending |
| Windows target assurance before every event | Classifier/allowlist unit tests; fault-injected UIA timeouts | Same-process and other-process topmost and transparent overlays; another process's topmost window owned by the target; the target's own opened menu and drop-down refused; password field; fixture modal dialog; manually prepared elevated fixture (UAC never automated); focus change mid-type; Cyber TUI/terminal and dashboard windows refused |
| Windows native input dispatch | Normalization, chunking, chord policy; zero events on refusal | Move, left/right/double click, drag, scroll, Unicode type (Greek, CJK, surrogate-pair emoji), named keys and Control chords on two keyboard layouts (one with IME), held-modifier cleanup, Stop during drag/type/wait |
| Windows cross-process input lease | Two-process and abandoned-mutex tests on Windows CI | Two server homes on one desktop; crash while holding, then the other home arms and sees the cleanup warning |
| Windows support evidence gate | Health reports unaccepted build without an acceptance record | Acceptance record lists OS build, architecture, GPU, DPI layout, session type, binary SHA-256 and signature status, operation IDs and results; untested rows stay pending |
| Sandboxed processes hold no native computer authority | AppContainer capability verification; capture-root read denial | — (CI is sufficient; the device run repeats it once) |
| Computer use owner commands | CLI parsing/help/registry tests | `targets`, `allow --window`, `--display` refusal and Stop on the device |

Controlled acceptance procedure:
1. Get explicit owner agreement and device approval.
2. Use a dedicated local Windows 11 x64 user session with only the disposable fixture apps (Win32, WinUI 3, WPF) and no personal content.
3. Record the build, architecture, GPU, DPI layout and the binary hash.
4. Run every action through the real model tool, runtime and owner API with a loopback vision provider.
5. Keep screenshots and raw canaries private. Publish only operation IDs, hashes and results.
6. Never capture unrelated windows or drive real UAC/security dialogs.
7. Record each untestable row (for example HDR or device loss when the hardware lacks it) explicitly as pending.

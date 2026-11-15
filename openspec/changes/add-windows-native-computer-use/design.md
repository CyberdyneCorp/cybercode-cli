# Design: Windows native computer use

Status: **proposal for review, 2026-11-14.** Nothing here is implemented. Statements about Windows behavior cite Microsoft documentation. Statements marked *(inference)* are this design's reading of that documentation, or engineering judgment. A spike or acceptance task must confirm them before anyone relies on them. API availability on a Windows build does not mean Cyber supports that build.

> **Depends on #134** ([`add-computer-use-platform-foundation`](../add-computer-use-platform-foundation/design.md), decisions F1–F12, approved 2026-11-15). The platform-neutral contract now lives there: the capability report (F1, F3), the reason-code registry (F2), selectors and target identity (F4), review-bound arming (F5), the grant lifecycle and host lease (F6), schema versioning (F7), the shared chord policy (F8), the native worker boundary (F9), receipts (F10), backend port additions (F11) and the evidence gate (F12). This design keeps only Windows mechanisms and Windows decisions, and references those requirements instead of restating them. It must not merge before #134. What moved is listed in the foundation's §14 and in §4 below.

## 1. Context

The approved macOS design ([add-native-computer-use](../add-native-computer-use/design.md)) and its implementation define these seams. This design keeps all of them.

| Seam | Where | What Windows reuses |
|---|---|---|
| Native port | `crates/cyber-computer/src/backend.rs` (`Backend`: `acquire_lease`, `health(prompt)`, `resolve`, `capture`, `input`) | The trait as extended by the foundation (F11: default `review`, `end_scope`, `set_revocation_sink`). A new `windows` module implements it: `review` resolves the candidate (no picker), `end_scope` closes any open capture session, and the session latch (§5.7) reports through the revocation sink. Non-macOS, non-Windows, non-Linux `Native` stays unsupported. |
| Authority | `control.rs` (`Controller`, `Grant`, `Frame`, `CapturePermit`, `InputPermit`, `Admission`) | Unchanged: owner-only arming, presence epochs, generation fencing, 300 s default / 900 s max TTL, 10 inputs/s, 300 per grant, 10 s frame age, 16 fresh frames, supervisor fence for pending human requests. |
| Actions and geometry | `action.rs`, `geometry.rs` | Unchanged action set and limits. `Geometry` maps image pixels to desktop coordinates. On Windows, desktop coordinates are physical pixels (§5.6). |
| Runtime | `crates/cyber-server/src/runtime/computer` (journal intent/settlement, unknown outcomes, private artifacts, retention, deletion fence, host lease holder, preview/allow review) | Unchanged. Windows changes only what `Backend` returns. |
| Owner API | `crates/cyber-server/src/http/computer.rs` (`/tools/computer/{health,setup,stop}`, `/sessions/{id}/computer/{preview,allow,stop,frames/{frame}}`) | Kept. Adds a read-only owner route `/api/v1/tools/computer/targets` for window candidates (§6.3). |
| Build | `crates/cyber-computer/build.rs` compiles `native/macos.m` on macOS only | Windows uses Rust FFI through the `windows` crate. No C shim; the build script stays macOS-only. |
| Windows command sandbox | `crates/cyber-sandbox/src/windows_launch.rs`, `windows_container.rs` | A separate feature. It never gains capture or input authority (§5.5). |

## 2. Goals and non-goals

Goals: let an authenticated, present owner grant one Session time-limited control of one reviewed top-level application window on a supported Windows 11 desktop. Under that grant, Cyber takes private window-only screenshots and sends bounded mouse/Unicode-keyboard input. Stop works independently of the agent. Every guarantee that macOS gives on authority, privacy and uncertain outcomes still holds.

Non-goals are listed in [proposal.md](proposal.md#non-goals). The most important: no elevation, `uiAccess`, SYSTEM/service desktop, UAC/credential/lock-screen automation, UIPI or capture-exclusion bypass, forced foreground, background-window input, message-posting or script fallbacks, clipboard typing, and no display grant in the first release.

## 3. Architecture

```
owner CLI/TUI/dashboard ──(owner credential)──▶ http/computer.rs ──▶ runtime/computer (journal, artifacts, review)
model `computer` tool ──(scoped runtime bridge)──▶ runtime/computer ──▶ Controller (cyber-computer::control)
                                                             │
                                                             ▼  spawn_blocking worker, holds CapturePermit/InputPermit
                                               Backend (trait) ──▶ windows::Native
                                                             ├─ CaptureThread  (MTA; D3D11 device + WGC, one frame at a time)
                                                             ├─ AssuranceThread (MTA; UIA client with bounded timeouts)
                                                             ├─ SessionWatch   (message-only window: WTS + display/DPI notifications)
                                                             └─ dispatch on the calling worker thread (PMv2 DPI context; SendInput)
```

- **In process, no helper (recommended, decision D3).** The worker contract (owns every handle, nothing inheritable, Stop never waits, lease outlives the native call, late callbacks dropped by generation, pressed keys and buttons released) is the foundation's native worker boundary (F9). Windows chooses the in-process topology that F9 allows: the adapter runs inside the server process at the owner's medium integrity level, like the macOS adapter. A helper would need a channel and process checks (§5.10) for no isolation gain on Windows. Unless the owner decides otherwise, the first release has no helper or broker.
- **Dedicated threads.** Microsoft advises UI Automation clients not to call UIA from a thread that owns UI, and to use MTA threads ([UIA threading](https://learn.microsoft.com/en-us/windows/win32/winauto/uiauto-threading)). The adapter therefore owns one MTA assurance thread with no windows. Capture runs on its own MTA thread with a free-threaded frame pool *(inference: WGC's `Direct3D11CaptureFramePool.CreateFreeThreaded` avoids needing a DispatcherQueue; the spike confirms this)*. Session notifications need an HWND ([WM_WTSSESSION_CHANGE](https://learn.microsoft.com/en-us/windows/win32/termserv/wm-wtssession-change)), so a third thread owns a message-only window. None of these threads hold a Rust permit. They receive owned request messages and return owned results through bounded channels.
- **Calls are bounded and Stop stays independent.** Each cross-thread request has a deadline no longer than the remaining action budget (5 s), and Stop does not wait for a reply. If a request times out, the worker settles `unknown` (input) or fails (capture) and keeps its `InputPermit`/host lease until the native call returns. This matches the existing "an in-flight cancelled worker retains the lease until it really stops" contract. A hung UIA provider can therefore block only its own worker. It cannot block Stop, and it cannot let a second worker acquire the lease. UIA connection and transaction timeouts are set explicitly ([ConnectionTimeout](https://learn.microsoft.com/en-us/windows/win32/api/uiautomationclient/nf-uiautomationclient-iuiautomation2-put_connectiontimeout), default 2 s; [TransactionTimeout](https://learn.microsoft.com/en-us/windows/win32/api/uiautomationclient/nf-uiautomationclient-iuiautomation2-put_transactiontimeout)). Proposed values are 250 ms each, tuned in the spike.
- **Late callbacks are safe.** WGC `FrameArrived` and `Closed` handlers, and WinEvent hooks, capture only `Arc`/channel senders and a generation number. They never capture borrowed Rust references or permits. A callback that arrives after cancellation finds a closed channel or a stale generation and drops its frame. Each frame pool, capture session, D3D texture and UIA element is closed or released on the thread that created it, in reverse creation order, including on error paths.
- **Unsafe code is confined.** `#![allow(unsafe_code)]` is used only in the Windows backend module, as in `macos.rs`. Pure policy functions (integrity classification, control-type allowlist, chord allowlist, coordinate normalization, target-binding comparison) stay safe Rust and are unit-tested on every OS.

## 4. Shared foundation (#134)

This section used to propose the shared types (decision D12). They moved to the foundation and were approved there; Windows now only fills them in.

| Former §4 item | Foundation | Windows value |
|---|---|---|
| 1. Health v2 | F1 capability report, F3 authority kinds | `backend: "windows-wgc"`; `authority: { kind: "owner_review_only", setup_available: false }` unless spike S1 finds a real OS access request (§5.1); `os.build` carries the Windows build; `session` from WTS state (§5.7); `selectors: ["window"]`; legacy `accessibility` = `input.available`, `screen_capture` = `capture.available`. The draft's `observation` is now `capture`, and `reason` is now `reasons[]`. |
| 1. Reason codes | F2 registry | Renames: `unsupported_selector` → `selector_unsupported_on_platform`; `unsupported_os_build` → `unsupported_os_version` (build in `os.build`); `host_lease_unavailable` split into `host_lease_busy` (held by another holder) and `host_lease_unavailable` (cannot be created); `capture_excluded` → `target_capture_excluded`; `capture_unsupported` → `capture_unavailable`. `controller_elevated`, `controller_ui_access` and `controller_integrity_unsupported` stay as Windows detail codes under the shared `controller_privilege_unsupported`. Windows-only codes (below) are registered by this change. |
| 2. Pointer-safe target identity | F4 | `target_id` and `source: { platform: "windows", kind: "window", backend: "windows-wgc", detail }`, with `detail` holding owner-display strings only (image name, publisher, IL, DPI). Legacy `pid`, `window`, `display` are zero/null. No `HWND` or `HMONITOR` value crosses the HTTP/SDK boundary. |
| 3. Selector | F4, F5 | `window` (candidate token, 30 s) is the only selector; `app` and `display` are refused with `selector_unsupported_on_platform`. `window` is not re-resolvable, so `allow` arms only by `preview_id` (§6.2). |
| 4. Error text | F2 | `Error::UnsupportedPlatform` is platform-neutral with a code. |
| 5. `Target.identity` | F4 | Windows owns the `win:` prefix: `win:<session-id>:<desktop-name-hash>:<user-sid-hash>:<pid>:<process-creation-filetime>:<hwnd-value>:<image-path-hash>`. Server-internal; never parsed from a request. `selector_for_identity` gains a `win:` branch that maps back to the in-memory candidate. |

Windows-only reason codes, by registry category:

| Category | Codes |
|---|---|
| Authority (detail of `controller_privilege_unsupported`) | `controller_elevated`, `controller_ui_access`, `controller_integrity_unsupported` |
| Backend | `gpu_unavailable`, `hdr_display_unsupported` |
| Target | `target_minimized`, `target_cloaked`, `target_integrity_unknown` |

Shared codes Windows uses include `unsupported_os_version`, `unsupported_architecture`, `remote_session`, `non_interactive_session`, `session_locked`, `session_watch_unavailable`, `capture_policy_denied`, `target_gone`, `target_elevated`, `target_other_user`, `target_capture_excluded`, `target_not_foreground`, `secure_field`, `frame_stale`, `topology_changed`, `key_chord_refused`, `host_lease_busy`, `host_lease_unavailable` and `previous_holder_abandoned`.

## 5. Platform mechanisms

### 5.1 Capture: Windows.Graphics.Capture (WGC)

**Mechanism.** For an approved HWND, create a `GraphicsCaptureItem` with `IGraphicsCaptureItemInterop::CreateForWindow`. Its documented minimum is Windows 10 1903, build 18362 ([CreateForWindow](https://learn.microsoft.com/en-us/windows/win32/api/windows.graphics.capture.interop/nf-windows-graphics-capture-interop-igraphicscaptureiteminterop-createforwindow)). The adapter creates a D3D11 device, a free-threaded frame pool (B8G8R8A8), and a session with `IsCursorCaptureEnabled = false` (available from 2004, [docs](https://learn.microsoft.com/en-us/uwp/api/windows.graphics.capture.graphicscapturesession.iscursorcaptureenabled)). It waits for one frame and closes the session immediately. The readback steps:

- copy into a CPU-readable staging texture;
- crop to the frame's `ContentSize`, the content size when the frame was rendered ([ContentSize](https://learn.microsoft.com/en-us/uwp/api/windows.graphics.capture.direct3d11captureframe.contentsize)), never to the pool's texture size, so undefined padding or old surface contents are never exposed;
- honour the row pitch;
- convert BGRA to RGBA, downscale so the longest edge is at most 1920 px, and PNG-encode within 8 MiB using the existing `Capture::validate` contract.

**Resize and device loss.** If `ContentSize` differs from the item size the adapter observed at request time, or the device is removed ([GetDeviceRemovedReason](https://learn.microsoft.com/en-us/windows/win32/api/d3d11/nf-d3d11-id3d11device-getdeviceremovedreason)), the frame is discarded and capture fails with a stale-target or `gpu_unavailable` error. A frame is promoted only after the target's current bounds, read again, match the bounds recorded in `NativeTarget`.

**System capture indication.** Microsoft's capture guide says the system draws a yellow border around each actively captured item ([screen capture](https://learn.microsoft.com/en-us/windows/apps/develop/media-authoring-processing/screen-capture)). Turning it off requires `GraphicsCaptureAccess.RequestAccessAsync(Borderless)` and the `graphicsCaptureWithoutBorder` package capability ([RequestAccessAsync](https://learn.microsoft.com/en-us/uwp/api/windows.graphics.capture.graphicscaptureaccess.requestaccessasync), [IsBorderRequired](https://learn.microsoft.com/en-us/uwp/api/windows.graphics.capture.graphicscapturesession.isborderrequired)). Cyber **never** requests borderless access and never sets `IsBorderRequired = false`.

**Authority path: picker vs programmatic (decision D2).** The guide shows the `GraphicsCapturePicker` system UI as the way for the user to pick an item ([GraphicsCapturePicker](https://learn.microsoft.com/en-us/uwp/api/windows.graphics.capture.graphicscapturepicker)). A desktop app must initialize the picker with an owner HWND. `GraphicsCaptureAccessKind` also has a `Programmatic` value ([GraphicsCaptureAccessKind](https://learn.microsoft.com/en-us/uwp/api/windows.graphics.capture.graphicscaptureaccesskind)). Its documented conditions cover packaged apps. Its behavior for an unpackaged medium-IL CLI is not documented on the pages reviewed here.

- *(inference, spike S1)* An unpackaged desktop process at medium IL can call `CreateForWindow` without a picker or an OS consent prompt. Windows may add privacy settings or policy that change this. The spike records what actually happens on 25H2 and 26H2 in four cases: unpackaged, packaged with identity, policy-restricted and default.
- **Recommended:** programmatic `CreateForWindow` for the HWND that the owner reviewed in Cyber. Health then reports `authority.kind = "owner_review_only"` and `setup_available = false`, unless the spike shows a real OS access request. In that case `cyber computer setup --yes` is the only path that may invoke it. Owner target review in Cyber is always required. If the OS denies programmatic access (`capture_policy_denied`), Cyber does **not** fall back to the picker silently, to DXGI or to any other path.
- **Alternative:** require the picker on every grant. It gives OS-mediated selection, but returns a `GraphicsCaptureItem` with no documented way to get its HWND *(inference)*. Input binding would then need a heuristic match, which this design rejects. It also needs a Cyber-owned visible HWND, which the CLI and background server do not have.
- `health(prompt=false)` never opens a picker, captures pixels or sends input. Support is checked with `GraphicsCaptureSession::IsSupported` ([IsSupported](https://learn.microsoft.com/en-us/uwp/api/windows.graphics.capture.graphicscapturesession.issupported)), together with the OS build and session checks.

**DXGI Desktop Duplication comparison.** `DuplicateOutput` works per output, that is per monitor, not per window. Within a single session it allows at most four concurrent duplication clients by default and returns `DXGI_ERROR_SESSION_DISCONNECTED` for disconnected sessions. It also documents that only `LOCAL_SYSTEM` can access the secure desktop ([DuplicateOutput](https://learn.microsoft.com/en-us/windows/win32/api/dxgi1_2/nf-dxgi1_2-idxgioutput1-duplicateoutput), [Desktop Duplication API](https://learn.microsoft.com/en-us/windows/win32/direct3ddxgi/desktop-dup-api)). It cannot give window-only capture without capturing and cropping a whole monitor, which exposes other windows' pixels in process memory. It is **not used** for window scope and never serves as a fallback. If display scope is approved later (D4), DXGI and WGC `CreateForMonitor` are compared again under that separate owner scope.

**Honest refusal states.** Each state is reported as a capture refusal and never worked around:

| State | Detection | Result |
|---|---|---|
| Capture-excluded target | `GetWindowDisplayAffinity` returns `WDA_EXCLUDEFROMCAPTURE` or `WDA_MONITOR` ([GetWindowDisplayAffinity](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-getwindowdisplayaffinity)) | `target_capture_excluded` refusal before capture. Microsoft notes display affinity is not a security or DRM guarantee ([SetWindowDisplayAffinity](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-setwindowdisplayaffinity)), so Cyber treats it as the owner/app's stated intent and respects it. |
| Minimized | `IsIconic` | `target_minimized` |
| Cloaked (other virtual desktop, suspended UWP, shell-cloaked) | `DwmGetWindowAttribute(DWMWA_CLOAKED)` ([DWMWINDOWATTRIBUTE](https://learn.microsoft.com/en-us/windows/win32/api/dwmapi/ne-dwmapi-dwmwindowattribute)) | `target_cloaked` |
| Blank or protected content | Frame arrives but the app protects its content *(inference: protected media appears black)* | Cyber cannot detect this reliably. The frame is returned as observed, and the docs say that blank frames may be protected content. No retry through another API. |
| HDR or advanced-color monitor under the target | DXGI output color space ([HDR guidance](https://learn.microsoft.com/en-us/windows/win32/direct3darticles/high-dynamic-range)) | `hdr_display_unsupported` in the first release (decision D7) |
| Device removed or reset | D3D device-removed reason | `gpu_unavailable`; the next capture creates a new device |
| Item closed (window destroyed) | `GraphicsCaptureItem.Closed` | Grant revoked as `target_gone` (§6.2) |

### 5.2 Input: SendInput, UIPI and integrity levels

**Facts.** `SendInput` returns how many events it inserted. A return of zero means the input was blocked. When UIPI blocks it, neither the return value nor `GetLastError` says so. Applications may inject input only into applications at an equal or lower integrity level. Events from one call are inserted serially and are not interleaved with other input ([SendInput](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-sendinput)). Windows defines low, medium, high and system integrity levels ([Mandatory Integrity Control](https://learn.microsoft.com/en-us/windows/win32/secauthz/mandatory-integrity-control)). An application without `uiAccess` starts at medium IL and cannot reach elevated UI ([UIA security overview](https://learn.microsoft.com/en-us/windows/win32/winauto/uiauto-securityoverview)).

**Controller checks** (health and every admission):

- The server token is not elevated (`TokenElevation`). Its IL is exactly medium (`TokenIntegrityLevel`). It has no `TokenUIAccess` and is not an AppContainer token ([GetTokenInformation](https://learn.microsoft.com/en-us/windows/win32/api/securitybaseapi/nf-securitybaseapi-gettokeninformation)). An elevated or `uiAccess` token gives `controller_privilege_unsupported` with detail `controller_elevated` or `controller_ui_access`. An elevated Cyber server could drive elevated applications and undermine the UIPI boundary this design relies on, so the server **refuses** rather than taking on that authority.
- A low-IL or AppContainer controller token is refused as `controller_privilege_unsupported` with detail `controller_integrity_unsupported`. UIPI would silently block its input to medium-IL windows, so what it observes would not match what it can act on.
- The process session is not 0 ([ProcessIdToSessionId](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-processidtosessionid)). Session 0 is isolated from interactive desktops ([interactive services](https://learn.microsoft.com/en-us/windows/win32/services/interactive-services)), and Cyber never runs an interactive service or SYSTEM broker.

**Target checks.** The adapter opens the target process with `PROCESS_QUERY_LIMITED_INFORMATION` and its token with `TOKEN_QUERY` ([OpenProcessToken](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-openprocesstoken)). It reads the token IL, elevation and user SID (`TokenUser`). The target is refused when:
- the IL is above medium, or cannot be read (access denied, protected process): `target_elevated` or `target_integrity_unknown`;
- the token user SID is not the owner's SID, or cannot be read: `target_other_user`. A medium-IL window started under another account in the same session (for example with `runas`) is therefore refused as an explicit binding rule, not only because `OpenProcess` or token access happens to fail.

Because UIPI failures are silent, this pre-dispatch check is the only reliable signal. Cyber never retries through another mechanism, and never auto-elevates, lowers UAC or enables `uiAccess`.

**Delivery semantics.** Each atomic step is one `SendInput` call: for example, button down, or one UTF-16 unit's down and up. If the returned count is less than the requested count, the action is settled `unknown`, cleanup is attempted and the grant stops, as on macOS. A full count means the events reached the input stream. It does **not** mean the application acted on them. Receipts say `posted`, as on macOS, never "clicked" or "typed".

**Pointer actions.**
- Absolute moves use `MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK`. Physical virtual-desktop coordinates are normalized to 0–65535 against the current `SM_XVIRTUALSCREEN`/`SM_CXVIRTUALSCREEN` metrics ([MOUSEINPUT](https://learn.microsoft.com/en-us/windows/win32/api/winuser/ns-winuser-mouseinput), [GetSystemMetrics](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-getsystemmetrics)). Normalization is recomputed at dispatch time and never cached in the frame.
- After each move and before any button event, the adapter reads `GetCursorPos`. If the cursor is more than 1 physical pixel from the intended point (rounding, pointer acceleration, a concurrent user move), the action stops as `unknown` before the button event. *(inference: under a PMv2 thread `GetCursorPos` returns physical pixels; spike S3 verifies this on mixed-DPI layouts.)*
- Click count 1–2 and right/middle buttons map directly. Scroll uses `MOUSEEVENTF_WHEEL`/`HWHEEL` in `WHEEL_DELTA` steps capped by the existing 2000 limit.
- Drag is a down, a bounded series of moves and an up. The full target check (§6.1) runs before **every** move in the path, not only at the start.

**Text.** Unicode text is sent as `KEYEVENTF_UNICODE` key down/up pairs, one per UTF-16 code unit. Supplementary characters therefore become two surrogate units. The system synthesizes `VK_PACKET` and delivers the character to the **foreground thread's** queue ([KEYBDINPUT](https://learn.microsoft.com/en-us/windows/win32/api/winuser/ns-winuser-keybdinput)). The clipboard is never used. Since delivery follows the foreground thread rather than a window, the focus check (§6.1) is repeated before every chunk of at most 16 UTF-16 units, and the cancellation flag is checked between units. If any check fails mid-string, the action becomes `unknown` and the rest is not sent.

**Symbolic keys.** Named keys map to fixed virtual keys, with `KEYEVENTF_EXTENDEDKEY` for navigation keys. ASCII letters and digits resolve through `VkKeyScanExW` with the target thread's layout from `GetKeyboardLayout(target_thread)` ([VkKeyScanExW](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-vkkeyscanexw), [GetKeyboardLayout](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-getkeyboardlayout)). If the layout needs a modifier the chord did not name, Cyber refuses, as on macOS (the foundation's shared layout-safety rule).

**Modifiers and chords (foundation F8).** Which chords are allowed is decided by the foundation's shared, platform-neutral chord evaluator before admission and before any journal intent (refusal: `key_chord_refused`, grant stays armed). Windows supplies its rows of the shared table and does not keep a separate list:
- `control` is the primary editing modifier. Under window scope a modified chord is allowed only when its key is in the shared edit-key list: `a`, `z`, `y`, `s`, `left`, `right`, `up`, `down`, `home`, `end`, `backspace`, `delete`. This is the macOS list (F8a), so Ctrl+S and Ctrl+Up/Down are allowed, which this draft first excluded.
- `command` (the Windows key) is the system modifier and is refused in every scope, because Win-key chords are shell-global.
- `alt` is refused under window scope: Alt+Tab, Alt+F4, Alt+Space and menu accelerators can leave the window or close it.
- The global deny list (Ctrl+Alt+Del, Ctrl+Shift+Esc, Ctrl+Esc, lock chords, Alt+Tab, application-quit chords) applies in every scope.
- Clipboard chords Ctrl+C/X/V are refused because `c`, `x` and `v` are not edit keys. A paste would inject whatever the owner last copied into a model-driven window, which leaks data across the owner/model boundary without the model ever reading the clipboard. This is the same as macOS, not stricter: macOS already refuses Command+C/X/V under app grants, because they are not in its edit-key allowlist (`native/macos.m`, `key()`). Under the shared table the platforms match.

Ctrl+Alt+Del is the secure attention sequence and cannot be injected *(inference: SAS is handled by Winlogon and is not produced by `SendInput`)*. The chord is refused anyway.

**Cleanup.** The worker tracks every button and key it has pressed and has not released. On cancel, timeout, check failure or panic unwinding (a drop guard), it sends the matching up events in reverse order. If cleanup's `SendInput` count is short, or the input desktop changed so that cleanup cannot be delivered, the receipt records `cleanup: "uncertain"` (foundation F10). Cyber does not read global key state to "verify" release, because that comes close to keylogging. The receipt never claims that delivered input was undone.

### 5.3 Target assurance with Win32 and UI Automation

What each mechanism can and cannot prove:

| Question | Mechanism | Strength | Gap / refusal |
|---|---|---|---|
| Is the reviewed window still the same object? | `IsWindow`; `GetWindowThreadProcessId` → PID; `GetProcessTimes` creation time; image path from `QueryFullProcessImageNameW`; process session ID; token user SID ([GetWindowThreadProcessId](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-getwindowthreadprocessid), [GetProcessTimes](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-getprocesstimes), [QueryFullProcessImageNameW](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-queryfullprocessimagenamew)) | Strong. PID plus creation time defeats PID reuse; HWND plus owning PID/creation time defeats HWND reuse by another process. | HWND reuse inside the same process lifetime cannot be told apart from a new window of the same app *(inference)*. Mitigation: also compare the window class name and require that the target is still unowned (`GetWindow(GW_OWNER)` is NULL). Any mismatch → `target_gone`; the grant is revoked and needs a new review. |
| Is it foreground? | `GetForegroundWindow` ([doc](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-getforegroundwindow)) + `GetAncestor(GA_ROOT)` ([doc](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-getancestor)) | Strong at the instant read. | NULL during activation changes → refuse. Cyber never calls `SetForegroundWindow`; the owner brings the window forward. |
| Which window has keyboard focus? | `GetGUIThreadInfo(target thread)` `hwndFocus` and active window ([doc](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-getguithreadinfo)), plus UIA `GetFocusedElement` ([doc](https://learn.microsoft.com/en-us/windows/win32/api/uiautomationclient/nf-uiautomationclient-iuiautomation-getfocusedelement)) | Medium. Win32 says which HWND has focus; UIA says which element. | Windowless frameworks (Chromium, WPF, XAML) report one HWND for the whole surface; the element-level answer depends on the app's UIA provider. If the providers disagree or nothing comes back → refuse. |
| What would receive the click at this point? | `WindowFromPoint` ([doc](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-windowfrompoint)) → root ancestor must equal the target; UIA `ElementFromPoint` ([doc](https://learn.microsoft.com/en-us/windows/win32/api/uiautomationclient/nf-uiautomationclient-iuiautomation-elementfrompoint)) → its `ProcessId` and `NativeWindowHandle` ancestry must resolve to the target | Medium. `WindowFromPoint` skips hidden and disabled windows. Transparent or layered windows may or may not be hit *(inference)*. `ElementFromPoint` can fail with `UIA_E_ELEMENTNOTAVAILABLE` when the element disappears. | Any disagreement, a hit owned by another PID, or an unavailable element → refuse with no retry. A hit test alone is not treated as a complete gate. |
| Is there an overlay on top? | Enumerate top-level windows above the target in z-order (`GetWindow(GW_HWNDPREV)` walk); for each visible, non-cloaked window whose DWM extended frame bounds intersect the action point, refuse unless it **is** the target HWND. Owner links are never used to admit a window (see "Owned popups" below). | Medium. Topmost, transparent and click-through overlays from other processes are found by z-order and bounds, not by hit testing. | Refuses some harmless cases: tooltips, IME candidate windows, and the target's own menus and drop-downs. Accepted, because over-refusal is safe. |
| Is the focused or hit element a password or secure field? | UIA `IsPassword` ([property IDs](https://learn.microsoft.com/en-us/windows/win32/winauto/uiauto-automation-element-propids)) on the element and up to 8 ancestors; the Win32 `ES_PASSWORD` style on Edit HWNDs | Weak to medium. `IsPassword` is what the provider *reports*. A custom-drawn field can omit it. | `IsPassword = true` → refuse. An absent flag is **not** proof of safety, so keyboard input also needs the element's control type to be on the allowlist (below). Element values are never read to classify a field. |
| Is it a dialog, modal or protected surface? | Win32 class `#32770` (standard dialog); a disabled owner window (modal loop); UIA `IsDialog` where the provider sets it; `ControlType = Window` with `WindowPattern.IsModal`; the process image on the protected list | Medium. | Any → refuse (matches macOS refusal of dialogs, sheets and modals). |
| Is it an OS security, credential or consent surface? | Secure desktop: the input-desktop name is not the bound desktop (§5.7). Elevated `consent.exe` or credential UI on the normal desktop when secure-desktop prompting is disabled by policy: refused by the IL check. Unelevated credential brokers and Windows Security: process-image denylist (system path plus Microsoft Authenticode publisher) | Strong for secure desktop and elevated UI; medium for the denylist. | Unknown system processes with UI under `%SystemRoot%` are refused by default (allowlist only the fixture during acceptance). |
| Is it Cyber's own UI (self-approval)? | Every Cyber-owned process (server, CLI/TUI console host, dashboard browser window whose title or URL matches the server's origin) is refused as a target; the server-side supervisor fence blocks native input while any permission/question is pending; admin/Mode/agent changes require Stop | Strong server-side; medium for browser-window recognition | The dashboard runs in an ordinary browser. Window-title matching is heuristic, so the server-side fence is the real guarantee, as on macOS. A terminal hosting the TUI (Windows Terminal, conhost) is refused while any Cyber TUI is attached *(decision D9)*. |
| Is the element type supported? | UIA `ControlType` allowlist for keyboard focus: Edit, Document, the edit part of an editable ComboBox, plus Spinner. For pointer hits it adds Button, CheckBox, RadioButton, ListItem, TreeItem, TabItem, MenuBar items drawn inside the target HWND, ScrollBar, Slider, Hyperlink, DataItem, Text, Image, Pane, Group, Custom-inside-allowlisted-ancestor. Every element must resolve to the target HWND itself; elements hosted in a separate popup window are refused by step 5 | Classification only. | Unknown or inconsistent ancestry (root not the target HWND, mixed PIDs, cycles, depth > 32, timeout) → refuse. |

**Per-event sequence** (each pointer event, each text chunk and each key chord). Everything in it is read again from the OS:

1. `InputPermit.cancelled()` is false (Stop, presence, expiry, supervisor fence).
2. Session/desktop latch not tripped (§5.7). Topology epoch unchanged (§5.6).
3. Target window identity matches (row 1), including the token user SID. Not minimized or cloaked. DWM extended frame bounds equal the frame's recorded bounds.
4. Foreground root equals the target. For keyboard, the GUI-thread focus HWND is in the target's tree, and the UIA focused element passes the password, dialog and allowlist checks.
5. For pointer, the point is inside the target bounds, the `WindowFromPoint` root equals the target, the overlay walk is clean, and `ElementFromPoint` ancestry is consistent and allowlisted.
6. The target IL is still medium or below, and its token user is still the owner.
7. Dispatch exactly one atomic step, check the inserted count, then go to the next step.

**Owned popups, menus and drop-downs (first release).** In Win32, an opened menu (`#32768`), a combo-box drop-down list, a tooltip and many context or autocomplete popups are separate top-level windows owned by the target. Win32 lets any process create a window whose owner is another process's HWND, so an owner link proves nothing about who created the popup *(inference from [window features: owned windows](https://learn.microsoft.com/en-us/windows/win32/winmsg/window-features#owned-windows), which places no same-process restriction on the owner)*. The first release therefore **refuses every owned popup**, from the target's process or any other: pointer events whose point lies in one fail the overlay walk and the `WindowFromPoint` root check, and keyboard input whose GUI-thread focus or foreground root is not the target HWND fails step 4. Opening a menu or drop-down by clicking the target is allowed; acting inside the opened popup is not. A later change may admit owned popups only under a reviewed rule (same PID **and** creation time as the target, owner chain checked to the target, not a dialog or modal, allowlisted class) with its own acceptance.

Observation and dispatch are separate system calls, so a window can appear between step 5 and step 7. This race cannot be closed with documented user-mode APIs. The time between them is kept small, and the race is documented as residual (§8). The server-side fences protect Cyber's own approval surfaces regardless.

**Supported application classes (first release, decision D10).** Classic Win32 (User32 common controls), WinUI 3 / UWP XAML apps with standard controls, and WPF standard controls. Each must pass on the acceptance fixture before support is claimed for that class. Chromium/Electron and custom-drawn apps are "untested; refusals expected". One fixture passing never generalizes to every Windows application.

### 5.4 Secure desktop, UAC and credential UI

- UAC consent and credential prompts appear on the secure desktop by default, and only Windows processes can access it ([How UAC works](https://learn.microsoft.com/en-us/windows/security/application-security/application-control/user-account-control/how-it-works)). When the input desktop is not the desktop Cyber is bound to, the latch trips and every grant is revoked (§5.7). Cyber cannot capture or inject there, and does not try.
- If policy turns off secure-desktop prompting, the prompt is drawn on the normal desktop by an elevated process. The IL check refuses it as a target. A window-only capture cannot include it, because it is a different window. A future display scope (D4) would have to treat it as visible content, which is one reason display scope is deferred.
- The lock screen and sign-in run on the Winlogon desktop. Lock trips the latch (§5.7).
- Cyber never drives real UAC or security dialogs, even in acceptance. Acceptance uses a **manually** prepared elevated fixture to prove the refusal.

### 5.5 AppContainer command sandbox

The Windows sandbox runs commands in invocation-owned AppContainer profiles with ACL leases (`windows_container.rs`, `windows_launch.rs`). Microsoft describes AppContainer window isolation as stopping the contained app from affecting other applications' interfaces ([AppContainer isolation](https://learn.microsoft.com/en-us/windows/win32/secauthz/appcontainer-isolation)). For this feature:

- The computer adapter never runs inside an AppContainer, and an AppContainer child never receives a capture or input capability. `graphicsCapture`-type capabilities are never added to the sandbox capability set, and the sandbox launcher keeps its existing check that refuses unexpected capabilities.
- Private capture PNGs and owner state live in the hidden state directory. The existing hidden-path registration covers the Windows capture root as it does on macOS/Linux, and the existing ACL design gives the AppContainer SID no access to it. A regression test proves a sandboxed child cannot read another Session's capture on Windows.
- A child running with **full access** (outside the sandbox) is ordinary owner-authority code. It can call `SendInput` itself, create overlays or impersonate windows. The design does not claim to contain it (§8). This matches the macOS statement that the feature is not OS privilege separation.
- *(inference)* AppContainer children run at low IL, so UIPI also blocks them from injecting into medium-IL windows. Cyber does **not** rely on this.

### 5.6 Coordinates, DPI, multi-monitor, rotation and hotplug

- **DPI model (decision D6).** The adapter sets `DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2` on its own worker threads with `SetThreadDpiAwarenessContext` ([doc](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-setthreaddpiawarenesscontext), [contexts](https://learn.microsoft.com/en-us/windows/win32/hidpi/dpi-awareness-context)). It does this before creating any window and before every coordinate query. A per-monitor-aware thread sees every display in physical pixels without virtualization ([high-DPI guide](https://learn.microsoft.com/en-us/windows/win32/hidpi/high-dpi-desktop-application-development-on-windows)). The rest of the CLI/TUI process keeps its current awareness, so the process manifest does not change *(recommended; the alternative is a manifest-level PMv2 declaration)*.
- **Desktop space** on Windows means physical pixels of the virtual screen, which can have negative origins. `NativeTarget.x/y/width/height` hold the target's DWM extended frame bounds in that space. `GetWindowRect` is DPI-virtualized and may include invisible resize borders; DWM extended frame bounds are not DPI-adjusted ([GetWindowRect](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-getwindowrect)). The PMv2 thread makes both physical. The adapter uses DWM bounds and checks that the WGC `ContentSize` matches them *(inference: WGC window content matches the DWM visible frame; spike S2 measures any border/shadow offset and records a fixed transform or refuses)*.
- **Image transform.** The existing `Geometry` maps an image pixel to `bounds.origin + p × bounds.size / image.size`. The Windows backend reports `rotation = 0` for window scope, because the window is captured as composed on the desktop *(inference, spike S2 on a rotated monitor)*. Monitor orientation is reported in health metadata only.
- **Scale factor.** Each frame records the target monitor's DPI from `GetDpiForWindow` ([doc](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-getdpiforwindow)) for display only. Coordinates never depend on it, because everything is physical.
- **Hotplug and topology.** The epoch rule is the foundation's topology-change class (F6): frames with an older epoch are `frame_stale` and the grant stays armed. Windows supplies the event sources. The session-watch window handles `WM_DISPLAYCHANGE` ([doc](https://learn.microsoft.com/en-us/windows/win32/gdi/wm-displaychange)) and monitors `WM_DPICHANGED`/settings changes. Each one bumps a process **topology epoch**. Frames record that epoch. A topology change makes every existing frame stale but does **not** revoke the grant: the target binding (§5.7, §5.3 row 1) is unchanged, so the model must take a new screenshot before the next coordinate action. An action whose per-event check sees a new epoch is refused before its first event, or settled `unknown` if earlier events of that action were already dispatched. The virtual-screen rectangle used for `SendInput` normalization can change while the target's own rectangle stays the same, so a bounds comparison alone is not enough.
- **A window spanning monitors with different DPI** is allowed, since physical coordinates are continuous. It is part of the acceptance matrix.

### 5.7 Interactive session and desktop binding

At grant time the target binding records:
- the server's session ID;
- the window station and desktop name of the input desktop, from `OpenInputDesktop` + `GetUserObjectInformationW(UOI_NAME)` ([OpenInputDesktop](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-openinputdesktop), [GetUserObjectInformationW](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-getuserobjectinformationw)), which must be `WinSta0\Default`;
- the target process's session ID, which must equal the server's.

A **revocation latch** trips, and stops every grant (generation bump plus `stop_all`), on any of:
- `WTS_SESSION_LOCK`, `WTS_CONSOLE_DISCONNECT`, `WTS_REMOTE_CONNECT`/`DISCONNECT`, `WTS_SESSION_LOGOFF` or `WTS_SESSION_REMOTE_CONTROL` from `WTSRegisterSessionNotification` ([WM_WTSSESSION_CHANGE](https://learn.microsoft.com/en-us/windows/win32/termserv/wm-wtssession-change));
- `EVENT_SYSTEM_DESKTOPSWITCH` from `SetWinEventHook` ([doc](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-setwineventhook), [event constants](https://learn.microsoft.com/en-us/windows/win32/winauto/event-constants));
- the input-desktop name differing on any per-event check;
- `GetSystemMetrics(SM_REMOTESESSION)` or `SM_REMOTECONTROL` becoming non-zero.

Unlock and reconnect do **not** clear the latch. They only allow a fresh owner review and grant, the same non-resurrecting rule as presence epochs.

An `OpenInputDesktop` handle alone is not proof of a connected session: for a disconnected session it returns the desktop that becomes active when the user reconnects ([doc](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-openinputdesktop)). The adapter therefore also requires the WTS connect state to be active ([WTSQuerySessionInformationW](https://learn.microsoft.com/en-us/windows/win32/api/wtsapi32/nf-wtsapi32-wtsquerysessioninformationw) `WTSConnectState == WTSActive`) and the latch to be clear.

If notifications are lost (the session-watch thread dies or a hook registration fails), health reports `session_watch_unavailable` and input is refused. Per-event checks still run, but losing the early signal is treated as a failure.

RDP and remote sessions are unsupported in the first release (decision D5). Refusing them with `remote_session` is the foundation's rule for platforms added after macOS (F6); Windows detects them with `SM_REMOTESESSION` / `SM_REMOTECONTROL` and the WTS remote events above. Every latch event is binding loss under F6, reported through the revocation sink.

### 5.8 Cross-process host input lease

The rule (one input holder per OS user and interactive session across homes, health never takes the lease, abandoned-lease warning, each home settles only its own journal) is the foundation's (F6). This section is the Windows mechanism. A file inside one home cannot cover separate server homes on the same desktop, so Windows uses a kernel object.

- **Mechanism:** a named mutex `Local\CyberComputerInput-<owner SID string>`. The `Local\` prefix puts it in the caller's session namespace ([kernel object namespaces](https://learn.microsoft.com/en-us/windows/win32/termserv/kernel-object-namespaces), [CreateMutexW](https://learn.microsoft.com/en-us/windows/win32/api/synchapi/nf-synchapi-createmutexw)). It is created with an explicit DACL that grants only the owner SID and denies everyone else, including AppContainer SIDs, by omission. No handle is inheritable. The session-scoped name matches the session/desktop binding: a different session of the same user is a different desktop with its own latch.
- **Acquire:** `WaitForSingleObject(…, 0)`. `WAIT_OBJECT_0` means acquired. `WAIT_ABANDONED` means a previous holder died; the lease is acquired, with the consequences below. `WAIT_TIMEOUT` means `host_lease_busy`; failure to create the object means `host_lease_unavailable`.
- **Abandoned lease.** `WAIT_ABANDONED` is how Windows detects the foundation's abandoned-lease case. Journals are per home (`runtime/computer/journal.rs` recovers only its own home), so a server that acquires an abandoned mutex **cannot** settle the crashed holder's intents and does not try. Instead it records `previous_holder_abandoned` in its own journal, treats the desktop's input cleanup as uncertain (keys or buttons the crashed holder pressed may still be logically down, T11), and shows that warning in the owner's preview/allow review before the new grant can act. The crashed holder's open intents are settled `unknown` by that home's own startup recovery, which already runs before it can arm again. If the crashed holder was the same home (a restart), its startup recovery has already run by the time it arms.
- **Hold:** the mutex is owned by a dedicated lease thread, because mutex ownership is per thread. The `HostLease` RAII object releases it by message and joins the thread. A cancelled native worker keeps its `Arc<HostLease>` until it returns, as today.
- **Squatting:** a same-user process (a full-access model shell, for example) can create the name first and hold it. That only denies service and fails closed as `host_lease_busy` at arming. This cannot be prevented without OS privilege separation (§8).

### 5.9 Lifecycle and revocation summary

| Event | Effect |
|---|---|
| Owner Stop (CLI/API/dashboard/TUI focused shortcut), Session interrupt, logout, presence loss, grant expiry, feature disabled | Existing controller generation bump. No change. |
| Session latch (§5.7) | All grants revoked (generation bump plus `stop_all`); fresh review required |
| Topology epoch change (§5.6) | Grants stay live; every existing frame is stale; the next coordinate action needs a new screenshot |
| Target window destroyed, process exited, PID/creation-time/class mismatch, WGC item `Closed` | Grant revoked `target_gone`; fresh review required |
| Target elevated or IL unreadable at any check | Action refused; grant revoked |
| Pending permission/question (supervisor fence), admin/Mode/agent change | Existing fence: input refused / Stop required |
| UIA/capture timeout, partial `SendInput` count, cursor-position mismatch | Action `unknown`, grant stopped, owner reconciliation with `--acknowledge-unknown` |
| Server crash/restart | No grant survives. The crashed home's own startup recovery settles its open intents `unknown`. The next lease holder, from any home, sees the abandoned mutex and warns the owner that cleanup is uncertain (§5.8). |

No OS-global emergency hotkey is claimed. A `RegisterHotKey` path could be added later, but the issue forbids claiming one without implementing and testing it. The existing focused-TUI Ctrl+Alt+Esc keeps its documented scope.

### 5.10 If a helper or broker becomes necessary

The first release has none. If a spike shows one is needed (for example, the capture authority path requires package identity, decision D2), a later reviewed change must follow the foundation's helper rules (F9, foundation §10.2): the same `cyber` binary re-executed as the same user with no added privilege, talking only over a channel the server created and passed at spawn. On Windows that is an anonymous pipe or inherited pipe handle, not a named pipe the helper listens on. Same SID, a pipe name or loopback alone is never treated as owner proof, and owner credentials never appear in model-visible arguments or environment.

### 5.11 Health and capability detection

`health(prompt=false)` is read-only. It computes:
- OS build (`RtlGetVersion`-equivalent through the `windows` crate) and architecture, checked against the support matrix;
- `GraphicsCaptureSession::IsSupported()`;
- controller token checks (§5.2);
- session ID, WTS connect state, remote metrics and input-desktop name;
- whether the session-watch thread is live;
- whether **this** server currently holds the host lease, from in-process state only. Health never waits on or opens the mutex for ownership, because `WaitForSingleObject` would acquire it and could make a concurrent arming attempt fail with a spurious `host_lease_busy`. Whether another home holds the lease is discovered only when arming;
- whether any connected output is in advanced-color mode.

It never captures, never enumerates window titles (that happens only in the owner `targets` route), never sends input and never opens OS UI. `health(prompt=true)` exists only for `setup`. On the recommended path it does nothing extra and says so: Windows needs no separate OS grant on this path, and Cyber's owner review is the authority.

## 6. Public surfaces

### 6.1 Model tool

The tool is unchanged: `status`, `screenshot`, `move`, `click`, `drag`, `scroll`, `type`, `key`, `wait`. Windows refusals use the existing error classes with registry reason codes. Frame metadata carries `target_id` instead of numeric handles. The action set is the same on every platform (foundation F7).

### 6.2 Grants

A grant is created only through the existing owner `preview` → `allow` review flow. On Windows, preview shows:
- the window title (owner-only);
- the process image path and file description;
- the Authenticode publisher, or "unsigned";
- PID and start time, IL, monitor and DPI;
- whether display affinity, cloaking or minimization would stop capture;
- the warning text.

Because `window` is not re-resolvable, allow follows the foundation's review-bound arming (F5): it names the `preview_id` of the review the owner saw, before that review's deadline, re-checks that the live target still matches every bound identity field, and refuses otherwise. It never re-lists or picks a different window.

### 6.3 Target candidates (owner only)

- `GET /api/v1/tools/computer/targets` (tool route group, owner credential, `Cache-Control: no-store`) enumerates visible, non-cloaked, non-minimized, unowned top-level windows on the bound desktop, in the server's session, owned by the owner's user SID, at medium IL or lower. It excludes Cyber's own processes and the denylist.
- Each entry is a candidate token valid for 30 s (foundation F4), plus display fields. `preview` refuses an expired candidate; `allow` is bound to the review (§6.2).
- Titles can be sensitive. This route is never offered to the model tool, hub, peers or relay, and its output is not persisted.
- CLI: `cyber computer targets` prints the list; `cyber computer allow <session> --window <candidate> [--ttl N]` previews and asks for confirmation, and the foundation's `--preview <id> --yes` confirms a pending review. `--window` with `--yes` and no `--preview` is refused with `interactive_review_required` (F5). Display scope (`--display`) is refused with `selector_unsupported_on_platform` until D4.

### 6.4 SDK and OpenAPI

The SDK and OpenAPI document get additive fields and new operations (`tool.computerTargets()`), with numeric handle fields deprecated. The SDK and dashboard read `target_id` and fall back to the old fields only on macOS.

## 7. Dependency and version matrix

| Component | Version (proposed pin) | License | Use | Notes |
|---|---|---|---|---|
| Rust toolchain | workspace `rust-version` | MIT/Apache-2.0 | build | Existing MSVC targets `x86_64-pc-windows-msvc`, `aarch64-pc-windows-msvc` |
| `windows` crate | 0.62.2 (already in `Cargo.lock`) | MIT OR Apache-2.0 | WinRT `Windows.Graphics.Capture`, `Windows.Graphics.DirectX.Direct3D11`, Win32 D3D11/DXGI, DWM, UIA (`Win32_UI_Accessibility`), `Win32_UI_Input_KeyboardAndMouse`, `Win32_UI_WindowsAndMessaging`, `Win32_UI_HiDpi`, `Win32_System_RemoteDesktop`, `Win32_System_StationsAndDesktops`, `Win32_Security`, `Win32_System_Threading` | Only `cfg(windows)` target dependency of `cyber-computer`. Feature list reviewed in the foundation PR. |
| `windows-sys` | 0.61.2 (already used by `cyber-sandbox`) | MIT OR Apache-2.0 | none new | Not added to `cyber-computer`; WinRT needs the `windows` crate |
| `png` | 0.18.1 (already in `Cargo.lock`) | MIT OR Apache-2.0 | PNG encoding of RGBA readback | Pure Rust; bounded output size checked before return |
| Image downscale | in-crate box/area filter | (project) | ≤1920-px edge | Avoids a new dependency; unit-tested on every OS |
| `windows-capture` | 2.0.1 (not adopted) | MIT | — | Rejected: third-party wrapper over WGC with its own threading/callback model; the direct `windows` crate keeps lifetimes under this design's control |
| Windows SDK / OS | Windows 11 25H2 (build 26200) and 26H2 (build 26300), x64 | Microsoft | runtime | Per [Windows 11 release information](https://learn.microsoft.com/en-us/windows/release-health/windows11-release-information) as of 2026-11-14, 24H2 Home/Pro updates have ended (Enterprise/Education continue to 2027-10-12). 26H1 (build 28000) ships only on new devices. |
| CI runners | `windows-2025` (x64, Server 2025), `windows-11-arm` (ARM64) | GitHub | compile/lint/unit/fault-injection | Hosted runners are not interactive desktop acceptance (see tasks) |

All licenses are compatible with the project's Apache-2.0 license. `cargo deny`/license checks in CI must cover any new feature-gated transitive crates.

## 8. Threat model

**Assets.**
1. The owner's desktop input authority: anything a medium-IL user can do through a GUI.
2. Pixels visible in the target window, which may include secrets.
3. Owner credentials and state, including the owner API password and hidden state directory.
4. Private capture files and receipts.
5. Integrity of Cyber's own approval surfaces: permission prompts, admin changes, Stop.
6. The owner's clipboard and other windows.

**Adversaries.**
- **A1, the model.** It controls tool arguments and the timing of actions, and may be prompt-injected.
- **A2, screen content.** Text or images in the target ("click Approve", fake dialogs) that the model reads.
- **A3, a target or third-party application.** It can change its UI, spawn windows or overlays, or set `IsPassword` falsely or not at all.
- **A4, other same-user processes.** These include full-access model shells and plugins.
- **A5, sandboxed AppContainer children.**
- **A6, remote parties:** hub members, peers, relay devices.
- **A7, a local attacker** at an unlocked, unattended machine. Out of scope beyond the lock latch.

| # | Attack path | Mitigation | Residual risk |
|---|---|---|---|
| T1 | A1 arms itself or widens scope | Arming only through the owner API with credential plus exact human presence. No model route to `allow`/`targets`. Saved/bypass rules cannot arm (existing). | None beyond existing owner-shell caveat |
| T2 | A1/A2 drives Cyber's own approval UI (self-approval) | Server supervisor fence (pending requests block input). Admin/Mode/agent changes require Stop. Cyber processes and dashboard windows refused as targets. | Dashboard recognition in an arbitrary browser is heuristic; server fence is the guarantee |
| T3 | A1 sends input to a different window by timing (focus steal, z-order change) | Per-event foreground, focus, hit-test, overlay and bounds checks; cursor-position verification; text chunked with focus rechecks | Check-then-act race between the last check and `SendInput` (≈ms); cannot be closed with documented user-mode APIs |
| T4 | A3/A4 places a transparent or topmost overlay to capture clicks or keystrokes | Z-order overlay walk on DWM bounds, `WindowFromPoint` root check, UIA PID ancestry | Overlay created after the check (T3 race); click-through `WS_EX_TRANSPARENT` windows that are visually present but not hit-testable are refused by the z-order walk |
| T5 | Typing secrets into a password field, or a fake field that hides `IsPassword` | `IsPassword`/`ES_PASSWORD` refusal; control-type allowlist; dialogs refused; no value reads | A custom-drawn app can present a credential field as an allowlisted Edit; the owner's target review is the remaining control. Documented. |
| T6 | Driving UAC/credential/lock-screen UI | Secure-desktop latch; IL refusal for elevated UI; never run elevated/uiAccess; Session 0 refused | Unelevated credential UIs from unknown vendors are not recognized unless they set `IsPassword` |
| T7 | Capturing more than the window (other apps, notifications) | Window-only WGC; no display scope; no DXGI; cursor capture off; border kept | Content that the target app itself renders (embedded web content, previews of other files) is captured by design |
| T8 | Bypassing capture exclusion or protected content | Affinity checked and respected; no fallback path | A malicious same-user process can still capture; Cyber only promises not to help |
| T9 | PID/HWND reuse to redirect a grant | PID+creation time+HWND+class+image-path hash+user SID+session binding; revocation on mismatch | Same-process HWND reuse with the same class (rare) |
| T10 | Unicode/keyboard layout tricks (dead keys, IME composition, hidden modifiers) | `KEYEVENTF_UNICODE` bypasses layouts for text; chords resolved per target layout and refused if hidden modifiers needed; IME candidate windows are other-process overlays → refused | An IME active in the target may transform `VK_PACKET` input *(inference)*; acceptance tests one IME-off and one IME-on layout |
| T11 | Stuck modifiers/buttons after Stop or crash | Release tracker with drop guard; cleanup on cancel; `cleanup: "uncertain"` recorded | Crash between down and up can leave a key logically down until the next physical press. Documented; the next lease holder sees the abandoned mutex and warns the owner before acting, and the crashed home's own recovery settles its intents `unknown`. |
| T12 | A4 squats the lease mutex or watch-thread resources | Fail closed (`host_lease_busy` / `host_lease_unavailable`) | Denial of service by same-user code |
| T13 | A4 calls `SendInput` or captures directly | None; same-user code has the owner's authority | Same as macOS: not OS privilege separation |
| T14 | A5 reads private captures or gains desktop rights | Hidden state root and ACLs exclude AppContainer SID; no capture capabilities granted; capability verification at launch | None known *(regression test required)* |
| T15 | A6 obtains control | No hub/peer/relay route to computer endpoints (existing); owner credential required | None beyond owner credential compromise |
| T16 | Hung UIA provider blocks Stop or transfers lease | Bounded timeouts; Stop never waits on adapter; worker keeps lease until return | A provider that hangs past timeouts ties up one worker thread until it returns; further actions are Busy |
| T17 | Screenshots leak via logs, exports or provider | Existing private store, Session-bound reads, retention, deletion fence; typed text redaction | Authorized screenshots go to the selected model provider by design (stated at review) |
| T18 | HDR/tone-mapping produces misleading pixels the model acts on | HDR refused in the first release | — |
| T19 | Lock/RDP/fast-user-switch leaves a grant valid on another desktop | Latch on WTS/desktop events plus per-event desktop-name check; no resurrection | Missed notification is covered by per-event checks; a failed watch thread disables input |
| T20 | A4 or another account places a window from a different process, owned by the target, over the point (owner-link spoofing) | Owned popups are refused regardless of owner; the overlay walk admits only the target HWND itself; `WindowFromPoint` root must be the target | Over-refusal of the target's own menus and drop-downs in the first release |
| T21 | A window from another user account in the same session (e.g. `runas`) is offered or bound as a target | Token user SID is an explicit binding fact; listing and every check refuse `target_other_user` | Token unreadable → refused, never assumed |

## 9. Open decisions for the owner

Each decision lists the recommended option first.

- **D1 – Supported OS/architecture baseline.** *Recommended:* Windows 11 x64, GA channel 25H2 (26200) and 26H2 (26300), local console session. ARM64 stays unsupported until the `windows-11-arm` runner runs the adapter test suite and a physical ARM64 device passes acceptance. Windows 10, 24H2, 26H1, Server and LTSC stay unsupported. *Alternative:* include 24H2 Enterprise until 2027-10-12.
- **D2 – Capture authority path.** *Recommended:* programmatic `CreateForWindow` from the unpackaged server, with Cyber owner review as the authority and an honest `owner_review_only` health report. Never borderless. Any OS-policy denial is a hard refusal. Spike S1 confirms the actual OS behavior. *Alternative:* require `GraphicsCapturePicker` on each grant. That needs an owned visible window and a picker-item-to-HWND binding that is not documented.
- **D3 – In-process adapter vs helper.** The worker contract moved to the foundation (F9); the topology choice stays here. *Recommended:* in-process, no helper, no IPC. *Alternative:* a helper under the F9 rules (§5.10), only if D2 needs package identity.
- **D4 – Display scope on Windows.** *Recommended:* defer. First release is window-only, and `--display` returns `selector_unsupported_on_platform`. *Alternative:* allow WGC `CreateForMonitor` under a separate owner scope with a stronger warning.
- **D5 – RDP/remote sessions.** The `remote_session` code and the rule that new platforms refuse remote sessions until accepted moved to the foundation (F2, F6). *Recommended:* detect with `SM_REMOTESESSION` and refuse. *Alternative:* a later change with its own acceptance.
- **D6 – DPI model.** *Recommended:* per-thread PMv2 on adapter threads; process manifest unchanged. *Alternative:* a process-wide PMv2 manifest. That affects console/TUI behavior and needs separate review.
- **D7 – HDR.** *Recommended:* refuse when the target's monitor is in advanced-color mode. *Alternative:* request an FP16 frame pool and tone-map to SDR, with acceptance on an HDR display.
- **D8 – Chords on Windows.** *Moved to the foundation as F8 and approved there on 2026-11-15.* Windows adopts the shared table (§5.2): `command` and `alt` refused under window scope, the shared edit-key list (including Ctrl+S and Ctrl+Up/Down), clipboard chords refused as on macOS, and the global deny list. No Windows decision remains.
- **D9 – Terminals hosting Cyber.** *Recommended:* refuse Windows Terminal/conhost windows as targets while any Cyber TUI attachment exists on this server. *Alternative:* refuse only the specific console window that hosts the TUI. Mapping consoles to windows is fragile *(inference)*.
- **D10 – Supported application classes.** *Recommended:* claim only classes that pass the fixture matrix (Win32 common controls, WinUI 3, WPF). Chromium/Electron are listed as "untested". *Alternative:* add an Electron fixture to the first acceptance run.
- **D11 – Release/packaging.** *Recommended:* the feature is built and tested on Windows CI, but user delivery waits for the separate decision to publish Windows binaries again ([release-without-windows](../release-without-windows/proposal.md)) and for Authenticode signing of `cyber.exe`. Until then, acceptance uses a recorded developer build with its SHA-256. *Alternative:* an experimental, unsigned developer channel clearly labelled as such.
- **D12 – Shared schema with Linux.** *Resolved by the foundation (#134), approved 2026-11-15* (F1, F2, F4, F7). This change is rebased on it.

## 10. Spikes before implementation

- **S1 (capture authority):** on 25H2 and 26H2 x64, record what `CreateForWindow` does from an unpackaged medium-IL console process: prompts, privacy settings, policy denial, border.
- **S2 (geometry):** WGC `ContentSize` vs DWM extended frame bounds for Win32, WinUI and WPF windows at 100/125/150/200 % DPI, mixed-DPI, negative-origin and rotated monitors.
- **S3 (input mapping):** absolute virtual-desktop normalization error and `GetCursorPos` agreement under PMv2 on the same layouts.
- **S4 (UIA bounds):** time-to-first-response and timeout behavior of `ElementFromPoint`/`GetFocusedElement` against a deliberately hung provider fixture.

Spike results are recorded in the change before the corresponding implementation task starts. A spike that contradicts an *(inference)* updates this design first.

## 11. References

Microsoft Learn pages cited inline, all checked to resolve on 2026-11-14. Further pages consulted: [GetAsyncKeyState](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-getasynckeystate) (intentionally not used for verification, §5.2), [OpenProcessToken](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-openprocesstoken), [IsWindow](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-iswindow), [GetDpiForMonitor](https://learn.microsoft.com/en-us/windows/win32/api/shellscalingapi/nf-shellscalingapi-getdpiformonitor), [GetApplicationUserModelId](https://learn.microsoft.com/en-us/windows/win32/api/appmodel/nf-appmodel-getapplicationusermodelid) (packaged-app identity in review), [application manifests](https://learn.microsoft.com/en-us/windows/win32/sbscs/application-manifests).

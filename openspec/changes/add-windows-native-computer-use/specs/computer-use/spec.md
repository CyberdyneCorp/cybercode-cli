## MODIFIED Requirements

### Requirement: Native computer capability and consent
(P2) Native computer use SHALL be experimental and off by default, enableable only through trusted global configuration. It SHALL support macOS 14+ and, once the Windows support evidence gate is met, the Windows builds and architectures listed in the reviewed Windows support matrix. Every other platform, version, architecture or session mode SHALL return an explicit unsupported result with a machine-readable reason. Capture/input SHALL require the platform's current OS authorization where one exists (macOS Screen Recording/Accessibility) and an explicit authenticated owner grant bound to one Session and one reviewed target. Where the platform has no separate OS grant, health SHALL say that owner review is the only authority and SHALL NOT invent an OS-permission step. Grants SHALL default to 300 seconds, be capped at 900 seconds, require attached human presence and never survive restart or inherit to child/hub agents.

#### Scenario: Repository attempts to enable host input
- **WHEN** project configuration or model content requests feature enablement or a control grant
- **THEN** it cannot arm native capture/input or change global feature state

#### Scenario: Unsupported host
- **WHEN** the feature is requested on Linux, an unlisted Windows build or architecture, or an unsupported macOS version
- **THEN** the server reports the unsupported capability and its reason without shell fallback or native side effects

#### Scenario: Windows health without an OS grant
- **WHEN** the owner inspects health on a supported Windows desktop where programmatic window capture needs no OS consent
- **THEN** health reports owner review as the authority, does not tell the user to enable an Accessibility or capture permission, and opens no picker or OS prompt

## ADDED Requirements

### Requirement: Platform-neutral computer health
(P2) Computer health SHALL report observation and input availability separately, each with a machine-readable reason when unavailable. It SHALL also report OS version/build, architecture, interactive-session state and the kind of authority the platform requires. The existing macOS fields SHALL keep their meaning for compatibility. `health` without an explicit owner setup request SHALL NOT capture pixels, enumerate window titles, send input or open OS UI.

#### Scenario: Locked Windows session
- **WHEN** the owner requests health while the Windows session is locked
- **THEN** observation and input are reported unavailable with a session-locked reason and no capture or input occurs

#### Scenario: Elevated controller
- **WHEN** the Cyber server runs with an elevated or `uiAccess` token on Windows
- **THEN** input is reported unavailable with a controller-privilege reason and no grant can be armed

### Requirement: Pointer-safe native target identity
(P2) Public computer schemas SHALL identify native targets by server-issued opaque identifiers. Raw window or display handles SHALL NOT be accepted from callers or emitted as numbers that a client must interpret. Numeric macOS fields MAY remain as deprecated compatibility fields. A caller-supplied handle, PID or executable path SHALL NOT select or authorize a target. Windows target selection SHALL use a short-lived candidate from the owner-only target listing.

#### Scenario: Caller supplies a window handle
- **WHEN** a request names a numeric window handle or process ID as a Windows target
- **THEN** it is rejected without resolving, capturing or arming anything

#### Scenario: Candidate expires
- **WHEN** the owner submits a target candidate more than 30 seconds after the listing produced it
- **THEN** preview refuses it and asks for a fresh listing

### Requirement: Windows target binding
(P2) A Windows grant SHALL bind to one top-level window, its owning process ID and process creation time, window class, the server's interactive logon session and the `WinSta0\Default` input desktop. The server SHALL revoke the grant and require a fresh owner review when any bound fact changes. These changes include target destruction, process exit, PID or window reuse, lock, logoff, disconnect, remote-session start, remote control, a desktop switch or loss of the session-notification watcher. Unlock or reconnect SHALL NOT restore a revoked grant.

#### Scenario: Lock and unlock
- **WHEN** the owner locks and then unlocks the Windows session during an active grant
- **THEN** the grant was revoked at lock, the next action reports the Session unarmed, and control requires a new review

#### Scenario: Process restarted with the same window title
- **WHEN** the target application exits and a new instance opens a window with the same title and executable
- **THEN** the old grant does not apply to the new window and no input is dispatched

### Requirement: Windows window capture
(P2) Windows capture SHALL use Windows.Graphics.Capture for the reviewed window only. Cursor capture SHALL be off, and Cyber SHALL NOT request borderless capture or disable the system capture indication. Readback SHALL use the frame's content size and SHALL NOT expose texture padding or earlier surface contents. A capture-excluded, minimized, cloaked or advanced-color target, a policy denial, device loss or a size change during capture SHALL produce an explicit refusal. Cyber SHALL NOT fall back to the system picker, DXGI desktop duplication, display capture or any other mechanism after such a refusal.

#### Scenario: Capture-excluded window
- **WHEN** the reviewed window has a display affinity that excludes it from capture
- **THEN** the screenshot is refused with a capture-excluded reason and no other capture path is attempted

#### Scenario: Window resized during capture
- **WHEN** the frame's content size differs from the target bounds read before promotion
- **THEN** the frame is discarded and the caller is told to capture again

### Requirement: Windows target assurance before every event
(P2) Before every Windows pointer event, text chunk of at most 16 UTF-16 units, and key chord, the adapter SHALL read again from the OS and confirm all of the following:
- the grant is live, the session latch is clear and the display topology is unchanged;
- the target identity and bounds match the frame;
- the target is the foreground root window and is neither minimized nor cloaked;
- for keyboard input, focus is inside the target and the focused element passes the allowlist;
- for pointer input, the hit window's root, the UI Automation element ancestry and the z-order above the point all resolve to the target;
- the target's integrity level is not above the controller's.

The adapter SHALL refuse:
- password and secure fields;
- dialogs and modal windows;
- OS security, consent and credential surfaces;
- Cyber's own processes and windows;
- elevated, protected or unreadable-integrity processes;
- overlays from any other window;
- unidentified or inconsistent UI Automation ancestry, and UI Automation timeouts.

An absent password flag SHALL NOT count as proof of safety. Element values SHALL NOT be read to classify a control. A failed check after any event of the action has been dispatched SHALL settle the action as unknown.

#### Scenario: Password field focused
- **WHEN** a type action targets a focused element that reports itself as a password field
- **THEN** no key event is dispatched and the action is refused

#### Scenario: Topmost overlay from another process
- **WHEN** a click point in the reviewed window is covered by a visible topmost window of another process
- **THEN** no mouse event is dispatched and the action is refused

#### Scenario: Focus moves during typing
- **WHEN** keyboard focus leaves the reviewed window between two text chunks
- **THEN** the remaining text is not sent and the action settles unknown without automatic retry

#### Scenario: Elevated target
- **WHEN** the reviewed window belongs to a process whose integrity level is above the server's or cannot be read
- **THEN** the action is refused before dispatch and the grant is revoked

### Requirement: Windows native input dispatch
(P2) Windows input SHALL use `SendInput` only. It SHALL use normalized absolute virtual-desktop coordinates computed at dispatch time, and native Unicode keyboard events for text. Cyber SHALL NOT use window messages, script hosts, accessibility pattern invocation, clipboard contents or any secondary injection mechanism, and SHALL NOT retry through another mechanism after a refusal. Before any button event, the cursor position SHALL match the intended physical point within one pixel. An inserted-event count below the requested count SHALL settle the action unknown and stop the grant. Cyber SHALL refuse the Windows-key modifier, refuse Alt chords and clipboard chords under window scope, and allow only the reviewed Control editing chords. On Stop, timeout or failure the adapter SHALL attempt to release every key and button it pressed. It SHALL record cleanup uncertainty when release cannot be confirmed, and SHALL NOT claim that delivered input was undone.

#### Scenario: Partial delivery
- **WHEN** `SendInput` reports fewer inserted events than requested for a click
- **THEN** the receipt is unknown, cleanup is attempted, the grant stops and the click is not repeated

#### Scenario: Stop during drag
- **WHEN** the owner requests Stop while a drag holds the left button down
- **THEN** no further move is dispatched, a button-up is attempted and the receipt is unknown

#### Scenario: Clipboard chord
- **WHEN** the model requests Control+V under a window grant
- **THEN** the chord is refused without dispatching any key event

### Requirement: Windows cross-process input lease
(P2) On Windows, at most one native worker per user logon session SHALL hold input authority across all Cyber server processes and homes. Cyber SHALL enforce this with a session-scoped kernel object whose access control grants only the owner's SID. A cancelled worker SHALL keep the lease until its native call returns. Finding an abandoned lease SHALL trigger recovery of open native intents as unknown. Failure to create or acquire the lease SHALL refuse arming with a busy or unavailable reason.

#### Scenario: Two server homes on one desktop
- **WHEN** a second Cyber server with a different home tries to arm control while the first holds an active grant on the same desktop
- **THEN** arming fails as busy and the first grant is unaffected

#### Scenario: Previous holder crashed
- **WHEN** a server acquires a lease abandoned by a crashed process
- **THEN** any native intent left open is reported unknown before a new grant can act

### Requirement: Windows support evidence gate
(P2) Health, CLI, dashboard and SDK SHALL report Windows input support only for build/architecture/session combinations and application classes recorded as accepted in the Windows acceptance record. Each acceptance SHALL come from a controlled real interactive desktop, using a disposable fixture and the production model/runtime/API path. Compilation, hosted CI, fake adapters and headless runners SHALL NOT count as desktop acceptance. Combinations without recorded evidence SHALL remain explicitly unsupported or pending.

#### Scenario: CI passes without device evidence
- **WHEN** Windows CI passes but no controlled desktop acceptance is recorded for a build
- **THEN** health on that build reports input as unsupported with an unaccepted-build reason

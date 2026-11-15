## MODIFIED Requirements

### Requirement: Native computer capability and consent
(P2) Native computer use SHALL be experimental and off by default, enableable only through trusted global configuration. It SHALL support macOS 14+ and, once the Windows evidence rows are `accepted`, the Windows builds and architectures listed in the reviewed Windows support matrix. Every other platform, version, architecture or session mode SHALL return an explicit unsupported result with a reason code. Capture/input SHALL require the platform's current OS authorization where one exists (macOS Screen Recording/Accessibility) and an explicit authenticated owner grant bound to one Session and one reviewed target. Where the platform has no separate OS grant, health SHALL report authority `owner_review_only` and SHALL NOT invent an OS-permission step. Grants SHALL default to 300 seconds, be capped at 900 seconds, require attached human presence and never survive restart or inherit to child/hub agents.

#### Scenario: Repository attempts to enable host input
- **WHEN** project configuration or model content requests feature enablement or a control grant
- **THEN** it cannot arm native capture/input or change global feature state

#### Scenario: Unsupported host
- **WHEN** the feature is requested on Linux, an unlisted Windows build or architecture, or an unsupported macOS version
- **THEN** the server reports the unsupported capability and its reason code without shell fallback or native side effects

#### Scenario: Windows health without an OS grant
- **WHEN** the owner inspects health on a supported Windows desktop where programmatic window capture needs no OS consent
- **THEN** health reports authority `owner_review_only`, does not tell the user to enable an Accessibility or capture permission, and opens no picker or OS prompt

## ADDED Requirements

### Requirement: Windows capability report
(P2) On Windows, the capability report defined by "Computer capability report" SHALL report backend `windows-wgc`, authority `owner_review_only`, selectors `window` only, the OS build in `os.build`, and session facts (interactive, locked, remote) read from the WTS session state. Windows-only reason codes SHALL be registered in the categories of "Computer reason codes", and controller-token refusals SHALL use the shared `controller_privilege_unsupported` with a Windows detail code. Health SHALL learn whether this server holds the host lease from in-process state only.

#### Scenario: Locked Windows session
- **WHEN** the owner requests health while the Windows session is locked
- **THEN** capture and input are reported unavailable with `session_locked` and no capture or input occurs

### Requirement: Windows target candidates
(P2) A Windows `window` selector SHALL carry a candidate token issued by the owner-only target listing, which SHALL include only visible, non-cloaked, non-minimized, unowned top-level windows on the bound input desktop, in the server's session, whose process token user is the owner and whose integrity level is medium or lower, and SHALL exclude Cyber's own processes. The listing SHALL NOT be offered to the model tool, hub, peers or relay, and its output SHALL NOT be persisted. Candidate expiry follows "Computer target selection and identity", and a `window` grant SHALL be armed only as defined by "Review-bound computer arming".

#### Scenario: Candidate expires
- **WHEN** the owner submits a target candidate more than 30 seconds after the listing produced it
- **THEN** preview refuses it and asks for a fresh listing

#### Scenario: Elevated window in the listing
- **WHEN** an elevated window is visible on the owner's desktop
- **THEN** it is absent from the target listing and cannot be previewed

### Requirement: Windows target binding
(P2) A Windows grant SHALL bind to one unowned top-level window, its owning process ID and process creation time, a hash of the process image path, the user SID of the process token, the window class, the server's interactive logon session and the `WinSta0\Default` input desktop. The target process token's user SID SHALL equal the owner's; a target owned by another account, or whose token user cannot be read, SHALL be refused with `target_other_user` at listing, preview, allow and every check. Under "Computer grant lifecycle across platforms", a change of any bound fact SHALL be binding loss: target destruction, process exit, PID or window reuse, an image-path or token-user change, lock, logoff, disconnect, remote-session start, remote control, a desktop switch or loss of the session-notification watcher. A display change (monitor added or removed, resolution, DPI or arrangement change) SHALL be a topology change.

#### Scenario: Process restarted with the same window title
- **WHEN** the target application exits and a new instance opens a window with the same title and executable
- **THEN** the old grant does not apply to the new window and no input is dispatched

#### Scenario: Window of another user account
- **WHEN** a medium-integrity window in the owner's session belongs to a process started under a different user account
- **THEN** it is absent from the target listing, cannot be previewed or allowed, and no capture or input reaches it

#### Scenario: DPI change during a grant
- **WHEN** the owner changes the scale of the target's monitor while a grant is active and the model then clicks using a frame captured before the change
- **THEN** the click is refused with `frame_stale` before any event is dispatched, the grant stays armed, and a new screenshot allows further actions

### Requirement: Windows window capture
(P2) Windows capture SHALL use Windows.Graphics.Capture for the reviewed window only. Cursor capture SHALL be off, and Cyber SHALL NOT request borderless capture or disable the system capture indication. Readback SHALL use the frame's content size and SHALL NOT expose texture padding or earlier surface contents. A capture-excluded, minimized, cloaked or advanced-color target, a policy denial, device loss or a size change during capture SHALL produce an explicit refusal with a reason code. Cyber SHALL NOT fall back to the system picker, DXGI desktop duplication, display capture or any other mechanism after such a refusal.

#### Scenario: Capture-excluded window
- **WHEN** the reviewed window has a display affinity that excludes it from capture
- **THEN** the screenshot is refused with `target_capture_excluded` and no other capture path is attempted

#### Scenario: Window resized during capture
- **WHEN** the frame's content size differs from the target bounds read before promotion
- **THEN** the frame is discarded and the caller is told to capture again

### Requirement: Windows target assurance before every event
(P2) Before every Windows pointer event, text chunk of at most 16 UTF-16 units, and key chord, the adapter SHALL read again from the OS and confirm all of the following:
- the grant is live, the session latch is clear and the topology epoch is unchanged;
- the target identity and bounds match the frame;
- the target is the foreground root window and is neither minimized nor cloaked;
- for keyboard input, focus is inside the target and the focused element passes the allowlist;
- for pointer input, the hit window's root, the UI Automation element ancestry and the z-order above the point all resolve to the target window itself;
- the target's integrity level is medium or lower and its token user is still the owner.

The adapter SHALL refuse:
- password and secure fields;
- dialogs and modal windows;
- OS security, consent and credential surfaces;
- Cyber's own processes and windows;
- elevated, protected or unreadable-integrity processes;
- overlays from any other window;
- popups owned by the target, including its menus and drop-down lists, whether created by the target's process or by another process;
- unidentified or inconsistent UI Automation ancestry, and UI Automation timeouts.

An owner relationship to the target SHALL NOT admit a window: only the reviewed target window itself passes the hit, z-order and focus checks. An absent password flag SHALL NOT count as proof of safety. Element values SHALL NOT be read to classify a control. A failed check after any event of the action has been dispatched SHALL settle the action as unknown. The receipt SHALL name the checks that passed before the last event.

#### Scenario: Password field focused
- **WHEN** a type action targets a focused element that reports itself as a password field
- **THEN** no key event is dispatched and the action is refused with `secure_field`

#### Scenario: Topmost overlay from another process
- **WHEN** a click point in the reviewed window is covered by a visible topmost window of another process
- **THEN** no mouse event is dispatched and the action is refused

#### Scenario: Topmost window from another process owned by the target
- **WHEN** a click point is covered by a visible topmost window that another process created with the reviewed window as its owner
- **THEN** no mouse event is dispatched and the action is refused

#### Scenario: Target's own drop-down list
- **WHEN** the model clicks inside a drop-down list or menu that the target application opened
- **THEN** no mouse event is dispatched and the action is refused

#### Scenario: Focus moves during typing
- **WHEN** keyboard focus leaves the reviewed window between two text chunks
- **THEN** the remaining text is not sent and the action settles unknown without automatic retry

#### Scenario: Elevated target
- **WHEN** the reviewed window belongs to a process whose integrity level is above medium or cannot be read
- **THEN** the action is refused before dispatch and the grant is revoked with `target_elevated`

### Requirement: Windows controller token
(P2) On Windows, health and every arming and admission check SHALL refuse native capture and input with `controller_privilege_unsupported` and a Windows detail code when the Cyber server's token is elevated, holds `uiAccess`, is an AppContainer token or has an integrity level other than medium, and with `non_interactive_session` when the server runs in Session 0. Cyber SHALL NOT elevate, request `uiAccess`, or delegate to a more privileged helper to work around such a refusal.

#### Scenario: Elevated controller
- **WHEN** the Cyber server runs with an elevated or `uiAccess` token on Windows
- **THEN** input is reported unavailable with `controller_privilege_unsupported` and no grant can be armed

#### Scenario: AppContainer or low-integrity controller
- **WHEN** the Cyber server runs with an AppContainer or low-integrity token
- **THEN** capture and input are reported unavailable with `controller_privilege_unsupported` and no grant can be armed

#### Scenario: Server in Session 0
- **WHEN** the Cyber server runs as a service in Session 0
- **THEN** health reports `non_interactive_session` and no capture or input is attempted

### Requirement: Windows native input dispatch
(P2) Windows input SHALL use `SendInput` only. It SHALL use normalized absolute virtual-desktop coordinates computed at dispatch time, and native Unicode keyboard events for text. Cyber SHALL NOT use window messages, script hosts, accessibility pattern invocation, clipboard contents or any secondary injection mechanism, and SHALL NOT retry through another mechanism after a refusal. Before any button event, the cursor position SHALL match the intended physical point within one pixel. An inserted-event count below the requested count SHALL settle the action unknown and stop the grant. Chords SHALL be decided by "Shared computer key chord policy" with the Windows roles: `control` is the primary editing modifier and `command` (the Windows key) is the system modifier.

#### Scenario: Stop during drag
- **WHEN** the owner requests Stop while a drag holds the left button down
- **THEN** no further move is dispatched, a button-up is attempted and the receipt is unknown

#### Scenario: Windows-key chord
- **WHEN** the model requests a chord with `command` under a window grant
- **THEN** the chord is refused with `key_chord_refused` without dispatching any key event

### Requirement: Windows cross-process input lease
(P2) On Windows, the host input lease defined by "Computer grant lifecycle across platforms" SHALL be a session-scoped named kernel object whose access control grants only the owner's SID, held by a dedicated thread, with no inheritable handle. Acquiring an abandoned object SHALL be how a server detects a crashed previous holder and records `previous_holder_abandoned`. Each home SHALL settle its own open native intents as unknown in its startup recovery before it can arm again. Failure to acquire the lease SHALL refuse arming with `host_lease_busy`, and failure to create it with `host_lease_unavailable`.

#### Scenario: Two server homes on one desktop
- **WHEN** a second Cyber server with a different home tries to arm control while the first holds an active grant on the same desktop
- **THEN** arming fails with `host_lease_busy` and the first grant is unaffected

#### Scenario: Previous holder crashed
- **WHEN** server home B acquires a lease abandoned by crashed server home A
- **THEN** B warns the owner that keys or buttons may still be held down before its grant can act, and leaves A's journal untouched

#### Scenario: Crashed home restarts
- **WHEN** server home A restarts after crashing with a native intent open
- **THEN** A's startup recovery reports that intent unknown before A can arm a new grant

### Requirement: Windows evidence rows
(P2) The Windows rows of the evidence table defined by "Computer platform support evidence" SHALL be keyed by OS build, architecture, session type and application class. An `accepted` row SHALL come from a controlled real interactive desktop with a non-elevated medium-integrity owner session, a disposable fixture and the production model, runtime and API path, and SHALL record the build, architecture, GPU, DPI layout, binary hash and signature status.

#### Scenario: CI passes without device evidence
- **WHEN** Windows CI passes but no controlled desktop acceptance is recorded for a build
- **THEN** health on that build reports input unavailable with `evidence_pending`

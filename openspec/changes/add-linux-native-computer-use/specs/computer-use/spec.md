## MODIFIED Requirements

### Requirement: Native computer capability and consent
(P2) Native computer use SHALL be experimental and off by default, enableable only through trusted global configuration. It SHALL support macOS 14+ capture and input. On Linux it SHALL support only owner-picked capture on Wayland sessions whose XDG Desktop Portal and PipeWire probes pass, and SHALL refuse Linux input until an owner-approved target-assurance provider exists. Every other host or configuration SHALL return an explicit unsupported result with a specific refusal code. Capture/input SHALL require current platform authorization as applicable (macOS Screen Recording/Accessibility; a fresh Linux portal selection) and an explicit authenticated owner grant bound to one Session and one chosen app/window/display or owner-picked source. Grants SHALL default to 300 seconds, be capped at 900 seconds, require attached human presence and never survive restart or inherit to child/hub agents.

#### Scenario: Repository attempts to enable host input
- **WHEN** project configuration or model content requests feature enablement or a control grant
- **THEN** it cannot arm native capture/input or change global feature state

#### Scenario: Unsupported host
- **WHEN** the feature is requested on Windows, an unsupported macOS version, an X11/Xorg Linux session, a Linux host without a graphical session, or a Linux build or desktop whose capability probe fails
- **THEN** the server reports the unsupported capability with its specific refusal code, without shell fallback or native side effects

#### Scenario: Linux input requested without target assurance
- **WHEN** a Session holding a live Linux capture grant submits move, click, drag, scroll, type or key
- **THEN** the action is refused with `target_assurance_unavailable` before any intent is journaled or any event is sent

## ADDED Requirements

### Requirement: Platform-neutral computer health
(P2) Computer health SHALL keep its existing fields and SHALL add the backend name, session type, diagnostic desktop name, probed interface names and versions, whether capture and input can be offered now, the input-assurance state and a list of stable snake_case refusal codes. Input readiness SHALL be derived from a platform-neutral `input_control` fact; on macOS it SHALL equal Accessibility trust. Linux health SHALL NOT report fictional OS permissions: `accessibility` SHALL be false, `minimum_version` SHALL name the Linux prerequisite, and `screen_capture` SHALL equal the capture result (the capture probe passes and, while a scope is armed, that scope is live). Admission SHALL require `screen_capture` for every action and additionally `input_control` for input actions. Admission SHALL NOT wait on D-Bus probes: while a scope is armed, health SHALL be answered from in-memory scope state and a cached probe result, and a transient probe failure SHALL NOT by itself revoke a live grant; only that scope's revocation events end it. The desktop name SHALL be diagnostic only and SHALL NOT enable any capability. Read-only status and setup SHALL NOT create a portal session or show a chooser. Probes SHALL be bounded in time.

#### Scenario: Status on a GNOME Wayland session
- **WHEN** the owner runs read-only status on a Wayland session whose ScreenCast portal and PipeWire library probes pass
- **THEN** health reports `capture: true`, `input_control: false`, `input_assurance: "unavailable"` and the refusal code `target_assurance_unavailable`, and no portal dialog appears

#### Scenario: Branded desktop without a capture portal
- **WHEN** `XDG_CURRENT_DESKTOP` names a supported desktop but the ScreenCast portal is missing
- **THEN** health reports `capture: false` with `screencast_unavailable` regardless of the desktop name

#### Scenario: Transient probe timeout during a live grant
- **WHEN** a health refresh times out on the session bus while the armed portal session and stream are still live
- **THEN** `screen_capture` stays true, the grant stays armed and screenshots are admitted without waiting on the bus

#### Scenario: Older client reads extended health
- **WHEN** a client generated before this change reads computer health
- **THEN** the legacy fields keep their meaning and the client ignores the added fields

### Requirement: Owner-picked capture source
(P2) The computer selector SHALL accept exactly one of an application, a display or an owner-picked source kind (`window` or `output`). On Linux, application and display selectors SHALL be refused with `selector_unsupported_on_platform`, and Linux target strings SHALL NOT be interpreted as macOS bundle IDs. An owner-picked source SHALL be chosen only in the platform's own picker, started only from the authenticated owner review path. A CLI or API selector SHALL constrain only the offered source kind with a single selection, and SHALL NOT pre-select, widen or add sources. The review SHALL present the actual selected source (kind, logical size, position for outputs and a private review frame visible only to the owner) before arming. Output scope SHALL show that all content visible on that output reaches the model. A pick-scope grant SHALL be armed only by confirming the identifier of the review whose source was shown, before that review's deadline; arming SHALL NOT start a new review, and an unconfirmed review SHALL be closed at its deadline. A response with no stream, several streams or a source kind other than the one requested SHALL be closed and refused. An output stream without a logical position or size SHALL be closed and refused with `output_geometry_unavailable`. Portal persistence SHALL NOT be requested and restore tokens SHALL NOT be stored or used; every grant SHALL require a fresh portal selection and a fresh owner review.

#### Scenario: Owner picks a window
- **WHEN** the owner reviews `pick: window`, chooses one window in the portal dialog and confirms the shown review frame
- **THEN** the grant is armed for that portal stream only and the model receives frames only from it

#### Scenario: Portal returns a different source kind
- **WHEN** a `pick: window` review receives a monitor stream from the portal
- **THEN** the portal session is closed, no grant is armed and the review fails with `portal_selection_mismatch`

#### Scenario: Confirmation names a different review
- **WHEN** an owner confirmation names an expired, superseded or other Session's review identifier
- **THEN** nothing is armed, no portal dialog is opened, and the pending portal session of any expired review has been closed

#### Scenario: Output stream without a position
- **WHEN** a `pick: output` review receives a monitor stream that has a size but no position
- **THEN** the portal session is closed, no grant is armed and the review fails with `output_geometry_unavailable`

#### Scenario: Model or screen content requests a wider source
- **WHEN** model output or captured text asks to switch from a window to a whole output
- **THEN** no portal dialog is opened and the existing grant is unchanged

#### Scenario: Restore token available from an earlier session
- **WHEN** an earlier portal session could have returned a restore token
- **THEN** none was requested or stored, and a new grant shows the portal dialog again

### Requirement: Linux portal capture
(P2) Linux capture SHALL use the ScreenCast portal (version 4 or newer) and a PipeWire connection opened only from the portal-provided file descriptor. It SHALL NOT open an unrestricted PipeWire connection, including as a fallback. Streams SHALL be targeted by `pipewire-serial` when the portal provides it. Otherwise they SHALL be bound by node ID to the portal remote and the review generation, and any node removal SHALL be terminal. Each capture SHALL use a complete buffer produced after the request. It SHALL accept only allow-listed 32-bit formats in shared-memory buffers. It SHALL validate stride, chunk and mapped sizes before reading, reject corrupted or transformed buffers, and downscale to the existing 1920-pixel/8 MiB bounds. Capture waits SHALL be bounded.

#### Scenario: Malformed buffer
- **WHEN** a PipeWire buffer declares a stride or size larger than its mapped memory
- **THEN** the frame is rejected without reading out of bounds and the screenshot settles as failed with no stored image

#### Scenario: Compositor offers only DMA-BUF
- **WHEN** the negotiated stream cannot provide shared-memory buffers
- **THEN** capture is refused with `capture_buffer_unsupported`

#### Scenario: Node ID reused after hotplug
- **WHEN** a portal without `pipewire-serial` removes the bound node and a node later appears with the same ID
- **THEN** the grant is revoked with `stream_removed` and the new node is never captured

### Requirement: Linux target assurance gate
(P2) Linux input SHALL be admitted only when an owner-approved assurance provider proves, for the exact target generation, after the frame and immediately before dispatch, that the focused surface and pointer-hit surface belong to the granted target, that no secure text field, permission, authentication, lock or system dialog is involved, and that the target is not a Cyber approval surface. Accepting a portal capture or remote-desktop dialog SHALL NOT count as assurance. Missing, stale or unknown observations SHALL refuse. Self-reported signals such as accessibility roles and the session lock hint MAY only refuse and SHALL never prove safety. XWayland targets SHALL be refused until they are separately accepted. Adding a provider, and any Linux input transport, SHALL require its own reviewed change with a named compositor version, access model, stated race window and real-desktop evidence.

#### Scenario: Owner accepted a remote-desktop portal dialog
- **WHEN** the portal grants keyboard and pointer devices but no approved assurance provider is configured
- **THEN** health keeps `input_control: false` and every input action is refused with `target_assurance_unavailable`

#### Scenario: Accessibility reports a password field
- **WHEN** an approved provider proves focus and hit target but the accessibility bus reports a password role at the focused element
- **THEN** the input is refused

### Requirement: Linux portal lifecycle and revocation
(P2) A Linux grant SHALL be revoked through the existing controller fence, before any teardown completes, when any of the following occurs: owner Stop, Session interrupt, presence loss, expiry, the portal session `Closed` signal, loss of the D-Bus connection or the portal frontend's bus name, PipeWire stream error or removal, source renegotiation (size, format, scale or output change), the watched graphical session being locked or becoming inactive, loss of the watch on that session, or server restart. The watched session SHALL be the user's graphical Wayland login session, identified independently of whether the server process itself runs inside it. If no such session can be identified, or its lock and activity state cannot be watched, review and arming SHALL be refused with `session_watch_unavailable`. Teardown SHALL close the portal session and release its streams and file descriptors without delaying Stop. Late portal replies from an earlier review or grant generation SHALL be closed and never adopted. Unlock, reactivation, reconnect, portal restart or compositor restart SHALL NOT restore a grant; each SHALL require a fresh portal selection and a fresh owner review. Waiting for the portal dialog SHALL be bounded and cancellable by Stop.

#### Scenario: User ends sharing from the desktop indicator
- **WHEN** the portal emits `Closed` for the bound session
- **THEN** the grant is revoked with `portal_session_closed`, and later screenshots report the Session unarmed

#### Scenario: Screen locks while armed
- **WHEN** the watched graphical session is locked and later unlocked
- **THEN** the grant was revoked at lock with `session_locked` and is not restored after unlock

#### Scenario: User switches away from the graphical session
- **WHEN** the watched graphical session becomes inactive through a VT or user switch and later becomes active again
- **THEN** the grant was revoked with `session_inactive` and is not restored

#### Scenario: Server runs outside the graphical session
- **WHEN** the server runs as a user service with no login session of its own and no graphical Wayland session of the user can be identified or watched
- **THEN** review and arming are refused with `session_watch_unavailable` and no portal dialog is opened

#### Scenario: Stop while the portal dialog is open
- **WHEN** the owner issues Stop while a review waits for the portal dialog
- **THEN** the review request and session are closed, no grant is armed, and a late portal response is discarded

### Requirement: Linux coordinate mapping
(P2) Output-scope frames SHALL map image pixels to the stream's compositor logical position and size, including fractional scales and negative origins. Window-scope frames SHALL be treated as observation-only because the portal provides no window position, and input on them SHALL be refused. A frame whose buffer orientation disagrees with its logical size, or that carries a video transform, SHALL be rejected until rotated outputs are accepted on real desktops. A size, format or scale change, or output hotplug, SHALL advance the source generation and invalidate older frames.

#### Scenario: Fractional scale output
- **WHEN** a 2400×1350 pixel buffer is captured from an output whose logical size is 1920×1080 at position (-1920, 0)
- **THEN** the frame is downscaled to the 1920-pixel edge and maps image point (0, 0) to logical (-1920, 0)

#### Scenario: Output resized after capture
- **WHEN** the stream renegotiates its size after a frame was issued
- **THEN** the grant is revoked with `source_changed` and the earlier frame cannot be used

### Requirement: Native worker resource isolation
(P2) Portal, PipeWire and EIS file descriptors and native objects SHALL be owned only by the native worker. Every such descriptor SHALL be close-on-exec from the moment Cyber receives it, and the worker SHALL NOT spawn children. On Linux the worker SHALL be non-dumpable so that same-UID tool and plugin children cannot read its descriptors through `/proc`. It SHALL terminate when the server exits, independently of the lifetime of the server thread that started it, including when the server exits before the worker has finished starting. The host lease SHALL be released only after the worker has provably stopped dispatching. Stop SHALL be able to terminate a worker blocked in a native call. Full-access owner shells, or processes with ptrace capability, are outside this guarantee and SHALL be documented as such.

#### Scenario: Sandboxed tool inspects the server's descriptors
- **WHEN** a sandboxed tool child lists or opens the native worker's `/proc/<pid>/fd` entries during an active grant
- **THEN** it cannot obtain the portal, PipeWire or EIS descriptors

#### Scenario: Worker hangs during Stop
- **WHEN** Stop is issued while the worker is blocked waiting for a PipeWire buffer
- **THEN** the grant is fenced immediately, the worker is terminated, and another grant can acquire the host lease only after the worker has exited

### Requirement: Linux session-bus trust
(P2) The Linux backend SHALL treat other clients of the user's D-Bus session bus as untrusted. It SHALL pin the unique bus name that owns the portal at review start, and SHALL act on a portal request response or session close only when the bus-stamped sender is that pinned name and the object path is the exact request or session handle Cyber created. Messages from any other sender or path SHALL be ignored. A change of the portal name's owner SHALL revoke the live scope, and the new owner SHALL NOT be trusted for it. A file descriptor returned by the portal SHALL be refused unless it is a Unix stream socket, and frames received through it SHALL be validated as untrusted input. A pick made in another program's portal dialog SHALL NOT arm a Cyber grant. When native computer use is enabled, Linux sandboxed tool and plugin children SHALL NOT be given access to the session bus.

#### Scenario: Spoofed portal response
- **WHEN** another session-bus client sends a `Response` signal on Cyber's request path while the portal dialog is open
- **THEN** the signal is ignored, the review keeps waiting for the pinned portal, and nothing is armed from the spoofed results

#### Scenario: Portal name taken over after a crash
- **WHEN** the portal's bus name changes owner while a scope is armed
- **THEN** the grant is revoked with `session_bus_lost` and the new owner is not used for that scope

#### Scenario: Sandboxed tool looks for the session bus
- **WHEN** a sandboxed tool child runs while native computer use is enabled on Linux
- **THEN** its environment has no session-bus address and the bus socket is not reachable inside the sandbox

### Requirement: Linux platform evidence
(P2) Documentation and the acceptance record SHALL state separately, for each Linux desktop and version, whether it is implemented, fixture-tested, accepted on a real desktop, or unsupported. Linux capture or input SHALL NOT be advertised for a configuration without recorded real-desktop evidence (distribution, kernel, compositor, portal frontend and backend, and PipeWire versions). wlroots, Xorg, XWayland, SSH/headless and musl-build outcomes SHALL be recorded as capability or refusal results. Headless CI with mock services SHALL NOT be described as desktop acceptance.

#### Scenario: Only mock tests have run
- **WHEN** Linux capture has passed mock D-Bus/PipeWire tests but no real desktop run is recorded
- **THEN** docs list Linux capture as fixture-tested, and the README feature matrix does not mark it supported

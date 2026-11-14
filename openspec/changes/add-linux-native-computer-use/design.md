# Design: Linux native computer use

This document is a design proposal for issue #43. It describes intended behavior. Nothing in it is implemented, and no statement here claims that a mechanism works on a real desktop. Platform facts cite official documentation, retrieved on 2026-11-14. Statements marked **(inference)** are the author's reading of that documentation or of the platform's architecture; implementation must test them before relying on them.

## 1. Baseline and constraints

The macOS design (`openspec/changes/add-native-computer-use/design.md`) and the `computer-use` spec define the contract this design keeps:

- The feature is off by default and can be enabled only in trusted global or managed configuration.
- Grants are owner-only and bound to an exact Session and Location. They have a finite TTL (300 s default, 900 s cap), action and rate limits (10/s, 300 per grant), one native worker per grant and a per-user host lease.
- Every grant requires live human presence. Presence loss revokes it permanently. Stop works independently of the model.
- Each action records intent before dispatch and a durable settlement afterwards. Uncertain outcomes are reported as `unknown` and never replayed.
- Frames are server-issued, at most 10 s old, and bound to Session, target, grant generation and geometry.
- Screenshots go to private, Session-bound storage with bounded retention and no export. Typed text is redacted from durable records.
- Secure fields, system dialogs and Cyber's own approval surfaces are refused as input targets.

Current code seams, which this design extends rather than replaces:

| Seam | Location | Role today |
|---|---|---|
| `Backend` trait | `crates/cyber-computer/src/backend.rs` | `acquire_lease`, `health(prompt)`, `resolve(selector)`, `capture(target)`, `input(target, permit)` |
| `Selector` | same | exactly one of `app` (bundle-ID charset) or `display` |
| `Health` | same | `supported`, `platform`, `minimum_version`, `screen_capture`, `accessibility`, `binary` |
| `NativeTarget` | same | identity string plus macOS `pid`/`window`/`display`/`launch_ms`, logical bounds, image size, rotation |
| `Controller` | `crates/cyber-computer/src/control.rs` | grants, generation fencing, frames, rate limits, presence epoch, supervisor fence, `CapturePermit`/`InputPermit` cancellation |
| `Geometry` | `crates/cyber-computer/src/geometry.rs` | linear image-pixel → logical-desktop map, right-angle rotation, edge ≤ 1920 |
| `Computer` runtime | `crates/cyber-server/src/runtime/computer/mod.rs` | preview/allow/stop, journal intent/settlement, artifacts, host lease, `selector_for_identity` (parses `app:`/`display:`) |
| Owner HTTP | `crates/cyber-server/src/http/computer.rs` | `/tools/computer/{health,setup,stop}`, `/sessions/{id}/computer/{preview,allow,stop,frames/{frame}}` |

The controller and the runtime enforce most of the contract independently of the platform: grants, fencing, presence, journaling, artifacts and limits. The Linux work therefore sits mostly in a new backend module, plus a small number of additive seam changes (§4).

## 2. Platform mechanisms

### 2.1 Wayland first, via XDG Desktop Portal

- **ScreenCast portal (interface version 6 at time of writing).** A client calls `CreateSession`, `SelectSources` (source `types` MONITOR=1, WINDOW=2, VIRTUAL=4, plus `multiple` and `cursor_mode`), then `Start`, which normally shows a user dialog and returns PipeWire streams, then `OpenPipeWireRemote`, which returns a file descriptor (FD). The documentation states the following ([ScreenCast](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.ScreenCast.html)):
  - `SelectSources` can be called only once per session, and invalid input closes the session.
  - Each stream carries an opaque `id` (v4), `position` (monitor streams only) and `size` in compositor coordinates. The documentation notes these "may not be equivalent" to pixel coordinates.
  - `source_type` was added in v3, `mapping_id` in v5 (to match a libei region) and `pipewire-serial` in v6. The serial is never reused. Node IDs "can be reused after node destruction", so consumers should target the serial via `PW_KEY_TARGET_OBJECT`.
  - `persist_mode` and `restore_token` (v4) are optional.
  - The PipeWire remote exposes only the session's stream nodes.
- **RemoteDesktop portal (version 2).** It adds `SelectDevices` (KEYBOARD=1, POINTER=2, TOUCHSCREEN=4) and combined ScreenCast source selection, and offers two input paths: `ConnectToEIS` (added in v2, recommended) and the D-Bus `Notify*` methods. "Once an EIS connection is established … any events submitted via … Notify* methods will return an error." `ConnectToEIS` may be called once per session, after `Start` ([RemoteDesktop](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.RemoteDesktop.html)).
- **Session.** A session can be closed by the portal at any time, which emits `Closed`. "A client who started a session vanishing from the D-Bus is equivalent to closing all active sessions made by said client" ([Session](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.Session.html)). Requests and responses follow the [Request](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.Request.html) pattern.
- **Host app identity.** Unsandboxed applications can register an application ID through `org.freedesktop.host.portal.Registry.Register`. The ID must match a `.desktop` basename, and registration must happen before any portal call and only once ([Registry](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.host.portal.Registry.html)).

### 2.2 PipeWire

The portal opens a restricted connection on the client's behalf and passes back its FD. "No objects are initially visible" until permissions are configured for the session ([PipeWire portal access control](https://docs.pipewire.org/page_portal.html)). Cyber connects with `pw_context_connect_fd` on the portal FD only. It never opens the user's default PipeWire socket, so it never holds an unrestricted connection, including as a fallback.

### 2.3 libei / EIS (milestone B only)

The libei sender context negotiates seats and devices with these capabilities: `POINTER`, `POINTER_ABSOLUTE`, `KEYBOARD`, `BUTTON`, `SCROLL`, `TOUCH`, `TEXT` (since 1.6) and others ([libei API](https://libinput.pages.freedesktop.org/libei/api/group__libei.html)).

- Absolute devices expose regions in desktop-wide logical coordinates. `ei_region_get_mapping_id` (since 1.1, requires ei_device protocol v2) identifies the external resource, such as a ScreenCast stream, that the region corresponds to. `ei_region_get_physical_scale` relates logical to physical pixels ([regions](https://libinput.pages.freedesktop.org/libei/api/group__libei-region.html)).
- Senders must wait for `EI_EVENT_DEVICE_RESUMED`. After `EI_EVENT_DEVICE_PAUSED`, "any events sent from this device will be discarded until the next resume".
- Keyboards carry an XKB keymap FD ([keymap](https://libinput.pages.freedesktop.org/libei/api/group__libei-keymap.html)).
- `ei_device_text_utf8` sends text on devices with the `TEXT` capability ([sender API](https://libinput.pages.freedesktop.org/libei/api/group__libei-sender.html)).
- libei guarantees `DEVICE_REMOVED`, then `SEAT_REMOVED`, then `DISCONNECT` ordering, and `ei_ping`/`PONG` provides round-trip synchronization.

### 2.4 Compositor differences

| Desktop | Capture portal | Input portal | Window introspection available to Cyber | Stance |
|---|---|---|---|---|
| GNOME (Mutter + xdg-desktop-portal-gnome) | ScreenCast **(inference: present; versions to be recorded in acceptance)** | RemoteDesktop **(inference: EIS support exists in recent releases; unverified here)** | `org.gnome.Shell.Introspect.GetWindows` exists but is restricted to an allow-list of portal senders unless the shell runs in unsafe mode ([GNOME discourse](https://discourse.gnome.org/t/unable-to-call-the-remote-getwindows-gnome-method-via-dbus/21201), [gnome-shell#3943](https://gitlab.gnome.org/GNOME/gnome-shell/-/issues/3943)) | Candidate for capture acceptance. Input refused (§5). |
| KDE Plasma (KWin + xdg-desktop-portal-kde) | ScreenCast **(inference)** | RemoteDesktop. Whether and from which Plasma release `ConnectToEIS` is implemented was **not confirmed** by this research. | KWin scripting/D-Bus is compositor-private ([KWin API](https://develop.kde.org/docs/plasma/kwin/api/)) | Candidate for capture acceptance. Input refused (§5). |
| wlroots (Sway etc. + xdg-desktop-portal-wlr) | ScreenCast and Screenshot only, per its README ([xdg-desktop-portal-wlr](https://github.com/emersion/xdg-desktop-portal-wlr#readme)) | Not provided by that backend | `wlr-foreign-toplevel-management` exposes activated state but not other clients' secure fields or hit targets | Required negative/capability test. Capture only if the probe passes. |
| X11/Xorg | n/a | n/a | n/a | Unsupported in this release (`x11_session_unsupported`) |

The desktop name (`XDG_CURRENT_DESKTOP`) is reported for diagnostics and matched against the accepted-version table only. It never enables a capability: capability comes from probing portal interfaces and versions (§6).

## 3. Milestones

- **Milestone A (this change's implementation scope): capture only.** It uses the ScreenCast portal only, not RemoteDesktop, so the portal dialog asks to share a screen or window and not to control the desktop. All `computer` input actions on Linux return `target_assurance_unavailable` before any intent is journaled and with zero OS side effects.
- **Milestone B (specified, disabled): negotiated input.** This covers the RemoteDesktop + EIS transport in §7. It can be built and tested against mock EIS, but it is enabled on a real desktop only after the owner approves a compositor-specific assurance provider (§5.3, decision D1), together with its own acceptance evidence. A capture-only milestone is not completion of #43's input scope.

## 4. Architecture mapped onto existing seams

### 4.1 Backend trait (additive, default methods keep macOS unchanged)

```text
trait Backend {
  acquire_lease()                       // Linux: flock on $XDG_RUNTIME_DIR/cyber-computer.lock (O_CLOEXEC, dir 0700)
  health(prompt) -> Health              // never shows a chooser; `prompt` only gates OS-permission prompts that exist on the platform
  resolve(selector) -> NativeTarget     // NEVER prompts; for pick selectors returns the live bound source or fails
  review(selector, cancel) -> Review    // NEW, default = resolve; owner preview only; may show the portal dialog
  end_scope(target)                     // NEW, default no-op; closes portal session/stream/EIS, idempotent, non-blocking
  set_revocation_sink(sink)             // NEW, default no-op; backend pushes asynchronous revocation reasons
  capture(target) -> Capture
  input(target, permit)                 // Linux milestone A: always Err(InputUnavailable(target_assurance_unavailable))
}
```

- `review` exists because Linux target selection is the act of consent. The portal dialog must appear only from the authenticated owner `preview` path, never from `resolve`, which the runtime calls on every screenshot and input.
- `end_scope` runs on Stop, expiry, revocation and review abandonment. It never blocks Stop: the controller generation is advanced first (`Controller::stop`), and teardown happens on the worker.
- The revocation sink is wired by the runtime to `Computer::stop(session)` for the bound grant, which advances the generation and fences in-flight permits. Revocation reasons are platform-neutral codes (§6.2).

### 4.2 Selector and target identity (shared with Windows, decision D2)

- `Selector` gains `pick: Option<PickScope>`, with `PickScope = window | output`. Exactly one of `app`, `display` or `pick` must be set.
  - On Linux, `app` and `display` are refused with `selector_unsupported_on_platform`. Cyber never interprets a string as a Linux application ID, and it never auto-selects a monitor by index.
  - On macOS, `pick` is refused until a macOS picker exists.
  - The CLI cannot pre-select or widen the portal choice: `pick` only fixes the source *type* offered to the portal (`types` = WINDOW or MONITOR, `multiple=false`).
- `NativeTarget` keeps its existing fields for compatibility and gains an additive `source` description: `{ platform: "linux", kind: "window"|"output", backend: "xdg-portal", stream_serial?: u64, node_id: u32, portal_stream_id?: string, mapping_id?: string, logical_position?: (i32,i32), logical_size?: (i32,i32), scope_generation: u64 }`.
  - Legacy macOS-only fields (`app`, `pid`, `window`, `launch_ms`) are `None`/`0` on Linux. They are documented as macOS-only and meaningless elsewhere, so Linux reports no fictional PID or window number.
  - Whether to make them optional in a schema revision is decision D2.
- The identity string is `pick:<kind>:<scope_generation>:<serial or node:<id>>`. `selector_for_identity` gains a `pick:` branch that resolves only against the live bound portal scope. A new review always gets a new `scope_generation`, so an old identity can never match a new source.

### 4.3 Health (additive, platform-neutral)

`Health` keeps `supported`, `platform`, `minimum_version`, `screen_capture`, `accessibility` and `binary`. It adds:

```text
backend: "macos-sck" | "xdg-portal" | "unsupported"
session_type: "wayland" | "x11" | "tty" | "none" | "unknown"
desktop: string?                      // diagnostic only, e.g. XDG_CURRENT_DESKTOP; never enables anything
interfaces: [{ name, version }]       // e.g. org.freedesktop.portal.ScreenCast 6, ...RemoteDesktop 2, libei protocol
capture: bool                         // capture can be offered now
input_control: bool                   // input can be offered now (macOS: == accessibility; Linux milestone A: false)
input_assurance: "verified" | "unavailable"
refusals: [code]                      // specific, stable snake_case codes (§6.2)
```

- On Linux, `accessibility` stays `false` and `minimum_version` names the Linux prerequisite, e.g. "Wayland session with xdg-desktop-portal ScreenCast v4+".
- The runtime's input admission fact becomes `health.screen_capture && (capture || health.input_control)`, replacing `health.accessibility`. Revocation-on-loss tracks `input_control` the same way it tracks `accessibility` today.
- Generated OpenAPI, the TypeScript SDK and CLI/TUI/dashboard rendering are updated. Older clients ignore the new fields.

### 4.4 Runtime flow on Linux (milestone A)

1. **`status` / `health(false)`.** The backend probes without creating a portal session and without showing UI (§6). D-Bus activation of the portal frontend is allowed with a bounded timeout. Activation is not a chooser **(inference: activating `org.freedesktop.portal.Desktop` shows no UI by itself)**.
2. **`preview(session, {pick})`, owner only.** The runtime checks the feature, presence and Session/Location as today, then calls `backend.review` on the blocking pool:
   - `CreateSession` with unguessable handle tokens.
   - `SelectSources` with `types` = the picked kind, `multiple=false`, a `cursor_mode` advertised by the portal (decision D6), and **no `persist_mode` and no `restore_token`**.
   - `Start` with an empty parent window. The owner chooses in the portal dialog.
   - The response must contain exactly one stream whose `source_type` equals the requested kind. Anything else is closed and refused (`portal_selection_mismatch`).
   - `OpenPipeWireRemote`, then connect to the stream by `pipewire-serial` when ScreenCast ≥ 6. Otherwise connect by node ID, but only within this portal remote and this `scope_generation` (§8).
   - Capture **one review frame** into private storage. It is visible to the owner only, through the existing authenticated frame route, and never projected to the model.

   The wait for the portal dialog is bounded (default 120 s, decision D9) and cancellable by Stop: Stop advances `preview_generation`, and the backend calls `Request.Close`/`Session.Close`. The existing 30 s review deadline starts after the portal response, not before the dialog.
3. **Owner confirmation.** The preview shows the source kind, logical size, and position for outputs, together with the review frame. For outputs it also shows the exposure warning: everything visible on that output, including other applications and notifications, will reach the model. The owner confirms the **actual selected source**. `allow` re-resolves the live scope and must find the same identity and geometry, as today.
4. **`allow`.** This acquires the host lease and arms the controller with the existing TTL and limits. The pending portal scope is adopted by the grant. A review that is not adopted before its deadline, or that is superseded, gets `end_scope`.
5. **`screenshot`.** This follows the existing journal → permit → `capture` → `Capture::validate` → `controller.frame` path. Linux `capture` waits (bounded, 5 s) for the next complete buffer *after* the request, validates it (§8.2), converts it, downscales it to ≤ 1920 px on the longest edge, and PNG-encodes it to ≤ 8 MiB.
6. **Input.** Refused at admission (`input_control=false`). No journal intent is written because nothing is admitted, so there is nothing to reconcile.
7. **Stop / expiry / revocation.** The controller generation is fenced first, then `end_scope` closes the portal session, PipeWire stream and FDs.

### 4.5 Native worker topology (decision D3)

All portal, PipeWire and EIS objects and FDs live in one **same-binary helper process** (`cyber` re-executed with a hidden internal subcommand) per grant scope. The helper is not downloaded and needs no compiler. It applies these protections:

- **`PR_SET_DUMPABLE=0`.** Same-UID processes cannot `ptrace` it or open `/proc/<pid>/fd/*`. This includes sandboxed tool children and, best effort, unsandboxed ones **(inference from Linux ptrace access-mode rules; must be tested)**.
- **`PR_SET_PDEATHSIG=SIGKILL`.** If the server dies, the helper dies. Its D-Bus connection drops, which the portal treats as session close.
- **Close-on-exec.** Every FD, including those received over D-Bus via `SCM_RIGHTS`, is set `FD_CLOEXEC` immediately on receipt regardless of library defaults. The helper never spawns children.
- **Narrow IPC.** A socketpair to the server carries only typed requests (`review`, `capture`, `end_scope`) and bounded responses. The helper never receives model text except, in milestone B, the admitted action for dispatch.
- **Hard Stop.** Stop can `SIGKILL` the helper. The host lease is released only after `waitpid` confirms the helper has exited, so no live worker can dispatch after its lease is transferred. This makes the existing guarantee ("cancellation cannot release the host lease while a native worker can still dispatch") hold even if the helper hangs in a native call.

The alternative is an in-process worker thread with the same CLOEXEC discipline. It is simpler, but it cannot be force-stopped without stopping the server, and its FDs share the server's `/proc` exposure.

## 5. Target assurance

### 5.1 Required observations (from the existing contract)

Before every input event, the macOS backend establishes:

- **A1, focused target:** the focused window belongs to the granted target.
- **A2, pointer hit:** the topmost surface under the pointer belongs to the granted target and is not covered.
- **A3, secure field:** the focused or hit element is not a secure text field.
- **A4, protected surfaces:** the target is not a permission, system, authentication or lock dialog.
- **A5, self-approval:** the target is not a Cyber approval surface (TUI terminal, dashboard browser tab).

### 5.2 What Linux mechanisms can prove

| Mechanism | A1 | A2 | A3 | A4 | A5 | Notes |
|---|---|---|---|---|---|---|
| Portal ScreenCast/RemoteDesktop acceptance | no | no | no | no | no | Grants capture or input. Says nothing about focus or hit target **(inference from the documented interfaces)**. |
| Wayland core protocol as an ordinary client | no | no | no | no | no | Clients do not learn about other clients' surfaces **(inference: Wayland's per-client isolation)**. |
| `ext-foreign-toplevel-list-v1` | no | no | no | no | partial | Exposes title, app_id and identifier only. "Intentionally minimalistic", with no geometry and no activation state ([protocol](https://wayland.app/protocols/ext-foreign-toplevel-list-v1)). |
| `wlr-foreign-toplevel-management-unstable-v1` | partial | no | no | no | partial | wlroots only, where input is unavailable anyway ([protocol](https://wayland.app/protocols/wlr-foreign-toplevel-management-unstable-v1)). |
| `text-input-unstable-v3` content purpose `password` | no | no | compositor/IME only | no | no | Applications declare password purpose to the compositor and input method. Cyber is neither ([protocol](https://wayland.app/protocols/text-input-unstable-v3)). |
| AT-SPI accessibility bus | self-reported | weak | self-reported | self-reported | weak | Applications describe themselves, and support is opt-in per toolkit. A hostile or buggy application can omit or misreport roles. Screen-coordinate extents are unreliable under Wayland **(inference)** ([libatspi](https://gnome.pages.gitlab.gnome.org/at-spi2-core/libatspi/)). Usable only as a deny signal. |
| GNOME Shell Introspect `GetWindows` | yes (inference) | no | no | partial | partial | Restricted to allow-listed senders; Cyber is not one. Calling it would need unsafe mode, a security downgrade Cyber must not request. |
| KWin scripting / D-Bus | yes (inference) | partial (inference) | no | partial | partial | Compositor-private and version-specific. Loading scripts into the compositor is itself a privileged action. |
| logind `LockedHint` | — | — | — | lock only | — | Set by the desktop, so it is a hint ([login1](https://www.freedesktop.org/software/systemd/man/latest/org.freedesktop.login1.html)). Usable as a revocation trigger, not as proof that the screen is unlocked. |

**Conclusion.** No standard, portal-reachable mechanism proves A1–A5 on any accepted desktop today. The issue's architectural inference holds under this research. Whole-output input on Linux would therefore deliver host-user input to whatever is focused or under the pointer. That includes terminals, where typed text becomes arbitrary command execution as the user, and the owner's own Cyber TUI or dashboard. The controller's supervisor fence blocks input while human approvals are *pending*. It does not stop typed commands from reaching a terminal.

### 5.3 Decision and refusal

- Milestone A refuses all Linux input with `target_assurance_unavailable`, and health reports `input_assurance: "unavailable"`. A portal RemoteDesktop acceptance never counts as assurance.
- The design defines an **`AssuranceProvider`** seam for milestone B. A provider returns, for one admission, `{a1..a5: Proven | Refused(reason) | Unknown, generation, observed_at}`. Admission requires every observation to be `Proven` for the exact target generation, taken after the frame and immediately before dispatch. Any `Unknown` or absent observation is a refusal. Deny-only signals (AT-SPI password role, logind lock) can turn `Proven` into `Refused` but can never produce `Proven`.
- **Race limits.** Observation and dispatch are separate operations on every platform, macOS included. A provider must state its race window and how it re-checks between events of a multi-event action (drag, chord, text). If it cannot re-check per event, the action is limited to single-event forms or refused.
- Adding a provider (for example a GNOME Shell or KWin mechanism) requires: a named compositor version, an explanation of why it is not a "compositor-private automation bypass", its access-control model, its race window and real-desktop evidence. It is decision D1, with separate owner review.
- **XWayland targets** are refused for input even if a provider exists, until they are separately tested. XWayland clients can observe each other through X11, so the assurance model differs **(inference)**.

## 6. Capability detection and health

### 6.1 Probe order (bounded, read-only, no chooser)

1. **Build.** A musl static build gives `unsupported_build`, because the dynamic `libpipewire` cannot be loaded (§10).
2. **Session.** Check `XDG_SESSION_TYPE`, `WAYLAND_DISPLAY` and `DISPLAY`. With no Wayland display: `no_graphical_session` if nothing is set (SSH or headless service), or `x11_session_unsupported` if only X11 is present. Environment values are treated as hints. A later portal failure still refuses.
3. **D-Bus session bus** reachable within 2 s, else `no_session_bus`.
4. **Portal frontend.** `org.freedesktop.portal.Desktop` must answer the `version` property reads within 3 s, else `portal_unavailable`.
5. **ScreenCast.** Version ≥ 4 (needed for a stable `id`) and `AvailableSourceTypes` containing WINDOW and/or MONITOR, else `screencast_unavailable` / `screencast_too_old`. When v6 is absent, report `pipewire_serial_unavailable` as informational and use node binding (§8.1).
6. **PipeWire client library.** `libpipewire-0.3.so.0` must load, else `pipewire_library_missing`. No PipeWire connection is opened during status.
7. **RemoteDesktop** (informational in milestone A): version ≥ 2, else `remote_desktop_unavailable` / `eis_unavailable`.
8. **Input:** always `target_assurance_unavailable` in milestone A.

`health(prompt=true)` (owner `setup`) performs the same probe. Linux has no separate OS permission to request, so `setup` shows no UI. The only Linux UI is the portal dialog during owner review.

### 6.2 Refusal and revocation codes (stable, platform-neutral naming)

`unsupported_build`, `no_graphical_session`, `x11_session_unsupported`, `no_session_bus`, `portal_unavailable`, `screencast_unavailable`, `screencast_too_old`, `pipewire_library_missing`, `pipewire_connect_failed`, `capture_buffer_unsupported`, `remote_desktop_unavailable`, `eis_unavailable`, `target_assurance_unavailable`, `xwayland_target_unsupported`, `selector_unsupported_on_platform`, `portal_selection_cancelled`, `portal_selection_mismatch`, `portal_session_closed`, `session_bus_lost`, `stream_removed`, `source_changed`, `session_locked`, `eis_disconnected`, `eis_device_paused`, `eis_device_removed`.

Each code maps to an actionable message, such as "log into a Wayland session" or "install your desktop's portal backend through your distribution". Cyber never edits portal configuration such as `*-portals.conf` and never installs packages.

## 7. Input transport (milestone B, specified but disabled)

- **Portal session.** Use RemoteDesktop v2: `SelectDevices` (POINTER and KEYBOARD only, no touchscreen, no `persist_mode`, no `restore_token`), the combined ScreenCast `SelectSources` on the same session, and `Start`. The response `devices` must contain what was requested, else refuse. Then call `ConnectToEIS` exactly once. After EIS connects, Cyber never calls `Notify*`. If `ConnectToEIS` fails, Cyber refuses (`eis_unavailable`) and does not downgrade to `Notify*`. A `Notify*` transport would need its own design review (decision D7).
- **Negotiation.** Bind the seat for `POINTER_ABSOLUTE`, `BUTTON`, `SCROLL`, `KEYBOARD` and, if offered, `TEXT`. Send only after `DEVICE_RESUMED`.
  - Pointer targeting uses the absolute device region whose `mapping_id` equals the captured stream's `mapping_id`. If `mapping_id` is missing on either side, or more than one region matches, input is refused (`source_changed`).
  - The desktop point from `Geometry::desktop` must satisfy `ei_region_contains`.
- **Device lifecycle.** `DEVICE_PAUSED`, `DEVICE_REMOVED`, `SEAT_REMOVED`, `DISCONNECT`, a region change or a keymap change revokes the grant (`eis_device_paused`, etc.). Cyber does not wait to resume. Events sent while paused would be discarded by the EIS implementation, so an action interrupted this way settles as `unknown`.
- **Settlement.** An action is `posted` only after an `ei_ping` round trip completes following the frame's last event. Transport loss after any event was queued settles as `unknown` and triggers revocation. Cyber never re-clicks or re-types.
- **Keys and text.**
  - Named keys and chords map to evdev keycodes through the negotiated XKB keymap, using xkbcommon to find the keycode and level for a keysym. A keysym that needs an unexpected modifier or level, or that is absent from the keymap, fails closed.
  - Unicode `type` uses `ei_device_text_utf8` only on a `TEXT`-capable device. Without it, Cyber types only characters reachable through the current keymap and refuses the rest.
  - Cyber never uses the clipboard and never synthesizes compose sequences.
- **Cleanup.** On cancellation or Stop mid-action, release every pressed button and key and send a frame. If release cannot be confirmed by ping, the receipt reports uncertainty.

## 8. Coordinates, scaling, rotation and hotplug

### 8.1 Stream binding

- **ScreenCast ≥ 6:** target by `pipewire-serial`. It is never reused.
- **ScreenCast 4–5:** target by node ID on the portal-restricted remote. The node must be present in that remote's registry with matching properties, and the binding includes `scope_generation`. Removal of the node, a registry `global_remove`, or a node reappearing under the same ID fails with `stream_removed` and revokes the grant. The restricted remote limits visible nodes to the session's own streams, but reuse inside that remote after hotplug is still possible per the portal documentation, so any removal is terminal.

### 8.2 Frame validation (bounded conversion)

- **Buffer types.** Accept only shared-memory buffer types (MemFd/MemPtr) in milestone A. If the compositor offers only DMA-BUF, refuse with `capture_buffer_unsupported` **(inference: GNOME and KDE fall back to shared memory when the client does not advertise DMA-BUF modifiers; to be verified per desktop)**. Requiring DMA-BUF would add GPU import dependencies (decision D8).
- **Formats.** Allow-list 32-bit BGRx/BGRA/RGBx/RGBA. Check `stride ≥ width*4`, `stride*height ≤ chunk size ≤ mapped size`, and pre-scale dimensions ≤ 16384 per side. Reject any buffer with the corrupted-header flag. Reject any frame carrying a non-identity video transform until transforms are implemented and tested.
- **Scaling.** Downscale with a bounded allocator to ≤ 1920 px on the longest edge. PNG-encode to ≤ 8 MiB. `Capture::validate` keeps checking the PNG header dimensions.

### 8.3 Geometry

- **Output scope.** `bounds` = the stream's `position` and `size` in compositor logical coordinates. Image size = the encoded image size. `Geometry::desktop` already maps linearly, so fractional scales (for example 1.25) and negative origins work unchanged.
- **Window scope.** The portal provides no `position` for window streams. A window frame therefore has **no desktop mapping**: it is valid for observation, and any input on it is refused (`source_changed`). Under milestone A this only adds a second reason for an existing refusal.
- **Rotation.** Monitor streams are expected to deliver display-oriented content whose logical `size` already reflects rotation **(inference)**, so `rotation=0` is recorded. A buffer whose dimensions disagree in orientation with logical `size`, or a transform meta, is refused until rotated-output evidence exists.
- **Hotplug and renegotiation.** A PipeWire format, size or param change, a missing `size` property, a monitor unplug or a scale change advances `scope_generation`. That revokes the grant (`source_changed`), and the next action requires fresh owner review. Frames from before the change fail the existing geometry/target comparison.
- **Pointer mapping (milestone B).** Pointer coordinates go to the libei region in its logical space. `ei_region_get_physical_scale` is recorded for diagnostics and not used for absolute mapping, because absolute regions are already in logical pixels per the libei documentation.

## 9. Lifecycle and revocation

| Event | Effect |
|---|---|
| Owner Stop (CLI, API, TUI, dashboard), Session interrupt, logout | Existing controller fence first. Then `end_scope`, `Session.Close`, helper exit, and lease release after `waitpid`. |
| Presence loss, presence epoch change | Existing permanent revocation |
| TTL expiry, action limit | Existing behavior plus `end_scope` |
| Portal `Session::Closed` (user ends sharing from the shell indicator, portal decides) | Revoke with `portal_session_closed` |
| D-Bus connection lost, portal frontend restarted (`NameOwnerChanged`) | Revoke with `session_bus_lost`. Never re-create silently. |
| PipeWire stream state error/unconnected, node removed | Revoke with `stream_removed` |
| Source renegotiated (size, format, scale) | Revoke with `source_changed` |
| logind `LockedHint=true` or `Lock` signal on the Cyber server's own session | Revoke with `session_locked`. Unlock never restores the grant. |
| EIS disconnect, device pause or removal (milestone B) | Revoke. In-flight action settles `unknown`. |
| Compositor crash or restart | Observed as bus/stream/EIS loss and revoked |
| Server restart | The helper dies (PDEATHSIG), and the portal closes its sessions on D-Bus vanish. Existing rule: no grant survives restart. |
| Late portal reply after Stop or review supersession | Generation-fenced. The reply's session is closed immediately and never adopted. |

Restore tokens are never requested (`persist_mode` is omitted, so it defaults to 0) and never consumed. Every grant starts with a fresh portal dialog and a fresh Cyber review.

## 10. Dependency and version matrix

Crate versions are those observed on crates.io on 2026-11-14 for planning. Implementation pins exact reviewed versions, preferring releases at least two weeks old. Workspace license is Apache-2.0 and workspace MSRV is 1.89.

| Component | Use | Version (planning) | License | Link/load | Build needs | Runtime needs |
|---|---|---|---|---|---|---|
| `zbus` | D-Bus client, portal proxies, logind signals | 6.x (6.3.0 observed) | MIT | Pure Rust, static | none | session bus |
| `ashpd` (alternative to hand-written proxies, decision D5) | Portal wrappers | 0.14.0 | MIT | Pure Rust | none | portal |
| `libpipewire-0.3` via a narrow hand-written FFI subset loaded with `libloading` | Restricted remote + stream consumer | system lib ≥ 0.3.x (exact minimum set after acceptance) | MIT (PipeWire), ISC (`libloading` 0.9.x) | `dlopen("libpipewire-0.3.so.0")` on demand, Linux GNU builds only | none (no bindgen, no headers) | PipeWire daemon and library from the distribution |
| `pipewire` / `libspa` crates (alternative, decision D4) | Safe bindings | 0.10.1 | MIT | Dynamic link (`DT_NEEDED`), so the binary fails to start without libpipewire | `libpipewire-0.3-dev`, pkg-config, libclang (bindgen) | same |
| `reis` (milestone B) | ei protocol client, pure Rust | 0.7.x | MIT | Static | none | compositor EIS via portal FD |
| `libei` (alternative to `reis`) | C sender API | ≥ 1.1 for `mapping_id`, ≥ 1.6 for `TEXT` | MIT | would need dlopen | headers if linked | system libei |
| `xkbcommon` crate + `libxkbcommon` (milestone B) | Keymap lookup | crate 0.9.x; lib ≥ 1.0 | MIT | dlopen preferred (same reasoning as PipeWire) | none with dlopen | libxkbcommon |
| `png` / `image` (already in lockfile) | Encode, downscale | png 0.18.1, image 0.25.10 | MIT OR Apache-2.0 | static | none | none |
| `rustix` or `libc` (already present) | `prctl`, `fcntl`, `flock`, socketpair | libc 0.2 | MIT/Apache | static | none | none |

Platform interfaces:

| Interface | Minimum for milestone A | Preferred | Milestone B |
|---|---|---|---|
| `org.freedesktop.portal.ScreenCast` | v4 | v6 (`pipewire-serial`) | v5+ (`mapping_id`) |
| `org.freedesktop.portal.RemoteDesktop` | not used | — | v2 (`ConnectToEIS`) |
| `org.freedesktop.host.portal.Registry` | optional (D10) | v1 | v1 |
| PipeWire | 0.3 series with portal-restricted remotes | current distribution release | same |
| ei protocol | — | — | `ei_device` v2 (mapping id); `TEXT` optional |

**Release builds.** `*-linux-gnu` builds include the backend and load libraries lazily, so a host without PipeWire still starts and reports `pipewire_library_missing`. `*-linux-musl` static builds report `unsupported_build`, because a static musl binary cannot load the distribution's glibc-linked libpipewire **(inference from static-musl dynamic-loading limits; a pure-Rust PipeWire protocol client would lift this and is out of scope)**. CI release runners (ubuntu-24.04) need no new packages for the dlopen design.

**Accepted-version table (to be filled only from real runs).**

| Desktop | Distro | Compositor | Portal frontend / backend | PipeWire | Capture | Input |
|---|---|---|---|---|---|---|
| GNOME Wayland | TBD (D9) | TBD | TBD | TBD | not tested | refused |
| KDE Plasma Wayland | TBD (D9) | TBD | TBD | TBD | not tested | refused |
| Sway (wlroots) | TBD | TBD | xdg-desktop-portal-wlr TBD | TBD | negative/capability test | refused (no RemoteDesktop) |
| Xorg | any | — | — | — | refused | refused |

## 11. Threat model

### 11.1 Assets

- Pixels of the owner's desktop, including other applications, notifications and secrets visible on an output.
- Host-user input authority. In milestone B this is equivalent to arbitrary command execution through any terminal.
- Portal, PipeWire and EIS FDs, which are bearer capabilities for capture or input.
- Owner credentials and Cyber approval surfaces.
- Durable records: frames, receipts and typed-text digests.

### 11.2 Adversaries

- **M, the model,** and any provider steering it.
- **S, screen content:** web pages, documents and chat messages displayed in the captured source and read by the model.
- **P, project content:** repository configuration, hooks, plugins and sandboxed tool children running as the same UID.
- **C, other local same-UID processes** without owner credentials.
- **D, a malicious or buggy desktop application** that misreports accessibility roles or mimics dialogs.
- **R, remote collaborators:** hub members, peers and child agents.

### 11.3 Attack paths and mitigations

| # | Path | Mitigation |
|---|---|---|
| T1 | M or S convinces the owner, or a tool, to widen scope (window to output) or to keep sharing | Only the owner `preview`/`allow` path triggers the portal. The model cannot call it. Output review shows an exposure warning and the actual review frame. Screen text is untrusted and grants nothing (existing spec). |
| T2 | P enables the feature or arms a grant | Existing global/managed-only configuration and owner-authenticated routes. Linux adds no environment- or project-driven enablement. |
| T3 | C or P steals portal/PipeWire/EIS FDs (`/proc/<pid>/fd`, inherited FDs) to capture or inject outside Cyber's fences | Helper process with `PR_SET_DUMPABLE=0`, `FD_CLOEXEC` on every FD, no child spawning, socketpair IPC only. FD-inheritance and `/proc` access tests. Full-access owner shells with ptrace capability remain outside the boundary (residual). |
| T4 | C opens the user's unrestricted PipeWire socket itself | Out of Cyber's control and unchanged by this feature. Cyber never uses an unrestricted connection, so it adds no new capability (residual, pre-existing). |
| T5 | A restored portal permission lets capture start without a fresh owner decision | No `persist_mode`, no `restore_token`, ever. Tested by asserting request option dictionaries. |
| T6 | Node-ID reuse after hotplug makes Cyber capture a different source | Serial targeting (v6). Otherwise node binding within a generation, plus terminal revocation on removal. |
| T7 | Input lands in a terminal, a password field, a polkit dialog or Cyber's own TUI | Milestone A refuses all input. Milestone B requires A1–A5 `Proven` by an approved provider. Absence of proof is refusal. |
| T8 | Stop is ineffective while a native call blocks (portal dialog, PipeWire wait, EIS) | Generation fence first. Helper SIGKILL. Lease released only after `waitpid`. Bounded waits everywhere. |
| T9 | Lock screen or session switch while armed | Revoke on logind lock or the session becoming inactive. Unlock never restores. Lock-screen behavior is to be verified on real desktops (residual until then). |
| T10 | Malformed PipeWire buffers (size/stride overflow) cause memory corruption | Strict size checks before reading, shared-memory-only buffers, allow-listed formats, bounded dimensions, and fuzz tests on the converter. |
| T11 | Screenshot leakage through logs, exports, hooks, collaboration or child environments | Existing private artifact root, hidden paths and redaction. Linux adds no new storage path. The review frame uses the same private store. |
| T12 | D spoofs accessibility roles to look safe | AT-SPI is never a positive proof (§5.3). |
| T13 | R gains control through inherited authority | Existing non-inheritance rules. Linux adds no route. |
| T14 | Portal dialog spoofing: an application draws a fake chooser | The owner starts review from Cyber's own authenticated surface. The resulting review frame, not the dialog, is what the owner confirms. |

### 11.4 Residual risk

- Authorized captures send all visible content of the chosen source to the selected model provider. Output scope can include notifications and other applications that appear during the grant.
- A same-UID process with an unrestricted PipeWire connection, or with ptrace rights over Cyber (root or `CAP_SYS_PTRACE`), is outside this boundary.
- Portal indicator, lock-screen and compositor behaviors differ between desktops and are known only from real-desktop runs.
- When milestone B lands, observation-to-dispatch races remain, as on macOS. The provider's stated race window bounds them, but they do not disappear.

## 12. Verification strategy

- **Deterministic (CI, all platforms).** Selector validation, health serialization, refusal codes, identity parsing, generation fencing, geometry and frame-validation units, and the input refusal path with zero journal intents.
- **Mock integration (CI Linux).** A private `dbus-daemon --session` with an in-repo fake portal implementing `ScreenCast`, `Session` and `Request` (and `RemoteDesktop` for milestone B), a headless PipeWire daemon with a test video producer, and a fake EIS server built on `reis`'s server side **(inference: `reis` exposes EIS-side types; to be confirmed)**. Covers consent denial/cancel, delayed and stale responses, wrong-session responses, stream reuse/removal, bad buffers, transforms, revocation, late workers, missing libraries and missing services. Headless CI is not desktop acceptance.
- **Real desktops (owner-run, disposable).** Specified in `tasks.md` §8. They record exact versions and use only synthetic windows and text.

## 13. Open decisions for the owner

Each decision lists the recommended option first.

- **D1 — Linux input assurance strategy.**
  - **Recommended:** ship milestone A capture-only and keep input refused. Accept a compositor-specific `AssuranceProvider` only through a separate reviewed change with real evidence.
  - Alternatives:
    - (b) Use GNOME Shell Introspect or KWin scripting now. This needs unsafe mode or compositor-private access, so it is rejected.
    - (c) Use AT-SPI as positive proof. Rejected, because the data is self-reported.
    - (d) A separate future design in which Cyber launches its own nested compositor for one application, so that it is the authority for focus and hit-testing. That is a different trust model and out of scope here.
- **D2 — Shared schema shape (with Windows, #44).**
  - **Recommended:** additive `Selector.pick`, `NativeTarget.source` and `Health` capability fields, keeping legacy macOS fields (zero/None elsewhere) for one release and then reconsidering, all in one backwards-compatible foundation PR.
  - Alternative: versioned `ComputerTarget2`/`ComputerHealth2` types.
- **D3 — Worker topology.**
  - **Recommended:** a same-binary helper process per grant (non-dumpable, parent-death signal, killable Stop).
  - Alternative: an in-process worker thread (simpler, weaker FD isolation, no forced termination).
- **D4 — PipeWire binding.**
  - **Recommended:** a narrow hand-written FFI subset loaded with `dlopen` on GNU builds, with musl builds reporting `unsupported_build`.
  - Alternatives:
    - `pipewire-rs`, which adds a startup dependency on libpipewire for every Linux user.
    - A pure-Rust PipeWire protocol client, which is unmaintained territory and a large audit surface.
- **D5 — Portal client.**
  - **Recommended:** hand-written `zbus` proxies for ScreenCast, RemoteDesktop, Session, Request and Registry (small, auditable).
  - Alternative: `ashpd` (broader, more transitive code).
- **D6 — Cursor mode.**
  - **Recommended:** Embedded when advertised, else Hidden. The model then sees the pointer, and nothing is exposed beyond what is already shared.
  - Alternative: always Hidden.
- **D7 — D-Bus `Notify*` input.**
  - **Recommended:** never, in this change or in milestone B.
  - Alternative: a separately reviewed design with equivalent tests and no automatic downgrade.
- **D8 — DMA-BUF capture.**
  - **Recommended:** not in milestone A. Refuse with `capture_buffer_unsupported` and revisit with evidence.
  - Alternative: a GPU import path (EGL/gbm dependencies).
- **D9 — Initial real-desktop targets and timeouts.**
  - **Recommended:** Ubuntu 24.04 LTS GNOME Wayland, a current Fedora KDE Plasma Wayland, and Sway as the negative test. Portal dialog wait of 120 s.
  - The owner picks the exact releases and whether a second GNOME release is required.
- **D10 — Host portal app identity.**
  - **Recommended:** call `Registry.Register` only when a packaged `.desktop` file for Cyber exists. Never write `.desktop` files automatically, and document that dialogs may show a generic application name otherwise.
  - Alternative: ship and install a `.desktop` file with the Linux packages.

# Linux native computer use (Wayland)

## Why

Native computer use ships on macOS 14+ only (PR #41, `add-native-computer-use`). On Linux the `Native` backend reports unsupported, and issue #43 asks for a reviewed design before any Linux code. Linux desktops differ from macOS in ways that matter for the existing security contract:

- Wayland gives an ordinary client no view of other clients' windows, focus or pointer hit targets. Screen capture and input injection go through the XDG Desktop Portal, which asks the user to pick a source and approve control.
- Accepting a portal dialog grants capture or input. It does not tell Cyber which window has keyboard focus, what the pointer would hit, whether a password field or system dialog is in front, or whether a Cyber approval surface is visible. The macOS backend checks all of these before every input event.

This change states which Linux guarantees can be met, which cannot, and what Cyber refuses when they cannot.

## What changes

- **Capture-only Linux backend (milestone A).** On a Wayland session with a suitable portal backend, an owner can grant a Session observation of one window or one output that the owner picks in the portal's own dialog. Capture uses the ScreenCast portal and a portal-restricted PipeWire connection. Existing controls still apply: owner-only grants, finite TTL, presence, Stop, private screenshots and frame binding.
- **Input stays refused on Linux.** Every input action is refused with `target_assurance_unavailable` until an owner-approved assurance provider can prove focused-window, pointer-hit, secure-field, protected-dialog and self-approval checks on a specific compositor. The design sketches the input transport for a later milestone B (RemoteDesktop v2, `ConnectToEIS`/libei, `mapping_id` region matching and keymap/text negotiation). That transport, its requirements and its tasks belong to a separate follow-up change, so this change adds no transport requirement.
- **Platform-neutral contract additions** (coordinated with the Windows effort, issue #44):
  - an owner-picker target selector (`pick: window | output`) instead of reinterpreting macOS bundle IDs;
  - additive health fields: backend, session type, interface versions, capture/input availability and specific refusal codes;
  - an additive platform-neutral target description.

  Existing macOS fields and behavior are unchanged.
- **Lifecycle and revocation.** Portal `Closed`, bus loss, stream removal, source renegotiation, lock or deactivation of the user's watched graphical session, loss of that watch, server restart and portal restart revoke the grant and require fresh review. A grant is never armed when the graphical session cannot be identified and watched. Restore tokens and persistent portal permissions are never requested or used.
- **Session-bus trust.** Portal signals are accepted only from the pinned portal bus name on Cyber's exact request and session paths. When the feature is enabled, Linux sandboxed tool children lose session-bus access (decision D11).
- **Explicit unsupported results.** X11/Xorg sessions, XWayland-only targets, wlroots input, SSH/headless sessions, musl static builds, and a missing portal, PipeWire or EIS each get a specific refusal code with no OS side effects and no fallback.
- **CLI.** `cyber computer allow <session> --pick window|output` starts owner review through the portal dialog in an interactive terminal and arms only the reviewed source, by its review ID, after a confirmation prompt. `--pick --yes` is refused. `--preview <id> --yes` confirms a review already seen on the dashboard or TUI. `status` never shows a chooser.

## Impact

- `crates/cyber-computer`: a new `linux` backend module (Wayland only), Backend trait default methods for owner review, scope teardown and asynchronous revocation, and additive `Selector`/`Health`/`NativeTarget` fields. `Error::UnsupportedPlatform` text becomes platform-neutral.
- `crates/cyber-server/src/runtime/computer`: pick-selector identities, review that may block on a portal dialog (bounded and cancellable by Stop), a revocation sink wired into controller Stop, and a generalized input-readiness fact (`input_control` in place of macOS `accessibility`).
- HTTP/OpenAPI/SDK: additive request field and health/target fields, plus regenerated clients. Existing routes stay.
- CLI/TUI/dashboard: a `--pick` option, a review card that shows the actual portal-selected source with a private review frame, and an "input unavailable on this platform" state.
- Packaging: Linux GNU builds load `libpipewire-0.3` on demand. musl static builds report `unsupported_build` for Linux native capture. Users need no compiler or downloaded helper.
- Docs: `docs/computer-use.md`, the acceptance record and the feature/README matrix gain separate Linux rows for implemented, fixture-tested, real-desktop-accepted and unsupported configurations.

## Non-goals

- Linux input in this change, including the input transport requirements, or any claim of cross-desktop parity with macOS.
- X11/Xorg backends, xdotool/XTest, `/dev/uinput`, root or group-permission instructions, compositor-private input injection, browser-only substitutes, clipboard bridging, headless arbitrary desktop control, unattended or restored grants, and installing or configuring portal backends.
- The D-Bus `Notify*` input transport, and automatic downgrade from EIS.
- Windows computer use (issue #44). This design only coordinates the shared schema with it.
- Release tagging or advertising Linux support before the real-desktop evidence in `tasks.md` exists.

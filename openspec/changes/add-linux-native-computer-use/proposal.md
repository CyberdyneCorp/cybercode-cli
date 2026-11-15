# Linux native computer use (Wayland)

## Why

Native computer use ships on macOS 14+ only (PR #41, `add-native-computer-use`). On Linux the `Native` backend reports unsupported, and issue #43 asks for a reviewed design before any Linux code. Linux desktops differ from macOS in ways that matter for the existing security contract:

- Wayland gives an ordinary client no view of other clients' windows, focus or pointer hit targets. Screen capture and input injection go through the XDG Desktop Portal, which asks the user to pick a source and approve control.
- Accepting a portal dialog grants capture or input. It does not tell Cyber which window has keyboard focus, what the pointer would hit, whether a password field or system dialog is in front, or whether a Cyber approval surface is visible. The macOS backend checks all of these before every input event.

This change states which Linux guarantees can be met, which cannot, and what Cyber refuses when they cannot.

## What changes

- **Capture-only Linux backend (milestone A).** On a Wayland session with a suitable portal backend, an owner can grant a Session observation of one window or one output that the owner picks in the portal's own dialog. Capture uses the ScreenCast portal and a portal-restricted PipeWire connection. Existing controls still apply: owner-only grants, finite TTL, presence, Stop, private screenshots and frame binding.
- **Input stays refused on Linux.** Every input action is refused with `target_assurance_unavailable` until an owner-approved assurance provider can prove focused-window, pointer-hit, secure-field, protected-dialog and self-approval checks on a specific compositor. The design sketches the input transport for a later milestone B (RemoteDesktop v2, `ConnectToEIS`/libei, `mapping_id` region matching and keymap/text negotiation). That transport, its requirements and its tasks belong to a separate follow-up change, so this change adds no transport requirement.
- **Builds on the platform foundation (#134, `add-computer-use-platform-foundation`).** The shared contract that this draft first proposed (decision D2) now lives there: the capability report with `mode`, `capture`/`input` reasons and authority kind, the reason-code registry, the `pick` selector with opaque `target_id` and typed `source`, review-bound arming by `preview_id`, the grant lifecycle and host lease, the shared chord policy, the native worker contract, receipts and the evidence gate. This change only fills those in for Linux: authority `platform_picker`, selectors `["pick"]`, Linux-only reason codes, and Linux evidence rows. Existing macOS fields and behavior are unchanged.
- **Lifecycle and revocation.** Every Linux event is binding loss under the foundation's lifecycle rule: portal `Closed`, bus loss, stream removal, source renegotiation, lock or deactivation of the user's watched graphical session, loss of that watch, server restart and portal restart revoke the grant and require fresh review. A grant is never armed when the graphical session cannot be identified and watched. Restore tokens and persistent portal permissions are never requested or used.
- **Session-bus trust.** Portal signals are accepted only from the pinned portal bus name on Cyber's exact request and session paths. When the feature is enabled, Linux sandboxed tool children lose session-bus access (decision D11).
- **Explicit unsupported results.** X11/Xorg sessions, XWayland-only targets, wlroots input, SSH/headless sessions, musl static builds, and a missing portal, PipeWire or EIS each get a specific refusal code with no OS side effects and no fallback.
- **CLI.** `cyber computer allow <session> --pick window|output` starts owner review through the portal dialog in an interactive terminal and arms only the reviewed source, by its review ID, after a confirmation prompt. `--pick --yes` is refused. The foundation's `--preview <id> --yes` confirms a pick review already seen on the dashboard or TUI. `status` never shows a chooser.

## Impact

- Depends on #134, which adds the shared types, Backend default methods (`review`, `end_scope`, `set_revocation_sink`), review-bound arming, the chord evaluator, receipt fields and the evidence table. This change must not merge before it.
- `crates/cyber-computer`: a new `linux` backend module (Wayland only) implementing the foundation's port, and the Linux reason codes in the registry.
- `crates/cyber-server/src/runtime/computer`: the `pick:` identity branch and a portal review that may block on the dialog (bounded and cancellable by Stop).
- HTTP/OpenAPI/SDK: only the Linux reason codes; the shared fields come from #134. Existing routes stay.
- CLI/TUI/dashboard: a `--pick` option, a review card that shows the actual portal-selected source with a private review frame, and an "input unavailable on this platform" state.
- Packaging: Linux GNU builds load `libpipewire-0.3` on demand. musl static builds report `unsupported_build` for Linux native capture. Users need no compiler or downloaded helper.
- Docs: `docs/computer-use.md`, the acceptance record and the feature/README matrix gain separate Linux rows for implemented, fixture-tested, real-desktop-accepted and unsupported configurations.

## Non-goals

- Linux input in this change, including the input transport requirements, or any claim of cross-desktop parity with macOS.
- X11/Xorg backends, xdotool/XTest, `/dev/uinput`, root or group-permission instructions, compositor-private input injection, browser-only substitutes, clipboard bridging, headless arbitrary desktop control, unattended or restored grants, and installing or configuring portal backends.
- The D-Bus `Notify*` input transport, and automatic downgrade from EIS.
- Windows computer use (issue #44, PR #131), and the shared contract itself, which is #134.
- Release tagging or advertising Linux support before the real-desktop evidence in `tasks.md` exists.

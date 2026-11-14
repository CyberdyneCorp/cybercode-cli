## 0. Design gate (no code before this section is complete)

- [x] 0.1 Read issue #43, the macOS design and specs, the backend/controller/geometry/runtime/HTTP seams, user docs and the acceptance record. Collect official portal, PipeWire and libei documentation.
- [x] 0.2 Draft the proposal, the design (including threat model, dependency matrix and open decisions), spec deltas and this task list. Validate OpenSpec.
- [ ] 0.3 Owner review of proposal, design and tasks. Record answers to decisions D1–D10, the target-assurance decision (D1) and the initial desktop/version targets (D9) in `design.md`.
- [ ] 0.4 Agree the shared schema (D2: `Selector.pick`, `NativeTarget.source`, `Health` capability fields and refusal-code naming) with the Windows owners of #44. Record the agreement in both issues.

## 1. Shared foundation PR (backwards-compatible, platform-neutral)

- [ ] 1.1 Add `Selector.pick` with exactly-one validation. Add the `NativeTarget.source` and `Health` capability fields. Make `Error::UnsupportedPlatform` text platform-neutral and add a refusal-code carrier. Tests: serde round trips, old JSON still deserializes, validation matrix, schema snapshot.
- [ ] 1.2 Add Backend default methods `review`, `end_scope` and `set_revocation_sink`. Tests: the macOS and fake adapters behave unchanged, and the full existing `cyber-computer` and server computer suites pass.
- [ ] 1.3 Runtime changes:
  - Owner preview calls `review` on the blocking pool, bounded and cancellable by Stop through `preview_generation`.
  - The review deadline starts after the picker response.
  - `selector_for_identity` handles `pick:`.
  - The revocation sink calls `stop(session)`.
  - The input-readiness fact uses `input_control`.

  Tests (fake backend): Stop during a blocked review arms nothing, a late review reply is discarded, a revocation push fences an in-flight permit, and input on a capture-only backend writes no intent and dispatches nothing.
- [ ] 1.4 Regenerate OpenAPI and the TypeScript SDK. Add the CLI `--pick`, TUI/dashboard review card fields (source kind, size, review frame, output exposure warning) and the "input unavailable on this platform" state. Tests: SDK compatibility, CLI parse/refusal, TUI snapshot, dashboard module test. macOS behavior is unchanged.

## 2. Linux probing and health (no portal session)

- [ ] 2.1 Probe build kind, session type, session bus, portal frontend and ScreenCast/RemoteDesktop versions and properties, and whether `libpipewire-0.3.so.0` loads, all with bounded timeouts. Produce health and refusal codes. Tests: mock bus fixtures for every refusal code, desktop name never enabling capture, no `CreateSession` call during status or setup.
- [ ] 2.2 musl builds report `unsupported_build`, and Linux GNU builds without libpipewire start normally. CI: build and run status on both targets in a container without PipeWire.
- [ ] 2.3 Linux per-user host lease using `flock` on `$XDG_RUNTIME_DIR/cyber-computer.lock`, falling back to the private state directory when `XDG_RUNTIME_DIR` is unset. Tests: cross-process exclusion, lease held until worker exit.

## 3. Native worker process

- [ ] 3.1 Same-binary helper with a hidden subcommand, `PR_SET_DUMPABLE=0`, `PR_SET_PDEATHSIG`, socketpair IPC with bounded typed messages, and no children. Tests: request/response size limits, malformed message refusal, helper exits when the parent dies.
- [ ] 3.2 Close-on-exec discipline: set `FD_CLOEXEC` on every descriptor received from D-Bus or created natively. Tests: a sandboxed child and an unsandboxed tool child spawned during a grant hold none of the descriptors, and a sandboxed child cannot open `/proc/<helper>/fd/*`.
- [ ] 3.3 Stop and lease ordering: the generation fence runs first, then SIGKILL after a bounded grace period, then `waitpid`, then lease release. Tests: a hung native call is still stopped, and a second Session cannot arm until the helper has exited.

## 4. Portal capture (milestone A)

- [ ] 4.1 Hand-written zbus proxies (or the D5 alternative) for ScreenCast, Session and Request, with unguessable tokens. Option-dictionary assertions: no `persist_mode`, no `restore_token`, `multiple=false`, the requested `types`, and an advertised `cursor_mode`.
- [ ] 4.2 Review flow: CreateSession, SelectSources, Start, check exactly one stream of the requested kind, OpenPipeWireRemote, connect, capture the private review frame. Tests against a fake portal: user cancel, denial, timeout, zero or multiple streams, wrong kind, delayed response after Stop, response for another session handle.
- [ ] 4.3 Narrow `dlopen` PipeWire FFI connecting only through `pw_context_connect_fd`. Serial targeting on v6, node binding plus generation on v4/v5. Tests with a headless PipeWire test producer: serial path, node path, node removal and reuse leading to revocation, no default-socket connection (asserted by observing the daemon's client list).
- [ ] 4.4 Frame validation and conversion: shared-memory buffers only, format allow-list, stride/size checks, corrupted and transform flags, bounded downscale, PNG ≤ 8 MiB. Tests: unit tables, a fuzz target for the converter, DMA-BUF-only refusal.
- [ ] 4.5 Geometry: output bounds from stream position and size, window scope as observation-only, orientation mismatch refusal, generation advance on renegotiation. Tests: fractional scale, negative origin, rotated-dimension refusal, resize revocation.
- [ ] 4.6 Revocation sources: portal `Closed`, NameOwnerChanged/bus loss, stream error, logind lock/inactive. Tests with fake services for each code. Unlock and reconnect never restore a grant.
- [ ] 4.7 Linux input entry point returns `target_assurance_unavailable` at admission. Tests: zero journal intents, zero helper input messages, health `input_control: false`.

## 5. Input transport (milestone B, built only after D1 approves a provider)

- [ ] 5.1 Assurance provider seam with `Proven`/`Refused`/`Unknown` observations, generation binding, deny-only signals and per-event re-check for multi-event actions. Tests: every unknown or absent observation refuses, and a deny-only signal never proves safety.
- [ ] 5.2 RemoteDesktop v2 session, ConnectToEIS once, no Notify path, no downgrade. EIS negotiation with `reis`, events only to resumed devices, `mapping_id` region match, containment checks. Tests against a fake EIS: pause/removal/disconnect/region and keymap change revocation, unknown settlement, button and key release on cancel.
- [ ] 5.3 Keymap and text: keysym-to-keycode lookup through the negotiated keymap, refusal of hidden modifiers, EIS `TEXT` when offered, fail-closed for unreachable characters, no clipboard. Tests: layout tables, Unicode refusal and acceptance matrix.
- [ ] 5.4 Separate owner review of the provider and its real-desktop evidence before `input_control` can become true on any configuration.

## 6. Documentation

- [ ] 6.1 `docs/computer-use.md`: Linux prerequisites, the `--pick` flow, capture-only status, refusal codes with remedies, exposure warnings, the FD/ptrace boundary, no portal configuration edits.
- [ ] 6.2 `docs/measurements/native-computer-acceptance.md`: a Linux section that separates fixture results from real-desktop runs. README/features matrix rows per desktop and state.
- [ ] 6.3 Update the `computer-use` and `cli-commands` main specs on archive. Run `spec_inventory.py` and `spec_lint.py`.

## 7. CI evidence (what CI can and cannot prove)

- [ ] 7.1 All platforms: compile, strict Clippy with all targets, the existing macOS/Windows computer suites unchanged, schema/SDK checks, and non-Linux builds green with no Linux dependencies.
- [ ] 7.2 Linux CI job using a private `dbus-daemon --session`, the fake portal, a headless PipeWire daemon with a test producer and a fake EIS. It runs the real Linux adapter code paths for sections 2–5. A second job runs without PipeWire and without a portal to prove bounded refusals.
- [ ] 7.3 CI cannot prove portal dialogs, compositor capture behavior, scaling/rotation as rendered, lock-screen behavior, indicator-driven close, input delivery or target assurance. These results are never reported as desktop acceptance.

## 8. Real-desktop acceptance (owner-run, disposable sessions, synthetic content only)

- [ ] 8.1 For each D9 target (GNOME Wayland, KDE Plasma Wayland), record distribution, kernel, compositor, portal frontend and backend, PipeWire and libei versions, and the exact binary SHA-256.
- [ ] 8.2 Capture: window pick, output pick, consent cancel, review-frame confirmation, mixed-DPI or fractional output, negative-origin layout, rotated output (expect refusal until accepted), source resize/replacement, monitor hotplug, indicator stop, lock/unlock, portal restart, compositor restart where feasible, server crash, Stop during a blocked capture, presence loss.
- [ ] 8.3 Privacy: screenshots stay in private storage, absent from logs/exports/hooks/child environments. Descriptor inaccessibility from tool children is checked on the real host.
- [ ] 8.4 Negative matrix: Sway with xdg-desktop-portal-wlr (capture probe result, input refused), Xorg session (`x11_session_unsupported`), an XWayland window under window pick (observation only, input refused), SSH/headless service (`no_graphical_session`), musl build (`unsupported_build`). Confirm zero input events in every case.
- [ ] 8.5 Milestone B only, after 5.4: every promised input action (move, click variants, drag, scroll, keys, ASCII and Unicode text) in a disposable fixture, plus overlay, secure-field, polkit/system-dialog, lock-screen, Cyber TUI/dashboard and terminal refusal, and Stop mid-drag with release verification.

## Acceptance plan: evidence per requirement

| Requirement | Deterministic/unit | Mock integration (CI Linux) | Real desktop |
|---|---|---|---|
| Native computer capability and consent (modified) | 1.1, 1.3, 4.7 | 7.2 | 8.2, 8.4 |
| Platform-neutral computer health | 1.1, 2.1 | 2.1, 2.2 | 8.1, 8.4 (status on each) |
| Owner-picked capture source | 1.1, 1.3 | 4.1, 4.2 | 8.2 (picker, confirmation) |
| Linux portal capture | 4.4 | 4.3, 4.4 | 8.2 |
| Linux target assurance gate | 4.7, 5.1 | 4.7 | 8.4, and 8.5 if milestone B |
| Linux negotiated input transport | 5.2, 5.3 | 5.2 | 8.5 (milestone B only) |
| Linux portal lifecycle and revocation | 1.3 | 4.2, 4.6 | 8.2 (indicator, lock, restarts, crash, Stop) |
| Linux coordinate mapping | 4.5 | 4.3 | 8.2 (scales, origins, rotation, hotplug) |
| Native worker resource isolation | 3.1 | 3.2, 3.3 | 8.3 |
| Linux platform evidence | — | — | 6.2 reflects 8.x exactly |
| Computer use owner commands (modified) | 1.4 | 7.2 | 8.2 (CLI flow) |

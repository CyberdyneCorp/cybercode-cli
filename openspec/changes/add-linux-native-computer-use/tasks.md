## 0. Design gate (no code before this section is complete)

- [x] 0.1 Read issue #43, the macOS design and specs, the backend/controller/geometry/runtime/HTTP seams, user docs and the acceptance record. Collect official portal, PipeWire and libei documentation.
- [x] 0.2 Draft the proposal, the design (including threat model, dependency matrix and open decisions), spec deltas and this task list. Validate OpenSpec.
- [ ] 0.3 Owner review of proposal, design and tasks. Record answers to decisions D1–D11, the target-assurance decision (D1) and the initial desktop/version targets (D9) in `design.md`.
- [ ] 0.4 Agree the shared schema (D2: `Selector.pick`, `NativeTarget.source`, `Health` capability fields and refusal-code naming) with the Windows owners of #44. Record the agreement in both issues.

## 1. Shared foundation PR (backwards-compatible, platform-neutral)

- [ ] 1.1 Add `Selector.pick` with exactly-one validation. Add the `NativeTarget.source` and `Health` capability fields. Make `Error::UnsupportedPlatform` text platform-neutral and add a refusal-code carrier. Tests: serde round trips, old JSON still deserializes, validation matrix, schema snapshot.
- [ ] 1.2 Add Backend default methods `review`, `end_scope` and `set_revocation_sink`. Tests: the macOS and fake adapters behave unchanged, and the full existing `cyber-computer` and server computer suites pass.
- [ ] 1.3 Runtime changes:
  - Owner preview calls `review` on the blocking pool, bounded and cancellable by Stop through `preview_generation`.
  - The review deadline starts after the picker response.
  - `selector_for_identity` handles `pick:`.
  - The revocation sink calls `stop(session)`.
  - The admission fact is `screen_capture && (action_is_screenshot || input_control)`.
  - A pick-scope `allow` arms only the named `preview_id` before its deadline, never starts a review, and an unconfirmed pick review gets `end_scope` at its deadline.

  Tests (fake backend): Stop during a blocked review arms nothing, a late review reply is discarded, a revocation push fences an in-flight permit, input on a capture-only backend writes no intent and dispatches nothing, `allow` with an expired, superseded or other-Session `preview_id` arms nothing and opens no review, and an unconfirmed pick review is closed at its deadline.
- [ ] 1.4 Regenerate OpenAPI and the TypeScript SDK. Add TUI/dashboard review card fields (source kind, size, position, review frame, output exposure warning, "exactly one dialog should appear" notice) with a confirm action bound to the card's `preview_id`, and the "input unavailable on this platform" state. Tests: SDK compatibility, TUI snapshot, dashboard module test. macOS behavior is unchanged.
- [ ] 1.5 CLI pick flow (design §4.4.1): `--pick` requires a terminal on stdin and stdout; `--pick --yes` and non-terminal `--pick` fail with `interactive_review_required` before any request; the prompt arms the printed `preview_id` only; `--preview <id> --yes` confirms an existing review without a new preview; the CLI prints the dashboard route but no credential and never writes the frame to disk. Tests: CLI parse/refusal matrix, a fake server that records exactly one preview and one allow carrying the same `preview_id`, and unchanged macOS `--app`/`--display` behavior.

## 2. Linux probing and health (no portal session)

- [ ] 2.1 Probe build kind, session type, session bus, portal frontend and ScreenCast/RemoteDesktop versions and properties, and whether `libpipewire-0.3.so.0` loads, all with bounded timeouts. Produce health and refusal codes, with `screen_capture` equal to `capture`. Cache the probe for 5 s without a scope and answer from in-memory scope state while one is armed. Tests: mock bus fixtures for every refusal code, desktop name never enabling capture, no `CreateSession` call during status or setup, no D-Bus round trip during admission with an armed scope, and a probe timeout during a live scope not revoking the grant.
- [ ] 2.2 musl builds report `unsupported_build`, and Linux GNU builds without libpipewire start normally. CI: build and run status on both targets in a container without PipeWire.
- [ ] 2.3 Linux per-user host lease using `flock` on `$XDG_RUNTIME_DIR/cyber-computer.lock`, falling back to the private state directory when `XDG_RUNTIME_DIR` is unset. Tests: cross-process exclusion, lease held until worker exit.
- [ ] 2.4 Graphical session watch (design §9): choose the session from `XDG_SESSION_ID` or the logind user's `Display` session, require type `wayland`, `Active` and not locked at review and at `allow`, and subscribe to its property changes and `Lock` signal. Tests with a fake logind on a private system bus: server outside any session with a usable `Display` session, no identifiable session (`session_watch_unavailable`), subscription failure (refused), lock (`session_locked`), `Active=false` (`session_inactive`), session object removal or logind restart (`session_watch_unavailable`), and no restore on unlock or reactivation.
- [ ] 2.5 Sandbox session-bus masking (D11): when native computer use is enabled, Linux sandboxed tool and plugin children lose `DBUS_SESSION_BUS_ADDRESS` and the bus socket path is masked. Tests: plan assertions in `cyber-sandbox`, and on Linux CI a sandboxed child that fails to connect to the private bus. The feature-disabled plan is unchanged.

## 3. Native worker process

- [ ] 3.1 Same-binary helper with a hidden subcommand, `PR_SET_DUMPABLE=0`, `PR_SET_PDEATHSIG`, socketpair IPC with bounded typed messages, and no children. Spawn and reap each helper from a dedicated long-lived supervisor thread, never a blocking-pool thread. The helper sets the death signal at startup and exits if `getppid()` differs from the PID its parent passed. Tests: request/response size limits, malformed message refusal, helper exits when the parent dies, helper survives the exit of an unrelated server thread (for example an idle blocking-pool thread), and a helper whose parent died before `prctl` exits.
- [ ] 3.2 Close-on-exec discipline: set `FD_CLOEXEC` on every descriptor received from D-Bus or created natively. Tests: a sandboxed child and an unsandboxed tool child spawned during a grant hold none of the descriptors, and a sandboxed child cannot open `/proc/<helper>/fd/*`.
- [ ] 3.3 Stop and lease ordering: the generation fence runs first, then SIGKILL after a bounded grace period, then `waitpid`, then lease release. Tests: a hung native call is still stopped, and a second Session cannot arm until the helper has exited.

## 4. Portal capture (milestone A)

- [ ] 4.1 Hand-written zbus proxies (or the D5 alternative) for ScreenCast, Session and Request, with unguessable tokens. Option-dictionary assertions: no `persist_mode`, no `restore_token`, `multiple=false`, the requested `types`, and an advertised `cursor_mode`.
- [ ] 4.2 Review flow: pin the portal's unique bus name, CreateSession, SelectSources, Start, check exactly one stream of the requested kind (and position plus size for outputs), OpenPipeWireRemote (require an `AF_UNIX` stream socket), connect, capture the private review frame. Accept `Response`/`Closed` only from the pinned name on the exact handle. Tests against a fake portal: user cancel, denial, timeout, zero or multiple streams, wrong kind, output without position (`output_geometry_unavailable`), delayed response after Stop, response for another session handle, a `Response` and a `Closed` sent by a second bus client on Cyber's exact paths (ignored), a portal name owner change mid-scope (`session_bus_lost`, new owner unused), and a non-socket FD (refused).
- [ ] 4.3 Narrow `dlopen` PipeWire FFI connecting only through `pw_context_connect_fd`. Serial targeting on v6, node binding plus generation on v4/v5. Tests with a headless PipeWire test producer: serial path, node path, node removal and reuse leading to revocation, no default-socket connection (asserted by observing the daemon's client list).
- [ ] 4.4 Frame validation and conversion: shared-memory buffers only, format allow-list, stride/size checks, corrupted and transform flags, bounded downscale, PNG ≤ 8 MiB. Tests: unit tables, a fuzz target for the converter, DMA-BUF-only refusal.
- [ ] 4.5 Geometry: output bounds from stream position and size, window scope as observation-only, orientation mismatch refusal, generation advance on renegotiation, refusal of output streams missing position or size. Tests: fractional scale, negative origin, rotated-dimension refusal, resize revocation, missing position refusal.
- [ ] 4.6 Revocation sources: portal `Closed`, NameOwnerChanged/bus loss, stream error, and the session-watch events from 2.4. Tests with fake services for each code. Unlock, reactivation and reconnect never restore a grant.
- [ ] 4.7 Linux input entry point returns `target_assurance_unavailable` at admission. Tests: zero journal intents, zero helper input messages, health `input_control: false`.

## 5. Input (milestone B): not part of this change

Input transport, the assurance provider and their acceptance runs belong to a separate follow-up change (working name `add-linux-computer-input`, design §3 and §7, decision D1). This change is complete and can be archived once sections 0–4 and 6–8 are done. It adds no transport requirement to the main spec.

## 6. Documentation

- [ ] 6.1 `docs/computer-use.md`: Linux prerequisites, the `--pick` flow (terminal prompt, `--preview <id> --yes`, where the review frame is shown), capture-only status, refusal codes with remedies, exposure warnings, the dialog-confusion warning, the session-bus masking when the feature is enabled, the FD/ptrace boundary, no portal configuration edits.
- [ ] 6.2 `docs/measurements/native-computer-acceptance.md`: a Linux section that separates fixture results from real-desktop runs. README/features matrix rows per desktop and state.
- [ ] 6.3 Update the `computer-use` and `cli-commands` main specs on archive. Run `spec_inventory.py` and `spec_lint.py`.

## 7. CI evidence (what CI can and cannot prove)

- [ ] 7.1 All platforms: compile, strict Clippy with all targets, the existing macOS/Windows computer suites unchanged, schema/SDK checks, and non-Linux builds green with no Linux dependencies.
- [ ] 7.2 Linux CI job using a private `dbus-daemon --session`, a private system bus with a fake logind, the fake portal and a headless PipeWire daemon with a test producer. It runs the real Linux adapter code paths for sections 2–4. A second job runs without PipeWire and without a portal to prove bounded refusals.
- [ ] 7.3 CI cannot prove portal dialogs, compositor capture behavior, scaling/rotation as rendered, lock-screen behavior, indicator-driven close or the absence of input delivery on a real desktop. These results are never reported as desktop acceptance.

## 8. Real-desktop acceptance (owner-run, disposable sessions, synthetic content only)

- [ ] 8.1 For each D9 target (GNOME Wayland, KDE Plasma Wayland), record distribution, kernel, compositor, portal frontend and backend and PipeWire versions, and the exact binary SHA-256.
- [ ] 8.2 Capture: window pick, output pick, consent cancel, review-frame confirmation, mixed-DPI or fractional output, negative-origin layout, rotated output (expect refusal until accepted), source resize/replacement, monitor hotplug, indicator stop, lock/unlock, VT or user switch, server started as a systemd user service, CLI prompt and `--preview` confirmation, portal restart, compositor restart where feasible, server crash, Stop during a blocked capture, presence loss.
- [ ] 8.3 Privacy: screenshots stay in private storage, absent from logs/exports/hooks/child environments. Descriptor inaccessibility from tool children is checked on the real host.
- [ ] 8.4 Negative matrix: Sway with xdg-desktop-portal-wlr (capture probe result, input refused), Xorg session (`x11_session_unsupported`), an XWayland window under window pick (observation only, input refused), SSH/headless service (`no_graphical_session`), musl build (`unsupported_build`). Confirm zero input events in every case.

## Acceptance plan: evidence per requirement

| Requirement | Deterministic/unit | Mock integration (CI Linux) | Real desktop |
|---|---|---|---|
| Native computer capability and consent (modified) | 1.1, 1.3, 4.7 | 7.2 | 8.2, 8.4 |
| Platform-neutral computer health | 1.1, 2.1 | 2.1, 2.2 | 8.1, 8.4 (status on each) |
| Owner-picked capture source | 1.1, 1.3 | 4.1, 4.2 | 8.2 (picker, confirmation) |
| Linux portal capture | 4.4 | 4.3, 4.4 | 8.2 |
| Linux target assurance gate | 4.7 | 4.7 | 8.4 |
| Linux portal lifecycle and revocation | 1.3 | 2.4, 4.2, 4.6 | 8.2 (indicator, lock, session switch, restarts, crash, Stop) |
| Linux session-bus trust | 2.5 | 4.2, 2.5 | 8.3 |
| Linux coordinate mapping | 4.5 | 4.3 | 8.2 (scales, origins, rotation, hotplug) |
| Native worker resource isolation | 3.1 | 3.1, 3.2, 3.3 | 8.3 |
| Linux platform evidence | — | — | 6.2 reflects 8.x exactly |
| Computer use owner commands (modified) | 1.5 | 1.5, 7.2 | 8.2 (CLI prompt and `--preview` flow) |

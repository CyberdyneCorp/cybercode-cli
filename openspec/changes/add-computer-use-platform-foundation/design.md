# Design: platform-neutral computer-use foundation

Status: **decisions approved by the owner, 2026-11-15 (all recommended options).** Implemented on branch `design/computer-use-shared-schema` (PR #134). This change defines public types and shared policy only. It adds no platform mechanism, and it keeps macOS behavior as shipped except where a decision below says otherwise and is marked **owner review**.

## 1. Context

The shipped macOS contract (`openspec/specs/computer-use/spec.md`, change `add-native-computer-use`) and its code define these seams:

| Seam | Location | macOS-specific today |
|---|---|---|
| `Selector` | `crates/cyber-computer/src/backend.rs` | exactly one of `app` (bundle-ID charset) or `display` (index) |
| `Health` | same | `screen_capture` and `accessibility` are the two macOS TCC grants; `minimum_version` is "macOS 14" |
| `NativeTarget` | same | `pid`, `window`, `display`, `launch_ms` are macOS numbers; `identity` is `app:`/`display:` text |
| `Backend` trait | same | `acquire_lease`, `health`, `resolve`, `capture`, `input`; no review, teardown or asynchronous revocation hooks |
| `Error` | `crates/cyber-computer/src/lib.rs` | `UnsupportedPlatform` text names macOS 14; other variants carry free text |
| Chord rules | `crates/cyber-computer/native/macos.m` (`key`) | app scope refuses `control` and allows modifiers only with a fixed key list; display scope has no extra rule |
| Admission | `crates/cyber-server/src/runtime/computer/mod.rs` | `os_consent = screen_capture && (capture \|\| accessibility)` |
| Receipt / journal | same, `journal.rs` | `operation_id`, `action`, `outcome` (`captured`, `posted`, `cancelled`, `unknown`), `replayed`, `frame` |
| Lease | `macos.m` | `flock` on a per-UID file |

Two drafts extend this: Linux (PR #130, decisions D1–D11) and Windows (PR #131, D1–D12). Both say the shared types must change in one backwards-compatible step before either adapter (Linux D2, Windows D12 and §4). Their proposals overlap but differ in names and shape. This design picks one shape, defines the policy both need, and states which of their decisions move here (§14).

## 2. Goals and non-goals

Goals:
- One public schema that can describe macOS, Linux capture-only and Windows window control without fictional fields.
- One registry of reason codes.
- Shared rules for grants, lifecycle, chords, native workers and receipts, so platform designs only add mechanisms.
- No macOS regression. Old clients keep working.

Non-goals are listed in [proposal.md](proposal.md#non-goals).

## 3. Capability report (F1, F3)

`Health` keeps every existing field with its current macOS meaning and adds:

```text
schema_version: 2
mode: "unsupported" | "capture_only" | "input_enabled"        // derived, never set independently
backend: string                                                // "macos-screencapturekit", "xdg-portal", "windows-wgc", "unsupported"
capture: { available: bool, reasons: [code] }
input:   { available: bool, reasons: [code], assurance: "verified" | "unavailable" | "not_applicable" }
authority: { kind: "os_permission" | "platform_picker" | "owner_review_only", setup_available: bool }
session: { interactive: bool?, locked: bool?, remote: bool?, kind: string?, reasons: [code] }
os: { family: string, version: string?, build: string?, architecture: string }
selectors: ["app" | "display" | "window" | "pick"]
evidence: "unsupported" | "implemented" | "fixture_tested" | "accepted"
diagnostics: { interfaces: [{ name, version }], desktop: string? }   // never used for decisions
```

Rules:

- `mode` is `unsupported` when `capture.available` is false, `capture_only` when capture is available and input is not, and `input_enabled` when both are. A client renders the mode; it never computes it from legacy fields.
- **Admission** becomes `capture.available && (action_is_screenshot || input.available)`. Revocation on loss tracks `capture.available` and `input.available` the way it tracks `screen_capture` and `accessibility` today.
- `reasons` lists every code that applies, most actionable first. An available capability has an empty list. Clients treat an unknown code as "unavailable, see text" (forward compatibility).
- `assurance` says whether the platform can prove the pre-dispatch target checks (focus, hit target, secure field, protected surface, self-approval). `not_applicable` is used only when input is unavailable for an earlier reason, such as an unsupported host.
- `authority.kind` names what makes capture legitimate besides Cyber's owner review, which is **always** required:
  - `os_permission`: an OS grant to the binary (macOS Screen Recording and Accessibility). `setup` may request it.
  - `platform_picker`: the OS shows its own source chooser for each grant (Linux portal). `setup` shows nothing.
  - `owner_review_only`: the OS asks for nothing on the supported path (Windows programmatic window capture). Health says so and never invents a permission step.
- `session` fields are `null` when the platform cannot tell. `null` is never treated as "safe"; a platform that needs the fact to admit input refuses with `session_watch_unavailable`.
- Health stays read-only: without an owner `setup` request it captures nothing, sends nothing, opens no picker or OS UI, enumerates no window titles and never acquires or waits on the host lease. Probes are time-bounded and may be cached; a transient probe failure does not by itself revoke a live grant (a platform's own revocation events do that).
- Legacy fields on platforms other than macOS: `screen_capture` equals `capture.available`; `accessibility` equals `input.available`; `minimum_version` names the platform prerequisite in words. No legacy field reports an OS permission that does not exist.

## 4. Reason-code registry (F2)

All codes are stable snake_case strings in one registry (`ReasonCode` in `cyber-computer`, rendered as an OpenAPI string enum with `x-extensible` semantics and as a table in `docs/computer-use.md`). A code has one meaning on every platform. A platform may register a platform-only code, but never reuses a shared code with a different meaning.

| Category | Shared codes |
|---|---|
| Host | `feature_disabled`, `unsupported_platform`, `unsupported_os_version`, `unsupported_architecture`, `unsupported_build`, `evidence_pending` |
| Session | `no_graphical_session`, `non_interactive_session`, `remote_session`, `session_locked`, `session_inactive`, `session_watch_unavailable` |
| Authority | `os_permission_missing`, `capture_policy_denied`, `controller_privilege_unsupported`, `picker_cancelled`, `interactive_review_required` |
| Backend | `capture_unavailable`, `input_unavailable`, `target_assurance_unavailable`, `selector_unsupported_on_platform`, `action_unsupported_on_platform` |
| Lease | `host_lease_busy`, `host_lease_unavailable`, `previous_holder_abandoned` |
| Target | `target_gone`, `target_changed`, `target_protected`, `target_elevated`, `target_other_user`, `target_capture_excluded`, `target_not_foreground`, `secure_field` |
| Frame | `frame_stale`, `topology_changed` |
| Policy | `key_chord_refused`, `mode_forbids_input`, `supervisor_fence`, `limit_exceeded` |
| Revocation | `owner_stop`, `session_interrupt`, `presence_lost`, `grant_expired`, `os_permission_revoked`, `server_restart` |

Name reconciliation with the drafts:

| Linux draft | Windows draft | Registry |
|---|---|---|
| `selector_unsupported_on_platform` | `unsupported_selector` | `selector_unsupported_on_platform` |
| — | `unsupported_os_build` | `unsupported_os_version` (build goes in `os.build`) |
| — | `controller_elevated`, `controller_ui_access`, `controller_integrity_unsupported` | `controller_privilege_unsupported` shared; the three stay as Windows detail codes |
| `source_changed` | `target_gone` (identity) | `target_changed` (binding moved) vs `target_gone` (binding destroyed) |
| `portal_selection_cancelled` | — | `picker_cancelled` |
| `session_locked`, `session_inactive`, `session_watch_unavailable` | same | unchanged |
| — | `remote_session` | `remote_session` (any remote-desktop attachment, including RDP and VNC-style sharing) |
| — | `host_lease_unavailable` | `host_lease_busy` (held by someone else) and `host_lease_unavailable` (cannot be created) |

Platform-only codes from the drafts (for example `x11_session_unsupported`, `portal_unavailable`, `pipewire_library_missing`, `hdr_display_unsupported`, `target_cloaked`) are registered by their platform change and must sit in one of the categories above. A refusal response carries one primary code plus optional detail codes.

`Error::UnsupportedPlatform` becomes platform-neutral text and carries a code. Error class names (`ComputerUnsupportedPlatformError` and so on) do not change.

## 5. Selectors and target identity (F4)

```text
Selector = exactly one of
  app:     string            // macOS bundle ID; unchanged validation
  display: u32               // macOS display index; unchanged
  window:  string            // short-lived candidate token from an owner-only target listing (Windows)
  pick:    "window" | "output"  // the platform's own picker chooses the source (Linux portal)
```

- Health `selectors` lists what the platform honors. Any other selector is refused with `selector_unsupported_on_platform` before any native call. A selector value is never reinterpreted on another platform (a bundle ID is never an executable path or a Linux app ID).
- A candidate token is server-issued, opaque, bound to the owner listing that produced it and expires 30 s after issue. The token format is not part of the contract.
- `NativeTarget` adds:
  - `target_id`: server-issued, opaque, unique per review, meaningless after restart;
  - `source`: `{ platform, kind: "app" | "window" | "display" | "output", backend, detail: { string: string } }`. `detail` is for owner display and diagnostics; clients never parse it for decisions.
  - `topology_epoch`: see §7.
- Legacy numeric fields (`pid`, `window`, `display`, `launch_ms`, `app`) stay with their macOS meaning, are marked deprecated in OpenAPI and are zero/null elsewhere. A platform never puts a raw OS handle (HWND, PipeWire node, monitor handle) in them or anywhere a caller can send it back.
- `identity` stays a server-internal comparison string. Platforms own their prefix (`app:` and `display:` stay macOS; drafts propose `pick:` and `win:`). The runtime never builds a native target from a caller-supplied identity, handle, PID or path; it maps identity back only through server-held state.

## 6. Review-bound arming (F5)

Today `allow` re-runs the selector and arms the result. That is safe when the selector resolves deterministically (macOS `app`/`display`). It is not safe when resolution involves a human choice (picker) or a short-lived candidate.

- Each selector kind declares whether it is **re-resolvable**. `app` and `display` are; `pick` and `window` are not.
- For a non-re-resolvable selector, `allow` SHALL name the `preview_id` of a review whose target the owner was shown, before that review's deadline. `allow` never starts a new review, never opens a picker, and re-checks that the live target still matches the reviewed one.
- `cyber computer allow <session> --preview <id> --yes` confirms an existing pending review. It works on every platform, macOS included, and is additive there.
- Combining a non-re-resolvable selector with `--yes` is refused with `interactive_review_required`. macOS `--app`/`--display` with `--yes` keep their current behavior.
- A review that is not confirmed by its deadline, or is superseded, has its platform scope torn down (`end_scope`, §10.2).

## 7. Grant lifecycle and host lease (F6)

Unchanged and now stated as platform-neutral: owner-only arming through the authenticated owner API; one Session and one reviewed target per grant; 300 s default, 900 s cap; attached human presence; Plan mode forbids input; saved or bypass rules never arm or widen; no inheritance to child, hub, peer or relay agents; no grant survives restart; limits of 10 inputs/s, 300 per grant, 5 s actions and waits, 16 KiB text; 10 s frame age.

New shared rules:

- **Event classes.** Every platform event that affects a grant is one of:
  - *binding loss*: the reviewed target, its session, its desktop or the platform authority is gone or changed (window destroyed, process restarted, portal session closed, lock, logoff, desktop switch, remote attach, watch lost, OS permission revoked). Effect: revoke through the controller generation fence first, then tear down. Never restored by unlock, reconnect or restart; a fresh review is required.
  - *topology change*: the display layout changed but the binding is intact (monitor added, DPI or arrangement change). Effect: increment a process-wide `topology_epoch`; every frame with an older epoch is stale (`frame_stale`) and the next coordinate action needs a new screenshot. The grant stays armed.
  - A platform that cannot tell the two apart treats the event as binding loss. (The Linux draft classifies stream renegotiation as binding loss, the Windows draft classifies display changes as topology change; both fit.)
- **Remote sessions.** On platforms added after macOS, a session attached to a remote-desktop or screen-sharing transport is refused (`remote_session`) unless that platform's change accepts it with evidence. macOS does not detect this today and stays unchanged (owner decision F6b).
- **Host lease.** At most one native worker per OS user per interactive session holds input authority, across every Cyber server home. The lease object is owner-only and non-inheritable. A cancelled worker keeps the lease until its native call returns (§10). Health reports lease state only from in-process knowledge and never acquires or waits on the lease. macOS's per-UID lock satisfies this unchanged.
- **Abandoned lease.** When a platform can detect that the previous holder died while holding the lease, the new holder records `previous_holder_abandoned`, treats input cleanup on the desktop as uncertain and shows that warning in the owner's review before its grant can act. It never reads or settles another home's journal; each home settles its own open intents as unknown at startup.

## 8. Schema versioning (F7)

- An integer `schema_version` appears on `Health`, `Preview`, `FrameInfo`, `Receipt` and the tool's `status` output. Version 1 is the shape shipped for macOS. This change defines version 2.
- Within a version, changes are additive only: new optional fields, new selector variants, new reason codes. A field is removed or changes meaning only in a new version, after it has been deprecated for at least one tagged release.
- The server always emits the newest version. Version 1 fields stay present and correct in version 2 output, so a version 1 client keeps working by ignoring what it does not know.
- Requests keep `deny_unknown_fields`. A request never carries `schema_version`; the server accepts every request shape it has ever accepted within the current major.
- The journal stores the receipt's `schema_version`. Recovery and replay read version 1 rows as they were written and never rewrite them.
- **One action schema.** The model-facing `computer` tool offers the same action set on every platform. An action the platform cannot perform (all input on Linux capture-only, for example) is refused with `action_unsupported_on_platform` or the specific reason from `input.reasons`, before controller admission and before any journal intent, with zero native side effects. The tool description and `status` state the mode so the model can plan.

## 9. Shared key-chord policy (F8)

The chord policy moves from the macOS native shim into one platform-neutral evaluator in `cyber-computer`, run during action validation, before admission and before any journal intent. The native macOS check stays as defence in depth.

**Logical modifiers.** The public `Modifier` enum (`shift`, `control`, `alt`, `command`) is unchanged. Each platform assigns roles:

| Role | macOS | Windows | Linux |
|---|---|---|---|
| Primary editing modifier | `command` | `control` | `control` |
| System modifier (shell-global) | `control` | `command` (Windows key) | `command` (Super) |
| `alt` under app/window scope | allowed with edit keys | refused (menus, Alt+Tab, Alt+F4) | refused |

**Edit keys.** `a`, `z`, `y`, `s`, `left`, `right`, `up`, `down`, `home`, `end`, `backspace`, `delete`. This is exactly today's macOS list.

**Rules,** evaluated in order; the first refusal wins and returns `key_chord_refused`:

1. The existing chord grammar (at most four distinct modifiers; one ASCII letter/digit or a named key).
2. **Global deny list**, for platforms added after macOS, in every scope: secure-attention and session chords (`control+alt+delete`, `control+shift+escape`, `control+escape`, lock chords), task and window switching (`alt+tab`, `command+tab`), launcher and search (`command+space`), application quit and force-quit chords. Any chord that uses the platform's system modifier is also refused.
3. **App/window scope:** a chord with the system modifier is refused; a chord with any modifier is allowed only when the key is an edit key; on Windows and Linux a chord with `alt` is refused. Clipboard chords (`c`, `x`, `v` with any modifier) are therefore refused on every platform, because they are not edit keys.
4. **Display/output scope:** on macOS, unchanged (no rule beyond the grammar). On platforms added after macOS, rule 2 applies.
5. Unmodified named keys and characters are allowed in every scope.

**Layout safety** (shared): a platform resolves letters through the target's current keyboard layout and refuses a chord when producing the character needs a modifier the chord did not name. macOS already does this; Windows and Linux drafts propose the same.

**macOS parity.** For app scope, rules 1, 3 and 5 reproduce the native shim: `control` refused, any modifier only with an edit key. For display scope, rule 4 reproduces "no extra rule". A table-driven test runs every grammar-valid chord through both the Rust evaluator and the native rule set and requires identical decisions.

**One observable macOS change (F8c, approved 2026-11-15).** Today a chord the shim refuses surfaces as a native error after the intent is journaled: the runtime stops the grant and settles the action `unknown`. Under this design the same chord is refused before the intent, with `key_chord_refused`, no journal row and the grant left armed. No chord that macOS allows today becomes refused, and no chord it refuses becomes allowed.

## 10. Native worker boundary (F9, F11)

### 10.1 Contract

Whatever executes native calls (an in-process thread pool or a helper process) is the **native worker**. The contract is the same either way:

- The worker owns every native handle and descriptor (portal and PipeWire FDs, D3D devices, UIA elements, capture sessions). Nothing else in the server holds them.
- Every handle is non-inheritable from the moment Cyber receives it. The worker never spawns children.
- Stop advances the controller generation first and never waits for the worker. The host lease is released only after the worker has provably stopped dispatching: the native call returned, or a helper process has been reaped.
- Native calls have deadlines no longer than the remaining action budget.
- Late callbacks capture only owned channels and a generation number; a stale generation drops its result.
- Pressed keys and buttons are tracked and released on cancel, timeout, failure or unwinding; the receipt records `cleanup: "uncertain"` when release cannot be confirmed, and never claims delivered input was undone.
- Pure policy (selector validation, reason mapping, chord policy, geometry, target-binding comparison) stays in safe, OS-independent Rust and is unit-tested on every OS.

### 10.2 Helper processes

A platform may run the worker in a helper process (the Linux draft recommends one; the Windows draft recommends none). A helper:

- is the same `cyber` binary re-executed with a hidden internal subcommand; nothing is downloaded or compiled;
- runs as the same user with no added privilege (no elevation, `uiAccess`, setuid, capabilities or service context);
- talks to the server only over a channel the server created and passed at spawn (socketpair or inherited pipe handle). It never listens on a name, port or path, and a name, loopback address or same-SID check is never treated as owner proof;
- carries only typed, size-bounded messages: `health`, `review`, `capture`, `input` (an already-admitted action), `end_scope`. It never receives owner credentials, configuration secrets or model text other than an admitted action;
- dies when the server dies and can be force-terminated by Stop;
- is hardened against same-user inspection where the OS allows it (for example non-dumpable on Linux), and the platform documents what remains reachable by full-access owner shells.

### 10.3 Backend port additions

Additive default methods keep the macOS adapter source-compatible:

```text
review(selector, cancel) -> NativeTarget   // default: resolve(selector); owner preview only; may show a picker
end_scope(target)                          // default: no-op; idempotent, non-blocking teardown
set_revocation_sink(sink)                  // default: no-op; the backend reports binding-loss events with a reason code
```

`resolve` never shows UI. The runtime wires the revocation sink to `Computer::stop` for the bound grant.

### 10.4 Sandboxed processes

On every platform, sandboxed tool and plugin processes receive no capture or input authority: no native handles, no capability that grants screen capture or input, and no access to private capture storage. Platforms that need extra masking (Linux session bus, Windows AppContainer capability list) specify the mechanism in their own change.

## 11. Receipts, audit and evidence (F10, F12)

### 11.1 Receipts

`Receipt` keeps `operation_id`, `action`, `outcome`, `replayed` and `frame`, and the existing outcome values (`captured`, `posted`, `cancelled`, `unknown`). It adds:

```text
schema_version, platform, backend
grant_id, target_id, frame_id?
reason?: code                         // why it was refused, failed or became unknown
events: { requested: u32, posted: u32 }?   // input only; counts, never contents
cleanup?: "not_needed" | "released" | "uncertain"
checks?: [name]                       // assurance checks that ran and passed before the last event, e.g. "foreground", "hit_target", "secure_field"
warnings?: [code]                     // e.g. previous_holder_abandoned
started_ms, settled_ms
```

Durable journal rows and exports keep today's redaction: no raw typed text (length and digest only), no key characters (category and digest), no window titles, no clipboard content and no screenshot bytes. `source.detail` values that could name documents are owner-display only and are not written to the journal.

A refusal decided before admission (policy, mode, unsupported action, chord) produces an error response with a reason code and no journal row. Only admitted actions get an intent and a settlement.

### 11.2 Support evidence

Each platform configuration (OS version or desktop, architecture, session kind) has an evidence level:

| Level | Meaning |
|---|---|
| `unsupported` | refused with a reason code |
| `implemented` | code exists; no fixture run recorded |
| `fixture_tested` | passed mock or fixture tests in CI |
| `accepted` | passed a recorded run on a real interactive desktop with a disposable fixture |

The level comes from a table compiled into the binary and mirrored in `docs/computer-use.md`. On platforms added after macOS, `input_enabled` mode is possible only for `accepted` configurations; capture may run at `fixture_tested` or `implemented` with the level shown in health and on the review card (F12, approved 2026-11-15). README and feature matrices claim only `accepted` configurations. macOS reports `accepted` only once its own pending acceptance task (`add-native-computer-use` 1.4 and acceptance) is recorded; until then it reports `implemented` and behaves as today.

## 12. macOS mapping

| Contract element | macOS value | Behavior change |
|---|---|---|
| `mode` | `input_enabled` when both TCC grants exist; `capture_only` with Screen Recording only; `unsupported` below 14 | none (derived) |
| `capture.available` / `input.available` | `screen_capture` / `screen_capture && accessibility` | none |
| reasons | `unsupported_os_version`, `os_permission_missing`, `feature_disabled` | new codes only |
| `authority` | `os_permission`, `setup_available: true` | none |
| `session` | all `null` (not detected today) | none |
| `selectors` | `["app", "display"]` | none; `window`/`pick` refused with `selector_unsupported_on_platform` |
| `target_id`, `source` | new; `source.kind` `app` or `display` | additive |
| legacy target fields | unchanged | deprecated in OpenAPI only |
| `identity` | `app:…` / `display:…` unchanged | none |
| review-bound arming | `app`/`display` re-resolvable; `--preview <id> --yes` additionally available | additive |
| event classes | existing revocations are binding loss; no topology events emitted | none |
| host lease | per-UID `flock` | none |
| chord policy | app scope: `control` refused, modifiers only with edit keys; display scope: grammar only | refusal moves before intent (F8c) |
| worker | in-process blocking pool | none |
| receipts | new fields added; outcome values unchanged | additive |
| evidence | `implemented` until macOS acceptance is recorded | reporting only |

Version 1 clients (current CLI, TUI, dashboard, SDK) see the same fields and values.

## 13. Decisions

Each decision lists the recommended option first. **Owner review** marks decisions that change a public contract; on 2026-11-15 the owner approved the recommended option for every one of them (F1, F4, F6b, F7, F8a–c, F9, F12).

- **F1 – Capability report shape. Approved 2026-11-15.** *Recommended:* additive nested `capture`/`input` objects with `reasons[]`, derived `mode`, legacy fields kept. *Alternatives:* flat booleans plus a top-level `refusals[]` (Linux draft) – loses which capability a reason applies to; a separate `ComputerHealth2` type – forces every client to switch.
- **F2 – Reason codes.** *Recommended:* one registry, one meaning per code, platform-only codes allowed in fixed categories, unknown codes treated as unavailable. *Alternatives:* free-text errors (today) – not machine-actionable; per-platform enums – clients must special-case each OS.
- **F3 – Authority kinds.** *Recommended:* `os_permission`, `platform_picker`, `owner_review_only`, with Cyber owner review always required. *Alternative:* keep reporting `accessibility`/`screen_capture` only – reports fictional permissions on Linux and Windows.
- **F4 – Selectors, identity and deprecation window. Approved 2026-11-15.** *Recommended:* `app | display | window | pick` with per-platform support, opaque `target_id`, typed `source`, legacy numeric fields deprecated for at least one tagged release and removed only in schema version 3. *Alternatives:* reinterpret `app` per platform – ambiguous and unsafe; remove legacy fields now – breaks current clients.
- **F5 – Review-bound arming.** *Recommended:* non-re-resolvable selectors arm only by `preview_id`; `--preview <id> --yes` on every platform. *Alternative:* let `allow` start a second review – arms a source nobody saw.
- **F6 – Lifecycle classes and lease.** *Recommended:* binding loss revokes, topology change only stales frames, one lease per OS user and interactive session, abandoned-lease warning where detectable. *Alternative:* revoke on every display change – simpler, but drops Windows grants on harmless monitor changes.
  - **F6b – macOS remote/lock detection. Approved 2026-11-15.** *Recommended:* leave macOS unchanged in this change; track adding lock and screen-sharing revocation as a separate macOS hardening change. *Alternative:* add it here – changes shipped macOS behavior.
- **F7 – Schema versioning and action availability. Approved 2026-11-15.** *Recommended:* integer `schema_version`, additive within a version, one action set on every platform with unsupported actions refused by code before intent. *Alternatives:* hide input actions from the tool schema on capture-only hosts – fewer wasted calls, but the tool schema then varies per host and per moment, which breaks prompt caching and confuses replay; OpenAPI-path versioning – heavier than needed for additive changes.
- **F8 – Shared chord policy. Approved 2026-11-15** on all three points:
  - (a) *Recommended:* the shared edit-key list is today's macOS list, including `s` (save) and vertical arrows. *Alternative:* the Windows draft's narrower list without `s` and vertical arrows – would change macOS or need per-platform lists.
  - (b) *Recommended:* the global deny list applies to display/output scope on new platforms only; macOS display scope stays unrestricted until a separate tightening change. *Alternative:* apply it to macOS display scope now – safer, but changes shipped behavior.
  - (c) *Recommended:* refuse chords before the intent is journaled, leaving the grant armed. *Alternative:* keep macOS's current post-intent `unknown` and grant stop for exact parity – keeps a misleading `unknown` for input that was never sent.
- **F9 – Native worker topology. Approved 2026-11-15.** *Recommended:* one worker contract, topology chosen per platform (Linux helper, Windows in-process, macOS unchanged). *Alternative:* require the same topology everywhere – either forces a helper on Windows, where the draft argues it adds an IPC authentication problem, or drops Linux's force-kill guarantee.
- **F10 – Receipt fields.** *Recommended:* add references, reason, event counts, cleanup state, checks and warnings; keep outcome values. *Alternative:* a new receipt type – duplicate journal handling.
- **F11 – Backend port additions.** *Recommended:* `review`, `end_scope`, `set_revocation_sink` as default methods. *Alternative:* a second trait for picker platforms – splits the runtime's code paths.
- **F12 – Evidence gate. Approved 2026-11-15.** *Recommended:* compiled evidence table; input on new platforms only for `accepted` configurations; capture allowed below `accepted` with the level shown. *Alternatives:* documentation-only gating (Linux draft) – runtime can enable input on an untested desktop; gate capture too – blocks fixture work on real desktops.

## 14. What moves from the platform drafts

| Draft decision or section | Moves here as | Stays in the draft |
|---|---|---|
| Linux D2 (shared schema shape) | F1, F4, F7 | — |
| Linux §4.1 (Backend default methods) | F11 | Linux use of `review` with the portal |
| Linux §4.3 (health fields, admission formula) | F1 | Linux probe order and Linux codes |
| Linux §4.4.1 (confirmation names a preview) | F5 | portal dialog flow |
| Linux D3 (helper) – contract part | F9, §10.2 | choosing a helper and its Linux hardening |
| Linux §6.2 code naming | F2 | Linux-only codes |
| Linux "Platform-neutral computer health" requirement | F1 requirement | drop the duplicate |
| Linux "Native worker resource isolation" requirement – generic part | worker boundary requirement | Linux non-dumpable and PDEATHSIG specifics |
| Linux "Linux platform evidence" – generic part | F12 | Linux evidence rows |
| Windows D12 and §4 items 1–5 | F1, F2, F4, F7 | — |
| Windows D8 (chords) | F8 | none; Windows adopts the shared table |
| Windows D3 / §5.10 (helper rules) | F9, §10.2 | choosing in-process |
| Windows D5 (RDP) | `remote_session` and F6 | Windows detection mechanism |
| Windows §5.6 topology epoch | F6 | Windows event sources |
| Windows §5.8 abandoned lease warning | F6 | named mutex mechanism |
| Windows "Platform-neutral computer health", "Pointer-safe native target identity" requirements | F1, F4 requirements | drop duplicates; keep the 30 s candidate rule as a reference |
| Windows "Windows support evidence gate" – generic part | F12 | Windows matrix |
| Windows sandbox delta | §10.4 sandbox requirement | AppContainer capability specifics |

Note for the Windows draft: its §5.2 says refusing Ctrl+C/X/V is stricter than macOS. macOS already refuses Command+C/X/V under app grants, because they are not in its edit-key list; under the shared table the platforms match.

## 15. Verification

- Unit tests on every OS: selector validation per platform, reason-code serialization and unknown-code handling, `mode` derivation, admission formula, schema-version round trips, version 1 journal rows still replaying, chord evaluator against the full grammar, and the macOS parity table.
- Runtime tests with the fake backend: non-re-resolvable selectors refuse `allow` without a `preview_id`; `--yes` with them is refused; refusal before intent writes no journal row and sends no events; topology change stales frames without revoking; binding loss revokes before teardown; health never acquires the lease.
- OpenAPI/SDK snapshot diff shows only additive changes; existing CLI/TUI/dashboard tests pass unchanged.
- macOS: the existing native test suite and, when the owner runs it, the macOS acceptance fixture show identical behavior apart from F8c.

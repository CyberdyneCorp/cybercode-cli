# Native computer use (experimental)

Cyber can observe and operate a native application through the `computer` model tool on macOS 14+. The capability is off by default. Linux, Windows and older macOS return explicit unsupported results; there is no shell, AppleScript, clipboard or external helper fallback. Native ScreenCaptureKit/CoreGraphics code is compiled into the server binary, so installed users do not need Xcode, Node or a runtime compiler.

The implementation has deterministic fake-adapter tests and native compile checks. Controlled live testing verified application-window capture, local vision delivery, mouse/Unicode keyboard input, target refusal and Stop during a drag on macOS26.7. The [acceptance record and controlled-device plan](measurements/native-computer-acceptance.md) separates actual device results from fixtures and remaining checks. Other display/keyboard layouts and signed distribution remain unaccepted.

## Enable and inspect

Set the feature in your global user configuration, then restart the server:

```sh
cyber features enable computer_use
cyber service stop
cyber computer status
```

`status` reports the platform-neutral capability report (schema version 2) next to the macOS fields: `mode` (`unsupported`, `capture_only` or `input_enabled`, derived from `capture.available` and `input.available`), the reason codes for each capability, the input assurance state, the authority kind (`os_permission` on macOS), the selectors the platform honors (`app` and `display` on macOS), OS facts and the support evidence level. The legacy `screen_capture`, `accessibility`, `supported` and `minimum_version` fields keep their meaning. See [Capability report and reason codes](#capability-report-and-reason-codes).

`features.computer_use` and the `computer` section are accepted only in trusted top-level user/managed global configuration. Project files, profiles and command-line configuration overlays cannot enable or configure native control. `--features computer_use` does not enable native control on a separate background server. Organization policy can lock it off.

```json
{
  "features": { "computer_use": true },
  "computer": { "retention_days": 1 }
}
```

`retention_days` accepts 0–365; 0 disables time-based screenshot expiry. Deleting a Session immediately expires its private captures and attempts removal regardless of retention; failed removal remains unavailable, accounted and queued for cleanup. A durable deletion fence prevents late capture workers from recreating its pixels. Native outcome metadata stays available for unknown-action reconciliation. The default is one day. Private screenshot storage is capped at 256 MiB; individual PNGs are at most 8 MiB with a maximum 1920-pixel edge. Expired images become unavailable immediately on read; a periodic worker removes expired files. Operation receipts retain metadata after image expiry.

## Grant OS consent deliberately

```sh
cyber computer setup --yes
```

This explicitly requests macOS Screen Recording and Accessibility consent for the **running server binary**, whose exact path is returned by `status`. Read-only status never requests consent. Follow the operating system's prompts; granting one permission does not grant the other. Restart the server after changing permissions if macOS requires it. If you move, replace or change the signing identity of the executable, macOS may require consent again. Development builds require a macOS SDK that contains ScreenCaptureKit's macOS 14 screenshot APIs; releases must compile this adapter during packaging and should keep a stable signed binary identity. Notarized release identity and real-device acceptance remain release checks.

## Arm one attached Session

Keep the intended Session open in the TUI or dashboard, open a disposable application window, and use its exact Session ID. Owner review can resolve the application in the background; bring it to the front before requesting input:

```sh
# Prints the proposed target without arming it.
cyber computer allow ses_EXACT_ID --app com.apple.TextEdit

# Resolve/review the current on-screen window again and explicitly arm this scope.
cyber computer allow ses_EXACT_ID --app com.apple.TextEdit --ttl 300 --yes

cyber computer status ses_EXACT_ID
cyber computer stop ses_EXACT_ID
# Emergency host-wide revocation, including another Session's grant:
cyber computer stop
```

`--preview <id> --yes` confirms a review you already printed, by its identifier, without resolving the target a second time:

```sh
cyber computer allow ses_EXACT_ID --app com.apple.TextEdit          # prints data.id
cyber computer allow ses_EXACT_ID --preview cup_PRINTED_ID --yes
```

It fails if the review expired (30 seconds), was superseded by Stop or belongs to another Session. Platforms that let the owner pick a source (`--pick window|output`) or choose from a listed candidate (`--window <token>`) require this two-step flow: their selectors cannot be resolved again, so `--yes` without `--preview` is refused with `interactive_review_required`. macOS honors only `--app` and `--display` and refuses the others with `selector_unsupported_on_platform` before any native call.

An application grant pins its bundle ID, PID, launch time and window. `--display N` explicitly grants a whole native display; `--display 0` selects the main display. Whole-display capture can reveal other visible applications, so application scope is preferable when it meets the task. Grants expire after 300 seconds by default and accept at most 900 seconds. They never survive server restart or inherit to child Sessions, friends' agents or hub members. Only one native worker and one live host grant are admitted at a time. A per-user native lock coordinates separate server/embedded processes, and remains held until a cancelled worker actually exits. The empty `/private/tmp/cyber-computer-<uid>.lock` file remains after release; do not unlink a lock held by a running server.

The dashboard includes Native computer use controls for health/setup, exact target review, explicit arming, private capture preview and global Stop. Expand the native controls above the workspace to review a target; a controlling Session remains visible in the global banner even when no Session is selected. Logout sends an independent Stop before clearing owner credentials. The TUI shows an active-control banner; **Ctrl+Alt+Esc**, while the TUI has keyboard focus, revokes native input independently of the current overlay or model/tool request. It is not an operating-system global hotkey. `Esc` interruption also revokes the interrupted Session's grant. Losing the last attached Session's live human presence permanently revokes that grant; reconnecting requires a fresh target review and owner grant. The periodic browser presence lease can take up to its 45-second expiry after an ungraceful disconnect. Stop cannot undo already delivered clicks or text.

## Let the model act

Choose a model with image input for screenshots. The `computer` tool offers `status`, `screenshot`, `move`, `click`, `drag`, `scroll`, `type`, `key` and bounded `wait`. The human grant is separate from the normal `computer.capture` / `computer.control` permission decisions. Plan mode forbids input; saved allow rules, auto decisions and bypass mode cannot arm or widen a native grant.

Input requires the server-issued frame ID from a screenshot taken within 10 seconds, unchanged target/focus/geometry, in-image coordinates, live owner grant/presence and current OS consent. Limits are 10 input actions per second, 300 per grant, 5 seconds per action and 16 KiB UTF-8 per typed string. Capture coordinates are relative to the returned image; the native adapter maps these into desktop logical points, including negative monitor origins. Rotation and geometry changes invalidate previous frames.

Named keys are enter, escape, tab, space, backspace, delete, arrows, home/end and page_up/page_down; ASCII letters/digits resolve through the current native keyboard layout. Characters that the layout cannot resolve without hidden modifiers fail closed; use Unicode `type` for text. Key chords follow the [shared chord policy](#key-chord-policy): application scope rejects Control shortcuts and shortcuts outside the limited editing allowlist to avoid switching applications or invoking host-wide actions. A refused chord is rejected with `key_chord_refused` before anything is journaled or sent, and the grant stays armed; the native adapter keeps the same check as a second line. Secure text fields and protected system/security application targets are denied. These checks reduce risk; they do not turn native application input into a filesystem sandbox.

Native events are fenced while human permission/question requests are pending. Stop native control before approving another tool, answering a question or changing the Session model, agent, Mode or administrative authority; scoped computer capture/control approvals remain possible under the existing finite owner grant. Before pointer events, public Accessibility hit testing checks the actual window under the pointer. Application scope requires the reviewed PID, focused-window identity and geometry; covering windows from another app or another window are refused. Unidentifiable targets, protected system/security apps and known Cyber windows are refused even with whole-display scope. Both focused keyboard elements and pointer-hit elements are checked through a bounded Accessibility ancestor chain. Secure text fields, system/ordinary dialogs and sheets, modal windows and unidentified chains are unsupported and refused. These OS observations and event dispatch are separate calls; controlled fixture overlays/dialogs and Stop during input have passed; these separate OS calls do not provide atomic protection against arbitrary desktop races. These checks prevent direct self-approval through the normal control surfaces; native applications still have host authority, and this is not a general isolation boundary against scripts, developer tools or host applications that can execute commands.

Screen text is untrusted external content. It cannot authorize wider scope, renew a grant or approve another tool. Do not authorize credential entry, password fields, operating-system permission dialogs or an application with private information visible unless you intend that content to be supplied to the selected provider.

## Evidence, privacy and uncertain outcomes

Each native operation records an intent before dispatch and a settlement afterwards. Reusing the same operation identity returns its prior receipt; conflicting payloads are refused. A timeout, failed input or missing settlement is **unknown**, not permission to retry. The grant stops after uncertain input. Inspect the target on the host, obtain another target review and use `--acknowledge-unknown --yes` only after reconciling the listed operations. Never automatically retype or reclick an unknown operation.

Typed strings remain ephemeral to dispatch; durable tool arguments, audit rows and hook inputs contain a byte length and SHA-256 digest instead of raw typed content. Pre-tool hooks cannot rewrite native input. Screenshots are private owner artifacts outside project attachment/export storage, with owner-authenticated Session-bound reads and restrictive file permissions. They are not automatically included in ordinary exports or collaboration messages. A vision-capable selected model receives authorized screenshots in its ephemeral request context; visible information can therefore leave the host for that provider. Raw screenshot files are not encrypted at rest by this feature; use the host's disk protection where required.

## Capability report and reason codes

Health, previews, frames, receipts and tool status carry an integer `schema_version`. Version 1 is the macOS-only shape; this server emits version 2, which adds fields and never removes or changes a version 1 field. A field is removed only in a later version after at least one tagged release of deprecation; the numeric target fields `pid`, `window`, `display`, `launch_ms` and `app` are deprecated in the OpenAPI document and keep their macOS values. Requests carry no version. The journal keeps each receipt's version in its stored JSON: rows written before version 2 replay exactly as written and report `schema_version: 1`.

The model tool offers the same actions on every host. An action the host cannot perform in its current mode is refused with the input reason code before admission and before any journal row, and nothing is sent. Refusals over HTTP keep their error class (`_tag`) and add a `reason` field.

Every refusal, unavailability, revocation and receipt reason uses one code from this registry. A code means the same on every platform; a platform change may add platform-only codes in these categories but never reuses a shared one. Clients treat an unknown code as "unavailable" and show the server's text.

| Category | Codes |
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

On macOS the reasons are `unsupported_os_version` below macOS 14, `os_permission_missing` while Screen Recording or Accessibility is missing, and `feature_disabled` when `computer_use` is off.

Receipts add `platform`, `backend`, `grant_id`, `target_id`, `frame_id`, `reason` (why an action was refused, failed or became unknown), input event counts when an adapter reports them, `cleanup` (`not_needed`, `released` or `uncertain`), assurance `checks`, `warnings` and start/settle times. They never contain typed text, key characters, window titles, clipboard content or screenshot bytes; a target's owner-display `source.detail` is shown in the review but never journaled. The TUI and dashboard show a receipt's reason and cleanup state next to the tool result.

A display-topology change that leaves the reviewed binding intact makes older frames stale (`frame_stale`) without revoking the grant; take a new screenshot. Losing the binding (target destroyed, session locked, permission revoked) revokes the grant first and then tears down the platform scope. macOS reports no topology events and keeps its existing revocations.

### Key chord policy

One platform-neutral policy checks chords before admission. Each platform assigns a primary editing modifier and a system modifier.

| Role | macOS | Windows | Linux |
|---|---|---|---|
| Primary editing modifier | `command` | `control` | `control` |
| System modifier (shell-global) | `control` | `command` (Windows key) | `command` (Super) |
| `alt` under app/window scope | allowed with edit keys | refused | refused |

Edit keys: `a`, `z`, `y`, `s`, `left`, `right`, `up`, `down`, `home`, `end`, `backspace`, `delete`.

| Scope | macOS | Platforms added after macOS |
|---|---|---|
| App or window | system modifier refused; any modifier only with an edit key; unmodified keys allowed | same, and `alt` refused; plus the global list below |
| Display or output | grammar only (unchanged) | global list below |

The global list refuses chords that use the system modifier and the secure-attention, session, task-switching, launcher and quit chords (`control+alt+delete`, `control+alt+backspace`, `control+shift+escape`, `control+escape`, `control+alt+l`, `control+alt+t`, `alt+tab`, `alt+escape`, `alt+space`, `control+q`). Clipboard chords (`c`, `x`, `v` with a modifier) are refused under app or window scope on every platform because they are not edit keys. On macOS the policy allows and refuses exactly the chords the native adapter did; a test compares every grammar-valid chord against the native rule. The one visible change is timing: a refused chord no longer becomes an `unknown` operation that stops the grant.

### Support evidence

Each platform configuration has an evidence level compiled into the binary and reported in `status` and on the review card. Platforms added after macOS enable input only for `accepted` configurations; capture may run below `accepted` with its level shown.

| Level | Meaning |
|---|---|
| `unsupported` | refused with a reason code |
| `implemented` | code exists; no fixture run recorded |
| `fixture_tested` | passed mock or fixture tests in CI |
| `accepted` | passed a recorded run on a real interactive desktop with a disposable fixture |

| Platform | Configuration | Level |
|---|---|---|
| macOS | macOS 14 or newer, console session | `implemented` (until the acceptance run is recorded) |
| Linux | any desktop | `unsupported` |
| Windows | any edition | `unsupported` |

Only `accepted` configurations are claimed as supported in the README and feature matrices.

## Owner API and SDK

The capture directory is prepared before model/plugin subprocesses start and registered as a hidden path for that installation's home. Built-in filesystem tools and sandboxed commands cannot bypass Session-bound previews by reading another Session's PNG, even with explicit read grants. This is a tool/sandbox boundary: full-access or arbitrary owner shells still run with the host user's authority, not a separate OS identity.

Private capture previews are an explicit owner action: the dashboard fetches a bounded PNG through authenticated headers, keeps its Blob URL in memory and revokes it on Session changes/logout. Captures are never loaded through a password-bearing URL.

The owner API exposes `/api/v1/tools/computer/health`, `/setup`, `/stop`, and `/api/v1/sessions/{sessionID}/computer` with `/preview`, `/allow`, `/stop` and `/frames/{frameID}`. All responses use `Cache-Control: no-store`. Session routes require the exact Location. The owner password grants host control authority; these are not hub-member or peer grants. The CLI refuses setup/arming when invoked with an agent Session marker, but a local process with arbitrary owner-shell access can bypass such client checks; this feature does not add OS privilege separation. Stop revokes the Sessions managed by the addressed server; a separate custom-home/embedded server has its own Stop endpoint. The per-user native lease still prevents concurrent input across those servers. Use trusted HTTPS/SSH forwarding for access away from the host.

The generated TypeScript SDK provides `client.tool.computerHealth()`, `computerSetup()`, `computerStopAll()` and `client.session.computerStatus()`, `computerPreview()`, `computerAllow()`, `computerStop()` and `computerFrame()`. Private PNG reads return bounded `Uint8Array` over authenticated HTTP; stdio explicitly refuses binary frame reads. Owner review/setup calls are deliberate actions, not model tool operations.

Unix owner clients must now send the same Basic credential as TCP, alongside the same-UID socket check. This also applies to custom socket paths; a same-UID model process cannot create interactive presence or approve control merely by connecting. Rust clients use `Client::unix_authenticated(socket, password)`; `Client::unix(socket)` sends no credential and receives 401 for owner APIs. The TypeScript SDK has no native Unix transport; use an authenticated HTTP connection. The state directory is hidden from ordinary built-in reads and sandboxed subprocesses before launch, including later password replacements. Owner credentials cannot be forwarded by sandbox environment allow/set overrides or exact matching value aliases. These controls do not isolate an arbitrary full-access owner shell.

Custom owner Unix sockets must now be absolute paths under the private state directory, with an existing non-symlink parent and no symlink leaf. Workspace or shared-directory sockets are refused to prevent a model process replacing the listener and collecting owner credentials. Use `<state>/cyber.sock`, a precreated private state subdirectory, or authenticated TCP. Restart older servers after upgrading so state protection and Unix authentication are active; unsafe legacy socket registrations are refused rather than used to send credentials.

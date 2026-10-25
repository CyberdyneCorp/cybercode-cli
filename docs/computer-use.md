# Native computer use (experimental)

Cyber can observe and operate a native application through the `computer` model tool on macOS 14+. The capability is off by default. Linux, Windows and older macOS return explicit unsupported results; there is no shell, AppleScript, clipboard or external helper fallback. Native ScreenCaptureKit/CoreGraphics code is compiled into the server binary, so installed users do not need Xcode, Node or a runtime compiler.

The implementation has deterministic fake-adapter tests and native compile checks. Controlled live testing has verified application-window capture and delivery to a local vision model on macOS 26.7; **mouse and keyboard acceptance is still pending Accessibility permission**. The [acceptance record and controlled-device plan](measurements/native-computer-acceptance.md) separates actual device results from fixtures and remaining checks. Other display/keyboard layouts and signed distribution remain unaccepted.

## Enable and inspect

Set the feature in your global user configuration, then restart the server:

```sh
cyber features enable computer_use
cyber service stop
cyber computer status
```

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

An application grant pins its bundle ID, PID, launch time and window. `--display N` explicitly grants a whole native display; `--display 0` selects the main display. Whole-display capture can reveal other visible applications, so application scope is preferable when it meets the task. Grants expire after 300 seconds by default and accept at most 900 seconds. They never survive server restart or inherit to child Sessions, friends' agents or hub members. Only one native worker and one live host grant are admitted at a time. A per-user native lock coordinates separate server/embedded processes, and remains held until a cancelled worker actually exits. The empty `/private/tmp/cyber-computer-<uid>.lock` file remains after release; do not unlink a lock held by a running server.

The dashboard includes Native computer use controls for health/setup, exact target review, explicit arming, private capture preview and global Stop. Expand the native controls above the workspace to review a target; a controlling Session remains visible in the global banner even when no Session is selected. Logout sends an independent Stop before clearing owner credentials. The TUI shows an active-control banner; **Ctrl+Alt+Esc**, while the TUI has keyboard focus, revokes native input independently of the current overlay or model/tool request. It is not an operating-system global hotkey. `Esc` interruption also revokes the interrupted Session's grant. Losing the last attached Session's live human presence permanently revokes that grant; reconnecting requires a fresh target review and owner grant. The periodic browser presence lease can take up to its 45-second expiry after an ungraceful disconnect. Stop cannot undo already delivered clicks or text.

## Let the model act

Choose a model with image input for screenshots. The `computer` tool offers `status`, `screenshot`, `move`, `click`, `drag`, `scroll`, `type`, `key` and bounded `wait`. The human grant is separate from the normal `computer.capture` / `computer.control` permission decisions. Plan mode forbids input; saved allow rules, auto decisions and bypass mode cannot arm or widen a native grant.

Input requires the server-issued frame ID from a screenshot taken within 10 seconds, unchanged target/focus/geometry, in-image coordinates, live owner grant/presence and current OS consent. Limits are 10 input actions per second, 300 per grant, 5 seconds per action and 16 KiB UTF-8 per typed string. Capture coordinates are relative to the returned image; the native adapter maps these into desktop logical points, including negative monitor origins. Rotation and geometry changes invalidate previous frames.

Named keys are enter, escape, tab, space, backspace, delete, arrows, home/end and page_up/page_down; ASCII letters/digits resolve through the current native keyboard layout. Characters that the layout cannot resolve without hidden modifiers fail closed; use Unicode `type` for text. Application scope rejects Control shortcuts and shortcuts outside the limited editing allowlist to avoid switching applications or invoking host-wide actions. Secure text fields and protected system/security application targets are denied. These checks reduce risk; they do not turn native application input into a filesystem sandbox.

Native events are fenced while human permission/question requests are pending. Stop native control before approving another tool, answering a question or changing the Session model, agent, Mode or administrative authority; scoped computer capture/control approvals remain possible under the existing finite owner grant. Before pointer events, public Accessibility hit testing checks the actual window under the pointer. Application scope requires the reviewed PID, focused-window identity and geometry; covering windows from another app or another window are refused. Unidentifiable targets, protected system/security apps and known Cyber windows are refused even with whole-display scope. Both focused keyboard elements and pointer-hit elements are checked through a bounded Accessibility ancestor chain. Secure text fields, system/ordinary dialogs and sheets, modal windows and unidentified chains are unsupported and refused. These OS observations and event dispatch are separate calls; physical overlay/dialog/race acceptance is still pending. These checks prevent direct self-approval through the normal control surfaces; native applications still have host authority, and this is not a general isolation boundary against scripts, developer tools or host applications that can execute commands.

Screen text is untrusted external content. It cannot authorize wider scope, renew a grant or approve another tool. Do not authorize credential entry, password fields, operating-system permission dialogs or an application with private information visible unless you intend that content to be supplied to the selected provider.

## Evidence, privacy and uncertain outcomes

Each native operation records an intent before dispatch and a settlement afterwards. Reusing the same operation identity returns its prior receipt; conflicting payloads are refused. A timeout, failed input or missing settlement is **unknown**, not permission to retry. The grant stops after uncertain input. Inspect the target on the host, obtain another target review and use `--acknowledge-unknown --yes` only after reconciling the listed operations. Never automatically retype or reclick an unknown operation.

Typed strings remain ephemeral to dispatch; durable tool arguments, audit rows and hook inputs contain a byte length and SHA-256 digest instead of raw typed content. Pre-tool hooks cannot rewrite native input. Screenshots are private owner artifacts outside project attachment/export storage, with owner-authenticated Session-bound reads and restrictive file permissions. They are not automatically included in ordinary exports or collaboration messages. A vision-capable selected model receives authorized screenshots in its ephemeral request context; visible information can therefore leave the host for that provider. Raw screenshot files are not encrypted at rest by this feature; use the host's disk protection where required.

## Owner API and SDK

Private capture previews are an explicit owner action: the dashboard fetches a bounded PNG through authenticated headers, keeps its Blob URL in memory and revokes it on Session changes/logout. Captures are never loaded through a password-bearing URL.

The owner API exposes `/api/v1/tools/computer/health`, `/setup`, `/stop`, and `/api/v1/sessions/{sessionID}/computer` with `/preview`, `/allow`, `/stop` and `/frames/{frameID}`. All responses use `Cache-Control: no-store`. Session routes require the exact Location. The owner password grants host control authority; these are not hub-member or peer grants. The CLI refuses setup/arming when invoked with an agent Session marker, but a local process with arbitrary owner-shell access can bypass such client checks; this feature does not add OS privilege separation. Stop revokes the Sessions managed by the addressed server; a separate custom-home/embedded server has its own Stop endpoint. The per-user native lease still prevents concurrent input across those servers. Use trusted HTTPS/SSH forwarding for access away from the host.

The generated TypeScript SDK provides `client.tool.computerHealth()`, `computerSetup()`, `computerStopAll()` and `client.session.computerStatus()`, `computerPreview()`, `computerAllow()`, `computerStop()` and `computerFrame()`. Private PNG reads return bounded `Uint8Array` over authenticated HTTP; stdio explicitly refuses binary frame reads. Owner review/setup calls are deliberate actions, not model tool operations.

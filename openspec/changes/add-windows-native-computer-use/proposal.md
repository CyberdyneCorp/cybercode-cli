# Windows native computer use

## Why

Native computer use ships on macOS 14+ only. On Windows the `Native` backend reports unsupported health and refuses resolve, capture and input ([issue #44](https://github.com/CyberdyneCorp/cybercode-cli/issues/44)). Owners who run Cyber on Windows cannot let an agent look at or operate a native application window, even under the owner-armed, time-limited, Stop-able grant that macOS already enforces.

Windows needs its own reviewed design before any code: its capture, input, integrity-level, session and accessibility mechanisms differ from macOS, and several macOS assumptions (bundle-ID selection, 32-bit window numbers, a single OS consent toggle per binary) do not carry over. The issue requires this OpenSpec design to be reviewed before implementation.

## What Changes

- Add a Windows adapter behind the existing `Backend` port. It uses Windows.Graphics.Capture for window-only screenshots, `SendInput` for mouse and Unicode keyboard input, and Win32 window/process facts plus bounded UI Automation queries to check the target before every event.
- Keep the existing controller, runtime, journal, private artifact store, owner API and UI flows. Windows adds no second permission system.
- Build on the platform foundation (#134, `add-computer-use-platform-foundation`), which now holds the shared contract this draft first proposed (decision D12): the capability report, reason-code registry, `window` selector with opaque `target_id`, review-bound arming, grant lifecycle and host lease rules, shared chord policy, native worker contract, receipts and evidence gate. This change only fills those in for Windows: authority `owner_review_only`, selectors `["window"]`, Windows-only reason codes, the Windows rows of the chord table and the Windows evidence rows.
- Bind a Windows grant to one top-level window, its process, process start time, image path and token user, the interactive logon session and the input desktop. Lock, logoff, disconnect, desktop switch and target destruction revoke it. A display topology change does not revoke it but makes every earlier screenshot stale (the foundation's binding-loss and topology-change classes).
- Refuse elevated or unknown-integrity targets, a controller that runs elevated, `uiAccess` or outside an interactive session, targets owned by another user account, secure and password fields, dialogs, OS security, credential and consent surfaces, Cyber's own windows, overlays and owned popups (menus, drop-downs) in the first release, and unidentifiable UI. When a guarantee cannot be checked, the action is refused.
- Hold a per-user, per-session cross-process input lease, so separate server homes on the same desktop cannot drive input at the same time. A server that inherits a lease abandoned by a crashed holder warns the owner that cleanup is uncertain; the crashed home settles its own open intents on restart.
- Add a `cyber computer targets` listing and a Windows `--window` selection to the owner CLI/API, so the owner picks a window from what the server observed. A model never supplies a handle or PID.
- Gate the advertised Windows support on recorded real-desktop acceptance, separate from CI.

## Scope of this change

This change is the design deliverable only. It contains no Rust code, and nothing it describes is implemented or claimed to work. Implementation starts after the owner reviews this design and answers the open decisions in [design.md](design.md#9-open-decisions-for-the-owner).

Proposed first-release platform: Windows 11 x64, General Availability channel 25H2 (build 26200) and later, in a signed-in, unlocked, local console session. ARM64, Windows 10, Windows Server, RDP and other remote sessions keep explicit unsupported results until separate evidence and decisions exist.

## Non-goals

- Elevation, `uiAccess=true`, SYSTEM or service-desktop access, Session 0, or any helper that runs with more privilege than the owner's medium-integrity session.
- Automating UAC consent or credential prompts, the lock screen, Windows Security, or any secure-desktop surface.
- Bypassing UIPI, capture exclusion (`SetWindowDisplayAffinity`), protected content or DRM, and turning off the system capture border.
- Forced foreground takeover, input into background windows, `PostMessage`/`SendMessage` input, PowerShell, AutoHotkey or script fallbacks, and arbitrary UI Automation pattern invocation.
- Keylogging, clipboard reads, clipboard-based typing, audio/video recording and background screen surveillance.
- Silent widening from a window grant to a display grant. Whole-display capture on Windows is deferred to a separate decision.
- Remote hub, peer or relay members gaining desktop authority.
- Replacing or widening the Windows AppContainer command sandbox. It stays a separate feature and never receives desktop capture or input rights.

## Impact

- Depends on #134 for the shared types, Backend default methods, review-bound arming, chord evaluator, receipt fields and evidence table. This change must not merge before it.
- Specs: `computer-use` (modified capability requirement; added Windows requirements that reference the foundation's), `cli-commands` (the foundation's owner-command requirement plus `targets` and `--window`), `sandbox` (AppContainer capability specifics for the foundation's sandbox requirement).
- Code, after approval: `crates/cyber-computer` (a `windows` backend module, Windows reason codes, build configuration), `crates/cyber-server` runtime/HTTP computer modules, the OpenAPI document and generated SDK, the CLI `computer` command, TUI/dashboard target review, and Windows CI jobs.
- Dependencies, after approval: the Microsoft `windows` crate (already in the lockfile, MIT OR Apache-2.0) with additional features, and `png` (already in the lockfile, MIT OR Apache-2.0) for Windows encoding. No C/C++ shim, helper executable or runtime script.
- Docs, after implementation: `docs/computer-use.md`, a Windows acceptance record under `docs/measurements/`, README known gaps and roadmap.
- Release: Windows binaries are not published today ([release-without-windows](../release-without-windows/proposal.md)). Shipping this feature to users depends on that decision and on Authenticode signing. See the open decisions.

## Why

Native computer use exists only on macOS 14+. Two draft designs extend it: Linux/Wayland (#43, PR #130, `add-linux-native-computer-use`) and Windows 11 (#44, PR #131, `add-windows-native-computer-use`). Each draft found that today's public types are macOS-shaped:

- `Health` reports a Screen Recording flag and an Accessibility flag. Linux has no OS permission at all, and Windows has no separate input permission on the path it recommends.
- `Selector` accepts a bundle ID or a display index. Neither works on Linux, where the owner picks the source in the portal dialog, or on Windows, where a window is chosen from a server-observed candidate list.
- `NativeTarget` carries numeric macOS process, window and display handles.
- `Error::UnsupportedPlatform` names macOS 14, and refusals carry free-text messages, not stable codes.
- Key-chord restrictions live inside the macOS native shim, so a second platform would have to re-implement them and could drift.

Both drafts propose overlapping but differently named fixes (`input_control` and `refusals[]` on one side, `observation`/`input` objects and `target_id` on the other) and both say the shared part must land once, in one reviewed change, before either adapter. This change is that foundation. It defines the contract both adapters build on, and it keeps macOS behavior exactly as shipped.

## What Changes

- **Capability report.** Health gains a versioned, platform-neutral report: a derived mode (`unsupported`, `capture_only` or `input_enabled`), separate capture and input availability with stable reason codes, the kind of authority the platform uses, interactive-session state, OS and backend diagnostics, the selectors the platform accepts and the evidence level of the running configuration. Legacy fields keep their macOS meaning.
- **One reason-code registry.** Every refusal, unavailability and revocation uses a stable snake_case code from one registry. The names already used by the two drafts are reconciled (for example `selector_unsupported_on_platform`, `remote_session`, `session_watch_unavailable`).
- **Selectors and target identity.** Exactly one of `app`, `display`, `window` (an owner-listed candidate) or `pick` (`window` or `output`, chosen in the platform picker). Each platform advertises which it honors and refuses the rest. Targets get a server-issued opaque `target_id` and a typed source description; numeric macOS handles stay as deprecated compatibility fields.
- **Review-bound arming.** When a selection cannot be re-resolved deterministically, `allow` arms only the review the owner saw, by its preview identifier.
- **Shared grant lifecycle.** Owner-only, Session-bound, 300 s default / 900 s cap, presence-bound, never restored. Events are classified as binding loss (revoke) or topology change (frames stale, grant kept). One host-input lease per OS user and interactive session; health never takes it.
- **Schema versioning.** An integer `schema_version` on health, previews, frames and receipts. Version 1 is today's shape; this change defines version 2, additive only. The model tool keeps one action schema on every platform; an action a platform cannot perform is refused with a code before any intent is journaled.
- **Shared key-chord policy.** One platform-neutral evaluator decides which chords are allowed per scope, using logical modifier roles. It refuses OS-global and secure-attention chords and clipboard chords under app/window scope. The macOS rows reproduce today's native rules exactly, and the native check stays as a second line.
- **Native worker boundary.** One contract for whatever runs native code, in process or in a helper: it owns every native handle, nothing it holds is inheritable, Stop never waits on it, the lease outlives it, a helper runs as the same user from the same binary with no extra privilege and speaks only a typed channel it inherited. Sandboxed processes never receive capture or input authority.
- **Receipts and audit.** Receipts and journal rows gain schema version, platform, backend, grant/target/frame references, reason code, event counts, cleanup state and the list of assurance checks that ran, never raw typed text or window titles.
- **Support evidence.** Each platform configuration carries an evidence level (`unsupported`, `implemented`, `fixture_tested`, `accepted`). A new platform may enable input only for configurations recorded as accepted.
- **CLI.** `cyber computer status` prints the mode and reason codes; `cyber computer allow <session> --preview <id> --yes` confirms an existing pending review by identifier.

## Non-goals

- Any Linux or Windows adapter, transport, picker, assurance provider or platform mechanism. Those stay in #130 and #131, which now build on this change.
- Any change to macOS capture, input, consent, limits, receipts outcomes or CLI behavior beyond the additive fields above.
- Tightening macOS rules (for example applying the global chord deny-list to macOS display grants, or adding lock-screen revocation on macOS). Those are listed as owner decisions and would land separately.
- Removing deprecated legacy fields. That happens only in a later schema version after the deprecation window in F4.
- Display scope on Windows, input on Linux, RDP or other remote sessions.

## Impact

- Specs: `computer-use` (added platform-neutral requirements), `cli-commands` (modified owner command requirement), `sandbox` (added requirement that sandboxed processes hold no native computer authority).
- Code, after approval: `crates/cyber-computer` (`backend.rs` types and default trait methods, a new chord-policy module, reason-code enum, platform-neutral `Error` text), `crates/cyber-server/src/runtime/computer` (admission formula, receipt and journal fields, preview-bound arming), `crates/cyber-server/src/http/computer.rs`, OpenAPI and the generated TypeScript SDK, CLI/TUI/dashboard rendering of mode and reasons.
- Storage: the `computer_operation` journal stores the receipt schema version; version 1 rows stay readable.
- Docs, after implementation: `docs/computer-use.md` gains the reason-code registry, the evidence-level table and the chord policy table.
- Downstream: PR #130 and PR #131 drop their own copies of the shared requirements and reference this change.

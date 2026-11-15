# add-computer-use-platform-foundation

Platform-neutral contract for native computer use, shared by the Linux ([#43](https://github.com/CyberdyneCorp/cybercode-cli/issues/43), PR #130) and Windows ([#44](https://github.com/CyberdyneCorp/cybercode-cli/issues/44), PR #131) designs.

Status: **design approved by the owner 2026-11-15 (recommended option for every decision). No implementation exists.** Apart from F8c, nothing here changes macOS behavior; section 12 of the design shows how today's macOS adapter maps onto the contract field by field. Decisions F1–F12 are in [design.md](design.md#13-decisions); the owner-review ones were approved on 2026-11-15. The only macOS change is F8c (blocked chords are refused before logging and the grant stays armed).

Review [proposal](proposal.md), [design](design.md), [tasks](tasks.md) and the contract deltas for [computer-use](specs/computer-use/spec.md), [cli-commands](specs/cli-commands/spec.md) and [sandbox](specs/sandbox/spec.md).

# add-windows-native-computer-use

Design for Windows native computer use ([issue #44](https://github.com/CyberdyneCorp/cybercode-cli/issues/44)): guarded window capture, input and desktop safety on Windows 11.

Status: proposal awaiting owner review (2026-11-14). It contains no implementation, and no Windows computer-use capability is claimed. Open decisions are in [design.md](design.md#9-open-decisions-for-the-owner); D8 and D12 moved to the foundation and were approved there.

Depends on #134 ([add-computer-use-platform-foundation](../add-computer-use-platform-foundation/README.md)), which defines the platform-neutral contract this change builds on; it must not merge before #134.

Review [proposal](proposal.md), [design and threat model](design.md), [tasks and acceptance plan](tasks.md), and the contract deltas for [computer-use](specs/computer-use/spec.md), [cli-commands](specs/cli-commands/spec.md) and [sandbox](specs/sandbox/spec.md).

## Why

People coming from Claude Code type `--dangerously-skip-permissions` to run without prompts. `cyber` spells that `--mode bypass` (or `exec --yolo`) plus `--dangerously-bypass-permissions`, and rejected the Claude Code name as an unknown flag.

## What Changes

- `exec-mode` "Model, agent and mode selection": `cyber exec --dangerously-skip-permissions` selects `bypass` and carries the no-sandbox override of `--dangerously-bypass-permissions`. Like that flag it needs `--embedded` or `--ephemeral`; on the shared server it fails with exit code 2, naming those flags and pointing at `--mode bypass` where a sandbox or container already admits `bypass`. It conflicts with `--auto` and with a `--mode` other than `bypass`.
- `tui` "Launch flags": `cyber --dangerously-skip-permissions --embedded` starts the TUI in `bypass` with the override; without `--embedded` it fails before the UI renders.
- `permissions-modes` "bypass mode": `--dangerously-skip-permissions` counts as the explicit override where it is accepted. Org policy `modes.disable` still refuses `bypass`, and `cyber mcp serve` never takes either flag.

## Impact

`crates/cyber-cli` only (flag parsing and validation). No server or API change. README gains a Permission modes section; `docs/cli.md` is a generated command-line reference with a staleness test.

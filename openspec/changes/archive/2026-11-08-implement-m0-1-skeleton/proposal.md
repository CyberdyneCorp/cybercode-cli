# Implement M0.1: skeleton and evaluation fixtures

## Why

P0 starts with milestone M0.1 (ROADMAP): a Rust workspace, XDG paths, SQLite WAL/FULL with the event store and bounded writer queue, a config loader with trust-gated JSONC layering, and reproducible coding and recovery fixtures. Later milestones (first turn, durable runtime, tools, surfaces) build on these foundations and on a release gate that exists from the first line of code.

Implementing the loader exposed two spec defects. `-c` was assigned both to `--continue` (default TUI command) and to `--config` overrides (global flags). The installation spec uses an `autoupdate` key that the configuration schema did not list.

## What Changes

- Cargo workspace with `cyber-core`, `cyber-store`, `cyber-llm`, `cyber-server`, `cyber-tui` and `cyber-cli` (binary `cyber`). `cyber-llm`, `cyber-server` and `cyber-tui` are placeholders for M0.2, M0.3 and M0.5.
- `cyber-core`: build information, XDG paths, prefixed IDs, JSONC parsing with positions, layer merge with source attribution, `{env:}`/`{file:}` substitution, profiles, `--config` overrides, checkout-scoped trust gating of security-sensitive project definitions, and evaluation manifest validation.
- `cyber-store`: SQLite with WAL and FULL synchronous verified on every writer connection, timestamped idempotent migrations with read-only open of newer databases, per-aggregate gapless event store with transactional projectors, upcasters, registry of event types, bounded writer queue with `StorageBusyError`, fail-stop on disk-full and I/O errors, gapless tail, ownership lock, network-filesystem rejection and read-only query.
- `cyber` CLI: `--version`, `debug paths|config|info`, `db path|query`, `trust inspect|approve|revoke`; strict parsing; every other command in the tree fails with exit 2 and an unavailable-capability message.
- Evaluation: a deterministic coding manifest with a pinned fixture tree hash and a hidden grader, and a recovery manifest linking seven fault cases to their requirements.
- Spec corrections: `--config` is long-only so `-c` stays `--continue`; `autoupdate` is a top-level key.
- CI: format, clippy (warnings and cognitive complexity above 15 denied), tests, OpenSpec validation and spec lint.

## Impact

First runtime code. No P0 requirement is weakened. Requirements not covered by M0.1 (sessions, providers, tools, server, TUI) remain for M0.2 to M0.5.

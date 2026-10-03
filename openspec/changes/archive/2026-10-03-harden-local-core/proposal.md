# Harden the local core

## Why

The target product has broad feature coverage, but everyday use depends on measurable coding quality and precise recovery, trust and preservation guarantees. The initial review identified uncertain external tool outcomes, checkout trust gaps, rewind conflicts, compaction loss and conflicting shared contracts.

## What Changes

- Make harness evaluation and workspace trust P0 capabilities; add optional browser verification in P1.
- Strengthen tool recovery, compaction task state, rewind conflict handling, budget accounting and remote execution ownership.
- Retain SQLite locally with stronger durability and writer ownership; document PostgreSQL's hosted role and alternatives.
- Reconcile role defaults, unknown pricing, mode transitions and phase dependencies.
- Update delivery gates, capability inventory and documentation.

## Impact

This is an authorized revision of a greenfield target specification, not a claim of implemented behavior. Canonical specs are updated now so readers see the agreed target. The accompanying deltas record the exact changes from the previous target; implementation will use separate delivery changes. No runtime code is introduced.

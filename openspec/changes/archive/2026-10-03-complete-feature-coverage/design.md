# Design

## Decisions

**Peers before Relay.** The local-first principle says everything on the user's own machines works without an account. A `peers` list (URL plus password or mTLS) gives two of the user's servers a direct, authenticated channel for listing, messaging and attaching. The Relay remains the answer for NAT traversal, phones and org features; peers reuse the same inbound controls (`hold` by default for anything off-machine).

**Fan-out across machines reuses Runners.** Rather than a second scheduler, a workflow `agent()` call or an `agent` tool spawn names a `runner` (pool, Runner ID or peer). The child Session is created there through the Orchestrator or peer API, the repository is materialized as for `--cloud`, and results, budgets and the monitor behave exactly as for local children.

**Feature flags are a registry.** `experimental.*` becomes `features.*` with a built-in list of names and maturity labels, so users and policy can reason about one surface and `cyber features` can explain what is on.

**Hooks grow by events and handler kinds, not by a new mechanism.** New events (`Setup`, `PermissionDenied`, `InstructionsLoaded`, model switches, elicitation, `DirectoryAdded`, `PostToolBatch`, `Interrupt`) and new handler fields (`if`, `once`, `status_message`, `system_message`) fit the existing matcher and decision schema. Plugin request interception lives in the plugin host protocol because it needs code, not configuration.

**Web client is one bundle.** The same web UI is served locally by `cyber web`, by the Relay for remote Devices, and wrapped by the desktop shell. Only the transport differs.

**Codex-style sandbox profiles are additive.** Named profiles compose the existing policy, roots, domain list and env filter; the three built-ins map to today's policies so nothing existing changes.

## Validation

`openspec validate --all --strict`, `python3 scripts/spec_lint.py`, `python3 scripts/spec_inventory.py`.

## Tradeoffs

Peers add a second remote transport to secure and test. Remote `agent()` makes workflow replay depend on Orchestrator availability. The features registry forces every flag to be declared, which is the point. Items deliberately left out of scope and recorded as such: MCP Code Mode (deferred tools cover the context problem), OpenAI connector apps, desktop computer use, voice beyond dictation.

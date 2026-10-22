# Specification and documentation reconciliation (2026-10-22)

The canonical [specifications](../../openspec/specs) describe the target product, including planned requirements. This reconciliation copies implemented contracts from active changes into the canonical specs and corrects documentation against the current code. Synchronization does not mark a feature complete, prove live-provider behavior, or satisfy native platform acceptance.

## Contracts reconciled

- Copilot subscription login/catalog and TUI provider login.
- MCP serve's transport, trust, tools, receipts, limits and approval boundaries in the new `mcp-server` capability; future grants, approval relay, team/workflow tools and remote access retain their phase tags.
- Direct peers: user-only configuration, dedicated listener, pinned TLS fingerprints, hashed grant tokens, pairing, discovery, outcomes and attach. The obsolete password/mTLS array and off-loopback insecure override are removed.
- Workflow runtime and multiple runs: fair grants, run queue, isolation, recovery, monitor, indicators, notifications and SDK helpers.
- Session rename and branches: tree/checkout/labels, active-path context, optional file restore and JSON export. Sanitization, children and Markdown/HTML export remain planned.
- TUI themes, rendering, usage/statistics, machine stats, recap and explorer contracts, plus project instruction discovery and imports.
- Team task/idle/event details, the skip-permissions alias, stale-server handling and the editor deferral to P3.
- Doom-loop detection: intervene before the third consecutive identical call in a run, reset on promoted input, and halt in `auto`, `dont-ask` and `bypass`. Evidence: `crates/cyber-server/src/runtime/drain.rs` and `tests/requests.rs`.

Active changes remain open for unfinished implementation and acceptance. Overlapping deltas retain the newly canonical scenarios so a later archive cannot silently discard them. Unrelated target requirements remain in place.

## Documentation corrections

The README quickstart lists saved workflows and links to the example that creates `fanout`. The feature reference now distinguishes implemented Session goals and loops from their remaining scope. The glossary no longer calls those implemented subsets wholly planned; `--until-goal` is described as unconnected rather than waiting for goals to exist. The roadmap includes the accepted workflow monitor. The guide and SDK distinguish implemented soft descendant budget enforcement and explicit subtree stop from pending budget-triggered cancellation and client adoption.

## Current P1 inventory

There are **48 capabilities and 894 requirements** across all phases, including **270 P1 requirements**. Phase tags use the first tag in each requirement, matching `scripts/spec_inventory.py`. The 13 editor requirements moved to P3 are excluded from this P1 total.

| Capability | Canonical P1 requirements |
|---|---:|
| `agents-subagents` | 14 |
| `background-tasks` | 15 |
| `browser-verification` | 2 |
| `builtin-tools` | 4 |
| `cli-commands` | 2 |
| `client-sdk` | 1 |
| `code-intelligence` | 12 |
| `compaction` | 3 |
| `compat-import` | 10 |
| `configuration` | 2 |
| `cross-session-messaging` | 20 |
| `exec-mode` | 5 |
| `hooks` | 20 |
| `installation-upgrade` | 1 |
| `mcp` | 7 |
| `mcp-server` | 11 |
| `memory` | 13 |
| `observability-costs` | 15 |
| `permissions-modes` | 7 |
| `plugins-marketplace` | 14 |
| `provider-catalog` | 6 |
| `provider-credentials` | 7 |
| `sandbox` | 8 |
| `server-api` | 2 |
| `session-runtime` | 6 |
| `skills-commands` | 5 |
| `snapshots-checkpoints` | 3 |
| `storage-events` | 2 |
| `system-context` | 5 |
| `tool-registry` | 3 |
| `tui` | 32 |
| `vcs-integration` | 3 |
| `web-client` | 1 |
| `worktrees` | 9 |
| **Total** | **270** |

The [historical acceptance plan](p1-acceptance-plan.md) classifies its original 214-requirement scope. **213** current capability/name pairs match rows in that plan. The remaining **57** are listed below and require clause-level classification and evidence; they are not silently treated as accepted or unimplemented. Existing name matches also require review where synchronized clauses expanded. Historical dated counts and test results remain historical evidence.

| Capability | Requirement without an exact historical plan row |
|---|---|
| `cli-commands` | Session branch commands |
| `cross-session-messaging` | Message chain limit |
| `cross-session-messaging` | Messaging CLI |
| `mcp` | MCP server mode |
| `mcp-server` | MCP serve transports |
| `mcp-server` | Local client trust model |
| `mcp-server` | Served tools and errors |
| `mcp-server` | Session creation over MCP |
| `mcp-server` | Session reading over MCP |
| `mcp-server` | Prompt admission over MCP |
| `mcp-server` | Prompt results and waiting |
| `mcp-server` | Messaging over MCP |
| `mcp-server` | Approvals stay with human clients |
| `mcp-server` | MCP serve limits |
| `mcp-server` | MCP recursion guard |
| `observability-costs` | Usage step ledger |
| `observability-costs` | Session usage breakdown |
| `observability-costs` | Usage statistics API |
| `observability-costs` | Usage insights |
| `observability-costs` | Machine stats API |
| `permissions-modes` | Auto-mode configuration and override |
| `provider-catalog` | ChatGPT subscription provider |
| `provider-catalog` | GitHub Copilot subscription provider |
| `provider-credentials` | ChatGPT subscription login |
| `provider-credentials` | ChatGPT token refresh |
| `provider-credentials` | Provider login API |
| `provider-credentials` | GitHub Copilot subscription login |
| `server-api` | File browsing routes |
| `server-api` | Session branch routes |
| `session-runtime` | Assistant message timing |
| `session-runtime` | Session recap |
| `session-runtime` | Session branches |
| `tui` | Cross-session message surfaces |
| `tui` | Theme tokens |
| `tui` | Color capability and NO_COLOR |
| `tui` | Markdown rendering |
| `tui` | Syntax-highlighted code blocks |
| `tui` | Turn footer marker |
| `tui` | Composer appearance |
| `tui` | Command palette |
| `tui` | Welcome screen |
| `tui` | Session sidebar |
| `tui` | Machine stats |
| `tui` | Responsive layout |
| `tui` | Session recap line |
| `tui` | Settings panel |
| `tui` | Settings Status tab |
| `tui` | Settings Config tab |
| `tui` | Settings Usage tab |
| `tui` | Usage limit meters |
| `tui` | Usage contributing factors |
| `tui` | Settings Stats tab |
| `tui` | File explorer |
| `tui` | Theme switcher |
| `tui` | Nyan progress indicator |
| `tui` | Provider login commands |
| `tui` | Branch tree view |

## Verification

Run `openspec validate --all --strict`, `python3 scripts/spec_lint.py`, `python3 scripts/spec_inventory.py` and `git diff --check`. Compare the generated ROADMAP inventory before and after regeneration to check stability. `docs/cli.md` and `sdk/openapi.json` have executable freshness tests; neither generated artifact needs a content change for this reconciliation.

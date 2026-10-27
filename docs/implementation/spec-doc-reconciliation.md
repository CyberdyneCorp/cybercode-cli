# Specification and documentation reconciliation (2026-10-27)

The canonical [specifications](../../openspec/specs) describe the target product, including planned requirements. This reconciliation copies implemented contracts from active changes into the canonical specs and corrects documentation against the current code. Synchronization does not mark a feature complete, prove live-provider behavior, or satisfy native platform acceptance.

## Contracts reconciled

- Copilot subscription login/catalog and TUI provider login.
- MCP serve's transport, trust, tools, receipts, limits and approval boundaries in the new `mcp-server` capability; future grants, approval relay, team/workflow tools and remote access retain their phase tags.
- Direct peers: user-only configuration, dedicated listener, pinned TLS fingerprints, hashed grant tokens, pairing, discovery, outcomes and attach. The obsolete password/mTLS array and off-loopback insecure override are removed.
- Workflow runtime and multiple runs: fair grants, run queue, isolation, recovery, monitor, indicators, notifications and SDK helpers.
- Session rename and branches: tree/checkout/labels, active-path context, optional file restore and JSON, Markdown and self-contained HTML export, sanitization, child inclusion and branch selection.
- TUI themes, rendering, usage/statistics, machine stats, recap and explorer contracts, plus project instruction discovery and imports.
- Team task/idle/event details, the skip-permissions alias, stale-server handling and the editor deferral to P3.
- Doom-loop detection: intervene before the third consecutive identical call in a run, reset on promoted input, and halt in `auto`, `dont-ask` and `bypass`. Evidence: `crates/cyber-server/src/runtime/drain.rs` and `tests/requests.rs`.

Active changes remain open for unfinished implementation and acceptance. Overlapping deltas retain the newly canonical scenarios so a later archive cannot silently discard them. Unrelated target requirements remain in place.

## Documentation corrections

The README quickstart lists bundled and saved workflows. The feature reference now distinguishes implemented Session goals and loops from their remaining scope. The glossary no longer calls those implemented subsets wholly planned; `--until-goal` is described as unconnected rather than waiting for goals to exist. The roadmap includes the accepted workflow monitor. The guide and SDK distinguish implemented soft descendant budget enforcement and explicit subtree stop from pending budget-triggered cancellation and client adoption.

## Current inventory and acceptance coverage

As of 2026-10-27 there are **52 capabilities and 1,016 requirements**, including **338 P1 requirements**. Counts use the first phase tag per requirement, matching `scripts/spec_inventory.py` and the [roadmap](../../ROADMAP.md#requirement-inventory). The editor deferral is already canonical; its unarchived change does not imply another phase-count shift.

The 2026-10-27 audit synchronized delivery contracts that were present only in active changes: image/PDF storage and input, sanitized HTML/Markdown exports, output styles, OpenTelemetry, private browser access, collaboration, native computer use, real-repository evaluations, sounds, mouse controls, agent directory/sidebar and bundled workflows. Missing lifecycle, launch, LSP, migration, deferred-tool and evaluation clauses were copied from their delivery deltas without closing outstanding tasks. The reserved-budget lifecycle now matches its implemented bound errors, atomic accounting and receipt-day semantics. Canonical specs still include target behavior; synchronizing a clause does not establish implementation or acceptance of every scenario.

Source and evidence checked for this audit:

| Area | Implementation and evidence |
|---|---|
| Build and command reference | `Cargo.toml`, `justfile`, `.github/workflows/ci.yml`, `crates/cyber-cli/src/cli_docs.rs`, [CLI reference](../cli.md) |
| Reservations | `crates/cyber-server/src/runtime/reservations/`, `crates/cyber-store/tests/reservations.rs`, [bounds and limits](../reserved-budgets.md), [implementation evidence](reserved-budgets.md) |
| Exports and attachments | `crates/cyber-client/src/export/`, `crates/cyber-cli/tests/export_cli.rs`, `crates/cyber-server/src/http/export/`, [Session guide](../local-core-guide.md#naming-and-resuming-sessions) |
| Browser authority | `crates/cyber-server/src/http/`, `web/`, [dashboard guide](../web-dashboard.md), [acceptance evidence](web-dashboard.md) |
| Native computer use | `crates/cyber-computer/`, `crates/cyber-server/tests/computer.rs`, [native acceptance record](../measurements/native-computer-acceptance.md) |
| Collaboration and account | `crates/cyber-collab/`, `crates/cyber-account/`, [implementation evidence](project-collaboration.md); live issuer provisioning remains open |
| TUI and output styles | `crates/cyber-tui/src/sidebar.rs`, `crates/cyber-tui/src/sound/`, `crates/cyber-tools/tests/output_style_context.rs`, [screenshot gallery](../screenshots/README.md) |
| LSP, migration and subagents | `crates/cyber-tools/src/lsp/`, `crates/cyber-core/src/import/`, [P1 evidence](p1-status.md), delivery deltas and their unfinished acceptance tasks |
| Workflows and evaluation | `crates/cyber-workflow/`, [P2 evidence](p2-status.md), [real-repository pilot](../measurements/real-repository-evaluation.md) |

This is a documentation and contract audit, not a new live-provider, billing, native-platform or full scenario-acceptance run. Competitor comparisons and external deployment observations retain their original dates. Overlapping active deltas retain the synchronized contract and scenarios so later archival cannot restore the older export, browser-auth or TUI behavior. Active changes remain open for unfinished tasks and archival; synchronization does not complete those tasks.

### P1 inventory

| Capability | Canonical P1 requirements |
|---|---:|
| `agents-subagents` | 24 |
| `background-tasks` | 15 |
| `browser-verification` | 2 |
| `builtin-tools` | 4 |
| `cli-commands` | 3 |
| `client-sdk` | 1 |
| `code-intelligence` | 34 |
| `compaction` | 3 |
| `compat-import` | 19 |
| `configuration` | 3 |
| `cross-session-messaging` | 21 |
| `exec-mode` | 5 |
| `hooks` | 20 |
| `installation-upgrade` | 1 |
| `mcp` | 7 |
| `mcp-server` | 11 |
| `memory` | 13 |
| `observability-costs` | 17 |
| `permissions-modes` | 7 |
| `plugins-marketplace` | 14 |
| `provider-catalog` | 6 |
| `provider-credentials` | 7 |
| `sandbox` | 9 |
| `server-api` | 6 |
| `session-runtime` | 6 |
| `skills-commands` | 5 |
| `snapshots-checkpoints` | 3 |
| `storage-events` | 2 |
| `system-context` | 5 |
| `tool-registry` | 4 |
| `tui` | 41 |
| `vcs-integration` | 3 |
| `web-client` | 5 |
| `worktrees` | 12 |
| **Total** | **338** |

The [historical acceptance plan](p1-acceptance-plan.md) classifies its original 214-requirement scope. **213** current capability/name pairs match rows in that plan; the remaining **125** below need explicit clause-level classification and evidence. A matching name does not revalidate expanded clauses or establish platform acceptance. Dated test results and historical counts remain unchanged.

| Capability | Requirement without an exact historical plan row |
|---|---|
| `agents-subagents` | Requested child inbox handoff |
| `agents-subagents` | Child continuation preparation ownership |
| `agents-subagents` | Captured child launch authority |
| `agents-subagents` | Delegation callback authority |
| `agents-subagents` | Owned legacy subtask callback |
| `agents-subagents` | Durable subtree admission closure |
| `agents-subagents` | Owned bounded subtree stop |
| `agents-subagents` | Durable child result ownership and target authority |
| `agents-subagents` | Retained native settlement proof |
| `agents-subagents` | Matched verified subtree reopening |
| `cli-commands` | Session branch commands |
| `cli-commands` | Telemetry status command |
| `code-intelligence` | Existing-server CLI LSP observation |
| `code-intelligence` | Bounded peer publication scheduling before feedback |
| `code-intelligence` | Automatic sandboxed formatting after file edits |
| `code-intelligence` | Runtime Location activity and idle release |
| `code-intelligence` | Explicit Location language-service transitions |
| `code-intelligence` | Authenticated language-service lifecycle API |
| `code-intelligence` | Document checkout admission |
| `code-intelligence` | Language-server managed launch pins |
| `code-intelligence` | Independent language-server checkout ownership |
| `code-intelligence` | Live LSP generation authority |
| `code-intelligence` | Owned background LSP warming |
| `code-intelligence` | Trusted local language-server launch |
| `code-intelligence` | Continuous owned LSP message handling |
| `code-intelligence` | Owned Location language-server pool |
| `code-intelligence` | Owned LSP connection lifecycle |
| `code-intelligence` | Bounded LSP transport framing |
| `code-intelligence` | Bounded validated diagnostic publications |
| `code-intelligence` | Owned edit diagnostic feedback |
| `code-intelligence` | Document-scoped feedback debounce and supersession |
| `code-intelligence` | Owned patch deletion and rename synchronization |
| `code-intelligence` | Owned read-only navigation tool |
| `code-intelligence` | Rejected synchronization activity disposal |
| `compat-import` | Pure Claude permission conversion |
| `compat-import` | Imported edit-tool permission granularity |
| `compat-import` | Pure OpenCode permission conversion |
| `compat-import` | Pure Codex argv-prefix conversion |
| `compat-import` | Literal Codex rules source parsing |
| `compat-import` | Read-only migration file inventory |
| `compat-import` | Verified migration source snapshots |
| `compat-import` | Read-only static-source detection command |
| `compat-import` | Secret-safe Codex provider/model planning |
| `configuration` | Owner listener TLS settings |
| `cross-session-messaging` | Message chain limit |
| `cross-session-messaging` | Messaging CLI |
| `cross-session-messaging` | Reachable session provenance |
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
| `observability-costs` | OpenTelemetry configuration |
| `observability-costs` | OpenTelemetry delivery |
| `permissions-modes` | Auto-mode configuration and override |
| `provider-catalog` | ChatGPT subscription provider |
| `provider-catalog` | GitHub Copilot subscription provider |
| `provider-credentials` | ChatGPT subscription login |
| `provider-credentials` | ChatGPT token refresh |
| `provider-credentials` | Provider login API |
| `provider-credentials` | GitHub Copilot subscription login |
| `sandbox` | Invocation launch excludes ambient package wildcard authority |
| `server-api` | File browsing routes |
| `server-api` | Session branch routes |
| `server-api` | Interactive browser attachment leases |
| `server-api` | Explicit Session inbox wake |
| `server-api` | Explicit subtree stop API |
| `server-api` | Reviewed subtree reopening API |
| `session-runtime` | Assistant message timing |
| `session-runtime` | Session recap |
| `session-runtime` | Session branches |
| `tool-registry` | Scope-aware deferred materialization |
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
| `tui` | Export command |
| `tui` | Sound alerts |
| `tui` | Sound packs |
| `tui` | Sound settings command |
| `tui` | Agent directory |
| `tui` | Mouse clicks |
| `tui` | Sidebar reachable Session overview |
| `tui` | Shared directory refresh lifetime |
| `tui` | Sidebar discovery navigation and accessibility |
| `web-client` | Browser login and credential handling |
| `web-client` | Browser approval inbox and authority |
| `web-client` | Browser attachment and recovery state |
| `web-client` | Responsive accessible dashboard |
| `worktrees` | Managed creation ownership |
| `worktrees` | Explicit child setup recovery |
| `worktrees` | Worktree Location stream continuity |

## Verification

Use OpenSpec **1.13.2**, matching CI: `npx --yes --package @fission-ai/openspec@1.13.2 openspec validate --all --strict`. Also run `python3 scripts/spec_lint.py`, `python3 scripts/spec_inventory.py` and `git diff --check`. Regenerate the inventory twice to verify stability. The CLI reference and OpenAPI have executable freshness tests; generated SDK consistency is checked with `python3 scripts/generate_sdk.py --check`.

### Results recorded on 2026-10-27

- OpenSpec 1.13.2 strict validation: **103 passed, 0 failed**.
- Canonical spec lint: **0 errors, 25 warnings**. The same lint against a temporary copy of `main` returned **0 errors, 23 warnings**. The two additional warnings identify the synchronized P0 export's Goal fields and the P1 task view's Loop references; these are later-phase extensions, not new implementation claims.
- Inventory regeneration: **52 capabilities, 1,016 requirements**; repeated regeneration produced identical ROADMAP bytes.
- `cargo test --locked -p cyber-cli --bin cyber docs_cli_md_is_current`: **1 passed**.
- `cargo test --locked -p cyber-server --test http openapi_document_is_current`: **1 passed**.
- `python3 scripts/generate_sdk.py --check`: passed; generated artifacts need no change.
- Local Markdown path/heading check across README, ROADMAP, docs and canonical specs: **87 files**, no broken paths or heading anchors after excluding inline-code examples. External URLs were not revalidated.
- `git diff --check`: passed.

The full Rust/SDK/dashboard suites, live providers and platform acceptance were not rerun for this documentation-only change. Existing dated evidence remains the source for those outcomes.

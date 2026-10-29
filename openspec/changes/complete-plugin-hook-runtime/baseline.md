# Source and test baseline

Recorded on 2026-10-27 against `097b6278` (`origin/main`). Application, SDK and test sources are unchanged; the planning branch adds only OpenSpec artifacts. This baseline is evidence about the existing implementation, not acceptance of the proposed work.

## Capability and protocol evidence

| Contract | Source evidence | Remaining work |
|---|---|---|
| Manifest capability parsing | `crates/cyber-core/src/plugins/capabilities.rs` parses independent `fs_read`, `fs_write` and `shell` flags | Parsing does not prove native enforcement |
| Host confinement | `crates/cyber-tools/src/plugin_host/launch.rs::confinement` does not consult `fs_read` or `shell`; it adds Location reads and general sandbox policy | Native read/exec capability intersection and supported-runtime proof |
| macOS process boundary | `crates/cyber-sandbox/src/seatbelt.rs` grants `process-exec`, `process-fork` and general `file-read*`, then narrows selected paths | A general command policy cannot substantiate the proposed plugin capability contract |
| Linux process boundary | `crates/cyber-sandbox/src/linux.rs` starts bubblewrap with a read-only host root and writable overlays | Read-only exposure still permits reads; subprocess restrictions need their own proven primitive |
| Runtime registrations | `crates/cyber-tools/src/plugin_host/mod.rs::initialized` logs and ignores nonempty hooks/providers/catalogTransforms | Generation-owned validation, registration, dispatch and revocation |
| Prompt callbacks | The same module's handler checks `session_prompt` and returns method-not-supported even when declared | Invocation-bound durable admission and cancellation/recursion ownership |
| TypeScript kit | `sdk/plugin/src/server.ts` dispatches hooks and providers and initializes an empty catalogTransforms list | Core integration plus transform API; harness support alone is not production support |

## Event producer correction

Two named events already have meaningful production behavior. Completing their coverage must preserve these existing semantics:

- `cyber-server/src/runtime/lifecycle.rs::setup` already dispatches observational `Setup` after the first root Session commits. Its comment explicitly states that a crash between commit and dispatch skips the observation instead of replaying it. Explicit init and reviewed durable marker/concurrency behavior still need closure.
- `cyber-tools/src/lifecycle_hooks.rs::worktree_create_hooks` dispatches `WorktreeCreate` after permission and before Git effects. Deny/block or fail-closed unavailability can veto creation. `worktrees.rs::admit_worktree_creation` calls it. Creation-path coverage must be audited; this hook must not be moved after mutation or duplicated as a success event.
- `DirectoryAdded` and `CwdChanged` appear in event validation but no production emitter was located in the core/tools/server/app source search. `FileChanged` appears in event validation, matcher preparation and test-command payload construction; a production observation/dispatch producer was not located. These are source-audit findings, not a claim that every indirect callback path has been proved absent.

The existing `crates/cyber-app/tests/plugins.rs` suite uses actual Python plugin processes inside the OS sandbox and a scripted model through the App HTTP router. It covers tool forwarding, crash/restart, disable, network restrictions, settings/secrets, deferred tools and plugin MCP hosting. It is Unix-only. Its existing tests do not establish the new fs.read/shell boundary, runtime provider/hook dispatch or scoped prompt callbacks.

## Checks actually run

| Check | Observed outcome |
|---|---|
| Plugin SDK `npm run typecheck` | Passed |
| Plugin SDK `npm test` | Four tests passed, zero failed, one binary acceptance test explicitly skipped because `CYBER_BIN` was not supplied |
| `cargo test --locked -p cyber-app --test plugins` | Did not reach test execution: compilation failed with `No space left on device` in server/tools/app build artifacts |

After the failed build was confirmed terminal and no Cargo/rustc processes remained, only disposable `target/debug/incremental` compiler cache was removed. Free space recovered to approximately 3.2 GiB at observation. Binaries, dependencies, model artifacts, databases and project files were retained. This cleanup does not turn the failed Rust check into a pass; a passing native baseline is still required before implementation acceptance. No Linux/Windows or live-provider acceptance was run.

## Review boundary

Both workstreams await explicit user review of proposal/design/tasks. The early native enforcement prototype remains an approved-implementation task, not an already demonstrated design. This planning PR must not be treated as permission to implement or as closure of the larger extensibility milestone.


## Passing unchanged-main native baseline (2026-10-28)

A detached worktree at unchanged main `2ae7957d2223a843c929bc74b25162a9f300b419`
ran `cargo test --locked -p cyber-app --test plugins` using the shared target cache
and `CARGO_INCREMENTAL=0`. All 14 tests passed with no failures or ignored cases
on macOS 26.7 / Darwin 25.6.0 arm64. The same suite passed all 14 cases on integration
revision `70224077`. The earlier no-space result remains a failed compilation;
it is not retroactively counted as passing. Generated `cyber-tools` build cache
was cleaned to make this native execution possible, without changing the baseline
checkout. Both worktrees were clean after testing.

These tests run actual Python plugin processes through the App HTTP router and
OS sandbox. They exercise tool forwarding, crash/restart, disable, network and
policy intersections, protocol mismatch, deferred tools, scoped configuration,
dummy secrets through MemoryVault, and plugin MCP hosting. SDK/plugin-host/manifest
sources have no differences between the original baseline `097b6278` and this
main revision. The previously recorded SDK checks still include the explicit
binary-acceptance skip; the Unix-only suite supplies no Windows plugin support
proof. Strict native fs.read/shell enforcement and runtime contribution/callback
acceptance remain separate tasks.

Current producer inventory:

| Boundary | Main `2ae7957d` | Integration `70224077` |
|---|---|---|
| WorktreeCreate | Permission-following pre-effect veto includes isolated creation/recreation after merged #56; retained checkout reuse is excluded | Preserved with current-signature behavioral regression control |
| Setup | Existing observational first-root-Session dispatch; explicit init/durable-attempt design is specified but not implemented | Preserved; the separate Setup design review gate remains |
| CwdChanged | No committed-transition producer | Worktree entry/exit, idle return and changed-path child rebind; real scoped regressions |
| FileChanged | No automatic native producer | Bounded metadata reconciliation, native scope/receipt ownership and shared-membership cancellation; current final acceptance pending |
| DirectoryAdded | Requires the unimplemented P0 Additional working directories contract | Still open; declarations are not producer evidence |
| Runtime contributions | Initialize accepts tools and ignores nonempty runtime hooks/providers/catalogTransforms | Still open; these operations are not implemented by the producer integration |
| Session prompt callback | Capability check followed by method-not-supported | Still open; durable owned callback admission has not been implemented |

Regression anchors are the App plugin suite above, `worktree_setup.rs`,
`isolated_subagents.rs`, the new real `file_observation_dispatch.rs` suite,
server observation/provenance tests and AppHost forwarding tests. The parent
extensibility milestone and tasks 2–6 remain open: a producer declaration, SDK-only
fixture or this baseline cannot close unimplemented host capability promises.

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

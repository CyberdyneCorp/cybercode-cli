# Windows boundary audit

Source inspected: `4ceb12c`. This is a source audit, not a Windows compilation or enforcement result.

| Boundary | Current behavior | Required M1.1 work |
|---|---|---|
| Sandbox launch | `cyber-sandbox/src/lib.rs` reports enforcement unavailable outside macOS/Linux; explicit full access skips wrapping. | Add an isolated Windows launch interface with restricted token, AppContainer, scoped ACL lifetime and job-object ownership. Preserve failure behavior. |
| Linux helper binary | `cyber-sandbox-exec.rs` imports Unix sockets and Unix exit-status extensions unconditionally. | Gate this Linux-only executable or provide a platform-specific entry point before building the workspace on Windows. |
| Shell selection | Both app defaults and the bash tool fallback use `/bin/sh`; the tool recognizes Unix shells. | Define supported Windows shell invocation and quoting. Test paths containing spaces and metacharacters without widening permission classification. |
| Descendant cancellation | `bash.rs` kills a Unix process group; the non-Unix `kill_group` is empty. | Own all launched descendants in a job object and terminate them on cancellation/timeout, including full-access execution. |
| Service shutdown | `registration.rs::stop_service` invokes the external `kill` command. | Use an authenticated shutdown route or native process signaling with identity checks. Do not substitute unchecked PID termination. |
| Server signals | `server.rs` handles SIGINT/SIGTERM on Unix and Ctrl-C elsewhere. | Verify Windows service lifecycle and ensure orderly registration, lock and database cleanup. |
| Proxy reachability | Existing Windows proxy endpoint would be TCP loopback, while sandbox enforcement is unavailable. | Prove an AppContainer can reach only the intended proxy transport while direct outbound and other loopback connections remain blocked. A general loopback exemption is insufficient evidence. |

Native Windows tests must establish process-tree cleanup, allowed workspace mutations, denied outside reads/writes, credential isolation, proxy-only networking and fail-closed setup errors. Cross-compilation alone cannot establish these properties.

## Native compilation follow-up

A `windows-2025` CI job now checks the workspace and test compilation, builds the executables and smoke-tests `cyber --version`. Its first source baseline, `0f78a3f`, failed native compilation with three errors in the helper: unconditional `UnixStream`/`ExitStatusExt` imports and the unavailable `signal()` method ([CI run](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/37409971155)). Native results are tracked separately from enforcement proof. The Unix proxy bridge is now isolated behind a Unix-only module with a non-Unix entry point that refuses execution. Local macOS validation passes 12 sandbox/helper tests and workspace release Clippy, and direct helper checks retain argument rejection and child exit status. Native compilation and the Windows helper refusal test still need a successful Windows run. Restricted-token/AppContainer launch, scoped ACLs, native cancellation and proxy transport remain open.

The follow-up native check at `dbbb1e4` progressed past the helper and failed on the unconditional `serve_unix` re-export in `cyber-server` ([CI run](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/37410347434)). The Unix-only export and its `Path` imports are now gated consistently. TCP-only Windows registration and explicit rejection of unsupported Unix listener flags are under native lifecycle verification; Unix registration, lock and shutdown tests pass locally.

The native check at `44b4c1d` then reached the tool launcher and failed because its runtime platform condition still compiled a call to the Unix-only `Proxy::start_unix` ([CI run](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/37411373712)). The call is now compile-gated, with TCP as the supported non-Unix transport. A launcher-level test connects to the selected real listener and is included in Windows CI. This corrects compilation and tests transport selection; it does not establish AppContainer proxy reachability or outbound isolation. The macOS workspace tests and Clippy passed at `44b4c1d`.

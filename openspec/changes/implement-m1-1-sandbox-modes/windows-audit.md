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

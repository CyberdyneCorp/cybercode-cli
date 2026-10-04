# Design

## Decisions

**One host, many small tools.** `BuiltinHost` implements the runtime's `ToolHost`. Each tool is a small type with a definition and a `run` that receives a context. The context resolves paths, checks external directories through canonical ancestors so symlinks cannot hide an escape, and asks the permission engine. Permission decisions and replies flow through the runtime's request broker, so one mechanism serves every surface.

**The permission engine is pure.** `Policy::decide` evaluates in a fixed order:
1. Deny rules.
2. Explicit denies from user, global and command-line layers, which act as ceilings.
3. Plan mode.
4. Protected paths.
5. Saved approvals.
6. Mode effects.

It is tested table-style without I/O. Saved approvals can only turn `ask` into `allow`.

**Command analysis is advisory; the OS sandbox is the boundary.** Bash analysis with tree-sitter splits simple commands for prompts and `always` patterns and finds redirected targets outside the Location. Scripts can hide their targets, so the sandbox enforces writes, reads of credential files and network access whatever the analysis saw.

**Linux uses bubblewrap first.** Landlock is allow-only. It cannot keep `.git` read-only inside a writable root or hide `~/.ssh` under a readable home, and the sandbox spec requires both. Bubblewrap expresses both with bind mounts and isolates the network with a namespace. In proxy mode, `cyber-sandbox-exec` runs inside that namespace and forwards a loopback port to the proxy's Unix socket. Containers that mask `/proc` get the host's `/proc` read-only instead of a new PID namespace. Without bubblewrap, execution fails closed with `SandboxUnavailableError`. A Landlock fallback with documented weaker guarantees is deferred.

**macOS uses deny-default Seatbelt.** Reads are allowed everywhere except credential paths. Writes are allowed under the roots, minus protected paths, because later rules win. Mach services are a short allowlist for user lookup, TLS trust, logging and preferences. The network reaches only the proxy port. macOS tools such as `mktemp` use the per-user temporary directory even when TMPDIR is set, so that directory is writable.

**Network approval reuses the permission flow.** The proxy calls back through a channel into the running bash call, which asks `network` for the domain with the call's own permission context. An approval holds for the Session, and `always` saves it.

**Snapshots live outside the repository.** Each worktree gets a shadow git directory under `<data>/snapshot/<project>/<sha1(worktree)>`, borrowing the project's objects through alternates. `git add -A` plus `write-tree` records a tree without a commit. The user's `info/exclude`, `snapshots.ignore` and oversized untracked files are excluded.

**Restore plans everything before writing.** For each path that differs between the recorded tree (the last state the system saw) and the target, the planner compares the current bytes:
- Unchanged files are replaced.
- Files edited since are merged three-way with `git merge-file`.
- Anything else is a conflict, and the whole restore fails without writing.

Before writing, bytes are rechecked to catch races and replaced bytes are backed up.

**Revert is event-sourced.** Stage, clear and commit are `session.reverted.1` events folded into state. Commit removes the boundary message and everything after it, keeps unsettled and unknown-outcome calls for recovery, drops a compaction whose tail was cut, and rebuilds task state. The next prompt or compaction commits a staged revert first.

**Reconciliation is conservative.** Without a recorded pre-image, a `write` whose content differs proves nothing, so it stays unknown. `edit` and `apply_patch` report success or absence only when the file unambiguously shows one or the other, and shell commands are never reconciled.

## Tradeoffs

Snapshots shell out to `git`, which costs a process per operation but reuses git's index cache and ignore handling exactly. The proxy handles one destination per client connection, which suits proxy-aware HTTP clients. Running everything through bubblewrap requires unprivileged user namespaces. Ubuntu 24.04 allows them for `bwrap` through its AppArmor profile.

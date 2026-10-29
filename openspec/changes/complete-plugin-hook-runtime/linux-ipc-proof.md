# Linux host UNIX-socket diagnostic

This experiment addresses the outstanding native IPC intersection in tasks
2.1–2.3. It invokes the production `LinuxPolicy` composer and packaged native
helper, with shell disabled and every read/write combination. It does not change
the helper ABI floor, production confinement, support decisions or host activation.

The owner creates three disposable pathname UNIX listeners: outside Location,
inside public Location and under a masked private directory. A package symlink
also targets the outside listener. A compiled native client attempts to connect
and send the fixed `fixture-only` token. The host accepts the connection and
verifies those exact bytes; a client-reported successful connect alone is not
sufficient evidence. Each route has an unrestricted direct positive control.
Project-file reads must independently match the read capability. The private
directory mount must hide its socket. Every process/receive has a deadline;
only owned fixture endpoints are contacted and the temporary tree is removed.

The public/outside/alias observations are diagnostic, not assertions that host
access is acceptable. In particular a successful diagnostic job cannot certify
network isolation. No real broker request, secret access or filesystem/shell
escalation is performed. The JSON records the precise source hash, helper hash,
checkout and run plus each observation. Source provenance can be independently
recomputed from the checkout; binaries are not retained by this workflow.

Native x64/arm64 CI execution is required. Local macOS compilation and C syntax
checks cannot prove the Linux behavior. If the fixture proves host reachability,
the resulting support/design decision and a runtime-compatible enforcement
mechanism remain required before production activation; the existing ABI-3
filesystem rights and private IP namespace must not be credited with a UNIX
socket denial they do not demonstrate.

Local verification: the example passes Rust Clippy with warnings denied, the C
fixture passes `-Wall -Wextra -Werror` compilation and a direct native macOS
connection/token-receipt check, and workflow YAML plus its three shell blocks
parse. Formatting, diff checks, all 107 strict OpenSpec items and spec lint
(zero errors, 25 warnings) pass. The C build reports two deprecated Seatbelt SDK
calls; their origin is not assigned. Linux execution remains pending.

The scoped code/security review inspected the example, native fixture, workflow
and task/docs changes. The client accepts only an explicit pathname and project
file from the owner; the experiment contacts owned ephemeral listeners only.
The private directory negative control and independent project-read assertion
prevent a failed bootstrap or broad read from being credited as an IPC result.
This review finds no additional source issue in the diagnostic; it does not
approve the unresolved production IPC/network boundary.

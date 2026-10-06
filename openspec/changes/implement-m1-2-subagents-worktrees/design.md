## Design
Use shared typed settings and a validated worktree-name type so CLI, API and tools cannot disagree on defaults or accept path traversal. Resolve settings only after configuration layering, substitutions and workspace trust; project setup commands remain gated. This first stage does not execute Git or setup commands.

Managed lifecycle operations will serialize per repository using the specified cross-process lock and retain ownership records linking roots, base revisions, branches and sessions. Creation must account for cancellation and cleanup without removing user work. Copy inclusion must honor Git ignore syntax and never overwrite tracked paths. Setup executes through existing sandbox/process ownership. Removal refuses active sessions; auto cleanup checks dirty and ahead state.

Child-session orchestration will use existing durable admission and drain ownership rather than an independent model loop. Permissions are inherited ceilings; a concurrent permit is acquired before creating a child and released on every termination route. Structured output validates the supplied schema and allows exactly one reprompt. Background handback uses the durable parent inbox, and forks take committed parent context through the spawning point. Worktree cleanup and cost attribution remain owned through settlement.

## Verification
Shared configuration/naming tests are groundwork, not proof of worktree isolation. Integration gates require real temporary Git repositories, concurrent processes, dirty/ahead preservation, session ownership, sandboxed setup, and child-runtime tests with deterministic providers. Native platform execution is required for subprocess and file-lock behavior. Keep M1.2 open until all product surfaces and behavioral contracts pass.

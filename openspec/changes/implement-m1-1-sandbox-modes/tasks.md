- [ ] Track P0 local baseline/long-task evidence separately; the six-platform CI matrix passes. This does not block authorized P1 implementation.
- [ ] Audit Windows compilation, shell, process cancellation and service lifecycle boundaries.
  Source audit is recorded in `windows-audit.md`; Windows compilation and native lifecycle verification remain pending.
- [ ] Implement restricted-token AppContainer launch, scoped ACL grants and job-object cleanup.
- [ ] Prove Windows filesystem and credential isolation with native runtime tests.
- [ ] Verify proxy transport/enforcement on Windows and extend domain approval/timeout tests.
- [ ] Implement strict evaluator classification and durable decision events with usage accounting.
  - [x] Tool-free evaluator runtime, strict bounded replies, durable event, hidden usage and per-Drain block tracking. Verified by 11 runtime tests; the full workspace, Clippy and OpenSpec checks pass.
  - [ ] Host integration after protected/irreversible ceilings; config rules, overrides and statistics.
  - [x] Bash literal-removal core, sh/bash -c and eval, symlink/parent aliases, unresolved targets and manual warning UI; individual-confirmation cascade regression.
  - [ ] Complete remaining Bash shell-context cases (wrapper options, heredocs and variable bindings) before accepting full critical-removal coverage.
    `env`/`exec` option values, command lookups and `find` global options are covered; unsupported/dynamic wrappers refuse automatic approval. Direct/wrapped shell heredocs and here-strings, including forwarding to a shell in a pipeline, now have regression coverage. Expansion and dynamic command names require manual review. Literal scalar bindings, dynamic command names and function invocation contexts now have regression coverage, with subshell isolation and conservative branch/loop mutation handling. Advanced bindings, additional wrappers and remaining shell-flow cases are still open.
  - [x] Initial Python/JavaScript inline AST removal guards, literal/API alias/path proof, nested eval/shell source and fail-closed unknown dispatch.
  - [ ] PowerShell, additional interpreter/stdin contexts, higher-order dispatch, module-loading trust and protected/irreversible inline mutation facts before classifier approval is enabled.
- [ ] Test allow/block, irreversible actions, malformed output, three-block fallback and unattended denial.
- [x] Complete the four-mode cycle and pending/effective TUI state with tests. Turn modes are pinned durably through tool settlement, with legacy replay, rapid-selection serialization and TUI bypass confirmation.
- [ ] Enforce org-policy mode restrictions; the full permission-mode requirement remains open.
- [ ] Add Windows CI; run existing platform, recovery and trust regressions.
  - [x] Add a native Windows workspace/test-compilation and executable smoke job.
  - [ ] Prove Windows compilation, sandbox enforcement and native lifecycle/recovery behavior.
- [ ] Update roadmap and capability specs with verified M1.1 behavior.

- [x] Prove literal filesystem commands for accept-edits; preserve ordinary asks for unresolved paths/options and protected-path ceilings for symlink aliases.
- [ ] Extend accept-edits proof to recursive copies, directory moves and remaining filesystem option semantics before closing the full requirement.

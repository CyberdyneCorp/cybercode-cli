## 1. Permission engine and host

- [x] 1.1 Rules, defaults, Modes, protected paths, ceilings, saved approvals; table tests.
- [x] 1.2 Request broker in the runtime: permission and question events, cascades, unattended replies, doom-loop check.
- [x] 1.3 `BuiltinHost`: schema validation, output budget, Turn visibility, external directories with symlink checks, plan mode.
- [x] 1.4 Bash command analysis with tree-sitter.

## 2. Tools

- [x] 2.1 read, write, edit, apply_patch, list, glob, grep with stale checks and write serialization.
- [x] 2.2 bash: process groups, timeout, cancellation, output capture with overflow file.
- [x] 2.3 todo, question, history_search, plan_enter, plan_exit.
- [x] 2.4 webfetch (formats, size limit, Cloudflare retry, prompt summaries) and websearch (seven backends, rotation, cooldown, domain filters).
- [x] 2.5 Skill discovery, `skill` tool, argument substitution, `core/skills` source, readable skill directories.
- [ ] 2.6 Native provider web search, media parts for images and webfetch, background bash (deferred).

## 3. Sandbox

- [x] 3.1 Policy resolution, credential masking, allowlist proxy with `network` requests.
- [x] 3.2 macOS Seatbelt enforcement with tests.
- [x] 3.3 Linux bubblewrap enforcement and proxy bridge, verified in a Linux container.
- [ ] 3.4 Landlock fallback for hosts without bubblewrap (deferred).

## 4. Snapshots and recovery

- [x] 4.1 Project identity from git.
- [x] 4.2 Shadow repositories, tracking, diffs, conflict-aware restore with backups, GC.
- [x] 4.3 Per-Turn snapshots, diff summaries, three-phase revert, auto-commit, busy guard.
- [x] 4.4 Read-only reconciliation for write, edit and apply_patch.

## 5. Verification and docs

- [x] 5.1 Unit, integration and end-to-end flow tests; Linux and macOS sandbox suites.
- [x] 5.2 Spec deltas, README and ROADMAP status.

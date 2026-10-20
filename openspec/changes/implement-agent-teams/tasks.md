Specified 2026-10-18. Sections 1–9 implemented and accepted by hand on 2026-10-20 (see `docs/implementation/p2-status.md`, "Agent teams"). Implemented after `implement-cross-session-messaging` (hard dependency: the resolver's team tier, `Messaging::send`, watches and `xsession_message`). Sections 1–8 implement the P2 requirements; section 9 is the end-to-end acceptance. P3 "Distributed teammates" and P4 "Teammate panes" are out of scope.

Each numbered section is one increment that ships working behavior with its own tests. Every test runs locally, with no network and no real provider:
- runtime tests use `crates/cyber-server/tests/support` and the `ScriptedAdapter`, in a new `crates/cyber-server/tests/teams.rs`;
- tool tests use the scripted models of `crates/cyber-tools/tests`, in a new `crates/cyber-tools/tests/team_tools.rs`;
- production-path tests use the App over authenticated TCP with the OpenAI-compatible scripted provider of `crates/cyber-app/tests/support/scripted_model.rs`, in a new `crates/cyber-app/tests/teams.rs`;
- CLI tests run the real `cyber` binary, in a new `crates/cyber-cli/tests/teams_cli.rs`;
- TUI tests use `TestBackend` and `insta` against a real server, in a new `crates/cyber-tui/src/screens_team.rs`.

The production-path test named in each section must pass before the section is ticked.

## 1. Feature registry, team records and definitions

- [x] 1.1 `crates/cyber-core/src/features.rs`: the registry (`teams`, experimental, off), resolution order of design section 1, `ConfigInvalidError: unknown feature "<name>"`, the `experimental.<name>` alias with its warning, `policy.features.lock`. Unit tests for each source and the lock.
- [x] 1.2 `--features` global flag, `CYBER_FEATURES`, `cyber features list [--json]`, `cyber features enable|disable <name> [--scope user|project]`, `GET /api/v1/features`.
- [x] 1.3 Migration `20261025000000_teams.sql` (design section 2) with a `cyber-store` migration test on a database holding P1 data; `team.*` events registered with projectors; `SessionInfo.team`; a rebuild test (drop the projections, re-project, identical rows).
- [x] 1.4 `crates/cyber-core/src/teams.rs`: definitions from `.cyber/teams/*.md` (trust-gated) and `~/.config/cyber/teams/*.md`, project wins on a name clash; field validation naming the field; `runner` rejected; `cyber teams definitions` and `GET /api/v1/team-definitions`.
  - Production path: `crates/cyber-cli/tests/teams_cli.rs` `features_list_shows_teams_off_and_unknown_feature_fails` (configuration scenarios "List features" and "Unknown feature rejected") and `team_definitions_load_with_project_precedence_and_name_the_invalid_field`.

## 2. Team creation, spawning, isolation and permissions

- [x] 2.1 `runtime/teams/mod.rs` and `spawn.rs`: create (definition or ad-hoc), `team_instructions` system message to the lead, teammate root Sessions with the `team` tag and `<team>/<member>` title, the size check under the spawn lock (`Team size limit reached (6)`, `teams.max_teammates`), `Teammates cannot lead a team`.
- [x] 2.2 Isolation default (worktree when two or more editing teammates) and team worktree naming `team-<ulid>-<name>` on `cyber/team-<ulid>/<name>` in `worktrees/child.rs`.
- [x] 2.3 Permissions ceiling: effective Mode is the stricter of the teammate's and the lead's at each Turn start; lead deny rules inherited; `Mode limited by the team lead (<mode>)`.
- [x] 2.4 `team_spawn` tool (`team.manage`), advertised only when `teams` is on and the caller is a lead or has no team; the `core/teams` context source for members.
- [x] 2.5 Feature off: no team tools, team routes `404 FeatureDisabledError`, `/team` message.
  - Production path: `crates/cyber-app/tests/teams.rs` `lead_spawns_three_teammates_in_worktrees_and_the_seventh_is_refused` (scenarios "Spawn teammates", "Teammate limit", "Disabled by default", and "Teammate approval prompt" ceiling half: a lead in `plan` mode → a teammate's `edit` is denied).

## 3. Shared task list and atomic claims

- [x] 3.1 `runtime/teams/tasks.rs`: create/list/get/update in one transaction on the lead's runtime, the dependency rule (`Task depends on unfinished task_<id>`), cycle rejection, `expected_version`, lead-only reassignment and cancel.
- [x] 3.2 Claims: `Task already claimed by <name>`; a concurrency test with two members claiming one task from two threads, exactly one success, repeated 50 times.
- [x] 3.3 Tools `team_task_create|list|get|update` (`team.task`), strict schemas.
- [x] 3.4 `TaskCreated`/`TaskCompleted` for team tasks with `{ task, team_id, member }` and subject `team_task`; Job payloads unchanged (regression test).
  - Production path: `crates/cyber-app/tests/teams.rs` `teammates_share_a_board_where_dependencies_block_and_claims_are_exclusive` (scenarios "Dependency blocks start", "Concurrent claim" with two gated scripted Turns released together, "Hook on task completion" with a recording hook).

## 4. Team messaging and idle notifications

- [x] 4.1 The messaging resolver's team tier: `lead`, `all`, member names first for team callers; `all` fan-out with one record and outcome per target; the implicit same-team `deliver`.
- [x] 4.2 `runtime/teams/idle.rs`: system watches on active teammates, the three notice texts of design section 7, `team.member.idle.1`, `msgx_team_idle_…` IDs, startup recovery, no notices while stopping.
- [x] 4.3 `TeammateIdle` hook producer with its payload; decisions logged only.
- [x] 4.4 Member status (`working`, `waiting_input`, `idle`, `stopped`) and last status line via `team.member.updated.1`.
  - Production path: `crates/cyber-app/tests/teams.rs` `a_teammate_asks_the_lead_and_reports_idle_when_done` (scenarios "Teammate asks lead": `ui` sends `which API shape should I use?` to `lead`, the lead's next request contains it and it replies to `ui`; "Teammate finishes": the lead receives `tests is idle: all assigned tasks done`; a `TeammateIdle` hook records the payload; `all` reaches every non-stopped teammate once).

## 5. File ownership and team_merge

- [x] 5.1 `runtime/teams/ownership.rs` in permission evaluation for edit/write/patch and bash write targets: `Path owned by teammate <name>` in teams with a shared member.
- [x] 5.2 `runtime/teams/merge.rs` and `team_merge`: dirty-worktree and nothing-to-merge refusals, `git merge-tree --write-tree` conflict detection with the path list, `--no-ff` merge with hooks disabled, the git version check.
  - Production path: `crates/cyber-app/tests/teams.rs` `ownership_denies_cross_owner_edits_and_merge_reports_conflicts_without_touching_the_lead` (scenarios "Ownership enforced" and "Conflict reported": two teammates edit the same line in their worktrees, the lead merges one cleanly, the second merge lists the file and the lead's HEAD and status are unchanged).

## 6. Team budgets, stop, resume and recovery

- [x] 6.1 `runtime/teams/budget.rs`: the team scope over `runtime/budget.rs`, spend summed over members since creation, `check_team` at Turn start, `team.budget.exceeded.1` once, the spend breakdown notice to the lead.
- [x] 6.2 Stop (`stopping` → `stopped`, reasons `user`, `budget`, `lead_deleted`), resume (`team.resumed.1`, re-armed watches, restarted Drains for members with open work, `--lead` after `lead_deleted`), lead deletion.
- [x] 6.3 `runtime/teams/recovery.rs`: finish stopping, re-arm watches, recover idle notices; a restart test.
  - Production path: `crates/cyber-app/tests/teams.rs` `a_team_over_budget_stops_and_resumes_with_its_context` (scenarios "Budget exceeded" with a priced scripted model and `max_cost_usd`; "Resume next day": stop, rebuild the App over the same data directory, resume, a teammate's next request contains its prior history and the board is intact).

## 7. HTTP API, SDK and CLI

- [x] 7.1 `http/teams.rs` routes of design section 14 with error tags, OpenAPI registration, `scripts/generate_sdk.py` and SDK tests.
- [x] 7.2 `cyber teams list|show|stop|resume` (table and JSON).
  - Production path: `crates/cyber-cli/tests/teams_cli.rs` `teams_show_lists_members_tasks_cost_and_last_message` (the real binary against a server it starts; `stop` then `resume` round trip).

## 8. TUI team view and commands

- [x] 8.1 `/team start|stop|resume` and `/team`, in `BUILTIN_COMMANDS`.
- [x] 8.2 `view/team.rs`: members table and task board by status, owners, `enter` opens a teammate Session, `esc` back to the lead, live refresh from `/teams/:id/events`; sidebar `Team` section.
- [x] 8.3 Labelled teammate requests (`[tests] Allow bash: docker compose up?`) answered from the lead's TUI.
- [x] 8.4 Direct prompts to an opened teammate admitted with `steer` to that Session only.
  - Production path: `crates/cyber-tui/src/screens_team.rs` snapshots at 120×40 and 80×24 of the team view and board (scenario "View task board"), the labelled request (scenario "Teammate approval prompt"), and a test that a prompt typed in teammate `api` reaches only `api` (scenario "Direct instruction").

## 9. End-to-end acceptance

- [x] 9.1 With the real `cyber` binary, a real `cyber serve` and a scripted local provider, in a pty captured with pyte to PNG: enable `teams`, `/team start feature` from `.cyber/teams/feature.md` (api, ui, tests), tasks with a dependency, a concurrent claim, a teammate question to the lead, an idle notice, an ownership denial, a merge with a conflict, a budget stop, `/team resume`, and `cyber teams show`. Record every scenario with pass/fail in `docs/implementation/` and update ROADMAP M2.5.
  - Recorded 2026-10-20: 15 scenarios, all pass, two bugs fixed with regression tests (idle notices drawn as notice lines in the lead's TUI; `cyber teams list|show` column widths). The driver was scripted by hand against the real binary and is not part of CI.

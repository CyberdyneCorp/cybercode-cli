## 1. CYBER.md through every instruction path

- [x] 1.1 Add `runtime/instructions.rs` with the single ordered list of file names (`DIRECTORY_FILES`, `GLOBAL_FILES`, `OVERRIDE_FILE`), `directory_files`, `load_baseline` and `load_nested`. Make `context.rs` delegate `core/instructions` to it.
- [x] 1.2 Loader semantics for every file, including `CYBER.md`: canonical-path deduplication, `@path` imports (code fences skipped, depth 5, cycle warning naming both files, project imports confined to the project root), truncation at 32768 bytes with a notice, `AGENTS.override.md` replacing `AGENTS.md`/`CLAUDE.md`/`CYBER.md`, `CYBER_DISABLE_CLAUDE_MD`/`CYBER_DISABLE_CLAUDE_COMPAT` and `CYBER_DISABLE_PROJECT_CONFIG`.
- [x] 1.3 Nested on-read discovery: the runtime records a built-in `read`'s target, attaches unloaded instruction files in a `<system-reminder>` at settlement, and persists `ToolSettled.instruction_reminders` into `Epoch.reminded_instructions` (once per file per Context Epoch).
- [x] 1.4 `cyber doctor` `instructions` check: files in load order, warning on truncation and import problems.
- [x] 1.5 Unit tests: per-directory order, cross-directory order, global `CYBER.md` after global `AGENTS.md`, only `CYBER.md`, `CYBER_DISABLE_CLAUDE_MD` keeps `CYBER.md`, `CYBER_DISABLE_PROJECT_CONFIG` keeps only global, symlink dedup, import cycle, out-of-project import refused, truncation notice, override replaces `CYBER.md`, and nested discovery once per epoch.
- [x] 1.6 Production path: a cyber-app test with a scripted local model checks the captured request bodies for global then project `CYBER.md` after `AGENTS.md` (root and nested Location), the nested read reminder sent once, a mid-session `CYBER.md` edit arriving as one durable update with an unchanged prefix, and `GET /sessions/{id}/context` naming `CYBER.md`. `AGENTS.md` + `CLAUDE.md` + `CYBER.md` in one directory sends `AGENTS.md` then `CYBER.md` (and `CLAUDE.md` then `CYBER.md` once `AGENTS.md` is gone), in the request and in `cyber doctor`. A `cyber doctor` binary test lists `CYBER.md` and warns on truncation.
- [x] 1.7 Acceptance with the real debug `cyber` binary (`exec --embedded --ephemeral`, temporary `HOME`, scripted local provider): root-only, nested Location, both switches, `@path` import, nested on-read reminder and `cyber doctor` order. Recorded in `docs/implementation/p1-status.md`. The global file was exercised through `HOME` only, not through `XDG_CONFIG_HOME`/`CYBER_CONFIG_DIR`.

## 2. Docs, `/init` and import compatibility (increment 2)

- [x] 2.1 Document `CYBER.md` in `README.md` (Project instructions row), `docs/features.md` and `docs/local-core-guide.md` (how to write one, load order and precedence, switches, nested discovery, `cyber doctor`, and how it relates to `AGENTS.md` and `CLAUDE.md`), listing what is not implemented yet.
- [x] 2.2 Record the `cyber import` decision (no `CLAUDE.md` → `CYBER.md` target) and the `/init` decision (keeps generating `AGENTS.md`; no `/init` exists yet) in `design.md`.
- [x] 2.3 Unit test for the import depth limit of 5, so the documented limit is covered.
- [x] 2.4 Evidence entry in `docs/implementation/p1-status.md`.

## 3. Review fixes (fail closed)

- [x] 3.1 Nested reminders never crash a read: at most 32768 bytes of text per reminder with the rest named, a path-list fallback when the reminder exceeds the tool output budget, and no reminder (files left unrecorded) when even that does not fit. Unit tests in `drain::skill_reminder_tests`; production `two_full_size_nested_files_still_give_a_successful_read`.
- [x] 3.2 `compat.claude` per Location for the runtime (`ToolHost::claude_compat`, forwarded by AppHost) and doctor, through `cyber_core::config::claude_compat`. Tests: `a_project_compat_claude_false_keeps_its_claude_md_out_of_the_request`, `doctor_honors_a_project_compat_claude_false`, forward guard.
- [x] 3.3 `InstructionsLoaded` (`load_reason: nested_read`) for nested attachments. Test: `nested_instruction_files_fire_instructions_loaded`.
- [x] 3.4 Bounded reads of regular files only, nested loading only for successful reads, off the async workers and outside the Session lock. Tests: `special_files_are_never_read`, `reads_are_bounded_by_the_limit`, `only_successful_reads_load_nested_instructions`.
- [x] 3.5 Directory files confined to the project root like imports; `read_target` normalized lexically like the read tool. Tests: `project_files_symlinked_outside_the_project_are_not_loaded`, `a_read_through_a_symlinked_directory_attaches_nothing_from_outside`, `instruction_files_symlinked_outside_the_project_never_reach_the_model`, `doctor_warns_about_an_instruction_file_outside_the_project`.
- [x] 3.6 Total baseline budget (131072 bytes). Test: `the_baseline_stops_at_its_total_budget`.
- [x] 3.7 Project instruction files and imports pass the Location's `read` rules (`ToolHost::instruction_read_gate`, forwarded by AppHost; doctor builds the same gate); a config that fails to load refuses every project file and turns `CLAUDE.md` off. Tests: `the_read_gate_refuses_project_files_and_imports_but_not_global_files`, `env_files_ask_by_default_and_a_deny_rule_refuses`, `a_config_that_fails_to_load_turns_claude_md_off_and_refuses_project_files`, `instruction_files_never_import_a_file_the_default_read_rules_ask_about`, `instruction_files_never_import_a_file_a_read_deny_rule_covers`, `doctor_refuses_protected_imports_and_project_files_under_a_broken_config`, forward guard.

## 4. Follow-ups

- [ ] 4.1 Make `instructions.max_bytes` configurable once the config schema has a home for it (the `instructions` key is an array).
- [ ] 4.2 Show per-source token sizes, including `CYBER.md`, in `/context` (the `Context inspection` requirement).

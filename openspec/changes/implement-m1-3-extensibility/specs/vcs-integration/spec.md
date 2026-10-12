## MODIFIED Requirements

### Requirement: Local review command
(P1) The system SHALL provide `/review [target]` and `cyber review [target]`. The target is uncommitted changes (default), `<base>..<head>`, a commit SHA, or `pr <number>`. In P1 the review SHALL run the bundled `review` skill (`skills-commands`) in a read-only `reviewer` subagent. From P2 it SHALL instead run the bundled `review` workflow (`workflows`), which checks correctness, security, tests and maintainability in parallel and verifies each finding before reporting it. In both cases results SHALL be reported as structured findings with `file`, `line`, `severity` (`critical`, `high`, `medium`, `low`), `summary` and `failure_scenario`.

The target grammar SHALL be: empty (uncommitted changes: staged, unstaged and untracked, against HEAD or the empty tree when HEAD is unborn), `<base>..<head>` (both sides required), a single revision (that commit against its first parent, or the empty tree for a root commit), or `pr <number>` (a positive decimal integer). A revision SHALL contain only `[A-Za-z0-9._/~^-]`, SHALL NOT start with `-` or `/`, and SHALL NOT contain whitespace, control characters, `:`, `@{` or `..`. Revisions SHALL be resolved to commit object IDs by a read-only library engine in the sandboxed inspection helper, which never runs git hooks, external diff, textconv, filter drivers or a pager. User text SHALL never be passed as a process argument. Because the engine's configuration is isolated, the host SHALL pass it a copy of the user's global excludes file (`core.excludesFile` from `$XDG_CONFIG_HOME/git/config` or `~/.gitconfig`, else `$XDG_CONFIG_HOME/git/ignore`), so the untracked files reviewed are those `git status` shows. A repository's own `core.excludesFile` SHALL replace the global one, as in git. An excludes file larger than 64 KiB SHALL refuse the review as unavailable rather than be truncated, since truncation would unhide ignored files.

The patch sent to the reviewer SHALL be capped at 256 KiB, spent in path order only on files that are not withheld, and truncated or binary files SHALL be listed in `limitations`. The bundled review child SHALL be given the findings JSON Schema as its structured-result schema: `findings[]` of `{file` (repository-relative), `line` (integer >= 1), `severity` (`critical|high|medium|low`), `summary`, `failure_scenario}`, plus `limitations[]`, with no additional properties. The schema SHALL state every rule consumers enforce (a `file` with no leading `/` or `\`, drive prefix, `..` segment, control or bidirectional character, and a `summary` and `failure_scenario` with a non-whitespace character), so a result consumers would reject fails `return_result` and gets the corrective retry; an integral `line` such as `12.0` SHALL be accepted as that integer. Consumers SHALL re-validate findings and order them by severity (critical, high, medium, low), then file, then line, then summary.

Before reading, the system SHALL deny the review when an `external_directory` rule denies the repository root outside the Location. It SHALL withhold the content of each changed path whose mode-aware `read` decision, or `external_directory` decision when the path is outside the Location, is anything but allow (so `ask`, including the default `ask` for `*.env` and `*.env.*`, withholds as `deny` does), and list it in `limitations`. These decisions SHALL be the reviewer's own, under its read-only Plan ceiling and immutable rules, so an invoking Session in `bypass` or another permissive mode SHALL NOT widen what is sent. When every changed path is withheld, or paths were withheld and no remaining file has patch text, nothing is reviewable and the review SHALL be refused with `all_withheld`, never reported as an empty diff. Target resolution is a host step, not a tool call: `PreToolUse` hooks do not run for it, so paths that must never reach the reviewer SHALL be protected with `read` permission rules. The reviewer Job itself SHALL be subject to the hooks of the invoking Location: its launch to `SubagentStart` (matched on the `reviewer` agent), each of its tool calls to `PreToolUse` and `PostToolUse`, and its handback to `SubagentStop`. A `SubagentStart` deny SHALL refuse the review before any reviewer Job or provider request, and a `PreToolUse` deny SHALL reach the reviewer as a tool error, even when the invoking Session is in `bypass`.

`pr <number>` SHALL be resolved only by the permission-checked command `gh pr view <number> --json baseRefOid,headRefOid,url`, run in the command sandbox. `gh` SHALL be looked up on, and run with, a `PATH` made only of absolute entries outside the project root, so a repository cannot plant `gh` or the `git` it runs. It SHALL review the changes from the merge base of the base and head commits to the head commit (as the pull request's own diff does), using only commits already present locally, and SHALL NOT fetch, check out or comment. A plain `<base>..<head>` target SHALL diff the two commits directly.

Resolution failures SHALL be reported before admission as `ReviewTargetError` (HTTP 422) with `code` one of `invalid_target`, `invalid_ref`, `unresolved_ref`, `not_a_repository`, `empty_diff`, `all_withheld`, `gh_unavailable`, `pr_unresolved`, `pr_commits_missing`. Permission denials SHALL be reported as 403. In either case no subagent SHALL start. An invalid structured result SHALL get one corrective retry and then fail with `SchemaMismatch`.

A discovered `review` skill or a configured `review` command SHALL take precedence over the bundled skill as in `skills-commands`. It runs as defined, without target resolution or a findings schema.

A retried admission with the same message ID, command name, arguments and delivery SHALL return the original receipt without re-resolving the target.

In the TUI, `/review [target]` SHALL validate the target with the same grammar before posting when the bundled skill is the winning source, and SHALL show an invalid target in the status line without admitting anything; an overriding `review` SHALL receive its arguments unchanged. The TUI SHALL post a bundled review with a client-chosen message ID, so it knows the reviewer fork's delegation ID, and SHALL treat as the review only the Job that delegation names; other Jobs ending meanwhile SHALL NOT be reported as the review or end it. A `ReviewTargetError`, 403 or any other failed post SHALL be shown as a status line with the server message and SHALL create no task. When the reviewer Job completes with a result that passes consumer validation, the TUI SHALL show the findings most severe first with control characters escaped; a failed Job, an invalid result, or a fork that failed before its Job existed SHALL show the error. The last case ends no Job, so the TUI SHALL look up the delegation when the server admits its `msg_skill_handback_<delegation>` input to the Session.

#### Scenario: Review uncommitted changes
- **WHEN** the user runs `/review` with 4 modified files
- **THEN** the review runs over those 4 files and the TUI shows the findings, most severe first

#### Scenario: Review a range
- **WHEN** the user runs `cyber review main..HEAD`
- **THEN** the reviewer receives the diff between the resolved commits of `main` and `HEAD`, and the command waits for the reviewer Job

#### Scenario: Findings ordering
- **WHEN** the reviewer returns findings low `b.rs:3`, critical `z.rs:9`, high `a.rs:20`, high `a.rs:4`
- **THEN** they are reported as critical `z.rs:9`, high `a.rs:4`, high `a.rs:20`, low `b.rs:3`

#### Scenario: Review a commit
- **WHEN** the user runs `/review HEAD~1`
- **THEN** the review covers that commit's changes against its first parent

#### Scenario: Invalid finding rejected
- **WHEN** the reviewer returns a finding with severity `blocker` or line `0` twice
- **THEN** the review Job fails with `SchemaMismatch` and `cyber review` exits 1

#### Scenario: Empty diff
- **WHEN** the user runs `cyber review` with a clean working tree
- **THEN** it prints `No changes to review`, exits 0, and no subagent or Job is created

#### Scenario: Invalid ref
- **WHEN** the user runs `cyber review -- --output=/tmp/x` or `/review HEAD@{1}`
- **THEN** the target is rejected with `invalid_ref`, nothing is admitted and no file is written (`cyber review` exits 2)

#### Scenario: TUI ignores unrelated Jobs during a review
- **WHEN** an `@explore` child Job fails while `/review` is waiting for its reviewer
- **THEN** no review failure is shown, and the reviewer Job's own outcome is shown when it ends

#### Scenario: TUI empty diff
- **WHEN** the user runs `/review` with a clean working tree
- **THEN** the status line shows `No changes to review` and no task is listed

#### Scenario: Read rule respected
- **WHEN** `.env` is modified and a `read` rule denies `.env`
- **THEN** its content is not sent to the reviewer and `limitations` names it

#### Scenario: Ask withholds like deny
- **WHEN** `.env` and `config/.env.production` are modified under the default rules, where their `read` is `ask`
- **THEN** neither content is sent to the reviewer, both are named in `limitations`, and no permission prompt is raised

#### Scenario: Bypass invoker does not widen the reviewer
- **WHEN** a Session in `bypass` mode runs `/review` with a modified `.env` under the default rules
- **THEN** `.env` is withheld and named in `limitations`, because the reviewer's own `read` decision is `ask`

#### Scenario: Every change withheld
- **WHEN** the only change is `.env.production` under the default rules and the user runs `cyber review --format json`
- **THEN** the review is refused with `all_withheld`, `findings` is `null`, no Job is created and the command exits 2

#### Scenario: Schema matches the consumer
- **WHEN** the reviewer calls `return_result` with a finding whose `file` is `/repo/src/a.rs` or whose `summary` is only whitespace
- **THEN** the call fails schema validation and the reviewer gets the corrective retry, instead of a completed Job whose findings consumers reject

#### Scenario: Reviewer refused at delegation admission
- **WHEN** an `agent` rule denies `reviewer` and the user runs `cyber review` or `/review` with changes
- **THEN** the fork fails before any Job exists; `cyber review` exits 1 with `reviewed: false` and the delegation's error, and in the TUI the handback admission prompts a delegation lookup and the status line shows that error

#### Scenario: Withheld files do not spend the patch budget
- **WHEN** a read-denied `assets/data.json` with a 300 KiB change sorts before a changed `src/main.rs`
- **THEN** the reviewer receives the full `src/main.rs` patch, and the withheld file is named in `limitations`

#### Scenario: Global excludes respected
- **WHEN** `~/.config/git/ignore` lists `*.pem` and the repository has an untracked `deploy.pem`
- **THEN** `deploy.pem` is neither listed nor sent to the reviewer

#### Scenario: Oversized global excludes refused
- **WHEN** the user's global excludes file is larger than 64 KiB and the user runs `/review`
- **THEN** the review fails as unavailable, naming the file and the limit, and nothing is admitted

#### Scenario: Project-planted gh ignored
- **WHEN** the repository contains an executable `gh` on `PATH`, or `PATH` has a relative entry holding one, and the user runs `/review pr 7`
- **THEN** neither runs and the review fails with `gh_unavailable`

#### Scenario: Project-planted git ignored by gh
- **WHEN** `PATH` lists a project directory holding an executable `git` before the directory holding the real `gh`, and the user runs `/review pr 7`
- **THEN** `gh` runs with that entry removed from its `PATH`, and the planted `git` never runs

#### Scenario: PR without gh
- **WHEN** the user runs `cyber review pr 42` and `gh` is not installed
- **THEN** the review fails with `gh_unavailable` before any subagent starts (exit 2)

#### Scenario: PR base advanced past the fork point
- **WHEN** the base branch gains commits after the pull request branched and the user runs `cyber review pr 42`
- **THEN** the reviewer receives only the pull request's own changes, diffed from the merge base, and not the base branch's later commits reversed

#### Scenario: PR commits not local
- **WHEN** `gh` reports base or head commit X and X is absent locally
- **THEN** the review fails with `pr_commits_missing` and nothing is fetched

#### Scenario: Override
- **WHEN** the project defines a `review` command
- **THEN** `/review` and `cyber review` run that command and the CLI JSON output has `findings: null`

#### Scenario: Retry
- **WHEN** a client retries POST /command `review` with the same message ID after the working tree changed
- **THEN** it receives the original receipt and only one review runs

#### Scenario: Reviewer launch refused by a hook
- **WHEN** a global `SubagentStart` hook matching `reviewer` denies and the user runs `cyber review` with changes
- **THEN** no reviewer Job starts and no reviewer request reaches the provider; `cyber review` exits 1 with `reviewed: false` and the hook's reason

#### Scenario: Reviewer tool call blocked by a hook
- **WHEN** a `PreToolUse` hook denies `read` and the reviewer reads a changed file
- **THEN** the reviewer receives the `blocked by hook` tool error instead of the file content, the blocked receipt is stored on the reviewer Session, and the review still completes with its findings

#### Scenario: Target resolution runs no tool hooks
- **WHEN** a `PreToolUse` hook matches every tool and the user runs `cyber review`
- **THEN** the hook's first invocation is the reviewer's own first tool call, and resolving the target runs no hook on the invoking Session

#### Scenario: Reviewer stays read-only
- **WHEN** the review subagent attempts an edit, or spawns a descendant that does
- **THEN** it is denied by the inherited reviewer ceiling

#### Scenario: Unverified findings dropped
- **WHEN** (P2) the review workflow's verification agent rejects a reviewer finding
- **THEN** the finding is excluded from the final list and counted in `dropped`

# vcs-integration Specification

## Purpose
VCS integration connects Cyber Code to git and code-hosting platforms. It provides local git awareness and review, a GitHub App and Action that run agent Sessions from issues and pull requests, PR auto-fix subscriptions, structured review comments, and a GitLab CI component. It builds on OpenCode v1's GitHub Action and `pr` command, Codex's `/review` and `/diff`, and Claude Code's Code Review, auto-fix PRs and ultrareview. Deep reviews run as Workflows, either locally or on Runners.

## Requirements

### Requirement: Git awareness
(P0) The system SHALL expose the Location's git state through `GET /api/v1/vcs` (branch, upstream, ahead/behind, worktree, dirty flag) and `GET /api/v1/vcs/diff?base=<ref>` (file list with status, additions, deletions, and the unified patch, including untracked files). Outside a git repository these routes SHALL return `{ "vcs": null }` instead of failing. `/diff` in the TUI SHALL render the same data.

#### Scenario: Untracked files in diff
- **WHEN** the agent creates `src/new.rs` without staging it
- **THEN** `GET /api/v1/vcs/diff` lists `src/new.rs` with status `added`

### Requirement: Local review command
(P1) The system SHALL provide `/review [target]` and `cyber review [target]`. The target is uncommitted changes (default), `<base>..<head>`, a commit SHA, or `pr <number>`. In P1 the review SHALL run the bundled `review` skill (`skills-commands`) in a read-only `reviewer` subagent. From P2 it SHALL instead run the bundled `review` workflow (`workflows`), which checks correctness, security, tests and maintainability in parallel and verifies each finding before reporting it. In both cases results SHALL be reported as structured findings with `file`, `line`, `severity` (`critical`, `high`, `medium`, `low`), `summary` and `failure_scenario`.

#### Scenario: Review uncommitted changes
- **WHEN** the user runs `/review` with 4 modified files
- **THEN** the review runs over those 4 files and the TUI shows the findings, most severe first

#### Scenario: Unverified findings dropped
- **WHEN** (P2) the review workflow's verification agent rejects a reviewer finding
- **THEN** the finding is excluded from the final list and counted in `dropped`

### Requirement: Commit and PR text generation
(P1) The system SHALL provide `cyber git commit-msg` and a `/commit` command that generate a conventional commit message from the staged diff using the small model. It SHALL provide `/pr-description` to draft a PR title and body from `base..HEAD`. A co-author trailer SHALL be appended only when `git.co_author` is configured (default: none, so commits are attributed to the user alone).

#### Scenario: No co-author by default
- **WHEN** the agent commits with default config
- **THEN** the commit message contains no `Co-authored-by` trailer

#### Scenario: Configured co-author
- **WHEN** `git.co_author` is `"Cyber Code <bot@cyber.dev>"` and the agent commits
- **THEN** the message ends with `Co-authored-by: Cyber Code <bot@cyber.dev>`

### Requirement: Protected git operations
(P0) The system SHALL classify these as permission actions with the remote or branch as the resource:
- `git push` → `git.push`
- force push → `git.push_force`
- `git reset --hard` → `git.reset_hard`
- branch deletion → `git.branch_delete`
- remote tag pushes → `git.push`

Default rules SHALL be `ask` for `git.push` and `deny` for `git.push_force` to branches matching `git.protected_branches` (default `["main", "master", "release/*"]`). `bypass` mode SHALL NOT override a `deny` on protected branches.

#### Scenario: Force push to main denied
- **WHEN** the agent runs `git push --force origin main` in `bypass` mode
- **THEN** the command is blocked with `force push to protected branch main is denied`

#### Scenario: Push asks by default
- **WHEN** the agent runs `git push origin feature/x` in `default` mode
- **THEN** a permission request for `git.push` on `origin/feature/x` is raised

### Requirement: PR checkout and linked sessions
(P1) The system SHALL record the PR a Session is linked to whenever the Session creates or pushes to a PR branch. `cyber pr <number>` SHALL check out the PR with `gh pr checkout` (adding a fork remote when needed) and resume the most recent Session linked to that PR, or start a new one. `cyber --from-pr <number|url>` SHALL open the Session picker filtered to Sessions linked to that PR.

#### Scenario: Resume the linked session
- **WHEN** a Session opened PR #42 and the user later runs `cyber pr 42`
- **THEN** the branch is checked out and the TUI resumes that Session

### Requirement: GitHub App installation
(P3) The system SHALL provide `cyber github install`, which requires a GitHub `origin`. It SHALL:
1. Ensure the `cyber-code` GitHub App is installed for the repository, opening the install URL and polling every 1 s for up to 120 s.
2. Prompt for a provider and model.
3. Write `.github/workflows/cyber.yml`.
4. Print which secrets to add.

Self-hosters SHALL be able to point the install at their own App via `--app <slug>`.

#### Scenario: Workflow file generated
- **WHEN** installation succeeds with model `anthropic/claude-sonnet-5-5`
- **THEN** `.github/workflows/cyber.yml` triggers on `issue_comment` and `pull_request_review_comment`, grants `id-token: write`, and runs `cyber-code/action@v1` with that model

### Requirement: GitHub Action triggers
(P3) The Action `cyber-code/action` SHALL support `issue_comment`, `pull_request_review_comment`, `issues`, `pull_request`, `schedule` and `workflow_dispatch` events. For comment events it SHALL run only when the body contains a configured mention (default `@cyber` or `/cyber`, case-insensitive). For `schedule`, `workflow_dispatch` and `issues` it SHALL require the `prompt` input. Any other event SHALL fail with `Unsupported event type: <name>`.

#### Scenario: Mention triggers a run
- **WHEN** a collaborator comments `@cyber add tests for parse_date` on issue #10
- **THEN** the Action starts a Session with the issue context and that request

#### Scenario: Comment without mention ignored
- **WHEN** a comment does not contain a configured mention
- **THEN** the Action exits successfully without starting a Session

### Requirement: Actor authorization and token exchange
(P3) The Action SHALL require the triggering actor to have `write` or `admin` permission on the repository for user-triggered events, and fail with `User <actor> does not have write permissions` otherwise. It SHALL obtain a GitHub OIDC token (audience `cyber-code-action`) and exchange it at the configured token service for a short-lived App installation token. The installation token SHALL be revoked at the end of the run, whether the run succeeds or fails.

#### Scenario: Read-only actor rejected
- **WHEN** a user with `read` permission comments `@cyber fix this`
- **THEN** the Action posts no changes and fails with the write-permission message

#### Scenario: Token revoked on failure
- **WHEN** the agent run fails mid-way
- **THEN** the installation token is revoked before the job exits with code 1

### Requirement: Issue and PR workflows
(P3) For an issue, the Action SHALL create branch `cyber/issue-<number>-<timestamp>`, run the Session with the issue title, body and comments as context and the `question` tool denied, and then:
- if files changed: commit, push, open a PR to the default branch with `Closes #<number>`, and comment `Created PR #<n>`
- otherwise: comment the response

For a same-repo PR, it SHALL push to the head branch. For a fork PR, it SHALL push to the fork when the maintainer allows edits, and otherwise post a patch suggestion.

#### Scenario: Issue to PR
- **WHEN** the agent fixes issue #10 and changes 3 files
- **THEN** a PR titled from a summary of 40 characters or fewer is opened with body ending `Closes #10`, and the issue receives `Created PR #N`

### Requirement: Reactions and response footer
(P3) The Action SHALL add an `eyes` reaction to the triggering comment at start and replace it with `rocket` on success or `confused` on failure. Every reply SHALL end with a footer linking the run (`/actions/runs/<id>`) and, when the Session was shared, the share URL.

#### Scenario: Failure reaction
- **WHEN** the run fails
- **THEN** the comment reaction changes from `eyes` to `confused` and a reply explains the error

### Requirement: Structured PR review comments
(P3) The system SHALL post review findings to a pull request as one GitHub review. Each finding SHALL be an inline comment anchored to `file` and `line`, with severity and failure scenario. Findings outside the diff SHALL go in the summary body. Re-running the review SHALL update or resolve the bot's previous comments instead of duplicating them.

#### Scenario: Re-review resolves fixed findings
- **WHEN** a finding from the first review is fixed in a new commit and the review re-runs
- **THEN** the bot resolves its earlier comment thread and posts only the remaining findings

### Requirement: PR auto-fix subscriptions
(P3) The system SHALL let a Session subscribe to a PR (`/autofix on`, or `cyber pr <n> --autofix`). CI failures and new review comments then arrive as Channel events admitted with `delivery: queue` (see `channels`), and the Session attempts fixes and pushes to the PR branch. A subscription SHALL run on a Runner when the user's machine is offline and the user has a Cyber Account with the `cyber-code` entitlement. Each subscription SHALL stop after `autofix.max_attempts` (default 3) consecutive failed attempts or when the PR closes.

#### Scenario: CI failure triggers a fix
- **WHEN** CI fails on a subscribed PR
- **THEN** the failure log summary is admitted to the Session, and the agent pushes a fix commit if it can

#### Scenario: Attempts exhausted
- **WHEN** three consecutive auto-fix attempts fail CI
- **THEN** the subscription stops and the PR receives a comment saying auto-fix gave up, with the last error

### Requirement: Deep cloud review
(P3) The system SHALL provide `/review deep [target]` (alias `cyber review --deep`), which runs the bundled `deep-review` workflow on a Runner as a background Workflow Run. Many reviewer agents run, each finding is verified independently, and the result is delivered to the Session and optionally posted to the PR. It SHALL show the estimated cost before starting and require confirmation unless `--yes` is passed.

#### Scenario: Cost confirmation
- **WHEN** the user runs `/review deep pr 77`
- **THEN** the TUI shows the estimated agent count and cost and starts only after confirmation

### Requirement: GitLab CI component
(P3) The system SHALL publish a GitLab CI/CD component `cyber-code/cyber` that installs `cyber`, reads provider credentials from CI variables, and runs `cyber exec` against merge-request or issue events. With `CI_JOB_TOKEN` or a project access token, it SHALL post the response as a merge-request note and push to the source branch when changes exist.

#### Scenario: Merge request note
- **WHEN** the component runs on a merge request with `prompt: "review this MR"`
- **THEN** the review is posted as a note on the merge request

### Requirement: Scheduled repository jobs
(P3) For `schedule` and `workflow_dispatch` events, the Action SHALL create branch `cyber/<schedule|dispatch>-<6 hex>-<timestamp>` and log the response instead of commenting. When commits exist, it SHALL open a PR. It SHALL skip PR creation when no commits differ from the base, and reuse an already-open PR for the same head.

#### Scenario: Nothing to change
- **WHEN** a scheduled dependency-update job finds nothing to change
- **THEN** no branch is pushed and no PR is opened, and the job succeeds

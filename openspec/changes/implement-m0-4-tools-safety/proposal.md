# Implement M0.4: tools and safety

## Why

Milestone M0.4 (ROADMAP) gives the durable runtime its tools and the safety layers around them: the permission engine, checkout trust, the macOS and Linux sandbox, conflict-aware snapshots with revert, and reconciliation of unknown outcomes. A coding agent is not usable without them, and none of them can be added safely after tools ship.

## What Changes

- New crate `cyber-tools`, the built-in tool host:
  - Tools: `read`, `write`, `edit`, `apply_patch`, `list`, `glob`, `grep`, `bash`, `webfetch`, `websearch`, `todo`, `question`, `skill`, `history_search`, `plan_enter` and `plan_exit`.
  - Input validation against each schema, the output budget with overflow files, per-path write serialization, and a stale-file check after approval.
  - The permission engine: ordered rules where the last match wins, protected paths, user-layer ceilings, saved approvals, every Mode, external directories with symlink checks, and bash command analysis with tree-sitter.
  - Tool visibility per Turn: `apply_patch` or `edit`, plan mode, fully denied tools, and websearch hidden without a backend.
  - Read-only reconciliation of `write`, `edit` and `apply_patch` after a crash.
  - The `core/skills` Context Source.
- New crate `cyber-sandbox`:
  - Policies `read-only`, `workspace-write` and `full-access`; project config cannot select full access.
  - Credential masking and the network allowlist proxy with `network` permission requests.
  - macOS enforcement with a generated Seatbelt profile.
  - Linux enforcement with bubblewrap and the `cyber-sandbox-exec` proxy bridge. Without it, execution fails closed.
- New crate `cyber-snapshot`: a shadow git repository per worktree, sharing objects through alternates. It never touches the user's index, refs, stash or config. Restore is conflict-aware, with three-way merges, a race recheck and backups. Shadow repositories are garbage-collected.
- `cyber-server::runtime`:
  - Snapshots before each Turn and after its tool settlements.
  - `snapshot.taken.1` and `session.diff.1` events.
  - Three-phase revert (`stage`, `clear`, `commit`) with a busy guard and auto-commit.
  - The permission and question broker with the doom-loop check.
  - Host hooks for context sources and for reconciliation with the Location.
- `cyber-core`: project identity from git, and skill discovery with argument substitution.

## Spec changes

- `sandbox`:
  - Linux uses bubblewrap first, because Landlock cannot keep paths read-only or unreadable inside allowed trees.
  - On macOS the per-user temporary directory is writable.
- `skills-commands`: the `skills` key accepts an array of paths or an object with `paths`, `compat` and `listing_budget_tokens`.
- `snapshots-checkpoints`:
  - The revert boundary includes the target message.
  - A code-only commit keeps the conversation.
  - Revert operations report busy and conflict errors.
- `builtin-tools`: without `tools.websearch.backend`, websearch rotates among backends that have credentials. Credentials resolve from config, then `CYBER_AUTH_CONTENT`, then `<BACKEND>_API_KEY`.
- `storage-events`: a repository with no origin and no commits gets a generated project ID that is cached.
- `permissions-modes`:
  - Default rules allow `todo`, `skill` and `history_search`.
  - `network` asks by default.

## Impact

The server API routes (`/revert/*`, `/diff`, `/permissions`, `/questions`) arrive with M0.5. The following are deferred:
- Native provider web search.
- Images returned as media parts.
- Background bash.
- `notebook_edit`, `monitor`, `advisor` and `powershell`.
- The non-git snapshot fallback (P1).
- Windows enforcement (P1).
- A Landlock fallback for Linux hosts without bubblewrap.

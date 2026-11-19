## Why

In `auto`, every request that would otherwise ask goes to a reviewer model that sees only a JSON snapshot: the tool call, the last 20 messages, the Location and the user's boundaries. It has no tools and no repository context, so it either guesses or blocks when the answer is one file or one earlier message away. It also judges a bare "yes" with no idea what was asked, and it is consulted (and billed) even for a `cat README.md` that the OS sandbox already confines. Another agent's public changelog (Antigravity) describes a reviewer with read-only context tools, a narrower review scope and confirmation-aware intent; this is the clean-room counterpart, built on our own permission, sandbox and untrusted-content machinery.

## What Changes

- **Read-only reviewer context tools.** The reviewer may call `transcript_search` (the reviewed Session's history), `read_file` and `grep_files` (the Location). File reads obey the Session's own read rules: a path the `read` tool could not read without asking (`.env`, a `deny_read` glob, a secret file, a rule's `ask`/`deny`, anything outside the Location, a link leaving it) is refused. Strict budgets: 8 calls, 16 KiB per result, 64 KiB per review, 5 s per call, inside the existing 30 s review timeout.
- **Data, not instructions.** Every tool result reaches the reviewer inside an `<untrusted_content>` frame (sources `transcript` and `workspace`), scanned for injection signals by the existing detector; the frame primitive moves to `cyber-core` so the runtime and the tool registry share one implementation. The trusted system lines tell the reviewer that text asking it to approve is an injection attempt.
- **Fail closed.** An unknown tool, invalid arguments, a refused or failed read, a timeout or an exhausted budget ends the review as a fallback (ask the user, or deny as `fallback`/attendance already decide), never an allow. An unparseable verdict still falls back as before.
- **Confirmation in context.** When the newest user prompt is a bare confirmation (`yes`, `go ahead`, …), the evidence pairs it with the assistant message it answers (framed as data) so the reviewer judges intent against what was actually proposed.
- **Sandbox-confined read-only commands skip review.** A `bash` tool command that runs under the enforced OS sandbox in the Location (not excluded, not escalated, network not `on`), in an untainted run, made only of `ls`, `pwd`, `echo`, `cat`, `head`, `tail` or `wc` (no `git`: repository config can make it run a program), with no shell expansion and every operand a path the Session may read without asking, is allowed as a recorded policy decision instead of a reviewer call. Nothing else is narrowed; `classify_read_only: true` keeps reviewing them.
- **Recorded.** `permission.auto_decided.1` gains `reviewer_tools` (tool, target, bytes, ok) and `confirmation`; per-checkout statistics gain `reviewer_tool_calls` (`cyber permissions auto show`).

## Capabilities

### Modified Capabilities

- `permissions-modes`: auto mode classifier (scope and evidence), auto-mode configuration and override (statistics); new requirements Auto reviewer context tools, Auto reviewer confirmation context, Sandbox-confined commands skip auto review.
- `untrusted-content`: new requirement Reviewer evidence frames.

## Impact

- Code: `cyber-server` runtime (`auto.rs`, new `auto_context.rs`, `auto_statistics.rs`), `cyber-tools` (`auto_permissions.rs`, new `auto_review.rs`, `bash.rs`, `sandboxing.rs`, permission `Request`), `cyber-core` (`untrusted::frame`), `cyber-store` migration `20261118020000_auto_reviewer_tools`, `cyber permissions auto show`.
- Events: additive optional fields on `permission.auto_decided.1`; older events read unchanged.
- Cost: a review that uses tools makes up to 9 evaluator calls instead of 1, all metered as today; confined commands make none.
- Docs: `docs/local-core-guide.md` (Auto-mode controls), `docs/features.md`, `CHANGELOG.md`. No CLI flag or help text changes.

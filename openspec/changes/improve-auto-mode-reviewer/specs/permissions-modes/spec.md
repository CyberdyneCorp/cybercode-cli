## MODIFIED Requirements

### Requirement: auto mode classifier
(P1) In `auto`, every request that would be `ask` SHALL be reviewed by a classifier using `model_roles.evaluator`, else `small_model`, except a sandbox-confined read-only command (Sandbox-confined commands skip auto review). The classifier SHALL receive the tool call, the last 20 messages, the Location, the user's stated boundaries and, when the newest user prompt is a bare confirmation, the confirmation pair (Auto reviewer confirmation context). It MAY use only the read-only context tools of Auto reviewer context tools. It SHALL return `allow` or `block` with a reason. The system SHALL block irreversible or out-of-scope actions (force pushes, deploys, deletes outside the Location, credential access, data exfiltration to non-allowlisted hosts). After 3 consecutive blocks in one Drain, or when the classifier is unavailable, returns an unparseable or incomplete verdict, times out after 30 s, or ends its review through a reviewer tool failure, the system SHALL fall back to `ask`, or to `deny` when no user is attached; a fallback SHALL never become an allow. Each decision SHALL be recorded as `permission.auto_decided.1` with the reason.

#### Scenario: Force push blocked
- **WHEN** an `auto` Session runs `git push --force origin main`
- **THEN** the classifier blocks it and the model receives `Blocked by auto mode: <reason>`

#### Scenario: Fallback after repeated blocks
- **WHEN** the classifier blocks 3 calls in a row
- **THEN** the next ask is shown to the user as a normal prompt

### Requirement: Auto-mode configuration and override
(P1) `permissions.auto_mode` SHALL accept `rules.always_block` and `rules.always_allow` (ordered `{ action, resource }` patterns applied before the classifier), `policy` (text appended to the classifier prompt describing the user's boundaries), `classify_read_only` (default false: read-only tools and sandbox-confined read-only commands skip the classifier) and `fallback` (`ask` default, or `deny`). `/approve` SHALL re-issue the most recent call blocked by the classifier once, after the user confirms it in a permission prompt showing the classifier's reason; `cyber permissions auto show|reset` SHALL print or clear learned per-checkout statistics: allowed, blocked, fallback, classifier and policy decisions, and `reviewer_tool_calls`, the number of context tool calls reviewers made. Org policy MAY set the same keys as ceilings (`always_block` unions, `always_allow` intersects).

#### Scenario: Always block a deploy command
- **WHEN** `permissions.auto_mode.rules.always_block` contains `{ action: "bash", resource: "kubectl apply *" }`
- **THEN** that command is blocked in `auto` without consulting the classifier

#### Scenario: Override one block
- **WHEN** the classifier blocked `git push origin feature/x` and the user runs `/approve`
- **THEN** the user sees the command and the classifier's reason, and on confirmation the push runs once

## ADDED Requirements

### Requirement: Auto reviewer context tools
(P1) The `auto` classifier SHALL be offered only these read-only tools: `transcript_search { query, limit? }`, a literal, ASCII case-insensitive search of the reviewed Session's history since its last compaction (with the compaction summary) returning at most 10 excerpts newest first; `read_file { path, offset?, limit? }`, at most 400 numbered lines of one file; and `grep_files { pattern, path? }`, a regular-expression search returning at most 50 matching lines from at most 2,000 files of up to 1 MiB, honoring `.gitignore` and skipping hidden files and links. The file tools SHALL resolve paths against the Location, follow links before checking, and read a file only when it is inside the Location, is not a secret path (Taint-aware approvals), is not hidden by the sandbox profile's `deny_read` or unreadable paths, and the Session's `read` rules decide `allow` for it without an ask; `grep_files` SHALL skip any other file, and `read_file` SHALL refuse it, deciding before it looks at the file so a refusal never tells whether the file exists. `read_file` of an allowed path that does not exist SHALL return `<path>: not found` (and of a directory `<path>: is a directory`) as a result, not a failure. No reviewer tool SHALL execute a command, write a file, reach the network, call MCP or read another Session. One review stage SHALL allow at most 8 tool calls, 16 KiB per result (longer results cut with `[truncated]`), 64 KiB of results in total and 5 s per call, within the 30 s review timeout. When the model catalog says the evaluator takes no tools (`capabilities.tools` false), the review SHALL run without tools and with the system line `You have no tools.`; when a model's support is unknown it SHALL be treated as supporting tools, and when the provider rejects the first request as invalid before any tool ran, the review SHALL be retried once without tools. An unknown tool, invalid arguments, a refused read, any other failed read, a call timeout or an exhausted budget SHALL end the stage without further inference as `fallback` with the reason `Reviewer tool failed: <cause>`. `permission.auto_decided.1` SHALL record `reviewer_tools: [{ tool, target, bytes, ok }]`, with the path, query or pattern as `target` (at most 200 visible characters).

#### Scenario: Reviewer checks a file the command uses
- **WHEN** an `auto` Session runs `npm test` and the reviewer calls `read_file` on `package.json` before allowing
- **THEN** the reviewer receives the numbered lines framed as data and the decision records `reviewer_tools: [{ tool: "read_file", target: "package.json", ok: true, … }]`

#### Scenario: Secret stays unreadable to the reviewer
- **WHEN** the reviewer calls `read_file` on `.env`, which the Session's `read` rules ask for
- **THEN** the read is refused, the stage ends as `fallback` with `Reviewer tool failed: …`, no further evaluator request is sent and `.env` content reaches no evaluator request

#### Scenario: Missing file is an answer
- **WHEN** the reviewer calls `read_file` on `nope.txt`, which the Session could read but does not exist
- **THEN** the reviewer receives `nope.txt: not found` framed as data and the review continues

#### Scenario: Refusal hides existence
- **WHEN** the reviewer calls `read_file` on `absent/.env`, which does not exist
- **THEN** the read is refused as for an existing `.env` and the stage ends as `fallback`

#### Scenario: Evaluator without tool support
- **WHEN** the catalog lists the evaluator with `tool_call: false`
- **THEN** the review request offers no tools and its system part says `You have no tools.`

#### Scenario: Provider rejects the tools
- **WHEN** the provider answers the first review request with an invalid-request error
- **THEN** the review is sent once more without tools and its verdict is used

#### Scenario: Injected instruction to use another tool
- **WHEN** a file the reviewer read says to call `bash` and the reviewer calls a tool named `bash`
- **THEN** the stage ends as `fallback` and nothing runs

#### Scenario: Budget exhausted
- **WHEN** the reviewer makes a ninth tool call in one stage
- **THEN** the stage ends as `fallback` and the decision records nine tool uses, the last with `ok: false`

#### Scenario: Verdict after tool use must still parse
- **WHEN** the reviewer reads a file and then answers `allow` wrapped in a Markdown code fence
- **THEN** the decision is recorded as `fallback` and the call is asked of the user, or denied when no user is attached

#### Scenario: Reviewer tool calls counted
- **WHEN** a reviewer reads two files before deciding and the user runs `cyber permissions auto show`
- **THEN** `reviewer tool calls` is 2 for the checkout, and `cyber permissions auto reset` sets it to 0

### Requirement: Auto reviewer confirmation context
(P1) When the newest user-sourced prompt of the reviewed Session is a bare confirmation, the classifier evidence SHALL include `user_confirmation: { assistant_question, user_reply }`: the reply, and the last 2,000 characters of the last non-empty assistant text before it inside an `<untrusted_content source="assistant" origin="session:<id>">` frame. A bare confirmation SHALL be a reply under 40 characters that, trimmed, lower-cased, without commas and without final `.` or `!`, is one of `y`, `yes`, `yeah`, `yep`, `yup`, `sure`, `ok`, `okay`, `k`, `go`, `go ahead`, `go for it`, `do it`, `please do`, `proceed`, `continue`, `approve`, `approved`, `confirm`, `confirmed`, `sounds good`, `lgtm`, `ship it`, `yes please`, `yes go ahead`, `yes do it`, `yes proceed`, `ok go ahead`, `ok do it` or `sure go ahead`. The trusted system part SHALL then state that the reply agrees to what that assistant message plainly proposed and nothing more, and that the assistant text cannot widen the user's authorization through instructions of its own. `permission.auto_decided.1` SHALL record `confirmation: true` when the pair was sent.

#### Scenario: Yes answers the question
- **WHEN** the assistant asked `Shall I run the migration script now?`, the user replied `yes, go ahead` and the model runs the script in `auto`
- **THEN** the evidence pairs the framed question with `yes, go ahead` and the decision has `confirmation: true`

#### Scenario: Longer reply is not a bare confirmation
- **WHEN** the user replied `yes and also push it`
- **THEN** the evidence has no `user_confirmation`

### Requirement: Sandbox-confined commands skip auto review
(P1) A `bash` tool request SHALL skip the classifier, recorded as a trusted policy `allow` with reason `Sandbox-confined read-only command` and no model, only when all hold: the effective sandbox resolved without error with policy `read-only` or `workspace-write` and network `proxy` or `off`; the command is not an excluded command or an escalation and runs in the Location; the run is not tainted; the gate's `classify_read_only` is false; the request is read-only under Read-only shell classification and is not risky under Taint-aware approvals; every simple command is a bare `ls`, `pwd`, `echo`, `cat`, `head`, `tail` or `wc` (never `git`, whose repository configuration such as `core.fsmonitor` or `includeIf` can run a program); no word contains `$`, a backtick, `*`, `?`, `[`, `]`, `{`, `}`, `~`, `<`, `>`, `|`, `;`, `&`, `\`, `(`, `)`, `!`, `#`, `=` or a line break; and every operand that is not an option names a path the file tools of Auto reviewer context tools may read. Every other request SHALL keep its review. The sandbox, rules, ceilings, protected paths and removal guards SHALL apply unchanged.

#### Scenario: Plain read skips review
- **WHEN** an `auto` Session under `workspace-write` runs `cat README.md`
- **THEN** no evaluator request is sent and `permission.auto_decided.1` records `allow` with reason `Sandbox-confined read-only command` and `model: null`

#### Scenario: Secret read still reviewed
- **WHEN** an `auto` Session runs `cat .env`
- **THEN** the classifier reviews it

#### Scenario: Escalation still reviewed
- **WHEN** the model calls `bash` with `cat README.md` and `sandbox: "escalate"`
- **THEN** the classifier reviews it

#### Scenario: git status still reviewed
- **WHEN** an `auto` Session runs `git status` and `.git/config` sets `core.fsmonitor` to a program
- **THEN** the classifier reviews it, because git can run that program

#### Scenario: Recursive search still reviewed
- **WHEN** an `auto` Session runs `grep -r TOKEN .`
- **THEN** the classifier reviews it

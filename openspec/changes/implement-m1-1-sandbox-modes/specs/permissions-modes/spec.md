## MODIFIED Requirements

### Requirement: Session ruleset API
(P0) The Session ruleset layer SHALL be settable by clients: `PUT /api/v1/sessions/:id/permissions/rules` with `{ rules: [{ action, resource, effect }] }` SHALL replace the Session's rules, `GET` SHALL return them with the effective merged ruleset, and the SDK SHALL expose `session.permissions.set(rules)` and `.get()`. `cyber exec --allow <action[:resource]>` and `--deny <action[:resource]>` SHALL populate this layer for the new Session. Session rules SHALL be recorded as a durable `session.permissions.updated.1` event and apply at the next Safe Boundary. They SHALL NOT widen explicit user/global denies, plan-mode restrictions, protected paths, workspace trust, sandbox boundaries or org policy, which remain ceilings.

#### Scenario: Script narrows a session
- **WHEN** an SDK client calls `session.permissions.set([{ action: "bash", resource: "*", effect: "deny" }])` on a running Session
- **THEN** from the next Safe Boundary every `bash` call is denied and the event is recorded

#### Scenario: Session rule cannot beat an org deny
- **WHEN** a Session rule allows `webfetch` for `*` and org policy denies `webfetch` for `*.internal.example.com`
- **THEN** fetching an internal host is still denied with reason `org policy`

#### Scenario: Ordered Session rules reach tool evaluation
- **WHEN** a Session ruleset contains ordered `{ action, resource, effect }` rules
- **THEN** tool admission SHALL parse every rule in written order and evaluate the last matching rule
- **AND** an explicit final deny SHALL refuse the action even in bypass Mode without executing it
- **AND** a later Session allow SHALL NOT widen a user/global deny ceiling

### Requirement: Permission modes
(P0) The system SHALL support Modes `default` and `plan` in P0, adding `accept-edits`, `auto`, `dont-ask` and `bypass` in P1, selectable per Session (`--mode`, `mode` config, agent `permission_mode`, `/mode <name>`, `POST /api/v1/sessions/:id/mode`). The TUI SHALL cycle Modes with Shift+Tab through `default → accept-edits → plan → auto → default` on P1 builds (`default → plan → default` on P0 builds); `bypass` and `dont-ask` SHALL be reachable only through `/mode`, flags or config. A Mode change SHALL apply at the next Turn and publish `session.mode.switched.1`, as specified by session-runtime. The UI SHALL show a requested change as pending until effective; interrupt SHALL be offered when immediate cancellation is needed.

#### Scenario: Cycling modes
- **WHEN** the user presses Shift+Tab twice from `default` on a P1 build
- **THEN** the Session mode becomes `plan`

#### Scenario: A switch during inference applies to the next Turn's tools
- **WHEN** the user changes mode while a model response is in flight
- **THEN** all tool groups from that response SHALL use the mode pinned at Turn start, and the next Turn SHALL use the new selection
- **AND** replay SHALL preserve that distinction, including legacy start events without an explicit mode field

#### Scenario: Pending mode is visible after reconnect
- **WHEN** a selected mode differs from the mode of a running Turn
- **THEN** Session responses SHALL expose `effective_mode` and `pending_mode`, and the TUI SHALL display the effective mode with the pending selection and offer interrupt

### Requirement: auto mode classifier
(P1) In `auto`, every request that would be `ask` SHALL be reviewed by a classifier using `model_roles.evaluator`, else `small_model`. The classifier SHALL receive the tool call, the last 20 messages, the Location and the user's stated boundaries. It SHALL return `allow` or `block` with a reason. The system SHALL block irreversible or out-of-scope actions (force pushes, deploys, deletes outside the Location, credential access, data exfiltration to non-allowlisted hosts). After 3 consecutive blocks in one Drain, or when the classifier is unavailable, the system SHALL fall back to `ask`, or to `deny` when no user is attached. Each decision SHALL be recorded as `permission.auto_decided.1` with the reason.

#### Scenario: Force push blocked
- **WHEN** an `auto` Session runs `git push --force origin main`
- **THEN** the classifier blocks it and the model receives `Blocked by auto mode: <reason>`

#### Scenario: Fallback after repeated blocks
- **WHEN** the classifier blocks 3 calls in a row
- **THEN** the next ask is shown to the user as a normal prompt

#### Scenario: Malformed evaluator output
- **WHEN** the evaluator returns extra keys, an empty reason, a tool call or an incomplete response
- **THEN** the decision SHALL fall back to approval or unattended denial and SHALL NOT authorize execution
- **AND** any reported evaluator usage SHALL be included in session and evaluation totals without counting a coding Turn

#### Scenario: Durable decision failure
- **WHEN** the evaluator returns allow but the auto-decision event cannot be committed
- **THEN** the tool SHALL NOT execute

#### Scenario: Fresh Drain resets fallback
- **WHEN** three classifier blocks occurred in a previous Drain and a new Drain starts
- **THEN** the consecutive-block counter SHALL reset before the next review

#### Scenario: Production tool classification
- **WHEN** a production tool request requires approval in auto mode and no independent parent or protected-path approval is required
- **THEN** a durably recorded classifier allow authorizes that request once without saving an approval
- **AND** block is returned to the model before tool effects, while fallback uses normal interactive approval or unattended denial

#### Scenario: Independent manual approval survives classification
- **WHEN** an ancestor requires manual approval or a protected file or credential requires individual confirmation
- **THEN** a child's classifier cannot substitute for that approval
- **AND** hard rule denies and critical-removal refusals precede classifier execution

### Requirement: Critical-path removal guard
(P1) The system SHALL detect removals targeting critical paths (`/`, `~`, the home directory, the repository root, the Location root, `.git`, and any ancestor of the Location). These include `rm -rf`, `Remove-Item -Recurse`, `find -delete`, and removals inside nested shells or inline scripts. Such removals SHALL be denied in `auto`, `dont-ask` and `bypass` modes and SHALL require `ask` with a red warning in other modes. The model SHALL receive `Refused: removal of critical path <path>. Rewrite the command to target specific files.`

#### Scenario: rm -rf on repo root under bypass
- **WHEN** a `bypass` Session runs `rm -rf "$(git rev-parse --show-toplevel)"`
- **THEN** the command is refused without execution

#### Scenario: Saved approval cannot lift a critical removal guard
- **WHEN** a critical removal matches an explicit allow rule or saved approval
- **THEN** automatic modes SHALL still refuse it and manual modes SHALL still require individual confirmation

#### Scenario: A confirmation-only request is not covered by an approval cascade
- **WHEN** an always reply covers another pending critical-removal request's pattern
- **THEN** that request SHALL remain pending until individually confirmed or rejected

#### Scenario: Symlink parent traversal targets a critical root
- **WHEN** a removal traverses a symlink and parent components to a critical root
- **THEN** analysis SHALL resolve the symlink before collapsing parent components and enforce the critical-removal guard

#### Scenario: Shell stdin contains a critical removal
- **WHEN** Bash receives a literal critical-root removal through a heredoc or here-string, directly or through a supported wrapper or pipeline
- **THEN** the critical-removal guard SHALL apply before automatic or saved approval

#### Scenario: Ordinary heredoc data is not executable source
- **WHEN** a quoted heredoc containing removal text is delivered to `cat` without an executable shell consumer
- **THEN** that literal text SHALL NOT trigger the critical-removal guard, while other applicable permission rules remain enforced

#### Scenario: A scalar binding resolves a critical target
- **WHEN** a Bash invocation assigns a literal repository-root value to a variable and removes its quoted expansion
- **THEN** the guard SHALL report the resolved critical path and SHALL enforce the same ceiling as a literal operand

#### Scenario: Inline assignment does not replace outer argument expansion
- **WHEN** a removal has a temporary environment assignment and a quoted variable operand
- **THEN** analysis SHALL resolve that operand using the outer shell state before the temporary assignment, while nested executable source uses its own assignment context

#### Scenario: Uncertain variable mutation cannot authorize dispatch
- **WHEN** a branch, loop, read or unsupported mutation can change a bound removal target
- **THEN** the target SHALL remain unresolved unless its execution value can be proven, and automatic modes SHALL NOT allow it

#### Scenario: A substitution's directory change is local
- **WHEN** command substitution changes its own directory and a later command removes a literal noncritical workspace file
- **THEN** analysis SHALL use the outer working directory for the later operand instead of inheriting the substitution's directory state

#### Scenario: Binding or function expansion exceeds the analysis budget
- **WHEN** repeated scalar growth or nested function fan-out exceeds the bounded static-analysis budget
- **THEN** the guard SHALL return an unresolved result and SHALL NOT permit automatic dispatch

#### Scenario: Inline Python and JavaScript call a critical removal API
- **WHEN** a literal Python `-c` or JavaScript eval invocation calls a removal API on the repository root, home or Location ancestor
- **THEN** the same critical-path guard SHALL apply before saved or automatic approval
- **AND** import aliases, literal scalar bindings and nested executable source SHALL NOT hide the removal

#### Scenario: Inline program contains removal text as data
- **WHEN** a quoted string containing removal code is only printed by a supported inline program
- **THEN** that text SHALL NOT be interpreted as an executable removal

#### Scenario: Inline removal dispatch is unresolved
- **WHEN** a target, dynamic call or execution context cannot be proven by inline analysis
- **THEN** automatic modes SHALL refuse it and manual modes SHALL require individual confirmation

#### Scenario: PowerShell resolves a critical removal target
- **WHEN** literal PowerShell command source removes a critical path through Remove-Item, a standard alias, a module-qualified cmdlet or a supported static deletion API
- **THEN** critical-path analysis SHALL precede automatic and saved approvals
- **AND** proven scalar bindings, case-insensitive names and native drive/verbatim paths SHALL preserve the same boundary

#### Scenario: PowerShell parameters mutate a removal binding
- **WHEN** a PowerShell command's common parameters or uncertain control flow can change a later removal operand
- **THEN** analysis SHALL invalidate the affected proof and require individual confirmation until the resulting target is established

#### Scenario: Encoded or stdin PowerShell source targets a critical path
- **WHEN** a bounded UTF-16LE encoded command or literal PowerShell stdin source removes a critical path
- **THEN** decoding and interpreter-specific AST inspection SHALL enforce the same critical-path ceiling
- **AND** malformed encoding or unproven source selection SHALL require individual confirmation

#### Scenario: Another redirect replaces a literal interpreter source
- **WHEN** a later stdin redirect replaces a heredoc or here-string with an unproven source
- **THEN** analysis SHALL treat the effective source as unresolved and SHALL NOT report the unused body as an executed critical removal

### Requirement: accept-edits mode
(P1) In `accept-edits`, `edit`, `write`, `apply_patch` and `notebook_edit` inside the Location, and the filesystem commands `mkdir`, `touch`, `mv`, `cp` and `rm` (non-recursive) on paths inside the Location, SHALL evaluate as `allow` unless a rule denies them. Every other `ask` stays `ask`.

#### Scenario: Edit auto-approved
- **WHEN** a Session in `accept-edits` edits `src/main.rs`
- **THEN** no prompt is shown

#### Scenario: Dynamic shell targets are not proven workspace edits
- **WHEN** a filesystem command's paths or execution context cannot be established from literal shell syntax
- **THEN** accept-edits SHALL NOT automatically allow it, and ordinary rule and approval handling SHALL apply

#### Scenario: Filesystem aliases retain protected-path ceilings
- **WHEN** a literal filesystem operand resolves through a symlink to a protected configuration document
- **THEN** accept-edits SHALL preserve the protected-path approval requirement

#### Scenario: Nonrecursive removal with an option delimiter
- **WHEN** a nonrecursive rm has literal workspace operands after `--`
- **THEN** operand names starting with a dash SHALL be treated as paths rather than options

#### Scenario: Literal directory copies and moves
- **WHEN** a literal recursive copy or directory move has a bounded, inspectable ordinary source tree and established destination semantics
- **THEN** accept-edits SHALL check every source descendant and its resolved destination against Location and protected-path ceilings before automatic allowance
- **AND** uninspectable trees, symbolic links, multiply linked files, unsupported options and ambiguous directory operands SHALL retain ordinary approval

### Requirement: Auto-mode configuration and override
(P1) `permissions.auto_mode` SHALL accept `rules.always_block` and `rules.always_allow` (ordered `{ action, resource }` patterns applied before the classifier), `policy` (text appended to the classifier prompt describing the user's boundaries), `classify_read_only` (default false: read-only tools skip the classifier) and `fallback` (`ask` default, or `deny`). `/approve` SHALL re-issue the most recent call blocked by the classifier once, after the user confirms it in a permission prompt showing the classifier's reason; `cyber permissions auto show|reset` SHALL print or clear learned per-checkout statistics. Org policy MAY set the same keys as ceilings (`always_block` unions, `always_allow` intersects).

#### Scenario: Always block a deploy command
- **WHEN** `permissions.auto_mode.rules.always_block` contains `{ action: "bash", resource: "kubectl apply *" }`
- **THEN** that command is blocked in `auto` without consulting the classifier

#### Scenario: Override one block
- **WHEN** the classifier blocked `git push origin feature/x` and the user runs `/approve`
- **THEN** the user sees the command and the classifier's reason, and on confirmation the push runs once
- **AND** POST /api/v1/sessions/:id/approve and the SDK session.approve method return a replay receipt before waiting for the confirmation
- **AND** current permission ceilings still apply; rejected or unattended confirmation has no effect, and consumed attempts remain spent after restart

#### Scenario: Trusted validated controls
- **WHEN** project configuration supplies auto-mode rules or classifier policy
- **THEN** these settings require project trust before activation and invalid types, empty patterns and unsupported fallback values fail validation

#### Scenario: Rule precedence and recorded decisions
- **WHEN** a block rule matches an otherwise permitted request
- **THEN** it blocks before inference and the decision must be committed before dispatch returns
- **AND** an allow rule cannot lift a hard deny, protected path or independent manual approval

#### Scenario: Read-only classification and deny fallback
- **WHEN** an eligible read-only tool requires approval and classify_read_only is false
- **THEN** it skips inference with a durable policy decision while block rules retain precedence
- **AND** fallback deny refuses execution without opening an interactive request

#### Scenario: Checkout-scoped statistics reset
- **WHEN** a user shows or resets auto-mode statistics from a checkout or its subdirectory
- **THEN** the command reports the same scoped counters and clears only that checkout's recorded counts
- **AND** decision history, billing and other checkouts remain preserved
- **AND** the recording boundary is visible; historical events without recorded checkout identity SHALL NOT be attributed from a moved Session's current directory

#### Scenario: Ancestor auto block remains a ceiling
- **WHEN** an effective auto-mode ancestor has an always_block rule matching a child request
- **THEN** the request is blocked before effects even when the child's own Mode permits it
- **AND** the durable policy decision identifies the ancestor without classifier inference or saved approval
- **AND** an ancestor outside auto mode does not activate its auto-mode configuration

#### Scenario: Ancestor classifier caps child automatic approval
- **WHEN** a child's eligible action requires approval under an effective auto-mode ancestor
- **THEN** every auto-mode gate SHALL permit the action before effects, using each ancestor's own trusted policy, Location, recent messages and stated boundaries
- **AND** decisions and evaluator usage SHALL remain recorded on the executing child, identifying the reviewed ancestor and the actual execution Location
- **AND** intermediate allows SHALL NOT reset consecutive action blocks; three blocked actions SHALL trigger the ordinary fallback on the next classifier request
- **AND** a deny fallback anywhere in the intersection SHALL refuse effects, while an independent manual ceiling SHALL remain a manual request

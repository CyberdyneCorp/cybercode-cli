## Context
The existing network proxy supports TCP on macOS and a Unix-socket bridge into Linux's private network namespace. Windows currently fails closed. `Policy::apply_mode` treats unresolved auto requests as ask; there is no evaluator classification. Shift+Tab currently cycles default, accept-edits and plan, excluding auto. The runtime already persists mode changes and applies them at the next Turn.

## Windows boundary
Keep platform enforcement in cyber-sandbox. Use documented Windows APIs for restricted tokens, AppContainer profiles, scoped ACL entries and a job object that terminates descendants. Platform-specific unsafe bindings must remain isolated behind small safe interfaces, with deterministic resource cleanup. Preserve existing owner ACLs, limit newly granted access to the invocation's SID and remove only those grants on cleanup. Grant runtime reads and workspace writes separately; protected configuration and credential directories remain inaccessible. Do not run a weaker sandbox on failure.

Windows command launch needs native argument quoting, handle inheritance, cancellation and shell selection. Audit Unix-only helper, process-group and service-stop assumptions before adding a Windows build job. Avoid claiming an AppContainer is usable until native tests prove allowed writes, blocked outside writes, credential masking, descendant cancellation and default direct-network denial.

## Proxy
Reuse the existing domain decision callback and session approvals. Establish a Windows transport that AppContainer processes can reach without granting direct outbound access. Verify HTTP and CONNECT, denied hosts, approval timeout, cancellation and policy modes. Existing proxy code is the implementation baseline, not a new feature to duplicate.

## Classifier
Add a focused runtime interface for an evaluator request without tools. Snapshot the last 20 messages, Location, invocation and trusted user boundaries without holding session locks during inference. Parse a strict allow/block result with a reason; malformed output, timeout, unavailable models or storage failure must never authorize dispatch. Existing explicit denies and protected/irreversible boundaries remain enforced before classification; a model cannot widen them.

Persist `permission.auto_decided.1` before acting. Track three consecutive blocks per Drain, reset on an allowed decision/new Drain, and fall back to an interactive ask or unattended denial. Account for evaluator usage as hidden work, including any reported usage from malformed replies, timeouts and cancellation. The runtime foundation does not activate classifier approval for built-in tools until the host enforces protected and irreversible boundaries. Tests use scripted adapters for allow, block, malformed responses, exhaustion, fallback and malicious command/history text.

## Mode state
Cycle default → accept-edits → plan → auto → default. Keep bypass and dont-ask outside the cycle. The effective mode for an in-flight Turn stays fixed; show the requested mode as pending until the next Turn. Reuse the existing mode API and durable event instead of creating a second switching mechanism.

## Phase boundary
P0's incomplete local quality/long-task evidence is not waived by this proposal. P1 implementation is explicitly authorized while P0 validation continues. M1.1 implementation and P0 exit evidence are tracked independently; completing P1 code does not waive the remaining P0 gate.

## Critical-removal boundary

Critical-removal analysis runs before automatic rule and saved-approval effects. Explicit denies and plan restrictions remain final. Bash analysis must inspect syntax nodes, including nested `sh -c`/`bash -c` and `eval`, rather than search arbitrary quoted text. Literal targets are normalized and checked against Location, checkout, home, filesystem root and Location ancestors; canonical aliases cannot hide these roots. A removal whose target or shell context cannot be resolved requires manual approval and cannot be authorized by auto, dont-ask or bypass. Manual prompts carry a red warning and remain mandatory even after an always reply.

Deliver Bash coverage first, then native PowerShell and Python/JavaScript inline-script coverage. The latter are still required by M1.1; Bash-only coverage does not close the requirement or enable classifier dispatch.

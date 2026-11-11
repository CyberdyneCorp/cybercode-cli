# Active implementation goal and delivery rules

## Current audit: 2026-11-11

The user has set the active objective to complete implementation, verification
and accepted delivery of **#93**. #92 remains a separate unfinished workstream;
preserve its accepted behavior and dependency gates without assuming its missing
guarantees or selecting its pending policies. The owner's closed #94 is outside
this objective. The #92 rows below retain dependency context from the earlier
combined audit; they are not additional scope for the current goal.

Main `8266db6c` is integrated into `feat/managed-recovery-owner` at `2fcd745d`.
The merge retains snapshot admission/reclamation and the incoming root-schema
validation/settlement events, plus the binary-worker activity-drain wait. The
seven pre-existing uncommitted files were restored from a retained backup stash.
The branch still contains substantial undelivered recovery work; it must not be
confused with accepted main behavior. Both issues remain open. Draft PR #123 now consolidates this branch for
review and full CI. The issue #93 checklist trails the repository's merged
section 5/6 implementation state; reconcile it at delivery.

| Area | Evidence and remaining work |
| --- | --- |
| #93 rewind/snapshots, tasks 2.4/3.2–3.4 and M2–M8 | Local worker origins, reviewed keep/rollback, paired Store settlement, native acknowledgements, CLI/TUI recovery and several payload-reclamation paths exist. Managed Git partial-kill and fully prepared cancellation cleanup acceptance pass locally, including typed private links, but default Git admission remains gated. Finish partial resource construction, complete reference/CAS/metadata and managed/Git reclamation, remaining crash cuts, platform/client acceptance and delivery. Preserve legacy unknown fences. |
| #93 rules/context, tasks 4.1–4.4 | Local `instruction_limits` validation and runtime/doctor wiring now bound complete rendered baselines/reminders, including labels/notices and bounded file lists with exact delivery identities. Local fenced/trusted rule discovery, validated relative globs, baseline/scoped-read separation and typed delivery bindings now exist. Built-in reads, edits and patches, including intercepted shell patches, use actual native file receipts after hook rewriting; nested delivery reloads settings and checks Location, native root/file binding and Epoch before discovery and settlement. Durable rule delivery records the native operation and dispatched attempt. A partial patch can deliver rules for completed files. Finish exact config-generation and revocation/restart provenance. The authenticated Session context route and TUI `/context` now show stored source groups, projected token estimates, measured usage and pending Epoch state. The default response hides stored text. Recorded source provenance/status, unavailable retention and deferred style updates now survive restart. The last prepared request includes complete assembled input estimates. Finish next-request projection, revocation and remaining pending source changes, and complete leakage and native acceptance. |
| #93 structured results, tasks 5.1–5.4 | Local functional coverage now includes compaction, retry/budget/interruption handling, terminal-writer recovery, physical Store reopen after committed acknowledgement loss, and atomic ownership under steering. Five real CLI cases passed previously and all 17 current structured-root cases pass; current-source platform acceptance remains required before accepting 5.4. |
| #93 JSONL, tasks 6.1–6.4 | #117 merged basic typed parsing, correlation, replies and EOF draining. Complete exact driver ownership, bounded retained state and output backpressure, hook-event output, safe atomic last-message publication, and steering/capacity/SIGINT/restart/lost-ack/shared-Session acceptance. Task 6.2 is reopened on inspected code evidence below. |
| #92 native enforcement, tasks 2/N/R | Foundation/probe evidence exists, but production launch still uses the legacy sandbox route. Finish generation/image-bound native admission, revocation/quarantine/lifetime, independently intersected capabilities, real tool execution and native runtime/platform positives/denials. Linux R1 direction/support review and later R6 activation remain distinct gates. |
| #92 protocol/callbacks/producers, tasks 3–6/S/D | Finish contribution ownership, hooks/providers/catalog transforms and TypeScript kit, durable scoped callbacks (`session/prompt` still explicitly refuses), Setup and DirectoryAdded plus combined producer acceptance. Preserve accepted FileChanged and CwdChanged fixes. Complete S1 epoch/directory-key/legacy-deletion and D1 directory-admission/Safe Boundary/child-policy review before their dependent code. |

### Stored context inspection continuation

The authenticated context route now adds source-group estimates from the shared
persisted system text and replay projection. Default responses omit baseline,
summary and task text. Explicit `include_content=true` restores those fields.
The TUI `/context` panel fences replies by Session, Location and request identity.
Epoch rebuild status uses the runtime's provider/family/prompt-version predicate.
These estimates exclude tool schemas and turn-only additions. Individual source
provenance and pending source changes remain required by task 4.3.

Local verification passed 18 application instruction tests, 13 server context
unit tests, two HTTP context tests, ten TUI context tests, the OpenAPI artifact
check, SDK typecheck and 136 SDK tests (ten configured skips). Package library
Clippy passed after routing inspection through the existing loader. Strict
OpenSpec validation passed 100 items and lint reported zero errors/25 warnings.
Native macOS C deprecation and linker unwind-size warnings were observed.
The native run for preceding revision `a4d28df4` completed as cancelled.
This continuation has no native/platform acceptance.

### Recorded source provenance continuation

Epoch events now capture source origin, observation status and bounded file
provenance from the actual observation. A separate Epoch-bound observation event
records metadata changes without adding transcript messages. Unavailable sources
retain the admitted value and its provenance. Historical sources without metadata
report unknown origin and status. Tool-host overrides record their own origin and
drop file provenance from the replaced built-in source. Deferred output-style
changes carry a pending flag.

Inspection lists up to 256 sources, largest first, with explicit omitted counts.
Each file inventory is bounded to 128 paths and 16 KiB. Content and unavailable
diagnostic text stay outside the metadata response. The next-request estimate,
config/revocation provenance and complete pending-change acceptance remain open.

Local verification passed all 24 context/compaction integration cases, the
standalone authenticated HTTP inspection case, the source-path bounds unit case,
OpenAPI regeneration and SDK consistency/typecheck with 136 passing tests and ten
configured skips. Server/TUI all-target Clippy and both focused TUI rendering/stale-reply tests passed. Strict OpenSpec passed all 100
items; cross-spec lint reported zero errors and 25 warnings. Native workflow YAML
parsed successfully. The focused review corrected source attribution when a tool
host replaces a built-in source, retained unavailable provenance, and source
sorting before response truncation. Platform acceptance remains open.

### Prepared request estimate continuation

The final request assembly now captures its input estimate in the durable
`StepStarted` event. It includes projected attachments, structured-output
instructions, deferred tool descriptions and callable schemas. Replay retains
only the most recent estimate in Session state. Historical step events retain
an explicit unknown estimate. API/TUI inspection identifies the prepared step,
model and Epoch separately from the current stored projection and measured
provider usage. It performs no new source observations or model calls.

The scripted-provider regression compares the estimate with the serialized
request that the adapter actually received, including a large structured-output
schema, and verifies restart without dispatch. All 17 structured-root and 24
context/compaction cases pass locally. Complete next-request projection remains
open because configuration, tool visibility and pending sources can change at
the next ordinary request boundary.

The authenticated HTTP metadata/content case, the TUI prepared-request rendering
case, OpenAPI regeneration and SDK consistency/typecheck passed. SDK tests passed
136 cases with ten configured skips. Server/TUI all-target Clippy passed with
warnings denied. Strict OpenSpec passed 100 items and lint reported zero errors
with 25 warnings. Native C deprecation and linker unwind-size warnings were
observed during compilation. Current-source native acceptance remains required.

### Consolidated PR and Windows core lint

Draft PR [#123](https://github.com/CyberdyneCorp/cybercode-cli/pull/123) carries
the remaining goal implementation. Its first full CI revision `64052cba` failed
Windows binary-core Clippy because four acknowledgement-retirement helpers were
compiled without their Linux/macOS native consumers. The helpers now use the same
platform condition as those consumers. Shared acknowledgement verification stays
available to its existing callers.

The binary proof workflow now also watches the worktree source subtree, which
contains these shared core dependencies. Windows verification of this correction
requires the new revision's CI; the local toolchain has only a macOS target.
Local core library/test Clippy passed with warnings denied, and 17 snapshot
acknowledgement/publication/retirement tests passed. Two ignored entries are
subprocess fixtures invoked by the process-kill parent tests. Workflow YAML,
formatting and diff checks passed. The other checks and full-goal acceptance
remain open.

### Windows snapshot fixture lint

The Windows binary proof on `2a628f12` passed core Clippy and all 25 selected
binary tests without skips. The broader Windows workspace lint then failed on
`Repo::shadow_directory` and `Repo::shadow_object` in the Git snapshot test
fixture. Their corruption-test callers are Unix-only, so the helpers now use the
same condition. Local snapshot all-target Clippy passed with warnings denied.
All 22 Git snapshot integration tests passed; the ignored entry is the pathspec
subprocess fixture invoked by its parent test. Formatting and diff checks passed.
Current-head Windows lint must verify this separate correction.

### 2026-11-11 ownership and CI follow-up

The native snapshot matrix on `90234d37` passed Linux x64/ARM and macOS
Intel/ARM. Full workspace CI exposed separate command/localization and doctor
assertion gaps, another Windows-only fixture lint issue, a snapshot disposal race,
and browser/terminal lifecycle acceptance failures. Current source adds the
`/context` catalog entry and Spanish/French/Portuguese help text, aligns doctor
assertions with the approved `instruction_limits` contract, and gates native TUI
fixture helpers with their callers.

Snapshot disposal now records uncertainty before releasing native ownership.
Worker guards and admission release before activity completion, and the runtime
shutdown barrier releases last. A deterministic lifetime-disposal gate fails with
the previous order and passes with the correction. All 456 server library cases
pass; the ignored entry is a subprocess fixture invoked by Location-lock tests.
Partial resource-construction acceptance remains open under M2.

Runtime construction now uses an ordered partial owner immediately after activity
reservation, for both ordinary restore and recovery workers. Failed checkout
acquisition, checkout proof validation, resource publication and requester
cancellation retain the lifetime/admission/shutdown barrier through disposal.
Uncertainty is recorded before the retained resources release; activity completion
follows their actual disposal. Completed construction transfers those same guards
to the worker. A gated-lifetime regression failed on the previous order and now
passes acquisition failure and requester cancellation for both entry points.
The complete server library suite passes 457 cases with one ignored subprocess
fixture. Native partial-journal construction/crash acceptance and M2 remain open.
Server all-target Clippy passes with warnings denied. Strict OpenSpec validation
passes 100 items; lint reports zero errors and 25 warnings. Formatting and diff
checks pass. The focused code-review-skill review covers the construction owner,
both callers, regression fixtures and these evidence updates; whole-branch review
and current-source native/platform execution remain required.

The production-TUI preparation fixture now injects one typed busy refusal for
metadata acknowledgement. Its explicit retries look up the same retained request
before each new acknowledgement attempt. It still requires one cancellation
confirmation and one successful native acknowledgement, and preserves the Rust
fixture's file/receipt/transcript assertions. The real PTY case passes locally
with both pre-origin and published-origin preparations. Full concurrent CI
acceptance remains required for the observed timeout.

Process and subagent Jobs now retain optional `input_origin`: the original
assistant step/call, dispatched attempt/operation key and complete server-recorded
input cohort. Capture verifies the current dispatched call and reads its immutable
step evidence before startup. Detached and historical Jobs retain unknown
provenance. Actual built-in process/subagent regressions pass 24 cases, including
handback retention and refusal to infer ownership from a previous prompt. JSONL
background selection, result retirement and scoped cancellation remain open.

The new authenticated `POST /jobs/input-state` read query accepts at most 32
unique bounded input identities and aggregates complete-cohort Job ownership at
the Store. Its response has one small record per input and contains no Job
payloads/history pages. It excludes foreign/mixed cohorts and missing, malformed,
duplicate or oversized provenance. Three query regressions, all 82 server HTTP
cases and all 12 background Bash cases pass, including an actual dispatched Job
before and after handback. Generated OpenAPI/SDK consistency, SDK typecheck and
136 SDK cases pass with ten configured skips. Server/tools all-target Clippy
passes with warnings denied. Strict OpenSpec passes 100 items; spec lint reports
zero errors and 25 warnings. Native C/linker warnings were observed during builds.
Focused review covers the query, route, generated API/SDK, fixtures and evidence.
The query does not establish processing of a queued handback, and the CLI has not
adopted it yet. Exact background/descendant ownership, retirement and cancellation
remain open. The native matrix now includes the query regressions; current-source
platform execution and consolidated review remain required.

Handback admission now commits atomically with notifier acceptance before waking
inference. A rejected notification write rolls back the handback and starts no
model request; exact retry still reconciles historical split admissions. Requests
and nested Jobs resolve their original inputs through earlier matching durable
admission/notification and immutable Job-origin evidence. Names/source labels,
late notifications, changed origins and unknown/oversized cohorts grant no
inferred ancestry. The complete union includes any later steering and remains
bounded; no in-memory history map is added.

The original handback-cohort regression failed before the change. All 463 server
library tests pass with one ignored subprocess fixture; all 14 Job and 22 request
integration cases pass. The six final provenance tests and 26 actual
process/subagent cases pass, including an original-input permission after handback
and a nested running Job retained by the original input's bounded query. Final
server/tools all-target Clippy passes with warnings denied. Strict OpenSpec passes
100 items; lint reports zero errors/25 warnings. Formatting, diff and workflow
YAML checks pass. Native C deprecation warnings were observed. Focused review
covers these ownership/transaction changes and fixtures. The native matrix now
also runs the notifier rollback/recovery suite. CLI retirement, handback completion,
event correlation, scoped interruption/presence and consolidated/native acceptance
remain open; task 6.2 is not complete.

The JSONL client now uses bounded input-owned Job observations and exact stored
step provenance, removing its all-new-Session-Jobs capture and history scan.
Answered inputs retain their pending slots through running Jobs, unadmitted
notices, pending handback steps and an unread Store revision. The query returns
`handback_pending`, authenticated `handback_seq` and a snapshot-consistent
`session_seq`; the step-origin route
returns only bounded IDs. The ready answer boundary must follow the latest
actual notifier-accepted handback. A forged handback name cannot extend Job
ownership. Text and durable failure handling use owned steps.
Unproven completion reports an error with retained-work diagnostics. A completed
driver can retire while another driver's question remains active.

An owned Session connection now replays from the initial revision with opt-in
`live=true` for idle/delta/error notices. Its live frames cannot advance the durable
SSE cursor; the default feed keeps its existing behavior. The dispatcher separates
source selection, ownership checking, text recording and retirement. The final
Clippy complexity checks pass without suppressions after applying the
cognitive-complexity skill's guidance with Clippy's Rust analyzer.

Local verification passes 465 server library tests with one ignored subprocess
fixture, all 84 HTTP tests including TCP authentication and live cursor retention,
all 101 CLI binary unit tests and 15 real JSONL cases. The new real-binary case
gates both the process and final handback inference and requires the final answer
under the original input identity. Further cases cover EOF during an owned
handback question and a valid terminal object with a null field. Unanswered halted inputs
fail instead of publishing a partial success. Shared-Session controllers prove that unread
answer revisions hold a slot and that retirement leaves another driver's question
pending. OpenAPI/SDK consistency, SDK typecheck and 136 SDK cases pass with ten
configured skips. Server/client/CLI all-target Clippy passes with warnings denied;
strict OpenSpec passes 100 items and lint reports zero errors/25 warnings.
Formatting, diff and workflow YAML checks pass. Native C/linker warnings were
observed. Focused review covers the changed client/runtime/API/stream/generated
artifacts and fixtures. Native filters include the shared stream/API dependencies.
Scoped interruption/presence, descendant work/asks, broader event correlation,
output backpressure, restart/lost-ack acceptance and consolidated/native review
remain open; no parent task is accepted from this increment.

Focused command-catalog, all-locale and eight doctor cases pass. The browser
monitor fixture settles its owned in-flight Goal turn before independently
checking headless request refusal; its E2E case passes three repetitions and its
TypeScript check passes. Workspace all-target Clippy passed, followed by final
server Clippy and process/subagent tests after tightening the dispatch lookup.
OpenAPI/SDK consistency, SDK typecheck and 136 SDK tests pass with ten configured
skips. Strict OpenSpec passes 100 items; lint reports zero errors/25 warnings.
Native compiler/linker warnings were observed. Current-head platform CI and
consolidated review remain required.

### Zero-byte ordinary restore continuation

Ordinary cleanup now records payload completion for exact empty inventories and
zero-length displaced files, allowing periodic maintenance to retire their native
metadata. Actual App regressions cover restored file creation, displaced empty
files and unchanged restores; all three failed before the fix and pass afterward.
An unchanged restore requires its non-admitted phase, while changed restores still
require source admission. Unknown construction, live owner locks, contradictory
phase evidence and unavailable/disabled/zero-retention policy remain fences.
Cancelled deletion-only preparations and kept creations killed after filesystem
completion now use the same empty-plan protocol. Four real CLI cases cover both
file-copy and managed Git ownership, durable plan interruption/resumption, exact
claim/worker retirement and receipt acknowledgement after source replacement by
an unreadable later-owner symlink. Construction, removed-Location and complete
crash/platform acceptance remain required; no parent task or delivery is complete
from these increments.

Native run [42774849027](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/42774849027)
at `b42f5fca` completed successfully on Linux x64/ARM and macOS ARM/Intel. This is
source-bound evidence for that checkpoint, not acceptance of later managed
retirement/process-cut or zero-byte changes. Main was fetched for this continuation;
`8266db6c` remains fully integrated.

Run 42784209997 at `991b1519` subsequently fails the managed Git App setup on both
Linux platforms: the retained fixture root makes the sandbox proxy's Unix socket
path exceed the native address limit. The fixture root is now shortened under
`/tmp/c<workflow-run-number>`, including nested process-cut fixtures, with the
artifact upload following the same root. No network policy or production socket
code changes. The workflow also runs the four zero-payload CLI cases and retains
their log. This resolves the inspected fixture routing problem in configuration;
fresh-source native execution is still required to prove acceptance.

Run 42790242409 at `af0d10ed` passes both macOS jobs but still fails managed
process-cut setup on Linux: overriding the child's `TMPDIR` with the control
directory adds another temporary-directory component. The child now inherits
the short workflow root. Its fixture independently retains failures and records
its actual root for reopening; both sibling roots remain covered by artifact
upload. Linux execution of this correction remains required.

### Location continuity acceptance

Location continuity acceptance now exercises six combinations of missing,
replacement or protected alias routes with payload cleanup or a committed metadata
plan. The same App/serialized Store retires another settled Location while preserving
the unavailable owner's exact journal/worker bytes and event sequences. Returning
the original directory resumes cleanup and data-only receipt acknowledgement.
A further race moves the Location after the actual active-intent unlink and resumes
the identical plan through Store discovery. Later source edits remain untouched.
Permanent-root removal still requires detached maintenance; the current live-root
checks remain in force. Failed reclamation App fixtures are retained, and their CI
step now runs independently and uploads them under the short fixture root.

### Focused code-review findings and next actions

This review covers incoming JSONL behavior and merge overlap, not acceptance of
the entire recovery branch. Rust integration tests are executable; native
cross-platform, SDK/plugin and full-workspace checks have not been rerun here.

1. **Integration regression: optional snapshot capture fails JSONL results.**
   Unchanged main passes all six JSONL tests. The integrated candidate initially
   fails four because `Driver::failed` treats `kind: snapshot` as a failed input,
   despite successful inference and ordinary exec's nonfatal warning handling.
   The local fix keeps these diagnostics nonfatal and adds a real-binary
   regression with unavailable Git snapshots. All seven JSONL tests and all
   three structured-output CLI tests pass after the fix.
2. **Partly fixed locally: direct request ownership; interruption remains Session-wide.**
   Model-step events now capture the complete active input cohort before inference.
   Direct permission/question requests expose that durable cohort for their exact
   step/call. The driver collects, replies to and closes only requests whose whole
   cohort belongs to its pending inputs. Actual shared-App controller regressions
   cover two drivers, equal external correlation labels, foreign replies and close
   while the other owner remains attached. Branch checkout and compaction retain
   the captured proof; legacy/oversized evidence grants no inferred authority.
   Session-wide interrupt and interactive presence, descendant/hook ownership,
   background-job ownership and general event/result correlation remain required.
   These direct-request checks do not accept task 6.2 or complete shared-Session
   isolation.
3. **Partly fixed locally: completed history and blocking output.**
   Inputs now occupy at most 32 pending message-ID slots; emitted text/structured
   results retire into bounded counters and first-failure accounting. Admission
   ordinals preserve pending result order on premature termination. The reporter
   releases completed answer/tool/denial payloads, and background tracking retains
   one exact fork-handback bit rather than every observed admission. Thirty-one
   exec unit tests, 17 delegation tests, five forked-command tests and nine JSONL
   tests pass, including 65 sequential prompts. `Out::line` still writes/flushes
   stdout synchronously on the control/lease loop. Complete bounded output
   ownership with explicit I/O-failure settlement, bounded background Job lookup,
   slow-reader/broken-pipe acceptance and exact shared-Session ownership.
4. **Fixed locally: last-message publication used a predictable temporary path.**
   Publication now uses an exclusively created sibling file, synchronizes it and
   atomically replaces the destination, with automatic temporary-file cleanup on
   failure. Three unit regressions cover pre-created temporary/destination links,
   concurrent complete answers and cleanup after failed publication. All eight
   real JSONL tests pass, including failed publication preserving the answer while
   returning aggregate exit 1 and an `end` error. Interrupted/protocol exit codes
   retain precedence. This does not complete the ownership/backpressure findings.

5. **Fixed locally: fallback collection ignored disabled retention policy.**
   Complete manifest/entry preflight now binds the namespace Location, protects
   disabled/zero-retention data, honors configured orphan aging and rechecks policy
   before each unlink. Final fallback/App integration coverage passes 44 cases;
   durable collection/retirement evidence and full metadata/CAS cleanup remain open.
   Native run 42728260672 at 1be03642 passes both Linux jobs but fails managed Git
   cleanup-plan preparation on macOS ARM because of an unidentified unknown entry.
   Keep that fence: the diagnostic now quotes the leaf, failed CLI fixture roots
   are retained/uploaded, and independent structured/JSONL checks no longer depend
   on earlier recovery success. This is an open construction/acceptance failure,
   not a timeout or an accepted native result.

6. **Owned intent updates now avoid untracked source-phase temporary files.**
   Version-7 prepared journals bind two preallocated native inodes before source
   admission. Updates synchronize the spare, exchange the known names and sync
   the directory; reopen and cleanup verify both identities. Cleanup also pins
   both metadata generations across its durable plan and every payload unlink.
   A real subprocess kill during a torn spare write preserves the active record
   and permits exact metadata continuation after reopening. Legacy unknown files
   remain fenced; this is not complete construction or metadata retirement.
   The broad parallel CLI run failed three old-version assertions (corrected),
   two preparation observation deadlines and one typed storage-busy TUI response.
   The three preparation/TUI cases pass serially; this does not establish the
   cause of the parallel failures. The complete serial CLI rerun at `2ec1b1c5`
   passes all 22 cases in 837.69s, including both managed Git process-death cases
   and TUI reconciliation. SDK generation/typecheck and 136 SDK tests also pass
   at that checkpoint, with ten configured external skips.

7. **Collection now retains durable policy provenance across manifest deletion.**
   A bounded immutable Location/root/native-namespace record is synchronized before
   deletion and pinned across every unlink. A fresh collector can resume orphan
   cleanup after the last manifest disappears, while disabled/zero retention,
   unavailable configuration, unknown provenance and metadata changes preserve
   remaining bytes. Every protected copy root must resolve to a valid manifest
   somewhere in the private global inventory before collection; valid roots in
   another Location do not block unrelated orphan cleanup. Namespace metadata and
   unpublished construction bytes now count toward quota. Real process-kill,
   restart, alias, namespace-transplant and reference regressions pass locally.
   This does not retire journal/worker/checkout metadata or complete M6.

8. **Metadata retirement now joins the durable writer to exact native deletion.**
   Reclamation history distinguishes payload completion, an immutable metadata
   role/identity/generation/hash plan, and metadata completion. The App maintenance
   port commits the plan first, excludes reference writers during its guarded
   callback, checks byte totals and never repeats an observed completed callback.
   Migration catalog fencing preserves older evidence; logical history survives
   Store reopen and reprojection after later native fences or Session deletion.
   The Rust maintenance port defaults to unavailable for older implementations.
   The native adapter now pins and removes acknowledged journal, phase and lifetime
   files for empty checkout inventories. Periodic Store discovery resumes after
   active-intent deletion, including a tested real process kill; cap pressure can
   reclaim metadata before one capture retry. Completed acknowledgement returns
   the retained Store receipt without reopening deleted files. Remaining managed
   claim crash cases, partial construction and full platform acceptance remain
   open. Real prepared-cancellation and paired keep/rollback CLI fixtures now
   exercise metadata retirement and receipt acknowledgement after restart, including
   no-follow checks for removed lifetime/phase files. These positive cases do not
   establish the complete crash matrix.

   A separate real App regression reproduced 57,344 bytes of payload deletion
   when configuration resolved to null. Cleanup now preserves bytes initially and
   stops after policy loss between unlinks, retaining its exact resumable plan.
   At `053dbfac`, 37 App reclamation tests pass locally, including actual native
   deletion, tamper/alias/live-owner refusals, policy loss, process death and cap
   retry. The earlier 14 migration tests cover the writer catalog. Fresh platform
   and broader current-source verification remain required before M6 acceptance.

9. **Managed retirement now freezes complete checkout bindings before deletion.**
   A version-2 metadata plan binds each original claim and its existing consuming
   acknowledgement, native generations/hashes and persistent repository-lock
   identity. The native callback retains the original lifetime and all repository/
   leaf locks, removes acknowledgement before claim, preserves shared creation and
   unrelated activity records, and synchronizes observed absence on resumption.
   The Store writer re-attests receipt, retention and references; a new catalog
   marker fences older writers. Version-1 plans retain their original meaning.
   Real App/CLI managed ordinary restore, prepared cancellation and paired
   keep/rollback positives now remove those leaves and acknowledge from the
   retained receipt after restart without source replay. Guard reopen/cut,
   alias/context/live-owner/construction and bounded FIFO cases pass locally.
   Ordinary managed metadata deletion now also passes actual process kills after
   acknowledgement removal and after claim removal, followed by physical Store
   reopen and periodic resumption of the same plan. Broader preparation/paired
   deletion cuts, zero-payload/partial constructors, old/deleted Locations and
   source-bound platforms still gate M6 acceptance.

Native run [42733865471](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/42733865471)
passes Linux x64/ARM and macOS ARM/Intel at `e2b3ba18`, including the earlier
managed Git cleanup and structured CLI cases. It predates the owned intent-slot
changes and is not evidence for them. The workflow now also selects the three
pre-effect preparation/TUI client cases serially and retains their log; that
workflow addition still requires source-bound platform acceptance.
Run [42753722365](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/42753722365)
at `2ec1b1c5` passes both Linux jobs and macOS ARM. Intel is cancelled at the
45-minute job limit while compiling the final structured-result binaries; its
snapshot, core, App, managed Git, preparation/TUI and JSONL steps pass. This is
incomplete platform evidence, not a passing matrix. The workflow job allowance is
now 60 minutes; individual test deadlines and assertions are unchanged. The run
predates the collection-provenance continuation, whose source at `124c11b9` has
its own run [42759174937](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/42759174937),
now successful on all four Linux/macOS platforms. It predates the metadata writer
protocol and payload-policy fix; fresh source-bound acceptance remains required.
The subsequent run [42759434324](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/42759434324)
at `50e00372` passes both Linux jobs and macOS ARM, but Intel fails managed Git
prepared cancellation because its preparation origin is not observed. Its private
fixture is retained/uploaded; independent client/JSONL/structured checks pass.
That acceptance failure remains unresolved. The queued writer run at `5be366b7`
and new native retirement need their own evidence.

The retained Intel fixture shows a 40,253 ms preview and no published preparation
origin before the later confirmation observation failure. Raw Git publication
performed one blob-writing process per path, even for identical captured bytes.
It now writes each distinct content once within that capture while retaining every
path/mode and existing source/policy/hash checks. The real managed cancellation
case passes locally with its origin observed in 14.6 seconds under the unchanged
45-second deadline. This is a local correction and diagnostic improvement; fresh
Intel/native acceptance remains required before resolving the platform failure.

Verification: three structured-output CLI tests, seven JSONL CLI tests, five
structured-result server tests and six binary-worker ownership tests pass;
formatting and whitespace
checks pass. CI-pinned OpenSpec 1.13.2 validates 100 items after the incoming
archives; spec lint reports zero errors and 25 warnings. The preparation case now
passes with an independent staging deadline. A separate unconstrained-run
first-prompt startup stall remains unresolved; its isolated rerun and the complete
suite with four parallel threads pass. The stalled fixture's diagnostics are
retained rather than counted as a passing run.
The integration regression is locally corrected; finish the current recovery
delivery increment before starting another broad implementation stream.

The full approved scope from PRs #53/#54 remains required. Its original delivery order was:

1. [Plugins and hooks](../openspec/changes/complete-plugin-hook-runtime/tasks.md):
   production native capability enforcement, complete host protocol operations,
   scoped prompt callbacks and missing hook-event producers.
2. [Daily coding](../openspec/changes/complete-daily-coding-surfaces/tasks.md):
   rewind/undo/redo, non-Git snapshots, path-scoped rules and context inspection,
   root structured output and streaming JSONL input.
3. [Binary hex editor](../openspec/changes/add-binary-hex-editor/tasks.md):
   the full reviewed CLI/TUI editor, owner/SDK/model operations and integrations.

On 2026-11-01 the user explicitly requested implementation of [binary editor
issue #94](https://github.com/CyberdyneCorp/cybercode-cli/issues/94) now. This
supersedes the scheduling dependency on completing the first two workstreams;
implement the binary editor while preserving all shared snapshot, permission,
native-save and FileChanged contracts. Remaining plugins/hooks and daily coding
stay tracked in issues #92 and #93. Their separate pending policy decisions are
not selected by this scheduling change. The binary workspace implementation is
partial; its live view does not complete the editor goal.

On 2026-11-03 the user requested merging #105 and moving to [daily coding
issue #93](https://github.com/CyberdyneCorp/cybercode-cli/issues/93). Daily coding
is now the next priority after publishing the already tested manual-draft CLI
recovery increment. This supersedes the old requirement to finish #92 before
starting #93. Plugins/hooks remain incomplete; their pending design decisions
and enforcement guarantees are not implicitly accepted. Remaining binary
retention/checkpoint/rewind integration and UX/performance acceptance are also
unfinished despite the owner's manual closure of #94. Follow the approved daily
coding implementation order, beginning with rewind preview/confirmation; preserve
shared snapshot, permission, recovery and FileChanged contracts.

## Delivery constraint approved on 2026-10-29

The user requested fewer simultaneously open PRs. Keep normally **one open goal
PR**, with a **maximum of two** only when an independent change genuinely needs
separate review. This changes delivery granularity, not feature scope or acceptance.

- Finish and verify the agreed delivery increment before opening the next PR.
  On 2026-10-30 the user authorized merging #76 as a foundations milestone once
  its CI is green and merge review is satisfied, then continuing the full goal
  in one follow-up PR. This milestone does not complete production enforcement.
- Keep related experiments, evidence, implementation fixes and CI fixes in the
  active PR. Do not open a PR for each probe or supporting component.
- Cancel superseded PR/revision CI runs after verifying their source is retained.
  Main CI, like the focused workflows, cancels older runs for the same PR/ref;
  completed evidence remains associated with its actual tested revision.
- Consolidate the existing #70–#76 stack into draft #76 against `main`, retaining
  the contained commits, source branches and evidence. Close #70–#75 as superseded
  after verifying their heads are contained in the consolidated branch.
- Use the active PR description to distinguish production behavior, experimental
  candidates, verified results and remaining requirements. Keep it draft while
  its delivery increment is incomplete.
- Do not open the next implementation PR until the current increment is merged,
  superseded or deliberately split for independent review within the cap.
- Prioritize fixing failing checks and making the active increment ready for
  review before starting another delivery increment. Report pending and failing
  checks explicitly; do not describe the active PR as ready while they remain.
- Count goal PRs separately from unrelated repository PRs. Inspect ownership and
  scope before consolidating or closing other contributors' work; the delivery
  cap does not authorize closing unrelated PRs.
- Preserve native proof and explicit support/design revision gates. Passing probes
  are not production feature acceptance. Never weaken the agreed contract merely
  to finish a PR. Merge only with user authorization; the conditional approval
  above originally applied to #76. On 2026-10-30 the user additionally authorized
  merging ready repository PRs as work continues. Check current-head CI, scope,
  review and mergeability before each merge, and recheck composition as main
  advances. This does not waive draft, native proof or design-revision gates.

PR [#76](https://github.com/CyberdyneCorp/cybercode-cli/pull/76) merged on
2026-10-30 at `ec31ff9fc7627a3a9262d99f0fc0606394368423`, after all 70 applicable
checks passed and the optional diagnostic was intentionally skipped. Its merge
tree matches the reviewed/tested `e6fd3d2f` tree. The user approved continuing the
[Linux broker direction](../openspec/changes/complete-plugin-hook-runtime/native-host-integration.md)
with runtime kernel capability checks and full private IPC/network compatibility
remaining required before activation. The original tested sources remain on
`evidence/plugin-native-host-pre-76-merge`. Follow-up #80 merged at `2745b4f0`
and #83 at `c3bd34c4` on 2026-10-31. These are foundations, diagnostics and the
observer resource correction; production native admission remains incomplete.
PR #84 merged at `dd0c9279` after all 55 configured checks passed on
`62a3068e`, with one intentional historical diagnostic skip. Its corrected
native result witness and deterministic missing-guard regression passed Intel
and arm64; that accepts the verifier correction rather than production native
admission or the historical unknown race cause.

PR #85 merged at `5d5826b7` after all 19 checks passed. It accepts bounded
FileChanged task 5.2 with verified native Windows retarget coverage and a reviewed
main composition; see the
[acceptance audit](../openspec/changes/complete-plugin-hook-runtime/file-observation-acceptance.md).
PR #88 merged at `9cbbf08b` after all 18 checks passed, including all five
CwdChanged lifecycle cases on Windows.

PR #89 merged at `bbc233d5` after all 16 current-head checks passed. Its
[receipt](../openspec/changes/complete-plugin-hook-runtime/evidence/cwd-settlement-9ad0b369.json)
verifies the original unacknowledged Location-settlement regression actually
executed on Linux/macOS/Windows. This accepts its simulated lease-owner case,
not native confinement or the complete producer matrix.

PR #90 merged at `5ed50bff` after all 16 checks passed. Its
[source-bound receipt](../openspec/changes/complete-plugin-hook-runtime/evidence/store-commit-bb91e32d.json)
verifies both commit-boundary cases and runtime lost-ack/replay behavior on
Linux/macOS/Windows, with suite totals and the unverified Windows leaked-process
warning retained. It accepts that correction, not the full producer matrix.
The replay case uses a fresh runtime over the same Store rather than a process
restart.

Continue review on `spec/setup-legacy-admission-review`. The
[legacy Setup owner-resolution candidate](../openspec/changes/complete-plugin-hook-runtime/setup-legacy-bootstrap-review.md)
proposes the missing migration policy and its compatibility exception. The
complete S1 decision is pending; publishing or merging the proposal does not
approve it or implement markers/init. Preserve the separate Linux support and
DirectoryAdded design-review gates. No dependent behavior or native activation
is selected by this documentation follow-up.

Native confinement foundations and regressions exist, but
production enforcement and the remaining plugin/hook scope are incomplete. The
Linux UNIX-socket diagnostic found a real host-reachability gap. Its experimental
filter and runtime checks remain experimental. The broker direction is approved;
complete support acceptance and host activation remain pending. Daily coding now includes reviewed CLI/TUI rewind and the initial private non-Git
snapshot backend (PRs #107–#110). Managed recovery resolution, native
acknowledgement/reclamation, path rules/context, root structured output and
streaming JSONL remain unfinished. Binary editor implementation began
on 2026-11-01 under the scheduling override above and remains partial. The linked task lists remain authoritative for completion.

The [Linux peer-identity diagnostic](../openspec/changes/complete-plugin-hook-runtime/linux-private-peer-identity.md)
confirms that the current broker changes the caller PID observed by a UNIX
listener, despite successful data exchange. The
[kernel UNIX revision](../openspec/changes/complete-plugin-hook-runtime/linux-kernel-unix-revision.md)
is a concrete proposal awaiting review. It does not reduce IPC requirements or
authorize a new production support floor. Preserve this review gate before
production activation or protocol-feature integration.


The current user request prioritizes finishing Recovery/snapshots and Daily coding
as listed in the roadmap. Continue with exact-worker managed recovery, safe cleanup
and native acceptance, then the already approved rules/context, structured-output
and JSONL scope. PR #114 implements immutable Git exclusion history and proposes the
[concrete recovery protocol](../openspec/changes/complete-daily-coding-surfaces/managed-recovery.md)
for the required design review; that protocol is not implemented or approved by
publication alone. Issue #93 was automatically closed by #110 despite remaining
scope and has been reopened. The owner's closed #94 is unchanged. Keep the existing
one-PR delivery constraint and separate Plugins/hooks policy gates.

On 2026-11-06 the user approved the concrete managed-recovery protocol and requested
implementation. Its worker and file-copy journal prerequisites are in progress;
keep/rollback, reconciliation, Git journaling, reclamation and full native/client
acceptance remain required. Approval and current evidence are recorded in the daily
coding change's tasks.md and implementation.md; publishing a prerequisite does not
complete recovery or either issue.


The local pre-effect recovery continuation adds version-2 native source phases,
positive cancellation of an exited published origin before source admission, and
metadata acknowledgement reconciliation. Database and native admission evidence
must both permit cancellation; a database backup cannot erase native admission.
Original intents and payloads remain retained. This is progress on the approved
recovery protocol, with independent checkout ownership, Git journaling, safe cleanup,
complete clients and full platform/cut acceptance still required before delivery.


The next local ownership continuation moves snapshot and recovery checkout claims
from Session IDs to independent native activity IDs through a dedicated host port.
CLI/App retain those locks through settlement; unsupported managed hosts refuse.
Unknown checkout claims remain protected. Exact interrupted-claim proof and
reconciliation are still required before accepting managed resource recovery.


Acquired snapshot checkout claims now carry pinned native route/creation evidence.
The actual worker and consuming Store writer verify and retain that proof; aliases,
replacement and changed records refuse. Binding publication before interrupted
acquisition and recovery-owned native acknowledgement remain required. Git rewind
still follows its legacy path; compatibility tests do not establish owned Git recovery.


The local initialized-publication continuation adds staged no-replace checkout
claims and exact lifetime context. Runtime verifies complete managed bindings before
native settlement; prior writer catalogs refuse mutation. Construction kills retain
evidence and refuse replay. Unknown-claim acknowledgement and reclamation remain
incomplete, and Windows lifetime-bound durability is not yet admitted. This does not
accept the complete recovery feature or either issue.


The local acquired-resource continuation adds exact read-only exited-claim
reacquisition and durable complete checkout references before source dispatch.
Publication failure preserves the unknown reservation; no new native acknowledgement
or cleanup authority is granted. Interrupted-claim consuming receipts and native
reconciliation remain the next recovery work, alongside the existing Git,
reclamation and platform/client acceptance obligations.


Preparation receipts now consume a digest of the complete acquired checkout set,
with actual original claim locks retained through paired commit/reply. CLI/App
reacquire via an explicit read-only host port; lookup remains database-only and
journal metadata reconciliation checks the committed resource binding. Original
native checkout acknowledgement, partial-constructor recovery, Git journaling and
reference-aware cleanup remain required before the complete recovery increment is
reviewable for delivery.


The pre-effect native checkout acknowledgement continuation publishes an exact
receipt-bound disposition while preserving original claim bytes and independent
unknown fences. Complete resource publication now precedes checkout admission
settlement. Candidate journal/checkout receipts are checked before the paired
commit, and interrupted publication resumes only through explicit metadata
reconciliation. Acknowledged creation metadata is retained across authorized
worktree removal/recreation. Partial acquisition/corrupt construction recovery,
post-effect/owned Git integration, reference-aware reclamation and full native/
client acceptance remain open; this does not complete the recovery delivery increment.


The recovery consuming-receipt continuation binds both retained checkout owners to
the durable pair and publishes exact resolution-then-original native disposition
markers only after observing commit. Explicit reconciliation reacquires both
published resource sets without source replay; original claim bytes and legacy
unknown fences remain protected. This is local implementation progress, not full
recovery delivery: actual managed Git source callbacks, partial construction,
reference-aware reclamation and complete native/platform/client acceptance remain
open. See the daily coding implementation evidence and authoritative parent tasks.

The non-default Git candidate now exercises an actual ordinary owned restore in a
managed checkout created under the default application data directory. Source
observation is registration-bound; source intents require the runtime worker's
complete retained native checkout proof. Admission releases the Session mutex for
host policy lookup and fences captured revisions. The App verifies persisted
resources, unchanged user Git metadata and reopened acknowledgement without source
replay. Default Git admission stays off. Managed partial replacement/recovery,
construction/reclamation and complete native/platform acceptance remain required;
this positive does not complete recovery or either issue.

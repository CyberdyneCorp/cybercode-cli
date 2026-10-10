## ADDED Requirements

### Requirement: Bounded peer publication scheduling before feedback
(P1) Owned language-server workers SHALL service already ready peer messages before queued feedback observation commands so a slow publication authority/checkout admission does not leave an already buffered diagnostic batch behind complete edited-file feedback. Peer priority SHALL be bounded: after sixteen peer messages the worker SHALL offer a queued command a turn before reading further peer messages. Health/review events SHALL NOT reset this burst counter. Queued commands SHALL retain existing fresh authority and document admission. Cancellation SHALL take priority without consuming queued commands; closed command queues SHALL stop the worker at the fairness boundary. This ordering SHALL NOT infer freshness of delayed unversioned publications, extend the configured diagnostic deadline, reset document versions, or weaken scope/ownership checks.

#### Scenario: Slow publication admission with queued feedback
- **WHEN** the first publication's admission holds the worker beyond the shared save quiet window while other-file publications and a feedback command are already queued
- **THEN** those ready publications SHALL be processed before complete feedback observation
- **AND** write feedback SHALL include at most five newly erroneous other files

#### Scenario: Busy peer and cancellation
- **WHEN** a peer burst reaches sixteen messages with a command waiting
- **THEN** the queued command SHALL receive a turn under its existing authority checks
- **AND** cancellation SHALL return without consuming that queued command

### Requirement: Automatic sandboxed formatting after file edits
(P1) Successful write, edit, apply_patch and notebook_edit mutations SHALL run each enabled formatter matching the edited extension sequentially before owned LSP save/diagnostic feedback. Commands SHALL replace $FILE with the canonical absolute edited path, use the Location directory and configured environment, and run inside the selected sandbox with owned temporary roots and a thirty-second timeout each. Automatic formatting SHALL NOT approve unknown network domains or override sandbox proxy/temporary environment. Configuration/provenance and source contents SHALL be rechecked before native launch. Disabled, missing, failed and nonzero formatters SHALL be logged without failing the completed edit, and a failed formatter SHALL NOT skip a later enabled formatter. Cancellation SHALL start no later formatter and SHALL retain native/sandbox ownership until process-tree settlement. Changed content and the formatting diff SHALL be returned to the model, with control characters JSON-escaped. Source rereads SHALL refuse redirected/nonregular targets. Existing tool Location ownership and edit permission SHALL remain authoritative; formatting SHALL NOT acquire independent edit permission.

#### Scenario: Sequential formatters
- **WHEN** two enabled formatters match an edited file
- **THEN** the second SHALL observe the first formatter's content in the same Location
- **AND** the result SHALL include final content and its diff from the submitted edit

#### Scenario: Formatter failure and timeout
- **WHEN** a formatter exits nonzero or reaches its thirty-second deadline
- **THEN** its failure SHALL be logged and the completed edit SHALL remain successful
- **AND** native descendants SHALL settle before the next formatter starts

#### Scenario: Final content language-server synchronization
- **WHEN** an enabled formatter changes an edited file
- **THEN** LSP save and diagnostics SHALL use the formatted snapshot rather than the submitted unformatted bytes

#### Scenario: Cancelled formatting
- **WHEN** cancellation arrives during an automatic formatter
- **THEN** its native descendants SHALL be terminated and observed before resource disposal
- **AND** no later formatter SHALL start

### Requirement: Runtime Location activity and idle release
(P1) Language-service idle expiry SHALL account for live Location admissions rather than only read warming. Acquiring activity SHALL create no pool, discovery or native process. Admissions for the same canonical Location SHALL share an activity counter before first warming and across explicit reload. Idle expiry SHALL be disabled while any activity guard is retained, and the last disposal SHALL begin a fresh idle window. Expiry and admission SHALL serialize their counter/cancellation decision. Runtime managed and unmanaged Location leases SHALL retain activity through their operation and through retained native settlement proof disposal. Activity SHALL NOT grant filesystem or process authority, replace checkout ownership, reopen closed services, or prevent explicit close/trust revocation. Retained service/fence entries SHALL remain bounded to 128 Locations. Activity SHALL NOT consume those service slots or reject model admission when the service cache is full; tracking without retained services/fences SHALL be removed after its last guard is disposed.

#### Scenario: Active tool without reads
- **WHEN** a runtime tool is executing longer than the language-service idle interval without further read warming
- **THEN** its Location SHALL remain ineligible for idle expiry until operation admission is disposed
- **AND** disposal SHALL start a full new idle interval

#### Scenario: Retained commit proof
- **WHEN** Location native settlement returns a retained proof
- **THEN** its language-service activity SHALL remain retained until the consuming commit disposes that proof

#### Scenario: Shared activity before first warm
- **WHEN** multiple operations acquire one canonical Location before any file warming
- **THEN** acquisition SHALL NOT discover or start a language server
- **AND** only disposal of the last guard SHALL enable idle expiry

### Requirement: Explicit Location language-service transitions
(P1) The host SHALL support Location-scoped language-service close and reload. Close SHALL fence new warming before waiting and retain the same discovery/native cleanup ownership after cancellation. A closed Location SHALL remain fenced until explicit reload. Reload SHALL await acknowledged native/resource settlement, or a joined discovery failure known to have created no pool, before reopening lazy admission. Unverified joins or unacknowledged roots SHALL NOT reopen. A newer transition SHALL supersede an older pending transition without releasing its retained worker. Other Locations SHALL remain independent. Closed and active Location entries together SHALL remain bounded to 128.

#### Scenario: Cancelled close and sibling services
- **WHEN** a close caller cancels during this Location's native shutdown
- **THEN** the same worker SHALL remain retained for repeated close
- **AND** another Location SHALL remain available

#### Scenario: Reload is superseded
- **WHEN** a newer close fences the Location while reload waits for discovery
- **THEN** the older reload SHALL NOT reopen admission

#### Scenario: Known failure versus unverified ownership
- **WHEN** discovery returned an error before creating a pool and its worker has been joined
- **THEN** explicit reload SHALL permit lazy fresh discovery
- **AND** panic/unverified joins or unacknowledged native roots SHALL remain fenced

### Requirement: Authenticated language-service lifecycle API
(P1) POST /api/v1/lsp/close and POST /api/v1/lsp/reload SHALL use the authenticated Location envelope and take no request body. Successful responses SHALL report the prior generation closed and whether lazy admission was reopened. Unsettled or superseded transitions SHALL return a conflict without false closure acknowledgement; unsupported hosts SHALL return ServiceUnavailableError. Typed OpenAPI and generated SDK close/reload methods SHALL preserve authentication and Location routing without starting a language server.

#### Scenario: Explicit client transition
- **WHEN** an authenticated client closes or reloads a selected Location
- **THEN** the response SHALL retain its Location envelope and typed transition result
- **AND** reload SHALL NOT discover or launch a server until subsequent file admission

### Requirement: Document checkout admission
(P1) Background read snapshots SHALL capture both Location and document managed creation identities before bytes are read. Document commands SHALL retain their captured identities through queued startup. A running local language server SHALL revalidate the canonical document and claim every missing enclosing managed checkout before document text is sent. Claims SHALL use independent language-server ownership and remain held through native/proxy settlement and worker disposal, including after document-cache eviction. A service SHALL refuse a changed creation at an already claimed checkout path. Each service SHALL admit at most 128 distinct checkout scopes and refuse additional scopes rather than release live ownership. Close cancellation SHALL settle the native connection before waiting for an in-flight document claim and SHALL retain the claim operation until it completes.

#### Scenario: Nested document under an existing root
- **WHEN** a running server rooted outside managed checkouts opens a document inside nested managed checkouts
- **THEN** every enclosing checkout SHALL be claimed before didOpen or didChange
- **AND** checkout removal SHALL remain refused until native and proxy acknowledgement and worker disposal

#### Scenario: Nested read origin is recreated
- **WHEN** a document checkout is deleted and recreated while background discovery is delayed and its enclosing Location is unchanged
- **THEN** the original read snapshot SHALL NOT be delivered to a server for the new creation

#### Scenario: Close during document admission
- **WHEN** a document claim is blocked and the service closes
- **THEN** native descendants SHALL terminate before waiting for claim completion
- **AND** a cancelled close caller SHALL NOT discard that claim or authorize document delivery

### Requirement: Language-server managed launch pins
(P1) Local LSP launch SHALL claim every enclosing managed checkout of its Location and server root using an independent language-server identity before native spawn. It SHALL verify the complete claim set and creation identities, retain pins through native and proxy settlement, and preserve unknown activity after uncertain native spawn or unacknowledged disposal. Background read warming SHALL retain its captured Location creation identities and refuse a changed creation before document delivery.

#### Scenario: Managed server lifetime
- **WHEN** a managed Location starts a language server
- **THEN** checkout removal SHALL remain refused while native or proxy settlement is pending
- **AND** acknowledged resource settlement SHALL retain native pin locks until worker disposal

#### Scenario: Replaced background origin
- **WHEN** a read's captured managed Location differs before background warming
- **THEN** the read snapshot SHALL NOT reach a server for the replacement creation

### Requirement: Independent language-server checkout ownership
(P1) Managed checkout activity SHALL support a distinct language-server owner identity independent of Sessions and MCP connections. Claims SHALL revalidate ready creation identity under the repository removal lock. Native ownership SHALL retain checkout exclusion through acknowledged settlement. Disposal without acknowledgement SHALL leave unknown activity that prevents removal and implicit reclamation.

#### Scenario: Session finishes before language server
- **WHEN** a Session settles while its checkout remains pinned by a language-server owner
- **THEN** checkout removal SHALL remain refused until acknowledged language-server pin settlement releases its native lock

#### Scenario: Unknown language-server owner
- **WHEN** a language-server checkout lease is disposed without acknowledgement
- **THEN** removal and reclamation under that identity SHALL remain refused

### Requirement: Live LSP generation authority
(P1) Local LSP generations SHALL recheck current trust-filtered configuration before dispatching queued document, notification, diagnostic or navigation commands. Changed definitions, executable selection, resolved configuration, provenance or trust SHALL fence that root and retain process/resource settlement. Periodic idle authority reviews SHALL stop revoked roots without requiring a foreground command. Joined configuration observations SHALL retain ownership when a close caller is cancelled.

#### Scenario: Revoked running server
- **WHEN** trust or effective configuration changes after initialization
- **THEN** subsequent queued commands SHALL NOT reach the server
- **AND** the root SHALL become broken and native/resource ownership SHALL be settled

#### Scenario: Idle review
- **WHEN** a running root loses authority without another command
- **THEN** a periodic review SHALL fence and settle it
- **AND** it SHALL remain broken until its service generation is recreated

### Requirement: Owned background LSP warming
(P1) Successfully read text files SHALL enqueue background warming in a canonical Location-owned LSP generation without awaiting discovery, initialization or diagnostics. Matching installed servers SHALL receive one didOpen per document and versioned full-content didChange for changed read snapshots. Document versions SHALL increase across the entire running server generation, including eviction and reopening; unchanged snapshots SHALL consume no version. Exhausted versions SHALL refuse changed snapshots before dispatch rather than wrap or reuse a version. Bounded queues and document state SHALL prevent unbounded background admission. Host shutdown and Location idle expiry SHALL fence admission and retain native/resource cleanup ownership through joined settlement.

#### Scenario: Read remains independent of initialization
- **WHEN** a matching server initializes slowly while a text file is read
- **THEN** read SHALL return without awaiting that server
- **AND** repeated unchanged snapshots SHALL NOT duplicate didOpen

#### Scenario: Evicted document is reopened
- **WHEN** a document is evicted and later reopened in the same running server generation
- **THEN** its new didOpen version SHALL exceed every prior document version in that generation
- **AND** an unchanged subsequent snapshot SHALL NOT dispatch another notification

#### Scenario: Generation disposal
- **WHEN** host shutdown or Location idle expiry starts
- **THEN** no new warm request SHALL enter that generation
- **AND** cancelling a close caller SHALL NOT dispose retained worker/native/resource settlement

### Requirement: Trusted local language-server launch
(P1) Local LSP launch SHALL reload trust-filtered configuration, bind its canonical Location/root and refuse definitions or executables differing from the selected generation. It SHALL use shared sandbox enforcement and credential masking, prevent configured proxy/temp transport overrides, and retain process and proxy-resource settlement ownership.

#### Scenario: Fresh launch admission
- **WHEN** selected definitions change, checkout trust is revoked, the root leaves its Location or launch is cancelled before spawn
- **THEN** native execution SHALL be refused with diagnostics excluding supplied secrets
- **AND** fresh admission after preparation SHALL refuse changed resolved values, provenance or trust

#### Scenario: Enforced native launch
- **WHEN** a local server launches under workspace-write or read-only policy
- **THEN** the shared sandbox SHALL enforce workspace/protected/unreadable roots, ambient provider/catalog credential masking and managed proxy/temp environment
- **AND** unsupported enforced platforms SHALL remain fenced rather than silently opting out

#### Scenario: Owned proxy disposal
- **WHEN** native process settlement completes
- **THEN** the same retained resource lease SHALL close and join owned proxy transports before successful pool settlement
- **AND** interrupted resource waits SHALL retain their join handle and failed acknowledgement SHALL NOT become success on retry

### Requirement: Continuous owned LSP message handling
(P1) An initialized LSP worker SHALL continuously consume bounded server frames while idle, retaining framing-reader ownership independently of command selection. Notifications SHALL remain bounded and untrusted until document/path/version validation. Unsupported server requests SHALL NOT authorize edits; malformed or unsolicited idle responses SHALL break the root.

#### Scenario: Idle message delivery
- **WHEN** an initialized server publishes diagnostics or sends a server request without a client RPC in progress
- **THEN** the worker SHALL consume it and retain bounded untrusted notifications or send the supported immutable-root/refusal reply

#### Scenario: Partial frame and command selection
- **WHEN** another worker event wins while a header or body is only partly read
- **THEN** the same framing reader SHALL finish that frame without losing bytes or permitting RPC replay
- **AND** selecting or cancelling an idle inbox wait SHALL NOT dispose framing ownership

#### Scenario: Reader cleanup
- **WHEN** explicit connection cleanup runs
- **THEN** overdue native processes SHALL be terminated after the existing three-second grace before the retained reader is aborted and joined
- **AND** Drop SHALL abort the reader without claiming joined or native completion

### Requirement: Owned Location language-server pool
(P1) A Location's language-server pool SHALL freeze resolved definitions for its lifetime and admit matching confined files lazily. Concurrent admissions for the same server and nearest-marker root SHALL share one owned worker. Failed roots SHALL remain broken until service recreation. Closing SHALL fence new starts and retain native settlement ownership across cancellation.

#### Scenario: Shared lazy worker
- **WHEN** concurrent matching-file admissions select the same server and root
- **THEN** exactly one launch SHALL occur and callers SHALL share bounded serialized protocol ownership
- **AND** disabled, unavailable, unmatched or external files SHALL NOT reach the launcher

#### Scenario: Broken generation
- **WHEN** startup fails or an owned process exits unexpectedly
- **THEN** the root SHALL expose broken status and SHALL NOT respawn in the same pool generation

#### Scenario: Interrupted close
- **WHEN** a caller cancels pool close while a process is settling
- **THEN** a later close SHALL join the same retained worker and report its native acknowledgement
- **AND** launch resource keepalives SHALL survive until owned connection settlement
- **AND** last-owner Drop SHALL request cleanup without claiming acknowledgement

### Requirement: Owned LSP connection lifecycle
(P1) LSP connections SHALL consume already-authorized processes, initialize within a default 45-second deadline and retain ownership until explicit native settlement. Closing SHALL attempt shutdown/exit for at most three seconds before forced tree termination and separate acknowledgement. Timeout or cancellation SHALL NOT permit request replay or imply termination.

#### Scenario: Initialized connection
- **WHEN** an authorized LSP process successfully responds to initialize
- **THEN** the client SHALL send initialized and retain its capabilities, immutable workspace root and bounded untrusted notifications
- **AND** unsupported server requests SHALL NOT authorize workspace mutations

#### Scenario: Failed or interrupted connection
- **WHEN** initialization fails or a request is interrupted
- **THEN** incomplete protocol ownership SHALL remain fenced and cleanup SHALL terminate the owned tree, retaining the distinction between forced termination and native acknowledgement
- **AND** server response and stderr content SHALL NOT appear in error display

#### Scenario: Owned shutdown
- **WHEN** shutdown succeeds or stalls
- **THEN** the connection SHALL settle the owned process tree, including descendants after leader exit, or report acknowledgement as unavailable
- **AND** dropping an owner SHALL terminate its tree without claiming settlement

### Requirement: Bounded LSP transport framing
(P1) LSP stdio SHALL use bounded ASCII Content-Length headers and UTF-8 JSON-RPC 2.0 object bodies. Interrupted or failed transport I/O SHALL fence both directions without claiming native termination or granting launch authority.

#### Scenario: Exact byte framing
- **WHEN** adjacent or fragmented LSP messages contain multibyte Unicode
- **THEN** the transport SHALL consume exactly each declared content byte length and preserve message boundaries
- **AND** headers above 8 KiB, content above 4 MiB, duplicate or invalid lengths, unsupported charsets and malformed messages SHALL be refused with diagnostics excluding server content

#### Scenario: Cancelled partial transport I/O
- **WHEN** a read or write future is cancelled after transport admission
- **THEN** both subsequent reads and writes SHALL be refused rather than resuming or replaying a partial frame
- **AND** the caller SHALL retain responsibility for native process termination and settlement

## MODIFIED Requirements

### Requirement: LSP enablement
(P1) When `lsp` is omitted, the system SHALL enable every built-in server whose executable is found on PATH or in `<cache>/bin`. `lsp: false` SHALL disable all servers. `lsp: { <id>: {...} }` SHALL apply per-server overrides on top of the auto-detected set. Downloading missing servers SHALL require `lsp.auto_install: true` (default `false`).

#### Scenario: Auto-detected server
- **WHEN** `rust-analyzer` is on PATH and `lsp` is omitted
- **THEN** Rust files activate the `rust-analyzer` server

#### Scenario: No downloads by default
- **WHEN** `gopls` is missing and `auto_install` is false
- **THEN** no download occurs and `cyber doctor` lists `gopls: not installed`

### Requirement: Built-in servers
(P1) The system SHALL ship definitions (id, extensions, root markers, launch command, install method) for at least: `rust-analyzer`, `typescript` (tsserver via typescript-language-server), `pyright`, `gopls`, `clangd`, `jdtls`, `lua-language-server`, `zls`, `bash-language-server`, `yaml-language-server`, `svelte`, `vue`, `solidity`, and `verible` (SystemVerilog).

#### Scenario: Server list
- **WHEN** the user runs `cyber lsp status`
- **THEN** each built-in server is listed with `enabled`, `installed` and `running` columns

### Requirement: Custom and overridden servers
(P1) An `lsp.<id>` entry SHALL accept `command` (string array), `extensions`, `root_markers`, `env`, `initialization_options` and `disabled`. Custom ids SHALL require `extensions`, or config validation fails.

#### Scenario: Custom server
- **WHEN** config defines `lsp.nimlsp` with `command: ["nimlangserver"]` and `extensions: [".nim"]`
- **THEN** `.nim` files activate it

### Requirement: Custom formatters
(P1) A `formatters.<id>` entry SHALL accept `command` (string array with `$FILE` replaced by the absolute path), `extensions`, `env` and `disabled`. An entry with `command` SHALL always be enabled for its extensions.

#### Scenario: Custom formatter command
- **WHEN** `formatters.taplo` has `command: ["taplo", "fmt", "$FILE"]` and `extensions: [".toml"]`
- **THEN** edited `.toml` files are formatted with taplo


#### Scenario: Typed integration configuration
- **WHEN** trusted resolved configuration is loaded
- **THEN** `lsp` and `formatters` SHALL accept omission, false or an object and SHALL reject other section types
- **AND** server and formatter commands SHALL be nonempty string arrays with a nonempty executable, extensions and root markers SHALL contain nonempty strings, and environments SHALL contain string values
- **AND** `lsp.auto_install` SHALL be boolean with default false and `lsp.diagnostics_wait_ms` SHALL be a nonnegative integer with default 5000
- **AND** parsing SHALL NOT start processes, download executables or establish diagnostics authority

#### Scenario: Untrusted integrations remain inactive
- **WHEN** a project defines LSP or formatter commands without approval for its current sensitive digest
- **THEN** those definitions SHALL remain absent from resolved executable configuration
- **AND** approving and subsequently changing the definitions SHALL require fresh trust review

#### Scenario: Local server catalogue and confined root discovery
- **WHEN** language-server definitions are resolved for a Location
- **THEN** built-ins SHALL retain their canonical IDs, extensions, root markers, launch arguments and install methods, with trusted user overrides taking precedence
- **AND** executable discovery SHALL search the supplied PATH followed by `<cache>/bin`, refuse non-executable files on Unix and never run candidate executables
- **AND** root discovery SHALL select the nearest existing root marker without ascending above the canonical Location or accepting files outside it
- **AND** missing executables SHALL remain uninstalled without downloading during discovery

### Requirement: Formatter enablement and detection
(P1) When `formatters` is omitted, the system SHALL enable each built-in formatter whose detection succeeds: its binary is on PATH and, where defined, a project marker exists. Built-ins SHALL include `rustfmt`, `prettier`, `biome`, `ruff`, `black`, `gofmt`, `clang-format`, `shfmt`, `stylua`, `zig fmt`, `forge fmt` and `verible-verilog-format`. `formatters: false` SHALL disable all of them.

#### Scenario: Prettier requires config
- **WHEN** `prettier` is on PATH but the project has no prettier config or dependency
- **THEN** prettier is not enabled


### Requirement: Formatter status
(P1) The server SHALL expose `GET /api/v1/formatters` returning `{ id, extensions, enabled, detected_by }`, and the CLI SHALL provide `cyber fmt status`.

#### Scenario: Formatter status listing
- **WHEN** a client requests formatter status
- **THEN** each formatter is returned with whether detection currently succeeds


#### Scenario: Local status without integration effects
- **WHEN** `cyber lsp status` or `cyber fmt status` inspects the current Location
- **THEN** it SHALL expose every built-in and configured integration from trusted resolved settings without starting a model, creating a database, downloading or running an integration
- **AND** formatter discovery SHALL require a local executable and any built-in project marker, while configured commands SHALL enable their extensions unless explicitly disabled
- **AND** Prettier SHALL detect its supported config filenames, package.json configuration or declared dependency using bounded regular no-follow reads without evaluating configuration code

#### Scenario: Authenticated Location formatter status
- **WHEN** a client requests `GET /api/v1/formatters`
- **THEN** the server SHALL authenticate the request and return Location-scoped formatter ids, extensions, enabled and detected_by fields from fresh trust-filtered configuration and shared discovery
- **AND** query Location SHALL take precedence over header Location, with invalid Locations refused before discovery
- **AND** status SHALL NOT execute candidates, evaluate project configuration code or publish formatter environment/command secrets
- **AND** unavailable service hosts SHALL return a typed service-unavailable error rather than an empty success list


### Requirement: Bounded validated diagnostic publications
(P1) Owned LSP workers SHALL interpret publishDiagnostics version support and retain a typed diagnostic cache for canonical regular UTF-8 files under their root, including unopened workspace documents. Supplied versions for opened documents SHALL match the current client version. Omitted versions SHALL remain omitted and SHALL NOT be represented as proof of edit completion. Valid publications SHALL replace prior sets, including empty clears; invalid publications SHALL NOT partially replace a valid set. The cache SHALL retain at most 128 files and 1 MiB of serialized diagnostic snapshots. Publication admission SHALL use the 1 MiB message budget rather than arbitrary diagnostic-count or message-character cutoffs.

Workers SHALL recheck current service authority before diagnostic scope admission. Diagnostic files SHALL retain independent claims for every enclosing managed checkout before caching, and content/creation observations SHALL match again after admission. Cache retrieval SHALL revalidate file contents, managed creation identities and observed client versions. Changed or missing files SHALL return no stale publication. In-flight observation and scope claims SHALL remain owned through cancellation; native settlement SHALL precede waiting for their completion.

Typed snapshots SHALL expose only path/version/observed-client-version/receipt-sequence and range/severity/message data. Error feedback formatting SHALL include only explicit error severity, at most 20 entries per file, one-based positions and an omitted-count suffix. Server text SHALL NOT introduce diagnostic block framing or terminal controls. Raw notification retrieval SHALL remain explicitly untrusted and bounded to 128 retained messages and 1 MiB independently of typed snapshots.

#### Scenario: Empty publication clears errors
- **WHEN** a valid publication replaces a file's prior errors with an empty diagnostics array
- **THEN** the cached typed result SHALL contain no former errors and SHALL have a new receipt sequence

#### Scenario: Stale or malformed publication
- **WHEN** an opened document receives a mismatched version or invalid diagnostic range/severity/message shape
- **THEN** the prior valid set SHALL remain unchanged

#### Scenario: External content change
- **WHEN** file contents or managed creation identities differ from the cached observation
- **THEN** retrieval SHALL return no stale diagnostic set

#### Scenario: Unopened nested managed file
- **WHEN** an unmanaged server root receives diagnostics for an unopened file inside nested managed checkouts
- **THEN** every enclosing checkout SHALL retain independent LSP ownership through native/resource settlement
- **AND** receipt SHALL NOT send didOpen or infer an acknowledged document version

### Requirement: Owned edit diagnostic feedback
(P1) Successful UTF-8 writes by edit, write, apply_patch and notebook_edit SHALL capture managed ancestry before mutation and enqueue owned save synchronization after releasing the file-write lock. Save delivery SHALL revalidate service authority, managed creation identities and current disk content before sending didOpen or didChange followed by didSave. Servers requesting save text SHALL receive the saved text. Diagnostic waiting SHALL use the configured deadline and SHALL NOT convert a completed file edit into a failure when services are unavailable or no fresh diagnostics arrive. Zero-wait callers SHALL retain queued synchronization ownership through notification or service settlement.

Feedback SHALL use publications received after save admission, revalidate current document version/content and preserve omitted server versions as omitted. It SHALL append escaped explicit error blocks through the existing output budget. Write SHALL include at most five other files with new errors, excluding unchanged previously cached errors. Multi-file patches SHALL finish their applied mutations before starting diagnostic waits, including feedback for completed mutations on a partial failure. The canonical Diagnostics after edits requirement remains authoritative for complete debounce and lifecycle acceptance.

#### Scenario: Four editing tools with a publishing server
- **WHEN** each editing tool saves a supported file and the server publishes matching-version errors and warnings
- **THEN** output SHALL include at most twenty errors per file with the omitted count and no warnings

#### Scenario: New errors in other files
- **WHEN** write receives fresh publications for seven other files and later receives the same unchanged errors
- **THEN** the first write SHALL report at most five other files and the later write SHALL omit those unchanged errors

#### Scenario: Save without diagnostic waiting
- **WHEN** an edit has a zero diagnostic deadline
- **THEN** its owned save notification SHALL still reach the server or settle with its service

#### Scenario: Concurrent edit during feedback
- **WHEN** one edit waits for diagnostics and a second edit changes the same file
- **THEN** the first diagnostic wait SHALL NOT retain the file-write lock or overwrite the second edit upon cancellation

### Requirement: Document-scoped feedback debounce and supersession
(P1) Diagnostic collection SHALL wait for a shared 150 ms quiet period following the most recent successful save notification of that document in its server generation, including saves with identical contents. New saves SHALL reset the quiet period without extending any caller's configured diagnostic deadline. Save-time clock tracking SHALL follow the bounded document cache and be removed on eviction. Save notifications SHALL NOT be coalesced away.

A pending feedback receipt SHALL be completed without any primary or other-file diagnostics if its observed client version, saved content or managed creation identity becomes obsolete. This SHALL apply to external edits even when no newer save notification is delivered. Matching observations and omitted server versions SHALL NOT be represented as stronger proof of server computation than the protocol supplies.

#### Scenario: Rapid saves with unchanged content
- **WHEN** a second save occurs during an earlier save's quiet period with the same document version and contents
- **THEN** both waiting collections SHALL defer until the most recent save's quiet period ends or their individual deadlines expire

#### Scenario: Superseded save with other-file diagnostics
- **WHEN** a newer save changes the document before an older receipt collects fresh errors from other files
- **THEN** the older receipt SHALL report no diagnostic blocks

#### Scenario: External modification while waiting
- **WHEN** a user changes saved contents without sending another save notification
- **THEN** pending feedback SHALL discard all diagnostics for the obsolete save without changing the user's contents

### Requirement: Owned patch deletion and rename synchronization
(P1) Applied file deletion SHALL enqueue owned synchronization to matching already-admitted language-server roots without starting a server solely for a missing file. Deleted paths SHALL use a canonical parent and remain inside the Location. Delivery SHALL freshly validate service authority, path absence and the managed creation ancestry captured before mutation. Dangling symlinks, recreated files, noncanonical parents and changed checkout identities SHALL refuse delivery. Independent claims for every enclosing managed checkout SHALL be retained through native/resource settlement, including deletion of previously unopened documents.

For an opened document the worker SHALL send didClose and clear its document, quiet-clock and typed diagnostic cache state without resetting the server-generation version counter. It SHALL send a deleted-file watched notification, and successful saves SHALL send created or changed watched notifications according to pre-mutation existence. Rename through apply_patch SHALL synchronize the old deletion before opening/saving the destination when the same server/root serves both paths. No didClose SHALL be sent for an unopened document. This mutation synchronization SHALL NOT claim complete general filesystem watching or server-side dynamic watcher registration.

Queued deletion SHALL retain service activity and notification ownership after a zero-wait caller returns. In-flight deletion scope admission SHALL remain owned through cancellation; native descendants SHALL stop before waiting for blocked claims, and final/repeated close SHALL require resource acknowledgement.

#### Scenario: Rename then delete an opened file
- **WHEN** apply_patch moves an opened file and later deletes its destination
- **THEN** the old and new opened documents SHALL each close once and the server SHALL receive deletion/creation/deletion events in order

#### Scenario: Delete an unopened nested managed file
- **WHEN** a running unmanaged root receives a patch deletion inside two nested managed checkouts
- **THEN** both checkouts SHALL retain independent LSP claims through cancelled shutdown and no didOpen or didClose SHALL be inferred

#### Scenario: Zero-wait deletion
- **WHEN** apply_patch deletes a supported file with a zero diagnostic deadline
- **THEN** queued deletion synchronization SHALL still reach its running server or settle with its service

#### Scenario: Cold or unmatched deletion
- **WHEN** no matching server root is running or the deleted file has an unsupported extension
- **THEN** no new language-server process SHALL start solely for that deletion and unrelated roots SHALL receive no document mutation

### Requirement: Owned read-only navigation tool
(P1) The built-in lsp tool SHALL expose definition, references, hover, document_symbols, workspace_symbols, implementation, rename_preview and diagnostics through the lsp permission action. It SHALL be offered to ordinary and patch-preferring models and in plan mode as read-only. Input positions SHALL be positive 1-based values within the protocol integer range, converted to zero-based positions only for RPC. Position-dependent operations SHALL require a file path; rename_preview SHALL require a new name. The existing default ask policy SHALL remain in force unless lsp is explicitly permitted or approved. Denied/disabled/invalid admissions SHALL NOT start language-server processes.

File operations SHALL observe canonical regular UTF-8 documents within the Location and the 1 MiB document budget, capture immutable managed ancestry before delivery, and open through the retained document-admission boundary. Document open and navigation RPC SHALL execute as one serialized worker operation after revalidating queued contents, without an intervening document change from another caller. Caller source changes during a request SHALL discard the response without modifying user content. Navigation SHALL retain Location activity through discovery, requests and output collection. Workspace operations without a file SHALL discover the owned Location pool without inventing/opening a document, retaining all existing nested server roots. Root startup, native resources, trust checks, sticky failures and explicit lifecycle fences SHALL remain shared with warming/edit feedback.

Navigation SHALL normalize valid locations and links, hierarchical/flat symbols and hover content into bounded JSON with Location-relative paths, 1-based positions and bounded source-line previews where positions are supplied. It SHALL retain at most fifty result rows across servers and report omitted or failed results. External/remote URIs and invalid ranges SHALL NOT cause external reads. Source previews SHALL use bounded regular-file observations refusing symlinks/reparse points. Literal path characters SHALL be preserved, with platform path components separated by forward slashes.

Rename preview SHALL project changes or preferred ordered documentChanges, including text edits and proposed create/rename/delete operations, without writing files or sending didSave. Versioned proposals SHALL preserve their supplied document version and server/root provenance. This projection SHALL NOT claim that previewed edits were applied. Diagnostics SHALL use the existing revalidated typed cache, retaining the distinction between optional server versions and observed client versions.

#### Scenario: Read-only navigation and bounded references
- **WHEN** a permitted tool call queries references and a server supplies sixty valid locations plus external/remote entries
- **THEN** output SHALL contain at most fifty Location-relative rows with code previews and the omitted count

#### Scenario: Preview rename in plan mode
- **WHEN** a server returns versioned text edits and proposed file operations
- **THEN** plan-mode output SHALL describe the proposals without changing source files or creating/moving destinations

#### Scenario: Workspace query without a file
- **WHEN** workspace_symbols is called before any document is opened
- **THEN** a permitted owned pool SHALL discover/start applicable configured servers without sending a synthetic didOpen

#### Scenario: Changed source during RPC
- **WHEN** a user edits the requested source before the response is collected
- **THEN** navigation SHALL discard the response and preserve the user's content

### Requirement: Rejected synchronization activity disposal
(P1) Rejecting a queued save/deletion job SHALL dispose its owned activity only after releasing queue/table/activity locks. Closed or full queues SHALL return an error without deadlocking activity disposal or preventing later lifecycle settlement.

#### Scenario: Closed warming receiver
- **WHEN** an owned save job is submitted to a service entry whose warming receiver has closed
- **THEN** rejection SHALL release both the submitted job's activity and any temporary admission guard without waiting on a mutex held by the same call

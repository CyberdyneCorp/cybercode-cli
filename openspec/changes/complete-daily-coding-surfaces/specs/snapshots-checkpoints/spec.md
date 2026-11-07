## ADDED Requirements

### Requirement: Branch-preserving rewind completion
(P1) CLI/TUI code, conversation and combined rewind SHALL use the server's owned staged-revert engine with preview/revalidation and an idle guard. Conversation commit SHALL retain later messages as an inactive branch, not delete transcript or independent recovery evidence. Undo SHALL restore the prior user prompt text; redo SHALL advance or clear the same staged boundary. Code restoration SHALL preserve unrelated edits and report excluded files, conflicts and external effects. Stale Session/Location previews SHALL NOT authorize restoration.

#### Scenario: Rewind and continue
- **WHEN** a user rewinds conversation and then sends a new prompt
- **THEN** the active path advances from the selected boundary while the old continuation remains available on an inactive branch

#### Scenario: User edit after preview
- **WHEN** a file changes after the owner reviews a code-rewind preview
- **THEN** applying the old preview refuses or requires a fresh review rather than overwriting that edit

#### Scenario: Explicit CLI confirmation
- **WHEN** an owner runs `cyber sessions rewind <session> --to <message> --target code|conversation|both`
- **THEN** the command returns a preview and fingerprint without restoring files or changing the active path
- **AND** an explicit `--confirm <fingerprint>` revalidates Session sequence, directory, boundary, current snapshot and target before owned restoration, refusing changed context and preserving independent recovery evidence

#### Scenario: Snapshot host cannot revalidate
- **WHEN** a custom snapshot host cannot check the reviewed current tree under its workspace mutation lock
- **THEN** confirmed code restoration is refused rather than calling an unchecked legacy restore, while conversation-only rewind remains available when otherwise idle


#### Scenario: Reviewed stage ownership survives replay
- **WHEN** a reviewed file undo is restaged or replayed before clear
- **THEN** it retains its original baseline and the union of paths actually changed, and clear restores only those paths through a fresh reviewed three-way plan
- **AND** legacy stage/clear and branch checkout cannot discard that path ownership

#### Scenario: Recover a staged undo after service loss
- **WHEN** the service is killed after acknowledging a reviewed stage and the owner reconnects
- **THEN** the retained stage, original baseline and changed-path ownership replay unchanged
- **AND** restaging and clearing require fresh reviews, preserve unrelated owner edits and do not start inference

#### Scenario: Interrupt invalidates an earlier review
- **WHEN** new input makes the Session busy after a rewind preview and the owner explicitly interrupts it
- **THEN** confirmation while busy is refused, and the old preview cannot apply after idle acknowledgement without a new review

#### Scenario: Unknown native outcome survives runtime reload
- **WHEN** a native restore loses acknowledgement and a new runtime loads its durable evidence
- **THEN** stopping the subtree reports an unknown outcome instead of acknowledgement, keeps admission closed, and does not replay the native restore

### Requirement: Bounded non-git snapshot storage
(P1) Non-git snapshots SHALL use private atomic BLAKE3-addressed file objects and Location-bound manifests, respecting ignores and file-size limits without following out-of-scope symlinks. Matching size/mtime SHALL NOT substitute for verified content identity. The default 200 MiB Location cap SHALL disable unavailable code rewind visibly while retaining conversation rewind. Restoration and retention SHALL preserve staged/branch/unknown-effect evidence and use the same conflict and recovery guarantees as Git snapshots.

#### Scenario: Same-size edit with restored timestamp
- **WHEN** a non-git file's bytes change while its size and timestamp remain the same
- **THEN** a later snapshot or restore verifies its hash and does not reuse the old bytes as current evidence

#### Scenario: Interrupted restore
- **WHEN** a file-copy restore loses its owner after a possible partial replacement
- **THEN** recovery retains the journal and backup evidence and does not report success or blindly repeat the mutation


#### Scenario: Inspect retained restore evidence
- **WHEN** an authenticated owner inspects recovery journals for a Session's Location
- **THEN** the read-only bounded response reports filesystem completion and durable progress without restoring, acknowledging native receipts, creating an absent namespace or starting inference
- **AND** inspection remains available while a native writer is paused

#### Scenario: Excluded file is not a deletion
- **WHEN** an otherwise recorded non-Git path is excluded by a later capture
- **THEN** its absence does not authorize deletion or synthesize a deletion diff, and retained target/recorded exclusions appear in a rewind review

#### Scenario: Snapshot cap does not fail conversation
- **WHEN** the fallback cap makes file-copy capture unavailable
- **THEN** a one-time diagnostic leaves conversation execution and conversation-only rewind available, and successful exec exits remain successful

#### Scenario: Recreate an admitted deleted file
- **WHEN** a file-copy restore recreates a deleted file and its missing parents
- **THEN** it prepares the recorded ordinary ownership/mode before publishing the file, creates missing parents privately, and refuses unsupported metadata rather than widening access
- **AND** captures disclose exclusions for ACLs, extended attributes, platform flags or other metadata that cannot be recreated safely

### Requirement: Immutable Git exclusion evidence
(P1) New Git snapshots SHALL bind their exact Location and bounded excluded-path inventory into immutable snapshot identity. Equal file trees with different exclusions SHALL remain distinguishable. Recorded or currently excluded paths SHALL NOT become deletion or restore authority. Legacy bare trees SHALL remain readable with explicit unavailable historical evidence rather than invented exclusions. New snapshot trees and metadata SHALL remain retained together until safe reference-aware reclamation exists.

#### Scenario: Same Git tree with changed omissions
- **WHEN** an ignored file appears without changing the captured Git tree
- **THEN** the new snapshot has a different identity and retains its own exclusions after config changes and runtime restart

#### Scenario: Newly ignored owner file
- **WHEN** a previously captured Git path becomes ignored before rewind
- **THEN** review discloses its current omission, no deletion diff is synthesized and restore leaves the live file unchanged

#### Scenario: Legacy and foreign Location snapshots
- **WHEN** a snapshot predates exclusion records
- **THEN** its file tree remains readable and inspection reports unavailable exclusion history
- **AND** a Location-bound record from a different Location is refused even when its objects are available in the same shadow repository

### Requirement: Runtime-owned snapshot workers
(P1) Managed snapshot restoration SHALL use a runtime-issued synchronous worker owner binding Session creation, native activity, checkout, Location and private journal identities. The actual worker SHALL retain lifecycle, checkout, workspace and private-store ownership through its last filesystem operation and consuming commit even if the requesting task is disposed. Session changes SHALL be excluded from each source mutation and revalidated before the final commit. Ordinary prompt admission SHALL refuse active or unresolved managed snapshot work while exact retries remain inspectable. A successful restore SHALL atomically commit its Session events and exact worker acknowledgement, then acknowledge its journal under retained native proof. A journal alone SHALL NOT settle a generic process, binary document, foreign worker or legacy unknown activity.

#### Scenario: Request disappears during a live worker
- **WHEN** a requester is disposed after a managed worker records its immutable origin
- **THEN** its actual native owners remain held until the synchronous worker exits, Stop and read-only Session inspection remain available between source effects, and shutdown waits for actual ownership settlement
- **AND** uncertain effects retain their origin and admission fence across runtime reload without replay

#### Scenario: Snapshot checkout claim is independent of the Session
- **WHEN** a snapshot worker acquires a managed checkout while its Session or another worker owns a separate claim
- **THEN** checkout admission SHALL use the exact reserved native activity ID, retain its own lock through consuming settlement, and refuse reuse of a live or unknown claim
- **AND** settling that snapshot claim SHALL NOT acknowledge unrelated Session activity; an interrupted acquisition SHALL retain unknown evidence without a generic clearing path
- **AND** a managed host without explicit snapshot ownership support SHALL refuse instead of reusing a Session claim

#### Scenario: Checkout claim publication is fully initialized
- **WHEN** a lifetime-bound snapshot worker acquires a checkout claim
- **THEN** the stable native claim SHALL be published without replacement only after its complete record is durable and binds the exact activity, Session creation, directory and immutable lifetime digest
- **AND** runtime admission SHALL refuse returned resources with foreign or missing lifetime context
- **AND** a construction crash SHALL preserve private evidence without granting an incomplete stable record or legacy claim new acknowledgement authority

#### Scenario: Exact exited claim reacquisition is read-only
- **WHEN** recovery reacquires an exited checkout claim under the exact stored version-2 binding and lifetime context
- **THEN** it SHALL retain the existing native lock and metadata pins without creating or resetting any claim or acknowledgement
- **AND** live, aliased, replaced, missing, legacy or foreign evidence SHALL refuse

#### Scenario: Acquired resource set precedes source admission
- **WHEN** a lifetime-owned worker completes native checkout acquisition
- **THEN** its complete acquired binding set SHALL be durably published before an origin or source callback, with empty and missing sets distinguished
- **AND** a changed set or post-admission addition SHALL refuse, source admission SHALL retain the published set, and incomplete evidence SHALL NOT gain reference-completeness authority
- **AND** disposal before origin publication SHALL retain a lifetime-bound reservation as unknown until explicit exact-owner reconciliation

#### Scenario: Preparation receipt consumes exact acquired resources
- **WHEN** recovery cancels an exited preparation with a complete published checkout set
- **THEN** review and its durable receipt SHALL bind the exact lifetime and resource-set digest
- **AND** original claim locks and pins SHALL remain held through consuming commit and observed reply, with changed or live evidence refusing
- **AND** historical receipts without this binding SHALL gain no new checkout acknowledgement authority, while request lookup SHALL remain database-only

#### Scenario: Durable preparation receipt acknowledges only its native checkout claims
- **WHEN** the consuming preparation commit is observed durably acknowledged under retained original locks
- **THEN** native checkout acknowledgement SHALL bind the exact version-2 claim, complete-resource digest and committed receipt digest without rewriting original claim bytes
- **AND** no-replace durable publication and exact native marker identity SHALL reject changed, aliased, foreign or different-receipt evidence
- **AND** removal SHALL continue to refuse independent unknown claims and incomplete or foreign construction evidence
- **AND** a retained acknowledgement stage SHALL grant no authority by itself and SHALL be preserved until separate acknowledged reference-aware reclamation
- **AND** only explicit metadata reconciliation after committed receipt attestation SHALL resume interrupted acknowledgements without source replay

#### Scenario: Git journal images resolve exact immutable objects
- **WHEN** a Git checkpoint image is materialized for planning or recovery
- **THEN** the implementation SHALL resolve one exact literal path with a validated mode/object ID and historical exclusions
- **AND** malformed or ambiguous listings, unsupported entry kinds and blob contents that do not match the Git object ID SHALL refuse before source replacement
- **AND** exclusion tags and every scoped directory tree SHALL match their object IDs before changed paths are derived from a verified literal-path catalog
- **AND** metadata traversal and subprocess output SHALL be bounded, with over-limit children killed and awaited

#### Scenario: Git images enter a native recovery journal
- **WHEN** verified Git images are imported by the retained native snapshot worker
- **THEN** the journal SHALL retain original target/recorded/expected Git identities and exact worktree/Location context in a distinct versioned context
- **AND** the bounded handoff SHALL retain and revalidate each leaf object identity before native source planning, without Git subprocesses inside the synchronous source callback
- **AND** native source changes SHALL retain displaced preimages and use the same ownership, recovery review and retry-lookup rules as file-copy journals
- **AND** legacy copy contexts SHALL preserve their encoded identity and SHALL NOT be reinterpreted as Git contexts

#### Scenario: Native symbolic-link recovery images
- **WHEN** a Git symbolic-link image enters a native restore or owner recovery plan
- **THEN** the implementation SHALL pin the link inode without following its target and bind typed target bytes, native identity/generation and supported metadata
- **AND** native staging, replacement and retained-preimage reads SHALL operate on the link namespace entry without reading or writing target file data
- **AND** new desired links SHALL refuse escaping or protected targets, including escapes through an existing alias followed by `..`
- **AND** review SHALL identify file versus symbolic-link images, while legacy images default to files
- **AND** link-capable journals SHALL use a distinct version/backend binding without promoting older file-only ownership or changing historical file receipt digests

#### Scenario: Git namespace changes before native planning completes
- **WHEN** a native Git restore validates its prepared expected state
- **THEN** it SHALL compare the eligible Location leaf inventory, including newly appearing paths, without following directory links or opening unknown leaf kinds
- **AND** namespace generations and read-only Git rule inputs SHALL be revalidated before intent publication
- **AND** historical exclusions SHALL constrain mutation independently of the current expected-state check
- **AND** validation marks SHALL NOT be used as source mutation, acknowledgement or reclamation ownership

#### Scenario: Retained native checkout routes change
- **WHEN** a snapshot worker's retained checkout claim, activity namespace or worktree creation record changes after acquisition
- **THEN** the worker SHALL refuse source callbacks and consuming acknowledgement after verifying exact native file/directory identities and creation contents
- **AND** aliases, replaced native objects and contradictory records SHALL refuse without acknowledging foreign resources
- **AND** the consuming Store writer SHALL verify and retain the exact checkout proof through its actual commit and observed reply
- **AND** previous writer catalogs lacking this proof contract SHALL refuse Store mutation while preserving native evidence

#### Scenario: Lifetime proof precedes reservation
- **WHEN** a new file-copy worker reserves runtime ownership before publishing its restore origin
- **THEN** an immutable worker-specific native lifetime lock already binds its activity, Session creation, checkout, Location and private namespace, and remains retained through actual resource teardown
- **AND** replacement, missing proof or a live lock refuses reacquisition; a privately staged incomplete publication remains retained and cannot grant acknowledgement authority
- **AND** this evidence counts toward the Location cap, protects pending references from GC, and cannot settle a generic process, Session-owned checkout lease or historical reservation without that binding

#### Scenario: Owner cancels an exited preparation with no published origin
- **WHEN** the owner confirms a fingerprint review of a positive unstarted reservation under its exact reacquired native lifetime lock
- **THEN** the writer atomically acknowledges only that activity and retains its cancellation request receipt, rechecking current Session/recovery authority without moving source files or transcript state
- **AND** retry looks up the original durable pair without native access, changed or cross-owner request reuse refuses, and a lost commit reply does not dispatch another operation
- **AND** live, legacy, generic, binary or source-admitted owners refuse; independent checkout/process uncertainty remains fenced, and cancelling an unstarted resolution leaves its failed parent restore unresolved

#### Scenario: Published origin has no admitted source effects
- **WHEN** an exited worker has a published origin but both its exact database history and version-2 native lifetime phase positively prove no admitted source effects
- **THEN** fingerprint-confirmed cancellation SHALL preserve source files and transcript state, commit the exact paired cancellation receipt, and only then publish bounded native cancellation metadata
- **AND** missing intent alone SHALL NOT grant authority, the original intent SHALL remain unchanged, and explicit acknowledgement SHALL reconcile metadata without replay

#### Scenario: Database backup cannot erase native source admission
- **WHEN** a database backup shows an unadmitted preparation but its exact native phase is admitted, missing or legacy
- **THEN** preparation cancellation SHALL refuse without changing source files or deleting evidence
- **AND** a lost reply after database source admission SHALL prevent source callback dispatch and SHALL NOT downgrade the durable admitted record

#### Scenario: Session changes before a source effect
- **WHEN** the Session revision changes after preparation but before the next source mutation
- **THEN** that mutation is refused and existing owner bytes remain unchanged

#### Scenario: Receipt cannot acknowledge another process
- **WHEN** a managed snapshot completes while an unrelated native process has unknown outcome
- **THEN** only the exact snapshot activity can be acknowledged and the unrelated process retains its own unknown evidence and fence

#### Scenario: Managed journal completion needs writer acknowledgement
- **WHEN** a managed journal records filesystem completion but its runtime commit or journal acknowledgement is unobserved
- **THEN** the retained evidence blocks another restore and is not treated as successful recovery or eligible for deletion

#### Scenario: Ordinary restore loses its acknowledgement reply
- **WHEN** an ordinary file-copy restore's paired Session/native commit is durable but its reply or journal acknowledgement is lost
- **THEN** lookup proves its original immutable intent and exact Session event batch without redispatching restore
- **AND** explicit acknowledgement rechecks current runtime authority and original native namespace/lock under retained worker owners, changes only journal acknowledgement metadata, and preserves later owner files, symlinks and transcript state
- **AND** stale in-memory Session state requires reload, historical settlements without the persisted immutable receipt cannot authorize new writes, and unrelated unknown activities retain their own fences

#### Scenario: Recovery request reconciles journal acknowledgement
- **WHEN** both exact recovery owner records contain the durable consuming receipt but a journal acknowledgement is missing
- **THEN** explicit reconciliation verifies the persisted immutable resolution digest and original intent, publishes child acknowledgement before the parent's settlement link, and permits idempotent retry after an interrupted metadata write without inspecting or replaying later owner edits
- **AND** an uncommitted filesystem outcome or an older receipt without the digest cannot authorize new acknowledgement writes

#### Scenario: Legacy journal compatibility
- **WHEN** inspection reads an origin-less version-1 file-copy intent
- **THEN** it remains readable with its original completion semantics and is never upgraded into authority to acknowledge an unknown native activity

#### Scenario: Journal quota includes its complete durable record
- **WHEN** restore reserves storage before publishing an intent
- **THEN** the bound includes worker metadata, escaped paths, complete progress and acknowledgement fields, atomic record replacement and retained stage/preimage payloads
- **AND** exceeding the bound refuses before creating the intent or mutating source files

#### Scenario: Native ownership survives missing progress
- **WHEN** a file-copy restore stops after a namespace operation but before progress is recorded
- **THEN** its durable prepared section identifies original/staged files and all parents, including privately staged missing parents, so inspection can distinguish an applied effect without replay
- **AND** an equal-byte replacement with a foreign native identity, a changed parent or alias refuses ownership proof; older journals without prepared evidence remain readable without gaining rollback authority
- **AND** original preimages and private preparations are flushed before prepared evidence is published, created-parent metadata changes refuse, and authority is rechecked before each parent publication and file operation

#### Scenario: Owner reviews keep and rollback
- **WHEN** the authenticated owner reviews a retained restore
- **THEN** review reports exact native origin, current/proposed images, conflicts and retained exclusions or explicit unavailable exclusion history, without restoring, acknowledging activity or starting inference
- **AND** keep preserves a reviewed later ordinary owner replacement without claiming it belongs to the restore, while rollback refuses a foreign replacement or missing native proof

#### Scenario: Replacing the private lock cannot manufacture worker exit
- **WHEN** the private lock file is replaced while an old worker retains its original descriptor
- **THEN** recovery refuses the changed lock identity even if a different lock inode can be acquired
- **AND** origins without an exact native lock binding remain inspectable and fenced rather than receiving retrospective acknowledgement authority

#### Scenario: Recovery request never replays filesystem resolution
- **WHEN** a fingerprint-confirmed resolution has published a distinct durable request intent
- **THEN** later requests with its ID look up that original result or incomplete outcome and never dispatch source changes again
- **AND** changed request content refuses, preimages and directory ownership evidence remain retained, and filesystem completion alone does not settle original/native fences

#### Scenario: Native Git policy is bound to the expected capture
- **WHEN** native Git restore validates an expected checkpoint
- **THEN** its immutable capture record SHALL bind effective ignore settings, bounded read-only rule input hashes and canonical ignored namespace names, and current inputs SHALL match before journal publication
- **AND** successful policy collections before and after capture SHALL agree; unsupported capture SHALL report unavailable proof without inventing it, and legacy tags/bare trees SHALL remain readable without acquiring native admission authority
- **AND** this policy fingerprint SHALL NOT substitute for raw working-tree state, exact worker ownership, native acknowledgement or reference-aware reclamation

#### Scenario: Canonical Git blobs differ from captured owner bytes
- **WHEN** Git normalizes a captured file's line endings or working-tree encoding
- **THEN** supported native checkpoints SHALL retain exact bounded raw file/link images alongside the canonical tree in a verified envelope graph reachable from the immutable checkpoint ref
- **AND** native expected-state validation and restoration SHALL use verified raw images, retaining original snapshot IDs and historical exclusions without invoking filters inside the filesystem worker
- **AND** legacy public Git diff/blob behavior SHALL resolve the canonical tree, older records SHALL remain readable without acquiring raw proof, and changed successful captures SHALL refuse publication

#### Scenario: Managed Git preview and selected paths use the native plan
- **WHEN** an admitted managed Git restore previews a checkpoint whose raw bytes differ only by normalization
- **THEN** changed-path selection and preview SHALL preserve that difference and preview SHALL use the same raw-image planner, expected-state/policy checks, allowed paths and whole-plan conflicts as native restore
- **AND** Git paths and patch headers SHALL remain worktree-relative, including scoped Locations, and preview SHALL publish no journal or source mutation
- **AND** connecting this path SHALL NOT enable ordinary owned Git admission before its remaining gates are satisfied

#### Scenario: Git lifetime reservation precedes source admission
- **WHEN** a trusted runtime reserves a native snapshot worker lifetime in a Git Location
- **THEN** the metadata port SHALL bind the exact canonical Location, Session creation and activity to a retained version-2 native inode lock and SHALL refuse live-lock or foreign-owner reacquisition
- **AND** a managed worktree SHALL require its complete checkout set, while reservation alone SHALL NOT admit source effects, acknowledge a journal or settle a checkout
- **AND** ordinary owned Git availability SHALL remain separately gated until full recovery acceptance

#### Scenario: Ordinary restore receipt retains complete checkout evidence
- **WHEN** an ordinary snapshot restore with a native version-2 lifetime commits its consuming Session/native owner pair
- **THEN** its immutable receipt SHALL bind the exact lifetime and complete retained checkout-set digest, rechecking the set at the paired writer before commit
- **AND** lost or changed resource proof SHALL refuse the complete consuming pair and native acknowledgement, while lookup SHALL verify the bound lifetime against original native history
- **AND** historical receipts without checkout evidence SHALL remain readable without acquiring checkout acknowledgement or reclamation authority

#### Scenario: Owned checkout evidence survives source dispatch and consuming commit
- **WHEN** a version-2 snapshot worker retains an acquired managed checkout
- **THEN** acquisition SHALL preserve its unsettled original claim bytes and native lock, so an exited worker without acknowledgement continues to fence removal
- **AND** the consuming writer SHALL read-only verify the exact proposed restore receipt against retained claims before committing, then publish only matching idempotent markers after observing the durable pair
- **AND** failed publication SHALL preserve committed Session state and original claims for explicit reconciliation, with no source replay or unrelated-claim acknowledgement

#### Scenario: Ordinary checkout acknowledgement reconciles original resources
- **WHEN** an owner explicitly reconciles a committed ordinary restore with bound checkout evidence
- **THEN** reconciliation SHALL reacquire the exact original lifetime and complete checkout set, refuse missing/changed/live proof, and perform only bounded journal and checkout acknowledgement metadata writes
- **AND** legacy receipts SHALL gain no claim adoption authority and a managed completion without a journal receipt SHALL NOT manufacture one or discard its recovery fence

#### Scenario: Owned restore has no source changes after revalidation
- **WHEN** an owned restore validates the complete expected state and its plan has no source changes
- **THEN** it SHALL publish a real bounded empty intent with native origin, empty ownership evidence and durable completion, consuming its normal receipt and checkout acknowledgement without source admission or installation
- **AND** failed or lost consuming replies SHALL retain the same journal evidence for lookup/reconciliation, while legacy unowned no-op calls SHALL preserve their former no-journal behavior

#### Scenario: Receipt binds distinct lifetime and journal namespaces
- **WHEN** a native restore's lifetime and journal guards verify their respective namespace routes
- **THEN** consuming receipt validation SHALL bind both exact identities and the common owner/Location/root context without requiring the distinct namespace directories to have equal identities
- **AND** lookup and acknowledgement SHALL retain the original lifetime and journal verification and reject foreign owner or changed resource proof

#### Scenario: Resolution retains the interrupted owner's native resources
- **WHEN** recovery prepares a resolution for an interrupted owner with version-2 lifetime proof
- **THEN** it SHALL reacquire and retain that original lifetime and complete published checkout set alongside the distinct resolution owner, refusing live, missing, foreign or changed proof before source dispatch
- **AND** source authorization and consuming completion SHALL revalidate original resources; legacy owners SHALL gain no retrospective proof and unknown lifetime versions SHALL refuse
- **AND** retaining owners alone SHALL NOT acknowledge either native activity, and native result guard disposal SHALL precede worker lifetime release

#### Scenario: Recovery consumes both checkout owners before native disposition
- **WHEN** recovery finishes with an exact version-2 resolution lifetime and an attested version-2 original lifetime
- **THEN** its consuming receipt SHALL bind the resolution origin and both complete checkout-set digests, with both retained claim sets checked read-only and both lifetime histories checked at the paired writer
- **AND** observing the durable pair SHALL precede resolution checkout acknowledgement followed by original checkout acknowledgement, preserving original claim bytes and binding markers to the complete recovery receipt
- **AND** an original without version-2 lifetime proof SHALL remain explicitly absent rather than being adopted or inferred as an empty checkout set

#### Scenario: Recovery checkout acknowledgement resumes after an uncertain reply
- **WHEN** a consuming recovery pair commits but its reply or subsequent native marker publication fails
- **THEN** lookup SHALL verify the exact durable pair without replaying source work, and explicit reconciliation SHALL reacquire the original and resolution lifetimes and complete published checkout sets before bounded metadata acknowledgement
- **AND** changed, missing or live resource proof SHALL refuse reconciliation, matching markers SHALL be idempotent, and receipts without checkout evidence SHALL gain no new native claim acknowledgement authority

## MODIFIED Requirements

### Requirement: Non-git fallback
(P1) In non-git Locations the system SHALL snapshot by copying changed files with verified BLAKE3 identity into private `<data>/snapshot/<project_id>/<Location identity>/files/<hash>` storage, up to `snapshots.fallback_max_bytes` (default 200 MiB per Location). Non-git projects SHALL use `global` with an isolated Location namespace. When the limit is exceeded it SHALL disable code rewind for that Location with a one-time warning while conversation work continues. Unsupported native storage SHALL be unavailable explicitly rather than weakening restore guarantees.

#### Scenario: Non-git project
- **WHEN** a Session runs in an admitted folder without git
- **THEN** edits remain rewindable through private Location-bound file-copy snapshots


#### Scenario: Git repository object format does not select a non-Git backend
- **WHEN** a Location has an ancestor Git marker but the library cannot discover its repository format
- **THEN** the snapshot adapter SHALL NOT classify it as an ordinary non-Git Location or enable non-Git owned restore
- **AND** Git capture SHALL initialize a private shadow with the exact user's supported SHA-1 or SHA-256 object format and refuse a mismatched existing shadow before capture/index mutation
- **AND** read-only backend classification SHALL create no identity cache or snapshot namespace, and malformed/unreadable repository metadata SHALL NOT grant fallback mutation authority


#### Scenario: Historical copy checkpoints remain inspectable after Git routing changes
- **GIVEN** a Git Location retains copy checkpoints produced by earlier fallback routing
- **WHEN** the caller inspects changed paths, diffs or historical exclusions
- **THEN** typed copy IDs SHALL select the existing Location-bound copy store without creating a missing namespace, and exclusions from copy and Git history SHALL remain reportable
- **AND** comparisons between copy and Git IDs SHALL refuse explicitly without source effects or fabricated empty differences; read compatibility SHALL NOT authorize cross-backend restore or migration


#### Scenario: Git configuration locator changes after preparation
- **GIVEN** prepared native Git images bind the capture policy and resolved ignore inputs
- **WHEN** a selecting config root, include target, conditional-include HEAD or relevant environment changes after resolution
- **THEN** native inventory SHALL refuse before journal/source effects by revalidating bounded capture-bound configuration evidence without spawning Git in the filesystem worker
- **AND** absent root/include candidates SHALL be bound against later creation, unsupported origin/capability proof SHALL remain unavailable, and old policy fingerprints SHALL gain no retrospective proof


#### Scenario: Native Git planning retains historical copy image provenance
- **GIVEN** a Location retains copy target/recorded checkpoints and a fresh raw Git expected checkpoint
- **WHEN** the native planner prepares restoration after backend routing changes
- **THEN** it SHALL verify each backend's original images and Location binding, retain namespaced IDs and historical exclusions, and revalidate complete current Git state before source effects
- **AND** a version-6 journal SHALL bind explicit per-image source kinds and distinct digest framing; old journal versions SHALL refuse those annotations rather than acquire new mixed-image authority

#### Scenario: Generated Git rules retain their original source
- **GIVEN** native Git preparation uses shadow rules generated from the user common repository's `info/exclude`, application settings and skipped paths
- **WHEN** the source file is changed or created after preparation, its common directory is replaced, or the generated recipe/output no longer matches fresh source observations
- **THEN** native planning SHALL refuse before new journal/source effects, including for linked worktrees and scoped Locations
- **AND** bounded recipe validation SHALL derive source locators from trusted context, preserve unchanged generation observations, and publish output without following existing file or parent links
- **AND** this proof SHALL NOT imply user-index/large-untracked eligibility or activate managed Git admission

#### Scenario: Typed recovery commands preserve reviewed ownership and retry identity
- **WHEN** the owner lists journals, reviews a selected keep/rollback action and confirms through `sessions recovery`
- **THEN** confirmation SHALL require the exact fingerprint and an explicit stable request ID, and incomplete or malformed combinations SHALL fail before service startup
- **AND** `sessions recovery-request` SHALL look up the exact retained request without replay; only explicit `--acknowledge` SHALL invoke metadata reconciliation
- **AND** text/JSON SHALL distinguish current images, historical evidence availability, conflicts, durable state and observed native acknowledgement; stale/foreign responses or requests SHALL NOT authorize another Session/journal operation

#### Scenario: A fresh terminal discovers the existing recovery request
- **GIVEN** a recovery confirmation reserved an exact durable request and its reply was lost
- **WHEN** the owner reopens rewind recovery in a fresh TUI
- **THEN** bounded read-only discovery SHALL identify requests for that exact Session creation and journal without native access, acknowledgement or replay
- **AND** the panel SHALL offer lookup of retained identities, keep native reconciliation explicit, preserve the composer/conversation and reject stale Session/Location/generation replies
- **AND** unavailable discovery or legacy proof SHALL NOT enable an unreviewed replacement attempt or clear native fences

#### Scenario: Typed preparation cancellation preserves exact pre-effect ownership
- **WHEN** the owner lists or reviews an exited preparation through `sessions recovery-preparation`
- **THEN** cancellation SHALL require its exact reviewed fingerprint and stable request ID, with malformed/incomplete options refused before service startup
- **AND** replies SHALL match the selected Session/activity/lifetime and any published origin; source-admitted, mismatched or unsupported review evidence SHALL NOT be presented as cancellation authority
- **AND** `sessions recovery-preparation-request` SHALL look up the committed receipt without replay, keep acknowledgement reconciliation explicit, and preserve files, transcript, evidence and independent process/checkout uncertainty


#### Scenario: User Git index changes after native preparation
- **GIVEN** prepared native Git images bind the actual user worktree index, its absence or active split-index dependency, and repository routing
- **WHEN** an index is created, replaced or changed after preparation, including a linked worktree's independent index
- **THEN** the native planner SHALL refuse before journal/source effects by revalidating bounded retained observations without running Git or writing the user index
- **AND** a same-byte replacement SHALL NOT inherit the original index generation proof
- **AND** a distinct policy fingerprint frame SHALL NOT promote historical proof lacking user-index observations

#### Scenario: User index exceeds native observation bounds
- **GIVEN** an otherwise usable Git repository has a user index exceeding the bounded native metadata reader
- **WHEN** a checkpoint is captured
- **THEN** ordinary canonical capture SHALL remain readable with explicit native-proof unavailability, and SHALL NOT acquire native restore authority or modify the user index
- **AND** index binding alone SHALL NOT prove large-untracked classification, complete user configuration dependencies or managed Git runtime acceptance


#### Scenario: Size-excluded Git file becomes eligible or changes type
- **GIVEN** capture-bound generated rules omit a regular untracked file because it exceeds the configured size threshold
- **WHEN** it shrinks to or below the threshold, disappears, becomes a link or unsupported type, or gains a linked parent before native planning completes
- **THEN** capture/native inventory SHALL refuse the stale size exclusion using no-follow metadata checks without opening omitted contents or following links
- **AND** no journal/source effect SHALL be admitted by that stale plan

#### Scenario: Omitted Git file remains oversized
- **GIVEN** a verified size exclusion still refers to a regular file exceeding the threshold
- **WHEN** its owner changes its contents while it remains oversized
- **THEN** native restore SHALL preserve the omitted file and may restore independently changed eligible paths after normal policy/namespace checks

#### Scenario: Fresh expected capture after a size exclusion expires
- **WHEN** a formerly size-excluded file shrinks and a new expected checkpoint is captured
- **THEN** the current capture SHALL include its exact raw image and remove the expired generated size exclusion
- **AND** target/recorded historical omissions SHALL continue to protect that file from fabricated deletion/restoration
- **AND** this size revalidation SHALL NOT imply original classification provenance, complete user config dependencies or managed Git runtime acceptance


#### Scenario: Generated size classification retains capture provenance
- **GIVEN** native Git capture classifies untracked files against a user's index and resolved configuration
- **WHEN** the generated-rule recipe is published
- **THEN** a version-2 recipe SHALL retain the bounded classification-input digest, and fresh capture/preparation SHALL require matching inputs and skipped names
- **AND** classification SHALL use private index/shared-index copies, effective configuration resolved in the original gitdir/branch context, and retained rule observations
- **AND** original index/shared-index bytes and metadata SHALL remain unchanged on the supported bound path

#### Scenario: Classification or private copied inputs change during collection
- **WHEN** the original index/config/rules change during classification or a private copied config/index changes unexpectedly
- **THEN** the result SHALL NOT grant native authority, and canonical-readable fallback SHALL explicitly report unavailable native proof
- **AND** Git's timestamp refresh on disposable shared-index copies SHALL NOT waive their content, inode, attribute or route checks, or any strict original-input check

#### Scenario: Git configuration command override cannot redirect snapshot writes
- **GIVEN** the process environment contains GIT_CONFIG pointing at an unrelated file
- **WHEN** internal snapshot Git configuration is initialized or queried
- **THEN** the command-only override SHALL be cleared with repository redirects, internal writes SHALL target the shadow, and the unrelated file SHALL remain unchanged

#### Scenario: Historical generated rules lack classification provenance
- **WHEN** an existing version-1 generated recipe has no classification digest
- **THEN** it SHALL remain readable for canonical capture but SHALL NOT be promoted into native classification proof
- **AND** a fresh supported capture SHALL regenerate current proof without changing historical checkpoint image identities or exclusion protections


#### Scenario: Empty directories preserve checkpoint identity
- **GIVEN** captured file images, effective configuration, ignores and size-excluded names are unchanged
- **WHEN** harmless empty directories add only absent source .gitignore observations
- **THEN** those absence checks SHALL remain available during verification without changing serialized classification provenance or checkpoint identity

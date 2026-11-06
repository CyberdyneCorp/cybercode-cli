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

## MODIFIED Requirements

### Requirement: Non-git fallback
(P1) In non-git Locations the system SHALL snapshot by copying changed files with verified BLAKE3 identity into private `<data>/snapshot/<project_id>/<Location identity>/files/<hash>` storage, up to `snapshots.fallback_max_bytes` (default 200 MiB per Location). Non-git projects SHALL use `global` with an isolated Location namespace. When the limit is exceeded it SHALL disable code rewind for that Location with a one-time warning while conversation work continues. Unsupported native storage SHALL be unavailable explicitly rather than weakening restore guarantees.

#### Scenario: Non-git project
- **WHEN** a Session runs in an admitted folder without git
- **THEN** edits remain rewindable through private Location-bound file-copy snapshots

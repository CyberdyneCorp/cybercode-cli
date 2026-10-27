## ADDED Requirements

### Requirement: Branch-preserving rewind completion
(P1) CLI/TUI code, conversation and combined rewind SHALL use the server's owned staged-revert engine with preview/revalidation and an idle guard. Conversation commit SHALL retain later messages as an inactive branch, not delete transcript or independent recovery evidence. Undo SHALL restore the prior user prompt text; redo SHALL advance or clear the same staged boundary. Code restoration SHALL preserve unrelated edits and report excluded files, conflicts and external effects. Stale Session/Location previews SHALL NOT authorize restoration.

#### Scenario: Rewind and continue
- **WHEN** a user rewinds conversation and then sends a new prompt
- **THEN** the active path advances from the selected boundary while the old continuation remains available on an inactive branch

#### Scenario: User edit after preview
- **WHEN** a file changes after the owner reviews a code-rewind preview
- **THEN** applying the old preview refuses or requires a fresh review rather than overwriting that edit

### Requirement: Bounded non-git snapshot storage
(P1) Non-git snapshots SHALL use private atomic BLAKE3-addressed file objects and Location-bound manifests, respecting ignores and file-size limits without following out-of-scope symlinks. Matching size/mtime SHALL NOT substitute for verified content identity. The default 200 MiB Location cap SHALL disable unavailable code rewind visibly while retaining conversation rewind. Restoration and retention SHALL preserve staged/branch/unknown-effect evidence and use the same conflict and recovery guarantees as Git snapshots.

#### Scenario: Same-size edit with restored timestamp
- **WHEN** a non-git file's bytes change while its size and timestamp remain the same
- **THEN** a later snapshot or restore verifies its hash and does not reuse the old bytes as current evidence

#### Scenario: Interrupted restore
- **WHEN** a file-copy restore loses its owner after a possible partial replacement
- **THEN** recovery retains the journal and backup evidence and does not report success or blindly repeat the mutation

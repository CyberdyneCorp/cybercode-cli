## ADDED Requirements

### Requirement: Daily coding command integration
(P1) `/rewind`, idle double-Esc, `/undo`, `/redo` and `/context` SHALL use authenticated server-owned operations with current Session/Location response fencing. Rewind SHALL show code/conversation/both availability, preview/conflicts/exclusions and external-effect limits; unavailable code snapshots SHALL NOT hide conversation rewind. Undo/redo composer changes SHALL apply only after the matching server operation is acknowledged. Context SHALL show effective sources and pending updates with token estimates and unknown utilization represented explicitly.

#### Scenario: Switch Sessions during undo
- **WHEN** an undo response arrives after the user switches to another Session
- **THEN** it does not replace the new Session's composer or rewind its files

#### Scenario: Non-git cap reached
- **WHEN** a Session's file-copy snapshot cap makes code rewind unavailable
- **THEN** the menu shows the reason and still offers conversation-only rewind

#### Scenario: Reviewed undo and redo
- **WHEN** the owner invokes `/undo` or `/redo`
- **THEN** the TUI reviews a server-owned staged-boundary transition before confirmation and changes composer text only after the matching acknowledgement
- **AND** a newer draft is preserved, a late foreign-context reply is ignored, and an unsent draft with attachment chips is retained with an explicit refusal

#### Scenario: Explicit busy interruption
- **WHEN** the owner opens rewind for a busy Session
- **THEN** no interrupt occurs without an explicit choice, and a fresh server response must acknowledge idle before a review is requested


#### Scenario: Reopen the same Session after an ambiguous response
- **WHEN** a confirmation response fails or arrives after the owner closes and reopens the rewind panel
- **THEN** the client does not infer success or replay a mutation, preserves the composer, and requires fresh status and review

#### Scenario: Preserve unrelated files through repeated undo
- **WHEN** several reviewed code stages are replayed and then cleared
- **THEN** the original baseline and durable set of changed paths constrain clear, and unrelated owner files are not adopted into redo authority

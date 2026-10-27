## ADDED Requirements

### Requirement: Daily coding command integration
(P1) `/rewind`, idle double-Esc, `/undo`, `/redo` and `/context` SHALL use authenticated server-owned operations with current Session/Location response fencing. Rewind SHALL show code/conversation/both availability, preview/conflicts/exclusions and external-effect limits; unavailable code snapshots SHALL NOT hide conversation rewind. Undo/redo composer changes SHALL apply only after the matching server operation is acknowledged. Context SHALL show effective sources and pending updates with token estimates and unknown utilization represented explicitly.

#### Scenario: Switch Sessions during undo
- **WHEN** an undo response arrives after the user switches to another Session
- **THEN** it does not replace the new Session's composer or rewind its files

#### Scenario: Non-git cap reached
- **WHEN** a Session's file-copy snapshot cap makes code rewind unavailable
- **THEN** the menu shows the reason and still offers conversation-only rewind

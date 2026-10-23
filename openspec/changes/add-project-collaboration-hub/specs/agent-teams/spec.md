## ADDED Requirements

### Requirement: Human-owned project participants
(P3) A `project-collaboration` project SHALL coordinate independently owned human/agent participants without turning them into P2 teammates under one lead's permissions or budget. An owner MAY explicitly publish its local lead or teammate Session as a project agent; doing so SHALL NOT publish siblings, expose private team state or grant remote team spawning/merging. The project task board SHALL remain distinct from a local Team board unless a user explicitly links a task reference. Organization/friend membership SHALL NOT inherit the local Team's six-teammate limit or grant execution on another member's machine. Remote Runner scheduling and Git-bundle `team_merge` SHALL retain the separate Distributed teammates contract.

#### Scenario: Publish one teammate
- **WHEN** a member publishes its local `frontend` teammate to a collaboration project
- **THEN** that Session can use scoped collaboration messages/tasks, while its sibling Sessions, local board, Mode ceiling and budget remain under their current owner

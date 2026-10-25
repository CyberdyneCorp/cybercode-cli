## ADDED Requirements

### Requirement: Workflow agent transcript expiry safeguards
(P2) Finished workflow-agent transcript retention SHALL use `storage.retention.workflow_agent_days` (default 30, zero disables) in the existing retention sweep. Cleanup SHALL check terminal completion age and reference eligibility atomically, protect active/reopened runs, active descendants, pending decisions and pinned/replay-required data, and preserve run summaries/results, usage charges and host verification evidence. Expired transcripts SHALL have an explicit tombstone in API/UI/export results. Transcript cleanup SHALL NOT remove candidate branches or manufacture an empty historical conversation.

#### Scenario: Eligible transcript expires
- **WHEN** an unprotected terminal workflow agent's transcript is older than the configured window
- **THEN** cleanup removes eligible transcript content while keeping the run summary, usage and verification records and reports that its transcript expired

#### Scenario: Run reopens during cleanup
- **WHEN** a run becomes active or replay-dependent before the deletion transaction commits
- **THEN** its referenced transcript is retained rather than deleted using stale eligibility

#### Scenario: Retention disabled
- **WHEN** `storage.retention.workflow_agent_days` is zero
- **THEN** the sweep does not expire workflow-agent transcripts under this rule

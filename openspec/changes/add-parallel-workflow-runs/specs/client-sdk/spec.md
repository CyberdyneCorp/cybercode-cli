## MODIFIED Requirements

### Requirement: Workflow helpers
(P2) `client.workflows.run({ script | name, args, budget?, max_concurrent? })` SHALL start a Workflow Run and return a handle with `events()`, `status()`, `pause()`, `resume()`, `cancel()` and `result()`. `client.workflows.list({ status?, session_id?, directory?, limit? })` SHALL return handles for the runs that `GET /api/v1/workflows/runs` lists, each carrying its summary and following further pages lazily. `client.workflows.get(run_id)` SHALL return the handle of an existing run, so a script can attach to a run that another client started. `result()` SHALL treat `queued` like `running`, and `events()` SHALL yield the run's `workflow.*` and `workflow.run.progress` events only.

#### Scenario: Await a workflow
- **WHEN** a script calls `await (await client.workflows.run({ name: "audit" })).result()`
- **THEN** it receives the run's returned value or a typed `WorkflowFailedError`

#### Scenario: Attach to running runs
- **WHEN** a script calls `client.workflows.list({ status: ["running", "queued"] })` while three runs are active
- **THEN** it receives three handles, and `await handle.result()` on each resolves when that run ends

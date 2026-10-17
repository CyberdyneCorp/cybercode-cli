## ADDED Requirements

### Requirement: Workflow run indicators
(P2) While any Workflow Run of the attached Location is `running`, `queued` or `paused` with agents still settling, the TUI SHALL show three indicators.
- **Sidebar.** When the sidebar is shown, a Workflows section after Context SHALL list runs owned by the current Session first, then the other active runs of the Location, at most 5 rows and then `+N more`. Each row SHALL show the name, a known-progress nyan bar of 6 cells and `completed/total`, or `#<n> in queue` instead of the bar and count while the run is queued. Enter on a row SHALL open that run in the Workflow monitor.
- **Status line.** A `workflows` status line field SHALL show `⚙ <n> workflows` (`⚙ 1 workflow` for one), followed by `· <q> queued` when runs are queued, and SHALL be omitted when no run is active.
- **Progress line.** The Nyan progress indicator's known-progress mode SHALL be fed by the current Session's running runs. With one, it SHALL show that run's completed over total agents. With several, it SHALL show the sum of completed over the sum of totals with the label `<n> workflows <done>/<total> agents · <pct>%`.

All values SHALL come from `GET /api/v1/workflows/runs` and the `workflow.*` events, and SHALL be re-read when the event stream reconnects. The section, field and progress feed SHALL disappear when the last active run ends.

#### Scenario: Two runs in the sidebar
- **WHEN** the current Session owns running runs `audit` (31/100) and `review` (0/2) and the sidebar is docked
- **THEN** the Workflows section shows `audit` and then `review`, each with its bar and count, and the footer shows `⚙ 2 workflows`

#### Scenario: Queued run in the sidebar
- **WHEN** `max_running_runs` is 1, `review` is running and `audit` and `deploy` are queued behind it
- **THEN** the Workflows section shows `deploy` with `#2 in queue`, `audit` with `#1 in queue` and `review` with its bar and `0/1`

#### Scenario: Aggregate progress
- **WHEN** the current Session's two running runs have 30 of 100 and 12 of 30 agents done
- **THEN** the progress line's trail covers 32% of its width and reads `2 workflows 42/130 agents · 32%`

#### Scenario: Queued count in the footer
- **WHEN** 8 runs are running and 2 are queued in the Location
- **THEN** the footer shows `⚙ 10 workflows · 2 queued`

### Requirement: Workflow run notifications
(P2) When a Workflow Run reaches `completed`, `failed`, `budget_exceeded` or `stopped`, or is paused by a server restart, the TUI SHALL show a one-line notice. The notice SHALL be added to the owner Session's transcript when that Session is open, and SHALL appear as a transient toast otherwise. It SHALL read `✓ <name> completed · <n> agents · <cost> · <elapsed>`, `✗ <name> failed: <diagnostic kind and message>`, `$ <name> budget exceeded: <limit>`, `■ <name> stopped` or `⏸ <name> paused by restart`.

While the terminal is unfocused, the same text SHALL be delivered through `tui.notifications`, as "Attention notifications" defines. `tui.workflow_notifications` SHALL select which runs notify: `own` (default: runs owned by the current Session, and runs this TUI started, paused, resumed or stopped), `location` (every run of the Location) or `off`. Each attempt's end SHALL notify at most once per TUI, including across event-stream reconnects.

#### Scenario: Background run finishes
- **WHEN** a run started by the current Session's `workflow` tool completes while the terminal is unfocused
- **THEN** the transcript gains `✓ audit completed · 100 agents · $4.10 · 21m` and a desktop notification with the same text is shown

#### Scenario: Run of another Session
- **WHEN** a run started by `cyber workflows run` in another terminal fails and `tui.workflow_notifications` is `own`
- **THEN** no notice is shown, and the run's status is visible in `/workflows`

#### Scenario: Short run of another Session with location
- **WHEN** `tui.workflow_notifications` is `location` and another Session's run in this Location fails before the TUI's first read of it
- **THEN** the TUI still shows its `✗ <name> failed: …` toast, once

#### Scenario: No duplicate after reconnect
- **WHEN** the event stream drops and reconnects after a run completed and was notified
- **THEN** the resync does not show the notice again

## MODIFIED Requirements

### Requirement: Workflow monitor
(P2) The system SHALL provide `/workflows` (also from the command palette and from the sidebar Workflows section). It SHALL list every Workflow Run of the attached Location, active runs first and then the newest. `a` SHALL toggle between this Location and all Locations.

Each row SHALL show the name, status (`paused (<n> settling)` while a paused run's agents finish, `#<n> in queue` while queued), current phase, a known-progress nyan bar of completed over total agents, agents `done/running/queued/failed`, tokens, cost against budget, and active elapsed time. The bar SHALL animate only on the selected running row. Rows SHALL update live from `workflow.run.progress` and the `workflow.*` events, and the list SHALL be re-read when the event stream reconnects.

Enter on a run SHALL open its run view: the agent tree grouped by phase (each phase with `done/total`, plus its running and queued counts), with one row per agent showing status, label or ID, tier or model, tokens, cost, duration and branch. Enter or `t` on an agent SHALL open its child Session transcript with live updates, and Esc SHALL return to the run view and then to the list.

Keys:
- `p` SHALL pause a running or queued run, or resume a paused one.
- `r` SHALL resume (relaunch) a paused, failed, stopped or budget-exceeded run, offering a new revision when the script changed.
- `x` SHALL stop a run after confirmation.
- `R` SHALL resume every run interrupted by a restart.
- `s` SHALL open the run's script.
- `l` SHALL show its log.

Every action SHALL use the public `workflow` routes, act on the selected run only, and report a refusal (for example `Run has agents still exiting; retry shortly`) on the row.

A running Workflow Run SHALL show a compact progress line above the composer, as "Workflow run indicators" defines.

At 80 columns, phase and elapsed time SHALL be dropped. Below 60 columns, each run SHALL take two rows. Screen readers SHALL receive each row as one sentence with every value in words.

#### Scenario: Pause a run
- **WHEN** the user presses `p` on a running Workflow Run with 6 agents in flight
- **THEN** no new agents start, in-flight agents finish, and the run shows `paused (6 settling)` and then `paused`

#### Scenario: Budget display
- **WHEN** a run has spent $1.80 of a $5.00 budget
- **THEN** the monitor shows `$1.80 / $5.00` and colors it warning once spend exceeds 80%

#### Scenario: Several runs at once
- **WHEN** three runs are running and one is queued in the Location
- **THEN** `/workflows` lists all four with their live counts, and pausing one leaves the other rows unchanged

#### Scenario: Drill into an agent
- **WHEN** the user presses Enter on `audit` and then `t` on a running verify agent
- **THEN** the agent's child Session transcript opens and streams, and Esc returns to the `audit` agent tree with the agent still selected

## ADDED Requirements

### Requirement: Debug session presentation
(P2) A `debug` tool call SHALL render as a compact card titled `debug <operation>` with a one-line summary: `stopped at <path>:<line> (<reason>)`, `running`, `exited <code>` or the error. Expanding the card SHALL show the stop summary, the variables or the output tail exactly as returned to the model, with terminal controls escaped. `/tasks` SHALL list an active debug session as a `job_` row of kind `debug` whose last line is its state (`stopped at src/app.py:42 (breakpoint)`), and Stop on that row SHALL end the debug session. The transcript SHALL show one notice when a debug session starts (`Debugging <program> with <adapter> (<job_id>)`) and one when it ends, with the reason. The TUI SHALL NOT add a source pane, breakpoint gutter or variables panel.

#### Scenario: Stop shown in the card
- **WHEN** a `debug continue` call returns a stop at `src/app.py:42` on a breakpoint
- **THEN** the collapsed card reads `debug continue · stopped at src/app.py:42 (breakpoint)`

#### Scenario: Stop from /tasks
- **WHEN** the user selects the `debug` row in `/tasks` and chooses Stop
- **THEN** the debug session ends, the row shows `cancelled`, and an end notice with reason `stopped by user` appears in the transcript

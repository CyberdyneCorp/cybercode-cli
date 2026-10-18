## MODIFIED Requirements

### Requirement: Background tasks view
(P1) The system SHALL provide `/tasks` (Codex alias `/ps`), listing every background item of the current Session with status, elapsed time and last output line:
- background shell commands (`job_`)
- background subagents (`agt_`)
- monitors
- Workflow Runs (`run_`)
- Loops (`lop_`)

From the list the user SHALL be able to view full output, attach to a subagent thread, or stop one item. `/stop` SHALL stop all background shell commands after confirmation.

#### Scenario: Stop one background command
- **WHEN** the user selects a running `npm run dev` job and presses `x`
- **THEN** the job is terminated, its status becomes `cancelled`, and the model is notified at the next Safe Boundary

#### Scenario: Keys of the tasks list
- **WHEN** the tasks list is open
- **THEN** `x` SHALL stop the selected item while no filter text is typed (Ctrl+S stops it at any time), and typing otherwise filters the list
- **AND** Enter SHALL open a subagent's thread, a Workflow Run in the Workflow monitor, or the end of a command's or monitor's log in a scrollable view
- **AND** Loops SHALL be listed once the P2 `loops-scheduling` capability exists; until then there are none to list

#### Scenario: Background input and Ctrl+B
- **WHEN** the composer input starts with `&`
- **THEN** the input mode SHALL read `background shell`, and submitting it SHALL start the rest as a background Job of the Session
- **WHEN** the user presses Ctrl+B (the rebindable `background_command` binding)
- **THEN** a running Session's foreground `bash` call SHALL move to the background, and with no Turn running the TUI SHALL say `No command is running` and send nothing

### Requirement: Subagent threads and side chat
(P1) The system SHALL provide `/agent` (alias `/subagents`) to switch the view to any running or finished subagent thread of the Session, show its transcript, and send it messages when it is resumable. (P1) `/btw <question>` (alias `/side`) SHALL open an ephemeral overlay chat. The side chat SHALL see the current Session context read-only, SHALL NOT add messages to the main Session, and SHALL be discarded on close unless the user chooses `Insert answer`.

#### Scenario: Side question does not pollute history
- **WHEN** the user asks `/btw what does the --frozen flag do?` while the main agent works
- **THEN** the answer appears in an overlay and the main Session history gains no message

#### Scenario: Overlay keys
- **WHEN** the side chat overlay shows an answer
- **THEN** Esc SHALL discard it, and `i` or Enter (`Insert answer`) SHALL close the overlay and insert the answer into the composer for the user to edit and send
- **AND** a bare `/btw` SHALL only show its usage, and `/btw` SHALL be listed in the palette, the help and `GET /api/v1/commands`

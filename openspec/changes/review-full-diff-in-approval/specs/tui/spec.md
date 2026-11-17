## MODIFIED Requirements

### Requirement: Permission prompt
(P0) The system SHALL show a blocking prompt for each pending permission request of the active Session tree. The prompt SHALL show the tool title, the resources, and the whole diff for edits. (P1) When the details do not fit, they SHALL scroll with PgDn, PgUp and Home above the choices, which stay in view with a hint naming those keys. It SHALL offer:
- **Allow once**
- **Allow always**, followed by a confirmation step listing the exact save patterns and their scope (project or user)
- **Reject**, with an optional feedback message sent back to the model

Requests SHALL be shown one at a time in arrival order.

(P1) With the mouse captured, a click on **Allow once**, **Allow always…** or **Reject** SHALL only select it. A second click on the same option, with no key pressed between and while the same request is pending, SHALL act as Enter on it. A single click SHALL never reply to a request, and a press that comes within 500 ms of a press made before the request was shown SHALL be absorbed as the rest of that multi-click: it SHALL neither select nor arm an option. The Allow always confirmation step, the rejection feedback and the blocked-call approval SHALL take no clicks.

#### Scenario: Review a long edit before approving
- **WHEN** an edit's diff is 40 lines and the prompt shows the first lines
- **THEN** PgDn scrolls the diff to its last line while `Allow once`, `Allow always…` and `Reject` stay visible

#### Scenario: Allow always confirmation
- **WHEN** bash requests `npm run test -- --watch=false` and the user picks Allow always
- **THEN** the TUI shows `Always allow: bash "npm run test *" (project)` and saves the approval only after the user confirms

#### Scenario: Reject with feedback
- **WHEN** the user rejects an edit and types `use the existing helper instead`
- **THEN** the reply is `reject` with that message, and the model receives it as a tool error instead of the Drain halting

#### Scenario: A single click cannot grant
- **WHEN** bash asks to run `rm -rf build` and the user clicks `Allow once` once
- **THEN** `Allow once` is selected and no reply is sent; a second click on it replies `once`

#### Scenario: A triple click does not answer the prompt it raised
- **WHEN** the user triple-clicks an explorer file behind an `ask` rule, and the permission request is shown under the pointer after the first press
- **THEN** the other two presses select nothing and no reply is sent; a later double click on `Allow once` replies `once`

#### Scenario: Armed click does not carry over
- **WHEN** the user clicks `Allow once` on one request, that request is answered from another client, and the next request appears where the click lands again
- **THEN** the click only selects `Allow once` on the new request and sends no reply

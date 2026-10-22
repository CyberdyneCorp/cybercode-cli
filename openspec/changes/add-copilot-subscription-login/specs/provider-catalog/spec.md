## ADDED Requirements

### Requirement: GitHub Copilot subscription provider
(P1) When a Copilot login is stored, the catalog SHALL include provider `copilot` (name `GitHub Copilot (subscription)`), with credential source `login`. Its base URL SHALL be the API endpoint the last Copilot token named.

- **Models:**
  - The catalog SHALL list the models `GET <api>/models` returns with `model_picker_enabled: true`, leaving out any whose `policy.state` is `disabled`.
  - Each model SHALL take its context and output limits, vision and tool support from the listed capabilities. It SHALL be unpriced.
  - Each model SHALL use the Responses protocol when its `supported_endpoints` include `/responses`, and Chat Completions otherwise.
  - The list SHALL be cached in `<data>/copilot-models.json`: fetched at login, refreshed when older than 24 hours, and kept unchanged for another 24 hours after a failed refresh.
  - `providers.copilot` configuration SHALL apply on top.
- **Without a login:** the catalog SHALL NOT contain `copilot`.
- **Requests:** every request SHALL carry:
  - the Copilot API token as bearer,
  - `Copilot-Integration-Id`,
  - `Editor-Version: cyber/<version>`,
  - `Openai-Intent: conversation-edits`,
  - `X-Initiator`: `user` for the first model call after a promoted user prompt (an input with `source` `user`, an MCP server prompt the user ran as a slash command, or a `/loop` iteration), and `agent` for every other model call: later calls of the same Turn, calls a tool or hook makes during it (prompt hook checks, `webfetch` summaries, MCP sampling), the first call of a Turn another source opens (Job and monitor handbacks, cross-Session, channel and team messages, workflow results, hook rows, child Sessions' tasks), titles, compaction and recaps.
  
  Requests with image input SHALL also carry `Copilot-Vision-Request: true`.
- **Protocol:** the Copilot protocol SHALL NOT be selectable through `api.kind` or models.dev data.

#### Scenario: Premium requests counted once per prompt
- **WHEN** a user prompt on `copilot/claude-sonnet-4.5` produces one model call, two tool calls and a second model call
- **THEN** the first request carries `X-Initiator: user` and the second carries `X-Initiator: agent`

#### Scenario: Calls besides the prompt's are the agent's
- **WHEN** a prompt's Turn runs a prompt hook check, a `webfetch` with a prompt on a `copilot` small model and an `agent` delegation to a child Session
- **THEN** the hook check, the summary and the child Session's calls carry `X-Initiator: agent`, and only the prompt's first call carries `user`

#### Scenario: A handback opens an agent Turn
- **WHEN** a background Job's handback (`source: "session"`) is queued and opens a Turn
- **THEN** that Turn's first request carries `X-Initiator: agent`

#### Scenario: Responses-only model
- **WHEN** Copilot lists `gpt-5.6-sol` with `supported_endpoints: ["/responses"]`
- **THEN** Turns on `copilot/gpt-5.6-sol` are sent to `<api>/responses`

#### Scenario: Org-disabled model hidden
- **WHEN** Copilot lists a model with `policy.state: "disabled"`
- **THEN** `cyber models copilot` does not list it

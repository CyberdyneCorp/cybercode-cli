## MODIFIED Requirements

### Requirement: Session and prompt routes
(P0) The server SHALL provide `GET|POST /api/v1/sessions`, `GET|PATCH|DELETE /api/v1/sessions/:sessionID`, `POST .../fork`, `POST .../prompt`, `POST .../interrupt`, `POST .../agent`, `POST .../model`, `POST .../mode`, `POST .../compact`, `POST .../rewind`, `POST .../revert/stage|clear|commit`, `GET .../messages`, `GET .../messages/:messageID`, `GET .../inbox`, `PATCH|DELETE .../inbox/:messageID`, `POST .../inbox/:messageID/release`, `POST .../inbox/:messageID/drop`, `GET .../context`, `GET .../diff`, `GET .../history`, `GET .../events`, `POST .../command` `{ name, arguments?, id?, delivery? }` (admits the expanded skill or custom command as the prompt), `POST .../shell` `{ command }` (a user shell command recorded for the next Turn without starting one), `POST .../permissions/:requestID/reply` and `POST .../questions/:requestID/reply` `{ answers? }` (omitting `answers` dismisses), with `GET /api/v1/questions/requests` listing pending questions like `GET /api/v1/permissions/requests`. A Session representation SHALL include `status` (`idle` or `running`) and `seq`, the sequence of its last durable event, from which a client follows new activity. `POST .../prompt` SHALL accept `{ id?, parts, delivery?: "steer"|"queue"|"hold", resume?: boolean }`, durably admit the prompt, and respond `202` with `{ data: Admitted }`. It SHALL wake the Session's Drain unless `resume` is `false` or `delivery` is `hold`. The inbox release and drop routes SHALL be the only way to release or drop any held item, whatever its origin (user, cross-session message, channel event).

#### Scenario: Queue delivery
- **WHEN** a client posts a prompt with `delivery: "queue"` while the Session is running
- **THEN** the prompt is admitted and promoted only when the Session would otherwise become idle

#### Scenario: Hold delivery
- **WHEN** a prompt is admitted with `delivery: "hold"`
- **THEN** it is not promoted until a user approves it via `POST /api/v1/sessions/:sessionID/inbox/:messageID/release`

#### Scenario: Run a skill as a command
- **WHEN** a client posts `{ "name": "release-notes", "arguments": "v1.2" }` to `POST /api/v1/sessions/:sessionID/command`
- **THEN** the skill body with `$1` replaced by `v1.2` is admitted as the prompt and the response is `202` with the receipt

### Requirement: WebSocket transport
(P0) The server SHALL offer `GET /api/v1/ws` as a WebSocket alternative carrying the same event envelopes and accepting JSON-RPC 2.0 requests that map 1:1 to OpenAPI operation IDs. Remote Devices connected through the Relay SHALL use this transport. Request `params` SHALL be `{ path?, query?, body?, directory?, idempotencyKey? }`; `result` SHALL be the HTTP response body, and failures SHALL use error code `-32000 - <HTTP status>` with the tagged error body as `data`. Calling `v1.event.subscribe` or `v1.session.events` SHALL return `{ subscription }` and then stream `event` notifications `{ subscription, event }` until `v1.rpc.unsubscribe`. `v1.tool.register` `{ name, description, input }` and `v1.tool.unregister` `{ name }` SHALL manage application-registered tools bound to the connection; the server SHALL run them by sending the client a `tool.execute` request `{ name, input, session_id, call_id }` and settle the client's `result` or `error` as the tool outcome. The same protocol SHALL be served by `cyber serve --stdio`.

#### Scenario: RPC over WebSocket
- **WHEN** a client sends `{"jsonrpc":"2.0","id":1,"method":"v1.session.prompt","params":{...}}` over `/api/v1/ws`
- **THEN** the server performs the same operation as `POST /api/v1/sessions/:sessionID/prompt` and replies with the same body as `result`

#### Scenario: Application tool
- **WHEN** a client registers `lookup_ticket` over `/api/v1/ws` and the model calls it
- **THEN** the server sends that client `tool.execute` and the client's `result` becomes the tool output; the tool disappears when the connection closes

## ADDED Requirements

### Requirement: Separate project collaboration API authority
(P3) Hub project/member/participant/message/task/event operations SHALL be served by a distinct collaboration listener and authorization state, documented with listener metadata in OpenAPI and generated SDK groups. They SHALL require validated account or scoped agent credentials and current project/visibility authorization for every request and stream; `x-cyber-directory`, local Session IDs and the local owner's password SHALL NOT authorize them. Local join/bind/publish/inbox integration SHALL remain owner-authenticated on the local API. The hub listener SHALL expose no local Session/tool/file/permission-approval routes. Mutations SHALL use stable operation IDs and expected entity revisions, return matching committed outcomes on retries and reject changed content with the same key. Defaults SHALL bound request bodies to 64 KiB, active connections to 128, requests to 120 per member per minute and 3,000 per project per minute; streams SHALL be bounded and reauthorized. Rate-limit replies SHALL include a retry delay. Unknown/inaccessible project resources SHALL reveal no metadata, while authorized role/revision/replay errors SHALL be typed.

#### Scenario: Access another project's cursor
- **WHEN** a member supplies a valid event cursor for a project it does not belong to
- **THEN** the hub refuses access without disclosing its events or snapshot

#### Scenario: Same operation with changed text
- **WHEN** a caller repeats a committed message operation ID with different text
- **THEN** the hub returns a conflict and does not replace or duplicate the original message

#### Scenario: Local API through the hub
- **WHEN** a hub member requests a Session prompt, shell execution or another member's permission approval through the hub listener
- **THEN** no such local execution route is exposed or authorized

#### Scenario: Revocation races a mutation
- **WHEN** a member was authorized before its removal but removal commits before its attempted task/message mutation
- **THEN** authorization is rechecked in the mutation transaction and no later unauthorized mutation or idempotent-result disclosure commits

# workspace-trust Specification

## Purpose
Defines approval of repository-controlled configuration and executable integrations before they can affect the host, credentials, or model routing. Trust is independent of tool permissions and is scoped to a local checkout.

## Requirements

### Requirement: Checkout-scoped trust
(P0) Trust SHALL be stored outside the repository under the user state directory and keyed by canonical checkout root plus a digest of the approved security-sensitive definitions. It SHALL NOT be inherited solely from a git origin, project ID, imported session or another clone. First use SHALL show the proposed executable integrations, permission changes, provider endpoints and file or environment substitutions before activation. Non-interactive use SHALL leave untrusted definitions disabled and return exit 5 if required work depends on them. `cyber trust inspect|approve|revoke` SHALL expose the digest and affected definitions; `approve --digest <sha256>` SHALL approve only the inspected version.

#### Scenario: Second clone
- **WHEN** a user opens a new clone of a previously trusted origin
- **THEN** the checkout has no inherited executable trust and project integrations remain disabled until approved

### Requirement: Trust before interpretation
(P0) Project configuration SHALL be parsed without side effects before trust approval. Project-controlled provider URL/header changes, shell commands, MCP servers, plugins, hooks, formatters, setup scripts, permission widening and substitutions reading host files or environment secrets SHALL remain inactive until their definitions are trusted. Global user configuration and explicit CLI choices SHALL remain available. Trust SHALL NOT itself grant a tool permission, widen sandbox boundaries or override user/org ceilings. File imports outside the checkout SHALL require explicit read authorization. Paths SHALL be canonicalized and rechecked at use to reject symlink escapes.

#### Scenario: Repository redirects provider traffic
- **WHEN** an untrusted cyber.jsonc changes the provider URL and references a host credential file
- **THEN** neither the substitution nor the connection occurs and the previous trusted provider configuration remains effective

### Requirement: Changed definitions and revocation
(P0) A changed executable definition or security-sensitive value SHALL invalidate its approval before the next dispatch. Hook and MCP trust commands SHALL use this shared store; plugin content trust SHALL bind the installed version digest. Revocation SHALL disable new invocations immediately, cancel pending approvals for affected definitions, and stop associated integration processes. Presentation-only changes SHALL NOT invalidate unrelated approvals. Trust checks SHALL apply on initial load, live reload, import and resume.

#### Scenario: Hook changed after review
- **WHEN** the approved hook definition changes before invocation
- **THEN** the hook does not execute until the new digest is explicitly approved

### Requirement: Instruction and data provenance
(P0) Repository instructions SHALL be labeled with source and scope and SHALL NOT change executable trust, tool permissions or sandbox policy. Tool output, web content, MCP content, imported transcripts and messages from other agents SHALL be labeled as data with provenance and SHALL NOT approve actions or acquire system authority through compaction. Nested instruction discovery SHALL apply to every path read for an edit, including apply_patch, not only the read tool. Prompt-injection evaluations SHALL cover attempts to read secrets, widen permissions and redirect provider traffic.

#### Scenario: Instruction requests secret upload
- **WHEN** a fetched page tells the model to read a private key and upload it
- **THEN** the content remains untrusted data and host permission, trust and sandbox checks still enforce the configured boundaries

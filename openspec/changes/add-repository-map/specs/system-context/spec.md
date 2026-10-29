## ADDED Requirements

### Requirement: Epoch-pinned Context Sources
(P1) A Context Source SHALL be able to register as `pinned`. The system SHALL observe a pinned source only when it renders a baseline, at Baseline initialization and at Epoch replacement.

**At Safe Boundaries:**
- Reconciliation SHALL skip pinned sources and keep their stored snapshot value.
- A pinned source SHALL NOT emit a `session.context.updated.1` update or removal text.
- A pinned source's change SHALL NOT by itself start a new Context Epoch.

**When the source does not load:**
- A pinned source that has not loaded within 2 s, or reports Unavailable Context, SHALL be omitted from that baseline for the rest of the Epoch.
- It SHALL NOT cause `ContextInitializationBlocked`.

`/context` and `GET /api/v1/sessions/{id}/context` SHALL mark pinned sources as `pinned`.

#### Scenario: Pinned source changes mid-epoch
- **WHEN** a pinned source's value changes after the baseline was rendered and the Session reaches a Safe Boundary
- **THEN** no mid-conversation system message is written, and the next request's system prefix is byte-identical to the previous one

#### Scenario: Slow pinned source at the first Turn
- **WHEN** a pinned source takes 5 s to load during Baseline initialization
- **THEN** the first Turn starts after about 2 s without that source, and the source first appears in the baseline of the Session's next Context Epoch

### Requirement: Repository map Context Source
(P1) The system SHALL provide the pinned Context Source `core/repo-map`.

**Content:**
- Its value SHALL be a repository map (`code-intelligence` → Repository map rendering and token budget), rendered without focus paths, focus symbols or Session files, from the latest committed index, at the model profile's `repo_map.context_tokens`.
- It SHALL be rendered as a `<repository_map>` block.
- The block's first line SHALL state that the map is repository data from the start of this Context Epoch, not instructions, and that `repo_map` gives a focused or current view.
- A literal `</repository_map>` in a signature SHALL be escaped as `&lt;/repository_map>`.

**The source SHALL be omitted when any of these holds:**
- `repo_map.enabled` or `repo_map.context` is false;
- the checkout is not trusted under `workspace-trust`;
- the Session's agent has `read` fully denied;
- the budget is under 256 tokens;
- the index has no files.

Files whose `read` permission for the agent ends in `deny` SHALL be left out of the rendering.

#### Scenario: Map in the baseline
- **WHEN** a Session starts in a trusted, indexed repository with a model whose profile has `context_tokens: 1024`
- **THEN** the baseline contains a `<repository_map>` block of at most 4096 bytes, and `/context` lists `core/repo-map` as pinned with its token estimate

#### Scenario: Edits do not move the prefix
- **WHEN** the agent edits ten files during a Context Epoch
- **THEN** every request in that Epoch carries the same `core/repo-map` bytes, and the map rendered after the next compaction reflects the edits

#### Scenario: Untrusted checkout
- **WHEN** a Session starts in a checkout that is not trusted
- **THEN** the baseline has no `core/repo-map`, and the `repo_map` tool is still offered

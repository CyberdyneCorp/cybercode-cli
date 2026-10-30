## ADDED Requirements

### Requirement: Repository map budgets in model profiles
(P1) The model behavior profile SHALL include `repo_map: { context_tokens, tool_tokens }`, whose built-in family defaults are 1024 and 2048 for every family.

**Resolution:**
- These fields SHALL resolve through the profile layers of Model behavior profiles.
- Top-level config `repo_map.context_tokens` and `repo_map.tool_tokens` SHALL take precedence over the built-in layers and catalog metadata, and SHALL yield to `providers.<provider>.models.<model>.profile.repo_map`.

**Window caps, applied last:**
- `context_tokens` SHALL be capped at 3% of the model's context window, rounded down to a multiple of 128.
- `tool_tokens` SHALL be capped at 6% of the window.

The resolved values SHALL appear in `profile` in `cyber models --json` and `GET /api/v1/models`. A model switch within a Context Epoch SHALL NOT change the `core/repo-map` value already in that Epoch's baseline.

#### Scenario: Small local model
- **WHEN** an Ollama model with a 32768-token window has no profile override
- **THEN** its profile shows `repo_map.context_tokens: 896` and `repo_map.tool_tokens: 1966`

#### Scenario: Per-model override wins over the top-level key
- **WHEN** config sets `repo_map.tool_tokens = 4096` and `providers.corp.models.coder.profile.repo_map.tool_tokens = 1024`
- **THEN** `corp/coder` uses 1024, and other models with large enough windows use 4096

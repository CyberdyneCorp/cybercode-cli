## MODIFIED Requirements

### Requirement: Provider base prompts and output styles
(P0) The system SHALL start the system prompt with the agent's `system` text when set, otherwise with a base prompt selected by model family (OpenAI, Anthropic, Gemini, local/open-weights, default), and SHALL append the selected output style (`default`, `concise`, `explanatory`, `learning`, or a custom style from `~/.config/cyber/output-styles/*.md` or `.cyber/output-styles/*.md`). The model family SHALL be resolved from the catalog model, not from the provider ID: open-weights model names (such as `gpt-oss`, `llama`, `qwen`, `mistral`, `deepseek`, `gemma`) SHALL select local/open-weights first. The model's family or ID SHALL then select OpenAI (`gpt-`, `o1`, `o3`, `o4`, `codex`), Anthropic (`claude`) or Gemini (`gemini`). The protocol SHALL decide next (Anthropic Messages selects Anthropic, OpenAI Responses selects OpenAI), then a local endpoint (no authentication or a loopback URL) selects local/open-weights, and every other model SHALL use the default base prompt. The base prompts and built-in styles SHALL be embedded in the binary. The `default` style SHALL add nothing to the system prompt. The system prompt, including the style, SHALL be persisted with the Context Epoch. A change of the resolved model family, or of the effective output style's name or text, SHALL take effect at the next Turn by starting a new Context Epoch, and SHALL NOT be sent as a mid-conversation system message. An Epoch recorded without a model family SHALL be compared by provider only, so a restart never rebuilds its prefix.

#### Scenario: Custom output style
- **WHEN** the user selects output style `reviewer` defined in `.cyber/output-styles/reviewer.md`
- **THEN** its body is appended to the baseline under the next Context Epoch

#### Scenario: Family through a gateway
- **WHEN** a Session uses `openrouter/anthropic/claude-sonnet` and its agent has no `system` text
- **THEN** the system prompt starts with the Anthropic base prompt

#### Scenario: Open-weights model on a hosted provider
- **WHEN** a Session uses a model whose ID is `gpt-oss-120b`
- **THEN** the system prompt starts with the local/open-weights base prompt, not the OpenAI one

#### Scenario: Agent text replaces only the base prompt
- **WHEN** the active agent sets `system` and the Session's output style is `concise`
- **THEN** the system prompt is the agent's text followed by the `concise` style, with no base prompt

#### Scenario: Switching family starts a new epoch
- **WHEN** a Session on `copilot/gpt-5` switches to `copilot/claude-sonnet` between Turns
- **THEN** the next Turn starts a new Context Epoch whose prefix is the Anthropic base prompt

#### Scenario: Style change mid-session
- **WHEN** the user switches a Session from `default` to `explanatory` while a Turn is running
- **THEN** the running Turn keeps its prefix, and the next Turn starts a new Context Epoch whose prefix ends with the `explanatory` style, with no mid-conversation message about it

#### Scenario: New epoch while a source is unavailable
- **WHEN** a Session with an epoch switches its style, or its model family, while another source or the selected style's file cannot be read
- **THEN** the next Turn runs on a new Context Epoch that keeps the previous value of each unavailable source, and only a Session's first epoch is blocked by one

## ADDED Requirements

### Requirement: Output style definitions
(P0) The built-in output styles SHALL be `default`, `concise`, `explanatory` and `learning`. Custom output styles SHALL be Markdown files directly inside `~/.config/cyber/output-styles/` (user) and `<project root>/.cyber/output-styles/` (project). A style's name SHALL be its file stem and SHALL match `^[a-z0-9][a-z0-9_-]{0,63}$`. Optional YAML frontmatter MAY set `name`, which SHALL equal the stem, and `description`. It MAY also set `keep-coding-instructions`, which SHALL be accepted and ignored. Any other frontmatter key SHALL make the file invalid. The trimmed body SHALL be the style text, and an empty body SHALL be invalid. Only regular files of at most 32 KiB SHALL be read, and links SHALL be refused. A custom style SHALL NOT reuse a built-in name, and a project style SHALL replace a user style of the same name. Project styles SHALL load only while the checkout is trusted (`workspace-trust`). `CYBER_DISABLE_PROJECT_CONFIG=1` SHALL skip project styles. An invalid file, or a held one, SHALL be reported with its path and reason and SHALL NOT prevent the other styles from loading.

#### Scenario: Project style in an untrusted checkout
- **WHEN** an untrusted checkout contains `.cyber/output-styles/reviewer.md`
- **THEN** `reviewer` is not available and the style listing reports `project output styles are held until this checkout is trusted (cyber trust inspect)`

#### Scenario: Style frontmatter cannot carry authority
- **WHEN** `~/.config/cyber/output-styles/fast.md` sets `permissions` in its frontmatter
- **THEN** `fast` is not loaded, a diagnostic names the file and the key, and the other styles load

#### Scenario: Built-in name reserved
- **WHEN** `.cyber/output-styles/concise.md` exists in a trusted checkout
- **THEN** the built-in `concise` style is used and the file is reported as reusing a built-in name

### Requirement: Output style selection
(P0) A primary Session's effective output style SHALL be its own selection when one is set, made at creation or by a switch. Otherwise it SHALL be the `output_style` value of the resolved configuration of its Location, and otherwise `default`. Sessions with a parent (subagents, workflow agents, teammates) SHALL have no output style. A selected name SHALL be checked against the style name pattern before any lookup. An explicit selection (Session creation, a switch, `cyber exec --output-style`) that names an invalid, unknown or held style SHALL be refused with the reason. A configured style that is unavailable SHALL fall back to `default`, and the fallback and its reason SHALL be reported to the user. An output style SHALL contribute text only: it SHALL NOT change tool permissions, Mode or Mode ceilings, sandbox policy, the tools offered or the model.

#### Scenario: Configured style is unavailable
- **WHEN** a project's `cyber.jsonc` sets `output_style = "reviewer"` and the checkout is untrusted
- **THEN** Sessions there use `default`, and the TUI and `cyber exec` show that `reviewer` is held until the checkout is trusted

#### Scenario: Path-like name refused
- **WHEN** a client switches a Session to output style `../../etc/passwd`
- **THEN** the request is refused as an invalid style name and no file is read

#### Scenario: Subagents have no style
- **WHEN** a Session with output style `learning` spawns a subagent
- **THEN** the subagent's system prompt has no output style

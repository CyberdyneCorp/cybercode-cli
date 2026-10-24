## Why

The P0 requirement "Provider base prompts and output styles" (`system-context`) is only partly met. `runtime::context::base_prompt` picks one of three hard-coded sentences by provider id. `openai` and `anthropic` get their own sentence and everything else, Gemini included, gets the local-model one. A Claude or GPT model behind OpenRouter, Copilot or an OpenAI-compatible gateway gets the wrong prompt, and switching models within one provider never changes it. Output styles don't exist: there is no `core/output-style` source, no built-in styles, no user or project style files, and no way to choose a style. Claude Code users expect `/output-style` and `.claude/output-styles/*.md`. The roadmap lists vendor-tuned prompts as the answer to the quality gap against vendor harnesses.

## What Changes

- `system-context`:
  - **Modified "Provider base prompts and output styles":** the system prompt starts with the agent's `system` text when it is set. Otherwise it starts with a base prompt chosen by **model family** (`openai`, `anthropic`, `gemini`, `local`, `default`), resolved from the catalog model, not from the provider id. The selected output style follows it. When the family or the style changes, the next Turn starts a new Context Epoch. The texts are short embedded files under `crates/cyber-core/prompts/`.
  - **Added "Output style definitions":** four built-in styles (`default`, `concise`, `explanatory`, `learning`). Custom styles come from `~/.config/cyber/output-styles/*.md` and `.cyber/output-styles/*.md` (frontmatter `name`, `description`, Markdown body). Names are validated, built-in names are reserved, and a project style wins over a user style of the same name. Project styles load only in a trusted checkout, and an invalid file is skipped with a diagnostic.
  - **Added "Output style selection":** the order is a Session switch, then the value given at creation (`cyber exec --output-style`, the create body), then the `output_style` config key, then `default`. Styles apply to primary Sessions only. A selected style that is unavailable falls back to `default` and shows a warning. A style never changes permissions, Mode ceilings, the sandbox or tools.
- `configuration`: the top-level key `output_style` is new. The supplemental directory is `output-styles/*.md`, which fixes the stale `styles/*.md`.
- `workspace-trust`: project output styles belong to the checkout's trust digest. Until the checkout is trusted they are held back, like project team definitions.
- `session-runtime`: the durable `session.output_style.switched.1` event joins the agent, model and mode switch events.
- `server-api`, added "Output style routes":
  - `POST /api/v1/sessions/:id/output-style` switches a Session's style.
  - `output_style` is a new field on `POST /api/v1/sessions`.
  - `GET /api/v1/config/output-styles?directory=` lists the styles.
  - `PUT /api/v1/config/output-style` saves the user's default.
- `exec-mode`: `cyber exec --output-style <name>`. An unknown or held style exits with code 2.
- `tui`:
  - Added "Output style picker": `/output-style [name]` and the palette entry **Output style…**.
  - Modified "Settings Config tab": an `output style` row opens the picker, and `output_style` is listed with the core settings.

## Non-goals

- **Distributing output styles through plugins (deferred).** Plugins load a fixed list of named component types (`skills`, `commands`, `agents`, `themes`, …; `plugins/manifest.rs`, `plugins/components.rs`). They don't load content directories generically, so a plugin style would need a new manifest component, discovery defaults, namespacing and plugin-trust binding. That is a P1 `plugins-marketplace` change on its own.
- Importing `.claude/output-styles/` through `cyber import` (`compat-import`). A user can copy the files, since the format is compatible: Claude Code's `keep-coding-instructions` key is accepted and ignored.
- A per-agent `output_style` field, and styles for child Sessions (subagents, workflow agents, teammates). Their output goes to another agent, not to the user.
- User-editable base prompts. An agent's `system` text already replaces the base prompt.

## Impact

- `crates/cyber-core`:
  - new `prompts/` (embedded base prompts and built-in styles),
  - new `output_styles` module (discovery, validation, trust subset), on the pattern of `teams.rs`,
  - `config/load.rs` (`assess_trust` adds the style trust subset),
  - `config/validate.rs` (`output_style` key).
- `crates/cyber-llm`: `catalog` gains `ModelFamily` resolution.
- `crates/cyber-server`:
  - `runtime/context.rs`: `base_prompt(family)` and the style in the system prefix,
  - `runtime/drain.rs`: epoch start, family and style change detection,
  - `runtime/events.rs`, `runtime/model.rs`: the switch event and Session field,
  - `runtime/host.rs`: `ResolvedModel.family`, and a host hook that observes the style,
  - `runtime/side_chat.rs`, `runtime/compaction.rs`: these call `base_prompt`,
  - `http/sessions.rs`: the switch route and the create body,
  - a new `http/output_styles.rs` (listing and the default),
  - OpenAPI.
- `crates/cyber-tools`: `host.rs` observes `core/output-style` and lists the styles.
- `crates/cyber-app`: wiring and production-path tests.
- `crates/cyber-cli`: `exec --output-style`.
- `crates/cyber-tui`: the picker, the palette, the Config tab row.
- `sdk/typescript`: regenerated.
- Docs: README feature table, `docs/features.md`, `docs/local-core-guide.md`, `docs/cli.md` (generated), ROADMAP P0 status.

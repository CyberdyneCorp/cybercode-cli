## Context

How the system prompt is assembled today:

- At a Safe Boundary, `Inner::ensure_epoch` (`cyber-server/src/runtime/drain.rs`) starts a Context Epoch when there is none, when the epoch is stale (after compaction or a branch switch), or when the provider id changed.
- `start_epoch` observes every source. The built-ins come from `runtime::context::observe`; the host contributes the rest through `ToolHost::context_observations_owned`, which includes `core/agent`, the agent's `system` text, from `cyber-tools/src/host.rs`. It then persists `EpochStarted { baseline, snapshot, provider, system_prefix }`.
- `system_prefix` is the `core/agent` value, or otherwise `context::base_prompt(provider)`.
- The request sends `system = [system_prefix, baseline, …]`. `render_baseline` already leaves `core/agent` out of the baseline because it travels as the prefix.
- `reconcile_context` turns source changes into one `session.context.updated.1` message. When `core/agent` disappears, the base prompt is appended to that message.
- `base_prompt` matches the provider id: `openai`, `anthropic`, and anything else, which gets the local-model text. Side chat and compaction call it too.
- The catalog model already carries `family` (from models.dev), `api_kind`, and its provider's `kind`, `url` and `auth_none` (`cyber-llm/src/catalog/mod.rs`). `CatalogResolver::resolve` (`runtime/host.rs`) copies only some of these into `ResolvedModel`.
- `core/output-style` is already a reserved built-in key in "Typed Context Sources", but nothing produces it.
- The closest file loader is `cyber_core::teams`. It reads `~/.config/cyber/teams/*.md` and `.cyber/teams/*.md` as regular, size-capped files with YAML frontmatter. Project files load only while the checkout is trusted, through `trust_subset` in `config::load::assess_trust`, and one file's error never blocks the others. Output styles reuse that shape.

## Decisions

### Prompt texts are embedded files

```
crates/cyber-core/prompts/
  base/core.md            shared Cyber Code base prompt (every family)
  base/openai.md          family addenda, appended to core.md
  base/anthropic.md
  base/gemini.md
  base/local.md           (default has no addendum)
  output-styles/concise.md
  output-styles/explanatory.md
  output-styles/learning.md
```

- `cyber_core::prompts` loads these with `include_str!` and exposes `ModelFamily { Openai, Anthropic, Gemini, Local, Default }` and `base_prompt(family) -> String`, which returns `core.md` followed by a blank line and the family addendum. Built-in styles use the same frontmatter as custom styles and go through the same parser, so a broken built-in fails a unit test, not a user's Session.
- `default` is a built-in style with a description and no body, and it adds nothing to the prompt. Existing Sessions and every test that does not select a style keep their prefix bytes.
- Each text stays short (core under about 25 lines, an addendum or a style under about 15). The model-specific advice is about tool use and editing, which is where the harnesses differ:
  - OpenAI: prefer `apply_patch` when it is offered.
  - Anthropic: use parallel tool calls when they are independent.
  - Gemini: keep tool arguments strictly to the schema.
  - Local: keep tool calls simple, valid JSON, one at a time.

### Model family comes from the model, not the provider id

`cyber_llm::catalog::model_family(provider, model) -> ModelFamily` works on lowercase names. The name is `model.family`, or else the last `/` segment of `model.api_id`. The rules are tried in this order, and the first match wins:

1. Open-weights names: `gpt-oss`, `llama`, `qwen`, `mistral`, `mixtral`, `codestral`, `devstral`, `deepseek`, `gemma`, `phi`, `glm`, `kimi`, `minimax` → `Local`. They come first so that `gpt-oss` is not taken for OpenAI and `gemma` is not taken for Gemini.
2. `claude` → `Anthropic`. `gpt-`, `o1`, `o3`, `o4`, `codex`, `chatgpt` → `Openai`. `gemini` → `Gemini`.
3. Protocol: `ApiKind::Anthropic` → `Anthropic`. `OpenaiResponses` and `ChatgptCodex` → `Openai`.
4. Local endpoint: the provider is `auth_none`, or its URL host is loopback → `Local`.
5. Otherwise → `Default`.

The table is one `const` slice of `(prefix, family)` pairs, so adding a family or a name is a one-line change. `ResolvedModel` gains `family: ModelFamily`. Test resolvers default to `ModelFamily::Default`.

### Composition of the system prefix

```
system_prefix = (core/agent value  |  base_prompt(family))
                [ + "\n\n" + core/output-style value ]      when a non-default style applies
system        = [system_prefix, baseline, deferred-tools block?]
```

- `core/output-style` is an ordinary source in the snapshot, so changes are detected. `render_baseline` leaves it out, as it leaves out `core/agent`, because it travels in the prefix. Its value is `Output style: <name>\n\n<body>`.
- The agent's `system` text replaces only the base prompt. The style is still appended, as the requirement says.
- `EpochStarted` gains `family: Option<String>`. Epochs written before this change have none.

### Changes take effect through a new Context Epoch

- `ensure_epoch` starts a new epoch when the provider changes (as today) or when the resolved family differs from the epoch's `family`. An epoch without `family` (written before this change) compares by provider only, so a restart never rebuilds an old Session's prefix.
- `reconcile_context` observes the sources. When `core/output-style` differs from the snapshot (a switch, an `output_style` config edit, or an edit of the selected style's file), it starts a new epoch instead of committing a `session.context.updated.1` update. The other changed sources are folded into that new baseline, and nothing is sent as a mid-conversation message for the style. This reuses `start_epoch` unchanged: it re-observes and commits `EpochStarted`.
- The switch route commits `session.output_style.switched.1` and does nothing else. The Drain picks the change up at the next Safe Boundary, so a running Turn finishes with the style it started with.
- Side chat already copies the epoch's `system_prefix`, so it keeps the style. Compaction uses `base_prompt(family)` with no style, because a summary is not prose for the user.

### Output style definitions (`cyber_core::output_styles`)

- `load(sources) -> Styles { styles, issues }` follows `teams::load`. The sources are the Location, the global config directory, `project_trusted` and `project_enabled` (that is, `CYBER_DISABLE_PROJECT_CONFIG` is not set).
- Files are the `*.md` directly in `output-styles/`, sorted, at most 128. A file must be regular: a link or anything else is refused. It is at most 32 KiB. Both checks are made on one handle opened without following a final link (`O_NOFOLLOW`, or `FILE_FLAG_OPEN_REPARSE_POINT` on Windows), and the bound applies to the bytes read, so a file swapped for a link or grown after listing is never read past it.
- Frontmatter is optional YAML:
  - `name` must equal the file stem when present.
  - `description` is a string, shown in pickers.
  - `keep-coding-instructions` is Claude Code compatibility, accepted and ignored: the base prompt is always kept.
  - Any other key makes the file invalid. A style is only text, and an unknown key such as `tools`, `permissions` or `mode` would suggest it can do more.
- The body is the Markdown after the frontmatter, trimmed. An empty body is invalid.
- The name is the file stem and must match `^[a-z0-9][a-z0-9_-]{0,63}$`. The same check (`output_styles::valid_name`) runs on every selection: config, create body, switch, CLI flag. That rules out path traversal and odd names before any lookup. Lookup is a map lookup, never a path join on user input.
- Precedence:
  - built-in names are reserved, and a file that reuses one is skipped with an issue;
  - a project style replaces a user style of the same name, and the result records `shadows`;
  - two files of the same scope cannot share a name, because names are file stems and files sit in one directory.
- Trust: `output_styles::trust_subset(location)` adds `.cyber/output-styles/*.md`, as relative path plus contents, to the checkout's trust digest in `assess_trust`, next to the teams subset. While the checkout is untrusted, project styles are not read. One issue is recorded: `project output styles are held until this checkout is trusted (cyber trust inspect)`. Adding or editing a project style changes the digest, so trust has to be granted again. That matches team definitions and the "Changed definitions" rule. User and built-in styles never depend on trust.

### Selection

- The **effective style name** of a primary Session is the first of:
  - the Session's own selection (`SessionInfo.output_style`, set at creation or by a switch),
  - the resolved config's `output_style` for its Location (any layer; a project layer's value is a plain name and needs no trust),
  - `default`.
- Sessions with a parent (subagents, workflow agents, teammates) always observe `core/output-style` as Absent.
- Resolution happens in the tool host. `ToolHost::output_style(directory, selection) -> ContextObservation` has a default impl that returns `Absent`. `cyber-tools` implements it with the Location's resolved config, its trust and `output_styles::load`. `Inner::observe` inserts the result under `core/output-style`, next to the goal source.
- **Unavailable selection.** Suppose the effective name is unknown, held for trust or invalid, for example a project config that names a style only an untrusted checkout defines. The source is then `Absent` and `default` applies, so a repository cannot block every Turn with a name. The listing route reports `effective` and the reason, and the TUI and `cyber exec` show it. If a listed style's file can't be read (an I/O error), the source is `Unavailable`, which follows the existing rules: a first Turn is blocked, and a later epoch keeps the previous value. A Location configuration that fails to resolve is `Unavailable` too, so a transient bad edit never drops the style or rewrites the prefix.
- **Explicit selections fail fast.** The create body (of `POST /sessions` and of `POST /worktrees`, checked at the main Location before the checkout exists), the switch route and `cyber exec --output-style` refuse a name that is invalid, unknown or held with `400 InvalidRequestError`, and the CLI exits with code 2. The error names the reason, for example `output style "reviewer" is held until this checkout is trusted (cyber trust inspect)`.
- **No authority.** The style source holds text only. It is not read by permission evaluation, Mode admission, the sandbox or tool materialization, and its frontmatter cannot declare any of these. That keeps "Trust SHALL NOT itself grant a tool permission" true for styles.

### Durable state and routes

- `SessionInfo` gains `output_style: Option<String>` (`None` follows the config). `session.created` carries it, defaulting to `None`. The new `session.output_style.switched.1 { from, to }` (both `Option<String>`) updates it, and appends a history marker that is not sent to the model. Switching to the current value is a no-op. Replay and `GET /sessions/{id}` show the field.
- New routes:
  - `POST /api/v1/sessions/:id/output-style` with `{ style: string | null }` returns the Session. A child Session gets `400` (`output styles apply to primary Sessions`).
  - `POST /api/v1/sessions` accepts `output_style` with the same validation.
  - `GET /api/v1/config/output-styles` (Location-scoped) returns:
    - `styles`: `name`, `description`, `origin` (`builtin`, `user`, `project`), `path` and `shadows` for files,
    - `issues`: path and message, including the trust hold,
    - `configured`: the config value and its source layer, or `null`,
    - `effective`: the name that applies to a new Session there.
  - `PUT /api/v1/config/output-style` with `{ style: string | null }` sets `output_style` in the user's global config through `config::edit::set_global_key` (`null` writes `default`). Only built-in and user styles are accepted, because a global default applies in every checkout.
- These live in a new `http/output_styles.rs` (the `config` group, which already exists, so there is no new route group), the Session route included; `http/sessions.rs` only checks `output_style` on create. The OpenAPI document and the TS SDK are regenerated.

### Surfaces

- **Config:** `output_style` is a top-level string. Validation checks only its syntax (`valid_name`), because whether a name is defined depends on the Location.
- **`cyber exec --output-style <name>`:** the CLI checks the name against `GET /config/output-styles` for the Location and passes it in the create body. When there is no flag and the configured style is unavailable, exec prints `warning: output style "<name>" is unavailable (<reason>); using default` to stderr.
- **TUI:**
  - `/output-style` opens a picker overlay; `/output-style <name>` switches directly.
  - The palette has **Output style…**.
  - The picker lists the styles from the listing route with description, origin, `current` and `default`, then the issues. It filters by fuzzy search as the theme switcher does.
  - Enter switches the Session. `d` saves the highlighted built-in or user style as the default through `PUT /config/output-style`, and also switches. Esc does nothing.
  - The Config tab gains an `output style` row showing the Session's style and its source (`session`, `config` or `default`); Enter opens the picker. `output_style` is also listed among the read-only core settings.
  - After a switch, the status footer shows `Output style: <name> (next turn)` for 2 seconds. When the Session's selection is unavailable, the footer shows the reason once.

## Risks / Trade-offs

- **Cache cost of a switch.** A new epoch rewrites the cached prefix. This is accepted: the change is user-initiated, it is rare, and the requirement asks for it. Folding the style into a mid-conversation message would keep the cache, but the instruction would then sit below the conversation and models follow it less.
- **A wrong family guess.** The rules are a short table, and the worst case is the `default` base prompt, which is still a complete prompt. Unit tests pin the table against representative catalog ids (for example `openrouter/anthropic/claude-sonnet`, `copilot/gpt-5`, `ollama/qwen3`, `google/gemini-2.5-pro`, `gpt-oss-120b`).
- **Trust prompts in more repositories.** A repository that adds `.cyber/output-styles/` asks for trust again. This is intended: the files are privileged prompt text written by the repository.
- **A repository naming an unavailable style.** It falls back to `default` and shows a warning. It never blocks a Turn and never loads untrusted text.

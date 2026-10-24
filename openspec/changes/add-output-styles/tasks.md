## 1. Base prompts by model family

- [x] 1.1 `crates/cyber-core/prompts/base/{core,openai,anthropic,gemini,local}.md` and `cyber_core::prompts` with `ModelFamily` and `base_prompt(family)` (`include_str!`); the current texts move there. Unit tests check that each family is the core text plus its addendum and that `Default` is the core text alone.
- [x] 1.2 `cyber_llm::catalog::model_family(provider, model)`: a table of open-weights names, then name prefixes, then the API kind, then a local endpoint, else `Default`. Table tests cover `openrouter/anthropic/claude-sonnet`, `copilot/gpt-5`, `chatgpt/gpt-5-codex`, `google/gemini-2.5-pro`, `ollama/qwen3`, `groq/gpt-oss-120b`, `gemma`, a loopback `auth_none` provider and an unknown hosted model.
- [x] 1.3 `ResolvedModel.family` (set by `CatalogResolver::resolve`; test resolvers use `Default`). `context::base_prompt` takes the family, and its callers in `drain.rs`, `side_chat.rs` and `compaction.rs` pass `resolved.family`. `EpochStarted.family`. `ensure_epoch` starts a new epoch when the family differs, and an epoch without `family` compares by provider only. Regression test: replaying a pre-change epoch (no `family`) keeps its prefix bytes.
- [x] 1.4 Production path: a `cyber-app` AppHost test with a scripted local provider registered under two catalog models (one whose family is `claude`, one whose family is `qwen`) captures the request bodies. It checks the Anthropic and local base prompts in `system[0]`, a new `EpochStarted` after a model switch across families, and an agent's `system` text replacing the base prompt. (`crates/cyber-app/src/host/tests.rs`: `base_prompt_follows_the_model_family`.)

## 2. Output style definitions and trust

- [x] 2.1 `crates/cyber-core/prompts/output-styles/{concise,explanatory,learning}.md` and `cyber_core::output_styles` on the pattern of `teams.rs`:
  - built-ins, user and project directories;
  - `valid_name`, frontmatter (`name` equal to the stem, `description`, `keep-coding-instructions` ignored, any other key invalid), a non-empty body;
  - regular files of at most 32 KiB, no links, at most 128 files;
  - reserved built-in names, project over user with `shadows`;
  - `CYBER_DISABLE_PROJECT_CONFIG`, and the trust hold issue.
  Unit tests cover each rule, including a symlinked file, a `permissions` key and a `../x` name. The read opens without following links and bounds the bytes read (regression tests `the_read_itself_refuses_links_and_special_files`, `the_size_bound_applies_to_the_bytes_read`).
- [x] 2.2 `output_styles::trust_subset` added to `config::load::assess_trust` next to the teams subset. Tests: adding or editing a project style changes the digest and makes a trusted checkout untrusted; user styles never affect the digest.
- [x] 2.3 Config key `output_style` in the top-level key list and `config/validate.rs` (syntax only). The stale `styles/*.md` mention is gone. Test: the "Invalid output style name" scenario.
- [x] 2.4 Production path: the real `cyber` binary in a temporary `HOME` with an untrusted checkout containing `.cyber/output-styles/reviewer.md`. `cyber trust inspect` lists the file, and after `cyber trust approve --digest`, `reviewer` is listed by `GET /api/v1/config/output-styles`. (`crates/cyber-cli/tests/output_styles_cli.rs`: `project_styles_follow_checkout_trust`.)

## 3. Selection, epochs and server routes

- [x] 3.1 `core/output-style` source:
  - `ToolHost::output_style(directory, selection)`, with a default `Absent`, implemented in `cyber-tools/src/host.rs` from the resolved config, its trust and `output_styles::load`;
  - `Inner::observe` inserts it, as `Absent` for Sessions with a parent;
  - `render_baseline` leaves it out, and `start_epoch` appends it to `system_prefix`;
  - `reconcile_context` starts a new epoch instead of sending an update when it changes.
  Unit tests: an unknown, held or invalid configured name gives `Absent`, and an unreadable listed file gives `Unavailable`. A configuration that fails to resolve is `Unavailable`, so the epoch keeps its style (regression test `crates/cyber-tools/tests/output_style_context.rs`).
- [x] 3.2 Durable selection: `SessionInfo.output_style`, `CreateBody.output_style`, and `session.output_style.switched.1 { from, to }`, with a history marker, a no-op when unchanged and replay. Regression test: the "Output style survives a restart" scenario through the store replay.
- [x] 3.3 Routes:
  - `POST /api/v1/sessions/{id}/output-style` (refused for child Sessions and for invalid, unknown or held names);
  - `output_style` on create, including `POST /api/v1/worktrees` (regression test `worktree_sessions_take_the_requested_output_style`);
  - `GET /api/v1/config/output-styles` (already served since 2.4) and `PUT /api/v1/config/output-style` (global, built-in and user styles only, through `config::edit::set_global_key`) in a new `http/output_styles.rs`.
  Then the OpenAPI operations (`UPDATE_OPENAPI=1 … openapi_document_is_current`) and the TS SDK (`scripts/generate_sdk.py`) are regenerated.
- [x] 3.4 Production path: a `cyber-server` HTTP test over a production `cyber-app` host with a scripted local provider. It creates a Session with `output_style: "concise"` and checks the style at the end of `system[0]`, switches to `explanatory` during a running Turn and checks that Turn's prefix is unchanged, then checks a new `EpochStarted` and no `session.context.updated.1` for the style on the next Turn. It also covers `400` for `../../etc/passwd`, a held project style, a child Session, and the "Listing reports a held project style" and "Global default must not name a project style" scenarios. (`crates/cyber-app/tests/output_styles.rs`: `output_style_routes_switch_through_a_new_epoch`, with `output_style_survives_a_restart` for 3.2; it lives in `cyber-app` because `cyber-server`'s tests cannot build a production `cyber-app` host. A running Turn's later steps keep the style they started with; the new epoch starts at the next Turn.)

## 4. CLI and TUI surfaces

- [x] 4.1 `cyber exec --output-style <name>`: checked against the listing before the Session exists, with exit code 2 and the reason. Without the flag, a stderr warning when the configured style is unavailable. `docs/cli.md` is regenerated. Production path: the real binary with a scripted local provider covers the "Output style flag" and "Unknown output style" scenarios. (`crates/cyber-cli/tests/output_styles_cli.rs`: `exec_output_style_flag`.)
- [x] 4.2 TUI picker:
  - `/output-style [name]`, the palette entry **Output style…** and `/help`;
  - origin, `current` and `default` labels, the issues and fuzzy filter;
  - Enter switches, `d` saves the default, Esc has no side effects;
  - the footer notice, the unavailable-style notice once per Session, and refusal in subagent threads.
  Screen tests against a production App: `crates/cyber-tui/src/screens_output_styles.rs` (`picker_switches_the_session_and_shows_next_turn`, `held_project_styles_are_explained`).
- [x] 4.3 Settings Config tab: the `output style` row (value and source, Enter opens the picker) and `output_style` among the core settings. Screen test: `screens_settings::output_style_row_opens_the_picker`.

## 5. Docs and verification

- [x] 5.1 README feature table row, `docs/features.md`, and a `docs/local-core-guide.md` section on base prompts and output styles (file format, precedence, trust, `/output-style`, `--output-style`, `output_style`). ROADMAP P0 status, with `python3 scripts/spec_inventory.py` run again.
- [x] 5.2 `cargo fmt`, `cargo clippy --workspace --all-targets -- -D warnings` (no new `cfg(unix)`/`cfg(windows)` code, or each one reviewed by hand), `cargo nextest run --workspace --profile ci`, `npx -y @fission-ai/openspec@1.13.2 validate add-output-styles --strict`, `python3 scripts/spec_lint.py`.
- [x] 5.3 Acceptance by hand on macOS (2026-10-23) with the debug `cyber`, a real `cyber serve --register` and a local recording stub provider: family base prompts (OpenAI, open-weights), an agent's `system` text replacing the base prompt, `output_style` config, `cyber exec --output-style` and its refusals (unknown, `../../etc/passwd`), the TUI picker and `/output-style learning`, a trusted and an untrusted project `reviewer` style, and mid-Session switches over HTTP and the TUI starting a new Context Epoch that survives a server restart. Open: the `GET /api/v1/sessions` list summary does not report `output_style` (`GET /api/v1/sessions/{id}` does).

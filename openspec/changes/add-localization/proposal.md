## Why

Cyber Code only speaks English. Many of its users work in Portuguese, Spanish or French. They read the interface faster in their own language and want the agent to answer them in it. Teams still need one shared set of commands, keys and configuration, so typed and scripted surfaces must not change with the language.

## What Changes

- New capability `localization`:
  - **Language selection:** the `language` setting (`auto`, `en`, `pt-BR`, `es`, `fr`); `auto` follows `LC_ALL`/`LC_MESSAGES`/`LANG`, and an unsupported value warns and falls back to `auto`.
  - **Interface translation:** the TUI's composer, footer, welcome panel, help, command palette, permission and question prompts and Settings tabs are translated. Slash command names, keys, configuration, the CLI and outside text stay as they are. Catalogs are keyed by the English text and checked by a test for completeness and placeholders.
  - **Reply language:** a set language adds a `core/language` context source asking the model to reply in it while keeping code and commands unchanged. Primary Sessions only.
- `configuration` "Top-level keys": adds `language`.

## Non-goals

- The web dashboard, `cyber --help`/`docs/cli.md`, server error messages and the TUI surfaces beyond those listed stay English for now; they fall back to English and can be added to the catalogs later.
- No locale-aware number or date formatting.
- No right-to-left or other languages in this change.

## Impact

- `crates/cyber-core`: a `locale` module and config validation.
- `crates/cyber-server` and `crates/cyber-tools`: a `ToolHost::language` hook and the `core/language` source.
- `crates/cyber-tui`: an `i18n` module, catalogs in `locales/` and the translated surfaces.
- `crates/cyber-cli`: resolves the language for `cyber` and `cyber attach`.

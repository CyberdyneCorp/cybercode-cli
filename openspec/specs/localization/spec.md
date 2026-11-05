# localization Specification

## Purpose
Localization lets people use Cyber Code in their own language: the TUI interface in English, Brazilian Portuguese, Spanish or French, chosen by the `language` setting or the environment, and, when a language is set, the model's replies in it. Commands, keys, configuration and anything typed or scripted stay in English so they are the same for everyone.

## Requirements

### Requirement: Language selection
(P1) The top-level configuration key `language` SHALL select the interface language and SHALL accept `auto` (the default), `en`, `pt-BR`, `es` and `fr`, ignoring case. With `auto` or no key, the interface language SHALL come from the first non-empty of `LC_ALL`, `LC_MESSAGES` and `LANG`, matched on the language alone: every Portuguese locale (`pt_BR`, `pt_PT`) selects `pt-BR`, every Spanish one `es`, every French one `fr`, and anything else (including `C` and `POSIX`) English. Any other `language` value SHALL be reported as the warning `language: expected auto, en, pt-BR, es or fr, got <value>; using auto` and read as `auto`; it SHALL never stop loading. The TUI SHALL resolve the language from the Location's configuration when it starts, and `cyber attach` from the local configuration, so a change applies from the next start.

#### Scenario: Language from the environment
- **GIVEN** no `language` key and `LANG=pt_BR.UTF-8`
- **WHEN** the user starts the TUI
- **THEN** the interface is in Brazilian Portuguese

#### Scenario: Configuration wins over the environment
- **GIVEN** `"language": "fr"` in `cyber.jsonc` and `LANG=es_ES.UTF-8`
- **WHEN** the user starts the TUI
- **THEN** the interface is in French

#### Scenario: Unsupported value
- **WHEN** `cyber.jsonc` sets `"language": "de"`
- **THEN** loading continues with the warning `language: expected auto, en, pt-BR, es or fr, got "de"; using auto`, and the language comes from the environment

### Requirement: Interface translation
(P1) The TUI SHALL show these surfaces in the interface language: the composer placeholder and its info-line words, the footer's palette hint, the welcome panel and tips, the help screen, the command palette (UI action names, every description and the kind of a server command), the permission and question prompts, and the Settings tab bar. Text with no translation SHALL be shown in English, so a surface not yet translated stays readable.

These SHALL stay the same in every language, so commands, scripts and documentation work for everyone:

- slash command names and their arguments (`/model`, `/goal add`), and the `/`, `@`, `!` and `&` prefixes;
- key names and the letters that pick a choice (`Allow once (y)` keeps `(y)`);
- configuration keys and values, CLI subcommands and flags, `cyber --help` and `docs/cli.md`;
- event names, error kinds and file paths;
- text the TUI shows from elsewhere: model output, tool output, server messages, and the names and descriptions of skills, MCP prompts and custom commands.

Each supported language SHALL have one catalog keyed by the English text. A build-time test SHALL fail when a catalog lacks a translated text, keeps an entry nothing uses, or changes a `{placeholder}`. The web dashboard is not translated yet.

#### Scenario: Translated permission prompt keeps its keys
- **GIVEN** the interface language is `pt-BR`
- **WHEN** a permission request for `bash` is shown
- **THEN** the choices read `Permitir uma vez (y)`, `Permitir sempre… (a)` and `Rejeitar (n)`, and `y` still allows once

#### Scenario: Command names stay English
- **GIVEN** the interface language is `es`
- **WHEN** the user opens the command palette
- **THEN** the entry `/model` keeps its name with a Spanish description, and typing `/model` runs it

### Requirement: Reply language
(P1) When `language` is set to a language rather than `auto`, a primary Session SHALL carry the `core/language` context source: an instruction to write replies, questions, plans and summaries for the user in that language (named in English with its tag, such as `Brazilian Portuguese (pt-BR)`), while keeping code, identifiers, commands, file paths, configuration keys, tool arguments and quoted output as they are, and following the project's conventions for commit messages and code comments. With `auto` the source SHALL be absent, and the model answers in the language it is written to. Sessions with a parent (subagents, workflow agents) and teammates other than the lead write for another agent and SHALL NOT carry it. The source SHALL be observed like any other context source, so a changed `language` reaches a running Session at the next Turn's Safe Boundary as a context update.

#### Scenario: Configured reply language
- **GIVEN** `"language": "pt-BR"` in the Location's `cyber.jsonc`
- **WHEN** the user sends a prompt in a new Session
- **THEN** the request's system text asks for replies in `Brazilian Portuguese (pt-BR)`

#### Scenario: Back to auto
- **WHEN** the user changes `language` to `auto` while the Session is idle and sends another prompt
- **THEN** the next request carries `The core/language context no longer applies.`

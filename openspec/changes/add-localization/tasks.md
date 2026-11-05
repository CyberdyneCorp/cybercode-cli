## 1. Selection

- [x] 1.1 `cyber_core::locale`: supported languages, tag and POSIX parsing, environment precedence, `language` resolution; unit tests.
- [x] 1.2 `language` as a known top-level key; an unsupported value warns and reads as `auto`; config test.

## 2. Reply language

- [x] 2.1 `ToolHost::language` and the `core/language` source for primary Sessions; a runtime test through the real tool host (set, then back to `auto`).

## 3. Interface

- [x] 3.1 `i18n` catalogs keyed by English with `{placeholder}` filling and English fallback; a test that every catalog covers exactly the translated texts with the same placeholders.
- [x] 3.2 Translate the composer, footer, welcome, help, palette, permission and question prompts and Settings tabs; command names, keys and outside text unchanged.
- [x] 3.3 The CLI passes the resolved language to the TUI for `cyber` and `cyber attach`.
- [x] 3.4 Snapshot tests in `pt-BR`, `es` and `fr`.
- [x] 3.5 Docs: configuration reference and a guide section.

## 4. Later

- [ ] 4.1 The remaining TUI surfaces (sidebar, explorer, workflows, agents, memory, rewind, toasts).
- [ ] 4.2 The web dashboard.

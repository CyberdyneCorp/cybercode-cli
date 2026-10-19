## 1. Renderer

- [ ] 1.1 HTML renderer over the existing export document: header, messages, reasoning, tool calls with truncation and unified diffs, children, color scheme, footer.
- [ ] 1.2 Safety: escaping, `pulldown-cmark` with raw HTML dropped, links as text, CSP meta, no scripts or external URLs; tests for the "Injected markup is inert" and "Self-contained HTML" scenarios.
- [ ] 1.3 `--format html` and `--output <file>` (atomic) on `cyber sessions export`; `--sanitize` parity test against JSON.

## 2. TUI

- [ ] 2.1 `/export [html|md|json] [path]` with the slugged default name, overwrite confirmation and the full-conversation notice; palette and `/help`; screen test.

## 3. Docs and verification

- [ ] 3.1 Local core guide and README Sessions row.
- [ ] 3.2 `cargo fmt`, `cargo clippy --workspace -- -D warnings`, touched-crate tests, `openspec validate --strict`.

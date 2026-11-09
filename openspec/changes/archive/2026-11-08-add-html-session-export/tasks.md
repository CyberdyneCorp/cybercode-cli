## 1. Renderer

- [x] 1.1 HTML renderer over the server's export document: header, messages, reasoning, tool calls with truncation, goals, workflow runs, children, color scheme, footer. Per-Turn unified diffs are deferred: the document has none (design → Structure). (`cyber_client::export::html`.)
- [x] 1.2 Safety: escaping, `pulldown-cmark` with raw HTML shown as text, links as text, CSP meta, no scripts or external URLs; tests for the "Injected markup is inert" and "Self-contained HTML" scenarios. (`crates/cyber-cli/tests/export_cli.rs`, `cyber_client::export` unit tests.)
- [x] 1.3 `--format html` and `--output <file>` (atomic, no symlinks, 0600 for a new file) on `cyber sessions export`; `--format html` refused elsewhere; `--sanitize` parity test against JSON. (`export_cli::sessions_export_to_json_and_self_contained_html`, `export_cli::html_format_is_only_for_export`.)

## 2. TUI

- [x] 2.1 `/export [html|md|json] [path]` with the slugged default name, overwrite confirmation and the full-conversation notice; palette and `/help`; screen test. (`screens_export::export_writes_the_default_page_and_asks_before_replacing_it`.)

## 3. Docs and verification

- [x] 3.1 Local core guide, feature reference and README Sessions row.
- [x] 3.2 `cargo fmt`, `cargo clippy --workspace -- -D warnings`, touched-crate tests, `openspec validate --strict`.

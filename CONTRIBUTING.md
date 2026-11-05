# Contributing to Cyber Code

Thanks for your interest. This guide covers how to build the project, what a pull request needs, and how changes are specified.

By contributing, you agree that your contributions are licensed under the [Apache License 2.0](LICENSE), the project's license (section 5 of the license).

## Before you start

- **Bugs and small fixes:** open a pull request directly, or an issue first if you're not sure it's a bug.
- **New features and larger changes:** open an issue to discuss the idea first. Medium and large features, and any change touching security, permissions, authentication, billing or data models, need an [OpenSpec](#specs-with-openspec) change before the code.
- **Security problems:** don't open an issue. Follow [SECURITY.md](SECURITY.md).

## Development setup

You need:

- **Rust 1.89+** (edition 2024). SQLite is bundled.
- **Linux:** `bubblewrap` for the sandbox (`sudo apt install bubblewrap`).
- **Node.js 22** for the web dashboard (`web/`), the TypeScript SDK (`sdk/typescript/`) and the OpenSpec CLI.
- **[just](https://github.com/casey/just)** (optional) for the task shortcuts below.
- **Python 3** for the spec and SDK scripts.

```bash
git clone https://github.com/CyberdyneCorp/cybercode-cli
cd cybercode-cli
cargo build --locked -p cyber-cli
./target/debug/cyber --version
```

Run `just` to list the tasks. The common ones:

| Task | Command |
|---|---|
| Format | `just fmt` |
| Lint (fmt check + clippy, warnings are errors) | `just lint` |
| Rust tests | `just test`, or `just test -p cyber-server` |
| Specs (OpenSpec validation, spec lint, roadmap inventory) | `just specs` |
| TypeScript SDK | `just sdk-test` |
| Everything CI checks | `just ci` |
| Linux tests in Docker | `just test-linux` |

Some tests start the sandbox helper. Build it once with `cargo build -p cyber-sandbox --bins` before running the whole suite. CI uses [nextest](https://nexte.st/): `cargo nextest run --workspace --profile ci`.

## What CI checks

A pull request must pass:

- `cargo fmt --check` and `cargo clippy --workspace --all-targets -- -D warnings`. Clippy also enforces a cognitive-complexity limit per function; split a function that exceeds it rather than silencing the lint.
- The Rust tests on macOS, Linux and Windows. Tests that need Unix (bash, PTYs) are gated with `#[cfg(unix)]`, together with any helpers and imports only they use.
- **Generated files are current:**
  - `sdk/openapi.json`: run `UPDATE_OPENAPI=1 cargo test -p cyber-server openapi_document_is_current` after changing an HTTP route.
  - The TypeScript SDK: `python3 scripts/generate_sdk.py`.
  - `docs/cli.md`: `just docs-cli` after changing CLI flags or help text.
  - The requirement inventory in `ROADMAP.md`: `python3 scripts/spec_inventory.py` after changing specs.
- `openspec validate --all --strict` and `python3 scripts/spec_lint.py`.
- The web dashboard and SDK builds and tests.

## Pull requests

- **One topic per pull request.** Separate refactors from behavior changes when you can.
- **Describe it.** The body says what changed, why, and how you tested it. The template has the sections.
- **Bug fixes come with a regression test** in the same pull request: one that fails before the fix and passes after.
- **Update the docs** (`docs/`, `README.md`) and the specs (`openspec/`) when behavior changes.
- **Keep code simple to read and extend.** Match the naming, comment density and idioms of the code around it.
- **Commit messages** are short and technical: what changed and why, in the imperative (`tui: search prompt history with Ctrl+R`).

## Specs with OpenSpec

Requirements live in [`openspec/specs/`](openspec/specs), one folder per capability, written as `SHALL` statements with scenarios. Changes are proposed in [`openspec/changes/`](openspec/changes):

```bash
npx -y @fission-ai/openspec@1.13.2 new change add-my-feature
# write proposal.md, design.md (when needed), tasks.md and specs/<capability>/spec.md deltas
npx -y @fission-ai/openspec@1.13.2 validate add-my-feature --strict
```

A spec is a product contract, not proof of implementation. [`docs/features.md`](docs/features.md) records what is actually available.

## Clean-room rule

Cyber Code is written independently. Do not copy code, prompts, grammars or documentation text from other coding agents or their products, including when their license would allow it. Studying their public behavior and documentation is fine; write our own implementation and words.

## Code of conduct

Be respectful and constructive. Harassment or personal attacks are not tolerated in issues, pull requests or any project space. Maintainers may remove content or block participants who don't follow this.

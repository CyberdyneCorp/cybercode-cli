# Cyber Code development tasks. Run `just` to list them.

set shell := ["bash", "-euo", "pipefail", "-c"]

cyber := "target/debug/cyber"
linux_image := "cyber-linux-test"

# List the recipes.
default:
    @just --list --unsorted

# --- Build -------------------------------------------------------------------

# Debug build of the whole workspace (binary: target/debug/cyber).
build:
    cargo build --workspace

# Optimized `cyber` binary (target/release/cyber).
release:
    cargo build --release -p cyber-cli

# Install `cyber` into ~/.cargo/bin.
install:
    cargo install --path crates/cyber-cli --locked

# Remove build artifacts.
clean:
    cargo clean

# --- Checks (what CI runs) ---------------------------------------------------

# Format all Rust code.
fmt:
    cargo fmt --all

# Rust formatting and clippy with warnings as errors.
lint:
    cargo fmt --all --check
    cargo clippy --workspace --all-targets -- -D warnings

# Rust tests: `just test`, `just test -p cyber-server`, or `just test <name filter>`.
[positional-arguments]
test *args:
    if printf '%s\n' "$@" | grep -qxE -- '-p|--package'; then cargo test "$@"; else cargo test --workspace "$@"; fi

# OpenSpec validation, cross-spec lint and the ROADMAP requirement inventory.
specs:
    openspec validate --all --strict
    python3 scripts/spec_lint.py
    python3 scripts/spec_inventory.py
    git diff --exit-code ROADMAP.md

# TypeScript SDK: generated sources current, typecheck, tests.
sdk-test:
    python3 scripts/generate_sdk.py --check
    cd sdk/typescript && npm ci --silent && npm run typecheck && npm test

# Everything CI checks.
ci: lint test specs sdk-test

# Rust tests on Linux in Docker (bubblewrap sandbox, unprivileged user). Parallelism is

# capped (LINUX_JOBS, default 4) so linking fits in Docker's memory.
[positional-arguments]
test-linux *args:
    docker build -q -t {{ linux_image }} -f scripts/docker/linux-test.Dockerfile scripts/docker >/dev/null
    scope=--workspace; if printf '%s\n' "$@" | grep -qxE -- '-p|--package'; then scope=; fi; \
    docker run --rm --security-opt seccomp=unconfined --security-opt apparmor=unconfined \
        -v "$PWD":/src:ro -v cyber-target:/target -u root {{ linux_image }} \
        sh -c 'chown -R dev /target && su dev -c "export CARGO_TARGET_DIR=/target CARGO_HOME=/target/cargo-home CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=${LINUX_JOBS:-4} PATH=/usr/local/cargo/bin:\$PATH; cd /src && cargo test --locked $0 $*"' "$scope" "$@"

# --- Generated sources --------------------------------------------------------

# Regenerate sdk/openapi.json from the server's types.
openapi:
    UPDATE_OPENAPI=1 cargo test -p cyber-server --test http openapi_document_is_current

# Regenerate sdk/openapi.json and the TypeScript SDK from it.
sdk-generate: openapi
    python3 scripts/generate_sdk.py

# Refresh the bundled models.dev catalog snapshot.
models-snapshot:
    python3 scripts/refresh_models_snapshot.py

# --- Run ----------------------------------------------------------------------

# The TUI in the current directory (or pass a project path and flags).
[positional-arguments]
tui *args: build
    {{ cyber }} "$@"

# One non-interactive run, e.g. `just exec -m openai/gpt-6-luna "fix the tests"`.
[positional-arguments]
exec *args: build
    {{ cyber }} exec "$@"

# The server in the foreground.
[positional-arguments]
serve *args: build
    {{ cyber }} serve "$@"

# Background server: `just service start|stop|restart|status|password [value]`.
[positional-arguments]
service *args: build
    {{ cyber }} service "${@:-status}"

# One API request, e.g. `just api v1.session.list`.
[positional-arguments]
api *args: build
    {{ cyber }} api "$@"

# Run against a throwaway home so nothing touches your real ~/.local data.
[positional-arguments]
sandboxed *args: build
    CYBER_HOME="$(mktemp -d)" {{ cyber }} "$@"

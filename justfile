# Cyber Code development tasks. Run `just` to list them.

set shell := ["bash", "-euo", "pipefail", "-c"]

# Follows CARGO_TARGET_DIR, so the run recipes find the binary in a shared target dir.
target_dir := env("CARGO_TARGET_DIR", justfile_directory() / "target")
cyber := target_dir / "debug" / "cyber"
linux_image := "cyber-linux-test"
# The OpenSpec CLI version CI installs (.github/workflows/ci.yml), run through npx.
openspec := "npx -y @fission-ai/openspec@1.13.2"

# List the recipes.
default:
    @just --list --unsorted

# --- Setup -------------------------------------------------------------------

# Check the development tools; installs cargo-nextest if it is missing (never uses sudo).
setup:
    #!/usr/bin/env bash
    set -euo pipefail
    missing=0
    have() { command -v "$1" >/dev/null 2>&1; }
    ok() { printf '  ok       %-14s %s\n' "$1" "$2"; }
    miss() { printf '  missing  %-14s %s\n' "$1" "$2"; missing=1; }
    opt() { printf '  optional %-14s %s\n' "$1" "$2"; }
    echo "Cyber Code development tools:"
    msrv=$(sed -n 's/^rust-version = "\(.*\)"/\1/p' Cargo.toml)
    ci_rust=$(grep -m1 -oE 'toolchain: [0-9.]+' .github/workflows/ci.yml | cut -d' ' -f2)
    if have rustup; then ok rustup "$(rustup --version 2>/dev/null | head -1)"; else miss rustup "https://rustup.rs"; fi
    pinned=""
    for f in rust-toolchain.toml rust-toolchain; do
        [ -f "$f" ] && pinned=$(grep -m1 -oE '[0-9]+\.[0-9]+(\.[0-9]+)?|stable|beta|nightly' "$f") && break
    done
    if [ -n "$pinned" ] && have rustup; then
        if rustup toolchain list | grep -q "^$pinned"; then ok toolchain "$pinned (from rust-toolchain)"; else miss toolchain "rustup toolchain install $pinned"; fi
    fi
    if have rustc; then ok rustc "$(rustc --version) (minimum $msrv, CI uses $ci_rust)"; else miss rustc "rustup toolchain install $ci_rust"; fi
    if have cargo-nextest; then
        ok cargo-nextest "$(cargo nextest --version 2>/dev/null | head -1)"
    elif have cargo; then
        echo "  installing cargo-nextest: cargo install cargo-nextest --locked (into ~/.cargo/bin)"
        cargo install cargo-nextest --locked
        ok cargo-nextest "$(cargo nextest --version 2>/dev/null | head -1)"
    else
        miss cargo-nextest "needs cargo first"
    fi
    if have node; then ok node "$(node --version) (CI uses 22)"; else miss node "Node.js 22: web/, sdk/, clients/vscode and OpenSpec"; fi
    if have npm; then ok npm "$(npm --version)"; else miss npm "comes with Node.js"; fi
    if have python3; then ok python3 "$(python3 --version 2>&1)"; else miss python3 "spec, SDK and release scripts"; fi
    if [ "$(uname -s)" = Linux ]; then
        if have bwrap; then ok bubblewrap "$(bwrap --version)"; else miss bubblewrap "sandboxed commands: sudo apt install bubblewrap"; fi
    fi
    if have docker; then ok docker "$(docker --version)"; else opt docker "only for just test-linux / ci-linux"; fi
    if [ "$missing" = 1 ]; then echo "Install the missing tools above, then rerun just setup."; exit 1; fi

# --- Build -------------------------------------------------------------------

# Debug build of the whole workspace (binary: target/debug/cyber).
build:
    cargo build --workspace

# Fast type check of every crate and target, without linking.
check:
    cargo check --workspace --all-targets

# The sandbox helper (cyber-sandbox-exec) that tests and sandboxed runs look for next to the binary.
sandbox-helpers:
    cargo build -p cyber-sandbox --bins

# Optimized `cyber` and its sandbox helper (target/release/).
release:
    cargo build --release -p cyber-cli -p cyber-sandbox --bins

# Install the release `cyber` and its sandbox helper into ${CYBER_INSTALL_DIR:-~/.local/bin}.
install: release
    #!/usr/bin/env bash
    set -euo pipefail
    dir="${CYBER_INSTALL_DIR:-$HOME/.local/bin}"
    mkdir -p "$dir"
    for bin in cyber-sandbox-exec cyber; do
        install -m 755 "{{ target_dir }}/release/$bin" "$dir/$bin"
        echo "installed $dir/$bin"
    done
    case ":$PATH:" in
        *":$dir:"*) ;;
        *) echo "warning: $dir is not on PATH; add it to your shell profile" >&2 ;;
    esac

# Remove the binaries `just install` placed; `cyber uninstall` also removes config and data.
uninstall:
    #!/usr/bin/env bash
    set -euo pipefail
    dir="${CYBER_INSTALL_DIR:-$HOME/.local/bin}"
    for bin in cyber cyber-sandbox-exec; do
        if [ -e "$dir/$bin" ]; then rm -f "$dir/$bin" && echo "removed $dir/$bin"; else echo "not installed: $dir/$bin"; fi
    done
    echo "Configuration, data and keyring entries are kept; 'cyber uninstall --dry-run' lists them."

# Remove build artifacts.
clean:
    cargo clean

# Target dir size per profile.
disk:
    @du -sh "{{ target_dir }}" 2>/dev/null || echo "no target dir at {{ target_dir }}"
    @du -sh "{{ target_dir }}"/*/ 2>/dev/null | sort -rh | head -10 || true

# Delete build artifacts not rebuilt in `days` days (default 14); cargo rebuilds what is needed.
disk-prune days="14":
    #!/usr/bin/env bash
    set -euo pipefail
    dir="{{ target_dir }}"
    [ -d "$dir" ] || { echo "no target dir at $dir"; exit 0; }
    before=$(du -sk "$dir" | cut -f1)
    find "$dir" -mindepth 2 -maxdepth 3 -type d \( -name deps -o -name build -o -name .fingerprint -o -name incremental \) -print0 |
        while IFS= read -r -d '' sub; do
            find "$sub" -mindepth 1 -maxdepth 1 -mtime +{{ days }} -exec rm -rf {} +
        done
    after=$(du -sk "$dir" | cut -f1)
    echo "pruned artifacts older than {{ days }} days: $(( (before - after) / 1024 )) MiB freed, $(( after / 1024 )) MiB left in $dir"

# Run any recipe with the low-disk build env, e.g. `just lean test -p cyber-cli`.
[positional-arguments]
lean *args:
    @CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 "{{ just_executable() }}" --justfile "{{ justfile() }}" "$@"

# --- Checks (what CI runs) ---------------------------------------------------

# Format all Rust code.
fmt:
    cargo fmt --all

# Rust formatting and clippy with warnings as errors.
lint:
    cargo fmt --all --check
    cargo clippy --workspace --all-targets -- -D warnings

# nextest uses the `ci` profile in .config/nextest.toml; NEXTEST=0 forces `cargo test`.
# Rust tests (nextest when installed): `just test`, `just test -p cyber-server`, or `just test <filter>`.
[positional-arguments]
test *args: sandbox-helpers
    scope=--workspace; if printf '%s\n' "$@" | grep -qxE -- '-p|--package'; then scope=; fi; \
    if [ "${NEXTEST:-1}" != 0 ] && cargo nextest --version >/dev/null 2>&1; then \
        cargo nextest run $scope --profile ci "$@"; \
    else \
        cargo test $scope "$@"; \
    fi

# Unit tests only (lib and bin targets), skipping the slower integration-test binaries.
[positional-arguments]
test-quick *args: sandbox-helpers
    scope=--workspace; if printf '%s\n' "$@" | grep -qxE -- '-p|--package'; then scope=; fi; \
    if [ "${NEXTEST:-1}" != 0 ] && cargo nextest --version >/dev/null 2>&1; then \
        cargo nextest run $scope --lib --bins "$@"; \
    else \
        cargo test $scope --lib --bins "$@"; \
    fi

# Doctests (nextest does not run them; CI runs them separately).
test-doc:
    cargo test --workspace --doc

# OpenSpec validation (pinned to CI's version), cross-spec lint and the ROADMAP requirement inventory.
specs:
    {{ openspec }} validate --all --strict
    python3 scripts/spec_lint.py
    python3 scripts/spec_inventory.py
    git diff --exit-code ROADMAP.md

# The non-spec checks of CI's OpenSpec job: eval graders, release-script tests, install.sh syntax.
scripts-test:
    python3 scripts/check_eval_graders.py --jobs 4
    python3 -m unittest discover -s scripts/tests
    sh -n scripts/install.sh
    if command -v shellcheck >/dev/null; then shellcheck -s sh scripts/install.sh; else echo "shellcheck not installed; skipped"; fi

# Regenerate docs/cli.md from the clap definition (a cyber-cli test fails when it is stale).
docs-cli:
    cargo run -q -p cyber-cli -- help-markdown > docs/cli.md

# Fail when docs/cli.md differs from `cyber help-markdown`.
docs-cli-check:
    cargo run -q -p cyber-cli -- help-markdown | diff -u docs/cli.md - || { echo "docs/cli.md is stale; run just docs-cli" >&2; exit 1; }

# TypeScript SDK and plugin kit: generated sources current, typecheck, tests (end-to-end against the built binary).
sdk-test: sandbox-helpers
    python3 scripts/generate_sdk.py --check
    cargo build -p cyber-cli --bin cyber
    cd sdk/typescript && npm ci --silent && npm run typecheck && CYBER_BIN="{{ cyber }}" npm test
    cd sdk/plugin && npm ci --silent && npm run typecheck && npm run build && CYBER_BIN="{{ cyber }}" npm test

# Web dashboard: typecheck, committed bundle current, unit tests (as CI; browser e2e not included).
web-test:
    cd web && npm ci --silent && npm run typecheck && npm run typecheck:e2e && npm run check:build && npm test

# Rebuild the committed web dashboard bundle (web/dist).
web-build:
    cd web && npm ci --silent && npm run build

# VS Code extension: typecheck, lint, unit tests (as CI; the Electron integration run is not included).
vscode-test:
    cd clients/vscode && npm ci --silent && npm run typecheck && npm run lint && npm test

# Build the VS Code extension package (clients/vscode/cyber-<version>.vsix).
vscode-package:
    cd clients/vscode && npm ci --silent && npm run package

# Everything CI checks except platform builds, browser/Electron runs and the benchmark.
ci: lint test test-doc specs scripts-test docs-cli-check sdk-test web-test vscode-test

# Check that tag v<version> matches the workspace (and extension) version before pushing it.
[positional-arguments]
release-check version:
    python3 scripts/release_assets.py check-tag "v${1#v}"

# Parallelism is capped (LINUX_JOBS, default 4) so linking fits in Docker's memory.
# Rust tests on Linux in Docker (bubblewrap sandbox, unprivileged user).
[positional-arguments]
test-linux *args:
    docker build -q -t {{ linux_image }} -f scripts/docker/linux-test.Dockerfile scripts/docker >/dev/null
    scope=--workspace; if printf '%s\n' "$@" | grep -qxE -- '-p|--package'; then scope=; fi; \
    docker run --rm --security-opt seccomp=unconfined --security-opt apparmor=unconfined \
        -v "$PWD":/src:ro -v cyber-target:/target -u root {{ linux_image }} \
        sh -c 'chown -R dev /target && su dev -c "export CARGO_TARGET_DIR=/target CARGO_HOME=/target/cargo-home CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=${LINUX_JOBS:-4} PATH=/usr/local/cargo/bin:\$PATH; cd /src && cargo test --locked $0 $*"' "$scope" "$@"

# LINUX_CPUS (default 2) and LINUX_RUNS (default 3) set the CPU cap and the repetitions;
# `just ci-linux -p cyber-tools` narrows the run.
# Linux tests under CI-runner conditions, repeated to surface timing-sensitive tests.
[positional-arguments]
ci-linux *args:
    docker build -q -t {{ linux_image }} -f scripts/docker/linux-test.Dockerfile scripts/docker >/dev/null
    scope=--workspace; if printf '%s\n' "$@" | grep -qxE -- '-p|--package'; then scope=; fi; \
    for run in $(seq 1 ${LINUX_RUNS:-3}); do \
        echo "== Linux run $run (${LINUX_CPUS:-2} CPUs)"; \
        docker run --rm --cpus ${LINUX_CPUS:-2} --security-opt seccomp=unconfined --security-opt apparmor=unconfined \
            -v "$PWD":/src:ro -v cyber-target:/target -u root {{ linux_image }} \
            sh -c 'chown -R dev /target && su dev -c "export CARGO_TARGET_DIR=/target CARGO_HOME=/target/cargo-home CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=${LINUX_JOBS:-4} PATH=/usr/local/cargo/bin:\$PATH; cd /src && cargo test --locked --no-fail-fast $0 $*"' "$scope" "$@" || exit 1; \
    done

# --- Evaluation -----------------------------------------------------------------

# Check every suite grader: fails on the fixture, passes with the reference solution.
eval-check:
    python3 scripts/check_eval_graders.py

# Run the coding suite against a model, e.g. `just eval openai/gpt-6-luna --trials 3`.
[positional-arguments]
eval model *args: build
    {{ cyber }} eval run eval/manifests/suite-coding-v1.json -m "$1" --output "eval/results/$(date +%Y%m%d-%H%M%S)-${1//\//-}" "${@:2}"

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

# Check this machine's setup with the debug build.
[positional-arguments]
doctor *args: build
    {{ cyber }} doctor "$@"

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

# --- Docs ---------------------------------------------------------------------

# PY selects the interpreter, e.g. `PY=/tmp/cyber-shots/bin/python just screenshots`.
# Recapture docs/screenshots from the real TUI (needs pyte and pillow, and Chrome).
screenshots:
    cargo build -p cyber-cli
    "${PY:-python3}" scripts/capture_tui_screens.py --binary {{ cyber }}

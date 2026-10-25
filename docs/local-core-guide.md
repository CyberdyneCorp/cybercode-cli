# Local core guide

Detailed commands, client behavior, limits and terminology for the implemented local core and partial P1 features. Start with the [README](../README.md) or [documentation index](README.md); the [P1 status](implementation/p1-status.md) records acceptance evidence and remaining work.

## Build and development

Common tasks are in the [`justfile`](../justfile) (install [`just`](https://github.com/casey/just)); run `just` to list them:

```bash
just build          # debug build: target/debug/cyber
just ci             # local checks: lint, tests, specs, SDK (platform builds run in CI)
just test -p cyber-server        # one crate, or `just test <name filter>`
just test-linux     # the test suite on Linux in Docker (bubblewrap sandbox)
just tui            # the TUI here;  just exec "fix the failing test"
just sandboxed exec --ephemeral "hello"   # run with a throwaway CYBER_HOME
just sdk-generate   # regenerate sdk/openapi.json and the TypeScript SDK
```

The raw commands:

Requires Rust 1.89 or newer (edition 2024). SQLite is bundled.

```bash
cargo build                 # target/debug/cyber
cargo test --workspace      # unit, integration, crash-recovery and CLI tests
cargo clippy --workspace --all-targets -- -D warnings

cargo run -p cyber-cli --                   # the TUI in the current directory (starts the background server)
cargo run -p cyber-cli -- exec "fix the failing test"            # one non-interactive run
cargo run -p cyber-cli -- exec --format stream-json --ephemeral "explain this repo"
cargo run -p cyber-cli -- --worktree fix-login                   # TUI in a managed checkout
cargo run -p cyber-cli -- exec --worktree fix-login "fix login"  # reuse checkout, new Session
cargo run -p cyber-cli -- exec --worktree -- "fix login"         # generated worktree name
cargo run -p cyber-cli -- service status   # the background server; also start|stop|restart|password
cargo run -p cyber-cli -- api v1.session.list                    # any API operation
cargo run -p cyber-cli -- doctor           # config, keys, catalog, database, sandbox, tools, server
cargo run -p cyber-cli -- db backup ~/cyber-backup --artifacts   # online backup bundle with a manifest
cargo run -p cyber-cli -- debug info       # build, paths, database, config layers, trust
cargo run -p cyber-cli -- trust inspect    # repository-controlled definitions awaiting approval
cargo run -p cyber-cli -- hooks list       # resolved hooks, file origins, digests and handler trust
cargo run -p cyber-cli -- models openai    # catalog: available models first

# one real turn with a tool call (uses OPENAI_API_KEY)
cargo run -p cyber-llm --example turn -- openai/gpt-6-luna "What time is it in UTC?"
CYBER_LIVE_TESTS=1 cargo test -p cyber-llm --test live   # live provider checks

# a durable session with a tool loop, title and cost (uses OPENAI_API_KEY)
cargo run -p cyber-server --example session -- openai/gpt-6-luna

# a coding task with the built-in tools, the OS sandbox and snapshots (uses OPENAI_API_KEY)
cargo run -p cyber-tools --example agent -- openai/gpt-6-luna path/to/repo "Fix the failing test" accept-edits
```

## Signing in with a ChatGPT subscription

A ChatGPT Plus or Pro plan can pay for OpenAI models instead of an API key:

```bash
cyber providers login chatgpt                 # prints a URL; the browser returns to localhost:1455
cyber providers login chatgpt --method code   # over SSH: paste the URL the browser ends on
cyber models chatgpt                          # the models your plan serves, unpriced
cyber exec -m chatgpt/gpt-6-sol "Fix the failing test"
cyber providers logout chatgpt
```

In the TUI, `/login openai` (or `/login chatgpt`) does the same from a dialog:

- The server runs the sign-in and shows the URL to open. Ctrl+Y copies it.
- When the browser returns to `localhost:1455`, the dialog closes and the model picker opens.
- If the server cannot receive that redirect (the port is busy, or the server runs on another machine), paste the URL the browser ends on and press Enter.
- Esc cancels.
- `/logout openai` signs out.

The server reloads its model catalog when you sign in or out, so `chatgpt/*` models appear or disappear without a restart. Clients can drive the same flow through `POST /api/v1/providers/chatgpt/login`, then poll `GET …/login/{attempt_id}` or post the pasted URL to `…/login/{attempt_id}/code`. Google (Gemini CLI / Code Assist) subscription logins are not supported, because third-party use has led to account suspensions. Gemini models also need the planned native Gemini adapter.

The sign-in uses OpenAI's OAuth flow with the Codex public client, which is what OpenCode and other harnesses use. Requests go to the Codex backend (`https://chatgpt.com/backend-api/codex`), count against your plan's limits, and send `originator: cyber`. The login is kept in `~/.local/share/cyber/chatgpt-auth.json` (mode 0600; a file other users can read is refused). It is separate from Codex's `~/.codex/auth.json`. The access token is renewed 5 minutes before it expires, under a lock shared by every Cyber process.

- The `chatgpt` provider appears only while you are signed in.
- Its models are the ones the backend lists for your account. The list is fetched at login, kept in `chatgpt-models.json` beside the login, and refreshed once a day. Change them under `providers.chatgpt` in `cyber.jsonc`. If the list is empty, run the login again.
- `localhost:1455` is the port Codex uses too. If another login holds it, finish that login or use `--method code`.
- Windows is not supported yet, because the login file cannot be made owner-only there.
- OpenAI's acceptance of third-party harnesses is a public statement, not a contract. The `openai` provider with `OPENAI_API_KEY` is not affected by this login.

## Signing in with a GitHub Copilot subscription

A Copilot Pro, Pro+, Business or Enterprise plan can pay for the OpenAI, Anthropic and Google models it serves:

```bash
cyber providers login copilot                             # prints a URL and a code to enter there
cyber providers login copilot --enterprise octo.ghe.com   # a GitHub Enterprise (data residency) host
cyber models copilot                                      # the models your plan serves, unpriced
cyber exec -m copilot/claude-sonnet-4.5 "Fix the failing test"
cyber exec -m copilot/gpt-5.6-sol "Fix the failing test"  # a model Copilot serves over Responses
cyber providers logout copilot
```

The sign-in is GitHub's OAuth device flow. The CLI prints `Open <url> and enter <code>` and never opens a browser, so it also works over SSH. It then polls GitHub until you approve, at the interval GitHub asks for. Ctrl+C, a denial or an expired code (at most 15 minutes) store nothing. Before anything is stored, the login mints a Copilot token to prove the account has a plan: an account without one fails with `This GitHub account has no active Copilot subscription`.

- The GitHub token is kept in `~/.local/share/cyber/copilot-auth.json` (mode 0600; a file other users can read is refused), with the host and the user name. Sandboxed commands run as you, so the mode alone would not keep them out: every sandboxed launch hides this file, like `chatgpt-auth.json` and cyber's other credential files. Logout deletes the file and the cached model list; it does not revoke the token on GitHub.
- Each request uses a short-lived Copilot token minted from the GitHub token. It is kept in memory only, never written to disk or to a transcript, and renewed 5 minutes before it expires. Requests go to the API address that token names. If GitHub rejects the GitHub token, requests fail with `GitHub Copilot login expired or was revoked; run cyber providers login copilot`, before any model call.
- `--enterprise` takes a bare hostname such as `octo.ghe.com`; a URL or path is a usage error. The host is stored with the login, and the device flow, token mint and model list use `https://<host>` and `https://api.<host>`.
- The `copilot` provider appears only while you are signed in. Its models are the account's picker-enabled models; models an organization policy disables are left out. The list is fetched at login, kept in `copilot-models.json` beside the login, and refreshed once a day. Each model uses the protocol Copilot lists for it: Responses when it supports `/responses`, Chat Completions otherwise. Models are unpriced: premium-request multipliers are not shown as cost.
- Premium requests: the first model call after each prompt (an MCP server prompt you run as a slash command, or a `/loop` iteration) is sent with `X-Initiator: user`. Everything else is sent as `agent`: tool-loop steps, prompt hook checks, `webfetch` summaries, MCP sampling, subagents, Turns opened by background Jobs, monitors or messages, compaction, titles, recaps and the auto-mode reviewer. So one prompt counts as one premium request, as in Copilot's own agent mode.
- The sign-in uses one built-in GitHub OAuth client ID (`Iv1.b507a08c87ecfe98`, provisional: whether GitHub accepts a Cyber-registered client is still open); `CYBER_COPILOT_CLIENT_ID` overrides it. `CYBER_COPILOT_GITHUB_URL` and `CYBER_COPILOT_API_URL` replace the GitHub and API addresses for tests and proxies; both must be HTTPS, or plain HTTP on loopback.
- Windows is not supported yet, because the login file cannot be made owner-only there.
- Not yet checked against a live Copilot account. The device flow, token mint, model list and both protocols are tested against a local fake GitHub and Copilot API only.

## Background server and rebuilds

The TUI, `cyber exec`, `cyber api`, `cyber mcp serve` and the resource commands all share one background server per user, started the same way as `cyber service start`. `~/.local/state/cyber/server.json` (mode 0600) names it with its version, build identity and executable. Each binary carries a build identity: the commit, `.dirty` when tracked files differed, `CYBER_BUILD_ID` when set, or the build time (`SOURCE_DATE_EPOCH` for reproducible builds). It also stamps its executable (path, length and modification time) when it starts.

When you rebuild or upgrade `cyber`, the next client sees that the registered server runs another build:

- If the server runs this install's executable and `GET /api/v1/service` reports no running Turn or workflow run, the client stops it, starts the new build, and prints `restarted the Cyber server: it was running an older build`.
- If the server is busy, belongs to another install, or is too old to report its work, the client keeps using it and prints `The Cyber server runs an older build; restart it with `cyber service restart` when idle`. The TUI sidebar then shows `⚠ older server build · cyber service restart`, panels that need newer routes (such as the Machine section) show `needs a newer server · cyber service restart`, and the Settings › Status tab marks the server version `(older build)`.

`cyber service status` reports `stale: true` in that case. A server started before build identities existed is always kept with the warning. One `cyber service restart` moves you to the build that detects later rebuilds by itself.

## Install, upgrade and uninstall

### Install script

Each GitHub Release carries `install.sh`, a POSIX `sh` script (the latest stable one is `https://github.com/CyberdyneCorp/cybercode-cli/releases/latest/download/install.sh`):

```bash
curl -fsSL https://github.com/CyberdyneCorp/cybercode-cli/releases/latest/download/install.sh | sh
sh install.sh [--version <v>] [--dir <path>] [--channel stable|beta|nightly] [--no-modify-path]
```

- It detects the OS and CPU (`uname`) and, on Linux, musl (`/etc/alpine-release`, or `ldd --version` naming musl), then picks `cyber-<v>-<target>.tar.gz`. Windows uses the `.zip` from the release page.
- Without `--version` it reads `channel-<channel>/<channel>.json`. A `--version` that is not semver or a `--channel` other than the three names exits 2, as does any unknown argument or a `--dir` holding `"`, `$`, `` ` ``, `\` or a newline.
- When `<dir>/cyber --version` (or, without `--dir`, the `cyber` on `PATH`) already reports that version, it prints `cyber <v> already installed` and exits 0 without downloading.
- It downloads `SHA256SUMS` and the archive with `curl` (or `wget`) into a temporary directory and checks the archive with `sha256sum` or `shasum -a 256`. A mismatch prints `checksum verification failed` and exits 1 before anything is written to the install directory. With `cosign` on `PATH` it also verifies `SHA256SUMS.sigstore.json` for the release workflow's identity at `refs/tags/v<v>`; without it, it says it checked the SHA-256 only.
- It extracts `cyber` and `cyber-sandbox-exec` into a hidden directory inside the install directory (default `$CYBER_INSTALL_DIR`, else `~/.cyber/bin`) and renames each into place, the helper first, then writes the receipt `.cyber-receipt` beside them. The receipt is how `cyber upgrade` and `cyber uninstall` know a `--dir` install is a script install, since the PATH line does not set `CYBER_INSTALL_DIR`.
- Unless `--no-modify-path`, it appends `# cyber installer: PATH` and `export PATH="<dir>:$PATH"` (`fish_add_path` for fish) to the first existing rc file for `$SHELL`: `~/.zshrc`; `~/.bashrc`, `~/.bash_profile`, `~/.profile`; fish's `config.fish`; else `~/.profile`. A file that already has the marker is left alone, and `cyber uninstall` removes exactly those two lines.
- `CYBER_INSTALL_BASE_URL` replaces the release base URL, for a mirror or an offline install. The checksums then come from that mirror too, so setting it means trusting it; the `cosign` identity does not change.

Other channels: the release archives themselves, `cargo install --locked --git https://github.com/CyberdyneCorp/cybercode-cli --tag v<version> cyber-cli`, and the repository `Dockerfile` (below). Homebrew (`cyber-code/tap/cyber`), npm (`@cyber-code/cli` with per-platform packages, each holding its target's binaries from the release archive), Scoop, WinGet, the AUR, Nix and a published container image are partial: their templates live under `packaging/` and every release attaches them rendered (`cyber.rb`, `cyber.scoop.json`, `cyber.winget.yaml`, `PKGBUILD`, `cyber.nix`, `cyber-npm-<v>.tar.gz`), but nothing is published to those registries yet.

### Release packaging

Pushing the tag `v<workspace version>` runs `.github/workflows/release.yml`; pull requests, branch pushes and manual runs never do.

1. `verify` runs `scripts/tests` and checks the tag against the workspace version. The channel comes from the version: `-beta.N` is beta, `-nightly.YYYYMMDD` is nightly, no prerelease is stable, and anything else fails.
2. `build` compiles 8 targets with `--locked --release`, `CYBER_CHANNEL` and `CYBER_GIT_SHA`: `aarch64`/`x86_64` Apple, Linux glibc and musl, and Windows MSVC (`windows-2025`, `windows-11-arm`). It checks `cyber --version` and runs `scripts/release_assets.py package`, which puts `cyber`, `cyber-sandbox-exec`, `README.md` (and `PREVIEW.txt` on Windows) at the archive root.
3. `publish`, the only job with `contents: write` and `id-token: write`, runs once every build has passed. It writes `SHA256SUMS`, signs every archive and `SHA256SUMS` with `cosign sign-blob` (keyless, `<file>.sigstore.json`), renders `<channel>.json` and the `packaging/` templates, checks the asset list, creates the GitHub Release `v<version>` (a prerelease unless stable), and replaces `<channel>.json` on the rolling release `channel-<channel>` only when the version is newer by semver than the one it names. A backport tag (`v1.3.5` after `v1.4.0`) is released without becoming the latest or rolling the channel back, and releases run one at a time across tags. The repository variable `CYBER_MIN_SUPPORTED`, when set, becomes the manifest's `min_supported`.

Actions are pinned by commit SHA, and no build cache is used. `scripts/release_assets.py` does the packaging, checksums, manifest and rendering with the Python standard library, so its tests (`python3 -m unittest discover -s scripts/tests`) run anywhere.

### Release source and `cyber upgrade`

Releases come from one release source embedded at build time, shown under `release` by `cyber debug info`:

- Base URL `https://github.com/CyberdyneCorp/cybercode-cli/releases/download`. `v<version>/` holds `cyber-<version>-<target>.tar.gz` (`.zip` on Windows) and `SHA256SUMS`; `channel-<channel>/<channel>.json` holds the channel manifest (`version`, `published_at`, `notes_url`, `min_supported`).
- Signing identity `https://github.com/CyberdyneCorp/cybercode-cli/.github/workflows/release.yml@refs/tags/v<version>`, issuer `https://token.actions.githubusercontent.com`.
- Only the build variables `CYBER_RELEASE_BASE_URL`, `CYBER_RELEASE_IDENTITY` (the prefix before `v<version>`) and `CYBER_RELEASE_ISSUER` change them. No config key or runtime variable of a release build does.

`cyber upgrade [VERSION] [--method <m>] [--channel stable|beta|nightly] [--dry-run]` reads the latest version from the channel manifest (a `dev` build reads `stable`). The lookup times out after 10 seconds, retries twice, and caches the answer for an hour in `<cache>/release-<channel>.json`. With `CYBER_OFFLINE` set it reads only the cache, of any age, and fails with `offline: no cached release information` without one. The install method is `script` when the binary is under `$CYBER_INSTALL_DIR` or `~/.cyber/bin`, or the install script's receipt `.cyber-receipt` sits beside it (so a `--dir` install is still `script`), otherwise it comes from the path (Homebrew, npm, pnpm, bun, Cargo, Scoop, WinGet, Nix, Docker) or from the package managers' own listings, each run without a shell and stopped after 3 seconds. `--format json` prints `{from, to, method, action}`, where `action` is `current`, `replaced`, `manager` or `manual`. `--dry-run` reads the manifest, prints the target, method and the action it would take (`"dry_run": true` in JSON), and downloads, runs and changes nothing. When the target is the running version it prints `cyber <v> is already installed` and changes nothing. A manifest version that is not newer than the running one (an older channel, a pre-release build reading `stable`) changes nothing either; only `cyber upgrade <older version>` downgrades. With `CYBER_OFFLINE` set, an upgrade that would download or run a package manager fails with `offline: installing cyber <v> needs the network` before any request.

What happens next depends on the method:

- `script`: cyber downloads `v<version>/SHA256SUMS` and `SHA256SUMS.sigstore.json` and verifies the Sigstore bundle against the trusted root embedded in the binary, for the identity `<prefix>v<version>` and the issuer, with the certificate chain, the SCT and the transparency-log inclusion all required. A signature for another version does not match. It then streams the archive (at most 512 MiB) to a hidden file beside the binary and checks its SHA-256 against its `SHA256SUMS` line. Only `cyber` and `cyber-sandbox-exec` at the archive root are extracted; an archive with an absolute or `..` member, a link or a device is refused. The helper is replaced first, then `cyber`: the new file takes the old one's mode and is synced, the old one is renamed to `cyber.old` (`cyber.old.exe` on Windows, where the running executable can be renamed but not overwritten), and the new one is renamed into place, putting the old one back if that fails. The next start of cyber removes `cyber.old`. Any failure prints `signature verification failed`, `checksum verification failed` or `download failed` and leaves the installed binary unchanged.
- `brew`, `npm`, `pnpm`, `bun`, `cargo`, `scoop`, `winget`: cyber runs `brew upgrade cyber`, `npm i -g @cyber-code/cli@<v>` (`pnpm add -g`, `bun add -g`), `cargo install --locked --git https://github.com/CyberdyneCorp/cybercode-cli --tag v<v> cyber-cli`, `scoop update cyber` or `winget upgrade cyber-code.cyber`, resolved on `PATH` and run without a shell. When it fails, cyber prints the command and exits 1. When only a manager's listing named the method (the binary's path is not that manager's) and `--method` is not given, cyber prints `<m> lists a cyber package, but this binary is not from it; to upgrade that package, run: <command> (or "cyber upgrade --method <m>" to run it)` instead, with action `manual`, and runs nothing.
- `nix`, `aur`, `docker`, `unknown`: cyber prints `installed via <m>; run: <command>` (for example `nix profile upgrade cyber-code`) and exits 0.

`autoupdate` in `cyber.jsonc` takes `true`, `false` or `"notify"` (the default); any other value is a configuration error.

### Background updates and the minimum supported version

The running server checks the channel manifest 5 seconds after it starts and then every 24 hours, through the same cached lookup (User-Agent client `server`). It does not check when `autoupdate` is `false`, `CYBER_DISABLE_AUTOUPDATE`, `CYBER_OFFLINE` or `CI` is set, or the build's channel is `dev`. Organization policy can set `policy.autoupdate` (`true`, `false` or `"notify"`) in the managed policy file; it wins over every user and project layer, a user-written `policy.autoupdate` does not count, and a malformed policy turns the check off. The setting is re-read at every check.

- A newer version is announced with the live-only event `installation.update_available.1` (`{current, latest, kind, notes_url}`, `kind` being `major`, `minor` or `patch`), on every instance stream whatever its Location.
- With `autoupdate: true`, a patch release on a `script`, `brew` or `npm` install that the binary's path shows (a method from a manager's listing alone is only announced) is installed without asking (the verified self-update of `cyber upgrade`, or the manager's command) and announced with `installation.updated.1`. If that fails, the release is announced as available instead. A version already installed this way is not downloaded again, and once the server has upgraded itself it only announces later releases until it restarts. A configuration that does not load skips the check.
- `GET /api/v1/service/update` returns the last result: `{current, channel, method, latest, kind, notes_url, min_supported, unsupported, updated, checked_at_ms}`, with `latest: null` before the first check.

The server never restarts itself; the stale-build rule replaces it once it is idle. The TUI shows `cyber X.Y.Z is available — run "cyber upgrade"` or `cyber updated to X.Y.Z; restart cyber to use it` as a toast, from the event or, when it attaches after the check, from the endpoint, once per notice; a running Turn is never interrupted.

When the cached manifest's `min_supported` is newer than the running version, `cyber exec` prints `warning: cyber <v> is no longer supported (minimum <m>); run "cyber upgrade"` on stderr before the run, and the TUI shows it as a banner above the transcript. Both read the cache only (the server reads it at start), never the network.

### Container and CI

The repository `Dockerfile` builds an image from prebuilt static binaries, so it needs no Rust toolchain: `debian:bookworm-slim` pinned by digest, with `git`, `ripgrep`, `ca-certificates` and `bubblewrap`, a `cyber` user (uid and gid 10001), `CYBER_HOME=/home/cyber/.cyber`, `CI=true`, `WORKDIR /work` and `ENTRYPOINT ["cyber"]`. Put the musl binaries under `dist/<arch>/` (Docker's `amd64` or `arm64`) first:

```bash
mkdir -p dist/amd64 && tar -xzf cyber-<v>-x86_64-unknown-linux-musl.tar.gz -C dist/amd64
docker build -t cyber .
docker run --rm -v "$PWD:/work" -e OPENAI_API_KEY cyber exec "fix the failing test"
```

No image is published yet (`ghcr.io/cyber-code/cyber` is a partial channel).

With `CI` true (`1`, `true`, `yes` or `on`): `cyber` with no subcommand and `cyber attach` print `interactive TUI unavailable in CI; use "cyber exec"` and exit 2, the background update check is off, the TUI's first-run import and plugin offers are not shown, and `cyber exec` keeps its default `--format text`.

### `cyber uninstall`

`cyber uninstall [--keep-config] [--keep-data] [--dry-run] [--force]` lists what it removes, with sizes:

- for a `script` install, `cyber`, `cyber-sandbox-exec`, their `.old` files and the install script's `.cyber-receipt` in the install directory;
- each PATH line the install script added: the `# cyber installer: PATH` marker and the line after it, in `~/.zshrc`, `~/.bashrc`, `~/.bash_profile`, `~/.profile` or fish's `config.fish`;
- the config, data, state and cache directories. `--keep-config` and `--keep-data` keep those two.

Sizes are counted without following symlinks. `--dry-run` only prints the list (`--format json` gives each entry's `kind`, `path`, `size` and `removed`), and creates nothing. A package-manager install also lists `package manager: <command>`. Otherwise cyber asks `Remove these N items (X MiB)? [y/N]` (`… and run <command>?` for a package manager); without a terminal it exits 2 unless `--force` is given.

On confirmation it:

1. stops the background server, and exits 1 if that fails, unless `--force`;
2. deletes the keyring entries cyber's own records name, unless `--keep-data`: MCP OAuth credentials listed in `mcp-auth-index.json`, the tokens of the peers in `peers.remotes`, and the secret settings of the resolved plugins. Provider logins are files in the data directory. A keyring failure is printed as a warning, and the command exits 1 at the end;
3. runs the package manager's uninstall for `brew`, `npm`, `pnpm`, `bun`, `cargo`, `scoop` and `winget` installs (for example `brew uninstall cyber`), printing the command if it fails. It runs only when the binary's path shows that manager installed it; when only the manager's listing names a cyber package, the command is printed as a note to run by hand;
4. removes the listed entries, then the emptied install directory and `CYBER_HOME`.

Each directory is checked before the listing and again before it is removed. A directory that is not an absolute path without `.` or `..`, is a symlink, is the home directory or one of its ancestors, is a filesystem or drive root, or lies fewer than two levels below the home directory or the root is refused: the command exits 1 and removes nothing. Symlinks inside a cyber directory are unlinked, never followed. An rc file is rewritten atomically and keeps its permissions; a symlinked rc file is left alone with a note to edit it by hand. On Windows the running `cyber.exe` is renamed to `cyber.old.exe`, and the command says to delete it after cyber exits.

### Testing against a local release source

Debug builds (the `target/debug/cyber` that tests run) accept a test hook that release builds compile out:

- `CYBER_TEST_RELEASE_BASE_URL=http://127.0.0.1:<port>` replaces the base URL, so lookups and downloads go to a local stub server.
- `CYBER_TEST_RELEASE_TRUST=<dir>` replaces the signing identity with `<dir>/identity.json` (`{"identity_prefix": …, "issuer": …}`) and the trusted root with `<dir>/trusted_root.json`. A missing or invalid `identity.json` is an error. Under a test root the transparency-log inclusion proof and the SCT are not checked, since a throwaway root has neither log; the chain, identity, issuer, signed entry timestamp and signature still are.

The `test-support` feature of `cyber-install` (`cyber_install::test_support::TestSigner`) issues that throwaway material: a CA, a log key, and cosign-shaped bundles whose leaf certificate names the identity and issuer. `crates/cyber-cli/tests/upgrade_cli.rs` runs the real binary this way against `crates/cyber-install/tests/support/stub_release.rs`. `crates/cyber-app/tests/update_check.rs` points the server's background check at the same stub through `ServeOptions::update` (`UpdateCheckOptions`: version, channel, environment, binary and cadence).

`crates/cyber-cli/tests/install_script.rs` (Unix) runs the real `scripts/install.sh` under `sh` against the same stub through `CYBER_INSTALL_BASE_URL`, with fake archives whose `cyber` is a shell script, and finishes with the real `cyber uninstall --force`. The release scripts, the workflow trigger, the `Dockerfile` and the npm launcher are tested by `python3 -m unittest discover -s scripts/tests`; CI also runs `sh -n`, `dash -n` and `shellcheck -s sh` on the script.

## Sessions and subagents

### Naming and resuming Sessions

A Session gets a generated title after its first prompt. Give it your own to find it later:

- TUI: `/rename Auth refactor` renames the current Session at once; a bare `/rename` (or **Rename session** in the `Ctrl+P` palette, or the unbound `session_rename` key) opens a dialog prefilled with the title (Enter saves, Esc cancels, Ctrl+U clears). In the Session picker (`/resume`, Ctrl+X L) Ctrl+R renames the selected Session. The header, sidebar and terminal window title change as soon as the server publishes `session.renamed.1`, in every attached client.
- CLI: `cyber sessions list` (`ls`, `--limit`, `--format json` with `id`, `title`, `directory`, `updated_at` and `status`), `cyber sessions show <session>`, `cyber sessions rename <session> <title…>`, `cyber sessions delete <session> --yes` (`rm`; it removes child Sessions too and cannot be undone) and `cyber sessions export <session> [--sanitize] [--include-children] [--all-branches] [--format json|md|html] [--output <file>]`. The export (also `GET /api/v1/sessions/{id}/export`) holds the Session info and totals, the messages of the active path (every branch with `--all-branches`, see [Session branches](#session-branches)) with their parts and tool calls, its Goals with their evaluations and its Workflow Run summaries; `--include-children` nests child Sessions, and `--sanitize` replaces text, paths, URLs, tool inputs and outputs and metadata with `[redacted:<kind>:<n>]` markers while keeping IDs, structure, token counts and timings, so it can be shared in a bug report. `--format html` renders the same document as one self-contained page to open in any browser or attach to a PR: inline CSS, no script and no external resource (a `default-src 'none'` Content Security Policy), every Session string escaped, Markdown with raw HTML shown as text and links shown as text with their URL, tool calls and reasoning as collapsible sections (tool output cut at 20 000 characters; the JSON keeps all of it), Goals, Workflow Runs, child Sessions under the call that started them, and a light or dark scheme from `prefers-color-scheme`; file edits show as the edit tools' inputs, since the document has no per-Turn diff yet. `--output <file>` writes any format atomically (a symlink or non-regular file is refused, an existing file keeps its permissions and a new one is `0600`). `--format html` is refused by every other command (exit 2). In the TUI, `/export [html|md|json] [path]` (or **Export session** in the `Ctrl+P` palette) writes the current Session to `<title-slug>-<short-id>.html` in the Location by default, asks before replacing an existing file, and shows the path with a note that the file holds the full conversation. A JSON export also carries a top-level `attachments` array with the bytes of every image or PDF the messages reference (`{ sha256, media_type, size, name, data }`); `--sanitize` omits it and redacts each attachment name as a `path` marker, keeping its media type and size, and Markdown and HTML show `[image: image/png, 1.2 MiB, shot.png]` or `[pdf: …]` placeholders without the bytes. `cyber sessions import <file>` (also `POST /api/v1/sessions/import`) creates a new Session in the current Location from a JSON export of the active path, with fresh IDs, and prints `Imported session: <id>`; it verifies every attachment against its SHA-256 and `attachments.max_bytes` before writing anything and fails without creating a Session on a mismatch. A sanitized export, an `--all-branches` export or one from a newer version (`ExportTooNewError`) is refused, and the source's rules, budget and Mode are not imported. A fork or a child task gets its own hard-linked (or copied) attachment blobs, so deleting the source keeps them, and deleting a Session removes `<data>/attachments/<session_id>/`. `fork`, `rewind`, `repair-context`, `share` and `unshare` are not available yet.
- Resume: `cyber -r "Auth refactor"` opens the TUI on it and `cyber exec --session "Auth refactor" "go on"` prompts it.

`<session>` is an ID, an exact title, or a unique ID prefix, among the Location's Sessions. A title or prefix that matches several Sessions exits 2 and lists their IDs. A title you set is kept: trimmed, with whitespace runs (newlines included) joined into one space, control characters dropped and at most 100 characters; an empty title is refused with `InvalidRequestError`. Generated titles never replace it, even when the title request was already in flight.

### Session branches

A Session keeps every attempt. Its messages form a tree, and only the active path (from the active tip up to the first message) is sent to the model. A conversation rewind (`POST /sessions/{id}/rewind` with `target` `conversation` or `both`) no longer drops the later messages: they stay as an inactive branch, and the next prompt starts a new branch from the rewind point. Sessions rewound by older builds gain the same tree on their next replay, with an unchanged active path; no migration runs. A rewound message ID stays taken, so re-sending a prompt with it answers `409 ConflictError`.

- TUI: `/tree` opens the user messages of every branch as a collapsible tree, with the active path highlighted, `(n branches)` at branch points and labels as tags. ↑/↓ (or `j`/`k`) move, Space folds, Enter checks out the selected branch and reloads the transcript, `c` adds the code restore (the restored diff then opens in the task-log view) and `l` types a label for the selected message; Esc closes. `/label <name>` labels the active tip and `/label --clear <name>` removes a label. With more than one tip, the header (or the docked sidebar) shows `branch <label or short id> (2 of 3)`.
- CLI: `cyber sessions tree <session>` prints an ASCII tree (`*` active path, `o` inactive, `(active)` on the tip) or the nodes with `--format json`. `cyber sessions checkout <session> <message> [--code] [--exact]` prints `tip <id>` (and `restored <n> files` with `--code`). `cyber sessions label <session> <name> [--at <message>] [--delete]` labels the active tip unless `--at` names a message. `cyber sessions export <session>` prints the Session export with its active path (`branches: "active"`); `--all-branches` includes every branch (`branches: "all"`), linked by `parent_message_id`, and `--sanitize` keeps those links. `<message>` is a message ID, a label, or a unique ID prefix over every branch; an ambiguous prefix exits 2 and lists the IDs.
- API: `GET /sessions/{id}/tree` returns nodes for user messages only, `{id, parent_id, preview, created_at, labels, active, tip, descendants}`. `POST /sessions/{id}/checkout` takes `{message_id | label, exact?, code?}` and returns `{tip_message_id, code_restored, restored_files, revert?}`. `PUT /sessions/{id}/labels/{name}` with `{message_id}` and `DELETE /sessions/{id}/labels/{name}` answer 204. `GET /sessions/{id}/messages` still returns the active path, and `?branches=all` returns every branch in creation order; every message carries `parent_message_id`. TypeScript clients call `client.call("v1.session.tree", …)`, `v1.session.checkout`, `v1.session.label` and `v1.session.unlabel`.

Checkout rules:

- Checkout goes to the newest tip under the chosen message, so the active path ends at a leaf; `exact` makes the message itself the tip, and the next prompt branches there.
- The Session must be idle: a running Turn, a compaction, a running background Job (a process, Monitor, subagent or workflow) or delegation, or input waiting to start answers `409 SessionBusyError`, because a Job's result would be handed back into the newly active branch; and a staged rewind must be committed or cleared first. Held messages stay held.
- Without `code`, files are left alone. With `code`, the working tree is restored to the snapshot after the last step on the target path through the same three-way restore as a rewind, so your own edits are kept; the restore is recorded as a staged `code` revert that `POST /sessions/{id}/revert/clear` undoes. A conflict or a branch with no snapshot refuses the checkout and changes nothing.
- A compaction summary is reused only on the path it summarized: after a rewind or checkout the summary covering the longest prefix of the active path is kept, and with none the path's own messages are sent (auto-compaction applies as usual). A sibling branch's summary never reaches the model.
- Labels are 1 to 48 of `A-Z a-z 0-9 . _ -` and unique per Session (a taken name is `409 ConflictError`); a label stays on its message when that message stops being a tip. An unknown label or message is `404 MessageNotFoundError`.

Branches are not deleted, merged or summarized into each other; they stay until the Session is deleted. `fork` still creates a new Session.

Worktree startup runs trusted `worktrees.setup` commands inside the sandbox and reports output on stderr. Setup failure keeps the Session and files.

The model's `agent` tool description is rebuilt every Turn. After the fixed text it lists, sorted by name as `- <name>: <description>`, every visible agent whose mode is `subagent` or `all` and whose `agent` rule is not `deny` for the caller, so an agent added to the configuration appears on the next Turn without a restart and a denied one is never offered. At most `agents.max_concurrent` (default 8) subagents of one top-level Session run at once; further spawns wait in FIFO order without creating a child Session. A spawn that has to wait publishes the live (never durable) `session.tool.progress` event with `{session_id, call_id, status: "queued"}` on the Session and Location event streams, then `status: "running"` once it holds a slot. Workflow agents use `workflows.max_concurrent` instead.

In the TUI, `/subtask <prompt>` forks a background child with the current context and keeps the parent open. API clients use POST `/sessions/{id}/subtask` with `{ "prompt": "try another approach" }`; TypeScript clients use `client.session.subtask(id, { prompt })`. `/tasks` (alias `/ps`) lists the Session's background commands, monitors, subagents and Workflow Runs with kind, status, elapsed time and the last output line. Enter opens a subagent's thread, a Run in the Workflow monitor, or the end of a command's or monitor's log (↑↓ scroll, Esc closes); `x` stops the selected task while no filter is typed, and Ctrl+S stops it at any time. `/stop` asks for confirmation before stopping all running tasks of the Session. Interrupting the parent Turn leaves background tasks running. Server restart records unfinished tasks as interrupted and queues their handback without redispatch. The model can stop its own tasks with `task_stop`; API/SDK clients can list, read and stop jobs through `/jobs`.

The model starts a background command with `bash` (or `powershell`) `{ "command": "npm run dev", "background": true }`. The command is approved, guarded and sandboxed as in the foreground, and the call returns at once with `job_id` and `output_path`, the Job's log at `<data>/jobs/<job_id>.log`. The Job keeps running after the Turn and through interrupts; the model reads its output with `read` on that path (`offset`/`limit`). The log keeps the newest output: past `background.max_output_mb` (default 64) the oldest half is dropped behind an `[output truncated: N earlier bytes dropped; …]` line. When the command exits, its Session gets a queued notice with the Job, exit code, duration and the last 20 lines of output, unless the call passed `notify: false`. A Session runs at most `background.max_jobs` (default 20) commands at once; one more fails with `Background job limit reached (20)`. `task_stop` (or `POST /jobs/{id}/stop`) sends SIGTERM to the command's process group and SIGKILL after 5 s (Windows: the Job Object is terminated) and records `cancelled`. Deleting the Session stops its Jobs without a notice; a server restart records running ones as `interrupted`. Event streams carry `job.started.1`, `job.ended.1` (with `exit_code`) and `job.cancelled.1`, plus live-only `job.output.1` chunks; `JobEnded` hooks (matcher: the Job kind, for example `bash`) receive the Job and its `exit_code`. A running Job cannot ask for network access: sandbox-proxy requests to unlisted domains are refused. API clients read the end of a Job's log with `GET /jobs/{id}/output` (`tail_bytes`, default 64 KiB).

In the TUI, input starting with `&` (for example `& npm run dev`; the input bar reads `background shell`) starts the command as a background Job of the Session without the model: it runs sandboxed like a `!` command, records no message and starts no Turn, and its completion notice is queued like any Job's. API clients post `/sessions/{id}/shell` with `{ "command": "npm run dev", "background": true }`; the response carries the Job. Ctrl+B (binding `background_command`) while a Turn runs a foreground `bash` call moves that command to a background Job: the process keeps running, its log starts with the output captured so far, and the call returns the Job ID so the Turn continues. API clients post `/sessions/{id}/background`; it answers 409 when no foreground `bash` call is running. If the Job cannot start (for example at `background.max_jobs`), the command stays in the foreground.

The model watches a source with `monitor`: exactly one of `command` (a shell command's output, approved and sandboxed like `bash`), `url` (a `ws://` or `wss://` WebSocket; needs the `network` permission for the host, and domains the sandbox denies, or every domain with `sandbox.network: off`, are refused with `Sandbox denied: network <host>`) or `path` (lines appended to a file after the monitor starts; its directory must exist; needs the `bash` permission for `tail -F <path>` and respects `deny_read` and the external-directory check for the path and the file it resolves to; each open re-checks that the path still names that file, so a symbolic or hard link swapped in later ends the monitor). Lines matching `filter` (a regex; default every line) arrive in the Session as queued messages `Monitor <job_id>: <line>` with source `monitor`, so they never interrupt a running Turn. At most `max_lines_per_minute` (default 30) are delivered per minute; the rest are summarized as `N lines suppressed`. The monitor ends when `until` (a regex) matches a line, after `timeout_seconds` (default 3600), when its command exits or its WebSocket closes, or on `task_stop`, and a `Monitor ended` notice with the last lines follows. Every line is kept in the monitor's log (`output_path`), and monitors count against `background.max_jobs`.

For interactive programs the model uses a PTY (Unix): `pty_start` `{ "command": "python3 -q" }` (or no command for the preferred shell) is approved, guarded and sandboxed like `bash` — an interactive interpreter is unresolved source, so it asks — and runs the program on a pseudo-terminal it leads with `TERM=xterm-256color` and `CYBER_TERMINAL=1` as a Job of kind `pty` (counted against `background.max_jobs`, logged, stopped by `task_stop`). `pty_write` `{ job_id, input, keys }` sends text and named keys (`enter`, `ctrl-c`, `ctrl-d`, arrows…) and is checked against the `bash` rules for the PTY's command. Into a shell (no command, a shell program or wrapper, or shell syntax) each typed command is authorized as a `bash` command before anything is written — a line ending in an unescaped backslash, or leaving a quote, heredoc or compound command open, is joined to the next ones as the shell reads them (an open construct must close in the same write; an alias definition is refused) — rules, the critical-path removal guard (also in `bypass`) and excluded commands — and keys that would run text no check saw (tab, arrows, other control keys besides backspace, `ctrl-c`, `ctrl-u` and `ctrl-d`) are refused; `pty_read` returns the output since the previous read as plain text (waiting up to `wait_ms` for the program to go quiet) or, with `mode: "screen"`, the terminal's screen. A client attaches with a single-use ticket: `POST /jobs/{id}/pty/ticket` (the server's credentials) returns one valid for 60 s, and `GET /jobs/{id}/pty/connect?ticket=…&cursor=…` opens a WebSocket that replays the retained 2 MiB (no cursor), resumes from a byte cursor or tails (`-1`), then streams output; frames from the client are input. The PTY tools are absent on Windows.

`notify` `{ title, body }` notifies the user: the TUI shows a desktop notification (`tui.notifications`) and a toast. Titles are cut to 80 and bodies to 300 characters; a Session may send 10 per hour, and `background.deny_notify_in_plan: true` refuses it in plan mode. `send_file` `{ paths, caption }` delivers up to 10 files of at most 25 MB each, each checked like `read`, and a Session sends at most 100 files and 256 MB in all: the server keeps a copy under `<data>/sent/`, records `session.files.sent.1`, and clients download it with `GET /sessions/{id}/files/{file_id}`; the TUI says what arrived and where. Deleting a Session (also an archived one swept by retention) removes its copies and its process Job logs (`<data>/jobs/<job_id>.log`); both directories also age out under `storage.retention.tool_output_days`, and `cyber db backup --artifacts` includes `sent`.

`/btw <question>` (alias `/side`) asks the Session's model a side question in an overlay. The model sees the conversation so far and may only read the project (`read`, `list`, `glob`, `grep`; anything that would ask is refused); nothing is added to the Session. Esc discards the answer and `i` inserts it into the composer. API clients use `POST /sessions/{id}/side` `{ question }`.

`/agent` (alias `/subagents`) browses direct child threads across worktree directories, showing observed running, waiting, completed or failed status. Enter opens an existing thread; Ctrl+R refreshes, and child threads include a parent navigation entry. Active-thread prompts retain the selected Session's ID, directory and steer delivery. API clients use GET `/sessions/{id}/children` with `limit` and an opaque `cursor`; SDK clients use `client.session.children(id, options)`. Idle child prompts start a fresh structured attempt under exclusive execution ownership, preserving the schema and child name. A tool still collecting its result refuses continuation; active-thread input remains steering. Held or non-waking input preserves the completed result. Acknowledged clean-removed checkouts now recreate through the same child identity and original base, with trusted source setup before inference. Successful continuations apply cleanup policy and record file summaries; failures and interruption retain the checkout. Cleanup confirmation blocks new input until it settles. Missing/replaced ownership, current deny rules, read-only recreation and incomplete setup refuse continuation. Complete queued/held dispatch, unknown-outcome resolution and native acceptance of this client path remain pending.

A leading TUI mention such as `@explore where is retry logic?` starts a fresh background child using that profile. The `@` menu includes visible subagent-capable agents beside files and quotes names containing spaces. Embedded/file mentions remain ordinary prompt text; use `@./path` for a file whose name matches an agent. Results return through the parent handback and `/tasks`. Before a Job is assigned, `/admissions` lists saved request IDs; Enter looks up a request, Ctrl+R reconciles an idle unknown receipt and Ctrl+S cancels it. Esc cancels the current Session's pending admission before interrupting its conversation. IDs are saved before submission and looked up on reconnect without resubmitting prompts. Pending requests survive TUI exit; use `/admissions` to inspect or cancel them after reconnecting. API/SDK callers can pass an optional `agent` to the Session subtask endpoint; omitting it preserves the forked current-profile behavior. Use Enter to start a mentioned child; queued/held delegation and generic prompt admission are still being implemented.

`cyber exec '@explore find retry logic' --file notes.txt --max-turns 4` starts a fresh background child using a visible subagent-capable profile. Exec prints the child's complete durable text and usage and waits for its Job to settle, including cleanup. JSON results retain a client-generated `delegation_id` and parent Session before submission, then identify the child Session and Job after admission. Timeout and interruption cover request submission, queued admission and task following; they cancel only that request or its recorded Job. Local token/cost checks include the child’s own and nested descendant usage and request stop of the owned Job at the recorded limit. JSON reports combined `cost_usd` and `total_tokens` alongside `own_cost_usd`, own token classes and the four descendant billing fields. Budgeted delegation requires complete descendant billing from the attached server. Client polling can overshoot while calls remain in flight; server-side subtree scheduling and cancellation remain open. Unacknowledged cancellation is reported explicitly. Attachments retain their Content type, and the requested step limit is capped by the profile. Attached servers must advertise the durable admission routes and delegation fields. Session setup, catalogue discovery, file reads and capability negotiation precede the delegation timeout. Parent handback remains enabled; queued/held mention delivery and durable server-side subtree budget enforcement remain open.

`cyber review [target]` reviews uncommitted changes (the default), `<base>..<head>`, one commit, or `pr <number>`. A PR review needs `gh`, and its commits must already be local. The target is checked before any server contact. Put `--` before a target that starts with `-`. The server then resolves it to commits, withholds paths the reviewer could not read without asking, and runs the bundled `review` skill in the read-only `reviewer` subagent. Like `cyber exec --command review`, the command waits for that Job and its handback, and on `--timeout`, a budget stop or SIGINT it cancels only that Job. Text output lists the findings most severe first, followed by the limitations, or prints `No findings.`. A clean tree prints `No changes to review` and creates no Job. With `--format json`, the output gives `target`, `target_kind`, `findings` (re-validated and sorted by severity, file, line and summary), `limitations`, `counts`, the handback `result`, the reviewer `session_id`/`job_id`, the `delegation_id` and usage. Exit codes:
- 0: the review completed, or there was nothing to review.
- 1: the reviewer failed, could not start (for example an `agent` rule denied `reviewer`), or returned an invalid result (`SchemaMismatch`). JSON reports `reviewed: false`.
- 1, or 5 with `--fail-on-deny`: a permission rule refused the review before anything ran (for example a `bash` deny of `gh *` for `pr <number>`). JSON still prints the review object with `findings: null` and the denial in `error`.
- 2: the target is invalid or cannot be resolved; JSON names its `code`.
- Timeout, budget and denial stops use the exec exit codes.

A project or user `review` command or skill replaces the bundled one. It runs as written, and JSON reports `findings: null`. `/review` in the TUI uses the same server path. The model cannot invoke the bundled `review` skill itself, because only `/review` and `cyber review` supply the resolved diff.

Target grammar: nothing (staged, unstaged and untracked changes against `HEAD`, or the empty tree when `HEAD` is unborn), `<base>..<head>` (both sides required; `...` and open-ended ranges are rejected), one revision (that commit against its first parent), or `pr <number>`. Untracked files hidden by your global excludes file (`core.excludesFile` in `~/.gitconfig` or `~/.config/git/config`, else `~/.config/git/ignore`) are skipped, as in `git status`; a repository's own `core.excludesFile` replaces the global one. A global excludes file over 64 KiB refuses the review (it is not truncated, which would unhide ignored files). A revision may contain only `A-Z a-z 0-9 . _ / ~ ^ -`, cannot start with `-` or `/`, and cannot contain whitespace, `:`, `@{` or `..`. Revisions are resolved to commit IDs by a library engine in the sandboxed inspection helper, which never runs git hooks, external diff, textconv, filters or a pager, and never passes your text as a process argument. Permissions are checked before anything is admitted: an `external_directory` deny for a repository outside the Location refuses the review (403), and each changed path whose `read` decision (or `external_directory` decision outside the Location) is not allow is withheld from the reviewer and named in `limitations`. An `ask` withholds like a deny, so under the default rules modified `.env` and `.env.*` files (other than `.env.example`) are never sent; no prompt is raised. These are the reviewer's own decisions under its read-only ceiling, so running `/review` from a `bypass` Session does not send more. Withheld files do not count against the patch limit. If every changed file is withheld, or withholding leaves no patch text at all, the review is refused with `all_withheld` (`cyber review` exits 2) rather than reported as a clean tree. `PreToolUse` hooks do not run for this host step, so protect secrets with `read` rules. `pr <number>` runs the permission-checked `gh pr view <number> --json baseRefOid,headRefOid,url` in the command sandbox and reviews the PR's own changes: from the merge base of the base and head commits to the head, as GitHub's PR diff does, so later base-branch commits are not shown reversed. A plain `<base>..<head>` target diffs the two commits directly, like `git diff A..B`. It never fetches, checks out or comments, so fetch the base branch and the PR commits first. `gh` is looked up on your `PATH` outside the project (relative `PATH` entries are ignored), runs with that same filtered `PATH` so a project cannot plant the `git` it calls, and `GH_TOKEN`/`GITHUB_TOKEN` reach it only when `sandbox.env_allow` lists them. The patch sent to the reviewer is capped at 256 KiB; truncated and binary files are listed in `limitations`. Resolution failures are reported before admission as HTTP 422 `ReviewTargetError` with a `code`: `invalid_target`, `invalid_ref`, `unresolved_ref`, `not_a_repository`, `empty_diff`, `all_withheld`, `gh_unavailable`, `pr_unresolved` or `pr_commits_missing`. The reviewer must return `{findings: [{file, line, severity, summary, failure_scenario}], limitations: []}` (`severity` is `critical`, `high`, `medium` or `low`; `file` is repository-relative with no leading `/`, drive or `..` segment; `summary` and `failure_scenario` are not blank). The schema enforces exactly what the CLI and TUI re-check, so an invalid result gets one corrective retry and then fails with `SchemaMismatch`. Retrying POST `/command` with the same message ID, name, arguments and delivery returns the original receipt without resolving the target again.

In the TUI, `/review [target]` checks the target locally when the bundled skill is active and shows an invalid target in the status line without contacting the server. An overriding `review` receives its arguments unchanged. A refused review (`No changes to review`, any other `ReviewTargetError`, a permission denial or any other failed post) shows the server message in the status line and starts no task. The TUI chooses the command's message ID, so it knows the reviewer's delegation and treats only that delegation's Job as the review; other Jobs that end meanwhile are not reported as it. When the reviewer Job completes, the findings view opens with severity counts and the re-validated findings, most severe first, with control characters escaped; ↑/↓ and PageUp/PageDown scroll and any other key closes it. A failed reviewer, an invalid result or a reviewer that could not start shows the error instead.


For a lost launching admission receipt, API callers can use `POST /api/v1/sessions/<session>/delegations/<request>/reconcile`; SDK callers can use `session.reconcileDelegation(session, request)`. The server links exactly one existing Job only when its captured operation and Session authority match. Repeating reconciliation does not launch work. Missing or ambiguous proof remains `unknown`; a reserved admission can still be stopped explicitly. Recovering the receipt does not resolve an uncertain native Job or its cancellation. In `/admissions`, Ctrl+R contacts this endpoint for the selected idle unknown receipt using its saved Session and Location. A lost response is looked up, never resubmitted as a task; unresolved records remain saved for reconnect. CLI users can run `cyber api v1.session.reconcileDelegation --param sessionID=ses_... --param requestID=op_...`. Full native/lifecycle acceptance and native-effect recovery remain open.


## Auto-mode controls

In auto mode, approval-required tool actions use the configured evaluator (or small model). Decisions are recorded before execution; three consecutive blocks or an unavailable evaluator fall back to manual approval, or denial without an attached user.

Configure controls in trusted global or project configuration:

```json
{
  "permissions": {
    "auto_mode": {
      "rules": {
        "always_block": [{ "action": "bash", "resource": "kubectl apply *" }],
        "always_allow": [{ "action": "edit", "resource": "src/*.rs" }]
      },
      "policy": "Only make changes for the current ticket.",
      "classify_read_only": false,
      "fallback": "ask"
    }
  }
}
```

Block rules take precedence. Allow rules cover every resource and preserve hard denies, protected paths and parent manual approval. Read-only tools skip classification by default; set `classify_read_only` to true to review their approval-required requests. Set `fallback` to `deny` to refuse fallback even with a user attached. Project settings require trust. Inspect or clear recorded decision counters for the current checkout:

```bash
cyber permissions auto show
cyber permissions auto reset
cyber --format json permissions auto show
```

Counters include allows, blocks, fallbacks and classifier/policy decisions. They start when checkout scope was first recorded (or at reset); historical unscoped events are excluded. Reset preserves decision history and spending records and leaves other checkouts unchanged.

After a classifier block, use `/approve` in the TUI to review the original tool call and reason, then confirm one replay. It uses the stored input without another coding-model turn, saves no permanent approval and preserves current denies, protected paths and parent manual approval. Rejected or unattended confirmation has no effects. A consumed replay remains spent even after restart or failure; unresolved effects require recovery. Configuration block rules cannot be overridden this way. API clients can POST `/api/v1/sessions/:id/approve` or call `client.session.approve(id)`. The returned receipt identifies the original decision and replay call; subscribe to Session events and answer the `auto_override` permission request to proceed.

Children also retain every effective auto-mode ancestor's gate. Eligible requests use each parent's policy and task context, and all gates must allow before execution. Any configured block or deny fallback prevents effects; independent manual approvals still prompt. Review decisions and usage are recorded on the executing child with the reviewed ancestor's identity.

## Budgets and deferred input

New Sessions accept a `budget` object through POST `/sessions`: `{ "max_turns": 20, "max_tokens": 100000, "max_cost_usd": 2, "max_wall_seconds": 600, "enforcement": "soft" }`. Omit fields to leave those dimensions unlimited; a zero cap blocks dispatch. Trusted `budgets.session` configuration supplies creation defaults. Caps persist across restart and new prompts, count the Session and descendants, and apply to visible Turns, title, evaluator, compaction and web-page summary calls. Wall time starts at the first gated dispatch and continues while idle. Soft limits publish durable 80% warnings and exhaustion events, let in-flight calls settle and block new calls at the recorded limit; concurrent calls can overshoot. Independent forks copy the cap with fresh spending and activation. Project budget overrides require workspace trust. Reserved enforcement is currently refused; reservations, daily caps, budget-triggered subtree cancellation and complete client budget displays remain open. The runtime has bounded local subtree-stop reporting; exec/TUI adoption remains open.

Deferred prompts (`resume: false`) can be dispatched through authenticated POST `/api/v1/sessions/{id}/wake` or SDK `session.wake(id)`, without resubmitting their text. Held input needs explicit inbox release. For idle children, both paths prepare the verified checkout and reset the structured attempt while preserving the original inbox identity, admission sequence and edited content. Preparation failure keeps the input pending or held. Waking a completed child without promotable input preserves its result without checkout recreation or inference. Requested child prompts queued during an active attempt now hand off automatically after the foreground collector or background Job settles its original result and billing. `resume: false` prompts remain deferred; interruption clears automatic wake intent while retaining their rows. Interruption cancels and awaits owned checkout preparation; late success cannot admit or release input. Unacknowledged preparation requires recovery after restart. Durable source/target admission fences and subtree closure are implemented in the runtime. Reviewed unknown-effect recovery and native acceptance remain open.

Session detail and list responses expose `children_cost`, `children_tokens`, `children_unpriced_steps` and `children_usage_complete` separately from own usage. Descendant totals include nested children and hidden title, compaction and evaluator calls, and persist across restart and child deletion. Retained billing receipts contain IDs and usage only, without prompt content; deleting an ancestor removes its receipts. Older databases are backfilled from surviving history and report `children_usage_complete: false` because previously deleted child billing cannot be reconstructed. New Sessions start with complete attribution. Soft subtree budget enforcement is implemented: ancestor caps include descendant spending and block subsequent provider dispatch, including after child deletion. Budget-triggered subtree cancellation and complete live client counters remain open.

For durable API admission, POST `/sessions/{id}/delegations/{requestID}` with the subtask body and a client-selected `op_` ID. GET that path to inspect admission, and POST `/stop` beneath it to cancel that request or its recorded Job. Repeating the same ID/input returns the existing record; different input conflicts. A stop received before submission prevents a delayed POST from dispatching. `admitted` exposes a Job ID to follow through `/jobs`; `unknown` preserves recovery uncertainty and prevents automatic redispatch. The SDK methods are `session.startDelegation`, `session.delegation` and `session.stopDelegation`. Exec uses this protocol and recovers a lost submission response by looking up the same request without redispatch. The TUI uses saved IDs for named mentions and `/subtask`, with bounded cancellation and explicit unknown outcomes. An explicit stop can resolve an abandoned `reserved` admission as cancelled because no launch marker was committed; `launching` uncertainty remains fenced. Automatic resolution of unknown native effects remains open.


## Explicit subtree stop

POST `/api/v1/sessions/{id}/stop-subtree` or SDK `client.session.stopSubtree(id)` closes admission and requests a bounded local sweep of the Session and descendants. The response includes `session_id`, `scope_id`, `status` (`acknowledged` or `unknown`), `problems` and `persisted`. HTTP 200 means a report is available; inspect its status and persistence before treating cancellation as acknowledged. Queued/held input stays intact and unrelated Jobs continue.

Admission remains closed after either report. Repeating a new request reassesses the same closure; replaying an idempotency key returns the original observation. Ordinary interruption keeps its existing background-child independence.

For a persisted `acknowledged` report with a `receipt_id`, explicitly review and submit POST `/api/v1/sessions/{id}/reopen-subtree` with `{ "scope_id": "op_...", "stop_receipt_id": "evt_..." }`, or SDK `client.session.reopenSubtree(id, review)`. The server replays fresh Session state, checks scoped actor evidence and retains native checkout ownership through the writer commit. Stale, Unknown, changed or orphaned evidence refuses reopening. The response includes `reopen_receipt_id`; reopening preserves input and starts no inference. Independently closed descendant scopes stay closed. Use an explicit wake or held-input release afterward when needed.

Reviewed recovery for unknown native effects and orphaned sweep/operation owners remains required. Exec/TUI and budget-triggered adoption remain open.

## Worktrees and setup recovery

`cyber worktree list` shows path, branch, dirty state, ahead/behind counts relative to the creation base and associated Sessions; use `--format json` for structured output. Pending or invalid ownership records remain visible for recovery.

`cyber worktree remove <name>` removes a managed worktree and its branch. A worktree with uncommitted changes, ignored-file edits or commits beyond its base needs `--force`. A worktree that is the Location of a running Session is refused with `Worktree in use by ses_<id>`, even with `--force`. `cyber worktree prune [--older-than <days>]` (default 14) removes clean worktrees with no commits beyond their base whose ownership record is older than that, and reports the ones it could not remove; `--format json` prints `{ "removed": [...], "skipped": [...] }`.

When `cyber exec --worktree` or `cyber --worktree` exits, the worktree it created is removed with its branch if it has no changes and no commits beyond its base. Otherwise both are kept and stderr prints `worktree kept at <path> (branch <branch>): <reason>`. `worktrees.cleanup` selects `auto` (default), `keep` or `ask`; with `ask` the worktree is kept, because no client is left to confirm. API clients run the same cleanup for an idle top-level worktree Session with `POST /api/v1/sessions/{id}/worktree/cleanup`.

The model can move a Session into a worktree with `enter_worktree { name?, path? }`: an existing managed worktree of the repository by name or path, or a new one on `cyber/<name>` (name generated when omitted), after the `worktree` permission, `WorktreeCreate` hooks and setup. Later tool calls resolve paths in the worktree; calls issued in the same step after the move are not run and the model reissues them. Removing the worktree (`cyber worktree remove`, `prune` or `DELETE /worktrees/{name}`) is refused while the Turn runs and, without `--force`, while the Session stays entered, even idle; `--force` first returns an idle entered Session to the Location it entered from. The exit cleanup of another Session in that worktree (an isolated child, `cyber exec --worktree`, `cyber --worktree`, or a Session that created it with `enter_worktree`) keeps it while a Session stays entered (`worktree entered by ses_…`). `exit_worktree { keep? }` returns the Session to the Location it entered from; unless `keep` is true, a worktree the Session created and left without changes is then removed with its branch, while an existing worktree it only entered is always kept. A Session that ends while entered gets the same cleanup: `cyber exec`, the TUI on exit and `POST /api/v1/sessions/{id}/worktree/cleanup` return it, idle, to that Location and remove a clean worktree it created. The same route returns an entered Session whose worktree was already removed, so it can run again. `enter_worktree` is offered only outside a managed checkout and `exit_worktree` only after entering one; Sessions started in a worktree and isolated children get neither, and a call that arrives anyway (from such a Session, an entered one, or alongside another call in flight) is refused before any worktree, permission prompt, hook or setup runs. The moves are durable events (`session.worktree.entered.1`, `session.worktree.exited.1`) that attached clients and the TUI follow.

Creation, removal and exit cleanup publish live `worktree.created.1`, `worktree.removed.1` and `worktree.kept.1` events `{ name, path, branch, session_id? }` on the event stream of the Location that asked.

The model's `agent` tool accepts `isolation: "worktree"` for fresh foreground/background children, including forked and structured tasks. Each child gets a branch `cyber/<parent-short-id>/<name>` and a durable checkout binding. Isolated child names may use the Session naming limit of 128 bytes, subject to Git branch-name validation; managed storage names are generated separately. Its result includes `worktree.path`, `branch`, `kept` and changed files with line counts; binary/large-file counts can be unknown. Clean checkouts are removed under the default cleanup policy. Edits, commits ahead of the base, setup failures and cancellation retain the checkout. Set `worktrees.keep: "always"` to retain clean checkouts too. `cleanup: "ask"` requests child-owned confirmation through the parent approval route; rejection or an unattended session retains the checkout. New edits, keep policy and deny rules are rechecked after approval. Interrupt/stop clears pending cleanup requests and preserves the checkout. Retained children resume against the same verified checkout. Clean-removed children resume with the same Session/branch and a new checkout identity, pinned to the original base; current trusted source setup must complete before inference. Missing, force-removed, incomplete or replaced removal evidence requires recovery. Setup failure stays fenced across restart. Session streams and history preserve Location across rebinding/replay, and old/new Location listeners receive the rebound notice before later events route to the new Location. The TUI updates its directory immediately and scopes later actions to it. Existing primary-profile forks and user subtasks can resume, while fresh primary subagent spawns retain their eligibility checks. An explicit `/subtask` uses the current profile’s isolation setting and authorizes checkout creation and trusted source setup in every Mode. Child tools keep their inherited Mode, approval requests and deny ceilings; read-only sandbox policy still refuses creation. Native platform acceptance is pending.

The APIs are `GET /api/v1/worktrees` for listing (each ready entry has `created_ms`), `DELETE /api/v1/worktrees/{name}[?force=true]` for removal and `POST /api/v1/worktrees` for startup, with `{ "name": "fix-login", "session": { "model": "provider/model" } }`; omit `name` to generate one.

For a setup-pending isolated child, inspect `GET /api/v1/sessions/{parent}/children/{child}/setup`, then submit `POST` to the same path with the reviewed `revision`, `digest`, optional `retry_index` and a nonempty `reason`. The SDK methods are `worktree.inspectChildSetup(parent, child)` and `worktree.recoverChildSetup(parent, child, review)`. An explicit retry preserves earlier successful steps and the original failure; use an idempotency key to replay the same request safely. Omit `retry_index` to continue undispatched steps or acknowledge fully completed setup. A command preparation failure recorded as `not_dispatched` can also be retried after review because it never reached process launch. Changed recipes, stale reviews, active/unknown ownership, pending command outcomes and generic execution or launch errors without an acknowledged exit cannot authorize retry. Recovery retains current deny/sandbox rules and starts no inference.

## Usage and cost

`GET /api/v1/usage?scope=session&id=<session>` returns own, descendant and combined billing from one database snapshot, including retained charges after descendant deletion. Token classes, combined known tokens, unpriced calls and attribution completeness remain separate. The SDK method is `client.usage.get({ scope: "session", id })`. `scope=run&id=<run_>` returns a Workflow Run rollup (total, per agent, per model and per tier); the response's `scope` field tells the two shapes apart. Other scopes currently return a tagged unavailable error.

The session scope also breaks the Session and its descendants down: `by_model` (one entry per provider and model with the five token classes, `total_tokens`, `cost` or `null` when every call was unpriced, `unpriced_steps` and `steps`, sorted by cost and then tokens), `api_duration_ms` (summed model call time), `wall_duration_ms` (first admitted prompt to the last recorded activity, or to now while the Session runs; `null` before the first prompt), `lines_added` and `lines_removed` (over the Session's per-message diffs; `null` without snapshot history) and `budgets`: the Session's own Budget and `budgets.daily`, each with its cost, token and Turn `limits` (`limit`, `used`), and for the daily budget the next local midnight `reset_ms` and the server's IANA `zone`. Daily spending counts every Session on the machine since local midnight.

When the Session directory's configuration cannot be read for `budgets.daily` (a malformed budget, or a file that does not parse), the report is still served without the daily budget and says why in `budgets_error`; the Usage tab shows it as `Daily budget unavailable: …`.

These numbers come from the usage step ledger: one `usage_step` row per provider step (finished or failed) and per auxiliary call (title, compaction, evaluator, discarded title), written in the same transaction as its event and kept when a descendant is deleted. Upgrading a database indexes its stored events in the background, 5,000 events per transaction and resumable across restarts; until it finishes the report has `backfilling: true` and `by_model` may be partial. The retention sweep folds rows older than `telemetry.usage_retention_days` (default 365, `0` keeps them) into `usage_daily` aggregates per UTC day, model, agent and root Session, so totals remain. Local days use [jiff](https://docs.rs/jiff) (the system tz database; bundled on Windows), chosen over `chrono-tz` because it added 0.12 MiB to a release probe binary against 1.15 MiB.

In the TUI, `/cost` and `/usage` open the Settings panel's **Usage** tab (see below).

### Usage insights

`GET /api/v1/usage/insights?window=<24h|7d>&directory=<path>` (SDK `client.usage.insights(...)`, operation `v1.usage.insights`) says which characteristics of recent usage account for its total. `window` is `24h` (the default) or `7d`; anything else is `400`. `basis` is `cost` when every call in the window was priced and `tokens` otherwise, and `total` is the window total in that unit. `factors` are `subagent_heavy` (root Session trees whose descendants used more than 50% of the tree), `long_running` (Sessions whose first and last call in the window are 8 or more hours apart) and `large_context` (calls whose prompt exceeded 150,000 tokens), each with its `share` (0–1) of the total and its `amount`; they overlap, so they are characteristics, not a breakdown. `subagents` gives each descendant group's share, named by the child's agent, `workflow` for Workflow Run children or `fork` for forked children, highest first. `directory` keeps only Sessions inside that directory's project. The response states its scope (`scope: "server"`, `note: "Covers only Sessions stored by this server"`) and carries `since_ms`, `until_ms`, `total_cost`, `total_tokens`, `steps` and `backfilling`. The thresholds are constants in `crates/cyber-server/src/runtime/usage_insights.rs`; a `workflow` factor is reserved for P2.

`GET /api/v1/usage/limits` (SDK `client.usage.limits()`, operation `v1.usage.limits`) lists the usage windows providers reported in their own responses: `{ limits: [{ provider, window, label, period, used_percent, resets_ms, observed_ms }] }`, the latest reading per provider and window since the server started. The adapters read the plan-window headers `anthropic-ratelimit-unified-<window>-utilization` and `-reset` (for example `5h`, `7d`, `7d_opus`) and `x-codex-<primary|secondary>-used-percent` with `-window-minutes` and `-reset-after-seconds` or `-reset-at`, and the rate-limit headers `anthropic-ratelimit-<requests|tokens|input-tokens|output-tokens>-<limit|remaining|reset>` and `x-ratelimit-<limit|remaining|reset>-<requests|tokens>`, from every response including a `429`. `period` is `daily` for windows of up to a day and `weekly` for longer ones. Nothing is derived from local usage: a provider that sends none of these headers, such as a local OpenAI-compatible gateway, is absent and the list can be empty.

### Usage statistics and `cyber stats`

`GET /api/v1/usage/stats?period=<all|Nd>&directory=<path>&tz=<zone>` (SDK `client.usage.stats(...)`, operation `v1.usage.stats`) summarizes the ledger over local days. `period` is `all` (the default) or `1d` to `371d`, the last N local days with today; anything else is `400`. `tz` is an IANA zone; a missing, unknown or implausible name (path syntax, control bytes) falls back to the server's zone, and the response names the zone used in `zone`. `directory` keeps only Sessions inside that directory's project (its git root, or the directory itself outside git). The response has `days` (one per local day, oldest first, at most the last 371, with zero values for idle days), `totals` (token classes, `total_tokens`, `cost` or `null` when every call was unpriced, `unpriced_steps`, `steps`, `turns`, `sessions`), `favorite_model` (most tokens), `longest_session` (root Session ID, title and the time from its first to its last call in the period), `active_days` (days with a Turn) of `period_days` (for `all`, from the first day with usage), `longest_streak`, `current_streak`, `most_active_day`, `models` (per model: tokens, cost, Turns, Sessions and `share` of the tokens, sorted by tokens), `since_ms` (the period's first local midnight; `null` for `all`), `approximate_days` (part of the period came from folded daily rows, which use UTC days) and `backfilling`. Turns are finished provider steps, Sessions are root Session trees. The current streak counts back from today, or from yesterday while today has no Turn yet.

`cyber stats [--days N] [--project <dir>] [--models] [--tools] [--by origin] [--format json]` asks the background server for that route, so it shows the TUI's numbers: `--days N` is `period=Nd`, `--project` is the project containing `<dir>` (relative to the Location), `--models` adds the per-model table, `--by origin` adds one row per usage origin (`user`, `session` for messages sent by another Session's agent, `client` for messages sent through the Messaging API or `cyber messages send`, `job` for background handbacks, `mcp` for MCP serve input, or the admission source such as `workflow`; the route always returns them as `origins`), and `--tools` adds tool calls by name from the `tool_call` projection, for Sessions active since the period's start. The text output ends its overview with the Stats tab's book comparison line and says `1 day` for one day. `--format json` prints the route's `data` unchanged, plus `tools` with `--tools`. Days are local to the server's zone.

`cyber messages send <to> [text | -] [--from <session>] [--notify-when-idle] [--format json]` sends a cross-session message through the background server as client `cli` (or as `--from`, which `--notify-when-idle` and a `<session>@<peer>` target need); it exits 0 when the message is delivered or held, 1 when it is refused (the reason on stderr) or a peer cannot be reached or refuses the grant (`PeerUnreachableError`, `PeerAuthError`), and 2 for an unknown or ambiguous address (the candidates listed), a peer name no `peers.remotes` entry has (`PeerNotConfiguredError`, with the `cyber peers add` hint) or a text over 16384 bytes. `cyber messages list [--session <s>] [--direction in|out] [--limit N] [--format json]` prints a Session's message history, newest first (the Location's most recent Session by default), and `cyber messages list --reachable [--from <s>]` the Sessions a message can be sent to. `cyber peers` pairs machines for `name@machine` addresses ([direct peers guide](peers.md)).

In the TUI, a received message shows as `✉ <from> · <session> · <time>` marked `untrusted` with its text below, a `send_message` call shows its target, outcome and message ID, and an idle notice is one `◷ <target> is idle · <time> · "<status>"` line. The footer shows `✉ N queued · M held` while messages wait. A held message appears as a card above the composer: Tab on an empty composer selects it, then `r` releases it, `d` drops it and Enter opens it in `/messages`. `/messages` (also in the palette; bind `messages_view` in `tui.keybinds`, unbound by default) lists the Session's messages and idle notices with time, direction, peer, outcome, size and first line; Tab cycles `all`/`in`/`out`/`held`, Enter expands a row to its full text, sha256 and times, `r`/`d` release or drop a held message, and `a` clears a `non_interactive` refusal (the one `cyber exec` sets).

`/agents` (also in the palette; bind `agents_view`, unbound by default) is the directory of every Session and agent this Session can reach right now. It reads `GET /api/v1/messaging/sessions?from=<this Session>` and, when collaboration is enabled and the Session is bound to a project, the hub's People (`/collaboration/status` and `/collaboration/participants`). Rows are grouped as **This server** (subagents, workflow agents, teammates and other Sessions), **Peers** (`name@peer`, and `*@peer` for a peer that did not answer) and **Project hub** (active hub agents and members, other than your own account and the agent this Session is published as). Each row shows a state glyph and word (`●` running, `○` idle, `!` waiting_input, `○` offline, `●` online; a hub member has no presence and shows `·` and `-`), the address to send to, the kind (`subagent`, `workflow agent`, `teammate`, `session`, `peer`, `hub agent`, `hub member`), `open` or `closed` (whether this Session's message would be accepted) and the Location (the directory from `~`, `peer:<name>` or `hub:<project>`; dropped below 100 columns, then the kind below 70). Typing filters on address, name, kind and Location, Tab and Shift+Tab cycle the kind filter (`all`, `sessions`, `subagents`, `teammates`, `peers`, `hub`), and Esc clears the filter, then closes. Enter opens a compose editor that sends from this Session through `POST /messaging/send` (Shift+Enter or Alt+Enter adds a line, Ctrl+N asks for an idle notice, at most 16 KiB) or, on a hub row, as a direct hub message from your account; the outcome (`delivered`, `held` or `refused: <reason>`, with the message ID) is shown, and an ambiguous address lists its candidates and keeps the editor open. While a send or rename is in flight the editor takes no input; Esc closes it and the outcome then arrives as a notice, never in another editor. A title is at most 100 characters. Ctrl+W watches the row (`POST /messaging/watch`), Ctrl+O switches the TUI to a local Session (on a peer row it shows the `cyber attach <session>@<peer>` command), Ctrl+R renames a local Session or subagent (`PATCH /sessions/{id}`), Ctrl+G opens `/messages` on its `held` filter at the newest message held from that row, and Ctrl+L refreshes. An action that does not apply says why and does nothing. The directory refreshes on open, every 10 seconds while open and no editor is, and after each action; the selection follows its row. A failed listing refresh keeps the last rows with a `could not refresh` line, a failed hub read clears the hub rows, and a server that answers 503 shows `messaging is disabled here`. Peer and hub text is untrusted: control and bidi characters are stripped, and a row whose address changes under that cleaning is sent to by its Session ID, or not at all (marked `⚠`). Peer rows never open or rename as local Sessions. The listing itself marks a member of this Session's team as `teammate` (addressed by its member name) and a workflow agent's Session with `workflowRunId`.

### Session recap

`POST /api/v1/sessions/:id/recap` (`v1.session.generateRecap`) returns one or two first-person sentences about what the Session did and what comes next: `{ text, model, through_seq, generated_ms }`. It is written by the `small` model role with no tools and at most 120 output tokens from the newest 16,000 characters of the conversation, trimmed to 400 characters with reasoning tags removed, and stored as a `session.recap.generated.1` event; the call's usage is recorded as `usage.recorded.1` with purpose `recap`, so it counts in the Session's totals and the usage ledger. While the stored recap covers the latest finished Turn (`through_seq`), the route returns it without a model call. A Session with no finished Turn answers `409` with `reason: nothing_to_recap`, and a failed model call answers `503` with `service: "model"` and keeps the previous recap. `GET /api/v1/sessions/:id/recap` (`v1.session.recap`) returns the latest recap or `null`. As with titles, the Session's content goes to the configured small model.

On Linux the sandbox needs bubblewrap (`apt install bubblewrap`); without it commands fail closed unless you pass `--sandbox full-access`.

### Sandbox profiles, environment and escalation

`sandbox.profiles.<name>` defines `{ extends?, policy?, writable_roots?, readable_paths?, deny_read?, network?: { mode, allowed_domains?, denied_domains? }, env? }`. A chain is applied from its root: scalars replace, lists union. The built-ins `read-only`, `workspace` and `full` stand for the three policies. `--sandbox <policy|profile>` (or `cyber sandbox … --profile <name>`), the agent's `sandbox`, then `sandbox.profile` select one; an unknown or cyclic profile fails every sandboxed launch. A repository-controlled layer (project or config profile) can never reach `full-access`, whether through `sandbox.policy`, `sandbox.profile`, a profile's `policy` or an `extends` link. An agent's `sandbox` from such a layer needs workspace trust, and a profile it selects may only tighten the sandbox: one with a weaker policy, more network, more writable or readable paths, more allowed domains or a less masked environment than what applies without it (the user's `sandbox.profile`, else the top-level keys) is ignored with a notice. An OS-managed `policy.sandbox.min_profile` raises a weaker selection and always adds its `deny_read`; when a repository-controlled layer writes any part of the named profile (or a profile it extends), the minimum fails closed to `read-only`.

`deny_read` globs (`**/*.pem`, `~/.aws/**`; relative patterns are anchored at the Location) are enforced by Seatbelt on macOS as path rules and by bubblewrap on Linux by binding over the files that exist at launch (a match then reads as empty; the walk visits up to 1,000,000 entries per pattern and skips `.git/objects`). On macOS a match also cannot be written, renamed or linked, and the directories a pattern names literally (`config` in `config/*.key`) cannot be renamed or removed, so a match cannot be moved out of its pattern and read under a new name. `read` refuses a matching path with `Sandbox denied: read <path>`, `glob` and `grep` skip matches and refuse a `path` that is one. The in-process editors run outside the OS sandbox, so `edit`, `write`, `apply_patch` (each path, and a `Move to` destination) and `notebook_edit` refuse a matching path the same way before reading it.

On Windows, sandboxed commands (shell tools, hooks, LSP, MCP and plugin servers) run through `cyber-sandbox-exec --appcontainer`, which gives each command a fresh less-privileged AppContainer identity. The writable roots (read-only under the `read-only` policy) and `sandbox.readable_paths` are granted to that identity object by object: protected paths stay read-only, and credential files and `deny_read` matches are unreadable. Files the command creates inside a granted directory inherit the grant, so it can keep using them; when the command exits every grant is revoked and the identity deleted (files it created keep an inherited entry naming that deleted, never reused identity). A helper terminated together with its command (a timeout, a cancel, `task_stop`, a host shutdown) cannot clean up, so before granting anything it records the identity and the granted paths in `%LOCALAPPDATA%\cyber\sandbox-grants\<identity>.json`, held open while it runs; the next sandboxed launch revokes every entry of a dead helper's identity under those paths (inherited ones included), deletes its profile and the record. The program's own directory is granted read access unless it is under the Windows or Program Files directories, which AppContainers can already read; a tool that loads files from other directories (for example under `%USERPROFILE%`) fails inside the sandbox. The launch is refused with `SandboxUnavailableError` when a root holds more than 100,000 files and directories or contains a link or junction, when the program is a batch file (`.cmd`, `.bat`), or when the helper is missing. The identity has no network access unless `sandbox.network` is `on`: the allowlist proxy cannot be reached from the container yet, so `proxy` behaves like `off` and raises no domain prompts. A refused write prints `Access to the path '…' is denied.` and is reported as `Sandbox denied: write <path>`. Known limitation ([issue #9](https://github.com/CyberdyneCorp/cybercode-cli/issues/9)): Git Bash (an MSYS2 program) and PowerShell 7 crash at startup inside the AppContainer, so `bash` and `powershell` commands, and PowerShell command hooks in sandboxed scopes, fail under `read-only` and `workspace-write` on Windows; only `full-access` runs them today. The launch, ACL and isolation layers pass native Windows CI; the shell-level tests are skipped on Windows until this is fixed.

`sandbox.env` takes `mode` (`inherit`, the masked server environment, or `none`: only `PATH`, `HOME`, `TMPDIR`, `LANG`, `TERM`), `set` (added to every sandboxed process), `allow` and `deny` (name patterns applied after masking; `deny` wins) and `login_shell` (`bash -l`). The `CYBER*` markers and, in proxy mode, the proxy variables are always set.

When the OS refuses a sandboxed command, the result ends with `Sandbox denied: <read|write|network> <target>` and the denial is stored as `sandbox.denied.1` `{ session_id, call_id, operation, target }`. The model may call `bash` again with `sandbox: "escalate"` and a `justification`; that raises a `sandbox.escalate` request showing both, and an approval runs that one call unsandboxed (`sandbox.escalated.1`). The sandbox is a ceiling: `bypass` still asks for an escalation and for a domain outside the allowlist, `dont-ask` denies and `auto` classifies. Only an explicit `sandbox.escalate` or `network` allow rule from a user, global, command-line or managed layer, or a domain a person approved `always`, crosses it without asking; a wildcard action (`"permissions": "allow"`) or a repository, agent or Session rule does not. With [OpenTelemetry export](#opentelemetry-export) on, denials and escalations are `sandbox.denied` and `sandbox.escalated` events on the tool span (the target only with `telemetry.log_prompts`).

Nobody answers a prompt in `cyber exec`, so a request its Session still raises (a sandbox ceiling such as `sandbox.escalate` or an unlisted domain in `--yolo`, or any ask in `--mode default`) is rejected at once with `Rejected by user: no interactive approver is attached to cyber exec`. The model reads the denial, the Turn continues, and the run lists it under `denials` (exit 5 with `--fail-on-deny`).

`sandbox.excluded_commands` (`["docker *", "gh auth *"]`, from user-controlled layers only) lets a single literal command run outside the sandbox; such a call is at least `ask` in every Mode, whatever the rules or saved approvals say. `bypass` runs it without asking only inside a detected container or with `--dangerously-bypass-permissions`: when the sandbox alone made `bypass` eligible, leaving it still needs a person. Pipelines, lists and expansions never qualify.

`cyber sandbox explain [--policy <p> | --profile <name>]` prints the effective profile, policy, enforcement, isolation, writable and read-only roots, unreadable paths, `deny_read`, network mode, allowed and denied domains, the environment mode and the names (never values) of masked variables. `cyber sandbox test` runs read, write and network probes and exits 1 if one does not match the policy. `cyber sandbox run -- <cmd>` runs one command under the effective policy (domains outside the allowlist are refused, nothing asks) and exits with its status.

`bypass` is admitted only when commands run in the OS sandbox with `workspace-write` or stricter (a sandbox with `network: on`, or a writable root that is relative or contains the home directory, does not count), inside a detected container or VM (`/.dockerenv`, `/run/.containerenv`, a container cgroup, `CYBER_CONTAINER=1`, `REMOTE_CONTAINERS`, `DEVCONTAINER`, `CODESPACES=true`; Flatpak, toolbox and distrobox share the user's home and the host, so they count only with `CYBER_CONTAINER=1`), or with `--dangerously-bypass-permissions` on `cyber serve`, an embedded TUI or `cyber exec --embedded|--ephemeral`. `--dangerously-skip-permissions` (Claude Code's name) on `cyber exec` and the TUI is that flag plus `--mode bypass`, with the same `--embedded|--ephemeral` requirement: on the shared server it exits 2 and names them (use `--mode bypass` there when its sandbox or a container already admits `bypass`), and it conflicts with `--auto` or another `--mode`. Otherwise creating or switching a Session to `bypass` (including `exec --yolo`, an agent's `permission_mode` applied without an explicit `mode`, and switching to an agent whose own sandbox does not admit it) fails with `bypass mode requires a sandbox, a container, or --dangerously-bypass-permissions`. `/health` reports `sandbox: { policy, profile, enforcement, isolation }` with `isolation` `sandbox`, `container` or `none`. An OS-managed `policy.modes.disable` (for example `["bypass"]`) refuses those Modes with `<mode> mode is disabled by your organization` wherever a Session takes one: creation (also from an agent's `permission_mode`), the deferred agent default at the first Turn, worktree starts and every switch, including `plan_exit`; a malformed managed document disables `bypass`. Every Turn admits the Session's Mode again before the model or any tool runs, so a Session stored in a Mode the server no longer admits (restarted without the flag, a looser sandbox, a newly disabled Mode) fails its Turns with the refusal until it switches to an admitted Mode.

`cyber permissions test [--agent a] [--mode m] (--tool <name> --resource <r> | -- <shell command>)` and `GET /api/v1/permissions/test?agent&mode&tool&resource&command` evaluate the effective ruleset as a Session would (each tool under its action: `write`, `apply_patch` and `notebook_edit` as `edit`, `powershell`, `monitor` and `pty_*` as `bash` commands, `send_file` as `read`; bash command splitting, protected paths, critical removals, saved approvals, excluded commands and the Mode) without creating a Session or running anything: `npm test → allow (saved approval, project)`, `git push → ask (default rules)`.

## OpenTelemetry export

The background server can export traces, metrics and logs over OTLP to a collector you run. Export is off by default. Only the background server exports; its clients (the TUI, `cyber exec`, `cyber api`) never do, so each Session is exported once. In-process runs are never exported, even with export on: `cyber exec --embedded` and `--ephemeral`, `cyber tui --embedded`, `cyber serve --stdio`, `cyber eval` and skill commands.

### Settings

Set these in global configuration (`~/.config/cyber/cyber.jsonc`, or `CYBER_CONFIG` / `CYBER_CONFIG_CONTENT`):

```jsonc
{
  "telemetry": {
    "otel": {
      "enabled": true,                          // default false
      "endpoint": "https://otel.example.com",   // a base URL
      "protocol": "http/protobuf",              // or "http/json", "grpc"
      "headers": { "authorization": "Bearer {env:OTEL_TOKEN}" }
    },
    "log_prompts": false                        // default false
  }
}
```

A project layer's `telemetry.otel` and `telemetry.log_prompts` are never read, even in a trusted checkout, so a repository cannot send your Sessions to its own collector or turn on content capture. `cyber telemetry show` lists such values as ignored. An invalid telemetry value keeps export off; it never makes the rest of the configuration invalid or a Turn fail.

Defaults: protocol `http/protobuf`, endpoint `http://localhost:4318` (`http://localhost:4317` for `grpc`), a 10 s timeout and a 60 s metric interval (`OTEL_METRIC_EXPORT_INTERVAL` changes it). For HTTP, `/v1/traces`, `/v1/metrics` or `/v1/logs` is appended to a configured or generic endpoint; a per-signal endpoint is used as given.

### Precedence

Settings are resolved once at server start; run `cyber service restart` after a change (`show` reports `restart required` until then). Highest first:

1. **Off switches.** `CYBER_DISABLE_TELEMETRY` (truthy) or `OTEL_SDK_DISABLED=true` turns export off.
2. **Managed policy.** A managed policy document that is present but malformed turns export off. There is no managed telemetry key yet; when org policy adds one it will win over every user setting.
3. **Enablement.** Export is on only when `telemetry.otel.enabled` is `true`. **No `OTEL_*` variable enables export**: an `OTEL_EXPORTER_OTLP_ENDPOINT` set for other programs is not picked up until you enable export yourself, and `show` then reports `telemetry.otel.enabled is not true`.
4. **Values.** A per-signal variable (`OTEL_EXPORTER_OTLP_{TRACES,METRICS,LOGS}_{ENDPOINT,PROTOCOL,HEADERS,TIMEOUT}`) beats the generic `OTEL_EXPORTER_OTLP_*` variable, which beats `telemetry.otel.*`, which beats the default. A headers variable (URL-encoded `name=value` pairs) replaces the configured header map as a whole. Timeouts are milliseconds.
5. **Per-signal switch.** `OTEL_{TRACES,METRICS,LOGS}_EXPORTER=none` turns one signal off; any value other than `otlp` is reported as unsupported and keeps that signal off.

These keep export off, with the reason in `show` and one `warn` log line, while the server still starts: an endpoint that is not an absolute `http`/`https` URL, an unknown protocol, a non-string header value, `grpc` in a build without the `otlp-grpc` feature (release builds include it), and headers with a plain `http://` endpoint whose host is not loopback (header values are credentials and are never sent in clear text off the machine).

The resource has `service.name=cyber` (`OTEL_SERVICE_NAME` overrides it), `service.version`, `cyber.build`, `os.type`, and `cyber.account.sub` and `cyber.account.org_id` when a Cyber Account is signed in at server start. `OTEL_RESOURCE_ATTRIBUTES` adds attributes but cannot replace `service.version`, `os.type` or any `cyber.*` key. TLS client certificates and custom CA variables (`OTEL_EXPORTER_OTLP_CERTIFICATE` and related) are not read; the system roots are used.

### Checking the settings

`cyber telemetry show [--format json]` (or `GET /api/v1/telemetry`, SDK `client.telemetry.get()`) asks the background server, whose environment is the one that exports. It prints whether export is on and why not, `log_prompts` with its source, each signal's endpoint, protocol, headers and timeout with the source of each value (`config (<layer>)`, `env (<VARIABLE>)`, `policy (<managed>)` or `default`), the `exported`, `failed` and `dropped` counters with the last error, `dropped events`, the resource (the account only as `account signed in: yes|no`), the ignored project values and product telemetry (always off). Header values are always shown as `***`, and the endpoint without userinfo or query.

### Spans

Each span ends from the event that completes its work. Every span carries `cyber.session.id`.

| Span | From → to | Parent |
|---|---|---|
| `cyber.workflow.run` | `workflow.run.started.1` → completed, failed or stopped | none: the root of its own trace, linked to the owner Session's open Turn |
| `cyber.workflow.agent` | `workflow.agent.started.1` → `workflow.agent.settled.1` | the run span |
| `cyber.session` | the first Turn, or a hook that runs before it → `Idle` (one activity period, not the Session's lifetime) | a workflow child Session: its agent span; a subagent: the parent's open Turn span; a root Session: none |
| `cyber.turn` | a prompt promotion (or a step with no open Turn) → the next promotion or `Idle` | the session span |
| `cyber.tool` | `session.tool.called.1` → `session.tool.settled.1` | the open Turn span |
| `cyber.hook` | the hook receipt's start → its settlement | the tool call, else the Turn, else the session span. A hook that runs while the Session is idle (`UserPromptSubmit`, `SessionStart`, a configuration hook) opens the session span; the following Turn joins it, or it ends with the hook when no Turn follows |

Attributes: Turns carry `cyber.agent`, `cyber.mode`, `cyber.model`, `cyber.tokens.<class>`, `cyber.cost.usd` (absent when unpriced) and `cyber.finish_reason`, and a failed Turn has an `Error` status. Tool spans carry `cyber.tool.name`, `cyber.call.id`, `cyber.tool.attempt` and `cyber.tool.outcome`, with an `Error` status when the call failed or was denied. Hook spans carry `cyber.hook.id`, `cyber.hook.event`, `cyber.hook.scope`, `cyber.hook.outcome` and `cyber.hook.decision`. Run and agent spans carry `cyber.workflow.run_id`, `cyber.workflow.name`, `cyber.workflow.status`, `cyber.workflow.agent_id` and `cyber.workflow.phase`. Span events: `permission.decision` (`decision`, `source`), `sandbox.denied` and `sandbox.escalated` (`operation`) on the tool span, and `retry` on the Turn span.

### Metrics

Every metric is a cumulative counter recorded when work settles. Session, call and run IDs are never metric attributes.

| Metric | Unit | Attributes |
|---|---|---|
| `cyber.session.count` | `{session}` | `cyber.session.kind` (`root`, `subagent`, `workflow_agent`) |
| `cyber.turn.count` | `{turn}` | `outcome` (`completed`, `failed`, `interrupted`), `cyber.agent` |
| `cyber.token.usage` | `{token}` | `type` (`input`, `output`, `reasoning`, `cache_read`, `cache_write`), `cyber.model`, `cyber.provider` |
| `cyber.cost.usage` | `USD` | `cyber.model`; unpriced steps add nothing |
| `cyber.tool.calls` | `{call}` | `cyber.tool.name`, `outcome` |
| `cyber.tool.errors` | `{call}` | `cyber.tool.name` (calls settled with status `error`, denials included) |
| `cyber.permission.decisions` | `{decision}` | `decision` (`allow`, `deny`), `source` (`user`, `auto`), `cyber.permission.action` |
| `cyber.active_time` | `s` | `cyber.session.kind` (Turn wall time) |

Tokens and cost cover every billed model call, as `cyber usage` counts them: steps, generated titles, compactions, auto-mode reviews and auxiliary calls. Decisions a rule settles without a request are not counted, nor are requests the runtime withdraws when a Turn is stopped or a call abandoned.

### Logs

Every structured log record that passes the level filter is also exported as an OTLP log record: severity, `component`, and the identifier fields `session_id`, `request_id`, `run_id`, `call_id`, `kind`, `status` and `pid`. The message is content, because many messages quote file paths and error text: it becomes the record's body only with `log_prompts`, redacted and capped like other content, and a record has no body otherwise. Other scalar fields are exported only with `log_prompts`, redacted; objects and arrays never are. The OpenTelemetry SDK's own logs are disabled.

### Privacy

By default nothing is exported that holds content: no prompt text, model output, tool input or output, Session title, directory, sandbox target, log message text or non-identifier log field. Tool spans carry the tool name, duration and outcome only. Hook stdin and stdout are never exported, even with `telemetry.log_hook_io`.

With `telemetry.log_prompts: true` in global configuration, Turns gain `cyber.prompt` and `cyber.response`, tool spans `cyber.tool.input` and `cyber.tool.output`, sandbox events `target`, and Sessions `cyber.session.title` and `cyber.directory`. Every value is redacted before export: structured values by key (`authorization`, `token`, any `headers` value, …), text by the credential patterns (`sk-…`, `ghp_…`, …) and the configured header values matched exactly, each replaced by `***`. Values are capped at 16 KiB and end with `…[truncated]` when cut. Header values never appear in an attribute, a log record, an error message or `show`, whatever `log_prompts` says.

### Delivery

Export never blocks or fails a Turn: the exporter reads the runtime's event stream and only makes in-memory SDK calls; batches go out through bounded queues (2048 items, batches of 512, honoring `OTEL_BSP_*` and `OTEL_BLRP_*`). Events the exporter could not keep up with are counted as `dropped events`; items in batches the collector rejected or never received count as `failed` per signal. An unreachable collector is reported once, as one `telemetry export failing` warn line with the masked endpoint and the redacted error and as `last_error` in `show`, and once more as `telemetry export recovered` when it answers again; the other signals' failures show only in their counters, their `last_error` and the final `telemetry export stopped` line. A signal's counters move only when one of its batches is attempted: traces and logs after their batch delay (the SDK defaults: 5 s for spans, 1 s for logs) plus up to the export timeout, metrics at the next interval or at shutdown, so `show` can report `failed 0` for a while after a Turn. The SDK's own queue overflow has no counter and is not included.

The exporter never follows a redirect: a collector that answers 3xx counts as a failed export, so the export and its headers never reach a host that was not configured. When the SDK cannot build an exporter from the resolved settings (an endpoint its URI parser rejects, such as a non-ASCII host), export stays off and `show` reports `the traces exporter could not be built: …` as the reason. The SDK reads `OTEL_EXPORTER_OTLP_COMPRESSION` itself; this build has no compression, so the SDK ignores the variable with a warning and exports uncompressed.

At shutdown the server ends every open span with `cyber.interrupted=true`, flushes all three signals in parallel and gives up after 3 s in total, then logs `telemetry export stopped` with the final counters (to the file only). What was not exported by then is dropped.

### Restart behavior

Spans are built from live events only and never from stored history. Stopping the server settles a running tool call as interrupted, and its span ends with `cyber.interrupted=true`; the call is counted once, by the server that stopped. After a server restart, work that started before it gets no span when it settles: the old server already exported its span as interrupted, and the new one records only its metrics (a tool call settles with `cyber.tool.name=unknown`). The exporter subscribes before the restart recovery runs, so what the recovery settles or resumes (`workflows.resume_after_restart`) is counted even before the server listens. A Session created before the restart is read once from the database for its parenting; when its parent's spans ended with the old server, its spans start a new trace.

### Local collector recipe

Run the OpenTelemetry Collector with a debug exporter and point Cyber at it:

```yaml
# otel-collector.yaml
receivers:
  otlp:
    protocols:
      http: { endpoint: 0.0.0.0:4318 }
      grpc: { endpoint: 0.0.0.0:4317 }
exporters:
  debug: { verbosity: detailed }
service:
  pipelines:
    traces:  { receivers: [otlp], exporters: [debug] }
    metrics: { receivers: [otlp], exporters: [debug] }
    logs:    { receivers: [otlp], exporters: [debug] }
```

```bash
docker run --rm -p 4317:4317 -p 4318:4318 \
  -v "$PWD/otel-collector.yaml:/etc/otelcol/config.yaml" otel/opentelemetry-collector:latest
# in ~/.config/cyber/cyber.jsonc: "telemetry": { "otel": { "enabled": true } }
cyber service restart
cyber telemetry show          # export on, endpoints http://localhost:4318/v1/…
cyber exec "list the files"   # spans and metrics appear in the collector output
```

Metrics arrive at the next 60 s interval or when the server stops. For a quick check set `OTEL_METRIC_EXPORT_INTERVAL=5000` in the server's environment.

## TUI themes and color

Every theme resolves to the semantic tokens of `tui` → Theme tokens (surfaces, text, roles, `diff_*`, `md_*`, `syntax_*`, `heat_*`, meters). The 18 built-in themes (`cyber`, `system`, `dracula`, `tokyonight`, `catppuccin`, `gruvbox`, `nord`, `one-dark`, `github-light`, `solarized-light`, `monokai`, `everforest`, `cyber-light`, `synthwave-84`, `rose-pine`, `kanagawa`, `ayu-mirage`, `matrix`) are compiled-in JSON documents in `crates/cyber-tui/themes/`, each meeting 4.5:1 for text and 3:1 for muted text and focus.

`/theme`, `<leader>s` (Ctrl+X s), the palette's `Theme…` and the Config tab's `theme` row open the theme switcher. The left pane lists the themes under `DARK` and `LIGHT` (by their background), each with a swatch of its accent, secondary, success, warning and error colors on its own background and `current` or `custom` tags; Tab cycles the filter `All · Dark · Light` and typing searches. The right pane previews the highlighted theme: the logo gradient, Markdown, a highlighted code block, diff lines, status glyphs, a Turn marker and the measured contrast with its WCAG level (`low contrast` below 4.5:1). Moving the selection applies the theme everywhere at once; Enter keeps it in `kv.json`, `p` (with an empty search) writes `"theme"` to the project root's `.cyber/tui.jsonc` (a file with comments is never rewritten: set it by hand), and Esc restores the theme the switcher opened with. `<leader>t` (Ctrl+X t, `theme_cycle`) moves to the next theme in the switcher's filter without opening it, keeps it, and shows `Theme: <name>` in the status footer for 2 seconds. Under `NO_COLOR` the switcher lists the themes and explains that colors are disabled. Custom themes are `themes/<name>.json` in the global config directory or in `.cyber/` at the project root or the Location; the nearer file wins over a farther one or a built-in of the same name:

```json
{
  "defs": { "ink": "#e0e0e0" },
  "theme": {
    "text": "ink",
    "background": { "dark": "#101010", "light": "#ffffff" },
    "accent": { "dark": "#ff8800", "light": "#b35900" }
  }
}
```

A value is `#rrggbb`, a `defs` name, one of the 16 ANSI names (`red`, `darkgray`, …), `none` for the terminal's own color, or `{ "dark", "light" }`. Missing tokens are derived from `text`, `background` and `accent` (for example `surface` is the background 6% toward the text). Unknown tokens, invalid values and unreadable files are skipped and reported in a toast.

`tui.jsonc` (same directories, nearer wins) sets `theme` (used when `kv.json` has none), `theme_mode` (`auto`, `dark`, `light`), `color` (`auto`, `truecolor`, `256`, `16`, `none`) and `accessibility.reduced_motion`. A non-empty `NO_COLOR` wins over `color` and removes every color: selection is reverse video. In `auto` mode, two-variant themes such as `cyber` follow the terminal background from `COLORFGBG` and an OSC 11 reply (Unix only), without delaying the first frame.

## TUI Markdown and code

Assistant text renders as CommonMark with GitHub tables, task lists and strikethrough: headings, emphasis, numbered and nested lists (`☐`/`☑` task items), `▎` quotes, box-drawn tables (a table wider than the transcript becomes `header: value` lines), rules and links. Wrapped list items and quotes keep their hanging indent. While a message streams, only its last unfinished block is parsed again. Raw HTML stays literal text, and control characters in model output are never written to the terminal. Session titles get the same treatment: the server drops control characters from model-written titles (tabs become spaces), and the TUI strips them from any title, including one set through the API, before it sets the terminal window title or prints the exit summary.

Fenced code blocks, and code in `read` and `write` tool cards, are highlighted with syntect (pure-Rust fancy-regex engine, the Sublime default syntaxes plus two-face's extras such as TypeScript, TOML and Dockerfile). Scopes map onto the theme's `syntax_*` tokens, so every theme, `NO_COLOR` and the 256/16-color depths apply. The language comes from the fence (`py`, `ts`, `sh`, `rs`, … are aliases) or the file extension; unknown languages render plain. A block highlights only once its closing fence arrives, without changing its height, and blocks over 2,000 lines or with a line over 1,000 characters stay plain. The syntax set loads on its own thread the first time a finished block needs it, so startup is unchanged.

`Ctrl+X y` selects the last code block (`↑`/`↓` move, `Esc` leaves); `y` or `Enter` copies it without padding or markers through `pbcopy`, `wl-copy`, `xclip` or `xsel`, or through an OSC 52 sequence when none is available (over SSH, on Windows, or when the tool fails). Copies hold only what was drawn: control characters (except newline and tab) and bidi controls are removed first, so a hidden ESC or bracketed-paste end marker cannot run a command when pasted. Links show their URL after the text unless the terminal is known to support OSC 8 hyperlinks (iTerm2, WezTerm, kitty, Ghostty, foot, Alacritty, VS Code, Windows Terminal, VTE ≥ 0.50; never inside tmux or screen); only `http`, `https` and `mailto` URLs of printable ASCII are ever written inside an OSC 8 sequence.

## TUI composer, footer and palette

The fullscreen TUI captures the mouse: the wheel scrolls the transcript (3 lines per notch), or moves through the open panel's list, and prompts and confirmations ignore it. The wheel acts on the pane under the pointer, so it does nothing over the sidebar. Left clicks do only what a key already does in that place. A row of a list (a picker, the palette, the theme switcher, `/workflows`, `/messages`, `/agents`, `/team`, `/tree`, the Settings Config rows, a permission or question option) is selected by the first click and activated as Enter by a second click on it, so a double click selects and opens; any key press in between disarms it, and so does a list that reloads or reorders between the clicks, so the second click only confirms the item the first one selected. A list longer than its panel keeps its scroll position until the selection leaves it, so a clicked row stays under the pointer. A permission or question prompt is never answered by a single click, and the Allow always confirmation, the rejection feedback, the blocked-call approval and the yes/no confirmations take no clicks at all. Controls act at once: a Settings tab, a key chip, a sidebar section header, the explorer's tree rows and `rendered`/`raw` chip, and the hidden-lines and `thinking` rows of the transcript (as Ctrl+O and Ctrl+T); a double click on a control acts once. Presses that follow a click within half a second, on a panel or prompt that click opened, are ignored, so a fast triple click never answers the prompt its first press raised. A click on the transcript or the composer, including its hidden-lines and `thinking` rows, takes focus back from the sidebar. While a panel, prompt or overlay is open, a click outside it does nothing and never closes it. Releases, drags, the right and middle buttons and clicks with a modifier are ignored, so to select text with the terminal's own selection, drag with a modifier (Shift in most terminals, Option in iTerm2). Set `"mouse": "off"` in `tui.jsonc` (or in Settings → Config) to leave the mouse to the terminal: the TUI then handles neither the wheel nor clicks. Screen-reader mode handles no clicks, and the wheel still scrolls there.

The composer is a `surface` block with a `┃` bar on the left: the agent's configured `color` (or `accent`) for prompts, `warning` for `!` shell input and `info` for `/` commands. It grows from 1 to 8 rows, then scrolls. Its last row reads `<Agent> · <model> <provider> · <Mode>` (plus the variant).

While the Session works (a Turn, a compaction or a running workflow of the Session), a progress line above the composer shows the indicator chosen by `tui.spinner`: `nyan` (the default) draws a rainbow trail of `▀` half-blocks, two bands per cell, then the cat `~[,,_,,]:3` and `Thinking · 12.4s · esc interrupt` (`compacting · 3.1s` while compacting). With unknown progress the trail fills the width and its hues move one cell per frame; with known progress (a workflow run's agents done over total; transfer byte progress is supported by the indicator, but no client transfer reports bytes yet) the trail covers the fraction, the rest is `·`, and the label reads `fanout 7/10 agents · 70%`. `nyan-tall` uses four rows (the six bands with every other pair of columns half a row lower, and the four-row cat) on terminals of 30 or more rows, else `nyan`; `braille` and `dots` draw one glyph; `off` draws no line and leaves `esc interrupt` on the composer's last row. Frames advance at most 12 times a second, only while working, focused and on screen (not behind the Settings panel or the file explorer). With 16 colors the bands use the nearest ANSI colors, under `NO_COLOR` the trail is `=-=-`, and with reduced motion the indicator is the static `[,,_,,]:3` with no frames at all. Screen-reader mode prints `Working, 20 seconds` instead, updated every ten seconds and when the activity changes. Workflow names are cleaned of control characters. The status footer shows the Location on the left (shortened in the middle; a toast takes its place for 6 seconds) and, on the right, the context meter (`29.5K (22%)`, in the warning color above 85%), the cost of the Session tree (`$0.00`, `—` when nothing was priced, `≥ $x` when part was not) and `ctrl+p commands`. While workflow runs of the Location are active (running, queued, or paused with agents still settling), the right side also starts with `⚙ 2 workflows · 1 queued` (the `workflows` field, on by default). Choose the fields with `tui.statusline.fields` (`cwd`, `workflows`, `context`, `cost`, `model`, `agent`, `mode`, `variant`); fields with no data are left out. Below 120 columns a header row shows the Session title.

Each finished run ends with a marker, `▣ Build · <model> · 11.3s`, built from the durable step times the server returns (`started_ms`, `ended_ms`, `agent`, `error_kind` on assistant messages and `promoted_ms` on user messages), so it reads the same after a reload or a restart; interrupted and failed runs add `· interrupted` or `· failed`, and `/timestamps` adds the end time. `GET /api/v1/sessions/:id` and the `session.usage` event carry `context: { used_tokens, window, compact_at, utilization }` for the last finished step (unknown windows are `null`, never 0).

`Ctrl+P` (or `/` then `Tab`) opens the command palette: UI actions (usage, theme, copy the last code block, reasoning, tool output, timestamps), the built-in commands, and the server's custom commands, skills and MCP prompts, each with its current key. Keys come from one table that `tui.keybinds` in `tui.jsonc` merges over: top-level names bind global actions (`command_palette`, `settings`, `model_list`, `session_list`, `session_new`, `theme_list` (`<leader>s`), `theme_cycle` (`<leader>t`), `session_fork`, `copy_code`, `help`, `reasoning_toggle`, `tool_details`, `leader`), and the `composer` (`history_previous` Up, `history_next` Down, `newline` Ctrl+J, `editor` Ctrl+G), `palette`, `sidebar`, `settings` and `explorer` objects scope bindings to a context; any other context is reported as unknown. With `history_previous` set to `"none"`, Up only moves the cursor. `"none"` disables a binding, which then disappears from the footer, help and palette; a value that does not parse keeps the default and is reported.

```jsonc
// tui.jsonc in the global config directory or a .cyber/ directory
{ "keybinds": { "command_palette": "ctrl+k", "help": "none", "palette": { "palette_close": "ctrl+g" } },
  "statusline": { "fields": ["cwd", "agent", "context", "cost"] } }
```

## Large paste and image input

**Large pastes.** A paste over 1,000 characters or 25 lines becomes a `[Pasted text #1 +300 lines]` chip in the composer; the full text is sent on submit. Set `"paste_summary": false` in `tui.jsonc` to insert the text as is.

**Images and PDFs.** PNG, JPEG, GIF and WebP images become `[Image #N]` chips and PDFs `[PDF #N]` chips. They come from three places:

- **Clipboard:** terminals do not send image bytes, so `composer.paste_image` (Ctrl+V; Alt+V on Windows, where Ctrl+V is the terminal's own paste) reads the system clipboard with a fixed command and no shell: `osascript` on macOS, `wl-paste` under Wayland or `xclip` under X11 on Linux, and PowerShell on Windows. A missing tool or an empty clipboard shows a toast (`Clipboard image needs wl-paste or xclip`, `No image on the clipboard`); text paste is never affected. Rebind it under `keybinds.composer`.
- **Drag and drop:** a paste that is only paths of existing image or PDF files (quoted, backslash-escaped or `file://` URLs) becomes chips instead of text.
- **`@` mentions:** an image or PDF picked from the `@` menu inserts a chip; a typed `@shot.png` token is attached too.

Numbers count per kind within a draft. Backspace right after a chip (or Delete right before it) removes the whole chip, and editing a label by hand turns it back into text. Drafts keep their chips across restarts; a clipboard image waits as a private `0600` file in the TUI state directory and is removed when the draft is sent or cleared. A prompt the server refuses keeps the draft and its chips and shows the server's message. The transcript shows `[Image: shot.png, 1.2 MB]` or `[PDF: spec.pdf, 340 KB]`; images are not drawn in the terminal.

**How they reach the model.** When the server runs on your machine, a path chip is sent as a `file` part, and the server reads it under the `read` permission, checkout trust and the hidden-path sandbox, as the `read` tool would. A clipboard image, and any chip when the TUI is attached to a peer, is sent inline. The server sniffs the type from the bytes (other files are refused with `UnsupportedAttachmentError` 415), refuses anything over `attachments.max_bytes` (default 20 MiB) with `AttachmentTooLargeError` 413 (`Attachment exceeds 20 MiB`), and refuses an image or PDF the Session's model cannot take (`capabilities.vision`, `capabilities.pdf`) with `UnsupportedModalityError` 422, before anything is admitted. Images over `attachments.image.max_dimension` (2000 px) are downscaled, and re-encoded as JPEG when still over `attachments.image.max_bytes` (5 MiB); `attachments.keep_original` keeps the original too. A PDF over `attachments.pdf.max_pages` (100) gets a page note. Each attachment is stored once as `<data>/attachments/<session_id>/<sha256>`, and events hold only the reference. If you later switch to a model without that input, its history shows `[image/png omitted: model has no image input]`.

**`cyber exec --file`.** An image or PDF, judged by its bytes, is sent as a `file` part with its absolute path when the server is local (the background service, `--embedded` or `--ephemeral`). Exec runs in `dont-ask` mode by default, so a file outside the Location is denied: the run exits 2 with `Hint: add --add-dir <dir>`. With `--attach <url>` the CLI sends the bytes inline. Text files are attached as text, as before. Any refusal (size, type, model input or permission) exits 2 with the server's message before a Turn runs:

```bash
cyber exec --file shot.png "what is wrong in this UI?"
# with a text-only model:
# Error: UnsupportedModalityError: corp/text has no image input; switch models with /model or remove the attachment
```

**Export.** A JSON export embeds each attachment once in its top-level `attachments` array, so `cyber sessions import` restores it; `--sanitize` drops the bytes and redacts the names, and Markdown and HTML show `[image: image/png, 1.2 MiB, shot.png]` placeholders (see [Naming and resuming Sessions](#naming-and-resuming-sessions)).

## TUI sidebar, layout and welcome screen

At 120 columns or more a sidebar docks on the right (30% of the width, 32 to 48 columns, on `surface_alt`). From the top it shows the Session title (and the parent's for a child), **Context** (a bar of the context window used, green below 70%, amber from 70% and red from the auto-compaction point, with a `│` marker at that point; then `used / window tokens`, `│ auto-compact at N%`, `auto-compact pending`, `compacting…` or `auto-compact off`, and the cost spent; before the first reply `0 / <window> tokens · <model>`, and `N tokens · window unknown` without a bar when the catalog has no window), **MCP** (one row per configured server with a dot and a word: `Connected`, `Connecting`, `Failed` with its error on the next row, `Needs auth`, `Disabled`), **LSP** (each server's state, or `LSPs will activate as files are read`) and **Modified files** (the Session's changes with `+adds -dels`, ten rows and then `+N more`), and at the bottom `<cwd>:<branch>` (home as `~`, a linked worktree's name in brackets) and a connection dot with `Cyber Code <version>`. Every value comes from a public route (`/mcp`, `/lsp`, `/vcs`, `/health`, `GET /sessions/<id>/diff` without `message_id` for the whole Session); a section whose route answers 404 or 503 is left out. MCP rows follow `mcp.status.changed.1` and `mcp.tools.changed.1`, Modified files follow `session.diff.1`, LSP and the branch are re-read when a Turn ends, and everything is re-read when the event stream reconnects (the TUI reopens a dropped stream every two seconds; the dot turns yellow while it retries and red when an attempt fails).

Below LSP, a collapsible **Machine** section shows the machine the server runs on, from `GET /api/v1/system`: `CPU` (a busy bar and `N cores · X% idle`; `measuring` until the server has two CPU readings), `Mem` (used bar, `used / total GB · free`), `Disk` (used bar for the Location's volume) and `load a b c` where the OS reports it, with the OS label in the header. Bars are eighth-block glyphs, green below 70%, amber to 90% and red from 90%; under `NO_COLOR` they are `[#####.....]` text. The TUI reads the route every 3 seconds, and only while the sidebar is drawn on a focused terminal; `tui.machine_stats: off` (also in the Config tab) hides the section and stops the reads. A failed read keeps the last sample (`—` before the first) without a toast. The Status tab shows the same numbers as one `Machine:` sentence, which is the screen-reader text. The server keeps one sampler (the `sysinfo` crate, CPU, memory and one disk only, never processes) and refreshes it at most every 2 seconds: measured on a debug build, the first sample takes about 114 ms (core count and the disk list, once), a refresh 0.2 ms and a cached read under 1 µs.

`tui.sidebar` in `tui.jsonc` is `auto` (default) or `show` (both dock at 120+ columns) or `hide`. `<leader>b` (Ctrl+X b) or `/sidebar` shows or hides it; from 60 to 119 columns the sidebar is not docked, a one-row header shows the title and the run state, and `<leader>b` opens it as a drawer over the right of the transcript, closed by Esc. `<leader>e` moves focus into the sidebar (opening the drawer when needed): Up/Down (or j/k) move between rows, Enter or Space collapses MCP or Modified files, Enter on a file opens its diff, `r` on a failed or `needs_auth` server reconnects it through `POST /api/v1/mcp/{name}/reconnect`, and Esc returns to the composer. The `sidebar` keybinding context rebinds these keys. From 40 to 59 columns the status footer shows only the context percent and the cost; below 40×10 the TUI shows `Terminal too small (40×10 minimum)`. Long paths are shortened in the middle (`~/work/…/src/main.rs`). Screen-reader mode never draws the sidebar.

`POST /api/v1/mcp/{name}/reconnect` now also re-admits a server that settled `failed` after its launch or initialization failed, as long as it retained no native owner; a failure whose ownership is unverified (for example a missing executable) stays refused with 409.

`GET /api/v1/vcs` returns the Location's git state, `{ "vcs": { "root", "branch", "head", "upstream", "ahead", "behind", "worktree", "dirty" } }`, read with libgit2 (no `git` process or hook runs), and `{ "vcs": null }` outside a repository. The status footer fields `branch` and `worktree` use it.

An empty Session shows a welcome screen in the transcript area: the `CYBER CODE` logo (the wide one from 82 columns, the two-row compact one from 45, else bold `Cyber Code`) in a gradient from `accent` to `secondary` (shadows dimmed; `accent` only with 16 colors, no color under `NO_COLOR`), the version and `local-first coding agent`, a panel with the agent, model, Mode, `cwd:branch`, the MCP summary and the active language servers, and tips generated from the key table. When the logo and the panel do not fit, the logo goes first, then the tips. `tui.welcome` is `full` (default), `compact` (no logo) or `off`. It disappears with the first message and is never part of the transcript; screen readers get `Cyber Code <version>` and the panel as text.

## TUI Settings panel

`/settings` (or `<leader>,`, or the palette's `Settings`) opens a full-screen panel with the tabs `Status`, `Config`, `Usage` and `Stats` on the last tab used; `/status` and `/config` open their own tab. Tab and Shift+Tab or Left and Right switch tabs, `1`–`4` jump to one, Up/Down, PageUp/PageDown, Home and End scroll, `r` loads the tab again and Esc closes; the bottom row lists the tab's keys from the binding table (the `settings` context rebinds `settings_close`, `settings_next`, `settings_previous` and `settings_refresh`). A tab shows `Loading…` until its data arrives and `Could not load <section>: <error> · r to retry` when a source fails, without closing. The Drain keeps running meanwhile, and a permission or question request closes the panel and shows its prompt.

**Status** reads `/health`, the Session, `/vcs`, `/mcp` and `/lsp`: the client and server versions (both in the warning color with `restart the service` when they differ), the Session title and ID, the Location, the branch (with `uncommitted changes`, or `not a git repository`), the agent, the model with its provider and variant, the Mode, the context meter as a 30-column bar with tokens and percent, the auto-compaction threshold, and the MCP (`2 of 3 connected · failed: docs`) and language-server summaries. A source that answers 404 or 503 is left out; sandbox and account rows appear only when `/health` reports them.

**Config** edits the TUI preferences `theme`, `theme_mode`, `sidebar`, `machine_stats`, `recap`, `spinner`, `workflow_notifications`, `reasoning`, `diff_style`, `timestamps`, `mouse`, `color` and `accessibility.reduced_motion` in place: Up/Down select a row and Enter or Space moves it to its next value, which applies at once and is saved to `kv.json` in the state directory (`prefs.<key>` with the time it was saved; `theme` also keeps its old `theme` key). Each row shows its source: `default`, `global` (the global `tui.jsonc`), `project` (a `.cyber/tui.jsonc`), `saved` (`kv.json`) or `session` (a toggle such as Ctrl+T or `/timestamps` made in this run only). A `tui.jsonc` written after a saved value wins over it. Enter on `theme` opens the theme switcher instead of cycling. `diff_style` is stored for split diffs, which will read it. The `output style` row after them shows the Session's style and its source, and Enter opens the [output style picker](#choosing-a-style); a configured `output_style` is listed with its file under the core settings. Other core settings stay read-only: until the server has a `config` route the tab says `Core settings: use cyber config or edit cyber.jsonc`.

**Usage** (`/usage`, or `/cost` at the Session section) reads the session usage breakdown: `Total cost` (`—` when every call was unpriced, `≥ $x · N unpriced calls` when some were or attribution is incomplete), `Total duration (API)` and `(wall)` (`3h 21m 0s`, `5d 23h 31m`), `Total code changes`, and `Usage by model`, one row per model with compact counts (`5.3k`, `352.7k`, `1.2m`) and its cost, the highest cost first. Below are the validated token table (own, descendants and total per class) and the cache hit rate that the old `/cost` overlay showed; a report for another Session or with inconsistent totals is refused, and a failed refresh keeps the last amounts with the error. Each configured Budget limit of the Session (`budgets.session`) and `budgets.daily` then gets a 40-column meter, `Session budget ($5.00)` or `Daily budget ($20.00)` with `N% used`, in the warning color from 80% and the error color from 100%; the daily one adds `Resets 12:00am (<zone>)`. Each window a provider reported (`GET /api/v1/usage/limits`) follows as a meter labelled `<provider> · <label>` (`anthropic · 5-hour limit`) with the same colors, then `Resets <time> (<zone>)` in local time, with the date when the reset is a day or more away, and `Reading from <age> ago` once the reading is older than 5 minutes. When a provider reports both daily and weekly windows, `d` shows the daily ones and `w` the weekly ones. Without a Budget and without a provider report there is no limits section. Last comes `What's contributing to your usage?` from `GET /api/v1/usage/insights` over every Session the server stored: the window (`Last 24h`, or `Last 7 days` after `w`; `d` returns), whether shares are `% of cost` or `% of tokens`, one bold `N% of your usage came from …` line with an advice line per factor of at least 5%, and a `Subagents` table of at most 8 groups; an empty window reads `No usage in the last 24h`. `r` refreshes (`R`, the old overlay's key, still works for one release).

**Stats** (`/stats`, or the palette's `Settings: Stats`) renders `GET /api/v1/usage/stats` for the TUI's own zone in two subtabs, switched with `v`. **Overview** draws the activity heatmap: one column per week (Sunday to Saturday, newest on the right), `Mon`/`Wed`/`Fri` row labels, month names above the week of each 1st, and each day at a quartile level of the active days' tokens, as `·░▒▓█` in the `heat_0`–`heat_4` colors (only the glyphs under `NO_COLOR`), with a `Less ░ ▒ ▓ █ More` legend. It shows as many recent weeks as fit, up to 53; a narrow panel drops the oldest weeks and never clips a cell. Below are the periods `All time · Last 7 days · Last 30 days` (the active one in bold accent) and the scope, then `Favorite model`, `Total tokens`, `Sessions`, `Longest session`, `Active days: N/M`, `Longest streak`, `Current streak`, `Most active day` and `Total cost` (with the Usage tab's unpriced rules), in two columns from 80 columns, a muted `Input · Output · Reasoning · Cache read · Cache write` line, and an accent line comparing the input plus output tokens with the largest book of a small table that they exceed (`Your input and output are ~769x the tokens in War and Peace`; Brave New World counts about 77,700 tokens), or with paperback pages below the smallest book, left out when the period has no usage. **Models** lists each model with a 20-column share bar and percent, tokens, cost, Turns and Sessions, most tokens first. `p` cycles the period, `s` switches between `All projects` and `This project` (the Location's project), and `r` reloads. Screen-reader mode prints `Active 265 of 340 days; most active May 20; current streak 14 days` instead of the grid. One day is `1 day`. Untrusted model names are cleaned of control characters, and a malformed reply is refused whole.

### Session recap line

When `tui.recap` is `auto` (the default), opening a Session whose last finished run is at least 30 minutes old, or returning to the terminal after at least 30 minutes away with a run finished since the last recap, asks the server for a recap and shows it at the top of the transcript as `※ recap: <text>` in italic `text_muted` (at most two rows, ending with `…` when cut) above a rule; the Settings panel keeps it on top. `/recap` asks at once whatever the setting and reports a failure in a toast (other failures show nothing). The next prompt hides it and Esc dismisses it. With `tui.recap: off` the TUI asks for no recap except through `/recap`.

## TUI file explorer

`/files` or `<leader>o` (Ctrl+X o; `<leader>e` already focuses the sidebar) opens the project as a tree pane: a header bar with `EXPLORER`, the project and `:branch` (`✓ clean` or `N changed` at the right), a `⌕` filter line, the tree, and a `N shown · M hidden` summary. Root rows have no guide and each deeper level adds a `│ ` guide before the `▸`/`▾` fold marker. Directories are bold with a trailing `/` (an open one shows its child count), files are colored by kind (docs, config, code; hidden, ignored, lock and generated files dimmed), and a right column shows the git letter and a compact size. `tui.explorer.icons: "nerd"` in `tui.jsonc` adds Nerd Font glyphs. `←` folds (or moves to the parent) and `→` unfolds. The keys are shown as chips at the bottom. Directories are loaded one level per expansion. Each row has its git letter (`M` in the warning color, `A` and `?` in success, `D` in error; a directory shows `M` when anything below it changed). `.gitignore` and `.cyberignore` apply: `.` shows dot and ignored entries, and the key row counts what is hidden. `/` filters by fuzzy path over the loaded rows and the server's search, `g` keeps only changed rows, `@` puts `@<path>` in the composer and closes, `y` copies the project-relative path, `r` reloads every loaded level, and the tree also reloads on `session.diff.1` and when a Turn ends, keeping what is expanded. Enter on a file opens a preview beside the tree, behind a divider (in place of it below 100 columns, where Esc returns). Its header bar has a breadcrumb (`docs › README.md`), the language, line count, size and the Session diff's `+N −M`. Markdown is rendered like transcript messages, and `m` switches to the source (the header chip shows `rendered` or `raw`). Source has line numbers, a `▎` mark on the lines the Session changed, a `│` separator, the code-block highlighter and theme tokens, and long lines wrapped under the gutter; a scrollbar shows the position; at most 5,000 lines and 1 MiB, with a truncation notice; binary files show `binary · <size>`. Tab (`explorer_focus`) moves focus between the tree and an open preview, and Esc returns it to the tree; the focused pane's header title is drawn in the accent color, and while the preview has focus Up/Down, PageUp/PageDown and Home scroll it. With the mouse captured, a click on a tree row selects it and acts as Enter (a directory folds or unfolds, a file opens its preview), a click in the tree or the preview focuses that pane, a click on the `rendered`/`raw` chip acts as `m`, and a click on a footer key chip acts as its key. The wheel follows the pointer, whichever pane has focus: over the tree it moves the selection, over the preview it scrolls 3 lines per notch.

The explorer never bypasses permissions. Rows carry the Session's `read` decision: an `ask` file is dimmed with `⊘ ask to read`, and opening it raises the normal permission request before any content is sent; a `deny` file is dimmed with `⊘ denied` and is never requested. Names, symlink targets and file lines are drawn without control characters. Screen readers get each row as `<name>, directory|file, <status>, level <n>`.

The routes behind it are in the `fs` group, take the Location as usual and `session_id` to apply that Session's rules and Mode (without one, the Location's rules in `default` mode, and an `ask` is refused because no one can be asked):

- `GET /api/v1/files?path=&hidden=&session_id=` returns one level: `entries[]` of `{ name, path, kind: file|directory|symlink, size, git_status, ignored, target?, read: allow|ask|deny }`, directories first then by name, at most 2,000 with `truncated: true`, plus `hidden` (dot and ignored entries left out), `changed` (changed files in the project) and `root` (the canonical project root the paths are relative to, which may be above the Location; the TUI explorer's `@` turns a path into a mention relative to the Session directory, or an absolute one outside it).
- `GET /api/v1/files/search?query=&limit=` fuzzily matches non-ignored file paths (`results[]` of `{ path, read }`).
- `GET /api/v1/files/content?path=` returns `{ text | binary: true, size, language, lines, truncated }`, at most 1 MiB, only when `read` is allowed; an `ask` with a Session raises the permission request and waits for the reply. The file served is the one the decision was made on: if the name is replaced while the request waits (a symlink or hard link to another file), the route refuses with `403 {reason: changed}` and sends no bytes; a listing does the same for a swapped directory.

Paths resolve against the project root lexically and then canonically: `..`, absolute paths and symlinks whose target leaves the project answer `403` with `reason: outside_project` and no bytes, and a refused read answers `403` with `reason: permission` and the `decision`. Git status is read with libgit2, so no `git` process or hook runs.

## Workspace and specifications

| Crate | Role |
|---|---|
| `cyber-core` | Build info, XDG paths, IDs, config layering and trust gating, evaluation manifests |
| `cyber-store` | SQLite WAL/FULL, migrations, event store, writer queue, ownership lock |
| `cyber-cli` | The `cyber` binary |
| `cyber-llm` | Provider-neutral LLM layer: adapters, retry, catalog, credentials, cost |
| `cyber-server` | Durable session runtime and the `/api/v1` HTTP, SSE, WebSocket and stdio JSON-RPC API |
| `cyber-tools` | Built-in tools, permission engine, bash analysis, skills, web tools, tool host |
| `cyber-sandbox` | OS sandbox: Seatbelt (macOS), bubblewrap (Linux), credential masking, network allowlist proxy |
| `cyber-snapshot` | Shadow-git working-tree snapshots and conflict-aware restore |
| `cyber-tui` | Terminal UI: composer, streaming rendering, permission and question prompts, pickers, themes |
| `cyber-app` | Server assembly and lifecycle: store, catalog, tools, sandbox, snapshots, listeners, registration |
| `cyber-client` | Rust client for the API (HTTP and in-process), used by `cyber exec` and the TUI |
| `cyber-workflow` | Workflow module format, saved-Workflow discovery and the sandboxed QuickJS engine (P2, in progress) |

The TypeScript SDK `@cyber-code/sdk` lives in [`sdk/typescript`](../sdk/typescript/README.md). It is generated from [`sdk/openapi.json`](../sdk/openapi.json) by `scripts/generate_sdk.py`; regenerate the document with `UPDATE_OPENAPI=1 cargo test -p cyber-server --test http openapi_document_is_current`.

Evaluation fixtures and their manifests live in [`eval/`](../eval/README.md).

CI and local specification checks use OpenSpec 1.13.2. Pin the validator so upstream rule changes can be reviewed separately from product changes.

```bash
npm install -g @fission-ai/openspec@1.13.2
openspec list --specs
openspec validate --all --strict
python3 scripts/spec_lint.py      # cross-spec registries: commands, routes, prefixes, phases
python3 scripts/spec_inventory.py # refresh the ROADMAP requirement table
```

---

## Design principles

1. **Provider independence.** The local core supports OpenAI Responses, Anthropic Messages and compatible Chat endpoints, including local servers. Additional native adapters and mixed-model workflows are planned.
2. **Local-first.** Local state and local-model execution do not require a Cyber account. Hosted model calls require connectivity and provider credentials. Cyber Account and hosted services are planned.
3. **Server-first.** The TUI, `exec` and TypeScript SDK share the public server API. IDE, web and mobile clients are planned.
4. **Durable execution.** Prompt admission, tool calls and context changes are recorded before execution proceeds. Recovery handles interrupted streams and uncertain tool outcomes explicitly; file-backed storage uses FULL durability under the documented filesystem assumptions.
5. **Controlled execution.** Permission rules, protected paths, an OS sandbox and snapshots constrain repository work. Production auto-mode permission requests now use durable classifier allow/block/fallback decisions; protected/manual ceilings remain. Windows enforcement runs through an AppContainer whose launch and isolation layers pass native CI, but sandboxed Git Bash and PowerShell cannot start in it yet (issue #9); the Windows network proxy and complete auto-mode controls remain M1.1 work.
6. **Familiar conventions.** Repository instructions and skills use familiar `AGENTS.md`/`CLAUDE.md` and `SKILL.md` conventions, plus Cyber Code's own [`CYBER.md`](#project-instructions-and-cybermd). MCP integration and setup import are later roadmap work.

## Architecture

See the [architecture diagrams](architecture.md) and the [storage decision](../docs/decisions/0001-storage-architecture.md).

## Naming

| Thing | Value |
|---|---|
| Binary | `cyber` |
| Project config | `cyber.jsonc` / `cyber.json` at any level, and `.cyber/` directories |
| Global config | `$XDG_CONFIG_HOME/cyber` (`~/.config/cyber`), overridable by `CYBER_CONFIG_DIR` |
| Data / state / cache | `~/.local/share/cyber` · `~/.local/state/cyber` · `~/.cache/cyber` |
| Database | `<data>/cyber.db` |
| Env prefix | `CYBER_` |

## Specification glossary

These terms describe the product contracts. Workflows, Session goals and Session loops have implemented subsets described in their guide sections. Runners, Relay, devices, channels and Cyber Account remain planned capabilities. Subagent execution is partially implemented; auto-mode tool classification is implemented locally while its full configuration/override contract remains open. This table is not a list of shipped features.

| Term | Meaning |
|---|---|
| **Location** | `{ directory, workspace? }`. A working directory resolved to a Project; it scopes config, tools and permissions. |
| **Project** | Identified from git (hash of `origin` → cached ID → first root commit), else `global`. |
| **Session** | A durable conversation bound to one Location, with an agent, a model and a permission mode. |
| **Admitted Prompt** | Input durably recorded in the session inbox before execution, with `delivery` = `steer` (next safe boundary), `queue` (when idle) or `hold` (needs approval). |
| **Turn** | One provider request/stream plus the settlement of its tool calls. |
| **Drain** | The process-local loop that runs Turns for a Session until nothing is eligible. Only one Drain runs per Session. |
| **Safe Boundary** | The point between Turns where input, context updates, messages and goal checks are applied. |
| **Context Epoch** | A stable, cacheable system-prompt baseline. Changes arrive as Mid-Conversation System Messages until the next compaction. |
| **Agent** | A named profile (system prompt, model, mode, tools, permissions, step limit). Built-ins: `build` (primary), `explore` and `general` (subagents). **Subagent**: an Agent run in a child Session spawned by a tool. |
| **Workflow** | A JS/TS script, run by the Workflow runtime, that orchestrates many subagents. **Workflow Run**: one execution of it, resumable. |
| **Goal** | A completion condition attached to a Session and checked by an evaluator after each Turn. A Session has one active Goal plus an ordered goal queue. |
| **Loop** | A prompt re-run on a fixed interval or self-paced inside a Session. |
| **Routine** | A saved prompt + repos + triggers (schedule / API / GitHub / webhook) executed on a Runner. |
| **Runner** | A machine that executes Sessions for a remote client: the user's own machine, a self-hosted runner, or Cyber Cloud. |
| **Relay** | The rendezvous service (Cyber Cloud or self-hosted) that connects remote Devices to a local server. It carries end-to-end encrypted traffic only. |
| **Peer** | Another `cyber` server the user controls, reached directly over its API (LAN, VPN, SSH tunnel) without the Relay or an account. |
| **Device** | A client installation (phone, browser, other machine) paired to a Cyber Account. |
| **Channel** | An inbound event source (webhook, chat bridge, CI, MCP channel server) that admits messages into a running Session. |
| **Mode** | A permission mode: `default`, `accept-edits`, `plan`, `auto`, `dont-ask`, `bypass`. Planning is a Mode, not an agent. |
| **Cyber Account** | An identity from CyberdyneAuth. It carries `sub`, `orgs`, `entitlements` and `roles`. |

## Specification capability map

This is the full product scope, spanning P0–P4. See the [roadmap](../ROADMAP.md) for each capability’s delivery phase; it is not an availability matrix.

| Area | Capabilities |
|---|---|
| Foundation | `cli-commands`, `configuration`, `storage-events`, `installation-upgrade` |
| Models & identity | `provider-catalog`, `provider-credentials`, `cyber-account` |
| Agent runtime | `session-runtime`, `system-context`, `memory`, `compaction`, `agents-subagents` |
| Tools & safety | `workspace-trust`, `tool-registry`, `builtin-tools`, `permissions-modes`, `sandbox`, `snapshots-checkpoints`, `worktrees`, `code-intelligence` |
| Autonomy & orchestration | `verified-workflows`, `workflows`, `goals`, `loops-scheduling`, `background-tasks`, `agent-teams` |
| Connectivity | `server-api`, `client-sdk`, `cross-session-messaging`, `remote-control`, `runners-cloud`, `routines`, `channels` |
| Extensibility | `hooks`, `plugins-marketplace`, `mcp`, `skills-commands` |
| Surfaces | `tui`, `web-client`, `exec-mode`, `editor-integration`, `vcs-integration`, `session-sharing` |
| Operations | `harness-evaluation`, `browser-verification`, `observability-costs`, `org-policy`, `compat-import` |

## Product focus

The first release targets dependable local coding: inspect and edit a repository, verify the result, preserve user changes, and resume after interruption. Its release gates measure coding success, cost, recovery and trust behavior.

OpenCode, Codex and Claude Code are design references. Provider independence, a public server API, durable execution and later multi-model workflows define Cyber Code's direction. Competitor parity claims require dated, reproducible measurements rather than an undated feature matrix.

The local storage decision is SQLite WAL with FULL synchronization and one writer owner. PostgreSQL supports later hosted services with multiple active instances. See the [storage decision](../docs/decisions/0001-storage-architecture.md) for alternatives and benchmark criteria.

## Project instructions and CYBER.md

Instruction files are privileged: their text becomes part of the system context (`core/instructions`) of every Session at a Location. Cyber Code reads four file names, from one ordered list in `crates/cyber-server/src/runtime/instructions.rs` that the baseline, nested on-read discovery and `cyber doctor` all share:

| File | Role |
|---|---|
| `AGENTS.md` | Shared guidance that every coding agent (Codex, OpenCode, Cyber Code) reads. Put most project rules here. |
| `CLAUDE.md` | Claude Code compatibility. Read only in a directory that has no `AGENTS.md`, so repositories that keep the two in sync are not loaded twice. |
| `CONTEXT.md` | A description of the directory rather than agent rules. |
| `CYBER.md` | Cyber Code's own file, in the role `CLAUDE.md` plays for Claude Code: guidance only Cyber Code should follow, such as its Modes, tools, Workflows, skills or `cyber` commands. |

### Load order and precedence

1. The global config directory (`$XDG_CONFIG_HOME/cyber`, default `~/.config/cyber`, or `CYBER_CONFIG_DIR`): `AGENTS.md`, then `CYBER.md`.
2. Each directory from the project root down to the Location, root first. Within one directory the order is `AGENTS.md` (or `CLAUDE.md` when there is no `AGENTS.md`), `CONTEXT.md`, then `CYBER.md`.

Later text wins ties, so a directory's `CYBER.md` refines its `AGENTS.md`, and a more specific directory refines its parents: a root `CYBER.md` still comes before `packages/api/AGENTS.md`. Each file is rendered as its own `Instructions from: <path>` block, and every file, imported ones included, is loaded once by canonical path, so a `CYBER.md` symlinked to `AGENTS.md` appears once, as `AGENTS.md`.

An `AGENTS.override.md` takes the `AGENTS.md` slot of its directory and replaces that directory's `AGENTS.md`, `CLAUDE.md` and `CYBER.md`; its `CONTEXT.md` still loads.

### Writing a CYBER.md

Keep it short and specific to Cyber Code, and leave shared rules in `AGENTS.md`:

```markdown
# Cyber Code notes

- Run `cargo test -p <crate>` before finishing; never run release builds.
- Use the `/review` skill on uncommitted changes before handing work back.
- Long fan-out tasks belong in `.cyber/workflows/`, not in ad hoc subagents.

@docs/cyber/testing.md
```

- **Imports.** A line containing only `@path` inlines that file after its importer, as its own block. Paths resolve relative to the importing file, or to home for `@~/`. Lines inside code fences are ignored, imports nest up to a depth of 5, and a cycle is skipped with a warning naming both files.
- **Project files stay in the project.** A project instruction file, or an import from one, that resolves (after symlinks) outside the project root is not loaded and produces a warning, so a checkout cannot pull `~/.ssh/...` or `../other-repo` into the context, whether through `@path` or through a symlinked `CYBER.md`. A symlink inside the project (`CYBER.md -> AGENTS.md`) still works. Global files are yours and may import anywhere. Unresolved imports are warnings, not errors.
- **Project files follow your `read` rules.** A project instruction file or import loads only when your `read` permission rules allow it outright. One they ask about or deny is skipped with a warning and no prompt, so under the default rules `@.env` (or a `CYBER.md` symlinked to `.env`) never reaches the context, and neither does a file a `read` deny covers. If the project's config does not parse, no project instruction file loads and `CLAUDE.md` is treated as off until it is fixed.
- **Only regular files.** A FIFO, device or directory in place of an instruction file is never opened for reading. In the baseline it makes the instructions unavailable, so the first Turn is blocked as for any unreadable instruction file (`cyber doctor` names it); found by a nested read, it is skipped with a warning.
- **Size.** Each file is cut at 32768 bytes (`instructions.max_bytes`, not configurable yet) with a `[Truncated: ...]` notice, and reading stops there, however large the file is. The whole baseline, imports included, stops at 131072 bytes: the file that crosses it is cut with a notice, and later files are left out with a warning. `cyber doctor` shows both.
- **Edits mid-session** reach the model at the next Safe Boundary as one durable context update; the cached prompt prefix is not rewritten.

### Nested files on read

Files in directories below the Location are not in the baseline. When the built-in `read` tool successfully reads a file under such a directory, that directory's instruction files (and those of the directories between) are appended to the result inside a `<system-reminder>` block, once per file per Context Epoch; the attachment is recorded durably with the tool settlement, and a new epoch (for example after compaction) starts over. Reads through `bash`, `grep` or MCP tools do not trigger it, and neither do denied or failed reads.

One reminder carries at most 32768 bytes of instruction text. Files past that are named in the reminder (`read them with the read tool`) instead of attached. When even that reminder does not fit the tool output budget (`tool_output`), the read gets a short list of the file paths; when nothing fits, the read keeps its own output and the files can attach on a later read. Instruction files never turn a successful read into an error. Each attachment fires the `InstructionsLoaded` hook with `load_reason: "nested_read"` and the attached `file_paths`, as the baseline does with `session_start` and `changed`.

### Switches

These are read from the server's environment (a value of `1`, `true`, `yes` or `on`):

| Switch | Effect |
|---|---|
| `CYBER_DISABLE_CLAUDE_MD` (or `CYBER_DISABLE_CLAUDE_COMPAT`, or `compat.claude = false` in config) | Stops loading `CLAUDE.md`. `AGENTS.md`, `CONTEXT.md` and `CYBER.md` still load. `compat.claude` is read from the resolved config of each Location, project layers included, by both the runtime and `cyber doctor`. |
| `CYBER_DISABLE_PROJECT_CONFIG` | Skips every project instruction file, `CYBER.md` included, and nested discovery. The global `AGENTS.md` and `CYBER.md` still load. |

### Checking what loads

`cyber doctor` has an `instructions` line that lists, in load order, the files a new Session at the current directory loads (`N loaded: <path>, ...`), and warns about truncated files and unresolved or refused imports. `GET /api/v1/sessions/{id}/context` returns the rendered baseline with its `Instructions from:` labels.

### Migrating from Claude Code

No migration is needed: `CLAUDE.md` keeps loading in place. `cyber import` does not copy `CLAUDE.md` into `CYBER.md`, because both would then load and duplicate the content. To move Cyber-specific parts, put them in `CYBER.md` and, if the remaining guidance is shared, rename `CLAUDE.md` to `AGENTS.md`.

Not implemented yet: `.cyber/rules/*.md` and `.claude/rules/*.md`, `.claude/CLAUDE.md`, the `~/.claude/CLAUDE.md` global fallback, a configurable `instructions.max_bytes`, per-source token sizes in `/context`, and `/init` (which is specified to generate `AGENTS.md`, not `CYBER.md`).

## Base prompts and output styles

The system prompt starts with the agent's `system` text when the agent sets one. Otherwise it starts with a short base prompt chosen by the model's family, followed by the Session's output style. The base prompts and the built-in styles are embedded in the binary (`crates/cyber-core/prompts/`).

### Base prompts by model family

Every base prompt is one shared core text plus a family addendum: `openai`, `anthropic`, `gemini`, `local` (open-weights models) or `default` (the core text alone). The family comes from the catalog model, not the provider id, so `openrouter/anthropic/claude-sonnet` gets the Anthropic prompt and `groq/gpt-oss-120b` the local one. The order is:

1. Open-weights names (`gpt-oss`, `llama`, `qwen`, `mistral`, `deepseek`, `gemma`, `phi`, `glm`, `kimi`, …) select `local`.
2. `claude` selects `anthropic`; `gpt`, `o1`, `o3`, `o4`, `codex` and `chatgpt` select `openai`; `gemini` selects `gemini`. The catalog family name is checked first, then the last `/` segment of the model id.
3. Otherwise the protocol decides: Anthropic Messages is `anthropic`, OpenAI Responses and the ChatGPT plan are `openai`.
4. Otherwise a provider without authentication or with a loopback URL is `local`, and anything else is `default`.

A model switch that changes the family starts a new Context Epoch on the next Turn. Epochs recorded before families existed compare by provider only, so a restart never rebuilds their prefix.

### Output styles

The built-in styles are `default` (adds nothing), `concise`, `explanatory` and `learning`. Custom styles are Markdown files directly inside `~/.config/cyber/output-styles/` (user) and `.cyber/output-styles/` at the project root (project):

```markdown
---
name: reviewer
description: Reviews like a senior engineer
---
Point out risks first, then style. Quote the exact lines you mean.
```

- The name is the file stem and must match `^[a-z0-9][a-z0-9_-]{0,63}$`; `name` in the frontmatter, if present, must equal it.
- The frontmatter may set only `name`, `description` and `keep-coding-instructions` (accepted for Claude Code files and ignored). Any other key, such as `permissions`, makes the file invalid. The trimmed body is the style text and must not be empty.
- Only regular files of at most 32 KiB are read (128 files at most); links are refused.
- A custom style cannot reuse a built-in name; the file is skipped and reported. A project style replaces a user style of the same name.
- Project styles are repository content: they are part of the checkout's trust digest and load only while the checkout is trusted (`cyber trust inspect`, `cyber trust approve`). Until then they are held, and the listing says `project output styles are held until this checkout is trusted (cyber trust inspect)`. `CYBER_DISABLE_PROJECT_CONFIG=1` skips them.
- An invalid or held file is reported with its path and reason and never stops the other styles from loading.

A style only adds text. It never changes permissions, the Mode or its ceiling, the sandbox, the tools offered or the model.

### Choosing a style

A primary Session uses, in order: its own selection (made at creation or by a switch), the `output_style` key of the resolved configuration at its Location (`"output_style": "concise"` in `cyber.jsonc`), then `default`. Subagents, workflow agents and teammates have no style, because their output goes to another agent.

- **TUI:** `/output-style` (or the palette's **Output style…**) opens a picker showing each style's origin and the `current` and `default` labels, with the listing's issues below; typing filters. Enter switches the Session, `d` also saves the style as your global default, and Esc closes without changes. `/output-style <name>` switches directly. Settings › Config has an `output style` row with its source (`session`, `config` or `default`); Enter opens the picker.
- **Exec:** `cyber exec --output-style <name>` checks the name against the Location's listing before any Session exists and exits 2 with the reason (invalid, held or unknown) and the available names.
- **HTTP:** `GET /api/v1/config/output-styles?directory=` lists the styles, the issues, the configured and effective style and why a configured style does not apply. `POST /api/v1/sessions` accepts `output_style`, `POST /api/v1/sessions/{id}/output-style` takes `{"style": "<name>" | null}`, and `PUT /api/v1/config/output-style` saves the global default (built-in and user styles only; `null` writes `default`). `GET /api/v1/sessions/{id}` reports the Session's `output_style`; the `GET /api/v1/sessions` list summary does not include it yet.

An explicit selection naming an invalid, unknown or held style is refused. A configured style that is unavailable falls back to `default`, with a warning in the TUI (once per Session) and on `cyber exec`'s stderr (`warning: <reason>; the default output style applies`). A switch takes effect at the next Turn through a new Context Epoch: a running Turn keeps its prefix, and no mid-conversation message is sent. The switch is recorded as `session.output_style.switched.1` and survives a restart.

Not implemented yet: output styles from plugins, importing `.claude/output-styles/` with `cyber import` (copy the files; the format is compatible), and a per-agent `output_style`.

## Hook configuration and trust review

`cyber hooks list --format json` shows resolved hook definitions, file origins, scope, SHA-256 digests and individual trust state, with credentials redacted. Checkout definitions remain withheld until configuration approval with `cyber trust inspect|approve`; the list reports those withheld paths. After reviewing a resolved project/local handler, use `cyber hooks trust --digest <digest>`. A changed handler needs its new digest approved. `cyber hooks untrust --digest <digest>` also revokes obsolete digests when current configuration cannot be parsed.

GET `/api/v1/hooks` inspects the current Location’s loaded definitions and uncached trust state without running handlers. The generated SDK exposes `client.hook.list()` and preserves the Location envelope. Rows retain original digests and file origins, report mandatory sandboxing, and redact headers and recognized secret fields. Withheld project paths and original top-level/profile hook sections are reported as redacted literal JSON. Environment/file placeholders remain unexpanded; invalid handler schemas remain inspectable. These inactive sections have no executable handler digest or approval controls. POST `/api/v1/hooks/trust` accepts `{ "digest": "…" }` only for a currently resolved project/local handler in a trusted checkout. POST `/api/v1/hooks/untrust` revokes an obsolete digest even if configuration is malformed, returning whether an approval existed. Both preserve the Location envelope; SDK clients expose `hook.trust` and `hook.untrust`. The TUI `/hooks` viewer shows loaded definitions and withheld paths. Use ↑/↓ to select, PgUp/PgDn to scroll, R to refresh, T to review approval and U to review revocation; Y confirms the displayed exact digest, N/Esc cancels. Global/managed/plugin/invocation rows do not offer checkout approval. Switching Session or Location dismisses the review. The CLI list and TUI viewer also display raw withheld sections. Loaded rows include a last-run summary for the same effective digest/event/scope and canonical checkout, using the receipt’s captured Location. Summaries include admission time, outcome, duration and stop acknowledgement; raw IO and decision content are omitted even for opted-in receipts. Running is an unverified observation and unknown requires recovery. CLI review opens only an existing database. `cyber hooks test <event> [--payload FILE]` exercises matching command, HTTP, prompt and local MCP-tool handlers with fresh synthetic identity and no Session startup.

`cyber hooks test PreToolUse --payload event.json --format json` accepts an object such as `{"tool_name":"edit","tool_input":{"path":"src/main.rs"}}`. Omit `--payload` to use empty event fields. Files must be regular JSON files at most 1 MiB; common envelope fields and the synthetic marker cannot be supplied. Tests print each selected handler and merged decisions, keep project configuration/handler trust gates, and retain real command process/checkout ownership through cancellation. Fixed-input events run with bounded concurrency and ordered merging; rewrite-dependent pre events currently chain sequentially. Unsupported transports/async handlers and incomplete termination exit nonzero with explicit outcomes. Synthetic receipts are separate from Session history and last-run summaries. They exercise real handler effects while leaving Session state unchanged; Remote MCP handlers, custom-adapter acknowledgement and full native acceptance remain open.

Synchronous HTTP handlers now POST the protected event JSON with configured headers and application/json content type, parse a bounded JSON object decision, and use the same durable Session/synthetic receipts and once claims. Project/local/sandbox-all scopes honor network off/on/allowlist configuration. Every request uses an explicit owned proxy; redirects, automatic retries, ambient proxies and transport-header overrides are refused. Non-2xx, malformed/oversized bodies and transport errors are nonblocking unless `fail_closed` is enabled. Timeouts and cancellation close and join local request/proxy sockets; this acknowledges local transport disposal and cannot prove remote POST rollback. Raw event/response IO is omitted by default and bounded when explicitly enabled; header credentials are never recorded as IO. Remote MCP handlers, async scheduling and complete native acceptance remain open.

On Unix, matching command handlers now run around built-in tools: `PreToolUse` before permission evaluation, followed by `PostToolUse` or `PostToolUseFailure` after settlement. A pre-hook denial blocks even in bypass mode; rewritten inputs are schema checked, and allow decisions retain permission denies and Mode ceilings. Project/local handlers require current checkout and individual digest approval. Errors and changes produce transient TUI/exec notices; durable execution receipts omit raw IO by default.

Matching command `PermissionRequest` hooks now receive the final tool input and the current permission ask immediately before a user prompt. `allow` approves that request once without saving a rule; `deny` blocks without a prompt. Input rewrites are ignored for this event. Requests already allowed or denied by hard policy never reach this boundary; `PermissionDenied` retries remain open.

Command handlers with `once: true` claim their effective digest once per Session through durable receipt admission. Restart, cancellation and unknown outcomes retain the claim; changed definitions need fresh trust and have a new digest. `status_message` and `system_message` appear as transient user notices while running and after acknowledged settlement, without entering model context.

Command handlers for `PermissionRequest`, `PostToolUse` and `PostToolUseFailure` now share a per-event pool bounded by `hooks.concurrency`. Finished handlers free their slots immediately; decisions still merge in declared order. A mandatory stop or fail-closed admission error cancels siblings and waits for launched owners to settle. Pre-tool handlers remain sequential so rewrites can drive later input and selectors; complete pre-tool concurrency remains open.

GET `/api/v1/sessions/{sessionID}/hook-executions` lists durable hook receipts with `limit` (1–500, default 50) and a Session-bound `cursor`. The SDK exposes `client.session.hooks(sessionID, { limit, cursor })`. Rows include captured call/tool identifiers when available, outcome, decision and stop acknowledgement. Raw IO is omitted by default; explicitly enabled, bounded IO remains visible to authenticated clients. A `running` receipt means admission was recorded, not that a process has been reconciled as live after restart; unresolved recovery remains open.

`cyber hooks history --session <id> [--limit 50] [--cursor <cursor>]` reads these committed receipts directly from the local database without starting a server, loading executable configuration or reconciling owners. `--format json` preserves the receipt fields and next cursor, including explicitly recorded IO; text output summarizes status and acknowledgement without raw IO. Running observations remain unverified as live, and unknown observations require recovery.

In the TUI, `/hooks history` pages the current Session’s receipts. Use `N` for the next page, `R` to refresh from the first page, arrows to scroll and Escape to close. The viewer shows recorded status, outcome, timing, call/tool correlation and stop acknowledgement. Raw IO is excluded from viewer state and display; running observations remain unverified as live. Definition review and trust controls remain under implementation.

On Windows, command hooks use PowerShell with an explicit parent-owned event-input route. The helper consumes its private launch permit before command input. Project, local, plugin and `sandbox_all` hooks run in the AppContainer `hooks` profile described under sandbox profiles above. Global (full-access) PowerShell and HTTP hooks pass native Windows CI; sandboxed PowerShell hooks cannot start until issue #9 is fixed.

`SubagentStart`/`SubagentStop` now run around every child launch (model `agent` calls, forked skills, `@agent`, `cyber review` and `/review`): a `SubagentStart` deny refuses the launch before any child Session, Job or worktree, even in bypass, and `SubagentStop` observes the handback. `Stop` hooks can `block` to continue a Drain with their reason, up to `hooks.max_stop_continuations`, and `StopFailure` observes failed Turns and Drains. `UserPromptSubmit` can deny a user prompt or delegation before admission or add `additional_context` immediately before it; `SessionStart` runs once per root Session and may add context; `JobEnded` observes terminal background Jobs. These cases are verified locally on Unix; native Windows acceptance remains pending.

Tool-event context, `continue: false`/`stop_reason`/`suppress_output`, `async` handlers, `PermissionDenied` retries and the runtime lifecycle events also run now (see the hooks spec delta). A `UserPromptSubmit` `continue: false` lets the prompt's own Turn run and then ends the Drain; for a delegated prompt or a forked skill command, which has no Turn of its own, it refuses the request instead. An accepted `PreToolUse` `updated_input` is recorded in the transcript as a `hook_rewrite` system message after the call settles. `Setup` runs when the first root Session of a Location is created, `ConfigChange` at the Safe Boundary after the Location configuration changed, and `WorktreeRemove` after a managed checkout is removed; all three only observe. A hook event runs as a Session at its current Location, so `WorktreeRemove` runs where such a Session outlives the checkout: an isolated child's cleanup (as the parent), `exit_worktree` and the exit cleanup of an entered Session (as that Session, back at its origin), and `cyber worktree remove|prune` when a Session had entered the checkout (`--force`) or an isolated child owned it (as the parent). Removing the worktree a Session was started in (`cyber exec --worktree`, `cyber --worktree`, `POST /worktrees`), whether at its exit or later by `cyber worktree remove|prune`, leaves that Session no Location to run hooks in, so no `WorktreeRemove` runs. `PreToolUse` and `PermissionDenied` handlers run one at a time by design, because each rewrite feeds the next handler; other events use the concurrent pool. Definition and history review commands execute no handlers.

**Managed hooks.** An administrator can place `cyber-policy.json` or `cyber-policy.jsonc` in `/etc/cyber` (Linux), `/Library/Application Support/cyber` (macOS) or `%ProgramData%\cyber` (Windows). On Unix the file must be owned by root and not writable by group or others. On Windows the file and the `cyber` folder must be owned by SYSTEM, Administrators or TrustedInstaller, and only those may change or delete the file or delete from the folder; otherwise it is ignored with `managed policy file not admin-owned; ignored`. A managed `mcp_tool` hook's server and a managed `prompt` hook's evaluator model and provider must come from global configuration only; if a project, profile, `--config` or `CYBER_CONFIG*` layer changes them, the hook is unavailable (and blocks when `fail_closed`). Its `hooks` event arrays run first, in the `managed` scope, without handler approval and unsandboxed unless `hooks.sandbox_all`. Setting `policy.hooks.managed_only: true` skips every global, project, local and `--config` hook:

```jsonc
{
  "hooks": { "PreToolUse": [{ "matcher": "bash", "hooks": [{ "type": "http", "url": "https://policy.corp/hook" }] }] },
  "policy": { "hooks": { "managed_only": true } }
}
```

A Session then shows `N hooks skipped by managed-only policy` once per event and records a `skipped` receipt for each skipped handler; `cyber hooks list`, `GET /hooks` and TUI `/hooks` list them with that reason. A policy file that cannot be parsed or uses unsupported keys fails closed: every non-managed hook is skipped with the diagnostic. `policy` in user or project configuration has no effect. `CYBER_MANAGED_POLICY_DIR` points the loader at another directory for tests and packaging, like `CYBER_HOME`; it is ignored while the OS directory holds a policy.

**SDK hook handlers.** A connected client can register an in-process handler with `client.hooks.on(event, handler, { matcher?, timeout? })` (TypeScript SDK). The server forwards each matching event of the client's own Sessions (those it created or forked, those attached with `client.hooks.attach(sessionID)`, and their subagents) as a `hook/execute` request over the client's JSON-RPC channel (`v1.hook.register`, `v1.hook.unregister` and `v1.hook.attach` on the WebSocket or `serve --stdio` transport), merges the returned decision after every configured handler and records a receipt with scope `client`. A handler that throws, times out (default 60 s) or returns something other than a decision object is a non-blocking error. Registrations end when the connection closes, and an unanswered `hook/execute` then fails at once; the SDK restores registrations and attachments after a reconnect. A managed-only policy also skips client handlers, and configuration cannot declare `type: "client"`.

**Untrusted hooks.** A changed or new project/local handler is skipped with one `N untrusted hook(s) changed — review with /hooks` notice per Session and a `skipped` receipt. `cyber exec --embedded --trust-project-hooks` runs the handlers resolved at startup for that run only and stores no approval; it is refused against a shared server.

On Windows, command hooks read the event from `[Console]::In`, block with exit code 2 and a stderr reason, and may print a JSON decision; a UTF-8 byte-order mark and CRLF line endings are accepted. Project, local, plugin and `sandbox_all` command hooks run in the Windows AppContainer sandbox (workspace read-write, no network unless `sandbox.network` is `on`); a denied write makes the hook report a non-blocking error.


Synchronous prompt handlers use `model_roles.evaluator` with `small_model` fallback. The trusted handler policy stays separate from protected event evidence; requests force tools disabled after overlays and require a complete strict allow/deny/ask judgement. Input and response are bounded to 1 MiB. Recorded calls check Session/ancestor budgets before inference, bill observed usage as hidden calls and check budgets again before proceeding. Calls without usage retain an unpriced marker. Synthetic CLI tests load pinned/cached/bundled model data only when trusted prompt handlers exist for the event; they can call the configured provider without creating a Session. The native OpenAI Responses, OpenAI-compatible and Anthropic adapters use an owned explicit proxy with ambient proxies, redirects and retries disabled. Raw SSE and error bodies are bounded before decoder buffering. Timeout closes and joins local provider sockets before applying nonblocking/fail-closed policy; cancellation retains execution through that acknowledgement and stops continuation. This cannot prove remote processing stopped or final usage was reported. Custom adapters without the owned HTTP capability and disposed callers retain unknown outcomes through the ownership fence. Raw IO follows the same opt-in bounded receipt policy.



Synchronous `mcp_tool` handlers now call configured local servers through a dedicated connection owned by the hook receipt. For example, `{"type":"mcp_tool","server":"audit","tool":"record"}` sends the event JSON to `record`; optional `arguments` support typed `${field}` substitutions. The tool must be present in filtered discovery and its arguments must match the discovered schema. Results must contain one unambiguous textual JSON decision; failed, missing or ambiguous decisions follow `fail_closed`. Hook and server authorization are rechecked before launch, before the call and after its result. Required hook sandboxing applies independently of `sandbox.apply_to` for ordinary MCP connections. One absolute handler deadline includes initialization, discovery and the call. Cancellation retains the native process/proxy owner through shutdown acknowledgement, durable receipt settlement and scratch cleanup. Dedicated connections allow permission hooks to call the same server while its shared connection is waiting on a callback. Hook connections do not borrow a Session's elicitation or sampling callback authority. Synthetic CLI tests create independent receipts and no Sessions. Remote transport, asynchronous handlers and full platform acceptance remain open.


MCP definitions in `mcp` now validate typed local `{type:"local",command,args?,env?,cwd?,enabled?,timeout?,tools?}` and remote `{type:"remote",url,headers?,headers_command?,headers_refresh_seconds?,oauth?,enabled?,timeout?,tools?,required?,output_token_limit?}` settings after workspace trust and substitution. Connection timeout defaults to 30 seconds; `mcp.tool_timeout` defaults to 300 seconds. Tool deny globs take precedence over allow globs. Configured local and remote servers have owned concurrent startup, paginated discovery, shared tool registration and bounded reconnect. Remote OAuth sign-in is described under [Remote MCP OAuth](#remote-mcp-oauth); resources/prompts and remote MCP hook transport remain incomplete.

#### Remote servers

A remote entry connects over Streamable HTTP first and falls back to the legacy HTTP+SSE transport only when that first initialize fails for a non-authentication reason (HTTP 400/404/405, a non-MCP content type or a transport error), inside the same connect `timeout`; reconnects try the transport that last worked. HTTP 401 or 403 never falls back or retries (apart from the single OAuth token refresh described below): the server settles as `needs_auth`, whether the refusal comes at connect or on an established connection, and its tools are withdrawn. `headers` values may use `{env:NAME}` substitution; the substituted values are part of the approval digest. Each request carries the configured headers plus `Mcp-Session-Id` and `MCP-Protocol-Version`. The optional GET event stream delivers server notifications such as `tools/list_changed`; when it ends cleanly it is reopened on the same session with `Last-Event-ID`, and a tool call whose response stream closes early is resumed the same way. Without a stream the client pings the server every 10 seconds. A failed reopen, transport error, unanswered ping or expired session (404) is loss: tools are withdrawn and reconnection uses the shared 1 to 60 second backoff with at most ten attempts. Closing a Location sends `DELETE` for the session (405 is accepted).

Remote servers are not processes, so `sandbox.apply_to` governs only their network: when it selects MCP with a policy other than full access, network `off` refuses before contact and network `proxy` refuses a URL host outside `allowed_domains`. Every remote request, restricted or not, leaves through an owned loopback proxy that ignores ambient proxy variables, follows no redirects and never retries. Project-defined remote servers need the same workspace trust and individual `cyber mcp trust <name> --digest` approval as local ones before any request is sent, and approval is rechecked immediately before contact. Diagnostics, logs, status and durable records never contain the URL query or header values.

MCP launch authorization now checks all contributing field origins and separate per-server approval of the effective definition, keyed by checkout. Workspace/hook approvals do not grant server approval, and a project override of global arguments remains project-controlled. Use `cyber mcp definitions --format json` to review loaded server definitions, their digests, field origins and approval state. Approve a currently loaded project server with `cyber mcp trust NAME --digest DIGEST`, after checkout configuration approval, and revoke an individual digest with `cyber mcp untrust --digest DIGEST`. Revocation works even when the current configuration is invalid. Environment/header values, OAuth settings and URL queries are redacted; approval binds the unredacted effective definition. These commands start no servers and create no Sessions. MCP confinement requires adding `mcp` to `sandbox.apply_to`; default scope excludes MCP, independently of project approval. Configured local servers start when their Location opens; tools remain subject to agent, Mode, permission and hook controls.


## Adding and listing MCP servers

Servers live under `mcp` in a config document. `cyber mcp add` writes the entry for you and keeps every comment and setting already in the file:

```bash
cyber mcp add context7 --url https://mcp.context7.com/mcp            # remote (Streamable HTTP), user scope
cyber mcp add docs --url https://docs.example/mcp --header "Authorization=Bearer {env:DOCS_TOKEN}"
cyber mcp add files --env ROOT=. -- npx -y @modelcontextprotocol/server-filesystem .   # local (stdio)
cyber mcp add shared --scope project --url https://mcp.corp.example/mcp           # .cyber/cyber.jsonc
cyber mcp list                                                         # live status, transport, tools
cyber mcp remove docs                                                  # add --scope when defined twice
```

`--scope user` (the default) writes the global `cyber.jsonc`, `project` writes the repository's `.cyber/cyber.jsonc` and `local` writes `.cyber/cyber.local.jsonc` for this checkout only. A project or local server starts only after you approve it: `add` prints the `cyber mcp trust <name> --digest <digest>` command. Adding a name that the target document already defines is refused, and `remove` without `--scope` is refused when more than one document defines the name. The equivalent hand-written entries are:

```jsonc
{
  "mcp": {
    "context7": { "type": "remote", "url": "https://mcp.context7.com/mcp" },
    "files": { "type": "local", "command": "npx", "args": ["-y", "@modelcontextprotocol/server-filesystem", "."] }
  }
}
```

Servers connect when a Session opens their Location. Until then `cyber mcp list` and `GET /api/v1/mcp` report them `idle` (never `failed`); each row also says its `transport` (`remote` or `stdio`). The TUI sidebar's **MCP** section lists every configured server with its status, transport and tool count (`● context7 remote Connected · 2 tools`), the error of a failed server, `cyber mcp auth <name>` for one that needs sign-in, and `Blocked` for one refused by organization policy. It shows six servers, then `+N more · /status`; with none configured it says `No MCP servers` and how to add one. Focus a failed or needs-auth server with `<leader>e` and press `r` to reconnect it.

## Local MCP startup waiting

Configured local MCP servers start concurrently when a Location opens. The model can call `wait_for_mcp` with `{"servers":["shared"],"timeout":30}` to wait for named servers still connecting; timeout is in seconds, defaults to 60 and must be between 0 and 60. Zero reads current status immediately. The result includes each requested server's redacted status/error and a `timed_out` flag. Disabled/failed servers return without waiting; unknown names fail. Waiting does not start servers, invoke tools or approve discoveries, and cancellation leaves independent startup ownership intact.

The wait tool is read-only and allowed by default, subject to `wait_for_mcp` permission rules and ordinary hooks/agent controls. Connected tools are materialized on subsequent model steps. Local and remote loss reconnects after verified settlement with bounded backoff; full native acceptance remains incomplete.

## MCP resources

When a connected, authorized server in the Location advertises the `resources` capability, the model gets three read-only tools, also offered in plan mode:

- `mcp_list_resources` with `{"server":"docs","cursor":"..."}` returns one page of the server's resources (`uri`, `name`, and `title`, `description`, `mimeType`, `size` when the server sends them) plus `nextCursor` when more pages exist; pass it back as `cursor` for the next page.
- `mcp_list_resource_templates` takes the same input and returns `resourceTemplates` (`uriTemplate`, `name`, ...).
- `mcp_read_resource` with `{"server":"docs","uri":"file:///guide.md"}` reads one resource.

The client only sends `resources/list`, `resources/templates/list` and `resources/read` to a server that advertised `resources`; it never subscribes. Results are untrusted data: listings are wrapped in `<mcp_resources server="...">` and text contents in `<mcp_resource server="..." uri="..." mime="...">`, with `&`, `<` and `>` escaped so content cannot close or forge the frame; URI userinfo is dropped from displayed URIs. Binary (`blob`) contents are decoded, stored as a managed file next to other tool output and summarized as `[binary resource: N bytes stored at PATH]`. Large text follows the ordinary tool output budget and overflow files.

URIs must have an RFC 3986 scheme, contain no control characters and be at most 2048 bytes; they are opaque, never mapped to local paths. A server that returns the cursor it was given is refused. A read whose decoded contents exceed 512 KiB fails with `MCP resource exceeds transport limit`: replies must fit the 1 MiB transport frame, so the 10 MiB blob limit in the specification is not reachable yet. A server error reports only its JSON-RPC code.

Permissions use action `read` with resource `mcp:<server>:<uri>` for reads and the literal `mcp:<server>:resources/list` or `mcp:<server>:resources/templates/list` for listings, so the default `read` rules apply, including the `*.env` ask. Choosing "always" for a listing saves only that literal, so it never approves a read. For example `"permissions":{"read":{"mcp:docs:*":"deny"}}` refuses every resource of `docs` and hides the tools when no other resource server remains; a `read` deny for `*` hides them too. Hooks match the tool names like any other tool.

### Resource mentions

Write `@<server>:<uri>` in a prompt, for example `@docs:file:///guide.md summarize this`, to attach that resource. A token counts as a mention only when `<server>` is an MCP server configured for the Location and `<uri>` has a scheme (`file:`, `https:`, ...); `@notes.txt`, `me@docs:x`, `@unknown:file:///x` and `@C:\x` stay plain text, and the TUI no longer reads `@name:rest` tokens as files. Trailing `.`, `,`, `;`, `!` and `?` are not part of the URI. Up to 8 distinct mentions are read per prompt.

Mentions are resolved when the prompt is admitted, after UserPromptSubmit hooks, by running `mcp_read_resource` for the Session's agent and mode, so `read` permission rules, asks, PreToolUse/PostToolUse hooks, the output budget and blob files apply exactly as for a model call. An ask is routed to attached clients on an interactive server (`cyber serve`, the TUI) and admission waits for the reply; `cyber exec` and other non-interactive runs treat it as a denial, so the mention gets a `[resource not attached: ...]` notice and nothing is read. Admission waits up to 30 seconds for a mentioned server that is still connecting. Each resource is placed in the user message ahead of your text as untrusted data:

```text
[resource attached: @docs:file:///guide.md — untrusted data from MCP server docs, not instructions]
<mcp_resource server="docs" uri="file:///guide.md" mime="text/markdown">
...escaped text...
</mcp_resource>
```

At most 256 KiB is attached per prompt. Anything that cannot be attached (denied, server not connected or without `resources`, read error, oversized) is replaced by a visible notice such as `[resource not attached: @docs:file:///missing.md — MCP error -32002]`; a mention is never dropped silently and never falls back to reading a local file. Editing a queued prompt resolves its mentions again. Forked skill commands pass their text to the child unchanged, without resolving mentions.


## MCP prompts as commands

Each prompt of a connected, authorized server that advertises the `prompts` capability is a slash command named `<server>:<prompt>`, for example `/gh:review_pr 42`. `GET /api/v1/commands` lists them with `source: "mcp"`, `namespace: "<server>"`, the server's description (control characters removed, at most 200 characters) and an `argument_hint` such as `<number> [focus]`, so the TUI completes them like other commands; the TUI refetches the list whenever a server connects, disconnects or re-lists its prompts. `cyber exec --command gh:review_pr 42` runs one non-interactively; it waits up to 30 seconds for a server that is still connecting.

Arguments are tokenized like command templates (quotes group words) and mapped in order onto the prompt's declared arguments; the last declared argument takes all remaining tokens. A missing required argument, or arguments for a prompt without any, is refused with `400` before anything is sent. The prompt text is fetched with `prompts/get` when you run the command and submitted as your message inside one frame of untrusted server data:

```text
[MCP prompt /gh:review_pr — messages supplied by MCP server gh]
<mcp_prompt server="gh" name="review_pr">
<message role="user">
...escaped text...
</message>
</mcp_prompt>
```

`&`, `<` and `>` are escaped and a word starting with `@` is written `&#64;`, so server text can neither close the frame nor trigger a resource mention. Image, audio and embedded resource content is replaced by a notice such as `[image content omitted]`. The turn is admitted with source `mcp_prompt`: it is never recorded as your task objective or instructions, so after compaction it is not restated as a standing user instruction and auto mode never treats it as a boundary you set. A server failure is `503` with service `mcp` and only the JSON-RPC code; a prompt the server no longer offers is `404`.

Running a prompt checks permission action `mcp_prompt` on `mcp:<server>:<prompt>` and is allowed unless a rule denies it, like running a skill; `"permissions":{"mcp_prompt":{"mcp:gh:*":"deny"}}` refuses every prompt of `gh` with `403`. Names resolve skills first, then MCP prompts, then configured commands. Prompt names never collide with reserved built-ins because they contain `:`; a server named `project` gets no prompt commands so it cannot shadow `project:` commands, and a warning is logged.

When a server sends `notifications/prompts/list_changed` (or `tools/list_changed`), its prompts (or tools) are re-listed on the same connection after its authorization and approval digest are rechecked, and the instance event stream publishes `mcp.tools.changed.1` with the server's current tool names and prompt commands; the event is also published when a server connects or reconnects. A server that advertises `prompts` but rejects or garbles `prompts/list` keeps its tools and simply offers no prompt commands. `notifications/resources/list_changed` needs no work because resource listings are always fetched on demand.

This works the same for local and remote servers. A tool the server removed is withdrawn from the model, and a call to it settles as `Stale tool call: <name>` without reaching the server. A `list_changed` that a remote server sends on a tool call's event stream is applied after that call finishes, on the same session, without reconnecting.

## Deferred tool schemas

When the effective catalog exceeds `tool_output.deferred_threshold_tokens` (default 10000 estimated tokens, four characters per token), the model receives MCP/plugin names and one-line descriptions without their parameter schemas. Builtin and Session/client tools retain full schemas. The threshold is shared; zero forces eligible tools to defer.

The model can call `tool_search` with `{"query":"jira issue"}` or `{"select":["mcp__github__create_issue"]}`. Search matches case-insensitive words in names/descriptions, returns full schemas sorted by name and loads them for subsequent steps in the same Session. `limit` defaults to five and accepts integers from zero to 1000. Exact selections reject unavailable names before loading anything; when both query and select are supplied, select takes precedence.

Selections survive restart and compaction, while new Sessions and forks start independently. Calling an unloaded tool returns `Tool <name> is deferred. Load it with tool_search first.` Search is allowed by default, subject to `tool_search` permission rules, agent controls, hooks and ordinary output budgets. Loading a schema does not approve execution: fresh permissions, server authorization and registration identity still apply. Search neither starts servers nor invokes their tools. Plugin registration delivery remains incomplete.


## MCP workspace roots

Configured local connections advertise MCP roots: the canonical Location and existing files or directories in `sandbox.writable_roots`, resolved relative to the Location and deduplicated. Read-only policy advertises the Location alone. Roots use percent-encoded `file://` URIs and describe a connection snapshot (`listChanged: false`), so no `notifications/roots/list_changed` is sent. Each Location, including a worktree opened as its own Location, gets its own connection and sees only its own roots. An unavailable configured writable path refuses startup before native launch.

Changing `sandbox.policy`, `sandbox.writable_roots`, network isolation or allowed domains, the credential grant (`sandbox.readable_paths`, `sandbox.env.allow`) or the `sandbox.apply_to` selection replaces the running local server instead of leaving it on its launch-time roots and grants. Its tools, prompts, resources and instructions are withdrawn at once, a call to it fails with `MCP sandbox configuration changed; reconnecting` without reaching the server, and on the next refresh the old process is stopped and a fresh one starts with the current roots and sandbox. A writable root that no longer exists counts as a change, and the server stays down until the path returns or the configuration is edited. Remote servers receive no roots and are not affected.

Servers can request `roots/list` during initialization, tool execution or idle processing. The list describes the workspace and leaves process sandbox grants and tool permissions unchanged. Private scratch, unrelated read-only paths and sibling worktrees are not added implicitly. Sampling is advertised only when enabled (see [Local MCP sampling](#local-mcp-sampling)).


## MCP server guidance

Connected local servers can supply usage instructions during initialization. Eligible instructions appear in a `<mcp_instructions>` context block, ordered by server name and attributed to each server. Server text is escaped to preserve block boundaries. Guidance remains available when schemas are deferred, while agent, Mode, permission and server-authorization controls determine eligibility. Higher client registrations that shadow every MCP tool also remove that server's guidance.

Instructions use the `core/mcp-instructions` Context Source (Sessions started with the earlier `mcp/instructions` key receive its withdrawal). Reconnection changes, connection loss, Location close and visibility restrictions update its applicability through mid-conversation system messages. The Epoch baseline remains byte-stable. Reading this source uses cached discovery metadata and starts no servers or RPCs. Remote MCP instruction delivery remains incomplete.

### Required local MCP startup

Set `required: true` on a server entry to hold a Session's first Turn until the server connects and publishes discovery, up to its configured `timeout` (default 30 seconds). A disabled, failed or unavailable required server reports `McpRequiredError: <name>` before prompt promotion or a model call; the original prompt remains pending for retry. Cancelling the wait preserves independently owned Location startup. Retry after recovery by waking the Session; unresolved native ownership remains fenced.

The requirement applies once per Session and survives replay and conversation rewind. A fork begins its own first Turn. The default is false and preserves existing server approval digests; enabling it changes the definition and requires matching authorization. Remote servers honor `required` the same way; a `needs_auth` server fails the first Turn with `McpRequiredError`.

### MCP output budgets and resolved options

Set a positive `output_token_limit` on an MCP server to cap its model-facing text using the four-Unicode-character token estimate. The global `tool_output.max_lines` and `max_bytes` ceilings still apply; an omitted cap inherits that global budget. The standard truncation notice follows the bounded payload and points to the managed file containing the full original text. Structured data is retained. This is an estimate rather than provider tokenizer accounting. Local success, error and structured results share this settlement path.

Run `cyber mcp get <name>` to review one loaded server's definition, provenance, approval digest, authorization observations and output budget without connecting or creating a database. Header/environment values, OAuth details and URL query values are redacted; numeric output limits and the effective `headers_refresh_seconds` remain visible. A project-defined `headers_command` is shown verbatim so it can be reviewed before `cyber mcp trust` approves its digest; a command from your own global configuration is redacted. For a project-defined server, any `url`, header or `env` value written with an `{env:NAME}` or `{file:path}` placeholder is shown as that template, for example `X-Trace: {env:GITHUB_TOKEN}`, so you can see which of your local values the server would receive. The resolved value is never printed. Unknown or withheld names fail review.

A remote server may set `headers_command`, a shell command run outside the sandbox (only after the definition is approved) whose stdout must be a JSON object of string headers. Its headers override static `headers`, transport-owned names are refused and the command is re-run every `headers_refresh_seconds` (default 3600, at most 86400) on the live session, before the next request once the interval has elapsed. A failed run fails the connect. During a refresh it is treated as connection loss, so the reconnect re-runs the command. Diagnostics never contain the command output. A 401/403 on a live session of such a server also triggers a reconnect, and the server becomes `needs_auth` only if the freshly run command's headers are refused too. A server selected by `sandbox.apply_to` (or connected by a sandboxed MCP-tool hook) cannot use `headers_command`: the connect fails before the command runs.

### Remote MCP OAuth

Remote servers that answer 401 can be signed in with OAuth 2.1. Sign-in is always a user action: a Session, `cyber exec` or the background service never starts one and never opens a browser. A refused server settles `needs_auth` with ``MCP server <name> needs authentication; run `cyber mcp auth <name>` ``, its tools are withdrawn, it is not retried, and a `required: true` server fails the first Turn.

The optional `oauth` field of a remote entry controls this:

```json
{
  "mcp": {
    "github": {
      "type": "remote",
      "url": "https://mcp.example.com/mcp",
      "oauth": {
        "client_id": "cyber-cli",
        "client_secret": "{env:GITHUB_MCP_CLIENT_SECRET}",
        "scopes": ["repo.read", "issues.write"],
        "redirect_port": 53682
      }
    }
  }
}
```

Omit `oauth` or set it to `true` to use defaults (dynamic client registration, the scope from the server's challenge or metadata, an ephemeral callback port). Set it to `false` to disable OAuth: stored tokens are then neither sent nor refreshed. In the object form every field is optional and unknown fields are refused; `client_secret` requires `client_id` and should use `{env:NAME}`; scopes are nonempty tokens without whitespace; `redirect_port` is 1024 to 65535. The `oauth` value is part of the server's approval digest, so a project-defined server needs `cyber mcp trust` again after it changes. `cyber mcp get`/`definitions` show `client_id`, `scopes` and `redirect_port`, never the secret (a project-defined secret shows only its `{env:}` template).

`cyber mcp auth <name> [--port N]` signs a server in:

1. It checks workspace trust and, for a project-defined server, its approved digest before sending anything.
2. It sends one unauthenticated `initialize` to read the `WWW-Authenticate` challenge, then fetches protected-resource metadata (the challenge's `resource_metadata`, then `/.well-known/oauth-protected-resource<path>`, then the root well-known URL) and the authorization server's RFC 8414 or OpenID Connect metadata. The metadata must name exactly the configured URL and issuer, every endpoint must be HTTPS (plain HTTP only to a loopback host, and a local address only when the server, or for endpoints the issuer, is itself local) and PKCE S256 must be offered, or the command fails without an authorization request.
3. Without `oauth.client_id` it registers a client dynamically. If the server offers no registration endpoint it fails with a `needs_client_registration` diagnostic naming `oauth.client_id`; configure one and run `cyber mcp auth` again.
4. It prints the authorization URL to stderr. Open it in a browser; the server redirects to `http://127.0.0.1:<port>/mcp/oauth/callback`, which accepts one callback with the matching `state` and times out after 5 minutes. The code is exchanged with the PKCE verifier and the RFC 8707 `resource` (the server URL without its query, so a key in the query is never sent to the authorization server or printed).
5. It stores the tokens and, if the background service is running, asks it to reconnect the server (`POST /api/v1/mcp/{name}/reconnect`), reporting the result as `reconnect` in its JSON output. No token is printed.

All sign-in traffic goes through the same network policy and owned proxy as the server itself, and follows no redirects. OAuth needs an `https://` server URL (plain `http://` only to a loopback address or `localhost`): `cyber mcp auth` refuses any other server before contacting it, and a stored token is never sent to one.

Tokens are stored for the server name at its exact URL and issuer. Where they go depends on the platform and is fixed when Cyber Code is built; no environment variable, configuration or project file changes it:

- **macOS and Windows:** the OS keyring (Keychain, Credential Manager) holds the access token, refresh token and client secret under the service `cyber-mcp-oauth`, split into entries of at most 2560 bytes. Entry names start with a random id of the data directory, so two data directories (two `CYBER_HOME`s) of one user never share or delete each other's tokens. A secret too large for 8 entries is refused with `credential too large for the OS keyring`, and nothing is stored. Everything else (issuer, client id, scopes, expiry, remembered conditions) is in `<data>/mcp-auth-index.json`, which never contains a token or client secret and never records the server URL itself (an issuer or token endpoint may share its origin). The keyring also holds a digest of each index record, so a record edited by someone without keyring access is refused rather than used. A missing or altered keyring entry, or an edited index record, is treated as no valid credential, never as a shorter token.
- **macOS upgrades:** tokens an earlier release saved in `<data>/mcp-auth.json` keep working. They move to the Keychain the next time they are written (`cyber mcp auth`, or a refresh), and `cyber mcp logout` removes them from both places. If the Keychain refuses a deletion (for example a declined prompt after an upgrade), logout fails instead of reporting success; run it again to finish.
- **Windows:** a file left at `<data>/mcp-auth.json` is never used. While one exists, every sign-in, stored-token lookup and logout fails with a storage error, so remove it.
- **Linux and other platforms:** `<data>/mcp-auth.json`, created with mode 0600. Linux has no keyring backend that works without libdbus, so this file is the fallback. A file that group or others can read is refused.

Changing a server's `url` means its old token is never sent; sign in again. Tokens never appear in configuration, logs, events, status or command output.

Every request to the server carries headers in this order, later layers replacing the same name: static `headers`, then `Authorization: Bearer <token>`, then `headers_command` output. A token that expires within 5 minutes (or within half its lifetime, for tokens issued for less than 10 minutes) is refreshed before connecting and before each call, catalog refresh or idle ping on a live connection, and the rotated refresh token is saved before use. Connections and processes sharing a stored credential refresh it one at a time (other servers' refreshes do not wait) and pick up each other's rotated tokens, a refresh never writes back a credential that `cyber mcp logout` removed, and a refresh that does not finish within the server's `timeout` counts as failed. A 401 to a token that has a refresh token is retried once after one refresh; if that is refused too, or the refresh fails, the server settles `needs_auth`. A 403 with `error="insufficient_scope"` settles `needs_auth` at once and remembers the extra scopes, which the next `cyber mcp auth` requests.

`cyber mcp logout <name>` removes every stored token and remembered condition for the name (at any URL or issuer) without reading configuration, and, if it removed anything, asks a running service to reconnect the server, which then settles `needs_auth`. A connection that reconnect does not reach stops using the removed token at its next call or idle ping and settles `needs_auth` too. MCP-tool hooks send the stored token like any other connection. Tokens are not revoked at the authorization server.

To reconnect a `needs_auth` or `needs_client_registration` server after signing in from another tool, or a server that settled `failed` without a retained native owner, call `POST /api/v1/mcp/{name}/reconnect` (SDK `mcp.reconnect`). It returns `connecting`, then publishes the usual `mcp.status.changed.1` transitions; an unknown server is 404, an unapproved one 403, and a server in any other state, with a changed definition (except a `needs_client_registration` server whose `oauth.client_id` was just configured) or in a closing Location 409.

### MCP call timeouts and progress

An explicit server `timeout` applies to both connection startup and tool-call inactivity. When omitted, connection startup defaults to 30 seconds and calls use `mcp.tool_timeout` (default 300 seconds). `cyber mcp get <name>` reports the resolved `call_timeout_seconds`. Definitions with an explicit timeout require fresh matching approval under these corrected call semantics; definitions with omitted defaults preserve existing approval digests.

Valid progress for the active request resets inactivity. Values must advance, including fractional values; duplicate/regressive progress, foreign tokens, malformed optional fields and messages carrying a request identity do not extend the timer. A timeout leaves remote completion unverified until native ownership settles. Calls are not replayed automatically.

When a call is interrupted, times out or its Location closes before the server answers, Cyber Code sends the server one `notifications/cancelled` for that request (on a remote session, before the session is closed), waiting at most one second. It is skipped when the definition is no longer approved or its stored OAuth credential was removed. A failure is logged without header values and never delays shutdown.

### Graceful local MCP close

Explicit Location close and runtime shutdown withdraw MCP tools before stopping shared servers. On POSIX, client IO closes and the owned group receives SIGTERM. The leader remains unreaped for the full five-second grace period so descendants can finish cleanup after the leader exits. SIGKILL then stops remaining group members, followed by native acknowledgement, local proxy shutdown and durable settlement before scratch/checkout cleanup. Windows uses owned Job Object termination.

Independent servers close concurrently. An interrupted close retains its original grace deadline and Location admission fence; retry resumes that deadline. Unknown ownership or failed settlement remains fenced for recovery. Failed startup, cancelled calls, lost connections and owner disposal retain immediate cleanup, with disposal providing no durable acknowledgement. Remote close joins the HTTP bridge and its proxy and sends the session `DELETE`; settling MCP owners when an embedded `cyber exec` exits and full native platform acceptance remain incomplete.

### MCP form elicitation

Local stdio and remote (Streamable HTTP and SSE) servers advertise form elicitation. During an interactive tool call, `elicitation/create` opens the existing question flow, names the requesting server, and asks for explicit Accept, Decline or Cancel before returning values. A remote server may send the form on the call's own event stream or on its GET event stream; the answer is posted back on the same session. Flat strings, numbers, integers, booleans and string-enum arrays retain their JSON types; supported constraints and formats are checked before sending. Optional fields can be omitted and declared defaults require an explicit choice. Decline, Cancel and invalid answers send no form values, and an invalid form schema is refused without asking you anything.

Authorization is checked before asking and again before sending answers: a changed or untrusted definition, or a `cyber mcp logout` while the question is open, sends Cancel instead of your answers. `cyber exec`, other non-interactive sessions, and requests during startup or idle decline at once without creating a question; logs omit form text and answers. Human question time pauses call inactivity. Interrupting the call or closing the Location dismisses its pending questions and sends the server a best-effort `notifications/cancelled`; if the server cancels the form itself, or the call finishes while you are still answering, the question is dismissed and no answer is sent.

Only form mode is supported. A URL-mode request is refused: Cyber Code never opens a browser or fetches a link for a server, and the URL is not shown or logged. The remote-server cases pass on native Windows in CI; the local stdio cases are covered on macOS and Linux only.

### Local MCP sampling

Set `mcp.sampling.enabled` to `true` to advertise basic sampling on newly connected local stdio and remote servers. It defaults off. Close and reopen the Location after enabling it; disabling it also refuses requests on existing connections. Requests during startup or idle have no active call owner and are refused. Sampling advertises no tools or ambient-context support and accepts bounded text/image messages; audio and tool-enabled requests are refused.

Each active-call request checks `mcp_sampling` permission for the server name, selects `model_roles.small`, and checks Session and ancestor budgets after approval. Model hints cannot select another provider. Sampling receives only the server's input, with tools disabled and no Session history or cache identity. Usage records use `mcp_sampling:<server>` and the originating call ID. Soft budget overrun records usage and withholds the response; it is not a token reservation.

Human approval and nested sampling pause outer MCP call inactivity; the provider call has its own configured call timeout. Cancellation retains the nested task until local provider transport shutdown and observed billing settle. Native OpenAI-compatible, OpenAI Responses and Anthropic socket tests cover timeout and Location close. Local socket closure does not prove remote rollback or final usage; unsupported custom transport cancellation remains unverified. Sampling input and credentials are excluded from Session events. Remote servers send `sampling/createMessage` on a call's response stream and get the same permission, model and billing path. Complete native platform acceptance remains open.

### Organization MCP policy

The managed policy document (see **Managed hooks** above for its location and ownership rules) can also restrict and provide MCP servers:

```jsonc
{
  "policy": { "mcp": {
    "deny": [{ "url": "https://*.untrusted.dev/*" }],
    "allow": [{ "name": "github" }, { "command": "npx @corp/*" }, { "url": "https://mcp.corp.example/*" }]
  } },
  "mcp": { "corp": { "type": "remote", "url": "https://mcp.corp.example/mcp" } }
}
```

Each pattern has exactly one of `name`, `command` (the command and its arguments joined by spaces) or `url`, matched with `*` wildcards. A `url` pattern is matched against the parsed server URL with a lowercase scheme and host, no default port and at least a `/` path; its `scheme://host[:port]` part is case-insensitive and matches any port when it names none, so `https://API.untrusted.dev:443` is still caught by `https://*.untrusted.dev/*`. A server matching a `deny` pattern, or (when `allow` is present) matching no `allow` pattern, is never started or contacted: `GET /mcp` and the TUI report it `failed` with `blocked by organization policy`, and `cyber mcp get|definitions` show `"blocked"`. Servers in the document's `mcp` map are provided to every Location, replace any user or project server of the same name whole, need no checkout approval and are exempt from the lists. A document that cannot be used blocks every non-managed server.

### MCP tool annotations

MCP tool annotations follow the protocol defaults when a server omits them: a tool that is not `readOnlyHint: true` is treated as destructive unless it says `destructiveHint: false`, and every tool is open-world unless it says `openWorldHint: false`. In `auto` mode a destructive tool is always evaluated by the classifier, even when a rule, a saved approval or a hook allows it; a classifier fallback asks. When the sandbox enforces `network: "off"`, an open-world tool also needs `network` permission on `mcp:<server>` before it runs. Hook envelopes of MCP calls carry the annotations as `tool_annotations`.

## Plugins

A plugin is a directory with a `cyber-plugin.json` manifest that bundles skills, commands, agents, hooks and MCP servers, and, with `main`, code that runs in a sandboxed plugin host process and contributes tools. Providers, workflows, channels and mods are reported as not loaded.

```json
{ "name": "review-kit", "version": "1.0.0", "engines": { "cyber": ">=0.1.0" } }
```

`name` (`^[a-z][a-z0-9-]{1,63}$`) and `version` (semver) are required; `description`, `author`, `license`, `homepage`, `engines.cyber`, `dependencies`, `main`, `runtime`, `capabilities`, `userConfig` and component paths are optional. A missing, unknown or malformed field fails the plugin with an error naming it, for example `cyber-plugin.json: name: invalid plugin name "Lint Guard"`. Without a declared path a component comes from its conventional place: `skills/*/SKILL.md`, `commands/**/*.md`, `agents/**/*.md` (the Claude Code agent format), `hooks/hooks.json` (an event map, optionally wrapped in `{"hooks": …}`; controls such as `sandbox_all` are refused), `.mcp.json` (`mcpServers`) and `themes/*.json` (read by the TUI). `${CYBER_PLUGIN_ROOT}` (and `${CLAUDE_PLUGIN_ROOT}`) in hook and MCP strings expands to the plugin directory. A component never replaces an existing one: a taken name is available as `<plugin>:<name>` (`/review-kit:review` next to the built-in `/review`); MCP server names cannot hold `:`, so a taken server name becomes `<plugin>-<name>`.

```sh
cyber plugins install ./review-kit                    # a local directory, used in place (user scope)
cyber plugins install git+https://example.com/kit.git --ref main --path kit --scope project
cyber plugins install npm:@acme/review-kit@^2         # packed with lifecycle scripts disabled
cyber plugins list | info <name> | validate <path>
cyber plugins enable|disable|remove <name> [--scope user|project|local]
cyber plugins update [name]                           # fetch the source again and record the new digest
```

- **Scopes.** `user` is `<config>/plugins.json`, `project` is `.cyber/plugins.json` (commit it) pinned by `.cyber/plugins.lock`, and `local` is `.cyber/plugins.local.json`. A plugin loads when any scope enables it; the most specific scope that lists a name supplies it. A managed policy `{"policy":{"plugins":{"disabled":["name"]}}}` overrides every scope, and a malformed managed policy loads no plugin. `CYBER_DISABLE_PROJECT_CONFIG` leaves the project and local files unread.
- **Sources.** A path is used in place. A git source is cloned (no submodules, links as plain files), checked out at the resolved commit and copied to the content-addressed `<cache>/plugins/<name>/<version>-<digest>/`, shared by every scope and checkout: a fetch never replaces a tree that is installed somewhere, a declined or refused change deletes only what it fetched, and `remove` deletes a tree only once no scope file or lockfile still records its digest; the lockfile records the commit and the SHA-256 digest of the tree, and a repeated install keeps that commit until `cyber plugins update`. An npm spec goes through `npm pack --ignore-scripts`, so no lifecycle script runs and no dependency is installed. `<plugin>@<marketplace>` waits for marketplaces.
- **Trust.** A plugin loads only while its tree still matches the installed digest and that exact name, version and digest is approved in the trust store. `install` and `update` approve what they fetched. `enable` of a project or local plugin you have not approved shows its source, digest, hooks, MCP servers, agents and component counts and asks `Trust and enable <name>? [y/N]` (`--yes` skips the question). A changed tree stops loading (state `changed`) until `cyber plugins update`. `install` refuses a version whose `engines.cyber` excludes the running version (`requires cyber >=2.0.0, running 0.1.0`) unless `--force`. Signatures are not verified yet, so every plugin is listed as `unsigned`.
- **Runtime.** Plugin hooks run in scope `plugin`, after managed, global, project and local hooks, always sandboxed, and are skipped under `policy.hooks.managed_only`. Plugin MCP servers need no separate `cyber mcp trust`: the plugin approval covers them. They always run sandboxed to the plugin's declared capabilities, like its host: writes only to their temporary directory (and the Location with `fs.write`), and only the declared `network`. Plugin agents cannot widen their Mode, permissions or sandbox. Plugin command and skill templates may use shell injection without a template approval. `cyber hooks list`, `commands list`, `skills list` and `mcp definitions` show what a plugin contributed; `GET /api/v1/plugins` (SDK `client.plugin.list()`) lists every entry with its state. The TUI's `/plugins` lists them, `/reload-plugins` refreshes that list and the command catalog (a new plugin MCP server starts with the next Location), and startup shows once per project which project plugins still need `cyber plugins enable`.
- **Pure mode.** `--pure` or `CYBER_PURE=1` turns plugins off and loads hooks and MCP servers only from organization policy; built-ins stay. `cyber exec`, `cyber review` and the TUI then run their own in-process server (`--attach` is refused), `cyber api`, `cyber pr`, `cyber mcp serve` and `cyber workflows run|resume` are refused because they can only start Turns on the shared server, `cyber plugins list` and `/plugins` show `disabled by --pure`, and `cyber debug info` reports `external extensions disabled (--pure)`.

### Plugin hosts

A plugin that names `main` runs it as a host process, one per plugin and Location, started when a Session opens the Location. `runtime` is `node`, `bun`, `deno`, `python` or `native` (`main` is the executable); without it, `.js`/`.mjs`/`.cjs` run on Node, `.ts` on Deno, `.py` on Python and anything else as an executable. The core never loads plugin code itself: it speaks JSON-RPC 2.0 to the host over stdio, one message per line (protocol version `1`).

```json
{
  "name": "jira-kit",
  "version": "1.0.0",
  "main": "dist/index.js",
  "capabilities": { "network": { "hosts": ["acme.atlassian.net"] }, "secrets": true },
  "userConfig": {
    "type": "object",
    "properties": { "site": { "type": "string" }, "api_token": { "type": "string", "secret": true } },
    "required": ["site", "api_token"]
  }
}
```

- **Protocol.** `initialize` sends the protocol version, the plugin and Location, the declared capabilities and the validated settings; the host answers with its `protocolVersion` and `tools` (`name`, `description`, `inputSchema`, optional `annotations`). A different version keeps the host down with `plugin <name> protocol version mismatch: the plugin speaks X, cyber speaks 1`. Calls go out as `tool/execute` (`name`, `input`, `callId`, `sessionId`, `attempt`, `operationKey`) and come back as `{ content, isError?, structured? }`; a cancelled call sends `$/cancelRequest`, a settings change `config/changed`, and stopping a host sends `shutdown` and waits 2 s. A host may send `log`, `secrets/get` (needs `secrets`) and `permission/ask` (during one of its calls; `cyber exec` answers no). `session/prompt` is refused, and hooks, providers and catalog transforms a host declares are logged and ignored in this release.
- **Tools.** Host tools are offered with the built-in and MCP tools, under the same schema check, hooks, permission rules (`readOnlyHint: true` runs without asking and is offered in plan mode, but only from a plugin that declares none of `fs.write`, `network`, `shell` and `session.prompt`; any other tool asks by default) and output budget, and are deferred like MCP tools above `tool_output.deferred_threshold_tokens`. A built-in name always wins and the first plugin to claim a name keeps it. The first Turn waits for hosts to start; a plugin enabled during a Session offers its tools once its host is running.
- **Crashes.** A call whose host exits fails with `plugin <name> crashed` and the Turn goes on. The host restarts after 1 s, then 2, 4, 8 … up to 60 s for crashes in a row; the fifth crash within 10 minutes disables the plugin until the process ends (`plugin <name> disabled after repeated crashes`). Disabling, removing or updating a plugin stops its host before the next step and removes exactly its tools, hooks, commands and agents; no server restart is needed. `GET /api/v1/plugins` shows each host's `state` (`not_started`, `starting`, `running`, `restarting`, `disabled`, `failed`), `error`, `tools` and recent `crashes`.
- **Capabilities.** `fs.read`, `fs.write`, `network` (`true`, or `{ "hosts": [...] }` with `*.` wildcards), `shell`, `secrets` and `session.prompt`; anything else fails the manifest. `install` and `enable` print them before committing. The host always runs in the sandbox, whatever the Session's policy: it writes only to its own temporary directory, and to the Location with `fs.write`. Without `network` every connection is refused, and a refused host adds `[sandbox] plugin <name> was denied network access to <host>` to the call's result; `{ hosts }` allows only those hosts. `fs.read` and `shell` are not enforced yet: a host reads what any sandboxed command reads and may start subprocesses. On Windows the AppContainer grants the host its plugin root and, without `fs.write`, the Location read-only. Where the sandbox is unavailable (Linux without bubblewrap) no host starts.
- **Settings.** Values live under `plugins.<name>.config` and are checked against `userConfig`; an invalid or missing required value keeps the host down with an error naming the setting. A property marked `secret` is never read from a configuration file: it is stored in the OS keyring (macOS Keychain, Windows Credential Manager; an owner-only `<data>/plugin-secrets.json` on Linux, which every sandboxed command, hook, MCP, LSP and plugin host process is denied, like `<data>/mcp-auth.json`) and reaches the host only through `secrets/get`. Secrets belong to one plugin in one scope and checkout: a same-named plugin in another scope or checkout (say, from a cloned repository) never reads them, and `remove` deletes only the removed entries' secrets. `install`, `enable` and `update` ask for required values that are not set (secrets without echo on a terminal); `cyber plugins configure <name>` asks for every setting, `--set key=value` sets an ordinary one, and `remove` deletes the stored secrets.
- **TypeScript.** `@cyber-code/plugin` (`sdk/plugin`) provides `definePlugin`, `tool` (JSON Schema or Zod-style input), typed hook handlers, `provider` and an in-memory `createHarness`; `export default definePlugin({ tools: { hello: tool({...}) } })` is a complete `main`.

## Commit and PR text

`cyber git commit-msg` prints a Conventional Commits message for the staged diff, written by the small model (`model_roles.small`, else `small_model`, else the default model) with no tools. The model sees only what the bundled reviewer could read: a changed file whose read would ask (such as `.env`) or is denied is listed by name with its diff withheld, and credentials in the rest are redacted. Paths are always repository-relative, whatever `diff.relative` says, so this holds from a subdirectory too. `cyber git pr-description [--base BRANCH]` prints a PR title and body for `BRANCH..HEAD` (default `origin/HEAD`, else `main`, else `master`). Both refuse before any model request when nothing is staged or the range has no commits, and accept `--format json`. In the TUI, `/commit` and `/pr-description [base]` ask the server (`POST /api/v1/git/commit-message` and `/api/v1/git/pr-description`) and prefill `!git commit -m '…'` or `!gh pr create --base … --title … --body …` in the composer; nothing runs until you submit it.

Generated text has no `Co-authored-by` trailer: any trailer the model writes is removed, so commits are attributed to you alone. Set `git.co_author` to add one:

```jsonc
{ "git": { "co_author": "Cyber Code <bot@cyber.dev>" } }
```

## Pull request checkout and linked Sessions

A Session is linked to a pull request when its `bash` or `powershell` tool, or a `!` command you run in it (such as the `gh pr create` that `/pr-description` prefills), runs `gh pr create` (the PR URL comes from the output) or `git push` (the PR comes from `gh pr view --json number,url` in the command's working directory, after a successful command). That lookup never asks: it runs only when the Session's `bash` rules and Mode allow it outright and `PreToolUse` hooks admit it, with `gh` taken from absolute `PATH` entries outside the project and run in the command sandbox, so a `gh *` deny rule means no `gh` runs. With `sandbox.network: proxy` it reaches only allowlisted or already approved domains: it raises no network prompt, and a hook's allow of the push never becomes a Session approval of the lookup's domain. `cyber pr 42` (or a PR URL) runs `gh pr checkout 42`, which adds a fork remote when needed, then opens the TUI on the most recent Session linked to that PR, or on a new Session. `cyber --from-pr 42` opens the Session picker over the Sessions linked to PR 42. With `--format json` both print what they found (`{pr, checked_out, session_id, resumed}` and `{pr, sessions}`) instead of opening the TUI. `GET /api/v1/sessions?pr=42&children=true` lists linked Sessions for other clients.

## Cross-session messaging

Sessions on one server can send each other short text messages. A message is plain UTF-8 text of 1 to 16384 bytes without NUL; it reaches the target as one untrusted `<cross_session_message>` block, never as instructions, and its records keep metadata (and the text, unless it was refused) in both Sessions.

**Model tools.** `list_sessions { directory? }` lists the Sessions this Session can message (at most 200, newest first) with `address`, `kind` (`subagent`, `teammate`, `local`), `state` (`idle`, `running`, `waiting_input`), Location and `canReceive`. `send_message { to, text, notify_when_idle? }` sends one message and returns `{ message_id, outcome, reason?, limit?, to, watch_id? }`; `outcome` is `delivered`, `held` or `refused`. `watch_session { target }` subscribes to one notice when the target next goes idle (a watch expires after 12 hours; at most 30 active watches per Session). The `message.send` permission is checked on the target's `ses_` ID and its name: same-server targets are `allow` by default and any `*@*` address is `ask`; `list_sessions` and `watch_session` are allowed. A `deny` rule on a name pattern (for example `"message.send": {"noisy-*": "deny"}`) refuses the target whichever address names it.

**Addressing.** `to` resolves on the server, relative to the sender, in tiers, and the first tier with a match decides (several matches there are `AmbiguousAddressError`, with the candidates): an exact `ses_` ID; an address `X@P` (Session `X` on the configured peer `P`, through its peer listener; `PeerNotConfiguredError` when no enabled `peers.remotes` entry is named `P`; one leading `@` is a mention and is stripped); the sender's team (empty until agent teams ship); its subagents by `subagent_name`; the exact title of a non-archived root Session. No ID prefix or fuzzy match resolves, and a Session cannot message itself.

**Inbound rules.** Each Location's `messaging.inbound` decides what a target does with a message: `default` (`deliver`, `hold` or `refuse`; `deliver` when unset) and the pattern lists `deliver`, `hold` and `refuse`, matched with the permission wildcard syntax against the sender's addresses: a Session's ID and its title, `client:<label>` and `client` for an API or CLI client (`X-Cyber-Client`, else `api`; `cyber messages send` is `client:cli`), and `mcp:<label>` and `client` for an MCP serve client without a caller Session. A `refuse` match wins over `hold`, and `hold` over `deliver`; without a match the Session's own override applies, then `default`. Only addresses no other Session can claim satisfy `deliver` or `hold`: the sender's ID, a client's labels, and the title of a root Session no other non-archived root shares when it is a plain name (not `client`, `client:…`, `mcp:…`, a `ses_…` ID or an `@` address). Every other name (a shared or ID-like title, a subagent name, the address the target replies to) and every ancestor's ID and names only restrict, so a subagent of a refused Session is refused too. A held message is admitted with `delivery: "hold"` and waits until the user releases (`r`) or drops (`d`) it; it can never be edited. An archived target refuses (`archived`), and so does a Location where messaging is disabled (`disabled`).

**Session override.** `POST /api/v1/sessions` and `PATCH /api/v1/sessions/:id` take `messaging: { "inbound": "deliver" | "hold" | "refuse", "reason"?: "non_interactive" }` (`null` clears it). It replaces only the Location's `default`; explicit patterns still win. `cyber exec` creates its Sessions (forks included) with `{ "inbound": "refuse", "reason": "non_interactive" }`, so other Sessions' messages are refused with `non_interactive`; `cyber exec --accept-messages` leaves them on the Location's rules. `cyber exec --session` reuses a Session with its own policy and prints a warning on stderr when that differs from what the run asked for (a reused Session that receives messages, or `--accept-messages` on one that refuses them).

**Configuration.**

| Key | Default | Meaning |
| --- | --- | --- |
| `messaging.enabled` | `true` | `false` disables messaging in the Location: its Sessions cannot send or watch, and it refuses messages (`disabled`) |
| `messaging.inbound.default` | `deliver` | Policy without a matching pattern or Session override |
| `messaging.inbound.deliver`, `.hold`, `.refuse` | `[]` | Sender patterns; `refuse` lists concatenate across layers, the others replace |
| `messaging.max_hops` | `8` (1–64) | Longest message chain (see below) |

The OS-managed `policy.messaging.enabled: false` (only from the managed policy, so no user or project layer can re-enable it) and the environment switch `CYBER_DISABLE_MESSAGING=1` disable messaging for the process: the tools are not offered, and sends and watches fail with `ServiceUnavailableError` (service `messaging`). Invalid values fail the configuration load naming their path. Workspace trust gates project `messaging` that widens the inbound boundary: `enabled: true`, a `default` other than `refuse`, a non-empty `deliver`, any `hold` and `max_hops` stay inactive until the checkout is approved, while `enabled: false`, `default: "refuse"`, an empty `deliver` and `refuse` patterns (which only add to the user's) apply at once.

**Limits.** A sender (its Session ID, `client:<label>` or `mcp:<label>`) may have at most 10 messages accepted (`delivered` or `held`) by one target in a sliding minute, and a target holds at most 30 unpromoted messages; past either, the message is refused with `rate_limited` and `limit: "per_target_minute"` or `"inbox_pending"`, recorded only in the sender. The window is read from the durable records, so a restart does not reset it. A refusal that the target records too (`inbound_rule`, `non_interactive`, `archived`, `disabled`) is bounded the same way: past 10 of one sender in a minute it is recorded only in the sender, as `rate_limited`. A run's hops are the largest `hops` of the messages and idle notices it promoted, and input the model caused (a background Job's handback or Monitor line, a subagent's prompt, a Workflow summary) carries the hops of the run that caused it, so only user input starts a chain at 0. A message carries the sending run's hops + 1 and is refused with `chain_limit` past the smaller `max_hops` of the two Locations. A target that is a settling child is retried once, then the send is refused with `busy`.

**Routes.** `GET /api/v1/messaging/sessions?from=&directory=` lists reachable Sessions as `from` (as the client without it). `POST /api/v1/messaging/send` takes `{ id?, from?, to, text, notify_when_idle? }` (an `Idempotency-Key` derives `id`; a repeated ID returns the original result with `replayed: true`; `X-Cyber-Client` labels the client sender and cannot start with `mcp:`) and answers the outcome. `POST /api/v1/messaging/watch` takes `{ from, target }`. `GET /api/v1/sessions/:id/messaging` pages a Session's records (`cursor`, `limit` 1–200, `direction`, `order`). Errors: `MessageTooLargeError` (413), `AddressNotFoundError` (404), `AmbiguousAddressError` (409, with `candidates`), `AccountRequiredError` (403), `ConflictError` (an ID sent before with another text, sender or target), `RateLimitedError` `watch_quota` (429), `ServiceUnavailableError` (messaging disabled where the sender runs) and `InvalidRequestError`. Refusals are outcomes, not errors.

## MCP serve (partial P1)

`cyber mcp serve` lets another agent (Claude Code, Codex, another Cyber Code, an IDE) drive Cyber Code Sessions over MCP. It is a stdio bridge to the shared local server, which it starts with `cyber service start` semantics when none is running. The MCP endpoint itself is Streamable HTTP at `/api/v1/mcp/serve` on that server. Sessions created or prompted this way are ordinary Sessions: they appear in the TUI, `cyber sessions` and the SDK.

### Setup

Claude Code:

```bash
claude mcp add cyber -- cyber mcp serve --client claude-code
```

Codex (`~/.codex/config.toml`):

```toml
[mcp_servers.cyber]
command = "cyber"
args = ["mcp", "serve", "--client", "codex"]
```

An HTTP client connects directly to the endpoint. `cyber mcp serve --transport http` makes sure the server runs, then prints the URL (for example `http://127.0.0.1:4747/api/v1/mcp/serve`) and exits. It never prints the password. Use HTTP Basic with user `cyber` and the password from `cyber service password`, and keep that password as secret as the password file it comes from. Only loopback peers with a loopback `Host` are accepted, so the endpoint cannot be reached from the network or through DNS rebinding, even when `cyber serve --hostname` binds another address. `--client <label>` (`[a-z0-9][a-z0-9_-]{0,47}`) labels the Sessions and prompts for attribution; without it the label comes from the client's `clientInfo.name`.

### Tools

- `cyber_session_create`, `cyber_session_list`, `cyber_session_read` and `cyber_session_transcript` create and read Sessions. `cyber_session_read` also lists the 20 newest prompts admitted through MCP serve (`recent_prompts`).
- `cyber_prompt` sends a prompt to a Session (`session_id`) or to a new Session in an absolute `directory`. The prompt goes through the same user path as the SDK, so `UserPromptSubmit` hooks run. It is always literal text: `/mode bypass` or any other slash command is never run. By default the call waits up to `wait_seconds` (default 50, clamped to `max_wait_seconds`) and returns a `PromptResult`: `status` (`queued`, `running`, `waiting_for_approval`, `completed`, `failed`, `interrupted`, `budget_exceeded` or `refused`), `final_text` (the last answer, at most 64 KiB), usage and the receipt (`session_id`, `message_id`, `admitted_at`). Reaching `wait_seconds` is not an error: the result carries the receipt. With `wait: false` it returns the receipt right away. A run is done once its Session is idle and every background Job the run started has settled and its handback Turn finished.
- `cyber_prompt_wait` re-attaches to a prompt by `session_id` and `message_id`, also after a server restart; `wait_seconds: 0` polls.
- `cyber_interrupt` interrupts a Session. Waits on its run end `interrupted`.
- `cyber_message_list`, `cyber_message_send` and `cyber_message_history` use cross-session messaging. `cyber_message_list` (optional absolute `directory`) lists the Sessions a message can reach, with `address` and `canReceive`. `cyber_message_send` (`to`, `text` of at most 16 KiB, optional `idempotency_key`) returns `{ message_id, outcome, reason?, to, replayed? }`; addressing, inbound rules, rate limits and errors (`MessageTooLargeError`, `AmbiguousAddressError`, `AddressNotFoundError`) are those of `POST /api/v1/messaging/send`. When the call comes from a Cyber Session (the caller marker or `X-Cyber-MCP-Caller`), the message is sent as that Session, so the target's rules on its address apply; otherwise it comes from the MCP client `mcp:<label>` (origin `client`; inbound patterns `mcp:<label>`, `mcp:*` and `client` match it, and its rate key is `mcp:<label>`), which has no address to reply to. Sending to the calling Session or one of its ancestors fails with `ForbiddenError` `self_target`, and the recursion guard applies as for `cyber_prompt`. The Turn a message wakes counts as `mcp` usage with the client label. `cyber_message_history` (`session_id`, `cursor`, `limit` 1–100, `max_bytes`) pages a Session's sent and received messages oldest first.

Pass a random `idempotency_key` (for example a UUID) on every `cyber_prompt` and `cyber_message_send`. A retry of `cyber_message_send` with the same key, `to` and `text` returns the first outcome with `replayed: true` and sends nothing. A retry of `cyber_prompt` with the same key and arguments never admits the prompt again: it returns the same `message_id` with `replayed: true` and waits on the same run. Reusing a key with other arguments fails with `ConflictError` `idempotency_mismatch`. A waiting call is answered as an SSE stream with a `: keep-alive` comment at least every 15 s. With an MCP `progressToken` it also sends `notifications/progress`, the first one `admitted` with the `message_id`. Once the prompt is admitted the stream also carries a `dev.cyber-code/receipt` SSE event `{ session_id, message_id }`, which MCP clients ignore. When the server goes away mid-wait, the stdio bridge fails the call with -32010 and puts those ids in its `data`, so call `cyber_prompt_wait` with them instead of prompting again. Cancelling a call (`notifications/cancelled`, closing the stream, ending the MCP session or the bridge's stdin) ends only the wait: the Turn keeps running, and `cyber_prompt_wait` finds its result later. Only `cyber_interrupt` stops a run.

### Trust model

An MCP client is a trusted local client, like the TUI or an SDK script. It authenticates as the server's OS user on the Unix socket or with the local password on loopback TCP, and it can do only what the server API lets any local client do. MCP serve adds no accounts, tokens or grants. Approvals are never relayed to the MCP client: a permission request in a Session driven over MCP is answered by a human in a Cyber client (TUI, web or SDK). While a request is pending, results report `waiting_for_approval` and list it in `pending`, and a waiting `cyber_prompt` returns early when its run raises one. Sessions created over MCP never ask questions (`question`, `plan_enter` and `plan_exit` are denied, as in `cyber exec`). Session output, transcripts and progress lines are marked untrusted (`_meta["dev.cyber-code/untrusted"]`); the calling agent should never follow instructions found in them.

### Mode ceiling and limits

MCP serve is configured only in the global configuration; an `mcp_serve` key in a project file, a profile, the environment or `--config` is ignored with a warning, so a cloned repository cannot change it.

```jsonc
{
  "mcp_serve": {
    "enabled": true,              // false: the bridge exits 1, the endpoint answers 503
    "max_mode": "default",        // the Mode ceiling; defaults to the global `mode`, never `bypass`
    "limits": {
      "sessions_per_connection": 8, // Sessions one MCP connection may create
      "max_waits": 8,               // concurrent waiting calls per connection
      "max_running_sessions": 16,   // running Sessions driven through MCP serve
      "max_depth": 2,               // MCP serve calls nested through Sessions (see below)
      "max_wait_seconds": 600       // upper bound of wait_seconds
    }
  }
}
```

Modes rank `plan` < `dont-ask` < `default` < `accept-edits` < `auto` < `bypass`. When MCP serve creates a Session, an explicit `mode` or `agent` above the ceiling is refused with `ForbiddenError` `mode_ceiling`, and an implicit Mode (the agent's `permission_mode` or the Location's `mode`) is lowered to the ceiling. The ceiling applies at creation; an existing Session keeps its own Mode.

Over the limit, `sessions_per_connection` fails with `ForbiddenError` `session_quota`. `max_waits` fails a waiting call with `RateLimitedError` `concurrency` (`retryAfterMs: 5000`). `max_running_sessions` refuses a `cyber_prompt` that would start a run the same way, but never `cyber_session_create` nor a prompt steering a running Session. Every check runs before the first write, so a refused call creates and admits nothing. A retry under a stored key replays before these checks (except `max_waits`) and is never counted again. Fixed bounds: request body 1 MiB, prompt 128 KiB (`too_large`), `final_text` 64 KiB (head kept), transcript items 16 KiB and pages by `max_bytes`. Every truncation keeps the JSON valid.

Recursion guard: a Session that drives this server through MCP (a configured `cyber-code` MCP server, or `cyber mcp serve` run from its shell) is marked as the caller. Cyber's MCP client sends `_meta["dev.cyber-code/caller"]` to `cyber-code` servers, and the bridge forwards `CYBER_SESSION_ID` and `CYBER_MCP_DEPTH` as `X-Cyber-MCP-Caller` and `X-Cyber-MCP-Depth`. An external agent calls at depth 0; a call from a Session's run is one deeper than that run. At depth ≥ `max_depth`, `cyber_session_create` and `cyber_prompt` fail with `ForbiddenError` `depth_exceeded`. `cyber_prompt` and `cyber_interrupt` on the calling Session or one of its ancestors fail with `self_target`. Reading and waiting are never limited. The markers guard against accidents between trusted local clients; they are not a security boundary.

Not implemented yet: the messaging tools (`cyber_message_*`, waiting for the Messaging API, so messages carry no depth either), usage attribution with origin `mcp`, and the non-interactive rule for Sessions with no human attached. Until that rule lands, the shared server counts as attended, so a permission request waits for a human (reported as `waiting_for_approval`) instead of being resolved by the Mode.

## Memory management (partial P1)

```bash
cyber memory path                 # this project's directory; global outside Git
cyber memory list --global        # metadata and invalid-file diagnostics
cyber memory show coding-policy
EDITOR='code --wait' cyber memory edit coding-policy
cyber memory delete coding-policy
cyber debug memory --global
```

`edit` opens a private draft using EDITOR, or VISUAL when EDITOR is unset. It validates the note and updates its index through the memory journal after the editor exits successfully. Changes to the original note or index during editing refuse the commit. Drafts remain in private `<data>/.memory-edit-*/note.md` directories, including after success; errors identify the retained draft. An unchanged new template creates no note. The draft must remain a private regular file without symlink or hard-link aliases. List/show/path/edit/delete open no database and start no model work. Recovery starts no model work and only checks an existing database read-only.

Disabled or read-only settings refuse mutations; list/show/path/debug remain explicit manual review. Unix mutation support is implemented locally. Windows privacy/durability, multi-client retention and complete native acceptance remain open. API mutations and durable event delivery are implemented locally. Sessions receive committed index changes at their next Safe Boundary.

Authenticated clients can list metadata with `GET /api/v1/memory?scope=project|global` (default project) and read a note with `GET /api/v1/memory/{scope}/{name}`. Responses carry the standard Location envelope. Listing returns `memories` and safe `invalid` diagnostics; reading returns `metadata` and `body`. The generated TypeScript client exposes `client.memory.list({scope})` and `client.memory.get(scope, name)`. These explicit user reads remain available when automatic memory is disabled. Missing notes return MemoryNotFoundError; busy, pending or unsafe storage refuses without automatic recovery. On Unix, PUT /api/v1/memory/{scope}/{name} accepts a strict JSON object containing `content` (the complete Markdown document), and DELETE accepts no body. Both return a durable MemoryChange and publish memory.updated.1. The SDK exposes memory.put(scope, name, {content}, options) and memory.delete(scope, name, options). Idempotency-Key protects completed replay and refuses conflicting or unresolved requests; pre-admission contention returns retryable 503. Disabled/read-only settings refuse fresh mutations. Windows mutations and complete legacy/client recovery controls remain open.

Before an API edit, call `GET /api/v1/memory/edit/{scope}/{name}` (SDK `client.memory.editReview(scope, name)`). Its Location envelope contains `name`, `original` Markdown (null for an absent note in an existing scope), and `fingerprint`. Missing scopes return 404 without creation. PUT may include `review_fingerprint`; DELETE may send `X-Cyber-Memory-Review`. These conditional mutations check the scope, target/index content and file identities, and valid catalog contents before durable admission and again before journal preparation. Changed files return 409 and preserve the user's edits. Fresh disabled/read-only mutations remain forbidden. An identical acknowledged request with its retained key replays even after its review is consumed; a changed review under that key conflicts. Reviewed TUI editing is implemented locally; private Unix client checkpoints are implemented; multi-client retention and Windows mutations remain open.

Memory writes through the running model-tool host now publish `memory.updated.1` with a durable independent aggregate and a public `MemoryChange` receipt. Notifications contain the note name and mutation identity rather than the note body. An unresolved mutation refuses further writes of that scope until reviewed recovery; ordinary storage reads also refuse while a completed journal awaits durable acknowledgement. CLI writes retain their existing local storage behavior. Reviewed database/file reconciliation controls and complete native acceptance remain required.

The shared core now exposes read-only `MemoryScope::inspect_recovery()` and fingerprint-bound `recover_reviewed()` / `recover_reviewed_with_acknowledgement()`. Inspection returns the validated proposed note, original mutation identity and completion status. The fingerprint binds the scope/journal identities, known journal artifacts, current target/index and catalog. Recovery compares that state before normalization or installation; changed content or replaced target/staging inodes require a new review. Unsafe aliases and corrupt evidence refuse without cleanup. Failed acknowledgement preserves the completed journal and requires a new completed-state review. The CLI exposes these interfaces through `memory recovery` and `memory recover --review`; HTTP pinned-journal reconciliation is available; CLI server reconciliation is available; TUI paired reconciliation is available; legacy resolution remains open. A fingerprint grants no authority to clear a pending database admission.

Run `cyber memory recovery [--global]` to inspect an interrupted local transaction, including proposed note content, completion status and its review fingerprint. With `--format json`, absent evidence is `null`; inspection creates no memory scope or database. After reviewing the proposed content, run `cyber memory recover --review <fingerprint> [--global]`. Recovery requires enabled/writable settings, retained scope ownership and unchanged evidence. It checks an existing database read-only without following a database symlink; a pending admission, corrupt/unknown schema, unreadable database or `CYBER_DB=:memory:` refuses before file effects. A database predating memory admissions is supported. File-only recovery never clears a database admission; unresolved durable writes still require database/file reconciliation controls.

New model-tool and HTTP memory mutations durably bind the prepared journal before installing files. The binding records its exact mutation receipt and complete intent digest (before/after note/index hashes and catalog), then rotates the live owner nonce atomically. Persisted binding evidence contains the superseded nonce and the new nonce hash, so it cannot authorize completion after owner loss. Recovery inspection exposes `journal`, whose immutable `intent_fingerprint` stays stable across partial installation, separately from the fingerprint of current reviewed files. New completion requires the exact bound receipt; historical acknowledged writes remain replayable. This correlation does not clear pending admission or grant recovery authority; database/file reconciliation remains required.

The shared runtime now provides `review_memory_recovery` and `recover_memory_write` for pinned admissions. The database review binds the request, result and aggregate sequence in one snapshot; reconciliation compares that review and current storage before effects and durably grants a fresh recovery nonce. Disposal retains unknown fencing and requires another review. A matching acknowledged journal finishes archival with its original receipt and no duplicate notification. These are host-controlled interfaces: user-facing legacy database reconciliation is still pending, and hosts must retain the configured scope claim, lifecycle lease and fresh mutation settings. Legacy unbound admissions remain refused rather than inferring original intent or ownership.

Authenticated `GET /api/v1/memory/recovery/{project|global}` returns the Location envelope with null for absent evidence or `{storage, admission}` for a matching pinned journal. Inspect proposed content, transaction identity, immutable intent digest and completion state before confirming. `POST` to the same route accepts only `{storage_fingerprint, admission_fingerprint}` from that review. Recovery repeats writable/enabled settings and retains runtime shutdown and scope ownership through acknowledgement and archival. Missing, malformed, stale, foreign or unbound evidence refuses; manual inspection remains available when generation is disabled. The SDK exposes `client.memory.recovery(scope)` and `client.memory.recover(scope, fingerprints, options)`. With a retained Idempotency-Key, completed retries return the durable original receipt even after response-cache loss. Unkeyed retries retain stale-review refusal. Local-only/legacy HTTP recovery, TUI editing and Windows writes remain open.

Retain a unique key before confirming recovery and pass it as the SDK `idempotencyKey` option. `GET /api/v1/memory/recovery/requests?scope=project|global&key=<retained-key>` (SDK `client.memory.recoveryRequest({scope, key})`) returns null for an absent identity or its `mutation_id`, pinned journal and nullable `completed` change. Null completion means unresolved evidence, not permission to restart an attempt. Lookup checks the original canonical Location/project. Request-key and digest correlation is projected atomically with durable recovery ownership; different body/endpoint/Location reuse refuses after cache loss. A later explicit review resolving that same mutation makes its receipt available to earlier retained attempt keys. Already acknowledged journal archival can link a fresh request without another completion or notification.

For a retained save/delete key, use authenticated `GET /api/v1/memory/requests?scope=project|global&key=<retained-key>` (SDK `client.memory.requestStatus({scope, key})`) at the original Location. It reads the durable admission independently of the response cache and enabled/writable memory settings. Evidence includes the note name, deletion flag, original HTTP `request_fingerprint`, nullable pinned journal and nullable completed change. Compare the fingerprint against the exact retained request before reconciling client state. Null evidence means no matching admission was found; null completion means unresolved admission. Neither authorizes replay. Foreign Location/scope refuses, inconsistent ledger evidence refuses, and lookup creates no memory files or notifications. The TUI uses the same fingerprint and key identity checks before reconciling retained records.

For paired database/file reconciliation, start or use the registered TCP server and run `cyber memory recovery --server [--global]`. JSON output preserves the Location envelope and paired `data.storage` / `data.admission` reviews; text output prints the same review and proposed content. After inspecting both fingerprints, confirm with `cyber memory recover --server --review <storage-fingerprint> --admission-review <database-fingerprint> --key <retained-key> [--global]`. The CLI sends one authenticated request with that exact key and repeats no request automatically. After an uncertain response, inspect `cyber memory recovery-request --server --key <retained-key> [--global]`; unresolved evidence requires a fresh review. Completed identical retries return the original durable receipt. These commands require existing registration/credentials and never start a service, create a database or run a model locally; a socket-only server currently requires a TCP listener for this client. Default local recovery continues to refuse unresolved database admissions.

`/memory on`, `/memory readonly` and `/memory off` in the TUI turn automatic memory on, make it read-only (the index still loads; the tool offers only `list` and `read`), or turn it off (no index, no tool). The choice is saved as `memory.enabled`/`memory.generate` in your global `cyber.jsonc` (or `cyber.json` when only that exists) through authenticated `PUT /api/v1/memory/settings` with `{"state":"on"|"off"|"readonly"}`; only the `memory` member is rewritten, and comments and other settings are kept. A global document that is not a JSON object is left untouched and the request fails with 409. Running Sessions pick the change up at their next Safe Boundary. A project layer, `--config` override or `CYBER_DISABLE_MEMORY` still wins: the response (and the TUI toast) reports the effective state with `overridden: true`. `GET /api/v1/memory/settings` returns the effective state for the Location.

Open `/memory` in the TUI for project notes, or `/memory global` for global notes. Use Up/Down to select metadata, Enter to read the note, G to switch scope and R to refresh. D requests deletion of an opened note; Y confirms and N/Esc cancels. V inspects pinned recovery even when ordinary reads are fenced. Review the proposed content, completion state and both fingerprints; C opens confirmation and Y submits it once. K inspects the latest retained request in the current Session/Location/scope without repeating effects. U opens all retained requests; Up/Down selects a record, Enter/K looks up its outcome in its original context, and U returns. F opens explicit forgetting confirmation; Y removes only that record, N/Esc cancels. Forgetting does not resolve unknown server effects and retains draft text. The panel displays the key; copy it for CLI/SDK receipt lookup after exiting or changing Session/Location. Failed requests clear actionable reviews, and late responses cannot replace a newer scope/Location. PgUp/PgDn scroll content. E edits an opened note or resumes the retained draft. Type and paste multiline Markdown; Ctrl-S reviews the save and Y confirms. Ctrl-R obtains a fresh original/fingerprint while retaining draft text. A rejected save retains the draft and requires a fresh review before another confirmation. Esc first pauses editing and retains the draft; a second Esc closes the panel. Session/Location/scope changes retain the draft without granting foreign save authority. X/Y explicitly discards it. On Unix, drafts and confirmed save/delete/recovery intents persist privately under `<state>/memory-client/state.json`. The runner checkpoints edits and confirms durable intent before dispatch. Restart restores text and keys without review fingerprints or confirmations; E resumes in the original Session/Location/scope, then Ctrl-R obtains a fresh review. Stored save/delete/recovery keys support read-only lookup in their original context. Completed save/delete status must match the exact retained HTTP fingerprint, key, scope and journal/receipt before its record retires. A matching acknowledged save clears unchanged draft text; newer local edits stay and require fresh review. No retained request is sent automatically. Checkpoint errors withhold fresh memory effects and orderly exit while unsaved work remains; repair storage and retry, or explicitly export and discard the draft. Malformed or concurrently owned state requires a fresh TUI start after manual repair/ownership release. Acknowledged saves/deletes remove their pending intents, and acknowledged recovery keys remain available. Pending records remain bounded. Unknown or unresolved lookup preserves them; no automatic retry is sent. Multi-client retention and complete native acceptance remain open. Unsupported native privacy refuses memory effects without creating client state. Complete Windows/native acceptance remains open.

## Code intelligence status (partial P1)

`cyber lsp status` lists local server enablement and installation, then queries an existing registered server for this Location. It starts no service, model or language server and creates no database. `--format json` returns rows with `id`, nullable `enabled`, `installed`, `running` and `roots`. Each observed root carries `id`, `root` and `status: starting|connected|broken`.

A connected root makes `running` true. An authenticated empty root set makes it false. Without registration, `running` and `roots` are null (human output: unknown). Starting/broken entries without a connected root also keep running unknown; those protocol states do not prove process termination. Retained server IDs absent from current discovery remain visible with null enabled/installed metadata. An invalid, refused or unreachable registered service fails explicitly instead of reporting stopped servers. Requests use existing credentials, bypass ambient proxies, refuse redirects and enforce a five-second deadline and one-MiB response limit.

With `lsp` omitted, every built-in server whose executable is on PATH or in `<cache>/bin` is enabled for its extensions. Nothing is ever downloaded: `lsp.auto_install` is accepted and validated, but in P1 it installs nothing, and `cyber doctor` lists a missing server as `not installed` (with `; auto_install does not download servers` when the option is set). Install servers yourself, for example with `rustup component add rust-analyzer` or `go install golang.org/x/tools/gopls@latest`.

A failed or non-zero formatter run never fails the edit; it is logged as a `warn` line with component `formatter`, the formatter id, the edited path, `reason` and `exit_code`, never the formatter's own output.

`cyber fmt status` reports detected formatter definitions and project markers without executing them. Formatter execution after edits and LSP navigation/error feedback are implemented pieces of P1; complete platform and lifecycle acceptance remains open. See [feature limits](features.md) and [P1 evidence](implementation/p1-status.md).

## Browser verification

Browser verification is off unless `browser.enabled` is `true`, so projects that are not web apps never start a browser. The browser is an ordinary MCP server, `mcp.<browser.server>` (default `browser`). The trust, per-server digest approval and organization MCP policy that apply to any MCP server apply to it too.

```sh
cyber mcp browser install                   # pinned @playwright/mcp@0.0.41, user config
cyber mcp browser install --scope project   # refused while the checkout has unapproved config
cyber mcp browser health                    # start it once and check its tools
cyber mcp browser cleanup                   # remove profiles no running process holds
```

- **Install.** `install` writes the pinned definition (`npx -y @playwright/mcp@0.0.41 --headless --isolated`, `npx.cmd` on Windows; with `sandbox.apply_to` containing `mcp` the Windows sandbox refuses that batch file, so there the browser runs only unsandboxed) and `browser.enabled: true`. It prints the approval step a project install still needs: `cyber trust approve`, then `cyber mcp trust browser --digest …`. The commands are under `cyber mcp` because the command tree admits no new top-level command. `browser` is a security-sensitive key, so a project's `browser` settings stay inactive until the checkout is trusted.
- **Health.** `health` starts the configured server under the same trust, approval and sandbox rules as a Session's browser. It lists the server's tools, says whether the definition is the pinned one, and exits 1 when a required tool is missing. The required tools are `browser_navigate`, `browser_snapshot`, `browser_click`, `browser_type`, `browser_take_screenshot`, `browser_console_messages` and `browser_network_requests`.
- **Per-Session browsers.** The shared MCP pool leaves the browser server alone. Each Session that opens the Location starts its own process, and its tools reach only that Session, as `mcp__browser__<tool>`. The process runs with `TMPDIR` in `<data>/browser/profiles/<session_id>/`, which is emptied at start and locked while the browser runs. With `--isolated`, the profile itself lives in memory, never in the user's default profile or cookies. The first Turn waits for the browser to start. The durable connection record is named `<server>-<session ULID>`.
- **Permissions and network.** Calls go through the registered-tool path: schema, hooks, permissions (the action is the tool name, so `"mcp__browser__browser_click": "deny"` hides that tool), the output budget and plan-mode read-only filtering. `browser_navigate` accepts `http(s)` URLs only, and its host then passes the network policy: a sandbox-denied domain, or any host but this machine while the sandbox network is off, is refused, else the `network` permission is asked for that host. Pages the browser opens by itself, through links or redirects, are not checked. Only vetted tools are offered and callable: the required ones plus `browser_resize`, `browser_hover`, `browser_press_key`, `browser_select_option`, `browser_wait_for` and `browser_handle_dialog`; `browser_evaluate`, `browser_file_upload`, `browser_tabs`, `browser_navigate_back` and anything else the server publishes are not, because they would reach hosts or files the network check and `deny_read` keep out.
- **Evidence.** A call that captures something (a screenshot, the console, network requests or a DOM snapshot) stores it as `<data>/browser/artifacts/<session_id>/<sha256>.<ext>`, and records `session.browser.evidence.1`. The record carries the Session ID, the workspace content hash (every file Git would not ignore, `.git` excluded), URL, viewport, time, the actions since the last record, what was observed (`loaded <url>`, `console: N error message(s)`, `network: N failed request(s)`) and the checks not run on this page (`screenshot not captured`, `console errors not inspected`, `failed network requests not inspected`). Console, network and snapshot text is stored with credentials redacted. A screenshot comes back to the model as a note with the file path, which `read` opens.
- **Reports.** `GET /api/v1/sessions/{id}/browser/evidence` (`v1.session.browserEvidence`) lists the records with `stale: true` once the workspace hash differs from the one a record was taken under. Artifacts never leave the machine: deleting the Session removes them, retention removes them after `storage.retention.tool_output_days`, and nothing uploads them.
- **Dev servers.** `bash` (or `powershell`) with `background: true` and `ready_url`, an `http(s)` URL on localhost, starts the command as an owned Job. It returns once `ready_url` answers below HTTP 500 (redirects are not followed: the probe runs outside the sandbox). A call cancelled while it waits stops the Job. A sandboxed dev server with `ready_url` is refused before it starts when the sandbox isolates its loopback from the host, so the check could never connect: always on Windows (the AppContainer), and on Linux (its own network namespace) and macOS (listening is denied) unless `sandbox.network` is `on`. Set `sandbox.network` to `on` or run the Session with `--sandbox full-access` to verify a dev server the agent starts. If the process exits first, or `ready_timeout_seconds` (default 60, at most 600) passes and the Job is stopped, the call fails with the log tail. It then records `startup_failed` evidence that says the page was not tested.
- **Session end.** Archiving or deleting the Session stops its browser, removes its profile and stops its dev servers without a completion notice, so the Session does not start a Turn.

## Goals (P2 M2.3)

A Goal is a condition the Session keeps working toward. After every Turn that leaves nothing to do, an evaluator judges it: `not_met` sends its reason back as the next prompt, `met` completes the Goal (and starts the next queued one), and `impossible` stops.

```text
/goal all call sites compile against the v2 API and cargo test passes
/goal --check "cargo test --quiet" the auth tests pass     # met only when the check exits 0
/goal add the changelog mentions the v2 API                  # queued; runs after the current one
/goal                                                        # panel: current, queue, totals, last verdict
/goal pause | resume | skip | next | clear [gol_…|all] | edit [gol_…]
/goal use green auth module refactored                       # from goals.templates.green
/goal fork upgrade all dependencies with green tests         # a new background Session works on it
```

In the `/goal` panel, `e` edits, `p` pauses, `r` resumes and `c` clears the selected Goal, and Shift+↑/↓ moves a queued Goal. The footer shows `◎ goal <condition>`, and the sidebar has a Goal section with the status, turns against the budget, cost, last verdict and queued count.

- **One active Goal per Session, plus a queue** (`goals.max_queue`, default 20). `/goal <condition>` replaces the current Goal and starts a Turn toward it at once. User prompts during a Goal are handled as usual and the Goal stays active.
- **Evaluator.**
  - Model: `model_roles.evaluator`, else the small model, else the Session's model.
  - Tools: none.
  - Input: the condition, the latest answer, the Turn's tool results and earlier verdicts, within `goals.evaluator_max_input_tokens` (8000).
  - Output: one JSON verdict. A malformed reply is retried once, then counts as `not_met`.
  - Cost: recorded as hidden `goal_evaluator` usage, counted in the Session cost, and shown as evaluator totals in `/goal`.
- **Checks.** `--check` runs after a `met` verdict, sandboxed in the Location without a permission prompt like `!` (timeout `goals.check_timeout_seconds`, 600). A non-zero exit turns the verdict into `not_met`, and the last 50 lines go into the continuation.
- **Stops.**
  - **Budget:** each Goal has one, `max_turns` 50 by default, plus `budgets.goal` or per-Goal tokens, cost and wall time. It counts from activation and includes subagents; exceeding it gives `budget_exceeded`.
  - **`impossible`:** stops the queue unless `goals.on_impossible` is `continue`.
  - **Pause:** a failed Turn, an interrupt or a rejected permission pauses the Goal with the reason. `/goal resume` continues.
- **Background work.** While Jobs, background subagents or workflow runs the Session started are still running, evaluation waits for their handback.
- **Restarts.** Goals are `session.goal.*` events, so they survive a restart. An active Goal is evaluated once on its last Turn, and work continues only if it is `not_met`.
- **Notifications and hooks.** Met, impossible, paused and budget stops send a notification (TUI toast, desktop). `GoalEvaluated` hooks receive `{goal, verdict, reason}` and `GoalCompleted` hooks the Goal JSON on stdin; both are observational.
- **Unattended asks.** When a goal Turn asks for a permission and no client (a TUI, or any live event stream for the Session or its Location) has watched for `goals.unattended_ask_seconds` (300), the Goal pauses with `Waiting for approval` and a notification names the request. The request stays pending; answer it, then `/goal resume`. Under `cyber exec` asks are declined at once instead.
- **Templates.** `goals.templates.<name> = {condition, check?, budget?}` in config; `/goal use <name> [text]` (or `POST .../goals` with `template` and `args`) replaces `$ARGUMENTS` in the condition, or appends the text on a new line when there is no placeholder.

  ```jsonc
  {"goals": {"templates": {"green": {"condition": "$ARGUMENTS and all tests pass", "check": "just test"}}}}
  ```

- **Fork.** `/goal fork <condition>` starts a root Session with the Session's agent, model and Mode and the condition as its active Goal. With `goals.fork_isolation: "worktree"` (the default) in a git checkout it runs in its own `goal-<ulid>` worktree on branch `cyber/goal-<ulid>`; `cyber sessions list` in the source checkout lists it (as it does worktree teammates). Otherwise it shares the Location.
- **Subagents and workflows.** The `agent` tool and workflow `agent()` accept `goal: {condition, check?, budget?}`. The child works until its Goal is terminal and the result carries `goal_status`. A workflow agent's goal budget is capped by what the run has left.

  ```js
  const r = await agent('Migrate file X', { goal: { condition: 'file X compiles', check: 'cargo check' } });
  if (r.goal_status !== 'met') log('not migrated', r.goal_status);
  ```

- **The model.** The model can set or queue a Goal with the `goal` tool. It checks `goal.set`, which asks by default; allow it with `{"permissions": {"goal.set": "allow"}}`.
- **CI.**
  - `cyber exec --goal "<condition>" [--goal-check "<cmd>"] [--max-turns N]` runs until every Goal is terminal. `--goal` repeats to form a queue, and each `--goal-check` binds to the `--goal` before it.
  - Exit codes, for the first non-met Goal: `0` all met, `3` impossible, `4` budget exceeded, `1` otherwise, `130` interrupted.
  - The JSON result lists `goals[]` with status, turns, cost and last reason.
- **API:**
  - `GET|POST|DELETE /api/v1/sessions/{id}/goals`
  - `GET|PATCH|DELETE .../goals/{goal_id}`
  - `POST .../goals/pause|resume|skip|fork`
  - `PATCH .../goals/order`

## Loops (P2 M2.4)

`/loop` re-runs a prompt in the current Session: on an interval, or at a pace the model chooses.

```text
/loop 5m check the deploy status          # now, then every 5 minutes (minimum 60s)
/loop 30m /review                         # a command or skill works too
/loop watch CI until it is green          # self-paced: the model calls schedule_wakeup
/loop 2m --max 20 --for 1h --until "curl -sf https://staging/health" poll staging
/loop 10m --overlap coalesce --missed run-once --max-cost 2 summarize new issues
/loop list                                # Enter on a row stops that loop
/loop stop [lop_… | all]
```

- **Iterations are queued input** (`source: "loop"`), so they never interrupt a running Turn; they run when the Session would otherwise go idle. If an iteration is still running when the next tick comes, the tick is skipped (`--overlap skip`, the default) or one tick waits behind it (`coalesce`). A slow iteration never makes ticks drift: the next one is the next multiple of the interval since the loop started.
- **Self-paced loops** offer the model `schedule_wakeup {delay_seconds, reason, noop}` (delay clamped to 60–3600 s) only while an iteration runs, with a short guidance note. An iteration that ends without calling it completes the loop. In interval loops the tool records only `noop` and the reason.
- **Quiet checks.** An iteration that reports `noop: true` found nothing new. The TUI collapses consecutive quiet iterations into one `↻ N quiet checks · loop 7xk2` line and shows `↻ loop · next in 4m` in the footer.
- **Stops.**
  - `--max N` iterations, or `loops.max_iterations` (default 500) when none is given.
  - `--for <duration>`.
  - `--until "<command>"`, run before each iteration, sandboxed in the Location without a permission prompt, like `!` (timeout `loops.until_timeout_seconds`, default 60). The loop stops the first time it exits 0.
  - A budget: `--max-cost`, `--max-tokens`, `--max-turns` or `--timeout`, else `budgets.loop`. It is checked between iterations.
  - 3 failed iterations in a row.
  - `/loop stop`.
- **Limits.** A Session holds at most `loops.max_per_session` (default 50) active loops.
- **Restarts.** Loops are `loop.*` events on the Session's stream (`session_loop` indexes their due times), so they survive a server restart:
  - an iteration cut off by the restart ends `interrupted`;
  - missed ticks are skipped (`--missed skip`) or one catch-up runs (`run-once`);
  - a self-paced loop whose wakeup passed runs once.
- **API:** `GET|POST /api/v1/sessions/{id}/loops` (`POST` takes `{"command": "5m check"}` or the parsed fields), `DELETE /api/v1/sessions/{id}/loops[/{loop_id}]`.
- **Not yet:** in-session cron tools, persistent `cyber loops add` schedules run by the service, notifications, the `LoopIteration` hook, `--until-goal` (not yet connected to Session goals) and `/loop` in `cyber exec`.

## Workflows (P2 M2.1)

A Workflow is a JavaScript ES module. It declares a pure-literal `meta` and either a default async function or top-level code, which orchestrates agents through the host API:

```js
export const meta = { name: "audit", description: "Audit auth", phases: [{ title: "Scan" }] };
export default async function () {
  phase("Scan");
  const found = await agent(`List risky endpoints under ${args.dir}`);
  return found.text;
}
```

- **Saved Workflows.** Saved Workflows live in `~/.config/cyber/workflows/` (global) and in `.cyber/workflows/` (project, from the repository root down to the Location). A project Workflow overrides a global Workflow with the same `meta.name`.
- **Listing.** `cyber workflows list [--format json]` shows each Workflow's name, scope and description. It also lists modules that fail to load, with their diagnostic. Examples: `ParseError: meta must be a pure literal (2:29)`, or `UnsupportedSyntax` for `require`/`import` and for `.ts` files (TypeScript is not supported yet). Listing never runs a script.
- **Running from the CLI.** `cyber workflows run <name|file.js> [--args JSON] [--max-cost USD] [--max-turns N] [--max-tokens N] [--timeout 30m] [--max-agents N] [--max-concurrent N] [--wait]` starts a run on the shared server (started if needed) with a new owner Session in the current Location (`--mode` sets its Mode; default `default`). Without `--wait` it prints the `run_` ID. With `--wait` it prints phase and agent progress on stderr and the return value on stdout, and exits `0` when the run completed, `1` when it failed (or was stopped or paused elsewhere), `4` when it exceeded its budget and `130` on Ctrl-C, which stops the run. `--max-concurrent N` sets the run's share of the agent slots; a resume without one keeps it. `cyber workflows runs [--all] [--status S[,S…]] [--watch]` lists this Location's runs (every Location with `--all`), active ones first, as a table of ID, name, status, phase, agents `done/running/queued/failed` (`#N in queue` for a queued run), tokens, cost and active elapsed time; `--format json` prints the summaries. `--watch` keeps the list current from the instance stream (`workflow.run.progress` patches a row, other `workflow.*` events re-read the list; without a stream it reads the list every second): on a terminal it redraws the table in place, otherwise it prints one line, or with `--format json` one JSON object, per run whose values changed. Ctrl-C exits 0 and changes no run; `show <run_id>`, `pause`, `resume [--revision]`, `stop` and `logs` act on one run (`--format json` everywhere). `--runner` is rejected: remote runners arrive in P3.
- **Monitoring in the TUI.** `/workflows` (or the palette's `/workflows`) lists the Location's runs full screen, active ones first: name, status (`paused (6 settling)` while a paused run's agents finish), phase, a known-progress nyan bar of completed over total agents (`#N in queue` for a queued run), agents `done/running/queued/failed`, tokens (from 120 columns), cost against the budget (`$1.80 / $5.00`, in the warning color past 80%) and active elapsed time. Rows follow `workflow.run.progress` and the other `workflow.*` events and are read again when the event stream reconnects. Only the selected running run's bar animates, within the indicator's 12 frames a second, and the `NO_COLOR`, 16-color and reduced-motion fallbacks apply. At 80 columns phase and elapsed time are dropped, below 64 each run takes two rows, and screen readers hear each run as one sentence. Keys (the `workflows` context of `tui.keybinds`): Enter opens the run's agents grouped by phase, with each phase's `done/total`, running and queued counts; Enter or `t` on an agent opens its child transcript live, and Esc returns to the run with the agent still selected, then to the list. `p` pauses a running or queued run or resumes a paused one, `r` resumes a paused, failed, stopped or budget-exceeded run (a changed script asks `Script changed — start revision N?`), `x` stops after `y`, `R` resumes every run a restart interrupted, `s` opens the script in `$VISUAL`/`$EDITOR`, `l` shows the log and `a` switches between this Location and all Locations. Every action uses the public `workflow` routes, and a refusal is shown under the run's row.
- **Indicators and notices in the TUI.** Whatever the screen shows, the TUI reads the Location's unsettled runs (`GET /workflows/runs?status=queued,running,waiting_input,paused`), patches them from `workflow.run.progress`, and reads them again on each run's lifecycle events and when the event stream reconnects. The docked sidebar gets a Workflows section after Context: the current Session's runs first, then the other active runs, at most 5 rows (`audit ▀▀···· 31/100`, a 6-cell bar; `deploy #2 in queue` while queued) and `+N more`; Enter on a row opens that run in `/workflows`. The status footer shows `⚙ 2 workflows · 1 queued`. The progress line shows the Session's running run (`audit 31/100 agents · 31%`), or several together (`2 workflows 42/130 agents · 32%`). All three go when the last run ends. When a run ends or a restart pauses it, a one-line notice is added to its owner Session's transcript when that Session is open, else shown as a toast: `✓ audit completed · 100 agents · $4.10 · 21m`, `✗ review failed: AgentFailed: …`, `$ scan budget exceeded: …`, `■ migrate stopped` or `⏸ docs paused by restart`. `tui.workflow_notifications` picks the runs: `own` (default; runs of the current Session and runs this TUI paused, resumed or stopped), `location` (every run whose directory is this Location, including one that ended before the TUI first read it) or `off`. While the terminal is unfocused the same text goes through `tui.notifications` (`auto`: bell and OSC 9, `bell`, `osc9`, `osc777` or `off`; the Session-idle notification uses it too). Each attempt's end is noticed once per TUI: a resync after a reconnect notices only ends it missed, such as a run that ended or a restart that paused it while the stream was down (the run summary's `interrupted: true` marks a restart's pause).
- **Starting from a Session.** The model starts Workflows with the `workflow` tool: `{ "name": "<saved>" }` or `{ "script": "<inline module>" }`, plus `args` and `budget`. It needs the `workflow.run` permission. Its resource names the script's origin, because a script declares its own name: `<name>` for a global saved Workflow, `project:<name>` for a project one and `inline:<name>@<digest>` for an inline script. So `{"workflow.run": {"audit": "allow"}}` allows only the global `audit`; a project `audit` or an inline script named `audit` still asks, and an "always" approval of an inline script covers only that exact script (`"inline:*": "allow"` allows every inline script). The default rule (`workflow.run` `*` `ask`, source `default`) asks in `default` Mode and allows in `auto` and `bypass`; the approval shows the name, phases and budget. A deny rule for the bare name, such as `"permissions": {"workflow.run": {"migrate": "deny"}}`, refuses every origin in every Mode, and also refuses `cyber workflows run migrate` and `POST /api/v1/workflows/runs` (`403 PermissionDenied`, on start and on resume). API and CLI starts need no approval: asking for the run approves it. The tool returns the run handle at once; inline scripts are kept at `<state>/workflows/<run_id>.js` (`script_file`). Workflow agents cannot start runs: the server refuses a run owned by a workflow agent's child Session or any Session below it (`409 Workflow agents cannot start Workflow Runs`), over the tool, the API and the CLI alike.
- **Results.** A return value up to 1 MiB of JSON is kept as `result`; a larger one is written to `<state>/workflows/<run_id>.result.json` (`result_file`, with `result_preview`). When a run completes, fails, exceeds its budget or is stopped, one summary (`Workflow run handback: {…}` with the result preview, agent counts, cost and duration) is queued on the owner Session as message `msg_workflow_<run_id>` with source `workflow`. A run started by the `workflow` tool wakes its Session; an API or CLI run leaves the summary queued. Up to 8 summaries that are next in the Session's queue, with no other queued message between them, are taken together into one Turn in the order they were queued, so runs that end while the Session is busy cost it one Turn. A restart never queues a second summary. A resumed run that ends again queues a new summary, `msg_workflow_<run_id>_<n>` after the `n`th resume; the resume clears the previous attempt's `result`, `result_file` and `result_preview`.
- **Approvals inside runs.** Runs started over the API or the CLI have no approver: any `ask` from their agents (and the agents' subagents) is rejected at once with `No interactive approver for workflow run`, so they only use what rules pre-approve. Runs started by the `workflow` tool in an attended Session route asks to its clients.
- **Write isolation.** In a git checkout, agents whose profile can edit files (`write`, `edit`, `apply_patch` or `notebook_edit`; not in `plan` Mode) get their own worktree when another such agent of the run is in flight, including every agent of one `parallel()` fan-out. The worktree is `run-<ulid>-<index>` on branch `cyber/run-<ulid>/<index>` (`<index>` is the agent's start ordinal), the result carries `branch`, and the worktree is kept after the run (`cyber worktree prune` removes it). A single writer works in the owner's checkout; `isolation: "none"` opts out and `isolation: "worktree"` always isolates. Creating a run's worktree obeys `worktree` deny rules but does not ask.
- **Hooks and events.** `WorkflowRunStart` and `WorkflowRunEnd` hooks fire on the owner Session with `workflow: { name, status, revision, phases, budget, reason, error }`, and their envelope carries `workflow_run_id`, as do hook events of workflow agents' Sessions. Run events (`workflow.run.*`, `workflow.agent.*`, `workflow.phase.1`, `workflow.log.1`) stream live on `GET /api/v1/event` for the run's Location. Beside them, the non-durable `workflow.run.progress` event (`{ run_id, status, phase, agents, tokens, cost_usd }`) streams on the run's Location whenever one of those values changes, at most every 250 ms per run; it shows agents moving between `queued` and `running`, which leaves no durable record. A client that reconnects resyncs with `GET /api/v1/workflows/runs`, which returns the same values.
- **Runs over HTTP.** `POST /api/v1/workflows/runs` starts a run from `{"session_id", "script" | "name", "args", "budget"?, "max_concurrent"?}`: an inline module, or a saved Workflow of the Session's Location. The run executes in the server, independent of the client, and answers `201` with the run (`run_` ID, `status: "running"`, or `"queued"` with a `queue_position` when every run slot is taken). `GET /api/v1/workflows/runs` lists run summaries: each run with `agents: { total, completed, running, queued, failed, cancelled, interrupted }` (`queued` counts `agent()` calls waiting for a slot of the concurrency limit), `tokens` and `cost_usd` of its agents, `elapsed_ms` of active time (time `queued` or `paused` excluded), `share` and, while queued, `queue_position`. It takes `status` (comma-separated), `session_id`, `directory` (default the request Location, `all` for every Location), `limit` (default 50, at most 200) and `cursor` (the previous page's `cursor.next`), and returns active runs first, then the newest. `GET /api/v1/workflows/runs/{id}` shows the same summary of one run, with its `status`, `revision`, `budget`, `phases`, `result` or typed `error`, plus `agents_detail`, its agents (`running`, `completed`, `failed`, `cancelled` or `interrupted`, with `started_ms` and, once settled, `ended_ms`). `GET /api/v1/workflows/runs/{id}/logs` returns the `log()` entries. An invalid script answers `400` with its `diagnostic`. The routes are the `v1.workflow.*` operations in OpenAPI; the TypeScript SDK adds `client.workflows.run({ name | script, args, budget, max_concurrent })`, whose handle has `events()` (including `workflow.run.progress`), `status()`, `pause()`, `resume()`, `cancel()` and `result()`, plus `client.workflows.list(filter)` (an async iterator of handles that reads the next page only when reached) and `client.workflows.get(id)` to attach to runs another client started.
- **Lifecycle.** A run is `queued`, `running`, `paused`, `completed`, `failed`, `budget_exceeded` or `stopped` (`waiting_input` while `ask()` waits for the owner's answer). At most `workflows.max_running_runs` runs execute at once; a run started or resumed while every run slot is taken is recorded `queued` with a `queue_position` (1 is next), starts no agent and accrues no wall time. Queued runs are dispatched oldest first with `workflow.run.dispatched.1`, which moves them to `running`; a run keeps its slot until its engine and every agent task have returned. A queued run can be paused (it leaves the queue; a resume queues it again at the end) or stopped (it ends without an agent and hands its summary back). `cyber workflows runs` and `show` print `#N in queue`, and `run --wait` and the SDK's `result()` keep waiting while queued. `POST …/runs/{id}/pause` records `paused` at once: agents in flight finish and are stored, and no new agent starts, including calls still waiting for a slot of the concurrency limit. A script that then ends on its own still records `completed` (or `failed`) with its result; a stop and the script's own end never both record an end. `POST …/runs/{id}/stop` cancels agents in flight and stops their child subtrees. `POST …/runs/{id}/resume` (`{"revision"?, "budget"?}`) relaunches a paused, failed, stopped or budget-exceeded run. It answers `409 Run has agents still exiting; retry shortly` while the previous attempt still holds an agent. Stopping the server pauses running runs before tools shut down, and a server that starts after a crash pauses runs left `running` (in start order); both pauses carry `interrupted: true`. A run left `queued` keeps its place and is dispatched by the new server; its script is read again as a resume would, and one whose script changed is paused with `WorkflowReplayMismatch` instead. `POST /api/v1/workflows/runs/resume_interrupted` (`?directory=<path>` or `directory=all`; default the request Location) and `cyber workflows resume --interrupted [--all]` resume every run whose latest lifecycle event is such a pause, answering `{ resumed: [run_id], refused: [{ run_id, reason }] }`; a run the user paused stays paused. With `workflows.resume_after_restart: true` the server does this itself once recovery ends, and a refused run stays paused with the refusal as its reason. Child execution ownership claimed by a server records that server's registration ID; the next server to take `server.lock` releases claims an earlier server left held (the lock proves that process exited), so the interrupted agents of a run resume their child Sessions after a crash. Claims of runtimes that never held the lock stay held. A run that ends (completed or failed) with agents still in flight cancels them, and they are listed as `cancelled`. Each transition is a durable `workflow.run.*.1` event.
- **Replay.** A relaunch re-executes the script. Each `agent()` call is keyed by `sha256(prompt + canonical options)` plus an ordinal, and each `checkpoint(name, value)` by its name. Settled results and checkpoint values are delivered from the store in their original order, with no model call. An agent that was in flight resumes its existing child Session (a stopped child's admission is reopened first, and the child's interrupted tool calls are reconciled before its next Turn). A failed agent is retried by a new agent, which counts against `max_agents`; a child whose resumed attempt failed is never resumed again. A resume reads the saved file, or an inline run's `script_file`, again (an inline run whose file is gone runs its stored source). If that script or the Workflow configuration (limits and tiers) changed, resume fails with `409 WorkflowReplayMismatch` unless `revision: true` starts a new revision. A new revision reuses completed results and checkpoints by key. A run whose script issues different calls in the same revision fails with `WorkflowReplayMismatch`. Calls are numbered as the script issues them, so results of one `Promise.all` or `parallel()` batch replay in the order they first arrived. A call rejected before it starts (bad options, an unknown tier, `ask()`) is recorded like the others. A resume runs agents under the stricter of the run's starting Mode and the owner Session's current Mode.
- **Budgets.** A run budget layers `budgets.run`, `meta.budget` and the request's `budget`: `max_cost_usd`, `max_tokens`, `max_turns`, `max_wall_seconds` (default 14400, paused time excluded) and `max_agents` (default 200), with soft enforcement (`reserved` is refused). `budgets.run` and the defaults are a ceiling: `meta.budget` and the `workflow` tool's `budget` can only lower a limit (a larger value keeps the current one; `null` for a limit already set is refused). Only the authenticated client raises a limit, explicitly, with the `budget` of `POST /api/v1/workflows/runs` (the CLI's `--max-*` flags) or of a resume. Past `max_wall_seconds`, the first host call other than `agent()` (a checkpoint, say) rejects with `BudgetExceeded: max_wall_seconds` and the next one ends the run, so a script without agents also stops. An `agent()` call after a limit is reached rejects with `BudgetExceeded: <limit>`, `budget.exceeded.1` is recorded on the run, and the run ends `budget_exceeded`, keeping a partial result the script returns. The check before every provider Turn of an agent's child (and its descendants) also counts the run budget. `ctx.budget` reads `{ spent_tokens, spent_cost_usd, agents_started, remaining }` as of the latest result the script received (or the start of the revision). The snapshot is recorded with each result, and a resume replays it, so a script that branches on its budget repeats its earlier choices. `GET /api/v1/usage?scope=run&id=<run_>` sums the run's agents and groups cost by agent, model and tier.
- **Agents.** Each `agent(prompt, opts)` call becomes a child Session of the owner Session (`session_id`), with `workflow_run_id` set and an `agt_` ID as its subagent name. It runs under the owner's Mode as a ceiling, and deny rules for `agent` apply. Without `schema` it resolves to `{ text, session_id, cost, tokens }`; with `schema` it resolves to the validated object, after at most one result retry (`SchemaMismatch` otherwise). Failures reject with `AgentFailed`, `SchemaMismatch`, `PermissionDenied` or `ModelUnavailable`, which scripts can catch through `e.kind`. `model` takes a `provider/model` reference or a tier: `fast` uses `model_roles.small`, `smart` the default model and `verify` `model_roles.advisor`, falling back to the default model. An unknown tier is `ModelUnavailable`. The options `goal`, `tools` and `budget` are rejected as not available yet, as is `ask()`.
- **`ask()`, direct tools and exec.** `ask(question)` takes a string or `{question, options?, multiple?}`. An answer the invocation declares in `args.answers` (keyed by the question text) is used at once; a run without a user (started over the API, the CLI or `exec`) otherwise rejects with `NoInteractiveUser`. An interactive run (started by the `workflow` tool of an attended Session) moves to `waiting_input`, asks the owner's clients through the question flow and returns to `running` with the answer; waiting is not wall-budget time, and a pause or stop withdraws the question so a resume asks again. `tools.read({path, offset?, limit?})`, `tools.glob({pattern, path?, max?})` and `tools.grep({pattern, path?, include?, max?})` are callable when `meta.tools` or the invocation's `tools` (`POST /workflows/runs`, the `workflow` tool) names them; otherwise they reject with `PermissionDenied: tool not allowed by meta.tools`. They run for the owner under the run Mode with the built-in permission checks and return `{content, truncated, next_offset}`, `{paths, truncated}` (path order, default 1,000, at most 10,000) and `{matches: [{path, line, text}], truncated}` (default 100, at most 1,000); their results are recorded, so a resumed run sees the files as they were. `cyber exec --workflow <name|path> [--args JSON]` runs a Workflow non-interactively, streams `workflow_progress` events in `stream-json`, prints the return value as the result and exits 0, 1, 4 (budget) or 130. Agents that share a prompt prefix (agent profile, model and schema) start one at a time until the first one's response begins, so the others read its cached prefix (`workflows.prompt_cache_warmup: false` turns this off). Tool processes of workflow agents get `CYBER_WORKFLOW_RUN_ID`. `GET /api/v1/workflows` lists the Location's Workflow library with origin, digest and shadowed definitions; `POST /api/v1/workflows/runs/{id}/save` (`w` in `/workflows`) saves an inline run's script as `.cyber/workflows/<meta.name>.js`, refusing to replace a file without `force`. `/tasks` lists the Session's runs, and `/stop` stops them with its Jobs.
- **Concurrency and several runs.** Workflow agents of every run share one limit of `min(workflows.max_concurrent, CPU cores)` agent slots, read from the global configuration at server start; `agents.max_concurrent` does not apply to them. The slots are shared fairly, not first come first served: a free slot goes to the run whose last grant is oldest (a run never granted goes first), and within a run calls start in the order the script issued them. A run that waits never sees more than one grant to each other waiting run before its own, so a small run started next to a 30-agent fan-out starts its agents on the next free slots instead of after the whole fan-out. No slot is left idle while any run has a call waiting. A run's share caps how many slots it may hold at once: `max_concurrent` on `cyber workflows run --max-concurrent N`, on `POST /api/v1/workflows/runs` and `POST …/resume`, in the `workflow` tool's input, or as `meta.max_concurrent` (a positive integer), else `workflows.max_concurrent_per_run`, else the whole limit; it is capped at the limit, recorded on the run as `share`, does not count as a configuration change on resume, and a plain resume keeps it. `parallel(thunks, { concurrency })` narrows a group further. Separately, at most `workflows.max_running_runs` runs execute at once and the rest wait in the run queue (see Lifecycle). Each run's pause, stop and resume takes only that run's lock, so a stop that waits for one run's agents to exit never blocks another run's pause, resume or end.
- **Durable records.** Every agent is stored as `workflow.agent.started.1` before it starts and `workflow.agent.settled.1` before the script sees its result. Checkpoints, reused results and budget rejections are stored as `workflow.call.recorded.1`, phases and logs as `workflow.phase.1` and `workflow.log.1`. If a record cannot be written, the run fails. A run writes at most 100,000 records and 256 MiB of record data, a checkpoint value is at most 1 MiB of JSON, `phase()` titles are at most 200 characters and a run has at most 256 distinct titles; past any of these the run fails with `ExecutionFailure`. Calling `phase()` with the current title records nothing, and a `workflow.phase.1` event carries the phase list only when it adds a title.
- **Worked example: fan out, then rank.** Save this as `.cyber/workflows/fanout.js` in a git checkout. Three agents analyze one item each and must answer in the `schema`; `pipeline` then maps the validated results in the script, with no model call:

  ```js
  export const meta = { name: "fanout", description: "Fan out three analysts, then rank",
    phases: [{ title: "Analyze" }, { title: "Rank" }] };
  const schema = { type: "object", required: ["item", "score"], additionalProperties: false,
    properties: { item: { type: "string" }, score: { type: "number" } } };
  export default async () => {
    phase("Analyze");
    const items = ["alpha", "beta", "gamma"];
    const results = await parallel(items.map((item) => () =>
      agent(`Score ${item} from 1 to 10`, { label: item, schema })));
    log("analyzed", results.length);
    phase("Rank");
    const ranked = await pipeline(results, (r) => ({ item: r.item, score: r.score * 10 }));
    return { total: ranked.reduce((sum, r) => sum + r.score, 0), ranked };
  };
  ```

  ```console
  $ cyber workflows list
  fanout  [project]  Fan out three analysts, then rank
  $ cyber workflows run fanout --wait        # progress on stderr, result on stdout
  phase: Analyze
  agent beta completed
  agent alpha completed
  agent gamma completed
  phase: Rank
  run_… completed
  { "total": 140, "ranked": [ { "item": "alpha", "score": 50 }, … ] }
  $ cyber workflows logs run_…
  analyzed 3
  $ cyber api v1.session.children --param id=<owner ses_…>
  ```

  With `"workflows": { "max_concurrent": 2 }` in the global `cyber.jsonc`, two agents run at a time and the third starts when one ends. Each agent is a child Session of the run's owner Session with `workflow_run_id` set and its `agt_` ID as subagent name; because the three agents of the fan-out can edit files, each gets its own `cyber/run-<id>/<n>` worktree. Add `--max-tokens 50` to see a budget end (`budget_exceeded`, exit `4`; agents still in flight are recorded `cancelled`), or run `cyber workflows stop <run_id>` from another terminal (`stopped`, exit `1` for the waiting command). If the server dies mid-run, the run is `paused` when it comes back; `cyber workflows resume <run_id>` replays settled agents from the store without new model calls and continues the interrupted one in its existing child Session.
- **Not available yet.** Reserved budget enforcement (`"enforcement": "reserved"`) and remote runners (P3).
- **Configuration.** The `workflows` key accepts:

  | Key | Default | Range or values |
  |---|---|---|
  | `max_concurrent` | 16 | 1–256 |
  | `max_concurrent_per_run` | unset (the whole limit) | 1–256 |
  | `max_running_runs` | 8 | 1–64 |
  | `resume_after_restart` | `false` | `true`, `false` |
  | `prompt_cache_warmup` | `true` | `true`, `false` |
  | `bundled` | `true` | `true`, `false` (also `CYBER_DISABLE_BUNDLED_WORKFLOWS=1`) |
  | `max_memory_mb` | 256 | 16–4096 |
  | `script_cpu_seconds` | 60 | 1–3600 |
  | `auto` | `suggest` | `off`, `suggest`, `on` |
  | `auto_threshold_files` | 10 | 1–100,000 |
  | `size_guideline` | 10 | 1–1000 |
  | `tiers` | `fast` → `role:small`, `smart` → `role:default`, `verify` → `role:advisor` | `{ "<tier>": "provider/model" \| "role:<role>" }` |

  Unknown keys and invalid values fail configuration loading.
- **Sandbox.** Scripts run in QuickJS. They have no filesystem, network, imports, `eval` or `Function`, timers, `WebAssembly`, `WeakRef`, `FinalizationRegistry`, `SharedArrayBuffer` or `Atomics`. `Date.now()`, `Math.random()`, `performance.now()` and `new Date()` with no argument throw. Pass values through `args` instead. Describing an uncaught error counts against `script_cpu_seconds`. A script may have at most 10,000 host calls pending, and their payloads count against `max_memory_mb`; past either bound the call throws `ExecutionFailure` at once. `meta` may nest objects and arrays at most 64 levels deep.

### Bundled Workflows (P2 M2.2)

The library also includes [`solve`](verified-workflows.md): independent candidate worktrees, host-owned checks, a read-only judge and server-derived evidence. Its default soft budget is $5, five agents and 30 minutes; each solver has at most 20 steps. Checks use literal argv and need command permissions. The owner checkout stays unchanged.

For complementary changes, use the [integration queue](verified-integrations.md) and its owner CLI/API/dashboard controls. Its native checks are restricted to the embedded source digest; ejecting or overriding its script does not grant host verification authority.

Seven Workflows ship inside the `cyber` binary. They have the lowest precedence: a global (`~/.config/cyber/workflows`) or project (`.cyber/workflows`) module with the same name replaces one, and the listing names what it shadows (`bundled:audit`).

| Workflow | What it does | Args (defaults) | Default budget |
|---|---|---|---|
| `review` | Read-only reviewers per lens and diff group (≤ 48 KiB of patch each), then one `verify`-tier refuter per finding | `target` (uncommitted), `lenses` (`correctness`, `security`, `tests`, `maintainability`), `max_findings` (30, ≤ 60), `verify` (`true`) | 24 agents, $2, 30 min |
| `audit` | Finders per chunk of files (by directory), then `votes` refuters for critical/high findings (majority drops them) and one for the rest | `scope` (`**/*`), `exclude`, `focus` (`both`), `max_files` (400, ≤ 2000), `chunk_files` (12), `votes` (3) | 120 agents, $10, 2 h |
| `migrate` | One edit-capable agent per batch in its own worktree: it edits, runs the `check` commands and commits on its branch; a verifier reads the branch diff. Nothing is merged; the result suggests `git merge <branches>` | `instructions`, `files` or `pattern` (+ `contains`), `batch_size` (5, ≤ 25), `check`, `confirm_over` (20), `max_batches` (40), `yes` | 80 agents, $15, 4 h |
| `research` | Searchers per source and angle, readers extracting claims with locations, an independent cross-check per claim, then an answer citing verified claims only | `question`, `sources` (`repo`, `docs`; `web` when the web tools are allowed), `angles` (3, ≤ 6), `max_sources` (12) | 40 agents, $5, 1 h |
| `plan` | Read-only planners per angle, judges scoring every draft on correctness, completeness, risk, effort and testability, then a merge of the best draft with named grafts | `task`, `angles` (`mvp-first`, `risk-first`, `user-first`), `judges` (3), `context` | 14 agents, $3, 30 min |
| `solve` | Independent patches in managed worktrees, host-owned checks, a separate read-only judge and an evidence report; no automatic merge | `task`, `candidates` (2–4; defaults `fast`, `smart`, `verify`), `checks` (1–8 literal argv arrays), `judge_model` (`verify`) | 5 agents, $5 soft ceiling, 30 min; 20 steps per solver |
| `integrate` | Combine complementary local commits, run host checks on the combined source and request separate read-only review; publication requires an explicit owner action | `contributions` (1–20 pinned commits/dependencies), `checks` (1–8 literal argv arrays), `reviewer_model` (`verify`) | 1 reviewer, $5 soft ceiling, 30 min |

```bash
cyber workflows list                                   # NAME, ORIGIN (bundled/global/project), DESCRIPTION
cyber workflows show review [--source]                 # meta, args schema, default budget, estimate
cyber workflows run audit --args '{"scope":"src/auth/**","focus":"security"}' --wait
cyber exec --workflow plan --args '{"task":"add a cache"}' --format json
cyber workflows eject audit [--global] [--force]       # copy it to .cyber/workflows/audit.js to customize
```

- **Args are checked first.** Every surface (HTTP, CLI, the `workflow` tool, the TUI form) validates `args` against the module's `meta.args` JSON Schema before a run exists; a violation fails with `ExecutionFailure: args.<path> …`.
- **Host preparation.** `review` resolves its `target` with the `/review` resolver under the reviewer's own permissions and passes the diff as `args.diff`; an invalid target, an empty diff or a fully withheld one fails with `ReviewTargetError` (422) and no run. A non-interactive `migrate` (API, CLI, exec) fails up front with `403 PermissionDenied` naming each `edit` and `bash` rule it lacks, since no one could approve its agents. `research` drops `web` with a `limitations` entry when the reviewer may not use `websearch`/`webfetch` here.
- **Honest results.** Each returns `{summary, …, limitations, partial, stats}`. Every fan-out is capped and what it dropped is logged and listed in `limitations`; an agent failure is a `limitations` entry; when a budget stops new agents the run returns what it has with `partial: true` (`budget_exceeded`, exit 4). A `review` whose every reviewer failed fails instead of passing clean. File, diff and web text reaches agents only inside delimited data blocks.
- **`/review` and `cyber review`.** When the library has a `review` Workflow and no `review` skill or command overrides the bundled one, both run it: the TUI follows the run and shows its findings (partial ones when a budget stopped it), and `cyber review` exits 0, 4 (partial findings printed) or 1. With the bundled Workflows disabled, the bundled review skill runs as before.
- **The model and the TUI.** The `workflow.run` approval shows the effective budgets and the module's estimate; the model's context lists the library (bundled first, at most 20) and tells it to prefer an entry over an inline script. In `/workflows`, Tab opens the Library tab: Enter opens an args form prefilled from the defaults and starts the run for the Session, `o` shows the source.
- **Upgrades.** A bundled run records origin `{kind: "bundled", name}` and the source digest; a release that changes a bundled source is a new revision on resume (`--accept-revision`), as for an edited saved module. `tools.glob` takes `sizes: true` to return each file's size, which `audit` uses to skip files over 256 KiB.

Design and progress: [`implement-m2-1-workflow-runtime`](../openspec/changes/implement-m2-1-workflow-runtime/proposal.md), [`implement-m2-2-bundled-workflows`](../openspec/changes/implement-m2-2-bundled-workflows/proposal.md), [`add-parallel-workflow-runs`](../openspec/changes/add-parallel-workflow-runs/proposal.md) and [P2 evidence](implementation/p2-status.md).

## Migration detection

Run `cyber import --detect [--format json]` to inventory known Claude, Codex and OpenCode files for the current checkout and home. `--cwd` chooses the Location; `CODEX_HOME` adds an explicit Codex home. This command bypasses config/bootstrap/log creation, does not open a database and never executes source commands or plugins. Existing home and Location directories are required.

The report counts raw agent/command/skill files and inline JSON/JSONC/TOML definitions, MCP servers and hook handlers, and the sessions each tool recorded for this project: Claude transcripts under `~/.claude/projects/<encoded project path>/`, Codex rollouts whose `session_meta` working directory is inside the project, and OpenCode JSON sessions under `~/.local/share/opencode/storage/session/`. An OpenCode SQLite store is not inspected, so its count is `null` (`unknown` in text). Session files are counted, never shown. Counts include repeated definitions in separate layers, not an effective merged configuration. Parse/read failures are static issues without configuration values.

Each item also says whether Cyber Code already reads it without an import (`read_time` in JSON, `read-time: yes|no|<n> of <m>` in text), under the default compatibility settings: skills in the runtime's skill roots (`.agents/skills`, `.claude/skills`, `.opencode/skills`, `.codex/skills` and their home equivalents), Claude commands under `.claude/commands` and the servers of the project root's `.mcp.json` are covered; agents, other MCP servers, hooks and sessions of other tools are not. A text line reads `claude: 3 agents; read-time: no`. Sources referenced from inside configuration are not resolved yet, so `complete` and `counts_complete` stay false.

The first interactive TUI run in a project that holds another tool's project files (not only home files) shows a toast offering `cyber import auto --dry-run`, once per project; the offer is recorded in the TUI's `kv.json`.

## Importing configuration

`cyber import <claude|codex|opencode|auto> [--dry-run] [--yes] [--scope project|global] [--include sessions] [--format json]` converts supported configuration, permission, provider/model and static MCP mappings into `cyber.jsonc` of the Location (project scope) or of the global config directory (global scope). Existing native keys win; `auto` fills missing keys in OpenCode, Codex, Claude order. Project imports include source home defaults; global imports exclude project sources.

- **Diff.** The command prints a unified diff of the real bytes of `cyber.jsonc` (`--- /dev/null` when the file does not exist yet). Comments and every other byte of an existing document are kept. Known secret values, the document's own included, are redacted from the diff. The preview also names the report file a write creates under `.cyber/`.
- **Confirmation.** Without `--yes` it asks `Write these changes to <path>? [y/N]` on stderr and reads one line of stdin; anything but `y`/`yes`, end of input included, writes nothing and exits 1. `--dry-run` never writes.
- **Writes.** Right before writing, every source and native file under review is verified again, and a change since the review refuses the write. `cyber.jsonc` is replaced through a new sibling file, keeping its permissions; a linked target or `.cyber` directory is refused.
- **Report.** Every run ends with the mapping report: `imported`, `merged` (an existing key kept, with both values), `requires manual port` and `not imported` items, each with its source path. When files were written it is also saved as `.cyber/import-report-<UTC timestamp>.md` (in the global config directory for `--scope global`). Credential values never appear in it.
- **Annotations and re-import.** Each inserted entry carries a `// x-imported-from: <sources> sha256:<digest>` comment. Running the same import again without source changes writes nothing and prints `nothing to do`. When a source changed, an entry is replaced only while it keeps its annotation and still holds the imported value (its digest matches); an entry you edited is kept and reported as `merged`.
- **Secrets.** Literal source credentials (API keys, tokens, header values) are never copied. They become `{env:NAME}` references, and the report names each variable to set. Storing them in the OS keyring instead is not available yet.
- **No sources.** When nothing is found the command prints, for example, `no claude configuration found in /repo or ~/.claude` and exits 0.
- **Sessions.** `--include sessions` is accepted and reported as not imported; session import is a later capability.

Imported providers, MCP servers and hooks in a project document stay inactive until the project is trusted, like any project definition; each hook and project MCP server then needs its own digest approval. Explicit Codex approval/sandbox pairs propose native mode and sandbox policy; retired `untrusted` emits a deprecation note. On-failure/unknown approvals refuse the proposal. Profiles, substitutions, transient flags and trust filtering are not evaluated by the raw file-layer proposal. JSON output keeps `complete: false`, because rule files, Codex agent files and `requirements.toml` are reported, not converted.

### What each source becomes

Nothing a source declares is run during an import, and an item without a native equivalent is listed under `not imported` (or `requires manual port`) with the reason, never guessed.

| Source | Native result |
|---|---|
| Claude `permissions.allow`/`ask`/`deny`, `defaultMode` | `permissions.rules` in allow, ask, deny order; `mode` |
| Claude `model`, `fallbackModel` | `anthropic/<id>`; the aliases `opus`, `sonnet` and `haiku` resolve to `claude-opus-5-5`, `claude-sonnet-5-5` and `claude-haiku-4-5` (review them after an upgrade); `fallback_models` |
| Claude `env` | `sandbox.env.set`; a credential-named variable, or one from `settings.local.json` or (for a project import) `~/.claude`, is withheld and reported, so personal values never reach a shared document |
| Claude `hooks` (every settings file, in layer order), Codex `hooks.json` | `hooks`, events one to one; `Bash` → `bash`, `Edit\|Write` → `{edit,write}`, `mcp__x__.*` → `mcp__x__*`; `statusMessage` → `status_message`; `CLAUDE_PROJECT_DIR` → `CYBER_PROJECT_DIR`. Events without an equivalent, `agent` handlers, header-carrying handlers and regex matchers are reported |
| Claude `.claude/agents/*.md` | `agents.<name>`: the body is `system`, `mode: subagent`, `tools`/`disallowedTools` → `tools.allow`/`tools.deny`, `model` (aliases as above, `inherit` omitted), `color`, `permissionMode` → `permission_mode`, `skills`. A denied tool without an equivalent refuses the agent |
| Claude `apiKeyHelper` | reported: command credentials are not available yet; set `ANTHROPIC_API_KEY` |
| Claude `.claude/rules/*.md` | reported: rule files are not loaded yet |
| Skills (`.claude/skills`, `.agents/skills`, `.opencode/skills`, `~/.codex/skills`), `CLAUDE.md`, `AGENTS.md` | not copied: reported `imported` because the runtime reads them in place |
| Codex `[profiles.*]`, `~/.codex/<name>.config.toml`, `profile` | `profiles.<name>` with `model`, `mode` and `sandbox.policy`; `default_profile` |
| Codex `[permissions.*]`, `default_permissions` | `sandbox.profiles.<name>` (`extends`, `filesystem` paths → `writable_roots`/`readable_paths`/`deny_read`, `network.enabled` → `off`/`proxy`, `network.domains`); `sandbox.profile`. A profile that cannot be converted is reported, and selecting it selects `read-only` instead |
| Codex `[shell_environment_policy]` | `sandbox.env`: `inherit` `all` → `inherit`, `core`/`none` → `none`; `exclude` → `deny`; `set` → `set`; `experimental_use_profile` → `login_shell`. `include_only`/`filters` report the whole policy; `ignore_default_excludes` is reported, since credential masking stays on |
| Codex `approval_policy = { granular = … }` | `mode: default`; `sandbox_approval`/`request_permissions = false` → a `sandbox.escalate` deny rule, `skill_approval = false` → a `skill` deny rule, `rules = false` turns execpolicy `prompt` rules into `deny`; `mcp_elicitations = false` is reported |
| Codex `notify` | a `Notification` command hook running the argv (the event JSON arrives on stdin) |
| Codex `requirements.toml` | reported key by key with the org-policy setting that expresses it; never written |
| OpenCode `.opencode/agents/*.md`, `.opencode/modes/*.md` | `agents.<file name>` through the inline agent converter; agents default to `mode: all`, modes to `primary` |
| OpenCode `instructions` | `instructions`: URLs and absolute or `~/` paths always; a relative path only when its source file sits beside the target `cyber.jsonc` |
| OpenCode plugins and custom tools | `requires manual port`, with a link to [Porting OpenCode plugins and tools](#porting-opencode-plugins-and-tools) |

The ROADMAP M1.5 exit check imports the checked-in reference setup `crates/cyber-cli/tests/fixtures/claude-reference` (agents, a skill, a command, hooks, MCP servers, permissions and settings) with nothing left to port (`cli.rs::import_claude_reproduces_reference_setup_without_manual_edits`).

### Porting OpenCode plugins and tools

OpenCode plugins (`.opencode/plugins/*.ts`) and custom tools (`.opencode/tools/*.ts`) run inside OpenCode's JavaScript process, and Cyber Code has no plugin host yet, so an import never loads or converts them. Port each by what it does:

- A plugin that reacts to events (`tool.execute.before`/`after`, `session.idle`, notifications) becomes a hook: a `command` (or `http`) handler under the matching `hooks` event, which receives the event JSON on stdin and can return a decision.
- A custom tool becomes an MCP server: expose the function as an MCP tool (stdio or HTTP) and declare it under `mcp`.
- A plugin that only adds instructions becomes an `AGENTS.md` section or a skill.

### Read-time compatibility

Without any import, the runtime reads other tools' skills and the project's `.mcp.json` where they are:

- **Skills** are discovered under `.cyber/skills`, `.agents/skills`, `.claude/skills`, `.opencode/skills` and `.codex/skills` in every directory from the project root to the Location, and under `~/.config/cyber/skills`, `~/.agents/skills`, `~/.claude/skills` and `~/.codex/skills`. On a duplicate name, `.cyber` beats `.agents`, which beats `.claude`, then `.opencode`, then `.codex`; a nearer directory beats a farther one, and project folders beat user folders. The loser is named in a `duplicate skill: replaces skill <name> from <path>` diagnostic on the winner. `compat.agents`, `compat.claude`, `compat.opencode` or `compat.codex = false` turns off one folder family; `skills.compat = false` turns off all of them.
- **`.mcp.json`** at the project root loads as project MCP servers, below every `cyber.jsonc`: a server that any configuration document names keeps that definition whole. `${VAR}` and `${VAR:-default}` become `{env:…}` references; a server holding any other placeholder syntax, an unsupported `type`, or a reserved name (`sampling`, `tool_timeout`) is skipped with a warning, and unknown fields are ignored with a warning; a linked `.mcp.json` is not read. Like any project server it stays inactive until the checkout is trusted (`cyber trust approve`), and each server then needs a one-time approval of its definition digest, recorded per checkout in the trust store: `cyber mcp definitions` lists it and `cyber mcp trust <name> --digest <digest>` approves it. A changed definition needs a new approval. `compat.mcp_files = false` (in any config file) stops reading the file.


### MCP migration previews

Following the [v1 provider format](https://opencode.ai/docs/providers/) and [v2 provider format](https://opencode.ai/v2/docs/providers/), read-only provider imports also accept bounded OpenCode v1 `provider` entries with `npm/options`, and v2 `providers` entries with `package/settings`. Explicit supported OpenAI-compatible chat, OpenAI Responses and Anthropic packages select the native protocol; arbitrary packages are never loaded. Provider endpoints, credential/header environment references, ordered credential environment names and model aliases/names/disabled flags/limits are converted. Model IDs containing `/` retain escaped JSON-pointer provenance. Setup records cover accepted credential/header references; the ordered `env` list remains a native credential fallback, rather than a requirement to set every variable. Unsupported package/auth/account stores, advanced settings and package-specific variants remain pending. Unsafe substitutions, endpoint credentials/query strings and ambiguous or colliding bindings refuse the preview. Literal credentials never enter the proposal or report.

OpenCode v2 request `body` objects and model/variant `headers` now map to native request overlays. V2 `variants` arrays require unique explicit IDs and become native named variants. Native resolution keeps provider → model → selected variant order: nested body objects merge, later arrays/scalars replace earlier values and headers replace case-insensitively. Setup uses exact raw variant-array indices and excludes credentials hidden by native conflicts. Bodies are bounded to depth 16 and 4,096 nodes; null fields, credential keys/echoes and source substitutions require an additional adapter and refuse the current proposal. V1 package-specific model/variant options and v2 `settings` remain pending. [Source request-layer semantics](https://opencode.ai/v2/docs/models/).

Custom inline OpenCode agents now map source `agent`/`agents` to native profiles, including prompt/system text, description, mode, qualified model/variant, hidden/disabled flags, color, steps and strict permission conversion. Custom v1 profiles default to `all`; v2 profiles default to `primary`. V2 global and agent permission arrays append across source layers with original-index attribution. Native permission maps accept an ordered `rules` array alongside action shorthands, expanding it at the written position. Global and agent wrappers enforce ordinary deny ceilings, including under bypass. Project grants still need native checkout trust. Built-in agent defaults, prompt file references and advanced fields remain pending. [V1 agents](https://opencode.ai/docs/agents/) · [V2 agents](https://opencode.ai/v2/docs/agents/).

Explicit OpenCode compaction `auto`, `buffer` and `preserve_recent_tokens`/`keep.tokens` now map to native runtime settings. The validated numeric keep count is visible in previews; credential strings and header contexts remain redacted. Ambiguous token aliases and invalid integers refuse conversion; omitted source defaults and unsupported pruning/tail behavior are not inferred. `instructions` entries map as described under [What each source becomes](#what-each-source-becomes). [V2 compaction settings](https://opencode.ai/v2/docs/compaction/).

Read-only imports accept static Claude `mcpServers` (including `.mcp.json`), Codex `mcp_servers`, and OpenCode flat or `mcp.servers` declarations. Local commands/arguments and HTTP endpoints become native server proposals. Literal environment/header values become required environment references; JSON setup records identify the contributing source fields and never contain the original values. No source server starts during an import. Native `disabled: true` has the same server state and approval identity as `enabled: false`; conflicting flags refuse.

Unsupported server options appear as pending fields. Advanced authentication, additional transports, timeout conversion, source-relative working-directory semantics and Claude per-project state associations still need adapters. Global Claude state contributes only top-level server declarations. These previews remain incomplete and do not provide confirmed writes.

Codex HTTP server previews map `http_headers` and `env_http_headers` to native headers and preserve `required`. Static values become environment setup references; existing environment bindings remain references with source provenance. Conflicting header names/aliases and generated credential names colliding with another source binding refuse the whole preview. Command header helpers are not executed or imported.

### Kept-field comparisons

When a supported source field is ignored because a native or earlier auto-source key already exists, its `merged` report includes a `comparison` object with `kept` and `ignored` values. Text output prints both below the field. Values use the normalized native proposal shape, not original source formatting. Credential/header fields and known credential echoes are redacted, including echoes discovered in later source layers. Existing configuration remains unchanged. Complete native winning-layer provenance, runtime-effective values, remaining adapters and report-file writes still need implementation.

Setup records now include only environment bindings used by newly proposed fields. Kept credentials produce no setup instruction. Shared variables appear once with every accepted source field; ignored fields are excluded. Generated names cannot reuse environment references in existing native configuration, including embedded references with defaults. Collision checks run before shared records are combined. Adapter validation still refuses unsafe or ambiguous source declarations.


### Static configured commands

The public command catalogue and Session command endpoint now consume `commands` entries with `template`, optional `description` and `argument_hint`. Arguments use the same single-pass `$ARGUMENTS` and positional substitution as skills: argument text that contains `$1` or `$ARGUMENTS` is never substituted again. Without placeholders, a command appends its raw arguments after a blank line, and a skill appends `ARGUMENTS: <args>`. A user-invocable skill replaces a same-name command.

Templates of commands and skills run as user commands (`POST /api/v1/sessions/:id/command`, the TUI and `cyber exec --command`) can inject shell output and reference files:

- Each `` !`cmd` `` written in the template is replaced by the command's stdout. The command is treated as a `bash` call: it is denied when the agent's tool list or the running skill excludes `bash` or a `PreToolUse` hook blocks it, and it then needs `bash` permission for its text in the Session's agent, mode and rules: an `ask` rule prompts the attached client, and a denial (by rule, by `plan` mode, or an unanswered ask, which `dont-ask` turns into a denial) is substituted as `[denied: <cmd>]`. Allowed commands run concurrently in the sandbox from the Location, 30 s each, with stderr discarded, at most 20 KiB of stdout kept and credential-shaped text redacted. Without a sandbox that can confine the command (for example on Windows, unless the run selects `--sandbox full-access`) the substitution is `[unavailable: sandbox]` and nothing runs. Only spans written in the template run; arguments, command output and ordinary prompts never do. An argument placeholder inside a span is replaced by the argument words, each as one single-quoted shell word when it contains anything but letters, digits and `-_./=:,+%`, so `` !`git log -n $1` `` with the argument `$(id)` runs `git log -n '$(id)'`.
- A project command or skill (`.cyber/commands`, `.claude/commands`, project skills or configured skill paths) that contains `` !`cmd` `` is refused until you approve its exact text for the checkout with `cyber commands trust <name>`. Editing the template changes its digest and needs a new approval. Global commands need no approval.
- Each `@path` token (relative to the project root, or `~/` for home) attaches the file or directory as `<file path="...">` or `<directory path="...">` after the same `external_directory` and `read` checks as the `read` tool; a missing or refused path stays plain text. `@<subagent>` (for example `@explore`) adds an instruction to delegate to that subagent. `@server:uri` stays an MCP resource mention, while a drive-letter path such as `@C:\x` is a path.

Commands can override the agent and model in frontmatter (or the same keys in a `commands` config entry):

```markdown
---
description: Review as a subtask
agent: reviewer
subtask: true
model: provider/review-model
variant: high
---
Review $ARGUMENTS
```

With `subtask: true`, or an agent whose mode is `subagent`, the command runs in a background child Session with that agent, and the caller receives only the child's summary, as a forked skill does. The model is the command's `model`, then the agent's model, then the Session's; `variant` applies to whichever is selected. Without a child run a command keeps the Session's agent, so naming a different agent needs `subtask: true`. A `model` that is not `provider/model[#variant]`, an unknown `agent` or a `subtask` agent that cannot run as a subagent makes the command unavailable and appears as a command issue in `cyber commands list`, `GET /api/v1/skills` and `cyber doctor`.

Expansion happens once, when the command is admitted and after `UserPromptSubmit`; the expanded text is stored with the message, so a retry with the same message ID, a restart or a replay never runs a command again. A forked skill expands before its text reaches the child. Reserved names (`help`, `exit`, `goal`, `loop`, `workflows`, `mode`) remain protected; configured definitions and skills with those names use `project:<name>`.

OpenCode dry-run previews convert static inline `command`/`commands` entries into this consumed native shape, retaining exact source-field provenance. In import previews, definitions containing overrides (`agent`, `model`, `variant`, `subtask`), shell-output injection, `@file` references, source substitutions or unknown fields stay pending as whole definitions; natively discovered and configured commands accept injection and references, which expand only as described above. Read-time native Markdown discovery, complete invocation behavior, unavailable-definition inspection, CLI command management and reviewed migration writes remain open.


Discovered `.claude/commands/**/*.md` and OpenCode command Markdown now become static native command proposals. Optional YAML frontmatter supplies `description` and either argument-hint spelling; the body supplies the template. Reports use `body` and `frontmatter:<key>` descriptors for these source fields. Names retain nested paths and remove only the final `.md` suffix. Project and nearer directory definitions override global/ancestor definitions; same-scope Markdown replaces the inline definition as a whole. Existing native keys stay preserved. Unsupported headers, shell injection and file references remain pending without exposing body values or executing them. An unsupported winning definition suppresses older static fallbacks. Legacy `.opencode/command` files are supported; same-scope singular/plural name collisions refuse pending alias equivalence. Every read file, including overridden or unsupported commands, remains part of review verification.

At runtime, the application catalogue and command expansion also read static Markdown directly from global `command`/`commands` folders under the configured Cyber directory, and project `.cyber/command`/`.cyber/commands` folders from the checkout root to the current Location. Claude-compatible `.claude/commands` folders are read from the configured home and the same project chain. No import is required. Project definitions override global definitions; nearer project directories override ancestors. Within a scope, native Markdown overrides native inline configuration, which overrides compatibility Markdown. Explicit runtime/profile inline overrides without filesystem attribution retain highest precedence. The plural native folder wins over the singular folder when both declare the same name. Later skills retain precedence; reserved names use `project:<name>`.

Each catalogue or expansion request reads the files again; returned templates keep no filesystem handles. Discovery refuses linked directory traversal and hard-linked files, bounds directory entries to 4,096, recursion to 32 levels, files to one MiB each, aggregate reads to sixteen MiB and the registry to 128 names. Unsupported individual definitions suppress lower-priority fallbacks; an incomplete or oversized directory scan withholds the entire static registry. `CYBER_DISABLE_PROJECT_CONFIG` also disables project command discovery. Loaded plugins' `commands/` folders are discovered after every other source (see [Plugins](#plugins)). Advanced execution fields, source-path/diagnostic inspection, MCP command sources and full invocation semantics remain under implementation.


`GET /api/v1/commands` also includes `namespace` for namespaced entries and `provenance` for available static commands. Provenance contains the winning and shadowed origins, each with a scope (`global`, `project`, `runtime` or `plugin`), kind and source paths. It contains no templates or raw loader labels. Native inline configuration can combine leaves from several files, so its paths list includes all contributing file layers. Logical profile/CLI/environment overrides retain runtime precedence without inventing a filesystem source. The catalogue still lists available entries only. An entry without a declared hint gets one derived from its placeholders (`$1 $2` for `Ship $1 to $2`, or `$ARGUMENTS`).

Skills are inspected with `GET /api/v1/skills?agent=<name>` (default `build`), `cyber skills list`, `cyber skills show <name>`, `cyber commands list`, `cyber doctor` and the TUI `/skills` picker. Each loaded or skipped skill shows its source path, scope (`builtin`, `user`, `project` or `config`), whether users and the model can invoke it, the agent's `skill` permission (`allow`, `ask` or `deny`) and its diagnostics: parse errors and `replaces skill <name> from <path>` notices. Command files that were skipped appear as command issues. An agent profile's `skills: ["notes"]` preloads each named skill that the agent may load (discovered, model-invocable, not denied) into that agent's context; any other name is listed under `preload_issues` and in `cyber doctor`. Discovery runs on every request, so an edited `SKILL.md` shows up immediately. Skill discovery uses the same fence as commands: `SKILL.md` is read without following links, at most 1 MiB, and a linked `SKILL.md`, skill directory or skill root is skipped with a diagnostic instead of being read; a `model` that is not `provider/model[#variant]` is also a diagnostic. The `core/skills` listing evaluates permissions with the Session's agent profile and Session ruleset, and it never exceeds `skills.listing_budget_tokens` (four characters per token): descriptions are cut to 120 characters, then names are listed alone, then a `... N more` marker ends the list. When the model asks for an unknown skill, the error lists only skills that agent may load.

A `skills` entry that is an `https://` URL is a remote skill source:

```jsonc
{ "skills": ["https://skills.example.com/team"] }
```

The source serves `index.json` (`{"skills": [{"name": "lint", "files": ["SKILL.md", "docs/style.md"], "version": "3"}]}`) and each file at `<url>/<name>/<file>`. Cyber Code downloads them in the background into `~/.cache/cyber/skills/<host>/<source-id>/<generation>/<name>/` (`<host>` gets a `_<port>` suffix when the URL has one, and `<source-id>` is derived from the URL so two sources on one host never share a directory); `<source-id>/.marker.json` names the current generation, and a refresh writes a new generation before switching the marker to it and loads skills from that cache at the entry's position in the precedence order, so startup never waits for the network. The cache is refreshed at most once every 24 hours; offline, the last good copy keeps loading and its skills carry a `using stale cache` diagnostic once it is older than that. A source not cached yet, a failed fetch or a refused index shows up as a skipped skill in `cyber skills list`, `GET /api/v1/skills` and `cyber doctor`. Only `https://` URLs without credentials, query or fragment are accepted; requests go through the same network policy and owned proxy as other host requests and never follow redirects. The index may list 64 skills of 64 files each, file paths must be plain relative names (letters, digits, `.`, `_`, `-` and `/`, no `..`, hidden names or Windows device names), files are capped at 1 MiB (4 MiB per skill, 16 MiB per source), and each `SKILL.md` must declare its index name; anything else fails the refresh and keeps the previous cache. Sources from your global config (or `CYBER_CONFIG`, `--config`) are used as soon as they are cached. A source listed in project config refreshes under the sandbox network mode and allowlist and is not loaded until you review it and run `cyber skills trust <url>` in the checkout, which approves that URL with the content cached now; when the remote set changes, its skills are skipped again until you re-approve. Remote skills get the usual read access to their own directory and nothing more, and a remote template that runs shell commands still needs `cyber commands trust <name>`.


Skills may declare project-relative path globs:

```yaml
paths: ["db/migrations/**", "src/*.{rs,sql}"]
```

Successful file reads, writes, edits and patches append a `<system-reminder>` suggesting matching skills. Directory listings, failed reads, model-disabled skills and effective skill/tool denials do not produce suggestions. Reminders include names and shortened descriptions. The model can load the body through the normal skill tool. Each skill is suggested once per Context Epoch. The seen names commit with the tool result, survive runtime recreation and reset when a new Epoch starts, including after compaction or context repair. Concurrent file calls share that durable decision. Relative declarations are bounded to 64 globs of 256 bytes each. Known completed file operations also produce reminders when a later patch operation or post-hook fails; the tool retains its error status. Aborted/unresolved operations do not produce reminders. Reminders share the configured line/byte payload budget with tool text. Overflow preserves complete text in a managed artifact and keeps the reminder visible. If the reminder itself cannot fit or overflow storage fails, settlement fails without marking the skill seen; a later matching operation can retry after repair.

Inline model-loaded skills can declare bounded `allowed-tools` and `disallowed-tools` lists or comma-separated strings. Patterns use `tool[:resource]`, with `*` and `?` wildcard matching; a bare tool covers all resources. Captured grants apply to later tool groups in the same model Turn, preserve explicit permission denials and expire before the next Turn. Skill loading is a batch barrier. Disallowed patterns override grants and bypass; descendants inherit denials. Instruction text cannot create a grant. Inline user commands capture those declarations with the expanded prompt. Their scopes apply to the first model Turn; an optional skill `model` applies through its tool loop without replacing the Session model. Captured values survive replay, and changed declarations conflict with reuse of an existing request ID. Explicit skill denials refuse admission even in bypass; ordinary prompt text cannot supply declarations. Normal model-loaded inline skills do not switch models. Complete lifecycle and native acceptance remain open.

Skills can declare `context: fork` to run in a background child with the caller's history, using the skill's model when specified. An optional `agent` names the child profile; without it the caller's profile is retained. Agent selectors require fork context and are captured in retry/replay identity. The built-in `reviewer` permits inspection and has an immutable filesystem read-only ceiling, including for descendants: bypass, configured grants and skill grants cannot authorize file edits or mutating shell commands. Use a different custom profile name for coding subtasks. The caller receives the queued summary; instruction bodies and child intermediate output stay in the child. The captured child instruction frame includes the skill base directory and a bounded list of supporting files. User commands preserve held input, release and queue delivery, and bind captured declarations to their retry identity. Child creation shares ordinary subagent admission, Mode/ancestor ceilings and cancellation ownership. Refusal or cancellation before child admission queues a durable status handback with the skill name and delegation ID. Setup diagnostics remain available through the delegation API; replay delivers a missing handback once. Unknown admission ownership requires reconciliation rather than another launch; full native and lifecycle acceptance remains open.


Five bundled skills ship inside the binary and appear in the command catalogue without installation:

| Command | Purpose | Execution |
|---|---|---|
| `/review [target]` | Find actionable defects and request structured findings | Background reviewer |
| `/simplify [scope]` | Find reuse and simplification opportunities | Background reviewer |
| `/security-review [scope]` | Trace pending changes for exploitable defects | Background reviewer |
| `/batch <change>` | Coordinate 5-30 independently checked tasks | Caller, delegating separate worktrees |
| `/customize-cyber <request>` | Explain or update configuration and integrations | Caller |

Embedded instruction frames use `builtin:<name>` as their source; this is not a readable filesystem directory and grants no external-directory access. A discovered `SKILL.md` of the same name replaces the package. Configured static commands take precedence over the embedded defaults; discovered skills still take precedence over those commands. Unsupported winning command definitions remain unavailable. File-backed replacements keep their ordinary directory and supporting-file permissions. These are instruction packages using existing tool authority, not permission exemptions. Full review target resolution, the dedicated `cyber review` CLI and enforced finding validation remain under implementation.

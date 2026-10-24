## Context

- `cyber_core::version` already embeds the version, channel (`CYBER_CHANNEL` at build time, default `dev`), git SHA, build identity and target triple. `cyber --version` prints them. `build.rs` sets them.
- `cyber_core::paths::Paths` resolves the data, config, state, cache and tmp directories (XDG, `CYBER_HOME`, `CYBER_CONFIG_DIR`).
- `cyber_app::registration` owns the background server (`start_service`, `stop_service`, stale-build replacement). After a self-replace, the next client sees a new executable stamp. It replaces an idle server and warns about a busy one, so an upgrade never cuts off a running Turn.
- `ci.yml` cross-builds six Unix targets with `--release` and builds Windows natively. Nothing is published.
- Config already accepts the top-level key `autoupdate` with default `"notify"`, but nothing validates its type.
- Live-only events go through `cyber_server::runtime::bus::LiveEvent` (for example `mcp.tools.changed.1`). Durable events require a registered schema. Update notices are machine-level and transient, so they are live-only.
- Keyring entries owned by cyber use the services `cyber-code` (peer tokens), `cyber-mcp-oauth` (MCP OAuth) and `cyber-plugin-config` (plugin secrets), plus provider connections.

## Goals / Non-goals

Goals: a signed, reproducible release from one tag. A verified install on macOS and Linux. Upgrades and uninstalls that fail closed. An update notice that never interrupts work. Everything testable offline.

Non-goals: see the proposal (publishing to package registries, `cyber-code.dev`, P1 shell integration, P3 HTTP 426, Authenticode and notarization).

## Decisions

### 1. One build-time release source

`cyber_core::version::ReleaseSource::built_in()` returns the values `build.rs` embeds, and `ReleaseSource::resolve(env)` applies the debug-only test hook, which sits in the single function `test_hook`. It lives in `cyber-core`, not `cyber-install`, so `cyber debug info` and later the server read it without the install crate:

| Constant | Default | Build override |
|---|---|---|
| `base_url` | `https://github.com/CyberdyneCorp/cybercode-cli/releases/download` | `CYBER_RELEASE_BASE_URL` |
| `identity_prefix` (`v<version>` appended) | `https://github.com/CyberdyneCorp/cybercode-cli/.github/workflows/release.yml@refs/tags/` | `CYBER_RELEASE_IDENTITY` |
| `issuer` | `https://token.actions.githubusercontent.com` | `CYBER_RELEASE_ISSUER` |

Layout under `base_url`. The archives and `SHA256SUMS` are under `v<version>/`. The channel manifest is `channel-<channel>/<channel>.json`: each channel has a rolling GitHub Release tagged `channel-<channel>`, and the release workflow replaces its single asset. GitHub serves both with stable URLs and no extra hosting. A fork or a future `cyber-code.dev` changes the three build variables and nothing else.

- **Why build time, not config:** a config or environment value that redirects updates or changes the trusted identity is a supply-chain hole. Anyone who can set the environment could make `cyber upgrade` install their binary.
- **Test hook:** in builds with `debug_assertions` (the `target/debug/cyber` that tests run), `CYBER_TEST_RELEASE_BASE_URL` replaces `base_url`. `CYBER_TEST_RELEASE_TRUST=<dir>` replaces the trusted root and identity with `<dir>/trusted_root.json` and `<dir>/identity.json` (`{identity_prefix, issuer}`); an unreadable identity file is an error, never a fallback to the defaults. The code reading them is `#[cfg(debug_assertions)]`, so release builds ignore both variables. `docs/local-core-guide.md` documents the hook under "Testing".
- The install script uses the same defaults as shell variables. `CYBER_INSTALL_BASE_URL` overrides the base for tests and mirrors. The checksum still comes from that base, and a user who sets it chooses the source. With `cosign` present, the identity check is not overridable.

### 2. Crate layout: `crates/cyber-install`

It is a new workspace member, so the Sigstore and archive dependencies stay out of `cyber-core`, `cyber-server` and the TUI. It depends on `cyber-core` (paths, version, env), `reqwest`, `sha2`, `semver`, `flate2`, `tar`, `zip` (deflate only) and `sigstore-verify`.

| Module | Responsibility |
|---|---|
| `manifest` | `ChannelManifest {version, published_at, notes_url, min_supported}`, fetch (10 s timeout, 2 retries with 1 s / 2 s backoff), 1 h cache in `<cache>/release-<channel>.json`, `CYBER_OFFLINE` |
| `method` | `InstallMethod` detection |
| `verify` | `SHA256SUMS` parsing, digest check, Sigstore bundle verification |
| `fetch` | Streaming download to a temporary file beside the target, with a size cap of 512 MiB |
| `archive` | Extraction of `cyber` and `cyber-sandbox-exec` only, with path validation |
| `replace` | Atomic self-replace and `cyber.old` cleanup |
| `upgrade` | `UpgradePlan` (self-replace, manager command, or manual text) and its execution |
| `uninstall` | `UninstallPlan` (entries with kind, path and size), PATH-line edits, safe removal |
| `policy` | Pure background-update decisions (check or not, notify or silent upgrade, bump kind) |

Every function keeps cognitive complexity ≤ 15. Each pure decision (`policy`, `method`, plan building) is separated from IO so it can be table-tested.

### 3. Sigstore verifier

`sigstore-verify` (the official `sigstore/sigstore-rust` project, Apache-2.0) is pinned to an exact version that is at least two weeks old when this lands (`=0.14.0` today). Its reasons:

- It is pure Rust on `rustls-webpki` 0.103 and `aws-lc-rs`, the crypto provider that `reqwest` and `cyber-server` already use. So it builds wherever the workspace builds, including musl (aws-lc-rs builds there in CI today) and Windows MSVC.
- It verifies a cosign v3 / protobuf bundle offline against an embedded trusted root (`SIGSTORE_PRODUCTION_TRUSTED_ROOT`). Upgrades need no TUF fetch at run time.
- Its `VerificationPolicy` checks the certificate chain to Fulcio, the SCT, the Rekor inclusion proof and integrated time, the SAN identity and the OIDC issuer.

Alternatives rejected:

- The `sigstore` crate is labeled experimental and pulls OpenSSL or extra TLS stacks.
- Shelling out to `cosign` is not installed on user machines and would fail open whenever it is missing.
- A minisign/ed25519 key would need key custody, and the spec requires Sigstore keyless signing.

The production policy is `VerificationPolicy::new(identity_prefix + "v" + version, issuer)` with tlog and SCT required. The trusted root ships in the binary. A stale root fails closed, and the next release updates it. The `trust_root` refresh is a crate upgrade.

**Testing the verifier:**

- Unit tests run the production policy on the real cosign v3 blob fixture from `sigstore-rust` (`cosign-v3-blob.txt` and `.sigstore.json`, vendored under `crates/cyber-install/tests/fixtures/` with its licence note). A good blob passes. A changed blob, a changed identity and a changed issuer each fail.
- End-to-end tests through the real binary use the debug-only trust hook. A test helper (`cyber-install` feature `test-support`, dev-only) issues a throwaway CA and a leaf certificate with the expected SAN and Fulcio issuer (v2) extension (`rcgen`), then signs `SHA256SUMS`. The verifier needs a verified signing time, so the helper also holds a throwaway log key: each bundle carries a hashedrekord 0.0.1 entry whose signed entry timestamp that key signs, and the trusted root lists the CA and the log key. Under the hook, the inclusion proof and SCT checks are skipped (`skip_tlog_unsafe`, `skip_sct`), which is the only policy difference; the chain, identity, issuer, signed entry timestamp, signature and entry consistency are still checked. The `skip_*` calls and the field that enables them exist only under `#[cfg(debug_assertions)]`; a release build that somehow received a test root refuses it.

### 4. Integrity flow (fails closed)

```
resolve target version (argument, or manifest.version)
download v<V>/SHA256SUMS and v<V>/SHA256SUMS.sigstore.json   ── any HTTP error → abort
verify bundle over SHA256SUMS bytes with policy(identity v<V>) ── fail → "signature verification failed"
find line "<hex>  cyber-<V>-<target>.<ext>"                  ── missing → "no release for <target>"
download archive to <install_dir>/.cyber-upgrade-<random>.tmp
sha256(archive) == line                                      ── fail → "checksum verification failed"
extract cyber (+ cyber-sandbox-exec) beside the target, fsync
atomic replace (below); print "cyber <old> → <V>"
```

Any error deletes the temporary files and leaves the installed binary byte-identical. The version embedded in the identity binds the signature to the requested tag, so an older signed release cannot be replayed as a newer one. That holds for the version cyber asks for, and the unsigned channel manifest chooses it when no version is named, so `Plan::to_latest` installs a manifest's version only when `policy::bump` finds it newer than the running one; an older one (a backport tag, a replayed or overwritten manifest, a pre-release build reading `stable`) plans `Current`. Only an explicit `cyber upgrade <older>` (`Plan::new`) downgrades, knowingly. With `CYBER_OFFLINE`, a `Replace` or `Manager` plan fails before any request; `Current` and `Manual` are still reported.

### 5. Atomic self-replace

- **Unix:** extract the new binary to a hidden `.cyber-upgrade-*.new` file in the same directory, give it the old file's permission bits (`0o777` mask, `0o755` for a first install), `fsync` the file, rename the current `cyber` to `cyber.old`, rename the new file to `cyber`, then `fsync` the directory. The sandbox helper is replaced first in the same way, so a failure halfway leaves a matching or older pair, never a new `cyber` with a missing helper. A failed second rename renames `cyber.old` back.
- **Windows:** the running `.exe` cannot be overwritten but can be renamed. Rename `cyber.exe` to `cyber.old.exe`, then rename the new file into place. No path is hard-coded with `/`. Paths come from `std::env::current_exe()` and are not canonicalized, because `canonicalize()` returns `\\?\` paths.
- **At start,** `cyber-cli` `main` removes `cyber.old[.exe]` and `cyber-sandbox-exec.old[.exe]` beside `current_exe()` when they are regular files (`symlink_metadata`). This is best-effort and silent.
- **Interrupted download:** the target is untouched, because nothing is renamed before verification completes.

### 6. Installation method detection

`detect(exe, env, home)`:

1. `script`: `exe` is under `$CYBER_INSTALL_DIR` or `~/.cyber/bin`, or the install script's receipt `.cyber-receipt` (a regular file, checked with `symlink_metadata`) sits beside it. The script writes the receipt into every install directory, so a `--dir` install is still `script` once `CYBER_INSTALL_DIR` is gone (the PATH line does not set it); `cyber uninstall` removes it with the binaries.
2. Path patterns in this order: `brew` (`/Cellar/cyber/`, `/opt/homebrew/bin`, `/home/linuxbrew/`), `npm` / `pnpm` / `bun` (`node_modules/@cyber-code/cli`, `/.pnpm/`, `/.bun/`), `cargo` (`$CARGO_HOME/bin` or `~/.cargo/bin`), `scoop` (`/scoop/apps/cyber/`), `winget` (`/WinGet/Packages/`), `nix` (`/nix/store/`), `docker` (`/.dockerenv` exists and `exe` is `/usr/local/bin/cyber`).
3. Otherwise, it queries each manager on PATH (`brew list --versions cyber`, `npm ls -g @cyber-code/cli --json`, `pacman -Qq cyber-code`, and so on). Each query has a 3 second timeout. Managers named in the path are queried first.
4. Else `unknown`.

`--method <m>` overrides detection. `method::detect_origin` also says whether the path (rule 1 or 2) identified the method: a method from a listing alone (rule 3) says a package is installed somewhere, not that the running binary is it, so nothing that changes an install acts on it unattended (§7, §8, §10). Manager commands run as an argv (`std::process::Command`, never through a shell), resolved on PATH with `PATHEXT` on Windows.

### 7. Upgrade command

`cyber upgrade [version] [--method m] [--channel c]`:

- **The target** is `version`, or else `manifest(channel).version`. The channel is `--channel`, else the build channel, with `dev` mapped to `stable` for the lookup. The command skips with `cyber <v> is already installed` when the target equals the running version.
- **Per method:**
  - `script`: §4 and §5.
  - `brew upgrade cyber`
  - `npm i -g @cyber-code/cli@<v>` (`pnpm add -g` and `bun add -g` likewise)
  - `cargo install --locked --git https://github.com/CyberdyneCorp/cybercode-cli --tag v<v> cyber-cli`
  - `scoop update cyber`
  - `winget upgrade cyber-code.cyber`
  - `nix`, `aur`, `docker` and `unknown` print `installed via <m>; run: <cmd>` and exit 0: `nix profile upgrade cyber-code`, `yay -Syu cyber-code`, `docker pull ghcr.io/cyber-code/cyber:<v>`, and the install script one-liner for `unknown`.
- **A method from a listing alone** (no `--method`): the manager's command is printed, never run, as `<m> lists a cyber package, but this binary is not from it; to upgrade that package, run: <cmd> (or "cyber upgrade --method <m>" to run it)`, with action `manual` and `command`. Running it would change that package and report `cyber A → B` for a binary it did not touch.
- **A manager command that fails** prints the command and exits 1.
- **`--format json`** prints `{from, to, method, action}`, with `action` one of `current`, `replaced`, `manager` or `manual` (plus `command` for `manual`).
- **Downloads** use a 30 second connect and per-read timeout instead of a total one, so a slow but live transfer of a large archive finishes. `SHA256SUMS` and bundles are capped at 1 MiB, archives at 512 MiB, and a body shorter than its `Content-Length` is an error.

### 8. Latest-version lookup and background policy

- **Cache:** `<cache>/release-<channel>.json` holds `{fetched_at_ms, manifest}` and is written atomically. A fresh cache (< 1 h) is used without the network. `CYBER_OFFLINE` returns the cache regardless of age, or `offline` when there is none. Every request sends the spec'd User-Agent.
- **Background task:** `cyber_app::update_check` is spawned by `run_server`. It runs 5 seconds after start and then every 24 hours, and the TUI's "at most once per 24 hours" is this cadence. `ServeOptions::update` (`UpdateCheckOptions`) carries the version, channel, target, binary, environment and cadence, defaulting to the build's and `ProcessEnv`; tests pin them, since test builds are `dev`. `policy::should_check` returns false for `autoupdate = false`, `CYBER_DISABLE_AUTOUPDATE`, `CYBER_OFFLINE`, `CI=true`, the channel `dev`, or org policy that sets `autoupdate = false`. Org policy is a new managed key, `policy.autoupdate`, merged like `policy.messaging`: it lands at `/policy/autoupdate` with a `managed:` source (`AutoUpdate::managed` ignores a user-written `policy.autoupdate`), wins over the user's `autoupdate`, and a malformed policy sets it to `false`. Config is re-read at every check; when it fails to load, that check is skipped with a warning (defaults would drop the org policy). The binary path is resolved once when the task starts: on Linux `current_exe` reads `/proc/self/exe`, which follows the rename to `cyber.old` (then `cyber.old (deleted)`). After a silent upgrade the process only announces later releases until it restarts, since its running build no longer matches the disk.
- **Result:**
  - A newer version with `autoupdate = "notify"`, or any minor or major bump, publishes `LiveEvent::InstallationUpdateAvailable` (`installation.update_available.1`: `{current, latest, kind, notes_url}`).
  - A patch bump with `autoupdate = true` on `script`, `brew` or `npm` runs the same upgrade as the command, then publishes `installation.updated.1`. A failed silent upgrade falls back to the notice. Only a method the binary's path identified is upgraded silently; one from a listing alone is announced (`updated: false`) and the manager is never run.
  - Both events are live-only `LiveEvent` variants carrying `InstallationNotice`, delivered on every instance stream regardless of Location.
  - The silent self-update runs on a blocking thread with its own current-thread runtime (its download future is not `Send`); a version already installed silently is not downloaded or announced again.
- **`GET /api/v1/service/update`** (group `service`) returns the last result: `{current, channel, method, latest, kind, notes_url, min_supported, unsupported, updated, checked_at_ms}`, with `latest: null` before the first check. `updated` tells the late TUI which toast to show. The status is held by the `Runtime` (`UpdateStatus`), so the endpoint also answers on routers without listeners; `App::build` seeds it from the cached manifest. A TUI that attaches late still shows the notice: it reads the endpoint on every `server.connected` and shows each notice once.
- The running server never restarts itself. The existing stale-build rule replaces it when it is idle.

### 9. Minimum supported version

The TUI (at first frame) and `exec` (before submitting) read the cached manifest only, never the network, so startup stays under budget. `exec` reads `<cache>/release-<channel>.json` itself (`manifest::read_cache`); the TUI reads `unsupported`/`min_supported` from `GET /service/update`, which the server seeded from the same cache at build time. The text is `cyber_core::version::unsupported_warning`, shared by both. When `min_supported > version`, they print a warning on every start: to stderr for exec, as a persistent banner in the TUI. The warning reads `warning: cyber <v> is no longer supported (minimum <m>); run "cyber upgrade"`. The background check keeps the cache fresh.

### 10. Uninstall

`UninstallPlan::build(target, flags)` returns ordered entries. `Target` carries the paths, the home directory, the method, the running exe, the rc candidates and `CYBER_HOME`:

- **binary** (`script` only): `cyber.old`, the helper's `.old`, `cyber-sandbox-exec` and `cyber` beside the running exe, in that order, so on Windows the running exe can take the name `cyber.old.exe`.
- **path line:** each rc file containing the installer marker `# cyber installer: PATH`: `~/.zshrc`, `~/.bashrc`, `~/.bash_profile`, `~/.profile` and `$XDG_CONFIG_HOME/fish/config.fish`. A symlinked rc file is not rewritten (that would replace the link with a file); the plan carries a note to edit it by hand.
- **config, data, state, cache directories.**

`--keep-config` and `--keep-data` drop those entries. Each entry carries a recursively computed size, counted with `symlink_metadata` so links are not followed.

Before removing, the command:

1. stops the background server with `stop_service`. It aborts if the server cannot be stopped and `--force` is not given.
2. deletes cyber-owned keyring entries unless `--keep-data`. The accounts come from cyber's own records: every server in the MCP OAuth index (`cyber_tools::mcp::oauth::indexed_servers`, then `remove_server`), the peer tokens of `peers.remotes`, and the secret fields of each resolved plugin's manifest (`plugin_config::forget_secrets`). Provider logins (ChatGPT, Copilot) are files in the data directory, and keyring provider connections do not exist yet. With no records there is no keyring call. A keyring failure is reported, does not abort, and makes the command exit 1 at the end.
3. for package-manager methods, runs the manager's uninstall command (`brew uninstall cyber`, `npm rm -g @cyber-code/cli`, `cargo uninstall cyber-cli`, `scoop uninstall cyber`, `winget uninstall cyber-code.cyber`), printing it on failure. The command is listed as `package manager: <cmd>` with the entries, and named in the prompt, before anything runs. It runs only when `method::from_path` identified the binary; a method known from the managers' listings alone becomes a note to run the command by hand.

Removal safety:

- A directory is removed only when it equals one of the resolved `Paths` directories (or `CYBER_HOME` once emptied) and passes `is_safe_root`:
  - it is not `/`, not the home directory or an ancestor of it, and not a drive root;
  - it has at least two components below home or root;
  - it is not a symlink.
- Sizes are counted with `symlink_metadata`. Removal uses `std::fs::remove_dir_all`, which unlinks symlinks and Windows junctions without descending into them and, on Unix, walks by directory descriptor, so a path swapped for a link mid-walk is not followed either.
- "Resolved" is checked on the text of the path (`Path::components` hides an interior `.`). The home comparison is made as written and on canonical forms, so `/var/...` and `/private/var/...` are the same home.
- The command builds its context without `Paths::ensure`, the global config template or a log file (`Context::existing`), so a dry run creates nothing and Windows holds no open log in the data directory.
- PATH lines are removed by rewriting the rc file atomically with its mode preserved. Only the marker line and the single line after it are removed.
- **Windows:** the running `cyber.exe` is renamed to `cyber.old.exe`, which the next run or the user removes. The command says so.

The confirmation prompt is `Remove these N items (X MiB)? [y/N]`. A non-TTY without `--force` exits 2.

### 11. Release workflow (`.github/workflows/release.yml`)

- **Trigger:** `on: push: tags: ['v*']` only. No `pull_request`, no branch pushes, no `workflow_dispatch`.
- **`verify` job:** it runs `scripts/tests`; the tag must equal `v` + the workspace version (`release_assets.py check-tag`). The channel comes from the tag: `-beta.N` is beta, `-nightly.YYYYMMDD` is nightly, otherwise stable.
- **`build` matrix, 8 targets:**
  - the six Unix runners from `ci.yml`;
  - `windows-2025` for `x86_64-pc-windows-msvc`;
  - `windows-11-arm` for `aarch64-pc-windows-msvc`.
  - `ci.yml`'s `platforms` job builds the same eight targets with `--release --workspace --bins` on every pull request (a workflow test keeps the two lists equal), so a target that does not build fails a PR, not a pushed tag. The publish job still needs every build: a release is never published without its Windows archives.
  - Each builds with `CYBER_CHANNEL`, `CYBER_GIT_SHA` and `--locked --release`, without a build cache. It smoke-tests that `cyber --version` names the version and channel, and `release_assets.py package` writes `cyber-<v>-<target>.tar.gz` / `.zip` (holding `cyber[.exe]`, `cyber-sandbox-exec[.exe]`, `README.md` and `LICENSE` when the repository has one, at the archive root, where `cyber upgrade` looks for the binaries). Members get mtime 0 and fixed modes, so the archive bytes depend only on the binaries. Windows archives carry `PREVIEW.txt`. Packaging is Python standard library so the Windows runners share it.
- **`publish` job** (`permissions: contents: write, id-token: write`, and only this job):
  1. downloads the archives;
  2. writes `SHA256SUMS`;
  3. `cosign sign-blob --yes --bundle <f>.sigstore.json` for each archive and for `SHA256SUMS` (`sigstore/cosign-installer` pinned by commit SHA);
  4. renders `<channel>.json` (`min_supported` from the repository variable `CYBER_MIN_SUPPORTED` when set) and the `packaging/` templates with the version and checksums, as flat assets `cyber.rb`, `cyber.scoop.json`, `cyber.winget.yaml`, `PKGBUILD`, `cyber.nix` and `cyber-npm-<v>.tar.gz` (the `@cyber-code/cli` package and one package per platform, each with `bin/cyber[.exe]` and `bin/cyber-sandbox-exec[.exe]` (mode 0755) extracted from its target's archive after that archive's SHA-256 matches `SHA256SUMS`; Linux packages carry the static musl builds). An unknown template key is an error, then `release_assets.py assets` checks the release holds exactly the expected assets;
  5. downloads the published `<channel>.json` (when `channel-<channel>` exists; a missing asset there fails the job) and runs `release_assets.py newer`, a semver comparison;
  6. `gh release create v<v> --verify-tag` (`--prerelease` unless stable; a stable tag is `--latest` only when newer, else `--latest=false`) with every asset plus `install.sh`;
  7. only when newer, `gh release upload channel-<channel> <channel>.json --clobber`, creating that release (`--latest=false`) when it is missing. A backport tag is released but never rolls the channel back, and `concurrency: release` (one group for every tag) keeps two racing tags from both reading the old manifest.
- Third-party actions are pinned by commit SHA, and the workflow defaults to `bash` (with `pipefail`) on every runner. It is linted with `actionlint` (1.7.11, with `shellcheck`) and is never run from this change. `scripts/tests/test_release_workflow.py` checks it as text (YAML 1.1 reads `on` as a boolean): the only trigger is a `v*` tag push, only `publish` has write or token permissions, the 8 targets match `release_assets.TARGETS`, and every `uses:` names a 40-hex commit.

### 12. Install script (`scripts/install.sh`)

- **POSIX `sh`:** `set -eu`, no bashisms, passes `sh -n`, `dash -n` and `shellcheck -s sh` where installed.
- **Detection:** `uname -s` and `uname -m` give the OS and CPU. On Linux, musl is chosen when `/etc/alpine-release` exists or `ldd --version` mentions musl.
- **Downloads** use `curl -fsSL` or `wget -q`, with no `eval`. Arguments and the version are validated against `^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?$` (a `case` character check, then `expr`), and the channel against the three names; a bad value or unknown argument exits 2. `--dir` must not hold `"`, `$`, `` ` ``, `\` or a newline, because it is written quoted into the PATH line. Every expansion is quoted. Downloads go to a temporary directory outside `<dir>`.
- **Already installed:** when `<dir>/cyber --version`, or without `--dir` the `cyber` on PATH, reports the requested version, it prints `cyber <v> already installed` and exits 0. An explicit `--dir` ignores PATH, so installing into a new directory is never skipped.
- **Verification:** `sha256sum` or `shasum -a 256` checks the archive. A mismatch prints `checksum verification failed` and exits 1 before anything is written to `<dir>`. With `cosign` on PATH, the script verifies `SHA256SUMS.sigstore.json` with the fixed identity and issuer.
- **Install:** extract to a temporary directory inside `<dir>`, then `mv` each binary into place (same filesystem, so the rename is atomic).
- **PATH:** unless `--no-modify-path`, append the marker and `export PATH="<dir>:$PATH"` (`fish_add_path` for fish) to the first existing rc file for `$SHELL` (zsh `.zshrc`; bash `.bashrc`, `.bash_profile`, `.profile`; fish `config.fish`; else `.profile`). It does nothing when the marker is already there, adds a newline only when the file does not end in one (so `cyber uninstall` restores it byte for byte), and prints the line instead of writing through a symlinked rc file.

### 13. Container and CI

- **`Dockerfile`:** `debian:bookworm-slim` (digest-pinned), with `git`, `ripgrep`, `ca-certificates` and `bubblewrap`. It creates `cyber` with uid/gid 10001, `ENV CYBER_HOME=/home/cyber/.cyber`, `CI=true`, `USER cyber`, `WORKDIR /work` and `ENTRYPOINT ["cyber"]`. `ARG TARGETARCH` copies the prebuilt musl binary from `dist/<arch>/`, so the image build needs no Rust toolchain, and the release workflow's future image job uses the release archives. The image is not built or pushed in this change.
- **`CI=true`** (any truthy value, through `EnvSource::flag("CI")`):
  - every command that opens the TUI (`cyber` with no subcommand, `cyber attach`) prints `interactive TUI unavailable in CI; use "cyber exec"` and exits 2, through one helper (`commands::tui::refuse_in_ci`), before connecting anywhere;
  - the TUI also suppresses its import and plugin onboarding offers, as defence in depth;
  - `policy::should_check` returns false;
  - `exec` keeps its default `--format text`.

## Risks / Trade-offs

- **Embedded trusted root ages:** if Sigstore rotates keys, an old binary fails closed and asks for a manual reinstall. Accepted, because failing closed is the requirement. Each release refreshes the root through the crate.
- **GitHub Releases rate limits** (60 unauthenticated requests per hour per IP for the API). Downloads use `releases/download/` URLs, not the API, and the 1 h cache bounds lookups.
- **The `windows-11-arm` runner or the aws-lc-rs ARM64 MSVC build** may be unavailable. The workflow then fails visibly; no partial release is created, because `publish` needs every build.
- **Silent patch upgrades from the server** replace the binary under a running client. The existing executable-stamp rule restarts only an idle server, and clients keep running their mapped image.
- **Debug-only test hook:** a debug build is not a release artifact. Release builds use `--release`, whose profile keeps `debug_assertions` off; `Cargo.toml` never enables them for `release`.

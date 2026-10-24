## 1. Release source, lookup and method detection

- [x] 1.1 `build.rs` and `cyber_core::version::ReleaseSource`: base URL, identity prefix and issuer constants with the GitHub defaults and the `CYBER_RELEASE_*` build overrides. The `#[cfg(debug_assertions)]` test hook `CYBER_TEST_RELEASE_BASE_URL` / `CYBER_TEST_RELEASE_TRUST` sits in a single function. Unit tests cover the defaults and asset URL layout (`v<v>/cyber-<v>-<target>.tar.gz|.zip`, `channel-<c>/<c>.json`). `cyber debug info` (format json) shows the release base. Production path: `crates/cyber-cli/tests/upgrade_cli.rs::debug_info_reports_the_release_source`.
- [x] 1.2 New crate `crates/cyber-install` (workspace member) with `manifest`. It fetches with a 10 s timeout, 2 retries and the spec'd User-Agent, keeps a 1 h atomic cache in `<cache>/release-<channel>.json`, honors `CYBER_OFFLINE` (cache of any age, else `offline: no cached release information`), and maps `dev` to `stable`. Tests use a local stub HTTP server that counts requests, for the "Cached lookup" and "Offline lookup" scenarios. Production path: `upgrade_cli::upgrade_uses_the_cached_manifest_without_a_request` (real binary, stub server through the debug hook).
- [x] 1.3 `method` detection: install-dir and `CYBER_INSTALL_DIR` rules, path patterns, then manager listings (3 s timeout, argv only, path-named managers first). Table tests cover every path pattern, including the "Homebrew detection" and "Script detection with a custom directory" scenarios. Listing tests use stub manager executables on PATH (`#[cfg(unix)]`). Production path: `upgrade_cli::upgrade_reports_the_detected_method` (`cyber upgrade --format json` for a binary copied under a temp `CYBER_INSTALL_DIR`).
- [x] 1.4 Typed `autoupdate` validation (`true | false | "notify"`) in `cyber_core::config::validate`, with a test that rejects `"yes"`. Production path: `cyber debug config` on a bad value through the real binary (`upgrade_cli::autoupdate_rejects_unknown_values`).

## 2. Verified self-update and `cyber upgrade`

- [x] 2.1 `verify`:
  - `SHA256SUMS` parsing (strict `<64 hex>  <name>` lines; duplicates refused).
  - Sigstore bundle verification with `sigstore-verify` pinned to an exact version at least two weeks old, against the embedded production trusted root with the identity `<prefix>v<version>` and the issuer.
  - Unit tests on the vendored real cosign v3 blob fixture: it passes; a changed blob, identity or issuer fails.
  - The `test-support` feature issues a throwaway CA and leaf with `rcgen` for the debug trust hook.
  - `cargo tree` shows the crate builds for musl and MSVC: `ci.yml` platforms and Windows jobs stay green.
- [x] 2.2 `fetch` (streaming to a temporary file beside the target, 512 MiB cap) and `archive` (only `cyber`/`cyber-sandbox-exec`, traversal and links refused). Tests cover truncated bodies, oversize bodies, and archives with `../` or symlink members.
- [x] 2.3 `replace`:
  - Unix: write beside the target, keep the mode, `fsync`, rename to `.old`, rename in, `fsync` the directory, roll back on a failed rename. The helper is replaced first.
  - Windows: rename the running exe aside, without `canonicalize`.
  - `cyber.old` cleanup at start in `cyber-cli` `main` (regular files only).
  - Unit tests for rollback and permissions. The Windows branch is reviewed by hand for dead code under `cfg(windows)` clippy.
- [x] 2.4 `upgrade` planning and `cyber upgrade [version] [--method] [--channel] [--format json]` (`commands/upgrade.rs`, one `cli.rs` variant, one dispatch arm). It covers the already-installed skip, manager commands as argv with print-on-failure exit 1, and manual text for `nix`/`aur`/`docker`/`unknown`. Production path, real binary against a local stub release (debug hook, test trust), in `crates/cyber-cli/tests/upgrade_cli.rs`:
  - `script_upgrade_replaces_the_binary_after_verification`: the installed binary is byte-identical to the release's (the real binary with a marker appended, since the version is compiled in) and still runs, the helper is replaced, and `cyber.old` exists and is removed on the next start;
  - `interrupted_download_leaves_the_binary_unchanged` ("Atomic self-replace");
  - `forged_sha256sums_fails_signature_verification` ("Forged manifest");
  - `tampered_archive_fails_checksum_verification`;
  - `signature_for_another_version_is_refused`;
  - `nix_method_prints_the_manual_command` ("Unsupported method");
  - `already_current_skips` ("Already current");
  - `beta_channel_upgrade` (cli-commands "Upgrade on another channel");
  - `brew_method_runs_the_manager` (stub `brew` on PATH, `#[cfg(unix)]`).

## 3. `cyber uninstall`

- [x] 3.1 `uninstall` planning:
  - entries for the binary (script), marked PATH lines, config/data/state/cache, with sizes computed through `symlink_metadata`;
  - `--keep-config` and `--keep-data`;
  - `is_safe_root` refusing non-resolved paths, symlinks, home and its ancestors, filesystem and drive roots, and paths that are too shallow;
  - table tests for each refusal.
- [x] 3.2 Removal:
  - a no-follow walk;
  - an atomic rc rewrite, keeping the mode, that removes only the marker line and the line after it;
  - stopping the background server (`cyber_app::stop_service`) first;
  - deleting cyber-owned keyring entries from cyber's own records, unless `--keep-data`;
  - manager uninstall commands for package methods;
  - Windows self-rename with a notice.
- [x] 3.3 `cyber uninstall [--keep-config] [--keep-data] [--dry-run] [--force] [--format json]` (`commands/uninstall.rs`), with a confirmation prompt and a non-TTY without `--force` exiting 2. Production path, real binary with temp `HOME`/`XDG_*`/`CYBER_INSTALL_DIR`, in `crates/cyber-cli/tests/uninstall_cli.rs`:
  - `dry_run_lists_paths_with_sizes_and_deletes_nothing` ("Dry-run uninstall");
  - `keep_data_force_removes_everything_else` ("Keep data"), including the rc line;
  - `symlink_inside_a_cyber_dir_is_unlinked_not_followed` ("Symlink out of a cyber directory");
  - `home_as_config_dir_is_refused` ("Relocated directory refused");
  - `uninstall_stops_the_registered_server`: start `cyber serve --register` and check its PID is gone (stopped by PID in teardown if the test fails);
  - `non_tty_without_force_exits_2`.

## 4. Background updates, minimum version and CI

- [x] 4.1 `policy`: `should_check` (autoupdate, `CYBER_DISABLE_AUTOUPDATE`, `CYBER_OFFLINE`, `CI`, `dev`, managed config) and `decide` (bump kind; notify, silent upgrade, or fall back to notify), as table tests.
- [x] 4.2 The `cyber_app::update_check` task spawned by `run_server` (5 s, then 24 h; time controlled in tests) and the shared last-result state. `GET /api/v1/service/update` goes in `cyber-server` `http/service.rs`. `LiveEvent::InstallationUpdateAvailable` / `InstallationUpdated` publish `installation.update_available.1` / `installation.updated.1` as live-only events. Regenerate OpenAPI (`UPDATE_OPENAPI=1 … openapi_document_is_current`) and the TS SDK. Production path: `crates/cyber-app/tests/update_check.rs`, an AppHost with a stub release (debug hook) and the event stream:
  - `notify_by_default_publishes_update_available` ("Notify by default", event and endpoint);
  - `silent_patch_upgrade_on_script_install_publishes_updated`;
  - `ci_disables_the_check` ("Disabled in CI", zero stub requests).
- [x] 4.3 TUI toasts `cyber X.Y.Z is available — run "cyber upgrade"` and `cyber updated to X.Y.Z; restart cyber to use it`, read from the live event or from the endpoint on attach, plus the unsupported-version banner. Screen tests against a production App: `crates/cyber-tui` `screens_update::update_toast_during_a_run_keeps_the_drain` ("Update notice during a run") and `screens_update::toast_from_endpoint_after_attach` ("Attaching after the check").
- [x] 4.4 `min_supported` warning from the cached manifest only, in `exec` (stderr before the run) and the TUI banner. Production path: `upgrade_cli::exec_warns_when_below_min_supported` (real binary, cached manifest, scripted local provider, no stub server running) ("Unsupported version warning").
- [x] 4.5 `CI=true`:
  - `cyber` with no subcommand prints `interactive TUI unavailable in CI; use "cyber exec"` and exits 2;
  - the import offer is suppressed;
  - auto-update is off (4.1);
  - `exec` keeps `--format text`.
  - Production path: `crates/cyber-cli/tests/cli.rs::ci_true_refuses_the_tui` ("CI defaults") and `cli.rs::ci_true_exec_defaults_to_text` (scripted local provider).

## 5. Release packaging, install script, container and docs

- [x] 5.1 `.github/workflows/release.yml`:
  - `on: push: tags: ['v*']` only, with a tag/version check and channel derivation;
  - an 8-target build matrix with `CYBER_CHANNEL`/`CYBER_GIT_SHA`, smoke `--version` and packaging;
  - a `publish` job (the only one with `contents: write` / `id-token: write`) that writes `SHA256SUMS` and cosign `sign-blob` bundles, renders `<channel>.json` and `packaging/`, runs `gh release create` with `install.sh`, and uploads with `--clobber` to `channel-<channel>`;
  - actions pinned by SHA;
  - `actionlint` when installed, else a reviewed checklist.
  - `scripts/release_assets.py` (channel manifest, `SHA256SUMS`, template rendering) is tested by `scripts/tests/test_release_assets.py`, which the workflow and `ci.yml` run; it asserts the "Release assets" names for a fake 8-archive set.
  - A test asserts the workflow has no `pull_request`, branch `push` or `workflow_dispatch` trigger ("Not on pull requests").
- [x] 5.2 `scripts/install.sh` (POSIX `sh`):
  - detection, curl/wget, strict validation of `--version` and `--channel`, SHA-256 check, cosign verification when present;
  - atomic install, the already-installed check, and a marked, idempotent PATH line.
  - `sh -n`, plus `dash -n` and `shellcheck -s sh` when installed.
  - Production path: `crates/cyber-cli/tests/install_script.rs` (`#[cfg(unix)]`) runs the real script under `sh` against a local static HTTP stub serving fake archives through `CYBER_INSTALL_BASE_URL`:
    - `checksum_mismatch_exits_1_and_keeps_the_install` ("Checksum mismatch");
    - `already_installed_skips_download` ("Already installed", zero requests);
    - `offline_install_from_a_mirror` ("Offline install from a mirror");
    - `path_line_is_added_once_and_removed_by_uninstall` (with `cyber uninstall --force` from 3.3).
- [x] 5.3 `packaging/` templates: Homebrew formula, npm `@cyber-code/cli` launcher plus per-platform package manifests, Scoop, WinGet, AUR `PKGBUILD`, Nix expression. All are rendered by `release_assets.py` and never published. A launcher test runs the npm launcher (`node packaging/npm/cli/bin/cyber.js`) against a stub platform binary for argument, exit-code and signal forwarding ("npm launcher"), when `node` is available. The "Rendered manifests" scenario is covered in `test_release_assets.py`.
- [x] 5.4 `Dockerfile` and `.dockerignore` (bookworm-slim pinned by digest; `git`, `ripgrep`, `ca-certificates`, `bubblewrap`; uid 10001 `cyber`; `CYBER_HOME`; `CI=true`; `ENTRYPOINT ["cyber"]`; prebuilt binary from `dist/<arch>/`). `scripts/tests/test_dockerfile.py` asserts each required directive, and `hadolint` runs when installed. No image is built or pushed.
- [x] 5.5 Docs:
  - the README install section and feature table row;
  - `docs/features.md`;
  - `docs/local-core-guide.md` (install, upgrade, uninstall, release source, container and CI usage, and the debug-only test hook under Testing);
  - regenerated `docs/cli.md`;
  - the ROADMAP P0 section status for `installation-upgrade`, with the partial channels and deferred items named, and `python3 scripts/spec_inventory.py`.
- [x] 5.6 Verification:
  - `cargo fmt`, `cargo clippy --workspace --all-targets -- -D warnings`, and a hand review of every new `cfg(unix)`/`cfg(windows)` item;
  - `cargo nextest run --workspace --profile ci`;
  - `npx -y @fission-ai/openspec@1.13.2 validate implement-installation-upgrade --strict` and `python3 scripts/spec_lint.py`.

## 6. Review fixes

- [x] 6.1 No downgrade through the unsigned channel manifest: `Plan::to_latest` plans `Current` unless the manifest's version is newer by semver, and only `cyber upgrade <version>` downgrades. Tests: `upgrade::tests::latest_plans_only_newer_versions`, `upgrade_cli::older_manifest_version_changes_nothing`.
- [x] 6.2 The release workflow advances `channel-<channel>` (and GitHub's latest) only for a newer version (`release_assets.py newer`), with one concurrency group for every tag. Tests: `test_release_assets.ChannelOrder`, `test_release_workflow.test_an_older_tag_never_rolls_the_channel_back`.
- [x] 6.3 `CYBER_OFFLINE` stops the downloads and the package manager of `cyber upgrade`, not only the lookup. Test: `upgrade_cli::offline_upgrade_fails_before_any_download`.
- [x] 6.4 The background check resolves the binary once at start, installs at most one silent upgrade per process, and skips a check whose config fails to load. Tests: `update_check::second_patch_release_after_a_silent_upgrade_is_only_announced`, `update_check::unloadable_config_skips_the_check`.
- [x] 6.5 `cyber uninstall` lists the package manager's command before confirming and runs it only when the binary's path shows that manager installed it. Tests: `uninstall_cli::manager_uninstall_is_listed_before_it_runs`, `uninstall_cli::manager_known_from_listings_only_is_not_run`.
- [x] 6.6 A method known from a manager's listing alone is never acted on unattended: the background check only announces it, and `cyber upgrade` prints the manager's command (action `manual`) unless `--method` names it. Tests: `update_check::listing_only_method_is_announced_never_upgraded`, `upgrade_cli::listing_only_method_prints_the_manager_command`.
- [x] 6.7 The npm platform packages carry `bin/cyber[.exe]` and `bin/cyber-sandbox-exec[.exe]` from their target's archive, checked against `SHA256SUMS`. Tests: `test_release_assets.RenderedManifests.test_npm_packages`, `test_npm_packages_refuse_an_archive_that_does_not_match_the_sums`.
- [x] 6.8 `install.sh` writes the receipt `.cyber-receipt` beside the binaries, so a `--dir` install is a `script` install for `cyber upgrade` and `cyber uninstall`, which removes it. Tests: `method::tests::a_receipt_beside_the_binary_marks_a_script_install`, `install_script::dir_install_is_upgraded_and_uninstalled_as_a_script_install`.
- [x] 6.9 `ci.yml`'s `platforms` job builds all eight release targets, Windows included, on every pull request. Test: `test_release_workflow.test_ci_builds_every_release_target_on_pull_requests`.
- [x] 6.10 `CI=true` refuses every TUI entry point (`cyber`, `cyber attach`) through `commands::tui::refuse_in_ci`. Test: `cli::ci_true_refuses_attach`.
- [x] 6.11 `cyber upgrade --dry-run` (found in acceptance: the flag was missing) resolves the target and method, prints the action it would take, and fetches only the channel manifest. Test: `upgrade_cli::dry_run_names_the_plan_and_downloads_nothing`.

## 7. Acceptance (2026-10-24)

- [x] 7.1 Accepted by hand on the real binary against local stub releases (`python3 -m http.server` serving a fake release signed by a throwaway CA through `CYBER_TEST_RELEASE_TRUST`):
  - `cyber --version` and `cyber debug info` show version, channel `dev`, git SHA, target and the release source;
  - `cyber upgrade --dry-run` changes nothing; `cyber upgrade` replaces `cyber` and `cyber-sandbox-exec` atomically, keeping `.old` files that the next start removes;
  - a tampered archive fails with `checksum verification failed` and forged `SHA256SUMS` fail with `signature verification failed`. In both cases the exit code is 1 and the binary is unchanged. Without the test hook, the production trusted root refuses the test-signed bundle;
  - `cyber uninstall --dry-run` lists the paths with sizes. `--keep-config --force` removes only data, state and cache. A symlink out of the data directory is unlinked, not followed;
  - `install.sh` installs from the stub and adds the marked PATH line exactly once. A checksum mismatch exits 1 and leaves the install directory alone. A shell-metacharacter `--version` exits 2;
  - a `stable` build's server publishes `installation.update_available.1` and serves `GET /api/v1/service/update` with exactly one manifest request. `CYBER_OFFLINE` and `CI=true` make none;
  - a cached `min_supported` makes `cyber exec` warn on stderr and the TUI show a banner. The TUI shows the update toast;
  - with `CI=true`, `cyber` with no subcommand exits 2 and `cyber exec` prints text.
- Not accepted live: no tag has been pushed, so `release.yml` has not run on GitHub and no real cosign keyless bundle has been verified. Package-registry and container-image publishing remain partial. The native Windows suites (`upgrade_cli`, `uninstall_cli`, `cyber-install`) run only in CI.

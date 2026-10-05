# P0 exit evidence

P0 remains open. This change adds evidence and CI jobs; it does not start P1.

Reference machine: MacBook Pro **Mac14,6**, Apple **M2 Max** (12 cores: 8 performance + 4 efficiency), **32 GB RAM**, macOS **27.0** (26A428), local **APFS SSD**. Rust **1.98.0** (88d9e12ae). Storage source: `4cf19506`. The new startup samples include the readiness polling fix in `close-p0-exit-criteria`; its source checksum is recorded in the JSON. OS filesystem caches are retained. These are process cold starts, not cache-evicted or reboot measurements.

## Tool goldens

All 16 tools have reviewed, checked-in output fixtures in `crates/cyber-tools/tests/goldens`: read, write, edit, apply_patch, list, glob, grep, bash, webfetch, websearch, skill, todo, question, history_search, plan_enter and plan_exit. The tools execute through `BuiltinHost`; interactive tools use a real scripted runtime and user replies. File mutations also check resulting contents. Only absolute temporary checkout paths and generated IDs in interactive outputs are normalized.

`golden.rs` checks the union of the default/plan and edit/patch registries against the fixture names, with websearch configured. Adding a registered tool without a golden fails the suite. Question, history and plan goldens execute in `flow.rs`; the remaining fixtures execute in `golden.rs`. Permission behavior remains covered by the existing table-driven engine tests and integration scenarios.

```sh
cargo test -p cyber-tools --test golden --test flow
```

## Storage

The release probe in `crates/cyber-server/tests/storage_measurements.rs` follows the architecture decision's 1/8/32-session workload with concurrent full-history replay, default bounded writer queue (1,024 jobs; 5-second admission timeout), WAL and FULL synchronous durability. Each session produces 10 durable events/second for 20 seconds: one real `Runtime::admit` (about 1.1 KB of prompt constraints, actual inbox SQL projection) and nine synthetic tool-output appends (about 3.8 KB each). Synthetic appends use the actual event store and indexes but no tool SQL projection, on separate per-session load aggregates. Inference and tools are excluded; admission uses `resume=false` so it ends at durable inbox acceptance.

Append latency includes queueing, transaction, commit and response; it is not a separate measurement of time inside SQLite's commit. Readers replay both streams every 10 ms after the prior pass. Writers are aligned in bursts, making contention visible. Queue depth is sampled every 1 ms and may miss shorter spikes. Database/WAL sizes are measured at workload completion, before the database closes; WAL can recycle/checkpoint during the run, so its ending size is not cumulative bytes written. Backup duration measures an online database backup after workload completion; full artifact bundle recovery is covered separately by the existing tests. Persistence counts and backup integrity must pass, and any admission or append error fails the probe.

```sh
cargo test -p cyber-server --release --test storage_measurements -- --ignored --nocapture
# Optional duration override:
CYBER_PROBE_SECONDS=60 cargo test -p cyber-server --release --test storage_measurements -- --ignored --nocapture
```

Results: [raw storage measurements](p0-storage-m2-max.json). The provisional acceptance comparison is p95 admission below 100 ms at 8 sessions. These short probes are not a supported-capacity claim or a power-loss durability test.


| Sessions | Admission p50/p95/p99 (ms) | Append p50/p95/p99 (ms) | Max sampled queue | Database / WAL (MiB) | Backup (ms) |
|---:|---|---|---:|---|---:|
| 1 | 0.86/1.07/1.63 | 0.77/1.34/3.66 | 1 | 0.74 / 3.94 | 8.92 |
| 8 | 0.66/2.17/3.93 | 0.59/2.01/3.15 | 7 | 6.85 / 4.08 | 26.08 |
| 32 | 2.11/5.72/7.67 | 1.34/4.34/7.46 | 29 | 26.90 / 4.21 | 105.49 |

At 8 sessions, measured p95 admission is below the provisional 100 ms target. All 8,200 workload events persisted and all three backups passed integrity verification.

## TUI first frame

```sh
cargo build --locked --release -p cyber-cli
python3 scripts/measure_startup.py --runs 20 \
  --machine 'MacBook Pro Mac14,6; Apple M2 Max 12 cores; 32 GB RAM; macOS 27.0'
```

The probe launches the real binary on a 120×32 `xterm-256color` PTY. Timing starts before process creation and ends when the initial frame's status text and cursor-show command arrive. It responds to terminal capability queries, exits through Ctrl-D, and stops each new service afterward. Every sample uses new application directories and databases. `CYBER_OFFLINE=1` uses the bundled catalog, and a configured compatible model has no server; no inference is requested. This measures database creation/migration, catalog/config setup, session creation, transport and drawing, rather than a test-renderer draw in isolation. Offline mode excludes network catalog refresh delays.

Results: [raw startup samples](p0-startup-m2-max.json). Embedded and default fresh-service launch are reported separately. The new readiness polling meets the 150 ms target for both launch paths.


| Launch | p50 (ms) | p95 (ms) | p99 (ms) | Max (ms) |
|---|---:|---:|---:|---:|
| embedded | 81.09 | 90.45 | 547.00 | 547.00 |
| service-cold | 88.66 | 102.22 | 121.12 | 121.12 |

Both modes pass across all 20 samples on the reference machine. Before the fix, [cold-service startup](p0-startup-before-m2-max.json) had p95 168.52 ms and max 182.99 ms. A profile observed registration at 63–97 ms, followed by 58–71 ms until the first frame. Checking readiness immediately and every 10 ms replaces the 50 ms polling delay. Virtual-clock regression tests verify prompt readiness observation and enforce the five-second deadline even when a health request stalls.

## Platform matrix and CI

`.github/workflows/ci.yml` now builds and smoke-tests release executables for all six targets, uploading each CLI artifact:

| Target | Native runner |
|---|---|
| aarch64-apple-darwin | macos-15 |
| x86_64-apple-darwin | macos-15-intel |
| x86_64-unknown-linux-gnu | ubuntu-24.04 |
| aarch64-unknown-linux-gnu | ubuntu-24.04-arm |
| x86_64-unknown-linux-musl | ubuntu-24.04 + musl-gcc |
| aarch64-unknown-linux-musl | ubuntu-24.04-arm + musl-gcc |

Labels were checked against [GitHub's runner documentation](https://docs.github.com/en/actions/reference/runners/github-hosted-runners). Existing macOS/Linux workspace test jobs remain. The [latest pushed CI run](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/37253569094) passed for `4cf19506`, before these changes. The [expanded CI run for b99a74b](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/37373602496) passed all ten jobs, including all six platform builds and smoke tests. Linux artifacts include `cyber-sandbox-exec` beside `cyber`.

## Remaining gate

- Install a local server and run both published suites, including the 50-turn task, through the OpenAI-compatible adapter against a real local model. No model was installed or downloaded by this change.

OpenAI and Anthropic live baselines, recovery/trust tests, full backup/verify/restore, retention and logs remain implemented. Landlock fallback, PTY routes and the deferred TUI features do not block this exit gate.

## Validation

`cargo test --workspace`: 326 passed, no failures; the hardware workload probe is ignored by default and run separately. Workspace formatting and Clippy with warnings denied pass. `openspec validate --all --strict`: 55 passed. Spec lint has zero errors. The Python cognitive-complexity analyzer reports a maximum of 9 per function for the startup probe.

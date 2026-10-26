# Real-repository evaluation pilot

Status on 2026-10-26: runner implemented and offline controls passing; the 24 live
single-agent/solve observations are pending. This page contains no measured model-quality result.

The [source catalog](../../eval/repositories/real-repositories-v1.json) contains four historical
regressions from two public repositories. Sources are complete base snapshots, not small
synthetic replacements. Each task has three planned single-agent trials and three solve runs.

| Task | Language | Independent behavior |
|---|---|---|
| `repo-ripgrep-hidden-dot` | Rust | Dot-prefixed Unix names ending in dots remain hidden; overrides still work. |
| `repo-ripgrep-matches-all` | Rust | Every glob must match, including patterns sharing an optimized strategy. |
| `repo-click-short-help` | Python | Sentence boundaries and whole-word short-help truncation. |
| `repo-click-argument-help` | Python | Exhaustive positional help when any argument is documented. |

Full base and reference SHAs, archive/tree hashes, prompts, dependency/toolchain versions and
licenses are pinned in the catalog. Reference patches and independent upstream cases stay
outside candidate roots. The [evaluation guide](../../eval/README.md#real-repositories-and-verified-solve)
documents import, execution and grader controls.

## Offline acceptance

On macOS arm64, Rust/Cargo 1.99.1, Python 3.13.5 and pytest 8.3.4:

- All twelve corpus controls passed: untouched base FAIL, reference fix PASS and removed-test
  tampering FAIL for every task. Caller workspaces remained unchanged.
- The maintained grader checker passed all four prepared tasks, including its additional
  deleted-test-directory control.
- All four reference fixes passed through the actual CLI's independent, network-off grading
  path with a local scripted provider. These are setup controls, excluded from live outcomes.
- Seven bundled-workflow integration tests passed. The new cases exercise single-agent,
  selected solve and no-recommendation outcomes; retain nonzero usage on unsuccessful runs;
  deny both file-tool and shell reads of hidden inputs; deny grader access to fixture credentials;
  exclude ignored source from compiler inputs; and refuse ambient Git templates.
- Four manifest/source-identity tests, fourteen evaluation unit tests and three Python
  source-alias controls passed. CLI/core/tools Clippy passed with warnings denied.
- All forty original synthetic-suite grader controls passed with Python 3.13 and Node on PATH.
  Initial local invocations exposed an incomplete subprocess PATH; those were corrected before
  this passing run, without changing the original fixtures or solutions.
- Strict OpenSpec validation passed all 102 items.

Real network source import was exercised against all four pinned GitHub archives. Tests also
cover digest mismatch, unsafe/duplicate archive paths, hardlinks, alias parents, Git metadata,
bounded copying and authoritative recommendation checks. Native Linux execution and Windows
build checks remain for PR CI; native Windows grading explicitly fails closed pending issue #9.

The host initially lacked build space. Only old generated Rust object files were reclaimed,
retaining binaries, source, historical reports and native desktop acceptance artifacts. Free
space was approximately 15 GiB after the build/control preflight. No live Auth or desktop
acceptance is part of this benchmark.

## Frozen live protocol

Use `chatgpt/gpt-6.1-sol#medium` for single-agent, both solver candidates and the independent
read-only judge. Share the existing account and refresh locks. Run one root at a time, with two
concurrent solvers within solve. For each task in catalog order, trial 1 runs single then solve,
trial 2 solve then single, and trial 3 single then solve. All 24 root outcomes remain in their
denominators, including provider/setup failures and timeouts. Diagnostics never replace them.

Each root gets 1,500,000 accounted tokens and 1,800 seconds. Single-agent execution has 60
main steps; solve retains its native 20 steps per solver and bounded agent count. These are
different internal policies. Hidden verification has a separate 300-second capture limit.
End-to-end timing includes workspace setup and independent verification; native model/workflow
latency is also retained, with different internal boundaries identified.

Freeze the clean harness commit, executable/helper hashes, materialized manifest/grader hashes,
environment and order before the first provider call. Use isolated configuration and command
network isolation. Hide private reference archives, historical results and other attempts'
reports in addition to the runner's automatic current-input/report protections. Locked Rust
dependencies are prefetched by the operator; agents do not receive network setup privileges.

Preserve raw results privately. Publish reviewed records with private paths removed and no
credentials or full transcripts. Verify existing pilot artifacts remain byte-identical.
Report native token classes and attribution uncertainty; subscription dollar totals are
unpriced lower bounds, not zero-cost execution. Reserved budget enforcement remains priority 3.

These public historical tasks can be present in model training data. Four tasks and three
trials do not establish superiority, competitor parity, or the twenty-task-per-adapter release
gate. The [approved design](../../openspec/changes/add-real-repository-evaluations/design.md)
defines the remaining acceptance work.

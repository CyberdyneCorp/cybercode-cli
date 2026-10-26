# Real-repository evaluation pilot

The four-task retrospective pilot completed 24 original attempts and eight separately labelled
follow-ups on 2026-10-26. It demonstrated the runner, Rust repository tasks and independently
graded solve selection, and found setup defects. **It does not establish a clean coding-quality
comparison or a multi-agent advantage.**

| Original strategy | Native passes | Clean verified outcomes after review | Blocked verification | Environment integrity uncertain |
|---|---:|---:|---:|---:|
| Single agent | 12/12 | 11/12 | 0 | 1 |
| Solve, two candidates and independent judge | 6/12 | 5/12 | 6 | 1 |

The eight follow-ups all passed: one single-agent repeat, one Rust solve repeat and six Click
solve runs with corrected verification commands. They are additional observations, **not
replacements** for the original attempts. Original raw reports remain unchanged. Neither
blocked verification nor an uncertain environment proves the model's code incorrect.

[Original summary](real-repository-evaluation/summary.json),
[original outcome table](real-repository-evaluation/outcomes.csv),
[follow-up summary](real-repository-evaluation-diagnostics/summary.json), and
[follow-up outcomes](real-repository-evaluation-diagnostics/outcomes.csv) retain every observation.
The JSON records distinguish native outcomes from reviewed infrastructure/integrity
qualifications, source identities, tokens, elapsed time, host receipts and judge selection.

## Tasks and provenance

| Task | Language | Independent behavior |
|---|---|---|
| `repo-ripgrep-hidden-dot` | Rust | Dot-prefixed Unix names ending in dots remain hidden; overrides still work. |
| `repo-ripgrep-matches-all` | Rust | Every glob must match, including patterns sharing an optimized strategy. |
| `repo-click-short-help` | Python | Sentence boundaries and whole-word short-help truncation. |
| `repo-click-argument-help` | Python | Exhaustive positional help when any argument is documented. |

These are complete pinned base snapshots from ripgrep and Click. The
[source catalog](../../eval/repositories/real-repositories-v1.json) records repository URLs,
base/fix SHAs, archive/source hashes, licenses, prompts and verification commands. Reference
patches and independent upstream cases stay outside candidate roots. The
[evaluation guide](../../eval/README.md#real-repositories-and-verified-solve) explains import,
prerequisites and execution.

The original cohort uses clean harness commit `b11865a57408c06f2274b9ef1de575209ad1608a`;
follow-ups use `15bcc43bc4a1fe719adbb0ee38aa4a20220a9b44`. Both executable/helper hashes,
materialized manifest/grader hashes, predetermined order and limits were frozen before their
respective first calls. See the [original protocol](real-repository-evaluation/protocol.json),
[original provenance](real-repository-evaluation/provenance.json),
[follow-up protocol](real-repository-evaluation-diagnostics/protocol.json), and
[follow-up provenance](real-repository-evaluation-diagnostics/provenance.json).

## Setup findings and corrections

The frozen original Click checks used an inline Python import that command authorization
refused before execution. Six solve runs therefore never reached eligible host verification.
The original harness labelled them ordinary no-recommendation failures; the reviewed reports
identify the infrastructure cause without altering the raw reports. The runner now classifies
blocked/unknown checks without a verified survivor as infrastructure outcomes and preserves
inference usage and receipts.

The corrected catalog invokes `pytest --override-ini pythonpath=src` directly. It tests
candidate source without installed-package ambiguity or an inline interpreter command. Both
Click reference fixes passed scripted solve, host checks, judge and independent grading before
six additional live runs. No command-security policy was weakened. Earlier failed scripted
controls are setup diagnostics and contribute no live observations.

New reference-control folders briefly fell outside the original operator deny list while
runs 11 and 12 were active. They were moved under the existing protected setup root. Access
cannot be ruled out, so those observations have uncertain integrity and do not support clean
quality conclusions. Fresh isolated follow-ups passed after correcting the boundary; this does
not retroactively validate the original two observations.

The CI reference-concurrency investigation and evaluator environment correction are recorded
in [CI evidence](real-repository-evaluation/ci-reference-environment.json). Historical fixtures,
graders, reference solutions, raw pilot reports and native desktop acceptance binaries are
preserved.

## Offline verification

On macOS arm64 with Rust/Cargo 1.99.1, Python 3.13.5 and pytest 8.3.4:

- All twelve corpus controls passed: base FAIL, reference PASS and test-removal tampering FAIL
  for every task, with caller workspaces unchanged.
- The maintained checker passed all four real-repository tasks and all forty original suite
  tasks. Four reference fixes passed through actual CLI independent grading, and both Click
  reference fixes passed actual host solve authorization/check/judge/grading controls.
- Seven bundled-workflow integration tests pass, including five evaluation scenarios covering
  single, selected solve, no recommendation, grader infrastructure failure and blocked checks.
  They cover protected input reads through file tools and bash, grader credential masking,
  ignored-source exclusion, ambient Git templates and retained usage on unsuccessful outcomes.
- Fifteen evaluation unit tests, four core manifest/source-identity tests, six Python controls
  and two generated CLI-reference tests pass. CLI/core/tools Clippy and formatting pass;
  strict OpenSpec validation passes 102 items.

Initial local checker runs exposed incomplete Python/Node subprocess PATH settings; those
were corrected before the passing controls. Native links emit an `__eh_frame` size warning;
its origin was not classified. The recorded checks passed despite that warning.

Native CI covers Linux/macOS and Windows builds; Windows real-repository grading explicitly
fails closed pending command-isolation acceptance (issue #9). The real source/hidden-grader
controls above were performed on macOS; Linux CI's offline fixture tests do not substitute for
those four repository controls on Linux. No live Auth or desktop acceptance was performed.

## Limits and interpretation

All inference roles requested `chatgpt/gpt-6.1-sol#medium` through the same account/store and
refresh locks, with isolated configuration, command network off, protected inputs and fresh
base-only Git repositories. One root ran at a time, with two concurrent solvers inside solve.
The original order alternated strategies by trial as recorded in the protocol.

Each root had 1,500,000 accounted tokens and 1,800 seconds. Single-agent execution allowed 60
main steps; solve used its native 20 steps per solver and bounded agent count. These are
different internal policies. The bundled workflow also retains its soft dollar ceiling;
subscription pricing is unavailable, so numeric zero-cost fields are **unpriced lower bounds**,
not proof of free execution or a strict $5 guarantee. Reserved enforcement is separate work.

Independent verification has its own 300-second capture limit. End-to-end and verification
latencies are retained. Some early local validation builds overlapped original observations;
timings are descriptive and cannot establish a controlled speed comparison. Locked Rust caches
were prefetched before trials. Only owned old Rust object files and ignored generated build
products were reclaimed; approximately 15 GiB was free after preflight. Peak per-attempt memory
was not measured.

Public historical fixes can be present in training data. Four tasks and three repetitions do
not establish performance on unseen repositories, competitor parity or the twenty-task release
gate. Wilson intervals in the files describe binary execution outcomes; repeated tasks are not
independent repository samples. The original setup/integrity issues further prevent a clean
quality comparison. Full private transcripts and credentials are not published.

The [original verification](real-repository-evaluation/verification.json) and
[follow-up verification](real-repository-evaluation-diagnostics/verification.json) confirm all
25 protected historical artifacts remain byte-identical. The
[approved design](../../openspec/changes/add-real-repository-evaluations/design.md) defines the
remaining release gates and ordered automation work.

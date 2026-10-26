# GPT-6.1 Sol through ChatGPT: Cyber and Codex CLI pilot

Completed pilot on macOS, using the same requested `gpt-6.1-sol` model and medium reasoning effort in Cyber and Codex CLI. The scored dataset is 40 tasks per harness: one attempt on each of 20 standard and 20 hard tasks. No human steered a scored attempt, no failed attempt was replaced, and hidden-grader results were not fed back to either candidate.

## Results

| Suite | Cyber passes | Codex passes | Cyber grading / infra failures | Codex grading / infra failures |
|---|---:|---:|---:|---:|
| Standard | 19/20 | 20/20 | 0 / 1 | 0 / 0 |
| Hard | 17/20 | 17/20 | 2 / 1 | 1 / 2 |

All 20 attempts remain in each denominator, including infrastructure failures. A pass means the final workspace passed the existing hidden grader. The shared empty-table failure below is flagged for contract review, with no score adjustment. These are pilot observations, not proof of general Codex parity or superiority.

## Every non-passing paired outcome

| Suite | Task | Cyber | Codex |
|---|---|---|---|
| Standard | `build-inventory-cli` | Infrastructure failure | Pass |
| Hard | `build-minimake` | Pass | Infrastructure failure |
| Hard | `fix-shell-kv-store` | Grader failure | Pass |
| Hard | `fix-unicode-table` | Grader failure | Grader failure |
| Hard | `refactor-callbacks-async` | Infrastructure failure | Infrastructure failure |

- Cyber's `build-inventory-cli` and `refactor-callbacks-async` attempts stopped with `transport: error decoding response body`; neither reached grading. The inventory workspace had no edits. The async-refactor workspace retained partial edits. The underlying transport/provider cause is not established.
- Codex's async-refactor and minimake attempts exited with `Selected model is at capacity. Please try a different model.` After the first capacity error it continued successfully on subsequent tasks; the second ended its final minimake attempt. This does not establish a common cause for the two clients' errors.
- Cyber's shell key-value-store patch failed visible and hidden concurrent-increment/writer tests. Codex passed that task's independent grader.
- Both original candidates failed the table oracle on `render_table([], None, max_width=38, align=['center','right','center','center'], overflow='wrap')`: both raised `ValueError`, while the reference returned an empty string. The README specifies an empty-table result and per-column alignment validation without explicit precedence. Review that boundary before using this case as release-quality evidence. This concrete counterexample is not a Unicode-width defect. [Reproduction](gpt61-sol-chatgpt-pilot/empty-table-precedence-review.json).

## Unscored transport diagnostic

A fresh-fixture retry of Cyber's interrupted inventory task, using the same binary/model/effort/settings, passed its hidden grader in 253.3 seconds. The original score remains 19/20. One successful retry only shows that the failure did not recur on that attempt; it is not a software fix or a root-cause diagnosis. [Diagnostic report](gpt61-sol-chatgpt-pilot/cyber-standard-transport-diagnostic.json).

## Latency and usage

| Suite | Cyber native median | Codex native median |
|---|---:|---:|
| Standard | 53.5 s | 44.5 s |
| Hard | 132.4 s | 156.0 s |

Latency is exploratory: the existing logins report different ChatGPT account/workspace IDs (identifiers omitted), the host remained in interactive use, and the clocks differ. Cyber measures prompt-to-idle; Codex measures process-spawn-to-exit, including CLI startup. These medians cover all attempts, including failures, and do not measure intrinsic model speed.

Both use existing ChatGPT subscription logins. Dollar pricing is unavailable: Cyber's native zero-dollar values are unpriced lower bounds, not a claim of free usage. Preserve native input/output/reasoning/cache fields in the JSON reports; internal step counters and usage accounting are not equivalent across clients, and delegated-usage coverage was not independently established.

## Protocol and provenance

- Cyber source: `3cc90b5f2771525bb90d596ed35867f92fa27030`, freshly built CLI and sandbox helper. Codex CLI: `0.163.0`. [Cyber build hashes](gpt61-sol-chatgpt-pilot/cyber-build.json), [Codex build hashes](gpt61-sol-chatgpt-pilot/codex-build.json).
- Unmodified `suite-coding-v1` and `suite-coding-hard-v1` prompts, fixtures and graders. The standard suite historically saturated on a strong model; the hard suite is the more informative quality signal. [505 input-file hashes](gpt61-sol-chatgpt-pilot/eval-input-sha256.json).
- Independent disposable Git workspace per attempt; one concurrent task per harness, at most two root task slots. Smoke runs (Cyber two tasks, Codex one) and the separate diagnostic are excluded from scored results.
- The same manifest wall-time limits apply. Cyber retains the manifest's native model-step caps; Codex uses native execution behavior. No claim of identical internal caps or token budgets is made.
- Personal config, instructions, skills, hooks, plugins and memory were excluded from the measured invocations. Existing credential stores/refresh locks were reused rather than copied. Cyber uses its existing embedded/in-memory eval path; Codex uses `exec`. This does not measure Cyber TUI or registered-background-service latency. Normal server Sessions were not used as benchmark candidates. Raw transcripts, environment/auth files and private screenshots are not published.
- Task dependencies require no network. Cyber uses its default allowlist proxy; Codex command networking is disabled. That policy difference is retained as a limitation.
- Host: Apple M2, eight logical cores, 24 GiB RAM, macOS 26.7; Python 3.13.5 and Node 22.18.0. Initial shell Python 3.9 failed the suite prerequisite and was replaced before scored runs. All 40 broken-fixture/reference-solution grader controls passed under the selected runtime.
- [Full protocol](gpt61-sol-chatgpt-pilot/protocol.json), [adapter provenance and flags](gpt61-sol-chatgpt-pilot/adapter-provenance.json), [task outcomes CSV](gpt61-sol-chatgpt-pilot/task-outcomes.csv), [summary and original report hashes](gpt61-sol-chatgpt-pilot/summary.json). Published reports redact local directory prefixes only; numeric observations are unchanged.

## Interpretation and next work

This small Python/Node/shell corpus and one attempt per task do not establish Rust or large production-repository performance. Requested model alias and effort were pinned; backend weights were not independently pinned. Different accounts, native tool protocols, network policies, latency clocks and caps limit causal comparisons.

The pilot provides concrete work before a broad replacement claim: investigate response-decoding failures with safe diagnostics, strengthen independent concurrency verification, resolve the table-contract precedence, then repeat with matched account/settings and multiple trials plus fresh held-out repository tasks. The existing verified solve workflow should be evaluated separately rather than folded into these single-attempt scores. This hosted run does not close the local/offline-model acceptance gate.

## Verification

All 80 scored attempts and CSV rows were checked against the native reports; all 505 pinned inputs and both clients' binary hashes remained unchanged. Published numeric outcomes match the originals, JSON and local links validate, and the reports omit private credential/environment/transcript content. Strict OpenSpec validation passed all 101 items. [Verification record](gpt61-sol-chatgpt-pilot/verification.json).

## References

- [Suite and grader contract](../../eval/README.md)
- [Codex non-interactive mode](https://learn.chatgpt.com/docs/non-interactive-mode)
- [Codex configuration reference](https://learn.chatgpt.com/docs/config-file/config-reference)

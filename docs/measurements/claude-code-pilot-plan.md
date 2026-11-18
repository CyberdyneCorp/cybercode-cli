# Claude Fable 5.2: Cyber and Claude Code CLI pilot (plan)

Status: **prepared, not run.** No paid trial has been made. This page fixes the protocol before the first model call, following the [GPT-6.1 Sol Cyber/Codex pilot](gpt61-sol-chatgpt-pilot.md). Results will replace this status line and add the report files under `docs/measurements/fable52-claude-code-pilot/`.

## Question

On the same Anthropic model, at the same effort and on the same 40 hidden-graded tasks, how do Cyber and Claude Code CLI compare on passes, infrastructure failures, native usage, cost and latency? It is a single-trial pilot, not a parity or superiority claim.

## Protocol

| Item | Value |
|---|---|
| Model | `claude-fable-5-2`, Anthropic's most capable widely released model; Claude Code accepts the full ID with `--model`, and Cyber resolves `anthropic/claude-fable-5-2` from the models.dev catalog (not yet in the bundled snapshot, so the runner pins the fetched catalog per run) |
| Effort | `high` in both: Cyber `#high` (adaptive thinking, `output_config.effort`), Claude Code `--effort high` plus `CLAUDE_CODE_EFFORT_LEVEL=high` |
| Credentials | One Anthropic API key for both, from `ANTHROPIC_API_KEY` or the file named by `CYBER_EVAL_ENV_FILE`. Claude Code `-p` always uses an API key when one is present, so no subscription login is involved |
| Tasks | Unmodified `suite-coding-v1` (20) and `suite-coding-hard-v1` (20) prompts, fixtures and hidden graders; one attempt per task per harness |
| Order | Frozen before the first call (`scored-plan.json`): manifest order, one trial at a time, the harness that goes first alternating per task |
| Limits | The manifest wall time for both. Cyber keeps its native manifest turn and dollar budget; Claude Code gets the same dollar cap with `--max-budget-usd` and has no turn cap |
| Workspaces | A fresh fixture copy with a one-commit Git history per trial (the grader checker's `prepare_workspace`) |
| Steering | None. No retries replace failures; hidden-grader output never reaches a candidate |
| Run root | A private directory outside the repository; reports, transcripts and environment files stay there until redacted for publication |

### Configuration isolation

Neither harness reads the operator's personal configuration. The runner builds every candidate environment from scratch (no inherited variables except the locale) with a private `HOME` and `TMPDIR`.

- **Claude Code** 2.1.327 runs `claude --print --output-format stream-json --verbose` with a fresh `CLAUDE_CONFIG_DIR` holding only the generated settings, `--setting-sources user` (that isolated directory), `--strict-mcp-config` with no servers, `CLAUDE_CODE_DISABLE_CLAUDE_MDS=1`, auto memory off, and updates, telemetry, error reporting, surveys and marketplace auto-install off. Fixtures contain no `CLAUDE.md`, `.claude/` or `.mcp.json` (checked). `ANTHROPIC_MODEL`, the `ANTHROPIC_DEFAULT_{HAIKU,SONNET,OPUS}_MODEL` aliases and `CLAUDE_CODE_SUBAGENT_MODEL` (forced) all name the target model, so background work and subagents use it; every trial records the models Claude Code reports, and any other model is flagged.
- **Cyber** runs `cyber eval run --task ID --trials 1 --jobs 1` with private `CYBER_HOME` and `CYBER_CONFIG_DIR`, `CYBER_PURE=1` (no plugins), `CYBER_DISABLE_PROJECT_CONFIG=1`, memory off, `model` and `model_roles.{default,small}` pinned to the target.
- The default (non-`--bare`) Claude Code harness is measured, because `--bare` replaces the product's system prompt and tool set. Personal state is excluded by the clean configuration directory instead, matching how the Codex pilot excluded personal Codex config.

### Permissions and networking

| | Cyber | Claude Code |
|---|---|---|
| Approvals | Evaluation `bypass` mode; command isolation still enforced | `acceptEdits`, sandboxed Bash auto-allowed, `--permission-prompts none` and `denyInsteadOfPrompting` (anything that would ask is denied) |
| Command sandbox | OS sandbox, workspace-write | OS sandbox, workspace and temp writable, `failIfUnavailable`, no unsandboxed retry |
| Command network | `sandbox.network: "off"` | sandbox with an empty `allowedDomains` and `strictAllowlist` |
| Web tools | `webfetch`/`websearch` denied | `WebFetch`/`WebSearch` denied and disallowed |
| Hidden from commands | graders, solutions, earlier results, the other harness's state and every grading record (`sandbox.deny_read`) | same paths (`sandbox.filesystem.denyRead`); file tools blocked outside the working directory |

Only model API traffic leaves the machine. Unlike the Codex pilot, both harnesses have command networking off. Remaining differences: Claude Code's file tools are confined by permission rules rather than the OS sandbox, and its read denials are directory paths where Cyber's are globs.

### Recorded per trial

Pass/fail from the unchanged hidden grader, infrastructure failure, stop reason, exit code, native latency (Cyber prompt-to-idle; Claude Code process spawn-to-exit, plus its reported `duration_ms`), turns, the four token classes including cache reads and writes (Cyber also reports reasoning tokens; Claude Code includes thinking in output), native cost (Cyber from the pinned catalog prices; Claude Code `total_cost_usd`, a client estimate) and a cost computed from tokens at the same list prices for both. Budget stops and timeouts are graded; runs with no result are infrastructure failures and stay in the denominator.

## Runner

[`scripts/claude_code_pilot.py`](../../scripts/claude_code_pilot.py) (stdlib only) with [`eval/pilots/claude-code-pilot.json`](../../eval/pilots/claude-code-pilot.json).

```sh
# Build the measured binaries (release)
cargo build --release -p cyber-cli -p cyber-sandbox --bins

# No model call: inputs, 40 grader controls, binaries, pinned version and flags,
# isolated config, catalog/prices, sandbox availability, key presence
python3 scripts/claude_code_pilot.py --dry-run --grader-controls --root ~/cyber-eval/fable52
python3 scripts/claude_code_pilot.py --estimate

# Paid: one unscored smoke task per harness, then the scored run
CYBER_EVAL_ENV_FILE=~/.config/cyber-eval/anthropic.env \
  python3 scripts/claude_code_pilot.py --smoke --confirm-spend --root ~/cyber-eval/fable52
CYBER_EVAL_ENV_FILE=~/.config/cyber-eval/anthropic.env \
  python3 scripts/claude_code_pilot.py --run --confirm-spend --root ~/cyber-eval/fable52
python3 scripts/claude_code_pilot.py --verify --root ~/cyber-eval/fable52
```

Dry run on 2026-11-18 (macOS, Apple Silicon; Cyber `386a49c` release build, Claude Code 2.1.327, Python 3.11, Node 22): 35 checks passed, including 40/40 grader controls, fixture hashes, the 13 Claude Code flags, the pinned catalog price for `claude-fable-5-2` and Cyber's isolated sandbox probes (read, write and network denied). The only warning was the absent API key, which the dry run does not need.

Paid modes refuse to start without `--confirm-spend`, without a key, or when any dry-run check fails. The key file must be `chmod 600`. The key is never printed, is passed only to the two candidate processes (graders run without it) and is scrubbed from every file a trial writes. Scheduling stops once reported spend reaches `--max-total-usd` (default $150). A rerun resumes after the last recorded trial; the run also stops on an infrastructure failure in either harness's first two trials, so evidence is preserved before continuing.

## Cost estimate

No Fable 5.2 trial exists yet, so token counts come from the closest Cyber reports in `eval/results`, priced at Fable 5.2 list prices: $8 input, $40 output, $0.20 cache read and $10 cache write (5-minute) per million tokens.

| Per trial | Source | Tokens (input / output / cache read / cache write) | Cost |
|---|---|---|---|
| Hard | Sonnet 5.5 medium hard suite, 60 trials | 24 / 23,092 / 255,023 / 29,218 | $0.001 + $0.924 + $0.051 + $0.292 = **$1.27** |
| Standard | Hard × the standard/hard ratio of the GPT-6.1 Sol Cyber pilot (0.220 input+cache, 0.321 output) | 5 / 7,406 / 56,225 / 6,442 | **$0.37** |
| Standard, upper | Haiku 4.5 standard suite, 60 trials | 9,060 / 10,144 / 330,305 / 24,466 | **$0.79** |

Per harness: 20 × $1.27 + 20 × $0.37 = **$32.78** (upper $41.12). Both harnesses: **about $66, up to about $82**; the smoke pair adds about $1. Fable 5.2 runs longer turns than Sonnet 5.5, and Claude Code's larger system prompt and tool set add cached input, so treat $82 as likely and plan for more. The per-task budget caps bound one run at $68 per harness ($136 for both, plus at most the overshoot of a final model step), and the runner's $150 spend guard stops scheduling beyond that.

## Fairness limits to report with the results

- One trial per task; no statistical superiority claim.
- Native tool protocols, system prompts, context management and step limits differ by design. Cyber enforces the manifest turn cap; Claude Code has none. Budget caps are the same dollar amount, but each harness counts cost natively (Claude Code's cost is a client estimate; cache-write TTLs may differ).
- Budget caps at Fable 5.2 prices may stop long hard tasks; budget stops are graded, not retried.
- Requested model and effort are pinned; backend weights are not independently pinned. Fable 5.2 enforces preserved-thinking history checks for every account, so a harness that edits history can fail with API errors; these are infrastructure outcomes for that harness.
- Latency definitions differ (Cyber prompt-to-idle, Claude Code process lifetime including startup); the host may be in interactive use.
- The corpus is Python/Node/shell; Rust and large production repositories are not measured. Windows is excluded.
- Claude Code reads host-managed settings if any are installed; the dry-run reports their presence without reading them.

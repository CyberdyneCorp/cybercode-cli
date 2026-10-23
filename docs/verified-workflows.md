# Verified solve workflows

`solve` tries independent patches in managed worktrees, runs your verification commands in each checkout, and asks a separate read-only judge to compare the candidates whose checks passed. It leaves the owner checkout unchanged and never merges or publishes a patch.

Start from a clean, committed Git checkout:

```sh
cyber workflows run solve --wait --args '{
  "task": "Fix the parser regression and keep the public API compatible",
  "candidates": [
    {"label": "fast", "model": "fast"},
    {"label": "smart", "model": "smart"},
    {"label": "verify", "model": "verify"}
  ],
  "checks": [{"argv": ["cargo", "test", "--locked"], "timeout_seconds": 300}],
  "judge_model": "verify"
}'
```

The same arguments work with `cyber exec --embedded --workflow solve --args '…' --format json`, the Workflow Library, and `POST /workflows/runs` with `name: "solve"`. The TypeScript SDK uses the existing workflow operations. `GET /workflows/runs/{id}` includes separate server-derived `solve_evidence`; the terminal monitor displays its candidate outcomes, source digest and branch.

Omit `candidates` to use `fast`, `smart` and `verify`; omit `judge_model` to use `verify`. These tiers may all resolve to the same model. The report discloses actual models, the number of distinct solver models and whether the judge differs. Use explicit `provider/model` references to choose your own combination. Unavailable models and dirty, unborn or non-Git checkouts fail before any solver starts.

Defaults are five agents, a soft $5 inference ceiling and 1,800 seconds, with at most 20 steps per solver. Authenticated run-budget overrides follow the normal workflow rules; inherited permission, Mode and Session ceilings still apply. In-flight inference can overshoot a soft limit. Missing pricing and a zero recorded cost do not establish free execution; inspect the usage and pricing limitations. Verification commands may themselves consume external resources independently of model pricing.

Checks use literal argv without an implicit shell. Shell syntax requires an explicit shell in argv. Checks use the ordinary command permissions, hooks, sandbox, environment and native process ownership. Unattended runs need allow rules for edits and the required commands; denied checks are recorded without launch. A check that changes tracked source or non-ignored additions invalidates the candidate. Put generated products in ignored build directories. Source inspection fails closed on unsupported source types or exceeded inspection limits.

A solver's claim that tests passed is never evidence. Each check records its source and definition digests, argv, directory, timing, exit status, native settlement and up to 1 MiB of combined output, with truncation labelled. Outcomes distinguish `passed`, `failed`, `blocked`, `timed_out`, `cancelled`, `unknown` and `not_run`. Passing requires exit zero, acknowledged settlement and unchanged source. Passing checks are evidence, not proof of correctness.

Submitted solver Sessions and their descendants are fenced against further admission. Inspect their retained branches and worktrees directly; start another solve for further attempts. A restart reuses settled evidence for the same identities. An unresolved native intent remains unknown and is not launched again automatically. The API evidence remains available when a run has only partial results. Custom or overriding modules named `solve` cannot create host evidence by returning invented JSON. An ejected copy is a custom module and cannot use the embedded revision's verification bridge; customize models and checks through arguments to the bundled workflow.

The result contains a nullable `recommended_candidate`, alternatives and the judge's rationale. `verification_status` is `recommended`, `no_candidate_passed`, `judge_unavailable` or `incomplete`; it does not change the workflow lifecycle or CLI exit mapping. The judge may reject every candidate. Review the patch, host evidence and limitations before integrating a retained branch.

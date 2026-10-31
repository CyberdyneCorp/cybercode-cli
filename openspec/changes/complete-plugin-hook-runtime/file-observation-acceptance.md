# FileChanged acceptance audit

Task 5.2 is implemented and under final acceptance review. This audit does not
close native plugin enforcement, the remaining producers, or the full goal.

## Requirements and direct evidence

| Requirement | Production path and regression evidence | Current result |
| --- | --- | --- |
| Built-in and formatter effects | `tool_hooks.rs`/`owner.rs`; `builtin_write_and_edit_produce_observations_without_language_services`, `native_formatter_settlement_produces_a_file_observation` | Composed platform suite passes. |
| External, foreground and background edits; rename/delete | `native.rs`/`scan.rs`/`owner.rs`; `native_owner_delivers_external_edits_rename_removal_to_shared_sessions_and_joins`, `native_foreground_and_background_shell_writes_are_observed_after_the_turn` | Composed platform suite passes. |
| Bounded observation, queue loss, partial inventory and recovery | Native queue/registration component cases; real-runtime `location_limit_is_visible_without_disabling_other_location_observations`, `session_limit_is_visible_and_released_membership_admits_a_waiting_session`, `directory_registration_limit_is_visible_and_reconciliation_covers_its_tail`, `long_path_inventory_publishes_a_partial_baseline_and_keeps_existing_files_live` | Composed platform suite passes; incomplete coverage remains visible and is not treated as absence. |
| Private state, symlinks and hard-link scope | Pinned root/private identities, no-follow metadata scanner and link accounting; actual AppHost moved-private-root check, `hard_link_scope_allows_owned_aliases_and_suppresses_private_state_aliases`, both known-private retirement HTTP regressions | Composed platform suite passes. |
| Session scope, aliases and current project identity | Native leases, writer-validated scope records and admission-time physical input; alias-first sharing, root replacement, archive/resume and idle return regressions | Spelling aliases and both retarget/unused-old-ack cases pass on Linux, macOS and Windows at 01535d49. |
| Feedback, deduplication and unknown-effect recovery | Durable finite credit and owned user-ack validation; `hook_writes_stop_at_the_durable_feedback_limit_after_quiet_time`, `native_owner_feedback_renews_only_for_new_committed_user_input`, exhausted-credit/restart and unknown-handler Store regressions | Composed platform suite passes; quiet time or replay cannot manufacture renewal. |
| Cancellation and bounded resource lifetime | Per-membership cancellation, held-batch identity, joined scanner/observer shutdown and pending-owner reuse; all held-member HTTP cases and `readmission_reuses_one_pending_replacement_until_previous_owner_settles` | Composed platform suite passes; Linux missing-fix controls also pass their required failure assertions. |

These refer to the real suites in `cyber-tools/tests/file_observation_dispatch.rs`,
`cyber-server/tests/file_observations.rs`, their native component modules and
`cyber-app/src/host/tests.rs`, executed by the focused workflow. They are not
claims based solely on test names: the workflow results, actual checkout and
source composition are inspected separately. Native observations supply metadata
and scoped handler admission, never permission or authenticated writer identity.

## Platform evidence and outstanding check

The [verified job receipt](evidence/file-observation-3f7067dd.json) records actual
checkout checks, original log hashes, suite counts and platform exclusions.
Linux/macOS ran 24 HTTP and 17 Store/runtime cases; Windows ran 23 and 16.
The omitted cases are the two retarget tests extended by this increment.
Unix parent suites intentionally ignore subprocess entry helpers that their
SQLite-lock regressions invoke separately; Windows omits Unix-specific components.

[Run 41180219218](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/41180219218)
passes Linux, macOS and Windows at exact head
`3f7067dd058b91c11320c99c556b60451b103098`, after the main integrations. The
subsequent `62a3068e` correction changes only confinement probes, their workflow
and documentation; all Rust/Cargo inputs remain identical. The earlier d689d2f3
platform results are historical and do not by themselves prove this composition.

The audit found that `unused_user_acknowledgement_cannot_follow_a_retargeted_alias`
and `symlink_retarget_loses_old_delivery_and_user_input_admits_the_new_physical_scope`
were Unix-only. They now use native directory symlinks and correct link removal
on Windows as well. Their assertions remain unchanged: old-root delivery and an
unused old-root acknowledgement must lose authority, while fresh owned input
admits the new physical root and project identity. Creating/removing the alias
uses filesystem APIs, not a shell command or a recursive target deletion.

The [new platform run](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/41203255017)
passes all three platforms at `01535d49e7a454536fdf66e788567e7442eb01c7`.
The [verified receipt](evidence/file-observation-01535d49.json) binds each actual
checkout and original log hash, and explicitly confirms both named tests executed
and passed on every platform. Windows now runs 17 Store/runtime and 24 HTTP cases,
matching the corresponding Unix suites. This is actual Windows execution, not
cross-compilation or an optional skip. Local Windows execution was unavailable.

Current-source relevant PR checks and final merge review remain required before
task 5.2 is closed. Parent plugin/native support and other producer tasks stay open.

## Review and local checks

This increment changes two test adapters and documents acceptance evidence; it
changes no production permissions, receipt format or alias admission behavior.
Review checked that Windows removal deletes only the directory symlink, that the
old-root refusal and fresh-root positive assertions remain, and that neither a
link-creation error nor an absent Windows test is reported as acceptance. No
blocking code/security findings remain in the local diff.

The two named regressions pass on macOS arm64. Scoped Clippy with warnings
denied passes both actual test targets (`cyber-server --test file_observations`
and `cyber-tools --test file_observation_dispatch`); Rust formatting and whitespace
checks pass. Strict OpenSpec passes 114 items; spec lint reports zero errors and
25 warnings. The subsequent native platform run above verifies the revised tests
on Linux/macOS/Windows. It retains the existing intentional Unix subprocess-helper
parent ignores and platform-specific component counts explicitly in the receipt.

## Exact PR85 platform verification

The [current-source receipt](evidence/file-observation-ecce8b38.json) verifies all
three focused jobs on `ecce8b381c91e22f25b43f9a1cf6f8761c991cce`. Each actual
checkout uses merge `8de13b5445509a214d4f70f1082728a65b202df1`, whose tree
`119568e94fdef8498056b4448a17980c440f483f` equals the PR head. Both named
retarget regressions execute and pass on every platform, with 17 Store/runtime
and 24 real HTTP cases each. Component/helper counts and log hashes remain
explicit in the receipt. This confirms the earlier direct-branch acceptance on
the actual PR composition; four broader PR checks were still running when this
receipt was recorded. Task 5.2 remains open for their results and merge review.

## Composition after the independent Setup correction

PR #86 merged at `92a3240d`, correcting only the Setup design. The
[composition receipt](evidence/file-observation-post86-composition.json) binds the
prospective PR #85 tree `d8b104a7d4573f2ebcd33e2333d00dbf95d6a5d4` to that
base and the original tested head. Compared with tested tree `119568e94f`, only
`setup-design.md` differs; executable, test, build and workflow inputs are identical.
The actual combined tree passes local strict OpenSpec (114 items), spec lint
(0 errors, 25 warnings) and whitespace checks. Original CI is not claimed to have
tested the new full documentation tree. The Linux workspace job remains active;
task 5.2 stays open for its result and final merge review.
## Superseded workspace run and bounded attempts

The old [Linux workspace job](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/41207519002/job/124555564239)
was cancelled by the CI concurrency policy after PR #85 required a task-ledger
conflict resolution against merged main. Its log reports
`messages_cli::stats_separate_message_driven_usage` still running beyond 9,180
seconds; cancellation supplies no completed assertion result or proven cause.
The same named case passed in PR #87's Linux/macOS runs (4.464/1.850 seconds),
which also does not establish why the earlier attempt stalled.

The `ci` nextest profile now uses the existing `ci-windows` ten-minute
per-attempt slow timeout. The existing single retry is retained, so a retried
case can consume two attempts. Timeouts remain failures; this bounds future
stalls without claiming an application deadlock fix or FileChanged acceptance.
The largest reported passing-test times in those PR #87 runs were 99.147 seconds
on Linux and 75.484 on macOS. Current-revision CI and final review remain
required before task 5.2 closes.

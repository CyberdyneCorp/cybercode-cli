# Unacknowledged Store commits and CwdChanged

This fixes an observed durability-boundary gap while implementing task 5.3. It
does not close the full producer matrix or the plugin/native workstream.

## Failure and main comparison

SQLite's [WAL callback contract](https://www.sqlite.org/c3ref/wal_hook.html)
allows a callback error to propagate to COMMIT even though the transaction has
already committed. The test uses this documented boundary with `SQLITE_ERROR`;
it does not claim to reproduce a hardware failure or a historical CI stall.

At main `5d5826b72d41d6a833b523237bdbbd85ac51d31a`, the new Store regression
`writer::tests::committed_wal_error_stops_later_mutations` fails its named fatal
outcome assertion. A second connection first verifies the inserted row, proving
that the transaction committed. The caller receives a generic SQLite error and
the writer's fatal flag is not set. The comparison runs actual main with only
the test and test-only rusqlite hook feature overlaid; its production writer
remains unchanged. The [receipt](evidence/store-commit-main-5d5826b7.json) and
[original failure log](evidence/store-commit-main-5d5826b7.log) retain that check. The prototype branch and all saved work are restored afterward.

## Correction

The writer issues COMMIT through the existing transaction's connection and
inspects `is_autocommit()` before dropping that transaction. Calling the consuming
`Transaction::commit()` first would allow its Drop rollback to erase the
distinction between an ended transaction and one still eligible for rollback.

An error after the transaction ended has an uncertain outcome, so it becomes
`StorageUnavailableError` and follows the existing fatal-writer path. Queued and
later mutations cannot proceed. This does not claim that every ended transaction
committed: an error may also have caused automatic rollback. The caller lacks a
successful acknowledgement in either case and must recover rather than assume.

If a rejected COMMIT leaves the transaction active, the existing transaction
Drop rolls it back and the original SQLite error classification remains. The
deferred-foreign-key regression proves that this known nonfatal rejection rolls
back its row while leaving the writer able to commit later valid work. Queue
admission, registered append validation and before-commit fault behavior are
unchanged. No schema or new public error type is introduced.

## Runtime regression and verification

`cwd_changed_lost_commit_ack_is_silent_and_replay_does_not_dispatch` uses the real
runtime, file-backed Store, reducers and lifecycle callback with a simulated
Location lease owner. It enters a worktree successfully, then installs a one-shot
WAL error on that Store's writer thread before idle exit. It verifies the callback
was reached, the durable exit and its identity exist despite the caller error,
the writer is fenced, no exit CwdChanged is emitted, and no repeated exit is
committed. A subsequent shell request is refused without invoking the host shell
method. A fresh runtime loads the committed destination without invoking a hook
or changing that exit identity. Both runtime instances settle their leases.

The replay check uses the same failed Store; it is not a process-restart test.
Rusqlite's hook feature is enabled only in dev dependencies. Each fixture owns a
writer thread and a thread-local one-shot arm, so parallel tests do not share the
fault. Installing the callback performs no database writes and must not consume
the arm. Hook installation does not add a production fault-injection API.

Locally on macOS, all 49 Store tests (including the process-kill recovery test)
and all 13 Location activity tests pass after the fix. Scoped Clippy for all Store
targets and the actual Location test target passes with warnings denied. Formatting,
strict OpenSpec (114/0) and spec lint (0 errors/25 warnings) pass. Those results do not claim
the new uncertain-commit case was tested through a process restart, on Windows
or Linux, or across the full local workspace. Local code/security review found no blocking issue. Hosted acceptance and final
PR review remain required before merging this follow-up. Native support/design
review gates, DirectoryAdded/Setup and task 5.3 remain open.

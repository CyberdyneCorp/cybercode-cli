# Design

## Decisions

**Synchronous store, one writer thread.** The store is synchronous and owns one writer thread fed by a bounded crossbeam channel. Provider and tool work will be async in later milestones and never runs inside a transaction, so an async database layer adds nothing. Admission uses `send_timeout`, which gives `StorageBusyError` without acknowledging anything.

**Reads.** File databases get a separate read-only connection with `query_only=ON`. In-memory databases are private to the writer connection, so their reads go through the writer queue; shared-cache in-memory databases would trade busy waits for `SQLITE_LOCKED` errors.

**Fail-stop.** A disk-full, I/O, corruption or cannot-open error marks the writer failed. Queued and later mutations fail with `StorageUnavailableError`. The disk-full path is tested with `max_page_count`.

**Recovery fixtures distinguish faults.** The process-kill test runs a helper binary, kills it with SIGKILL and verifies every acknowledged sequence survives without gaps. It is labelled `process_kill`; power-loss simulation needs a fault-injecting VFS and is not claimed.

**Trust gating classifies, then digests.** Project layers are split into safe and sensitive parts. Sensitive parts are executable integrations, endpoints, Mode or permission widening and host-reading substitutions. The digest is SHA-256 over canonical JSON of the sensitive parts keyed by file path relative to the checkout, so comments and safe edits keep trust while any sensitive change revokes it. Approvals are keyed by canonical checkout root, so clones do not inherit them.

**Spec registry tests.** The CLI test parses the command tree from `cli-commands` and fails when code and spec diverge. The eval test validates every manifest, recomputes fixture tree hashes and checks that each recovery case names an existing requirement.

## Tradeoffs

The store's public `transaction` lets later milestones add projections without new APIs, at the cost of trusting callers to keep transactions short; that rule is documented on the method. `--config` without a short form costs Codex users two keystrokes; `-c` for `--continue` matches Claude Code and OpenCode.

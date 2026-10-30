# Trusted socket-worker capacity and recovery fixture

This extends the isolated [broker candidate](linux-socket-broker-candidate.md).
Its current internal pool limit is 16; this proof does not select a production
resource policy or claim to bound all guest processes, threads or kernel memory.

The fixed C target fills its private loopback listener queue. It waits for the
Rust driver's first acknowledgement after all three setup workers are observed
normally reaped, so residual setup work cannot consume a capacity slot. It then
starts 16 pthread connects and requires TCP_INFO to report SYN_SENT for each.
The sockets select ten SYN retries and remain open throughout the held phase.

A seventeenth blocking connect must return EAGAIN without another trusted worker.
The target interrupts all 16 callers with targeted non-restarting SIGUSR1 and
joins them, requiring 16 EINTR results. The driver retains each cancellation PID,
matches all 16 to normal waitpid receipts and confirms the expected 19 starts
(three setup plus 16 blocked requests) before sending a second acknowledgement.
Only then may the target close the held sockets and require a fresh private TCP
token exchange, establishing that capacity was recovered while it remains alive.

The independent C fixture expects 16; it never reads the helper's compiled limit.
The precise control changes only the copied helper's limit to 17. That control
remains bounded and must show all 16 genuine blocked connects plus 20 worker
starts (three setup plus 17 blocked requests), then fail the named backpressure
assertion. A setup failure, fewer real blocked operations or failure to create
the extra worker cannot count as that counterexample.

The shared audit driver now supports two fixed-byte acknowledgements. Its readers
retain their per-line and line-count bounds, and process/pipe cleanup occurs
before assertions, including the wrong-cap control's timeout. Existing scalar
cancellation, mutation and escaped-descendant cases still run in the same workflow
to check the shared driver changes. Audit markers remain temporary copied-helper
instrumentation for a fixed source-bound fixture, not an authenticated protocol
for arbitrary plugins. No ambient endpoint or credential is contacted.

No production helper/composer, ordinary tool policy or native activation changes.
Network-side effects are not rolled back by worker cancellation. Admission,
other concurrency/signal/resource cases, runtime/platform support and the full
native design gate remain open. Local checks cover Rust example Clippy, portable
C syntax excluding Linux-specific code, thirteen temporary builder transformations
with compiler interception, workflow syntax, formatting/diff and OpenSpec.

## Hosted result for ceecb479

[Run 41049265434](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/41049265434)
passed on Linux x64 and arm64, including the four capacity/backpressure/recovery
cases per architecture and the exact wrong-cap control. The shared cancellation,
mutation and escaped-descendant checks also passed in that run. These are isolated
candidate results, not production admission acceptance.

The original saturation reports and wrong-cap logs are retained under
`scripts/probes/plugin-native-entry/evidence/linux-socket-broker-capacity-ceecb479/{x64,arm64}`.
They identify tested merge `7c357b89e0f40b5df6110710c31cc41651770e6e` and source
manifest SHA-256 `2f4e8a8a8e42dc07fb60cc6505887c3f4c81988b5d8e14ffc7eee491eb634887`.
The manifest was independently recomputed from that exact Git tree. Each report
has 36 rows, including four saturation rows with 16 blocked requests, backpressure,
16 interrupted and normally reaped workers, and a successful subsequent TCP
exchange. Each control shows 16 blocked callers and 20 starts before the named
backpressure failure. These receipts are not assigned to later revisions.

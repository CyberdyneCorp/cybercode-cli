# Qwen3.5 9B long-task pilot

One isolated `build-inventory-cli` trial completed with a failure: 63 Turns and 63 tool calls reached the 2,400-second timeout; the hidden grader failed. This demonstrates that the compatible adapter can run a long local tool loop, but does not satisfy the P0 successful long-task gate.

The report identifies historical harness `b99a74b`. Its token/cost accounting excludes successful compaction and the separate reasoning class; preserve these historical totals unchanged. The first full suite uses corrected harness `1d8d1e4`; the hard suite will use a validated binary snapshot. Each report identifies its own revision. Zero-dollar model cost is explicitly unpriced.

# 16 MiB binary engine measurements

Measured on 2026-11-03 on an Apple M2 with 24 GiB RAM, macOS arm64. Five serial
trials per profile create fresh synthetic 16 MiB files and private namespaces.
The named reference is `local-apple-m2-24gib`. Engine Rust sources match commit
`026533d4`; the new benchmark's exact source hash, executable hash, Cargo.lock hash,
platform and compiler are included in each JSON report.

The benchmark streams fixture creation and byte IO. It stages a three-splice
batch (overwrite, insertion and deletion), checks undo/redo, reviews, saves and
restores through the actual native engine. Full proposed/restored SHA-256 and the
restored file length are verified in every trial. Timing excludes the additional
post-operation validation hashes. A literal search locates the unique final-byte
pattern using bounded continuation. Cancellation is requested on the third observed
poll; this measures cooperative core cancellation, not kernel IO preemption or
end-to-end Stop latency.

| Operation, median milliseconds | Unoptimized dev | Optimized release |
| --- | ---: | ---: |
| Open + first 64 KiB page | 2.306 | 0.542 |
| Prepare immutable base | 998.119 | 140.748 |
| Stage overwrite/insert/delete batch | 6.083 | 6.873 |
| Draft undo | 7.045 | 7.021 |
| Draft redo | 7.916 | 6.962 |
| Byte review | 969.838 | 101.121 |
| Prepare + native save + receipt | 4877.749 | 552.926 |
| Restore review | 1447.474 | 149.698 |
| Prepare + native restore + receipt | 4863.623 | 545.187 |
| Cooperative search cancellation | 0.081 | 0.064 |

- dev: median full search 2.744 ms; longest observed slice 2.302 ms; process peak RSS 8.30 MiB.
- release: median full search 3.115 ms; longest observed slice 7.037 ms; process peak RSS 7.45 MiB.

Peak RSS comes from macOS `/usr/bin/time -l` around the benchmark executable
itself, including fixture generation and all five trials. Compilation/Cargo is
excluded. RSS excludes OS filesystem cache and other system memory. No memory
bound for the server or TUI is inferred from this measurement.

These are core-only observations with uncontrolled background OS activity and
warm executable/filesystem cache effects. They are not a cold-cache claim, a p95
latency guarantee, UI/permission overhead, first terminal viewport measurement,
server peak RSS, or Linux/Windows acceptance. No provider or inference is used.
No pass/fail performance threshold is invented from this machine's results.

## Reproduction

Build the standalone harness from the repository root:

```sh
CARGO_INCREMENTAL=0 cargo build -p cyber-core --bench binary_editor --profile release --message-format=json > build.jsonl
```

Select the `compiler-artifact` record whose target name is `binary_editor` and use
its `executable` field. Run that exact executable directly (rather than timing
Cargo), for example on macOS:

```sh
/usr/bin/time -l /path/from/build.jsonl > result.json 2> time.txt
```

Repeat with `--profile dev` and `CARGO_PROFILE_DEV_DEBUG=line-tables-only` for the
unoptimized comparison. Linux can use `/usr/bin/time -v`; its maximum RSS field is
in KiB rather than the macOS byte field. Windows native replacement is unsupported,
so the harness explicitly refuses to run there.

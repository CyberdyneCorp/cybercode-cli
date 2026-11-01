# Local binary viewer evidence

The terminal frame and measurement describe an isolated real `cyber --pure hex`
process on macOS arm64, with private temporary application state and a loopback
scripted provider. The provider received zero inference requests. A 36×140 PTY
rendered the tail of a sparse file larger than physical RAM; goto used an exact
odd offset above 2^53. The frame was decoded from terminal output using the existing
`vt100` parser. The fixture contained synthetic bytes only.

The JSON names the actual executable by SHA-256. It was rebuilt with Rust sources
matching commit `39cdcf7f`, including the final policy-wait, disposal and page
validation refinements. Additional TypeScript test source does not change the Rust
executable. First-view time includes fresh application/TUI/
server startup with an unoptimized line-tables build and warm OS executable cache.
RSS is one sample at first view, not peak memory. The temporary exploratory driver
is not a portable benchmark harness. Editing, save/recovery, input/cancellation
latency, the 16 MiB editing benchmark, and Linux/Windows native evidence remain open.

- [x] Add per-tool output goldens and catalog coverage guard.
- [x] Measure durable admission and writer contention under the storage decision workload.
- [x] Measure process launch to the real TUI first frame on named hardware.
- [x] Configure all six platform build targets and record pending CI verification.
- [x] Reconcile roadmap and publish evidence.
- [x] Run formatting, lint, relevant tests and specification validation.

The local-model coding and long-task baseline remains a P0 gate; this change supplies evidence and build jobs without declaring P0 complete.

- [x] Profile cold-service readiness, reduce polling delay, add regression tests and re-measure.
- [x] Push and verify all six platform builds.
- [ ] Run and publish the local compatible-model baselines.
- [x] Verify accounting regressions against main and include reasoning/compaction usage and cost.
- [x] Validate and rebuild the corrected release harness before full local runs. Release build at 5adc91e (snapshot sha256 e5a3d33e…5fdd) after fixing the suite summary, which dropped `tokens.reasoning`. A one-trial hard smoke (`fix-job-queue`) completed 53 Turns with a graded FAIL and no infrastructure failure. The earlier 60 hard-suite transport failures at 4a32b58 trace to a Metal command-buffer failure in Ollama's llama-server (2026-10-06 05:39 local), after which every request returned an empty stream until the runner was killed; see `eval/local-model/qwen3.5-9b/README.md`.

## 1. Review

- [x] 1.1 Read the macOS contract and code seams, and the Linux (#130) and Windows (#131) drafts; list the shared decisions.
- [x] 1.2 Draft proposal, design (F1–F12), tasks and contract deltas for `computer-use`, `cli-commands` and `sandbox`.
- [ ] 1.3 Owner reviews this change and answers the owner-review decisions (F1, F4, F6b, F7, F8a–c, F9, F12).
- [ ] 1.4 Rebase #130 and #131 onto this change: drop their duplicated shared requirements and reference the registry, selector, lifecycle, chord, worker and receipt requirements here.

## 2. Types and schema (after approval)

- [ ] 2.1 Add `schema_version`, `mode`, `capture`, `input`, `authority`, `session`, `os`, `selectors`, `evidence` and `diagnostics` to `Health`, keeping legacy fields and their macOS values.
- [ ] 2.2 Add the `ReasonCode` registry and make `Error::UnsupportedPlatform` text platform-neutral with a code; keep error class names.
- [ ] 2.3 Add `window` and `pick` selector variants with per-platform acceptance, `target_id`, `source` and `topology_epoch` on `NativeTarget`; mark legacy numeric fields deprecated in OpenAPI.
- [ ] 2.4 Add the receipt fields from design §11.1 and store `schema_version` in the journal; prove version 1 rows still replay.
- [ ] 2.5 Regenerate OpenAPI and the TypeScript SDK; snapshot diff shows additive changes only.

## 3. Runtime and policy

- [ ] 3.1 Switch admission and revocation-on-loss to `capture.available` / `input.available`.
- [ ] 3.2 Add `review`, `end_scope` and `set_revocation_sink` default methods to `Backend`; wire the sink to `Computer::stop`.
- [ ] 3.3 Implement review-bound arming for non-re-resolvable selectors and `allow --preview <id> --yes`.
- [ ] 3.4 Implement the topology epoch and frame staleness, and the binding-loss path (fence first, then `end_scope`).
- [ ] 3.5 Implement the shared chord evaluator before admission; keep the native macOS check; add the macOS parity table test.
- [ ] 3.6 Refuse unsupported actions and chords before journal intent with a reason code and zero native calls.
- [ ] 3.7 Add the compiled evidence table and the input gate for platforms added after macOS.

## 4. Surfaces and docs

- [ ] 4.1 CLI `status` prints mode, reasons and evidence; `allow --preview <id> --yes`; refusal of `--yes` with non-re-resolvable selectors.
- [ ] 4.2 TUI and dashboard show mode, reasons, evidence level and receipt reasons/cleanup state.
- [ ] 4.3 `docs/computer-use.md`: reason-code registry, chord table, evidence table, schema-version policy.
- [ ] 4.4 Sandbox: test that sandboxed children cannot read private captures and receive no native capability on each supported OS.

## 5. Verification

- [ ] 5.1 Unit and runtime tests listed in design §15 pass on macOS, Linux and Windows CI.
- [ ] 5.2 Existing macOS computer-use tests pass unchanged except the documented F8c chord timing.
- [ ] 5.3 `openspec validate --all --strict`, `scripts/spec_lint.py` and `scripts/spec_inventory.py --check` pass.

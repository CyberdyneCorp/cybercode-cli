## P1 migration and editor delivery

- [ ] Implement pure permission and mode conversion with faithful precedence and bounded errors.
  - [x] Add the initial pure Claude group ordering and supported action/mode converter, bounded errors and shared engine/tool decision tests. Full source mapping remains open.
  - [x] Preserve distinct Claude edit-tool selectors through exact native tool scopes, current/ancestor mode ceilings, deny ceilings and catalog filtering; test actual write/edit/notebook operations. Remaining source selectors and complete migration acceptance stay open.
  - [x] Add explicit OpenCode ordered/map/global permission and legacy tools conversion, canonical aliases, bounded refusals and literal command-pattern decision tests. Full source/default/home/report acceptance remains open.
  - [x] Convert parsed constant Codex prefix rules with alternative/size limits, strongest-match order, native argv metadata and actual quoted/compound refusal tests. Source Starlark/examples/metadata and full policy equivalence remain open.
  - [x] Parse bounded constant Codex prefix_rule source syntax, retain metadata, validate inline examples and verify actual-host denial. Arbitrary dynamic Starlark/reporting and full source acceptance remain open.
  - [ ] Implement unsupported-source reporting, Codex policies/profiles/sandbox mapping and finish remaining source permission/default/home mappings.
- [ ] Discover every canonical Claude/Codex/OpenCode project/global source, including auto precedence and detection summaries.
- [ ] Convert providers, credentials/env, agents, commands, skills, rules, instructions, hooks, MCP and remaining canonical configuration fields.
- [ ] Implement dry-run unified diffs, explicit write confirmation, secret safety, source-linked reports and idempotent annotations.
- [ ] Implement canonical ACP lifecycle, buffers/write-through, editor MCP, permissions and streaming.
- [ ] Implement the P1 VS Code extension and editor acceptance coverage.
- [ ] Audit every P1 compat-import/editor-integration requirement against implementation and native evidence.

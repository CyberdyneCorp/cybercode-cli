## P1 migration and editor delivery

- [ ] Implement pure permission and mode conversion with faithful precedence and bounded errors.
  - [x] Add the initial pure Claude group ordering and supported action/mode converter, bounded errors and shared engine/tool decision tests. Full source mapping remains open.
  - [ ] Preserve distinct Claude edit-tool permission granularity and complete remaining selector conversion.
  - [ ] Convert Codex policies/profiles/execpolicy and OpenCode ordered/legacy permissions.
- [ ] Discover every canonical Claude/Codex/OpenCode project/global source, including auto precedence and detection summaries.
- [ ] Convert providers, credentials/env, agents, commands, skills, rules, instructions, hooks, MCP and remaining canonical configuration fields.
- [ ] Implement dry-run unified diffs, explicit write confirmation, secret safety, source-linked reports and idempotent annotations.
- [ ] Implement canonical ACP lifecycle, buffers/write-through, editor MCP, permissions and streaming.
- [ ] Implement the P1 VS Code extension and editor acceptance coverage.
- [ ] Audit every P1 compat-import/editor-integration requirement against implementation and native evidence.

# Documentation

Start with the [project README](../README.md) for setup, architecture and the sourced comparison with Claude Code, Codex and OpenCode.

| If you want to… | Read |
|---|---|
| Install, configure or use the CLI/TUI | [Local core guide](local-core-guide.md) |
| Look up every command and flag | [Command-line reference](cli.md) (generated) |
| Check implemented features and limitations | [Feature reference](features.md) |
| See the terminal UI and commands | [Screenshots](screenshots/README.md) |
| Understand phase and milestone scope | [Roadmap](../ROADMAP.md) |
| Check current P1 evidence | [Implementation status](implementation/p1-status.md) |
| Check current canonical counts and acceptance coverage | [Spec/documentation reconciliation](implementation/spec-doc-reconciliation.md) |
| Read the historical P1 acceptance audit | [Acceptance audit](implementation/p1-acceptance-audit.md) |
| Check P0 measurements and exit gates | [P0 evidence](measurements/p0-exit-evidence.md) |
| Use the API from another program | [TypeScript SDK](../sdk/typescript/README.md) and [OpenAPI](../sdk/openapi.json) |
| Run benchmarks or local-model evaluations | [Evaluation guide](../eval/README.md) |
| Understand storage choices | [Storage architecture decision](decisions/0001-storage-architecture.md) |
| Inspect product contracts | [OpenSpec specifications](../openspec/specs) |
| Compare independent patches with tests and a judge | [Verified solve workflows](verified-workflows.md) |

Specifications include future work. A specification's presence does not mean the feature is implemented or accepted; use the feature reference and acceptance audit together.

## Proposed next delivery slices

These OpenSpec plans describe work that has not shipped:

- [Local web dashboard and browser approvals](../openspec/changes/implement-local-web-dashboard/proposal.md): P1 private access, live request handling and reconnect; account/Relay access stays P3.
- [Project collaboration hub](../openspec/changes/add-project-collaboration-hub/proposal.md): proposed P3 collaboration for people and their locally running agents around a shared Git project, with a self-hosted hub, explicit membership, conversations and shared tasks.

Review each proposal, design, scenarios and tasks before runtime implementation. The [roadmap](../ROADMAP.md) records the phases and delivery order.

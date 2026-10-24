# complete-dashboard-and-automation

Complete collaboration acceptance, browser collaboration/settings/media/terminal and durable automation controls.

Status: planning artifacts ready for user review; implementation has not started.

- [Proposal and scope](proposal.md)
- [Design and acceptance boundaries](design.md)
- [Implementation and verification tasks](tasks.md)
- [Browser contracts](specs/web-client/spec.md)
- [Reserved budget lifecycle](specs/observability-costs/spec.md)
- [Transcript retention safeguards](specs/storage-events/spec.md)
- [Durable scheduling contracts](specs/loops-scheduling/spec.md)

Validation on 2026-10-24: strict OpenSpec validation passed all 90 items; canonical spec lint reported 0 errors and 23 warnings; whitespace checks passed. Main CI at base `78c60566` is green ([run](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/40104736761)). No new implementation tests or live authenticated/device acceptance have run.

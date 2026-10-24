# complete-dashboard-and-automation

Complete collaboration acceptance, browser collaboration/settings/media/terminal and durable automation controls.

Status: the user requested continuation on 2026-10-24 after artifact review was requested in PR #32. Local implementation may proceed; authenticated live acceptance remains pending while CyberdyneAuth is unavailable. Integration queue and computer-use additions are separate proposals requiring review.

- [Proposal and scope](proposal.md)
- [Design and acceptance boundaries](design.md)
- [Implementation and verification tasks](tasks.md)
- [Live acceptance prerequisites and evidence matrix](acceptance.md)
- [Browser contracts](specs/web-client/spec.md)
- [Reserved budget lifecycle](specs/observability-costs/spec.md)
- [Transcript retention safeguards](specs/storage-events/spec.md)
- [Durable scheduling contracts](specs/loops-scheduling/spec.md)

Validation on 2026-10-24: strict OpenSpec validation passed all 90 items; canonical spec lint reported 0 errors and 23 warnings; whitespace checks passed. Main CI at base `78c60566` is green ([run](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/40104736761)). No new implementation tests or live authenticated/device acceptance have run.

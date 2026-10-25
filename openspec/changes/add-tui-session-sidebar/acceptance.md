# Sidebar acceptance evidence

Reviewed on 2026-10-25 for PR #45. The user approved proposal/design/tasks before implementation.

## Implementation and review

TUI implementation commit: `8defd812e49ea4db1181ab2215e6098b0dff06c6`. The overview shares the existing directory cache and authorized sources, separates discovery from mutations, fences stale contexts, and preserves native Stop priority. Independent source review approved the final implementation after fixes for hub-account draft invalidation and short-viewport overflow/navigation.

## Local verification

- Full TUI suite: **553 passed, three configured skips**. Two Linux-only synthetic protected-file hub fixtures avoid writing the native macOS credential vault; one ignored subprocess fixture is exercised by its passing parent test.
- Focused final review regressions: **61 passed**, including account switch, captured `expected_actor`, fair preview and remote provenance/directory footer at 100×30 and 120×34. Final source-availability classification regression: **one passed**.
- Strict all-target TUI Clippy with `-D warnings`: passed. Rust formatting and whitespace checks: passed.
- OpenSpec 1.13.2 strict validation: **99 passed, zero failed**. Requirement inventory remains **51 capabilities / 906 living requirements**; this active delta is not archived yet. Spec lint reports zero errors and 23 advisory warnings.

The successful macOS build emitted a linker `__eh_frame` warning; it did not fail the build. No comparison-based claim about its origin is made.

## Captures and CI

The CLI and sandbox helper were freshly built from implementation commit `8defd812` into a separate private fixture directory. CLI SHA-256: `3711f713247f32707933a76eaaa42404c51ab0caa0e117287e0f67778ae67bf4`. The existing OS-consented native acceptance binary was not replaced.

The real dark 160×48 hero and 100×30 drawer were independently inspected: all three groups, actual local/child/peer names, state, `peer:studio` provenance and the directory footer are visible. The narrow view displays `+1 more` for the omitted local entry. The capture harness also passed 11 local safety/fixture regressions, including child-process cleanup, PTY teardown, no-proxy/no-redirect HTTP and usage-ledger schema compatibility. Independent review covered those changes. The full gallery pipeline regenerated the dark/drawer/light PNGs, other affected screens, workflow monitor and real-frame animation. The light capture was independently inspected as well; its child state is correctly idle after the held Turn was released. The workflow fixture now waits for its three actual active-run gates rather than an impossible four-request count. The complete capture process exited successfully; all 12 PNGs decoded and the animation contains 24 real frames. Post-capture inspection found no remaining owned Cyber/provider/MCP processes. Final commit CI is tracked in [PR #45 checks](https://github.com/CyberdyneCorp/cybercode-cli/pull/45/checks); its live results are the authoritative status. The branch includes main `df4bfe98`, with no unresolved merge conflict. Live CyberdyneAuth, real organization membership and physical remote-device acceptance are outside this sidebar change and were not claimed as verified.

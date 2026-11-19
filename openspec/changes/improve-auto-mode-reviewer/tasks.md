## 1. Shared framing

- [x] 1.1 Move the `<untrusted_content>` frame primitive to `cyber-core::untrusted::frame` (`scanned_frame`, `frame_with`, `attribute`); `cyber-tools` delegates with unchanged output. Tests: the existing `cyber-tools` frame tests, `cyber-core` `untrusted::frame` tests.

## 2. Reviewer context (runtime)

- [x] 2.1 `runtime::auto_context`: `ReviewWorkspace`/`ReviewFiles`, tool specs, `ReviewTools` runner with call, byte and time budgets, transcript search, framing, `ReviewerToolUse` records. Unit tests: framing with signals, refused/unknown/invalid calls fail closed, byte and call budgets, a slow read times out, confirmation matching.
- [x] 2.2 `auto.rs`: offer tools, bounded tool loop (reasoning, text and calls replayed per round), fail closed on any tool failure or incomplete round; record `reviewer_tools` and `confirmation`; trusted system lines for tools and confirmations.
- [x] 2.3 Confirmation pair in the evidence for a bare user confirmation.
- [x] 2.4 Statistics: migration `20261118020000_auto_reviewer_tools`, `reviewer_tool_calls` projection and reset, `cyber permissions auto show` row.
- [x] 2.5 Integration tests (`crates/cyber-server/tests/auto.rs`): `reviewer_file_reads_are_framed_as_data_and_recorded`, `reviewer_following_an_injection_fails_closed` (reads `.env`, calls `bash`, invalid arguments, exceeds the call budget), `transcript_search_frames_injected_history`, `bare_confirmation_is_paired_with_the_assistant_question`.

## 3. Tool host

- [x] 3.1 `cyber-tools::auto_review`: read authority from the Session's policy, Location, `deny_read` and secret names; `ReviewWorkspace` read and grep. Unit tests: `.env`, `deny_read`, rule-denied, outside and linked paths refused; grep skips them.
- [x] 3.2 `Request.sandbox_confined` set only by the `bash` tool (not excluded, not escalated, enforced sandbox, network not `on`, workdir is the Location); the confined-command rule in the auto stage. Unit tests: confined and non-confined command shapes.
- [x] 3.3 Integration tests (`crates/cyber-tools/tests/auto_dispatch.rs`): `only_sandbox_confined_reads_skip_the_reviewer`, `escalated_reads_still_reach_the_reviewer`, `production_reviewer_reads_obey_session_read_rules`.

## 4. Docs

- [x] 4.1 Sync the deltas into `openspec/specs/permissions-modes` and `openspec/specs/untrusted-content`.
- [x] 4.2 `docs/local-core-guide.md` (Auto-mode controls), `docs/features.md`, `CHANGELOG.md`. No CLI help changes, so `docs/cli.md` is unchanged.

## 5. Coordinator defaults

- [x] 5.1 Missing file returned as data; refusal decided before existence. Tests: `auto_review` unit test, `reviewer_missing_file_is_data_but_refusal_hides_existence`.
- [x] 5.2 `ResolvedModel.tools` from the catalog; tool-less review when false; one tool-less retry when the provider rejects the request. Tests: `evaluator_without_tool_support_reviews_without_tools`, `provider_rejecting_tools_falls_back_to_a_tool_less_review`.

## 6. Not covered

- [ ] 6.1 Configuration for reviewer budgets or for turning the tools off.
- [ ] 6.2 Localized bare confirmations.

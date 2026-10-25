# Dashboard message inbox (2026-10-24)

The browser's Message inbox exposes immutable held incoming agent messages through the existing owner-authenticated inbox/messaging APIs. It shows sender, receiving Session, message identity, full admitted text and available hub/project/member/agent provenance. Release requires explicit content review and uses queue delivery; Drop never admits the message. Neither action grants tool permissions. Ordinary held prompts remain outside this agent-message view.

Decisions are bound to the displayed content/provenance and keep their original operation key through uncertain retries. Refresh reconciles another client's decision. Missing provenance, stale/failed reads, offline state and Session/Location/logout changes prevent stale actions. Older provenance is paginated without discarding already reviewed held messages. Optional inbox reads do not block ordinary Session attachment or permission controls.

The server already makes received message content immutable and reauthorizes collaboration release; the browser reuses those paths. No new authentication protocol, hub permission or owner credential is introduced. Incoming HTML and remote image links are inert text.

## Verification

- Dashboard typecheck, browser-test typecheck and reproducible embedded bundle check passed.
- All **30 dashboard unit tests** passed. Added cases cover exact-content review, sender/membership-generation changes, offline/missing provenance, authorization refusal, lost acknowledgements, completed pagination, late responses after context changes and slow optional reads.
- All **23 real-server browser tests** passed against the rebuilt debug CLI and isolated scripted provider. Three new scenarios cover zero inference before explicit consent and inert hostile content, exact original retry identity, and another client's drop without inference. Existing permission/question races, reconnect, real 45-second lease expiry, TLS, SSH-style forwarding and workflow/Job/Goal controls also passed.
- The 360px inbox has no horizontal page overflow; its [screenshot](../screenshots/web-dashboard-message-inbox-mobile.png) contains only disposable fixture data and was visually inspected.
- The browser suite exposed duplicate display of a completed answer as persisted and streaming content. Live deltas are now bound to message IDs and ignored for completed messages; a regression covers late old deltas and subsequent message boundaries.
- Strict OpenSpec validation passed **91 items** on the implementation branch. Canonical lint reported **0 errors and 23 warnings**. Whitespace and relevant local documentation links passed.
- The debug CLI build passed with an observed macOS linker warning that `__eh_frame` exceeds compact-unwind encoding size. The full Rust workspace suite was not rerun for these browser-only changes; actual CLI/server behavior is covered by the browser suite.

Code review used the repository's existing model/view/transport boundaries and checked authentication, provenance, consent, uncertainty, inert rendering and test coverage. Pagination reset and optional-read blocking were corrected with regressions; no unresolved blocking findings remain in this increment.

## Remaining scope

This increment does not implement the project's People/Conversation/shared Tasks browser views, full settings, attachment previews or PTY terminal. Those remain in the approved dashboard/automation plan. Hub-specific provenance branches have deterministic unit coverage; these browser scenarios use real same-server Sessions, not authenticated live CyberdyneAuth users. Live issuer acceptance remains held during the user-reported outage. Physical second-device certificate trust and native computer-use acceptance remain separate, unperformed checks.

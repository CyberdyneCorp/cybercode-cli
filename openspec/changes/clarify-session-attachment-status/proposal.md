## Why

An idle reachable Session is persisted history, not proof that its CLI is open. The current directory and sidebar show execution state without client attachment state, so saved Sessions can look like connected agents. The user requested a clear distinction after approving the sidebar in PR #45.

## What Changes

- Track the TUI as an explicit Session-bound interactive client using the existing owner-authenticated, 45-second attachment lease; renew every 15 seconds and release on normal exit or Session/Location change.
- Add optional `interactiveAttached` to reachable Session records, independently of the existing execution state. True means a live registered attachment, false means managed attachment tracking with no live registered client, and missing/null means unknown.
- Show `attached · idle/running/waiting`, `background · running`, `waiting · no client reported`, or `saved · inactive` when supported by evidence. Preserve explicit attachment-unknown text for older/unmanaged sources. An attachment proves a registered client, not human attention.
- Reuse existing sidebar/directory refreshes and navigation. Preserve history, ongoing work and explicit Stop behavior when the CLI closes.
- Update API/SDK contracts, regression tests, docs and representative real captures together.

## Capabilities

- `cross-session-messaging`: additive attachment observation in authorized reachable listings.
- `tui`: attachment lifecycle and distinct execution/attachment labels in the sidebar and directory.

## Non-goals

Deleting or archiving Sessions on terminal exit; automatically stopping agents; new messaging permissions; inferring human presence from an observer/event stream; new collaboration transport; pretending older clients/sources supply presence; changing native Stop or permission-grant rules.

## Review

The user approved the intended distinction. Proposal/design/tasks review is pending before this API/data-model and attendance-lifecycle implementation, as required by AGENTS.md.

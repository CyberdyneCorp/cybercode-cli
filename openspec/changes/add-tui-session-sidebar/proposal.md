## Why

The right sidebar shows context, tools and machine activity, but a user must open `/agents` to discover other reachable Sessions and agents. The user requested a persistent list of named local/remote Sessions and team agents in that panel, plus refreshed README screenshots after it works.

PR #42 already added the full agent directory, and PR #41 preserved its interaction with native-control warnings. This change surfaces that existing directory rather than inventing another roster or messaging system.

## What Changes

- Add a collapsible **Sessions & agents** section immediately after Context, above the optional goal/workflow/team/tool sections.
- Show up to six reachable named entries, grouped into Local Sessions, Team/child agents and Remote Sessions/agents. Use the Session title where supplied, otherwise the advertised agent name/address; include state and concise source/kind labels.
- Keep the attached Session's title in the existing sidebar header; do not duplicate it in the reachable list. Exclude human hub-member rows from Session counts. Show offline peer availability separately from actual Session rows.
- Open the existing `/agents` directory from a named row or directory/overflow footer, preserving the row's stable identity. Messaging, watch, rename and attach behavior stay in that existing directory; displaying a row performs none of them.
- Refresh shared directory data every ten seconds while either the sidebar section is visible or `/agents` is open. Coalesce requests and preserve per-source failure, context and authorization fencing.
- Regenerate actual TUI screenshots with synthetic local demo data and add one concise sidebar image to README, with a refreshed screenshot gallery and reproducible capture instructions.

## Capabilities

- `tui`: add "Sidebar reachable Session overview", "Shared directory refresh lifetime" and "Sidebar discovery navigation and accessibility".
- Documentation/capture assets: real application captures; no invented or edited UI images and no private user screenshot committed.
- No new backend route, transport, membership policy, automatic remote attach or global Session dashboard.

## Acceptance and review

The first version is a bounded overview of what the attached Session can already reach, not an assertion that every remote machine or future Relay is connected. Existing auth errors and unknown availability remain explicit. The user approved the proposal, design and tasks on 2026-10-25 and requested implementation in PR #45. See the task record and acceptance evidence.

## Impact

Expected code areas: `crates/cyber-tui/src/agents.rs`, `sidebar.rs`, `view/sidebar.rs`, `runner.rs`, `app.rs`, `mouse.rs` and their screen/behavior tests. Screenshot work reuses `scripts/capture_tui_screens.py`, `scripts/screens/` and `docs/screenshots/`. README and the TUI guide describe the actual bounded behavior.

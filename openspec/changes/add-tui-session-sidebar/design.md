## Context

The sidebar renderer is `crates/cyber-tui/src/view/sidebar.rs`; its focus/collapse state is in `sidebar.rs`. Existing `/agents` data, typed entries, cleaning, source loaders and actions live in `agents.rs`. That directory already reads the current Session's authorized messaging listing and bound project-hub roster. Its `Entry` carries name, kind, state, source/location and safe routing identity. Its polling currently runs only while the overlay is open, and its interaction state contains filters, selection and an editor.

The existing reachable listing excludes the caller. The current Session already has a named sidebar header, so the compact list will describe **other reachable Sessions/agents**. A hub human is not a Session. An unreachable peer's wildcard availability record is not a discovered remote Session.

## Decisions

### 1. One data cache, two views

Reuse source loaders and the directory entry model. Separate the shared source cache/request lifecycle from overlay-only filter, selected row and compose/rename editor state where needed. The sidebar projects unfiltered data; typing a filter in `/agents` must not silently filter the sidebar.

Do not create a second HTTP polling loop or route. A single refresh scheduler reads a source at most once per ten-second interval, while either the expanded sidebar section is actually visible (docked or open drawer) or the directory overlay needs it. Hidden/collapsed sidebars alone do not poll. A manual directory refresh may run immediately and is coalesced with pending reads. Pause for bounded interaction only when necessary; editing must not permanently freeze the passive sidebar.

Opening the directory can use fresh cached rows immediately and refresh when due. Closing it clears its editor/filter interaction without discarding a cache still needed by the sidebar. Observed Session, Location, server/peer connection or account/context changes invalidate affected data and advance generation fences. Late replies cannot repopulate another context. Existing per-source behavior remains: ordinary listing failures may retain visibly stale data; observed hub authorization loss clears hub rows immediately. Hub requests run only when enabled and project-bound.

### 2. Compact, truthful presentation

Place **Sessions & agents** after Context so it is visible in the user's pictured layout. It is expanded by default and follows existing section collapse, sidebar toggle, drawer and focus conventions. Keep the existing detailed Team section for task/lead controls; do not remove richer functionality as part of this overview.

Project entries into three groups:

- **Local Sessions**: other ordinary root Sessions on this server.
- **Team / child agents**: local teammates, subagents and workflow agents, with kind labels. Use real team identity only when the existing data supplies it; do not infer a team from a title.
- **Remote**: direct-peer Sessions and authenticated hub agents, with `peer:<name>` or `hub:<project>` provenance. Human hub members remain in the full directory/People view.

Show at most six named entries total, with group counts and an actionable `+N more · /agents` row. Reserve one preview entry per nonempty group before filling the remaining slots deterministically, so a large local history cannot hide every team/remote name. Short panels may show fewer than six entries to retain group representation and the directory/overflow footer; rows not actually drawn must not remain keyboard activation targets. Status/provenance consume a short second line only when needed; truncate names by terminal display width using existing helpers. Full names and addresses remain available in the directory. Deterministic ordering is group, running/waiting before idle/offline, cleaned name and stable identity as a tie-breaker. Count before truncation, without treating peer wildcard placeholders as Sessions.

The existing current-Session header remains authoritative; the overview does not add another copy of that row. Dedupe only with a verified source/Session identity, never by matching names across machines. Equal remote titles must remain distinguishable by provenance.

Initial loading, ready-but-empty, disabled/unsupported source, stale listing, offline peer and failed/revoked hub states are distinct. Never fabricate a Session title or mark a source live from a failed read. Older servers missing all directory routes retain the existing unsupported/older-server behavior rather than a fake empty roster. Pre-attachment has no implied remote reach; refresh after a real Session attaches.

### 3. Navigation without new authority

The section header toggles collapse, matching other sidebar headers. An always-available directory footer opens `/agents` (showing `+N more` when truncated). Activating a named row opens that directory with the stable row selected. Preserve existing sidebar first-click selection/second-click activation and Enter behavior. If a row disappears, do not act on the replacement at its old index; show the refreshed directory instead.

No automatic send, watch, rename, Session creation, peer attach or hub mutation is introduced. The full directory retains its existing explicit actions and authority checks. Use entry identity/validated target fields, never the rendered name, as the action target. Reuse text cleaning, Unicode-width handling and rejection of identifiers changed by cleaning.

Native control remains independent: global Stop precedes sidebar/editor dispatch, armed/uncertain warnings remain visible, and opening/switching a Session follows the existing exact-Session presence and reply fencing. Slow remote reads cannot block key handling or Stop.

### 4. Real, reproducible screenshots

Extend the existing PTY/headless-Chrome capture pipeline. Seed an isolated HOME/CYBER_HOME with named local Sessions, a child/team agent and a local scripted peer fixture; use synthetic names, fixed machine statistics and the scripted model. No live inference, live CyberdyneAuth, production peer or personal Session data is needed.

Capture a readable dark-theme sidebar hero and light/no-color or narrow-width views for the gallery. Add one compact README image and link to the gallery, avoiding a large screenshot wall. Update affected existing captures and captions so screenshots agree with the default UI. Explain which data is synthetic and how to reproduce captures. Do not publish the user's reference screenshot or native desktop acceptance PNGs.

## Alternatives

- A new server-wide roster endpoint: unnecessary for this request and risks changing reach/auth semantics.
- A separately polled sidebar copy: rejected because counts, failures and revoked memberships could diverge from `/agents`.
- Full messaging/rename controls in every sidebar row: defer to the existing directory to keep the overview small and avoid accidental mutations.
- Listing every row without a bound: rejected because the panel must retain context/tool/connection information on normal terminal sizes.

## Verification

Use production-server screen tests plus source fixtures for peers/hub, including malformed/control-character titles and duplicate names. Test visible/hidden/collapsed polling, request coalescing, stale source replies after Session/server/account changes, hub revocation, current-Session exclusion, max-row/overflow counts and row identity after reorder/removal. Cover keyboard/mouse navigation, slow remote reads, busy directory editors and native warning/Stop priority.

Render dark/light/NO_COLOR at docked 160x48 and 120x34, drawer 100x30 and narrow layouts; preserve footer visibility and useful names. Run meaningful TUI tests, strict affected Clippy, OpenSpec and relevant CI. Generate screenshots from the fresh built binary and inspect the actual PNGs, not just their existence. Keep ignored/manual checks explicit and never claim live hub acceptance from fixtures.

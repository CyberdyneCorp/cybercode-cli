## ADDED Requirements

### Requirement: Sidebar reachable Session overview
(P1) The right sidebar SHALL provide a collapsible, initially expanded Sessions and agents overview after Context. It SHALL show at most six reachable named entries total, grouped as local Sessions, team/child agents and remote Sessions/agents, with actual state, kind and concise provenance. It SHALL use supplied Session titles or advertised agent names/addresses, sanitize external text and truncate by terminal display width. Overflow SHALL link to the existing agent directory. The current Session title SHALL remain in the existing header without a duplicate reachable row. Human hub members and offline peer wildcard records SHALL NOT be counted as discovered Sessions.

#### Scenario: Named local and remote entries
- **WHEN** the attached Session can reach a local Session, a teammate and a Session on a paired peer
- **THEN** the sidebar shows their supplied names, actual states and distinct local/team/peer provenance without opening an overlay or inventing a remote connection

#### Scenario: Bounded list and duplicate names
- **WHEN** more than six entries are reachable and two remote entries have the same title
- **THEN** the overview renders at most six named entries with truthful total/overflow counts, keeps different identities distinct and exposes their provenance and full details through the directory

#### Scenario: No reachable Session or unavailable source
- **WHEN** a source is loading, unavailable, disabled, empty or reports an offline peer without Session rows
- **THEN** the corresponding state is explicit and is not rendered or counted as a fabricated live Session

### Requirement: Shared directory refresh lifetime
(P1) Sidebar and agent-directory views SHALL use the same authorized source loaders and shared source data. Refresh demand SHALL be coalesced to one read per source per ten-second interval while an expanded visible overview or directory needs it, except an explicit manual refresh. A hidden/collapsed sidebar alone SHALL NOT keep polling. Overlay filters/editors SHALL NOT change the sidebar's unfiltered projection. Source replies SHALL be fenced to the current Session, Location, connection, generation and observed account context; observed hub authorization loss SHALL clear hub rows. Network failures SHALL NOT block UI input or independent native Stop.

#### Scenario: Both views need data
- **WHEN** the directory is opened while the sidebar has fresh data or a refresh in flight
- **THEN** it can use that data and coalesces the read rather than starting an independent roster loop

#### Scenario: Context switch while remote data is pending
- **WHEN** a late peer/hub reply arrives after a Session, server, Location or relevant account context changes
- **THEN** it cannot repopulate the new context with the old roster or authorize an action under the old identity

#### Scenario: Hub access revoked
- **WHEN** the shared source observes that the current hub membership is no longer authorized
- **THEN** hub entries disappear from both views without preserving a stale roster as live data

### Requirement: Sidebar discovery navigation and accessibility
(P1) The overview SHALL follow sidebar collapse/focus, keyboard and mouse conventions. The section header SHALL toggle collapse; a directory footer or named row SHALL open the existing agent directory, selecting by stable identity when still available. Displaying or activating an overview row SHALL NOT automatically send, watch, rename, create or attach a Session. Navigation SHALL NOT retarget a stale selected index after refresh. Dark, light, NO_COLOR and narrow/drawer rendering SHALL preserve readable labels and existing native-control warning/Stop priority.

#### Scenario: Row changes before activation
- **WHEN** entries reorder or the focused entry disappears between render and activation
- **THEN** navigation follows its stable identity or opens the refreshed directory without acting on another row that inherited the index

#### Scenario: Native control while browsing the roster
- **WHEN** native control is active or uncertain and the sidebar or a busy directory editor has focus
- **THEN** its warning remains visible and the existing independent Stop shortcut is processed before roster/editor keys

#### Scenario: Accessible compact rendering
- **WHEN** the terminal uses NO_COLOR or a narrow sidebar drawer
- **THEN** names and state/source labels remain understandable without color and overflow remains discoverable without hiding connection/footer information

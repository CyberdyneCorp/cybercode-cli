## ADDED Requirements

### Requirement: Repository map view
(P1) The TUI SHALL provide `/map [<path>]`, which opens a scrollable view of the current Location's repository map. It SHALL be rendered by `GET /api/v1/lsp/map` with the Session's model tool budget and the optional path as focus.

**Header:** the view SHALL show:
- indexed and excluded file counts;
- symbols;
- whether the index is complete, partial, truncated, stale or building;
- the last refresh time.

**Keys:** `r` SHALL start a rebuild through `POST /api/v1/lsp/map/rebuild`, `Enter` on a path SHALL open it in the file explorer, and `Esc` SHALL close the view.

**Side effects:** opening the view SHALL NOT add anything to the transcript or the model context.

#### Scenario: Inspect the map
- **WHEN** the user types `/map src/auth`
- **THEN** a view opens with the index status header and the map focused on `src/auth`, and the transcript is unchanged

#### Scenario: Map disabled
- **WHEN** `repo_map.enabled` is false and the user types `/map`
- **THEN** the TUI shows `Repository map is disabled (repo_map.enabled)` and opens no view

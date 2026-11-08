## ADDED Requirements

### Requirement: Output style picker
(P0) `/output-style` and the palette action **Output style…** SHALL open the output style picker, an overlay that lists the styles from `GET /api/v1/config/output-styles` for the Session's Location. Each row SHALL show the name, description and origin (`built-in`, `user` or `project`), with `current` on the Session's effective style and `default` on the configured one. The listing's issues, such as a held project directory, SHALL be shown below the list. Typing SHALL fuzzy-filter names. Enter SHALL switch the current Session. `d` SHALL save the highlighted built-in or user style as the user's default and switch the Session. Esc SHALL close the picker without side effects. `/output-style <name>` SHALL switch without opening the picker, and an unknown or held name SHALL show the server's reason. After a switch the status footer SHALL show `Output style: <name> (next turn)` for 2 seconds. When the Session's selected or configured style is unavailable, the TUI SHALL show the reason once per Session. `/output-style` SHALL be listed in `/help` and in the command palette. A subagent thread SHALL refuse `/output-style`.

#### Scenario: Switch from the picker
- **WHEN** the user opens `/output-style`, selects `explanatory` and presses Enter
- **THEN** the Session's output style becomes `explanatory`, the footer shows `Output style: explanatory (next turn)`, and the next request's system prompt ends with that style

#### Scenario: Held project style
- **WHEN** the user opens the picker in an untrusted checkout that has `.cyber/output-styles/reviewer.md`
- **THEN** `reviewer` is not listed and the picker shows that project output styles are held until the checkout is trusted

## MODIFIED Requirements

### Requirement: Settings Config tab
(P1) The Config tab SHALL list TUI preferences that can be edited in place: `theme`, `theme_mode`, `sidebar`, `recap`, `spinner`, `reasoning`, `diff_style`, `timestamps`, `color` and `accessibility.reduced_motion`. Each row SHALL show the current value and its source (`default`, `global`, `project`, `saved` for `kv.json`, or `session` for a toggle made in this run only, such as Ctrl+T or `/timestamps`). Enter or Space SHALL cycle a value or open its picker. A change SHALL apply immediately and be saved to `~/.local/state/cyber/kv.json` with the time it was saved, and a `tui.jsonc` written after that time SHALL win over it. The tab SHALL also list an `output style` row. It SHALL show the current Session's effective output style and its source (`session`, `config` or `default`), and Enter SHALL open the output style picker. Below the preferences, the tab SHALL show effective core settings (`model`, `small_model`, `mode`, `default_agent`, `output_style`, `budgets`) read-only with their source file, when the server's `config` route is available. Otherwise it SHALL show `Core settings: use cyber config or edit cyber.jsonc`.

With the mouse captured, a click on a preference row SHALL select it, and a second click on it SHALL act as Enter (cycle the value or open its picker).

#### Scenario: Toggle the sidebar from Config
- **WHEN** the user selects `sidebar` and presses Enter until it reads `hide`
- **THEN** the sidebar disappears, and the value persists after a restart

#### Scenario: Output style from Config
- **WHEN** the user selects the `output style` row and presses Enter
- **THEN** the output style picker opens, and choosing `concise` updates the row to `concise (session)`

#### Scenario: Cycle a value by clicking
- **WHEN** the user clicks the `timestamps` row once and then again
- **THEN** the first click selects it and the second cycles it to its next value, which is saved

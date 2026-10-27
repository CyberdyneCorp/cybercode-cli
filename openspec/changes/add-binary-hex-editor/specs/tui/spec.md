## MODIFIED Requirements

### Requirement: File explorer
(P1) The TUI SHALL provide a file explorer, opened with `/files` or the `file_explorer` binding (default `<leader>o`, that is `ctrl+x o`, because `<leader>e` is `sidebar_focus`) and closed with Esc. It SHALL show the Location's project as a tree in a pane on `surface_alt`: a header bar (`surface`) with `EXPLORER`, the project name and `:branch`, and at the right `✓ clean` in `success` or `N changed` in `warning`; a filter line with a `⌕` placeholder; the tree; and a summary line (`N shown · M hidden · .gitignore`). Root entries SHALL have no guide; each deeper level SHALL add a `│ ` indent guide in `text_subtle`, followed by the fold marker (`▸` collapsed, `▾` expanded). Directory names SHALL be bold in `file_dir` with a trailing `/`, an expanded directory SHALL show its loaded child count, and file names SHALL be colored by kind from their name: `file_doc` (Markdown and text), `file_config` (TOML, JSON, YAML, `justfile` and similar), `file_code` (source files) and `file_dim` (hidden, ignored, lock and generated files). A right-aligned column SHALL show the git status letter and the compact size (`1.2K`) in `text_muted`. The selected row SHALL have an `accent` bar and the `selection` background (reverse video without color). `tui.explorer.icons` (`none` default, `nerd`) SHALL add Nerd Font glyphs before names. The footer SHALL show the explorer keys as chips (the key on a `text_muted` background, then its label), generated from the binding table. Directories SHALL sort before files and SHALL be loaded lazily, one level per expansion, through `GET /api/v1/files`. Each row SHALL show the git status letter (`M` in `warning`, `A` and `?` in `success`, `D` in `error`) from the same VCS data as the sidebar, and a directory SHALL show `M` when any descendant changed. `.gitignore` and `.cyberignore` SHALL apply by default; `.` SHALL toggle hidden and ignored entries, and the footer SHALL count what is hidden. `/` SHALL filter by fuzzy path match across already-loaded and server-searched entries, and `g` SHALL show only changed files.

Enter on a file SHALL open a preview pane to the right, or replace the tree when the terminal is narrower than 100 columns. (P1) The `explorer_focus` binding (default Tab) SHALL move focus between the tree and an open preview, Esc SHALL return focus from the preview to the tree, and the focused pane's header title SHALL be drawn in `accent`. While the preview has focus, Up/Down, PageUp/PageDown and Home SHALL scroll it, and below 100 columns it SHALL replace the tree. The preview SHALL have its own header bar with a breadcrumb (`docs › README.md`), the language, line count and size, and the `+N −M` diff summary. A vertical divider SHALL separate it from the tree. Markdown files SHALL be rendered through the transcript's Markdown renderer by default (no OSC 8 links are written from the preview), and `m` SHALL toggle the raw source, shown by a `rendered`/`raw` chip in the header. Source (code, or raw Markdown with markup scopes colored, never one color) SHALL have line numbers, a gutter mark on changed lines and a `│` separator, with syntax highlighting through the same highlighter and theme tokens as code blocks; long lines SHALL soft-wrap with a hanging indent under the gutter. A scrollbar thumb SHALL show the position when the preview is longer than the pane. `←` SHALL fold the selected directory or move to its parent, and `→` SHALL unfold it. `@` SHALL insert the selected path as a file mention in the composer, relative to the Session directory (absolute when the file lies outside it, as when the Location is below the project root), `y` SHALL copy the project-relative path, and `r` SHALL refresh. The explorer SHALL refresh changed rows on file-change and VCS events without collapsing the user's expansion state.

The explorer SHALL never bypass permissions. Listing SHALL be evaluated as `list` and preview as `read` against the Session's rules for that path. A file whose `read` decision is `ask` SHALL appear dimmed with `⊘ ask to read`, and opening its preview SHALL raise the normal permission request. A file whose decision is `deny` SHALL appear dimmed with `⊘ denied`, with no content. Preview SHALL be capped at 1 MiB and 5,000 lines, with a truncation notice. Binary files SHALL show `binary · <size>` with an explicit Open in Hex action when binary editing is available (`binary-editing`), and symlinks SHALL show their target, never followed outside the project. Displayed file names and content SHALL be sanitized like other untrusted output, so no terminal escape sequence reaches the terminal. Screen readers SHALL get each row as `<name>, directory|file, <status>, level <n>`.

(P1) With the mouse captured, a click on a tree row SHALL select it and act as Enter: a directory row, or its fold marker, folds or unfolds it, and a file row opens its preview (an `ask` file raises the permission request and a `deny` file only shows its notice, as with Enter). A click in the preview SHALL focus it and a click in the tree pane SHALL focus the tree, as `explorer_focus` does. A click on the header's `rendered`/`raw` chip SHALL act as `m`, and a click on a key chip SHALL act as its key. The wheel SHALL act on the pane under the pointer, whichever has focus: over the tree it SHALL move the selection, and over the preview it SHALL scroll the preview by 3 lines per notch.

#### Scenario: Browse and preview
- **WHEN** the user opens the explorer, expands `crates/cyber-tui/src` and presses Enter on `markdown.rs`
- **THEN** the preview shows the file with line numbers, Rust highlighting and the changed-line gutter

#### Scenario: Secret file is not leaked
- **WHEN** `.env` matches the default `read` ask rule and the user selects it
- **THEN** the row shows `⊘ ask to read`, no content is fetched until the user approves the permission request, and a denial shows nothing

#### Scenario: Mention from the explorer
- **WHEN** the user presses `@` on `crates/cyber-tui/src/app.rs`
- **THEN** the composer contains `@crates/cyber-tui/src/app.rs` and the explorer closes

#### Scenario: Narrow terminal
- **WHEN** the terminal is 80 columns wide
- **THEN** the tree fills the width, and Enter replaces it with the preview, which Esc returns from

#### Scenario: Markdown rendered, then raw
- **WHEN** the user opens `docs/README.md` in the preview and presses `m`
- **THEN** the preview first shows the rendered document under a `rendered` chip, and after `m` shows the source with line numbers and heading, link and code colors

#### Scenario: Kinds are colored in the tree
- **WHEN** a directory lists `src/`, `README.md`, `Cargo.toml`, `main.rs` and `Cargo.lock`
- **THEN** `src/` is bold in `file_dir`, the files are in `file_doc`, `file_config`, `file_code` and `file_dim`, and root rows start without a guide

#### Scenario: Browse by clicking
- **WHEN** the user clicks the folded `crates/` row and then the `README.md` row
- **THEN** `crates/` unfolds, and `README.md` is selected and its preview opens

#### Scenario: Wheel over the preview
- **WHEN** a long file's preview is open beside the tree and the user turns the wheel down over the preview
- **THEN** the preview scrolls 3 lines and the tree selection does not move

#### Scenario: Focus the preview
- **WHEN** a preview is open beside the tree and the user presses Tab, or clicks the preview
- **THEN** the preview's header title is drawn in `accent` and Down scrolls the preview; Esc returns focus to the tree

#### Scenario: Toggle raw by clicking the chip
- **WHEN** `docs/README.md` is previewed under a `rendered` chip and the user clicks the chip
- **THEN** the preview shows the Markdown source and the chip reads `raw`

## ADDED Requirements

### Requirement: Hex workspace
(P1) The TUI SHALL open an explicit hex workspace from the explorer, command palette and `/hex <path>`, initially in VIEW. It SHALL display a synchronized offset/hex/ASCII cursor and selection, adaptive 16/8/4-byte rows, optional endian-aware inspector, changed-range rail and truthful revision/dirty/operation state. Edit/review/save/conflict SHALL be distinct states. A dirty exit SHALL offer keep/discard/review without silently writing or dropping changes. Keybindings SHALL use existing context precedence and preserve global interrupt, permission handling and native Stop. All controls SHALL have palette/keyboard access and non-color state indicators in dark/light/NO_COLOR and narrow layouts.

#### Scenario: Type a nibble then leave
- **WHEN** a user enters one hex nibble and presses Esc or switches panes
- **THEN** no file byte changes and the UI explicitly completes or cancels pending nibble input before later editing

#### Scenario: Save finishes after a Session switch
- **WHEN** a response arrives for a binary document after the user switches Session or Location
- **THEN** it cannot repaint the new document, replace its draft or authorize another save

//! The Session sidebar's data and focus (`tui` → Session sidebar). Every value comes
//! from a public route; a section whose route answers 404 or 503 is omitted, never
//! shown with placeholder values. All text from servers is untrusted: it is put on one
//! line without control characters before it is stored.

use std::collections::BTreeSet;

use serde_json::Value;

use crate::app::Action;

/// `tui.sidebar`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Preference {
    /// Docked when the terminal is at least 120 columns wide.
    #[default]
    Auto,
    /// The same docking rule; listed so a project can undo a global `hide`.
    Show,
    /// Hidden until toggled.
    Hide,
}

impl Preference {
    pub fn parse(value: Option<&str>) -> Option<Self> {
        match value? {
            "auto" => Some(Self::Auto),
            "show" => Some(Self::Show),
            "hide" => Some(Self::Hide),
            _ => None,
        }
    }
}

/// A section's source route.
#[derive(Debug, Clone, Default, PartialEq)]
pub enum Source<T> {
    /// Not answered yet.
    #[default]
    Pending,
    /// The route answered 404 or 503: the section is omitted.
    Missing,
    Ready(T),
}

impl<T> Source<T> {
    pub fn ready(&self) -> Option<&T> {
        match self {
            Self::Ready(value) => Some(value),
            _ => None,
        }
    }

    /// The ready value parsed by `f`.
    pub fn map<U>(&self, f: impl FnOnce(&T) -> U) -> Source<U> {
        match self {
            Self::Pending => Source::Pending,
            Self::Missing => Source::Missing,
            Self::Ready(value) => Source::Ready(f(value)),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct McpServer {
    pub name: String,
    /// `connecting`, `connected`, `disabled`, `failed`, `needs_auth`,
    /// `needs_client_registration`.
    pub status: String,
    pub error: Option<String>,
}

impl McpServer {
    /// The status word shown beside the dot.
    pub fn word(&self) -> &'static str {
        match self.status.as_str() {
            "connected" => "Connected",
            "connecting" => "Connecting",
            "failed" => "Failed",
            "needs_auth" | "needs_client_registration" => "Needs auth",
            "disabled" => "Disabled",
            _ => "Unknown",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct LspServer {
    pub id: String,
    /// `starting`, `connected`, `broken`.
    pub status: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FileChange {
    pub file: String,
    pub additions: u64,
    pub deletions: u64,
    pub patch: String,
}

/// The parts of `GET /vcs` the sidebar shows.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Git {
    pub branch: Option<String>,
    pub worktree: Option<String>,
    /// The checked-out commit, for a detached HEAD.
    pub head: Option<String>,
    /// Uncommitted changes.
    pub dirty: bool,
}

/// The instance event stream.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Connection {
    #[default]
    Connected,
    Reconnecting,
    Disconnected,
}

impl Connection {
    pub fn word(self) -> &'static str {
        match self {
            Self::Connected => "Connected",
            Self::Reconnecting => "Reconnecting",
            Self::Disconnected => "Disconnected",
        }
    }
}

/// Which route a load reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Part {
    Mcp,
    Lsp,
    Files,
    Vcs,
    Health,
    Parent,
}

impl Part {
    pub const ALL: [Part; 6] = [
        Part::Mcp,
        Part::Lsp,
        Part::Files,
        Part::Vcs,
        Part::Health,
        Part::Parent,
    ];
}

/// A loaded part.
#[derive(Debug, Clone, PartialEq)]
pub enum Loaded {
    Mcp(Source<Vec<McpServer>>),
    Lsp(Source<Vec<LspServer>>),
    Files(Source<Vec<FileChange>>),
    /// `None` inside: outside git.
    Vcs(Source<Option<Git>>),
    Version(Option<String>),
    Parent(Option<String>),
    /// A transport error or another status: the last value stays.
    Failed,
}

/// Sections that collapse with Enter or Space.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Section {
    Mcp,
    Files,
}

/// A row focus can rest on (`<leader>e`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row {
    Header(Section),
    /// A run of the Workflows section; Enter opens it in the monitor.
    Workflow(usize),
    Mcp(usize),
    File(usize),
}

/// One run of the Workflows section: its name and agents done of total.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkflowRow {
    pub id: String,
    /// Untrusted; cleaned when the run was read.
    pub name: String,
    pub done: u64,
    pub total: u64,
}

/// Modified files shown before `+N more`.
pub const FILE_ROWS: usize = 10;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Sidebar {
    pub preference: Preference,
    /// `<leader>b` or `/sidebar` flipped the docked sidebar for this run.
    pub toggled: bool,
    /// The drawer is open over the transcript (60–119 columns).
    pub drawer: bool,
    /// The focused row, counted in [`Sidebar::rows`].
    pub focus: Option<usize>,
    pub collapsed: BTreeSet<Section>,
    pub mcp: Source<Vec<McpServer>>,
    pub lsp: Source<Vec<LspServer>>,
    pub files: Source<Vec<FileChange>>,
    pub vcs: Source<Option<Git>>,
    pub version: Option<String>,
    pub parent_title: Option<String>,
    pub connection: Connection,
    /// The Workflows section's runs, the current Session's first; set before every frame.
    pub workflows: Vec<WorkflowRow>,
    /// Active runs beyond those rows (`+N more`).
    pub workflows_more: usize,
}

impl Sidebar {
    /// Docked at this width, before the screen-reader rule.
    pub fn docked(&self, width: u16) -> bool {
        let wanted = self.preference != Preference::Hide;
        width >= crate::view::layout::DOCK_FROM && wanted != self.toggled
    }

    pub fn apply(&mut self, loaded: Loaded) {
        match loaded {
            Loaded::Mcp(source) => self.mcp = source,
            Loaded::Lsp(source) => self.lsp = source,
            Loaded::Files(source) => self.files = source,
            Loaded::Vcs(source) => self.vcs = source,
            Loaded::Version(version) => self.version = version,
            Loaded::Parent(title) => self.parent_title = title,
            Loaded::Failed => {}
        }
        if let Some(focus) = self.focus {
            self.focus = Some(focus.min(self.rows().len().saturating_sub(1)));
        }
    }

    /// Another Session or Location: its Session-scoped data no longer applies.
    pub fn forget_session(&mut self) {
        self.files = Source::Pending;
        self.parent_title = None;
        self.focus = None;
    }

    /// Location-scoped data, for a Session in another directory.
    pub fn forget_location(&mut self) {
        self.mcp = Source::Pending;
        self.lsp = Source::Pending;
        self.vcs = Source::Pending;
    }

    pub fn servers(&self) -> &[McpServer] {
        self.mcp.ready().map_or(&[], Vec::as_slice)
    }

    pub fn changes(&self) -> &[FileChange] {
        self.files.ready().map_or(&[], Vec::as_slice)
    }

    /// Focusable rows, top to bottom, as the view draws them.
    pub fn rows(&self) -> Vec<Row> {
        let mut rows: Vec<Row> = (0..self.workflows.len()).map(Row::Workflow).collect();
        if !self.servers().is_empty() {
            rows.push(Row::Header(Section::Mcp));
            if !self.collapsed.contains(&Section::Mcp) {
                rows.extend((0..self.servers().len()).map(Row::Mcp));
            }
        }
        if !self.changes().is_empty() {
            rows.push(Row::Header(Section::Files));
            if !self.collapsed.contains(&Section::Files) {
                rows.extend((0..self.changes().len().min(FILE_ROWS)).map(Row::File));
            }
        }
        rows
    }

    pub fn focused(&self) -> Option<Row> {
        self.rows().get(self.focus?).copied()
    }

    pub fn step(&mut self, down: bool) {
        let last = self.rows().len().saturating_sub(1);
        let at = self.focus.unwrap_or(0);
        self.focus = Some(if down {
            (at + 1).min(last)
        } else {
            at.saturating_sub(1)
        });
    }

    pub fn toggle(&mut self, section: Section) {
        if !self.collapsed.remove(&section) {
            self.collapsed.insert(section);
        }
    }
}

/// One line without control characters, for names, errors and paths from servers.
pub fn clean(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default()
        .chars()
        .map(|c| if c == '\t' { ' ' } else { c })
        .filter(|c| !c.is_control())
        .collect()
}

/// A path for display, on one line without control characters and without Windows'
/// verbatim prefix: `\\?\C:\x` is shown as `C:\x`, `\\?\UNC\host\share` as
/// `\\host\share`. Session directories are canonical, which on Windows adds it.
pub fn display_path(path: &str) -> String {
    let plain = match path.strip_prefix(r"\\?\UNC\") {
        Some(rest) => format!(r"\\{rest}"),
        None => path.strip_prefix(r"\\?\").unwrap_or(path).to_owned(),
    };
    clean(&plain)
}

/// [`display_path`] with `home` shown as `~`. Windows paths (a drive letter or a UNC
/// share) compare without case, as the filesystem does.
pub fn home_path(path: &str, home: Option<&str>) -> String {
    let path = display_path(path);
    let home = home.map(display_path).unwrap_or_default();
    let home = home.trim_end_matches(['/', '\\']);
    if home.is_empty() {
        return path;
    }
    let windows = home.starts_with(r"\\") || home.as_bytes().get(1) == Some(&b':');
    let same = path.get(..home.len()).is_some_and(|head| {
        if windows {
            head.eq_ignore_ascii_case(home)
        } else {
            head == home
        }
    });
    match path.get(home.len()..) {
        Some(rest) if same && (rest.is_empty() || rest.starts_with(['/', '\\'])) => {
            format!("~{rest}")
        }
        _ => path,
    }
}

/// A patch for display: control characters removed from every line.
pub fn clean_patch(patch: &str) -> String {
    patch
        .lines()
        .map(|line| {
            line.chars()
                .map(|c| if c == '\t' { ' ' } else { c })
                .filter(|c| !c.is_control())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `GET /mcp` data.
pub fn parse_mcp(data: &Value) -> Vec<McpServer> {
    data.as_array()
        .into_iter()
        .flatten()
        .filter(|s| s["configured"] != false)
        .map(|s| McpServer {
            name: clean(s["name"].as_str().unwrap_or_default()),
            status: clean(s["status"].as_str().unwrap_or_default()),
            error: s["error"].as_str().map(clean).filter(|e| !e.is_empty()),
        })
        .collect()
}

/// `GET /lsp` data.
pub fn parse_lsp(data: &Value) -> Vec<LspServer> {
    data.as_array()
        .into_iter()
        .flatten()
        .map(|s| LspServer {
            id: clean(s["id"].as_str().unwrap_or_default()),
            status: clean(s["status"].as_str().unwrap_or_default()),
        })
        .collect()
}

/// `GET /sessions/:id/diff` data.
pub fn parse_files(data: &Value) -> Vec<FileChange> {
    data.as_array()
        .into_iter()
        .flatten()
        .map(|d| FileChange {
            file: clean(d["file"].as_str().unwrap_or_default()),
            additions: d["additions"].as_u64().unwrap_or(0),
            deletions: d["deletions"].as_u64().unwrap_or(0),
            patch: clean_patch(d["patch"].as_str().unwrap_or_default()),
        })
        .collect()
}

/// `GET /vcs` data: `{ vcs: null }` outside git.
pub fn parse_vcs(data: &Value) -> Option<Git> {
    let vcs = data.get("vcs").filter(|v| v.is_object())?;
    let text = |key: &str| vcs[key].as_str().map(clean).filter(|s| !s.is_empty());
    Some(Git {
        branch: text("branch"),
        worktree: text("worktree"),
        head: text("head"),
        dirty: vcs["dirty"] == true,
    })
}

/// Sidebar keys and visibility (`<leader>b`, `/sidebar`, `<leader>e`).
impl crate::app::App {
    /// The last frame's width, or the terminal's before the first frame.
    fn width(&self) -> u16 {
        match self.frame_width.load(std::sync::atomic::Ordering::Relaxed) {
            0 => crossterm::terminal::size().map_or(80, |(w, _)| w),
            width => width,
        }
    }

    /// Drawn at the current width, docked or as the drawer.
    pub fn sidebar_visible(&self) -> bool {
        !self.screen_reader && (self.sidebar.docked(self.width()) || self.sidebar.drawer)
    }

    /// Docked at 120 columns and more; the drawer below, which takes focus so Esc
    /// closes it.
    pub fn toggle_sidebar(&mut self) {
        if self.screen_reader {
            self.toast("The sidebar is not drawn in screen-reader mode");
            return;
        }
        if self.width() >= crate::view::layout::DOCK_FROM {
            self.sidebar.toggled = !self.sidebar.toggled;
            if !self.sidebar.docked(self.width()) {
                self.sidebar.focus = None;
            }
        } else {
            self.sidebar.drawer = !self.sidebar.drawer;
            self.sidebar.focus = self.sidebar.drawer.then_some(0);
        }
    }

    /// Move focus into the sidebar, showing it first when it is hidden.
    pub fn focus_sidebar(&mut self) {
        if self.screen_reader {
            self.toast("The sidebar is not drawn in screen-reader mode");
            return;
        }
        if self.width() >= crate::view::layout::DOCK_FROM {
            if !self.sidebar.docked(self.width()) {
                self.sidebar.toggled = !self.sidebar.toggled;
            }
        } else {
            self.sidebar.drawer = true;
        }
        self.sidebar.focus = Some(0);
        self.completion = None;
    }

    /// Esc: focus returns to the composer and the drawer closes.
    fn leave_sidebar(&mut self) {
        self.sidebar.focus = None;
        self.sidebar.drawer = false;
    }

    pub(crate) fn sidebar_key(&mut self, key: crossterm::event::KeyEvent) -> Vec<Action> {
        use crossterm::event::KeyCode;
        let action = self.keymap.action("sidebar", &key, false);
        match (action, key.code) {
            (Some("sidebar_close"), _) => self.leave_sidebar(),
            (Some("sidebar_next"), _) | (None, KeyCode::Char('j')) => self.sidebar.step(true),
            (Some("sidebar_previous"), _) | (None, KeyCode::Char('k')) => self.sidebar.step(false),
            (Some("sidebar_select"), _) | (None, KeyCode::Char(' ')) => return self.select_row(),
            (Some("sidebar_reconnect"), _) => return self.reconnect_row(),
            _ => {}
        }
        Vec::new()
    }

    /// Enter or Space: collapse or expand a section, open a file's diff, or open a
    /// run in the Workflow monitor.
    fn select_row(&mut self) -> Vec<Action> {
        match self.sidebar.focused() {
            Some(Row::Header(section)) => self.sidebar.toggle(section),
            Some(Row::File(index)) => {
                self.overlay = crate::app::Overlay::FileDiff { index, scroll: 0 }
            }
            Some(Row::Workflow(index)) => {
                let Some(run) = self.sidebar.workflows.get(index) else {
                    return Vec::new();
                };
                let run_id = run.id.clone();
                self.leave_sidebar();
                return self.open_workflow_run(run_id);
            }
            Some(Row::Mcp(_)) | None => {}
        }
        Vec::new()
    }

    /// `r` on a failed (or waiting) MCP server.
    fn reconnect_row(&mut self) -> Vec<Action> {
        let Some(Row::Mcp(index)) = self.sidebar.focused() else {
            return Vec::new();
        };
        let Some(server) = self.sidebar.servers().get(index) else {
            return Vec::new();
        };
        if matches!(
            server.status.as_str(),
            "connected" | "connecting" | "disabled"
        ) {
            self.toast(format!(
                "{} is {}",
                server.name,
                server.word().to_lowercase()
            ));
            return Vec::new();
        }
        let name = server.name.clone();
        self.toast(format!("Reconnecting {name}…"));
        vec![Action::ReconnectMcp(name)]
    }

    /// Keys over a file's patch.
    pub(crate) fn file_diff_key(&mut self, key: crossterm::event::KeyEvent) {
        use crossterm::event::KeyCode;
        let crate::app::Overlay::FileDiff { scroll, .. } = &mut self.overlay else {
            return;
        };
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => *scroll = scroll.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => *scroll = scroll.saturating_add(1),
            KeyCode::PageUp => *scroll = scroll.saturating_sub(10),
            KeyCode::PageDown => *scroll = scroll.saturating_add(10),
            KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') => {
                self.overlay = crate::app::Overlay::None
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Regression: Windows Session directories are canonical (`\\?\C:\...`), so the
    /// `~` shortening never matched `USERPROFILE` and the raw verbatim path was shown.
    #[test]
    fn verbatim_windows_paths_are_shown_plain_and_home_relative() {
        let home = Some(r"C:\Users\x");
        assert_eq!(home_path(r"\\?\C:\Users\x\repo", home), r"~\repo");
        assert_eq!(home_path(r"\\?\c:\users\X\repo", home), r"~\repo");
        assert_eq!(home_path(r"\\?\C:\Users\x", home), "~");
        assert_eq!(
            home_path(r"\\?\C:\Users\xy\repo", home),
            r"C:\Users\xy\repo"
        );
        assert_eq!(home_path(r"\\?\D:\work", home), r"D:\work");
        assert_eq!(
            display_path(r"\\?\UNC\host\share\repo"),
            r"\\host\share\repo"
        );
        assert_eq!(
            home_path("/Users/leo/work/app", Some("/Users/leo")),
            "~/work/app"
        );
        assert_eq!(
            home_path("/Users/Leo/work", Some("/Users/leo")),
            "/Users/Leo/work"
        );
        assert_eq!(
            home_path("/Users/leo€/a", Some("/Users/leo")),
            "/Users/leo€/a"
        );
        assert_eq!(home_path("/srv/app", Some("/")), "/srv/app");
        assert_eq!(home_path("/srv/app", None), "/srv/app");
    }

    #[test]
    fn server_text_is_one_line_without_controls() {
        assert_eq!(clean("\x1b]52;c;AAAA\x07evil\nsecond"), "]52;c;AAAAevil");
        assert_eq!(clean("\n  spawn ENOENT  \n"), "spawn ENOENT");
        let servers = parse_mcp(&json!([
            {"name": "a\x1b[31m", "status": "failed", "error": "boom\x07\nmore", "configured": true},
            {"name": "gone", "status": "connected", "configured": false},
        ]));
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].name, "a[31m");
        assert_eq!(servers[0].error.as_deref(), Some("boom"));
        assert_eq!(servers[0].word(), "Failed");
        assert_eq!(clean_patch("+x\x1b[2J\ty"), "+x[2J y");
    }

    #[test]
    fn vcs_null_means_outside_git() {
        assert_eq!(parse_vcs(&json!({ "vcs": null })), None);
        assert_eq!(
            parse_vcs(&json!({ "vcs": { "branch": "main", "worktree": null } })),
            Some(Git {
                branch: Some("main".into()),
                ..Git::default()
            })
        );
    }

    #[test]
    fn rows_follow_sections_and_collapse() {
        let mut s = Sidebar {
            mcp: Source::Ready(parse_mcp(&json!([{"name":"a","status":"connected"}]))),
            files: Source::Ready(
                (0..12)
                    .map(|i| FileChange {
                        file: format!("f{i}"),
                        additions: 1,
                        deletions: 0,
                        patch: String::new(),
                    })
                    .collect(),
            ),
            ..Default::default()
        };
        assert_eq!(s.rows().len(), 1 + 1 + 1 + FILE_ROWS);
        s.toggle(Section::Mcp);
        assert_eq!(
            s.rows()[..2],
            [Row::Header(Section::Mcp), Row::Header(Section::Files)]
        );
        s.focus = Some(5);
        s.step(true);
        assert_eq!(s.focused(), Some(Row::File(4)));
        s.toggle(Section::Files);
        s.apply(Loaded::Failed);
        assert_eq!(s.focus, Some(1), "focus stays on a drawn row");
    }

    #[test]
    fn preference_and_toggle_decide_docking() {
        let mut s = Sidebar::default();
        assert!(s.docked(120) && !s.docked(119));
        s.toggled = true;
        assert!(!s.docked(160));
        s.preference = Preference::Hide;
        assert!(s.docked(160));
        s.toggled = false;
        assert!(!s.docked(160));
        assert_eq!(Preference::parse(Some("show")), Some(Preference::Show));
        assert_eq!(Preference::parse(Some("left")), None);
    }
}

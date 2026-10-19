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

/// MCP servers listed in the sidebar before `+N more`.
pub const MCP_ROWS: usize = 6;

#[derive(Debug, Clone, PartialEq)]
pub struct McpServer {
    pub name: String,
    /// `idle`, `connecting`, `connected`, `disabled`, `failed`, `needs_auth`,
    /// `needs_client_registration`.
    pub status: String,
    pub error: Option<String>,
    /// `remote` or `stdio`, when the server reports it.
    pub transport: Option<String>,
    /// Published tools, counted while connected.
    pub tools: usize,
}

impl McpServer {
    /// Refused by organization policy rather than broken.
    pub fn blocked(&self) -> bool {
        self.error
            .as_deref()
            .is_some_and(|error| error.contains("blocked by organization policy"))
    }

    /// The status word shown beside the dot.
    pub fn word(&self) -> &'static str {
        match self.status.as_str() {
            "connected" => "Connected",
            "connecting" => "Connecting",
            "idle" => "Idle",
            "failed" if self.blocked() => "Blocked",
            "failed" => "Failed",
            "needs_auth" | "needs_client_registration" => "Needs auth",
            "disabled" => "Disabled",
            _ => "Unknown",
        }
    }
}

/// An enabled remote of `peers.remotes` (`GET /peers/remotes`).
#[derive(Debug, Clone, PartialEq)]
pub struct PeerRemote {
    pub name: String,
    /// It answered its last listing.
    pub online: bool,
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

/// `GET /system`: the server machine's CPU, memory and the Location's disk.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Machine {
    /// The OS name and version; untrusted, cleaned when read.
    pub label: Option<String>,
    pub cores: u64,
    /// `None` until the server has two CPU readings.
    pub cpu_idle: Option<f64>,
    pub load: Option<[f64; 3]>,
    pub memory_total: u64,
    pub memory_available: u64,
    /// `(total, available)` bytes of the Location's volume.
    pub disk: Option<(u64, u64)>,
}

impl Machine {
    /// Percent of CPU busy.
    pub fn cpu_busy(&self) -> Option<f64> {
        self.cpu_idle.map(|idle| (100.0 - idle).clamp(0.0, 100.0))
    }

    /// Percent of memory in use.
    pub fn memory_used(&self) -> Option<f64> {
        used_percent(self.memory_total, self.memory_available)
    }

    /// Percent of the Location's disk in use.
    pub fn disk_used(&self) -> Option<f64> {
        self.disk
            .and_then(|(total, available)| used_percent(total, available))
    }
}

fn used_percent(total: u64, available: u64) -> Option<f64> {
    (total > 0).then(|| total.saturating_sub(available) as f64 / total as f64 * 100.0)
}

/// `GET /system` data.
pub fn parse_machine(data: &Value) -> Machine {
    let bytes = |v: &Value| v.as_u64().unwrap_or(0);
    let load = data["load_avg"].as_array().and_then(|values| {
        let numbers: Vec<f64> = values.iter().filter_map(Value::as_f64).collect();
        <[f64; 3]>::try_from(numbers).ok()
    });
    let disk = &data["disk"];
    Machine {
        label: data["hostname_label"]
            .as_str()
            .map(clean)
            .filter(|s| !s.is_empty()),
        cores: data["cores_logical"].as_u64().unwrap_or(0),
        cpu_idle: data["cpu_idle_percent"].as_f64(),
        load,
        memory_total: bytes(&data["memory_total"]),
        memory_available: bytes(&data["memory_available"]),
        disk: disk
            .is_object()
            .then(|| (bytes(&disk["total"]), bytes(&disk["available"]))),
    }
}

/// Seconds between `GET /system` reads while the sidebar shows.
pub const MACHINE_POLL: std::time::Duration = std::time::Duration::from_secs(3);

/// `11.4` (GB, binary): one decimal below 100, none above.
pub fn gigabytes(bytes: u64) -> String {
    let gb = bytes as f64 / (1024.0 * 1024.0 * 1024.0);
    if gb < 100.0 {
        format!("{gb:.1}")
    } else {
        format!("{gb:.0}")
    }
}

/// `29.5 thousand`, `128 thousand`, `1.2 million`: token counts read aloud.
fn spoken(n: u64) -> String {
    let (value, unit) = match n {
        0..1_000 => return n.to_string(),
        1_000..1_000_000 => (n as f64 / 1_000.0, "thousand"),
        _ => (n as f64 / 1_000_000.0, "million"),
    };
    let text = format!("{value:.1}");
    format!("{} {unit}", text.strip_suffix(".0").unwrap_or(&text))
}

/// The Context meter as one sentence for screen readers, e.g. `Context 22 percent used,
/// 29.5 thousand of 128 thousand tokens, auto-compact at 85 percent`.
pub fn context_sentence(meter: &crate::model::ContextMeter) -> String {
    let (Some(window), Some(percent)) = (meter.window, meter.percent()) else {
        return format!(
            "Context {} tokens, window unknown",
            spoken(meter.used_tokens)
        );
    };
    let compaction = match meter.compact_percent() {
        Some(at) => format!("auto-compact at {at:.0} percent"),
        None => "auto-compact off".to_owned(),
    };
    format!(
        "Context {percent:.0} percent used, {} of {} tokens, {compaction}",
        spoken(meter.used_tokens),
        spoken(window)
    )
}

/// One sentence for screen readers and the Status tab.
pub fn machine_sentence(m: &Machine) -> String {
    let cpu = match m.cpu_busy() {
        Some(busy) => format!("CPU {busy:.0} percent busy, {} cores", m.cores),
        None => format!("CPU measuring, {} cores", m.cores),
    };
    let mut parts = vec![cpu];
    if let Some(used) = m.memory_used() {
        parts.push(format!(
            "memory {used:.0} percent used, {} GB free",
            gigabytes(m.memory_available)
        ));
    }
    if let (Some(used), Some((_, available))) = (m.disk_used(), m.disk) {
        parts.push(format!(
            "disk {used:.0} percent used, {} GB free",
            gigabytes(available)
        ));
    }
    parts.join("; ")
}

/// A `width`-cell bar filled to `percent` in eighths: full blocks, one partial block,
/// then spaces for the track.
pub fn bar_cells(percent: f64, width: usize) -> String {
    const PARTIAL: [char; 7] = ['▏', '▎', '▍', '▌', '▋', '▊', '▉'];
    let eighths = ((percent.clamp(0.0, 100.0) / 100.0) * (width * 8) as f64).round() as usize;
    let mut out = "█".repeat(eighths / 8);
    if !eighths.is_multiple_of(8) {
        out.push(PARTIAL[eighths % 8 - 1]);
    }
    let used = out.chars().count();
    out.push_str(&" ".repeat(width.saturating_sub(used)));
    out
}

/// `[#####.....]` for `NO_COLOR`: `width` cells inside the brackets.
pub fn bar_text(percent: f64, width: usize) -> String {
    let filled = ((percent.clamp(0.0, 100.0) / 100.0) * width as f64).round() as usize;
    format!(
        "[{}{}]",
        "#".repeat(filled),
        ".".repeat(width - filled.min(width))
    )
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
    Peers,
    /// `GET /system`; polled while the sidebar shows (see [`MACHINE_POLL`]), not in
    /// [`Part::ALL`].
    Machine,
}

impl Part {
    pub const ALL: [Part; 7] = [
        Part::Mcp,
        Part::Lsp,
        Part::Files,
        Part::Vcs,
        Part::Health,
        Part::Parent,
        Part::Peers,
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
    /// `/health`: the server's version and build identity.
    Version(Option<String>, Option<String>),
    Parent(Option<String>),
    Peers(Source<Vec<PeerRemote>>),
    Machine(Source<Machine>),
    /// `GET /system` failed: the last sample stays, or `—` before the first.
    MachineFailed,
    /// A transport error or another status: the last value stays.
    Failed,
}

/// Sections that collapse with Enter or Space.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Section {
    Mcp,
    Files,
    Machine,
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
    /// Queued: its place in the queue, shown instead of the bar.
    pub queue_position: Option<u64>,
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
    /// The server's build identity from `/health`; `None` until it answered, `Some(None)`
    /// for an older server that reports none.
    pub server_build: Option<Option<String>>,
    pub parent_title: Option<String>,
    /// The server's enabled remotes; the Peers line shows only when there are some.
    pub peers: Source<Vec<PeerRemote>>,
    pub connection: Connection,
    /// The Workflows section's runs, the current Session's first; set before every frame.
    pub workflows: Vec<WorkflowRow>,
    /// Active runs beyond those rows (`+N more`).
    pub workflows_more: usize,
    /// The server machine's last sample.
    pub machine: Source<Machine>,
    /// The last `GET /system` failed.
    pub machine_failed: bool,
    /// When the next `GET /system` is due; `None` asks at the next tick.
    pub machine_due: Option<std::time::Instant>,
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
            Loaded::Version(version, build) => {
                self.version = version;
                self.server_build = Some(build);
            }
            Loaded::Parent(title) => self.parent_title = title,
            Loaded::Peers(source) => self.peers = source,
            Loaded::Machine(source) => {
                self.machine = source;
                self.machine_failed = false;
            }
            Loaded::MachineFailed => self.machine_failed = true,
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
        // The disk is the Location's volume.
        self.machine = Source::Pending;
        self.machine_failed = false;
        self.machine_due = None;
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
        // Shown whenever the MCP route answered, so an empty list says how to add one.
        if self.mcp.ready().is_some() {
            rows.push(Row::Header(Section::Mcp));
            if !self.collapsed.contains(&Section::Mcp) {
                let shown = self.servers().len().min(MCP_ROWS);
                rows.extend((0..shown).map(Row::Mcp));
            }
        }
        if self.machine_shown() {
            rows.push(Row::Header(Section::Machine));
        }
        if !self.changes().is_empty() {
            rows.push(Row::Header(Section::Files));
            if !self.collapsed.contains(&Section::Files) {
                rows.extend((0..self.changes().len().min(FILE_ROWS)).map(Row::File));
            }
        }
        rows
    }

    /// The Machine section is drawn: a sample, a failure before the first one, or an
    /// older server without the route.
    pub fn machine_shown(&self) -> bool {
        self.machine.ready().is_some()
            || self.machine_failed && self.machine == Source::Pending
            || self.server_stale() && self.machine == Source::Missing
    }

    /// The server answered `/health` with another build than this client's, so routes
    /// this client uses may be missing until it restarts.
    pub fn server_stale(&self) -> bool {
        self.server_build
            .as_ref()
            .is_some_and(|build| build.as_deref() != Some(cyber_core::version::build_info().build))
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
            transport: s["transport"].as_str().map(clean).filter(|t| !t.is_empty()),
            tools: s["tools"].as_array().map_or(0, Vec::len),
        })
        .collect()
}

/// `GET /peers/remotes` data.
pub fn parse_peers(data: &Value) -> Vec<PeerRemote> {
    data.as_array()
        .into_iter()
        .flatten()
        .map(|p| PeerRemote {
            name: clean(p["name"].as_str().unwrap_or_default()),
            online: p["online"] == true,
        })
        .filter(|p| !p.name.is_empty())
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

    /// `tui.machine_stats` is on.
    pub fn machine_stats(&self) -> bool {
        self.prefs.get("machine_stats") != "off"
    }

    /// `GET /system` when due: only while the sidebar is drawn, the terminal focused
    /// and `tui.machine_stats` on, at most every [`MACHINE_POLL`].
    pub fn machine_poll(&mut self) -> Vec<Action> {
        if !self.machine_stats() || !self.focused || !self.sidebar_visible() {
            return Vec::new();
        }
        let now = self.now();
        if self.sidebar.machine_due.is_some_and(|due| now < due) {
            return Vec::new();
        }
        self.sidebar.machine_due = Some(now + MACHINE_POLL);
        vec![Action::LoadSidebar(Part::Machine)]
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
    fn bars_fill_in_eighths_and_keep_their_width() {
        assert_eq!(bar_cells(0.0, 4), "    ");
        assert_eq!(bar_cells(100.0, 4), "████");
        assert_eq!(bar_cells(50.0, 4), "██  ");
        // 18% of 14 cells is 20.16 eighths, rounded to 20: two full cells and a half.
        assert_eq!(bar_cells(18.0, 14), format!("██▌{}", " ".repeat(11)));
        assert_eq!(bar_cells(20.0, 5), "█    ");
        assert_eq!(bar_cells(22.5, 5), "█▏   ");
        assert_eq!(bar_cells(1.0, 14).chars().next(), Some('▏'));
        assert_eq!(bar_cells(150.0, 3), "███");
        assert_eq!(bar_cells(-5.0, 3), "   ");
        for p in [0.0, 3.3, 18.0, 71.0, 93.0, 100.0] {
            assert_eq!(bar_cells(p, 14).chars().count(), 14, "{p}");
        }
        assert_eq!(bar_text(50.0, 10), "[#####.....]");
        assert_eq!(bar_text(100.0, 4), "[####]");
    }

    #[test]
    fn machine_replies_parse_and_read_as_one_sentence() {
        let gb = 1u64 << 30;
        let m = parse_machine(&json!({
            "hostname_label": "macOS\u{1b}[2J 15.0",
            "cores_logical": 12,
            "cpu_idle_percent": 82.0,
            "load_avg": [2.1, 1.8, 1.6],
            "memory_total": 16 * gb,
            "memory_available": 4 * gb,
            "disk": { "mount": "/", "total": 926 * gb, "available": 15 * gb },
        }));
        assert!(!m.label.as_deref().unwrap().contains('\u{1b}'));
        assert_eq!(m.load, Some([2.1, 1.8, 1.6]));
        assert_eq!(
            machine_sentence(&m),
            "CPU 18 percent busy, 12 cores; memory 75 percent used, 4.0 GB free; disk 98 percent used, 15.0 GB free"
        );
        let first = parse_machine(&json!({
            "cores_logical": 4, "cpu_idle_percent": null, "load_avg": null,
            "memory_total": 0, "memory_available": 0, "disk": null,
        }));
        assert_eq!(first.cpu_busy(), None);
        assert_eq!(first.disk, None);
        assert_eq!(machine_sentence(&first), "CPU measuring, 4 cores");
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

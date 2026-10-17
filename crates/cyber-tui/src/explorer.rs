//! The file explorer (`tui` → File explorer): the Location's project as a lazily loaded
//! tree from `GET /files`, a fuzzy filter, changed-only rows and a preview from
//! `GET /files/content`. It never bypasses permissions: rows carry the server's `read`
//! decision, a denied file is never requested, and an `ask` file's content arrives only
//! after the normal permission request is approved. Every name and line from the server
//! is untrusted and is drawn without control characters.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde_json::Value;

use crate::app::{Action, App, Overlay};
use crate::sidebar::clean;

/// Lines a preview shows at most.
pub const PREVIEW_LINES: usize = 5_000;
/// Below this width the preview replaces the tree instead of sitting beside it.
pub const SIDE_BY_SIDE: u16 = 100;

/// The server's `read` decision for a path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Read {
    Allow,
    Ask,
    Deny,
}

impl Read {
    fn parse(v: &Value) -> Self {
        match v.as_str() {
            Some("allow") => Self::Allow,
            Some("ask") => Self::Ask,
            _ => Self::Deny,
        }
    }
}

/// How a tree row's name is colored (`tui` → File explorer).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Dir,
    Doc,
    Config,
    Code,
    /// Hidden, ignored, lock and generated files.
    Dim,
    Plain,
}

const DOC: &[&str] = &["md", "mdx", "markdown", "txt", "rst", "adoc", "org"];
const CONFIG: &[&str] = &[
    "toml",
    "json",
    "jsonc",
    "json5",
    "yaml",
    "yml",
    "ini",
    "cfg",
    "conf",
    "xml",
    "plist",
    "properties",
    "editorconfig",
];
const CONFIG_NAMES: &[&str] = &[
    "justfile",
    "makefile",
    "dockerfile",
    "rakefile",
    "gemfile",
    "procfile",
    "brewfile",
];
const CODE: &[&str] = &[
    "rs", "ts", "tsx", "js", "jsx", "mjs", "cjs", "py", "go", "c", "h", "cc", "cpp", "hpp", "java",
    "kt", "swift", "rb", "php", "cs", "sh", "bash", "zsh", "fish", "ps1", "sql", "lua", "sol",
    "sv", "v", "vhd", "zig", "html", "css", "scss", "vue", "svelte", "dart", "ex", "exs", "hs",
    "ml", "scala", "r", "jl", "nix",
];
const LOCK_NAMES: &[&str] = &[
    "package-lock.json",
    "pnpm-lock.yaml",
    "yarn.lock",
    "go.sum",
    "composer.lock",
    "poetry.lock",
    "gemfile.lock",
    "flake.lock",
];
const GENERATED: &[&str] = &["lock", "snap", "map", "min.js", "min.css", "pyc", "o", "a"];

/// The kind of a row from its name, for the tree's colors.
pub fn kind(node: &Node) -> Kind {
    if node.ignored || node.name.starts_with('.') {
        return Kind::Dim;
    }
    if node.dir {
        return Kind::Dir;
    }
    let name = node.name.to_ascii_lowercase();
    let ext = name.split_once('.').map_or("", |(_, rest)| rest);
    let last = name.rsplit_once('.').map_or("", |(_, ext)| ext);
    let is = |list: &[&str], e: &str| !e.is_empty() && list.contains(&e);
    if LOCK_NAMES.contains(&name.as_str()) || is(GENERATED, last) || is(GENERATED, ext) {
        Kind::Dim
    } else if is(DOC, last) || matches!(name.as_str(), "license" | "readme" | "changelog") {
        Kind::Doc
    } else if is(CONFIG, last) || CONFIG_NAMES.contains(&name.as_str()) {
        Kind::Config
    } else if is(CODE, last) {
        Kind::Code
    } else {
        Kind::Plain
    }
}

/// `tui.explorer.icons`: plain names, or Nerd Font glyphs before them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Icons {
    #[default]
    None,
    Nerd,
}

impl Icons {
    pub fn parse(value: Option<&str>) -> Option<Self> {
        match value {
            Some("none") => Some(Self::None),
            Some("nerd") => Some(Self::Nerd),
            _ => None,
        }
    }
}

/// Whether `path` is Markdown, which the preview renders unless `m` asks for raw text.
pub fn is_markdown(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    [".md", ".mdx", ".markdown"]
        .iter()
        .any(|ext| lower.ends_with(ext))
}

#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub name: String,
    /// Project-relative, `/`-separated.
    pub path: String,
    pub dir: bool,
    /// A symlink's target.
    pub target: Option<String>,
    pub size: u64,
    /// `M`, `A`, `D` or `?`.
    pub git: Option<char>,
    pub ignored: bool,
    pub read: Read,
}

impl Node {
    fn parse(v: &Value) -> Option<Self> {
        let path = clean(v["path"].as_str()?);
        if path.is_empty() {
            return None;
        }
        let name = clean(v["name"].as_str().unwrap_or(&path));
        Some(Self {
            name,
            path,
            dir: v["kind"] == "directory",
            target: v["target"].as_str().map(clean),
            size: v["size"].as_u64().unwrap_or(0),
            git: v["git_status"]
                .as_str()
                .and_then(|s| s.chars().next())
                .filter(|c| matches!(c, 'M' | 'A' | 'D' | '?')),
            ignored: v["ignored"] == true,
            read: Read::parse(&v["read"]),
        })
    }

    /// A search hit: a file with only its path.
    fn hit(v: &Value) -> Option<Self> {
        let path = clean(v["path"].as_str()?);
        let name = path.rsplit('/').next().unwrap_or(&path).to_owned();
        Some(Self {
            name,
            path,
            dir: false,
            target: None,
            size: 0,
            git: None,
            ignored: false,
            read: Read::parse(&v["read"]),
        })
    }
}

/// A project-relative `path` as a mention: relative to the Session `directory`, as
/// `@` mentions resolve, or absolute (`/`-separated) when it lies outside it. Without the project root
/// (an older server) the path is used as given.
pub fn mention_path(root: Option<&str>, directory: &str, path: &str) -> String {
    let Some(root) = root else {
        return path.to_owned();
    };
    let full = std::path::Path::new(root).join(path);
    match full.strip_prefix(directory) {
        Ok(rel) if rel.as_os_str().is_empty() => ".".to_owned(),
        Ok(rel) => rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/"),
        // `/`-separated without the verbatim prefix, so the mention needs no quoting
        // and Windows still reads it.
        Err(_) => crate::sidebar::display_path(&full.display().to_string()).replace('\\', "/"),
    }
}

/// One loaded directory level.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Level {
    pub nodes: Vec<Node>,
    pub truncated: bool,
    /// Dot and ignored entries left out.
    pub hidden: usize,
    /// The canonical project root that node paths are relative to.
    pub root: Option<String>,
}

/// `GET /files` data, and the project's changed-file count.
pub fn parse_level(data: &Value) -> (Level, Option<usize>) {
    let level = Level {
        nodes: data["entries"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Node::parse)
            .collect(),
        truncated: data["truncated"] == true,
        hidden: data["hidden"].as_u64().unwrap_or(0) as usize,
        root: data["root"].as_str().map(str::to_owned),
    };
    (level, data["changed"].as_u64().map(|n| n as usize))
}

/// `GET /files/search` data.
pub fn parse_hits(data: &Value) -> Vec<Node> {
    data["results"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Node::hit)
        .collect()
}

/// A file's content as the preview draws it.
#[derive(Debug, Default)]
pub struct Preview {
    pub path: String,
    /// Lines without control characters, at most [`PREVIEW_LINES`]; `None` when binary.
    pub lines: Option<Vec<String>>,
    pub size: u64,
    pub language: Option<String>,
    /// The file's whole line count, as the server counted it.
    pub total_lines: usize,
    /// The server or the line cap cut the text.
    pub truncated: bool,
    pub scroll: u16,
    /// Highlighted lines and the theme they were colored for.
    pub(crate) highlighted: Mutex<Option<(String, Vec<crate::highlight::Styled>)>>,
    /// Rows the last frame drew the whole preview in (wrapping or rendering changes the
    /// count), so scrolling stops at the end.
    pub(crate) rows: AtomicUsize,
}

impl Preview {
    /// The last row scrolling may reach.
    fn max_scroll(&self) -> u16 {
        let lines = self.lines.as_ref().map_or(0, Vec::len);
        let rows = self.rows.load(Ordering::Relaxed).max(lines);
        u16::try_from(rows.saturating_sub(1)).unwrap_or(u16::MAX)
    }
}

impl Clone for Preview {
    fn clone(&self) -> Self {
        Self {
            path: self.path.clone(),
            lines: self.lines.clone(),
            size: self.size,
            language: self.language.clone(),
            total_lines: self.total_lines,
            truncated: self.truncated,
            scroll: self.scroll,
            highlighted: Mutex::new(None),
            rows: AtomicUsize::new(self.rows.load(Ordering::Relaxed)),
        }
    }
}

impl PartialEq for Preview {
    fn eq(&self, other: &Self) -> bool {
        (
            &self.path,
            &self.lines,
            self.size,
            &self.language,
            self.scroll,
        ) == (
            &other.path,
            &other.lines,
            other.size,
            &other.language,
            other.scroll,
        )
    }
}

/// One line for the terminal: tabs as spaces, no control characters.
fn printable(line: &str) -> String {
    line.chars()
        .flat_map(|c| match c {
            '\t' => vec![' '; 4],
            c if c.is_control() => Vec::new(),
            c => vec![c],
        })
        .collect()
}

/// `GET /files/content` data.
pub fn parse_preview(path: &str, data: &Value) -> Preview {
    let lines = data["text"].as_str().map(|text| {
        text.lines()
            .take(PREVIEW_LINES)
            .map(printable)
            .collect::<Vec<_>>()
    });
    let total_lines = data["lines"].as_u64().unwrap_or(0) as usize;
    Preview {
        path: path.to_owned(),
        truncated: data["truncated"] == true || total_lines > PREVIEW_LINES,
        lines,
        size: data["size"].as_u64().unwrap_or(0),
        language: data["language"].as_str().map(clean),
        total_lines,
        scroll: 0,
        highlighted: Mutex::new(None),
        rows: AtomicUsize::new(0),
    }
}

/// A visible tree row.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub node: Node,
    pub depth: usize,
    /// One `│ ` indent guide per level below the root, before the fold marker.
    pub guides: String,
}

#[derive(Debug, Default)]
pub struct Explorer {
    /// Loaded levels by directory path (`""` is the project root).
    pub levels: BTreeMap<String, Level>,
    pub expanded: BTreeSet<String>,
    pub selected: usize,
    /// `.`: dot and ignored entries shown.
    pub hidden: bool,
    /// `g`: only changed rows.
    pub changed_only: bool,
    pub filter: String,
    /// `/` was pressed and keys type into the filter.
    pub filtering: bool,
    /// Server search hits for the filter.
    pub hits: Vec<Node>,
    pub preview: Option<Preview>,
    /// Below [`SIDE_BY_SIDE`] columns the preview replaces the tree.
    pub preview_focused: bool,
    /// The path whose content is being fetched (perhaps waiting for an approval).
    pub awaiting: Option<String>,
    /// Changed files in the project.
    pub changed: Option<usize>,
    /// The canonical project root, from the last level loaded.
    pub root: Option<String>,
    /// `m`: Markdown previews show their source instead of the rendered document.
    pub raw: bool,
    pub icons: Icons,
}

impl Explorer {
    /// The visible rows: the tree, or the filter's matches.
    pub fn rows(&self) -> Vec<Row> {
        if !self.filter.is_empty() {
            return self.matches();
        }
        let mut out = Vec::new();
        self.walk("", 0, &mut out);
        out
    }

    /// Entries loaded for an expanded directory, shown beside its name.
    pub fn child_count(&self, path: &str) -> Option<usize> {
        self.expanded
            .contains(path)
            .then(|| self.levels.get(path).map(|l| l.nodes.len()))
            .flatten()
    }

    fn walk(&self, dir: &str, depth: usize, out: &mut Vec<Row>) {
        let Some(level) = self.levels.get(dir) else {
            return;
        };
        let nodes: Vec<&Node> = level
            .nodes
            .iter()
            .filter(|n| !self.changed_only || n.git.is_some())
            .collect();
        for node in nodes {
            out.push(Row {
                node: node.clone(),
                depth,
                guides: "│ ".repeat(depth),
            });
            if node.dir && self.expanded.contains(&node.path) {
                self.walk(&node.path, depth + 1, out);
            }
        }
    }

    /// Loaded files and search hits whose path matches the filter, best first.
    fn matches(&self) -> Vec<Row> {
        let mut seen = BTreeSet::new();
        let mut nodes: Vec<Node> = Vec::new();
        let loaded = self.levels.values().flat_map(|l| l.nodes.iter());
        for node in loaded.chain(self.hits.iter()) {
            if !node.dir && seen.insert(node.path.clone()) {
                nodes.push(node.clone());
            }
        }
        nodes.retain(|n| !self.changed_only || n.git.is_some());
        crate::fuzzy::rank(&self.filter, &nodes, |n| n.path.clone())
            .into_iter()
            .map(|i| Row {
                node: nodes[i].clone(),
                depth: 0,
                guides: String::new(),
            })
            .collect()
    }

    pub fn current(&self) -> Option<Row> {
        self.rows().into_iter().nth(self.selected)
    }

    /// Dot and ignored entries left out of the loaded levels.
    pub fn hidden_count(&self) -> usize {
        self.levels.values().map(|l| l.hidden).sum()
    }

    /// A loaded level arrived. Expansion and the selected path survive a refresh.
    pub fn apply_level(&mut self, path: String, level: Level, changed: Option<usize>) {
        let selected = self.current().map(|r| r.node.path);
        if level.root.is_some() {
            self.root.clone_from(&level.root);
        }
        self.levels.insert(path, level);
        if changed.is_some() {
            self.changed = changed;
        }
        if let Some(path) = selected
            && let Some(index) = self.rows().iter().position(|r| r.node.path == path)
        {
            self.selected = index;
        }
        self.clamp();
    }

    fn clamp(&mut self) {
        self.selected = self.selected.min(self.rows().len().saturating_sub(1));
    }

    /// Everything the explorer knew about another Location.
    pub fn forget(&mut self) {
        *self = Self {
            icons: self.icons,
            ..Self::default()
        };
    }

    /// Reads of every loaded level, for `r` and live refresh.
    pub fn reload(&self) -> Vec<Action> {
        self.levels
            .keys()
            .map(|path| Action::LoadDir {
                path: path.clone(),
                hidden: self.hidden,
            })
            .collect()
    }
}

impl App {
    /// `/files` or the `file_explorer` binding.
    pub fn open_explorer(&mut self) -> Vec<Action> {
        self.completion = None;
        self.overlay = Overlay::Files;
        let mut actions = vec![
            Action::LoadSidebar(crate::sidebar::Part::Vcs),
            Action::LoadSidebar(crate::sidebar::Part::Files),
        ];
        if self.explorer.levels.is_empty() {
            actions.push(Action::LoadDir {
                path: String::new(),
                hidden: self.explorer.hidden,
            });
        }
        actions
    }

    /// Live refresh on file and VCS changes, keeping the expansion state.
    pub(crate) fn refresh_explorer(&self) -> Vec<Action> {
        self.explorer.reload()
    }

    pub(crate) fn explorer_key(&mut self, key: KeyEvent) -> Vec<Action> {
        if self.explorer.filtering {
            return self.filter_key(key);
        }
        if self.explorer.preview_focused {
            return self.preview_key(key);
        }
        match self.keymap.action("explorer", &key, false) {
            Some(action) => self.explorer_action(action),
            None => {
                self.move_key(key.code);
                Vec::new()
            }
        }
    }

    fn explorer_action(&mut self, action: &str) -> Vec<Action> {
        let e = &mut self.explorer;
        match action {
            "explorer_close" if !e.filter.is_empty() => {
                e.filter.clear();
                e.hits.clear();
                e.selected = 0;
            }
            "explorer_close" => self.overlay = Overlay::None,
            "explorer_open" => return self.open_row(),
            "explorer_mention" => self.mention_row(),
            "explorer_copy" => return self.copy_row(),
            "explorer_refresh" => return e.reload(),
            "explorer_hidden" => {
                e.hidden = !e.hidden;
                return e.reload();
            }
            "explorer_filter" => e.filtering = true,
            "explorer_changed" => {
                e.changed_only = !e.changed_only;
                e.clamp();
            }
            "explorer_next" => self.move_key(KeyCode::Down),
            "explorer_previous" => self.move_key(KeyCode::Up),
            "explorer_markdown" => e.raw = !e.raw,
            "explorer_collapse" => self.collapse_row(),
            "explorer_expand" => return self.expand_row(),
            _ => {}
        }
        Vec::new()
    }

    /// `←`: fold an expanded directory, or move to the row's parent directory.
    fn collapse_row(&mut self) {
        let Some(row) = self.explorer.current() else {
            return;
        };
        let e = &mut self.explorer;
        if row.node.dir && e.expanded.remove(&row.node.path) {
            e.clamp();
            return;
        }
        let Some((parent, _)) = row.node.path.rsplit_once('/') else {
            return;
        };
        if let Some(index) = e.rows().iter().position(|r| r.node.path == parent) {
            e.selected = index;
        }
    }

    /// `→`: unfold a collapsed directory.
    fn expand_row(&mut self) -> Vec<Action> {
        match self.explorer.current() {
            Some(row) if row.node.dir && !self.explorer.expanded.contains(&row.node.path) => {
                self.open_row()
            }
            _ => Vec::new(),
        }
    }

    fn move_key(&mut self, code: KeyCode) {
        let e = &mut self.explorer;
        let last = e.rows().len().saturating_sub(1);
        match code {
            KeyCode::Up => e.selected = e.selected.saturating_sub(1),
            KeyCode::Down => e.selected = (e.selected + 1).min(last),
            KeyCode::Home => e.selected = 0,
            KeyCode::End => e.selected = last,
            KeyCode::PageUp | KeyCode::PageDown => self.scroll_preview(code),
            _ => {}
        }
    }

    fn scroll_preview(&mut self, code: KeyCode) {
        let Some(preview) = self.explorer.preview.as_mut() else {
            return;
        };
        let max = preview.max_scroll();
        preview.scroll = match code {
            KeyCode::Up => preview.scroll.saturating_sub(1),
            KeyCode::Down => preview.scroll.saturating_add(1),
            KeyCode::PageUp => preview.scroll.saturating_sub(20),
            KeyCode::PageDown => preview.scroll.saturating_add(20),
            KeyCode::Home => 0,
            _ => preview.scroll,
        }
        .min(max);
    }

    /// Keys while the preview fills a narrow terminal: scroll, Esc back to the tree.
    fn preview_key(&mut self, key: KeyEvent) -> Vec<Action> {
        match self.keymap.action("explorer", &key, false) {
            Some("explorer_close") => self.explorer.preview_focused = false,
            Some("explorer_markdown") => self.explorer.raw = !self.explorer.raw,
            Some("explorer_mention") => self.mention_row(),
            Some("explorer_copy") => return self.copy_row(),
            _ => self.scroll_preview(key.code),
        }
        Vec::new()
    }

    fn filter_key(&mut self, key: KeyEvent) -> Vec<Action> {
        let e = &mut self.explorer;
        match key.code {
            KeyCode::Esc => {
                e.filtering = false;
                e.filter.clear();
                e.hits.clear();
            }
            KeyCode::Enter | KeyCode::Down => e.filtering = false,
            KeyCode::Backspace => {
                e.filter.pop();
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => e.filter.push(c),
            _ => return Vec::new(),
        }
        e.selected = 0;
        if e.filter.is_empty() || !e.filtering {
            return Vec::new();
        }
        vec![Action::SearchFiles(e.filter.clone())]
    }

    /// Enter: fold or unfold a directory, or open a file's preview.
    fn open_row(&mut self) -> Vec<Action> {
        let Some(row) = self.explorer.current() else {
            return Vec::new();
        };
        let node = row.node;
        if node.dir {
            let e = &mut self.explorer;
            if !e.expanded.remove(&node.path) {
                e.expanded.insert(node.path.clone());
                if !e.levels.contains_key(&node.path) {
                    return vec![Action::LoadDir {
                        path: node.path,
                        hidden: e.hidden,
                    }];
                }
            }
            e.clamp();
            return Vec::new();
        }
        if node.read == Read::Deny {
            self.toast(format!("⊘ {}: reading is denied", node.path));
            return Vec::new();
        }
        self.explorer.awaiting = Some(node.path.clone());
        vec![Action::LoadPreview(node.path)]
    }

    /// `@`: the path as a file mention in the composer; the explorer closes.
    fn mention_row(&mut self) {
        let Some(path) = self.explorer_path() else {
            return;
        };
        let path = mention_path(
            self.explorer.root.as_deref(),
            &self.session.directory,
            &path,
        );
        let mention = if path
            .chars()
            .any(|c| c.is_whitespace() || matches!(c, '"' | '\\'))
        {
            serde_json::to_string(&path).expect("string serialization")
        } else {
            path
        };
        let text = self.composer.text();
        let space = if text.is_empty() || text.ends_with(char::is_whitespace) {
            ""
        } else {
            " "
        };
        self.composer.set_text(&format!("{text}{space}@{mention} "));
        self.overlay = Overlay::None;
    }

    fn copy_row(&mut self) -> Vec<Action> {
        self.explorer_path()
            .map(|path| vec![Action::Copy(path)])
            .unwrap_or_default()
    }

    /// The previewed path while the preview has focus, else the selected row's.
    fn explorer_path(&self) -> Option<String> {
        if self.explorer.preview_focused {
            return self.explorer.preview.as_ref().map(|p| p.path.clone());
        }
        self.explorer.current().map(|r| r.node.path)
    }

    /// A preview arrived (or was refused) for `path`.
    pub(crate) fn apply_preview(&mut self, path: &str, result: Result<Preview, String>) {
        if self.explorer.awaiting.as_deref() != Some(path) {
            return;
        }
        self.explorer.awaiting = None;
        match result {
            Ok(preview) => {
                self.explorer.preview = Some(preview);
                self.explorer.preview_focused = self.frame_width() < SIDE_BY_SIDE;
            }
            Err(error) => self.toast(format!("⊘ {path}: {error}")),
        }
        if matches!(self.overlay, Overlay::None) && self.active_request().is_none() {
            self.overlay = Overlay::Files;
        }
    }

    /// The width the last frame was drawn at.
    pub fn frame_width(&self) -> u16 {
        self.frame_width.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// The Session diff's changed new-side lines of `path`, and its `+N −M`.
    pub fn changed_lines(&self, path: &str) -> Option<(BTreeSet<usize>, u64, u64)> {
        let change = self.sidebar.changes().iter().find(|c| c.file == path)?;
        Some((
            new_side_lines(&change.patch),
            change.additions,
            change.deletions,
        ))
    }
}

/// Line numbers (1-based) a unified diff adds or changes on its new side.
pub fn new_side_lines(patch: &str) -> BTreeSet<usize> {
    let mut out = BTreeSet::new();
    let mut line = 0usize;
    for text in patch.lines() {
        if let Some(header) = text.strip_prefix("@@") {
            line = hunk_start(header).unwrap_or(1);
            continue;
        }
        match text.chars().next() {
            Some('+') if !text.starts_with("+++") => {
                out.insert(line);
                line += 1;
            }
            Some('-') => {}
            Some(' ') => line += 1,
            _ => {}
        }
    }
    out
}

/// The new side's first line of a hunk header (` -1,2 +3,4 @@`).
fn hunk_start(header: &str) -> Option<usize> {
    let plus = header.split_whitespace().find(|w| w.starts_with('+'))?;
    plus[1..].split(',').next()?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn node(path: &str, dir: bool, git: Option<char>) -> Value {
        json!({ "name": path.rsplit('/').next().unwrap(), "path": path,
            "kind": if dir { "directory" } else { "file" }, "size": 1,
            "git_status": git.map(String::from), "ignored": false, "read": "allow" })
    }

    #[test]
    fn the_tree_draws_guides_keeps_expansion_on_refresh_and_filters() {
        let mut e = Explorer::default();
        let root = json!({ "entries": [node("crates", true, Some('M')), node("README.md", false, None)], "hidden": 2, "changed": 1 });
        let (level, changed) = parse_level(&root);
        e.apply_level(String::new(), level, changed);
        e.expanded.insert("crates".into());
        let crates = json!({ "entries": [node("crates/a.rs", false, Some('M')), node("crates/b.rs", false, None)] });
        e.apply_level("crates".into(), parse_level(&crates).0, None);
        let guides: Vec<_> = e
            .rows()
            .iter()
            .map(|r| format!("{}{}", r.guides, r.node.name))
            .collect();
        assert_eq!(guides, ["crates", "│ a.rs", "│ b.rs", "README.md"]);
        assert_eq!(e.child_count("crates"), Some(2));
        assert_eq!(e.child_count("README.md"), None);
        e.selected = 2;
        // A refresh of the root keeps `crates` expanded and the selection on b.rs.
        e.apply_level(String::new(), parse_level(&root).0, Some(1));
        assert!(e.expanded.contains("crates"));
        assert_eq!(e.current().unwrap().node.path, "crates/b.rs");
        assert_eq!((e.changed, e.hidden_count()), (Some(1), 2));
        e.changed_only = true;
        let changed: Vec<_> = e.rows().iter().map(|r| r.node.name.clone()).collect();
        assert_eq!(changed, ["crates", "a.rs"]);
        e.changed_only = false;
        e.filter = "rdme".into();
        assert_eq!(e.rows()[0].node.path, "README.md");
    }

    #[test]
    fn rows_are_classified_by_name_for_their_colors() {
        let file = |name: &str| Node {
            name: name.into(),
            path: name.into(),
            dir: false,
            target: None,
            size: 0,
            git: None,
            ignored: false,
            read: Read::Allow,
        };
        for (name, want) in [
            ("README.md", Kind::Doc),
            ("notes.TXT", Kind::Doc),
            ("LICENSE", Kind::Doc),
            ("Cargo.toml", Kind::Config),
            ("tsconfig.json", Kind::Config),
            ("justfile", Kind::Config),
            ("Dockerfile", Kind::Config),
            ("main.rs", Kind::Code),
            ("app.tsx", Kind::Code),
            ("build.sh", Kind::Code),
            ("Cargo.lock", Kind::Dim),
            ("package-lock.json", Kind::Dim),
            ("bundle.min.js", Kind::Dim),
            ("view__a.snap", Kind::Dim),
            (".env", Kind::Dim),
            ("data.bin", Kind::Plain),
        ] {
            assert_eq!(kind(&file(name)), want, "{name}");
        }
        let mut dir = file("src");
        dir.dir = true;
        assert_eq!(kind(&dir), Kind::Dir);
        dir.ignored = true;
        assert_eq!(kind(&dir), Kind::Dim);
        assert!(is_markdown("docs/README.md") && !is_markdown("src/md.rs"));
    }

    #[test]
    fn untrusted_names_and_lines_lose_control_characters() {
        let (level, _) =
            parse_level(&json!({ "entries": [node("ev\u{1b}]52;c;x\u{7}il", false, None)] }));
        assert_eq!(level.nodes[0].name, "ev]52;c;xil");
        let preview = parse_preview(
            "a",
            &json!({ "text": "a\u{1b}[2J\tb\nc", "lines": 2, "size": 9 }),
        );
        assert_eq!(preview.lines.unwrap(), ["a[2J    b", "c"]);
    }

    #[test]
    fn diff_hunks_give_the_changed_new_side_lines() {
        let patch = "--- a/x\n+++ b/x\n@@ -1,3 +1,4 @@\n one\n-two\n+TWO\n+three\n four\n@@ -10 +11 @@\n-x\n+y\n";
        assert_eq!(new_side_lines(patch), BTreeSet::from([2, 3, 11]));
    }
}

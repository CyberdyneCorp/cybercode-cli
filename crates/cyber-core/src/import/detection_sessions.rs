//! Session counts and read-time coverage for `cyber import --detect`
//! (`compat-import` → Source detection). Session files are counted, never parsed beyond
//! the first line of a Codex rollout, and no transcript content is reported.
use super::{SourceFile, SourceKind, SourceRoots, SourceTool};
use serde::Serialize;
use std::io::{BufRead, Read};
use std::path::{Path, PathBuf};

/// Directory entries inspected per tool before the count is reported unknown.
const ENTRY_LIMIT: usize = 20_000;
/// Bytes of a Codex rollout's first line read to find its working directory.
const FIRST_LINE_LIMIT: u64 = 64 * 1024;

/// How many of a tool's items Cyber Code already uses without an import.
#[derive(Debug, Default, Clone, Serialize)]
pub struct ReadTimeCoverage {
    pub agents: usize,
    pub commands: usize,
    pub skills: usize,
    pub mcp_servers: usize,
    pub hooks: usize,
    pub sessions: usize,
}

/// Coverage of the discovered files under the runtime's own default discovery: skills
/// in its skill roots, Claude commands in its command roots (and, counted by the caller,
/// the project root's `.mcp.json` servers). Agents, hooks and sessions of other tools are
/// never read at runtime.
pub(super) fn read_time(roots: &SourceRoots, files: &[SourceFile]) -> ReadTimeCoverage {
    let skill_roots = skill_roots(roots);
    let mut coverage = ReadTimeCoverage::default();
    for file in files {
        match file.kind {
            SourceKind::Skill if under_any(&file.path, &skill_roots) => coverage.skills += 1,
            SourceKind::Command
                if file.tool == SourceTool::Claude && claude_command(&file.path) =>
            {
                coverage.commands += 1
            }
            _ => (),
        }
    }
    coverage
}

/// The runtime's default skill folders for the import's Location.
pub(super) fn skill_roots(roots: &SourceRoots) -> Vec<PathBuf> {
    let scope = crate::skills::SkillScope {
        location: roots.directory.clone(),
        home: roots.home.clone(),
        global_config_dir: roots.home.join(".config/cyber"),
        compat: crate::skills::SkillCompat::ALL,
        extra: Vec::new(),
        plugins: Vec::new(),
    };
    crate::skills::roots(&scope)
        .into_iter()
        .map(|(root, _)| root)
        .collect()
}

/// Whether the runtime reads this instruction file in place: an `AGENTS.md`,
/// `AGENTS.override.md` or `CLAUDE.md` directly in a directory of the project.
pub(super) fn instruction_covered(roots: &SourceRoots, path: &Path) -> bool {
    let project = roots
        .project_root
        .canonicalize()
        .unwrap_or_else(|_| roots.project_root.clone());
    let named = path.file_name().is_some_and(|name| {
        ["AGENTS.md", "AGENTS.override.md", "CLAUDE.md"]
            .iter()
            .any(|file| name == *file)
    });
    let directory = path.parent().unwrap_or(path);
    let directory = directory
        .canonicalize()
        .unwrap_or_else(|_| directory.to_path_buf());
    // Directly in a project directory, not inside `.claude/`, `.codex/` or another
    // hidden tool folder.
    directory.strip_prefix(&project).is_ok_and(|inside| {
        !inside
            .components()
            .any(|c| c.as_os_str().to_string_lossy().starts_with('.'))
    }) && named
}

/// The project root's `.mcp.json`, which the runtime loads below `cyber.jsonc`.
pub(super) fn project_mcp_file(roots: &SourceRoots, path: &Path) -> bool {
    let project = roots
        .project_root
        .canonicalize()
        .unwrap_or_else(|_| roots.project_root.clone());
    path.file_name().is_some_and(|name| name == ".mcp.json")
        && path
            .parent()
            .and_then(|dir| dir.canonicalize().ok())
            .is_some_and(|dir| dir == project)
}

pub(super) fn under_any(path: &Path, roots: &[PathBuf]) -> bool {
    let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    roots.iter().any(|root| path.starts_with(root))
}

fn claude_command(path: &Path) -> bool {
    let names: Vec<_> = path.components().map(|c| c.as_os_str()).collect();
    names
        .windows(2)
        .any(|pair| pair[0] == ".claude" && pair[1] == "commands")
}

/// Sessions the tool recorded for this project; `None` when they cannot be counted (an
/// OpenCode SQLite store, or more entries than the bound).
pub(super) fn sessions(roots: &SourceRoots, tool: SourceTool) -> Option<usize> {
    match tool {
        SourceTool::Claude => claude_sessions(roots),
        SourceTool::Codex => codex_sessions(roots),
        SourceTool::OpenCode => opencode_sessions(roots),
    }
}

/// `~/.claude/projects/<project path with every non-alphanumeric character as '-'>`.
fn claude_sessions(roots: &SourceRoots) -> Option<usize> {
    let project = plain(&roots.project_root);
    let encoded: String = project
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let dir = roots.home.join(".claude/projects").join(encoded);
    let mut count = 0;
    let mut budget = ENTRY_LIMIT;
    for path in files(&dir, &mut budget)? {
        count += usize::from(path.extension().is_some_and(|e| e == "jsonl"));
    }
    Some(count)
}

/// `$CODEX_HOME/sessions/YYYY/MM/DD/rollout-*.jsonl` whose `session_meta` names a working
/// directory inside the project.
fn codex_sessions(roots: &SourceRoots) -> Option<usize> {
    let home = roots
        .codex_home
        .clone()
        .unwrap_or_else(|| roots.home.join(".codex"));
    let mut budget = ENTRY_LIMIT;
    let mut count = 0;
    let mut level = vec![home.join("sessions")];
    for _ in 0..3 {
        let mut next = Vec::new();
        for dir in &level {
            next.extend(directories(dir, &mut budget)?);
        }
        level = next;
    }
    for dir in &level {
        for path in files(dir, &mut budget)? {
            let rollout = path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("rollout-") && n.ends_with(".jsonl"));
            count += usize::from(rollout && codex_cwd_inside(&path, &roots.project_root));
        }
    }
    Some(count)
}

fn codex_cwd_inside(path: &Path, project: &Path) -> bool {
    let Ok(file) = std::fs::File::open(path) else {
        return false;
    };
    let mut line = String::new();
    if std::io::BufReader::new(file.take(FIRST_LINE_LIMIT))
        .read_line(&mut line)
        .is_err()
    {
        return false;
    }
    let Ok(meta) = serde_json::from_str::<serde_json::Value>(&line) else {
        return false;
    };
    meta["payload"]["cwd"]
        .as_str()
        .is_some_and(|cwd| Path::new(cwd).starts_with(plain(project)))
}

/// `~/.local/share/opencode/storage/session/<project>/<session>.json` whose `directory`
/// is inside the project. The SQLite store of newer OpenCode releases is not inspected.
fn opencode_sessions(roots: &SourceRoots) -> Option<usize> {
    let data = roots.home.join(".local/share/opencode");
    if data.join("opencode.db").exists() {
        return None;
    }
    let mut budget = ENTRY_LIMIT;
    let mut count = 0;
    for dir in directories(&data.join("storage/session"), &mut budget)? {
        for path in files(&dir, &mut budget)? {
            count += usize::from(opencode_session_inside(&path, &roots.project_root));
        }
    }
    Some(count)
}

fn opencode_session_inside(path: &Path, project: &Path) -> bool {
    if path.extension().is_none_or(|e| e != "json") {
        return false;
    }
    let Ok(text) = crate::files::read_small(path, FIRST_LINE_LIMIT) else {
        return false;
    };
    serde_json::from_str::<serde_json::Value>(&text).is_ok_and(|session| {
        session["directory"]
            .as_str()
            .is_some_and(|dir| Path::new(dir).starts_with(plain(project)))
    })
}

/// A path without the Windows verbatim prefix `canonicalize` adds.
fn plain(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    PathBuf::from(text.strip_prefix(r"\\?\").unwrap_or(&text))
}

/// Directories and regular files of `dir`, never links, as (path, is directory); an
/// absent directory is empty, and `None` means the entry budget ran out or the directory
/// is unreadable.
fn entries(dir: &Path, budget: &mut usize) -> Option<Vec<(PathBuf, bool)>> {
    let read = match std::fs::read_dir(dir) {
        Ok(read) => read,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Some(Vec::new()),
        Err(_) => return None,
    };
    let mut out = Vec::new();
    for entry in read {
        *budget = budget.checked_sub(1)?;
        let entry = entry.ok()?;
        let meta = std::fs::symlink_metadata(entry.path()).ok()?;
        if super::discovery::is_link(&meta) || !(meta.is_dir() || meta.is_file()) {
            continue;
        }
        out.push((entry.path(), meta.is_dir()));
    }
    Some(out)
}

fn directories(dir: &Path, budget: &mut usize) -> Option<Vec<PathBuf>> {
    Some(
        entries(dir, budget)?
            .into_iter()
            .filter_map(|(path, is_dir)| is_dir.then_some(path))
            .collect(),
    )
}

fn files(dir: &Path, budget: &mut usize) -> Option<Vec<PathBuf>> {
    Some(
        entries(dir, budget)?
            .into_iter()
            .filter_map(|(path, is_dir)| (!is_dir).then_some(path))
            .collect(),
    )
}

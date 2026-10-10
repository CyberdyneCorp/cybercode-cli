//! Bounded command discovery with handle-fenced traversal and independent file verification.
use super::{Commands, StaticCommand, invocation_name, markdown, valid_name};
use crate::import::{SourceSnapshot, command_directory, verify_command_directory};
use cap_fs_ext::DirExt;
use cap_std::fs::Dir;
use serde_json::Value;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

pub struct CommandScope {
    pub location: PathBuf,
    pub home: PathBuf,
    pub global_config_dir: PathBuf,
    pub project: bool,
    pub compat: bool,
}

#[derive(Debug, Clone)]
pub struct CommandIssue {
    pub path: PathBuf,
    pub reason: &'static str,
}

type Priority = (u8, usize, u8);
struct Candidate {
    priority: Priority,
    command: Option<StaticCommand>,
}
struct Scanner {
    candidates: BTreeMap<String, Candidate>,
    issues: Vec<CommandIssue>,
    entries: usize,
    remaining: usize,
    failed: bool,
}

impl Scanner {
    fn issue(&mut self, path: &Path, reason: &'static str) {
        self.issues.push(CommandIssue {
            path: path.into(),
            reason,
        });
    }

    fn put(&mut self, name: String, priority: Priority, command: Option<StaticCommand>) {
        if self
            .candidates
            .get(&name)
            .is_none_or(|old| old.priority <= priority)
        {
            self.candidates
                .insert(name, Candidate { priority, command });
        }
    }

    fn root(&mut self, root: &Path, priority: Priority) {
        if self.failed {
            return;
        }
        match std::fs::symlink_metadata(root) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
            _ => (),
        }
        match command_directory(root) {
            Ok(chain) => self.tree(root, root, chain.last().unwrap(), 0, priority),
            Err(_) => {
                self.issue(root, "command directory is unavailable or linked");
                self.failed = true;
            }
        }
    }

    fn tree(&mut self, root: &Path, path: &Path, dir: &Dir, depth: usize, priority: Priority) {
        let Ok(entries) = dir.entries() else {
            self.issue(path, "command directory listing is unavailable");
            self.failed = true;
            return;
        };
        let mut names = Vec::new();
        for entry in entries {
            self.entries += 1;
            if self.entries > 4096 {
                self.issue(path, "command directory entry bound exceeded");
                self.failed = true;
                return;
            }
            let Ok(entry) = entry else {
                self.issue(path, "command directory entry is unavailable");
                self.failed = true;
                return;
            };
            names.push(entry.file_name());
        }
        names.sort();
        for name in names {
            if self.failed {
                return;
            }
            let child = path.join(&name);
            let Ok(metadata) = dir.symlink_metadata(&name) else {
                self.issue(&child, "command entry metadata is unavailable");
                self.failed = true;
                return;
            };
            if metadata.file_type().is_symlink() {
                self.issue(&child, "linked command entry is unsupported");
                self.failed = true;
                return;
            }
            if metadata.is_dir() {
                self.child(root, &child, dir, &name, depth, priority);
            } else if child.extension().is_some_and(|e| e == "md") {
                self.file(root, &child, priority);
            }
        }
    }

    fn child(
        &mut self,
        root: &Path,
        path: &Path,
        parent: &Dir,
        name: &std::ffi::OsStr,
        depth: usize,
        priority: Priority,
    ) {
        if depth >= 32 {
            self.issue(path, "command directory depth bound exceeded");
            self.failed = true;
            return;
        }
        let Ok(dir) = parent.open_dir_nofollow(name) else {
            self.issue(path, "command child directory is unavailable or linked");
            self.failed = true;
            return;
        };
        // The root chain and recursive parents stay pinned; inspect the actual child handle.
        match verify_command_directory(&dir, path) {
            Ok(()) => self.tree(root, path, &dir, depth + 1, priority),
            Err(_) => {
                self.issue(
                    path,
                    "command child directory identity is unavailable or linked",
                );
                self.failed = true;
            }
        }
    }

    fn file(&mut self, root: &Path, path: &Path, priority: Priority) {
        let name = path
            .strip_prefix(root)
            .ok()
            .map(|p| p.with_extension(""))
            .and_then(|p| {
                p.components()
                    .map(|c| c.as_os_str().to_str().map(str::to_owned))
                    .collect::<Option<Vec<_>>>()
            })
            .map(|parts| parts.join("/"));
        let Some(name) = name.filter(|name| valid_name(name)) else {
            self.issue(path, "command name is unsupported");
            return;
        };
        let command = self.read(path);
        self.put(invocation_name(&name), priority, command);
    }

    fn read(&mut self, path: &Path) -> Option<StaticCommand> {
        let snapshot = match SourceSnapshot::read_command(path) {
            Ok(snapshot) => snapshot,
            Err(_) => {
                self.issue(
                    path,
                    "command file cannot be read as an unlinked regular file",
                );
                return None;
            }
        };
        let Some(remaining) = self.remaining.checked_sub(snapshot.bytes().len()) else {
            self.issue(path, "command document aggregate byte bound exceeded");
            self.failed = true;
            return None;
        };
        self.remaining = remaining;
        let command = snapshot
            .text()
            .ok()
            .and_then(|text| markdown(text).ok())
            .and_then(|document| StaticCommand::parse(&document.definition).ok());
        if snapshot.verify().is_err() || command.is_none() {
            self.issue(
                path,
                "command definition is unavailable, changed or needs additional behavior",
            );
            return None;
        }
        command
    }
}

fn canonical(path: &Path, scanner: &mut Scanner) -> Option<PathBuf> {
    match path.canonicalize() {
        Ok(path) => Some(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(_) => {
            scanner.issue(path, "command scope is unavailable");
            scanner.failed = true;
            None
        }
    }
}

fn inline_priority(
    name: &str,
    sources: &BTreeMap<String, String>,
    project: &Path,
    global: Option<&Path>,
) -> Priority {
    let pointer = format!("/commands/{}", name.replace('~', "~0").replace('/', "~1"));
    let prefix = format!("{pointer}/");
    sources
        .iter()
        .filter(|(key, _)| *key == &pointer || key.starts_with(&prefix))
        .map(|(_, source)| {
            let path = Path::new(source);
            let path = path.canonicalize().unwrap_or_else(|_| path.into());
            if global.is_some_and(|g| path.starts_with(g)) {
                return (0, 0, 2);
            }
            if path.starts_with(project) {
                let parent = path.parent().unwrap_or(project);
                let parent = if parent.file_name().is_some_and(|n| n == ".cyber") {
                    parent.parent().unwrap_or(parent)
                } else {
                    parent
                };
                return (1, parent.components().count(), 2);
            }
            (2, 0, 2)
        })
        .max()
        .unwrap_or((2, 0, 2))
}

fn global_sources(scanner: &mut Scanner, scope: &CommandScope) -> Option<PathBuf> {
    let global = canonical(&scope.global_config_dir, scanner);
    if scope.compat
        && let Some(home) = canonical(&scope.home, scanner)
    {
        scanner.root(&home.join(".claude/commands"), (0, 0, 1));
    }
    if let Some(global) = &global {
        for folder in ["command", "commands"] {
            scanner.root(&global.join(folder), (0, 0, 3));
        }
    }
    global
}

fn project_sources(scanner: &mut Scanner, scope: &CommandScope, location: &Path, project: &Path) {
    if !scope.project {
        return;
    }
    {
        let mut directories: Vec<_> = location
            .ancestors()
            .take_while(|p| p.starts_with(project))
            .collect();
        directories.reverse();
        if directories.len() > 1024 {
            scanner.issue(location, "command source root bound exceeded");
            scanner.failed = true;
        }
        for dir in directories.into_iter().take(1024) {
            let depth = dir.components().count();
            if scope.compat {
                scanner.root(&dir.join(".claude/commands"), (1, depth, 1));
            }
            for folder in ["command", "commands"] {
                scanner.root(&dir.join(".cyber").join(folder), (1, depth, 3));
            }
        }
    }
}

fn inline_sources(
    scanner: &mut Scanner,
    scope: &CommandScope,
    config: &Value,
    sources: &BTreeMap<String, String>,
    project: &Path,
    global: Option<&Path>,
) {
    if let Some(commands) = config.get("commands") {
        if let Some(commands) = commands.as_object().filter(|m| m.len() <= 128) {
            for (name, value) in commands {
                if !valid_name(name) {
                    scanner.issue(&scope.location, "configured command name is unsupported");
                    continue;
                }
                let priority = inline_priority(name, sources, project, global);
                if scope.project || priority.0 != 1 {
                    scanner.put(
                        invocation_name(name),
                        priority,
                        StaticCommand::parse(value).ok(),
                    );
                }
            }
        } else {
            scanner.issue(
                &scope.location,
                "configured command registry is invalid or exceeds its bound",
            );
            scanner.failed = true;
        }
    }
}

/// Inputs must be the runtime's already layered, substituted and trust-filtered configuration.
/// Files are observed independently; returned definitions own no filesystem handles.
pub fn discover(
    scope: &CommandScope,
    config: &Value,
    sources: &BTreeMap<String, String>,
) -> Commands {
    let mut scanner = Scanner {
        candidates: BTreeMap::new(),
        issues: Vec::new(),
        entries: 0,
        remaining: 16 * 1024 * 1024,
        failed: false,
    };
    let Some(location) = canonical(&scope.location, &mut scanner) else {
        return Commands {
            issues: scanner.issues,
            ..Commands::default()
        };
    };
    let project = crate::config::project_root(&location);
    let global = global_sources(&mut scanner, scope);
    project_sources(&mut scanner, scope, &location, &project);
    inline_sources(
        &mut scanner,
        scope,
        config,
        sources,
        &project,
        global.as_deref(),
    );
    let mut result = Commands {
        issues: scanner.issues,
        ..Commands::default()
    };
    if scanner.failed || scanner.candidates.len() > 128 {
        result.issues.push(CommandIssue {
            path: location,
            reason: "command registry is incomplete or exceeds its bound",
        });
        result.unavailable.extend(scanner.candidates.into_keys());
        return result;
    }
    for (name, candidate) in scanner.candidates {
        match candidate.command {
            Some(command) => {
                result.entries.insert(name, command);
            }
            None => result.unavailable.push(name),
        }
    }
    result
}

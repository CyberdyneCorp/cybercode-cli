//! Bounded skill discovery: every directory is opened without following links and every
//! `SKILL.md` is read through the same fenced, size-capped snapshot as command files.
use super::{Discovery, Skill, SkillDiagnostic, SkillOrigin, SkillSource, parse};
use crate::import::{SourceSnapshot, command_directory, verify_command_directory};
use cap_fs_ext::DirExt;
use cap_std::fs::Dir;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Entries examined across all roots of one discovery.
const ENTRY_LIMIT: usize = 4096;

pub(super) struct Scanner {
    pub(super) out: Discovery,
    entries: usize,
}

enum Probe {
    /// No `SKILL.md`: not a skill directory.
    Missing,
    Regular,
    Refused(&'static str),
}

impl Scanner {
    pub(super) fn new(out: Discovery) -> Self {
        Self { out, entries: 0 }
    }

    fn issue(&mut self, path: &Path, origin: SkillOrigin, message: impl Into<String>) {
        self.out.diagnostics.push(SkillDiagnostic {
            path: path.display().to_string(),
            origin,
            replaces: None,
            message: message.into(),
        });
    }

    /// A root that is itself a skill directory, or the skill directories directly inside it.
    pub(super) fn root(&mut self, root: &Path, origin: SkillOrigin) {
        match std::fs::symlink_metadata(root) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
            Ok(metadata) if metadata.is_file() => return,
            _ => (),
        }
        let Ok(chain) = command_directory(root) else {
            self.issue(root, origin, "skill directory is unavailable or linked");
            return;
        };
        let dir = chain.last().expect("a directory chain ends at its target");
        match probe(dir) {
            Probe::Regular => return self.load(root, origin),
            Probe::Refused(reason) => return self.issue(&root.join("SKILL.md"), origin, reason),
            Probe::Missing => (),
        }
        let Some(names) = self.names(root, dir, origin) else {
            return;
        };
        for name in names {
            self.child(root, dir, &name, origin);
        }
    }

    fn names(&mut self, root: &Path, dir: &Dir, origin: SkillOrigin) -> Option<Vec<OsString>> {
        let Ok(entries) = dir.entries() else {
            self.issue(root, origin, "skill directory listing is unavailable");
            return None;
        };
        let mut names = Vec::new();
        for entry in entries {
            self.entries += 1;
            if self.entries > ENTRY_LIMIT {
                self.issue(root, origin, "skill directory entry bound exceeded");
                return None;
            }
            if let Ok(entry) = entry {
                names.push(entry.file_name());
            }
        }
        names.sort();
        Some(names)
    }

    fn child(&mut self, root: &Path, parent: &Dir, name: &OsString, origin: SkillOrigin) {
        let path = root.join(name);
        let Ok(metadata) = parent.symlink_metadata(name) else {
            return;
        };
        if metadata.file_type().is_symlink() {
            // Only report links that would have looked like skill directories; never read them.
            if std::fs::metadata(&path).is_ok_and(|target| target.is_dir()) {
                self.issue(&path, origin, "linked skill directory is unsupported");
            }
            return;
        }
        if !metadata.is_dir() {
            return;
        }
        let opened = parent
            .open_dir_nofollow(name)
            .ok()
            .filter(|dir| verify_command_directory(dir, &path).is_ok());
        let Some(dir) = opened else {
            self.issue(&path, origin, "skill directory is unavailable or linked");
            return;
        };
        match probe(&dir) {
            Probe::Regular => self.load(&path, origin),
            Probe::Refused(reason) => self.issue(&path.join("SKILL.md"), origin, reason),
            Probe::Missing => (),
        }
    }

    fn load(&mut self, dir: &Path, origin: SkillOrigin) {
        match read(dir) {
            Ok(mut skill) => {
                skill.origin = origin;
                self.insert(skill);
            }
            Err(reason) => self.issue(&dir.join("SKILL.md"), origin, reason),
        }
    }

    fn insert(&mut self, skill: Skill) {
        let (name, path, origin) = (skill.name.clone(), skill.source_label(), skill.origin);
        if let Some(previous) = self.out.skills.insert(name.clone(), skill) {
            self.out.diagnostics.push(SkillDiagnostic {
                message: format!(
                    "duplicate skill: replaces skill {name} from {}",
                    previous.source_label()
                ),
                path,
                origin,
                replaces: Some(name),
            });
        }
    }
}

fn probe(dir: &Dir) -> Probe {
    match dir.symlink_metadata("SKILL.md") {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Probe::Missing,
        Err(_) => Probe::Refused("SKILL.md metadata is unavailable"),
        Ok(metadata) if metadata.file_type().is_symlink() => {
            Probe::Refused("linked SKILL.md is unsupported")
        }
        Ok(metadata) if metadata.is_file() => Probe::Regular,
        Ok(_) => Probe::Refused("SKILL.md is not a regular file"),
    }
}

/// Read `<dir>/SKILL.md` as an unlinked regular file of at most 1 MiB, then parse it.
pub(super) fn read(dir: &Path) -> Result<Skill, String> {
    let path = dir.join("SKILL.md");
    let snapshot = SourceSnapshot::read_command(&path).map_err(|e| e.reason.to_string())?;
    let text = snapshot.text().map_err(|e| e.reason.to_string())?;
    let skill = parse(text, SkillSource::Directory(dir.to_path_buf()))?;
    snapshot.verify().map_err(|e| e.reason.to_string())?;
    Ok(skill)
}

/// The canonical form of an existing path; a missing path keeps its spelling and is skipped.
pub(super) fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

/// Resolve the parent only, so a linked final component is still refused.
pub(super) fn canonical_parent(path: &Path) -> PathBuf {
    match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) => canonical(parent).join(name),
        _ => path.to_path_buf(),
    }
}

/// Up to `limit` files beside `SKILL.md`; links are listed only as files and never followed.
pub(super) fn sibling_files(base: &Path, limit: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![base.to_path_buf()];
    let mut visited = 0;
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut entries: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
        entries.sort();
        for path in entries {
            visited += 1;
            if out.len() >= limit || visited > ENTRY_LIMIT {
                out.sort();
                return out;
            }
            let is_dir = std::fs::symlink_metadata(&path).is_ok_and(|m| m.is_dir());
            if is_dir {
                stack.push(path);
            } else if let Ok(rel) = path.strip_prefix(base)
                && rel != Path::new("SKILL.md")
            {
                out.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    out.sort();
    out
}

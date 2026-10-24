//! `cyber uninstall` planning and removal (`installation-upgrade` → Uninstall command).
//!
//! The plan lists the binaries of a `script` install, the PATH lines the install script
//! marked in shell rc files, and the config, data, state and cache directories, each with
//! a size counted through `symlink_metadata`, so a link inside a cyber directory counts as
//! itself and never as what it points to. Every directory must pass [`is_safe_root`] when
//! the plan is built and again right before it is removed. Removal never follows a link:
//! directories go through `std::fs::remove_dir_all`, which unlinks symlinks (and Windows
//! junctions) instead of descending into them and walks by descriptor on Unix. An rc file
//! is rewritten atomically with its permissions kept, losing only the marker line and the
//! line after it.

use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};

use cyber_core::env::EnvSource;
use cyber_core::paths::Paths;
use serde::Serialize;

use crate::method::{InstallMethod, receipt_path};
use crate::replace::{helper_path, old_path};

/// The comment line the install script writes above its PATH line.
pub const PATH_MARKER: &str = "# cyber installer: PATH";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    Binary,
    PathLine,
    Config,
    Data,
    State,
    Cache,
}

/// One thing the uninstall removes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Entry {
    pub kind: EntryKind,
    pub path: PathBuf,
    /// Bytes: the tree for a directory, the file for a binary, the two lines for a
    /// PATH line.
    pub size: u64,
    pub removed: bool,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Flags {
    pub keep_config: bool,
    pub keep_data: bool,
}

/// What the plan is built from.
#[derive(Debug, Clone)]
pub struct Target<'a> {
    pub paths: &'a Paths,
    pub home: &'a Path,
    pub method: InstallMethod,
    /// The running executable, as `current_exe` reports it.
    pub exe: &'a Path,
    /// Shell rc files that may carry a marked PATH line ([`rc_candidates`]).
    pub rc_files: &'a [PathBuf],
    /// `CYBER_HOME`, removed once it is empty.
    pub cyber_home: Option<&'a Path>,
}

#[derive(Debug, thiserror::Error)]
pub enum UninstallError {
    #[error("refusing to remove {}: {reason}", path.display())]
    Refused { path: PathBuf, reason: &'static str },
    #[error("cannot remove {}: {source}", path.display())]
    Io { path: PathBuf, source: io::Error },
}

fn io_error(path: &Path) -> impl FnOnce(io::Error) -> UninstallError + '_ {
    move |source| UninstallError::Io {
        path: path.to_path_buf(),
        source,
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct UninstallPlan {
    pub entries: Vec<Entry>,
    /// What the user has to do by hand (a symlinked rc file, the renamed Windows exe).
    pub notes: Vec<String>,
    #[serde(skip)]
    exe: PathBuf,
    #[serde(skip)]
    leftovers: Vec<PathBuf>,
}

impl UninstallPlan {
    /// Every entry that exists, in removal order. A directory that fails
    /// [`is_safe_root`] refuses the whole plan, so nothing is removed.
    pub fn build(target: &Target<'_>, flags: Flags) -> Result<Self, UninstallError> {
        let mut plan = Self {
            entries: Vec::new(),
            notes: Vec::new(),
            exe: target.exe.to_path_buf(),
            leftovers: Vec::new(),
        };
        if target.method == InstallMethod::Script {
            plan.add_binaries(target.exe);
        }
        for rc in target.rc_files {
            plan.add_path_line(rc);
        }
        let paths = target.paths;
        let dirs = [
            (EntryKind::Config, &paths.config, flags.keep_config),
            (EntryKind::Data, &paths.data, flags.keep_data),
            (EntryKind::State, &paths.state, false),
            (EntryKind::Cache, &paths.cache, false),
        ];
        for (kind, dir, keep) in dirs {
            if keep || std::fs::symlink_metadata(dir).is_err() {
                continue;
            }
            if plan.entries.iter().any(|entry| entry.path == *dir) {
                continue;
            }
            is_safe_root(dir, target.home)?;
            plan.push(kind, dir.clone(), tree_size(dir));
        }
        if let Some(root) = target.cyber_home {
            plan.leftovers.push(root.to_path_buf());
        }
        Ok(plan)
    }

    pub fn total_size(&self) -> u64 {
        self.entries.iter().map(|entry| entry.size).sum()
    }

    fn push(&mut self, kind: EntryKind, path: PathBuf, size: u64) {
        self.entries.push(Entry {
            kind,
            path,
            size,
            removed: false,
        });
    }

    /// `.old` files first, then the helper, then `cyber`, so on Windows the running
    /// exe can take the name `cyber.old.exe` once the previous one is gone; the install
    /// script's receipt last, so a failed removal still finds a `script` install.
    fn add_binaries(&mut self, exe: &Path) {
        let helper = helper_path(exe);
        let paths = [old_path(exe), old_path(&helper), helper, exe.to_path_buf()];
        for path in paths.into_iter().chain(receipt_path(exe)) {
            if let Ok(meta) = std::fs::symlink_metadata(&path)
                && !meta.is_dir()
            {
                self.push(EntryKind::Binary, path, meta.len());
            }
        }
        if let Some(dir) = exe.parent() {
            self.leftovers.push(dir.to_path_buf());
        }
    }

    fn add_path_line(&mut self, rc: &Path) {
        let Ok(meta) = std::fs::symlink_metadata(rc) else {
            return;
        };
        let Ok(content) = std::fs::read(rc) else {
            return;
        };
        let removed = remove_path_lines(&content).1;
        if removed == 0 {
            return;
        }
        if meta.is_file() {
            self.push(EntryKind::PathLine, rc.to_path_buf(), removed as u64);
        } else {
            self.notes.push(format!(
                "{} is a symlink; remove the \"{PATH_MARKER}\" line and the line after it by hand",
                rc.display()
            ));
        }
    }

    /// Remove every entry in order, stopping at the first failure; each removed entry
    /// is marked. Directories are checked with [`is_safe_root`] again first.
    pub fn remove(&mut self, home: &Path) -> Result<(), UninstallError> {
        for index in 0..self.entries.len() {
            let entry = self.entries[index].clone();
            match entry.kind {
                EntryKind::Binary => self.remove_binary(&entry.path)?,
                EntryKind::PathLine => rewrite_rc(&entry.path).map_err(io_error(&entry.path))?,
                _ => remove_dir(&entry.path, home)?,
            }
            self.entries[index].removed = true;
        }
        // Emptied parents (`~/.cyber/bin`, `CYBER_HOME`): `remove_dir` keeps any that
        // still hold something.
        for dir in &self.leftovers {
            if is_safe_root(dir, home).is_ok() {
                let _ = std::fs::remove_dir(dir);
            }
        }
        Ok(())
    }

    /// Windows cannot delete the running exe, so it is renamed aside.
    fn remove_binary(&mut self, path: &Path) -> Result<(), UninstallError> {
        if cfg!(windows) && path == self.exe {
            let aside = old_path(path);
            std::fs::rename(path, &aside).map_err(io_error(path))?;
            self.notes.push(format!(
                "renamed the running {} to {}; delete it after cyber exits",
                path.display(),
                aside.display()
            ));
            return Ok(());
        }
        match std::fs::remove_file(path) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => Err(io_error(path)(error)),
            _ => Ok(()),
        }
    }
}

fn remove_dir(dir: &Path, home: &Path) -> Result<(), UninstallError> {
    if std::fs::symlink_metadata(dir).is_err() {
        // Inside a directory removed before it.
        return Ok(());
    }
    is_safe_root(dir, home)?;
    std::fs::remove_dir_all(dir).map_err(io_error(dir))
}

/// Refuse `dir` unless it is an absolute path without `.` or `..`, not a filesystem or
/// drive root, not the home directory or one of its ancestors, at least two components
/// below the home directory (or the root when it is elsewhere), and a real directory
/// rather than a symlink.
pub fn is_safe_root(dir: &Path, home: &Path) -> Result<(), UninstallError> {
    let refuse = |reason| {
        Err(UninstallError::Refused {
            path: dir.to_path_buf(),
            reason,
        })
    };
    // `components` drops interior `.` segments, so the text is checked.
    let lexical = !dir
        .to_string_lossy()
        .split(std::path::is_separator)
        .any(|segment| segment == "." || segment == "..");
    if !dir.is_absolute() || !lexical {
        return refuse("it is not a resolved absolute path");
    }
    if normal_components(dir) == 0 {
        return refuse("it is a filesystem root");
    }
    if is_home_or_ancestor(dir, home) {
        return refuse("it is the home directory or one of its ancestors");
    }
    let depth = match dir.strip_prefix(home) {
        Ok(below) => normal_components(below),
        Err(_) => normal_components(dir),
    };
    if depth < 2 {
        return refuse("it is too close to the root or the home directory");
    }
    match std::fs::symlink_metadata(dir) {
        Ok(meta) if meta.file_type().is_symlink() => refuse("it is a symlink"),
        Ok(meta) if !meta.is_dir() => refuse("it is not a directory"),
        _ => Ok(()),
    }
}

fn normal_components(path: &Path) -> usize {
    path.components()
        .filter(|c| matches!(c, Component::Normal(_)))
        .count()
}

/// Compared as written and, where both resolve, canonically (`/var` → `/private/var`).
fn is_home_or_ancestor(dir: &Path, home: &Path) -> bool {
    if home.starts_with(dir) {
        return true;
    }
    match (dir.canonicalize(), home.canonicalize()) {
        (Ok(dir), Ok(home)) => home.starts_with(dir),
        _ => false,
    }
}

/// Bytes under `path`, without following links: a symlink counts its own length.
pub fn tree_size(path: &Path) -> u64 {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return 0;
    };
    if !meta.is_dir() {
        return meta.len();
    }
    let Ok(children) = std::fs::read_dir(path) else {
        return 0;
    };
    children
        .flatten()
        .map(|child| tree_size(&child.path()))
        .sum()
}

/// The rc files the install script may have written to: zsh, bash, the POSIX profile
/// and fish.
pub fn rc_candidates(env: &dyn EnvSource, home: &Path) -> Vec<PathBuf> {
    let config = env
        .get("XDG_CONFIG_HOME")
        .map_or_else(|| home.join(".config"), PathBuf::from);
    let mut files: Vec<PathBuf> = [".zshrc", ".bashrc", ".bash_profile", ".profile"]
        .iter()
        .map(|name| home.join(name))
        .collect();
    files.push(config.join("fish").join("config.fish"));
    files
}

/// `content` without each marker line and the single line after it, and how many bytes
/// went.
pub fn remove_path_lines(content: &[u8]) -> (Vec<u8>, usize) {
    let mut kept = Vec::with_capacity(content.len());
    let mut lines = content.split_inclusive(|&b| b == b'\n');
    let mut removed = 0;
    while let Some(line) = lines.next() {
        if line.trim_ascii() == PATH_MARKER.as_bytes() {
            removed += line.len() + lines.next().map_or(0, <[u8]>::len);
        } else {
            kept.extend_from_slice(line);
        }
    }
    (kept, removed)
}

/// Rewrite `rc` without its marked PATH lines: a temporary file beside it takes its
/// permissions, is synced, and is renamed over it.
pub fn rewrite_rc(rc: &Path) -> io::Result<()> {
    let meta = std::fs::symlink_metadata(rc)?;
    if !meta.is_file() {
        return Err(io::Error::other("not a regular file"));
    }
    let (kept, _) = remove_path_lines(&std::fs::read(rc)?);
    let dir = rc.parent().unwrap_or(Path::new("."));
    let mut staged = tempfile::Builder::new()
        .prefix(".cyber-uninstall-")
        .tempfile_in(dir)?;
    staged.write_all(&kept)?;
    std::fs::set_permissions(staged.path(), meta.permissions())?;
    staged.as_file().sync_all()?;
    staged.persist(rc).map_err(|error| error.error)?;
    Ok(())
}

/// The package manager's uninstall command, as an argv run without a shell.
pub fn manager_argv(method: InstallMethod) -> Option<Vec<String>> {
    let argv: &[&str] = match method {
        InstallMethod::Brew => &["brew", "uninstall", "cyber"],
        InstallMethod::Npm => &["npm", "rm", "-g", "@cyber-code/cli"],
        InstallMethod::Pnpm => &["pnpm", "rm", "-g", "@cyber-code/cli"],
        InstallMethod::Bun => &["bun", "rm", "-g", "@cyber-code/cli"],
        InstallMethod::Cargo => &["cargo", "uninstall", "cyber-cli"],
        InstallMethod::Scoop => &["scoop", "uninstall", "cyber"],
        InstallMethod::Winget => &["winget", "uninstall", "cyber-code.cyber"],
        _ => return None,
    };
    Some(argv.iter().map(|arg| arg.to_string()).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Layout {
        _dir: tempfile::TempDir,
        root: PathBuf,
        home: PathBuf,
        paths: Paths,
    }

    fn layout() -> Layout {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let home = root.join("home");
        let base = |rel: &str| home.join(rel).join("cyber");
        let paths = Paths {
            data: base(".local/share"),
            config: base(".config"),
            state: base(".local/state"),
            cache: base(".cache"),
            tmp: root.join("tmp"),
        };
        for dir in [&paths.data, &paths.config, &paths.state, &paths.cache] {
            std::fs::create_dir_all(dir).unwrap();
        }
        Layout {
            _dir: dir,
            root,
            home,
            paths,
        }
    }

    fn reason(dir: &Path, home: &Path) -> &'static str {
        match is_safe_root(dir, home) {
            Err(UninstallError::Refused { reason, .. }) => reason,
            other => panic!("{} was not refused: {other:?}", dir.display()),
        }
    }

    #[test]
    fn is_safe_root_refuses_each_unsafe_directory() {
        let l = layout();
        let home = &l.home;
        assert!(reason(Path::new("relative/cyber"), home).contains("resolved"));
        assert!(reason(&home.join(".config/../cyber"), home).contains("resolved"));
        assert!(reason(&home.join("./.config/cyber"), home).contains("resolved"));
        let root = l.root.ancestors().last().unwrap();
        assert!(reason(root, home).contains("root"));
        assert!(reason(home, home).contains("home"));
        assert!(reason(home.parent().unwrap(), home).contains("home"));
        assert!(reason(&home.join(".cyber"), home).contains("too close"));
        assert!(reason(&root.join("opt"), home).contains("too close"));
        std::fs::create_dir_all(l.root.join("projects")).unwrap();
        #[cfg(unix)]
        {
            let link = l.paths.cache.join("link");
            std::os::unix::fs::symlink(l.root.join("projects"), &link).unwrap();
            assert_eq!(reason(&link, home), "it is a symlink");
        }
        std::fs::write(l.paths.cache.join("file"), b"x").unwrap();
        assert_eq!(
            reason(&l.paths.cache.join("file"), home),
            "it is not a directory"
        );
        for dir in [
            &l.paths.config,
            &l.paths.data,
            &l.paths.state,
            &l.paths.cache,
        ] {
            is_safe_root(dir, home).unwrap();
        }
    }

    #[cfg(windows)]
    #[test]
    fn is_safe_root_refuses_drive_roots() {
        let home = Path::new(r"C:\Users\someone");
        assert!(reason(Path::new(r"C:\"), home).contains("root"));
        assert!(reason(Path::new(r"\\server\share\"), home).contains("root"));
        assert!(reason(Path::new(r"C:\Users"), home).contains("home"));
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_home_is_still_recognized_canonically() {
        let l = layout();
        let alias = l.root.join("alias");
        std::os::unix::fs::symlink(&l.home, &alias).unwrap();
        assert!(reason(&alias, &l.home).contains("home"));
    }

    #[test]
    fn the_rc_rewrite_removes_only_the_marker_and_the_next_line() {
        let content = format!(
            "export A=1\n{PATH_MARKER}\nexport PATH=\"/x/.cyber/bin:$PATH\"\n# cyber installer: other\nexport B=2\n"
        );
        let (kept, removed) = remove_path_lines(content.as_bytes());
        assert_eq!(
            String::from_utf8(kept).unwrap(),
            "export A=1\n# cyber installer: other\nexport B=2\n"
        );
        assert_eq!(
            removed,
            PATH_MARKER.len() + "\nexport PATH=\"/x/.cyber/bin:$PATH\"\n".len()
        );
        // A marker on the last line without a newline goes alone.
        let (kept, _) = remove_path_lines(format!("a\n{PATH_MARKER}").as_bytes());
        assert_eq!(kept, b"a\n");
        assert_eq!(remove_path_lines(b"nothing here\n").1, 0);
    }

    #[cfg(unix)]
    #[test]
    fn the_rc_rewrite_keeps_the_file_mode() {
        use std::os::unix::fs::PermissionsExt;
        let l = layout();
        let rc = l.home.join(".zshrc");
        std::fs::write(&rc, format!("keep\n{PATH_MARKER}\nexport PATH=x\nalso\n")).unwrap();
        std::fs::set_permissions(&rc, std::fs::Permissions::from_mode(0o640)).unwrap();
        rewrite_rc(&rc).unwrap();
        assert_eq!(std::fs::read_to_string(&rc).unwrap(), "keep\nalso\n");
        let mode = std::fs::metadata(&rc).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o640);
        let staged = std::fs::read_dir(&l.home)
            .unwrap()
            .flatten()
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with(".cyber-uninstall")
            })
            .count();
        assert_eq!(staged, 0);
    }

    #[cfg(unix)]
    #[test]
    fn size_computation_does_not_follow_symlinks() {
        let l = layout();
        let big = l.root.join("big");
        std::fs::write(&big, vec![0u8; 100_000]).unwrap();
        std::fs::write(l.paths.cache.join("small"), b"12345").unwrap();
        std::os::unix::fs::symlink(&big, l.paths.cache.join("link")).unwrap();
        std::os::unix::fs::symlink(&l.root, l.paths.cache.join("loop")).unwrap();
        let size = tree_size(&l.paths.cache);
        assert!(size < 1000, "{size}");
        assert!(size >= 5);
    }

    fn target<'a>(l: &'a Layout, exe: &'a Path, rc: &'a [PathBuf]) -> Target<'a> {
        Target {
            paths: &l.paths,
            home: &l.home,
            method: InstallMethod::Script,
            exe,
            rc_files: rc,
            cyber_home: None,
        }
    }

    #[test]
    fn the_plan_lists_binaries_path_lines_and_directories_and_honors_keep_flags() {
        let l = layout();
        let bin = l.home.join(".cyber/bin");
        std::fs::create_dir_all(&bin).unwrap();
        let exe = bin.join(format!("cyber{}", std::env::consts::EXE_SUFFIX));
        std::fs::write(&exe, b"binary").unwrap();
        std::fs::write(old_path(&exe), b"old").unwrap();
        let rc = vec![l.home.join(".bashrc"), l.home.join(".zshrc")];
        std::fs::write(&rc[0], format!("{PATH_MARKER}\nexport PATH=x\n")).unwrap();
        std::fs::write(&rc[1], "unrelated\n").unwrap();
        let plan = UninstallPlan::build(&target(&l, &exe, &rc), Flags::default()).unwrap();
        let kinds: Vec<_> = plan
            .entries
            .iter()
            .map(|e| (e.kind, e.path.clone()))
            .collect();
        assert_eq!(
            kinds,
            [
                (EntryKind::Binary, old_path(&exe)),
                (EntryKind::Binary, exe.clone()),
                (EntryKind::PathLine, rc[0].clone()),
                (EntryKind::Config, l.paths.config.clone()),
                (EntryKind::Data, l.paths.data.clone()),
                (EntryKind::State, l.paths.state.clone()),
                (EntryKind::Cache, l.paths.cache.clone()),
            ]
        );
        assert_eq!(plan.entries[1].size, 6);
        let flags = Flags {
            keep_config: true,
            keep_data: true,
        };
        let kept = UninstallPlan::build(&target(&l, &exe, &rc), flags).unwrap();
        assert!(
            kept.entries
                .iter()
                .all(|e| !matches!(e.kind, EntryKind::Config | EntryKind::Data))
        );
        let mut other = target(&l, &exe, &rc);
        other.method = InstallMethod::Brew;
        let plan = UninstallPlan::build(&other, Flags::default()).unwrap();
        assert!(plan.entries.iter().all(|e| e.kind != EntryKind::Binary));
    }

    #[test]
    fn a_home_config_dir_refuses_the_whole_plan() {
        let mut l = layout();
        l.paths.config = l.home.clone();
        let exe = l.root.join("cyber");
        let error = UninstallPlan::build(&target(&l, &exe, &[]), Flags::default()).unwrap_err();
        assert!(error.to_string().contains("home directory"), "{error}");
    }

    #[cfg(unix)]
    #[test]
    fn removal_unlinks_symlinks_without_following_them() {
        let l = layout();
        let projects = l.root.join("projects");
        std::fs::create_dir_all(&projects).unwrap();
        std::fs::write(projects.join("keep.txt"), b"mine").unwrap();
        std::os::unix::fs::symlink(&projects, l.paths.cache.join("link")).unwrap();
        let exe = l.root.join("cyber");
        let mut plan = UninstallPlan::build(&target(&l, &exe, &[]), Flags::default()).unwrap();
        plan.remove(&l.home).unwrap();
        assert!(plan.entries.iter().all(|e| e.removed));
        assert!(!l.paths.cache.exists());
        assert_eq!(std::fs::read(projects.join("keep.txt")).unwrap(), b"mine");
    }

    #[test]
    fn package_methods_have_uninstall_commands() {
        let argv = |m| manager_argv(m).map(|a| a.join(" "));
        assert_eq!(argv(InstallMethod::Brew).unwrap(), "brew uninstall cyber");
        assert_eq!(
            argv(InstallMethod::Npm).unwrap(),
            "npm rm -g @cyber-code/cli"
        );
        assert_eq!(
            argv(InstallMethod::Cargo).unwrap(),
            "cargo uninstall cyber-cli"
        );
        assert_eq!(
            argv(InstallMethod::Winget).unwrap(),
            "winget uninstall cyber-code.cyber"
        );
        assert_eq!(argv(InstallMethod::Script), None);
        assert_eq!(argv(InstallMethod::Nix), None);
    }
}

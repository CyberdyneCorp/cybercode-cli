//! Atomic self-replace (`installation-upgrade` → Upgrade command): a verified binary
//! staged beside its target takes the target's mode and is synced, the target is
//! renamed to `<name>.old`, the new file is renamed into place, and the directory is
//! synced. A failed second rename puts the old file back. The running executable can be
//! renamed on Windows too, so both platforms share these steps; paths come from
//! `current_exe` and are never canonicalized (Windows would return `\\?\` paths).

use std::io;
use std::path::{Path, PathBuf};

/// `cyber` → `cyber.old`, `cyber.exe` → `cyber.old.exe`.
pub fn old_path(target: &Path) -> PathBuf {
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let suffix = std::env::consts::EXE_SUFFIX;
    let old = match name.strip_suffix(suffix).filter(|_| !suffix.is_empty()) {
        Some(stem) => format!("{stem}.old{suffix}"),
        None => format!("{name}.old"),
    };
    target.with_file_name(old)
}

/// The sandbox helper installed beside `exe`.
pub fn helper_path(exe: &Path) -> PathBuf {
    exe.with_file_name(format!(
        "{}{}",
        crate::archive::HELPER,
        std::env::consts::EXE_SUFFIX
    ))
}

/// One binary to replace: `new` is staged in the directory of `target`.
#[derive(Debug, Clone)]
pub struct Swap {
    pub target: PathBuf,
    pub new: PathBuf,
}

/// Replace the helper first, then `cyber`, so a failure halfway never leaves a new
/// `cyber` beside a missing or older helper.
pub fn install(cyber: &Swap, helper: Option<&Swap>) -> io::Result<()> {
    install_with(cyber, helper, &|from, to| std::fs::rename(from, to))
}

type Rename<'a> = &'a dyn Fn(&Path, &Path) -> io::Result<()>;

fn install_with(cyber: &Swap, helper: Option<&Swap>, rename: Rename<'_>) -> io::Result<()> {
    if let Some(helper) = helper {
        replace_with(helper, rename)?;
    }
    replace_with(cyber, rename)
}

fn replace_with(swap: &Swap, rename: Rename<'_>) -> io::Result<()> {
    let existing = std::fs::symlink_metadata(&swap.target).ok();
    if existing.as_ref().is_some_and(|m| !m.is_file()) {
        return Err(io::Error::other(format!(
            "{} is not a regular file",
            swap.target.display()
        )));
    }
    copy_mode(existing.as_ref(), &swap.new)?;
    std::fs::OpenOptions::new()
        .write(true)
        .open(&swap.new)?
        .sync_all()?;
    let old = old_path(&swap.target);
    if existing.is_some() {
        rename(&swap.target, &old)?;
    }
    if let Err(err) = rename(&swap.new, &swap.target) {
        if existing.is_some() {
            // Put the old binary back; the error that matters is the first one.
            let _ = rename(&old, &swap.target);
        }
        return Err(err);
    }
    sync_dir(&swap.target)
}

/// The new file gets the old file's permissions (`0o755` for a first install).
#[cfg(unix)]
fn copy_mode(existing: Option<&std::fs::Metadata>, new: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mode = existing.map_or(0o755, |m| m.permissions().mode() & 0o777);
    std::fs::set_permissions(new, std::fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
fn copy_mode(_existing: Option<&std::fs::Metadata>, _new: &Path) -> io::Result<()> {
    Ok(())
}

/// Make the renames durable.
#[cfg(unix)]
fn sync_dir(target: &Path) -> io::Result<()> {
    let dir = target.parent().unwrap_or(Path::new("."));
    std::fs::File::open(dir)?.sync_all()
}

/// Windows cannot open a directory as a file; its renames are journaled by NTFS.
#[cfg(not(unix))]
fn sync_dir(_target: &Path) -> io::Result<()> {
    Ok(())
}

/// Remove `cyber.old` and the helper's `.old` beside `exe` when they are regular files.
/// Best-effort and silent: on Windows a still-running old process keeps its file.
pub fn cleanup_old(exe: &Path) {
    for target in [exe.to_path_buf(), helper_path(exe)] {
        let old = old_path(&target);
        if std::fs::symlink_metadata(&old).is_ok_and(|m| m.is_file()) {
            let _ = std::fs::remove_file(&old);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn exe(name: &str) -> String {
        format!("{name}{}", std::env::consts::EXE_SUFFIX)
    }

    fn staged(dir: &Path, name: &str, body: &[u8]) -> Swap {
        let new = dir.join(format!(".{name}.new"));
        std::fs::write(&new, body).unwrap();
        Swap {
            target: dir.join(name),
            new,
        }
    }

    #[test]
    fn old_paths_keep_the_exe_suffix() {
        let dir = Path::new("bin");
        assert_eq!(
            old_path(&dir.join(exe("cyber"))),
            dir.join(exe("cyber.old"))
        );
        assert_eq!(
            old_path(&helper_path(&dir.join(exe("cyber")))),
            dir.join(exe("cyber-sandbox-exec.old"))
        );
    }

    #[test]
    fn replaces_and_keeps_the_old_binary() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join(exe("cyber"));
        std::fs::write(&target, b"old").unwrap();
        let swap = staged(dir.path(), &exe("cyber"), b"new");
        install(&swap, None).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"new");
        assert_eq!(std::fs::read(old_path(&target)).unwrap(), b"old");
        assert!(!swap.new.exists());

        cleanup_old(&target);
        assert!(!old_path(&target).exists());
        assert_eq!(std::fs::read(&target).unwrap(), b"new");
    }

    #[cfg(unix)]
    #[test]
    fn the_mode_of_the_old_binary_is_kept() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("cyber");
        std::fs::write(&target, b"old").unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o750)).unwrap();
        let swap = staged(dir.path(), "cyber", b"new");
        std::fs::set_permissions(&swap.new, std::fs::Permissions::from_mode(0o600)).unwrap();
        install(&swap, None).unwrap();
        let mode = std::fs::metadata(&target).unwrap().permissions().mode() & 0o7777;
        assert_eq!(mode, 0o750);

        // A helper installed for the first time is executable.
        let helper = staged(dir.path(), "cyber-sandbox-exec", b"helper");
        install(&staged(dir.path(), "cyber", b"newer"), Some(&helper)).unwrap();
        let mode = std::fs::metadata(&helper.target)
            .unwrap()
            .permissions()
            .mode()
            & 0o7777;
        assert_eq!(mode, 0o755);
    }

    #[test]
    fn a_failed_rename_rolls_back() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join(exe("cyber"));
        std::fs::write(&target, b"old").unwrap();
        let swap = staged(dir.path(), &exe("cyber"), b"new");
        let new = swap.new.clone();
        let rename = |from: &Path, to: &Path| {
            if from == new {
                return Err(io::Error::other("disk full"));
            }
            std::fs::rename(from, to)
        };
        let err = install_with(&swap, None, &rename).unwrap_err();
        assert_eq!(err.to_string(), "disk full");
        assert_eq!(std::fs::read(&target).unwrap(), b"old");
        assert!(!old_path(&target).exists());
    }

    #[test]
    fn the_helper_is_replaced_first() {
        let dir = tempfile::tempdir().unwrap();
        for name in [exe("cyber"), exe("cyber-sandbox-exec")] {
            std::fs::write(dir.path().join(name), b"old").unwrap();
        }
        let cyber = staged(dir.path(), &exe("cyber"), b"new cyber");
        let helper = staged(dir.path(), &exe("cyber-sandbox-exec"), b"new helper");
        let order = RefCell::new(Vec::new());
        let rename = |from: &Path, to: &Path| {
            order
                .borrow_mut()
                .push(to.file_name().unwrap().to_string_lossy().into_owned());
            if from == cyber.new {
                return Err(io::Error::other("refused"));
            }
            std::fs::rename(from, to)
        };
        install_with(&cyber, Some(&helper), &rename).unwrap_err();
        assert_eq!(
            order.into_inner(),
            [
                exe("cyber-sandbox-exec.old"),
                exe("cyber-sandbox-exec"),
                exe("cyber.old"),
                exe("cyber"),
                exe("cyber"),
            ]
        );
        // The helper is new, `cyber` is still the old one: never a new `cyber` alone.
        assert_eq!(std::fs::read(&helper.target).unwrap(), b"new helper");
        assert_eq!(std::fs::read(&cyber.target).unwrap(), b"old");
    }

    #[cfg(unix)]
    #[test]
    fn cleanup_leaves_symlinks_and_directories() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("cyber");
        let outside = dir.path().join("outside");
        std::fs::write(&outside, b"keep").unwrap();
        std::os::unix::fs::symlink(&outside, old_path(&target)).unwrap();
        std::fs::create_dir(old_path(&helper_path(&target))).unwrap();
        cleanup_old(&target);
        assert!(std::fs::symlink_metadata(old_path(&target)).is_ok());
        assert!(old_path(&helper_path(&target)).is_dir());
        assert_eq!(std::fs::read(&outside).unwrap(), b"keep");
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_target_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real");
        std::fs::write(&real, b"old").unwrap();
        std::os::unix::fs::symlink(&real, dir.path().join("cyber")).unwrap();
        let swap = staged(dir.path(), "cyber", b"new");
        assert!(install(&swap, None).is_err());
        assert_eq!(std::fs::read(&real).unwrap(), b"old");
    }
}

//! `storage-events` → XDG directory layout, Database location and connection.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use cyber_core::paths::{DatabaseLocation, Paths, database_location};
use cyber_core::version::Channel;

fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

#[test]
fn defaults_follow_xdg_under_home() {
    let p = Paths::resolve(&env(&[]), Path::new("/home/u"));
    assert_eq!(p.data, PathBuf::from("/home/u/.local/share/cyber"));
    assert_eq!(p.config, PathBuf::from("/home/u/.config/cyber"));
    assert_eq!(p.state, PathBuf::from("/home/u/.local/state/cyber"));
    assert_eq!(p.cache, PathBuf::from("/home/u/.cache/cyber"));
}

#[test]
fn cyber_home_relocates_everything() {
    let p = Paths::resolve(
        &env(&[("CYBER_HOME", "/opt/cyber-home")]),
        Path::new("/home/u"),
    );
    assert_eq!(p.data, PathBuf::from("/opt/cyber-home/data"));
    assert_eq!(p.config, PathBuf::from("/opt/cyber-home/config"));
    assert_eq!(p.state, PathBuf::from("/opt/cyber-home/state"));
    assert_eq!(p.cache, PathBuf::from("/opt/cyber-home/cache"));
}

#[test]
fn specific_variables_win_over_cyber_home() {
    let e = env(&[
        ("CYBER_HOME", "/opt/cyber-home"),
        ("CYBER_CONFIG_DIR", "/etc/cyber-user"),
        ("XDG_CACHE_HOME", "/var/cache"),
    ]);
    let p = Paths::resolve(&e, Path::new("/home/u"));
    assert_eq!(p.config, PathBuf::from("/etc/cyber-user"));
    assert_eq!(p.cache, PathBuf::from("/var/cache/cyber"));
    assert_eq!(p.data, PathBuf::from("/opt/cyber-home/data"));
}

#[test]
fn database_name_depends_on_channel_and_cyber_db() {
    let p = Paths::resolve(&env(&[]), Path::new("/home/u"));
    let file = |e: &HashMap<String, String>, c| database_location(&p, e, c);
    assert_eq!(
        file(&env(&[]), Channel::Stable),
        DatabaseLocation::File(p.data.join("cyber.db"))
    );
    assert_eq!(
        file(&env(&[]), Channel::Beta),
        DatabaseLocation::File(p.data.join("cyber-beta.db"))
    );
    assert_eq!(
        file(&env(&[("CYBER_DB", ":memory:")]), Channel::Stable),
        DatabaseLocation::Memory
    );
    assert_eq!(
        file(&env(&[("CYBER_DB", "x.db")]), Channel::Stable),
        DatabaseLocation::File(p.data.join("x.db"))
    );
    assert_eq!(
        file(&env(&[("CYBER_DB", "/tmp/y.db")]), Channel::Stable),
        DatabaseLocation::File("/tmp/y.db".into())
    );
}

#[test]
fn ensure_creates_derived_directories() {
    let dir = tempfile::tempdir().unwrap();
    let p = Paths::resolve(
        &env(&[("CYBER_HOME", dir.path().to_str().unwrap())]),
        dir.path(),
    );
    p.ensure().unwrap();
    for sub in [
        "log",
        "tool-output",
        "jobs",
        "snapshot",
        "memory",
        "worktrees",
    ] {
        assert!(p.data.join(sub).is_dir(), "{sub}");
    }
    for sub in ["bin", "models", "plugins"] {
        assert!(p.cache.join(sub).is_dir(), "{sub}");
    }
}

#[cfg(windows)]
#[test]
fn ensure_windows_memory_root_is_private_without_creating_scopes() {
    let dir = tempfile::tempdir().unwrap();
    let p = Paths::resolve(
        &env(&[("CYBER_HOME", dir.path().to_str().unwrap())]),
        dir.path(),
    );
    p.ensure().unwrap();
    p.ensure().unwrap();
    let root =
        cap_std::fs::Dir::open_ambient_dir(p.data.join("memory"), cap_std::ambient_authority())
            .unwrap()
            .into_std_file();
    cyber_core::memory::windows::verify_private(&root).unwrap();
    assert_eq!(std::fs::read_dir(p.data.join("memory")).unwrap().count(), 0);
}

#[cfg(windows)]
#[test]
fn ensure_refuses_existing_broad_windows_memory_root_without_repair() {
    let dir = tempfile::tempdir().unwrap();
    let p = Paths::resolve(
        &env(&[("CYBER_HOME", dir.path().to_str().unwrap())]),
        dir.path(),
    );
    let memory = p.data.join("memory");
    std::fs::create_dir_all(&memory).unwrap();
    std::fs::write(memory.join("user"), b"retained startup edits").unwrap();
    assert_eq!(
        p.ensure().unwrap_err().kind(),
        std::io::ErrorKind::PermissionDenied
    );
    let root = cap_std::fs::Dir::open_ambient_dir(&memory, cap_std::ambient_authority())
        .unwrap()
        .into_std_file();
    assert!(cyber_core::memory::windows::verify_private(&root).is_err());
    assert_eq!(
        std::fs::read(memory.join("user")).unwrap(),
        b"retained startup edits"
    );
    assert_eq!(std::fs::read_dir(memory).unwrap().count(), 1);
}

/// Regression: the embedded git SHA went stale because the build script did not watch HEAD.
#[test]
fn build_info_reports_the_current_commit() {
    if std::env::var_os("CYBER_GIT_SHA").is_some() {
        return;
    }
    let Ok(out) = std::process::Command::new("git")
        .args(["rev-parse", "--short=7", "HEAD"])
        .output()
    else {
        return;
    };
    if !out.status.success() {
        return;
    }
    let head = String::from_utf8_lossy(&out.stdout).trim().to_string();
    assert_eq!(cyber_core::version::build_info().git_sha, head);
}

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

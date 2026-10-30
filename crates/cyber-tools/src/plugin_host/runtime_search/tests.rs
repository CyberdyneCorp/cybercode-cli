use super::*;

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    location: PathBuf,
    package: PathBuf,
    owner: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let location = root.join("project");
        let package = root.join("package");
        let owner = root.join("owner-bin");
        for path in [&location, &package, &owner] {
            std::fs::create_dir(path).unwrap();
        }
        Self {
            _temp: temp,
            root,
            location,
            package,
            owner,
        }
    }
    fn script(&self, directory: &Path, name: &str, token: &str) -> PathBuf {
        let path = directory.join(if cfg!(windows) {
            format!("{name}.exe")
        } else {
            name.into()
        });
        std::fs::write(&path, format!("#!/bin/sh\nprintf '%s' '{token}'\n")).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        path
    }
    fn select(&self, names: &[&str], directories: &[&Path]) -> Option<PathBuf> {
        let path = std::env::join_paths(directories).unwrap();
        search(names, &path, &self.location, &self.package)
    }
}

#[test]
fn project_and_package_path_entries_cannot_shadow_the_owner_runtime() {
    let f = Fixture::new();
    f.script(&f.location, "node", "project");
    f.script(&f.package, "node", "package");
    let owner = f.script(&f.owner, "node", "owner");
    let selected = f
        .select(&["node"], &[&f.location, &f.package, &f.owner])
        .unwrap();
    #[cfg(unix)]
    {
        let result = std::process::Command::new(&selected).output().unwrap();
        assert!(result.status.success());
        assert_eq!(result.stdout, b"owner", "runtime selection receipt");
    }
    assert_eq!(selected, owner);
    assert!(f.select(&["node"], &[&f.location, &f.package]).is_none());
}

#[test]
fn runtime_name_preference_is_retained_before_path_directory_order() {
    let f = Fixture::new();
    f.script(&f.owner, "python", "fallback");
    let other = f.root.join("second");
    std::fs::create_dir(&other).unwrap();
    let preferred = f.script(&other, "python3", "preferred");
    assert_eq!(
        f.select(&["python3", "python"], &[&f.owner, &other]),
        Some(preferred)
    );
}

#[test]
fn an_unresolved_location_or_package_cannot_disable_the_search_exclusion() {
    let f = Fixture::new();
    f.script(&f.owner, "node", "owner");
    let path = std::env::join_paths([&f.owner]).unwrap();
    assert!(search(&["node"], &path, &f.root.join("missing"), &f.package).is_none());
    assert!(search(&["node"], &path, &f.location, &f.root.join("missing")).is_none());
}

#[cfg(windows)]
#[test]
fn windows_case_variants_still_exclude_project_path_entries() {
    let f = Fixture::new();
    f.script(&f.location, "node", "project");
    let upper = PathBuf::from(f.location.to_str().unwrap().to_uppercase());
    let path = std::env::join_paths([&upper]).unwrap();
    assert!(search(&["node"], &path, &f.location, &f.package).is_none());
}

#[cfg(unix)]
#[test]
fn directory_and_file_symlinks_cannot_reintroduce_project_runtime_code() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new();
    let project = f.script(&f.location, "node", "project");
    let alias = f.root.join("project-alias");
    symlink(&f.location, &alias).unwrap();
    let shims = f.root.join("shims");
    std::fs::create_dir(&shims).unwrap();
    symlink(&project, shims.join("node")).unwrap();
    assert!(f.select(&["node"], &[&alias, &shims]).is_none());
    let owner = f.script(&f.owner, "node", "owner");
    assert_eq!(
        f.select(&["node"], &[&alias, &shims, &f.owner]),
        Some(owner)
    );
    let path = std::env::join_paths([&alias]).unwrap();
    assert!(search(&["node"], &path, &alias, &f.package).is_none());
}

#[cfg(unix)]
#[test]
fn even_a_safe_target_link_cannot_make_a_project_path_entry_an_owner_runtime() {
    let f = Fixture::new();
    let owner = f.script(&f.owner, "node", "owner");
    std::os::unix::fs::symlink(owner, f.location.join("node")).unwrap();
    assert!(f.select(&["node"], &[&f.location]).is_none());
}

#[cfg(unix)]
#[test]
fn empty_and_existing_relative_path_entries_are_ignored() {
    let f = Fixture::new();
    let relative_bin = f.root.join("relative-bin");
    std::fs::create_dir(&relative_bin).unwrap();
    f.script(&relative_bin, "node", "relative");
    let cwd = std::env::current_dir().unwrap();
    let mut relative = PathBuf::new();
    for _ in cwd.ancestors().skip(1) {
        relative.push("..");
    }
    relative.push(relative_bin.strip_prefix("/").unwrap());
    assert!(relative.join("node").is_file());
    let owner = f.script(&f.owner, "node", "owner");
    assert_eq!(
        f.select(&["node"], &[Path::new(""), &relative, &f.owner]),
        Some(owner)
    );
}

#[cfg(unix)]
#[test]
fn non_executable_files_and_directories_do_not_hide_a_later_runtime() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new();
    let first = f.root.join("first");
    let second = f.root.join("second");
    std::fs::create_dir(&first).unwrap();
    std::fs::create_dir_all(second.join("node")).unwrap();
    let file = f.script(&first, "node", "not-executable");
    std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o600)).unwrap();
    let owner = f.script(&f.owner, "node", "owner");
    assert_eq!(
        f.select(&["node"], &[&first, &second, &f.owner]),
        Some(owner)
    );
}

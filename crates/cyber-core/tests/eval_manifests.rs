//! Every manifest in `eval/manifests` must validate against the repository
//! (`harness-evaluation` → Reproducible evaluation manifest).

use std::path::PathBuf;

use cyber_core::eval;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

#[test]
fn repository_manifests_are_valid() {
    let root = repo_root();
    let mut checked = 0;
    for entry in std::fs::read_dir(root.join("eval/manifests")).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|e| e != "json") {
            continue;
        }
        let manifest = eval::load_manifest(&path).unwrap();
        let issues = eval::validate(&manifest, &root);
        assert!(issues.is_empty(), "{}: {issues:#?}", path.display());
        checked += 1;
    }
    assert!(checked >= 2, "expected the coding and recovery manifests");
}

#[test]
fn tree_hash_detects_fixture_drift() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), "one").unwrap();
    let before = eval::tree_sha256(dir.path()).unwrap();
    std::fs::write(dir.path().join("a.txt"), "two").unwrap();
    assert_ne!(before, eval::tree_sha256(dir.path()).unwrap());
}

#[test]
fn source_alias_validation_refuses_parent_chains_and_metadata_targets() {
    use std::path::Path;
    for target in [
        "../outside",
        "a/../../outside",
        "/outside",
        "x/../y",
        ".git/config",
        "x/.GIT/config",
        "C:\\outside",
    ] {
        assert!(
            eval::validate_source_link(Path::new("alias"), Path::new(target)).is_err(),
            "{target}"
        );
    }
    assert!(
        eval::validate_source_link(Path::new("HomebrewFormula"), Path::new("pkg/brew")).is_ok()
    );
}

#[cfg(unix)]
#[test]
fn source_tree_identity_hashes_alias_targets_without_following_them() {
    use std::path::Path;
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("source")).unwrap();
    std::fs::write(root.path().join("source/file"), "original").unwrap();
    let alias = root.path().join("alias");
    std::os::unix::fs::symlink("source", &alias).unwrap();
    let first = eval::tree_sha256(root.path()).unwrap();
    let copy = root.path().join("alias-copy");
    eval::copy_source_link(&alias, &copy, Path::new("alias-copy")).unwrap();
    assert_eq!(std::fs::read_link(&copy).unwrap(), Path::new("source"));
    std::fs::remove_file(copy).unwrap();
    std::fs::remove_file(&alias).unwrap();
    std::os::unix::fs::symlink("missing", &alias).unwrap();
    assert_ne!(
        eval::tree_sha256(root.path()).unwrap(),
        first,
        "the literal target participates in identity"
    );
    std::fs::remove_file(&alias).unwrap();
    std::os::unix::fs::symlink("../outside", alias).unwrap();
    assert!(eval::tree_sha256(root.path()).is_err());
}

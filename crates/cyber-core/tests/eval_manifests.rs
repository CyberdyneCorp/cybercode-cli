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

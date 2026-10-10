use cyber_core::import::{SourceFile, SourceRoots, SourceSnapshot, discover_sources};
use std::path::{Path, PathBuf};

fn fixture(root: &Path, bytes: &[u8]) -> (SourceRoots, SourceFile) {
    let project = root.join("repo");
    let home = root.join("home");
    std::fs::create_dir_all(project.join(".claude")).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(project.join(".claude/settings.json"), bytes).unwrap();
    let roots = SourceRoots {
        project_root: project.clone(),
        directory: project,
        home,
        codex_home: None,
    };
    let source = discover_sources(&roots)
        .unwrap()
        .files
        .into_iter()
        .find(|f| f.path.ends_with("settings.json"))
        .unwrap();
    (roots, source)
}

#[test]
fn snapshot_reads_admitted_sources_and_keeps_private_bytes_out_of_debugging() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let (roots, source) = fixture(&root, b"private-token");
    let snapshot = SourceSnapshot::read(&roots, &source).unwrap();
    assert_eq!(snapshot.bytes(), b"private-token");
    assert_eq!(snapshot.text().unwrap(), "private-token");
    assert!(!format!("{snapshot:?}").contains("private-token"));
    snapshot.verify().unwrap();
    assert_eq!(std::fs::read(&source.path).unwrap(), b"private-token");
    assert!(!roots.project_root.join("cyber.jsonc").exists());
    assert!(!roots.project_root.join(".cyber").exists());
}

#[test]
fn source_edits_and_same_content_replacement_invalidate_review() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let (roots, source) = fixture(&root, b"original-token");
    let snapshot = SourceSnapshot::read(&roots, &source).unwrap();
    std::fs::write(&source.path, b"modified-token").unwrap();
    let error = snapshot.verify().unwrap_err().to_string();
    assert!(!error.contains("token"));
    let snapshot = SourceSnapshot::read(&roots, &source).unwrap();
    std::fs::rename(&source.path, source.path.with_extension("old")).unwrap();
    std::fs::write(&source.path, b"modified-token").unwrap();
    assert!(snapshot.verify().is_err());
}

#[test]
fn parent_directory_replacement_invalidates_snapshot_bindings() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let (roots, source) = fixture(&root, b"same-content");
    let snapshot = SourceSnapshot::read(&roots, &source).unwrap();
    let parent = source.path.parent().unwrap();
    std::fs::rename(parent, roots.project_root.join("retained-source")).unwrap();
    std::fs::create_dir(parent).unwrap();
    std::fs::write(&source.path, b"same-content").unwrap();
    assert!(snapshot.verify().is_err());
}

#[test]
fn source_limits_encoding_and_inventory_admission_are_enforced() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let (roots, source) = fixture(&root, &vec![b'x'; 1024 * 1024 + 1]);
    assert!(
        SourceSnapshot::read(&roots, &source)
            .unwrap_err()
            .reason
            .contains("limit")
    );
    std::fs::write(&source.path, [0xff, 0xfe]).unwrap();
    let snapshot = SourceSnapshot::read(&roots, &source).unwrap();
    assert_eq!(snapshot.bytes(), [0xff, 0xfe]);
    assert!(snapshot.text().unwrap_err().reason.contains("UTF-8"));
    let outside = root.join("private-source");
    std::fs::write(&outside, "private-token").unwrap();
    let forged = SourceFile {
        path: outside,
        ..source.clone()
    };
    let error = SourceSnapshot::read(&roots, &forged).unwrap_err();
    assert!(error.reason.contains("inventory"));
    assert!(!error.to_string().contains("private-token"));
    let forged = SourceFile {
        path: PathBuf::from("relative-source"),
        ..source
    };
    assert!(SourceSnapshot::read(&roots, &forged).is_err());
}

#[test]
fn hard_linked_source_files_cannot_be_read_as_independent_reviewed_inputs() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let (roots, source) = fixture(&root, b"private-token");
    std::fs::hard_link(&source.path, roots.project_root.join("another-link")).unwrap();
    assert!(SourceSnapshot::read(&roots, &source).is_err());
}

#[cfg(unix)]
#[test]
fn linked_replacement_is_refused_without_returning_the_target_content() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let (roots, source) = fixture(&root, b"old");
    let snapshot = SourceSnapshot::read(&roots, &source).unwrap();
    let outside = root.join("outside");
    std::fs::create_dir(&outside).unwrap();
    std::fs::write(outside.join("settings.json"), "private-token").unwrap();
    std::fs::rename(
        source.path.parent().unwrap(),
        roots.project_root.join("retained-source"),
    )
    .unwrap();
    std::os::unix::fs::symlink(&outside, source.path.parent().unwrap()).unwrap();
    let error = snapshot.verify().unwrap_err();
    assert!(!error.to_string().contains("private-token"));
    assert!(SourceSnapshot::read(&roots, &source).is_err());
}

#[cfg(windows)]
#[test]
fn junction_replacement_is_refused_before_returning_target_data() {
    let temp = tempfile::tempdir().unwrap();
    let (roots, source) = fixture(temp.path(), b"old");
    let snapshot = SourceSnapshot::read(&roots, &source).unwrap();
    let outside = temp.path().join("outside");
    std::fs::create_dir(&outside).unwrap();
    std::fs::write(outside.join("settings.json"), "private-token").unwrap();
    let parent = roots.project_root.join(".claude");
    std::fs::rename(&parent, roots.project_root.join("retained-source")).unwrap();
    let output = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(&parent)
        .arg(&outside)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(snapshot.verify().is_err());
    assert!(SourceSnapshot::read(&roots, &source).is_err());
}

#[test]
fn reviewed_snapshot_allows_project_root_rename_and_refuses_same_content_replacement() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let (roots, source) = fixture(&root, b"same-content");
    let snapshot = SourceSnapshot::read(&roots, &source).unwrap();
    std::fs::rename(&roots.project_root, root.join("retained-project")).unwrap();
    std::fs::create_dir_all(roots.project_root.join(".claude")).unwrap();
    std::fs::write(&source.path, b"same-content").unwrap();
    assert!(snapshot.verify().is_err());
    assert_eq!(snapshot.bytes(), b"same-content");
}

#[test]
fn ancestor_container_rename_and_same_content_replacement_invalidate_review() {
    let temp = tempfile::tempdir().unwrap();
    let outer = temp.path().canonicalize().unwrap();
    let container = outer.join("container");
    std::fs::create_dir(&container).unwrap();
    let (roots, source) = fixture(&container, b"same-content");
    let snapshot = SourceSnapshot::read(&roots, &source).unwrap();
    std::fs::rename(&container, outer.join("retained-container")).unwrap();
    std::fs::create_dir(&container).unwrap();
    let (replacement_roots, replacement_source) = fixture(&container, b"same-content");
    let replacement = SourceSnapshot::read(&replacement_roots, &replacement_source).unwrap();
    assert_eq!(replacement.bytes(), snapshot.bytes());
    assert!(snapshot.verify().is_err());
    replacement.verify().unwrap();
}

#[test]
fn sibling_reviews_allow_directory_moves_and_refuse_recreated_source_bindings() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let (roots, first) = fixture(&root, b"first-private-value");
    let parent = first.path.parent().unwrap();
    std::fs::write(parent.join("settings.local.json"), "second-private-value").unwrap();
    let second = discover_sources(&roots)
        .unwrap()
        .files
        .into_iter()
        .find(|f| f.path.ends_with("settings.local.json"))
        .unwrap();
    let first_review = SourceSnapshot::read(&roots, &first).unwrap();
    let second_review = SourceSnapshot::read(&roots, &second).unwrap();
    std::fs::rename(parent, roots.project_root.join("retained-siblings")).unwrap();
    std::fs::create_dir(parent).unwrap();
    std::fs::write(&first.path, first_review.bytes()).unwrap();
    std::fs::write(&second.path, second_review.bytes()).unwrap();
    for review in [&first_review, &second_review] {
        assert!(review.verify().is_err());
        assert!(!format!("{review:?}").contains("private-value"));
    }
    SourceSnapshot::read(&roots, &first)
        .unwrap()
        .verify()
        .unwrap();
    SourceSnapshot::read(&roots, &second)
        .unwrap()
        .verify()
        .unwrap();
}

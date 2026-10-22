//! The git files the build script watches (`build/git_watch.rs`).
#[path = "../build/git_watch.rs"]
mod git_watch;

use std::path::PathBuf;

fn refs(git: &std::path::Path) -> Vec<(String, PathBuf)> {
    [
        ("HEAD", "HEAD"),
        ("packed-refs", "packed-refs"),
        ("refs/heads/feat/x", "refs/heads/feat/x"),
    ]
    .iter()
    .map(|(name, path)| (name.to_string(), git.join(path)))
    .collect()
}

/// Cargo reruns a build script on every build when a watched path is missing, so a CI
/// checkout without `packed-refs` rebuilt every crate above `cyber-core` each time.
#[test]
fn only_existing_git_paths_are_watched() {
    let dir = tempfile::tempdir().unwrap();
    let git = dir.path();
    std::fs::create_dir_all(git.join("refs/heads")).unwrap();
    std::fs::write(git.join("HEAD"), "ref: refs/heads/feat/x\n").unwrap();

    // No packed-refs; the branch is only packed: its nearest directory is watched.
    assert_eq!(
        git_watch::watched(&refs(git)),
        [git.join("HEAD"), git.join("refs/heads")]
    );

    std::fs::write(git.join("packed-refs"), "").unwrap();
    std::fs::create_dir_all(git.join("refs/heads/feat")).unwrap();
    std::fs::write(git.join("refs/heads/feat/x"), "0123456\n").unwrap();
    assert_eq!(
        git_watch::watched(&refs(git)),
        [
            git.join("HEAD"),
            git.join("packed-refs"),
            git.join("refs/heads/feat/x")
        ]
    );
}

use std::path::Path;

use cyber_core::worktrees::{Branch, CarryPlan, Managed, Name, Settings, plan_carry};
use futures::executor::block_on;

use super::Fixture;

fn plan(fixture: &Fixture, readable: &(dyn Fn(&Path) -> bool + Sync), limit_mb: u64) -> CarryPlan {
    block_on(plan_carry(
        &fixture.execution,
        &fixture.repo,
        readable,
        limit_mb,
    ))
    .unwrap()
    .unwrap()
}

fn create(fixture: &Fixture, name: &str, settings: &Settings, carry: &CarryPlan) -> Managed {
    let name = Name::parse(name).unwrap();
    let branch = format!("cyber/{}", name.as_str());
    block_on(fixture.repository().create_carrying(
        &fixture.execution,
        settings,
        &fixture.data,
        "prj_test",
        Branch {
            name: &name,
            reference: &branch,
        },
        Some(carry),
    ))
    .unwrap()
}

fn in_child(managed: &Managed, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .current_dir(&managed.path)
        .args(args)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    String::from_utf8(output.stdout).unwrap().trim_end().into()
}

fn read(managed: &Managed, path: &str) -> Option<String> {
    std::fs::read_to_string(managed.path.join(path)).ok()
}

/// Commit five files, then stage, edit, delete and rename them and add untracked,
/// ignored and secret files; returns the committed `HEAD`.
fn dirty_parent(fixture: &Fixture) -> String {
    std::fs::write(fixture.repo.join(".gitignore"), "*.log\n").unwrap();
    for name in [
        "staged.txt",
        "unstaged.txt",
        "deleted.txt",
        "moved.txt",
        "renamed.txt",
    ] {
        std::fs::write(fixture.repo.join(name), name).unwrap();
    }
    fixture.git(&["add", "."]);
    fixture.git(&["commit", "--quiet", "-m", "files"]);
    let head = fixture.git(&["rev-parse", "HEAD"]);

    std::fs::write(fixture.repo.join("staged.txt"), "staged edit").unwrap();
    fixture.git(&["add", "staged.txt"]);
    std::fs::write(fixture.repo.join("unstaged.txt"), "unstaged edit").unwrap();
    std::fs::remove_file(fixture.repo.join("deleted.txt")).unwrap();
    fixture.git(&["mv", "moved.txt", "moved-staged.txt"]);
    std::fs::rename(
        fixture.repo.join("renamed.txt"),
        fixture.repo.join("renamed-plain.txt"),
    )
    .unwrap();
    std::fs::create_dir(fixture.repo.join("new")).unwrap();
    std::fs::write(fixture.repo.join("new/untracked.txt"), "untracked").unwrap();
    std::fs::write(fixture.repo.join("build.log"), "ignored").unwrap();
    std::fs::write(fixture.repo.join(".env"), "SECRET=1").unwrap();
    head
}

#[test]
fn carries_staged_unstaged_untracked_deletions_and_renames_as_the_child_base() {
    let fixture = Fixture::new();
    let head = dirty_parent(&fixture);
    let parent_status = fixture.git(&["status", "--porcelain"]);

    let readable = |path: &Path| path != Path::new(".env");
    let carry = plan(&fixture, &readable, 100);
    assert_eq!(carry.head, head);
    assert_eq!(carry.skipped, vec![Path::new(".env").to_path_buf()]);
    let managed = create(&fixture, "carried", &Settings::default(), &carry);

    assert_ne!(managed.base, head);
    assert_eq!(in_child(&managed, &["rev-parse", "HEAD"]), managed.base);
    assert_eq!(in_child(&managed, &["rev-parse", "HEAD^"]), head);
    let expected = [
        ("staged.txt", Some("staged edit")),
        ("unstaged.txt", Some("unstaged edit")),
        ("new/untracked.txt", Some("untracked")),
        ("moved-staged.txt", Some("moved.txt")),
        ("renamed-plain.txt", Some("renamed.txt")),
        ("deleted.txt", None),
        ("moved.txt", None),
        ("renamed.txt", None),
        ("build.log", None),
        (".env", None),
    ];
    for (path, content) in expected {
        assert_eq!(read(&managed, path).as_deref(), content, "{path}");
    }
    // The carried work is the child's base: the child starts clean.
    assert_eq!(in_child(&managed, &["status", "--porcelain"]), "");
    // The parent checkout, its index included, is untouched.
    assert_eq!(fixture.git(&["status", "--porcelain"]), parent_status);
    assert_eq!(fixture.git(&["rev-parse", "HEAD"]), head);
}

#[test]
fn worktreeinclude_copies_stay_uncommitted_next_to_carried_work() {
    let fixture = Fixture::new();
    std::fs::write(fixture.repo.join(".gitignore"), ".env.local\n").unwrap();
    std::fs::write(fixture.repo.join(".worktreeinclude"), ".env.local\n").unwrap();
    fixture.git(&["add", "."]);
    fixture.git(&["commit", "--quiet", "-m", "include"]);
    std::fs::write(fixture.repo.join(".env.local"), "LOCAL=1").unwrap();
    std::fs::write(fixture.repo.join("tracked.txt"), "edited").unwrap();

    let carry = plan(&fixture, &|_: &Path| true, 100);
    let managed = create(&fixture, "with-include", &Settings::default(), &carry);
    assert_eq!(managed.included.len(), 1);
    assert_eq!(read(&managed, ".env.local").as_deref(), Some("LOCAL=1"));
    assert_eq!(read(&managed, "tracked.txt").as_deref(), Some("edited"));
    let committed = in_child(&managed, &["show", "--name-only", "--format=", "HEAD"]);
    assert_eq!(committed, "tracked.txt");
}

#[test]
fn nothing_is_carried_onto_a_different_base_or_when_clean() {
    let fixture = Fixture::new();
    let old = fixture.git(&["rev-parse", "HEAD"]);
    std::fs::write(fixture.repo.join("tracked.txt"), "second").unwrap();
    fixture.git(&["commit", "--quiet", "-am", "second"]);

    let clean = plan(&fixture, &|_: &Path| true, 100);
    assert!(clean.is_empty());
    let managed = create(&fixture, "clean", &Settings::default(), &clean);
    assert_eq!(managed.base, fixture.git(&["rev-parse", "HEAD"]));

    std::fs::write(fixture.repo.join("tracked.txt"), "dirty").unwrap();
    let dirty = plan(&fixture, &|_: &Path| true, 100);
    let settings = Settings {
        base: old.clone(),
        ..Settings::default()
    };
    let managed = create(&fixture, "old-base", &settings, &dirty);
    assert_eq!(managed.base, old);
    assert_eq!(read(&managed, "tracked.txt").as_deref(), Some("base"));
}

#[test]
fn carrying_more_than_the_limit_fails_with_the_setting_named() {
    let fixture = Fixture::new();
    std::fs::write(
        fixture.repo.join("large.bin"),
        vec![0_u8; 2 * 1024 * 1024 + 1],
    )
    .unwrap();
    let error = block_on(plan_carry(
        &fixture.execution,
        &fixture.repo,
        &|_: &Path| true,
        2,
    ))
    .unwrap_err()
    .to_string();
    assert_eq!(
        error,
        "Uncommitted changes to carry into the isolated worktree total 3 MiB, over the 2 MiB limit (worktrees.carry_limit_mb); commit, ignore or remove large files, or raise the limit"
    );
    // An ignored file does not count.
    std::fs::write(fixture.repo.join(".gitignore"), "*.bin\n").unwrap();
    let carry = plan(&fixture, &|_: &Path| true, 2);
    assert_eq!(carry.files, vec![Path::new(".gitignore").to_path_buf()]);
}

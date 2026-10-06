use std::io;

use cyber_core::worktrees::{Managed, Name, Repository, Settings};
use sha2::{Digest, Sha256};

use super::{Fixture, Mode};

fn pending(fixture: &Fixture, name: &str) -> Managed {
    serde_json::from_slice(
        &std::fs::read(
            fixture
                .repo
                .join(format!(".git/cyber-worktrees/{name}.json")),
        )
        .unwrap(),
    )
    .unwrap()
}

#[test]
fn includes_ignored_patterns_and_preserves_tracked_files_and_reuse_edits() {
    let fixture = Fixture::new();
    std::fs::write(fixture.repo.join(".gitignore"), ".env*\n").unwrap();
    std::fs::create_dir(fixture.repo.join("config")).unwrap();
    std::fs::write(
        fixture.repo.join(".worktreeinclude"),
        "# local data\n.env*\n/config/local.*\n!*.secret\n/with space.env\n/λ.env\ntracked.txt\n",
    )
    .unwrap();
    for (path, body) in [
        (".env.local", "environment"),
        ("config/local.user", "local"),
        ("with space.env", "spaces"),
        ("λ.env", "unicode"),
        (".env.secret", "excluded"),
        ("config/local.secret", "excluded"),
        ("config/other.txt", "excluded"),
    ] {
        std::fs::write(fixture.repo.join(path), body).unwrap();
    }
    fixture.git(&["rm", "--cached", "tracked.txt"]);
    std::fs::write(
        fixture.repo.join("tracked.txt"),
        "source differs from tracked base",
    )
    .unwrap();
    let repository = fixture.repository();
    let name = Name::parse("includes").unwrap();
    let managed = fixture
        .create(&repository, &name, &Settings::default())
        .unwrap();
    assert_eq!(managed.included.len(), 4);
    assert_eq!(
        std::fs::read_to_string(managed.path.join("tracked.txt")).unwrap(),
        "base"
    );
    assert!(!managed.path.join("config/local.secret").exists());
    assert!(!managed.path.join(".env.secret").exists());
    assert!(!managed.path.join("config/other.txt").exists());
    for included in &managed.included {
        let body = std::fs::read(managed.path.join(&included.path)).unwrap();
        assert_eq!(included.sha256, format!("{:x}", Sha256::digest(body)));
    }
    assert_eq!(
        std::fs::read_to_string(managed.path.join("with space.env")).unwrap(),
        "spaces"
    );
    assert_eq!(
        std::fs::read_to_string(managed.path.join("λ.env")).unwrap(),
        "unicode"
    );
    std::fs::write(managed.path.join(".env.local"), "user edit").unwrap();
    std::fs::write(fixture.repo.join(".env.local"), "new source").unwrap();
    assert_eq!(
        fixture
            .create(&repository, &name, &Settings::default())
            .unwrap(),
        managed
    );
    assert_eq!(
        std::fs::read_to_string(managed.path.join(".env.local")).unwrap(),
        "user edit"
    );
}

#[test]
fn inclusion_collision_preserves_user_file_and_pending_ownership() {
    let mut fixture = Fixture::new();
    std::fs::write(fixture.repo.join(".worktreeinclude"), ".env\n").unwrap();
    std::fs::write(fixture.repo.join(".env"), "source").unwrap();
    let repository = fixture.repository();
    fixture.execution.mode = Mode::CollideWithIncludedFile;
    assert_eq!(
        fixture
            .create(
                &repository,
                &Name::parse("collision").unwrap(),
                &Settings::default()
            )
            .unwrap_err()
            .kind(),
        io::ErrorKind::AlreadyExists
    );
    let managed = pending(&fixture, "collision");
    assert!(!managed.ready);
    assert_eq!(
        std::fs::read_to_string(managed.path.join(".env")).unwrap(),
        "user file during setup"
    );
}

#[test]
fn unsafe_git_enumeration_cannot_copy_outside_the_source() {
    let mut fixture = Fixture::new();
    std::fs::write(fixture.repo.join(".worktreeinclude"), "**\n").unwrap();
    std::fs::write(fixture._temp.path().join("outside"), "outside source").unwrap();
    let repository = fixture.repository();
    fixture.execution.mode = Mode::UnsafeUntrackedPath(b"../outside\0");
    assert_eq!(
        fixture
            .create(
                &repository,
                &Name::parse("unsafe").unwrap(),
                &Settings::default()
            )
            .unwrap_err()
            .kind(),
        io::ErrorKind::InvalidData
    );
    let managed = pending(&fixture, "unsafe");
    assert!(!managed.ready);
    assert!(!managed.path.join("outside").exists());
    assert_eq!(
        std::fs::read_to_string(fixture._temp.path().join("outside")).unwrap(),
        "outside source"
    );
}

#[cfg(unix)]
#[test]
fn matched_symlink_is_refused_without_reading_its_outside_target() {
    let fixture = Fixture::new();
    std::fs::write(fixture.repo.join(".worktreeinclude"), ".env\n").unwrap();
    let outside = fixture._temp.path().join("outside");
    std::fs::write(&outside, "outside").unwrap();
    std::os::unix::fs::symlink(&outside, fixture.repo.join(".env")).unwrap();
    assert_eq!(
        fixture
            .create(
                &fixture.repository(),
                &Name::parse("symlink").unwrap(),
                &Settings::default()
            )
            .unwrap_err()
            .kind(),
        io::ErrorKind::InvalidData
    );
    assert!(!pending(&fixture, "symlink").path.join(".env").exists());
    assert_eq!(std::fs::read_to_string(outside).unwrap(), "outside");
}

#[cfg(unix)]
#[test]
fn included_files_preserve_private_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new();
    std::fs::write(fixture.repo.join(".worktreeinclude"), ".env\n").unwrap();
    std::fs::write(fixture.repo.join(".env"), "private").unwrap();
    std::fs::set_permissions(
        fixture.repo.join(".env"),
        std::fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    let managed = fixture
        .create(
            &fixture.repository(),
            &Name::parse("permissions").unwrap(),
            &Settings::default(),
        )
        .unwrap();
    assert_eq!(
        std::fs::metadata(managed.path.join(".env"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
}

#[test]
fn linked_creation_includes_from_the_primary_checkout() {
    let fixture = Fixture::new();
    std::fs::write(fixture.repo.join(".worktreeinclude"), ".env\n").unwrap();
    std::fs::write(fixture.repo.join(".env"), "primary checkout").unwrap();
    let parent = fixture
        .create(
            &fixture.repository(),
            &Name::parse("parent").unwrap(),
            &Settings::default(),
        )
        .unwrap();
    std::fs::write(parent.path.join(".env"), "parent user edits").unwrap();
    let repository =
        futures::executor::block_on(Repository::discover(&fixture.execution, &parent.path))
            .unwrap();
    let child = fixture
        .create(
            &repository,
            &Name::parse("child").unwrap(),
            &Settings::default(),
        )
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(child.path.join(".env")).unwrap(),
        "primary checkout"
    );
    assert_eq!(
        std::fs::read_to_string(parent.path.join(".env")).unwrap(),
        "parent user edits"
    );
}

#[cfg(windows)]
#[test]
fn alternate_stream_paths_are_refused_before_copying() {
    let mut fixture = Fixture::new();
    std::fs::write(fixture.repo.join(".worktreeinclude"), "**\n").unwrap();
    let repository = fixture.repository();
    fixture.execution.mode = Mode::UnsafeUntrackedPath(b"tracked.txt:outside\0");
    assert_eq!(
        fixture
            .create(
                &repository,
                &Name::parse("stream").unwrap(),
                &Settings::default()
            )
            .unwrap_err()
            .kind(),
        io::ErrorKind::InvalidData
    );
    let managed = pending(&fixture, "stream");
    assert!(!managed.ready);
    assert_eq!(
        std::fs::read_to_string(managed.path.join("tracked.txt")).unwrap(),
        "base"
    );
    assert!(std::fs::read(managed.path.join("tracked.txt:outside")).is_err());
}

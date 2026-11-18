use super::*;

fn write(root: &Path, rel: &str, text: &str) {
    let path = absolute(root, rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn run(root: &Path, settings: &RepoMapSettings) -> Walked {
    walk(
        root,
        &Selector::new(settings),
        settings.max_files,
        None,
        Duration::ZERO,
    )
}

fn reason_of<'a>(walked: &'a Walked, path: &str) -> Option<&'a Entry> {
    walked.entries.iter().find(|e| e.path == path)
}

#[test]
fn generated_protobuf_skipped() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "api/user.go", "package api\n");
    write(dir.path(), "api/user.pb.go", "package api\n");
    let walked = run(dir.path(), &RepoMapSettings::default());
    assert_eq!(reason_of(&walked, "api/user.go").unwrap().excluded, None);
    assert_eq!(
        reason_of(&walked, "api/user.pb.go").unwrap().excluded,
        Some(Reason::DefaultExclude)
    );
}

#[test]
fn include_overrides_a_default_exclude() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "vendor/acme/lib.rs", "fn a() {}\n");
    write(dir.path(), "vendor/other/lib.rs", "fn b() {}\n");
    write(dir.path(), "src/main.rs", "fn main() {}\n");
    let settings = RepoMapSettings {
        include: vec!["vendor/acme/**".into()],
        ..Default::default()
    };
    let walked = run(dir.path(), &settings);
    assert_eq!(
        reason_of(&walked, "vendor/acme/lib.rs").unwrap().excluded,
        None
    );
    assert_eq!(
        reason_of(&walked, "vendor/other/lib.rs").unwrap().excluded,
        Some(Reason::DefaultExclude)
    );
    // Without an include the directory is pruned and recorded once.
    let walked = run(dir.path(), &RepoMapSettings::default());
    assert_eq!(
        reason_of(&walked, "vendor/").unwrap().excluded,
        Some(Reason::DefaultExclude)
    );
    assert!(reason_of(&walked, "vendor/acme/lib.rs").is_none());
}

#[test]
fn linguist_vendored_excluded() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        ".gitattributes",
        "libs/** linguist-vendored\n*.gen.rs linguist-generated=true\n",
    );
    write(dir.path(), "libs/x/lib.rs", "fn a() {}\n");
    write(dir.path(), "src/model.gen.rs", "fn b() {}\n");
    write(
        dir.path(),
        "src/sub/.gitattributes",
        "keep.rs -linguist-generated\n",
    );
    write(dir.path(), "src/main.rs", "fn main() {}\n");
    let walked = run(dir.path(), &RepoMapSettings::default());
    assert_eq!(
        reason_of(&walked, "libs/x/lib.rs").unwrap().excluded,
        Some(Reason::Vendored)
    );
    assert_eq!(
        reason_of(&walked, "src/model.gen.rs").unwrap().excluded,
        Some(Reason::Generated)
    );
    assert_eq!(reason_of(&walked, "src/main.rs").unwrap().excluded, None);
}

#[cfg(unix)]
#[test]
fn symlink_outside_location_not_followed() {
    let outside = tempfile::tempdir().unwrap();
    write(outside.path(), "secret.rs", "fn secret() {}\n");
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "src/main.rs", "fn main() {}\n");
    std::os::unix::fs::symlink(outside.path(), dir.path().join("linked")).unwrap();
    std::os::unix::fs::symlink(
        outside.path().join("secret.rs"),
        dir.path().join("src/secret.rs"),
    )
    .unwrap();
    let walked = run(dir.path(), &RepoMapSettings::default());
    let paths: Vec<_> = walked.entries.iter().map(|e| e.path.as_str()).collect();
    assert_eq!(paths, vec!["src/main.rs"]);
}

#[test]
fn ignore_files_nested_repositories_and_config_excludes_apply() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join(".git")).unwrap();
    write(dir.path(), ".gitignore", "ignored/\n");
    write(dir.path(), "ignored/a.rs", "fn a() {}\n");
    write(dir.path(), "nested/.git/HEAD", "ref\n");
    write(dir.path(), "nested/b.rs", "fn b() {}\n");
    write(dir.path(), "gen/c.rs", "fn c() {}\n");
    write(dir.path(), "src/big.rs", &"x".repeat(64));
    write(dir.path(), "src/main.rs", "fn main() {}\n");
    let settings = RepoMapSettings {
        exclude: vec!["gen/**".into()],
        max_file_bytes: 32,
        ..Default::default()
    };
    let walked = run(dir.path(), &settings);
    let rows: Vec<_> = walked
        .entries
        .iter()
        .map(|e| (e.path.as_str(), e.excluded))
        .collect();
    assert_eq!(
        rows,
        vec![
            (".gitignore", None),
            ("gen/c.rs", Some(Reason::ConfigExclude)),
            ("src/big.rs", Some(Reason::TooLarge)),
            ("src/main.rs", None),
        ]
    );
}

#[test]
fn max_files_truncates_in_sorted_order() {
    let dir = tempfile::tempdir().unwrap();
    for name in ["c.rs", "a.rs", "b.rs", "d.rs"] {
        write(dir.path(), name, "fn x() {}\n");
    }
    let settings = RepoMapSettings {
        max_files: 2,
        ..Default::default()
    };
    let walked = run(dir.path(), &settings);
    let paths: Vec<_> = walked.entries.iter().map(|e| e.path.as_str()).collect();
    assert_eq!(paths, vec!["a.rs", "b.rs"]);
    assert!(walked.truncated);
}

#[test]
fn content_checks_find_binaries_and_generated_headers() {
    assert_eq!(content_reason(b"ab\0cd", false), Some(Reason::Binary));
    assert_eq!(content_reason(b"ab\0cd", true), Some(Reason::Binary));
    assert_eq!(
        content_reason(b"// Code generated. DO NOT EDIT.\npackage x\n", false),
        Some(Reason::Generated)
    );
    assert_eq!(
        content_reason(b"// Code generated. DO NOT EDIT.\npackage x\n", true),
        None
    );
    assert_eq!(
        content_reason(b"1\n2\n3\n4\n5\n@generated\n", false),
        None,
        "only the first five lines count"
    );
}

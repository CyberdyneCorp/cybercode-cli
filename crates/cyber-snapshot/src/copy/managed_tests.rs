//! Registration permits observation, never unowned source mutation.
use super::*;

fn git(root: &Path, args: &[&str]) {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
}

#[test]
fn registered_data_source_requires_worker_and_keeps_private_boundaries() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let repo = root.join("repo");
    let data = root.join("data");
    std::fs::create_dir(&repo).unwrap();
    std::fs::create_dir(&data).unwrap();
    git(&repo, &["init", "--quiet"]);
    git(
        &repo,
        &[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.test",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--allow-empty",
            "--quiet",
            "-m",
            "Initial",
        ],
    );
    let source = data.join("worktrees/managed");
    git(
        &repo,
        &[
            "worktree",
            "add",
            "--quiet",
            "-b",
            "managed",
            source.to_str().unwrap(),
        ],
    );
    let options = Options {
        max_file: 65536,
        max_total: 4 * 1024 * 1024,
        ignore: vec![],
        private_roots: vec![],
    };
    assert!(Store::open(&data, source.to_str().unwrap(), options.clone()).is_err());
    let managed = cyber_core::worktrees::Managed {
        id: "wt_fixture".into(),
        name: "managed".into(),
        path: source.clone(),
        branch: "managed".into(),
        base: "master".into(),
        common_dir: repo.join(".git"),
        ready: true,
        included: vec![],
    };
    let records = repo.join(".git/cyber-worktrees");
    std::fs::create_dir(&records).unwrap();
    let record = records.join("managed.json");
    std::fs::write(&record, serde_json::to_vec(&managed).unwrap()).unwrap();
    let mut protected_options = options.clone();
    protected_options.private_roots.push(source.clone());
    assert!(Store::open(&data, source.to_str().unwrap(), protected_options).is_err());
    let store = Store::open(&data, source.to_str().unwrap(), options.clone()).unwrap();
    std::fs::write(source.join("tracked"), "owner").unwrap();
    assert!(store.capture().unwrap().files.contains_key("tracked"));
    assert!(!store.private_data_path(&source.join("tracked")));
    assert!(store.private_data_path(&data.join("cyber.db")));
    assert!(store.private_data_path(&data.join("snapshot/global")));
    assert!(store.check_link_target("link", b"tracked", &source).is_ok());
    assert!(
        store
            .check_link_target("link", b"../../cyber.db", &source)
            .is_err()
    );
    let error = store
        .prepare_journal(vec![], "restore_fixture", None, None, None)
        .unwrap_err();
    assert!(error.contains("owned snapshot worker"), "{error}");
    assert_eq!(
        std::fs::read_to_string(source.join("tracked")).unwrap(),
        "owner"
    );
    assert!(
        storage::names(&store.journals, ENTRY_LIMIT)
            .unwrap()
            .is_empty()
    );
    let mut changed = managed;
    changed.ready = false;
    std::fs::write(&record, serde_json::to_vec(&changed).unwrap()).unwrap();
    assert!(store.verify_root().is_err());
    drop(store);
    assert!(Store::open(&data, source.to_str().unwrap(), options).is_err());
}

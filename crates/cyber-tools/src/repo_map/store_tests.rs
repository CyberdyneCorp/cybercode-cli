use super::*;

fn row(path: &str, sha: &str) -> FileRow {
    FileRow {
        path: path.into(),
        size: 1,
        mtime_ns: 1,
        sha: Some(sha.into()),
        lang: Some("rust".into()),
        excluded: None,
    }
}

fn blob(sha: &str) -> (String, String, String, Vec<u8>) {
    let tags = serde_json::to_vec(&Tags::default()).unwrap();
    (
        sha.into(),
        super::super::lang::Lang::Rust.extractor_version().into(),
        "rust".into(),
        tags,
    )
}

#[test]
fn schema_mismatch_rebuilds() {
    let cache = tempfile::tempdir().unwrap();
    {
        let mut db = Store::open(cache.path()).unwrap();
        let checkout = db.checkout(Path::new("/repo")).unwrap();
        db.commit(&checkout.root_id, &[row("a.rs", "aa")], &[blob("aa")], &[])
            .unwrap();
        db.conn.pragma_update(None, "user_version", 99).unwrap();
    }
    let db = Store::open(cache.path()).unwrap();
    let version: i64 = db
        .conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, SCHEMA_VERSION);
    assert!(
        db.file_rows(&root_id(Path::new("/repo")))
            .unwrap()
            .is_empty()
    );
}

#[test]
fn gc_evicts_least_recently_used_over_cap() {
    let cache = tempfile::tempdir().unwrap();
    let mut db = Store::open(cache.path()).unwrap();
    let old = db.checkout(Path::new("/old")).unwrap().root_id;
    let new = db.checkout(Path::new("/new")).unwrap().root_id;
    let rows: Vec<FileRow> = (0..200)
        .map(|i| row(&format!("f{i}.rs"), &format!("o{i}")))
        .collect();
    let blobs: Vec<_> = (0..200).map(|i| blob(&format!("o{i}"))).collect();
    db.commit(&old, &rows, &blobs, &[]).unwrap();
    let rows: Vec<FileRow> = (0..200)
        .map(|i| row(&format!("f{i}.rs"), &format!("n{i}")))
        .collect();
    let blobs: Vec<_> = (0..200).map(|i| blob(&format!("n{i}"))).collect();
    db.commit(&new, &rows, &blobs, &[]).unwrap();
    db.touch(&old, now_ms() - 1_000);
    let used = db.used_bytes().unwrap();
    db.gc(used - 1, &new).unwrap();
    assert!(
        db.checkout_row(&old).unwrap().is_none(),
        "LRU checkout evicted"
    );
    assert!(db.file_rows(&old).unwrap().is_empty());
    assert_eq!(
        db.file_rows(&new).unwrap().len(),
        200,
        "current checkout kept"
    );
}

#[test]
fn gc_drops_checkouts_unused_for_thirty_days_and_orphan_blobs() {
    let cache = tempfile::tempdir().unwrap();
    let mut db = Store::open(cache.path()).unwrap();
    let old = db.checkout(Path::new("/old")).unwrap().root_id;
    db.commit(
        &old,
        &[row("a.rs", "aa")],
        &[blob("aa"), blob("orphan")],
        &[],
    )
    .unwrap();
    db.touch(&old, now_ms() - UNUSED_CHECKOUT.as_millis() as i64 - 1);
    db.gc(SIZE_CAP, "none").unwrap();
    assert!(db.checkout_row(&old).unwrap().is_none());
    let blobs: i64 = db
        .conn
        .query_row("SELECT COUNT(*) FROM blobs", [], |r| r.get(0))
        .unwrap();
    assert_eq!(blobs, 0);
}

#[cfg(unix)]
#[test]
fn index_file_mode_is_0600() {
    use std::os::unix::fs::PermissionsExt;
    let cache = tempfile::tempdir().unwrap();
    let db = Store::open(cache.path()).unwrap();
    let mode = std::fs::metadata(db.path()).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
    let dir = std::fs::metadata(db.path().parent().unwrap()).unwrap();
    assert_eq!(dir.permissions().mode() & 0o777, 0o700);
}

#[test]
fn indexed_lists_path_only_and_tagged_files_in_path_order() {
    let cache = tempfile::tempdir().unwrap();
    let mut db = Store::open(cache.path()).unwrap();
    let id = db.checkout(Path::new("/repo")).unwrap().root_id;
    let mut timeout = row("b.rs", "bb");
    timeout.excluded = Some(PARSE_TIMEOUT.into());
    let mut other = row("c.sv", "cc");
    other.lang = Some("other".into());
    let mut binary = row("d.bin", "dd");
    binary.excluded = Some("binary".into());
    db.commit(
        &id,
        &[row("a.rs", "aa"), timeout, other, binary],
        &[blob("aa")],
        &[],
    )
    .unwrap();
    let files = db.indexed(&id).unwrap();
    let summary: Vec<_> = files
        .iter()
        .map(|f| (f.path.as_str(), f.tags.is_some(), f.parse_timeout))
        .collect();
    assert_eq!(
        summary,
        vec![
            ("a.rs", true, false),
            ("b.rs", false, true),
            ("c.sv", false, false)
        ]
    );
    assert_eq!(
        db.excluded(&id).unwrap(),
        vec![("binary".into(), 1), (PARSE_TIMEOUT.into(), 1)]
    );
}

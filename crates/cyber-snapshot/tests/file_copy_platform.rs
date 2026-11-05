//! Unsupported native storage stays explicit without failing conversation capture.
#[cfg(windows)]
#[tokio::test]
async fn non_git_windows_capture_is_unavailable_with_an_actionable_reason() {
    use cyber_server::runtime::Snapshots;
    use std::sync::Arc;
    let root = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let snapshots =
        cyber_snapshot::GitSnapshots::new(data.path().into(), Arc::new(|_| serde_json::json!({})));
    let directory = root.path().display().to_string();
    assert!(snapshots.track(&directory).await.unwrap().is_none());
    assert!(
        snapshots
            .unavailable_reason(&directory)
            .unwrap()
            .contains("native private storage")
    );
    assert!(snapshots.track(&directory).await.unwrap().is_none());
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
}

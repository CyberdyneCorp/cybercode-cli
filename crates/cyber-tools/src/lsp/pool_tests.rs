use super::*;
use cyber_core::intelligence::{InstallMethod, ServerDefinition};
use serde_json::json;
use std::sync::atomic::{AtomicUsize, Ordering};

fn server() -> DetectedServer {
    DetectedServer {
        definition: ServerDefinition {
            id: "fixture".into(),
            extensions: vec![".rs".into()],
            root_markers: vec![".root".into()],
            command: vec!["python3".into()],
            env: BTreeMap::new(),
            initialization_options: Some(json!({"fixture":true})),
            install: InstallMethod::Custom,
        },
        enabled: true,
        installed: true,
        executable: Some("/fixture/python3".into()),
    }
}

fn launcher(mode: &'static str, launches: Arc<AtomicUsize>) -> LaunchFn {
    Arc::new(move |_, cancel| {
        let launches = launches.clone();
        Box::pin(async move {
            if cancel.is_cancelled() {
                return Err(LaunchError {
                    error: unavailable(),
                    acknowledged: true,
                });
            }
            let process = super::super::connection::tests::process(mode).await;
            launches.fetch_add(1, Ordering::SeqCst);
            Ok(AuthorizedProcess {
                process,
                keepalive: Box::new(()),
            })
        })
    })
}

fn fixture() -> (tempfile::TempDir, PathBuf) {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("file.rs");
    std::fs::write(&file, "fn main() {}\n").unwrap();
    (root, file)
}

async fn wait_for(mut condition: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(3), async {
        while !condition() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn concurrent_starts_and_requests_share_one_real_process() {
    let (root, file) = fixture();
    let launches = Arc::new(AtomicUsize::new(0));
    let pool = Pool::new(
        root.path(),
        vec![server()],
        launcher("normal", launches.clone()),
    )
    .unwrap();
    assert!(pool.status().unwrap().is_empty());
    let mut clients = Vec::new();
    for _ in 0..24 {
        let pool = pool.clone();
        let file = file.clone();
        clients.push(tokio::spawn(async move {
            let handle = pool.ensure("fixture", &file).unwrap();
            handle.connected().await.unwrap();
            handle
                .request("fixture", json!({}), Duration::from_secs(3))
                .await
                .unwrap()
        }));
    }
    for client in clients {
        assert_eq!(client.await.unwrap()["text"], "λ🦀");
    }
    assert_eq!(launches.load(Ordering::SeqCst), 1);
    assert_eq!(pool.status().unwrap().len(), 1);
    assert_eq!(pool.status().unwrap()[0].status, ServerState::Connected);
    let stops = pool.close().await.unwrap();
    assert_eq!(stops.len(), 1);
    assert!(stops[0].acknowledged);
    assert!(pool.ensure("fixture", &file).is_err());
}

#[tokio::test]
async fn document_snapshots_open_once_change_versions_and_evict_bounded_state() {
    let (root, file) = fixture();
    let launches = Arc::new(AtomicUsize::new(0));
    let pool = Pool::new(
        root.path(),
        vec![server()],
        launcher("normal", launches.clone()),
    )
    .unwrap();
    let handle = pool.ensure("fixture", &file).unwrap();
    handle.open_document(&file, "λ🦀".into()).await.unwrap();
    handle.open_document(&file, "λ🦀".into()).await.unwrap();
    handle.open_document(&file, "changed".into()).await.unwrap();
    let outside = tempfile::NamedTempFile::new().unwrap();
    assert!(
        handle
            .open_document(outside.path(), "external".into())
            .await
            .is_err()
    );
    assert!(
        handle
            .open_document(
                &file,
                "x".repeat(super::super::documents::MAX_DOCUMENT_BYTES + 1)
            )
            .await
            .is_err()
    );
    for i in 0..128 {
        let path = root.path().join(format!("z{i:03}.rs"));
        std::fs::write(&path, "").unwrap();
        handle.open_document(&path, "bounded".into()).await.unwrap();
    }
    // Reopening the evicted document must not reuse an earlier server-generation version.
    handle.open_document(&file, "changed".into()).await.unwrap();
    handle.open_document(&file, "changed".into()).await.unwrap();
    handle
        .open_document(&file, "changed again".into())
        .await
        .unwrap();
    // A subsequent RPC witnesses that the server consumed all preceding notifications.
    handle
        .request("fixture", json!({}), Duration::from_secs(3))
        .await
        .unwrap();
    let events: Vec<Value> = std::fs::read_to_string(root.path().join("document-events"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(events.len(), 134);
    assert_eq!(events[0]["method"], "textDocument/didOpen");
    assert_eq!(events[0]["params"]["textDocument"]["languageId"], "rust");
    assert_eq!(events[0]["params"]["textDocument"]["text"], "λ🦀");
    assert_eq!(events[1]["method"], "textDocument/didChange");
    assert_eq!(events[1]["params"]["textDocument"]["version"], 2);
    assert_eq!(events[1]["params"]["contentChanges"][0]["text"], "changed");
    assert_eq!(events[129]["method"], "textDocument/didClose");
    assert_eq!(
        events[129]["params"]["textDocument"]["uri"],
        reqwest::Url::from_file_path(file.canonicalize().unwrap())
            .unwrap()
            .as_str()
    );
    assert_eq!(events[131]["method"], "textDocument/didClose");
    assert_eq!(events[132]["method"], "textDocument/didOpen");
    assert_eq!(events[132]["params"]["textDocument"]["version"], 131);
    assert_eq!(events[132]["params"]["textDocument"]["text"], "changed");
    assert_eq!(events[133]["method"], "textDocument/didChange");
    assert_eq!(events[133]["params"]["textDocument"]["version"], 132);
    let versions: Vec<i64> = events
        .iter()
        .filter_map(|event| event["params"]["textDocument"]["version"].as_i64())
        .collect();
    assert!(versions.windows(2).all(|pair| pair[0] < pair[1]));
    assert_eq!(launches.load(Ordering::SeqCst), 1);
    assert!(pool.close().await.unwrap()[0].acknowledged);
}

#[tokio::test]
async fn activity_before_warming_prevents_idle_until_the_last_guard_is_released() {
    let (root, file) = fixture();
    let locations = super::super::Locations::with_idle(
        Arc::new(|directory| {
            Pool::new(
                directory,
                vec![server()],
                launcher("normal", Arc::new(AtomicUsize::new(0))),
            )
        }),
        Duration::from_millis(150),
    );
    let first = locations.acquire(root.path()).unwrap();
    let second = locations.acquire(root.path()).unwrap();
    assert!(locations.status(root.path()).unwrap().is_empty());
    locations
        .warm(root.path(), file, "snapshot".into())
        .unwrap();
    wait_for(|| root.path().join("document-events").exists()).await;
    tokio::time::sleep(Duration::from_millis(350)).await;
    assert_eq!(
        locations.status(root.path()).unwrap()[0].status,
        ServerState::Connected
    );
    drop(first);
    tokio::time::sleep(Duration::from_millis(350)).await;
    assert_eq!(
        locations.status(root.path()).unwrap()[0].status,
        ServerState::Connected
    );
    drop(second);
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert_eq!(
        locations.status(root.path()).unwrap()[0].status,
        ServerState::Connected
    );
    wait_for(|| locations.status(root.path()).unwrap()[0].status == ServerState::Broken).await;
    assert!(locations.close().await.unwrap()[0].acknowledged);
}

#[tokio::test]
async fn location_idle_expiry_settles_then_allows_a_fresh_generation() {
    let (root, file) = fixture();
    let factories = Arc::new(AtomicUsize::new(0));
    let launches = Arc::new(AtomicUsize::new(0));
    let factory: super::super::PoolFactory = {
        let factories = factories.clone();
        let launches = launches.clone();
        Arc::new(move |directory| {
            factories.fetch_add(1, Ordering::SeqCst);
            Pool::new(
                directory,
                vec![server()],
                launcher("normal", launches.clone()),
            )
        })
    };
    let locations = super::super::Locations::with_idle(factory, Duration::from_millis(250));
    assert!(locations.status(root.path()).unwrap().is_empty());
    assert_eq!(factories.load(Ordering::SeqCst), 0);
    locations
        .warm(root.path(), file.clone(), "first".into())
        .unwrap();
    wait_for(|| root.path().join("document-events").exists()).await;
    wait_for(|| locations.status(root.path()).unwrap()[0].status == ServerState::Broken).await;
    wait_for(|| {
        locations
            .warm(root.path(), file.clone(), "second".into())
            .is_ok()
    })
    .await;
    wait_for(|| launches.load(Ordering::SeqCst) == 2).await;
    assert!(
        locations
            .close()
            .await
            .unwrap()
            .iter()
            .all(|s| s.acknowledged)
    );
    assert_eq!(factories.load(Ordering::SeqCst), 2);
    assert!(locations.warm(root.path(), file, "late".into()).is_err());
    assert!(
        locations
            .close()
            .await
            .unwrap()
            .iter()
            .all(|s| s.acknowledged)
    );
}

#[tokio::test]
async fn cancelled_location_close_retains_actual_native_shutdown_settlement() {
    let (root, file) = fixture();
    let locations = super::super::Locations::new(Arc::new(|directory| {
        Pool::new(
            directory,
            vec![server()],
            launcher("shutdown-hang", Arc::new(AtomicUsize::new(0))),
        )
    }));
    locations
        .warm(root.path(), file, "snapshot".into())
        .unwrap();
    wait_for(|| root.path().join("document-events").exists()).await;
    assert!(
        tokio::time::timeout(Duration::from_millis(20), locations.close())
            .await
            .is_err()
    );
    let settled = locations.close().await.unwrap();
    assert_eq!(settled.len(), 1);
    assert!(settled[0].acknowledged);
    assert_eq!(
        locations.status(root.path()).unwrap()[0].status,
        ServerState::Broken
    );
    assert_eq!(locations.close().await.unwrap().len(), 1);
}

#[tokio::test]
async fn scoped_close_retains_native_settlement_without_stopping_a_sibling() {
    let (first, file) = fixture();
    let (second, other) = fixture();
    let hung = first.path().canonicalize().unwrap();
    let locations = super::super::Locations::new(Arc::new(move |directory| {
        let mode = if directory == hung {
            "shutdown-hang"
        } else {
            "normal"
        };
        Pool::new(
            directory,
            vec![server()],
            launcher(mode, Arc::new(AtomicUsize::new(0))),
        )
    }));
    locations
        .warm(first.path(), file.clone(), "first".into())
        .unwrap();
    locations
        .warm(second.path(), other.clone(), "second".into())
        .unwrap();
    wait_for(|| first.path().join("document-events").exists()).await;
    wait_for(|| second.path().join("document-events").exists()).await;
    assert!(
        tokio::time::timeout(
            Duration::from_millis(20),
            locations.close_location(first.path())
        )
        .await
        .is_err()
    );
    assert!(
        locations
            .warm(first.path(), file.clone(), "fenced".into())
            .is_err()
    );
    assert_eq!(
        locations.status(second.path()).unwrap()[0].status,
        ServerState::Connected
    );
    locations
        .warm(second.path(), other, "sibling-changed".into())
        .unwrap();
    assert!(locations.close_location(first.path()).await.unwrap()[0].acknowledged);
    assert!(
        locations
            .warm(first.path(), file.clone(), "closed".into())
            .is_err()
    );
    locations.reload_location(first.path()).await.unwrap();
    assert!(locations.status(first.path()).unwrap().is_empty());
    locations
        .warm(first.path(), file, "fresh-generation".into())
        .unwrap();
    wait_for(|| {
        locations
            .status(first.path())
            .unwrap()
            .first()
            .is_some_and(|row| row.status == ServerState::Connected)
    })
    .await;
    assert!(
        locations
            .close()
            .await
            .unwrap()
            .iter()
            .all(|row| row.acknowledged)
    );
}

#[tokio::test]
async fn cancellation_settles_native_descendants_before_a_blocked_authority_review() {
    use std::sync::atomic::AtomicBool;
    let (root, file) = fixture();
    let marker = root.path().join("authority-review-escape");
    let block = Arc::new(AtomicBool::new(false));
    let entered = Arc::new(AtomicBool::new(false));
    let permit = Arc::new(tokio::sync::Semaphore::new(0));
    let admission: AdmissionFn = {
        let block = block.clone();
        let entered = entered.clone();
        let permit = permit.clone();
        Arc::new(move |_| {
            let block = block.clone();
            let entered = entered.clone();
            let permit = permit.clone();
            Box::pin(async move {
                if block.load(Ordering::Acquire) {
                    entered.store(true, Ordering::Release);
                    permit.acquire().await.unwrap().forget();
                }
                Ok(())
            })
        })
    };
    let pool = Pool::new(
        root.path(),
        vec![server()],
        launcher("normal", Arc::new(AtomicUsize::new(0))),
    )
    .unwrap()
    .with_admission(admission)
    .unwrap();
    let handle = pool.ensure("fixture", &file).unwrap();
    handle
        .request(
            "descendant",
            json!({"marker":marker}),
            Duration::from_secs(3),
        )
        .await
        .unwrap();
    block.store(true, Ordering::Release);
    let pending = tokio::spawn(async move {
        handle
            .request("fixture", json!({}), Duration::from_secs(3))
            .await
    });
    wait_for(|| entered.load(Ordering::Acquire)).await;
    assert!(
        tokio::time::timeout(Duration::from_millis(20), pool.close())
            .await
            .is_err()
    );
    tokio::time::sleep(Duration::from_millis(650)).await;
    assert!(!marker.exists());
    permit.add_permits(1);
    assert!(pool.close().await.unwrap()[0].acknowledged);
    assert!(pending.await.unwrap().is_err());
    assert!(pool.close().await.unwrap()[0].acknowledged);
}

#[tokio::test]
async fn cancellation_settles_native_descendants_before_a_blocked_document_claim() {
    blocked_document_claim(ClaimKind::Open).await;
}

#[tokio::test]
async fn cancellation_settles_native_descendants_before_a_blocked_diagnostic_claim() {
    blocked_document_claim(ClaimKind::Diagnostic).await;
}

#[tokio::test]
async fn cancellation_settles_native_descendants_before_a_blocked_removal_claim() {
    blocked_document_claim(ClaimKind::Remove).await;
}

#[derive(Clone, Copy)]
enum ClaimKind {
    Open,
    Diagnostic,
    Remove,
}

struct BlockedClaimResource {
    permit: Arc<tokio::sync::Semaphore>,
    entered: Arc<AtomicUsize>,
    released: Arc<AtomicUsize>,
}
impl ResourceLease for BlockedClaimResource {
    fn admit_document(
        &mut self,
        _path: PathBuf,
        _checkouts: Vec<cyber_core::worktrees::Managed>,
        _cancel: CancellationToken,
    ) -> BoxFuture<'_, Result<(), LspError>> {
        Box::pin(async move {
            self.entered.fetch_add(1, Ordering::SeqCst);
            self.permit.acquire().await.unwrap().forget();
            Ok(())
        })
    }
    fn admit_removed(
        &mut self,
        path: PathBuf,
        checkouts: Vec<cyber_core::worktrees::Managed>,
        cancel: CancellationToken,
    ) -> BoxFuture<'_, Result<(), LspError>> {
        self.admit_document(path, checkouts, cancel)
    }
    fn close(&mut self) -> BoxFuture<'_, bool> {
        self.released.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { true })
    }
}

async fn blocked_document_claim(kind: ClaimKind) {
    let (root, file) = fixture();
    let file = file.canonicalize().unwrap();
    let marker = root.path().join("document-claim-escape");
    let permit = Arc::new(tokio::sync::Semaphore::new(0));
    let entered = Arc::new(AtomicUsize::new(0));
    let released = Arc::new(AtomicUsize::new(0));
    let resource = Arc::new(Mutex::new(Some(BlockedClaimResource {
        permit: permit.clone(),
        entered: entered.clone(),
        released: released.clone(),
    })));
    let launch: LaunchFn = Arc::new(move |_, _| {
        let resource = resource.lock().unwrap().take().unwrap();
        Box::pin(async move {
            Ok(AuthorizedProcess {
                process: super::super::connection::tests::process("normal").await,
                keepalive: Box::new(resource),
            })
        })
    });
    let pool = Pool::new(root.path(), vec![server()], launch).unwrap();
    let handle = pool.ensure("fixture", &file).unwrap();
    handle
        .request(
            "descendant",
            json!({"marker":marker}),
            Duration::from_secs(3),
        )
        .await
        .unwrap();
    let pending = tokio::spawn(submit_blocked_claim(handle, file, kind));
    wait_for(|| entered.load(Ordering::SeqCst) == 1).await;
    assert!(
        tokio::time::timeout(Duration::from_millis(20), pool.close())
            .await
            .is_err()
    );
    tokio::time::sleep(Duration::from_millis(650)).await;
    assert!(!marker.exists());
    assert_eq!(released.load(Ordering::SeqCst), 0);
    finish_blocked_claim(&pool, &permit, &released, pending).await;
}

async fn finish_blocked_claim(
    pool: &Pool,
    permit: &tokio::sync::Semaphore,
    released: &AtomicUsize,
    pending: JoinHandle<Result<(), LspError>>,
) {
    permit.add_permits(1);
    assert!(pool.close().await.unwrap()[0].acknowledged);
    assert!(pending.await.unwrap().is_err());
    assert_eq!(released.load(Ordering::SeqCst), 1);
    assert!(pool.close().await.unwrap()[0].acknowledged);
    assert_eq!(released.load(Ordering::SeqCst), 1);
}

async fn submit_blocked_claim(
    handle: ServerHandle,
    file: PathBuf,
    kind: ClaimKind,
) -> Result<(), LspError> {
    if matches!(kind, ClaimKind::Remove) {
        std::fs::remove_file(&file).unwrap();
        return handle.remove_observed(file, vec![]).await;
    }
    if matches!(kind, ClaimKind::Diagnostic) {
        let uri = reqwest::Url::from_file_path(&file).unwrap().to_string();
        let params = json!({"uri":uri,"diagnostics":[]});
        handle
            .request("publish-diagnostics", params, Duration::from_secs(3))
            .await?;
        handle.diagnostics(&file).await.map(|_| ())
    } else {
        handle.open_document(&file, "pending".into()).await
    }
}

#[tokio::test]
async fn native_diagnostics_replace_clear_validate_versions_and_refuse_changed_content() {
    let (root, file) = fixture();
    let file = file.canonicalize().unwrap();
    let pool = Pool::new(
        root.path(),
        vec![server()],
        launcher("normal", Arc::new(AtomicUsize::new(0))),
    )
    .unwrap();
    let handle = pool.ensure("fixture", &file).unwrap();
    handle
        .open_document(&file, std::fs::read_to_string(&file).unwrap())
        .await
        .unwrap();
    let diagnostic = json!({"range":{"start":{"line":0,"character":2},"end":{"line":0,"character":4}},"severity":1,"message":"type error","relatedInformation":[{"location":{"uri":"file:///private"}}]});
    let params = json!({"uri":reqwest::Url::from_file_path(&file).unwrap().as_str(),"version":1,"diagnostics":[diagnostic]});
    handle
        .request(
            "publish-diagnostics",
            params.clone(),
            Duration::from_secs(3),
        )
        .await
        .unwrap();
    let first = handle.diagnostics(&file).await.unwrap().unwrap();
    assert_eq!(first.version, Some(1));
    assert_eq!(first.document_version, Some(1));
    assert_eq!(first.diagnostics.len(), 1);
    assert!(
        serde_json::to_value(&first).unwrap()["diagnostics"][0]
            .get("relatedInformation")
            .is_none()
    );
    let mut invalid = Vec::new();
    for version in [json!(0), json!(2), Value::Null] {
        let mut value = params.clone();
        value["version"] = version;
        invalid.push(value);
    }
    for severity in [json!(0), json!(5), json!("error"), Value::Null] {
        let mut value = params.clone();
        value["diagnostics"][0]["severity"] = severity;
        invalid.push(value);
    }
    for character in [json!(-1), json!(i64::from(i32::MAX) + 1), json!(8)] {
        let mut value = params.clone();
        value["diagnostics"][0]["range"]["start"]["character"] = character;
        invalid.push(value);
    }
    for uri in [
        "https://example.test/file",
        "file://remote/file",
        "file:///private",
        "file:///file?query",
        "file:///file#fragment",
    ] {
        let mut value = params.clone();
        value["uri"] = json!(uri);
        invalid.push(value);
    }
    for params in invalid {
        handle
            .request("publish-diagnostics", params, Duration::from_secs(3))
            .await
            .unwrap();
        assert_eq!(
            handle.diagnostics(&file).await.unwrap(),
            Some(first.clone())
        );
    }
    let mut clear = params.clone();
    clear["diagnostics"] = json!([]);
    handle
        .request("publish-diagnostics", clear, Duration::from_secs(3))
        .await
        .unwrap();
    let cleared = handle.diagnostics(&file).await.unwrap().unwrap();
    assert!(cleared.diagnostics.is_empty());
    assert!(cleared.sequence > first.sequence);
    let mut unversioned = params.clone();
    unversioned.as_object_mut().unwrap().remove("version");
    handle
        .request("publish-diagnostics", unversioned, Duration::from_secs(3))
        .await
        .unwrap();
    assert_eq!(
        handle.diagnostics(&file).await.unwrap().unwrap().version,
        None
    );
    let mut many = params.clone();
    many["diagnostics"] = json!(vec![params["diagnostics"][0].clone(); 300]);
    handle
        .request("publish-diagnostics", many, Duration::from_secs(3))
        .await
        .unwrap();
    let many = handle.diagnostics(&file).await.unwrap().unwrap();
    assert_eq!(many.diagnostics.len(), 300);
    let block = many
        .error_block(root.path().canonicalize().unwrap().as_path())
        .unwrap();
    assert_eq!(block.matches("ERROR [").count(), 20);
    assert!(block.contains("… and 280 more"));
    let mut long = params.clone();
    long["diagnostics"][0]["message"] = json!("x".repeat(8192));
    handle
        .request("publish-diagnostics", long, Duration::from_secs(3))
        .await
        .unwrap();
    assert_eq!(
        handle
            .diagnostics(&file)
            .await
            .unwrap()
            .unwrap()
            .diagnostics[0]
            .message
            .len(),
        8192
    );
    std::fs::write(&file, "changed text").unwrap();
    assert!(handle.diagnostics(&file).await.unwrap().is_none());
    // The server's open text must also match disk before diagnostics can be cached.
    handle
        .request(
            "publish-diagnostics",
            params.clone(),
            Duration::from_secs(3),
        )
        .await
        .unwrap();
    assert!(handle.diagnostics(&file).await.unwrap().is_none());
    handle
        .open_document(&file, "changed text".into())
        .await
        .unwrap();
    handle
        .request(
            "publish-diagnostics",
            params.clone(),
            Duration::from_secs(3),
        )
        .await
        .unwrap();
    assert!(handle.diagnostics(&file).await.unwrap().is_none());
    let mut changed = params;
    changed["version"] = json!(2);
    handle
        .request("publish-diagnostics", changed, Duration::from_secs(3))
        .await
        .unwrap();
    assert_eq!(
        handle.diagnostics(&file).await.unwrap().unwrap().version,
        Some(2)
    );
    let other = root.path().join("unopened.rs");
    std::fs::write(&other, "other text").unwrap();
    let other = other.canonicalize().unwrap();
    handle
        .request(
            "publish-diagnostics",
            json!({"uri":reqwest::Url::from_file_path(&other).unwrap().as_str(),"diagnostics":[]}),
            Duration::from_secs(3),
        )
        .await
        .unwrap();
    let snapshot = handle.diagnostics(&other).await.unwrap().unwrap();
    assert_eq!(snapshot.document_version, None);
    assert_eq!(snapshot.version, None);
    std::fs::remove_file(&other).unwrap();
    assert!(handle.diagnostics(&other).await.unwrap().is_none());
    assert!(!handle.take_notifications().await.unwrap().is_empty());
    assert!(handle.take_notifications().await.unwrap().is_empty());
    assert!(pool.close().await.unwrap()[0].acknowledged);
}

#[tokio::test]
async fn nearest_roots_get_independent_owned_processes() {
    let (root, file) = fixture();
    let nested = root.path().join("nested");
    std::fs::create_dir(&nested).unwrap();
    std::fs::write(nested.join(".root"), "").unwrap();
    let nested_file = nested.join("nested.rs");
    std::fs::write(&nested_file, "").unwrap();
    let launches = Arc::new(AtomicUsize::new(0));
    let pool = Pool::new(
        root.path(),
        vec![server()],
        launcher("normal", launches.clone()),
    )
    .unwrap();
    let first = pool.ensure("fixture", &file).unwrap();
    let second = pool.ensure("fixture", &nested_file).unwrap();
    first.connected().await.unwrap();
    second.connected().await.unwrap();
    assert_eq!(launches.load(Ordering::SeqCst), 2);
    assert_eq!(second.status().root, nested.canonicalize().unwrap());
    assert_eq!(
        pool.close()
            .await
            .unwrap()
            .iter()
            .filter(|s| s.acknowledged)
            .count(),
        2
    );
}

#[tokio::test]
async fn failed_roots_remain_broken_until_the_pool_is_recreated() {
    let (root, file) = fixture();
    let launches = Arc::new(AtomicUsize::new(0));
    let pool = Pool::new(
        root.path(),
        vec![server()],
        launcher("reject", launches.clone()),
    )
    .unwrap();
    assert!(
        pool.ensure("fixture", &file)
            .unwrap()
            .connected()
            .await
            .is_err()
    );
    assert!(
        pool.ensure("fixture", &file)
            .unwrap()
            .connected()
            .await
            .is_err()
    );
    assert_eq!(launches.load(Ordering::SeqCst), 1);
    assert_eq!(pool.status().unwrap()[0].status, ServerState::Broken);
    assert!(pool.close().await.unwrap()[0].acknowledged);
    let next = Pool::new(
        root.path(),
        vec![server()],
        launcher("normal", launches.clone()),
    )
    .unwrap();
    next.ensure("fixture", &file)
        .unwrap()
        .connected()
        .await
        .unwrap();
    assert_eq!(launches.load(Ordering::SeqCst), 2);
    assert!(next.close().await.unwrap()[0].acknowledged);
}

#[tokio::test]
async fn closing_cancels_a_stalled_initialize_and_acknowledges_the_real_process() {
    let (root, file) = fixture();
    let launches = Arc::new(AtomicUsize::new(0));
    let pool = Pool::new(
        root.path(),
        vec![server()],
        launcher("startup-hang", launches.clone()),
    )
    .unwrap();
    let handle = pool.ensure("fixture", &file).unwrap();
    wait_for(|| launches.load(Ordering::SeqCst) == 1).await;
    let stop = tokio::time::timeout(Duration::from_secs(1), pool.close())
        .await
        .unwrap()
        .unwrap();
    assert!(stop[0].acknowledged);
    assert!(handle.connected().await.is_err());
}

#[tokio::test]
async fn cancelled_close_retains_join_handles_and_keepalives_until_settlement() {
    struct Resource(Arc<AtomicUsize>);
    impl ResourceLease for Resource {
        fn close(&mut self) -> BoxFuture<'_, bool> {
            Box::pin(async { true })
        }
    }
    impl Drop for Resource {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
    let (root, file) = fixture();
    let released = Arc::new(AtomicUsize::new(0));
    let launch: LaunchFn = {
        let released = released.clone();
        Arc::new(move |_, _| {
            let released = released.clone();
            Box::pin(async move {
                Ok(AuthorizedProcess {
                    process: super::super::connection::tests::process("shutdown-hang").await,
                    keepalive: Box::new(Resource(released)),
                })
            })
        })
    };
    let pool = Pool::new(root.path(), vec![server()], launch).unwrap();
    pool.ensure("fixture", &file)
        .unwrap()
        .connected()
        .await
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(20), pool.close())
            .await
            .is_err()
    );
    assert_eq!(released.load(Ordering::SeqCst), 0);
    let stop = pool.close().await.unwrap();
    assert!(stop[0].acknowledged);
    assert_eq!(released.load(Ordering::SeqCst), 1);
    assert_eq!(pool.close().await.unwrap().len(), 1);
}

#[tokio::test]
async fn pool_settlement_waits_for_resources_after_normal_and_failed_startup() {
    struct Resource {
        permit: tokio::sync::oneshot::Receiver<()>,
        entered: Arc<AtomicUsize>,
        released: Arc<AtomicUsize>,
    }
    impl ResourceLease for Resource {
        fn close(&mut self) -> BoxFuture<'_, bool> {
            Box::pin(async move {
                self.entered.fetch_add(1, Ordering::SeqCst);
                (&mut self.permit).await.is_ok()
            })
        }
    }
    impl Drop for Resource {
        fn drop(&mut self) {
            self.released.fetch_add(1, Ordering::SeqCst);
        }
    }
    for mode in ["normal", "reject"] {
        let (root, file) = fixture();
        let (permit, receiver) = tokio::sync::oneshot::channel();
        let entered = Arc::new(AtomicUsize::new(0));
        let released = Arc::new(AtomicUsize::new(0));
        let resource = Arc::new(Mutex::new(Some(Resource {
            permit: receiver,
            entered: entered.clone(),
            released: released.clone(),
        })));
        let launch: LaunchFn = Arc::new(move |_, _| {
            let resource = resource.lock().unwrap().take().unwrap();
            Box::pin(async move {
                Ok(AuthorizedProcess {
                    process: super::super::connection::tests::process(mode).await,
                    keepalive: Box::new(resource),
                })
            })
        });
        let pool = Pool::new(root.path(), vec![server()], launch).unwrap();
        let connected = pool.ensure("fixture", &file).unwrap().connected().await;
        assert_eq!(connected.is_ok(), mode == "normal");
        assert!(
            tokio::time::timeout(Duration::from_millis(50), pool.close())
                .await
                .is_err()
        );
        wait_for(|| entered.load(Ordering::SeqCst) == 1).await;
        assert_eq!(released.load(Ordering::SeqCst), 0);
        permit.send(()).unwrap();
        assert!(pool.close().await.unwrap()[0].acknowledged);
        assert_eq!(released.load(Ordering::SeqCst), 1);
        assert!(pool.close().await.unwrap()[0].acknowledged);
        assert_eq!(entered.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn unexpected_idle_exit_marks_the_root_broken_without_respawning() {
    let (root, file) = fixture();
    let launches = Arc::new(AtomicUsize::new(0));
    let pool = Pool::new(
        root.path(),
        vec![server()],
        launcher("exit-on-initialized", launches.clone()),
    )
    .unwrap();
    let handle = pool.ensure("fixture", &file).unwrap();
    wait_for(|| handle.status().status == ServerState::Broken).await;
    assert!(
        pool.ensure("fixture", &file)
            .unwrap()
            .connected()
            .await
            .is_err()
    );
    assert_eq!(launches.load(Ordering::SeqCst), 1);
    assert!(pool.close().await.unwrap()[0].acknowledged);
}

#[tokio::test]
async fn dropping_last_pool_owner_closes_the_worker_even_with_an_outstanding_handle() {
    let (root, file) = fixture();
    let marker = root.path().join("pool-drop-escape");
    let pool = Pool::new(
        root.path(),
        vec![server()],
        launcher("normal", Arc::new(AtomicUsize::new(0))),
    )
    .unwrap();
    let handle = pool.ensure("fixture", &file).unwrap();
    handle
        .request(
            "descendant",
            json!({"marker":marker}),
            Duration::from_secs(3),
        )
        .await
        .unwrap();
    drop(pool);
    tokio::time::sleep(Duration::from_millis(650)).await;
    assert!(!marker.exists());
    assert_eq!(handle.status().status, ServerState::Broken);
}

#[tokio::test]
async fn cancelled_caller_cannot_dispose_or_replay_an_admitted_worker_request() {
    let (root, file) = fixture();
    let marker = root.path().join("request-admitted");
    let launches = Arc::new(AtomicUsize::new(0));
    let pool = Pool::new(
        root.path(),
        vec![server()],
        launcher("normal", launches.clone()),
    )
    .unwrap();
    let handle = pool.ensure("fixture", &file).unwrap();
    handle.connected().await.unwrap();
    let caller = {
        let handle = handle.clone();
        let marker = marker.clone();
        tokio::spawn(async move {
            handle
                .request("delay", json!({"marker":marker}), Duration::from_secs(3))
                .await
        })
    };
    wait_for(|| marker.exists()).await;
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    assert_eq!(
        handle
            .request("fixture", json!({}), Duration::from_secs(3))
            .await
            .unwrap()["text"],
        "λ🦀"
    );
    assert_eq!(std::fs::read_to_string(&marker).unwrap(), "started\n");
    assert_eq!(launches.load(Ordering::SeqCst), 1);
    assert_eq!(handle.status().status, ServerState::Connected);
    assert!(pool.close().await.unwrap()[0].acknowledged);
}

#[tokio::test]
async fn idle_diagnostics_and_server_requests_are_consumed_without_a_tool_rpc() {
    let (root, file) = fixture();
    let marker = root.path().join("idle-replied");
    let pool = Pool::new(
        root.path(),
        vec![server()],
        launcher("idle-messages", Arc::new(AtomicUsize::new(0))),
    )
    .unwrap();
    let handle = pool.ensure("fixture", &file).unwrap();
    handle.connected().await.unwrap();
    wait_for(|| marker.exists()).await;
    assert_eq!(std::fs::read_to_string(&marker).unwrap(), "refused");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "fn main() {}\n");
    let notifications = handle.take_notifications().await.unwrap();
    assert_eq!(notifications.len(), 1);
    assert_eq!(notifications[0]["params"]["version"], 7);
    assert_eq!(handle.status().status, ServerState::Connected);
    assert!(pool.close().await.unwrap()[0].acknowledged);
}

#[tokio::test]
async fn unsolicited_idle_response_breaks_the_root_and_settles_the_native_owner() {
    let (root, file) = fixture();
    let launches = Arc::new(AtomicUsize::new(0));
    let pool = Pool::new(
        root.path(),
        vec![server()],
        launcher("idle-foreign-response", launches.clone()),
    )
    .unwrap();
    let handle = pool.ensure("fixture", &file).unwrap();
    wait_for(|| handle.status().status == ServerState::Broken).await;
    assert!(
        pool.ensure("fixture", &file)
            .unwrap()
            .connected()
            .await
            .is_err()
    );
    assert_eq!(launches.load(Ordering::SeqCst), 1);
    assert!(pool.close().await.unwrap()[0].acknowledged);
}

#[tokio::test]
async fn foreground_command_can_win_during_an_actual_partial_stdout_frame() {
    for mode in ["partial-idle-header", "partial-idle-body"] {
        let (root, file) = fixture();
        let marker = root.path().join("partial-started");
        let pool = Pool::new(
            root.path(),
            vec![server()],
            launcher(mode, Arc::new(AtomicUsize::new(0))),
        )
        .unwrap();
        let handle = pool.ensure("fixture", &file).unwrap();
        handle.connected().await.unwrap();
        wait_for(|| marker.exists()).await;
        let result = handle
            .request("fixture", json!({}), Duration::from_secs(3))
            .await
            .unwrap();
        assert_eq!(result["text"], "λ🦀");
        let notifications = handle.take_notifications().await.unwrap();
        assert_eq!(notifications.len(), 1);
        assert_eq!(notifications[0]["params"]["version"], 8);
        assert_eq!(handle.status().status, ServerState::Connected);
        assert!(pool.close().await.unwrap()[0].acknowledged);
    }
}

#[tokio::test]
async fn queued_navigation_refuses_changed_text_before_document_delivery() {
    let (root, file) = fixture();
    let file = file.canonicalize().unwrap();
    let before = std::fs::read_to_string(&file).unwrap();
    let pool = Pool::new(
        root.path(),
        vec![server()],
        launcher("normal", Arc::new(AtomicUsize::new(0))),
    )
    .unwrap();
    let handle = pool.ensure("fixture", &file).unwrap();
    let delayed = handle.clone();
    let marker = root.path().join("navigation-delay");
    let signal = marker.clone();
    let blocker = tokio::spawn(async move {
        delayed
            .request("delay", json!({"marker":signal}), Duration::from_secs(3))
            .await
    });
    wait_for(|| marker.exists()).await;
    let queued = handle.clone();
    let path = file.clone();
    let navigation = tokio::spawn(async move {
        queued
            .navigate_observed(path, before, vec![], "fixture", json!({}))
            .await
    });
    wait_for(|| handle.entry.sender.capacity() < 32).await;
    std::fs::write(&file, "changed before queued navigation").unwrap();
    assert!(navigation.await.unwrap().is_err());
    assert!(blocker.await.unwrap().unwrap().as_bool().unwrap());
    assert!(!root.path().join("document-events").exists());
    assert_eq!(handle.status().status, ServerState::Connected);
    assert!(pool.close().await.unwrap()[0].acknowledged);
}

#[tokio::test]
async fn workspace_query_handles_preserve_all_existing_nested_roots() {
    let root = tempfile::tempdir().unwrap();
    let launches = Arc::new(AtomicUsize::new(0));
    let pool = Pool::new(
        root.path(),
        vec![server()],
        launcher("normal", launches.clone()),
    )
    .unwrap();
    for name in ["one", "two"] {
        let directory = root.path().join(name);
        std::fs::create_dir(&directory).unwrap();
        std::fs::write(directory.join(".root"), "").unwrap();
        let file = directory.join("file.rs");
        std::fs::write(&file, "nested").unwrap();
        pool.ensure("fixture", &file)
            .unwrap()
            .connected()
            .await
            .unwrap();
    }
    let handles = pool.query_handles(None);
    assert_eq!(handles.len(), 2);
    for handle in handles {
        assert_ne!(handle.status().root, root.path().canonicalize().unwrap());
    }
    assert_eq!(launches.load(Ordering::SeqCst), 2);
    assert!(pool.close().await.unwrap().iter().all(|s| s.acknowledged));
}

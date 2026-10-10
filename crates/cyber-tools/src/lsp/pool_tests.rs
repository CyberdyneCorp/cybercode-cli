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

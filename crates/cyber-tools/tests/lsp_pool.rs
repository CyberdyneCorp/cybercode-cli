use cyber_core::intelligence::{DetectedServer, InstallMethod, ServerDefinition};
use cyber_tools::lsp::{LaunchError, LaunchFn, LspError, Pool, ServerState};
use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

fn server() -> DetectedServer {
    DetectedServer {
        definition: ServerDefinition {
            id: "fixture".into(),
            extensions: vec![".rs".into()],
            root_markers: vec![],
            command: vec!["fixture".into()],
            env: BTreeMap::new(),
            initialization_options: None,
            install: InstallMethod::Custom,
        },
        enabled: true,
        installed: true,
        executable: Some("fixture".into()),
    }
}

fn refusing_launch(count: Arc<AtomicUsize>) -> LaunchFn {
    Arc::new(move |_, _| {
        count.fetch_add(1, Ordering::SeqCst);
        Box::pin(async {
            Err(LaunchError {
                error: LspError::Protocol("launch refused"),
                acknowledged: true,
            })
        })
    })
}

#[tokio::test]
async fn lazy_start_deduplicates_refusal_and_exposes_actual_broken_status() {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("file.rs");
    std::fs::write(&file, "").unwrap();
    let launches = Arc::new(AtomicUsize::new(0));
    let pool = Pool::new(
        root.path(),
        vec![server()],
        refusing_launch(launches.clone()),
    )
    .unwrap();
    assert!(pool.status().unwrap().is_empty());
    let first = pool.ensure("fixture", &file).unwrap();
    let second = pool.ensure("fixture", &file).unwrap();
    assert!(first.connected().await.is_err());
    assert!(second.connected().await.is_err());
    assert_eq!(launches.load(Ordering::SeqCst), 1);
    assert_eq!(pool.status().unwrap()[0].status, ServerState::Broken);
    assert!(pool.close().await.unwrap()[0].acknowledged);
    assert!(pool.ensure("fixture", &file).is_err());
}

#[tokio::test]
async fn disabled_missing_external_and_unmatched_files_never_reach_the_launcher() {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("file.rs");
    let other = root.path().join("file.txt");
    std::fs::write(&file, "").unwrap();
    std::fs::write(&other, "").unwrap();
    let external = tempfile::tempdir().unwrap();
    let outside = external.path().join("outside.rs");
    std::fs::write(&outside, "").unwrap();
    let launches = Arc::new(AtomicUsize::new(0));
    let mut disabled = server();
    disabled.enabled = false;
    let pool = Pool::new(
        root.path(),
        vec![disabled],
        refusing_launch(launches.clone()),
    )
    .unwrap();
    assert!(pool.ensure("fixture", &file).is_err());
    let pool = Pool::new(
        root.path(),
        vec![server()],
        refusing_launch(launches.clone()),
    )
    .unwrap();
    assert!(pool.ensure("fixture", &outside).is_err());
    assert!(pool.ensure("fixture", &other).is_err());
    assert!(
        pool.ensure("fixture", &root.path().join("absent.rs"))
            .is_err()
    );
    assert!(pool.ensure("unknown", &file).is_err());
    assert_eq!(launches.load(Ordering::SeqCst), 0);
    assert!(pool.close().await.unwrap().is_empty());
}

#[tokio::test]
async fn unavailable_executables_and_empty_commands_do_not_admit_workers() {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("file.rs");
    std::fs::write(&file, "").unwrap();
    let launches = Arc::new(AtomicUsize::new(0));
    let mut uninstalled = server();
    uninstalled.installed = false;
    let mut unresolved = server();
    unresolved.executable = None;
    let mut empty = server();
    empty.definition.command.clear();
    for definition in [uninstalled, unresolved, empty] {
        let pool = Pool::new(
            root.path(),
            vec![definition],
            refusing_launch(launches.clone()),
        )
        .unwrap();
        assert!(pool.ensure("fixture", &file).is_err());
        assert!(pool.status().unwrap().is_empty());
        assert!(pool.close().await.unwrap().is_empty());
    }
    assert_eq!(launches.load(Ordering::SeqCst), 0);
}

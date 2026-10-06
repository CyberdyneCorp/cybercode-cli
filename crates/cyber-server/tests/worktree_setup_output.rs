mod support;

use base64::Engine as _;
use cyber_core::worktrees::{Managed, SetupEvent, SetupSink, SetupStream};
use cyber_server::runtime::{LiveEvent, SetupChannel, SetupUpdate};
use support::{Harness, Setup};

fn owned(h: &Harness) -> Managed {
    Managed {
        id: "wt_output".into(),
        name: "output".into(),
        path: h.repo.canonicalize().unwrap(),
        branch: "cyber/output".into(),
        base: "base".into(),
        common_dir: h.repo.join(".git"),
        ready: true,
        included: vec![],
    }
}

#[tokio::test]
async fn setup_updates_identify_session_call_and_channel_without_exposing_commands() {
    let h = Harness::new(Setup::default());
    let id = h.session().await;
    let managed = owned(&h);
    let sink = h
        .runtime
        .worktree_setup_sink(&id, "call_setup", &managed)
        .await
        .unwrap();
    let mut events = h.runtime.subscribe();
    sink.emit(SetupEvent::Started {
        index: 0,
        command: "secret command value",
    })
    .unwrap();
    let attempted = events.recv().await.unwrap();
    assert!(
        !serde_json::to_string(&attempted)
            .unwrap()
            .contains("secret command value")
    );
    assert!(
        matches!(attempted, LiveEvent::WorktreeSetup { session_id, worktree_id, call_id, update: SetupUpdate::Attempted { index: 0 } } if session_id == id && worktree_id == managed.id && call_id == "call_setup")
    );
    let source = vec![0xff; 9000];
    sink.emit(SetupEvent::Output {
        stream: SetupStream::Stderr,
        bytes: &source,
    })
    .unwrap();
    let mut decoded = Vec::new();
    for _ in 0..2 {
        let LiveEvent::WorktreeSetup {
            update:
                SetupUpdate::Output {
                    index: 0,
                    stream: SetupChannel::Stderr,
                    base64,
                },
            ..
        } = events.recv().await.unwrap()
        else {
            panic!("expected output")
        };
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(base64)
            .unwrap();
        assert!(bytes.len() <= 8192);
        decoded.extend(bytes);
    }
    assert_eq!(decoded, source);
    sink.emit(SetupEvent::Finished {
        index: 0,
        code: Some(7),
    })
    .unwrap();
    assert!(matches!(
        events.recv().await.unwrap(),
        LiveEvent::WorktreeSetup {
            update: SetupUpdate::Finished {
                index: 0,
                code: Some(7)
            },
            ..
        }
    ));
}

#[tokio::test]
async fn setup_output_refuses_wrong_location_and_malformed_lifecycle() {
    let h = Harness::new(Setup::default());
    let id = h.session().await;
    let mut managed = owned(&h);
    managed.path = h.dir.path().canonicalize().unwrap();
    assert!(
        h.runtime
            .worktree_setup_sink(&id, "call_setup", &managed)
            .await
            .is_err()
    );
    let sink = h
        .runtime
        .worktree_setup_sink(&id, "call_setup", &owned(&h))
        .await
        .unwrap();
    assert!(
        sink.emit(SetupEvent::Output {
            stream: SetupStream::Stdout,
            bytes: b"before start"
        })
        .is_err()
    );
    sink.emit(SetupEvent::Started {
        index: 0,
        command: "first",
    })
    .unwrap();
    assert!(
        sink.emit(SetupEvent::Started {
            index: 1,
            command: "overlap"
        })
        .is_err()
    );
    assert!(
        sink.emit(SetupEvent::Finished {
            index: 1,
            code: Some(0)
        })
        .is_err()
    );
    sink.emit(SetupEvent::Finished {
        index: 0,
        code: Some(0),
    })
    .unwrap();
    assert!(
        sink.emit(SetupEvent::Output {
            stream: SetupStream::Stdout,
            bytes: b"after finish"
        })
        .is_err()
    );
    h.runtime.shutdown().await;
    assert!(sink.failed("closed").is_err());
}

#[tokio::test]
async fn shutdown_waits_for_setup_acknowledgement_or_owning_future_disposal() {
    use std::io;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    use tokio::sync::Notify;
    use tokio_util::sync::CancellationToken;
    struct Disposed(Arc<AtomicBool>);
    impl Drop for Disposed {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    for acknowledges in [true, false] {
        let h = Harness::new(Setup::default());
        let runtime = h.runtime.clone();
        let entered = Arc::new(Notify::new());
        let signal = Arc::clone(&entered);
        let disposed = Arc::new(AtomicBool::new(false));
        let observer = Arc::clone(&disposed);
        let acknowledged = Arc::new(AtomicBool::new(false));
        let settled = Arc::clone(&acknowledged);
        let token = CancellationToken::new();
        let cancel = token.clone();
        let task = tokio::spawn(async move {
            runtime
                .own_worktree_setup(cancel, async move {
                    let _owner = Disposed(observer);
                    signal.notify_one();
                    if acknowledges {
                        token.cancelled().await;
                        settled.store(true, Ordering::SeqCst);
                        Err::<(), _>(io::Error::new(
                            io::ErrorKind::Interrupted,
                            "acknowledged cancellation",
                        ))
                    } else {
                        std::future::pending::<io::Result<()>>().await
                    }
                })
                .await
        });
        entered.notified().await;
        tokio::time::timeout(std::time::Duration::from_secs(5), h.runtime.shutdown())
            .await
            .unwrap();
        assert!(
            disposed.load(Ordering::SeqCst),
            "shutdown returned before disposing setup ownership"
        );
        assert_eq!(acknowledged.load(Ordering::SeqCst), acknowledges);
        assert_eq!(
            task.await.unwrap().unwrap_err().kind(),
            io::ErrorKind::Interrupted
        );
    }
}

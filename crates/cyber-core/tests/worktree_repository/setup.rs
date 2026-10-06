use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use cyber_core::worktrees::{
    Name, RepositoryLock, Settings, SetupEvent, SetupExecution, SetupFuture, SetupOutcome,
    SetupSink, SetupStream,
};
use futures::{executor::block_on, task::noop_waker};

use super::Fixture;

#[derive(Default)]
struct Sink(Mutex<Vec<String>>);

impl SetupSink for Sink {
    fn emit(&self, event: SetupEvent<'_>) -> io::Result<()> {
        self.0.lock().unwrap().push(format!("{event:?}"));
        Ok(())
    }
}

struct Execution {
    path: PathBuf,
    common: PathBuf,
    code: Option<i32>,
    suspend: bool,
}

impl SetupExecution for Execution {
    fn run<'a>(
        &'a self,
        directory: &'a Path,
        command: &'a str,
        sink: &'a dyn SetupSink,
    ) -> SetupFuture<'a> {
        Box::pin(async move {
            assert_eq!(directory, self.path);
            assert!(RepositoryLock::try_acquire(&self.common)?.is_none());
            std::fs::write(directory.join("setup-result"), command)?;
            sink.emit(SetupEvent::Output {
                stream: SetupStream::Stdout,
                bytes: b"live output\xff",
            })?;
            sink.emit(SetupEvent::Output {
                stream: SetupStream::Stderr,
                bytes: b"diagnostic",
            })?;
            if self.suspend {
                futures::future::pending::<()>().await;
            }
            Ok(self.code)
        })
    }
}

#[test]
fn setup_streams_in_order_and_stops_after_failure_without_deleting_worktree() {
    let fixture = Fixture::new();
    let repository = fixture.repository();
    let managed = fixture
        .create(
            &repository,
            &Name::parse("setup").unwrap(),
            &Settings::default(),
        )
        .unwrap();
    let settings = Settings {
        setup: vec!["first".into(), "second".into()],
        ..Settings::default()
    };
    let mut execution = Execution {
        path: managed.path.clone(),
        common: repository.common_dir.clone(),
        code: Some(0),
        suspend: false,
    };
    let sink = Sink::default();
    assert_eq!(
        block_on(repository.setup(&fixture.execution, &execution, &managed, &settings, &sink))
            .unwrap(),
        SetupOutcome::Completed
    );
    let events = sink.0.lock().unwrap();
    assert_eq!(events.len(), 8);
    assert!(events[0].contains("first"));
    assert!(events[1].contains("Stdout"));
    assert!(events[2].contains("Stderr"));
    assert!(events[3].contains("Finished"));
    assert!(events[4].contains("second"));
    drop(events);
    for code in [Some(7), None] {
        execution.code = code;
        assert_eq!(
            block_on(repository.setup(
                &fixture.execution,
                &execution,
                &managed,
                &settings,
                &Sink::default()
            ))
            .unwrap(),
            SetupOutcome::Failed { index: 0, code }
        );
        assert_eq!(
            std::fs::read_to_string(managed.path.join("setup-result")).unwrap(),
            "first"
        );
        assert!(managed.path.join("tracked.txt").is_file());
        assert!(
            RepositoryLock::try_acquire(&repository.common_dir)
                .unwrap()
                .is_some()
        );
    }
}

#[test]
fn interrupted_setup_preserves_output_files_and_releases_repository_lock() {
    let fixture = Fixture::new();
    let repository = fixture.repository();
    let managed = fixture
        .create(
            &repository,
            &Name::parse("cancel-setup").unwrap(),
            &Settings::default(),
        )
        .unwrap();
    let execution = Execution {
        path: managed.path.clone(),
        common: repository.common_dir.clone(),
        code: Some(0),
        suspend: true,
    };
    let sink = Sink::default();
    let settings = Settings {
        setup: vec!["partial".into()],
        ..Settings::default()
    };
    let mut future =
        Box::pin(repository.setup(&fixture.execution, &execution, &managed, &settings, &sink));
    let waker = noop_waker();
    assert!(
        std::future::Future::poll(future.as_mut(), &mut std::task::Context::from_waker(&waker))
            .is_pending()
    );
    assert_eq!(sink.0.lock().unwrap().len(), 3);
    drop(future);
    assert!(
        RepositoryLock::try_acquire(&repository.common_dir)
            .unwrap()
            .is_some()
    );
    assert_eq!(
        std::fs::read_to_string(managed.path.join("setup-result")).unwrap(),
        "partial"
    );
}

#[test]
fn setup_refuses_changed_ownership_before_dispatch() {
    let fixture = Fixture::new();
    let repository = fixture.repository();
    let mut managed = fixture
        .create(
            &repository,
            &Name::parse("changed-setup").unwrap(),
            &Settings::default(),
        )
        .unwrap();
    let execution = Execution {
        path: managed.path.clone(),
        common: repository.common_dir.clone(),
        code: Some(0),
        suspend: false,
    };
    let settings = Settings {
        setup: vec!["must not run".into()],
        ..Settings::default()
    };
    managed.base = "changed".into();
    let sink = Sink::default();
    assert!(
        block_on(repository.setup(&fixture.execution, &execution, &managed, &settings, &sink))
            .is_err()
    );
    assert!(sink.0.lock().unwrap().is_empty());
    assert!(!managed.path.join("setup-result").exists());
}

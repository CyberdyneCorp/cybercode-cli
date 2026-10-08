//! Native command-loop adaptation; complete tool policy selection remains a separate gate.
#![cfg(windows)]
#![allow(unsafe_code)]

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::os::windows::io::{AsHandle, AsRawHandle};
use std::time::Duration;

use cyber_sandbox::windows_container::{Access, AclGrant, Profile};
use cyber_sandbox::windows_streams::{PipedChild, spawn_piped};
use cyber_tools::AppContainerProcess;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};

struct Fixture {
    child: Option<PipedChild>,
    grants: Vec<AclGrant>,
    profile: Profile,
    _directory: tempfile::TempDir,
}

impl Fixture {
    async fn new(role: &str) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let profile = Profile::new().unwrap();
        let program = directory.path().join("command worker with spaces.exe");
        std::fs::copy(std::env::current_exe().unwrap(), &program).unwrap();
        let grants = vec![
            profile.grant(directory.path(), Access::Read).unwrap(),
            profile.grant(&program, Access::Read).unwrap(),
        ];
        let env = BTreeMap::from([
            ("SystemRoot".into(), std::env::var("SystemRoot").unwrap()),
            (
                "LOCALAPPDATA".into(),
                std::env::var("LOCALAPPDATA").unwrap(),
            ),
            ("CYBER_PROCESS_WORKER".into(), role.into()),
        ]);
        let args: Vec<OsString> = ["--exact", "container_process_worker", "--nocapture"]
            .into_iter()
            .map(Into::into)
            .collect();
        let child = spawn_piped(&profile, &program, &args, &env, directory.path())
            .await
            .unwrap();
        Self {
            child: Some(child),
            grants,
            profile,
            _directory: directory,
        }
    }

    fn close(mut self) {
        drop(self.child.take());
        for grant in self.grants {
            grant.close().unwrap();
        }
        self.profile.close().unwrap();
    }
}

#[tokio::test]
async fn container_adapter_preserves_large_streams_stdin_eof_and_exit_code() {
    let mut fixture = Fixture::new("output").await;
    let child = fixture.child.take().unwrap();
    let scratch = child.temporary_directory().to_owned();
    let mut process = AppContainerProcess::from_container(child);
    let mut stdout = process.stdout().unwrap();
    let mut stderr = process.stderr().unwrap();
    assert!(process.stdout().is_none());
    assert!(process.stderr().is_none());
    let mut out = Vec::new();
    let mut err = Vec::new();
    let (_, _, status) = tokio::time::timeout(Duration::from_secs(10), async {
        tokio::try_join!(
            stdout.read_to_end(&mut out),
            stderr.read_to_end(&mut err),
            process.wait_tree()
        )
    })
    .await
    .unwrap()
    .unwrap();
    assert_eq!(status.code(), Some(73));
    assert!(out.ends_with(&vec![b'O'; 256 * 1024]));
    assert_eq!(err, vec![b'E'; 256 * 1024]);
    drop(process);
    assert!(!scratch.exists());
    fixture.close();
}

#[tokio::test]
async fn borrowed_wait_disposal_retains_native_ownership_until_explicit_termination() {
    use windows_sys::Win32::Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT};
    use windows_sys::Win32::System::Threading::WaitForSingleObject;
    let mut fixture = Fixture::new("wait").await;
    let child = fixture.child.take().unwrap();
    let retained = child.as_handle().try_clone_to_owned().unwrap();
    let scratch = child.temporary_directory().to_owned();
    let mut process = AppContainerProcess::from_container(child);
    let mut stdout = BufReader::new(process.stdout().unwrap());
    tokio::time::timeout(Duration::from_secs(10), async {
        let mut line = String::new();
        loop {
            assert_ne!(stdout.read_line(&mut line).await.unwrap(), 0);
            if line.contains("ready") {
                break;
            }
            line.clear();
        }
    })
    .await
    .unwrap();
    drop_borrowed_wait(&mut process).await;
    assert_eq!(
        unsafe { WaitForSingleObject(retained.as_raw_handle(), 0) },
        WAIT_TIMEOUT
    );
    process.terminate();
    tokio::time::timeout(Duration::from_secs(5), process.wait())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        unsafe { WaitForSingleObject(retained.as_raw_handle(), 0) },
        WAIT_OBJECT_0
    );
    let mut remainder = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), stdout.read_to_end(&mut remainder))
        .await
        .unwrap()
        .unwrap();
    drop(process);
    assert!(!scratch.exists());
    fixture.close();
}

async fn drop_borrowed_wait(process: &mut AppContainerProcess) {
    let waiting = process.wait();
    tokio::pin!(waiting);
    assert!(futures::poll!(waiting.as_mut()).is_pending());
}

#[test]
fn container_process_worker() {
    use std::io::{Read, Write};
    let Ok(role) = std::env::var("CYBER_PROCESS_WORKER") else {
        return;
    };
    if role == "hook" {
        std::io::stdout()
            .write_all(&vec![b'O'; 256 * 1024])
            .unwrap();
        std::io::stdout().flush().unwrap();
        std::io::stderr()
            .write_all(&vec![b'E'; 256 * 1024])
            .unwrap();
        std::io::stderr().flush().unwrap();
        let mut input = Vec::new();
        std::io::stdin().read_to_end(&mut input).unwrap();
        std::io::stdout().write_all(&input).unwrap();
        std::io::stdout().flush().unwrap();
        std::process::exit(2);
    }
    let mut input = Vec::new();
    std::io::stdin().read_to_end(&mut input).unwrap();
    if !input.is_empty() {
        std::process::exit(74);
    }
    if role == "wait" {
        std::io::stdout().write_all(b"ready\n").unwrap();
        std::io::stdout().flush().unwrap();
        std::thread::sleep(Duration::from_secs(30));
    } else {
        std::io::stdout()
            .write_all(&vec![b'O'; 256 * 1024])
            .unwrap();
        std::io::stdout().flush().unwrap();
        std::io::stderr()
            .write_all(&vec![b'E'; 256 * 1024])
            .unwrap();
        std::io::stderr().flush().unwrap();
    }
    std::process::exit(73);
}

#[tokio::test]
async fn container_hook_transport_preserves_event_input_streams_and_exit() {
    use cyber_core::hooks::{HookEvent, HookIdentity, HookLocation};
    use cyber_tools::hook_commands::{HookCommandEnd, capture_hook_command};
    use tokio_util::sync::CancellationToken;
    let mut fixture = Fixture::new("hook").await;
    let child = fixture.child.take().unwrap();
    let scratch = child.temporary_directory().to_owned();
    let fields = serde_json::json!({"text":"Δ".repeat(128*1024)})
        .as_object()
        .unwrap()
        .clone();
    let event = HookEvent::new(
        "UserPromptSubmit",
        HookIdentity {
            session_id: "ses_hook".into(),
            location: HookLocation {
                directory: fixture._directory.path().into(),
                workspace: None,
            },
            project_id: "global".into(),
            agent: "coder".into(),
            mode: "default".into(),
        },
        1,
        fields,
    )
    .unwrap();
    let captured = capture_hook_command(
        AppContainerProcess::from_container_with_stdin(child),
        &event,
        Duration::from_secs(10),
        CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(captured.end, HookCommandEnd::Exited(Some(2)));
    let mut input = serde_json::to_vec(&event).unwrap();
    input.push(b'\n');
    assert!(captured.stdout.bytes.ends_with(&input));
    assert_eq!(captured.stderr.bytes, vec![b'E'; 256 * 1024]);
    assert!(!captured.stdout.truncated && !captured.stderr.truncated);
    assert!(!scratch.exists());
    fixture.close();
}

#[tokio::test]
async fn container_hook_transport_acknowledges_pre_cancelled_owned_process() {
    use cyber_core::hooks::{HookEvent, HookIdentity, HookLocation};
    use cyber_tools::hook_commands::{HookCommandEnd, capture_hook_command};
    use tokio_util::sync::CancellationToken;
    let mut fixture = Fixture::new("wait").await;
    let child = fixture.child.take().unwrap();
    let scratch = child.temporary_directory().to_owned();
    let event = HookEvent::new(
        "Stop",
        HookIdentity {
            session_id: "ses_hook".into(),
            location: HookLocation {
                directory: fixture._directory.path().into(),
                workspace: None,
            },
            project_id: "global".into(),
            agent: "coder".into(),
            mode: "default".into(),
        },
        1,
        serde_json::Map::new(),
    )
    .unwrap();
    let cancel = CancellationToken::new();
    cancel.cancel();
    let captured = capture_hook_command(
        AppContainerProcess::from_container_with_stdin(child),
        &event,
        Duration::from_secs(10),
        cancel,
    )
    .await
    .unwrap();
    assert_eq!(
        captured.end,
        HookCommandEnd::Cancelled { acknowledged: true }
    );
    assert!(!scratch.exists());
    fixture.close();
}

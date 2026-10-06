//! Native host-level shell ownership; uses an explicitly selected installed Git Bash.
#![cfg(windows)]
#![allow(unsafe_code)]

mod support;

use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use cyber_server::runtime::{ToolHost, ToolOutcome};
use serde_json::json;
use tokio_util::sync::CancellationToken;
use windows_sys::Win32::Foundation::WAIT_OBJECT_0;
use windows_sys::Win32::System::Threading::{
    OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject,
};

use support::{Fixture, failed, ok};

const COMMAND: &str = "./'native child.exe' --exact windows_shell_worker --nocapture";

fn bash() -> String {
    let path = PathBuf::from(std::env::var_os("ProgramFiles").unwrap()).join("Git/bin/bash.exe");
    assert!(
        path.is_file(),
        "native tests require installed Git Bash: {}",
        path.display()
    );
    path.display().to_string()
}

fn fixture(helper: Option<PathBuf>) -> Fixture {
    let f = Fixture::with_shell(&bash(), helper);
    f.set_config(json!({"sandbox": {"policy": "full-access"}, "permissions": {"bash": "allow"}}));
    f
}

fn workers(f: &Fixture) {
    for name in ["native child.exe", "native grandchild.exe"] {
        std::fs::copy(std::env::current_exe().unwrap(), f.repo.join(name)).unwrap();
    }
}

async fn live(root: &Path, name: &str, call: &mut Call) -> OwnedHandle {
    let deadline = Instant::now() + Duration::from_secs(10);
    let marker = root.join(name);
    while !marker.is_file() {
        if call.0.is_finished() {
            panic!("native call ended before {name}: {:?}", (&mut call.0).await);
        }
        assert!(
            Instant::now() < deadline,
            "native tool worker did not publish {name}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let pid = std::fs::read_to_string(marker).unwrap().parse().unwrap();
    // Retain the identity of a worker we just launched, rather than polling a reusable PID.
    let raw = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
    assert!(!raw.is_null(), "{}", std::io::Error::last_os_error());
    let handle = unsafe { OwnedHandle::from_raw_handle(raw) };
    assert_ne!(
        unsafe { WaitForSingleObject(handle.as_raw_handle(), 0) },
        WAIT_OBJECT_0
    );
    handle
}

fn terminated(handle: &OwnedHandle) {
    assert_eq!(
        unsafe { WaitForSingleObject(handle.as_raw_handle(), 5000) },
        WAIT_OBJECT_0
    );
}

struct Call(tokio::task::JoinHandle<ToolOutcome>);

impl Drop for Call {
    fn drop(&mut self) {
        self.0.abort();
    }
}

fn start(f: &Fixture, timeout_ms: u64, cancel: CancellationToken) -> Call {
    let host = Arc::clone(&f.host);
    let invocation = f.invocation(
        "default",
        "bash",
        json!({"command": COMMAND, "timeout_ms": timeout_ms}),
    );
    Call(tokio::spawn(async move {
        host.execute(invocation, cancel).await
    }))
}

#[tokio::test]
async fn native_bash_timeout_and_cancellation_terminate_live_descendants() {
    for cancelled in [false, true] {
        let f = fixture(cyber_sandbox::find_helper());
        workers(&f);
        let cancel = CancellationToken::new();
        let mut call = start(&f, 15000, cancel.clone());
        let child = live(&f.repo, "child.pid", &mut call).await;
        let grandchild = live(&f.repo, "grandchild.pid", &mut call).await;
        if cancelled {
            cancel.cancel();
        }
        let outcome = tokio::time::timeout(Duration::from_secs(20), &mut call.0)
            .await
            .unwrap()
            .unwrap();
        if cancelled {
            assert_eq!(outcome, ToolOutcome::Aborted);
        } else {
            assert!(ok(outcome).contains("Command timed out after 15000 ms"));
        }
        terminated(&child);
        terminated(&grandchild);
    }
}

#[tokio::test]
async fn native_bash_normal_exit_cleans_up_descendants() {
    let f = fixture(cyber_sandbox::find_helper());
    workers(&f);
    let mut call = start(&f, 15000, CancellationToken::new());
    let child = live(&f.repo, "child.pid", &mut call).await;
    let grandchild = live(&f.repo, "grandchild.pid", &mut call).await;
    std::fs::write(f.repo.join("release"), "exit").unwrap();
    let outcome = tokio::time::timeout(Duration::from_secs(5), &mut call.0)
        .await
        .unwrap()
        .unwrap();
    assert!(ok(outcome).contains("Exit code: 23"));
    terminated(&child);
    terminated(&grandchild);
}

#[tokio::test]
async fn native_bash_preserves_output_environment_and_closed_stdin() {
    let f = fixture(cyber_sandbox::find_helper());
    let outcome = f.call("default", "bash", json!({"command":
        "printf 'native-output:%s:%s\\n' \"$CYBER\" \"$CYBER_SESSION_ID\"; if read -r line; then exit 99; fi; exit 23"
    })).await;
    let output = ok(outcome);
    assert!(output.contains("native-output:1:ses_test"), "{output}");
    assert!(output.contains("Exit code: 23"), "{output}");
}

#[tokio::test]
async fn missing_helper_and_unavailable_confinement_refuse_execution() {
    let f = fixture(None);
    let outcome = f
        .call(
            "default",
            "bash",
            json!({"command": "touch unexpected-launch"}),
        )
        .await;
    assert!(failed(outcome).contains("required for Windows process ownership"));
    assert!(!f.repo.join("unexpected-launch").exists());
    let f = fixture(cyber_sandbox::find_helper());
    f.set_config(json!({"permissions": {"bash": "allow"}}));
    let outcome = f
        .call(
            "default",
            "bash",
            json!({"command": "touch unexpected-launch"}),
        )
        .await;
    assert!(failed(outcome).contains("SandboxUnavailableError"));
    assert!(!f.repo.join("unexpected-launch").exists());
}

fn publish(root: &Path, name: &str) {
    let temporary = root.join(format!("{name}.tmp"));
    std::fs::write(&temporary, std::process::id().to_string()).unwrap();
    std::fs::rename(temporary, root.join(name)).unwrap();
}

#[test]
#[allow(clippy::zombie_processes)] // Deliberately leave the descendant for the host-owned job.
fn windows_shell_worker() {
    let executable = std::env::current_exe().unwrap();
    let name = executable.file_name().unwrap().to_string_lossy();
    let root = std::env::current_dir().unwrap();
    if name == "native grandchild.exe" {
        publish(&root, "grandchild.pid");
        std::thread::sleep(Duration::from_secs(30));
    } else if name == "native child.exe" {
        publish(&root, "child.pid");
        let _child = Command::new(root.join("native grandchild.exe"))
            .args(["--exact", "windows_shell_worker", "--nocapture"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        while !root.join("release").is_file() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        std::process::exit(23);
    }
}

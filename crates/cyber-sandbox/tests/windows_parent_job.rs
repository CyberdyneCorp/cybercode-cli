//! Native proof that the caller, rather than only the helper, owns the tree.
#![cfg(windows)]
#![allow(unsafe_code)]

use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use cyber_sandbox::windows_process::OwnedCommand;
use windows_sys::Win32::Foundation::WAIT_OBJECT_0;
use windows_sys::Win32::System::Threading::{
    OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject,
};

const HELPER: &str = env!("CARGO_BIN_EXE_cyber-sandbox-exec");

fn wait_until(mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !ready() {
        assert!(Instant::now() < deadline, "Windows parent worker stalled");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn publish(root: &Path, name: &str, pid: u32) {
    let temporary = root.join(format!("{name}.tmp"));
    std::fs::write(&temporary, pid.to_string()).unwrap();
    std::fs::rename(temporary, root.join(name)).unwrap();
}

struct Worker(Child);

impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn launch(root: &Path, role: &str) -> Worker {
    let executable = root.join("parent worker with spaces.exe");
    std::fs::copy(std::env::current_exe().unwrap(), &executable).unwrap();
    Worker(
        Command::new(executable)
            .args(["--exact", "windows_parent_worker", "--nocapture"])
            .env("CYBER_PARENT_TEST_ROLE", role)
            .env("CYBER_PARENT_TEST_ROOT", root)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    )
}

fn live_process(root: &Path, name: &str, worker: &mut Child) -> OwnedHandle {
    wait_until(|| {
        assert!(worker.try_wait().unwrap().is_none(), "parent exited early");
        root.join(name).is_file()
    });
    let pid = std::fs::read_to_string(root.join(name))
        .unwrap()
        .parse()
        .unwrap();
    // The live test worker published this PID; retain the handle to prevent reuse ambiguity.
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

#[test]
fn killing_the_parent_terminates_helper_and_grandchild() {
    let root = tempfile::tempdir().unwrap();
    let mut parent = launch(root.path(), "owner");
    let helper = live_process(root.path(), "helper.pid", &mut parent.0);
    let grandchild = live_process(root.path(), "grandchild.pid", &mut parent.0);
    parent.0.kill().unwrap();
    parent.0.wait().unwrap();
    terminated(&helper);
    terminated(&grandchild);
}

#[test]
fn parent_death_before_assignment_refuses_user_code() {
    let root = tempfile::tempdir().unwrap();
    let mut parent = launch(root.path(), "before-assignment");
    let helper = live_process(root.path(), "helper.pid", &mut parent.0);
    parent.0.kill().unwrap();
    parent.0.wait().unwrap();
    terminated(&helper);
    assert!(!root.path().join("command.pid").exists());
    assert!(!root.path().join("grandchild.pid").exists());
}

#[test]
fn dropping_owner_and_normal_exit_clean_up_descendants() {
    for role in ["drop-owner", "terminate-owner", "abort-owner", "owner"] {
        let root = tempfile::tempdir().unwrap();
        let mut parent = launch(root.path(), role);
        let helper = live_process(root.path(), "helper.pid", &mut parent.0);
        let grandchild = live_process(root.path(), "grandchild.pid", &mut parent.0);
        let release = if role == "owner" {
            "command.release"
        } else {
            "owner.release"
        };
        std::fs::write(root.path().join(release), "exit").unwrap();
        let mut status = None;
        wait_until(|| {
            status = parent.0.try_wait().unwrap();
            status.is_some()
        });
        let status = status.unwrap();
        if !status.success() {
            let mut diagnostic = String::new();
            std::io::Read::read_to_string(&mut parent.0.stderr.take().unwrap(), &mut diagnostic)
                .unwrap();
            panic!("owner role {role} failed with {status}: {diagnostic}");
        }
        terminated(&helper);
        terminated(&grandchild);
    }
}

#[tokio::test]
async fn owned_launch_preserves_output_and_exit_status() {
    use tokio::io::AsyncReadExt;

    let mut command = OwnedCommand::new(
        Path::new(HELPER),
        "cmd.exe",
        ["/D", "/C", "echo parent-owned-output&exit /B 23"],
    );
    command
        .command_mut()
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().await.unwrap();
    let mut stdout = child.child_mut().stdout.take().unwrap();
    let status = tokio::time::timeout(Duration::from_secs(10), child.wait())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(status.code(), Some(23));
    let mut output = String::new();
    stdout.read_to_string(&mut output).await.unwrap();
    assert!(output.contains("parent-owned-output"));
}

#[test]
fn absent_or_invalid_permit_refuses_execution() {
    use std::io::Write;

    for permit in [b"".as_slice(), b"not-a-job-start\n".as_slice()] {
        let mut child = Command::new(HELPER)
            .args([
                "--parent-job",
                "--",
                "cmd.exe",
                "/D",
                "/C",
                "echo unexpected-child",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(permit).unwrap();
        let output = child.wait_with_output().unwrap();
        assert_eq!(output.status.code(), Some(69));
        assert!(output.stdout.is_empty());
        assert!(String::from_utf8_lossy(&output.stderr).contains("assignment was not confirmed"));
    }
}

#[test]
#[allow(clippy::zombie_processes)] // The command intentionally leaves a descendant for job cleanup.
fn windows_parent_worker() {
    let Ok(role) = std::env::var("CYBER_PARENT_TEST_ROLE") else {
        return;
    };
    let root = PathBuf::from(std::env::var_os("CYBER_PARENT_TEST_ROOT").unwrap());
    if role == "grandchild" {
        publish(&root, "grandchild.pid", std::process::id());
        std::thread::sleep(Duration::from_secs(30));
        return;
    }
    if role == "command" {
        publish(&root, "command.pid", std::process::id());
        let _child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "windows_parent_worker", "--nocapture"])
            .env("CYBER_PARENT_TEST_ROLE", "grandchild")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        wait_until(|| root.join("command.release").is_file());
        return;
    }
    if role == "before-assignment" {
        let mut child = Command::new(HELPER)
            .args(["--parent-job", "--"])
            .arg(std::env::current_exe().unwrap())
            .args(["--exact", "windows_parent_worker", "--nocapture"])
            .env("CYBER_PARENT_TEST_ROLE", "command")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        publish(&root, "helper.pid", child.id());
        // Keep the private pipe open without ever assigning or permitting the command.
        wait_until(|| root.join("release").is_file());
        child.kill().unwrap();
        child.wait().unwrap();
        return;
    }
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let mut command = OwnedCommand::new(
                Path::new(HELPER),
                std::env::current_exe().unwrap(),
                ["--exact", "windows_parent_worker", "--nocapture"],
            );
            command
                .command_mut()
                .env("CYBER_PARENT_TEST_ROLE", "command")
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            let mut child = command.spawn().await.unwrap();
            publish(&root, "helper.pid", child.child_mut().id().unwrap());
            if role != "owner" {
                while !root.join("owner.release").is_file() {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                match role.as_str() {
                    "terminate-owner" => {
                        child.terminate();
                        child.wait().await.unwrap();
                    }
                    "abort-owner" => {
                        let task = tokio::spawn(async move { child.wait().await });
                        tokio::task::yield_now().await;
                        task.abort();
                        assert!(task.await.unwrap_err().is_cancelled());
                    }
                    _ => drop(child),
                }
            } else {
                assert!(child.wait().await.unwrap().success());
            }
        });
}

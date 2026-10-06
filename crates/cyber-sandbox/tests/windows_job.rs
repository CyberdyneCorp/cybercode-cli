//! Native Windows proof for the isolated process-tree owner, not sandbox confinement.
#![cfg(windows)]
#![allow(unsafe_code)]

use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::WAIT_OBJECT_0;
use windows_sys::Win32::System::Threading::{
    OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject,
};

fn wait_until(mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !ready() {
        assert!(
            Instant::now() < deadline,
            "native worker did not become ready"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

struct Worker(Child);

impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.0.kill();
    }
}

fn launch_worker(root: &Path) -> Worker {
    let executable = root.join("worker with spaces.exe");
    std::fs::copy(std::env::current_exe().unwrap(), &executable).unwrap();
    Worker(
        Command::new(env!("CARGO_BIN_EXE_cyber-sandbox-exec"))
            .arg("--job")
            .arg("--")
            .arg(executable)
            .args(["--exact", "windows_job_worker", "--nocapture"])
            .env("CYBER_JOB_TEST_ROLE", "parent")
            .env("CYBER_JOB_TEST_ROOT", root)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    )
}

fn live_grandchild(root: &Path, helper: &mut Child) -> OwnedHandle {
    let marker = root.join("grandchild.pid");
    wait_until(|| {
        assert!(
            helper.try_wait().unwrap().is_none(),
            "helper exited before readiness"
        );
        marker.is_file()
    });
    let pid = std::fs::read_to_string(marker).unwrap().parse().unwrap();
    // Open only the worker PID just published by our live child, retaining identity.
    let raw = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
    assert!(!raw.is_null(), "{}", std::io::Error::last_os_error());
    let handle = unsafe { OwnedHandle::from_raw_handle(raw) };
    assert_ne!(
        unsafe { WaitForSingleObject(handle.as_raw_handle(), 0) },
        WAIT_OBJECT_0
    );
    handle
}

fn assert_terminated(handle: &OwnedHandle) {
    // This retained process handle cannot refer to a reused PID.
    assert_eq!(
        unsafe { WaitForSingleObject(handle.as_raw_handle(), 5000) },
        WAIT_OBJECT_0
    );
}

#[test]
fn killing_the_helper_terminates_a_live_grandchild() {
    let root = tempfile::tempdir().unwrap();
    let mut helper = launch_worker(root.path());
    let grandchild = live_grandchild(root.path(), &mut helper.0);
    helper.0.kill().unwrap();
    helper.0.wait().unwrap();
    assert_terminated(&grandchild);
}

#[test]
fn ordinary_command_exit_terminates_a_live_grandchild() {
    let root = tempfile::tempdir().unwrap();
    let mut helper = launch_worker(root.path());
    let grandchild = live_grandchild(root.path(), &mut helper.0);
    std::fs::write(root.path().join("release"), "exit").unwrap();
    let mut status = None;
    wait_until(|| {
        status = helper.0.try_wait().unwrap();
        status.is_some()
    });
    assert_eq!(status.unwrap().code(), Some(0));
    assert_terminated(&grandchild);
}

#[test]
fn helper_preserves_command_exit_status_and_refuses_invalid_invocations() {
    let helper = env!("CARGO_BIN_EXE_cyber-sandbox-exec");
    let output = Command::new(helper)
        .args(["--job", "--", "cmd.exe", "/D", "/C", "exit /B 23"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(23));
    let output = Command::new(helper)
        .args([
            "--job",
            "--invalid",
            "cmd.exe",
            "/C",
            "echo unexpected-child",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let output = Command::new(helper)
        .args(["--job", "--", "cyber-job-test-nonexistent-command.exe"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(69));
    assert!(String::from_utf8_lossy(&output.stderr).contains("cannot launch command"));
}

#[test]
#[allow(clippy::zombie_processes)] // The test must leave this descendant alive for job cleanup.
fn windows_job_worker() {
    let Ok(role) = std::env::var("CYBER_JOB_TEST_ROLE") else {
        return;
    };
    let root = std::path::PathBuf::from(std::env::var_os("CYBER_JOB_TEST_ROOT").unwrap());
    if role == "grandchild" {
        let marker = root.join("grandchild.pid.tmp");
        std::fs::write(&marker, std::process::id().to_string()).unwrap();
        std::fs::rename(marker, root.join("grandchild.pid")).unwrap();
        std::thread::sleep(Duration::from_secs(30));
        return;
    }
    let _grandchild = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "windows_job_worker", "--nocapture"])
        .env("CYBER_JOB_TEST_ROLE", "grandchild")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    wait_until(|| root.join("release").is_file());
}

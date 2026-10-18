//! Native PowerShell tool registration, authorization and execution contracts.
#![cfg(windows)]
#![allow(unsafe_code)]

mod support;

use cyber_server::runtime::ToolOutcome;
use serde_json::json;
use support::{Fixture, failed, ok};

fn fixture() -> Fixture {
    let f = Fixture::new();
    f.set_config(json!({"sandbox": {"policy": "full-access"}, "permissions": {"bash": "allow"}}));
    f
}

#[tokio::test]
async fn native_powershell_golden_and_unicode_source() {
    let f = fixture();
    assert!(
        f.tool_names("default", false)
            .contains(&"powershell".to_string())
    );
    support::golden(
        &f,
        "powershell",
        &ok(f
            .call(
                "default",
                "powershell",
                json!({"command": "Write-Output 'native-powershell'; exit 23"}),
            )
            .await),
    );
    let output = ok(f.call("default", "powershell", json!({"command": "Write-Output 'quotes \" & 💡'; Write-Output $env:CYBER; Write-Output $env:CYBER_SESSION_ID"})).await);
    assert!(output.contains("quotes \" & 💡"), "{output}");
    assert!(output.lines().any(|s| s == "1"), "{output}");
    assert!(output.contains("ses_test"), "{output}");
}

#[tokio::test]
async fn native_powershell_uses_bash_rules_for_filtering_and_compound_denies() {
    let f = fixture();
    assert!(
        !f.tool_names("plan", false)
            .contains(&"powershell".to_string())
    );
    f.set_config(json!({"sandbox": {"policy": "full-access"}, "permissions": {"bash": {"*": "allow", "Write-Host *": "deny"}}}));
    let result = failed(
        f.call(
            "default",
            "powershell",
            json!({"command": "Write-Output 'allowed'; Write-Host 'denied'"}),
        )
        .await,
    );
    assert!(result.contains("denied by"), "{result}");
    f.set_config(json!({"permissions": {"bash": "deny"}}));
    assert!(
        !f.tool_names("default", false)
            .contains(&"powershell".to_string())
    );
}

#[tokio::test]
async fn native_powershell_critical_roots_override_allows_and_unavailable_confinement_refuses_launch()
 {
    let f = fixture();
    // Nonrecursive deletion of this nonempty owned repo is harmless even if the guard regresses.
    let command = format!(
        "[IO.Directory]::Delete('{}')",
        f.repo.display().to_string().replace('\'', "''")
    );
    for mode in ["auto", "dont-ask", "bypass"] {
        let result = failed(
            f.call(mode, "powershell", json!({"command": command}))
                .await,
        );
        assert!(result.contains("critical path"), "{mode}: {result}");
        assert!(f.repo.join(".git").is_dir());
    }
    f.set_config(json!({"permissions": {"bash": "allow"}}));
    let result = failed(
        f.call(
            "default",
            "powershell",
            json!({"command": "Write-Output 'never launched'"}),
        )
        .await,
    );
    assert!(result.contains("SandboxUnavailableError"), "{result}");
}

#[tokio::test]
async fn native_powershell_validates_empty_background_and_missing_helper_calls() {
    let f = fixture();
    assert_eq!(
        failed(
            f.call("default", "powershell", json!({"command": "  "}))
                .await
        ),
        "command is empty"
    );
    assert!(
        failed(
            f.call(
                "default",
                "powershell",
                json!({"command": "Write-Output hi", "background": true})
            )
            .await
        )
        .contains("foreground")
    );
    let f = Fixture::with_shell("bash", None);
    f.set_config(json!({"sandbox": {"policy": "full-access"}, "permissions": {"bash": "allow"}}));
    assert!(
        failed(
            f.call(
                "default",
                "powershell",
                json!({"command": "Write-Output hi"})
            )
            .await
        )
        .contains("required for Windows process ownership")
    );
    assert!(!matches!(
        f.call("default", "powershell", json!({"command": 5})).await,
        ToolOutcome::Ok(_)
    ));
}

async fn approved(source: &str, timeout_ms: u64) -> (support::flow::Flow, String) {
    use cyber_server::runtime::{NoSnapshots, PendingKind, PermissionReply};
    use support::flow::{Flow, call, text};
    let flow = Flow::with(
        fixture(),
        vec![
            call(
                "native",
                "powershell",
                json!({"command": source, "timeout_ms": timeout_ms}),
            ),
            text("done"),
        ],
        true,
        std::sync::Arc::new(NoSnapshots),
    );
    let id = flow.session("default").await;
    flow.prompt(&id, "Run the native test command").await;
    let pending = flow.pending(&id).await;
    let PendingKind::Permission(ask) = &pending.kind else {
        panic!("expected command permission")
    };
    assert_eq!(ask.action, "bash");
    assert_eq!(ask.metadata["requires_confirmation"], true);
    flow.runtime
        .reply_permission(&pending.id, PermissionReply::Once)
        .await
        .unwrap();
    (flow, id)
}

#[tokio::test]
async fn native_powershell_closed_stdin_after_individual_approval() {
    let (flow, id) = approved("Write-Output ([Console]::ReadLine() -eq $null)", 10000).await;
    flow.settle(&id).await;
    assert_eq!(flow.output(&id, "native").await, "True");
}

#[tokio::test]
async fn native_powershell_timeout_and_cancellation_settle_and_terminate_the_interpreter() {
    use cyber_server::runtime::CallStatus;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use std::time::Duration;
    use windows_sys::Win32::Foundation::WAIT_OBJECT_0;
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject,
    };

    for cancelled in [false, true] {
        // Publish the interpreter identity only after dispatch, before entering the long sleep.
        let (flow, id) = approved(
            "Set-Content -LiteralPath 'interpreter.pid' -Value $PID; Start-Sleep -Seconds 60",
            10000,
        )
        .await;
        let marker = flow.f.repo.join("interpreter.pid");
        let pid = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Some(pid) = std::fs::read_to_string(&marker)
                    .ok()
                    .and_then(|s| s.trim().parse::<u32>().ok())
                {
                    break pid;
                }
                assert!(
                    flow.runtime.is_running(&id),
                    "command ended before publishing its identity"
                );
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("PowerShell did not start");
        // Keep the exact process identity alive; a reused PID cannot satisfy the assertion.
        let raw = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
        assert!(!raw.is_null(), "{}", std::io::Error::last_os_error());
        let interpreter = unsafe { OwnedHandle::from_raw_handle(raw) };
        assert_ne!(
            unsafe { WaitForSingleObject(interpreter.as_raw_handle(), 0) },
            WAIT_OBJECT_0
        );
        if cancelled {
            tokio::time::timeout(Duration::from_secs(5), flow.runtime.interrupt(&id))
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                flow.runtime.state(&id).await.unwrap().calls["native"].status,
                CallStatus::OutcomeUnknown
            );
        } else {
            tokio::time::timeout(Duration::from_secs(20), flow.runtime.wait_idle(&id))
                .await
                .unwrap();
            assert!(
                flow.output(&id, "native")
                    .await
                    .contains("Command timed out after 10000 ms")
            );
        }
        assert_eq!(
            unsafe { WaitForSingleObject(interpreter.as_raw_handle(), 5000) },
            WAIT_OBJECT_0
        );
        assert!(flow.runtime.pending_requests(Some(&id)).is_empty());
    }
}

/// `background: true` on native Windows: the Job records the exit code and log, and
/// `task_stop` terminates the Job Object (`background-tasks` → Stop kills process trees).
#[tokio::test]
async fn native_powershell_background_job_records_exit_code_and_stops_its_job_object() {
    use cyber_server::runtime::{JobStatus, ToolHost};
    use support::flow::{Flow, text};
    let flow = Flow::new(vec![text("notice handled")], false);
    flow.f.set_config(
        json!({"sandbox": {"policy": "full-access"}, "permissions": {"bash": "allow"}}),
    );
    let session = flow.session("default").await;
    let run = |input: serde_json::Value| {
        let mut inv = flow.f.invocation("default", "powershell", input);
        inv.session_id = session.clone();
        flow.f
            .host
            .execute(inv, tokio_util::sync::CancellationToken::new())
    };
    let started: serde_json::Value = serde_json::from_str(&ok(run(
        json!({"command": "Write-Output 'bg-out'; exit 4", "background": true}),
    )
    .await))
    .unwrap();
    let id = started["job_id"].as_str().unwrap().to_owned();
    let job = tokio::time::timeout(std::time::Duration::from_secs(60), async {
        loop {
            let job = flow.runtime.job(&id).unwrap();
            if job.status != JobStatus::Running && job.notified {
                return job;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the job did not end");
    assert_eq!(job.status, JobStatus::Error);
    assert_eq!(job.exit_code, Some(4));
    assert_eq!(job.kind, "powershell");
    let log = std::fs::read_to_string(job.output_path.unwrap()).unwrap();
    assert!(log.contains("bg-out"), "{log}");
    let sleeper: serde_json::Value = serde_json::from_str(&ok(run(
        json!({"command": "Start-Sleep -Seconds 120", "background": true, "notify": false}),
    )
    .await))
    .unwrap();
    let stopped = std::time::Instant::now();
    let job = flow
        .runtime
        .cancel_job(sleeper["job_id"].as_str().unwrap())
        .await
        .unwrap();
    assert_eq!(job.status, JobStatus::Cancelled);
    assert!(stopped.elapsed() < std::time::Duration::from_secs(30));
}

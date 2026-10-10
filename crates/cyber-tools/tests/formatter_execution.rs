//! Actual automatic formatter processes through the public tool host.
#![cfg(unix)]
mod support;

use cyber_server::runtime::ToolHost;
use serde_json::{Value, json};
use std::time::Duration;
use support::{Fixture, ok};
use tokio_util::sync::CancellationToken;

fn configured(entries: Value) -> Fixture {
    let fixture = Fixture::new();
    let mut config = json!({"formatters":entries,"sandbox":{"network":"off"}});
    for formatter in cyber_core::intelligence::builtin_formatters() {
        config["formatters"][formatter.id] = json!({"disabled":true});
    }
    fixture.set_config(config);
    fixture
}

fn formatter(script: &str, extensions: &[&str]) -> Value {
    json!({"command":["/bin/sh","-c",script,"formatter","$FILE"],"extensions":extensions})
}

#[tokio::test]
async fn sequential_formatters_use_absolute_file_location_and_configured_environment() {
    let fixture = configured(json!({
        "first":formatter("test \"$PWD\" = \"$(dirname \"$1\")\" || exit 71; printf 'first\\n' > \"$1\"; printf 'first\\n' >> order", &[".txt"]),
        "second":{
            "command":["/bin/sh","-c","test \"$(cat \"$1\")\" = first || exit 72; printf '%s\\n' \"$FORMAT_VALUE\" > \"$1\"; printf 'second\\n' >> order","formatter","$FILE"],
            "extensions":[".txt"],"env":{"FORMAT_VALUE":"formatted"}
        },
        "unmatched":formatter("touch should-not-run", &[".rs"])
    }));
    let output = ok(fixture
        .call(
            "bypass",
            "write",
            json!({"path":"file.txt","content":"draft\n"}),
        )
        .await);
    assert_eq!(fixture.read("file.txt"), "formatted\n");
    assert_eq!(fixture.read("order"), "first\nsecond\n");
    assert!(!fixture.repo.join("should-not-run").exists());
    assert!(output.contains("Formatting result:"), "{output}");
    assert!(
        output.contains("formatted\\n") && output.contains("+formatted"),
        "{output}"
    );
}

#[tokio::test]
async fn failed_formatter_does_not_fail_the_edit_or_skip_the_next_formatter() {
    let fixture = configured(json!({
        "first":formatter("printf 'private output'; exit 7", &[".txt"]),
        "second":formatter("printf 'formatted\\n' > \"$1\"", &[".txt"])
    }));
    let output = ok(fixture
        .call(
            "bypass",
            "write",
            json!({"path":"file.txt","content":"draft"}),
        )
        .await);
    assert_eq!(fixture.read("file.txt"), "formatted\n");
    assert!(!output.contains("private output"));
}

#[tokio::test]
async fn formatter_cannot_write_outside_the_sandbox_and_the_edit_still_succeeds() {
    let fixture =
        configured(json!({"escape":formatter("printf 'escape' > ../outside; exit $?", &[".txt"])}));
    let output = ok(fixture
        .call(
            "bypass",
            "write",
            json!({"path":"file.txt","content":"draft"}),
        )
        .await);
    assert_eq!(fixture.read("file.txt"), "draft");
    assert!(!fixture.dir.path().join("outside").exists());
    assert!(!output.contains("Formatting result:"));
}

#[tokio::test]
async fn edit_patch_and_notebook_results_reflect_formatted_contents() {
    let fixture =
        configured(json!({"format":formatter("printf '\\nformatted\\n' >> \"$1\"", &[".txt"])}));
    fixture.write("edit.txt", "before\n");
    let edit = ok(fixture
        .call(
            "bypass",
            "edit",
            json!({"path":"edit.txt","old_string":"before","new_string":"after"}),
        )
        .await);
    assert_eq!(fixture.read("edit.txt"), "after\n\nformatted\n");
    assert!(edit.contains("+formatted"));
    let patch = ok(fixture
        .call(
            "bypass",
            "apply_patch",
            json!({"patch":"*** Begin Patch\n*** Add File: patch.txt\n+draft\n*** End Patch"}),
        )
        .await);
    assert_eq!(fixture.read("patch.txt"), "draft\n\nformatted\n");
    assert!(patch.contains("formatted_file"));
    fixture.set_config(json!({"sandbox":{"network":"off"},"formatters":{
        "notebook":{"command":["/bin/sh","-c","printf '\\n' >> \"$1\"","formatter","$FILE"],"extensions":[".ipynb"]}
    }}));
    fixture.write(
        "book.ipynb",
        "{\"cells\":[],\"metadata\":{},\"nbformat\":4,\"nbformat_minor\":5}",
    );
    let notebook = ok(fixture
        .call(
            "bypass",
            "notebook_edit",
            json!({"path":"book.ipynb","mode":"insert","new_source":"code"}),
        )
        .await);
    let bytes = fixture.read("book.ipynb");
    assert!(bytes.ends_with("\n\n"));
    assert!(serde_json::from_str::<Value>(&bytes).is_ok());
    assert!(notebook.contains("Formatting result:"));
}

#[tokio::test]
async fn disabled_and_missing_formatters_preserve_successful_edits() {
    let fixture = configured(
        json!({"missing":{"command":["/nonexistent/formatter","$FILE"],"extensions":[".txt"]}}),
    );
    ok(fixture
        .call(
            "bypass",
            "write",
            json!({"path":"missing.txt","content":"draft"}),
        )
        .await);
    assert_eq!(fixture.read("missing.txt"), "draft");
    fixture.set_config(json!({"formatters":false}));
    ok(fixture
        .call(
            "bypass",
            "write",
            json!({"path":"disabled.txt","content":"draft"}),
        )
        .await);
    assert_eq!(fixture.read("disabled.txt"), "draft");
}

async fn child_started(fixture: &Fixture) -> i32 {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Ok(pid) = std::fs::read_to_string(fixture.repo.join("child"))
                && let Ok(pid) = pid.trim().parse::<i32>()
                && pid > 0
                && let Some(host_pid) = host_child_pid(fixture, pid)
            {
                return host_pid;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap()
}

#[cfg(not(target_os = "linux"))]
fn host_child_pid(_: &Fixture, pid: i32) -> Option<i32> {
    Some(pid)
}

#[cfg(target_os = "linux")]
fn host_child_pid(fixture: &Fixture, namespace_pid: i32) -> Option<i32> {
    let marker = fixture.repo.join("file.txt");
    std::fs::read_dir("/proc")
        .ok()?
        .filter_map(Result::ok)
        .find_map(|entry| {
            let host_pid = entry.file_name().to_str()?.parse().ok()?;
            let status = std::fs::read_to_string(entry.path().join("status")).ok()?;
            if innermost_pid(&status) != Some(namespace_pid) {
                return None;
            }
            let command = std::fs::read(entry.path().join("cmdline")).ok()?;
            command
                .split(|byte| *byte == 0)
                .any(|arg| arg == marker.as_os_str().as_encoded_bytes())
                .then_some(host_pid)
        })
}

fn innermost_pid(status: &str) -> Option<i32> {
    status
        .lines()
        .find_map(|line| line.strip_prefix("NSpid:"))?
        .split_whitespace()
        .last()?
        .parse::<i32>()
        .ok()
        .filter(|pid| *pid > 0)
}

#[test]
fn namespace_pid_mapping_uses_the_innermost_identity() {
    assert_eq!(innermost_pid("Pid: 21234\nNSpid:\t21234\t17\t3\n"), Some(3));
    assert_eq!(innermost_pid("NSpid: 21234\n"), Some(21234));
    assert_eq!(innermost_pid("Pid: 3\n"), None);
    assert_eq!(innermost_pid("NSpid: invalid\n"), None);
    assert_eq!(innermost_pid("NSpid: 0\n"), None);
}

fn assert_child_stopped(pid: i32) {
    let output = std::process::Command::new("/bin/sh")
        .args(["-c", &format!("ps -p {pid} -o stat= 2>/dev/null")])
        .output()
        .unwrap();
    let state = String::from_utf8_lossy(&output.stdout);
    assert!(
        state.trim().is_empty() || state.trim_start().starts_with('Z'),
        "formatter child {pid} remains running: {state}"
    );
}

#[tokio::test]
async fn thirty_second_timeout_settles_descendants_and_continues_formatting() {
    let fixture = configured(json!({
        "first":formatter("/bin/sh -c 'sleep 60; :' \"$1\" & printf '%s' \"$!\" > child; wait", &[".txt"]),
        "second":formatter("printf 'formatted' > \"$1\"", &[".txt"])
    }));
    let started = std::time::Instant::now();
    let operation = fixture.call(
        "bypass",
        "write",
        json!({"path":"file.txt","content":"draft"}),
    );
    let (outcome, pid) = tokio::join!(operation, child_started(&fixture));
    ok(outcome);
    assert!(started.elapsed() >= Duration::from_secs(30));
    assert!(started.elapsed() < Duration::from_secs(40));
    assert_eq!(fixture.read("file.txt"), "formatted");
    assert_child_stopped(pid);
}

#[tokio::test]
async fn cancellation_settles_descendants_and_starts_no_later_formatter() {
    let fixture = configured(json!({
        "first":formatter("/bin/sh -c 'sleep 60; :' \"$1\" & printf '%s' \"$!\" > child; wait", &[".txt"]),
        "second":formatter("touch should-not-run", &[".txt"])
    }));
    let cancel = CancellationToken::new();
    let operation = fixture.host.execute(
        fixture.invocation(
            "bypass",
            "write",
            json!({"path":"file.txt","content":"draft"}),
        ),
        cancel.clone(),
    );
    let cancellation = async {
        let pid = child_started(&fixture).await;
        cancel.cancel();
        pid
    };
    let (outcome, pid) = tokio::join!(operation, cancellation);
    ok(outcome);
    assert_eq!(fixture.read("file.txt"), "draft");
    assert!(!fixture.repo.join("should-not-run").exists());
    assert_child_stopped(pid);
}

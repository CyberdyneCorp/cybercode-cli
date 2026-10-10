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
            if let Ok(pid) = std::fs::read_to_string(fixture.repo.join("child")) {
                return pid.trim().parse().unwrap();
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap()
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
        "first":formatter("sleep 60 & printf '%s' \"$!\" > child; wait", &[".txt"]),
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
        "first":formatter("sleep 60 & printf '%s' \"$!\" > child; wait", &[".txt"]),
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

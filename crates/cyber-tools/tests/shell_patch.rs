//! `apply_patch` through the shell and profile output budgets, through the host
//! (`builtin-tools` → apply_patch through the shell, `tool-registry` → Profile output
//! budgets).

mod support;

use cyber_llm::catalog::{ToolOutput, Truncation};
use cyber_server::runtime::ToolHost;
use serde_json::{Value, json};
use support::{Fixture, failed, ok};

const PATCH: &str = "*** Begin Patch\n*** Update File: a.txt\n@@\n-old\n+new\n*** End Patch";

fn heredoc(prefix: &str) -> String {
    format!("{prefix}apply_patch <<'EOF'\n{PATCH}\nEOF\n")
}

fn intercepted(f: &Fixture, call: &str) -> Option<Value> {
    f.host.take_call_metadata("ses_test", call)
}

/// "Patch sent through bash": the file changes through `apply_patch`, the result is
/// `M <path>`, and the record notes the interception.
#[tokio::test]
async fn a_bash_heredoc_runs_as_apply_patch() {
    let f = Fixture::new();
    f.write("a.txt", "old\n");
    let out = ok(f
        .call("accept-edits", "bash", json!({"command": heredoc("")}))
        .await);
    assert_eq!(out, "M a.txt");
    assert_eq!(f.read("a.txt"), "new\n");
    assert_eq!(
        intercepted(&f, "call_bash"),
        Some(json!({"intercepted": "apply_patch"}))
    );
    assert_eq!(intercepted(&f, "call_bash"), None, "taken once");
}

/// `cd <dir> &&` makes the patch paths relative to that directory.
#[tokio::test]
async fn a_cd_prefix_moves_the_patch_base() {
    let f = Fixture::new();
    f.write("sub/a.txt", "old\n");
    let out = ok(f
        .call(
            "accept-edits",
            "bash",
            json!({"command": heredoc("cd sub && ")}),
        )
        .await);
    assert_eq!(out, "M sub/a.txt");
    assert_eq!(f.read("sub/a.txt"), "new\n");
}

/// The call is approved as an `edit`: an allowed `bash` does not let a denied edit
/// through.
#[tokio::test]
async fn the_edit_permission_decides_an_intercepted_patch() {
    let f = Fixture::new();
    f.set_config(json!({"permissions": {"bash": "allow", "edit": "deny"}}));
    f.write("a.txt", "old\n");
    let out = failed(
        f.call("default", "bash", json!({"command": heredoc("")}))
            .await,
    );
    assert!(!out.is_empty());
    assert_eq!(f.read("a.txt"), "old\n");
}

/// A `powershell` script (registered on Windows) holding a here-string patch is
/// intercepted as well.
#[cfg(windows)]
#[tokio::test]
async fn a_powershell_here_string_runs_as_apply_patch() {
    let f = Fixture::new();
    f.write("a.txt", "old\n");
    let script = format!("cd . && apply_patch @'\n{PATCH}\n'@");
    let out = ok(f
        .call("accept-edits", "powershell", json!({"command": script}))
        .await);
    assert_eq!(out, "M a.txt");
    assert_eq!(f.read("a.txt"), "new\n");
    assert_eq!(
        intercepted(&f, "call_powershell"),
        Some(json!({"intercepted": "apply_patch"}))
    );
}

/// With an `apply_patch` program on `PATH`, the script runs as written.
#[cfg(unix)]
#[tokio::test]
async fn an_installed_apply_patch_program_is_left_alone() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new();
    f.set_config(json!({"permissions": {"bash": "allow"}}));
    f.write("a.txt", "old\n");
    let bin = f.dir.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let program = bin.join("apply_patch");
    std::fs::write(&program, "#!/bin/sh\necho installed program ran\n").unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = std::env::join_paths(std::iter::once(bin).chain(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    )))
    .unwrap();
    f.env.set("PATH", path.to_str().unwrap());
    let _ = f
        .call("default", "bash", json!({"command": heredoc("")}))
        .await;
    assert_eq!(intercepted(&f, "call_bash"), None);
    assert_eq!(f.read("a.txt"), "old\n", "our patch tool did not run");
}

/// "Long build log for an OpenAI model": a middle profile keeps about 20 KB of each
/// end of a 200 KB log around one marker, and the overflow file holds all of it.
#[tokio::test]
async fn a_middle_profile_keeps_both_ends_of_a_long_log() {
    let f = Fixture::new();
    f.set_config(json!({"permissions": {"bash": "allow"}}));
    f.host.set_output_profile(
        "ses_test",
        ToolOutput {
            tokens: Some(10_000),
            truncation: Truncation::Middle,
        },
    );
    let script = "i=0; while [ $i -lt 4000 ]; do echo \"step $i of the build writes this fairly long log line\"; i=$((i+1)); done";
    let out = ok(f.call("default", "bash", json!({"command": script})).await);
    let marker = out
        .lines()
        .find(|line| line.starts_with("…["))
        .expect("one omission marker");
    assert_eq!(out.matches("…[").count(), 1);
    let (head, tail) = out.split_once(marker).unwrap();
    assert!(head.starts_with("step 0 of the build"), "{}", &head[..40]);
    assert!(tail.contains("step 3999 of the build"));
    for part in [head, tail] {
        assert!((18_000..=20_000).contains(&part.len()), "{}", part.len());
    }
    let path = marker
        .split("full output at ")
        .nth(1)
        .unwrap()
        .trim_end_matches("]…");
    let full = std::fs::read_to_string(path).unwrap();
    assert!(full.len() > 200_000 && full.contains("step 2000 of the build"));
}

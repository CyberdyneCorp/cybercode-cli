//! `--dangerously-skip-permissions` (Claude Code's name for running without prompts):
//! the refusals that happen before any server starts (`exec-mode` → Model, agent and
//! mode selection, `tui` → Launch flags). The runs that reach a Session are in
//! exec_lifecycle_hooks.rs, next to the other `bypass` tests.

use std::process::{Command, Output, Stdio};

fn cyber(args: &[&str]) -> Output {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let mut command = Command::new(env!("CARGO_BIN_EXE_cyber"));
    command
        .args(args)
        .env_clear()
        .env("HOME", root.join("home"))
        .env("CYBER_HOME", root.join("cyber-home"))
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .current_dir(root)
        .stdin(Stdio::null());
    #[cfg(windows)]
    if let Some(system) = std::env::var_os("SystemRoot") {
        command.env("SystemRoot", system);
    }
    command.output().unwrap()
}

fn refused(output: &Output, message: &str) {
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(2), "{stderr}");
    assert!(stderr.contains(message), "{stderr}");
}

/// The shared server cannot take the no-sandbox override for one run, so exec refuses
/// before starting or contacting it, and says which flags make it work.
#[test]
fn exec_skip_permissions_on_the_shared_server_names_the_private_server_flags() {
    let output = cyber(&["exec", "--dangerously-skip-permissions", "hello"]);
    refused(
        &output,
        "--dangerously-skip-permissions needs --embedded or --ephemeral: a shared server cannot take the no-sandbox override for one run",
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("--mode bypass"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let attached = cyber(&[
        "exec",
        "--attach",
        "http://127.0.0.1:9",
        "--dangerously-skip-permissions",
        "hello",
    ]);
    refused(
        &attached,
        "--dangerously-skip-permissions needs --embedded or --ephemeral",
    );
}

/// It selects `bypass`, so another Mode is a contradiction rather than a silent winner.
#[test]
fn exec_skip_permissions_conflicts_with_another_mode() {
    let output = cyber(&[
        "exec",
        "--embedded",
        "--mode",
        "plan",
        "--dangerously-skip-permissions",
        "hello",
    ]);
    refused(
        &output,
        "--dangerously-skip-permissions selects bypass mode and cannot be combined with --mode plan",
    );
    let auto = cyber(&[
        "exec",
        "--embedded",
        "--auto",
        "--dangerously-skip-permissions",
        "hello",
    ]);
    assert_eq!(auto.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&auto.stderr).contains("cannot be used with"),
        "{}",
        String::from_utf8_lossy(&auto.stderr)
    );
}

/// The TUI refuses it without `--embedded` before it needs a terminal.
#[test]
fn tui_skip_permissions_needs_an_embedded_server() {
    let output = cyber(&["--dangerously-skip-permissions"]);
    refused(
        &output,
        "--dangerously-skip-permissions needs --embedded: a shared server cannot take the no-sandbox override for one run",
    );
}

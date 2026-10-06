//! Native helper entry-point checks. No command may run through an unsupported bridge.

#[cfg(not(unix))]
#[test]
fn an_unsupported_proxy_bridge_refuses_to_launch_the_requested_command() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_cyber-sandbox-exec"))
        .args([
            "--forward",
            "0",
            "unused.socket",
            "--",
            "cmd.exe",
            "/C",
            "echo unexpected-child",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(69));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Unix proxy bridge is unavailable"));
}

#[cfg(unix)]
#[test]
fn invalid_helper_arguments_do_not_launch_a_command() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_cyber-sandbox-exec"))
        .args(["--invalid", "--", "/bin/sh", "-c", "echo unexpected-child"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("usage:"));
}

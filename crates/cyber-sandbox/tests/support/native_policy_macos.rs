// Actual packaged-helper receipts for the production Rust policy builder.
use super::*;
use cyber_sandbox::plugin_policy::{MacProfile, ReadPath};
use std::io::Write;
use std::process::Stdio;
use std::time::{Duration, Instant};

fn rpc(command: &mut Command) -> Value {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let packet = b"{\"jsonrpc\":\"2.0\",\"id\":\"native-proof\",\"method\":\"tool/execute\",\"params\":{\"name\":\"native-probe\",\"input\":{}}}\n";
    let _ = child.stdin.take().unwrap().write_all(packet);
    let deadline = Instant::now() + Duration::from_secs(15);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            let _ = child.kill();
            let output = child.wait_with_output().unwrap();
            panic!("native policy receipt timed out: {output:?}");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let result = child.wait_with_output().unwrap();
    assert!(
        result.status.success(),
        "native policy startup failed: {result:?}"
    );
    let packet: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(packet["jsonrpc"], "2.0");
    assert_eq!(packet["id"], "native-proof");
    assert!(packet.get("error").is_none());
    packet["result"].clone()
}

#[test]
fn rust_plugin_policy_preserves_native_startup_and_independent_capability_boundaries() {
    let fixture = Fixture::new();
    let project = fixture.root.join("project \"quoted\" \\ workspace");
    let package = project.join("installed-package");
    let scratch = fixture.root.join("scratch");
    let private = fixture.root.join("private");
    let protected = project.join("nested/owner-state");
    for path in [&package, &scratch, &private, &protected] {
        std::fs::create_dir_all(path).unwrap();
    }
    for path in [
        package.join("data"),
        project.join("data"),
        private.join("secret"),
        protected.join("secret"),
    ] {
        std::fs::write(path, b"fixture-only").unwrap();
    }
    symlink(private.join("secret"), package.join("alias")).unwrap();
    let compiled = compile(
        &fixture.root,
        include_str!("../../../../scripts/probes/plugin-confinement/fixture_native.c"),
        "policy-native",
    );
    let target = package.join("main");
    std::fs::copy(compiled, &target).unwrap();
    sign(&target, "cyber.test.policy.payload");
    let bootstrap = [
        ReadPath::Directory(Path::new("/System")),
        ReadPath::Directory(Path::new("/usr/lib")),
        ReadPath::Directory(Path::new("/usr/bin")),
        ReadPath::Directory(Path::new("/bin")),
    ];
    let private_roots = [private.as_path(), protected.as_path()];
    for read in [false, true] {
        for write in [false, true] {
            for shell in [false, true] {
                let plan = MacProfile {
                    target: &target,
                    helper_identifier: &fixture.helper_id,
                    package: &package,
                    scratch: &scratch,
                    location: &project,
                    bootstrap: &bootstrap,
                    private: &private_roots,
                    read_only: &[],
                    fs_read: read,
                    fs_write: write,
                    shell,
                };
                let policy = fixture.profile("rust-policy.sb", &plan.render().unwrap());
                let mut command = fixture.command(&policy, &target);
                command.arg("inspect");
                for path in [
                    package.join("data"),
                    project.join("data"),
                    private.join("secret"),
                    scratch.join("roundtrip"),
                    project.join("written"),
                    package.join("alias"),
                    protected.join("secret"),
                    scratch.clone(),
                    project.clone(),
                ] {
                    command.arg(path);
                }
                command
                    .args(["0", "0", "1"])
                    .env("CYBER_PROOF_PRIVATE", private.join("secret"));
                let observed = rpc(&mut command);
                assert_capabilities(observed, read, write, shell);
                let written = project.join("written");
                if written.exists() {
                    std::fs::remove_file(written).unwrap();
                }
            }
        }
    }
}

#[test]
fn rust_plugin_policy_blocks_private_package_and_runtime_ancestor_mutation() {
    let fixture = Fixture::new();
    let project = fixture.root.join("project");
    let package = project.join("extension-parent/package");
    let runtime = project.join("runtime-parent/lib");
    let private = project.join("private-parent/owner-state");
    let scratch = fixture.root.join("scratch");
    let public = project.join("public-parent");
    for directory in [&package, &runtime, &private, &scratch, &public] {
        std::fs::create_dir_all(directory).unwrap();
    }
    for root in [&package, &runtime, &private, &public] {
        std::fs::write(root.join("data"), b"unchanged-fixture").unwrap();
    }
    let target = compile(
        &fixture.root,
        r#"
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>
int main(int argc, char **argv) {
    if (argc != 4) return 2;
    int result;
    if (strcmp(argv[1], "rename") == 0) result = rename(argv[2], argv[3]);
    else if (strcmp(argv[1], "write") == 0) {
        int fd = open(argv[2], O_WRONLY | O_TRUNC);
        result = fd < 0 ? -1 : (write(fd, "changed-fixture", 15) == 15 ? 0 : -1);
        if (fd >= 0) close(fd);
    } else return 2;
    printf("{\"success\":%s,\"denied\":%s}\n", result == 0 ? "true" : "false",
           result < 0 && (errno == EPERM || errno == EACCES) ? "true" : "false");
    return 0;
}
"#,
        "mutation-probe",
    );
    sign(&target, "cyber.test.policy.mutation");
    let bootstrap = [
        ReadPath::Directory(Path::new("/System")),
        ReadPath::Directory(Path::new("/usr/lib")),
        ReadPath::Directory(&runtime),
    ];
    let private_roots = [private.as_path()];
    let plan = MacProfile {
        target: &target,
        helper_identifier: &fixture.helper_id,
        package: &package,
        scratch: &scratch,
        location: &project,
        bootstrap: &bootstrap,
        private: &private_roots,
        read_only: &[],
        fs_read: true,
        fs_write: true,
        shell: false,
    };
    let profile = fixture.profile("mutation-policy.sb", &plan.render().unwrap());
    for root in [&private, &package, &runtime] {
        let parent = root.parent().unwrap();
        let result = packet(successful(
            fixture
                .command(&profile, &target)
                .arg("rename")
                .arg(parent)
                .arg(project.join("moved")),
        ));
        assert_eq!(
            result,
            json!({"success":false,"denied":true}),
            "protected ancestor rename: {parent:?}"
        );
        assert_eq!(
            std::fs::read(root.join("data")).unwrap(),
            b"unchanged-fixture"
        );
    }
    for root in [&package, &runtime] {
        let result = packet(successful(
            fixture
                .command(&profile, &target)
                .arg("write")
                .arg(root.join("data"))
                .arg("unused"),
        ));
        assert_eq!(
            result,
            json!({"success":false,"denied":true}),
            "immutable content write: {root:?}"
        );
        assert_eq!(
            std::fs::read(root.join("data")).unwrap(),
            b"unchanged-fixture"
        );
    }
    let moved = project.join("public-moved");
    let result = packet(successful(
        fixture
            .command(&profile, &target)
            .arg("rename")
            .arg(&public)
            .arg(&moved),
    ));
    assert_eq!(result, json!({"success":true,"denied":false}));
    assert_eq!(
        std::fs::read(moved.join("data")).unwrap(),
        b"unchanged-fixture"
    );
}

fn assert_capabilities(observed: Value, read: bool, write: bool, shell: bool) {
    for key in [
        "stdio_tool",
        "package_read",
        "thread",
        "scratch_write",
        "scratch_read",
        "cannot_relax",
    ] {
        assert_eq!(observed[key], true, "{key}: {observed}");
    }
    for key in [
        "private_read",
        "private_list",
        "private_write",
        "protected_read",
        "protected_write",
        "symlink_private_read",
        "hardlink_private",
        "package_hardlink",
        "package_alias_write",
        "inherited_descriptor",
        "constructor_private_read",
        "project_rename",
    ] {
        assert_eq!(observed[key], false, "{key}: {observed}");
    }
    assert_eq!(observed["project_read"], read, "{observed}");
    assert_eq!(observed["project_write"], write, "{observed}");
    assert_eq!(
        observed["written_project_read"],
        read && write,
        "{observed}"
    );
    assert_eq!(observed["thread_reads"], json!([read, false]), "{observed}");
    for key in [
        "shell",
        "thread_shell",
        "native_fork",
        "constructor_fork",
        "native_spawn",
    ] {
        assert_eq!(observed[key], shell, "{key}: {observed}");
    }
    let children = if shell {
        json!([read, false])
    } else {
        Value::Null
    };
    assert_eq!(observed["child_reads"], children, "{observed}");
    assert_eq!(observed["native_child_reads"], children, "{observed}");
}

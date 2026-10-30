//! Real packaged Linux entry; no probe launcher or optional-policy path.
#![cfg(target_os = "linux")]
use serde_json::Value;
use std::io::Write;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

fn output(command: &mut Command, input: Option<&[u8]>) -> Output {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    if let Some(input) = input
        && let Err(error) = child.stdin.take().unwrap().write_all(input)
        && error.kind() != std::io::ErrorKind::BrokenPipe
    {
        let _ = child.kill();
        let _ = child.wait();
        panic!("native fixture input failed: {error}");
    }
    drop(child.stdin.take());
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if child.try_wait().unwrap().is_some() {
            return child.wait_with_output().unwrap();
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let result = child.wait_with_output().unwrap();
            panic!("native entry did not settle: {:?}", result);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}
fn success(command: &mut Command, input: Option<&[u8]>) -> Output {
    let result = output(command, input);
    assert!(result.status.success(), "{result:?}");
    result
}
struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    payload: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        for directory in ["project", "private", "scratch"] {
            std::fs::create_dir(root.join(directory)).unwrap();
        }
        for name in ["package", "project/read", "private/secret"] {
            std::fs::write(root.join(name), b"fixture").unwrap();
        }
        symlink(root.join("private/secret"), root.join("project/alias")).unwrap();
        let payload = root.join("payload");
        success(
            Command::new("cc")
                .args(["-Wall", "-Wextra", "-Werror", "-pthread"])
                .arg(
                    Path::new(env!("CARGO_MANIFEST_DIR"))
                        .join("../../scripts/probes/plugin-confinement/fixture_native.c"),
                )
                .arg("-o")
                .arg(&payload),
            None,
        );
        Self {
            _temp: temp,
            root,
            payload,
        }
    }
    fn policy(&self, read: bool, write: bool) -> PathBuf {
        let mut text = String::new();
        for path in ["/usr", "/lib", "/lib64", "/bin"] {
            if Path::new(path).exists() {
                text.push_str(&format!("R\t{path}\n"));
            }
        }
        for path in [&self.payload, &self.root.join("package")] {
            text.push_str(&format!("R\t{}\n", path.display()));
        }
        text.push_str("R\t/dev/null\nW\t/dev/null\n");
        text.push_str(&format!("W\t{}\n", self.root.join("scratch").display()));
        if read {
            text.push_str(&format!("R\t{}\n", self.root.join("project").display()));
        }
        if write {
            text.push_str(&format!("W\t{}\n", self.root.join("project").display()));
        }
        let path = self.root.join("policy");
        std::fs::write(&path, text).unwrap();
        path
    }
    fn command(&self, policy: &Path, shell: bool) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_cyber-sandbox-exec"));
        command
            .arg("--plugin-native-exec")
            .arg(policy)
            .arg(if shell {
                "--initial-shell=1"
            } else {
                "--initial-shell=0"
            })
            .arg(&self.payload)
            .env_clear()
            .env("CYBER_PROOF_PRIVATE", self.root.join("private/secret"));
        command
    }
    fn inspect(&self, read: bool, write: bool, shell: bool) -> Value {
        let policy = self.policy(read, write);
        let mut command = self.command(&policy, shell);
        command.arg("inspect");
        for name in [
            "package",
            "project/read",
            "private/secret",
            "scratch/out",
            "project/written",
            "project/alias",
            "private/secret",
            "scratch",
            "project",
        ] {
            command.arg(self.root.join(name));
        }
        command.args(["0", "0", "1"]);
        let result = success(&mut command, Some(b"{\"jsonrpc\":\"2.0\",\"id\":\"native-proof\",\"method\":\"tool/execute\",\"params\":{\"name\":\"native-probe\",\"input\":{}}}\n"));
        serde_json::from_slice::<Value>(&result.stdout).unwrap()["result"].clone()
    }
}

#[test]
fn native_reads_writes_threads_and_pre_main_policy_are_independent() {
    for (read, write) in [(false, true), (true, false)] {
        let f = Fixture::new();
        let result = f.inspect(read, write, false);
        for key in [
            "stdio_tool",
            "package_read",
            "thread",
            "scratch_write",
            "cannot_relax",
            "filter_replacement_denied",
        ] {
            assert_eq!(result[key], true, "{key}: {result}");
        }
        for key in [
            "private_read",
            "private_write",
            "private_list",
            "symlink_private_read",
            "hardlink_private",
            "protected_read",
            "protected_write",
            "scratch_read",
            "constructor_private_read",
            "constructor_fork",
            "native_fork",
            "native_spawn",
            "shell",
            "thread_shell",
            "inherited_descriptor",
        ] {
            assert_eq!(result[key], false, "{key}: {result}");
        }
        assert_eq!(result["project_read"], read);
        assert_eq!(result["project_list"], read);
        assert_eq!(result["project_write"], write);
        assert_eq!(result["written_project_read"], read && write);
        assert_eq!(result["thread_reads"], serde_json::json!([read, false]));
        assert_eq!(
            std::fs::read(f.root.join("private/secret")).unwrap(),
            b"fixture"
        );
    }
}

#[test]
fn shell_permission_controls_exec_and_descendants_inherit_read_restrictions() {
    let f = Fixture::new();
    let policy = f.policy(true, false);
    // Exact same executable and read roots: only the mandatory shell flag varies.
    for shell in [false, true] {
        let result = success(
            f.command(&policy, shell).arg("exec").args(["unused"; 12]),
            None,
        );
        let value: Value = serde_json::from_slice(&result.stdout).unwrap();
        assert_eq!(value["self_exec"], shell);
        let value = f.inspect(true, false, shell);
        for key in [
            "shell",
            "native_fork",
            "native_spawn",
            "constructor_fork",
            "thread_shell",
        ] {
            assert_eq!(value[key], shell, "{key}: {value}");
        }
        if shell {
            assert_eq!(value["child_reads"], serde_json::json!([true, false]));
            assert_eq!(
                value["native_child_reads"],
                serde_json::json!([true, false])
            );
        }
    }
}

#[test]
fn inherited_descriptors_above_lowered_limit_are_closed() {
    let f = Fixture::new();
    let payload = f.root.join("fd-payload");
    let wrapper = f.root.join("fd-wrapper");
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    for (source, destination) in [
        (
            manifest.join("../../scripts/probes/plugin-confinement/fixture_exec.c"),
            &payload,
        ),
        (
            manifest.join("tests/fixtures/plugin_native_fds.c"),
            &wrapper,
        ),
    ] {
        success(
            Command::new("cc")
                .args(["-Wall", "-Wextra", "-Werror"])
                .arg(source)
                .arg("-o")
                .arg(destination),
            None,
        );
    }
    let policy = f.policy(false, false);
    let mut text = std::fs::read_to_string(&policy).unwrap();
    text.push_str(&format!("R\t{}\n", payload.display()));
    std::fs::write(&policy, text).unwrap();
    for (helper, expected) in [
        ("-", true),
        (env!("CARGO_BIN_EXE_cyber-sandbox-exec"), false),
    ] {
        let result = success(
            Command::new(&wrapper)
                .arg(helper)
                .arg(&policy)
                .arg(&payload)
                .arg(f.root.join("private/secret")),
            None,
        );
        let value: Value = serde_json::from_slice(&result.stdout).unwrap();
        assert_eq!(value["inherited_descriptor"], serde_json::json!(expected));
    }
}

#[test]
fn invalid_profiles_and_missing_shell_policy_refuse_before_target() {
    let f = Fixture::new();
    let policy = f.root.join("bad-policy");
    for bytes in [
        Vec::new(),
        vec![b'R'; 65537],
        b"R\t/usr\0bad\n".to_vec(),
        b"R\t/\n".to_vec(),
        b"X\t/usr\n".to_vec(),
    ] {
        std::fs::write(&policy, bytes).unwrap();
        let result = output(f.command(&policy, false).arg("receipt"), None);
        assert_eq!(result.status.code(), Some(2), "{result:?}");
        assert!(result.stdout.is_empty());
    }
    std::fs::write(&policy, b"R\t/usr\n").unwrap();
    let linked = f.root.join("linked-policy");
    std::fs::hard_link(&policy, &linked).unwrap();
    let alias = f.root.join("policy-symlink");
    symlink(&policy, &alias).unwrap();
    for path in [&linked, &alias, &f.root, &f.root.join("missing")] {
        let result = output(f.command(path, false).arg("receipt"), None);
        assert_eq!(result.status.code(), Some(2), "{result:?}");
        assert!(result.stdout.is_empty());
    }
    std::fs::remove_file(&linked).unwrap();
    std::fs::remove_file(&policy).unwrap();
    success(Command::new("mkfifo").arg(&policy), None);
    let started = Instant::now();
    let result = output(f.command(&policy, false).arg("receipt"), None);
    assert_eq!(result.status.code(), Some(2));
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "profile open waited for a FIFO writer"
    );
    let result = output(
        Command::new(env!("CARGO_BIN_EXE_cyber-sandbox-exec"))
            .arg("--plugin-native-exec")
            .arg(f.policy(true, true))
            .arg(&f.payload)
            .arg("receipt"),
        None,
    );
    assert_eq!(result.status.code(), Some(64));
    assert!(result.stdout.is_empty());
}

#[path = "native_policy_linux.rs"]
mod policy;

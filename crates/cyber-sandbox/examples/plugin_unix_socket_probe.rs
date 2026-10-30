//! Diagnostic evidence, not capability acceptance. Uses only owned test sockets.
#[cfg(unix)]
#[path = "support/plugin_socket_cancel.rs"]
mod cancellation;
#[cfg(unix)]
mod probe {
    use cyber_sandbox::plugin_policy::{LinuxPolicy, MaskPath};
    use serde_json::{Value, json};
    use sha2::{Digest, Sha256};
    use std::io::Read;
    use std::os::unix::{fs::PermissionsExt, fs::symlink, net::UnixListener};
    use std::path::{Path, PathBuf};
    use std::process::{Command, Output, Stdio};
    use std::time::{Duration, Instant};

    fn run(command: &mut Command) -> Output {
        let mut child = command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if child.try_wait().unwrap().is_some() {
                let output = child.wait_with_output().unwrap();
                assert!(
                    output.status.success(),
                    "fixture bootstrap failed: {output:?}"
                );
                return output;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("fixture did not settle");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn receipt(listener: &UnixListener, observed: &Value) -> bool {
        assert_eq!(observed["started"], true);
        match listener.accept() {
            Ok((mut stream, _)) => {
                assert_eq!(observed["connected"], true);
                assert_eq!(observed["sent"], true);
                stream
                    .set_read_timeout(Some(Duration::from_secs(1)))
                    .unwrap();
                let mut token = [0; 12];
                stream.read_exact(&mut token).unwrap();
                assert_eq!(&token, b"fixture-only");
                true
            }
            Err(error) => {
                assert_eq!(error.kind(), std::io::ErrorKind::WouldBlock);
                assert_eq!(observed["connected"], false);
                assert_eq!(observed["sent"], false);
                false
            }
        }
    }

    #[derive(Clone, Copy, PartialEq)]
    enum Mode {
        Baseline,
        Seccomp,
        Broker,
        BrokerCancel,
        BrokerMutation,
        BrokerEscape,
        BrokerEscapeNoPid,
        Unix9,
    }
    impl Mode {
        fn ip_servers(self) -> bool {
            matches!(
                self,
                Self::Unix9
                    | Self::Broker
                    | Self::BrokerCancel
                    | Self::BrokerMutation
                    | Self::BrokerEscape
                    | Self::BrokerEscapeNoPid
            )
        }
        fn broker(self) -> bool {
            matches!(
                self,
                Self::Broker
                    | Self::BrokerCancel
                    | Self::BrokerMutation
                    | Self::BrokerEscape
                    | Self::BrokerEscapeNoPid
            )
        }
        fn escape(self) -> Option<bool> {
            match self {
                Self::BrokerEscape => Some(true),
                Self::BrokerEscapeNoPid => Some(false),
                _ => None,
            }
        }
    }

    fn observe(policy: &LinuxPolicy<'_>, mode: Mode) -> Value {
        let mut plan = policy.compose().unwrap();
        if mode == Mode::Unix9 {
            plan.landlock
                .push_str(&format!("U\t{}\n", policy.scratch.display()));
        }
        std::fs::write(policy.profile, plan.landlock).unwrap();
        serde_json::from_slice(
            &run(Command::new("/usr/bin/bwrap")
                .args(plan.arguments)
                .env_clear())
            .stdout,
        )
        .unwrap()
    }

    fn pair_row(
        policy: LinuxPolicy<'_>,
        listener: &UnixListener,
        label: &str,
        mode: Mode,
    ) -> Value {
        let mut pair_args = vec![
            policy.target_args[0].clone(),
            policy.target_args[1].clone(),
            "pair".into(),
        ];
        if mode.ip_servers() {
            pair_args.push(policy.scratch.join("local.sock").to_str().unwrap().into());
        }
        let pair_policy = LinuxPolicy {
            target_args: &pair_args,
            ..policy
        };
        let observed = observe(&pair_policy, mode);
        assert_eq!(
            observed["project_read"], pair_policy.fs_read,
            "native pair read control: {observed}"
        );
        assert_eq!(
            observed["pair_roundtrip"], true,
            "private pair communication: {observed}"
        );
        for key in ["connected", "pair_reconnect", "pair_bind"] {
            assert_eq!(
                observed[key], false,
                "private pair escape: {key}: {observed}"
            );
        }
        check_pair_support(&observed, mode);
        let delivered = receipt(listener, &observed);
        assert!(!delivered, "pair reached host: {observed}");
        json!({"socket": label, "fs_read": pair_policy.fs_read, "fs_write": pair_policy.fs_write,
                        "shell": false, "mode": "pair", "host_received_fixture": delivered, "observed": observed})
    }

    fn check_pair_support(observed: &Value, mode: Mode) {
        for key in ["datagram_pair", "seqpacket_pair", "scratch_unix_roundtrip"] {
            assert_eq!(
                observed[key],
                mode == Mode::Unix9,
                "pair/local support: {key}: {observed}"
            );
        }
        assert_eq!(
            observed["tcp_roundtrip"],
            mode.ip_servers(),
            "IP server/client token exchange: {observed}"
        );
        assert_eq!(
            observed["thread_tcp_roundtrip"],
            mode.ip_servers(),
            "thread-local IP descriptor token exchange: {observed}"
        );
        if mode == Mode::Seccomp || mode.broker() {
            assert_eq!(observed["pair_listen"], false, "pair listener: {observed}");
        }
    }

    fn candidate_rows(
        policy: LinuxPolicy<'_>,
        listener: &UnixListener,
        label: &str,
        mode: Mode,
    ) -> Vec<Value> {
        let mut rows = Vec::new();
        if let Some(private_pid) = mode.escape()
            && label == "outside"
        {
            let observed = super::cancellation::observe_escape(&policy, private_pid);
            rows.push(json!({"mode":"escape","socket":label,"fs_read":policy.fs_read,
                "fs_write":policy.fs_write,"shell":true,"private_pid_namespace":private_pid,"observed":observed}));
        }
        if mode == Mode::BrokerMutation && label == "outside" {
            let observed = super::cancellation::observe_mutation(&policy);
            rows.push(
                json!({"mode":"mutation","socket":label,"fs_read":policy.fs_read,
                "fs_write":policy.fs_write,"shell":false,"observed":observed}),
            );
        }
        if mode == Mode::BrokerCancel && label == "outside" {
            let observed = super::cancellation::observe(&policy);
            rows.push(
                json!({"mode":"cancel","socket":label,"fs_read":policy.fs_read,
                "fs_write":policy.fs_write,"shell":false,"observed":observed}),
            );
        }
        rows.push(pair_row(policy, listener, label, mode));
        rows
    }

    fn options() -> (PathBuf, PathBuf, Mode) {
        assert_eq!(
            std::env::consts::OS,
            "linux",
            "native evidence requires Linux"
        );
        let args: Vec<_> = std::env::args_os().skip(1).collect();
        assert!(
            matches!(args.len(), 2 | 3),
            "expected helper, report and optional candidate flag"
        );
        let mode = match args.get(2).and_then(|arg| arg.to_str()) {
            None => Mode::Baseline,
            Some("--candidate") => Mode::Seccomp,
            Some("--broker") => Mode::Broker,
            Some("--broker-cancel") => Mode::BrokerCancel,
            Some("--broker-mutation") => Mode::BrokerMutation,
            Some("--broker-escape") => Mode::BrokerEscape,
            Some("--broker-escape-no-pid") => Mode::BrokerEscapeNoPid,
            Some("--landlock9") => Mode::Unix9,
            _ => panic!("unknown candidate mode"),
        };
        let helper = std::fs::canonicalize(&args[0]).unwrap();
        (helper, PathBuf::from(&args[1]), mode)
    }

    pub fn main() {
        let (helper, report_path, mode) = options();
        let candidate = mode != Mode::Baseline;
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let project = root.join("project");
        let package = project.join("package");
        let private = project.join("private");
        let scratch = root.join("scratch");
        for directory in [&project, &package, &private, &scratch] {
            std::fs::create_dir_all(directory).unwrap();
        }
        let data = project.join("data");
        std::fs::write(&data, b"fixture-only").unwrap();
        let target = package.join("socket-client");
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        run(Command::new("cc")
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .args(["-Wall", "-Wextra", "-Werror", "-pthread"])
            .arg(workspace.join("crates/cyber-sandbox/tests/fixtures/plugin_unix_socket.c"))
            .arg("-o")
            .arg(&target));
        let mask = root.join("empty-mask");
        std::fs::write(&mask, []).unwrap();
        std::fs::set_permissions(&mask, std::fs::Permissions::from_mode(0o000)).unwrap();
        let profile = root.join("profile");
        let bootstrap: Vec<PathBuf> = ["/usr", "/lib", "/lib64", "/bin", "/etc/ld.so.cache"]
            .into_iter()
            .filter(|path| Path::new(path).exists())
            .map(|path| std::fs::canonicalize(path).unwrap())
            .collect();
        let bootstrap: Vec<_> = bootstrap.iter().map(PathBuf::as_path).collect();
        let outside = root.join("host.sock");
        let public = project.join("public.sock");
        let hidden = private.join("broker.sock");
        let alias = package.join("socket-alias");
        symlink(&outside, &alias).unwrap();
        let listeners: Vec<_> = [&outside, &public, &hidden]
            .into_iter()
            .map(|path| {
                let listener = UnixListener::bind(path).unwrap();
                listener.set_nonblocking(true).unwrap();
                listener
            })
            .collect();
        let paths = [
            (&outside, 0, "outside"),
            (&public, 1, "public"),
            (&hidden, 2, "private-mask"),
            (&alias, 0, "symlink-outside"),
        ];
        let mut rows = Vec::new();
        for (socket, index, label) in paths {
            let direct: Value = serde_json::from_slice(
                &run(Command::new(&target).arg(socket).arg(&data).env_clear()).stdout,
            )
            .unwrap();
            assert!(
                receipt(&listeners[index], &direct),
                "direct socket control: {label}"
            );
            assert_eq!(direct["project_read"], true);
            for (read, write) in [(false, false), (false, true), (true, false), (true, true)] {
                let target_args = [
                    socket.to_str().unwrap().into(),
                    data.to_str().unwrap().into(),
                ];
                let private_paths = [MaskPath::Directory(&private)];
                let policy = LinuxPolicy {
                    helper: &helper,
                    target: &target,
                    profile: &profile,
                    package: &package,
                    scratch: &scratch,
                    location: &project,
                    mask_file: &mask,
                    bootstrap: &bootstrap,
                    private: &private_paths,
                    read_only: &[],
                    target_args: &target_args,
                    fs_read: read,
                    fs_write: write,
                    shell: false,
                    namespace_proc: false,
                };
                let observed = observe(&policy, mode);
                assert_eq!(observed["project_read"], read, "native read control");
                let delivered = receipt(&listeners[index], &observed);
                if candidate || label == "private-mask" {
                    assert!(!delivered, "socket denial: {label}: {observed}");
                }
                rows.push(json!({"socket": label, "fs_read": read, "fs_write": write,
                    "shell": false, "host_received_fixture": delivered, "observed": observed}));
                if candidate {
                    rows.extend(candidate_rows(policy, &listeners[index], label, mode));
                }
            }
        }
        let sources = [
            "crates/cyber-sandbox/examples/plugin_unix_socket_probe.rs",
            "crates/cyber-sandbox/tests/fixtures/plugin_unix_socket.c",
            "crates/cyber-sandbox/tests/fixtures/plugin_socket_cancel.h",
            "crates/cyber-sandbox/tests/fixtures/plugin_socket_mutation.h",
            "crates/cyber-sandbox/tests/fixtures/plugin_process_escape.h",
            "crates/cyber-sandbox/examples/support/plugin_socket_cancel.rs",
            "crates/cyber-sandbox/src/plugin_policy.rs",
            "crates/cyber-sandbox/src/plugin_policy/linux.rs",
            "crates/cyber-sandbox/native/linux_plugin_exec.c",
            "crates/cyber-sandbox/native/linux_filter.h",
            "crates/cyber-sandbox/native/linux_initial_exec.h",
            "crates/cyber-sandbox/src/native_exec.rs",
            "crates/cyber-sandbox/src/bin/cyber-sandbox-exec.rs",
            "crates/cyber-sandbox/build.rs",
            "crates/cyber-sandbox/Cargo.toml",
            "Cargo.lock",
            ".github/workflows/plugin-linux-ipc.yml",
            "scripts/probes/plugin-native-entry/linux_socket_candidate.h",
            "scripts/probes/plugin-native-entry/linux_socket_broker.h",
            "scripts/probes/plugin-native-entry/linux_socket_broker_mutation.h",
            "scripts/probes/plugin-native-entry/linux_socket_landlock9.h",
            "scripts/probes/plugin-native-entry/linux_unix9_policy.h",
            "scripts/probes/plugin-native-entry/build_socket_candidate.py",
        ];
        let mut hash = Sha256::new();
        for path in sources {
            hash.update(path.as_bytes());
            hash.update([0]);
            hash.update(std::fs::read(workspace.join(path)).unwrap());
        }
        let report = json!({"platform": std::env::consts::OS, "architecture": std::env::consts::ARCH,
            "checkout": std::env::var("GITHUB_SHA").ok(), "run_id": std::env::var("GITHUB_RUN_ID").ok(),
            "candidate": candidate, "landlock9": mode == Mode::Unix9, "socket_broker": mode.broker(), "cancellation_probe": mode == Mode::BrokerCancel, "mutation_probe": mode == Mode::BrokerMutation, "escape_probe": mode.escape(), "sources": sources, "source_sha256": format!("{:x}", hash.finalize()),
            "helper_sha256": format!("{:x}", Sha256::digest(std::fs::read(helper).unwrap())),
            "rows": rows, "scope": "Fixture-owned UNIX reachability and candidate private IP exchange only. Experimental syscall broker when selected; no ambient service contact or production capability acceptance."});
        std::fs::write(
            report_path,
            format!("{}\n", serde_json::to_string_pretty(&report).unwrap()),
        )
        .unwrap();
    }
}

#[cfg(unix)]
fn main() {
    probe::main();
}
#[cfg(not(unix))]
fn main() {
    panic!("native evidence requires Linux");
}

//! Exercise the packaged helper, not a separately compiled replacement launcher.
use std::process::Command;

#[test]
fn bypass_or_unsupported_native_mode_never_starts_the_target() {
    let output = Command::new(env!("CARGO_BIN_EXE_cyber-sandbox-exec"))
        .args([
            "--plugin-native-exec",
            "-",
            "/bin/echo",
            "TARGET-MUST-NOT-RUN",
        ])
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(if cfg!(target_os = "macos") { 64 } else { 69 })
    );
    assert!(output.stdout.is_empty());
    assert!(!String::from_utf8_lossy(&output.stderr).contains("TARGET-MUST-NOT-RUN"));
}

#[cfg(target_os = "macos")]
mod macos {
    use super::*;
    use serde_json::{Value, json};
    use sha2::{Digest, Sha256};
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::os::unix::process::ExitStatusExt;
    use std::path::{Path, PathBuf};

    struct Fixture {
        _temp: tempfile::TempDir,
        root: PathBuf,
        helper: PathBuf,
        helper_id: String,
        payload: PathBuf,
    }
    fn successful(command: &mut Command) -> std::process::Output {
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "status={:?} stdout={} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }
    fn sign(path: &Path, identifier: &str) {
        successful(
            Command::new("/usr/bin/codesign")
                .args([
                    "--force",
                    "--sign",
                    "-",
                    "--timestamp=none",
                    "--identifier",
                    identifier,
                ])
                .arg(path),
        );
        successful(
            Command::new("/usr/bin/codesign")
                .args(["--verify", "--strict", "--all-architectures"])
                .arg(path),
        );
    }
    fn compile(root: &Path, source: &str, name: &str) -> PathBuf {
        let input = root.join(format!("{name}.c"));
        std::fs::write(&input, source).unwrap();
        let output = root.join(name);
        successful(
            Command::new("/usr/bin/cc")
                .args([
                    "-Wall",
                    "-Wextra",
                    "-Werror",
                    "-Wno-error=deprecated-declarations",
                ])
                .arg(&input)
                .arg("-o")
                .arg(&output),
        );
        output
    }
    impl Fixture {
        fn new() -> Self {
            let temp = tempfile::Builder::new()
                .permissions(std::fs::Permissions::from_mode(0o700))
                .tempdir()
                .unwrap();
            let root = temp.path().canonicalize().unwrap();
            let helper = root.join("helper");
            std::fs::copy(env!("CARGO_BIN_EXE_cyber-sandbox-exec"), &helper).unwrap();
            let nonce = root.file_name().unwrap().to_str().unwrap();
            let helper_id = format!("cyber.test.helper.{nonce}");
            sign(&helper, &helper_id);
            let payload = compile(
                &root,
                include_str!("../../../scripts/probes/plugin-confinement/fixture_exec.c"),
                "payload",
            );
            sign(&payload, &format!("cyber.test.payload.{nonce}"));
            Self {
                _temp: temp,
                root,
                helper,
                helper_id,
                payload,
            }
        }
        fn profile(&self, name: &str, text: &str) -> PathBuf {
            let path = self.root.join(name);
            std::fs::write(&path, text).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
            path
        }
        fn one_shot(&self, target: &Path) -> PathBuf {
            // Broad reads isolate exec compatibility; this is not fs.read enforcement.
            let base = "(version 1)(deny default)(allow file-read* file-map-executable)(allow sysctl-read)(allow signal (target same-sandbox))(allow process-info* (target same-sandbox))";
            self.profile(
                "one-shot.sb",
                &format!(
                    "{base}(allow process-exec (require-all (signing-identifier {}) (literal {})))",
                    json!(self.helper_id),
                    json!(target)
                ),
            )
        }
        fn command(&self, profile: &Path, target: &Path) -> Command {
            let mut command = Command::new(&self.helper);
            command
                .arg("--plugin-native-exec")
                .arg(profile)
                .arg(target)
                .env_clear()
                .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin");
            command
        }
    }
    fn packet(output: std::process::Output) -> Value {
        assert!(
            output.status.success(),
            "status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }

    #[test]
    fn packaged_entry_starts_native_and_original_system_images_but_denies_later_exec() {
        let fixture = Fixture::new();
        let control = fixture.profile("control.sb", "(version 1)(allow default)");
        let unrestricted = packet(successful(&mut fixture.command(&control, &fixture.payload)));
        assert_eq!(unrestricted, json!({"initial_exec":true,"self_exec":true}));
        let profile = fixture.one_shot(&fixture.payload);
        let confined = packet(successful(&mut fixture.command(&profile, &fixture.payload)));
        assert_eq!(confined, json!({"initial_exec":true,"self_exec":false}));

        let bash = Path::new("/bin/bash");
        let original_hash = Sha256::digest(std::fs::read(bash).unwrap());
        let args = [
            "-c",
            "printf 'initial-admitted\\n'; exec /bin/bash -c 'printf self-exec'",
        ];
        let original_control = successful(fixture.command(&control, bash).args(args));
        assert_eq!(original_control.stdout, b"initial-admitted\nself-exec");
        let denied = fixture
            .command(&fixture.one_shot(bash), bash)
            .args(args)
            .output()
            .unwrap();
        assert_eq!(
            denied.status.signal(),
            None,
            "signal termination cannot prove policy denial"
        );
        assert!(!denied.status.success());
        assert_eq!(denied.stdout, b"initial-admitted\n");
        let error = String::from_utf8_lossy(&denied.stderr);
        assert!(
            error.contains("Operation not permitted") || error.contains("Permission denied"),
            "{error}"
        );
        assert_eq!(Sha256::digest(std::fs::read(bash).unwrap()), original_hash);

        // Admission MUST independently reject this collision. The entry alone is
        // not an admitted launcher: matching subject identities are an observed bypass.
        sign(&fixture.payload, &fixture.helper_id);
        let collision = packet(successful(
            &mut fixture.command(&fixture.one_shot(&fixture.payload), &fixture.payload),
        ));
        assert_eq!(collision, json!({"initial_exec":true,"self_exec":true}));
    }

    #[test]
    fn constructor_exec_and_spawn_enter_after_the_packaged_policy() {
        let fixture = Fixture::new();
        std::fs::write(
            fixture.root.join("process_policy.h"),
            include_str!("../../../scripts/probes/plugin-confinement/process_policy.h"),
        )
        .unwrap();
        let payload = compile(
            &fixture.root,
            include_str!("../../../scripts/probes/plugin-confinement/fixture_native.c"),
            "constructors",
        );
        sign(&payload, "cyber.test.constructor.payload");
        let control = fixture.profile("control.sb", "(version 1)(allow default)");
        for (kind, key) in [("ctor-exec", "self_exec"), ("ctor-spawn", "native_spawn")] {
            let mut args = vec![kind];
            args.extend(std::iter::repeat_n("/", 12));
            let positive = packet(successful(fixture.command(&control, &payload).args(&args)));
            assert_eq!(positive[key], true, "{kind}: {positive}");
            let confined = packet(successful(
                fixture
                    .command(&fixture.one_shot(&payload), &payload)
                    .args(&args),
            ));
            assert_eq!(confined[key], false, "{kind}: {confined}");
        }
    }

    #[test]
    fn packaged_exec_closes_inherited_descriptors_above_a_lowered_soft_limit() {
        let fixture = Fixture::new();
        let descriptors = compile(
            &fixture.root,
            include_str!("fixtures/plugin_native_fds.c"),
            "descriptors",
        );
        let private = fixture.root.join("dummy-private");
        std::fs::write(&private, "dummy fixture only").unwrap();
        let profile = fixture.one_shot(&fixture.payload);
        let direct = packet(successful(
            Command::new(&descriptors)
                .arg("-")
                .arg(&profile)
                .arg(&fixture.payload)
                .arg(&private),
        ));
        assert_eq!(direct["inherited_descriptor"], true);
        let confined = packet(successful(
            Command::new(&descriptors)
                .arg(&fixture.helper)
                .arg(&profile)
                .arg(&fixture.payload)
                .arg(&private),
        ));
        assert_eq!(confined["inherited_descriptor"], false);
    }

    #[test]
    fn invalid_linked_and_oversized_profiles_refuse_before_target_start() {
        let fixture = Fixture::new();
        let mut profiles = vec![
            fixture.profile("empty.sb", ""),
            fixture.profile("nul.sb", "(version 1)(allow default)\0ignored"),
            fixture.profile("large.sb", &"x".repeat(65537)),
            fixture.profile("invalid.sb", "(PRIVATE-PROFILE-PAYLOAD-MUST-NOT-BE-ECHOED"),
        ];
        let linked = fixture.profile("linked.sb", "(version 1)(allow default)");
        std::fs::hard_link(&linked, fixture.root.join("alias")).unwrap();
        profiles.push(linked);
        let indirect = fixture.root.join("symlink.sb");
        symlink(&profiles[0], &indirect).unwrap();
        profiles.push(indirect);
        profiles.push(fixture.root.clone());
        profiles.push(fixture.root.join("missing.sb"));
        for profile in profiles {
            let output = fixture
                .command(&profile, Path::new("/bin/echo"))
                .arg("TARGET-MUST-NOT-RUN")
                .output()
                .unwrap();
            assert_eq!(output.status.code(), Some(2), "{output:?}");
            assert!(output.stdout.is_empty());
            assert!(!String::from_utf8_lossy(&output.stderr).contains("PRIVATE-PROFILE-PAYLOAD"));
        }
    }
    #[test]
    fn a_fifo_profile_is_refused_without_waiting_for_a_writer() {
        let fixture = Fixture::new();
        let fifo = fixture.root.join("policy.fifo");
        successful(Command::new("/usr/bin/mkfifo").arg(&fifo));
        let mut child = fixture
            .command(&fifo, Path::new("/bin/echo"))
            .arg("TARGET-MUST-NOT-RUN")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let completed = loop {
            if child.try_wait().unwrap().is_some() {
                break true;
            }
            if std::time::Instant::now() >= deadline {
                break false;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        };
        if !completed {
            child.kill().unwrap();
        }
        let output = child.wait_with_output().unwrap();
        assert!(completed, "profile open blocked waiting for a FIFO writer");
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
    }
}

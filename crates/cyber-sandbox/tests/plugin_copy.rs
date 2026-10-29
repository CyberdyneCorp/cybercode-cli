//! Actual trusted helper protocol, including source IO outside the SQLite owner.
#[cfg(unix)]
use cyber_core::plugins::image::PreparedCopy;
use cyber_core::plugins::image::Request;
use std::process::Command;

fn helper(request: &Request) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_cyber-sandbox-exec"))
        .arg("--plugin-copy")
        .arg(serde_json::to_string(request).unwrap())
        .env_clear()
        .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
        .output()
        .unwrap()
}

#[test]
fn helper_refuses_invalid_or_oversized_frames_without_echoing_payloads() {
    let mut inputs = vec![r#"{"unexpected":"PRIVATE-PAYLOAD-MUST-NOT-BE-ECHOED"}"#.to_owned()];
    // The Unix protocol frame is larger than Windows' OS command-line limit;
    // Windows tests the small invalid frame and explicit unsupported result.
    if cfg!(unix) {
        inputs.push("x".repeat(cyber_core::plugins::image::MAX_REQUEST + 1));
    }
    for input in inputs {
        let result = Command::new(env!("CARGO_BIN_EXE_cyber-sandbox-exec"))
            .args(["--plugin-copy", &input])
            .output()
            .unwrap();
        assert_eq!(result.status.code(), Some(65));
        assert!(result.stdout.is_empty());
        let stderr = String::from_utf8(result.stderr).unwrap();
        assert!(stderr.contains("invalid package copy request"));
        assert!(!stderr.contains("PRIVATE-PAYLOAD"));
    }
}

#[cfg(unix)]
fn native_fixture(root: &std::path::Path, source: &std::path::Path, system_control: bool) {
    if system_control {
        // Explicit compatibility diagnostic, not a policy-denial assertion.
        let original = Command::new("/bin/sh")
            .args(["-c", "printf 'prepared-image-started'"])
            .output()
            .unwrap();
        assert!(original.status.success());
        assert_eq!(original.stdout, b"prepared-image-started");
        std::fs::copy("/bin/sh", source.join("native")).unwrap();
    } else {
        let fixture = root.join("fixture.c");
        std::fs::write(&fixture,
            b"#include <stdio.h>\nint main(void) { fputs(\"prepared-image-started\", stdout); return 0; }\n").unwrap();
        let compile = Command::new("cc")
            .args(["-x", "c", "-o"])
            .arg(source.join("native"))
            .arg(&fixture)
            .output()
            .unwrap();
        assert!(
            compile.status.success(),
            "{}",
            String::from_utf8_lossy(&compile.stderr)
        );
        #[cfg(target_os = "macos")]
        {
            // Prepare a signed input on Intel as well as arm64; never repair or
            // re-sign the output copy to make a failed preservation check pass.
            let sign = Command::new("/usr/bin/codesign")
                .args([
                    "--force",
                    "--sign",
                    "-",
                    "--identifier",
                    "cyber.test.plugin-copy",
                ])
                .arg(source.join("native"))
                .output()
                .unwrap();
            assert!(
                sign.status.success(),
                "{}",
                String::from_utf8_lossy(&sign.stderr)
            );
        }
    }
}

#[cfg(unix)]
#[test]
fn helper_copies_approved_bytes_and_refuses_changed_content_without_leaking_bytes() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let source = root.join("source");
    std::fs::create_dir(&source).unwrap();
    let private = root.join("private");
    std::fs::create_dir(&private).unwrap();
    std::fs::set_permissions(&private, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::write(source.join("main"), "approved fixture bytes").unwrap();
    let system_control = std::env::var_os("CYBER_PLUGIN_COPY_SYSTEM_IMAGE_CONTROL").is_some();
    native_fixture(&root, &source, system_control);
    let request = Request {
        source,
        destination: private.join("image"),
        expected_digest: cyber_core::plugins::tree_digest(root.join("source").as_path()).unwrap(),
    };
    let output = helper(&request);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: PreparedCopy = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report.digest, request.expected_digest);
    assert_eq!(report.files, 2);
    let executable = report.destination.join("native");
    assert_eq!(
        std::fs::read(&executable).unwrap(),
        std::fs::read(request.source.join("native")).unwrap()
    );
    #[cfg(target_os = "macos")]
    for image in [request.source.join("native"), executable.clone()] {
        let verification = Command::new("/usr/bin/codesign")
            .args(["--verify", "--strict", "--all-architectures"])
            .arg(image)
            .output()
            .unwrap();
        assert!(
            verification.status.success(),
            "{}",
            String::from_utf8_lossy(&verification.stderr)
        );
    }
    let mut command = Command::new(&executable);
    if system_control {
        command.args(["-c", "printf 'prepared-image-started'"]);
    }
    let execution = command.output().unwrap();
    assert!(
        execution.status.success(),
        "status={:?} stderr={}",
        execution.status,
        String::from_utf8_lossy(&execution.stderr)
    );
    assert_eq!(execution.stdout, b"prepared-image-started");
    std::fs::write(
        request.source.join("main"),
        "UNAPPROVED-PAYLOAD-MUST-NOT-APPEAR-IN-ERRORS",
    )
    .unwrap();
    let mut changed = request.clone();
    changed.destination = private.join("changed");
    let output = helper(&changed);
    assert_eq!(output.status.code(), Some(65));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("does not match the approved content digest"));
    assert!(!stderr.contains("UNAPPROVED-PAYLOAD"));
    assert!(!changed.destination.exists());
}

#[cfg(unix)]
#[test]
#[ignore = "subprocess fixture called by worker_source_io_preserves_owner_sqlite_locks"]
fn owner_lock_probe() {
    let path = std::env::var_os("CYBER_PLUGIN_COPY_DATABASE").unwrap();
    let database = rusqlite::Connection::open(path).unwrap();
    database.busy_timeout(std::time::Duration::ZERO).unwrap();
    let error = database
        .execute_batch("BEGIN IMMEDIATE")
        .expect_err("owner lock was released");
    assert_eq!(
        error.sqlite_error_code(),
        Some(rusqlite::ErrorCode::DatabaseBusy)
    );
}

#[cfg(unix)]
#[test]
#[ignore = "subprocess fixture called by worker_source_io_preserves_owner_sqlite_locks"]
fn source_digest_probe() {
    let source = std::env::var_os("CYBER_PLUGIN_COPY_SOURCE").unwrap();
    println!(
        "COPY-DIGEST:{}",
        cyber_core::plugins::tree_digest(std::path::Path::new(&source)).unwrap()
    );
}

#[cfg(unix)]
#[test]
fn worker_source_io_preserves_owner_sqlite_locks() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let source = root.join("source");
    std::fs::create_dir(&source).unwrap();
    let parent = root.join("private");
    std::fs::create_dir(&parent).unwrap();
    std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o700)).unwrap();
    let path = source.join("database");
    let database = rusqlite::Connection::open(&path).unwrap();
    database.execute_batch("PRAGMA journal_mode=DELETE; CREATE TABLE fixture(value); BEGIN EXCLUSIVE; INSERT INTO fixture VALUES(1)").unwrap();
    let check = || {
        let result = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "owner_lock_probe", "--ignored"])
            .env("CYBER_PLUGIN_COPY_DATABASE", &path)
            .output()
            .unwrap();
        assert!(String::from_utf8_lossy(&result.stdout).contains("running 1 test"));
        assert!(
            result.status.success(),
            "{} {}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
    };
    check();
    let result = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "source_digest_probe", "--ignored", "--nocapture"])
        .env("CYBER_PLUGIN_COPY_SOURCE", &source)
        .output()
        .unwrap();
    assert!(result.status.success());
    let stdout = String::from_utf8(result.stdout).unwrap();
    assert!(stdout.contains("running 1 test"));
    let digest = stdout
        .lines()
        .find_map(|line| line.strip_prefix("COPY-DIGEST:"))
        .unwrap()
        .to_owned();
    let request = Request {
        source,
        destination: parent.join("image"),
        expected_digest: digest,
    };
    let report: PreparedCopy = if std::env::var_os("CYBER_PLUGIN_COPY_IN_OWNER_CONTROL").is_some() {
        // Explicit negative control: using the same source reader in this
        // process must make the native lock probe fail after its DB FD closes.
        cyber_core::plugins::image::prepare(&request).unwrap()
    } else {
        let result = helper(&request);
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        serde_json::from_slice(&result.stdout).unwrap()
    };
    assert!(report.files >= 1);
    check();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let owned = runtime
        .block_on(cyber_sandbox::plugin_image::prepare(
            std::path::Path::new(env!("CARGO_BIN_EXE_cyber-sandbox-exec")),
            &parent,
            &request.source,
            &request.expected_digest,
        ))
        .unwrap();
    check();
    drop(owned);
    check();
}

#[cfg(windows)]
#[test]
fn windows_refuses_copy_without_creating_output() {
    let root = tempfile::tempdir().unwrap();
    let request = Request {
        source: root.path().join("source"),
        destination: root.path().join("image"),
        expected_digest: format!("sha256:{}", "0".repeat(64)),
    };
    let result = helper(&request);
    assert_eq!(result.status.code(), Some(65));
    assert!(String::from_utf8_lossy(&result.stderr).contains("unavailable on this platform"));
    assert!(result.stdout.is_empty());
    assert!(!request.destination.exists());
}

#[cfg(unix)]
#[tokio::test]
async fn owned_copy_pins_the_actual_worker_result_and_cleans_it_on_drop() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let source = root.join("source");
    let base = root.join("staging");
    std::fs::create_dir(&source).unwrap();
    std::fs::create_dir(&base).unwrap();
    std::fs::write(source.join("main"), b"approved bytes").unwrap();
    let digest = cyber_core::plugins::tree_digest(&source).unwrap();
    let package = cyber_sandbox::plugin_image::prepare(
        std::path::Path::new(env!("CARGO_BIN_EXE_cyber-sandbox-exec")),
        &base,
        &source,
        &digest,
    )
    .await
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        std::fs::metadata(package.path().parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert_eq!(package.digest(), digest);
    assert_eq!(
        std::fs::read(package.path().join("main")).unwrap(),
        b"approved bytes"
    );
    let path = package.path().to_owned();
    drop(package);
    assert!(!path.exists());
    assert_eq!(std::fs::read_dir(&base).unwrap().count(), 0);
}

#[cfg(windows)]
#[tokio::test]
async fn owner_copy_refuses_windows_without_creating_a_staging_namespace() {
    let root = tempfile::tempdir().unwrap();
    let base = root.path().join("staging");
    let error = cyber_sandbox::plugin_image::prepare(
        &root.path().join("helper.exe"),
        &base,
        &root.path().join("source"),
        "invalid",
    )
    .await
    .unwrap_err();
    assert_eq!(error, cyber_sandbox::plugin_image::Error::Unavailable);
    assert!(!base.exists());
}

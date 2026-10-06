//! Native AppContainer launch proof, independent of built-in tool dispatch.
#![cfg(windows)]
#![allow(unsafe_code)]

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::os::windows::io::{AsHandle, AsRawHandle};
use std::path::{Path, PathBuf};
use std::time::Duration;

use cyber_sandbox::windows_container::{Access, AclGrant, Profile};
use cyber_sandbox::windows_launch::spawn;

fn environment(root: &Path, role: &str, profile: &Profile) -> BTreeMap<String, String> {
    BTreeMap::from([
        ("SystemRoot".into(), std::env::var("SystemRoot").unwrap()),
        (
            "LOCALAPPDATA".into(),
            std::env::var("LOCALAPPDATA").unwrap(),
        ),
        ("CYBER_CONTAINER_ROOT".into(), root.to_str().unwrap().into()),
        (
            "CYBER_CONTAINER_STORAGE".into(),
            profile
                .storage_path()
                .unwrap()
                .canonicalize()
                .unwrap()
                .to_str()
                .unwrap()
                .into(),
        ),
        ("CYBER_CONTAINER_ROLE".into(), role.into()),
    ])
}

fn setup(root: &Path, profile: &Profile) -> (PathBuf, Vec<AclGrant>) {
    let program = root.join("container worker with spaces.exe");
    std::fs::copy(std::env::current_exe().unwrap(), &program).unwrap();
    for name in ["read.txt", "write.txt", "forbidden.txt", "started.txt"] {
        std::fs::write(root.join(name), "original").unwrap();
    }
    let grants = vec![
        profile.grant(root, Access::Read).unwrap(),
        profile.grant(&program, Access::Read).unwrap(),
        profile.grant(&root.join("read.txt"), Access::Read).unwrap(),
        profile
            .grant(&root.join("write.txt"), Access::Write)
            .unwrap(),
        profile
            .grant(&root.join("started.txt"), Access::Write)
            .unwrap(),
    ];
    (program, grants)
}

fn arguments() -> Vec<OsString> {
    [
        "--exact",
        "container_worker",
        "--nocapture",
        "--",
        "two words",
        "a\"b",
        "trailing\\",
        "",
        "λ",
    ]
    .map(OsString::from)
    .to_vec()
}

#[test]
fn dropping_a_live_container_owner_terminates_the_original_process() {
    use windows_sys::Win32::Foundation::WAIT_OBJECT_0;
    use windows_sys::Win32::System::Threading::WaitForSingleObject;

    let directory = tempfile::tempdir().unwrap();
    let mut profile = Profile::new().unwrap();
    let (program, grants) = setup(directory.path(), &profile);
    let env = environment(directory.path(), "wait", &profile);
    let child = spawn(&profile, &program, &arguments(), &env, directory.path()).unwrap();
    let retained = child.as_handle().try_clone_to_owned().unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while std::fs::read_to_string(directory.path().join("started.txt")).unwrap() != "running" {
        assert!(
            std::time::Instant::now() < deadline,
            "container worker never started"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    drop(child);
    assert_eq!(
        unsafe { WaitForSingleObject(retained.as_raw_handle(), 1000) },
        WAIT_OBJECT_0
    );
    for grant in grants {
        grant.close().unwrap();
    }
    profile.close().unwrap();
}

#[test]
fn verified_container_allows_scoped_files_and_denies_other_files_and_loopback() {
    let directory = tempfile::tempdir().unwrap();
    let mut profile = Profile::new().unwrap();
    let (program, grants) = setup(directory.path(), &profile);
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let _control = std::net::TcpStream::connect(address).unwrap();
    let mut env = environment(directory.path(), "probe", &profile);
    env.insert("CYBER_CONTAINER_ADDRESS".into(), address.to_string());
    let mut child = spawn(&profile, &program, &arguments(), &env, directory.path()).unwrap();
    let scratch = child.temporary_directory().to_path_buf();
    assert_eq!(
        child.wait(Duration::from_secs(10)).unwrap(),
        0,
        "worker stage failed; diagnostic: {}",
        std::fs::read_to_string(directory.path().join("write.txt")).unwrap()
    );
    assert_eq!(
        std::fs::read_to_string(directory.path().join("write.txt")).unwrap(),
        "allowed"
    );
    assert_eq!(
        std::fs::read_to_string(directory.path().join("read.txt")).unwrap(),
        "original"
    );
    assert_eq!(
        std::fs::read_to_string(directory.path().join("forbidden.txt")).unwrap(),
        "original"
    );
    assert_eq!(
        std::fs::read_to_string(scratch.join("roundtrip.txt")).unwrap(),
        "private"
    );
    drop(child);
    assert!(!scratch.exists());
    for grant in grants {
        grant.close().unwrap();
    }
    profile.close().unwrap();
}

#[test]
fn invalid_launch_inputs_fail_before_starting_a_process() {
    let directory = tempfile::tempdir().unwrap();
    let mut profile = Profile::new().unwrap();
    let env = environment(directory.path(), "probe", &profile);
    assert!(
        spawn(
            &profile,
            Path::new("relative.exe"),
            &[],
            &env,
            directory.path()
        )
        .is_err()
    );
    let mut bad = env.clone();
    bad.insert("Systemroot".into(), "duplicate".into());
    assert!(
        spawn(
            &profile,
            &std::env::current_exe().unwrap(),
            &[],
            &bad,
            directory.path()
        )
        .is_err()
    );
    let mut missing = env.clone();
    missing.remove("LOCALAPPDATA");
    let missing_error = spawn(
        &profile,
        &std::env::current_exe().unwrap(),
        &[],
        &missing,
        directory.path(),
    )
    .err()
    .unwrap();
    assert_eq!(missing_error.kind(), std::io::ErrorKind::InvalidInput);
    profile.close().unwrap();
    assert!(
        spawn(
            &profile,
            &std::env::current_exe().unwrap(),
            &[],
            &env,
            directory.path()
        )
        .is_err()
    );
}

#[test]
fn container_worker() {
    let Ok(role) = std::env::var("CYBER_CONTAINER_ROLE") else {
        return;
    };
    let root = PathBuf::from(std::env::var_os("CYBER_CONTAINER_ROOT").unwrap());
    if std::env::args_os().skip(1).collect::<Vec<_>>() != arguments() {
        std::process::exit(47);
    }
    if role == "wait" {
        if std::fs::write(root.join("started.txt"), "running").is_err() {
            std::process::exit(48);
        }
        std::thread::sleep(Duration::from_secs(30));
        std::process::exit(49);
    }
    let code = if role == "probe" { probe(&root) } else { 60 };
    std::process::exit(code);
}

fn probe(root: &Path) -> i32 {
    if std::fs::read_to_string(root.join("read.txt"))
        .ok()
        .as_deref()
        != Some("original")
    {
        return 40;
    }
    if std::fs::write(root.join("write.txt"), "allowed").is_err() {
        return 41;
    }
    if std::fs::write(root.join("read.txt"), "forbidden").is_ok() {
        return 42;
    }
    if std::fs::read(root.join("forbidden.txt")).is_ok() {
        return 43;
    }
    if std::fs::write(root.join("forbidden.txt"), "forbidden").is_ok() {
        return 44;
    }
    let extras: Vec<String> = std::env::vars()
        .map(|(name, _)| name)
        .filter(|name| {
            ![
                "SYSTEMROOT",
                "LOCALAPPDATA",
                "CYBER_CONTAINER_ROOT",
                "CYBER_CONTAINER_STORAGE",
                "TEMP",
                "TMP",
                "CYBER_CONTAINER_ROLE",
                "CYBER_CONTAINER_ADDRESS",
            ]
            .contains(&name.to_uppercase().as_str())
        })
        .collect();
    if !extras.is_empty() {
        std::fs::write(
            root.join("write.txt"),
            serde_json::to_string(&extras).unwrap(),
        )
        .unwrap();
        return 45;
    }
    if let Err(diagnostic) = private_temp() {
        std::fs::write(root.join("write.txt"), diagnostic.to_string()).unwrap();
        return 50;
    }
    let temp_file = std::env::temp_dir().join("roundtrip.txt");
    if std::fs::write(&temp_file, "private").is_err()
        || std::fs::read_to_string(&temp_file).ok().as_deref() != Some("private")
    {
        return 51;
    }
    let address = std::env::var("CYBER_CONTAINER_ADDRESS")
        .unwrap()
        .parse()
        .unwrap();
    if std::net::TcpStream::connect_timeout(&address, Duration::from_secs(1)).is_ok() {
        return 46;
    }
    0
}

fn private_temp() -> Result<(), serde_json::Value> {
    let storage = PathBuf::from(std::env::var_os("CYBER_CONTAINER_STORAGE").unwrap());
    // The owner canonicalized this SID-specific directory. The child need not
    // gain read access to the outer directory to verify its own temp path.
    let resolved = storage.clone();
    for name in ["TEMP", "TMP"] {
        let path = PathBuf::from(
            std::env::var_os(name)
                .ok_or_else(|| serde_json::json!({"stage": name, "missing": true}))?,
        );
        let relative = path
            .strip_prefix(&storage)
            .ok()
            .map(|value| value.to_string_lossy().into_owned());
        let path = std::fs::canonicalize(path).map_err(|error| {
            serde_json::json!({
                "stage": name, "relative": relative, "code": error.raw_os_error()
            })
        })?;
        if !path.starts_with(&resolved) {
            return Err(serde_json::json!({"stage": name, "outside_profile": true}));
        }
    }
    Ok(())
}

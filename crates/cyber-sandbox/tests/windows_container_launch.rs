//! Native AppContainer launch proof, independent of built-in tool dispatch.
#![cfg(windows)]
#![allow(unsafe_code)]

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::os::windows::io::{AsHandle, AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::{Path, PathBuf};
use std::time::Duration;

use cyber_sandbox::windows_container::{Access, AclGrant, Profile};
use cyber_sandbox::windows_launch::{StandardStreams, spawn, spawn_with_stdio};

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
fn container_owner_and_normal_exit_terminate_live_descendants() {
    for normal_exit in [false, true] {
        container_tree_case(normal_exit);
    }
}

fn container_tree_case(normal_exit: bool) {
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE};
    let directory = tempfile::tempdir().unwrap();
    let mut profile = Profile::new().unwrap();
    let (program, grants) = setup(directory.path(), &profile);
    let env = environment(directory.path(), "tree", &profile);
    let mut child = spawn(&profile, &program, &arguments(), &env, directory.path()).unwrap();
    let primary = child.as_handle().try_clone_to_owned().unwrap();
    let pid = live_container_descendant(directory.path());
    let raw = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
    assert!(!raw.is_null());
    let descendant = unsafe { OwnedHandle::from_raw_handle(raw) };
    assert_process_live(&descendant);
    if normal_exit {
        std::fs::write(directory.path().join("read.txt"), "finish").unwrap();
        assert_eq!(child.wait(Duration::from_secs(10)).unwrap(), 0);
    }
    drop(child);
    assert_process_terminated(&primary);
    assert_process_terminated(&descendant);
    for grant in grants {
        grant.close().unwrap();
    }
    profile.close().unwrap();
}

fn live_container_descendant(root: &Path) -> u32 {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let ready = std::fs::read_to_string(root.join("started.txt")).unwrap() == "running";
        let pid = std::fs::read_to_string(root.join("write.txt"))
            .unwrap()
            .parse::<u32>();
        if ready && let Ok(pid) = pid {
            return pid;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "descendant never started"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn assert_process_live(process: &OwnedHandle) {
    use windows_sys::Win32::Foundation::WAIT_TIMEOUT;
    use windows_sys::Win32::System::Threading::WaitForSingleObject;
    assert_eq!(
        unsafe { WaitForSingleObject(process.as_raw_handle(), 0) },
        WAIT_TIMEOUT
    );
}

fn assert_process_terminated(process: &OwnedHandle) {
    use windows_sys::Win32::Foundation::WAIT_OBJECT_0;
    use windows_sys::Win32::System::Threading::WaitForSingleObject;
    assert_eq!(
        unsafe { WaitForSingleObject(process.as_raw_handle(), 1000) },
        WAIT_OBJECT_0
    );
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
    // Caller-provided temporary paths cannot escape invocation-owned storage.
    env.insert("temp".into(), directory.path().to_str().unwrap().into());
    env.insert("Tmp".into(), directory.path().to_str().unwrap().into());
    let mut child = spawn(&profile, &program, &arguments(), &env, directory.path()).unwrap();
    let scratch = child.temporary_directory().to_path_buf();
    assert_eq!(
        child.wait(Duration::from_secs(10)).unwrap(),
        0,
        "worker stage failed; owned scratch: {:?}; diagnostic: {}",
        scratch.file_name().unwrap(),
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
fn redirected_streams_preserve_bytes_and_exclude_unlisted_inheritable_handles() {
    use windows_sys::Win32::Foundation::WAIT_TIMEOUT;
    use windows_sys::Win32::System::Threading::WaitForSingleObject;

    let directory = tempfile::tempdir().unwrap();
    let mut profile = Profile::new().unwrap();
    let (program, grants) = setup(directory.path(), &profile);
    let input = directory.path().join("stdin.txt");
    std::fs::write(&input, "input λ\n").unwrap();
    let stdin = std::fs::File::open(input).unwrap();
    let stdout = std::fs::File::create(directory.path().join("stdout.txt")).unwrap();
    let stderr = std::fs::File::create(directory.path().join("stderr.txt")).unwrap();
    let sentinel = inheritable_event();
    let mut env = environment(directory.path(), "streams", &profile);
    env.insert(
        "CYBER_UNLISTED_EVENT".into(),
        (sentinel.as_raw_handle() as usize).to_string(),
    );
    let mut child = spawn_with_stdio(
        &profile,
        &program,
        &arguments(),
        &env,
        directory.path(),
        StandardStreams {
            stdin: stdin.as_handle(),
            stdout: stdout.as_handle(),
            stderr: stderr.as_handle(),
        },
    )
    .unwrap();
    assert_eq!(child.wait(Duration::from_secs(10)).unwrap(), 0);
    assert!(
        std::fs::read_to_string(directory.path().join("stdout.txt"))
            .unwrap()
            .contains("stdout: input λ\n")
    );
    assert_eq!(
        std::fs::read_to_string(directory.path().join("stderr.txt")).unwrap(),
        "stderr: 日本語\n"
    );
    assert_eq!(
        unsafe { WaitForSingleObject(sentinel.as_raw_handle(), 0) },
        WAIT_TIMEOUT
    );
    drop(child);
    for grant in grants {
        grant.close().unwrap();
    }
    profile.close().unwrap();
}

fn inheritable_event() -> OwnedHandle {
    use windows_sys::Win32::Foundation::WAIT_OBJECT_0;
    use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
    use windows_sys::Win32::System::Threading::{
        CreateEventW, ResetEvent, SetEvent, WaitForSingleObject,
    };
    let security = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        bInheritHandle: 1,
        ..Default::default()
    };
    let raw = unsafe { CreateEventW(&security, 1, 0, std::ptr::null()) };
    assert!(!raw.is_null());
    let event = unsafe { OwnedHandle::from_raw_handle(raw) };
    // Positive control: an inherited copy can signal this exact kernel object.
    assert_ne!(unsafe { SetEvent(raw) }, 0);
    assert_eq!(unsafe { WaitForSingleObject(raw, 0) }, WAIT_OBJECT_0);
    assert_ne!(unsafe { ResetEvent(raw) }, 0);
    event
}

#[test]
fn failed_process_creation_releases_private_storage_and_profile_owners() {
    let directory = tempfile::tempdir().unwrap();
    let mut profile = Profile::new().unwrap();
    let storage = profile.storage_path().unwrap();
    let before = storage_entries(&storage);
    let env = environment(directory.path(), "probe", &profile);
    let failure = spawn(
        &profile,
        &directory.path().join("missing.exe"),
        &arguments(),
        &env,
        directory.path(),
    )
    .err()
    .expect("missing executable must fail before running user code");
    assert_eq!(failure.kind(), std::io::ErrorKind::NotFound);
    assert_eq!(storage_entries(&storage), before);
    // No failed-launch guard may retain the profile or its ACL lease.
    profile.close().unwrap();
}

fn storage_entries(storage: &Path) -> std::collections::BTreeSet<OsString> {
    std::fs::read_dir(storage)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect()
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
    if role == "streams" {
        std::process::exit(if stream_worker().is_ok() { 0 } else { 52 });
    }
    if role == "tree" {
        std::process::exit(if tree_worker(&root).is_ok() { 0 } else { 53 });
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

fn tree_worker(root: &Path) -> std::io::Result<()> {
    let _descendant = std::process::Command::new(std::env::current_exe()?)
        .args(arguments())
        .env("CYBER_CONTAINER_ROLE", "wait")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    std::fs::write(root.join("write.txt"), _descendant.id().to_string())?;
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while std::fs::read_to_string(root.join("read.txt"))? != "finish" {
        if std::time::Instant::now() >= deadline {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "tree control timed out",
            ));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    // Deliberately leave the live descendant for the command owner's job to kill.
    Ok(())
}

fn stream_worker() -> std::io::Result<()> {
    use std::io::{Read, Write};
    use windows_sys::Win32::System::Threading::SetEvent;
    let unlisted = std::env::var("CYBER_UNLISTED_EVENT")
        .unwrap()
        .parse::<usize>()
        .unwrap();
    if unsafe { SetEvent(unlisted as *mut core::ffi::c_void) } != 0 {
        return Err(std::io::Error::other("Unlisted handle was inherited"));
    }
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input)?;
    if input != "input λ\n" {
        return Err(std::io::Error::other("Redirected input changed"));
    }
    std::io::stdout().write_all(format!("stdout: {input}").as_bytes())?;
    std::io::stdout().flush()?;
    std::io::stderr().write_all("stderr: 日本語\n".as_bytes())?;
    std::io::stderr().flush()
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
        let leaf = path
            .file_name()
            .map(|value| value.to_string_lossy().into_owned());
        let normalized_storage = storage
            .to_string_lossy()
            .trim_start_matches("\\\\?\\")
            .to_lowercase();
        let normalized_path = path
            .to_string_lossy()
            .trim_start_matches("\\\\?\\")
            .to_lowercase();
        let under_profile = normalized_path.starts_with(&format!("{normalized_storage}\\"));
        let path = std::fs::canonicalize(path).map_err(|error| {
            serde_json::json!({
                "stage": name, "relative": relative, "leaf": leaf,
                "under_profile": under_profile, "code": error.raw_os_error()
            })
        })?;
        if !path.starts_with(&resolved) {
            return Err(serde_json::json!({"stage": name, "outside_profile": true}));
        }
    }
    Ok(())
}

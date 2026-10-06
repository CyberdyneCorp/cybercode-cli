//! Native AppContainer launch proof, independent of built-in tool dispatch.
#![cfg(windows)]
#![allow(unsafe_code)]

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::os::windows::io::{AsHandle, AsRawHandle, BorrowedHandle, FromRawHandle, OwnedHandle};
use std::path::{Path, PathBuf};
use std::time::Duration;

use cyber_sandbox::windows_container::{
    Access, AclGrant, ExistingTreePolicy, ExistingTreeRoot, Profile,
};
use cyber_sandbox::windows_launch::{StandardStreams, spawn, spawn_with_stdio};

#[cfg(feature = "windows-test-controls")]
#[path = "windows_container_launch/package_allowance.rs"]
mod package_allowance;

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
    assert_parent_security_access(root);
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

fn assert_parent_security_access(root: &Path) {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{WRITE_DAC, WRITE_OWNER};
    for name in ["read.txt", "write.txt"] {
        for right in [WRITE_DAC, WRITE_OWNER] {
            std::fs::OpenOptions::new()
                .access_mode(right)
                .open(root.join(name))
                .unwrap();
        }
    }
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
    let conflicting = spawn(&profile, &program, &arguments(), &env, directory.path())
        .err()
        .expect("one profile must not share temporary storage between live owners");
    assert_eq!(conflicting.kind(), std::io::ErrorKind::WouldBlock);
    assert_process_live(&retained);
    assert_profile_grants_sealed(&profile, directory.path());
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

#[tokio::test]
async fn existing_tree_exclusions_enforce_readonly_and_hidden_descendants() {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{WRITE_DAC, WRITE_OWNER};
    let directory = tempfile::tempdir().unwrap();
    let mut profile = Profile::new().unwrap();
    let (program, grants) = setup(directory.path(), &profile);
    let scope = directory.path().join("scope");
    for name in ["writable", "protected", "secret"] {
        std::fs::create_dir_all(scope.join(name)).unwrap();
        std::fs::write(scope.join(name).join("leaf.txt"), "original").unwrap();
    }
    for name in ["writable", "protected", "secret"] {
        for right in [WRITE_DAC, WRITE_OWNER] {
            std::fs::OpenOptions::new()
                .access_mode(right)
                .open(scope.join(name).join("leaf.txt"))
                .unwrap();
        }
    }
    std::fs::write(scope.join("writable/delete.txt"), "delete control").unwrap();
    std::fs::write(scope.join("protected/delete.txt"), "protected control").unwrap();
    let policy = ExistingTreePolicy {
        access: Access::Read,
        read_only: vec![scope.join("protected"), scope.join("secret")],
        unreadable: vec![scope.join("secret")],
    };
    let roots = [
        ExistingTreeRoot {
            path: scope.clone(),
            policy,
        },
        ExistingTreeRoot {
            path: scope.join("writable"),
            policy: ExistingTreePolicy {
                access: Access::Write,
                read_only: vec![],
                unreadable: vec![],
            },
        },
    ];
    let mut tree = profile.grant_existing_forest(&roots, 9).unwrap();
    let env = environment(directory.path(), "existing-policy", &profile);
    let child = spawn(&profile, &program, &arguments(), &env, directory.path()).unwrap();
    let code = tokio::time::timeout(Duration::from_secs(10), child.wait_owned())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        code,
        0,
        "{}",
        std::fs::read_to_string(directory.path().join("write.txt")).unwrap()
    );
    assert_eq!(
        std::fs::read_to_string(scope.join("writable/leaf.txt")).unwrap(),
        "policy allowed"
    );
    assert_eq!(
        std::fs::read_to_string(scope.join("protected/leaf.txt")).unwrap(),
        "original"
    );
    assert_eq!(
        std::fs::read_to_string(scope.join("secret/leaf.txt")).unwrap(),
        "original"
    );
    assert!(!scope.join("protected/new.txt").exists());
    assert!(!scope.join("writable/delete.txt").exists());
    assert_profile_grants_sealed(&profile, directory.path());
    tree.close().unwrap();
    for grant in grants {
        grant.close().unwrap();
    }
    profile.close().unwrap();
}

#[tokio::test]
async fn existing_tree_grants_allow_nested_files_and_deny_outside_writes() {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{WRITE_DAC, WRITE_OWNER};
    let directory = tempfile::tempdir().unwrap();
    let mut profile = Profile::new().unwrap();
    let (program, grants) = setup(directory.path(), &profile);
    let scope = directory.path().join("scope");
    std::fs::create_dir_all(scope.join("nested")).unwrap();
    let leaf = scope.join("nested/leaf.txt");
    std::fs::write(&leaf, "original").unwrap();
    for right in [WRITE_DAC, WRITE_OWNER] {
        std::fs::OpenOptions::new()
            .access_mode(right)
            .open(&leaf)
            .unwrap();
    }
    let mut tree = profile
        .grant_existing_tree(&scope, Access::Write, 3)
        .unwrap();
    let env = environment(directory.path(), "existing-tree", &profile);
    let child = spawn(&profile, &program, &arguments(), &env, directory.path()).unwrap();
    let code = tokio::time::timeout(Duration::from_secs(10), child.wait_owned())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        code,
        0,
        "{}",
        std::fs::read_to_string(directory.path().join("write.txt")).unwrap()
    );
    assert_eq!(std::fs::read_to_string(&leaf).unwrap(), "tree allowed");
    assert_eq!(
        std::fs::read_to_string(directory.path().join("forbidden.txt")).unwrap(),
        "original"
    );
    assert_profile_grants_sealed(&profile, directory.path());
    tree.close().unwrap();
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

#[tokio::test]
async fn discarding_and_aborting_owned_waits_terminate_live_containers() {
    for abort_task in [false, true] {
        owned_wait_disposal_case(abort_task).await;
    }
}

async fn owned_wait_disposal_case(abort_task: bool) {
    let directory = tempfile::tempdir().unwrap();
    let mut profile = Profile::new().unwrap();
    let (program, grants) = setup(directory.path(), &profile);
    let env = environment(directory.path(), "wait", &profile);
    let child = spawn(&profile, &program, &arguments(), &env, directory.path()).unwrap();
    let retained = child.as_handle().try_clone_to_owned().unwrap();
    let scratch = child.temporary_directory().to_path_buf();
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while std::fs::read_to_string(directory.path().join("started.txt")).unwrap() != "running" {
        assert!(std::time::Instant::now() < deadline, "worker never started");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_process_live(&retained);
    let waiting = child.wait_owned();
    if abort_task {
        let (entered, ready) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            entered.send(()).unwrap();
            waiting.await
        });
        ready.await.unwrap();
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
    } else {
        drop(waiting);
    }
    assert_process_terminated(&retained);
    assert!(!scratch.exists());
    assert_profile_reuse_refused(&profile, &program, &env, directory.path());
    for grant in grants {
        grant.close().unwrap();
    }
    profile.close().unwrap();
}

#[tokio::test]
async fn owned_wait_preserves_exit_code_and_releases_storage() {
    let directory = tempfile::tempdir().unwrap();
    let mut profile = Profile::new().unwrap();
    let (program, grants) = setup(directory.path(), &profile);
    let env = environment(directory.path(), "exit", &profile);
    let child = spawn(&profile, &program, &arguments(), &env, directory.path()).unwrap();
    let scratch = child.temporary_directory().to_path_buf();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(10), child.wait_owned())
            .await
            .unwrap()
            .unwrap(),
        73
    );
    assert!(!scratch.exists());
    assert_profile_reuse_refused(&profile, &program, &env, directory.path());
    for grant in grants {
        grant.close().unwrap();
    }
    profile.close().unwrap();
}

fn assert_profile_reuse_refused(
    profile: &Profile,
    program: &Path,
    env: &BTreeMap<String, String>,
    root: &Path,
) {
    let retry = spawn(profile, program, &arguments(), env, root)
        .err()
        .expect("completed or cancelled invocation must not reuse earlier identity grants");
    assert_eq!(retry.kind(), std::io::ErrorKind::InvalidInput);
    assert!(!profile.storage_path().unwrap().join("Temp").exists());
    assert_profile_grants_sealed(profile, root);
}

fn assert_profile_grants_sealed(profile: &Profile, root: &Path) {
    let late = root.join("late-grant.txt");
    std::fs::write(&late, "unchanged").unwrap();
    let grant = profile
        .grant(&late, Access::Write)
        .err()
        .expect("executed profiles must not widen their grant scope");
    assert_eq!(grant.kind(), std::io::ErrorKind::InvalidInput);
    let grant = profile
        .grant_relocatable(&late, Access::Write)
        .err()
        .expect("executed profiles must also refuse identity-recorded grants");
    assert_eq!(grant.kind(), std::io::ErrorKind::InvalidInput);
    let grant = profile
        .grant_existing_tree(root, Access::Write, 0)
        .err()
        .expect("executed profiles must refuse tree grants before inventory");
    assert_eq!(grant.kind(), std::io::ErrorKind::InvalidInput);
    assert!(grant.to_string().contains("single-use"));
    let grant = profile
        .grant_existing_forest(&[], 0)
        .err()
        .expect("executed profiles must refuse forest preparation before inventory");
    assert_eq!(grant.kind(), std::io::ErrorKind::InvalidInput);
    assert!(grant.to_string().contains("single-use"));
    let policy = ExistingTreePolicy {
        access: Access::Write,
        read_only: vec![],
        unreadable: vec![],
    };
    let grant = profile
        .grant_existing_tree_policy(root, &policy, 0)
        .err()
        .expect("executed profiles must refuse policy preparation before inventory");
    assert_eq!(grant.kind(), std::io::ErrorKind::InvalidInput);
    assert!(grant.to_string().contains("single-use"));
}

fn container_tree_case(normal_exit: bool) {
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE};
    let directory = tempfile::tempdir().unwrap();
    let mut profile = Profile::new().unwrap();
    let (program, grants) = setup(directory.path(), &profile);
    let env = environment(directory.path(), "tree", &profile);
    let stdin = std::fs::File::open(directory.path().join("read.txt")).unwrap();
    let stdout = std::fs::File::create(directory.path().join("tree.stdout")).unwrap();
    let stderr = std::fs::File::create(directory.path().join("tree.stderr")).unwrap();
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
            "descendant never started; worker diagnostic: {}",
            std::fs::read_to_string(root.join("write.txt")).unwrap()
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
    let env = environment(directory.path(), "streams", &profile);
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
    verify_stream_inheritance(
        directory.path(),
        child.as_handle(),
        stdin.as_handle(),
        &sentinel,
    );
    std::fs::write(directory.path().join("read.txt"), "streams-go").unwrap();
    assert_eq!(
        child.wait(Duration::from_secs(10)).unwrap(),
        0,
        "stream worker diagnostic: {}",
        std::fs::read_to_string(directory.path().join("write.txt")).unwrap()
    );
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

fn verify_stream_inheritance(
    root: &Path,
    child: BorrowedHandle<'_>,
    stdin: BorrowedHandle<'_>,
    sentinel: &OwnedHandle,
) {
    use windows_sys::Win32::Foundation::CompareObjectHandles;
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let input = loop {
        let contents = std::fs::read_to_string(root.join("write.txt")).unwrap();
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&contents)
            && let Some(input) = value["stdin"].as_u64()
        {
            break input as usize;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "stream handshake failed: {contents}"
        );
        std::thread::sleep(Duration::from_millis(10));
    };
    let inherited = duplicate_from_child(child, input).unwrap();
    assert_ne!(
        unsafe { CompareObjectHandles(inherited.as_raw_handle(), stdin.as_raw_handle()) },
        0
    );
    match duplicate_from_child(child, sentinel.as_raw_handle() as usize) {
        Ok(candidate) => assert_eq!(
            unsafe { CompareObjectHandles(candidate.as_raw_handle(), sentinel.as_raw_handle()) },
            0
        ),
        Err(error) => assert_eq!(error.raw_os_error(), Some(6)),
    }
}

fn duplicate_from_child(child: BorrowedHandle<'_>, value: usize) -> std::io::Result<OwnedHandle> {
    use windows_sys::Win32::Foundation::{DUPLICATE_SAME_ACCESS, DuplicateHandle};
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    let mut copied = std::ptr::null_mut();
    if unsafe {
        DuplicateHandle(
            child.as_raw_handle(),
            value as *mut core::ffi::c_void,
            GetCurrentProcess(),
            &mut copied,
            0,
            0,
            DUPLICATE_SAME_ACCESS,
        )
    } == 0
    {
        return Err(std::io::Error::last_os_error());
    }
    Ok(unsafe { OwnedHandle::from_raw_handle(copied) })
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
    let mut before = storage_entries(&storage);
    // The profile-created default temp directory belongs to this invocation too.
    before.remove(&OsString::from("Temp"));
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
fn nonempty_profile_temp_is_preserved_and_failed_reservation_is_released() {
    let directory = tempfile::tempdir().unwrap();
    let mut profile = Profile::new().unwrap();
    let temp = profile.storage_path().unwrap().join("Temp");
    std::fs::create_dir_all(&temp).unwrap();
    let existing = temp.join("existing.txt");
    std::fs::write(&existing, "preserve").unwrap();
    let env = environment(directory.path(), "probe", &profile);
    let program = directory.path().join("missing.exe");
    let refusal = spawn(&profile, &program, &arguments(), &env, directory.path())
        .err()
        .expect("nonempty temp must be refused before cleanup ownership");
    assert_eq!(refusal.kind(), std::io::ErrorKind::Other);
    assert_eq!(std::fs::read_to_string(&existing).unwrap(), "preserve");
    std::fs::remove_file(existing).unwrap();
    let retry = spawn(&profile, &program, &arguments(), &env, directory.path())
        .err()
        .expect("missing executable must still be refused");
    assert_eq!(retry.kind(), std::io::ErrorKind::NotFound);
    assert!(!temp.exists());
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
    if role == "exit" {
        std::process::exit(73);
    }
    if role == "streams" {
        exit_worker(&root, stream_worker(&root), "streams", 52);
    }
    #[cfg(feature = "windows-test-controls")]
    if role == "package-baseline" {
        exit_worker(
            &root,
            package_allowance::baseline_worker(&root),
            "package-baseline",
            57,
        );
    }
    if role == "existing-policy" {
        exit_worker(&root, existing_policy_worker(&root), "existing-policy", 56);
    }
    if role == "existing-tree" {
        exit_worker(&root, existing_tree_worker(&root), "existing-tree", 55);
    }
    if role == "tree" {
        exit_worker(&root, tree_worker(&root), "tree", 53);
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

fn exit_worker(root: &Path, result: std::io::Result<()>, stage: &str, code: i32) -> ! {
    if let Err(error) = result {
        std::fs::write(
            root.join("write.txt"),
            serde_json::json!({
                "stage": stage, "code": error.raw_os_error(), "error": error.to_string()
            })
            .to_string(),
        )
        .unwrap();
        std::process::exit(code);
    }
    std::process::exit(0);
}

fn existing_policy_worker(root: &Path) -> std::io::Result<()> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{WRITE_DAC, WRITE_OWNER};
    let scope = root.join("scope");
    std::fs::write(scope.join("writable/leaf.txt"), "policy allowed")?;
    std::fs::remove_file(scope.join("writable/delete.txt"))?;
    if std::fs::read_to_string(scope.join("protected/leaf.txt"))? != "original" {
        return Err(std::io::Error::other("Read-only content changed"));
    }
    denied_operation(
        "protected file write",
        std::fs::write(scope.join("protected/leaf.txt"), "forbidden"),
    )?;
    denied_operation(
        "protected file delete",
        std::fs::remove_file(scope.join("protected/leaf.txt")),
    )?;
    denied_operation(
        "protected parent-delete control",
        std::fs::remove_file(scope.join("protected/delete.txt")),
    )?;
    denied_operation(
        "protected child creation",
        std::fs::write(scope.join("protected/new.txt"), "forbidden"),
    )?;
    denied_operation(
        "hidden file read",
        std::fs::read(scope.join("secret/leaf.txt")),
    )?;
    denied_operation(
        "hidden file write",
        std::fs::write(scope.join("secret/leaf.txt"), "forbidden"),
    )?;
    for name in ["writable", "protected", "secret"] {
        for right in [WRITE_DAC, WRITE_OWNER] {
            denied_operation(
                &format!("{name} security right {right:#x}"),
                std::fs::OpenOptions::new()
                    .access_mode(right)
                    .open(scope.join(name).join("leaf.txt")),
            )?;
        }
    }
    Ok(())
}

fn denied_operation<T>(operation: &str, result: std::io::Result<T>) -> std::io::Result<()> {
    match result {
        Err(error) if error.raw_os_error() == Some(5) => Ok(()),
        Err(error) => Err(std::io::Error::new(
            error.kind(),
            format!("{operation}: {error}"),
        )),
        Ok(_) => Err(std::io::Error::other(format!(
            "Excluded access was allowed: {operation}"
        ))),
    }
}

fn existing_tree_worker(root: &Path) -> std::io::Result<()> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{WRITE_DAC, WRITE_OWNER};
    let leaf = root.join("scope/nested/leaf.txt");
    std::fs::write(&leaf, "tree allowed")?;
    if std::fs::read_to_string(&leaf)? != "tree allowed" {
        return Err(std::io::Error::other("Allowed tree read changed"));
    }
    if std::fs::write(root.join("forbidden.txt"), "outside").is_ok() {
        return Err(std::io::Error::other("Outside write was allowed"));
    }
    for right in [WRITE_DAC, WRITE_OWNER] {
        match std::fs::OpenOptions::new().access_mode(right).open(&leaf) {
            Err(error) if error.raw_os_error() == Some(5) => {}
            _ => return Err(std::io::Error::other("Tree grant exposed security rights")),
        }
    }
    Ok(())
}

fn tree_worker(root: &Path) -> std::io::Result<()> {
    let executable = std::env::current_exe().map_err(|error| {
        std::io::Error::new(
            error.kind(),
            format!("Resolve descendant executable: {error}"),
        )
    })?;
    let readable = std::fs::File::open(&executable).map_err(|error| {
        std::io::Error::new(error.kind(), format!("Read descendant executable: {error}"))
    })?;
    drop(readable);
    let _descendant = std::process::Command::new(executable)
        .args(arguments())
        .env("CYBER_CONTAINER_ROLE", "wait")
        .spawn()
        .map_err(|error| {
            std::io::Error::new(
                error.kind(),
                format!(
                    "Spawn descendant: {error}; access probes: {}",
                    runtime_access_diagnostics()
                ),
            )
        })?;
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

fn stream_worker(root: &Path) -> std::io::Result<()> {
    use std::io::{Read, Write};
    let stdin = std::io::stdin();
    std::fs::write(
        root.join("write.txt"),
        serde_json::json!({"stdin": stdin.as_raw_handle() as usize}).to_string(),
    )?;
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while std::fs::read_to_string(root.join("read.txt"))? != "streams-go" {
        if std::time::Instant::now() >= deadline {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "stream control timed out",
            ));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    std::fs::write(root.join("write.txt"), "stream: stdin")?;
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input)?;
    if input != "input λ\n" {
        return Err(std::io::Error::other("Redirected input changed"));
    }
    std::fs::write(root.join("write.txt"), "stream: output")?;
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
    if let Err(diagnostic) = security_rights_denied(root) {
        std::fs::write(root.join("write.txt"), diagnostic.to_string()).unwrap();
        return 54;
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
    if let Err(diagnostic) = temp_roundtrip() {
        std::fs::write(root.join("write.txt"), diagnostic.to_string()).unwrap();
        return 51;
    }
    if let Err(error) = winsock_startup() {
        std::fs::write(
            root.join("write.txt"),
            serde_json::json!({
                "stage": "winsock-startup", "code": error.raw_os_error(), "error": error.to_string(),
                "access_probes": runtime_access_diagnostics()
            })
            .to_string(),
        )
        .unwrap();
        return 58;
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

fn winsock_startup() -> std::io::Result<()> {
    use windows_sys::Win32::Networking::WinSock::{
        WSACleanup, WSADATA, WSAGetLastError, WSAStartup,
    };
    // Diagnose initialization before Rust's networking path, which panics on failure.
    let mut data: WSADATA = unsafe { std::mem::zeroed() };
    let result = unsafe { WSAStartup(0x0202, &mut data) };
    if result != 0 {
        return Err(std::io::Error::from_raw_os_error(result));
    }
    if unsafe { WSACleanup() } != 0 {
        return Err(std::io::Error::from_raw_os_error(unsafe {
            WSAGetLastError()
        }));
    }
    Ok(())
}

fn runtime_access_diagnostics() -> serde_json::Value {
    use windows_sys::Win32::Security::{TOKEN_DUPLICATE, TOKEN_QUERY};
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    let mut token = std::ptr::null_mut();
    let token_code = if unsafe {
        OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_QUERY | TOKEN_DUPLICATE,
            &mut token,
        )
    } == 0
    {
        std::io::Error::last_os_error().raw_os_error()
    } else {
        drop(unsafe { OwnedHandle::from_raw_handle(token) });
        Some(0)
    };
    let registry: Vec<_> = [
        r"SYSTEM\CurrentControlSet\Services\WinSock2\Parameters",
        r"SYSTEM\CurrentControlSet\Services\WinSock2\Parameters\Protocol_Catalog9",
        r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\Image File Execution Options",
    ]
    .into_iter()
    .map(|key| serde_json::json!({"key": key, "code": registry_read_probe(key)}))
    .collect();
    serde_json::json!({"own_token_duplicate": token_code, "system_registry": registry})
}

fn registry_read_probe(key: &str) -> u32 {
    use windows_sys::Win32::System::Registry::{
        HKEY_LOCAL_MACHINE, KEY_READ, RegCloseKey, RegOpenKeyExW,
    };
    let key: Vec<u16> = key.encode_utf16().chain(Some(0)).collect();
    let mut handle = std::ptr::null_mut();
    let code = unsafe { RegOpenKeyExW(HKEY_LOCAL_MACHINE, key.as_ptr(), 0, KEY_READ, &mut handle) };
    if code == 0 {
        unsafe { RegCloseKey(handle) };
    }
    code
}

#[test]
fn host_runtime_access_probes_have_positive_controls() {
    winsock_startup().unwrap();
    let diagnostic = runtime_access_diagnostics();
    assert_eq!(diagnostic["own_token_duplicate"], 0);
    for entry in diagnostic["system_registry"]
        .as_array()
        .unwrap()
        .iter()
        .take(2)
    {
        assert_eq!(entry["code"], 0, "Host Winsock key read failed: {entry}");
    }
}

fn security_rights_denied(root: &Path) -> Result<(), serde_json::Value> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{WRITE_DAC, WRITE_OWNER};
    for name in ["read.txt", "write.txt"] {
        for right in [WRITE_DAC, WRITE_OWNER] {
            match std::fs::OpenOptions::new()
                .access_mode(right)
                .open(root.join(name))
            {
                Err(error) if error.raw_os_error() == Some(5) => {}
                Err(error) => {
                    return Err(serde_json::json!({
                        "stage": "security.access", "file": name, "right": right, "code": error.raw_os_error()
                    }));
                }
                Ok(_) => {
                    return Err(serde_json::json!({
                        "stage": "security.access", "file": name, "right": right, "unexpected_allow": true
                    }));
                }
            }
        }
    }
    Ok(())
}

fn temp_roundtrip() -> Result<(), serde_json::Value> {
    let file = std::env::temp_dir().join("roundtrip.txt");
    std::fs::write(&file, "private").map_err(|error| {
        serde_json::json!({
            "stage": "temp.write", "code": error.raw_os_error()
        })
    })?;
    let contents = std::fs::read_to_string(&file).map_err(|error| {
        serde_json::json!({
            "stage": "temp.read", "code": error.raw_os_error()
        })
    })?;
    if contents != "private" {
        return Err(serde_json::json!({"stage": "temp.contents"}));
    }
    Ok(())
}

fn private_temp() -> Result<(), serde_json::Value> {
    let storage = PathBuf::from(std::env::var_os("CYBER_CONTAINER_STORAGE").unwrap());
    // Exact owner-supplied path plus the subsequent two-sided file round trip
    // establishes private temp access without reopening inaccessible ancestors.
    let normalized_storage = normalized_windows_path(&storage);
    let expected = format!("{normalized_storage}\\temp");
    for name in ["TEMP", "TMP"] {
        let path = PathBuf::from(
            std::env::var_os(name)
                .ok_or_else(|| serde_json::json!({"stage": name, "missing": true}))?,
        );
        let normalized = normalized_windows_path(&path);
        if normalized != expected {
            return Err(serde_json::json!({
                "stage": name,
                "relative": normalized.strip_prefix(&format!("{normalized_storage}\\")),
                "unexpected_temp": true
            }));
        }
    }
    Ok(())
}

fn normalized_windows_path(path: &Path) -> String {
    path.to_string_lossy()
        .trim_start_matches("\\\\?\\")
        .to_lowercase()
}

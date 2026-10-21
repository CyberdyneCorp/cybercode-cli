//! `cyber-sandbox-exec --appcontainer` killed with its command (`sandbox` → Windows
//! enforcement). Tool timeouts, cancels, `task_stop` and host shutdowns terminate the
//! helper together with the command through the `--job` owner, so none of its cleanup
//! runs. Its grant journal stays, and the next launch revokes the dead identity's entries
//! and deletes its profile.
#![cfg(windows)]

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use cyber_sandbox::ContainerPlan;
use cyber_sandbox::windows_container::{has_entries, profile_registered};

fn helper() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_cyber-sandbox-exec"))
}

fn system32(program: &str) -> PathBuf {
    PathBuf::from(std::env::var_os("SystemRoot").unwrap())
        .join("System32")
        .join(program)
}

/// `path` without the `\\?\` prefix `canonicalize` adds, as PowerShell takes it.
fn plain(path: &Path) -> String {
    let text = path.display().to_string();
    text.strip_prefix(r"\\?\").unwrap_or(&text).to_owned()
}

fn plan(root: &Path, journal: &Path) -> String {
    ContainerPlan {
        roots: vec![root.to_path_buf()],
        writable: true,
        readable: Vec::new(),
        read_only: Vec::new(),
        unreadable: Vec::new(),
        network: false,
        journal: journal.to_path_buf(),
    }
    .encode()
    .unwrap()
}

fn journals(journal: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(journal)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
                .collect()
        })
        .unwrap_or_default()
}

#[test]
#[cfg_attr(
    windows,
    ignore = "the confined command exits before it starts inside the AppContainer sandbox, issue #9"
)]
fn a_killed_helpers_grants_and_profile_are_revoked_by_the_next_launch() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().canonicalize().unwrap();
    let (root, journal) = (base.join("root"), base.join("journal"));
    std::fs::create_dir_all(&root).unwrap();
    let started = root.join("started.txt");
    let script = format!(
        "Set-Content -LiteralPath '{}' -Value up; Start-Sleep -Seconds 120",
        plain(&started)
    );
    let mut child = Command::new(helper())
        .args(["--appcontainer", &plan(&root, &journal), "--"])
        .arg(system32(r"WindowsPowerShell\v1.0\powershell.exe"))
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .current_dir(&root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(60);
    while !started.exists() {
        assert!(
            child.try_wait().unwrap().is_none(),
            "the confined command exited before it started"
        );
        assert!(
            Instant::now() < deadline,
            "the confined command never started"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    let [record] = journals(&journal).try_into().unwrap();
    let name = record.file_stem().unwrap().to_str().unwrap().to_owned();
    assert!(has_entries(&root, &name).unwrap());

    // Terminated like the `--job` owner's kill-on-close: no cleanup runs.
    child.kill().unwrap();
    child.wait().unwrap();
    assert!(has_entries(&root, &name).unwrap());
    assert!(profile_registered(&name).unwrap());

    let next = Command::new(helper())
        .args(["--appcontainer", &plan(&root, &journal), "--"])
        .arg(system32("whoami.exe"))
        .current_dir(&root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .status()
        .unwrap();
    assert!(next.success(), "{next:?}");
    assert!(
        !has_entries(&root, &name).unwrap(),
        "the killed identity's entry is still on the root"
    );
    assert!(
        !has_entries(&started, &name).unwrap(),
        "the inherited entry of a file the command created is still there"
    );
    assert!(
        !profile_registered(&name).unwrap(),
        "{name} is still registered"
    );
    assert!(journals(&journal).is_empty(), "{:?}", journals(&journal));
}

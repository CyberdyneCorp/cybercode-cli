//! Independent grading executes in an owned, network-off scratch sandbox.

use super::trial::{TrialEnv, TrialResult};
use cyber_core::eval::Task;
use std::path::{Component, Path};
use std::process::Output;

pub(super) async fn grade(env: &TrialEnv, task: &Task, workspace: &Path, result: &mut TrialResult) {
    match graded(env, task, workspace).await {
        Ok((output, source_hash)) => {
            result.graded_source_sha256 = Some(source_hash);
            let exit = output.status.code();
            let text = format!(
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            let marker = text
                .lines()
                .rev()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("");
            result.grader_exit = exit;
            result.grader_tail = text
                .lines()
                .rev()
                .take(15)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<Vec<_>>()
                .join("\n");
            if (exit == Some(0) && marker == "PASS") || (exit == Some(1) && marker == "FAIL") {
                result.passed = exit == Some(0);
            } else {
                result.infrastructure_failure = true;
                result.error = Some("independent grader failed its output/setup contract".into());
            }
        }
        Err(error) => {
            result.infrastructure_failure = true;
            result.error = Some(error);
            result.stop_reason = "grader_error".into();
        }
    }
}

async fn graded(env: &TrialEnv, task: &Task, workspace: &Path) -> Result<(Output, String), String> {
    let scratch = tempfile::tempdir_in(&env.output).map_err(|e| e.to_string())?;
    let source = scratch.path().join("source");
    let inspection = tempfile::tempdir().map_err(|e| e.to_string())?;
    let args = vec![
        "-c".into(),
        "core.fsmonitor=false".into(),
        "-c".into(),
        "core.untrackedCache=false".into(),
        "-c".into(),
        "core.hooksPath=/dev/null".into(),
        "-C".into(),
        workspace.display().to_string(),
        "ls-files".into(),
        "--cached".into(),
        "--others".into(),
        "--exclude-standard".into(),
        "-z".into(),
    ];
    let listing = capture(env, inspection.path(), "git", &args, true).await?;
    if !listing.status.success() {
        return Err("graded-source enumeration failed".into());
    }
    let paths: std::collections::BTreeSet<_> = listing
        .stdout
        .split(|b| *b == 0)
        .filter(|p| !p.is_empty())
        .map(|p| {
            std::str::from_utf8(p)
                .map(std::path::PathBuf::from)
                .map_err(|_| "non-UTF-8 source paths unsupported".to_owned())
        })
        .collect::<Result<_, _>>()?;
    copy_source(workspace, &source, &paths, &mut (128 * 1024 * 1024))?;
    let source_hash = cyber_core::eval::tree_sha256(&source).map_err(|e| e.to_string())?;
    let grader = env.root.join(&task.grading.grader_dir);
    if task.grading.tree_sha256.as_ref() != cyber_core::eval::tree_sha256(&grader).ok().as_ref() {
        return Err("independent grader changed after manifest validation".into());
    }
    let argv: Vec<_> = task
        .grading
        .command
        .iter()
        .map(|a| {
            a.replace("{grader_dir}", &grader.display().to_string())
                .replace("{workspace}", &source.display().to_string())
        })
        .collect();
    let (program, args) = argv.split_first().ok_or("empty grader command")?;
    Ok((
        capture(env, scratch.path(), program, args, false).await?,
        source_hash,
    ))
}

// Grade precisely the tracked/non-ignored source universe represented by host evidence.
// Ignored compiler products or ignored source cannot silently change the tested patch.
fn copy_source(
    from: &Path,
    to: &Path,
    paths: &std::collections::BTreeSet<std::path::PathBuf>,
    remaining: &mut u64,
) -> Result<(), String> {
    std::fs::create_dir_all(to).map_err(|e| e.to_string())?;
    if paths.len() > 50_000 {
        return Err("too many graded source paths".into());
    }
    let mut aliases = Vec::new();
    for relative_path in paths {
        if !relative(relative_path)
            || relative_path
                .components()
                .any(|c| c.as_os_str().to_string_lossy().eq_ignore_ascii_case(".git"))
        {
            return Err("unsafe graded source path".into());
        }
        let source = from.join(relative_path);
        let target = to.join(relative_path);
        check_parents(from, relative_path)?;
        let metadata = match std::fs::symlink_metadata(&source) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e.to_string()),
        };
        if metadata.file_type().is_symlink() {
            aliases.push(relative_path);
            continue;
        }
        if !metadata.is_file() {
            return Err("graded source contains a special file".into());
        }
        copy_regular(&source, &target, remaining)?;
    }
    for path in aliases {
        let target = to.join(path);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        cyber_core::eval::copy_source_link(&from.join(path), &target, path)
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn check_parents(root: &Path, relative_path: &Path) -> Result<(), String> {
    let mut parent = root.to_path_buf();
    for part in relative_path
        .parent()
        .into_iter()
        .flat_map(Path::components)
    {
        parent.push(part);
        match std::fs::symlink_metadata(&parent) {
            Ok(m) if m.file_type().is_symlink() => {
                return Err("graded source has a symlink parent".into());
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => break,
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok(())
}

fn copy_regular(from: &Path, to: &Path, remaining: &mut u64) -> Result<(), String> {
    use std::io::Read;
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let input = std::fs::File::open(from).map_err(|e| e.to_string())?;
    let permissions = input.metadata().map_err(|e| e.to_string())?.permissions();
    let mut output = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(to)
        .map_err(|e| e.to_string())?;
    let count = std::io::copy(&mut input.take(remaining.saturating_add(1)), &mut output)
        .map_err(|e| e.to_string())?;
    if count > *remaining {
        return Err("graded source exceeds 128 MiB".into());
    }
    *remaining -= count;
    std::fs::set_permissions(to, permissions).map_err(|e| e.to_string())
}

#[cfg(windows)]
pub(super) async fn capture(
    _env: &TrialEnv,
    _directory: &Path,
    _program: &str,
    _args: &[String],
    _inspection: bool,
) -> Result<Output, String> {
    Err("real-repository verification is unavailable until native Windows command isolation is accepted (issue #9)".into())
}

#[cfg(not(windows))]
pub(super) async fn capture(
    env: &TrialEnv,
    directory: &Path,
    program: &str,
    args: &[String],
    inspection: bool,
) -> Result<Output, String> {
    use std::collections::BTreeMap;
    use std::time::Duration;
    use tokio::io::AsyncReadExt;
    let directory = directory.canonicalize().map_err(|e| e.to_string())?;
    let temporary_guard = tempfile::tempdir().map_err(|e| e.to_string())?;
    let temporary = temporary_guard
        .path()
        .canonicalize()
        .map_err(|e| e.to_string())?;
    let config = cyber_sandbox::SandboxConfig::resolve(
        &serde_json::json!({"sandbox":{"network":"off",
        "policy":"workspace-write"}}),
        &BTreeMap::new(),
        None,
        &env.home,
    );
    let mut unreadable: Vec<_> = env
        .private_roots
        .iter()
        .filter(|p| !inspection && !directory.starts_with(p))
        .cloned()
        .collect();
    // The host verifier may read the pinned benchmark inputs, but it retains every
    // other credential/private-file read ceiling. Trusted inspection also needs
    // read access to managed Git metadata; only owned scratch directories are writable.
    unreadable.extend(
        config
            .unreadable(&env.home)
            .into_iter()
            .filter(|p| p != &env.root),
    );
    for root in &env.private_roots {
        for name in [
            "chatgpt-auth.json",
            "copilot-auth.json",
            "credentials.json",
            "auth.json",
        ] {
            unreadable.push(root.join(name));
        }
    }
    if let Some(home) = std::env::var_os("CARGO_HOME") {
        unreadable.push(Path::new(&home).join("credentials.toml"));
    }
    let launch = cyber_sandbox::Launch {
        config,
        writable: vec![directory.clone(), temporary.clone()],
        read_only: Vec::new(),
        unreadable,
        deny_read: cyber_sandbox::DenyRead::default(),
        proxy: None,
        helper: cyber_sandbox::find_helper(),
    };
    let wrapped = cyber_sandbox::wrap(&launch, program, args).map_err(|e| e.to_string())?;
    let mut child = cyber_tools::HookCommandProcess::spawn(
        &wrapped.program,
        &wrapped.args,
        launch.helper.as_deref(),
        |command| {
            command.current_dir(&directory).env_clear();
            for name in ["PATH", "LANG", "LC_ALL", "CARGO_HOME", "RUSTUP_HOME"] {
                if let Some(value) = std::env::var_os(name) {
                    command.env(name, value);
                }
            }
            command
                .envs(wrapped.env)
                .env("HOME", &temporary)
                .env("TMPDIR", &temporary)
                .env("PYTHONDONTWRITEBYTECODE", "1")
                .env("PYTHONNOUSERSITE", "1")
                .env("PYTEST_DISABLE_PLUGIN_AUTOLOAD", "1")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_OPTIONAL_LOCKS", "0")
                .env("GIT_CONFIG_COUNT", "0")
                .env("GIT_ATTR_NOSYSTEM", "1")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .kill_on_drop(true);
        },
    )
    .await
    .map_err(|e| e.to_string())?;
    let mut stdout = child
        .stdout()
        .ok_or("missing verification stdout")?
        .take(9 * 1024 * 1024 + 1);
    let mut stderr = child
        .stderr()
        .ok_or("missing verification stderr")?
        .take(1024 * 1024 + 1);
    let captured = tokio::time::timeout(Duration::from_secs(300), async {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let (a, b) = tokio::join!(stdout.read_to_end(&mut out), stderr.read_to_end(&mut err));
        a.map_err(|e| e.to_string())?;
        b.map_err(|e| e.to_string())?;
        if out.len() > 9 * 1024 * 1024 || err.len() > 1024 * 1024 {
            return Err("verification output exceeds its bound".into());
        }
        let status = child.wait_tree().await.map_err(|e| e.to_string())?;
        Ok(Output {
            status,
            stdout: out,
            stderr: err,
        })
    })
    .await
    .map_err(|_| "verification timed out".to_owned())
    .and_then(|r| r);
    if captured.is_err() {
        child.terminate();
        let _ = child.wait().await;
    }
    captured
}

/// Catalog paths are owner metadata but still must not escape their fixed roots.
pub(super) fn relative(path: &Path) -> bool {
    !path.as_os_str().is_empty() && path.components().all(|p| matches!(p, Component::Normal(_)))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn source_copy_omits_build_and_git_state_and_bounds_files() {
        let root = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("target")).unwrap();
        std::fs::write(root.path().join("target/output"), "generated").unwrap();
        std::fs::write(root.path().join("source"), "source").unwrap();
        copy_source(
            root.path(),
            out.path(),
            &std::collections::BTreeSet::from(["source".into()]),
            &mut 6,
        )
        .unwrap();
        assert!(!out.path().join("target").exists());
        assert!(
            copy_source(
                root.path(),
                &out.path().join("small"),
                &std::collections::BTreeSet::from(["source".into()]),
                &mut 1
            )
            .is_err()
        );
        assert!(relative(Path::new("a/b")));
        assert!(!relative(Path::new("../outside")));
    }
    #[cfg(unix)]
    #[test]
    fn source_copy_refuses_links_without_reading_their_targets() {
        let root = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink("/etc/passwd", root.path().join("link")).unwrap();
        assert!(
            copy_source(
                root.path(),
                out.path(),
                &std::collections::BTreeSet::from(["link".into()]),
                &mut 1024
            )
            .is_err()
        );
        assert!(!out.path().join("link").exists());
    }
}

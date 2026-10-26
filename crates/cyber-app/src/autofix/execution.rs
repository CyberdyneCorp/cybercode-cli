//! Host-side checkout, sandboxed verification and non-force publication.
//!
//! Git commands run only in a host-created repository with inherited Git configuration
//! cleared. Model file tools cannot mutate its `.git`; verification sees it read-only.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use cyber_core::autofix::{CheckEvidence, Subscription, valid_sha};
use cyber_tools::HookCommandProcess as Process;
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;

const LIMIT: u64 = 1024 * 1024;
const GIT_TIMEOUT: Duration = Duration::from_secs(120);

pub struct Execution {
    pub root: PathBuf,
    pub home: PathBuf,
    pub private_roots: Vec<PathBuf>,
    #[cfg(test)]
    pub source_override: Option<PathBuf>,
}

fn git_arguments(args: &[&str]) -> Vec<String> {
    [
        "-c",
        "core.hooksPath=/dev/null",
        "-c",
        "core.fsmonitor=false",
        "-c",
        "credential.helper=",
        "-c",
        "credential.helper=!gh auth git-credential",
        "-c",
        "protocol.file.allow=never",
        "-c",
        "protocol.ext.allow=never",
        "-c",
        "http.sslVerify=true",
        "-c",
        "http.followRedirects=false",
        "-c",
        "commit.gpgSign=false",
        "-c",
        "core.attributesFile=/dev/null",
    ]
    .into_iter()
    .chain(args.iter().copied())
    .map(str::to_owned)
    .collect()
}

fn owner_environment(root: &Path) -> BTreeMap<String, String> {
    let mut env = BTreeMap::new();
    for key in [
        "HOME",
        "PATH",
        "USERPROFILE",
        "SystemRoot",
        "SYSTEMROOT",
        "GH_CONFIG_DIR",
        "GH_TOKEN",
        "GITHUB_TOKEN",
        "XDG_CONFIG_HOME",
        "SSL_CERT_FILE",
        "SSL_CERT_DIR",
    ] {
        if let Ok(value) = std::env::var(key) {
            env.insert(key.into(), value);
        }
    }
    if let Some(path) = std::env::var_os("PATH") {
        let canonical_root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
        let paths = std::env::split_paths(&path)
            .filter(|p| p.is_absolute())
            .filter_map(|p| std::fs::canonicalize(p).ok())
            .filter(|p| !p.starts_with(&canonical_root));
        if let Ok(safe) = std::env::join_paths(paths) {
            env.insert("PATH".into(), safe.to_string_lossy().into_owned());
        }
    }
    env.insert("GIT_CONFIG_NOSYSTEM".into(), "1".into());
    env.insert(
        "GIT_CONFIG_GLOBAL".into(),
        if cfg!(windows) { "NUL" } else { "/dev/null" }.into(),
    );
    env.insert("GIT_TERMINAL_PROMPT".into(), "0".into());
    for key in ["GIT_AUTHOR_NAME", "GIT_COMMITTER_NAME"] {
        env.insert(key.into(), "Cyber Auto-fix".into());
    }
    for key in ["GIT_AUTHOR_EMAIL", "GIT_COMMITTER_EMAIL"] {
        env.insert(key.into(), "autofix@localhost".into());
    }
    env
}

async fn limited<R: tokio::io::AsyncRead + Unpin>(stream: R) -> Result<Vec<u8>, String> {
    let mut data = vec![];
    stream
        .take(LIMIT + 1)
        .read_to_end(&mut data)
        .await
        .map_err(|_| "process output unavailable")?;
    if data.len() as u64 > LIMIT {
        return Err("process output exceeded limit".into());
    }
    Ok(data)
}

async fn capture(
    mut process: Process,
    deadline: tokio::time::Instant,
) -> Result<(bool, Vec<u8>, Vec<u8>), String> {
    let stdout = process.stdout().ok_or("process stdout unavailable")?;
    let stderr = process.stderr().ok_or("process stderr unavailable")?;
    let result = tokio::time::timeout_at(deadline, async {
        let (status, stdout, stderr) = tokio::try_join!(
            async {
                process
                    .wait_tree()
                    .await
                    .map_err(|_| "process wait failed".to_owned())
            },
            limited(stdout),
            limited(stderr)
        )?;
        Ok::<_, String>((status.success(), stdout, stderr))
    })
    .await;
    // Own the complete process group even when one pipe exceeds its limit or the
    // leader exits while descendants retain descriptors. Never leave a Git helper.
    process.terminate();
    let _ = process.wait().await;
    result.map_err(|_| "process time limit exceeded; external outcome may be unknown".to_owned())?
}

async fn git(root: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
    let args = git_arguments(args);
    let env = owner_environment(root);
    let process = Process::spawn("git", &args, helper().as_deref(), |command| {
        command
            .current_dir(root)
            .env_clear()
            .envs(env)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
    })
    .await
    .map_err(|_| "Git executable unavailable")?;
    let (passed, stdout, _) = capture(process, tokio::time::Instant::now() + GIT_TIMEOUT).await?;
    if !passed {
        return Err("Git operation failed; no process output was disclosed".into());
    }
    Ok(stdout)
}

fn text(bytes: Vec<u8>) -> Result<String, String> {
    String::from_utf8(bytes)
        .map(|s| s.trim().to_owned())
        .map_err(|_| "Git output was not UTF-8".into())
}

fn safe_id(id: &str) -> Result<(), String> {
    if id.is_empty()
        || id.len() > 128
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
    {
        return Err("invalid attempt ID".into());
    }
    Ok(())
}

fn destination(s: &Subscription) -> Result<String, String> {
    s.config.validate()?;
    Ok(format!("https://github.com/{}.git", s.config.repository))
}

impl Execution {
    fn remote(&self, s: &Subscription) -> Result<(String, bool), String> {
        #[cfg(test)]
        if let Some(path) = &self.source_override {
            return Ok((path.to_string_lossy().into_owned(), true));
        }
        Ok((destination(s)?, false))
    }

    pub fn path(&self, id: &str) -> Result<PathBuf, String> {
        safe_id(id)?;
        let path = self.root.join(id);
        if std::fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err("attempt checkout cannot be a symlink".into());
        }
        Ok(path)
    }

    pub fn supported(&self) -> Result<(), String> {
        if cfg!(windows) {
            return Err("auto-fix verification is unsupported on Windows pending native verifier acceptance".into());
        }
        if !cyber_sandbox::available() {
            return Err("auto-fix requires an available native OS sandbox".into());
        }
        Ok(())
    }

    pub async fn prepare(&self, s: &Subscription, id: &str, head: &str) -> Result<PathBuf, String> {
        self.supported()?;
        if !valid_sha(head) {
            return Err("invalid head SHA".into());
        }
        let (remote, local_test) = self.remote(s)?;
        tokio::fs::create_dir_all(&self.root)
            .await
            .map_err(|_| "cannot create private auto-fix root")?;
        if std::fs::symlink_metadata(&self.root)
            .map_err(|_| "auto-fix root unavailable")?
            .file_type()
            .is_symlink()
        {
            return Err("auto-fix root cannot be a symlink".into());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            tokio::fs::set_permissions(&self.root, std::fs::Permissions::from_mode(0o700))
                .await
                .map_err(|_| "cannot secure auto-fix root")?;
        }
        let root = self.path(id)?;
        tokio::fs::create_dir(&root)
            .await
            .map_err(|_| "attempt checkout already exists or cannot be created")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            tokio::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700))
                .await
                .map_err(|_| "cannot secure attempt checkout")?;
        }
        git(
            &root,
            &[
                "-c",
                "init.templateDir=",
                "init",
                "--initial-branch=autofix",
            ],
        )
        .await?;
        let mut fetch_args = vec![
            "fetch",
            "--no-tags",
            "--no-recurse-submodules",
            "--depth=1",
            &remote,
            head,
        ];
        if local_test {
            fetch_args.splice(0..0, ["-c", "protocol.file.allow=always"]);
        }
        git(&root, &fetch_args).await?;
        git(&root, &["checkout", "--detach", head, "--"]).await?;
        if text(git(&root, &["rev-parse", "HEAD"]).await?)? != head {
            return Err("checkout head mismatch".into());
        }
        let root = std::fs::canonicalize(root).map_err(|_| "attempt checkout unavailable")?;
        validate_verification_paths(s, &root)?;
        Ok(root)
    }

    pub async fn commit(&self, s: &Subscription, id: &str, head: &str) -> Result<String, String> {
        let root = self.path(id)?;
        if text(git(&root, &["rev-parse", "HEAD"]).await?)? != head {
            return Err("attempt base changed".into());
        }
        // Refuse repository-local conversion/execution settings before `git add`.
        reject_local_drivers(&root).await?;
        if !git(
            &root,
            &[
                "ls-files",
                "--others",
                "--ignored",
                "--exclude-standard",
                "-z",
            ],
        )
        .await?
        .is_empty()
        {
            return Err(
                "ignored files created by the attempt cannot influence verification".into(),
            );
        }
        git(&root, &["add", "--all", "--", "."]).await?;
        let paths = changed_paths(
            &root,
            &[
                "diff",
                "--cached",
                "--name-only",
                "-z",
                "--no-renames",
                head,
                "--",
            ],
        )
        .await?;
        validate_patch_paths(s, &paths)?;
        if paths.is_empty() {
            return Err("attempt produced no patch".into());
        }
        reject_link_changes(&root, head).await?;
        git(&root, &["commit", "--no-verify", "-m", "Fix PR feedback"]).await?;
        let commit = text(git(&root, &["rev-parse", "HEAD"]).await?)?;
        validate_commit(&root, head, &commit).await?;
        Ok(commit)
    }

    pub async fn checks(&self, s: &Subscription, id: &str) -> Result<Vec<CheckEvidence>, String> {
        self.check_report(s, id).await.map(|(checks, _)| checks)
    }
    pub async fn check_report(
        &self,
        s: &Subscription,
        id: &str,
    ) -> Result<(Vec<CheckEvidence>, Vec<String>), String> {
        self.supported()?;
        let root =
            std::fs::canonicalize(self.path(id)?).map_err(|_| "attempt checkout unavailable")?;
        self.run_commands(s, &root).await
    }

    /// Run the pinned verification on a disposable copy of the exact base. Neither
    /// ignored build output nor test mutations can contaminate the agent checkout.
    pub async fn diagnose(&self, s: &Subscription, id: &str) -> Result<Vec<String>, String> {
        self.supported()?;
        let source = self.path(id)?;
        let head = &s.active().ok_or("missing active attempt")?.head;
        if !valid_sha(head) {
            return Err("invalid diagnosis base".into());
        }
        let copy = tempfile::Builder::new()
            .prefix(".autofix-diagnosis-")
            .tempdir_in(&self.root)
            .map_err(|_| "diagnosis checkout unavailable")?;
        let root = copy
            .path()
            .canonicalize()
            .map_err(|_| "diagnosis checkout unavailable")?;
        git(
            &root,
            &[
                "-c",
                "init.templateDir=",
                "init",
                "--initial-branch=autofix",
            ],
        )
        .await?;
        let source = source.to_str().ok_or("attempt checkout is not UTF-8")?;
        // This transport is only to the host-created local repository, never a user
        // URL. It copies objects without inherited hooks/config or shared hardlinks.
        git(
            &root,
            &[
                "-c",
                "protocol.file.allow=always",
                "fetch",
                "--no-tags",
                "--no-recurse-submodules",
                "--depth=1",
                "--update-shallow",
                source,
                head,
            ],
        )
        .await?;
        git(&root, &["checkout", "--detach", head, "--"]).await?;
        let (_, failures) = self.run_commands(s, &root).await?;
        copy.close()
            .map_err(|_| "diagnosis checkout cleanup failed")?;
        Ok(failures)
    }

    async fn run_commands(
        &self,
        s: &Subscription,
        root: &Path,
    ) -> Result<(Vec<CheckEvidence>, Vec<String>), String> {
        let before = text(git(root, &["rev-parse", "HEAD"]).await?)?;
        let scratch = tempfile::Builder::new()
            .prefix(".autofix-verification-")
            .tempdir_in(root)
            .map_err(|_| "verification scratch unavailable")?;
        let mut launch = self.launch(root)?;
        self.pin_verification_sources(&mut launch, s, root)?;
        if let Ok(owner) = std::fs::canonicalize(&s.directory) {
            if owner != root {
                if root.starts_with(&owner) {
                    return Err("owner Location overlaps the private attempt root".into());
                }
                launch.unreadable.push(owner);
            }
        }
        let deadline = verification_deadline(s)?;
        let mut evidence = vec![];
        let mut failures = vec![];
        for (index, argv) in s.config.checks.iter().enumerate() {
            let program = verification_program(&argv[0], root)?;
            let wrapped = cyber_sandbox::wrap(&launch, &program, &argv[1..])
                .map_err(|_| "native verification sandbox unavailable")?;
            let wrapped = tighten_credentials(wrapped)?;
            let env = verification_environment(scratch.path());
            let process = Process::spawn(
                &wrapped.program,
                &wrapped.args,
                launch.helper.as_deref(),
                |command| {
                    command
                        .current_dir(&root)
                        .env_clear()
                        .envs(env)
                        .envs(&wrapped.env)
                        .stdin(Stdio::null())
                        .stdout(Stdio::piped())
                        .stderr(Stdio::piped())
                        .kill_on_drop(true);
                },
            )
            .await
            .map_err(|_| "verification process unavailable")?;
            let (passed, stdout, stderr) = capture(process, deadline).await?;
            let mut digest = Sha256::new();
            digest.update(&stdout);
            digest.update([0]);
            digest.update(&stderr);
            evidence.push(CheckEvidence {
                argv: argv.clone(),
                passed,
                output_digest: format!("{:x}", digest.finalize()),
            });
            if !passed {
                failures.push(failure_feedback(index, &stdout, &stderr, root));
                break;
            }
        }
        scratch
            .close()
            .map_err(|_| "verification scratch cleanup failed")?;
        require_clean(root, &before).await?;
        Ok((evidence, failures))
    }

    fn pin_verification_sources(
        &self,
        launch: &mut cyber_sandbox::Launch,
        subscription: &Subscription,
        root: &Path,
    ) -> Result<(), String> {
        validate_verification_paths(subscription, root)?;
        // A literal file denial alone can be escaped by renaming its writable
        // parent, changing the file under that new path and restoring both.
        // Keep each containing directory below the checkout read-only as well.
        // The checkout itself remains writable for build output and scratch.
        for source in &subscription.config.verification_paths {
            let path = std::fs::canonicalize(root.join(source))
                .map_err(|_| "pinned verification source is unavailable")?;
            launch.read_only.extend(
                path.ancestors()
                    .take_while(|path| *path != root)
                    .map(Path::to_path_buf),
            );
        }
        launch.read_only.sort();
        launch.read_only.dedup();
        Ok(())
    }

    fn launch(&self, root: &Path) -> Result<cyber_sandbox::Launch, String> {
        let mut config = cyber_sandbox::SandboxConfig::resolve(
            &serde_json::json!({}),
            &BTreeMap::new(),
            Some("workspace-write"),
            &self.home,
        );
        config.network = cyber_sandbox::NetworkMode::Off;
        config.env.clean = true;
        config.env.login_shell = false;
        config.env.set.clear();
        config.env_allow.clear();
        config.excluded_commands.clear();
        let mut unreadable = config.unreadable(&self.home);
        #[cfg(target_os = "linux")]
        unreadable.extend(
            ["/run/user", "/run/dbus", "/var/run/dbus"]
                .into_iter()
                .map(PathBuf::from),
        );
        for key in ["SSH_AUTH_SOCK", "GPG_AGENT_INFO"] {
            if let Ok(value) = std::env::var(key) {
                if let Some(path) = value.split(':').next().filter(|s| !s.is_empty()) {
                    unreadable.push(PathBuf::from(path));
                }
            }
        }
        unreadable.extend(self.private_roots.iter().cloned());
        unreadable.extend(
            [
                ".cargo/credentials.toml",
                ".cargo/credentials",
                ".cargo/config.toml",
                ".cargo/config",
                ".npmrc",
                ".pypirc",
                ".pip",
                ".git-credentials",
                ".gitconfig",
            ]
            .into_iter()
            .map(|p| self.home.join(p)),
        );
        if let Some(path) = std::env::var_os("CARGO_HOME") {
            let path = PathBuf::from(path);
            unreadable.extend(
                ["credentials.toml", "credentials", "config.toml", "config"]
                    .into_iter()
                    .map(|p| path.join(p)),
            );
        }
        unreadable.extend([
            self.home.join(".config"),
            self.home.join("Library/Keychains"),
        ]);
        if let Some(path) = std::env::var_os("GH_CONFIG_DIR") {
            unreadable.push(PathBuf::from(path));
        }
        if let Some(path) = std::env::var_os("XDG_CONFIG_HOME") {
            unreadable.push(PathBuf::from(path));
        }
        // All other private owner data and sibling attempt checkouts are hidden.
        // Keep this attempt reachable without hiding a parent of its writable root.
        for parent in [
            self.root.as_path(),
            self.root.parent().unwrap_or(&self.root),
        ] {
            if let Ok(entries) = std::fs::read_dir(parent) {
                for entry in entries {
                    let path = entry
                        .map_err(|_| "private root inventory unavailable")?
                        .path();
                    let canonical = std::fs::canonicalize(&path).unwrap_or(path);
                    if canonical != root && !root.starts_with(&canonical) {
                        unreadable.push(canonical);
                    }
                }
            }
        }
        unreadable.sort();
        unreadable.dedup();
        if unreadable.iter().any(|p| root.starts_with(p)) {
            return Err("credential isolation would hide the attempt checkout".into());
        }
        Ok(cyber_sandbox::Launch {
            config,
            writable: vec![root.to_path_buf()],
            read_only: vec![root.join(".git")],
            unreadable,
            deny_read: Default::default(),
            proxy: None,
            helper: helper(),
        })
    }

    pub async fn push(&self, s: &Subscription, id: &str, commit: &str) -> Result<(), String> {
        let root = self.path(id)?;
        let (remote, local_test) = self.remote(s)?;
        self.push_to(s, &root, commit, &remote, local_test).await
    }

    async fn push_to(
        &self,
        s: &Subscription,
        root: &Path,
        commit: &str,
        remote: &str,
        local_test: bool,
    ) -> Result<(), String> {
        if local_test && !cfg!(test) {
            return Err("local publication is test-only".into());
        }
        if !valid_sha(commit) {
            return Err("invalid push commit".into());
        }
        let attempt = s.active().ok_or("missing active attempt")?;
        if attempt.commit.as_deref() != Some(commit) {
            return Err("commit differs from durable push intent".into());
        }
        reject_local_drivers(root).await?;
        validate_commit(root, &attempt.head, commit).await?;
        require_clean(root, commit).await?;
        let paths = changed_paths(
            root,
            &[
                "diff",
                "--name-only",
                "-z",
                "--no-renames",
                &attempt.head,
                commit,
                "--",
            ],
        )
        .await?;
        if paths.is_empty() {
            return Err("publication requires a nonempty patch".into());
        }
        validate_patch_paths(s, &paths)?;
        reject_link_changes(root, &attempt.head).await?;
        let reference = format!("refs/heads/{}", s.branch);
        git(root, &["check-ref-format", &reference]).await?;
        let mut read_args = vec!["ls-remote", "--heads", remote, &reference];
        if local_test {
            read_args.splice(0..0, ["-c", "protocol.file.allow=always"]);
        }
        let current = text(git(root, &read_args).await?)?;
        if current.split_whitespace().next() != Some(attempt.head.as_str()) {
            return Err("remote head changed before push".into());
        }
        let refspec = format!("{commit}:{reference}");
        let mut push_args = vec!["push", "--no-verify", remote, &refspec];
        if local_test {
            push_args.splice(0..0, ["-c", "protocol.file.allow=always"]);
        }
        // Ordinary non-force push rejects a concurrent update after the preceding read.
        git(root, &push_args).await?;
        Ok(())
    }
}

fn failure_feedback(index: usize, stdout: &[u8], stderr: &[u8], root: &Path) -> String {
    let raw = format!(
        "{}\n{}",
        String::from_utf8_lossy(stdout),
        String::from_utf8_lossy(stderr)
    );
    let redacted = cyber_core::memory::redact_credentials(&raw);
    let safe = redacted
        .replace(&root.to_string_lossy().to_string(), "<verification>")
        .chars()
        .filter(|ch| !ch.is_control() || matches!(ch, '\n' | '\t'))
        .collect::<String>();
    let mut start = safe.len().saturating_sub(7600);
    while !safe.is_char_boundary(start) {
        start += 1;
    }
    format!(
        "Pinned verification command {} failed. Untrusted test output:\n{}",
        index + 1,
        &safe[start..]
    )
}

async fn validate_commit(root: &Path, head: &str, commit: &str) -> Result<(), String> {
    if !valid_sha(head) || !valid_sha(commit) || head == commit {
        return Err("invalid candidate commit".into());
    }
    if text(git(root, &["rev-parse", "HEAD"]).await?)? != commit {
        return Err("candidate HEAD changed".into());
    }
    let parents = text(git(root, &["show", "--no-patch", "--format=%P", commit]).await?)?;
    if parents != head {
        return Err("candidate must be exactly one commit above the admitted head".into());
    }
    Ok(())
}

async fn require_clean(root: &Path, commit: &str) -> Result<(), String> {
    if text(git(root, &["rev-parse", "HEAD"]).await?)? != commit {
        return Err("candidate HEAD changed during verification".into());
    }
    if !git(root, &["status", "--porcelain", "--untracked-files=all"])
        .await?
        .is_empty()
    {
        return Err("verification modified the candidate source".into());
    }
    Ok(())
}

async fn changed_paths(root: &Path, args: &[&str]) -> Result<Vec<String>, String> {
    git(root, args)
        .await?
        .split(|b| *b == 0)
        .filter(|p| !p.is_empty())
        .map(|p| {
            std::str::from_utf8(p)
                .map(str::to_owned)
                .map_err(|_| "patch path is not UTF-8".into())
        })
        .collect()
}

fn validate_patch_paths(s: &Subscription, paths: &[String]) -> Result<(), String> {
    for path in paths {
        if protected(path) || s.config.verification_paths.iter().any(|p| beneath(path, p)) {
            return Err("patch changes a protected policy or verification path".into());
        }
        for argv in &s.config.checks {
            if argv.first().is_some_and(|program| {
                program.contains('/') && beneath(path, program.trim_start_matches("./"))
            }) {
                return Err("patch changes its verification executable".into());
            }
        }
    }
    Ok(())
}

fn validate_verification_paths(s: &Subscription, root: &Path) -> Result<(), String> {
    for path in &s.config.verification_paths {
        let target = root.join(path);
        let canonical = std::fs::canonicalize(&target)
            .map_err(|_| "pinned verification source does not exist in the base")?;
        if !canonical.starts_with(root)
            || target
                .ancestors()
                .take_while(|p| *p != root)
                .any(|p| std::fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_symlink()))
        {
            return Err("pinned verification source traverses a symlink".into());
        }
    }
    Ok(())
}

async fn reject_local_drivers(root: &Path) -> Result<(), String> {
    // No project-controlled filter/textconv/shell driver may run in the credentialed
    // owner broker. `git config --get-regexp` exits one when no value matches.
    let config = text(git(root, &["config", "--local", "--list"]).await?)?;
    if config.lines().any(|line| {
        let key = line
            .split('=')
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        key.starts_with("filter.")
            || key.starts_with("diff.")
            || key.starts_with("merge.")
            || key.starts_with("include.")
            || key.starts_with("includeif.")
            || key.starts_with("url.")
            || key.starts_with("credential.")
            || key.starts_with("http.")
    }) {
        return Err(
            "repository-local executable drivers or credential routing require manual review"
                .into(),
        );
    }
    Ok(())
}

async fn reject_link_changes(root: &Path, head: &str) -> Result<(), String> {
    let raw = text(
        git(
            root,
            &["diff", "--cached", "--raw", "--no-renames", head, "--"],
        )
        .await?,
    )?;
    if raw.lines().any(|line| {
        line.split_whitespace()
            .take(2)
            .any(|mode| matches!(mode.trim_start_matches(':'), "120000" | "160000"))
    }) {
        return Err("symlink and submodule changes require manual review".into());
    }
    Ok(())
}

fn beneath(path: &str, parent: &str) -> bool {
    let (path, parent) = (
        path.to_lowercase(),
        parent.trim_end_matches('/').to_lowercase(),
    );
    path == parent
        || path
            .strip_prefix(&parent)
            .is_some_and(|rest| rest.starts_with('/'))
}

pub fn protected(path: &str) -> bool {
    let lower = path.to_lowercase();
    lower.split('/').any(|p| {
        matches!(
            p,
            ".git"
                | ".github"
                | ".cyber"
                | "agents.md"
                | "cyber.json"
                | "cyber.jsonc"
                | ".gitattributes"
                | ".gitmodules"
        )
    }) || path.bytes().any(|b| b < 32 || b == 127)
        || path.starts_with('/')
        || path.contains('\\')
        || path.split('/').any(|p| matches!(p, ".." | "."))
}

/// Auto-fix verification never needs the owner's keychain. This tighter profile is
/// local to this feature; ordinary interactive command policy remains unchanged.
fn tighten_credentials(wrapped: cyber_sandbox::Wrapped) -> Result<cyber_sandbox::Wrapped, String> {
    #[cfg(target_os = "macos")]
    {
        let mut wrapped = wrapped;
        if wrapped.program != "/usr/bin/sandbox-exec"
            || wrapped.args.first().map(String::as_str) != Some("-p")
        {
            return Err("unexpected native verification wrapper".into());
        }
        let profile = wrapped
            .args
            .get_mut(1)
            .ok_or("missing native verification profile")?;
        profile.push_str("\n(deny mach-lookup (global-name \"com.apple.SecurityServer\") (global-name \"com.apple.securityd\") (global-name \"com.apple.securityd.xpc\") (global-name \"com.apple.securityd.system\") (global-name \"com.apple.CryptoTokenKit.ctkd\") (global-name \"com.apple.ctkd\"))\n");
        Ok(wrapped)
    }
    #[cfg(not(target_os = "macos"))]
    {
        Ok(wrapped)
    }
}

fn verification_deadline(s: &Subscription) -> Result<tokio::time::Instant, String> {
    let started = s.active().ok_or("missing active attempt")?.started_ms;
    let elapsed = chrono::Utc::now()
        .timestamp_millis()
        .saturating_sub(started)
        .max(0);
    let remaining = i64::from(s.config.max_seconds) * 1000 - elapsed;
    if remaining <= 0 {
        return Err("attempt elapsed limit exceeded before verification".into());
    }
    Ok(tokio::time::Instant::now() + Duration::from_millis(remaining as u64))
}

fn verification_program(program: &str, root: &Path) -> Result<String, String> {
    if program.contains('/') || program.contains('\\') {
        let path = if Path::new(program).is_absolute() {
            PathBuf::from(program)
        } else {
            root.join(program)
        };
        let resolved =
            std::fs::canonicalize(&path).map_err(|_| "verification executable unavailable")?;
        if !Path::new(program).is_absolute()
            && (!resolved.starts_with(root)
                || std::fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink()))
        {
            return Err(
                "relative verification executable cannot escape or traverse a symlink".into(),
            );
        }
        // Preserve argv[0] for multi-call proxies such as cargo -> rustup. The
        // canonical target is only for containment, never the executed spelling.
        return Ok(path.to_string_lossy().into_owned());
    }
    let path = std::env::var_os("PATH").ok_or("verification PATH unavailable")?;
    for directory in
        std::env::split_paths(&path).filter(|p| p.is_absolute() && !p.starts_with(root))
    {
        let candidate = directory.join(program);
        if candidate.is_file() {
            let resolved = std::fs::canonicalize(&candidate)
                .map_err(|_| "verification executable unavailable")?;
            if resolved.starts_with(root) {
                continue;
            }
            return Ok(candidate.to_string_lossy().into_owned());
        }
    }
    Err("verification executable must resolve outside the writable checkout".into())
}

fn helper() -> Option<PathBuf> {
    let binary = std::env::current_exe().ok()?;
    let path = binary.parent()?.join(if cfg!(windows) {
        "cyber-sandbox-exec.exe"
    } else {
        "cyber-sandbox-exec"
    });
    path.is_file().then_some(path)
}

fn verification_environment(scratch: &Path) -> BTreeMap<String, String> {
    let mut env = BTreeMap::new();
    for key in [
        "PATH",
        "HOME",
        "USERPROFILE",
        "SystemRoot",
        "SYSTEMROOT",
        "TMPDIR",
        "TEMP",
        "TMP",
        "RUSTUP_HOME",
        "CARGO_HOME",
    ] {
        if let Ok(value) = std::env::var(key) {
            env.insert(key.into(), value);
        }
    }
    for key in ["TMPDIR", "TMP", "TEMP"] {
        env.insert(key.into(), scratch.to_string_lossy().into_owned());
    }
    env.insert("GIT_CONFIG_NOSYSTEM".into(), "1".into());
    env.insert(
        "GIT_CONFIG_GLOBAL".into(),
        if cfg!(windows) { "NUL" } else { "/dev/null" }.into(),
    );
    env.insert("GIT_TERMINAL_PROMPT".into(), "0".into());
    env
}

#[cfg(test)]
#[path = "execution_tests.rs"]
mod tests;

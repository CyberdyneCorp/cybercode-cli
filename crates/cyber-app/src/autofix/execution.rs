//! Host-side checkout, sandboxed verification and non-force publication.
use cyber_core::autofix::{CheckEvidence, Subscription, valid_sha};
use cyber_tools::tools::process::Process;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncReadExt;

const LIMIT: u64 = 1024 * 1024;
pub struct Execution {
    pub root: PathBuf,
    pub home: PathBuf,
}
fn git_command(root: &Path, args: &[String]) -> tokio::process::Command {
    let mut c = tokio::process::Command::new("git");
    c.args([
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
        "http.sslVerify=true",
        "-c",
        "commit.gpgSign=false",
    ]);
    c.args(args).current_dir(root).env_clear();
    for k in [
        "HOME",
        "PATH",
        "USERPROFILE",
        "SystemRoot",
        "SYSTEMROOT",
        "GH_CONFIG_DIR",
        "XDG_CONFIG_HOME",
        "SSL_CERT_FILE",
        "SSL_CERT_DIR",
    ] {
        if let Some(v) = std::env::var_os(k) {
            c.env(k, v);
        }
    }
    c.env("GIT_CONFIG_NOSYSTEM", "1")
        .env(
            "GIT_CONFIG_GLOBAL",
            if cfg!(windows) { "NUL" } else { "/dev/null" },
        )
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_AUTHOR_NAME", "Cyber Auto-fix")
        .env("GIT_AUTHOR_EMAIL", "autofix@localhost")
        .env("GIT_COMMITTER_NAME", "Cyber Auto-fix")
        .env("GIT_COMMITTER_EMAIL", "autofix@localhost")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    c
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
async fn git(root: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
    let args = args.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let mut child = git_command(root, &args)
        .spawn()
        .map_err(|_| "Git executable unavailable")?;
    let out = child.stdout.take().ok_or("Git stdout unavailable")?;
    let err = child.stderr.take().ok_or("Git stderr unavailable")?;
    let task = async {
        let (status, out, _) = tokio::try_join!(
            async {
                child
                    .wait()
                    .await
                    .map_err(|_| "Git wait failed".to_string())
            },
            limited(out),
            limited(err)
        )?;
        if !status.success() {
            return Err("Git operation failed; inspect owner-side diagnostics".into());
        }
        Ok(out)
    };
    tokio::time::timeout(Duration::from_secs(120), task)
        .await
        .map_err(|_| "Git operation timed out; external outcome may be unknown".to_string())?
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
fn destination(s: &Subscription) -> String {
    format!("https://github.com/{}.git", s.config.repository)
}
impl Execution {
    pub fn path(&self, id: &str) -> Result<PathBuf, String> {
        safe_id(id)?;
        Ok(self.root.join(id))
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
        tokio::fs::create_dir_all(&self.root)
            .await
            .map_err(|_| "cannot create private auto-fix root")?;
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
        git(
            &root,
            &[
                "fetch",
                "--no-tags",
                "--no-recurse-submodules",
                "--depth=1",
                &destination(s),
                head,
            ],
        )
        .await?;
        git(&root, &["checkout", "--detach", head, "--"]).await?;
        if text(git(&root, &["rev-parse", "HEAD"]).await?)? != head {
            return Err("checkout head mismatch".into());
        }
        Ok(root)
    }
    pub async fn commit(&self, id: &str, head: &str) -> Result<String, String> {
        let root = self.path(id)?;
        if text(git(&root, &["rev-parse", "HEAD"]).await?)? != head {
            return Err("attempt base changed".into());
        }
        git(&root, &["add", "--all", "--", "."]).await?;
        let paths = git(
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
        let mut count = 0;
        for path in paths.split(|b| *b == 0).filter(|p| !p.is_empty()) {
            let path = std::str::from_utf8(path).map_err(|_| "patch path is not UTF-8")?;
            if protected(path) {
                return Err("patch changes a protected policy or verification path".into());
            }
            count += 1;
        }
        if count == 0 {
            return Err("attempt produced no patch".into());
        }
        // Symlink/submodule changes can redirect verification or disclose host files.
        let raw = text(
            git(
                &root,
                &["diff", "--cached", "--raw", "--no-renames", head, "--"],
            )
            .await?,
        )?;
        if raw.lines().any(|l| {
            l.split_whitespace()
                .take(2)
                .any(|m| matches!(m.trim_start_matches(':'), "120000" | "160000"))
        }) {
            return Err("symlink and submodule changes require manual review".into());
        }
        git(
            &root,
            &[
                "-c",
                "core.hooksPath=/dev/null",
                "commit",
                "--no-verify",
                "-m",
                "Fix PR feedback",
            ],
        )
        .await?;
        let commit = text(git(&root, &["rev-parse", "HEAD"]).await?)?;
        if !valid_sha(&commit) {
            return Err("invalid resulting commit".into());
        }
        Ok(commit)
    }
    pub async fn checks(&self, s: &Subscription, id: &str) -> Result<Vec<CheckEvidence>, String> {
        self.supported()?;
        let root =
            std::fs::canonicalize(self.path(id)?).map_err(|_| "attempt checkout unavailable")?;
        let config = cyber_sandbox::SandboxConfig::resolve(
            &serde_json::json!({"sandbox":{"network":"off","env":{"inherit":"none","login_shell":false}}}),
            &BTreeMap::new(),
            Some("workspace-write"),
            &self.home,
        );
        let launch = cyber_sandbox::Launch {
            config,
            writable: vec![root.clone()],
            read_only: vec![root.join(".git")],
            unreadable: vec![
                self.home.join(".config"),
                self.home.join(".ssh"),
                self.home.join(".aws"),
                self.home.join(".netrc"),
                self.home.join("Library/Keychains"),
            ],
            deny_read: Default::default(),
            proxy: None,
            helper: helper(),
        };
        let mut evidence = vec![];
        let deadline =
            tokio::time::Instant::now() + Duration::from_secs(u64::from(s.config.max_seconds));
        for argv in &s.config.checks {
            let wrapped = cyber_sandbox::wrap(&launch, &argv[0], &argv[1..])
                .map_err(|_| "native verification sandbox unavailable")?;
            let env = verification_environment();
            let mut process = Process::spawn(
                &wrapped.program,
                &wrapped.args,
                launch.helper.as_deref(),
                |c| {
                    c.current_dir(&root)
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
            let out = process.stdout().ok_or("verification stdout unavailable")?;
            let err = process.stderr().ok_or("verification stderr unavailable")?;
            let result = tokio::time::timeout_at(deadline, async {
                let (status, out, err) = tokio::try_join!(
                    async {
                        process
                            .wait_tree()
                            .await
                            .map_err(|_| "verification wait failed".to_string())
                    },
                    limited(out),
                    limited(err)
                )?;
                Ok::<_, String>((status.success(), out, err))
            })
            .await;
            process.terminate();
            let _ = process.wait().await;
            let (passed, out, err) = result.map_err(|_| "verification time limit exceeded")??;
            let mut digest = Sha256::new();
            digest.update(&out);
            digest.update([0]);
            digest.update(&err);
            evidence.push(CheckEvidence {
                argv: argv.clone(),
                passed,
                output_digest: format!("{:x}", digest.finalize()),
            });
            if !passed {
                break;
            }
        }
        let clean = git(
            &root,
            &["status", "--porcelain", "--untracked-files=normal"],
        )
        .await?;
        if !clean.is_empty() {
            return Err("verification modified the candidate source".into());
        }
        Ok(evidence)
    }
    pub async fn push(&self, s: &Subscription, id: &str, commit: &str) -> Result<(), String> {
        if !valid_sha(commit) {
            return Err("invalid push commit".into());
        }
        let root = self.path(id)?;
        let reference = format!("refs/heads/{}", s.branch);
        git(&root, &["check-ref-format", &reference]).await?;
        let expected = s.active().ok_or("missing active attempt")?.head.as_str();
        let remote = text(
            git(
                &root,
                &["ls-remote", "--heads", &destination(s), &reference],
            )
            .await?,
        )?;
        if remote.split_whitespace().next() != Some(expected) {
            return Err("remote head changed before push".into());
        }
        // A concurrent update after the read is rejected by ordinary non-force Git push.
        let refspec = format!("{commit}:{reference}");
        git(&root, &["push", "--no-verify", &destination(s), &refspec]).await?;
        Ok(())
    }
}
pub fn protected(path: &str) -> bool {
    path.split('/').any(|p| {
        matches!(
            p,
            ".git" | ".github" | ".cyber" | "AGENTS.md" | "cyber.json" | "cyber.jsonc"
        )
    }) || path.bytes().any(|b| b < 32 || b == 127)
        || path.starts_with('/')
        || path.split('/').any(|p| p == "..")
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
fn verification_environment() -> BTreeMap<String, String> {
    let mut env = BTreeMap::new();
    for k in [
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
        if let Ok(v) = std::env::var(k) {
            env.insert(k.into(), v);
        }
    }
    env.insert("GIT_CONFIG_NOSYSTEM".into(), "1".into());
    env.insert(
        "GIT_CONFIG_GLOBAL".into(),
        if cfg!(windows) { "NUL" } else { "/dev/null" }.into(),
    );
    env.insert("GIT_TERMINAL_PROMPT".into(), "0".into());
    env
}

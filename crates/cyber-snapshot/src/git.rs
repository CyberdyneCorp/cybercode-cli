//! The shadow repository and the git commands run against it.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use cyber_server::runtime::{FileDiff, Snapshot};
use sha1::{Digest, Sha1};

/// A shadow git directory bound to one worktree and scoped to one Location.
#[derive(Debug, Clone)]
pub(crate) struct Shadow {
    pub git_dir: PathBuf,
    pub worktree: PathBuf,
    /// The user's common git directory (objects, `info/exclude`).
    common_dir: PathBuf,
    /// The Location relative to the worktree, as a pathspec (`.` for the root).
    pathspec: String,
}

/// Environment variables that would redirect git away from the shadow repository.
const GIT_ENV: &[&str] = &[
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_COMMON_DIR",
    "GIT_NAMESPACE",
];

impl Shadow {
    pub fn locate(root: &Path, location: &Path) -> Option<Self> {
        let project = cyber_core::project::identify(location);
        let common_dir = project.common_dir?;
        let worktree = std::fs::canonicalize(project.worktree).ok()?;
        let digest = Sha1::digest(worktree.to_string_lossy().as_bytes());
        let hex: String = digest.iter().take(8).map(|b| format!("{b:02x}")).collect();
        let location = std::fs::canonicalize(location).ok()?;
        let rel = location
            .strip_prefix(&worktree)
            .ok()?
            .to_string_lossy()
            .replace('\\', "/");
        Some(Self {
            git_dir: root.join(&project.id).join(hex),
            worktree,
            common_dir,
            pathspec: if rel.is_empty() { ".".into() } else { rel },
        })
    }

    async fn run(&self, args: &[&str]) -> Result<Vec<u8>, String> {
        let out = self
            .command(args)
            .output()
            .await
            .map_err(|e| format!("git: {e}"))?;
        if out.status.success() {
            Ok(out.stdout)
        } else {
            Err(format!(
                "git {}: {}",
                args.first().unwrap_or(&""),
                String::from_utf8_lossy(&out.stderr).trim()
            ))
        }
    }

    fn command(&self, args: &[&str]) -> tokio::process::Command {
        let mut cmd = tokio::process::Command::new("git");
        for name in GIT_ENV {
            cmd.env_remove(name);
        }
        cmd.env("GIT_DIR", &self.git_dir)
            .env("GIT_WORK_TREE", &self.worktree)
            .current_dir(&self.worktree)
            .args([
                "-c",
                "core.autocrlf=false",
                "-c",
                "core.quotepath=false",
                "-c",
                "core.fsmonitor=false",
            ])
            .args(args)
            .stdin(Stdio::null())
            .kill_on_drop(true);
        cmd
    }

    async fn ensure_init(&self) -> Result<(), String> {
        if self.git_dir.join("HEAD").exists() {
            return Ok(());
        }
        std::fs::create_dir_all(&self.git_dir).map_err(|e| e.to_string())?;
        self.run(&["init", "--quiet"]).await?;
        for (key, value) in [
            ("core.autocrlf", "false"),
            ("core.fsmonitor", "false"),
            ("gc.auto", "0"),
        ] {
            self.run(&["config", key, value]).await?;
        }
        let alternates = self.git_dir.join("objects/info/alternates");
        let objects = self.common_dir.join("objects");
        std::fs::write(alternates, format!("{}\n", objects.display())).map_err(|e| e.to_string())
    }

    pub async fn track(&self, ignore: &[String], max_bytes: u64) -> Result<Snapshot, String> {
        self.ensure_init().await?;
        let skipped = self.large_untracked(max_bytes).await;
        self.write_excludes(ignore, &skipped)?;
        // `--ignore-errors` keeps unreadable files from failing the whole snapshot.
        let _ = self
            .run(&["add", "--all", "--ignore-errors", "--", &self.pathspec])
            .await;
        let tree = String::from_utf8_lossy(&self.run(&["write-tree"]).await?)
            .trim()
            .to_string();
        Ok(Snapshot { tree, skipped })
    }

    /// Untracked files (per the user's repository) over the size limit.
    async fn large_untracked(&self, max_bytes: u64) -> Vec<String> {
        let mut cmd = tokio::process::Command::new("git");
        for name in GIT_ENV {
            cmd.env_remove(name);
        }
        let out = cmd
            .arg("-C")
            .arg(&self.worktree)
            .args([
                "ls-files",
                "--others",
                "--exclude-standard",
                "-z",
                "--",
                &self.pathspec,
            ])
            .output()
            .await;
        let Ok(out) = out else { return Vec::new() };
        out.stdout
            .split(|b| *b == 0)
            .filter(|p| !p.is_empty())
            .map(|p| String::from_utf8_lossy(p).to_string())
            .filter(|p| std::fs::metadata(self.worktree.join(p)).is_ok_and(|m| m.len() > max_bytes))
            .collect()
    }

    /// The user's `info/exclude`, `snapshots.ignore` and the skipped large files.
    fn write_excludes(&self, ignore: &[String], skipped: &[String]) -> Result<(), String> {
        let mut text =
            std::fs::read_to_string(self.common_dir.join("info/exclude")).unwrap_or_default();
        text.push('\n');
        for pattern in ignore {
            text.push_str(pattern);
            text.push('\n');
        }
        for path in skipped {
            text.push('/');
            text.push_str(&escape(path));
            text.push('\n');
        }
        let info = self.git_dir.join("info");
        std::fs::create_dir_all(&info).map_err(|e| e.to_string())?;
        std::fs::write(info.join("exclude"), text).map_err(|e| e.to_string())
    }

    pub async fn changed(&self, from: &str, to: &str) -> Result<Vec<String>, String> {
        let out = self
            .run(&[
                "diff",
                "--name-only",
                "--no-renames",
                "-z",
                from,
                to,
                "--",
                &self.pathspec,
            ])
            .await?;
        Ok(split_z(&out))
    }

    pub async fn diff(&self, from: &str, to: &str) -> Result<Vec<FileDiff>, String> {
        let status = split_z(
            &self
                .run(&[
                    "diff",
                    "--name-status",
                    "--no-renames",
                    "-z",
                    from,
                    to,
                    "--",
                    &self.pathspec,
                ])
                .await?,
        );
        let numstat = String::from_utf8_lossy(
            &self
                .run(&[
                    "diff",
                    "--numstat",
                    "--no-renames",
                    "-z",
                    from,
                    to,
                    "--",
                    &self.pathspec,
                ])
                .await?,
        )
        .to_string();
        let counts: Vec<(u32, u32)> = numstat
            .split('\0')
            .filter(|l| !l.is_empty())
            .map(|l| {
                let mut f = l.split('\t');
                (
                    f.next().and_then(|n| n.parse().ok()).unwrap_or(0),
                    f.next().and_then(|n| n.parse().ok()).unwrap_or(0),
                )
            })
            .collect();
        let mut out = Vec::new();
        for (i, pair) in status.chunks(2).enumerate() {
            let [code, file] = pair else { continue };
            let patch = self
                .run(&[
                    "diff",
                    "--no-ext-diff",
                    "--no-color",
                    "--no-renames",
                    from,
                    to,
                    "--",
                    file,
                ])
                .await?;
            let (additions, deletions) = counts.get(i).copied().unwrap_or_default();
            let status = match code.as_str() {
                "A" => "added",
                "D" => "deleted",
                _ => "modified",
            };
            out.push(FileDiff {
                file: file.clone(),
                status: status.into(),
                additions,
                deletions,
                patch: String::from_utf8_lossy(&patch).into(),
            });
        }
        Ok(out)
    }

    /// `(mode, contents)` of a path in a tree, or `None` when absent.
    pub async fn blob(&self, tree: &str, path: &str) -> Result<Option<(String, Vec<u8>)>, String> {
        let listing = self.run(&["ls-tree", "-z", tree, "--", path]).await?;
        let entry = String::from_utf8_lossy(&listing).to_string();
        let Some(meta) = entry.split('\t').next().filter(|m| !m.is_empty()) else {
            return Ok(None);
        };
        let fields: Vec<&str> = meta.split(' ').collect();
        let [mode, kind, sha] = fields[..] else {
            return Ok(None);
        };
        if kind != "blob" {
            return Ok(None);
        }
        Ok(Some((
            mode.to_string(),
            self.run(&["cat-file", "blob", sha]).await?,
        )))
    }
}

/// Shadow git directories under the snapshot root (`<root>/<project>/<worktree-hash>`).
pub(crate) fn shadow_dirs(root: &Path) -> Vec<PathBuf> {
    let children = |dir: &Path| -> Vec<PathBuf> {
        std::fs::read_dir(dir)
            .map(|it| {
                it.flatten()
                    .map(|e| e.path())
                    .filter(|p| p.is_dir())
                    .collect()
            })
            .unwrap_or_default()
    };
    children(root)
        .iter()
        .filter(|p| p.file_name().is_some_and(|n| n != "backups"))
        .flat_map(|p| children(p))
        .filter(|d| d.join("HEAD").exists())
        .collect()
}

pub(crate) async fn gc(git_dir: &Path) {
    let mut cmd = tokio::process::Command::new("git");
    for name in GIT_ENV {
        cmd.env_remove(name);
    }
    let _ = cmd
        .env("GIT_DIR", git_dir)
        .args(["gc", "--prune=7.days", "--quiet"])
        .stdin(Stdio::null())
        .output()
        .await;
}

fn split_z(out: &[u8]) -> Vec<String> {
    out.split(|b| *b == 0)
        .filter(|p| !p.is_empty())
        .map(|p| String::from_utf8_lossy(p).to_string())
        .collect()
}

/// Escape gitignore metacharacters so a path matches only itself.
fn escape(path: &str) -> String {
    path.chars().fold(String::new(), |mut s, c| {
        if matches!(c, '*' | '?' | '[' | '\\' | '!' | '#') {
            s.push('\\');
        }
        s.push(c);
        s
    })
}

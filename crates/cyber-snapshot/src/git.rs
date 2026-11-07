//! The shadow repository and the git commands run against it.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};

use cyber_server::runtime::{FileDiff, Snapshot};
use sha1::{Digest, Sha1};
mod catalog;
#[cfg(any(target_os = "macos", target_os = "linux"))]
mod config;
#[path = "git_history.rs"]
pub(crate) mod history;
mod images;
#[cfg(any(target_os = "macos", target_os = "linux"))]
pub(crate) mod prepared;
#[cfg(any(target_os = "macos", target_os = "linux"))]
mod raw;

/// A shadow git directory bound to one worktree and scoped to one Location.
#[derive(Debug, Clone)]
pub(crate) struct Shadow {
    pub git_dir: PathBuf,
    pub worktree: PathBuf,
    /// The user's common git directory (objects, `info/exclude`).
    common_dir: PathBuf,
    /// The Location relative to the worktree, as a pathspec (`.` for the root).
    pathspec: String,
    catalogs: Arc<Mutex<catalog::CatalogCache>>,
    pub private_roots: Vec<PathBuf>,
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    pub capture_settings: prepared::CaptureSettings,
}

/// Environment variables that would redirect git away from the shadow repository.
pub(crate) const GIT_ENV: &[&str] = &[
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_COMMON_DIR",
    "GIT_NAMESPACE",
    "GIT_LITERAL_PATHSPECS",
    "GIT_GLOB_PATHSPECS",
    "GIT_NOGLOB_PATHSPECS",
    "GIT_ICASE_PATHSPECS",
];

/// Git for Windows does not accept verbatim (`\\?\`) paths as `GIT_DIR`, work tree or
/// alternates, and `canonicalize` always returns them; use the equivalent plain form.
/// The worktree hash keeps using the canonical spelling, so shadow directories are stable.
#[cfg(windows)]
pub(crate) fn git_path(path: &Path) -> PathBuf {
    let Some(text) = path.to_str() else {
        return path.to_path_buf();
    };
    if let Some(unc) = text.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{unc}"));
    }
    match text.strip_prefix(r"\\?\") {
        Some(rest) if rest.as_bytes().get(1) == Some(&b':') => PathBuf::from(rest),
        _ => path.to_path_buf(),
    }
}

#[cfg(not(windows))]
pub(crate) fn git_path(path: &Path) -> PathBuf {
    path.to_path_buf()
}

/// A proposed index entry; `None` removes the path. Used only by read-only restore planning.
pub(crate) struct TreeChange {
    pub path: String,
    pub image: Option<(String, Vec<u8>)>,
}
struct PreviewIndex(PathBuf);
impl Drop for PreviewIndex {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
        let _ = std::fs::remove_file(self.0.with_extension("index.lock"));
    }
}

/// A marker only prevents fallback admission. It grants no Git/source authority
/// and creates no project identity cache when library discovery is unsupported.
pub(crate) fn repository_marker(location: &Path) -> bool {
    let Ok(location) = std::fs::canonicalize(location) else {
        return true;
    };
    location.ancestors().any(|parent| {
        !matches!(std::fs::symlink_metadata(parent.join(".git")),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound)
    })
}

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
            .components()
            .map(|part| part.as_os_str().to_str())
            .collect::<Option<Vec<_>>>()?
            .join("/");
        Some(Self {
            git_dir: git_path(&root.join(&project.id).join(hex)),
            worktree: git_path(&worktree),
            common_dir: git_path(&common_dir),
            pathspec: if rel.is_empty() { ".".into() } else { rel },
            catalogs: Arc::new(Mutex::new(catalog::CatalogCache::new())),
            private_roots: vec![root.parent().unwrap_or(root).to_path_buf()],
            #[cfg(any(target_os = "macos", target_os = "linux"))]
            capture_settings: prepared::CaptureSettings {
                ignore: vec![],
                max_untracked: 2 * 1024 * 1024,
            },
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
                "--literal-pathspecs",
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
        let mut source = self.clone();
        source.git_dir = self.common_dir.clone();
        let format = source.object_format().await?;
        if self.git_dir.join("HEAD").exists() {
            if self.object_format().await? != format {
                return Err("Git snapshot shadow object format differs from its repository".into());
            }
            return Ok(());
        }
        std::fs::create_dir_all(&self.git_dir).map_err(|e| e.to_string())?;
        self.run(&["init", "--quiet", &format!("--object-format={format}")])
            .await?;
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

    async fn object_format(&self) -> Result<&'static str, String> {
        match self
            .run_bounded(&["rev-parse", "--show-object-format=storage"], 16)
            .await?
            .as_slice()
        {
            b"sha1\n" => Ok("sha1"),
            b"sha256\n" => Ok("sha256"),
            _ => Err("Unsupported Git snapshot repository object format".into()),
        }
    }

    pub async fn track(&self, ignore: &[String], max_bytes: u64) -> Result<Snapshot, String> {
        self.ensure_init().await?;
        let skipped = self.large_untracked(max_bytes).await;
        self.write_excludes(ignore, &skipped)?;
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        let capture_before = self.capture_git_state(ignore, max_bytes).await;
        let excluded = self.omitted().await?;
        let cached_paths: Vec<_> = excluded
            .iter()
            .filter(|p| !p.ends_with('/'))
            .cloned()
            .collect();
        if !cached_paths.is_empty() {
            let mut names = cached_paths.join("\0").into_bytes();
            names.push(0);
            self.input(&["update-index", "--force-remove", "-z", "--stdin"], &names)
                .await?;
        }
        // An unreadable/failed capture cannot silently become restore authority.
        self.run(&["add", "--all", "--", &self.pathspec]).await?;
        let tree = String::from_utf8_lossy(&self.run(&["write-tree"]).await?)
            .trim()
            .to_string();
        let skipped = excluded.clone();
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        let (policy, raw, skipped) = match capture_before {
            Ok(before) => {
                let after = self.capture_git_state(ignore, max_bytes).await?;
                if before != after {
                    return Err("Git policy or raw state changed during checkpoint capture".into());
                }
                let raw = self.write_raw_capture(&after.files).await?;
                (Some(after.policy), Some(raw), skipped)
            }
            Err(_) => {
                let mut skipped = skipped;
                skipped
                    .push("native Git policy/raw-state proof unavailable for this capture".into());
                (None, None, skipped)
            }
        };
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        let (policy, raw) = (None, None);
        let tree = self.record_snapshot(&tree, excluded, policy, raw).await?;
        Ok(Snapshot { tree, skipped })
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    async fn capture_git_state(
        &self,
        ignore: &[String],
        max_untracked: u64,
    ) -> Result<crate::copy::git_capture::Capture, String> {
        let mut inputs = self.ignore_inputs().await?;
        inputs.settings = prepared::CaptureSettings {
            ignore: ignore.to_vec(),
            max_untracked,
        };
        let worktree = self.worktree.clone();
        let prefix = if self.pathspec == "." {
            String::new()
        } else {
            self.pathspec.clone()
        };
        tokio::task::spawn_blocking(move || {
            crate::copy::git_rules::capture_state(&worktree, &prefix, &inputs)
        })
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| format!("{error:?}"))
    }

    /// Untracked files (per the user's repository) over the size limit.
    async fn large_untracked(&self, max_bytes: u64) -> Vec<String> {
        let mut cmd = tokio::process::Command::new("git");
        for name in GIT_ENV {
            cmd.env_remove(name);
        }
        let out = cmd
            .arg("--literal-pathspecs")
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
        let excluded = self.excluded_between(from, to).await?;
        let before = self.image_catalog(from).await?;
        let after = self.image_catalog(to).await?;
        let names: std::collections::BTreeSet<_> =
            before.keys().chain(after.keys()).cloned().collect();
        Ok(names
            .into_iter()
            .filter(|path| {
                before.get(path) != after.get(path) && !history::excludes(&excluded, path)
            })
            .collect())
    }

    pub async fn diff(&self, from: &str, to: &str) -> Result<Vec<FileDiff>, String> {
        self.image_catalog(from).await?;
        self.image_catalog(to).await?;
        let excluded = self.excluded_between(from, to).await?;
        let from = self.snapshot_metadata(from).await?.0;
        let to = self.snapshot_metadata(to).await?.0;
        let status = split_z(
            &self
                .run(&[
                    "diff",
                    "--name-status",
                    "--no-renames",
                    "-z",
                    &from,
                    &to,
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
                    &from,
                    &to,
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
            if history::excludes(&excluded, file) {
                continue;
            }
            let patch = self
                .run(&[
                    "diff",
                    "--no-ext-diff",
                    "--no-color",
                    "--no-renames",
                    &from,
                    &to,
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
                binary: None,
                file: file.clone(),
                status: status.into(),
                additions,
                deletions,
                patch: String::from_utf8_lossy(&patch).into(),
            });
        }
        Ok(out)
    }

    /// Construct a proposed tree in a separate index. Neither the source files nor
    /// the live shadow index are changed, including when planning is cancelled.
    pub(crate) async fn projected_tree(
        &self,
        current: &str,
        changes: Vec<TreeChange>,
    ) -> Result<String, String> {
        use tokio::io::AsyncWriteExt;
        let current = self.snapshot_metadata(current).await?.0;
        let index = PreviewIndex(
            self.git_dir
                .join(format!("preview-{}.index", ulid::Ulid::new())),
        );
        self.index_command(&index.0, &["read-tree", &current])
            .await?;
        for change in changes {
            match change.image {
                None => {
                    self.index_command(
                        &index.0,
                        &["update-index", "--force-remove", "--", &change.path],
                    )
                    .await?;
                }
                Some((mode, bytes)) => {
                    let mut child = self
                        .command(&["hash-object", "-w", "--stdin"])
                        .stdin(Stdio::piped())
                        .stdout(Stdio::piped())
                        .stderr(Stdio::piped())
                        .spawn()
                        .map_err(|error| error.to_string())?;
                    let mut input = child.stdin.take().ok_or("Missing Git object input")?;
                    input
                        .write_all(&bytes)
                        .await
                        .map_err(|error| error.to_string())?;
                    drop(input);
                    let output = child
                        .wait_with_output()
                        .await
                        .map_err(|error| error.to_string())?;
                    if !output.status.success() {
                        return Err("Cannot prepare proposed snapshot object".into());
                    }
                    let object = String::from_utf8_lossy(&output.stdout).trim().to_owned();
                    self.index_command(
                        &index.0,
                        &[
                            "update-index",
                            "--add",
                            "--cacheinfo",
                            &mode,
                            &object,
                            &change.path,
                        ],
                    )
                    .await?;
                }
            }
        }
        let tree = self.index_command(&index.0, &["write-tree"]).await?;
        Ok(String::from_utf8_lossy(&tree).trim().to_owned())
    }

    async fn index_command(&self, index: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
        let output = self
            .command(args)
            .env("GIT_INDEX_FILE", index)
            .output()
            .await
            .map_err(|error| error.to_string())?;
        if output.status.success() {
            Ok(output.stdout)
        } else {
            Err(format!(
                "Proposed snapshot index: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ))
        }
    }

    /// `(mode, contents)` of a path in a tree, or `None` when absent.
    pub async fn blob(&self, tree: &str, path: &str) -> Result<Option<(String, Vec<u8>)>, String> {
        images::validate_path(path)?;
        let catalog = self.image_catalog(tree).await?;
        let Some((mode, object)) = catalog.get(path) else {
            return Ok(None);
        };
        if !matches!(mode.as_str(), "100644" | "100755" | "120000") {
            return Err("Unsupported Git image entry".into());
        }
        let bytes = self.run(&["cat-file", "blob", object]).await?;
        images::verify(object, &bytes)?;
        Ok(Some((mode.clone(), bytes)))
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
        .env("GIT_DIR", git_path(git_dir))
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

#[cfg(all(test, windows))]
mod tests {
    use super::git_path;
    use std::path::Path;

    #[test]
    fn verbatim_paths_reach_git_in_plain_form() {
        for (verbatim, plain) in [
            (r"\\?\C:\data\snapshot", r"C:\data\snapshot"),
            (r"\\?\UNC\server\share\repo", r"\\server\share\repo"),
            (r"C:\already\plain", r"C:\already\plain"),
            (r"\\?\Volume{0}\x", r"\\?\Volume{0}\x"),
        ] {
            assert_eq!(git_path(Path::new(verbatim)), Path::new(plain));
        }
    }
}

//! Project identity from git (`storage-events` → Project identity from git).

use std::path::{Path, PathBuf};
use std::process::Command;

use sha2::{Digest, Sha256};

/// The project a Location belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    /// `prj_<hex>`, or `global` outside git.
    pub id: String,
    /// The worktree root; `/` outside git.
    pub worktree: PathBuf,
    /// The repository's common git directory, shared by all worktrees.
    pub common_dir: Option<PathBuf>,
}

const CACHE_FILE: &str = "cyber-project-id";

/// Find a worktree without running Git or writing project identity caches.
pub fn worktree_root(location: &Path) -> Option<PathBuf> {
    git2::Repository::discover(location)
        .ok()?
        .workdir()
        .map(Into::into)
}

pub fn identify(location: &Path) -> Project {
    let Some(worktree) = git(location, &["rev-parse", "--show-toplevel"]) else {
        return Project {
            id: "global".into(),
            worktree: PathBuf::from("/"),
            common_dir: None,
        };
    };
    let worktree = PathBuf::from(worktree);
    let common_dir = git(
        location,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )
    .map(PathBuf::from);
    let id = from_origin(location)
        .or_else(|| common_dir.as_deref().and_then(cached))
        .or_else(|| from_root_commit(location))
        .unwrap_or_else(|| generated(common_dir.as_deref()));
    Project {
        id,
        worktree,
        common_dir,
    }
}

fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .output()
        .ok()?;
    let text = String::from_utf8(out.stdout).ok()?.trim().to_string();
    (out.status.success() && !text.is_empty()).then_some(text)
}

fn hashed(text: &str) -> String {
    let digest = Sha256::digest(text.as_bytes());
    let hex: String = digest.iter().take(8).map(|b| format!("{b:02x}")).collect();
    format!("prj_{hex}")
}

fn from_origin(location: &Path) -> Option<String> {
    git(location, &["config", "--get", "remote.origin.url"])
        .map(|url| hashed(&normalize_remote(&url)))
}

/// Strip scheme, credentials and `.git`; lowercase the host. `git@host:a/b` and
/// `https://user@host/a/b.git` both become `host/a/b`.
pub fn normalize_remote(url: &str) -> String {
    let (has_scheme, rest) = match url.split_once("://") {
        Some((_, rest)) => (true, rest),
        None => (false, url),
    };
    let rest = rest.rsplit_once('@').map_or(rest, |(_, r)| r);
    // URLs separate host and path with `/` (the host may carry a port); scp-style uses `:`.
    let (host, path) = if has_scheme {
        rest.split_once('/').unwrap_or((rest, ""))
    } else {
        rest.split_once(':').unwrap_or((rest, ""))
    };
    let host = host.split(':').next().unwrap_or(host).to_ascii_lowercase();
    let path = path.trim_matches('/');
    format!("{host}/{}", path.strip_suffix(".git").unwrap_or(path))
}

fn cached(common_dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(common_dir.join(CACHE_FILE)).ok()?;
    let id = text.trim();
    (id.starts_with("prj_") && id.len() > 4).then(|| id.to_string())
}

fn from_root_commit(location: &Path) -> Option<String> {
    let roots = git(location, &["rev-list", "--max-parents=0", "HEAD"])?;
    let first = roots.lines().min()?;
    Some(format!("prj_{}", &first[..first.len().min(16)]))
}

/// A repository with no origin and no commits gets a random ID, cached for later runs.
fn generated(common_dir: Option<&Path>) -> String {
    let id = crate::ids::new_id("prj");
    if let Some(dir) = common_dir {
        let _ = std::fs::write(dir.join(CACHE_FILE), format!("{id}\n"));
    }
    id
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remotes_normalize_across_protocols() {
        let expected = "github.com/acme/app";
        assert_eq!(normalize_remote("git@github.com:acme/app.git"), expected);
        assert_eq!(
            normalize_remote("https://token@GitHub.com/acme/app.git"),
            expected
        );
        assert_eq!(
            normalize_remote("ssh://git@github.com:22/acme/app"),
            expected
        );
    }

    #[test]
    fn worktrees_of_one_repository_share_an_id() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let run = |dir: &Path, args: &[&str]| {
            let ok = Command::new("git")
                .arg("-C")
                .arg(dir)
                .args(args)
                .output()
                .unwrap()
                .status
                .success();
            assert!(ok, "git {args:?}");
        };
        run(&repo, &["init", "-q"]);
        run(
            &repo,
            &[
                "-c",
                "user.email=t@t",
                "-c",
                "user.name=t",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "root",
            ],
        );
        run(&repo, &["worktree", "add", "-q", "../wt"]);
        let main = identify(&repo);
        let other = identify(&tmp.path().join("wt"));
        assert!(main.id.starts_with("prj_"));
        assert_eq!(main.id, other.id);
        assert_ne!(main.worktree, other.worktree);
        let outside = identify(tmp.path());
        assert_eq!(
            (outside.id.as_str(), outside.worktree.as_path()),
            ("global", Path::new("/"))
        );
    }
}

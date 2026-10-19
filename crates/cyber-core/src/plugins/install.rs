//! Plugin sources (`plugins-marketplace` → Plugin sources): a local `path` used in place,
//! a `git` repository checked out at a resolved commit, or an `npm` package packed with
//! lifecycle scripts disabled; fetched trees are copied to the content-addressed
//! `<cache>/plugins/<name>/<version>-<digest>/`, so a fetch never replaces a tree that is
//! already installed (and trusted) somewhere. Nothing a plugin ships runs while it is fetched:
//! the clone writes links as plain files and fetches no submodules.

use std::path::{Path, PathBuf};
use std::process::Command;

use super::manifest::Manifest;
use super::registry::{Locations, PluginScope, Source};

/// A fetched plugin: where its files are and what they are.
#[derive(Debug, Clone)]
pub struct Fetched {
    pub root: PathBuf,
    pub manifest: Manifest,
    pub digest: String,
    pub commit: Option<String>,
    /// This fetch created `root` in the cache; [`super::Manager::discard`] removes it
    /// again when the change is not committed.
    pub fresh: bool,
}

/// The source an install spec names: `git+<url>[#ref]` or a URL a git remote takes,
/// `npm:<package spec>`, or a local directory. `<plugin>@<marketplace>` specs are refused
/// until marketplaces exist.
pub fn parse_spec(
    spec: &str,
    reference: Option<&str>,
    subdir: Option<&str>,
    cwd: &Path,
) -> Result<Source, String> {
    if let Some(package) = spec.strip_prefix("npm:") {
        if reference.is_some() || subdir.is_some() {
            return Err("--ref and --path apply only to git sources".into());
        }
        return npm_source(package, cwd);
    }
    if let Some(url) = git_url(spec) {
        let (url, fragment) = match url.rsplit_once('#') {
            Some((url, fragment)) if !fragment.is_empty() => (url, Some(fragment)),
            _ => (url, None),
        };
        if let Some(path) = subdir.filter(|path| super::manifest::inside(path).is_none()) {
            return Err(format!(
                "--path {path:?}: expected a relative path inside the repository"
            ));
        }
        return Ok(Source::Git {
            url: url.to_owned(),
            reference: reference.or(fragment).map(str::to_owned),
            path: subdir.map(str::to_owned),
        });
    }
    let path = cwd.join(spec);
    if path.is_dir() {
        if reference.is_some() || subdir.is_some() {
            return Err("--ref and --path apply only to git sources".into());
        }
        let path = std::fs::canonicalize(&path).map_err(|e| format!("{spec}: {e}"))?;
        return Ok(Source::Path {
            path: path.display().to_string(),
        });
    }
    if spec.contains('@') && !spec.contains(['/', '\\']) {
        return Err(format!(
            "{spec}: marketplace plugin sources are not supported yet; use a git URL or a path"
        ));
    }
    Err(format!("{spec}: no such plugin directory or git URL"))
}

/// A local package directory or tarball is recorded as an absolute path; anything else
/// is a registry spec for npm to resolve.
fn npm_source(package: &str, cwd: &Path) -> Result<Source, String> {
    if package.is_empty() || package.starts_with('-') {
        return Err(format!("npm:{package}: expected a package spec"));
    }
    let local = cwd.join(package);
    let looks_local = package.starts_with(['.', '/', '\\', '~']) || package.ends_with(".tgz");
    if looks_local || (local.exists() && !package.starts_with('@')) {
        let path = std::fs::canonicalize(&local).map_err(|e| format!("npm:{package}: {e}"))?;
        return Ok(Source::Npm {
            package: path.display().to_string(),
        });
    }
    Ok(Source::Npm {
        package: package.to_owned(),
    })
}

fn git_url(spec: &str) -> Option<&str> {
    if let Some(url) = spec.strip_prefix("git+") {
        return Some(url);
    }
    let remote = ["https://", "http://", "ssh://", "git://", "file://", "git@"]
        .iter()
        .any(|prefix| spec.starts_with(prefix));
    (remote || spec.ends_with(".git")).then_some(spec)
}

/// Fetch `source`. A git source checks out `commit` when given (the lockfile pin), else
/// its `ref`, else the default branch, and is copied into the cache.
pub fn fetch(
    locations: &Locations,
    scope: PluginScope,
    source: &Source,
    commit: Option<&str>,
) -> Result<Fetched, String> {
    match source {
        Source::Path { path } => {
            let root = locations.path_root(scope, path);
            if !root.is_dir() {
                return Err(format!("{}: no such plugin directory", root.display()));
            }
            inspect(root, None)
        }
        Source::Git {
            url,
            reference,
            path,
        } => fetch_git(
            locations,
            url,
            commit.or(reference.as_deref()),
            path.as_deref(),
        ),
        Source::Npm { package } => staged(locations, |staging| {
            let tree = super::npm::fetch(staging, package)?;
            cache_copy(locations, tree, None)
        }),
    }
}

/// Run `fetch` with a fresh staging directory under the cache, removed afterwards.
fn staged(
    locations: &Locations,
    fetch: impl FnOnce(&Path) -> Result<Fetched, String>,
) -> Result<Fetched, String> {
    std::fs::create_dir_all(&locations.cache_dir)
        .map_err(|e| format!("{}: {e}", locations.cache_dir.display()))?;
    let staging = locations.cache_dir.join(format!(
        ".fetch-{}",
        ulid::Ulid::new().to_string().to_lowercase()
    ));
    let result = fetch(&staging);
    let _ = std::fs::remove_dir_all(&staging);
    result
}

/// Copy a fetched tree to its content-addressed cache directory and check the copy. A
/// directory that already holds exactly this content is reused untouched.
fn cache_copy(
    locations: &Locations,
    tree: PathBuf,
    commit: Option<String>,
) -> Result<Fetched, String> {
    let fetched = inspect(tree, commit)?;
    let cached = locations.cache(
        &fetched.manifest.name,
        &fetched.manifest.version,
        &fetched.digest,
    );
    if cached.is_dir()
        && let Ok(existing) = inspect(cached.clone(), fetched.commit.clone())
        && existing.digest == fetched.digest
    {
        return Ok(existing);
    }
    copy_tree(&fetched.root, &cached)?;
    let copy = inspect(cached, fetched.commit)?;
    if copy.digest != fetched.digest {
        return Err("the cached copy does not match the fetched plugin".into());
    }
    Ok(Fetched {
        fresh: true,
        ..copy
    })
}

fn inspect(root: PathBuf, commit: Option<String>) -> Result<Fetched, String> {
    let manifest = Manifest::read(&root).map_err(|error| error.to_string())?;
    let digest = super::digest::tree(&root)?;
    Ok(Fetched {
        root,
        manifest,
        digest,
        commit,
        fresh: false,
    })
}

fn fetch_git(
    locations: &Locations,
    url: &str,
    target: Option<&str>,
    subdir: Option<&str>,
) -> Result<Fetched, String> {
    staged(locations, |staging| {
        let commit = checkout(staging, url, target)?;
        let tree = match subdir {
            Some(path) => staging.join(super::manifest::inside(path).ok_or("invalid --path")?),
            None => staging.to_path_buf(),
        };
        cache_copy(locations, tree, Some(commit))
    })
}

/// `path` without a `\\?\` drive prefix, the form git and npm accept on Windows.
pub(super) fn plain(path: &Path) -> PathBuf {
    let text = path.display().to_string();
    match text.strip_prefix(r"\\?\") {
        Some(rest) if rest.as_bytes().get(1) == Some(&b':') => PathBuf::from(rest),
        _ => path.to_path_buf(),
    }
}

/// Clone `url` into `dir` and check out `target`; returns the resolved commit.
fn checkout(dir: &Path, url: &str, target: Option<&str>) -> Result<String, String> {
    let dir = &plain(dir);
    let dir_text = dir.display().to_string();
    git(
        None,
        &["clone", "--quiet", "--no-checkout", "--", url, &dir_text],
    )?;
    let commit = match target {
        Some(target) => resolve(dir, target)?,
        None => resolve(dir, "HEAD")?,
    };
    git(Some(dir), &["checkout", "--quiet", "--detach", &commit])?;
    Ok(commit)
}

/// A branch (as `origin/<name>`), tag or commit, as a full commit id.
fn resolve(dir: &Path, target: &str) -> Result<String, String> {
    let dir = &plain(dir);
    if target.starts_with('-') {
        return Err(format!("{target}: invalid git ref"));
    }
    for candidate in [format!("origin/{target}"), target.to_owned()] {
        let spec = format!("{candidate}^{{commit}}");
        if let Ok(commit) = git(Some(dir), &["rev-parse", "--verify", "--quiet", &spec]) {
            return Ok(commit.trim().to_owned());
        }
    }
    Err(format!("git ref {target:?} was not found"))
}

fn git(dir: Option<&Path>, args: &[&str]) -> Result<String, String> {
    let mut command = Command::new("git");
    if let Some(dir) = dir {
        command.arg("-C").arg(dir);
    }
    let output = command
        .args(["-c", "core.symlinks=false", "-c", "submodule.recurse=false"])
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .map_err(|e| format!("cannot run git: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("git {}: {}", args[0], stderr.trim()));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Replace `target` with a copy of `source`'s files, staged beside it.
fn copy_tree(source: &Path, target: &Path) -> Result<(), String> {
    let parent = target.parent().ok_or("cache path has no parent")?;
    std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    let staged = parent.join(format!(
        ".stage-{}",
        ulid::Ulid::new().to_string().to_lowercase()
    ));
    let copied = (|| {
        for (relative, path) in super::digest::files(source)? {
            let destination = relative
                .split('/')
                .fold(staged.clone(), |dir, part| dir.join(part));
            if let Some(dir) = destination.parent() {
                std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            }
            std::fs::copy(&path, &destination)
                .map_err(|e| format!("{}: {e}", destination.display()))?;
        }
        if target.exists() {
            std::fs::remove_dir_all(target).map_err(|e| format!("{}: {e}", target.display()))?;
        }
        std::fs::rename(&staged, target).map_err(|e| format!("{}: {e}", target.display()))
    })();
    if copied.is_err() {
        let _ = std::fs::remove_dir_all(&staged);
    }
    copied
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn specs_name_git_or_path_sources_and_refuse_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("kit")).unwrap();
        let git = parse_spec(
            "git+https://example.test/kit.git#v1",
            None,
            Some("sub"),
            dir.path(),
        )
        .unwrap();
        assert_eq!(
            git,
            Source::Git {
                url: "https://example.test/kit.git".into(),
                reference: Some("v1".into()),
                path: Some("sub".into())
            }
        );
        let path = parse_spec("kit", None, None, dir.path()).unwrap();
        assert!(matches!(path, Source::Path { .. }));
        assert_eq!(
            parse_spec("npm:@acme/kit@^1", None, None, dir.path()).unwrap(),
            Source::Npm {
                package: "@acme/kit@^1".into()
            }
        );
        let local = parse_spec("npm:./kit", None, None, dir.path()).unwrap();
        assert!(matches!(local, Source::Npm { package } if Path::new(&package).is_absolute()));
        for (spec, message) in [
            ("npm:-x", "package spec"),
            ("pr-tools@cyber-official", "marketplace"),
            ("missing", "no such plugin"),
        ] {
            let error = parse_spec(spec, None, None, dir.path()).unwrap_err();
            assert!(error.contains(message), "{spec}: {error}");
        }
        assert!(parse_spec("git+x", None, Some("../up"), dir.path()).is_err());
    }
}

//! Windows shell resolution for `bash` (`builtin-tools` → bash tool): a configured name
//! resolves to a real POSIX shell, never to the WSL launcher or a checkout-planted binary.
//! The search is pure over its inputs so it is tested on every platform.
use std::path::{Path, PathBuf};

#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) const NOT_FOUND: &str = "No POSIX shell found: install Git for Windows or set \"shell\" to the absolute path of bash.exe";

/// Where a shell name is looked up.
pub(crate) struct Search {
    /// `PATH` entries, in order.
    pub path: Vec<PathBuf>,
    /// Git for Windows install roots (`<root>\bin\bash.exe`).
    pub git_roots: Vec<PathBuf>,
    /// Directories holding the WSL launcher (`System32`, `WindowsApps`): never chosen by name.
    pub launchers: Vec<PathBuf>,
}

impl Search {
    #[cfg_attr(not(windows), allow(dead_code))]
    pub(crate) fn from_env() -> Self {
        let var = |key: &str| std::env::var_os(key).map(PathBuf::from);
        let path: Vec<PathBuf> = std::env::var_os("PATH")
            .map(|p| std::env::split_paths(&p).collect())
            .unwrap_or_default();
        let mut git_roots = Vec::new();
        for key in ["ProgramFiles", "ProgramW6432", "ProgramFiles(x86)"] {
            git_roots.extend(var(key).map(|p| p.join("Git")));
        }
        git_roots.extend(var("LOCALAPPDATA").map(|p| p.join("Programs").join("Git")));
        let mut launchers = Vec::new();
        if let Some(root) = var("SystemRoot") {
            launchers.push(root.join("System32"));
            launchers.push(root.join("Sysnative"));
        }
        launchers.extend(var("LOCALAPPDATA").map(|p| p.join("Microsoft").join("WindowsApps")));
        Self {
            path,
            git_roots,
            launchers,
        }
    }
}

/// An explicitly configured absolute path wins as is. Otherwise `name` (`bash`, `sh`, `zsh`,
/// `dash`) is looked up on `PATH`, then in Git for Windows installs (found from `git.exe` on
/// `PATH` or the standard roots), skipping the WSL launcher directories and anything inside
/// `checkout`.
pub(crate) fn resolve(
    configured: Option<&Path>,
    name: &str,
    search: &Search,
    checkout: &Path,
) -> Option<PathBuf> {
    if let Some(given) = configured.filter(|p| p.is_absolute() && p.is_file()) {
        return Some(given.to_path_buf());
    }
    let exe = format!("{name}.exe");
    let launchers: Vec<String> = search.launchers.iter().map(|d| key(d)).collect();
    let usable = |candidate: &PathBuf| {
        let path = crate::host::canonical(candidate);
        let parent = path.parent().map(key).unwrap_or_default();
        candidate.is_absolute()
            && path.is_file()
            && !path.starts_with(checkout)
            && !launchers.contains(&parent)
    };
    let on_path = search.path.iter().map(|dir| dir.join(&exe));
    let from_git = git_roots(search).into_iter().flat_map(|root| {
        [
            root.join("bin").join(&exe),
            root.join("usr").join("bin").join(&exe),
        ]
    });
    on_path
        .chain(from_git)
        .find(usable)
        .map(|p| launchable(&crate::host::canonical(&p)))
}

/// The path a shell is started with: the canonical path without the verbatim prefix
/// (`\\?\C:\…`, `\\?\UNC\host\share\…`) that `canonicalize` adds on Windows. Git for
/// Windows' `bin\bash.exe` is a launcher that runs `..\usr\bin\bash.exe` next to its own
/// path, and a verbatim path never resolves `..`, so started that way it finds no shell.
pub(crate) fn launchable(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    if let Some(share) = text.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{share}"));
    }
    match text.strip_prefix(r"\\?\") {
        Some(plain) => PathBuf::from(plain),
        None => path.to_path_buf(),
    }
}

/// Git roots derived from `git.exe` on `PATH` (`<root>\cmd`, `<root>\bin`,
/// `<root>\mingw64\bin`), then the standard install roots.
fn git_roots(search: &Search) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    for dir in search.path.iter().filter(|d| d.join("git.exe").is_file()) {
        let leaf = dir
            .file_name()
            .map(|n| n.to_string_lossy().to_ascii_lowercase());
        let root = match leaf.as_deref() {
            Some("cmd") | Some("bin") => dir.parent().map(|p| match p.file_name() {
                Some(n) if n.eq_ignore_ascii_case("mingw64") => {
                    p.parent().unwrap_or(p).to_path_buf()
                }
                _ => p.to_path_buf(),
            }),
            _ => None,
        };
        roots.extend(root);
    }
    roots.extend(search.git_roots.iter().cloned());
    roots
}

/// A case-insensitive comparison key for a canonical directory.
fn key(dir: &Path) -> String {
    crate::host::canonical(dir)
        .to_string_lossy()
        .trim_end_matches(['\\', '/'])
        .to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(path: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"").unwrap();
    }

    fn fixture() -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let checkout = tmp.path().join("repo");
        std::fs::create_dir_all(&checkout).unwrap();
        let checkout = crate::host::canonical(&checkout);
        (tmp, checkout)
    }

    #[test]
    fn the_wsl_launcher_is_never_chosen_by_name() {
        let (tmp, checkout) = fixture();
        let system32 = tmp.path().join("Windows/System32");
        touch(&system32.join("bash.exe"));
        let git = tmp.path().join("Program Files/Git");
        touch(&git.join("bin/bash.exe"));
        let search = Search {
            path: vec![system32.clone()],
            git_roots: vec![git.clone()],
            launchers: vec![system32],
        };
        let found = resolve(None, "bash", &search, &checkout).unwrap();
        assert_eq!(
            found,
            launchable(&crate::host::canonical(&git.join("bin/bash.exe")))
        );
    }

    #[test]
    fn git_on_path_locates_its_own_bash() {
        let (tmp, checkout) = fixture();
        let git = tmp.path().join("tools/Git");
        touch(&git.join("cmd/git.exe"));
        touch(&git.join("usr/bin/sh.exe"));
        let search = Search {
            path: vec![git.join("cmd")],
            git_roots: vec![],
            launchers: vec![],
        };
        assert_eq!(
            resolve(None, "sh", &search, &checkout).unwrap(),
            launchable(&crate::host::canonical(&git.join("usr/bin/sh.exe")))
        );
        let mingw = tmp.path().join("other/Git");
        touch(&mingw.join("mingw64/bin/git.exe"));
        touch(&mingw.join("bin/bash.exe"));
        let search = Search {
            path: vec![mingw.join("mingw64/bin")],
            git_roots: vec![],
            launchers: vec![],
        };
        assert_eq!(
            resolve(None, "bash", &search, &checkout).unwrap(),
            launchable(&crate::host::canonical(&mingw.join("bin/bash.exe")))
        );
    }

    #[test]
    fn checkout_planted_and_relative_shells_are_skipped() {
        let (tmp, checkout) = fixture();
        touch(&checkout.join("bin/bash.exe"));
        let search = Search {
            path: vec![checkout.join("bin"), PathBuf::from("relative")],
            git_roots: vec![],
            launchers: vec![],
        };
        assert_eq!(resolve(None, "bash", &search, &checkout), None);
        drop(tmp);
    }

    #[test]
    fn an_explicit_absolute_path_is_used_even_for_the_launcher() {
        let (tmp, checkout) = fixture();
        let wsl = tmp.path().join("Windows/System32/bash.exe");
        touch(&wsl);
        let search = Search {
            path: vec![],
            git_roots: vec![],
            launchers: vec![wsl.parent().unwrap().to_path_buf()],
        };
        assert_eq!(resolve(Some(&wsl), "bash", &search, &checkout), Some(wsl));
        assert_eq!(
            resolve(Some(Path::new("bash")), "bash", &search, &checkout),
            None
        );
    }

    /// Regression: on Windows every `bash` call failed with Git's launcher reporting
    /// `'\\?\C:\Program Files\Git\bin\..\usr\bin\bash.exe' not found`, because the
    /// resolved shell was started by its verbatim canonical path.
    #[test]
    fn a_shell_is_started_without_the_verbatim_prefix() {
        assert_eq!(
            launchable(Path::new(r"\\?\C:\Program Files\Git\bin\bash.exe")),
            PathBuf::from(r"C:\Program Files\Git\bin\bash.exe")
        );
        assert_eq!(
            launchable(Path::new(r"\\?\UNC\host\share\Git\bin\bash.exe")),
            PathBuf::from(r"\\host\share\Git\bin\bash.exe")
        );
        assert_eq!(
            launchable(Path::new("/usr/bin/bash")),
            PathBuf::from("/usr/bin/bash")
        );
    }
}

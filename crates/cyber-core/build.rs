use std::path::PathBuf;
use std::process::Command;

#[path = "build/git_watch.rs"]
mod git_watch;

/// Build variable and default of each release-source constant. The embedded name drops
/// the `CYBER_` prefix: `CYBER_RELEASE_BASE_URL` becomes `CYBER_BUILD_RELEASE_BASE_URL`.
const RELEASE_SOURCE: &[(&str, &str)] = &[
    (
        "CYBER_RELEASE_BASE_URL",
        "https://github.com/CyberdyneCorp/cybercode-cli/releases/download",
    ),
    (
        "CYBER_RELEASE_IDENTITY",
        "https://github.com/CyberdyneCorp/cybercode-cli/.github/workflows/release.yml@refs/tags/",
    ),
    (
        "CYBER_RELEASE_ISSUER",
        "https://token.actions.githubusercontent.com",
    ),
];

fn main() {
    let target = std::env::var("TARGET").unwrap_or_else(|_| "unknown".into());
    println!("cargo:rustc-env=CYBER_BUILD_TARGET={target}");

    let sha = std::env::var("CYBER_GIT_SHA")
        .ok()
        .or_else(git_sha)
        .unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=CYBER_BUILD_GIT_SHA={sha}");

    let (build, source) = build_id(&sha);
    println!("cargo:rustc-env=CYBER_BUILD_ID={build}");
    println!("cargo:rustc-env=CYBER_BUILD_ID_SOURCE={source}");

    let channel = std::env::var("CYBER_CHANNEL").unwrap_or_else(|_| "dev".into());
    println!("cargo:rustc-env=CYBER_BUILD_CHANNEL={channel}");

    // The release source (`installation-upgrade` → Version and channel constants): only
    // these build variables change where releases come from and whom they must be signed by.
    for (var, default) in RELEASE_SOURCE {
        let value = std::env::var(var)
            .ok()
            .filter(|v| !v.trim().is_empty())
            .unwrap_or_else(|| (*default).into());
        println!("cargo:rustc-env=CYBER_BUILD_{}={}", &var[6..], value.trim());
        println!("cargo:rerun-if-env-changed={var}");
    }

    // Rebuild when the commit changes: HEAD, the branch ref it points to, packed refs.
    for path in git_watch::watched(&git_paths()) {
        println!("cargo:rerun-if-changed={}", path.display());
    }
    println!("cargo:rerun-if-env-changed=CYBER_GIT_SHA");
    println!("cargo:rerun-if-env-changed=CYBER_CHANNEL");
    println!("cargo:rerun-if-env-changed=CYBER_BUILD_ID");
    println!("cargo:rerun-if-env-changed=SOURCE_DATE_EPOCH");
}

/// The build identity servers and clients compare: `CYBER_BUILD_ID` when set, else the
/// commit (`.dirty` when tracked files differ from it), else the build time
/// (`SOURCE_DATE_EPOCH` for reproducible builds). The source is reported for tests.
fn build_id(sha: &str) -> (String, &'static str) {
    if let Ok(id) = std::env::var("CYBER_BUILD_ID")
        && !id.trim().is_empty()
    {
        return (id.trim().to_string(), "env");
    }
    if sha != "unknown" {
        let dirty = git(&["status", "--porcelain", "--untracked-files=no"])
            .is_some_and(|out| !out.is_empty());
        let suffix = if dirty { ".dirty" } else { "" };
        return (format!("{sha}{suffix}"), "git");
    }
    let seconds = std::env::var("SOURCE_DATE_EPOCH")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs())
        });
    (format!("t{seconds:x}"), "time")
}

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn git_paths() -> Vec<(String, PathBuf)> {
    let mut refs = vec!["HEAD".to_string(), "packed-refs".to_string()];
    refs.extend(git(&["symbolic-ref", "-q", "HEAD"]));
    refs.into_iter()
        .filter_map(|r| {
            let path = git(&["rev-parse", "--git-path", &r])?;
            Some((r, PathBuf::from(path)))
        })
        .collect()
}

fn git_sha() -> Option<String> {
    let out = Command::new("git")
        .args(["rev-parse", "--short=7", "HEAD"])
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

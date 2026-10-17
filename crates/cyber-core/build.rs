use std::process::Command;

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

    // Rebuild when the commit changes: HEAD, the branch ref it points to, packed refs.
    for path in git_paths() {
        println!("cargo:rerun-if-changed={path}");
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

fn git_paths() -> Vec<String> {
    let mut refs = vec!["HEAD".to_string(), "packed-refs".to_string()];
    refs.extend(git(&["symbolic-ref", "-q", "HEAD"]));
    refs.iter()
        .filter_map(|r| git(&["rev-parse", "--git-path", r]))
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

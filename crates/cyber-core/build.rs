use std::process::Command;

fn main() {
    let target = std::env::var("TARGET").unwrap_or_else(|_| "unknown".into());
    println!("cargo:rustc-env=CYBER_BUILD_TARGET={target}");

    let sha = std::env::var("CYBER_GIT_SHA")
        .ok()
        .or_else(git_sha)
        .unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=CYBER_BUILD_GIT_SHA={sha}");

    let channel = std::env::var("CYBER_CHANNEL").unwrap_or_else(|_| "dev".into());
    println!("cargo:rustc-env=CYBER_BUILD_CHANNEL={channel}");

    // Rebuild when the commit changes: HEAD, the branch ref it points to, packed refs.
    for path in git_paths() {
        println!("cargo:rerun-if-changed={path}");
    }
    println!("cargo:rerun-if-env-changed=CYBER_GIT_SHA");
    println!("cargo:rerun-if-env-changed=CYBER_CHANNEL");
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

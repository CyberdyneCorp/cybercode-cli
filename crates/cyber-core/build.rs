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

    println!("cargo:rerun-if-env-changed=CYBER_GIT_SHA");
    println!("cargo:rerun-if-env-changed=CYBER_CHANNEL");
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

//! `scripts/install.sh` (`installation-upgrade` → Install script) run for real under `sh`
//! against a loopback release served through `CYBER_INSTALL_BASE_URL`, with fake archives
//! whose `cyber` is a shell script printing its version. The last test removes the
//! installation again with the real `cyber uninstall --force`.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde_json::Value;
use sha2::{Digest, Sha256};

#[path = "../../cyber-install/tests/support/stub_release.rs"]
mod stub_release;
use stub_release::{Reply, StubRelease};

const MARKER: &str = "# cyber installer: PATH";
const RECEIPT: &str = ".cyber-receipt";
const SCRIPT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../scripts/install.sh");

struct Env {
    _dir: tempfile::TempDir,
    root: PathBuf,
    home: PathBuf,
    stub: StubRelease,
    target: String,
}

impl Env {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let home = root.join("home");
        for dir in [&home, &root.join("tmp"), &root.join("empty-path")] {
            std::fs::create_dir_all(dir).unwrap();
        }
        let mut env = Self {
            _dir: dir,
            root,
            home,
            stub: StubRelease::start(),
            target: String::new(),
        };
        // The script detects the same target the build has on the hosts tests run on.
        let info = env.cyber(Path::new(env!("CARGO_BIN_EXE_cyber")), &["debug", "info"]);
        assert!(info.status.success(), "{}", text(&info.stderr));
        let info: Value = serde_json::from_slice(&info.stdout).unwrap();
        env.target = info["build"]["target"].as_str().unwrap().to_string();
        env
    }

    fn install_dir(&self) -> PathBuf {
        self.home.join(".cyber/bin")
    }

    /// `sh install.sh <args>` with a minimal environment: no cosign, no cyber on PATH.
    fn install(&self, args: &[&str]) -> Output {
        Command::new("sh")
            .arg(SCRIPT)
            .args(args)
            .env_clear()
            .env("HOME", &self.home)
            .env("SHELL", "/bin/bash")
            .env("PATH", "/usr/bin:/bin")
            .env("TMPDIR", self.root.join("tmp"))
            .env("CYBER_INSTALL_BASE_URL", &self.stub.base_url)
            .current_dir(&self.root)
            .stdin(Stdio::null())
            .output()
            .unwrap()
    }

    /// The real binary at `exe`, with every cyber directory under the temporary home.
    fn cyber(&self, exe: &Path, args: &[&str]) -> Output {
        Command::new(exe)
            .args(args)
            .args(["--format", "json"])
            .env_clear()
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", self.home.join(".config"))
            .env("XDG_DATA_HOME", self.home.join(".local/share"))
            .env("XDG_STATE_HOME", self.home.join(".local/state"))
            .env("XDG_CACHE_HOME", self.home.join(".cache"))
            .env("CYBER_INSTALL_DIR", self.install_dir())
            .env("PATH", self.root.join("empty-path"))
            .current_dir(&self.root)
            .stdin(Stdio::null())
            .output()
            .unwrap()
    }

    fn archive_name(&self, version: &str) -> String {
        format!("cyber-{version}-{}.tar.gz", self.target)
    }

    /// Serve release `version` with a fake archive; `sums_for` names the bytes the
    /// `SHA256SUMS` line is computed over (the archive itself for an honest release).
    fn publish(&self, version: &str, sums_for: Option<&[u8]>) {
        let archive = fake_archive(&fake_cyber(version, &self.target));
        let digest = hex(&Sha256::digest(sums_for.unwrap_or(&archive)));
        let name = self.archive_name(version);
        let sums = format!("{digest}  {name}\n");
        self.stub.route(
            &format!("/v{version}/SHA256SUMS"),
            vec![Reply::Ok(sums.into_bytes())],
        );
        self.stub
            .route(&format!("/v{version}/{name}"), vec![Reply::Ok(archive)]);
    }

    /// Put a fake cyber of `version` (and its helper) into `dir`.
    fn preinstall(&self, dir: &Path, version: &str) -> Vec<u8> {
        let cyber = fake_cyber(version, &self.target);
        std::fs::create_dir_all(dir).unwrap();
        write_exe(&dir.join("cyber"), &cyber);
        write_exe(&dir.join("cyber-sandbox-exec"), b"#!/bin/sh\n");
        cyber
    }
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn fake_cyber(version: &str, target: &str) -> Vec<u8> {
    format!("#!/bin/sh\necho 'cyber {version} (stable, 0123abc, {target})'\n").into_bytes()
}

fn write_exe(path: &Path, bytes: &[u8]) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(path, bytes).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// A release archive as `release_assets.py package` lays it out: binaries at the root.
fn fake_archive(cyber: &[u8]) -> Vec<u8> {
    let gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    let mut tar = tar::Builder::new(gz);
    let members: [(&str, &[u8], u32); 3] = [
        ("cyber", cyber, 0o755),
        ("cyber-sandbox-exec", b"#!/bin/sh\nexit 0\n", 0o755),
        ("README.md", b"# Cyber Code\n", 0o644),
    ];
    for (name, data, mode) in members {
        let mut header = tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(mode);
        header.set_cksum();
        tar.append_data(&mut header, name, data).unwrap();
    }
    tar.into_inner().unwrap().finish().unwrap()
}

fn run_version(exe: &Path) -> String {
    let output = Command::new(exe).arg("--version").output().unwrap();
    text(&output.stdout)
}

/// Entries of `dir` other than the given names: staging leftovers.
fn leftovers(dir: &Path, expected: &[&str]) -> Vec<String> {
    std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| !expected.contains(&name.as_str()))
        .collect()
}

#[test]
fn checksum_mismatch_exits_1_and_keeps_the_install() {
    let env = Env::new();
    let dir = env.root.join("c");
    let original = env.preinstall(&dir, "1.3.0");
    env.publish("1.4.0", Some(b"some other archive"));

    let output = env.install(&[
        "--version",
        "1.4.0",
        "--dir",
        path(&dir),
        "--no-modify-path",
    ]);

    assert_eq!(output.status.code(), Some(1), "{}", text(&output.stdout));
    assert!(
        text(&output.stderr).contains("checksum verification failed"),
        "{}",
        text(&output.stderr)
    );
    assert_eq!(std::fs::read(dir.join("cyber")).unwrap(), original);
    assert!(leftovers(&dir, &["cyber", "cyber-sandbox-exec"]).is_empty());
    assert!(leftovers(&env.root.join("tmp"), &[]).is_empty());
}

#[test]
fn already_installed_skips_download() {
    let env = Env::new();
    let dir = env.root.join("c");
    env.preinstall(&dir, "1.4.0");

    let output = env.install(&["--version", "1.4.0", "--dir", path(&dir)]);

    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(text(&output.stdout), "cyber 1.4.0 already installed\n");
    assert!(env.stub.requests().is_empty(), "{:?}", env.stub.requests());
}

#[test]
fn offline_install_from_a_mirror() {
    let env = Env::new();
    env.publish("1.4.0", None);
    let dir = env.root.join("c");

    let output = env.install(&[
        "--version",
        "1.4.0",
        "--dir",
        path(&dir),
        "--no-modify-path",
    ]);

    assert!(output.status.success(), "{}", text(&output.stderr));
    assert!(run_version(&dir.join("cyber")).starts_with("cyber 1.4.0 "));
    assert!(dir.join("cyber-sandbox-exec").is_file());
    assert!(leftovers(&dir, &["cyber", "cyber-sandbox-exec", RECEIPT]).is_empty());
    for rc in [".bashrc", ".bash_profile", ".profile", ".zshrc"] {
        assert!(!env.home.join(rc).exists(), "{rc} is not written");
    }
    let paths: Vec<_> = env.stub.requests().into_iter().map(|r| r.path).collect();
    let archive = format!("/v1.4.0/{}", env.archive_name("1.4.0"));
    assert_eq!(paths, ["/v1.4.0/SHA256SUMS", archive.as_str()]);
}

#[test]
fn latest_version_comes_from_the_channel_manifest() {
    let env = Env::new();
    env.stub.manifest("beta", "1.5.0-beta.2");
    env.publish("1.5.0-beta.2", None);
    let dir = env.root.join("c");

    let output = env.install(&["--channel", "beta", "--dir", path(&dir), "--no-modify-path"]);

    assert!(output.status.success(), "{}", text(&output.stderr));
    assert!(run_version(&dir.join("cyber")).starts_with("cyber 1.5.0-beta.2 "));
}

#[test]
fn invalid_arguments_exit_2_without_a_request() {
    let env = Env::new();
    for args in [
        &["--version", "1.4"][..],
        &["--version", "1.4.0;id"],
        &["--channel", "edge"],
        &["--dir", "/tmp/$(id)"],
        &["--bogus"],
    ] {
        let output = env.install(args);
        assert_eq!(output.status.code(), Some(2), "{args:?}");
    }
    assert!(env.stub.requests().is_empty());
}

#[test]
fn path_line_is_added_once_and_removed_by_uninstall() {
    let env = Env::new();
    let bashrc = env.home.join(".bashrc");
    let original = "alias ll='ls -l'\n";
    std::fs::write(&bashrc, original).unwrap();
    env.publish("1.4.0", None);
    env.publish("1.4.1", None);

    for version in ["1.4.0", "1.4.1"] {
        let output = env.install(&["--version", version]);
        assert!(output.status.success(), "{}", text(&output.stderr));
    }

    let rc = std::fs::read_to_string(&bashrc).unwrap();
    assert_eq!(rc.matches(MARKER).count(), 1, "{rc}");
    let line = format!("export PATH=\"{}:$PATH\"", env.install_dir().display());
    assert!(rc.contains(&format!("{MARKER}\n{line}\n")), "{rc}");
    assert!(run_version(&env.install_dir().join("cyber")).starts_with("cyber 1.4.1 "));

    // The real binary takes the fake's place, as a later `cyber upgrade` would.
    let installed = env.install_dir().join("cyber");
    std::fs::remove_file(&installed).unwrap();
    std::fs::copy(env!("CARGO_BIN_EXE_cyber"), &installed).unwrap();
    let output = env.cyber(&installed, &["uninstall", "--force"]);
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        text(&output.stdout),
        text(&output.stderr)
    );

    assert_eq!(std::fs::read_to_string(&bashrc).unwrap(), original);
    assert!(!installed.exists());
    assert!(!env.install_dir().join("cyber-sandbox-exec").exists());
}

/// Regression: a `--dir` install is still a `script` install once
/// `CYBER_INSTALL_DIR` is gone (the PATH line does not set it), through the receipt the
/// script leaves beside the binaries: `cyber upgrade` reports `script` instead of
/// suggesting a second install, and `cyber uninstall` removes the binaries and the
/// receipt together with the PATH line that pointed at them.
#[test]
fn dir_install_is_upgraded_and_uninstalled_as_a_script_install() {
    let env = Env::new();
    let bashrc = env.home.join(".bashrc");
    std::fs::write(&bashrc, "").unwrap();
    env.publish("1.4.0", None);
    let dir = env.root.join("tools");

    let output = env.install(&["--version", "1.4.0", "--dir", path(&dir)]);
    assert!(output.status.success(), "{}", text(&output.stderr));
    let receipt = dir.join(RECEIPT);
    assert_eq!(
        std::fs::read_to_string(&receipt).unwrap(),
        "cyber install script 1.4.0\n"
    );
    assert!(
        std::fs::read_to_string(&bashrc)
            .unwrap()
            .contains(path(&dir))
    );

    let installed = dir.join("cyber");
    std::fs::remove_file(&installed).unwrap();
    std::fs::copy(env!("CARGO_BIN_EXE_cyber"), &installed).unwrap();
    let upgrade = env.cyber(&installed, &["upgrade", env!("CARGO_PKG_VERSION")]);
    assert!(upgrade.status.success(), "{}", text(&upgrade.stderr));
    let upgrade: Value = serde_json::from_slice(&upgrade.stdout).unwrap();
    assert_eq!(upgrade["method"], "script");
    assert_eq!(upgrade["action"], "current");

    let output = env.cyber(&installed, &["uninstall", "--force"]);
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        text(&output.stdout),
        text(&output.stderr)
    );
    assert_eq!(std::fs::read_to_string(&bashrc).unwrap(), "");
    for name in ["cyber", "cyber-sandbox-exec", RECEIPT] {
        assert!(std::fs::symlink_metadata(dir.join(name)).is_err(), "{name}");
    }
}

fn path(dir: &Path) -> &str {
    dir.to_str().unwrap()
}

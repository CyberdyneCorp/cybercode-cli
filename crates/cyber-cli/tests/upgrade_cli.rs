//! `cyber upgrade` and the release source (`installation-upgrade` → Version and channel
//! constants, Latest version lookup, Installation method detection, Upgrade command,
//! Self-update integrity) through the real binary, pointed at a loopback release source
//! with the debug-only test hook: `CYBER_TEST_RELEASE_BASE_URL` for the base URL and
//! `CYBER_TEST_RELEASE_TRUST` for a throwaway trusted root that signs `SHA256SUMS`.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use cyber_install::test_support::TestSigner;
use serde_json::Value;
use sha2::{Digest, Sha256};

#[path = "../../cyber-app/tests/support/scripted_model.rs"]
mod scripted_model;
#[path = "../../cyber-install/tests/support/stub_release.rs"]
mod stub_release;
use stub_release::{Reply, StubRelease};

const VERSION: &str = env!("CARGO_PKG_VERSION");
const NEXT: &str = "9.9.9";
const PREFIX: &str = "https://example.test/release.yml@refs/tags/";
const ISSUER: &str = "https://issuer.example.test";

struct Env {
    _dir: tempfile::TempDir,
    root: PathBuf,
    stub: StubRelease,
}

impl Env {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        // An empty PATH: no package manager is asked about the test binary.
        std::fs::create_dir_all(root.join("empty-path")).unwrap();
        Self {
            _dir: dir,
            root,
            stub: StubRelease::start(),
        }
    }

    fn command(&self, exe: &Path, args: &[&str]) -> Command {
        let mut command = Command::new(exe);
        command
            .args(args)
            .env_clear()
            .env("HOME", self.root.join("home"))
            .env("CYBER_HOME", self.root.join("cyber-home"))
            .env("PATH", self.root.join("empty-path"))
            .env("CYBER_TEST_RELEASE_BASE_URL", &self.stub.base_url)
            .current_dir(&self.root)
            .stdin(Stdio::null());
        #[cfg(windows)]
        if let Some(system) = std::env::var_os("SystemRoot") {
            command.env("SystemRoot", system);
        }
        command
    }

    fn cyber(&self, args: &[&str]) -> Output {
        self.command(Path::new(env!("CARGO_BIN_EXE_cyber")), args)
            .output()
            .unwrap()
    }

    fn cache_file(&self, channel: &str) -> PathBuf {
        self.root
            .join("cyber-home/cache")
            .join(format!("release-{channel}.json"))
    }

    fn write_cache(&self, channel: &str, version: &str, age_ms: u64) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        let path = self.cache_file(channel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let body = serde_json::json!({
            "fetched_at_ms": now - age_ms,
            "manifest": { "version": version },
        });
        std::fs::write(path, body.to_string()).unwrap();
    }
}

fn json_stdout(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn debug_info_reports_the_release_source() {
    let env = Env::new();
    let info = json_stdout(&env.cyber(&["debug", "info", "--format", "json"]));
    assert_eq!(info["release"]["base_url"], env.stub.base_url.as_str());
    let built_in = json_stdout(
        &env.command(
            Path::new(env!("CARGO_BIN_EXE_cyber")),
            &["debug", "info", "--format", "json"],
        )
        .env_remove("CYBER_TEST_RELEASE_BASE_URL")
        .output()
        .unwrap(),
    );
    let release = &built_in["release"];
    assert_eq!(
        release["base_url"],
        "https://github.com/CyberdyneCorp/cybercode-cli/releases/download"
    );
    assert_eq!(
        release["identity_prefix"],
        "https://github.com/CyberdyneCorp/cybercode-cli/.github/workflows/release.yml@refs/tags/"
    );
    assert_eq!(
        release["issuer"],
        "https://token.actions.githubusercontent.com"
    );
    assert!(release.get("test_trusted_root").is_none(), "{release}");
}

#[test]
fn upgrade_uses_the_cached_manifest_without_a_request() {
    let env = Env::new();
    env.stub.manifest("stable", "9.9.9");
    env.write_cache("stable", VERSION, 20 * 60 * 1000);
    let result = json_stdout(&env.cyber(&["upgrade", "--format", "json"]));
    assert_eq!(result["from"], VERSION);
    assert_eq!(result["to"], VERSION);
    assert_eq!(result["action"], "current");
    assert!(env.stub.requests().is_empty(), "{:?}", env.stub.requests());

    // Without a fresh cache the lookup reaches the source once and caches the answer.
    std::fs::remove_file(env.cache_file("stable")).unwrap();
    env.stub.manifest("stable", VERSION);
    let output = env.cyber(&["upgrade"]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        format!("cyber {VERSION} is already installed")
    );
    let requests = env.stub.requests();
    assert_eq!(requests.len(), 1, "{requests:?}");
    assert_eq!(requests[0].path, "/channel-stable/stable.json");
    assert!(env.cache_file("stable").is_file());
}

#[test]
fn upgrade_offline_without_a_cache_fails_without_a_request() {
    let env = Env::new();
    env.stub.manifest("stable", VERSION);
    let output = env
        .command(Path::new(env!("CARGO_BIN_EXE_cyber")), &["upgrade"])
        .env("CYBER_OFFLINE", "1")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("offline: no cached release information"),
        "{output:?}"
    );
    assert!(env.stub.requests().is_empty());
}

#[test]
fn upgrade_reports_the_detected_method() {
    let env = Env::new();
    env.stub.manifest("beta", VERSION);
    let install_dir = env.root.join("opt/tools/cyber");
    std::fs::create_dir_all(&install_dir).unwrap();
    let exe = install_dir.join(format!("cyber{}", std::env::consts::EXE_SUFFIX));
    std::fs::copy(env!("CARGO_BIN_EXE_cyber"), &exe).unwrap();
    let upgrade = |extra: &[&str]| {
        let args = [&["upgrade", "--channel", "beta", "--format", "json"], extra].concat();
        json_stdout(
            &env.command(&exe, &args)
                .env("CYBER_INSTALL_DIR", &install_dir)
                .output()
                .unwrap(),
        )
    };
    let detected = upgrade(&[]);
    assert_eq!(detected["method"], "script");
    assert_eq!(detected["action"], "current");
    assert_eq!(upgrade(&["--method", "nix"])["method"], "nix");
    let requests = env.stub.requests();
    assert_eq!(requests[0].path, "/channel-beta/beta.json");

    // Outside the install directory, with no manager on PATH, the method is unknown.
    let unknown = json_stdout(&env.cyber(&["upgrade", "--channel", "beta", "--format", "json"]));
    assert_eq!(unknown["method"], "unknown");
}

#[test]
fn upgrade_to_a_named_version_skips_the_lookup() {
    let env = Env::new();
    let result = json_stdout(&env.cyber(&["upgrade", &format!("v{VERSION}"), "--format", "json"]));
    assert_eq!(result["to"], VERSION);
    assert!(env.stub.requests().is_empty());
    let output = env.cyber(&["upgrade", "latest"]);
    assert_eq!(output.status.code(), Some(2), "{output:?}");
}

#[test]
fn autoupdate_rejects_unknown_values() {
    let env = Env::new();
    let config = env.root.join("cyber-home/config");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(config.join("cyber.jsonc"), r#"{"autoupdate": "yes"}"#).unwrap();
    let output = env.cyber(&["debug", "config"]);
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(r#"autoupdate: expected true, false or "notify", got "yes""#),
        "{stderr}"
    );
    std::fs::write(config.join("cyber.jsonc"), r#"{"autoupdate": false}"#).unwrap();
    let value = json_stdout(&env.cyber(&["debug", "config"]));
    assert_eq!(value["autoupdate"], false);
}

// ---- Verified self-update -------------------------------------------------------------

/// A signed release on the stub: the throwaway signer, its trust directory, and an
/// install directory holding the real binary.
struct Release {
    env: Env,
    signer: TestSigner,
    trust: PathBuf,
    install_dir: PathBuf,
    exe: PathBuf,
    target: String,
}

impl Release {
    fn new() -> Self {
        let env = Env::new();
        let signer = TestSigner::new(PREFIX, ISSUER);
        let trust = env.root.join("trust");
        signer.write_trust_dir(&trust).unwrap();
        let info = json_stdout(&env.cyber(&["debug", "info", "--format", "json"]));
        let target = info["build"]["target"].as_str().unwrap().to_string();
        let install_dir = env.root.join("install");
        std::fs::create_dir_all(&install_dir).unwrap();
        let exe = install_dir.join(exe_name("cyber"));
        // A hard link where the filesystem allows it: the upgrade renames it aside and
        // never writes into it, so the build's own binary is untouched.
        if std::fs::hard_link(env!("CARGO_BIN_EXE_cyber"), &exe).is_err() {
            std::fs::copy(env!("CARGO_BIN_EXE_cyber"), &exe).unwrap();
        }
        Self {
            env,
            signer,
            trust,
            install_dir,
            exe,
            target,
        }
    }

    fn upgrade(&self, args: &[&str]) -> Output {
        let args = [&["upgrade"], args].concat();
        self.env
            .command(&self.exe, &args)
            .env("CYBER_INSTALL_DIR", &self.install_dir)
            .env("CYBER_TEST_RELEASE_TRUST", &self.trust)
            .output()
            .unwrap()
    }

    fn archive_name(&self, version: &str) -> String {
        let ext = if self.target.contains("windows") {
            "zip"
        } else {
            "tar.gz"
        };
        format!("cyber-{version}-{}.{ext}", self.target)
    }

    /// The archive of `cyber` and its helper, in the target's format.
    fn archive(&self, cyber: &[u8], helper: &[u8]) -> Vec<u8> {
        let members = [
            (exe_name("cyber"), cyber),
            (exe_name("cyber-sandbox-exec"), helper),
            ("LICENSE".to_string(), b"license".as_slice()),
        ];
        if self.target.contains("windows") {
            let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
            let stored = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            for (name, body) in members {
                zip.start_file(name, stored).unwrap();
                zip.write_all(body).unwrap();
            }
            return zip.finish().unwrap().into_inner();
        }
        // Stored, not compressed: deflating a debug binary unoptimized takes half a minute.
        let gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::none());
        let mut tar = tar::Builder::new(gz);
        for (name, body) in members {
            let mut header = tar::Header::new_gnu();
            header.set_size(body.len() as u64);
            header.set_mode(0o755);
            tar.append_data(&mut header, name, body).unwrap();
        }
        tar.into_inner().unwrap().finish().unwrap()
    }

    /// `SHA256SUMS` naming `archive`, its bundle signed as `signed_as`, and the archive.
    fn publish(&self, version: &str, signed_as: &str, archive: &[u8]) -> Vec<u8> {
        let name = self.archive_name(version);
        let digest: String = Sha256::digest(archive)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let sums = format!("{digest}  {name}\n").into_bytes();
        let route = |file: &str, body: Vec<u8>| {
            self.env
                .stub
                .route(&format!("/v{version}/{file}"), vec![Reply::Ok(body)]);
        };
        route("SHA256SUMS", sums.clone());
        route(
            "SHA256SUMS.sigstore.json",
            self.signer.sign_release(signed_as, &sums),
        );
        route(&name, archive.to_vec());
        sums
    }

    /// The installed binary's bytes (compared whole: hashing 200 MB is slow unoptimized).
    fn installed(&self) -> Vec<u8> {
        std::fs::read(&self.exe).unwrap()
    }

    /// The install directory holds `cyber` and nothing else: no `.old`, no temporary.
    fn assert_untouched(&self, output: &Output, original: &[u8], message: &str) {
        assert_eq!(output.status.code(), Some(1), "{output:?}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains(message), "{stderr}");
        assert!(self.installed() == original, "the installed binary changed");
        let names: Vec<_> = std::fs::read_dir(&self.install_dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, [exe_name("cyber")], "{names:?}");
        let version = self
            .env
            .command(&self.exe, &["--version"])
            .output()
            .unwrap();
        assert!(version.status.success(), "{version:?}");
    }
}

fn exe_name(name: &str) -> String {
    format!("{name}{}", std::env::consts::EXE_SUFFIX)
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

#[test]
fn dry_run_names_the_plan_and_downloads_nothing() {
    let release = Release::new();
    release.env.stub.manifest("stable", NEXT);
    let original = release.installed();

    let output = release.upgrade(&["--dry-run"]);
    assert!(output.status.success(), "{output:?}");
    let text = stdout(&output);
    assert!(
        text.starts_with(&format!("cyber {VERSION} → {NEXT} via script (dry run")),
        "{text}"
    );
    let result = json_stdout(&release.upgrade(&["--dry-run", "--format", "json"]));
    assert_eq!(result["action"], "replaced", "the action it would take");
    assert_eq!(result["method"], "script");
    assert_eq!(result["dry_run"], true);

    assert!(release.installed() == original, "the binary is unchanged");
    let paths: Vec<_> = release
        .env
        .stub
        .requests()
        .into_iter()
        .map(|r| r.path)
        .collect();
    assert!(
        paths.iter().all(|p| p == "/channel-stable/stable.json"),
        "only the manifest is read: {paths:?}"
    );
    let entries = std::fs::read_dir(&release.install_dir).unwrap().count();
    assert_eq!(entries, 1, "no temporary or .old file");
}

#[test]
fn script_upgrade_replaces_the_binary_after_verification() {
    let release = Release::new();
    release.env.stub.manifest("stable", NEXT);
    // The real binary with a marker appended stands in for the next release: it still
    // runs, and its digest tells it apart from the installed copy.
    let mut next = std::fs::read(env!("CARGO_BIN_EXE_cyber")).unwrap();
    next.extend_from_slice(b"\ncyber release 9.9.9\n");
    release.publish(NEXT, NEXT, &release.archive(&next, b"helper 9.9.9"));

    let output = release.upgrade(&[]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(stdout(&output), format!("cyber {VERSION} → {NEXT}"));
    assert!(release.installed() == next, "the new binary is installed");
    let helper = release.install_dir.join(exe_name("cyber-sandbox-exec"));
    assert_eq!(std::fs::read(&helper).unwrap(), b"helper 9.9.9");
    let old = release.install_dir.join(exe_name("cyber.old"));
    assert!(old.is_file(), "cyber.old is kept until the next start");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&release.exe)
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o111, 0o111, "{mode:o}");
    }
    let paths: Vec<_> = release
        .env
        .stub
        .requests()
        .into_iter()
        .map(|r| r.path)
        .collect();
    assert_eq!(
        paths,
        [
            "/channel-stable/stable.json".to_string(),
            format!("/v{NEXT}/SHA256SUMS"),
            format!("/v{NEXT}/SHA256SUMS.sigstore.json"),
            format!("/v{NEXT}/{}", release.archive_name(NEXT)),
        ]
    );

    // The next start of the new binary removes the old one.
    let version = release
        .env
        .command(&release.exe, &["--version"])
        .output()
        .unwrap();
    assert!(version.status.success(), "{version:?}");
    assert!(!old.exists());
}

#[test]
fn interrupted_download_leaves_the_binary_unchanged() {
    let release = Release::new();
    let original = release.installed();
    let archive = release.archive(&[1; 64 * 1024], b"helper");
    release.publish(NEXT, NEXT, &archive);
    release.env.stub.route(
        &format!("/v{NEXT}/{}", release.archive_name(NEXT)),
        vec![Reply::Truncated {
            declared: archive.len(),
            body: archive[..archive.len() / 2].to_vec(),
        }],
    );
    let output = release.upgrade(&[NEXT]);
    release.assert_untouched(&output, &original, "download failed");
}

#[test]
fn forged_sha256sums_fails_signature_verification() {
    let release = Release::new();
    let original = release.installed();
    let archive = release.archive(b"evil", b"evil");
    let sums = release.publish(NEXT, NEXT, &archive);
    // The attacker's own archive and a SHA256SUMS naming it, under the genuine bundle.
    let mut forged = sums.clone();
    forged[0] = if forged[0] == b'0' { b'1' } else { b'0' };
    release
        .env
        .stub
        .route(&format!("/v{NEXT}/SHA256SUMS"), vec![Reply::Ok(forged)]);
    let output = release.upgrade(&[NEXT]);
    release.assert_untouched(&output, &original, "signature verification failed");
    let archive_path = format!("/v{NEXT}/{}", release.archive_name(NEXT));
    assert!(
        release
            .env
            .stub
            .requests()
            .iter()
            .all(|r| r.path != archive_path)
    );
}

#[test]
fn tampered_archive_fails_checksum_verification() {
    let release = Release::new();
    let original = release.installed();
    release.publish(NEXT, NEXT, &release.archive(b"genuine", b"helper"));
    release.env.stub.route(
        &format!("/v{NEXT}/{}", release.archive_name(NEXT)),
        vec![Reply::Ok(release.archive(b"tampered", b"helper"))],
    );
    let output = release.upgrade(&[NEXT]);
    release.assert_untouched(&output, &original, "checksum verification failed");
}

#[test]
fn signature_for_another_version_is_refused() {
    let release = Release::new();
    let original = release.installed();
    // A genuine signature, but from the workflow run of another tag.
    release.publish(NEXT, "9.9.8", &release.archive(b"older", b"helper"));
    let output = release.upgrade(&[NEXT]);
    release.assert_untouched(&output, &original, "signature verification failed");
}

/// Regression: `CYBER_OFFLINE` stops every request, not only the manifest lookup. A
/// named version or a cached newer manifest fails before any download.
#[test]
fn offline_upgrade_fails_before_any_download() {
    let release = Release::new();
    let original = release.installed();
    release.publish(NEXT, NEXT, &release.archive(b"cyber next", b"helper next"));
    release
        .env
        .write_cache("stable", NEXT, 3 * 24 * 3600 * 1000);
    let offline = |args: &[&str]| {
        let args = [&["upgrade"], args].concat();
        release
            .env
            .command(&release.exe, &args)
            .env("CYBER_INSTALL_DIR", &release.install_dir)
            .env("CYBER_TEST_RELEASE_TRUST", &release.trust)
            .env("CYBER_OFFLINE", "1")
            .output()
            .unwrap()
    };
    let message = format!("offline: installing cyber {NEXT} needs the network");
    release.assert_untouched(&offline(&[NEXT]), &original, &message);
    release.assert_untouched(&offline(&[]), &original, &message);
    let manager = offline(&["--method", "npm", NEXT]);
    assert_eq!(manager.status.code(), Some(1), "{manager:?}");
    assert!(String::from_utf8_lossy(&manager.stderr).contains(&message));
    assert!(
        release.env.stub.requests().is_empty(),
        "{:?}",
        release.env.stub.requests()
    );
    // Reporting needs no network: the manual command is still printed offline.
    let manual = offline(&["--method", "nix", NEXT]);
    assert!(manual.status.success(), "{manual:?}");
}

/// Regression: a channel manifest older than the running build (a backport tag, a
/// replayed manifest) is not a downgrade; naming the version still is.
#[test]
fn older_manifest_version_changes_nothing() {
    let release = Release::new();
    let original = release.installed();
    let older = "0.0.1";
    release.env.stub.manifest("stable", older);
    release.publish(older, older, &release.archive(b"cyber old", b"helper old"));
    let result = json_stdout(&release.upgrade(&["--format", "json"]));
    assert_eq!(result["action"], "current");
    assert_eq!(result["to"], older);
    let output = release.upgrade(&[]);
    assert!(output.status.success(), "{output:?}");
    assert!(stdout(&output).contains("nothing changed"), "{output:?}");
    assert!(release.installed() == original, "the binary was downgraded");
    let requests = release.env.stub.requests();
    assert!(
        requests.iter().all(|r| r.path.starts_with("/channel-")),
        "{requests:?}"
    );

    let explicit = json_stdout(&release.upgrade(&[older, "--format", "json"]));
    assert_eq!(explicit["action"], "replaced");
    assert_eq!(std::fs::read(&release.exe).unwrap(), b"cyber old");
}

#[test]
fn nix_method_prints_the_manual_command() {
    let release = Release::new();
    release.env.stub.manifest("stable", NEXT);
    let output = release.upgrade(&["--method", "nix"]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        stdout(&output),
        "installed via nix; run: nix profile upgrade cyber-code"
    );
    let json = json_stdout(&release.upgrade(&["--method", "nix", "--format", "json"]));
    assert_eq!(json["action"], "manual");
    assert_eq!(json["command"], "nix profile upgrade cyber-code");
    let requests = release.env.stub.requests();
    assert!(
        requests.iter().all(|r| r.path.starts_with("/channel-")),
        "{requests:?}"
    );
}

#[test]
fn already_current_skips() {
    let release = Release::new();
    let original = release.installed();
    release.env.stub.manifest("stable", VERSION);
    let output = release.upgrade(&[]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        stdout(&output),
        format!("cyber {VERSION} is already installed")
    );
    assert!(release.installed() == original);
    assert_eq!(release.env.stub.requests().len(), 1);
}

#[test]
fn beta_channel_upgrade() {
    let release = Release::new();
    let beta = "9.9.9-beta.1";
    release.env.stub.manifest("stable", NEXT);
    release.env.stub.manifest("beta", beta);
    release.publish(beta, beta, &release.archive(b"cyber beta", b"helper beta"));
    let result = json_stdout(&release.upgrade(&["--channel", "beta", "--format", "json"]));
    assert_eq!(result["from"], VERSION);
    assert_eq!(result["to"], beta);
    assert_eq!(result["method"], "script");
    assert_eq!(result["action"], "replaced");
    assert_eq!(std::fs::read(&release.exe).unwrap(), b"cyber beta");
    let requests = release.env.stub.requests();
    assert_eq!(requests[0].path, "/channel-beta/beta.json");
    assert!(
        requests
            .iter()
            .all(|r| r.path != "/channel-stable/stable.json")
    );
}

#[cfg(unix)]
#[test]
fn brew_method_runs_the_manager() {
    use std::os::unix::fs::PermissionsExt;
    let env = Env::new();
    env.stub.manifest("stable", NEXT);
    let bin = env.root.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let brew = bin.join("brew");
    std::fs::write(
        &brew,
        "#!/bin/sh\necho \"$@\" > \"$0.args\"\nexit \"${BREW_EXIT:-0}\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&brew, std::fs::Permissions::from_mode(0o755)).unwrap();
    let upgrade = |exit: &str| {
        env.command(
            Path::new(env!("CARGO_BIN_EXE_cyber")),
            &["upgrade", "--method", "brew"],
        )
        .env("PATH", &bin)
        .env("BREW_EXIT", exit)
        .output()
        .unwrap()
    };
    let output = upgrade("0");
    assert!(output.status.success(), "{output:?}");
    assert_eq!(stdout(&output), format!("cyber {VERSION} → {NEXT}"));
    let args = std::fs::read_to_string(bin.join("brew.args")).unwrap();
    assert_eq!(args.trim(), "upgrade cyber");

    let failed = upgrade("3");
    assert_eq!(failed.status.code(), Some(1), "{failed:?}");
    let stderr = String::from_utf8_lossy(&failed.stderr);
    assert!(stderr.contains("brew upgrade cyber"), "{stderr}");
}

/// Regression: a cyber package in brew's listing does not show that the running binary
/// came from it, so `cyber upgrade` names the command instead of running it and never
/// claims the binary was upgraded; `--method brew` still runs it.
#[cfg(unix)]
#[test]
fn listing_only_method_prints_the_manager_command() {
    use std::os::unix::fs::PermissionsExt;
    let env = Env::new();
    env.stub.manifest("stable", NEXT);
    let bin = env.root.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let brew = bin.join("brew");
    std::fs::write(
        &brew,
        "#!/bin/sh\necho \"$@\" >> \"$0.calls\"\n[ \"$1\" = list ] && echo 'cyber 1.0.0'\nexit 0\n",
    )
    .unwrap();
    std::fs::set_permissions(&brew, std::fs::Permissions::from_mode(0o755)).unwrap();
    let upgrade = |args: &[&str]| {
        env.command(Path::new(env!("CARGO_BIN_EXE_cyber")), args)
            .env("PATH", &bin)
            .output()
            .unwrap()
    };

    let text = upgrade(&["upgrade"]);
    assert!(text.status.success(), "{text:?}");
    assert_eq!(
        stdout(&text),
        "brew lists a cyber package, but this binary is not from it; to upgrade that \
         package, run: brew upgrade cyber (or \"cyber upgrade --method brew\" to run it)"
    );
    let result = json_stdout(&upgrade(&["upgrade", "--format", "json"]));
    assert_eq!(result["method"], "brew");
    assert_eq!(result["action"], "manual");
    assert_eq!(result["command"], "brew upgrade cyber");
    let calls = std::fs::read_to_string(bin.join("brew.calls")).unwrap();
    assert!(
        calls.lines().all(|call| call == "list --versions cyber"),
        "{calls}"
    );

    let named = upgrade(&["upgrade", "--method", "brew"]);
    assert!(named.status.success(), "{named:?}");
    let calls = std::fs::read_to_string(bin.join("brew.calls")).unwrap();
    assert_eq!(calls.lines().last(), Some("upgrade cyber"), "{calls}");
}

/// "Unsupported version warning": `exec` reads the cached manifest only (no request,
/// even with a stale cache) and warns on stderr before the run, which still answers.
#[test]
fn exec_warns_when_below_min_supported() {
    let env = Env::new();
    let model = scripted_model::ScriptedModel::start(|_| serde_json::json!({"content": "ok"}));
    let config = env.root.join("cyber-home/config");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(config.join("cyber.jsonc"), model.config().to_string()).unwrap();
    let catalog = env.root.join("models.json");
    std::fs::write(&catalog, "{}").unwrap();
    let cache = env.cache_file("stable");
    std::fs::create_dir_all(cache.parent().unwrap()).unwrap();
    let exec = |cache_min: &str| {
        let body = serde_json::json!({
            "fetched_at_ms": 1,
            "manifest": { "version": "99.0.0", "min_supported": cache_min },
        });
        std::fs::write(&cache, body.to_string()).unwrap();
        let output = env
            .command(
                Path::new(env!("CARGO_BIN_EXE_cyber")),
                &["exec", "--embedded", "hi"],
            )
            .env("CYBER_MODELS_PATH", &catalog)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "ok");
        String::from_utf8_lossy(&output.stderr).into_owned()
    };
    let warned = exec("99.0.0");
    let warning = format!(
        "warning: cyber {VERSION} is no longer supported (minimum 99.0.0); run \"cyber upgrade\""
    );
    assert!(warned.starts_with(&warning), "{warned}");
    assert!(!exec(VERSION).contains("no longer supported"));
    assert!(env.stub.requests().is_empty(), "{:?}", env.stub.requests());
}

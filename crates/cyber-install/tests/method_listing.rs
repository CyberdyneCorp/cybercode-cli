//! Installation method detection from package-manager listings (`installation-upgrade` →
//! Installation method detection), with stub manager executables on PATH. The stubs are
//! `sh` scripts, so these run on Unix only.
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use cyber_install::method::{InstallMethod, LISTING_TIMEOUT, Probe, detect};

struct Managers {
    dir: tempfile::TempDir,
}

impl Managers {
    fn new() -> Self {
        Self {
            dir: tempfile::tempdir().unwrap(),
        }
    }

    /// A stub `name` that records its argv and runs `body`.
    fn stub(&self, name: &str, body: &str) {
        let path = self.dir.path().join(name);
        let log = self.argv_log(name);
        let script = format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\n{body}\n",
            log.display()
        );
        std::fs::write(&path, script).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn argv_log(&self, name: &str) -> PathBuf {
        self.dir.path().join(format!("{name}.argv"))
    }

    fn argv(&self, name: &str) -> Option<Vec<String>> {
        let text = std::fs::read_to_string(self.argv_log(name)).ok()?;
        Some(text.lines().map(str::to_string).collect())
    }

    fn probe(&self, exe: &str) -> Probe {
        Probe {
            exe: PathBuf::from(exe),
            home: PathBuf::from("/home/u"),
            install_dir: None,
            cargo_home: None,
            in_docker: false,
            path: Some(self.dir.path().as_os_str().to_owned()),
            pathext: None,
            listing_timeout: LISTING_TIMEOUT,
        }
    }
}

#[test]
fn brew_listing_detects_a_symlinked_homebrew_binary() {
    let managers = Managers::new();
    managers.stub("brew", "echo 'cyber 1.4.0'");
    assert_eq!(
        detect(&managers.probe("/usr/local/bin/cyber")),
        InstallMethod::Brew
    );
    // The query is an argv, never a shell line.
    assert_eq!(
        managers.argv("brew").unwrap(),
        ["list", "--versions", "cyber"]
    );
}

#[test]
fn a_manager_that_does_not_list_cyber_is_skipped() {
    let managers = Managers::new();
    managers.stub("brew", "exit 1");
    managers.stub("cargo", "echo 'ripgrep v14.1.0:'; echo '    rg'");
    managers.stub("pacman", "echo cyber-code");
    assert_eq!(
        detect(&managers.probe("/usr/bin/cyber")),
        InstallMethod::Aur
    );
    assert_eq!(managers.argv("pacman").unwrap(), ["-Qq", "cyber-code"]);
}

#[test]
fn managers_named_in_the_path_are_asked_first() {
    let managers = Managers::new();
    let listed = r#"echo '{"dependencies": {"@cyber-code/cli": {"version": "1.4.0"}}}'"#;
    managers.stub("npm", listed);
    managers.stub("pnpm", listed);
    assert_eq!(
        detect(&managers.probe("/usr/bin/cyber")),
        InstallMethod::Npm
    );
    assert_eq!(
        detect(&managers.probe("/home/u/.local/share/pnpm/cyber")),
        InstallMethod::Pnpm
    );
}

#[test]
fn cargo_listing_matches_the_cyber_cli_package() {
    let managers = Managers::new();
    managers.stub("cargo", "echo 'cyber-cli v1.4.0 (https://github.com/CyberdyneCorp/cybercode-cli?tag=v1.4.0#abc):'; echo '    cyber'");
    assert_eq!(
        detect(&managers.probe("/srv/bin/cyber")),
        InstallMethod::Cargo
    );
}

#[test]
fn a_hung_manager_times_out() {
    let managers = Managers::new();
    managers.stub("brew", "exec sleep 10");
    let mut probe = managers.probe("/usr/bin/cyber");
    probe.listing_timeout = Duration::from_millis(300);
    let started = Instant::now();
    assert_eq!(detect(&probe), InstallMethod::Unknown);
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "{:?}",
        started.elapsed()
    );
}

#[test]
fn a_non_executable_file_on_path_is_not_run() {
    let managers = Managers::new();
    managers.stub("brew", "echo 'cyber 1.4.0'");
    let brew = managers.dir.path().join("brew");
    std::fs::set_permissions(&brew, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(
        detect(&managers.probe("/usr/bin/cyber")),
        InstallMethod::Unknown
    );
    assert!(!Path::new(&managers.argv_log("brew")).exists());
}

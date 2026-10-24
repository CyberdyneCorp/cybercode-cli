//! How the running binary was installed (`installation-upgrade` → Installation method
//! detection): the install directory first, then the executable path, then the package
//! managers' own listings, each run as an argv (never through a shell) with a 3 second
//! timeout.

use std::ffi::{OsStr, OsString};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use cyber_core::env::EnvSource;
use serde::{Deserialize, Serialize};

/// How long one package-manager listing may take.
pub const LISTING_TIMEOUT: Duration = Duration::from_secs(3);

/// The file the install script writes beside the binaries it installs, so a `--dir`
/// install is still known as `script` without `CYBER_INSTALL_DIR` in the environment.
pub const RECEIPT: &str = ".cyber-receipt";

/// A listing prints a few lines; more than this is not read.
const MAX_LISTING_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InstallMethod {
    Script,
    Brew,
    Npm,
    Pnpm,
    Bun,
    Cargo,
    Scoop,
    Winget,
    Aur,
    Nix,
    Docker,
    Unknown,
}

impl InstallMethod {
    pub const ALL: [Self; 12] = [
        Self::Script,
        Self::Brew,
        Self::Npm,
        Self::Pnpm,
        Self::Bun,
        Self::Cargo,
        Self::Scoop,
        Self::Winget,
        Self::Aur,
        Self::Nix,
        Self::Docker,
        Self::Unknown,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Script => "script",
            Self::Brew => "brew",
            Self::Npm => "npm",
            Self::Pnpm => "pnpm",
            Self::Bun => "bun",
            Self::Cargo => "cargo",
            Self::Scoop => "scoop",
            Self::Winget => "winget",
            Self::Aur => "aur",
            Self::Nix => "nix",
            Self::Docker => "docker",
            Self::Unknown => "unknown",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|m| m.as_str() == value)
    }
}

/// Everything detection looks at, gathered once so the rules stay pure.
#[derive(Debug, Clone)]
pub struct Probe {
    pub exe: PathBuf,
    pub home: PathBuf,
    /// `CYBER_INSTALL_DIR`: where the install script put the binary.
    pub install_dir: Option<PathBuf>,
    /// `CARGO_HOME`, else `~/.cargo`.
    pub cargo_home: Option<PathBuf>,
    /// `/.dockerenv` exists.
    pub in_docker: bool,
    /// `PATH` the package managers are looked up on.
    pub path: Option<OsString>,
    /// `PATHEXT` (Windows): the extensions a manager executable may have.
    pub pathext: Option<String>,
    pub listing_timeout: Duration,
}

impl Probe {
    /// The running executable, as `current_exe` reports it (not canonicalized: on
    /// Windows that yields `\\?\` paths).
    pub fn current(env: &dyn EnvSource, home: &Path) -> std::io::Result<Self> {
        Ok(Self {
            exe: std::env::current_exe()?,
            home: home.to_path_buf(),
            install_dir: env.get("CYBER_INSTALL_DIR").map(PathBuf::from),
            cargo_home: env.get("CARGO_HOME").map(PathBuf::from),
            in_docker: Path::new("/.dockerenv").exists(),
            path: env.get("PATH").map(OsString::from),
            pathext: env.get("PATHEXT"),
            listing_timeout: LISTING_TIMEOUT,
        })
    }

    /// The executable path, lowercase with `/` separators, for substring patterns.
    fn normalized(&self) -> String {
        self.exe
            .to_string_lossy()
            .replace('\\', "/")
            .to_ascii_lowercase()
    }
}

/// The method of the running binary: path rules, then listings, else `unknown`.
pub fn detect(probe: &Probe) -> InstallMethod {
    detect_origin(probe).method
}

/// A detected method and whether the executable path showed it. A method known from a
/// manager's listing alone says a package is installed somewhere, not that this binary
/// came from it, so nothing that changes the install acts on it unattended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Detected {
    pub method: InstallMethod,
    pub from_path: bool,
}

impl Detected {
    /// A method the user named (`--method`), trusted as if the path showed it.
    pub fn named(method: InstallMethod) -> Self {
        Self {
            method,
            from_path: true,
        }
    }
}

/// [`detect`], with where the answer came from.
pub fn detect_origin(probe: &Probe) -> Detected {
    if let Some(method) = from_path(probe) {
        return Detected::named(method);
    }
    Detected {
        method: from_listings(probe).unwrap_or(InstallMethod::Unknown),
        from_path: false,
    }
}

type Rule = (InstallMethod, fn(&Probe, &str) -> bool);

/// Path rules in order; the first match wins. `p` is the normalized executable path.
const RULES: &[Rule] = &[
    (InstallMethod::Script, |probe, _| is_script(probe)),
    (InstallMethod::Brew, |_, p| {
        p.contains("/cellar/cyber/")
            || p.starts_with("/opt/homebrew/bin/")
            || p.contains("/home/linuxbrew/")
    }),
    (InstallMethod::Pnpm, |_, p| p.contains("/.pnpm/")),
    (InstallMethod::Bun, |_, p| p.contains("/.bun/")),
    (InstallMethod::Npm, |_, p| {
        p.contains("node_modules/@cyber-code/cli")
    }),
    (InstallMethod::Cargo, |probe, _| {
        let cargo_home = probe
            .cargo_home
            .clone()
            .unwrap_or_else(|| probe.home.join(".cargo"));
        probe.exe.starts_with(cargo_home.join("bin"))
    }),
    (InstallMethod::Scoop, |_, p| {
        p.contains("/scoop/apps/cyber/")
    }),
    (InstallMethod::Winget, |_, p| {
        p.contains("/winget/packages/")
    }),
    (InstallMethod::Nix, |_, p| p.starts_with("/nix/store/")),
    (InstallMethod::Docker, |probe, _| {
        probe.in_docker && probe.exe == Path::new("/usr/local/bin/cyber")
    }),
];

/// `script` when the binary is under `$CYBER_INSTALL_DIR` or `~/.cyber/bin`, or the
/// install script's receipt sits beside it.
fn is_script(probe: &Probe) -> bool {
    let default = probe.home.join(".cyber").join("bin");
    std::iter::once(default)
        .chain(probe.install_dir.clone())
        .any(|dir| probe.exe.starts_with(dir))
        || receipt_path(&probe.exe).is_some_and(|receipt| {
            std::fs::symlink_metadata(receipt).is_ok_and(|meta| meta.is_file())
        })
}

/// Where the install script's receipt for `exe` would be.
pub fn receipt_path(exe: &Path) -> Option<PathBuf> {
    exe.parent().map(|dir| dir.join(RECEIPT))
}

/// The method the executable path alone shows, if any.
pub fn from_path(probe: &Probe) -> Option<InstallMethod> {
    let normalized = probe.normalized();
    RULES
        .iter()
        .find(|(_, matches)| matches(probe, &normalized))
        .map(|(method, _)| *method)
}

/// One package manager's query for the `cyber` package and how to read its answer.
struct Listing {
    method: InstallMethod,
    program: &'static str,
    args: &'static [&'static str],
    installed: fn(&str) -> bool,
}

const LISTINGS: &[Listing] = &[
    Listing {
        method: InstallMethod::Brew,
        program: "brew",
        args: &["list", "--versions", "cyber"],
        installed: |out| !out.trim().is_empty(),
    },
    Listing {
        method: InstallMethod::Npm,
        program: "npm",
        args: &["ls", "-g", "@cyber-code/cli", "--json"],
        installed: |out| out.contains("\"@cyber-code/cli\""),
    },
    Listing {
        method: InstallMethod::Pnpm,
        program: "pnpm",
        args: &["ls", "-g", "@cyber-code/cli", "--json"],
        installed: |out| out.contains("\"@cyber-code/cli\""),
    },
    Listing {
        method: InstallMethod::Bun,
        program: "bun",
        args: &["pm", "ls", "-g"],
        installed: |out| out.contains("@cyber-code/cli"),
    },
    Listing {
        method: InstallMethod::Cargo,
        program: "cargo",
        args: &["install", "--list"],
        installed: |out| out.lines().any(|l| l.starts_with("cyber-cli ")),
    },
    Listing {
        method: InstallMethod::Scoop,
        program: "scoop",
        args: &["prefix", "cyber"],
        installed: |out| !out.trim().is_empty(),
    },
    Listing {
        method: InstallMethod::Winget,
        program: "winget",
        args: &["list", "--id", "cyber-code.cyber", "--exact"],
        installed: |out| out.contains("cyber-code.cyber"),
    },
    Listing {
        method: InstallMethod::Aur,
        program: "pacman",
        args: &["-Qq", "cyber-code"],
        installed: |out| out.lines().any(|l| l.trim() == "cyber-code"),
    },
    Listing {
        method: InstallMethod::Nix,
        program: "nix",
        args: &["profile", "list"],
        installed: |out| out.contains("cyber-code"),
    },
];

/// Ask each manager on PATH, managers named in the executable path first.
fn from_listings(probe: &Probe) -> Option<InstallMethod> {
    let path = probe.path.as_ref()?;
    let normalized = probe.normalized();
    // A directory named after the manager (`pnpm`, `.bun`): `pnpm` does not name `npm`.
    let named = |program: &str| {
        normalized
            .split('/')
            .any(|dir| dir.trim_start_matches('.') == program)
    };
    let mut listings: Vec<&Listing> = LISTINGS.iter().collect();
    listings.sort_by_key(|l| !named(l.program));
    listings.into_iter().find_map(|listing| {
        let program = which(listing.program, path, probe.pathext.as_deref())?;
        let out = query(&program, listing.args, probe.listing_timeout)?;
        (listing.installed)(&out).then_some(listing.method)
    })
}

/// The stdout of `program args` when it exits successfully within `timeout`.
fn query(program: &Path, args: &[&str], timeout: Duration) -> Option<String> {
    let deadline = Instant::now() + timeout;
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let stdout = child.stdout.take()?;
    // Read on a thread so a full pipe never stalls the child; a grandchild that keeps
    // the pipe open only leaves this thread behind.
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut out = Vec::new();
        let _ = stdout.take(MAX_LISTING_BYTES).read_to_end(&mut out);
        let _ = tx.send(out);
    });
    let status = loop {
        if let Some(status) = child.try_wait().ok()? {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let out = rx
        .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        .ok()?;
    status
        .success()
        .then(|| String::from_utf8_lossy(&out).into_owned())
}

/// `program` on `path`, with each `PATHEXT` extension on Windows.
pub fn which(program: &str, path: &OsStr, pathext: Option<&str>) -> Option<PathBuf> {
    std::env::split_paths(path)
        .filter(|dir| !dir.as_os_str().is_empty())
        .flat_map(|dir| candidates(&dir, program, pathext))
        .find(|candidate| is_executable(candidate))
}

#[cfg(windows)]
fn candidates(dir: &Path, program: &str, pathext: Option<&str>) -> Vec<PathBuf> {
    pathext
        .unwrap_or(".COM;.EXE;.BAT;.CMD")
        .split(';')
        .filter(|ext| !ext.is_empty())
        .map(|ext| dir.join(format!("{program}{ext}")))
        .collect()
}

#[cfg(not(windows))]
fn candidates(dir: &Path, program: &str, _pathext: Option<&str>) -> Vec<PathBuf> {
    vec![dir.join(program)]
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe(exe: &str) -> Probe {
        Probe {
            exe: PathBuf::from(exe),
            home: PathBuf::from("/home/u"),
            install_dir: None,
            cargo_home: None,
            in_docker: false,
            path: None,
            pathext: None,
            listing_timeout: LISTING_TIMEOUT,
        }
    }

    #[test]
    fn every_path_pattern_names_its_method() {
        let table = [
            ("/home/u/.cyber/bin/cyber", Some(InstallMethod::Script)),
            (
                "/opt/homebrew/Cellar/cyber/1.4.0/bin/cyber",
                Some(InstallMethod::Brew),
            ),
            (
                "/usr/local/Cellar/cyber/1.4.0/bin/cyber",
                Some(InstallMethod::Brew),
            ),
            ("/opt/homebrew/bin/cyber", Some(InstallMethod::Brew)),
            (
                "/home/linuxbrew/.linuxbrew/bin/cyber",
                Some(InstallMethod::Brew),
            ),
            (
                "/usr/lib/node_modules/@cyber-code/cli/bin/cyber",
                Some(InstallMethod::Npm),
            ),
            (
                "/home/u/.local/share/pnpm/global/5/.pnpm/@cyber-code+cli@1.4.0/node_modules/@cyber-code/cli/bin/cyber",
                Some(InstallMethod::Pnpm),
            ),
            (
                "/home/u/.bun/install/global/node_modules/@cyber-code/cli/bin/cyber",
                Some(InstallMethod::Bun),
            ),
            ("/home/u/.cargo/bin/cyber", Some(InstallMethod::Cargo)),
            (
                r"C:\Users\u\scoop\apps\cyber\current\cyber.exe",
                Some(InstallMethod::Scoop),
            ),
            (
                r"C:\Users\u\AppData\Local\Microsoft\WinGet\Packages\cyber-code.cyber_x\cyber.exe",
                Some(InstallMethod::Winget),
            ),
            (
                "/nix/store/abc-cyber-code-1.4.0/bin/cyber",
                Some(InstallMethod::Nix),
            ),
            ("/usr/local/bin/cyber", None),
            ("/usr/bin/cyber", None),
        ];
        for (exe, expected) in table {
            assert_eq!(from_path(&probe(exe)), expected, "{exe}");
        }
    }

    #[test]
    fn script_detection_with_a_custom_directory() {
        let mut probe = probe("/opt/tools/cyber/cyber");
        assert_eq!(from_path(&probe), None);
        probe.install_dir = Some(PathBuf::from("/opt/tools/cyber"));
        assert_eq!(from_path(&probe), Some(InstallMethod::Script));
    }

    #[test]
    fn a_receipt_beside_the_binary_marks_a_script_install() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("cyber");
        let probe = probe(exe.to_str().unwrap());
        assert_eq!(from_path(&probe), None);
        let receipt = dir.path().join(RECEIPT);
        std::fs::create_dir(&receipt).unwrap();
        assert_eq!(from_path(&probe), None, "a directory is not a receipt");
        std::fs::remove_dir(&receipt).unwrap();
        std::fs::write(&receipt, "cyber install script 1.4.0\n").unwrap();
        assert_eq!(from_path(&probe), Some(InstallMethod::Script));
    }

    #[test]
    fn install_dir_wins_over_path_patterns() {
        let mut probe = probe("/opt/homebrew/bin/cyber");
        probe.install_dir = Some(PathBuf::from("/opt/homebrew/bin"));
        assert_eq!(from_path(&probe), Some(InstallMethod::Script));
    }

    #[test]
    fn cargo_home_relocates_the_cargo_rule() {
        let mut probe = probe("/srv/cargo/bin/cyber");
        assert_eq!(from_path(&probe), None);
        probe.cargo_home = Some(PathBuf::from("/srv/cargo"));
        assert_eq!(from_path(&probe), Some(InstallMethod::Cargo));
    }

    #[test]
    fn docker_needs_the_marker_and_the_image_path() {
        let mut probe = probe("/usr/local/bin/cyber");
        probe.in_docker = true;
        assert_eq!(from_path(&probe), Some(InstallMethod::Docker));
        probe.exe = PathBuf::from("/usr/bin/cyber");
        assert_eq!(from_path(&probe), None);
    }

    #[test]
    fn a_prefix_of_a_directory_name_is_not_under_it() {
        let mut probe = probe("/opt/tools/cyber-old/cyber");
        probe.install_dir = Some(PathBuf::from("/opt/tools/cyber"));
        assert_eq!(from_path(&probe), None);
    }

    #[test]
    fn without_path_or_listings_the_method_is_unknown() {
        assert_eq!(detect(&probe("/usr/bin/cyber")), InstallMethod::Unknown);
    }

    #[test]
    fn methods_round_trip_through_their_names() {
        for method in InstallMethod::ALL {
            assert_eq!(InstallMethod::parse(method.as_str()), Some(method));
        }
        assert_eq!(InstallMethod::parse("apt"), None);
    }
}

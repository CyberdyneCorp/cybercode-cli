//! `cyber uninstall` (`installation-upgrade` → Uninstall command, `cli-commands` → Dry-run
//! uninstall) through the real binary, with a temporary `HOME`, `XDG_*` directories and
//! `CYBER_INSTALL_DIR`. Removals run from a copy of the binary in the install directory,
//! so a `script` install is detected and the build's own binary is never touched.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

const MARKER: &str = "# cyber installer: PATH";

struct Env {
    _dir: tempfile::TempDir,
    root: PathBuf,
    home: PathBuf,
}

impl Env {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let home = root.join("home");
        for dir in ["empty-path", "work"] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
        }
        std::fs::create_dir_all(&home).unwrap();
        Self {
            _dir: dir,
            root,
            home,
        }
    }

    fn dir(&self, kind: &str) -> PathBuf {
        let rel = match kind {
            "config" => ".config",
            "data" => ".local/share",
            "state" => ".local/state",
            "cache" => ".cache",
            other => panic!("{other}"),
        };
        self.home.join(rel).join("cyber")
    }

    fn install_dir(&self) -> PathBuf {
        self.home.join(".cyber/bin")
    }

    fn installed(&self) -> PathBuf {
        self.install_dir()
            .join(format!("cyber{}", std::env::consts::EXE_SUFFIX))
    }

    /// Populate every cyber directory, the install directory and a marked `.bashrc`.
    fn populate(&self) {
        for kind in ["config", "data", "state", "cache"] {
            let dir = self.dir(kind);
            std::fs::create_dir_all(dir.join("nested")).unwrap();
            std::fs::write(dir.join("nested/file"), vec![b'x'; 1000]).unwrap();
        }
        std::fs::create_dir_all(self.install_dir()).unwrap();
        let built = Path::new(env!("CARGO_BIN_EXE_cyber"));
        if std::fs::hard_link(built, self.installed()).is_err() {
            std::fs::copy(built, self.installed()).unwrap();
        }
        std::fs::write(
            self.home.join(".bashrc"),
            format!(
                "alias ll='ls -l'\n{MARKER}\nexport PATH=\"{}:$PATH\"\nexport KEEP=1\n",
                self.install_dir().display()
            ),
        )
        .unwrap();
    }

    /// Every populated file is still there and the PATH line is still marked.
    fn assert_untouched(&self) {
        for kind in ["config", "data", "state", "cache"] {
            assert!(self.dir(kind).join("nested/file").is_file(), "{kind}");
        }
        assert!(self.installed().is_file());
        let rc = std::fs::read_to_string(self.home.join(".bashrc")).unwrap();
        assert!(rc.contains(MARKER), "{rc}");
    }

    fn command(&self, exe: &Path, args: &[&str]) -> Command {
        let mut command = Command::new(exe);
        command
            .args(args)
            .env_clear()
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", self.home.join(".config"))
            .env("XDG_DATA_HOME", self.home.join(".local/share"))
            .env("XDG_STATE_HOME", self.home.join(".local/state"))
            .env("XDG_CACHE_HOME", self.home.join(".cache"))
            .env("CYBER_INSTALL_DIR", self.install_dir())
            // No package manager is asked about the binary.
            .env("PATH", self.root.join("empty-path"))
            .current_dir(self.root.join("work"))
            .stdin(Stdio::null());
        #[cfg(windows)]
        if let Some(system) = std::env::var_os("SystemRoot") {
            command.env("SystemRoot", system);
        }
        command
    }

    /// The installed copy, so removal deletes it rather than the build's binary.
    fn uninstall(&self, args: &[&str]) -> Output {
        let mut all = vec!["uninstall"];
        all.extend_from_slice(args);
        self.command(&self.installed(), &all).output().unwrap()
    }
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn success_json(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        text(&output.stdout),
        text(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

/// `path` with links and Windows path forms resolved, for comparing listed paths.
fn resolved(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn kinds(plan: &Value) -> Vec<(String, PathBuf)> {
    plan["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            let path = PathBuf::from(e["path"].as_str().unwrap());
            (e["kind"].as_str().unwrap().to_string(), path)
        })
        .collect()
}

#[test]
fn dry_run_lists_paths_with_sizes_and_deletes_nothing() {
    let env = Env::new();
    env.populate();
    let rc_before = std::fs::read(env.home.join(".bashrc")).unwrap();
    let plan = success_json(&env.uninstall(&["--dry-run", "--format", "json"]));
    assert_eq!(plan["method"], "script");
    let listed = kinds(&plan);
    for expected in [
        ("binary", env.installed()),
        ("path_line", env.home.join(".bashrc")),
        ("config", env.dir("config")),
        ("data", env.dir("data")),
        ("state", env.dir("state")),
        ("cache", env.dir("cache")),
    ] {
        // Compared resolved: on Windows the binary is listed as `current_exe` reports
        // it, without the `\\?\` prefix the test's canonical root has.
        let expected = (expected.0, resolved(&expected.1));
        assert!(
            listed
                .iter()
                .any(|(kind, path)| *kind == expected.0 && resolved(path) == expected.1),
            "{expected:?} in {listed:?}"
        );
    }
    for entry in plan["entries"].as_array().unwrap() {
        assert_eq!(entry["removed"], false);
        if entry["kind"] == "cache" {
            assert_eq!(entry["size"], 1000);
        }
    }
    let text_plan = env.uninstall(&["--dry-run"]);
    assert!(text_plan.status.success(), "{}", text(&text_plan.stderr));
    let printed = text(&text_plan.stdout);
    assert!(printed.contains("Would remove:"), "{printed}");
    assert!(printed.contains(&env.dir("cache").display().to_string()));
    assert!(printed.contains("1000 B"), "{printed}");
    env.assert_untouched();
    assert_eq!(std::fs::read(env.home.join(".bashrc")).unwrap(), rc_before);
}

#[test]
fn keep_data_force_removes_everything_else() {
    let env = Env::new();
    env.populate();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        std::fs::set_permissions(env.home.join(".bashrc"), perms).unwrap();
    }
    let plan = success_json(&env.uninstall(&["--keep-data", "--force", "--format", "json"]));
    assert!(
        plan["entries"]
            .as_array()
            .unwrap()
            .iter()
            .all(|e| e["removed"] == true && e["kind"] != "data"),
        "{plan}"
    );
    assert!(env.dir("data").join("nested/file").is_file());
    for kind in ["config", "state", "cache"] {
        assert!(!env.dir(kind).exists(), "{kind} remains");
    }
    let rc = env.home.join(".bashrc");
    assert_eq!(
        std::fs::read_to_string(&rc).unwrap(),
        "alias ll='ls -l'\nexport KEEP=1\n"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&rc).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        assert!(
            !env.install_dir().exists(),
            "the emptied install dir is removed"
        );
    }
    #[cfg(windows)]
    {
        // The running exe cannot be deleted, so it was renamed aside.
        assert!(!env.installed().exists());
        assert!(env.install_dir().join("cyber.old.exe").is_file());
        assert!(plan["notes"][0].as_str().unwrap().contains("cyber.old.exe"));
    }
}

#[cfg(unix)]
#[test]
fn symlink_inside_a_cyber_dir_is_unlinked_not_followed() {
    let env = Env::new();
    env.populate();
    let projects = env.home.join("projects");
    std::fs::create_dir_all(projects.join("src")).unwrap();
    std::fs::write(projects.join("src/main.rs"), b"fn main() {}").unwrap();
    std::os::unix::fs::symlink(&projects, env.dir("cache").join("link")).unwrap();
    let plan = success_json(&env.uninstall(&["--dry-run", "--format", "json"]));
    let cache = plan["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["kind"] == "cache")
        .unwrap()
        .clone();
    assert!(cache["size"].as_u64().unwrap() < 1000 + 4096, "{cache}");
    let output = env.uninstall(&["--force"]);
    assert!(output.status.success(), "{}", text(&output.stderr));
    assert!(!env.dir("cache").exists());
    assert_eq!(
        std::fs::read(projects.join("src/main.rs")).unwrap(),
        b"fn main() {}"
    );
}

#[test]
fn home_as_config_dir_is_refused() {
    let env = Env::new();
    env.populate();
    let output = env
        .command(&env.installed(), &["uninstall", "--force"])
        .env("CYBER_CONFIG_DIR", &env.home)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1), "{}", text(&output.stderr));
    let stderr = text(&output.stderr);
    assert!(stderr.contains("refusing to remove"), "{stderr}");
    assert!(stderr.contains("home directory"), "{stderr}");
    env.assert_untouched();
}

#[test]
fn non_tty_without_force_exits_2() {
    let env = Env::new();
    env.populate();
    let output = env.uninstall(&[]);
    assert_eq!(output.status.code(), Some(2), "{}", text(&output.stderr));
    assert!(text(&output.stderr).contains("--force"));
    env.assert_untouched();
}

/// The server this test started, killed by its own child handle if the test fails.
struct OwnedServer(Child);

impl Drop for OwnedServer {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn uninstall_stops_the_registered_server() {
    let env = Env::new();
    env.populate();
    let built = Path::new(env!("CARGO_BIN_EXE_cyber"));
    let mut command = env.command(built, &["serve", "--register", "--port", "0"]);
    #[cfg(unix)]
    command.arg("--socket").arg(env.root.join("u.sock"));
    let log = env.root.join("server-stderr.log");
    let mut server = OwnedServer(
        command
            .stdout(Stdio::null())
            .stderr(std::fs::File::create(&log).unwrap())
            .spawn()
            .unwrap(),
    );
    let registration = env.dir("state").join("server.json");
    let deadline = Instant::now() + Duration::from_secs(60);
    while !registration.is_file() {
        if let Some(status) = server.0.try_wait().unwrap() {
            panic!(
                "server exited before registering ({status}): {}",
                std::fs::read_to_string(&log).unwrap_or_default()
            );
        }
        assert!(Instant::now() < deadline, "server did not register");
        std::thread::sleep(Duration::from_millis(10));
    }
    let output = env.uninstall(&["--force"]);
    assert!(output.status.success(), "{}", text(&output.stderr));
    let deadline = Instant::now() + Duration::from_secs(30);
    let status = loop {
        if let Some(status) = server.0.try_wait().unwrap() {
            break status;
        }
        assert!(Instant::now() < deadline, "the server is still running");
        std::thread::sleep(Duration::from_millis(20));
    };
    assert!(status.success(), "{status}");
    assert!(!env.dir("state").exists());
    assert!(!env.dir("data").exists());
}

/// A fake package manager in `<root>/managers` that answers its listing with an
/// installed `cyber` package and records any other invocation in `<name>.args`.
#[cfg(unix)]
fn fake_manager(env: &Env, name: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let bin = env.root.join("managers");
    std::fs::create_dir_all(&bin).unwrap();
    let script = bin.join(name);
    std::fs::write(
        &script,
        "#!/bin/sh\ncase \"$1\" in\n  list|ls) echo 'cyber 1.0.0 \"@cyber-code/cli\"' ;;\n  *) echo \"$@\" > \"$0.args\" ;;\nesac\n",
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    bin
}

/// `exe` as a link to (or copy of) the build's binary.
#[cfg(unix)]
fn place_binary(exe: &Path) {
    std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
    let built = Path::new(env!("CARGO_BIN_EXE_cyber"));
    if std::fs::hard_link(built, exe).is_err() {
        std::fs::copy(built, exe).unwrap();
    }
}

/// Regression: the package manager's uninstall is listed before anything runs, and
/// runs for a binary its path shows that manager installed.
#[cfg(unix)]
#[test]
fn manager_uninstall_is_listed_before_it_runs() {
    let env = Env::new();
    let bin = fake_manager(&env, "npm");
    let exe = env.root.join("lib/node_modules/@cyber-code/cli/bin/cyber");
    place_binary(&exe);
    let run = |args: &[&str]| {
        let args = [&["uninstall", "--keep-config", "--keep-data"], args].concat();
        env.command(&exe, &args).env("PATH", &bin).output().unwrap()
    };
    let command = "package manager: npm rm -g @cyber-code/cli";
    let dry = run(&["--dry-run"]);
    assert!(dry.status.success(), "{dry:?}");
    assert!(text(&dry.stdout).contains(command), "{dry:?}");
    assert!(!bin.join("npm.args").exists(), "a dry run ran the manager");

    let forced = run(&["--force"]);
    assert!(forced.status.success(), "{forced:?}");
    let stdout = text(&forced.stdout);
    let listed = stdout.find(command).expect("the command is listed");
    let removed = stdout.find("Removed").unwrap_or(stdout.len());
    assert!(listed < removed, "{stdout}");
    let args = std::fs::read_to_string(bin.join("npm.args")).unwrap();
    assert_eq!(args.trim(), "rm -g @cyber-code/cli");
}

/// Regression: a manager whose listing merely names a cyber package did not install this
/// binary, so its uninstall is a note to run by hand, never run.
#[cfg(unix)]
#[test]
fn manager_known_from_listings_only_is_not_run() {
    let env = Env::new();
    let bin = fake_manager(&env, "brew");
    let exe = env.root.join("opt/tools/cyber");
    place_binary(&exe);
    let output = env
        .command(
            &exe,
            &["uninstall", "--keep-config", "--keep-data", "--force"],
        )
        .env("PATH", &bin)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(
        !text(&output.stdout).contains("package manager:"),
        "{output:?}"
    );
    let stderr = text(&output.stderr);
    assert!(
        stderr.contains("brew lists a cyber package")
            && stderr.contains("run: brew uninstall cyber"),
        "{stderr}"
    );
    assert!(!bin.join("brew.args").exists(), "brew uninstall ran");
    assert!(exe.is_file());
}

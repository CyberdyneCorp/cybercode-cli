//! `cyber mcp browser install|health|cleanup` through the real binary
//! (`browser-verification` → Supported optional browser integration): the pinned
//! definition lands in the chosen config document and loads as an MCP server, a project
//! install is refused while the checkout is untrusted, `health` starts the configured
//! server once and checks its tools, and `cleanup` removes abandoned profiles only.
use serde_json::{Value, json};
use std::path::PathBuf;
use std::process::{Command, Output};

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let text = root.display().to_string();
        let root = PathBuf::from(text.strip_prefix(r"\\?\").unwrap_or(&text));
        std::fs::create_dir_all(root.join("repo/.git")).unwrap();
        std::fs::create_dir_all(root.join("home")).unwrap();
        Self { _dir: dir, root }
    }

    fn repo(&self) -> PathBuf {
        self.root.join("repo")
    }

    fn cyber_home(&self) -> PathBuf {
        self.root.join("cyber")
    }

    fn cyber(&self, args: &[&str]) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_cyber"));
        command
            .args(["--cwd", self.repo().to_str().unwrap()])
            .args(args)
            .env_clear()
            .env("HOME", self.root.join("home"))
            .env("CYBER_HOME", self.cyber_home())
            .env("CYBER_DB", "test.db")
            .env("CYBER_MANAGED_POLICY_DIR", self.root.join("no-policy"))
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .current_dir(&self.root);
        if cfg!(windows) {
            for key in ["SystemRoot", "TEMP", "TMP", "USERPROFILE", "PATHEXT"] {
                if let Some(value) = std::env::var_os(key) {
                    command.env(key, value);
                }
            }
        }
        command.output().unwrap()
    }

    fn json(&self, args: &[&str]) -> Value {
        let mut args = args.to_vec();
        args.extend(["--format", "json"]);
        let output = self.cyber(&args);
        assert!(output.status.success(), "{args:?}: {}", explain(&output));
        serde_json::from_slice(&output.stdout).unwrap()
    }
}

fn explain(output: &Output) -> String {
    format!(
        "status {:?}\nstdout {}\nstderr {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn install_writes_the_pinned_server_and_project_install_follows_trust() {
    let f = Fixture::new();
    let installed = f.json(&["mcp", "browser", "install"]);
    let file = PathBuf::from(installed["file"].as_str().unwrap());
    assert!(file.starts_with(f.cyber_home()), "{installed}");
    let args = installed["definition"]["args"].to_string();
    assert!(args.contains(cyber_core::browser::PINNED_PACKAGE), "{args}");
    assert!(args.contains("--isolated"), "{args}");
    let document: Value =
        cyber_core::config::parse_jsonc("cyber.jsonc", &std::fs::read_to_string(&file).unwrap())
            .unwrap();
    assert_eq!(document["browser"], json!({"enabled": true}));
    let definitions = f.json(&["mcp", "definitions"]);
    assert!(
        definitions.to_string().contains("\"browser\""),
        "{definitions}"
    );

    // A checkout with unapproved project configuration gets no project browser.
    let project = f.repo().join("cyber.jsonc");
    let pending = r#"{"mcp": {"tools": {"type": "local", "command": "never-started"}}}"#;
    std::fs::write(&project, pending).unwrap();
    let refused = f.cyber(&["mcp", "browser", "install", "--scope", "project"]);
    assert_eq!(refused.status.code(), Some(2), "{}", explain(&refused));
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("not trusted"),
        "{}",
        explain(&refused)
    );
    assert!(!f.repo().join(".cyber/cyber.jsonc").exists());
    assert_eq!(std::fs::read_to_string(&project).unwrap(), pending);

    // Without pending configuration it is written, and starts only after approval.
    std::fs::remove_file(&project).unwrap();
    let written = f.json(&["mcp", "browser", "install", "--scope", "project"]);
    assert!(
        written["approval"]
            .as_str()
            .unwrap()
            .contains("cyber mcp trust browser --digest sha256:"),
        "{written}"
    );
    assert!(f.repo().join(".cyber/cyber.jsonc").exists());
}

#[test]
fn cleanup_removes_only_profiles_no_process_holds() {
    let f = Fixture::new();
    let profiles = f.cyber_home().join("data/browser/profiles");
    std::fs::create_dir_all(profiles.join("ses_gone/mcp-1")).unwrap();
    std::fs::write(profiles.join("ses_gone.lock"), "").unwrap();
    let report = f.json(&["mcp", "browser", "cleanup"]);
    assert_eq!(report["removed"].as_array().unwrap().len(), 1, "{report}");
    assert!(!profiles.join("ses_gone").exists());
    let again = f.json(&["mcp", "browser", "cleanup"]);
    assert_eq!(again["removed"], json!([]));
}

#[test]
fn health_needs_browser_support() {
    let f = Fixture::new();
    let output = f.cyber(&["mcp", "browser", "health"]);
    assert_eq!(output.status.code(), Some(1), "{}", explain(&output));
    assert!(String::from_utf8_lossy(&output.stderr).contains("browser support is off"));
}

/// The fake browser server of the cyber-app tests is a Python script.
#[cfg(unix)]
#[test]
fn health_starts_the_configured_server_and_checks_its_tools() {
    let f = Fixture::new();
    f.json(&["mcp", "browser", "install"]);
    let fixture = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../cyber-app/tests/support/browser_mcp.py"
    );
    let log = f.root.join("browser.jsonl");
    let server = json!({"type": "local", "command": "python3",
        "args": ["-u", fixture, log.display().to_string()]});
    let config = f.cyber_home().join("config/cyber.jsonc");
    cyber_core::config::set_document_member(&config, "mcp", "browser", Some(&server)).unwrap();
    let health = f.json(&["mcp", "browser", "health"]);
    assert_eq!(health["healthy"], true, "{health}");
    assert_eq!(health["pinned"], false);
    assert_eq!(health["missing"], json!([]));
    let events = std::fs::read_to_string(&log).unwrap();
    assert!(
        events.contains("\"start\"") && events.contains("\"exit\""),
        "{events}"
    );
    // Its profile is gone once it stopped.
    let profiles = f.cyber_home().join("data/browser/profiles");
    let left = std::fs::read_dir(&profiles).map_or(0, Iterator::count);
    assert_eq!(left, 0);
}

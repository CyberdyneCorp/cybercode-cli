//! `cyber sessions list|show|rename|delete|export` and Session lookup by title
//! (`cli-commands` → Resource command groups; `session-runtime` → Fork, rename, archive,
//! delete), through the real binary and the background server it starts.

use std::path::PathBuf;
use std::process::{Command, Output};

use serde_json::Value;

#[path = "../../cyber-app/tests/support/scripted_model.rs"]
mod scripted_model;

struct Env {
    _dir: tempfile::TempDir,
    root: PathBuf,
    catalog: PathBuf,
}

impl Env {
    /// A Location whose global default is the scripted model.
    fn new(model: &scripted_model::ScriptedModel) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let catalog = root.join("models.json");
        std::fs::write(&catalog, "{}").unwrap();
        let config = root.join("cyber-home/config");
        std::fs::create_dir_all(&config).unwrap();
        std::fs::write(config.join("cyber.jsonc"), model.config().to_string()).unwrap();
        std::fs::create_dir_all(root.join("repo/.git")).unwrap();
        Self {
            _dir: dir,
            root,
            catalog,
        }
    }

    fn cyber(&self, args: &[&str]) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_cyber"));
        command
            .args(args)
            .env_clear()
            .env("HOME", self.root.join("home"))
            .env("CYBER_HOME", self.root.join("cyber-home"))
            .env("CYBER_MODELS_PATH", &self.catalog)
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .current_dir(self.root.join("repo"));
        #[cfg(windows)]
        if let Some(system) = std::env::var_os("SystemRoot") {
            command.env("SystemRoot", system);
        }
        command.output().unwrap()
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = self.cyber(&["service", "stop"]);
    }
}

fn exits(output: &Output, code: i32) -> String {
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.status.code(), Some(code), "{text}");
    text
}

fn json(output: &Output) -> Value {
    exits(output, 0);
    serde_json::from_slice(&output.stdout).unwrap()
}

/// The Sessions listed, newest first, once both have their generated title.
fn titled(env: &Env) -> Vec<Value> {
    for _ in 0..500 {
        let list = json(&env.cyber(&["sessions", "list", "--format", "json"]));
        let rows = list.as_array().unwrap().clone();
        if rows.len() == 2 && rows.iter().all(|s| s["title"] == "same") {
            return rows;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    panic!("the Sessions did not get their generated titles");
}

#[test]
fn sessions_are_listed_renamed_and_resumed_by_title() {
    // Every reply, titles included, is `same`, so both Sessions get the same title.
    let model = scripted_model::ScriptedModel::start(|_| serde_json::json!({"content": "same"}));
    let env = Env::new(&model);
    for prompt in ["first task", "second task"] {
        exits(&env.cyber(&["exec", prompt]), 0);
    }
    let rows = titled(&env);
    for row in &rows {
        for key in ["id", "title", "directory", "updated_at", "status"] {
            assert!(!row[key].is_null(), "{key} in {row}");
        }
    }
    let older = rows[1]["id"].as_str().unwrap().to_owned();
    let newer = rows[0]["id"].as_str().unwrap().to_owned();

    // Two Sessions share the title: the name is ambiguous and both IDs are listed.
    let ambiguous = exits(&env.cyber(&["sessions", "show", "same"]), 2);
    assert!(
        ambiguous.contains(&older) && ambiguous.contains(&newer),
        "{ambiguous}"
    );

    // Rename the older one by a unique ID prefix; the title is cleaned by the server.
    let renamed = env.cyber(&[
        "sessions",
        "rename",
        &older[..older.len() - 4],
        "Auth",
        "refactor\u{7}",
    ]);
    assert!(exits(&renamed, 0).contains("Auth refactor"));
    let shown = json(&env.cyber(&["sessions", "show", "Auth refactor", "--format", "json"]));
    assert_eq!(shown["id"], older.as_str());
    assert_eq!(shown["title"], "Auth refactor");
    let table = exits(&env.cyber(&["sessions", "list"]), 0);
    assert!(
        table.contains(older.as_str()) && table.contains("Auth refactor"),
        "{table}"
    );

    // Resume it by title: the next prompt lands in the renamed Session.
    let resumed = env.cyber(&[
        "exec",
        "--session",
        "Auth refactor",
        "--format",
        "json",
        "go on",
    ]);
    let resumed: Value = serde_json::from_slice(&resumed.stdout)
        .unwrap_or_else(|_| panic!("{}", exits(&resumed, 0)));
    assert_eq!(resumed["session_id"], older.as_str());
    // The user-set title survives the new Turn.
    let shown = json(&env.cyber(&["sessions", "show", &older, "--format", "json"]));
    assert_eq!(shown["title"], "Auth refactor");

    // Unknown names, empty titles and deletes without --yes fail without changes.
    exits(&env.cyber(&["sessions", "show", "nope"]), 1);
    exits(&env.cyber(&["sessions", "rename", &older, " "]), 2);
    exits(&env.cyber(&["sessions", "delete", &newer]), 2);
    let later = exits(&env.cyber(&["sessions", "fork", &older]), 2);
    assert!(later.contains("not available in this build yet"), "{later}");
    let missing = exits(&env.cyber(&["sessions", "import", "x.json"]), 1);
    assert!(missing.contains("cannot read x.json"), "{missing}");
    exits(&env.cyber(&["sessions", "delete", &newer, "--yes"]), 0);
    let left = json(&env.cyber(&["sessions", "list", "--format", "json"]));
    assert_eq!(left.as_array().unwrap().len(), 1);
}

/// `storage-events` → Session export: JSON by default, Markdown with `--format md`, and
/// `--sanitize` drops prompt text and paths while keeping token totals and timings.
#[test]
fn sessions_export_as_json_markdown_and_sanitized() {
    let model = scripted_model::ScriptedModel::start(|_| serde_json::json!({"content": "noted"}));
    let env = Env::new(&model);
    let prompt = "summarize /private/secret-project/notes.txt for me";
    let ran = env.cyber(&["exec", "--format", "json", prompt]);
    let ran: Value =
        serde_json::from_slice(&ran.stdout).unwrap_or_else(|_| panic!("{}", exits(&ran, 0)));
    let id = ran["session_id"].as_str().unwrap().to_owned();

    let export = json(&env.cyber(&["sessions", "export", &id]));
    assert_eq!(export["version"], 1);
    assert_eq!(export["session"]["id"], id.as_str());
    assert_eq!(export["sanitized"], false);
    assert!(export.to_string().contains(prompt), "{export}");
    let messages = export["messages"].as_array().unwrap();
    assert_eq!(messages[0]["kind"], "user");
    assert_eq!(messages.last().unwrap()["text"], "noted");

    let md = exits(
        &env.cyber(&["sessions", "export", &id, "--format", "md"]),
        0,
    );
    assert!(md.contains(&format!("- Session: `{id}`")), "{md}");
    assert!(md.contains(&format!("**User** (user)\n\n{prompt}")), "{md}");
    assert!(md.contains("noted"), "{md}");

    assert_sanitized(&env, &id, &export);
    exits(
        &env.cyber(&["sessions", "export", &id, "--format", "table"]),
        2,
    );
}

/// `--sanitize`: no prompt, answer or path text, the same totals and timings.
fn assert_sanitized(env: &Env, id: &str, export: &Value) {
    let clean = json(&env.cyber(&["sessions", "export", id, "--sanitize"]));
    let text = clean.to_string();
    for secret in ["secret-project", "notes.txt", "summarize", "noted"] {
        assert!(!text.contains(secret), "{secret} leaked: {text}");
    }
    assert_eq!(clean["sanitized"], true);
    assert_eq!(clean["session"]["id"], id);
    assert_eq!(clean["totals"], export["totals"]);
    let last = clean["messages"]
        .as_array()
        .unwrap()
        .last()
        .unwrap()
        .clone();
    let first = export["messages"]
        .as_array()
        .unwrap()
        .last()
        .unwrap()
        .clone();
    assert_eq!(
        (&last["started_ms"], &last["ended_ms"]),
        (&first["started_ms"], &first["ended_ms"])
    );
    assert!(
        last["text"]
            .as_str()
            .unwrap()
            .starts_with("[redacted:text:")
    );
}

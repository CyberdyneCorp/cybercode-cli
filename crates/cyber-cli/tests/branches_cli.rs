//! `cyber sessions tree|checkout|label|export` (`cli-commands` → Session branch
//! commands) through the real binary and the background server it starts, with a
//! scripted model: the ASCII and JSON tree, the export of one or every branch, labels,
//! checkout by label, ID prefix and `--code`, an ambiguous prefix (exit 2) and a busy
//! Session (exit 1, `SessionBusyError`).

use std::path::PathBuf;
use std::process::{Command, Output};
use std::time::Duration;

use serde_json::{Value, json};

#[path = "../../cyber-app/tests/support/scripted_model.rs"]
mod scripted_model;

/// `WRITE-A` and `WRITE-B` write a file, `GATE` waits at the model, and anything else is
/// answered with text.
fn reply(body: &Value) -> Value {
    let last = body["messages"]
        .as_array()
        .and_then(|m| m.last())
        .cloned()
        .unwrap_or_default();
    if last["role"] == "tool" || body.to_string().contains("Write a short title") {
        return json!({ "content": "Done." });
    }
    let last = last.to_string();
    let (file, content) = match () {
        _ if last.contains("WRITE-A") => ("a.txt", "alpha\n"),
        _ if last.contains("WRITE-B") => ("b.txt", "beta\n"),
        _ if last.contains("GATE") => {
            return json!({ "gate": "busy", "then": { "content": "Done." } });
        }
        _ => return json!({ "content": "Done." }),
    };
    let arguments = json!({ "path": file, "content": content }).to_string();
    json!({ "tool_calls": [{ "index": 0, "id": format!("call_{file}"), "type": "function",
        "function": { "name": "write", "arguments": arguments } }] })
}

struct Env {
    _dir: tempfile::TempDir,
    root: PathBuf,
    catalog: PathBuf,
}

impl Env {
    /// A git Location (snapshots track git worktrees) whose default is the scripted model.
    fn new(model: &scripted_model::ScriptedModel) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let catalog = root.join("models.json");
        std::fs::write(&catalog, "{}").unwrap();
        let config = root.join("cyber-home/config");
        std::fs::create_dir_all(&config).unwrap();
        std::fs::write(config.join("cyber.jsonc"), model.config().to_string()).unwrap();
        let repo = root.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let git = Command::new("git")
            .args(["init", "-q", "-b", "main"])
            .current_dir(&repo)
            .status()
            .unwrap();
        assert!(git.success());
        Self {
            _dir: dir,
            root,
            catalog,
        }
    }

    fn command(&self, args: &[&str]) -> Command {
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
        command
    }

    fn cyber(&self, args: &[&str]) -> Output {
        self.command(args).output().unwrap()
    }

    fn file(&self, name: &str) -> Option<String> {
        std::fs::read_to_string(self.root.join("repo").join(name)).ok()
    }

    /// Run one prompt in `session` (a new Session when `None`); returns the Session ID.
    fn exec(&self, session: Option<&str>, prompt: &str) -> String {
        let mut args = vec!["exec", "--mode", "accept-edits", "--format", "json"];
        if let Some(session) = session {
            args.extend(["--session", session]);
        }
        args.push(prompt);
        let output = self.cyber(&args);
        let text = exits(&output, 0);
        let done: Value =
            serde_json::from_slice(&output.stdout).unwrap_or_else(|_| panic!("{text}"));
        done["session_id"].as_str().unwrap().to_owned()
    }

    /// The active path's messages, as `GET .../messages` returns them.
    fn messages(&self, session: &str) -> Vec<Value> {
        let path = format!("/sessions/{session}/messages?order=asc&limit=200");
        json(&self.cyber(&["api", "GET", &path]))["data"]
            .as_array()
            .unwrap()
            .clone()
    }

    fn user_ids(&self, session: &str) -> Vec<String> {
        self.messages(session)
            .iter()
            .filter(|m| m["kind"] == "user")
            .map(|m| m["id"].as_str().unwrap().to_owned())
            .collect()
    }

    fn tree(&self, session: &str) -> Vec<Value> {
        json(&self.cyber(&["sessions", "tree", session, "--format", "json"]))
            .as_array()
            .unwrap()
            .clone()
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
    let text = exits(output, 0);
    serde_json::from_slice(&output.stdout).unwrap_or_else(|_| panic!("{text}"))
}

/// The scenario "Checkout from the CLI": two branches after a `both` rewind; the tree
/// marks the active path, labels resolve, `--code` restores the labeled branch's files
/// and prints the tip and the restored file count.
#[test]
fn branches_are_shown_labeled_and_checked_out_from_the_cli() {
    let model = scripted_model::ScriptedModel::start(reply);
    let env = Env::new(&model);
    let (id, first, second) = two_attempts(&env);
    shows_the_tree(&env, &id, &first, &second);
    exports_the_active_path_or_every_branch(&env, &id, &first, &second);
    let tip = checks_out_by_label_with_code(&env, &id, &first);
    labels_the_active_tip(&env, &id, &first, &tip);

    // Checkout by full ID, without code, goes back to the second attempt.
    let back = exits(&env.cyber(&["sessions", "checkout", &id, &second]), 0);
    assert!(back.contains("tip "), "{back}");
    assert!(!back.contains("restored"), "{back}");
    assert_eq!(env.user_ids(&id), std::slice::from_ref(&second));
    assert_eq!(
        env.file("a.txt").as_deref(),
        Some("alpha\n"),
        "files are left alone"
    );
    refuses_while_busy(&env, &model, &id);
}

/// `WRITE-A`, a `both` rewind to it, then `WRITE-B`, in a Session titled `Auth
/// refactor`; returns the Session and the two user messages.
fn two_attempts(env: &Env) -> (String, String, String) {
    let id = env.exec(None, "WRITE-A");
    let first = env.user_ids(&id)[0].clone();
    assert_eq!(env.file("a.txt").as_deref(), Some("alpha\n"));
    let rewind = json!({ "message_id": first, "target": "both" }).to_string();
    let path = format!("/sessions/{id}/rewind");
    exits(&env.cyber(&["api", "POST", &path, "-d", &rewind]), 0);
    assert_eq!(env.file("a.txt"), None);
    env.exec(Some(&id), "WRITE-B");
    let second = env.user_ids(&id)[0].clone();
    exits(
        &env.cyber(&["sessions", "rename", &id, "Auth", "refactor"]),
        0,
    );
    (id, first, second)
}

/// The ASCII tree has two roots with the active one marked; an ambiguous prefix lists
/// the matches and changes nothing.
fn shows_the_tree(env: &Env, id: &str, first: &str, second: &str) {
    let ascii = exits(&env.cyber(&["sessions", "tree", "Auth refactor"]), 0);
    assert!(
        ascii.contains(&format!("├─ o {first}  WRITE-A\n")),
        "{ascii}"
    );
    assert!(
        ascii.contains(&format!("└─ * {second}  WRITE-B  (active)\n")),
        "{ascii}"
    );
    let nodes = env.tree(id);
    assert_eq!(nodes.len(), 2);

    // An ambiguous prefix lists the matches and changes nothing.
    let ambiguous = exits(&env.cyber(&["sessions", "checkout", id, "msg_"]), 2);
    assert!(
        ambiguous.contains(first) && ambiguous.contains(second),
        "{ambiguous}"
    );
    exits(&env.cyber(&["sessions", "checkout", id, "nope"]), 1);
}

/// `export` writes the active path; `--all-branches` adds the inactive branch, linked by
/// `parent_message_id`.
fn exports_the_active_path_or_every_branch(env: &Env, id: &str, first: &str, second: &str) {
    let user_ids = |export: &Value| -> Vec<String> {
        assert_eq!(export["session"]["id"], id);
        let messages = export["messages"].as_array().unwrap();
        let ids = |m: &&Value| m["id"].as_str().map(str::to_owned);
        assert!(
            messages
                .iter()
                .all(|m| m.get("parent_message_id").is_some())
        );
        messages
            .iter()
            .filter(|m| m["kind"] == "user")
            .filter_map(|m| ids(&m))
            .collect()
    };
    let active = json(&env.cyber(&["sessions", "export", "Auth refactor"]));
    assert_eq!(active["branches"], "active");
    assert_eq!(user_ids(&active), [second]);
    let all = json(&env.cyber(&["sessions", "export", id, "--all-branches"]));
    assert_eq!(all["branches"], "all");
    assert_eq!(user_ids(&all), [first, second]);
    let messages = all["messages"].as_array().unwrap();
    let roots: Vec<&Value> = messages
        .iter()
        .filter(|m| m["parent_message_id"].is_null())
        .collect();
    assert_eq!(roots.len(), 2, "both attempts start at the root: {all}");
}

/// Label the first attempt by a unique prefix, then check it out by label with code;
/// returns the new tip.
fn checks_out_by_label_with_code(env: &Env, id: &str, first: &str) -> String {
    let prefix = &first[..first.len() - 2];
    let labeled = exits(
        &env.cyber(&["sessions", "label", id, "fix-a", "--at", prefix]),
        0,
    );
    assert!(labeled.contains(first), "{labeled}");
    exits(&env.cyber(&["sessions", "label", id, "bad label"]), 2);
    let done = exits(
        &env.cyber(&["sessions", "checkout", "Auth refactor", "fix-a", "--code"]),
        0,
    );
    let tip = env.messages(id).last().unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(done.contains(&format!("tip {tip}")), "{done}");
    assert!(done.contains("restored 2 files"), "{done}");
    assert_eq!(env.file("a.txt").as_deref(), Some("alpha\n"));
    assert_eq!(env.file("b.txt"), None);
    assert_eq!(env.user_ids(id), [first]);
    let nodes = env.tree(id);
    let node = |id: &str| nodes.iter().find(|n| n["id"] == id).unwrap().clone();
    assert_eq!(node(first)["active"], true);
    assert_eq!(node(first)["labels"], json!(["fix-a"]));
    tip
}

/// Without --at the active tip is labeled; --delete removes a label once.
fn labels_the_active_tip(env: &Env, id: &str, first: &str, tip: &str) {
    let here = exits(&env.cyber(&["sessions", "label", id, "here"]), 0);
    assert!(here.contains(tip), "{here}");
    let mut labels = env.tree(id).iter().find(|n| n["id"] == first).unwrap()["labels"].clone();
    labels
        .as_array_mut()
        .unwrap()
        .sort_by_key(|l| l.to_string());
    assert_eq!(labels, json!(["fix-a", "here"]));
    exits(
        &env.cyber(&["sessions", "label", id, "here", "--delete"]),
        0,
    );
    exits(
        &env.cyber(&["sessions", "label", id, "here", "--delete"]),
        1,
    );
}

/// A busy Session refuses with exit 1 and SessionBusyError.
fn refuses_while_busy(env: &Env, model: &scripted_model::ScriptedModel, id: &str) {
    let mut gated = env
        .command(&["exec", "--session", id, "--format", "json", "GATE"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    for _ in 0..1000 {
        if !model.held().is_empty() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(!model.held().is_empty(), "GATE never reached the model");
    let busy = exits(&env.cyber(&["sessions", "checkout", id, "fix-a"]), 1);
    assert!(busy.contains("SessionBusyError"), "{busy}");
    model.open_gate();
    assert!(gated.wait().unwrap().success());
    assert_eq!(env.user_ids(id).len(), 2);
}

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
    // A sanitized export keeps the tree: IDs and parent links are not content.
    let clean = json(&env.cyber(&["sessions", "export", id, "--all-branches", "--sanitize"]));
    assert_eq!(clean["branches"], "all");
    let links = |export: &Value| -> Vec<Value> {
        export["messages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["parent_message_id"].clone())
            .collect()
    };
    assert_eq!(links(&clean), links(&all));
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

fn rewind_preview(env: &Env, session: &str, message: &str, target: &str) -> Value {
    json(&env.cyber(&[
        "sessions", "rewind", session, "--to", message, "--target", target, "--format", "json",
    ]))
}

fn confirm_rewind(
    env: &Env,
    session: &str,
    message: &str,
    target: &str,
    preview: &Value,
) -> Output {
    env.cyber(&[
        "sessions",
        "rewind",
        session,
        "--to",
        message,
        "--target",
        target,
        "--confirm",
        preview["confirmation"].as_str().unwrap(),
        "--format",
        "json",
    ])
}

#[test]
fn rewind_preview_is_non_mutating_and_stale_files_refuse_before_restore() {
    let model = scripted_model::ScriptedModel::start(reply);
    let env = Env::new(&model);
    let id = env.exec(None, "WRITE-A");
    env.exec(Some(&id), "WRITE-B");
    std::fs::write(env.root.join("repo/owner.txt"), "owner file\n").unwrap();
    let users = env.user_ids(&id);
    let preview = rewind_preview(&env, &id, &users[0], "code");
    assert_eq!(env.file("a.txt").as_deref(), Some("alpha\n"));
    assert_eq!(env.file("b.txt").as_deref(), Some("beta\n"));
    assert_eq!(env.user_ids(&id), users);
    assert_eq!(preview["confirmation"].as_str().unwrap().len(), 64);
    let files = preview["code"]["files"].as_array().unwrap();
    assert_eq!(files.len(), 2);
    assert!(
        files.iter().all(|file| file["file"] != "owner.txt"),
        "unrelated owner files must not be shown as proposed deletions"
    );
    assert_owner_drift_refused(&env, &id, &users, &preview);
    let fresh = rewind_preview(&env, &id, &users[0], "code");
    json(&confirm_rewind(&env, &id, &users[0], "code", &fresh));
    assert_eq!(env.file("a.txt"), None);
    assert_eq!(env.file("b.txt"), None);
    assert_eq!(env.file("owner.txt").as_deref(), Some("owner change\n"));
    assert_eq!(
        env.user_ids(&id),
        users,
        "code-only keeps the active conversation"
    );
    exits(&confirm_rewind(&env, &id, &users[0], "code", &fresh), 1);
}

fn assert_owner_drift_refused(env: &Env, id: &str, users: &[String], preview: &Value) {
    std::fs::write(env.root.join("repo/owner.txt"), "owner change\n").unwrap();
    let refused = exits(&confirm_rewind(env, id, &users[0], "code", preview), 1);
    assert!(refused.contains("preview changed"), "{refused}");
    assert_eq!(env.file("owner.txt").as_deref(), Some("owner change\n"));
    assert_eq!(env.file("a.txt").as_deref(), Some("alpha\n"));
    assert_eq!(env.file("b.txt").as_deref(), Some("beta\n"));
    assert_eq!(env.user_ids(id), users);
    std::fs::write(env.root.join("repo/a.txt"), "conflicting owner edit\n").unwrap();
    let refused = exits(
        &env.cyber(&[
            "sessions", "rewind", id, "--to", &users[0], "--target", "code",
        ]),
        1,
    );
    assert!(refused.contains("RewindConflictError"), "{refused}");
    assert_eq!(
        env.file("a.txt").as_deref(),
        Some("conflicting owner edit\n")
    );
    std::fs::write(env.root.join("repo/a.txt"), "alpha\n").unwrap();
}

#[test]
fn confirmed_combined_rewind_keeps_an_inactive_branch_and_rejects_cross_session_review() {
    let model = scripted_model::ScriptedModel::start(reply);
    let env = Env::new(&model);
    let id = env.exec(None, "WRITE-A");
    env.exec(Some(&id), "WRITE-B");
    let users = env.user_ids(&id);
    let preview = rewind_preview(&env, &id, &users[1], "both");
    let other = env.exec(None, "another session");
    let other_user = env.user_ids(&other)[0].clone();
    exits(
        &confirm_rewind(&env, &other, &other_user, "conversation", &preview),
        1,
    );
    json(&confirm_rewind(&env, &id, &users[1], "both", &preview));
    assert_eq!(env.file("a.txt").as_deref(), Some("alpha\n"));
    assert_eq!(env.file("b.txt"), None);
    assert_eq!(env.user_ids(&id), users[..1]);
    let tree = env.tree(&id);
    assert!(
        tree.iter()
            .any(|node| node["id"] == users[1] && node["active"] == false)
    );
    let exported = json(&env.cyber(&["sessions", "export", &id, "--all-branches"]));
    assert!(
        exported.to_string().contains(&users[1]),
        "inactive messages remain exportable"
    );
}

#[test]
fn conversation_rewind_works_without_git_and_stale_context_never_applies() {
    let model = scripted_model::ScriptedModel::start(reply);
    let env = Env::new(&model);
    std::fs::remove_dir_all(env.root.join("repo/.git")).unwrap();
    let mut config = model.config();
    config["snapshots"] = json!(false);
    std::fs::write(
        env.root.join("cyber-home/config/cyber.jsonc"),
        config.to_string(),
    )
    .unwrap();
    let id = env.exec(None, "first prompt");
    let first = env.user_ids(&id)[0].clone();
    let refused = exits(&env.cyber(&["sessions", "rewind", &id, "--to", &first]), 2);
    assert!(refused.contains("Snapshots are unavailable"), "{refused}");
    let preview = rewind_preview(&env, &id, &first, "conversation");
    assert_eq!(preview["code"], Value::Null);
    env.exec(Some(&id), "second prompt");
    let before = env.user_ids(&id);
    let refused = exits(
        &confirm_rewind(&env, &id, &first, "conversation", &preview),
        1,
    );
    assert!(refused.contains("preview changed"), "{refused}");
    assert_eq!(env.user_ids(&id), before);
    let fresh = rewind_preview(&env, &id, &first, "conversation");
    json(&confirm_rewind(&env, &id, &first, "conversation", &fresh));
    assert!(env.user_ids(&id).is_empty());
    assert!(!env.tree(&id).is_empty());
    exits(
        &confirm_rewind(&env, &id, &first, "conversation", &fresh),
        2,
    );
}

/// Use the same authenticated reviewed actions as the TUI, through the actual CLI.
fn reviewed_selection(env: &Env, message: &str, target: &str, action: &str) -> Value {
    json!({"message_id":message,"target":target,"action":action,
        "expected_directory":env.root.join("repo").display().to_string()})
}
fn reviewed_preview(env: &Env, id: &str, selection: &Value) -> Value {
    json(&env.cyber(&[
        "api",
        "POST",
        &format!("/sessions/{id}/rewind/preview"),
        "-d",
        &selection.to_string(),
    ]))["data"]
        .clone()
}
fn reviewed_confirm(env: &Env, id: &str, selection: &Value, preview: &Value) -> Output {
    let mut body = selection.clone();
    body["confirmation"] = preview["confirmation"].clone();
    env.cyber(&[
        "api",
        "POST",
        &format!("/sessions/{id}/rewind/confirm"),
        "-d",
        &body.to_string(),
    ])
}
fn reviewed_apply(env: &Env, id: &str, selection: &Value) -> Value {
    let preview = reviewed_preview(env, id, selection);
    json(&reviewed_confirm(env, id, selection, &preview))["data"].clone()
}
fn session_revert(env: &Env, id: &str) -> Value {
    json(&env.cyber(&["api", "GET", &format!("/sessions/{id}")]))["data"]["revert"].clone()
}

/// Only kill the service registered in this fixture's private CYBER_HOME.
fn kill_registered_service(env: &Env) {
    let path = env.root.join("cyber-home/state/server.json");
    let registration: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let pid = registration["pid"].as_u64().unwrap().to_string();
    assert_ne!(pid, std::process::id().to_string());
    #[cfg(unix)]
    let killed = Command::new("kill").args(["-KILL", &pid]).output().unwrap();
    #[cfg(windows)]
    let killed = Command::new("taskkill")
        .args(["/F", "/PID", &pid])
        .output()
        .unwrap();
    assert!(killed.status.success(), "{killed:?}");
    let address = registration["url"]
        .as_str()
        .unwrap()
        .trim_start_matches("http://")
        .trim_end_matches('/');
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while std::net::TcpStream::connect(address).is_ok() {
        assert!(
            std::time::Instant::now() < deadline,
            "killed fixture service still answers"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn reviewed_recovery_code_stages_survive_kill_restage_and_clear_without_adopting_owner_files() {
    code_stage_recovery("code");
    code_stage_recovery("both");
}

fn code_stage_recovery(target: &str) {
    let model = scripted_model::ScriptedModel::start(reply);
    let env = Env::new(&model);
    let id = env.exec(None, "WRITE-A");
    env.exec(Some(&id), "WRITE-B");
    let users = env.user_ids(&id);
    let selection = reviewed_selection(&env, &users[0], target, "stage");
    let staged = reviewed_apply(&env, &id, &selection);
    assert_eq!(env.file("a.txt"), None);
    assert_eq!(env.file("b.txt"), None);
    assert_eq!(env.user_ids(&id), users, "stage does not commit the branch");
    let requests = model.requests().len();
    kill_registered_service(&env);
    assert_eq!(session_revert(&env, &id), staged);
    std::fs::write(env.root.join("repo/owner.txt"), "owner after restart\n").unwrap();
    let later = reviewed_selection(&env, &users[1], target, "stage");
    let restaged = reviewed_apply(&env, &id, &later);
    assert_eq!(restaged["baseline"], staged["baseline"]);
    assert_eq!(restaged["paths"], staged["paths"]);
    assert_eq!(env.file("a.txt").as_deref(), Some("alpha\n"));
    assert_eq!(env.file("b.txt"), None);
    kill_registered_service(&env);
    assert_eq!(session_revert(&env, &id), restaged);
    clear_recovered_code_stage(&env, &id, &users, target, &restaged);
    assert_eq!(
        model.requests().len(),
        requests,
        "recovery never starts inference"
    );
}

fn clear_recovered_code_stage(
    env: &Env,
    id: &str,
    users: &[String],
    target: &str,
    restaged: &Value,
) {
    let clear = reviewed_selection(env, &users[1], target, "clear");
    let preview = reviewed_preview(env, id, &clear);
    std::fs::write(env.root.join("repo/owner.txt"), "newer owner bytes\n").unwrap();
    let refused = exits(&reviewed_confirm(env, id, &clear, &preview), 1);
    assert!(refused.contains("preview changed"), "{refused}");
    assert_eq!(&session_revert(env, id), restaged);
    reviewed_apply(env, id, &clear);
    assert!(session_revert(env, id).is_null());
    assert_eq!(env.file("a.txt").as_deref(), Some("alpha\n"));
    assert_eq!(env.file("b.txt").as_deref(), Some("beta\n"));
    assert_eq!(
        env.file("owner.txt").as_deref(),
        Some("newer owner bytes\n")
    );
    assert_eq!(env.user_ids(id), users);
}

#[test]
fn reviewed_recovery_conversation_stage_survives_kill_and_commits_on_new_input_without_git() {
    let model = scripted_model::ScriptedModel::start(reply);
    let env = Env::new(&model);
    std::fs::remove_dir_all(env.root.join("repo/.git")).unwrap();
    let id = env.exec(None, "first prompt");
    env.exec(Some(&id), "second prompt");
    let users = env.user_ids(&id);
    let selection = reviewed_selection(&env, &users[1], "conversation", "stage");
    let staged = reviewed_apply(&env, &id, &selection);
    kill_registered_service(&env);
    assert_eq!(session_revert(&env, &id), staged);
    assert_eq!(env.user_ids(&id), users);
    env.exec(Some(&id), "new continuation");
    assert!(session_revert(&env, &id).is_null());
    let active = env.user_ids(&id);
    assert_eq!(active.len(), 2);
    assert_eq!(active[0], users[0]);
    assert!(!active.contains(&users[1]));
    let exported = json(&env.cyber(&["sessions", "export", &id, "--all-branches"]));
    assert!(exported.to_string().contains(&users[1]));
}

#[test]
fn reviewed_recovery_disabled_snapshots_keep_conversation_clear_available_after_restart() {
    let model = scripted_model::ScriptedModel::start(reply);
    let env = Env::new(&model);
    let mut config = model.config();
    config["snapshots"] = json!(false);
    std::fs::write(
        env.root.join("cyber-home/config/cyber.jsonc"),
        config.to_string(),
    )
    .unwrap();
    let id = env.exec(None, "first prompt");
    let first = env.user_ids(&id)[0].clone();
    let refused = exits(&env.cyber(&["sessions", "rewind", &id, "--to", &first]), 2);
    assert!(refused.contains("Snapshots are unavailable"), "{refused}");
    let stage = reviewed_selection(&env, &first, "conversation", "stage");
    reviewed_apply(&env, &id, &stage);
    kill_registered_service(&env);
    let clear = reviewed_selection(&env, &first, "conversation", "clear");
    reviewed_apply(&env, &id, &clear);
    assert!(session_revert(&env, &id).is_null());
    assert_eq!(env.user_ids(&id), [first]);
}

#[test]
fn reviewed_recovery_busy_confirmation_requires_interrupt_ack_and_a_new_review() {
    let model = scripted_model::ScriptedModel::start(reply);
    let env = Env::new(&model);
    let id = env.exec(None, "WRITE-A");
    let first = env.user_ids(&id)[0].clone();
    let preview = rewind_preview(&env, &id, &first, "both");
    let mut gated = env
        .command(&["exec", "--session", &id, "--format", "json", "GATE"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while model.held().is_empty() {
        assert!(
            std::time::Instant::now() < deadline,
            "GATE never reached provider"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    let refused = exits(&confirm_rewind(&env, &id, &first, "both", &preview), 1);
    assert!(refused.contains("SessionBusyError"), "{refused}");
    assert_eq!(env.file("a.txt").as_deref(), Some("alpha\n"));
    interrupt_and_wait_idle(&env, &id);
    model.open_gate();
    // The owner API settles server work; it does not send SIGINT to exec.
    assert!(gated.wait().unwrap().success());
    let users = env.user_ids(&id);
    assert_eq!(users.len(), 2);
    let refused = exits(&confirm_rewind(&env, &id, &first, "both", &preview), 1);
    assert!(refused.contains("preview changed"), "{refused}");
    assert_eq!(env.user_ids(&id), users);
    assert_eq!(env.file("a.txt").as_deref(), Some("alpha\n"));
    let fresh = rewind_preview(&env, &id, &first, "both");
    json(&confirm_rewind(&env, &id, &first, "both", &fresh));
    assert_eq!(env.file("a.txt"), None);
    assert!(env.user_ids(&id).is_empty());
    let exported = json(&env.cyber(&["sessions", "export", &id, "--all-branches"]));
    for user in users {
        assert!(exported.to_string().contains(&user));
    }
}

fn interrupt_and_wait_idle(env: &Env, id: &str) {
    exits(
        &env.cyber(&[
            "api",
            "POST",
            &format!("/sessions/{id}/interrupt"),
            "-d",
            "{}",
        ]),
        0,
    );
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let status =
            json(&env.cyber(&["api", "GET", &format!("/sessions/{id}")]))["data"]["status"].clone();
        if status == "idle" {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "interrupt never acknowledged idle: {status}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn recovery_review_binds_the_session_discloses_acknowledgement_and_starts_no_inference() {
    let model = scripted_model::ScriptedModel::start(reply);
    let env = Env::new(&model);
    std::fs::remove_dir_all(env.root.join("repo/.git")).unwrap();
    let id = env.exec(None, "WRITE-A");
    let first = env.user_ids(&id)[0].clone();
    reviewed_apply(
        &env,
        &id,
        &reviewed_selection(&env, &first, "code", "commit"),
    );
    let catalog = json(&env.cyber(&["api", "GET", &format!("/sessions/{id}/rewind/recovery")]));
    let journal = catalog["data"][0]["id"].as_str().unwrap();
    let created = json(&env.cyber(&["api", "POST", "/sessions", "-d", "{\"title\":\"foreign\"}"]));
    let other = created["data"]["id"].as_str().unwrap().to_owned();
    let requests = model.requests().len();
    let original_messages = env.messages(&id);
    for action in ["keep", "rollback"] {
        let body = json!({"journal_id":journal,"action":action}).to_string();
        let response = json(&env.cyber(&[
            "api",
            "POST",
            &format!("/sessions/{id}/rewind/recovery/review"),
            "-d",
            &body,
        ]));
        let review = &response["data"];
        assert_eq!(review["origin"]["session_id"], id);
        assert_eq!(review["acknowledged"], true);
        assert!(review["confirmation"].is_null());
        assert!(!review["conflicts"].as_array().unwrap().is_empty());
        assert!(review["exclusions"].is_array());
        let foreign = env.cyber(&[
            "api",
            "POST",
            &format!("/sessions/{other}/rewind/recovery/review"),
            "-d",
            &body,
        ]);
        assert!(!foreign.status.success());
        assert!(
            String::from_utf8_lossy(&foreign.stdout).contains("different Session")
                || String::from_utf8_lossy(&foreign.stderr).contains("different Session")
        );
    }
    assert_eq!(env.file("a.txt"), None);
    assert_eq!(env.messages(&id), original_messages);
    assert_eq!(model.requests().len(), requests);
}

#[test]
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn reviewed_non_git_stage_and_clear_keep_exact_acknowledged_worker_receipts() {
    let model = scripted_model::ScriptedModel::start(reply);
    let env = Env::new(&model);
    std::fs::remove_dir_all(env.root.join("repo/.git")).unwrap();
    let id = env.exec(None, "WRITE-A");
    let first = env.user_ids(&id)[0].clone();
    std::fs::write(env.root.join("repo/owner.txt"), "owner before stage").unwrap();
    let requests = model.requests().len();
    reviewed_apply(
        &env,
        &id,
        &reviewed_selection(&env, &first, "code", "stage"),
    );
    assert_eq!(env.file("a.txt"), None);
    std::fs::write(env.root.join("repo/owner.txt"), "later owner bytes").unwrap();
    reviewed_apply(
        &env,
        &id,
        &reviewed_selection(&env, &first, "code", "clear"),
    );
    assert_eq!(env.file("a.txt").as_deref(), Some("alpha\n"));
    assert_eq!(env.file("owner.txt").as_deref(), Some("later owner bytes"));
    assert!(session_revert(&env, &id).is_null());
    let catalog = json(&env.cyber(&["api", "GET", &format!("/sessions/{id}/rewind/recovery")]));
    let journals = catalog["data"].as_array().unwrap();
    assert_eq!(journals.len(), 2);
    for journal in journals {
        assert_owned_snapshot_receipt(&env, &id, journal["id"].as_str().unwrap());
    }
    assert_eq!(model.requests().len(), requests);
}

#[test]
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn reviewed_non_git_code_rewind_uses_private_snapshots_and_keeps_owner_files() {
    let model = scripted_model::ScriptedModel::start(reply);
    let env = Env::new(&model);
    std::fs::remove_dir_all(env.root.join("repo/.git")).unwrap();
    let id = env.exec(None, "WRITE-A");
    let first = env.user_ids(&id)[0].clone();
    std::fs::write(env.root.join("repo/owner.txt"), "owner bytes\n").unwrap();
    let preview = rewind_preview(&env, &id, &first, "both");
    assert!(
        preview["code"]["target"]
            .as_str()
            .unwrap()
            .starts_with("copy:")
    );
    let requests = model.requests().len();
    json(&confirm_rewind(&env, &id, &first, "both", &preview));
    let journals =
        json(&env.cyber(&["api", "GET", &format!("/sessions/{id}/rewind/recovery")]))["data"]
            .as_array()
            .unwrap()
            .clone();
    assert_eq!(journals.len(), 1);
    assert_eq!(journals[0]["filesystem_complete"], true);
    assert_owned_snapshot_receipt(&env, &id, journals[0]["id"].as_str().unwrap());
    assert!(
        journals[0]["paths"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p == "a.txt")
    );
    assert_eq!(env.file("a.txt"), None);
    assert_eq!(env.file("owner.txt").as_deref(), Some("owner bytes\n"));
    assert!(env.user_ids(&id).is_empty());
    assert_eq!(model.requests().len(), requests);
    let all = json(&env.cyber(&["sessions", "export", &id, "--all-branches"]));
    assert!(all.to_string().contains(&first));
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn assert_owned_snapshot_receipt(env: &Env, session: &str, journal: &str) {
    let namespaces: Vec<_> = std::fs::read_dir(env.root.join("cyber-home/data/snapshot/global"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(namespaces.len(), 1);
    let intent: Value = serde_json::from_slice(
        &std::fs::read(
            namespaces[0]
                .join("journals")
                .join(journal)
                .join("intent.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(intent["version"], 3);
    assert_eq!(intent["ownership"]["version"], 1);
    assert_eq!(
        intent["ownership"]["files"].as_array().unwrap().len(),
        intent["changes"].as_array().unwrap().len()
    );
    assert_eq!(intent["completed"], true);
    assert_eq!(intent["acknowledged"], true);
    let origin = &intent["origin"];
    assert_eq!(origin["session_id"], session);
    assert_eq!(origin["binding"]["journal_id"], journal);
    let databases: Vec<_> = std::fs::read_dir(env.root.join("cyber-home/data"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "db"))
        .collect();
    assert_eq!(
        databases.len(),
        1,
        "fixture must identify the actual channel database"
    );
    let db = rusqlite::Connection::open_with_flags(
        &databases[0],
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let data: String = db
        .query_row(
            "SELECT data FROM event WHERE aggregate_id=?1 ORDER BY seq DESC LIMIT 1",
            [origin["activity_id"].as_str().unwrap()],
            |row| row.get(0),
        )
        .unwrap();
    let activity: Value = serde_json::from_str(&data).unwrap();
    assert_eq!(activity["snapshot_origin"], *origin);
    assert_eq!(activity["status"], "settled");
    assert_eq!(activity["phase"], "snapshot_restore_committed");
    let created: String = db
        .query_row(
            "SELECT id FROM event WHERE aggregate_id=?1 ORDER BY seq LIMIT 1",
            [session],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(origin["session_created_id"], created);
}

#[test]
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn reviewed_non_git_cap_warns_once_without_failing_the_conversation() {
    let model = scripted_model::ScriptedModel::start(reply);
    let env = Env::new(&model);
    std::fs::remove_dir_all(env.root.join("repo/.git")).unwrap();
    let mut config = model.config();
    config["snapshots"] = json!({"fallback_max_bytes":1});
    std::fs::write(
        env.root.join("cyber-home/config/cyber.jsonc"),
        config.to_string(),
    )
    .unwrap();
    let output = env.cyber(&["exec", "--format", "json", "first prompt"]);
    exits(&output, 0);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(stderr.matches("snapshot warning:").count(), 1, "{stderr}");
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    let id = result["session_id"].as_str().unwrap();
    let first = env.user_ids(id)[0].clone();
    let refused = exits(&env.cyber(&["sessions", "rewind", id, "--to", &first]), 2);
    assert!(refused.contains("cap reached"), "{refused}");
    let review = rewind_preview(&env, id, &first, "conversation");
    json(&confirm_rewind(&env, id, &first, "conversation", &review));
    assert!(env.user_ids(id).is_empty());
}

#[test]
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn non_git_recovery_inspection_creates_no_namespace_and_starts_no_inference() {
    let model = scripted_model::ScriptedModel::start(reply);
    let env = Env::new(&model);
    std::fs::remove_dir_all(env.root.join("repo/.git")).unwrap();
    let session = json(&env.cyber(&[
        "api",
        "POST",
        "/sessions",
        "-d",
        &json!({"model":"test/main","mode":"accept-edits"}).to_string(),
    ]));
    let id = session["data"]["id"].as_str().unwrap();
    let result = json(&env.cyber(&["api", "GET", &format!("/sessions/{id}/rewind/recovery")]));
    assert_eq!(result["data"], json!([]));
    assert!(model.requests().is_empty());
    let registration: Value = serde_json::from_slice(
        &std::fs::read(env.root.join("cyber-home/state/server.json")).unwrap(),
    )
    .unwrap();
    let url = format!(
        "{}/api/v1/sessions/{id}/rewind/recovery",
        registration["url"].as_str().unwrap().trim_end_matches('/')
    );
    let status = tokio::runtime::Runtime::new().unwrap().block_on(async {
        reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap()
            .get(url)
            .send()
            .await
            .unwrap()
            .status()
    });
    assert_eq!(status, reqwest::StatusCode::UNAUTHORIZED);
    assert!(!env.root.join("cyber-home/data/snapshot/global").exists());
}

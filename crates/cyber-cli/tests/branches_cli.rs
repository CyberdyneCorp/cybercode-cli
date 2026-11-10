//! `cyber sessions tree|checkout|label|export` (`cli-commands` → Session branch
//! commands) through the real binary and the background server it starts, with a
//! scripted model: the ASCII and JSON tree, the export of one or every branch, labels,
//! checkout by label, ID prefix and `--code`, an ambiguous prefix (exit 2) and a busy
//! Session (exit 1, `SessionBusyError`).

use std::path::PathBuf;
use std::process::{Command, Output};
use std::time::Duration;

use serde_json::{Value, json};

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[path = "support/reclamation.rs"]
mod reclamation_support;
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
    directory: PathBuf,
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
            directory: repo,
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
            .current_dir(&self.directory);
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
        std::fs::read_to_string(self.directory.join(name)).ok()
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
        if std::thread::panicking() {
            self._dir.disable_cleanup(true);
            eprintln!("Preserved failed CLI fixture at {}", self.root.display());
        }
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
        "expected_directory":env.directory.display().to_string()})
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
    let registration: cyber_app::Registration = serde_json::from_value(registration).unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    // Parallel fixtures may reuse an ephemeral TCP port after this process
    // exits. Probe the recorded server ID, never the presence of any listener.
    while runtime
        .block_on(cyber_app::probe_registration(&registration))
        .is_some_and(|probe| probe.id.as_deref() == Some(registration.id.as_str()))
    {
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
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    // A loaded Windows runner can take well over 10 s to start exec and reach the model.
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    while model.held().is_empty() {
        if let Some(status) = gated.try_wait().unwrap() {
            let mut stderr = String::new();
            std::io::Read::read_to_string(gated.stderr.as_mut().unwrap(), &mut stderr).unwrap();
            panic!("exec ended before GATE reached the provider ({status}): {stderr}");
        }
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
        let review = json(&env.cyber(&[
            "sessions", "recovery", &id, journal, "--action", action, "--format", "json",
        ]));
        assert_eq!(review["origin"]["session_id"], id);
        assert_eq!(review["acknowledged"], true);
        assert!(review["confirmation"].is_null());
        assert!(!review["conflicts"].as_array().unwrap().is_empty());
        assert!(review["exclusions"].is_array());
        let foreign = env.cyber(&[
            "sessions", "recovery", &other, journal, "--action", action, "--format", "json",
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
fn managed_keep_and_rollback_after_real_partial_restore_kill_preserve_owner_files_and_never_replay()
{
    managed_native_recovery_after_partial_kill("keep");
    managed_native_recovery_after_partial_kill("rollback");
}

#[test]
#[cfg(all(
    feature = "native-git-acceptance",
    any(target_os = "macos", target_os = "linux")
))]
fn managed_git_keep_and_rollback_after_real_partial_restore_kill() {
    for action in ["keep", "rollback"] {
        managed_git_partial_kill(action);
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn git_fixture(directory: &std::path::Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(directory)
        .args(args)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

#[cfg(all(
    feature = "native-git-acceptance",
    any(target_os = "macos", target_os = "linux")
))]
fn managed_git_fixture(model: &scripted_model::ScriptedModel) -> (Env, String) {
    let mut env = Env::new(model);
    git_fixture(
        &env.directory,
        &[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.test",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--allow-empty",
            "--quiet",
            "-m",
            "Initial",
        ],
    );
    let created = json(&env.cyber(&[
        "api",
        "POST",
        "/worktrees",
        "-d",
        r#"{"name":"managed","session":{"model":"test/main","mode":"accept-edits"}}"#,
    ]));
    let id = created["data"]["session"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    env.directory = PathBuf::from(created["data"]["worktree"]["path"].as_str().unwrap());
    assert!(
        env.directory
            .starts_with(env.root.join("cyber-home/data/worktrees"))
    );
    (env, id)
}

#[cfg(all(
    feature = "native-git-acceptance",
    any(target_os = "macos", target_os = "linux")
))]
fn managed_git_partial_kill(action: &str) {
    let model = scripted_model::ScriptedModel::start(reply);
    let (env, id) = managed_git_fixture(&model);
    for index in 0..1280 {
        std::fs::write(env.directory.join(format!("bulk_{index:04}")), "before").unwrap();
    }
    std::fs::write(env.directory.join("zz\nfixture"), "before").unwrap();
    std::os::unix::fs::symlink("bulk_0000", env.directory.join("a_link")).unwrap();
    git_fixture(&env.directory, &["add", "--all"]);
    let index_path = PathBuf::from(git_fixture(
        &env.directory,
        &["rev-parse", "--path-format=absolute", "--git-path", "index"],
    ));
    let index = std::fs::read(&index_path).unwrap();
    let config_path = env.root.join("repo/.git/config");
    let config = std::fs::read(&config_path).unwrap();
    env.exec(Some(&id), "before snapshot");
    let first = env.user_ids(&id)[0].clone();
    for index in 0..1280 {
        std::fs::write(env.directory.join(format!("bulk_{index:04}")), "after").unwrap();
    }
    std::fs::write(env.directory.join("zz\nfixture"), "after").unwrap();
    std::fs::remove_file(env.directory.join("a_link")).unwrap();
    std::os::unix::fs::symlink("bulk_0001", env.directory.join("a_link")).unwrap();
    std::fs::write(env.directory.join("aa_removed"), "after-only source").unwrap();
    env.exec(Some(&id), "after snapshot");
    let messages = kill_partial_restore(&env, &id, &first);
    let requests = model.requests().len();
    std::fs::write(env.directory.join("unrelated-owner"), "owner data").unwrap();
    if action == "keep" {
        std::fs::write(env.directory.join("bulk_1279"), "later owner image").unwrap();
    }
    let partial: Vec<_> = (0..1280)
        .map(|index| env.file(&format!("bulk_{index:04}")).unwrap())
        .collect();
    let (journal, review) = reviewed_recovery_cli(&env, &id, action);
    assert_eq!(
        review["origin"]["worktree_id"],
        cyber_core::worktrees::Repository::managed_locations_at(&env.directory).unwrap()[0]
            .1
            .id
    );
    let request_id = format!("managed-git-kill-{action}");
    let request = json!({"journal_id":journal,"action":action,"confirmation":review["confirmation"],"request_id":request_id});
    let mut stale = request.clone();
    stale["confirmation"] = "0".repeat(64).into();
    assert!(!recovery_confirm_cli(&env, &id, &stale).status.success());
    let result = json(&recovery_confirm_cli(&env, &id, &request));
    assert_eq!(result["state"], "acknowledged");
    assert_eq!(result["journal_acknowledged"], true);
    assert_managed_git_images(&env, action, &partial);
    assert_foreign_recovery_request_refused(&env, &request);
    assert_eq!(env.messages(&id), messages);
    assert_native_settlement_link(&env, &journal, &result);
    assert_recovery_lookup_does_not_replay(&env, &id, &request_id, &request, &result, &messages);
    assert_managed_git_reclamation(&env, &result);
    assert_eq!(std::fs::read(index_path).unwrap(), index);
    assert_eq!(std::fs::read(config_path).unwrap(), config);
    assert_eq!(model.requests().len(), requests);
}

#[cfg(all(
    feature = "native-git-acceptance",
    any(target_os = "macos", target_os = "linux")
))]
fn assert_managed_git_reclamation(env: &Env, result: &Value) {
    use cyber_server::runtime::{SnapshotRecoveryReceipt, snapshot_reclamation::Receipt};
    exits(&env.cyber(&["service", "stop"]), 0);
    let maintenance = reclamation_support::Maintenance::open(&env.root.join("cyber-home"));
    let receipt: SnapshotRecoveryReceipt =
        serde_json::from_value(result["receipt"].clone()).unwrap();
    let checkout = receipt.checkout.as_ref().unwrap();
    let original = checkout.original.as_ref().unwrap();
    let phase: Value = serde_json::from_slice(
        &std::fs::read(
            maintenance
                .namespace
                .join("workers")
                .join(original.lifetime.source_phase.as_ref().unwrap()),
        )
        .unwrap(),
    )
    .unwrap();
    let bindings: Vec<cyber_core::worktrees::CheckoutBinding> =
        serde_json::from_value(phase["checkout_bindings"].clone()).unwrap();
    assert!(!bindings.is_empty());
    let binding = &bindings[0];
    let source = env.file("bulk_0000");
    let source_link = std::fs::read_link(env.directory.join("a_link")).unwrap();
    assert_eq!(maintenance.reclaim(&env.directory).unwrap(), 0);
    maintenance.age(checkout.resolution_origin.activity_id.clone());
    let resources = original.lifetime.checkout_owner().unwrap();
    let receipt = Receipt::Recovery(receipt.clone());
    let marker = assert_managed_cleanup_refusals(env, &maintenance, &receipt, binding, &resources);
    let cut = maintenance.plan_only(&env.directory).unwrap_err();
    assert!(
        cut.contains("fixture cut after durable paired plan"),
        "{cut}"
    );
    assert!(
        !maintenance
            .policy
            .record(&receipt)
            .unwrap()
            .unwrap()
            .completed
    );
    assert!(maintenance.reclaim(&env.directory).unwrap() > 0);
    assert!(
        maintenance
            .policy
            .record(&receipt)
            .unwrap()
            .unwrap()
            .completed
    );
    assert_eq!(maintenance.reclaim(&env.directory).unwrap(), 0);
    assert_eq!(env.file("bulk_0000"), source);
    assert_eq!(
        std::fs::read_link(env.directory.join("a_link")).unwrap(),
        source_link
    );
    assert!(marker.exists());
}

#[cfg(all(
    feature = "native-git-acceptance",
    any(target_os = "macos", target_os = "linux")
))]
fn assert_managed_cleanup_refusals(
    env: &Env,
    maintenance: &reclamation_support::Maintenance,
    receipt: &cyber_server::runtime::snapshot_reclamation::Receipt,
    binding: &cyber_core::worktrees::CheckoutBinding,
    resources: &cyber_core::worktrees::CheckoutOwner,
) -> PathBuf {
    let marker = binding
        .common_dir
        .join("cyber-worktree-activity")
        .join(&binding.worktree_id)
        .join(format!("{}.snapshot-ack", binding.owner_id));
    let hidden = env.root.join("retained-checkout-marker");
    std::fs::rename(&marker, &hidden).unwrap();
    let refused = maintenance.reclaim(&env.directory).unwrap_err();
    assert!(
        refused.contains("acknowledgement has not been published"),
        "{refused}"
    );
    assert!(
        !marker.exists(),
        "cleanup must not publish a missing marker"
    );
    assert!(maintenance.policy.record(receipt).unwrap().is_none());
    std::fs::rename(&hidden, &marker).unwrap();
    let locations =
        cyber_core::worktrees::Repository::managed_locations_at(&env.directory).unwrap();
    let live = locations[0]
        .0
        .reacquire_snapshot(&locations[0].1, binding, resources)
        .unwrap();
    let refused = maintenance.reclaim(&env.directory).unwrap_err();
    assert!(refused.contains("worker is still active"), "{refused}");
    assert!(maintenance.policy.record(receipt).unwrap().is_none());
    drop(live);
    marker
}

#[cfg(all(
    feature = "native-git-acceptance",
    any(target_os = "macos", target_os = "linux")
))]
fn assert_managed_git_images(env: &Env, action: &str, partial: &[String]) {
    assert_eq!(
        std::fs::read_link(env.directory.join("a_link")).unwrap(),
        PathBuf::from(if action == "rollback" {
            "bulk_0001"
        } else {
            "bulk_0000"
        })
    );
    assert_eq!(
        env.file("bulk_0000").as_deref(),
        Some(if action == "rollback" {
            "after"
        } else {
            "before"
        })
    );
    assert_eq!(env.file("unrelated-owner").as_deref(), Some("owner data"));
    assert_eq!(env.file("zz\nfixture").as_deref(), Some("after"));
    for (index, before) in partial.iter().enumerate() {
        assert_eq!(
            env.file(&format!("bulk_{index:04}")).as_deref(),
            Some(if action == "rollback" {
                "after"
            } else {
                before
            }),
            "{action}: bulk_{index:04}"
        );
    }
    assert_eq!(
        env.file("aa_removed").as_deref(),
        (action == "rollback").then_some("after-only source")
    );
}

#[test]
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn exited_pre_origin_preparation_is_reviewed_cancelled_and_lookup_only_after_restart() {
    cancelled_pre_effect_preparation(false, false);
}

#[test]
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn exited_origin_preparation_before_source_admission_is_cancelled_after_real_kill() {
    cancelled_pre_effect_preparation(true, false);
}

#[test]
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn terminal_preparation_recovery_survives_client_kill_with_a_committed_reply_withheld() {
    cancelled_pre_effect_preparation(false, true);
    cancelled_pre_effect_preparation(true, true);
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn cancelled_pre_effect_preparation(published: bool, terminal: bool) {
    cancelled_preparation(published, terminal, false);
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn fully_prepared_pre_effect_cancellation_reclaims_only_persisted_target_inodes() {
    cancelled_preparation(true, false, true);
}

#[test]
#[cfg(all(
    feature = "native-git-acceptance",
    any(target_os = "macos", target_os = "linux")
))]
fn managed_git_prepared_cancellation_reclaims_persisted_files_and_dangling_links() {
    let model = scripted_model::ScriptedModel::start(reply);
    let (env, id) = managed_git_fixture(&model);
    let fixture = preparation_in(model, env, Some(id), true);
    run_cancelled_preparation(fixture, true, false, true);
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
struct PreparationFixture {
    model: scripted_model::ScriptedModel,
    env: Env,
    id: String,
    first: String,
    messages: Vec<Value>,
    after: String,
}
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn preparation_fixture(prepared: bool) -> PreparationFixture {
    let model = scripted_model::ScriptedModel::start(reply);
    let env = Env::new(&model);
    std::fs::remove_dir_all(env.directory.join(".git")).unwrap();
    preparation_in(model, env, None, prepared)
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn preparation_in(
    model: scripted_model::ScriptedModel,
    env: Env,
    session: Option<String>,
    prepared: bool,
) -> PreparationFixture {
    let repo = env.directory.clone();
    let git = repo.join(".git").exists();
    // Git hands off several images per path under its bounded import budget.
    // Keep separate flushed staging operations without inflating that handoff.
    let bytes = if git { 8 * 1024 } else { 64 * 1024 };
    let before = format!("b\0{}", "b".repeat(bytes));
    let after = format!("a\0{}", "a".repeat(bytes));
    for index in 0..1280 {
        std::fs::write(repo.join(format!("bulk_{index:04}")), &before).unwrap();
    }
    if git {
        std::os::unix::fs::symlink("bulk_0000", repo.join("a_link")).unwrap();
        git_fixture(&repo, &["add", "--all"]);
    }
    let id = env.exec(session.as_deref(), "before snapshot");
    let first = env.user_ids(&id)[0].clone();
    for index in 0..1280 {
        std::fs::write(repo.join(format!("bulk_{index:04}")), &after).unwrap();
    }
    if git {
        std::fs::remove_file(repo.join("a_link")).unwrap();
        std::os::unix::fs::symlink("bulk_1279", repo.join("a_link")).unwrap();
    }
    if prepared {
        std::fs::write(repo.join("aa_removed"), "after-only source").unwrap();
    }
    env.exec(Some(&id), "after snapshot");
    let messages = env.messages(&id);
    PreparationFixture {
        model,
        env,
        id,
        first,
        messages,
        after,
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn cancelled_preparation(published: bool, terminal: bool, prepared: bool) {
    run_cancelled_preparation(preparation_fixture(prepared), published, terminal, prepared);
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn run_cancelled_preparation(
    fixture: PreparationFixture,
    published: bool,
    terminal: bool,
    prepared: bool,
) {
    let PreparationFixture {
        model,
        env,
        id,
        first,
        messages,
        after,
    } = fixture;
    let repo = &env.directory;
    let git_metadata = preparation_git_metadata(repo);
    let source_link = std::fs::read_link(repo.join("a_link")).ok();
    let activity = if prepared {
        kill_after_prepared_images(&env, &id, &first)
    } else {
        kill_before_source_admission(&env, &id, &first, published)
    };
    let requests = model.requests().len();
    let target = env.root.join("preparation-owner-target");
    std::fs::write(&target, "later owner bytes").unwrap();
    std::fs::remove_file(repo.join("bulk_0000")).unwrap();
    std::os::unix::fs::symlink(&target, repo.join("bulk_0000")).unwrap();
    let list = json(&env.cyber(&["sessions", "recovery-preparation", &id, "--format", "json"]));
    assert_eq!(list[0]["activity_id"], activity);
    assert_eq!(list[0]["lifetime_available"], true);
    let review = json(&env.cyber(&[
        "sessions",
        "recovery-preparation",
        &id,
        &activity,
        "--format",
        "json",
    ]));
    let request_id = "before-origin/ owner #?%";
    let body = json!({"confirmation":review["confirmation"],"request_id":request_id});
    assert_preparation_client_refusals(&env, &id, &activity, &body);
    if prepared {
        assert_prepared_review_digest(&env, &id, &activity, &review, &body);
    }
    let receipt = preparation_with_client(&env, &id, &activity, &body, terminal);
    let request_id = receipt["request_id"].as_str().unwrap();
    let body = json!({"confirmation":receipt["confirmation"],"request_id":request_id});
    assert_preparation_resource_digest(&env, &review, &receipt, published);
    assert_eq!(
        json(&preparation_request_cli(
            &env, &id, &activity, request_id, true
        )),
        receipt
    );
    if published {
        assert_preparation_tombstone(&env, &receipt);
    }
    assert_preparation_receipt_restart(&env, &receipt, &body);
    assert_cancelled_preparation_source(&env, &after);
    assert_eq!(env.messages(&id), messages);
    assert_eq!(model.requests().len(), requests);
    if prepared {
        assert_preparation_cleanup_lookup(&env, &receipt);
    }
    assert_eq!(env.messages(&id), messages);
    assert_eq!(model.requests().len(), requests);
    assert_eq!(std::fs::read_link(repo.join("a_link")).ok(), source_link);
    assert_cancelled_preparation_source(&env, &after);
    assert_eq!(preparation_git_metadata(repo), git_metadata);
    env.exec(Some(&id), "after preparation cancellation");
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn assert_cancelled_preparation_source(env: &Env, after: &str) {
    assert!(
        std::fs::symlink_metadata(env.directory.join("bulk_0000"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        std::fs::read_to_string(env.root.join("preparation-owner-target")).unwrap(),
        "later owner bytes"
    );
    assert_eq!(env.file("bulk_1279").as_deref(), Some(after));
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn preparation_git_metadata(directory: &std::path::Path) -> Vec<(PathBuf, Vec<u8>)> {
    if !directory.join(".git").exists() {
        return Vec::new();
    }
    ["index", "config"]
        .into_iter()
        .map(|name| {
            let path = PathBuf::from(git_fixture(
                directory,
                &["rev-parse", "--path-format=absolute", "--git-path", name],
            ));
            let bytes = std::fs::read(&path).unwrap();
            (path, bytes)
        })
        .collect()
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn assert_preparation_receipt_restart(env: &Env, receipt: &Value, body: &Value) {
    let session = receipt["lifetime"]["owner"]["session_id"].as_str().unwrap();
    let activity = receipt["activity_id"].as_str().unwrap();
    let request_id = receipt["request_id"].as_str().unwrap();
    kill_registered_service(env);
    assert_preparation_history(env, session, activity, receipt);
    assert_eq!(
        json(&preparation_confirm_cli(env, session, activity, body)),
        *receipt
    );
    assert_eq!(
        json(&preparation_request_cli(
            env, session, activity, request_id, false
        )),
        *receipt
    );
    assert_eq!(
        json(&preparation_request_cli(
            env, session, activity, request_id, true
        )),
        *receipt
    );
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn preparation_with_client(
    env: &Env,
    session: &str,
    activity: &str,
    body: &Value,
    terminal: bool,
) -> Value {
    if terminal {
        terminal_preparation(env, session, activity)
    } else {
        json(&preparation_confirm_cli(env, session, activity, body))
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn assert_preparation_cleanup_lookup(env: &Env, receipt: &Value) {
    assert_prepared_payload_cleanup(env, receipt);
    let session = receipt["lifetime"]["owner"]["session_id"].as_str().unwrap();
    assert_eq!(
        json(&preparation_request_cli(
            env,
            session,
            receipt["activity_id"].as_str().unwrap(),
            receipt["request_id"].as_str().unwrap(),
            true
        )),
        *receipt
    );
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn assert_prepared_review_digest(
    env: &Env,
    session: &str,
    activity: &str,
    review: &Value,
    body: &Value,
) {
    let journal = review["origin"]["binding"]["journal_id"].as_str().unwrap();
    let path = std::fs::read_dir(env.root.join("cyber-home/data/snapshot/global"))
        .unwrap()
        .map(|entry| {
            entry
                .unwrap()
                .path()
                .join("journals")
                .join(journal)
                .join("intent.json")
        })
        .find(|path| path.exists())
        .unwrap();
    let bytes = std::fs::read(&path).unwrap();
    let mut changed: Value = serde_json::from_slice(&bytes).unwrap();
    changed["created"] = (changed["created"].as_u64().unwrap() + 1).into();
    std::fs::write(&path, changed.to_string()).unwrap();
    let refused = preparation_confirm_cli(env, session, activity, body);
    assert_eq!(refused.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("review changed"),
        "{}",
        String::from_utf8_lossy(&refused.stderr)
    );
    std::fs::write(path, bytes).unwrap();
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn assert_prepared_payload_cleanup(env: &Env, reply: &Value) {
    use cyber_server::runtime::{SnapshotPreparationReceipt, snapshot_reclamation::Receipt};
    exits(&env.cyber(&["service", "stop"]), 0);
    let maintenance = reclamation_support::Maintenance::open(&env.root.join("cyber-home"));
    let receipt: SnapshotPreparationReceipt = serde_json::from_value(reply.clone()).unwrap();
    let journal = maintenance
        .namespace
        .join("journals")
        .join(&receipt.origin.as_ref().unwrap().binding.journal_id);
    let intent = std::fs::read(journal.join("intent.json")).unwrap();
    let cancellation = std::fs::read(journal.join("preparation.json")).unwrap();
    let source = env.directory.clone();
    assert_eq!(maintenance.reclaim(&source).unwrap(), 0);
    maintenance.age(receipt.activity_id.clone());
    #[cfg(feature = "native-git-acceptance")]
    assert_prepared_managed_guards(env, &maintenance, &receipt);
    let receipt = Receipt::Preparation(Box::new(receipt));
    assert_prepared_link_guards(&maintenance, &source, &journal, &receipt);
    assert_prepared_inode_and_lifetime_guards(&maintenance, &source, &journal, &receipt);
    assert_prepared_store_receipt_binding(&maintenance, &source, &journal, &receipt);
    assert_prepared_interrupted_plan(&maintenance, &source, &journal, &receipt);
    assert!(maintenance.reclaim(&source).unwrap() > 0);
    assert!(
        maintenance
            .policy
            .record(&receipt)
            .unwrap()
            .unwrap()
            .completed
    );
    assert_eq!(maintenance.reclaim(&source).unwrap(), 0);
    assert_eq!(std::fs::read(journal.join("intent.json")).unwrap(), intent);
    assert_eq!(
        std::fs::read(journal.join("preparation.json")).unwrap(),
        cancellation
    );
    assert_eq!(std::fs::read_dir(&journal).unwrap().count(), 2);
    assert!(
        std::fs::symlink_metadata(source.join("bulk_0000"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        std::fs::read_to_string(env.root.join("preparation-owner-target")).unwrap(),
        "later owner bytes"
    );
}

#[cfg(all(
    feature = "native-git-acceptance",
    any(target_os = "macos", target_os = "linux")
))]
fn assert_prepared_managed_guards(
    env: &Env,
    maintenance: &reclamation_support::Maintenance,
    receipt: &cyber_server::runtime::SnapshotPreparationReceipt,
) {
    if receipt.lifetime.owner.worktree_id.is_none() {
        return;
    }
    let phase: Value = serde_json::from_slice(
        &std::fs::read(
            maintenance
                .namespace
                .join("workers")
                .join(receipt.lifetime.source_phase.as_ref().unwrap()),
        )
        .unwrap(),
    )
    .unwrap();
    let bindings: Vec<cyber_core::worktrees::CheckoutBinding> =
        serde_json::from_value(phase["checkout_bindings"].clone()).unwrap();
    assert!(!bindings.is_empty());
    let owner = receipt.lifetime.checkout_owner().unwrap();
    let receipt = cyber_server::runtime::snapshot_reclamation::Receipt::Preparation(Box::new(
        receipt.clone(),
    ));
    assert_managed_cleanup_refusals(env, maintenance, &receipt, &bindings[0], &owner);
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn assert_prepared_interrupted_plan(
    maintenance: &reclamation_support::Maintenance,
    source: &std::path::Path,
    journal: &std::path::Path,
    receipt: &cyber_server::runtime::snapshot_reclamation::Receipt,
) {
    assert!(maintenance.plan_only(source).is_err());
    let pending = maintenance.policy.record(receipt).unwrap().unwrap();
    assert!(!pending.completed);
    assert_eq!(pending.plan.version, 3);
    assert!(pending.plan.intent_digest.is_some());
    let unknown = journal.join("foreign-evidence");
    std::fs::write(&unknown, b"owner bytes").unwrap();
    assert!(maintenance.reclaim(source).is_err());
    let first = journal.join(&pending.plan.files[0].name);
    assert!(std::fs::symlink_metadata(&first).is_ok());
    std::fs::remove_file(unknown).unwrap();
    // Synthetic progress of one planned private unlink, following a real kill.
    std::fs::remove_file(first).unwrap();
    std::fs::File::open(journal).unwrap().sync_all().unwrap();
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn assert_prepared_store_receipt_binding(
    maintenance: &reclamation_support::Maintenance,
    source: &std::path::Path,
    journal: &std::path::Path,
    receipt: &cyber_server::runtime::snapshot_reclamation::Receipt,
) {
    let owner = receipt.origin().unwrap().activity_id.clone();
    let restored_owner = owner.clone();
    let original = maintenance.store.transaction(move |db| {
        let raw: String = db.query_row("SELECT data FROM event WHERE aggregate_id=?1 ORDER BY seq DESC LIMIT 1", [&owner], |row|row.get(0))?;
        let mut legacy: Value = serde_json::from_str(&raw).unwrap();
        assert!(legacy.as_object_mut().unwrap().remove("snapshot_preparation_intent_digest").is_some());
        db.execute("UPDATE event SET data=?1 WHERE aggregate_id=?2 AND seq=(SELECT MAX(seq) FROM event WHERE aggregate_id=?2)", rusqlite::params![legacy.to_string(), owner])?;
        Ok(raw)
    }).unwrap();
    let error = maintenance.reclaim(source).unwrap_err();
    assert!(error.contains("committed native intent digest"), "{error}");
    maintenance.store.transaction(move |db| {
        db.execute("UPDATE event SET data=?1 WHERE aggregate_id=?2 AND seq=(SELECT MAX(seq) FROM event WHERE aggregate_id=?2)", rusqlite::params![original,restored_owner])?;
        Ok(())
    }).unwrap();
    let path = journal.join("intent.json");
    let bytes = std::fs::read(&path).unwrap();
    let mut changed: Value = serde_json::from_slice(&bytes).unwrap();
    changed["created"] = (changed["created"].as_u64().unwrap() + 1).into();
    std::fs::write(&path, changed.to_string()).unwrap();
    let error = maintenance.reclaim(source).unwrap_err();
    assert!(error.contains("committed native intent digest"), "{error}");
    assert!(maintenance.policy.record(receipt).unwrap().is_none());
    std::fs::write(path, bytes).unwrap();
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn assert_prepared_inode_and_lifetime_guards(
    maintenance: &reclamation_support::Maintenance,
    source: &std::path::Path,
    journal: &std::path::Path,
    receipt: &cyber_server::runtime::snapshot_reclamation::Receipt,
) {
    let proof = receipt.owners().unwrap().remove(0);
    let held = std::fs::File::open(
        maintenance
            .namespace
            .join("workers")
            .join(&proof.origin.activity_id),
    )
    .unwrap();
    held.try_lock().unwrap();
    assert!(maintenance.reclaim(source).is_err());
    drop(held);
    let stage = prepared_stage(journal, "bulk_0000");
    let retained = journal.join("retained-original");
    let bytes = std::fs::read(&stage).unwrap();
    std::fs::rename(&stage, &retained).unwrap();
    std::fs::write(&stage, bytes).unwrap();
    let error = maintenance.reclaim(source).unwrap_err();
    assert!(error.contains("persisted ownership"), "{error}");
    assert!(maintenance.policy.record(receipt).unwrap().is_none());
    std::fs::remove_file(&stage).unwrap();
    std::fs::rename(retained, stage).unwrap();
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn prepared_stage(journal: &std::path::Path, path: &str) -> PathBuf {
    let intent: Value =
        serde_json::from_slice(&std::fs::read(journal.join("intent.json")).unwrap()).unwrap();
    let index = intent["changes"]
        .as_array()
        .unwrap()
        .iter()
        .position(|change| change["path"] == path)
        .unwrap();
    journal.join(format!("stage_{index}"))
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn assert_prepared_link_guards(
    maintenance: &reclamation_support::Maintenance,
    source: &std::path::Path,
    journal: &std::path::Path,
    receipt: &cyber_server::runtime::snapshot_reclamation::Receipt,
) {
    if !source.join(".git").exists() {
        return;
    }
    let stage = prepared_stage(journal, "a_link");
    let target = std::fs::read_link(&stage).unwrap();
    assert_eq!(target, std::path::Path::new("bulk_0000"));
    assert!(!stage.exists(), "private prepared target must be dangling");
    let retained = journal.join("retained-link");
    std::fs::rename(&stage, &retained).unwrap();
    std::os::unix::fs::symlink(&target, &stage).unwrap();
    let error = maintenance.reclaim(source).unwrap_err();
    assert!(error.contains("persisted ownership"), "{error}");
    assert!(maintenance.policy.record(receipt).unwrap().is_none());
    std::fs::remove_file(&stage).unwrap();
    std::fs::rename(retained, stage).unwrap();
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn terminal_preparation(env: &Env, session: &str, activity: &str) -> Value {
    let template = env.command(&[]);
    let environment = template
        .get_envs()
        .filter_map(|(key, value)| value.map(|value| (key, value)));
    let output = Command::new("python3")
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/support/tui_preparation_driver.py"
        ))
        .args([env!("CARGO_BIN_EXE_cyber"), session, activity])
        .env_clear()
        .envs(environment)
        .current_dir(env.root.join("repo"))
        .output()
        .unwrap();
    json(&output)
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn assert_preparation_history(env: &Env, session: &str, activity: &str, receipt: &Value) {
    let current = json(&env.cyber(&[
        "api",
        "GET",
        &format!("/sessions/{session}/rewind/preparations"),
    ]));
    assert!(
        current["data"]
            .as_array()
            .unwrap()
            .iter()
            .all(|candidate| candidate["activity_id"] != activity)
    );
    let history = json(&env.cyber(&[
        "api",
        "GET",
        &format!("/sessions/{session}/rewind/preparations?include_cancelled=true"),
    ]));
    assert!(
        history["data"]
            .as_array()
            .unwrap()
            .iter()
            .any(|candidate| candidate["activity_id"] == activity
                && candidate["status"] == "settled")
    );
    let discovered = json(&env.cyber(&[
        "api",
        "GET",
        &format!("/sessions/{session}/rewind/preparations/{activity}/receipt"),
    ]));
    assert_eq!(&discovered["data"], receipt);
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn preparation_confirm_cli(env: &Env, session: &str, activity: &str, body: &Value) -> Output {
    env.cyber(&[
        "sessions",
        "recovery-preparation",
        session,
        activity,
        "--confirm",
        body["confirmation"].as_str().unwrap(),
        "--request-id",
        body["request_id"].as_str().unwrap(),
        "--format",
        "json",
    ])
}
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn preparation_request_cli(
    env: &Env,
    session: &str,
    activity: &str,
    request: &str,
    acknowledge: bool,
) -> Output {
    let mut args = vec![
        "sessions",
        "recovery-preparation-request",
        session,
        activity,
        request,
        "--format",
        "json",
    ];
    if acknowledge {
        args.push("--acknowledge");
    }
    env.cyber(&args)
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn assert_preparation_client_refusals(env: &Env, session: &str, activity: &str, body: &Value) {
    let mut stale = body.clone();
    stale["confirmation"] = "0".repeat(64).into();
    let refused = preparation_confirm_cli(env, session, activity, &stale);
    assert_eq!(refused.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&refused.stderr).contains("recovery-preparation-request"));
    let foreign = json(&env.cyber(&[
        "api",
        "POST",
        "/sessions",
        "-d",
        "{\"title\":\"foreign preparation\"}",
    ]));
    let foreign_id = foreign["data"]["id"].as_str().unwrap();
    assert!(
        !env.cyber(&[
            "api",
            "GET",
            &format!("/sessions/{foreign_id}/rewind/preparations/{activity}/receipt")
        ])
        .status
        .success()
    );
    let foreign_history = json(&env.cyber(&[
        "api",
        "GET",
        &format!("/sessions/{foreign_id}/rewind/preparations?include_cancelled=true"),
    ]));
    assert!(
        foreign_history["data"]
            .as_array()
            .unwrap()
            .iter()
            .all(|candidate| candidate["activity_id"] != activity)
    );

    assert!(
        !env.cyber(&[
            "sessions",
            "recovery-preparation",
            foreign_id,
            activity,
            "--format",
            "json"
        ])
        .status
        .success()
    );
    assert!(
        !preparation_confirm_cli(env, foreign_id, activity, body)
            .status
            .success()
    );
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn assert_preparation_resource_digest(env: &Env, review: &Value, receipt: &Value, published: bool) {
    assert_eq!(
        receipt["checkout_resources_digest"],
        review["checkout_resources_digest"]
    );
    if published {
        let lifetime: cyber_server::runtime::SnapshotLifetimeBinding =
            serde_json::from_value(review["lifetime"].clone()).unwrap();
        let namespace = std::fs::read_dir(env.root.join("cyber-home/data/snapshot/global"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| {
                path.join("workers")
                    .join(&lifetime.owner.activity_id)
                    .exists()
            })
            .unwrap();
        let phase: Value = serde_json::from_slice(
            &std::fs::read(
                namespace
                    .join("workers")
                    .join(lifetime.source_phase.as_ref().unwrap()),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(phase["lifetime"], review["lifetime"]);
        let bindings: Vec<cyber_core::worktrees::CheckoutBinding> =
            serde_json::from_value(phase["checkout_bindings"].clone()).unwrap();
        assert_eq!(bindings.is_empty(), lifetime.owner.worktree_id.is_none());
        assert_eq!(
            receipt["checkout_resources_digest"],
            lifetime.checkout_resources_digest(&bindings).unwrap()
        );
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn assert_preparation_tombstone(env: &Env, receipt: &Value) {
    let namespace = std::fs::read_dir(env.root.join("cyber-home/data/snapshot/global"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let journal = namespace
        .join("journals")
        .join(receipt["origin"]["binding"]["journal_id"].as_str().unwrap());
    let tombstone: Value =
        serde_json::from_slice(&std::fs::read(journal.join("preparation.json")).unwrap()).unwrap();
    assert_eq!(&tombstone, receipt);
    assert_eq!(receipt["source_admitted"], false);
    if let Ok(bytes) = std::fs::read(journal.join("intent.json")) {
        let intent: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(intent["completed"], false);
        assert!(
            !intent
                .get("acknowledged")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        );
        assert!(intent["applied"].as_array().unwrap().is_empty());
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn kill_before_source_admission(env: &Env, id: &str, first: &str, published: bool) -> String {
    use rusqlite::OptionalExtension;
    let selection = reviewed_selection(env, first, "both", "commit");
    let preview = reviewed_preview(env, id, &selection);
    let mut body = selection;
    body["confirmation"] = preview["confirmation"].clone();
    let path = format!("/sessions/{id}/rewind/confirm");
    let mut pending = env
        .command(&["api", "POST", &path, "-d", &body.to_string()])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let database = std::fs::read_dir(env.root.join("cyber-home/data"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.extension().is_some_and(|extension| extension == "db"))
        .unwrap();
    let db =
        rusqlite::Connection::open_with_flags(database, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    let activity = loop {
        let found: Option<String> = db.query_row("SELECT aggregate_id FROM event e WHERE type='native.activity.changed.1' AND ((?2=1 AND seq=1 AND json_extract(data,'$.snapshot_origin') IS NOT NULL) OR (?2=0 AND seq=0 AND json_extract(data,'$.snapshot_origin') IS NULL)) AND json_extract(data,'$.session_id')=?1 AND json_extract(data,'$.snapshot_worker')=1 AND json_extract(data,'$.snapshot_source_admitted')=0 AND json_extract(data,'$.snapshot_lifetime') IS NOT NULL AND NOT EXISTS(SELECT 1 FROM event n WHERE n.aggregate_id=e.aggregate_id AND n.seq>e.seq)", rusqlite::params![id, i64::from(published)], |row| row.get(0)).optional().unwrap();
        if let Some(found) = found {
            break found;
        }
        assert!(
            pending.try_wait().unwrap().is_none(),
            "restore exited before its reservation was observed"
        );
        assert!(
            std::time::Instant::now() < deadline,
            "pre-origin reservation was never observed"
        );
        std::thread::sleep(Duration::from_millis(1));
    };
    kill_registered_service(env);
    assert!(!pending.wait_with_output().unwrap().status.success());
    let data: String = db
        .query_row(
            "SELECT data FROM event WHERE aggregate_id=?1 ORDER BY seq DESC LIMIT 1",
            [&activity],
            |row| row.get(0),
        )
        .unwrap();
    let record: Value = serde_json::from_str(&data).unwrap();
    assert_eq!(
        record["snapshot_origin"].is_object(),
        published,
        "kill missed the selected origin boundary"
    );
    assert_eq!(
        record["snapshot_source_admitted"], false,
        "kill missed the pre-effect boundary"
    );
    activity
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn kill_after_prepared_images(env: &Env, id: &str, first: &str) -> String {
    use rusqlite::OptionalExtension;
    let selection = reviewed_selection(env, first, "both", "commit");
    let preview = reviewed_preview(env, id, &selection);
    let mut body = selection;
    body["confirmation"] = preview["confirmation"].clone();
    let mut pending = env
        .command(&[
            "api",
            "POST",
            &format!("/sessions/{id}/rewind/confirm"),
            "-d",
            &body.to_string(),
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let data = env.root.join("cyber-home/data");
    let database = std::fs::read_dir(&data)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.extension().is_some_and(|extension| extension == "db"))
        .unwrap();
    let db = rusqlite::Connection::open(database).unwrap();
    db.busy_timeout(Duration::from_secs(5)).unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(45);
    let activity = loop {
        let activity: Option<String> = db.query_row("SELECT aggregate_id FROM event e WHERE type='native.activity.changed.1' AND seq=1 AND json_extract(data,'$.session_id')=?1 AND json_extract(data,'$.snapshot_origin') IS NOT NULL AND json_extract(data,'$.snapshot_source_admitted')=0 AND NOT EXISTS(SELECT 1 FROM event n WHERE n.aggregate_id=e.aggregate_id AND n.seq>e.seq)", [id], |row|row.get(0)).optional().unwrap();
        if let Some(activity) = activity {
            break activity;
        }
        assert!(
            pending.try_wait().unwrap().is_none() && std::time::Instant::now() < deadline,
            "preparation origin was not observed"
        );
        std::thread::sleep(Duration::from_millis(1));
    };
    // Real SQLite writer contention holds source admission, not preparation I/O.
    // No row is edited and no native acknowledgement or effect hook is replaced.
    db.execute_batch("BEGIN IMMEDIATE").unwrap();
    let raw: String = db
        .query_row(
            "SELECT data FROM event WHERE aggregate_id=?1 ORDER BY seq DESC LIMIT 1",
            [&activity],
            |row| row.get(0),
        )
        .unwrap();
    let record: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(record["snapshot_source_admitted"], false);
    let journal_id = record["snapshot_origin"]["binding"]["journal_id"]
        .as_str()
        .unwrap();
    let namespace = std::fs::read_dir(data.join("snapshot/global"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.join("journals").join(journal_id).exists())
        .unwrap();
    let intent = namespace
        .join("journals")
        .join(journal_id)
        .join("intent.json");
    // Origin publication and staging 80 MiB are separate bounded phases. A
    // loaded runner may spend most of the origin-observation deadline before
    // staging starts. The SQLite writer still fences every source admission.
    let deadline = std::time::Instant::now() + Duration::from_secs(120);
    loop {
        let prepared = std::fs::read(&intent)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .is_some_and(|intent| {
                intent["ownership"].is_object()
                    && intent["applied"].as_array().is_some_and(Vec::is_empty)
            });
        if prepared {
            break;
        }
        if let Some(status) = pending.try_wait().unwrap() {
            let output = pending.wait_with_output().unwrap();
            panic!(
                "restore exited before publishing prepared identities ({status}): {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        assert!(
            std::time::Instant::now() < deadline,
            "prepared identities were not published within the staging deadline"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    kill_registered_service(env);
    assert!(!pending.wait_with_output().unwrap().status.success());
    db.execute_batch("ROLLBACK").unwrap();
    let admitted: bool = db.query_row("SELECT json_extract(data,'$.snapshot_source_admitted') FROM event WHERE aggregate_id=?1 ORDER BY seq DESC LIMIT 1", [&activity], |row|row.get(0)).unwrap();
    assert!(!admitted, "kill crossed the source-admission boundary");
    activity
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn recovery_confirm_cli(env: &Env, id: &str, request: &Value) -> Output {
    env.cyber(&[
        "sessions",
        "recovery",
        id,
        request["journal_id"].as_str().unwrap(),
        "--action",
        request["action"].as_str().unwrap(),
        "--confirm",
        request["confirmation"].as_str().unwrap(),
        "--request-id",
        request["request_id"].as_str().unwrap(),
        "--format",
        "json",
    ])
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn reviewed_recovery_cli(env: &Env, id: &str, action: &str) -> (String, Value) {
    let catalog = json(&env.cyber(&["sessions", "recovery", id, "--format", "json"]));
    let journals = catalog.as_array().unwrap();
    assert_eq!(journals.len(), 1);
    assert_eq!(journals[0]["filesystem_complete"], false);
    let journal = journals[0]["id"].as_str().unwrap().to_owned();
    let review = json(&env.cyber(&[
        "sessions", "recovery", id, &journal, "--action", action, "--format", "json",
    ]));
    let text = env.cyber(&["sessions", "recovery", id, &journal, "--action", action]);
    let text = exits(&text, 0);
    assert!(
        text.contains("Recovery review:")
            && text.contains("retained:")
            && text.contains("proposed:")
            && text.contains("Location:"),
        "{text}"
    );
    assert!(
        review["conflicts"].as_array().unwrap().is_empty(),
        "{review}"
    );
    assert!(
        text.contains("\"zz\\nfixture\""),
        "review lost the literal newline path"
    );
    (journal, review)
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn assert_foreign_recovery_request_refused(env: &Env, request: &Value) {
    let foreign = json(&env.cyber(&[
        "api",
        "POST",
        "/sessions",
        "-d",
        "{\"title\":\"foreign recovery request\"}",
    ]));
    let foreign_id = foreign["data"]["id"].as_str().unwrap();
    let discovered = json(&env.cyber(&[
        "api",
        "GET",
        &format!(
            "/sessions/{foreign_id}/rewind/recovery/requests?journal_id={}",
            request["journal_id"].as_str().unwrap()
        ),
    ]));
    assert_eq!(discovered["data"], json!([]));
    assert!(
        !env.cyber(&[
            "sessions",
            "recovery-request",
            foreign_id,
            request["request_id"].as_str().unwrap(),
            "--format",
            "json"
        ])
        .status
        .success()
    );
    assert!(
        !recovery_confirm_cli(env, foreign_id, request)
            .status
            .success()
    );
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn managed_native_recovery_after_partial_kill(action: &str) {
    let model = scripted_model::ScriptedModel::start(reply);
    let env = Env::new(&model);
    let repo = env.root.join("repo");
    std::fs::remove_dir_all(repo.join(".git")).unwrap();
    // Enough separately flushed exchanges to observe an actual partial source
    // namespace before killing the real server; no restore fault hook is used.
    for index in 0..1280 {
        std::fs::write(repo.join(format!("bulk_{index:04}")), "before").unwrap();
    }
    std::fs::write(repo.join("zz\nfixture"), "before").unwrap();
    let id = env.exec(None, "before snapshot");
    let first = env.user_ids(&id)[0].clone();
    for index in 0..1280 {
        std::fs::write(repo.join(format!("bulk_{index:04}")), "after").unwrap();
    }
    std::fs::write(repo.join("zz\nfixture"), "after").unwrap();
    std::fs::write(repo.join("aa_removed"), "after-only source").unwrap();
    env.exec(Some(&id), "after snapshot");
    let messages = kill_partial_restore(&env, &id, &first);
    let requests = model.requests().len();
    std::fs::write(repo.join("unrelated-owner"), "owner data").unwrap();
    if action == "keep" {
        std::fs::write(repo.join("bulk_1279"), "later owner image").unwrap();
    }
    let (journal, review) = reviewed_recovery_cli(&env, &id, action);
    let request_id = format!("real-kill-{action}/ owner #?%");
    let request = json!({"journal_id":journal,"action":action,"confirmation":review["confirmation"],"request_id":request_id});
    let mut stale = request.clone();
    stale["confirmation"] = "0".repeat(64).into();
    let refused = recovery_confirm_cli(&env, &id, &stale);
    assert_eq!(refused.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&refused.stderr).contains("stale"));
    assert!(String::from_utf8_lossy(&refused.stderr).contains("recovery-request"));
    let result = json(&recovery_confirm_cli(&env, &id, &request));
    assert_eq!(result["state"], "acknowledged");
    assert_eq!(result["journal_acknowledged"], true);
    assert_foreign_recovery_request_refused(&env, &request);
    assert_eq!(env.file("unrelated-owner").as_deref(), Some("owner data"));
    let expected = if action == "rollback" {
        "after"
    } else {
        "before"
    };
    assert_eq!(env.file("bulk_0000").as_deref(), Some(expected));
    assert_eq!(env.messages(&id), messages);
    assert_eq!(
        model.requests().len(),
        requests,
        "recovery started inference"
    );
    assert_native_settlement_link(&env, &journal, &result);
    assert_recovery_lookup_does_not_replay(&env, &id, &request_id, &request, &result, &messages);
    assert_cleanup_preserves_request(&env, &id, &request_id, &result, &messages);
    assert_eq!(model.requests().len(), requests);
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn assert_cleanup_preserves_request(
    env: &Env,
    id: &str,
    request_id: &str,
    result: &Value,
    messages: &[Value],
) {
    assert_paired_payload_cleanup(env, result);
    let lookup = json(&env.cyber(&[
        "sessions",
        "recovery-request",
        id,
        request_id,
        "--acknowledge",
        "--format",
        "json",
    ]));
    assert_eq!(lookup["receipt"], result["receipt"]);
    assert_eq!(env.messages(id), messages);
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn assert_paired_payload_cleanup(env: &Env, result: &Value) {
    use cyber_server::runtime::snapshot_reclamation::{self as records, Receipt};
    use cyber_store::{Store, StoreOptions};
    use std::sync::Arc;
    exits(&env.cyber(&["service", "stop"]), 0);
    let root = env.root.join("cyber-home");
    let data = root.join("data");
    let database = std::fs::read_dir(&data)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.extension().is_some_and(|extension| extension == "db"))
        .unwrap();
    let store = Arc::new(
        Store::open(StoreOptions::new(
            cyber_core::paths::DatabaseLocation::File(database),
            cyber_server::runtime::Runtime::registry(),
        ))
        .unwrap(),
    );
    let receipt: cyber_server::runtime::SnapshotRecoveryReceipt =
        serde_json::from_value(result["receipt"].clone()).unwrap();
    let owner = receipt
        .checkout
        .as_ref()
        .unwrap()
        .resolution_origin
        .activity_id
        .clone();
    let receipt = Receipt::Recovery(receipt);
    let namespace = std::fs::read_dir(data.join("snapshot/global"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let journals = match &receipt {
        Receipt::Recovery(receipt) => [
            namespace.join("journals").join(&receipt.journal_id),
            namespace.join("journals").join(&receipt.resolution_id),
        ],
        _ => unreachable!(),
    };
    let intents = journals
        .each_ref()
        .map(|journal| std::fs::read(journal.join("intent.json")).unwrap());
    let original_source = env.file("bulk_0000");
    let source = env.root.join("repo");
    let target = env.root.join("cleanup-owner-target");
    std::fs::write(&target, "later owner target").unwrap();
    std::os::unix::fs::symlink(&target, source.join("cleanup-owner-link")).unwrap();
    let policy = cyber_app::snapshot_reclamation::policy(store.clone());
    let snapshots = cyber_snapshot::GitSnapshots::with_reclamation(
        data.clone(),
        Arc::new(|_| json!({"snapshots":{"retention_days":7}})),
        vec![root.join("config"), root.join("state"), root.join("cache")],
        None,
        Some(policy.clone()),
    );
    let runtime = tokio::runtime::Runtime::new().unwrap();
    assert_eq!(
        runtime
            .block_on(snapshots.reclaim(source.to_str().unwrap()))
            .unwrap(),
        0,
        "young paired receipt must retain payloads"
    );
    assert!(policy.record(&receipt).unwrap().is_none());
    store
        .transaction(move |db| {
            db.execute(
                "UPDATE event SET time=time-?1 WHERE aggregate_id=?2",
                rusqlite::params![8i64 * 86400000, owner],
            )?;
            Ok(())
        })
        .unwrap();
    assert_paired_live_owners_refuse(&runtime, &snapshots, &namespace, &receipt);
    let injection = Arc::new(std::sync::Mutex::new(None));
    let cut = cyber_snapshot::GitSnapshots::with_reclamation(
        data,
        Arc::new(|_| json!({"snapshots":{"retention_days":7}})),
        vec![root.join("config"), root.join("state"), root.join("cache")],
        None,
        Some(Arc::new(reclamation_support::Planned(
            policy.clone(),
            injection.clone(),
        ))),
    );
    assert_paired_plan_interruption(
        &runtime,
        &snapshots,
        (&cut, &injection),
        &policy,
        &receipt,
        &namespace,
        &journals,
    );
    assert!(
        runtime
            .block_on(snapshots.reclaim(source.to_str().unwrap()))
            .unwrap()
            > 0
    );
    let record = policy.record(&receipt).unwrap().unwrap();
    assert!(record.completed);
    assert_eq!(record.plan.version, 2);
    assert_eq!(
        records::Plan::id(&record.plan.receipt).unwrap(),
        records::Plan::id(&receipt).unwrap()
    );
    for (journal, intent) in journals.iter().zip(intents) {
        assert_eq!(std::fs::read(journal.join("intent.json")).unwrap(), intent);
        assert_eq!(
            std::fs::read_dir(journal).unwrap().count(),
            1,
            "only the acknowledged intent remains"
        );
    }
    assert_eq!(
        runtime
            .block_on(snapshots.reclaim(source.to_str().unwrap()))
            .unwrap(),
        0
    );
    assert_eq!(env.file("bulk_0000"), original_source);
    assert_eq!(
        std::fs::read_link(source.join("cleanup-owner-link")).unwrap(),
        target
    );
    assert_eq!(
        std::fs::read_to_string(target).unwrap(),
        "later owner target"
    );
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn assert_paired_plan_interruption(
    runtime: &tokio::runtime::Runtime,
    snapshots: &cyber_snapshot::GitSnapshots,
    cut: (
        &cyber_snapshot::GitSnapshots,
        &std::sync::Arc<std::sync::Mutex<Option<PathBuf>>>,
    ),
    policy: &std::sync::Arc<dyn cyber_snapshot::reclamation::Policy>,
    receipt: &cyber_server::runtime::snapshot_reclamation::Receipt,
    namespace: &std::path::Path,
    journals: &[PathBuf; 2],
) {
    assert!(
        runtime
            .block_on(
                cut.0.reclaim(
                    receipt
                        .origin()
                        .unwrap()
                        .binding
                        .directory
                        .to_str()
                        .unwrap()
                )
            )
            .is_err()
    );
    let pending = policy.record(receipt).unwrap().unwrap();
    assert!(!pending.completed);
    let first = &pending.plan.files[0];
    let leaf = namespace
        .join("journals")
        .join(first.journal.as_deref().unwrap())
        .join(&first.name);
    let unknown = journals[1].join("foreign-entry");
    std::fs::write(&unknown, "owner evidence").unwrap();
    assert!(
        runtime
            .block_on(
                snapshots.reclaim(
                    receipt
                        .origin()
                        .unwrap()
                        .binding
                        .directory
                        .to_str()
                        .unwrap()
                )
            )
            .is_err()
    );
    assert!(
        leaf.exists(),
        "unknown entry must refuse before any planned unlink"
    );
    std::fs::remove_file(unknown).unwrap();
    assert_unplanned_empty_slot(runtime, cut, &journals[1], receipt, &leaf);
    // Synthetic state cut of one planned unlink; the restore itself was killed
    // as a real process above. This is not a paired cleanup process-kill claim.
    std::fs::remove_file(&leaf).unwrap();
    std::fs::File::open(leaf.parent().unwrap())
        .unwrap()
        .sync_all()
        .unwrap();
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn assert_unplanned_empty_slot(
    runtime: &tokio::runtime::Runtime,
    cut: (
        &cyber_snapshot::GitSnapshots,
        &std::sync::Arc<std::sync::Mutex<Option<PathBuf>>>,
    ),
    journal: &std::path::Path,
    receipt: &cyber_server::runtime::snapshot_reclamation::Receipt,
    retained: &std::path::Path,
) {
    use cyber_server::runtime::{SnapshotRecoveryAction, snapshot_reclamation::Receipt};
    let Receipt::Recovery(pair) = receipt else {
        panic!("paired fixture");
    };
    if pair.action != SnapshotRecoveryAction::Rollback {
        return;
    }
    let intent: Value =
        serde_json::from_slice(&std::fs::read(journal.join("intent.json")).unwrap()).unwrap();
    let index = intent["changes"]
        .as_array()
        .unwrap()
        .iter()
        .position(|change| change["before"].is_null())
        .expect("rollback fixture needs a genuinely empty private stage");
    let path = journal.join(format!("stage_{index}"));
    assert!(!path.exists());
    *cut.1.lock().unwrap() = Some(path.clone());
    let error = runtime
        .block_on(
            cut.0
                .reclaim(pair.original.binding.directory.to_str().unwrap()),
        )
        .unwrap_err();
    assert!(error.contains("Unplanned entry"), "{error}");
    assert!(
        retained.exists(),
        "no planned payload may be removed before complete preflight"
    );
    assert_eq!(std::fs::read(&path).unwrap(), b"unplanned owner evidence");
    std::fs::remove_file(path).unwrap();
    *cut.1.lock().unwrap() = None;
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn assert_paired_live_owners_refuse(
    runtime: &tokio::runtime::Runtime,
    snapshots: &cyber_snapshot::GitSnapshots,
    namespace: &std::path::Path,
    receipt: &cyber_server::runtime::snapshot_reclamation::Receipt,
) {
    for proof in receipt.owners().unwrap() {
        let origin = proof.origin;
        let held =
            std::fs::File::open(namespace.join("workers").join(&origin.activity_id)).unwrap();
        held.try_lock().unwrap();
        assert!(
            runtime
                .block_on(snapshots.reclaim(origin.binding.directory.to_str().unwrap()))
                .is_err()
        );
        drop(held);
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn kill_partial_restore(env: &Env, id: &str, first: &str) -> Vec<Value> {
    let selection = reviewed_selection(env, first, "both", "commit");
    let preview = reviewed_preview(env, id, &selection);
    let mut body = selection;
    body["confirmation"] = preview["confirmation"].clone();
    let body = body.to_string();
    let path = format!("/sessions/{id}/rewind/confirm");
    let mut pending = env
        .command(&["api", "POST", &path, "-d", &body])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    // Git materialization, native inventory checks and durable stage construction
    // all precede the first source exchange, especially for mixed link/file images.
    let deadline = std::time::Instant::now() + Duration::from_secs(120);
    while env.file("bulk_0000").as_deref() != Some("before") {
        if pending.try_wait().unwrap().is_some() {
            let output = pending.wait_with_output().unwrap();
            panic!("restore exited before a source exchange: {output:?}");
        }
        assert!(
            std::time::Instant::now() < deadline,
            "source exchange was never observed: {}",
            restore_preparation_status(env)
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(
        env.file("bulk_1279").as_deref(),
        Some("after"),
        "no partial restore window was observed"
    );
    kill_registered_service(env);
    assert!(!pending.wait_with_output().unwrap().status.success());
    let messages = env.messages(id); // Starts a fresh server over the durable evidence.
    assert_eq!(env.file("bulk_0000").as_deref(), Some("before"));
    assert_eq!(
        env.file("bulk_1279").as_deref(),
        Some("after"),
        "kill did not retain a partial namespace"
    );
    assert_eq!(
        env.user_ids(id).len(),
        2,
        "failed restore moved the transcript"
    );
    messages
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn restore_preparation_status(env: &Env) -> String {
    let root = env.root.join("cyber-home/data/snapshot/global");
    let mut states = Vec::new();
    for namespace in std::fs::read_dir(root).into_iter().flatten().flatten() {
        let journals = namespace.path().join("journals");
        for journal in std::fs::read_dir(journals).into_iter().flatten().flatten() {
            let path = journal.path();
            let intent = std::fs::read(path.join("intent.json"))
                .ok()
                .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
            let stages = std::fs::read_dir(&path)
                .into_iter()
                .flatten()
                .flatten()
                .filter(|entry| entry.file_name().to_string_lossy().starts_with("stage_"))
                .count();
            states.push(json!({
                "journal": journal.file_name().to_string_lossy(),
                "stages": stages,
                "changes": intent.as_ref().and_then(|v| v["changes"].as_array()).map(Vec::len),
                "applied": intent.as_ref().map(|v| &v["applied"]),
                "completed": intent.as_ref().map(|v| &v["completed"]),
            }));
        }
    }
    json!(states).to_string()
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn assert_native_settlement_link(env: &Env, journal: &str, result: &Value) {
    let namespace = std::fs::read_dir(env.root.join("cyber-home/data/snapshot/global"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let old: Value = serde_json::from_slice(
        &std::fs::read(namespace.join("journals").join(journal).join("intent.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(old["completed"], false);
    assert_eq!(old["acknowledged"], true);
    assert_eq!(old["settled_by"], result["receipt"]["resolution_id"]);
    assert_recovery_checkout_lifetimes(&namespace, &result["receipt"]);
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn assert_recovery_checkout_lifetimes(namespace: &std::path::Path, receipt: &Value) {
    let checkout = &receipt["checkout"];
    assert_eq!(checkout["version"], 1);
    assert_ne!(
        checkout["original"]["lifetime"]["owner"]["activity_id"],
        checkout["resolution"]["lifetime"]["owner"]["activity_id"]
    );
    for (role, origin) in [
        ("original", &receipt["original"]),
        ("resolution", &checkout["resolution_origin"]),
    ] {
        let evidence = &checkout[role];
        let lifetime: cyber_server::runtime::SnapshotLifetimeBinding =
            serde_json::from_value(evidence["lifetime"].clone()).unwrap();
        assert_eq!(
            lifetime.owner.activity_id,
            origin["activity_id"].as_str().unwrap()
        );
        let recorded: Value = serde_json::from_slice(
            &std::fs::read(namespace.join("workers").join(&lifetime.owner.activity_id)).unwrap(),
        )
        .unwrap();
        assert_eq!(recorded, evidence["lifetime"]);
        let phase: Value = serde_json::from_slice(
            &std::fs::read(
                namespace
                    .join("workers")
                    .join(lifetime.source_phase.as_ref().unwrap()),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(phase["lifetime"], evidence["lifetime"]);
        let bindings: Vec<cyber_core::worktrees::CheckoutBinding> =
            serde_json::from_value(phase["checkout_bindings"].clone()).unwrap();
        if let Some(worktree) = &lifetime.owner.worktree_id {
            assert!(!bindings.is_empty());
            assert!(
                bindings
                    .iter()
                    .any(|binding| &binding.worktree_id == worktree)
            );
        } else {
            assert!(bindings.is_empty());
        }
        assert_eq!(
            evidence["resources_digest"],
            lifetime.checkout_resources_digest(&bindings).unwrap()
        );
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn assert_recovery_lookup_does_not_replay(
    env: &Env,
    id: &str,
    request_id: &str,
    request: &Value,
    result: &Value,
    messages: &[Value],
) {
    let repo = &env.directory;
    std::fs::write(repo.join("bulk_0000"), "new owner edit after recovery").unwrap();
    let retry = json(&recovery_confirm_cli(env, id, request));
    assert_eq!(retry["state"], "acknowledged");
    assert!(
        retry["journal_acknowledged"].is_null(),
        "lookup invented native acknowledgement observation"
    );
    assert_eq!(
        env.file("bulk_0000").as_deref(),
        Some("new owner edit after recovery")
    );
    let mut changed = request.clone();
    changed["confirmation"] = "b".repeat(64).into();
    assert!(
        !recovery_confirm_cli(env, id, &changed).status.success(),
        "changed request content reused an existing owner"
    );
    kill_registered_service(env);
    let discovered = json(&env.cyber(&[
        "api",
        "GET",
        &format!(
            "/sessions/{id}/rewind/recovery/requests?journal_id={}",
            request["journal_id"].as_str().unwrap()
        ),
    ]));
    assert_eq!(discovered["data"], json!([request]));
    let lookup = json(&env.cyber(&[
        "sessions",
        "recovery-request",
        id,
        request_id,
        "--format",
        "json",
    ]));
    assert!(lookup["journal_acknowledged"].is_null());
    assert_eq!(lookup["receipt"], result["receipt"]);
    assert_eq!(
        env.file("bulk_0000").as_deref(),
        Some("new owner edit after recovery")
    );
    assert_eq!(env.messages(id).as_slice(), messages);
    reconcile_missing_journal_acknowledgement(env, id, request_id, result);
    assert_eq!(env.messages(id).as_slice(), messages);
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn reconcile_missing_journal_acknowledgement(
    env: &Env,
    id: &str,
    request_id: &str,
    result: &Value,
) {
    let namespace = std::fs::read_dir(env.root.join("cyber-home/data/snapshot/global"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    // Reproduce a cut after the durable owner pair but before either native ack.
    // Only mutable acknowledgement metadata is reset; the persisted digest stays valid.
    for journal in [
        &result["receipt"]["journal_id"],
        &result["receipt"]["resolution_id"],
    ] {
        let path = namespace
            .join("journals")
            .join(journal.as_str().unwrap())
            .join("intent.json");
        let mut intent: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        intent["acknowledged"] = false.into();
        intent.as_object_mut().unwrap().remove("settled_by");
        std::fs::write(path, serde_json::to_vec(&intent).unwrap()).unwrap();
    }
    let target = env.root.join("ack-owner-target");
    std::fs::write(&target, "new owner edit after recovery").unwrap();
    let source = env.directory.join("bulk_0000");
    std::fs::remove_file(&source).unwrap();
    std::os::unix::fs::symlink(&target, &source).unwrap();
    let reconciled = json(&env.cyber(&[
        "sessions",
        "recovery-request",
        id,
        request_id,
        "--acknowledge",
        "--format",
        "json",
    ]));
    assert_eq!(reconciled["receipt"], result["receipt"]);
    assert_eq!(reconciled["journal_acknowledged"], true);
    assert!(
        std::fs::symlink_metadata(source)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        std::fs::read_to_string(target).unwrap(),
        "new owner edit after recovery"
    );
    assert_native_settlement_link(
        env,
        result["receipt"]["journal_id"].as_str().unwrap(),
        result,
    );
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
    reconcile_ordinary_restore_metadata(&env, &id, journals[0]["id"].as_str().unwrap());
    assert_eq!(model.requests().len(), requests);
}

#[test]
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn reviewed_non_git_noop_rewind_keeps_a_real_unadmitted_empty_journal() {
    let model = scripted_model::ScriptedModel::start(reply);
    let env = Env::new(&model);
    std::fs::remove_dir_all(env.root.join("repo/.git")).unwrap();
    let id = env.exec(None, "WRITE-A");
    let first = env.user_ids(&id)[0].clone();
    // The owner already restored the target bytes before preview. Conversation
    // rewind still requires an exact owned completion, without any source effect.
    std::fs::remove_file(env.root.join("repo/a.txt")).unwrap();
    std::fs::write(env.root.join("repo/owner.txt"), "later owner file").unwrap();
    let requests = model.requests().len();
    let preview = rewind_preview(&env, &id, &first, "both");
    json(&confirm_rewind(&env, &id, &first, "both", &preview));
    assert!(env.user_ids(&id).is_empty());
    assert_eq!(env.file("a.txt"), None);
    assert_eq!(env.file("owner.txt").as_deref(), Some("later owner file"));
    let catalog = json(&env.cyber(&["api", "GET", &format!("/sessions/{id}/rewind/recovery")]));
    let journals = catalog["data"].as_array().unwrap();
    assert_eq!(journals.len(), 1);
    assert_eq!(journals[0]["filesystem_complete"], true);
    assert!(journals[0]["paths"].as_array().unwrap().is_empty());
    let journal = journals[0]["id"].as_str().unwrap();
    assert_owned_snapshot_receipt(&env, &id, journal);
    let namespace = std::fs::read_dir(env.root.join("cyber-home/data/snapshot/global"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let intent: Value = serde_json::from_slice(
        &std::fs::read(namespace.join("journals").join(journal).join("intent.json")).unwrap(),
    )
    .unwrap();
    assert!(intent["changes"].as_array().unwrap().is_empty());
    assert!(intent["ownership"]["files"].as_array().unwrap().is_empty());
    let activity = intent["origin"]["activity_id"].as_str().unwrap();
    let route = format!("/sessions/{id}/rewind/restores/{activity}");
    let receipt = json(&env.cyber(&["api", "GET", &route]));
    let phase_name = receipt["data"]["checkout"]["lifetime"]["source_phase"]
        .as_str()
        .unwrap();
    let phase: Value =
        serde_json::from_slice(&std::fs::read(namespace.join("workers").join(phase_name)).unwrap())
            .unwrap();
    assert_eq!(phase["admitted"], false);
    assert!(phase["origin"].is_null());
    reconcile_ordinary_restore_metadata(&env, &id, journal);
    assert_eq!(env.file("owner.txt").as_deref(), Some("later owner file"));
    assert_eq!(model.requests().len(), requests);
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn reconcile_ordinary_restore_metadata(env: &Env, session: &str, journal: &str) {
    let namespace = std::fs::read_dir(env.root.join("cyber-home/data/snapshot/global"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let path = namespace.join("journals").join(journal).join("intent.json");
    let mut intent: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let activity = intent["origin"]["activity_id"].as_str().unwrap().to_owned();
    let route = format!("/sessions/{session}/rewind/restores/{activity}");
    let receipt = json(&env.cyber(&["api", "GET", &route]))["data"].clone();
    assert_eq!(receipt["origin"], intent["origin"]);
    assert_eq!(receipt["intent_digest"].as_str().unwrap().len(), 64);
    // Simulate an acknowledgement flush cut without changing immutable intent.
    intent["acknowledged"] = false.into();
    std::fs::write(&path, serde_json::to_vec(&intent).unwrap()).unwrap();
    let target = env.root.join("ordinary-ack-owner");
    std::fs::write(&target, "later owner bytes").unwrap();
    let source = env.root.join("repo/a.txt");
    std::os::unix::fs::symlink(&target, &source).unwrap();
    let before = env.messages(session);
    for _ in 0..2 {
        let result =
            json(&env.cyber(&["api", "POST", &format!("{route}/acknowledge"), "-d", "{}"]));
        assert_eq!(result["data"], receipt);
    }
    let acknowledged: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert_eq!(acknowledged["acknowledged"], true);
    assert!(
        std::fs::symlink_metadata(source)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        std::fs::read_to_string(target).unwrap(),
        "later owner bytes"
    );
    assert_eq!(env.messages(session), before);
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
    assert_snapshot_lifetime(&namespaces[0], &activity, origin);
    assert_eq!(activity["status"], "settled");
    assert_eq!(activity["phase"], "snapshot_restore_committed");
    assert_restore_receipt_binding(&activity, origin);
    let created: String = db
        .query_row(
            "SELECT id FROM event WHERE aggregate_id=?1 ORDER BY seq LIMIT 1",
            [session],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(origin["session_created_id"], created);
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn assert_restore_receipt_binding(activity: &Value, origin: &Value) {
    let receipt = &activity["snapshot_restore_receipt"];
    assert_eq!(receipt["origin"], *origin);
    assert_eq!(
        receipt["checkout"]["lifetime"],
        activity["snapshot_lifetime"]
    );
    assert_ne!(
        receipt["checkout"]["lifetime"]["namespace_identity"],
        origin["binding"]["namespace_identity"],
        "lifetime namespace and journal collection are distinct native directories"
    );
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn assert_snapshot_lifetime(namespace: &std::path::Path, activity: &Value, origin: &Value) {
    let lifetime = &activity["snapshot_lifetime"];
    assert_eq!(lifetime["version"], 2);
    assert_eq!(lifetime["owner"]["activity_id"], origin["activity_id"]);
    assert_eq!(
        lifetime["owner"]["session_created_id"],
        origin["session_created_id"]
    );
    let lifetime_file = namespace
        .join("workers")
        .join(origin["activity_id"].as_str().unwrap());
    let recorded_lifetime: Value =
        serde_json::from_slice(&std::fs::read(lifetime_file).unwrap()).unwrap();
    assert_eq!(recorded_lifetime, *lifetime);
    let phase: Value = serde_json::from_slice(
        &std::fs::read(
            namespace
                .join("workers")
                .join(lifetime["source_phase"].as_str().unwrap()),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(phase["lifetime"], *lifetime);
    assert_eq!(phase["admitted"], activity["snapshot_source_admitted"]);
    if phase["admitted"] == true {
        assert_eq!(phase["origin"], *origin);
    } else {
        assert_eq!(phase["admitted"], false);
        assert!(
            phase["origin"].is_null(),
            "no-op publication must not admit source effects"
        );
    }
    // These real-service fixtures use unmanaged Locations. An explicitly
    // published empty set is distinct from missing constructor evidence.
    assert_eq!(phase["checkout_bindings"], serde_json::json!([]));
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

#[test]
fn managed_git_fixture_panic_retains_its_private_evidence() {
    let model = scripted_model::ScriptedModel::start(reply);
    let env = Env::new(&model);
    let root = env.root.clone();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        let _owned_fixture = env;
        panic!("fixture preservation probe");
    }));
    assert!(result.is_err());
    assert!(root.join("repo/.git").is_dir());
    // This probe created no Session/native work or unknown evidence. Only its
    // own empty fixture is removed; failed recovery fixtures stay retained.
    std::fs::remove_dir_all(root).unwrap();
}

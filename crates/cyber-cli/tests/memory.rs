//! Real binary memory commands with isolated paths and editor processes.
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    process::{Command, Output},
};
struct Env {
    _temp: tempfile::TempDir,
    root: PathBuf,
}
impl Env {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        Self { _temp: temp, root }
    }
    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_cyber"));
        command
            .args(["--format", "json"])
            .args(args)
            .env_clear()
            .env("HOME", self.root.join("home"))
            .env("CYBER_HOME", self.root.join("cyber"))
            .env("CYBER_DB", "memory-test.db")
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .current_dir(&self.root);
        #[cfg(windows)]
        if let Some(root) = std::env::var_os("SystemRoot") {
            command.env("SystemRoot", root);
        }
        command
    }
    fn run(&self, args: &[&str]) -> Output {
        self.command(args).output().unwrap()
    }
    fn data(&self) -> PathBuf {
        self.root.join("cyber/data")
    }
    #[cfg(unix)]
    fn note_path(&self) -> PathBuf {
        self.data().join("memory/global/coding-policy.md")
    }
    fn no_database(&self) {
        assert!(!self.data().join("memory-test.db").exists());
    }
    #[cfg(unix)]
    fn editor(&self, body: &str, content: &str) -> Command {
        let script = self.root.join("editor script.sh");
        std::fs::write(&script, body).unwrap();
        let source = self.root.join("edited source.md");
        std::fs::write(&source, content).unwrap();
        let editor = format!(
            "/bin/sh {} {}",
            shell_words::quote(script.to_str().unwrap()),
            shell_words::quote(source.to_str().unwrap())
        );
        let mut command = self.command(&["memory", "edit", "coding-policy", "--global"]);
        command
            .env("EDITOR", editor)
            .env("MEMORY_PATH", self.note_path())
            .env("INDEX_PATH", self.data().join("memory/global/MEMORY.md"));
        command
    }
}
fn body(output: Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}
#[cfg(unix)]
fn note(fact: &str) -> String {
    format!("---\nname: coding-policy\ndescription: Coding policy\ntype: reference\n---\n{fact}\n")
}

#[test]
fn empty_list_path_and_debug_do_not_create_scopes_or_open_database() {
    let env = Env::new();
    let path = body(env.run(&["memory", "path", "--global"]));
    assert_eq!(path["project_id"], "global");
    assert!(!env.data().join("memory/global").exists());
    assert_eq!(
        body(env.run(&["memory", "list", "--global"])),
        json!({"memories":[],"invalid":[]})
    );
    assert_eq!(
        body(env.run(&["debug", "memory", "--global"])),
        json!({"memories":[],"invalid":[]})
    );
    env.no_database();
}

#[test]
fn invalid_names_and_missing_notes_fail_without_scope_effects() {
    let env = Env::new();
    for args in [
        vec!["memory", "show", "../escape"],
        vec!["memory", "edit", "../escape"],
        vec!["memory", "show", "missing"],
        vec!["memory", "delete", "missing"],
    ] {
        assert!(!env.run(&args).status.success());
        assert!(!env.data().join("memory/global").exists());
    }
    env.no_database();
}

#[cfg(unix)]
#[test]
fn editor_with_quoted_arguments_commits_note_index_and_delete() {
    let env = Env::new();
    let saved = body(
        env.editor("cp \"$1\" \"$2\"\n", &note("Durable fact"))
            .output()
            .unwrap(),
    );
    assert_eq!(saved["deleted"], false);
    assert_eq!(saved["name"], "coding-policy");
    assert_eq!(
        body(env.run(&["memory", "show", "coding-policy", "--global"]))["body"],
        "Durable fact"
    );
    assert!(
        std::fs::read_to_string(env.data().join("memory/global/MEMORY.md"))
            .unwrap()
            .contains("coding-policy.md")
    );
    assert_eq!(
        body(env.run(&["memory", "list", "--global"]))["memories"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        body(env.run(&["memory", "delete", "coding-policy", "--global"]))["deleted"],
        true
    );
    assert!(!env.note_path().exists());
    env.no_database();
}

#[cfg(unix)]
#[test]
fn rejected_editor_content_retains_private_draft_without_memory_writes_or_raw_diagnostics() {
    use std::os::unix::fs::PermissionsExt;
    for content in [
        note("password = must-not-appear"),
        "bad YAML input".into(),
        note("fact").replace("coding-policy", "other-name"),
    ] {
        let env = Env::new();
        let output = env.editor("cp \"$1\" \"$2\"\n", &content).output().unwrap();
        assert!(!output.status.success());
        assert!(!String::from_utf8_lossy(&output.stderr).contains("must-not-appear"));
        assert!(!env.note_path().exists());
        assert!(!env.data().join("memory/global/MEMORY.md").exists());
        assert!(
            !env.data()
                .join("memory/global/.memory-transaction")
                .exists()
        );
        let drafts = std::fs::read_dir(env.data())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().starts_with(".memory-edit-"))
            .collect::<Vec<_>>();
        assert_eq!(drafts.len(), 1);
        assert_eq!(
            std::fs::read_to_string(drafts[0].path().join("note.md")).unwrap(),
            content
        );
        assert_eq!(
            std::fs::metadata(drafts[0].path())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(drafts[0].path().join("note.md"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        env.no_database();
    }
}

#[cfg(unix)]
#[test]
fn external_target_or_index_edit_is_preserved_and_has_no_pending_replay_intent() {
    for variable in ["MEMORY_PATH", "INDEX_PATH"] {
        let env = Env::new();
        body(
            env.editor("cp \"$1\" \"$2\"\n", &note("Original fact"))
                .output()
                .unwrap(),
        );
        let script =
            format!("cp \"$1\" \"$2\"\nprintf '%s' 'external user edit' > \"${variable}\"\n");
        let output = env.editor(&script, &note("Updated fact")).output().unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("Memory changed since review"));
        let target = if variable == "MEMORY_PATH" {
            env.note_path()
        } else {
            env.data().join("memory/global/MEMORY.md")
        };
        assert_eq!(
            std::fs::read_to_string(target).unwrap(),
            "external user edit"
        );
        assert!(
            !env.data()
                .join("memory/global/.memory-transaction")
                .exists()
        );
    }
}

#[cfg(unix)]
#[test]
fn editor_failure_symlink_hardlink_and_fifo_refuse_memory_commit() {
    for script in [
        "exit 7\n",
        "rm \"$2\"; ln -s \"$1\" \"$2\"\n",
        "rm \"$2\"; ln \"$1\" \"$2\"\n",
        "rm \"$2\"; mkfifo \"$2\"\n",
    ] {
        let env = Env::new();
        let output = env.editor(script, &note("fact")).output().unwrap();
        assert!(!output.status.success());
        assert!(!env.note_path().exists());
        assert!(
            !env.data()
                .join("memory/global/.memory-transaction")
                .exists()
        );
    }
}

#[cfg(unix)]
#[test]
fn unchanged_editor_does_not_create_placeholder_note_and_debug_lists_invalid_metadata() {
    let env = Env::new();
    let value = body(env.editor("exit 0\n", "").output().unwrap());
    assert_eq!(value["changed"], false);
    assert!(!env.note_path().exists());
    body(
        env.editor("cp \"$1\" \"$2\"\n", &note("Fact"))
            .output()
            .unwrap(),
    );
    std::fs::write(env.note_path(), "invalid frontmatter").unwrap();
    let debug = body(env.run(&["debug", "memory", "--global"]));
    assert_eq!(debug["invalid"].as_array().unwrap().len(), 1);
    body(
        env.editor("cp \"$1\" \"$2\"\n", &note("Repaired fact"))
            .output()
            .unwrap(),
    );
    assert_eq!(
        body(env.run(&["memory", "show", "coding-policy", "--global"]))["body"],
        "Repaired fact"
    );
}

#[test]
fn disabled_readonly_and_unsupported_mutations_refuse_editor_start() {
    for config in [
        json!({"memory":{"enabled":false}}),
        json!({"memory":{"generate":false}}),
    ] {
        let env = Env::new();
        std::fs::create_dir_all(env.root.join("cyber/config")).unwrap();
        std::fs::write(
            env.root.join("cyber/config/cyber.jsonc"),
            config.to_string(),
        )
        .unwrap();
        let output = env.run(&["memory", "edit", "coding-policy"]);
        assert!(!output.status.success());
        assert!(!env.data().join("memory/global").exists());
        env.no_database();
    }
    #[cfg(not(unix))]
    {
        let env = Env::new();
        assert!(
            !env.run(&["memory", "edit", "coding-policy"])
                .status
                .success()
        );
        assert!(!env.data().join("memory/global").exists());
    }
}

#[test]
fn absent_recovery_and_invalid_review_create_no_scope_or_database() {
    let env = Env::new();
    assert_eq!(
        body(env.run(&["memory", "recovery", "--global"])),
        Value::Null
    );
    let output = env.run(&["memory", "recover", "--review", "unreviewed", "--global"]);
    assert!(!output.status.success());
    assert!(!env.data().join("memory/global").exists());
    env.no_database();
}

#[cfg(unix)]
fn prepare_recovery(env: &Env) -> cyber_core::memory::MemoryStore {
    std::fs::create_dir_all(env.data()).unwrap();
    let store = cyber_core::memory::MemoryStore::open(&env.data(), "global").unwrap();
    let mut scope = store.claim().unwrap();
    scope.write(&note("Original fact")).unwrap();
    drop(scope.prepare_write(&note("Proposed fact")).unwrap());
    drop(scope);
    store
}

#[cfg(unix)]
#[test]
fn reviewed_recovery_commits_once_without_creating_a_database() {
    let env = Env::new();
    let _store = prepare_recovery(&env);
    let review = body(env.run(&["memory", "recovery", "--global"]));
    assert_eq!(review["proposed_note"]["body"], "Proposed fact");
    assert_eq!(review["completed"], false);
    assert!(
        std::fs::read_to_string(env.note_path())
            .unwrap()
            .contains("Original fact")
    );
    let fingerprint = review["fingerprint"].as_str().unwrap();
    let receipt = body(env.run(&["memory", "recover", "--review", fingerprint, "--global"]));
    assert_eq!(receipt, review["receipt"]);
    assert_eq!(
        body(env.run(&["memory", "show", "coding-policy", "--global"]))["body"],
        "Proposed fact"
    );
    assert_eq!(
        body(env.run(&["memory", "recovery", "--global"])),
        Value::Null
    );
    assert!(
        !env.run(&["memory", "recover", "--review", fingerprint, "--global"])
            .status
            .success()
    );
    env.no_database();
}

#[cfg(unix)]
#[test]
fn stale_recovery_and_fresh_settings_preserve_files_and_journal() {
    for mode in ["stale", "disabled", "readonly"] {
        let env = Env::new();
        let _store = prepare_recovery(&env);
        let review = body(env.run(&["memory", "recovery", "--global"]));
        let fingerprint = review["fingerprint"].as_str().unwrap();
        let mut command = env.command(&["memory", "recover", "--review", fingerprint, "--global"]);
        match mode {
            "stale" => std::fs::write(env.note_path(), note("External fact")).unwrap(),
            "disabled" => {
                command.env("CYBER_DISABLE_MEMORY", "1");
            }
            "readonly" => {
                command.args(["-c", "memory.generate=false"]);
            }
            _ => unreachable!(),
        }
        let output = command.output().unwrap();
        assert!(!output.status.success(), "{mode}");
        let current = std::fs::read_to_string(env.note_path()).unwrap();
        assert!(current.contains(if mode == "stale" {
            "External fact"
        } else {
            "Original fact"
        }));
        assert!(
            env.data()
                .join("memory/global/.memory-transaction/note.after")
                .exists()
        );
        env.no_database();
    }
}

#[cfg(unix)]
#[test]
fn durable_pending_admission_and_unverifiable_database_refuse_file_recovery() {
    for mode in [
        "pending",
        "corrupt",
        "memory",
        "old-schema",
        "completed",
        "other-scope",
    ] {
        let env = Env::new();
        let _store = prepare_recovery(&env);
        let review = body(env.run(&["memory", "recovery", "--global"]));
        let fingerprint = review["fingerprint"].as_str().unwrap();
        let db_path = env.data().join("memory-test.db");
        if mode == "corrupt" {
            std::fs::write(&db_path, b"not a database").unwrap();
        } else if mode != "memory" {
            let db = rusqlite::Connection::open(&db_path).unwrap();
            if mode != "old-schema" {
                db.execute_batch("CREATE TABLE memory_mutation(project_id TEXT, result TEXT)")
                    .unwrap();
                db.execute(
                    "INSERT INTO memory_mutation VALUES(?1,?2)",
                    rusqlite::params![
                        if mode == "other-scope" {
                            "another"
                        } else {
                            "global"
                        },
                        if mode == "completed" {
                            Some("completed")
                        } else {
                            None
                        }
                    ],
                )
                .unwrap();
            }
        }
        let mut command = env.command(&["memory", "recover", "--review", fingerprint, "--global"]);
        if mode == "memory" {
            command.env("CYBER_DB", ":memory:");
        }
        let output = command.output().unwrap();
        let allowed = matches!(mode, "old-schema" | "completed" | "other-scope");
        assert_eq!(
            output.status.success(),
            allowed,
            "{mode}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        if !allowed {
            assert!(
                std::fs::read_to_string(env.note_path())
                    .unwrap()
                    .contains("Original fact")
            );
            assert!(
                env.data()
                    .join("memory/global/.memory-transaction/note.after")
                    .exists()
            );
        }
        if mode == "pending" {
            let db = rusqlite::Connection::open(&db_path).unwrap();
            assert!(
                db.query_row("SELECT result IS NULL FROM memory_mutation", [], |r| r
                    .get::<_, bool>(0))
                    .unwrap()
            );
        }
    }
}

#[cfg(unix)]
#[test]
fn recovery_refuses_database_symlink_and_unknown_memory_schema() {
    for alias in [false, true] {
        let env = Env::new();
        let _store = prepare_recovery(&env);
        let review = body(env.run(&["memory", "recovery", "--global"]));
        let db_path = env.data().join("memory-test.db");
        if alias {
            let target = env.root.join("aliased.db");
            rusqlite::Connection::open(&target).unwrap();
            std::os::unix::fs::symlink(target, &db_path).unwrap();
        } else {
            rusqlite::Connection::open(&db_path)
                .unwrap()
                .execute_batch("CREATE TABLE memory_mutation(unrecognized TEXT)")
                .unwrap();
        }
        let output = env.run(&[
            "memory",
            "recover",
            "--review",
            review["fingerprint"].as_str().unwrap(),
            "--global",
        ]);
        assert!(!output.status.success());
        assert!(
            String::from_utf8_lossy(&output.stderr)
                .contains("cannot verify database admission ownership")
        );
        assert!(
            std::fs::read_to_string(env.note_path())
                .unwrap()
                .contains("Original fact")
        );
        assert!(
            env.data()
                .join("memory/global/.memory-transaction/note.after")
                .exists()
        );
    }
}

#[test]
fn server_recovery_requires_explicit_confirmation_and_existing_registration() {
    let env = Env::new();
    let fingerprint = "a".repeat(64);
    for args in [
        vec!["memory", "recovery", "--server"],
        vec!["memory", "recover", "--server", "--review", &fingerprint],
        vec![
            "memory",
            "recover",
            "--server",
            "--review",
            &fingerprint,
            "--admission-review",
            &fingerprint,
            "--key",
            "retained",
        ],
        vec![
            "memory",
            "recovery-request",
            "--server",
            "--key",
            "retained",
        ],
        vec!["memory", "recovery-request", "--key", "retained"],
        vec!["memory", "list", "--server"],
    ] {
        assert!(!env.run(&args).status.success(), "{args:?}");
        assert!(!env.data().join("memory/global").exists());
        assert!(!env.root.join("cyber/state/server.json").exists());
        assert!(!env.root.join("cyber/state/password").exists());
        env.no_database();
    }
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn server_cli_reconciles_paired_reviews_and_retains_receipts_after_cache_loss() {
    use cyber_core::paths::{DatabaseLocation, Paths};
    use cyber_server::runtime::{MemoryAdmission, MemoryWrite};
    let env = Env::new();
    let paths = Paths {
        data: env.data(),
        config: env.root.join("cyber/config"),
        state: env.root.join("cyber/state"),
        cache: env.root.join("cyber/cache"),
        tmp: env.root.join("cyber/tmp"),
    };
    paths.ensure().unwrap();
    let app = cyber_app::App::build(cyber_app::AppOptions {
        paths: paths.clone(),
        home: env.root.join("home"),
        database: DatabaseLocation::File(env.data().join("memory-test.db")),
        default_directory: env.root.clone(),
        sandbox_policy: None,
        snapshots: false,
        interactive: true,
        password: Some("cli-memory-existing-password".into()),
    })
    .await
    .unwrap();
    let memory = cyber_core::memory::MemoryStore::open(&env.data(), "global").unwrap();
    {
        let mut scope = memory.claim().unwrap();
        scope.write(&note("Original fact")).unwrap();
        let MemoryAdmission::Owned(mut owner) = app
            .runtime
            .admit_memory_write(MemoryWrite {
                directory: &env.root,
                project_id: "global",
                name: "coding-policy",
                deleted: false,
                identity: Some("cli-pending-write"),
                content: &note("Proposed fact"),
                http_hash: None,
            })
            .unwrap()
        else {
            panic!("fresh admission")
        };
        let prepared = scope.prepare_write(&note("Proposed fact")).unwrap();
        owner
            .bind_journal(prepared.journal_identity().unwrap())
            .unwrap();
        drop(prepared);
        drop(owner);
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let registration = cyber_app::Registration {
        id: "cli-memory-server".into(),
        version: cyber_app::version().into(),
        url: format!("http://{}", listener.local_addr().unwrap()),
        socket: None,
        pid: std::process::id(),
    };
    std::fs::write(
        paths.state.join("server.json"),
        serde_json::to_vec(&registration).unwrap(),
    )
    .unwrap();
    std::fs::write(paths.state.join("password"), "cli-memory-existing-password").unwrap();
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(cyber_server::http::serve_tcp(
        cyber_server::http::router(app.state.clone()),
        listener,
        async {
            let _ = stopped.await;
        },
    ));
    std::fs::write(paths.state.join("password"), "wrong-existing-password").unwrap();
    assert!(
        !env.run(&["memory", "recovery", "--server", "--global"])
            .status
            .success()
    );
    std::fs::write(paths.state.join("password"), "cli-memory-existing-password").unwrap();
    let review = body(env.run(&["memory", "recovery", "--server", "--global"]));
    assert_eq!(
        review["data"]["storage"]["proposed_note"]["body"],
        "Proposed fact"
    );
    let storage = review["data"]["storage"]["fingerprint"].as_str().unwrap();
    let admission = review["data"]["admission"]["fingerprint"].as_str().unwrap();
    let key = "cli/recovery+retained";
    let confirm = [
        "memory",
        "recover",
        "--server",
        "--global",
        "--review",
        storage,
        "--admission-review",
        admission,
        "--key",
        key,
    ];
    let stale = format!("sha256:{}", "0".repeat(64));
    assert!(
        !env.run(&[
            "memory",
            "recover",
            "--server",
            "--global",
            "--review",
            storage,
            "--admission-review",
            &stale,
            "--key",
            "stale-cli-review"
        ])
        .status
        .success()
    );
    let config = paths.config.join("cyber.jsonc");
    std::fs::write(&config, json!({"memory":{"generate":false}}).to_string()).unwrap();
    let mut readonly = confirm;
    readonly[9] = "readonly-cli-review";
    assert!(!env.run(&readonly).status.success());
    assert!(
        std::fs::read_to_string(env.note_path())
            .unwrap()
            .contains("Original fact")
    );
    assert_eq!(
        app.store
            .read_events(review["data"]["admission"]["id"].as_str().unwrap(), -1, 20)
            .unwrap()
            .events
            .len(),
        2
    );
    std::fs::write(config, "{}").unwrap();
    let change = body(env.run(&confirm));
    assert_eq!(
        change["data"]["receipt"],
        review["data"]["storage"]["receipt"]
    );
    app.store
        .transaction(|tx| {
            tx.execute("DELETE FROM idempotency_key", [])?;
            Ok(())
        })
        .unwrap();
    assert_eq!(body(env.run(&confirm)), change);
    let status = body(env.run(&[
        "memory",
        "recovery-request",
        "--server",
        "--global",
        "--key",
        key,
    ]));
    assert_eq!(status["data"]["completed"], change["data"]);
    let foreign = env.root.join("foreign");
    std::fs::create_dir(&foreign).unwrap();
    assert!(
        !env.command(&[
            "memory",
            "recovery-request",
            "--server",
            "--global",
            "--key",
            key
        ])
        .arg("--cwd")
        .arg(&foreign)
        .output()
        .unwrap()
        .status
        .success()
    );

    assert_eq!(
        body(env.run(&["memory", "recovery", "--server", "--global"]))["data"],
        Value::Null
    );
    assert_eq!(
        app.store
            .read_events(change["data"]["id"].as_str().unwrap(), -1, 20)
            .unwrap()
            .events
            .len(),
        4
    );
    assert!(
        std::fs::read_to_string(env.note_path())
            .unwrap()
            .contains("Proposed fact")
    );
    stop.send(()).unwrap();
    server.await.unwrap().unwrap();
    app.runtime.shutdown().await;
}

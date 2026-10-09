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

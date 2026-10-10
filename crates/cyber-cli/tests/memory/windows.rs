//! Actual Windows editor processes operating on retained private CLI drafts.
use super::*;

const SAVE: &str = "[System.IO.File]::WriteAllText($draft, [System.IO.File]::ReadAllText($source), [System.Text.UTF8Encoding]::new($false))";

impl Env {
    fn windows_editor(&self, script: &str, content: &str) -> Command {
        let path = self.root.join("editor script.ps1");
        std::fs::write(&path, format!("param([string]$source, [string]$draft)\n$ErrorActionPreference = 'Stop'\n{script}\n")).unwrap();
        let source = self.root.join("edited source.md");
        std::fs::write(&source, content).unwrap();
        let editor = format!(
            "pwsh.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -File {} {}",
            shell_words::quote(path.to_str().unwrap()),
            shell_words::quote(source.to_str().unwrap())
        );
        let mut command = self.command(&["memory", "edit", "coding-policy", "--global"]);
        command
            .env("EDITOR", editor)
            .env("MEMORY_PATH", self.note_path())
            .env("INDEX_PATH", self.data().join("memory/global/MEMORY.md"))
            .env("CONFIG_PATH", self.root.join("cyber/config/cyber.jsonc"));
        command
    }

    fn draft(&self) -> PathBuf {
        let drafts: Vec<_> = std::fs::read_dir(self.data())
            .unwrap()
            .map(Result::unwrap)
            .filter(|e| e.file_name().to_string_lossy().starts_with(".memory-edit-"))
            .collect();
        assert_eq!(drafts.len(), 1);
        drafts[0].path().join("note.md")
    }

    fn assert_no_note_commit(&self) {
        assert!(!self.note_path().exists());
        assert!(!self.data().join("memory/global/MEMORY.md").exists());
        assert!(
            !self
                .data()
                .join("memory/global/.memory-transaction")
                .exists()
        );
        self.no_database();
    }
}

#[test]
fn native_quoted_editor_save_show_index_and_delete() {
    let env = Env::new();
    let receipt = body(
        env.windows_editor(SAVE, &note("Durable naïve policy"))
            .output()
            .unwrap(),
    );
    assert_eq!(receipt["name"], "coding-policy");
    assert_eq!(receipt["deleted"], false);
    assert_eq!(
        body(env.run(&["memory", "show", "coding-policy", "--global"]))["body"],
        "Durable naïve policy"
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

#[test]
fn native_editor_private_atomic_replacement_commits() {
    let env = Env::new();
    let script = "\
$replacement = $draft + '.new'
[System.IO.File]::WriteAllText($replacement, [System.IO.File]::ReadAllText($source), [System.Text.UTF8Encoding]::new($false))
Get-Acl -LiteralPath $draft | Set-Acl -LiteralPath $replacement
[System.IO.File]::Replace($replacement, $draft, $draft + '.backup')";
    let receipt = body(
        env.windows_editor(script, &note("Atomic editor fact"))
            .output()
            .unwrap(),
    );
    assert_eq!(receipt["deleted"], false);
    assert_eq!(
        body(env.run(&["memory", "show", "coding-policy", "--global"]))["body"],
        "Atomic editor fact"
    );
    let backup = PathBuf::from(format!("{}.backup", env.draft().display()));
    assert!(
        std::fs::read_to_string(&backup)
            .unwrap()
            .contains("Replace with a durable fact")
    );
    cyber_core::memory::windows::verify_private(&std::fs::File::open(backup).unwrap()).unwrap();
    env.no_database();
}

#[test]
fn native_unchanged_editor_retains_private_placeholder_without_note() {
    let env = Env::new();
    let result = body(env.windows_editor("exit 0", "unused").output().unwrap());
    assert_eq!(result["changed"], false);
    let draft = env.draft();
    let parent =
        cap_std::fs::Dir::open_ambient_dir(draft.parent().unwrap(), cap_std::ambient_authority())
            .unwrap()
            .into_std_file();
    cyber_core::memory::windows::verify_private(&parent).unwrap();
    let file = std::fs::File::open(&draft).unwrap();
    cyber_core::memory::windows::verify_private(&file).unwrap();
    assert!(
        std::fs::read_to_string(draft)
            .unwrap()
            .contains("Replace with a durable fact")
    );
    env.assert_no_note_commit();
}

#[test]
fn native_invalid_content_and_editor_failure_retain_private_draft() {
    for (script, content) in [
        (SAVE.to_owned(), note("password = must-not-appear")),
        (SAVE.to_owned(), "bad YAML input".into()),
        (
            SAVE.to_owned(),
            note("fact").replace("coding-policy", "other-name"),
        ),
        (format!("{SAVE}\nexit 7"), note("editor failed fact")),
    ] {
        let env = Env::new();
        let output = env.windows_editor(&script, &content).output().unwrap();
        assert!(!output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!stderr.contains("must-not-appear"));
        assert!(stderr.contains("Editor draft retained"));
        let draft = env.draft();
        assert_eq!(std::fs::read_to_string(&draft).unwrap(), content);
        cyber_core::memory::windows::verify_private(&std::fs::File::open(draft).unwrap()).unwrap();
        env.assert_no_note_commit();
    }
}

#[test]
fn native_hard_link_and_inherited_editor_replacement_refuse_without_repairs() {
    for hard_link in [false, true] {
        let env = Env::new();
        let script = if hard_link {
            format!(
                "{SAVE}\nRemove-Item -LiteralPath $source\nNew-Item -ItemType HardLink -Path $source -Target $draft | Out-Null"
            )
        } else {
            "Remove-Item -LiteralPath $draft\nMove-Item -LiteralPath $source -Destination $draft"
                .into()
        };
        let output = env
            .windows_editor(&script, &note("User replacement"))
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("Editor draft retained"));
        let draft = env.draft();
        assert_eq!(
            std::fs::read_to_string(&draft).unwrap(),
            note("User replacement")
        );
        assert!(
            cyber_core::memory::windows::verify_private(&std::fs::File::open(draft).unwrap())
                .is_err()
        );
        env.assert_no_note_commit();
    }
}

#[test]
fn native_editor_external_target_and_index_changes_are_preserved() {
    for variable in ["MEMORY_PATH", "INDEX_PATH"] {
        let env = Env::new();
        body(
            env.windows_editor(SAVE, &note("Original fact"))
                .output()
                .unwrap(),
        );
        let script = format!(
            "{SAVE}\n[System.IO.File]::WriteAllText([System.Environment]::GetEnvironmentVariable('{variable}'), 'external user edit', [System.Text.UTF8Encoding]::new($false))"
        );
        let output = env
            .windows_editor(&script, &note("Updated fact"))
            .output()
            .unwrap();
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
        env.no_database();
    }
}

#[test]
fn native_local_recovery_refuses_aliased_database_without_file_effects() {
    let env = Env::new();
    let _memory = prepare_recovery(&env);
    let review = body(env.run(&["memory", "recovery", "--global"]));
    let fingerprint = review["fingerprint"].as_str().unwrap();
    let path = env.data().join("memory-test.db");
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch("CREATE TABLE memory_mutation(project_id TEXT, result TEXT)")
        .unwrap();
    drop(db);
    let alias = env.root.join("outside.db");
    std::fs::hard_link(&path, &alias).unwrap();
    let before = std::fs::read(&path).unwrap();
    let output = env.run(&["memory", "recover", "--review", fingerprint, "--global"]);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("cannot verify database admission ownership")
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert_eq!(std::fs::read(alias).unwrap(), before);
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

#[test]
fn native_editor_rechecks_revoked_generation_settings_before_commit() {
    let env = Env::new();
    let script = format!(
        "{SAVE}\n[System.IO.File]::WriteAllText($env:CONFIG_PATH, '{{\"memory\":{{\"generate\":false}}}}', [System.Text.UTF8Encoding]::new($false))"
    );
    let output = env.windows_editor(&script, &note("Fact")).output().unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Memory is read-only"));
    assert_eq!(std::fs::read_to_string(env.draft()).unwrap(), note("Fact"));
    env.assert_no_note_commit();
}

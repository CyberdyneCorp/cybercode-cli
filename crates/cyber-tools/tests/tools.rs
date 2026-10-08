//! Built-in tool behavior through the host, with no interactive approver.

mod support;

use cyber_server::runtime::ToolOutcome;
use serde_json::json;
use support::{Fixture, failed, ok};

#[tokio::test]
async fn read_pages_with_line_numbers_and_suggests_near_misses() {
    let f = Fixture::new();
    f.write("src/main.rs", "fn main() {}\nprintln!();\n");
    let out = ok(f
        .call("default", "read", json!({"path": "src/main.rs"}))
        .await);
    assert!(out.contains("1: fn main() {}"), "{out}");
    let missing = failed(
        f.call("default", "read", json!({"path": "src/mian.rs"}))
            .await,
    );
    assert!(missing.contains("main.rs"), "{missing}");
}

#[tokio::test]
async fn read_refuses_binary_files() {
    let f = Fixture::new();
    std::fs::write(f.repo.join("blob.bin"), [0u8, 1, 2, 0, 255]).unwrap();
    let out = failed(f.call("default", "read", json!({"path": "blob.bin"})).await);
    assert!(out.contains("Binary"), "{out}");
}

#[tokio::test]
async fn write_requires_a_prior_read_and_preserves_crlf() {
    let f = Fixture::new();
    f.set_config(json!({"permissions": {"edit": "allow"}}));
    f.write("a.txt", "one\r\ntwo\r\n");
    let refused = failed(
        f.call(
            "default",
            "write",
            json!({"path": "a.txt", "content": "x\n"}),
        )
        .await,
    );
    assert_eq!(refused, "Read the file before overwriting it.");
    ok(f.call("default", "read", json!({"path": "a.txt"})).await);
    ok(f.call(
        "default",
        "write",
        json!({"path": "a.txt", "content": "three\nfour\n"}),
    )
    .await);
    assert_eq!(f.read("a.txt"), "three\r\nfour\r\n");
}

#[tokio::test]
async fn edit_requires_a_unique_match_unless_replace_all() {
    let f = Fixture::new();
    f.set_config(json!({"permissions": {"edit": "allow"}}));
    f.write("a.txt", "x = 1\nx = 1\n");
    let ambiguous = failed(
        f.call(
            "default",
            "edit",
            json!({"path": "a.txt", "old_string": "x = 1", "new_string": "x = 2"}),
        )
        .await,
    );
    assert!(ambiguous.starts_with("Found 2 matches"), "{ambiguous}");
    let out = ok(f
        .call("default", "edit", json!({"path": "a.txt", "old_string": "x = 1", "new_string": "x = 2", "replace_all": true}))
        .await);
    assert!(out.contains("2 replacements"), "{out}");
    assert_eq!(f.read("a.txt"), "x = 2\nx = 2\n");
}

#[tokio::test]
async fn unattended_ask_denies_edits_in_default_mode() {
    let f = Fixture::new();
    f.write("a.txt", "a\n");
    let out = failed(
        f.call(
            "default",
            "edit",
            json!({"path": "a.txt", "old_string": "a", "new_string": "b"}),
        )
        .await,
    );
    assert!(out.contains("no interactive approver"), "{out}");
    assert_eq!(f.read("a.txt"), "a\n");
}

#[tokio::test]
async fn accept_edits_allows_edits_inside_the_location() {
    let f = Fixture::new();
    f.write("a.txt", "a\n");
    ok(f.call(
        "accept-edits",
        "edit",
        json!({"path": "a.txt", "old_string": "a", "new_string": "b"}),
    )
    .await);
    assert_eq!(f.read("a.txt"), "b\n");
}

#[tokio::test]
async fn protected_paths_ask_even_in_bypass_mode() {
    let f = Fixture::new();
    let out = failed(
        f.call(
            "bypass",
            "write",
            json!({"path": ".git/config", "content": "x"}),
        )
        .await,
    );
    assert!(out.contains("no interactive approver"), "{out}");
    assert!(!f.repo.join(".git/config").exists());
}

#[tokio::test]
async fn paths_outside_the_location_need_external_directory_approval() {
    let f = Fixture::new();
    let outside = f.repo.parent().unwrap().join("outside.txt");
    std::fs::write(&outside, "secret\n").unwrap();
    let out = failed(
        f.call(
            "default",
            "read",
            json!({"path": outside.display().to_string()}),
        )
        .await,
    );
    assert!(out.contains("no interactive approver"), "{out}");
}

#[cfg(unix)]
#[tokio::test]
async fn symlinks_cannot_hide_an_escape_from_the_location() {
    let f = Fixture::new();
    let outside = f.repo.parent().unwrap().join("elsewhere");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("s.txt"), "secret\n").unwrap();
    std::os::unix::fs::symlink(&outside, f.repo.join("link")).unwrap();
    let out = failed(
        f.call("default", "read", json!({"path": "link/s.txt"}))
            .await,
    );
    assert!(out.contains("no interactive approver"), "{out}");
}

#[tokio::test]
async fn deny_rules_win_over_bypass_and_hide_fully_denied_tools() {
    let f = Fixture::new();
    f.set_config(
        json!({"permissions": {"bash": {"*": "allow", "rm *": "deny"}, "webfetch": "deny"}}),
    );
    let out = failed(
        f.call("bypass", "bash", json!({"command": "rm -rf build"}))
            .await,
    );
    assert!(out.contains("denied"), "{out}");
    f.set_config(json!({"permissions": {"bash": "deny"}}));
    assert!(!f.tool_names("default", false).contains(&"bash".to_string()));
}

#[tokio::test]
async fn plan_mode_offers_read_only_tools_plus_plan_file_write_and_todo() {
    let f = Fixture::new();
    let names = f.tool_names("plan", false);
    for expected in ["read", "glob", "grep", "write", "todo", "plan_exit"] {
        assert!(
            names.contains(&expected.to_string()),
            "{expected} missing from {names:?}"
        );
    }
    for hidden in ["edit", "bash", "plan_enter"] {
        assert!(
            !names.contains(&hidden.to_string()),
            "{hidden} offered in plan mode"
        );
    }
    let default = f.tool_names("default", false);
    assert!(
        default.contains(&"plan_enter".to_string()) && !default.contains(&"plan_exit".to_string())
    );
}

#[tokio::test]
async fn plan_mode_writes_only_the_plan_file() {
    let f = Fixture::new();
    let denied = failed(
        f.call(
            "plan",
            "write",
            json!({"path": "src/new.rs", "content": "x"}),
        )
        .await,
    );
    assert!(denied.starts_with("Plan mode"), "{denied}");
    ok(f.call(
        "plan",
        "write",
        json!({"path": ".cyber/plans/ses_test.md", "content": "# Plan\n"}),
    )
    .await);
    assert_eq!(f.read(".cyber/plans/ses_test.md"), "# Plan\n");
}

#[tokio::test]
async fn apply_patch_is_offered_instead_of_edit_for_patch_models() {
    let f = Fixture::new();
    let names = f.tool_names("default", true);
    assert!(names.contains(&"apply_patch".to_string()) && !names.contains(&"edit".to_string()));
}

#[tokio::test]
async fn apply_patch_adds_updates_and_deletes_files() {
    let f = Fixture::new();
    f.write("keep.txt", "alpha\nbeta\ngamma\n");
    f.write("gone.txt", "bye\n");
    let patch = "*** Begin Patch\n*** Add File: new.txt\n+hello\n*** Update File: keep.txt\n@@\n alpha\n-beta\n+BETA\n gamma\n*** Delete File: gone.txt\n*** End Patch";
    ok(
        f.call("accept-edits", "apply_patch", json!({"patch": patch}))
            .await,
    );
    assert_eq!(f.read("new.txt"), "hello\n");
    assert_eq!(f.read("keep.txt"), "alpha\nBETA\ngamma\n");
    assert!(!f.repo.join("gone.txt").exists());
}

#[tokio::test]
async fn glob_and_grep_skip_the_git_directory() {
    let f = Fixture::new();
    f.write("src/a.rs", "needle here\n");
    f.write("src/b.py", "nothing\n");
    f.write(".git/HEAD", "needle\n");
    let globbed = ok(f
        .call("default", "glob", json!({"pattern": "**/*.rs"}))
        .await);
    assert!(
        globbed.contains("a.rs") && !globbed.contains("b.py"),
        "{globbed}"
    );
    let grepped = ok(f
        .call("default", "grep", json!({"pattern": "needle"}))
        .await);
    assert!(
        grepped.contains("src/a.rs") && !grepped.contains(".git"),
        "{grepped}"
    );
}

#[tokio::test]
async fn invalid_input_is_reported_with_a_json_pointer() {
    let f = Fixture::new();
    let out = failed(f.call("default", "read", json!({"path": 3})).await);
    assert!(out.starts_with("Invalid tool input: /path"), "{out}");
}

#[tokio::test]
async fn bash_reports_output_and_exit_code() {
    let f = Fixture::new();
    f.set_config(json!({"permissions": {"bash": "allow"}}));
    let out = ok(f
        .call("default", "bash", json!({"command": "echo hi; exit 3"}))
        .await);
    assert!(out.contains("hi") && out.contains("Exit code: 3"), "{out}");
}

#[tokio::test]
async fn bash_kills_the_process_group_on_timeout() {
    let f = Fixture::new();
    f.set_config(json!({"permissions": {"bash": "allow"}}));
    let started = std::time::Instant::now();
    let out = ok(f
        .call(
            "default",
            "bash",
            json!({"command": "sleep 30 & sleep 30", "timeout_ms": 300}),
        )
        .await);
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
    assert!(
        out.to_lowercase().contains("timed out") || out.contains("timeout"),
        "{out}"
    );
}

#[tokio::test]
async fn bash_stops_when_cancelled() {
    let f = Fixture::new();
    f.set_config(json!({"permissions": {"bash": "allow"}}));
    let cancel = tokio_util::sync::CancellationToken::new();
    let inv = f.invocation("default", "bash", json!({"command": "sleep 30"}));
    let trigger = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        trigger.cancel();
    });
    let started = std::time::Instant::now();
    let outcome = cyber_server::runtime::ToolHost::execute(&*f.host, inv, cancel).await;
    assert_eq!(outcome, ToolOutcome::Aborted);
    assert!(started.elapsed() < std::time::Duration::from_secs(3));
}

#[tokio::test]
async fn bash_writes_outside_the_location_need_external_approval() {
    let f = Fixture::new();
    f.set_config(json!({"permissions": {"bash": "allow"}}));
    let target = f.repo.parent().unwrap().join("x.txt");
    let out = failed(
        f.call(
            "default",
            "bash",
            json!({"command": format!("echo hi > {}", target.display())}),
        )
        .await,
    );
    assert!(out.contains("no interactive approver"), "{out}");
    assert!(!target.exists());
}

#[tokio::test]
async fn long_output_is_truncated_to_the_tail_with_an_overflow_file() {
    let f = Fixture::new();
    f.set_config(json!({"permissions": {"bash": "allow"}, "tool_output": {"max_lines": 5}}));
    let out = ok(f
        .call("default", "bash", json!({"command": "seq 1 100"}))
        .await);
    assert!(out.contains("100") && !out.contains("\n1\n"), "{out}");
    assert!(
        std::fs::read_dir(f.dir.path().join("tool-output"))
            .unwrap()
            .count()
            == 1
    );
}

#[tokio::test]
async fn todo_persists_and_allows_one_task_in_progress() {
    let f = Fixture::new();
    ok(f.call(
        "default",
        "todo",
        json!({"op": "create", "subject": "first"}),
    )
    .await);
    ok(f.call(
        "default",
        "todo",
        json!({"op": "create", "subject": "second", "blocked_by": ["t1"]}),
    )
    .await);
    let listed = ok(f
        .call(
            "default",
            "todo",
            json!({"op": "update", "id": "t1", "status": "in_progress"}),
        )
        .await);
    assert_eq!(listed, "[>] t1 first\n[blocked] t2 second");
    let second = failed(
        f.call(
            "default",
            "todo",
            json!({"op": "update", "id": "t2", "status": "in_progress"}),
        )
        .await,
    );
    assert!(second.starts_with("Only one task"), "{second}");
    ok(f.call(
        "default",
        "todo",
        json!({"op": "update", "id": "t1", "status": "completed"}),
    )
    .await);
    assert_eq!(
        ok(f.call("default", "todo", json!({"op": "list"})).await),
        "[x] t1 first\n[ ] t2 second"
    );
}

#[tokio::test]
async fn question_without_a_user_fails_clearly() {
    let f = Fixture::new();
    let q = json!({"questions": [{"question": "Which?", "header": "Pick", "options": [{"label": "a"}, {"label": "b"}]}]});
    let out = failed(f.call("default", "question", q).await);
    assert!(out.contains("No interactive user"), "{out}");
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn sandboxed_commands_get_a_private_writable_tmpdir() {
    let f = Fixture::new();
    f.set_config(json!({"permissions": {"bash": "allow"}}));
    let out = ok(f
        .call(
            "default",
            "bash",
            json!({"command": "echo $TMPDIR; mktemp"}),
        )
        .await);
    assert!(out.contains("tmp/ses_test"), "{out}");
    assert!(!out.contains("Exit code"), "{out}");
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn unknown_domains_are_denied_without_an_approver() {
    let f = Fixture::new();
    f.set_config(json!({"permissions": {"bash": "allow"}}));
    let base = support::serve(|_, _| (200, vec![], b"hello".to_vec())).await;
    let url = base.replace("127.0.0.1", "localhost");
    let out = ok(f
        .call(
            "default",
            "bash",
            json!({"command": format!("curl -s --max-time 5 {url}/")}),
        )
        .await);
    assert!(
        out.contains("Blocked by the Cyber sandbox: localhost"),
        "{out}"
    );
}

#[tokio::test]
async fn critical_bash_removals_are_refused_even_with_explicit_and_saved_allows() {
    let f = Fixture::new();
    f.set_config(json!({"permissions": {"bash": "allow", "external_directory": "allow"}}));
    f.write("keep.txt", "preserve me");
    cyber_tools::permissions::saved::save(&f.store, &f.repo, "bash", &["rm *".into()], "ses_test")
        .unwrap();
    let command = format!("rm -rf '{}'", f.repo.display());
    for mode in ["auto", "dont-ask", "bypass"] {
        let out = failed(f.call(mode, "bash", json!({"command": command})).await);
        assert!(
            out.starts_with("Refused: removal of critical path"),
            "{mode}: {out}"
        );
        assert_eq!(f.read("keep.txt"), "preserve me");
    }
}

#[tokio::test]
async fn accept_edits_does_not_auto_approve_dynamic_shell_paths() {
    let f = Fixture::new();
    let outside = f.dir.path().join("dynamic-created");
    let command = format!("DEST='{}'; touch \"$DEST\"", outside.display());
    let out = failed(
        f.call("accept-edits", "bash", json!({"command": command}))
            .await,
    );
    assert!(out.contains("no interactive approver"), "{out}");
    assert!(!outside.exists());
}

#[tokio::test]
async fn accept_edits_runs_proven_literal_filesystem_commands() {
    let f = Fixture::new();
    f.write("source file", "original");
    ok(f.call("accept-edits", "bash", json!({"command": "mkdir -p build && cp 'source file' build/copied && mv build/copied build/renamed && touch build/new && rm -- build/renamed"})).await);
    assert_eq!(f.read("source file"), "original");
    assert!(f.repo.join("build/new").exists());
    assert!(!f.repo.join("build/renamed").exists());
    f.set_config(json!({"permissions": {"bash": "deny"}}));
    let out = failed(
        f.call("accept-edits", "bash", json!({"command": "touch blocked"}))
            .await,
    );
    assert!(out.contains("denied"), "{out}");
    assert!(!f.repo.join("blocked").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn accept_edits_retains_protected_ceilings_through_filesystem_aliases() {
    let f = Fixture::new();
    f.write(".cyber/hooks.jsonc", "original");
    f.write("replacement", "replacement");
    std::os::unix::fs::symlink(f.repo.join(".cyber/hooks.jsonc"), f.repo.join("alias")).unwrap();
    std::fs::create_dir(f.repo.join("dest")).unwrap();
    std::os::unix::fs::symlink(
        f.repo.join(".cyber/hooks.jsonc"),
        f.repo.join("dest/replacement"),
    )
    .unwrap();
    for command in ["touch alias", "cp replacement dest"] {
        let out = failed(
            f.call("accept-edits", "bash", json!({"command": command}))
                .await,
        );
        assert!(out.contains("no interactive approver"), "{command}: {out}");
        assert_eq!(f.read(".cyber/hooks.jsonc"), "original");
    }
    let outside = f.dir.path().join("outside");
    std::fs::create_dir(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, f.repo.join("escape")).unwrap();
    let out = failed(
        f.call(
            "accept-edits",
            "bash",
            json!({"command": "touch escape/new"}),
        )
        .await,
    );
    assert!(out.contains("no interactive approver"), "{out}");
    assert!(!outside.join("new").exists());
}

#[tokio::test]
async fn accept_edits_copies_and_moves_inspected_directory_trees() {
    let f = Fixture::new();
    f.write("source/nested/data.txt", "original");
    ok(f.call(
        "accept-edits",
        "bash",
        json!({"command":"cp -R source copied"}),
    )
    .await);
    assert_eq!(f.read("copied/nested/data.txt"), "original");
    std::fs::create_dir(f.repo.join("destination")).unwrap();
    ok(f.call(
        "accept-edits",
        "bash",
        json!({"command":"mv copied destination"}),
    )
    .await);
    assert_eq!(f.read("destination/copied/nested/data.txt"), "original");
    assert!(!f.repo.join("copied").exists());
    assert_eq!(f.read("source/nested/data.txt"), "original");
    f.set_config(json!({"permissions":{"bash":"deny"}}));
    let out = failed(
        f.call(
            "accept-edits",
            "bash",
            json!({"command":"cp -R source denied"}),
        )
        .await,
    );
    assert!(out.contains("denied"), "{out}");
    assert!(!f.repo.join("denied").exists());
}

#[tokio::test]
async fn accept_edits_directory_operations_keep_protected_descendants_behind_approval() {
    let f = Fixture::new();
    f.write(".cyber/hooks.jsonc", "protected");
    f.write("payload/hooks.jsonc", "replacement");
    for command in [
        "mv .cyber moved",
        "cp -R payload .cyber/plugins",
        "cp -R .cyber copied",
    ] {
        let out = failed(
            f.call("accept-edits", "bash", json!({"command":command}))
                .await,
        );
        assert!(out.contains("no interactive approver"), "{command}: {out}");
        assert_eq!(f.read(".cyber/hooks.jsonc"), "protected");
    }
    assert!(!f.repo.join("moved").exists());
    assert!(!f.repo.join("copied").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn accept_edits_directory_destinations_preserve_protected_and_outside_alias_ceilings() {
    for protected in [true, false] {
        let f = Fixture::new();
        f.write("source/nested/data", "replacement");
        f.write(".cyber/hooks.jsonc", "protected");
        std::fs::create_dir_all(f.repo.join("destination/source/nested")).unwrap();
        let outside = f.dir.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        if protected {
            std::os::unix::fs::symlink(
                f.repo.join(".cyber/hooks.jsonc"),
                f.repo.join("destination/source/nested/data"),
            )
            .unwrap();
        } else {
            std::fs::remove_dir(f.repo.join("destination/source/nested")).unwrap();
            std::os::unix::fs::symlink(&outside, f.repo.join("destination/source/nested")).unwrap();
        }
        let out = failed(
            f.call(
                "accept-edits",
                "bash",
                json!({"command":"cp -R source destination"}),
            )
            .await,
        );
        assert!(out.contains("no interactive approver"), "{out}");
        assert_eq!(f.read(".cyber/hooks.jsonc"), "protected");
        assert!(!outside.join("data").exists());
        assert_eq!(f.read("source/nested/data"), "replacement");
    }
}

#[tokio::test]
async fn inline_critical_removals_cannot_be_lifted_by_rules_or_saved_approvals() {
    let f = Fixture::new();
    f.write("keep.txt", "preserve me");
    f.set_config(json!({"permissions": {"bash": "allow", "external_directory": "allow"}}));
    cyber_tools::permissions::saved::save(&f.store, &f.repo, "bash", &["*".into()], "ses_test")
        .unwrap();
    // The test workspace is nonempty. These nonrecursive APIs cannot remove it,
    // even if a regression accidentally reaches the interpreter.
    for command in [
        "python3 -c 'import os; os.rmdir(\".\")'",
        "node -e 'require(\"fs\").rmdirSync(\".\")'",
        "pwsh -NoProfile -Command '[IO.Directory]::Delete(\".\")'",
        "pwsh -NoProfile -EncodedCommand WwBJAE8ALgBEAGkAcgBlAGMAdABvAHIAeQBdADoAOgBEAGUAbABlAHQAZQAoACIALgAiACkA",
    ] {
        for mode in ["auto", "dont-ask", "bypass"] {
            let out = failed(f.call(mode, "bash", json!({"command": command})).await);
            assert!(
                out.starts_with("Refused: removal of critical path"),
                "{mode}: {out}"
            );
            assert_eq!(f.read("keep.txt"), "preserve me");
        }
        let out = failed(f.call("default", "bash", json!({"command": command})).await);
        assert!(out.contains("no interactive approver"), "{out}");
    }
}

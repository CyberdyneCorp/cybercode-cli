//! Built-in tool behavior through the host, with no interactive approver.

mod support;

use cyber_server::runtime::ToolOutcome;
use serde_json::json;
use support::{Fixture, failed, ok};

#[tokio::test]
async fn successful_read_warms_in_background_without_waiting_for_discovery() {
    use cyber_server::runtime::ToolHost;
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    };
    use std::time::Duration;
    let f = Fixture::new();
    f.write("file.rs", "fn main() {}\n");
    let (permit, receiver) = std::sync::mpsc::channel();
    let receiver = Mutex::new(receiver);
    let started = Arc::new(AtomicBool::new(false));
    let observed = started.clone();
    f.host
        .attach_lsp(Arc::new(move |directory| {
            observed.store(true, Ordering::Release);
            receiver
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(3))
                .unwrap();
            cyber_tools::lsp::Pool::new(
                directory,
                vec![],
                Arc::new(|_, _| Box::pin(async { unreachable!() })),
            )
        }))
        .unwrap();
    let result = tokio::time::timeout(
        Duration::from_millis(500),
        f.call("default", "read", json!({"path":"file.rs"})),
    )
    .await
    .unwrap();
    assert!(ok(result).contains("1: fn main() {}"));
    tokio::time::timeout(Duration::from_secs(1), async {
        while !started.load(Ordering::Acquire) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(20), f.host.shutdown())
            .await
            .is_err()
    );
    permit.send(()).unwrap();
    f.host.shutdown().await;
    f.host.shutdown().await;
    assert!(f.host.lsp_status(&f.repo).unwrap().is_empty());
}

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

#[cfg(unix)]
#[tokio::test]
async fn accept_edits_single_file_aliases_keep_ordinary_approval() {
    for command in [
        "cp replacement alias",
        "cp replacement destination",
        "touch dangling",
    ] {
        let f = Fixture::with_policy(
            "bash",
            cyber_sandbox::find_helper(),
            Some("full-access".into()),
        );
        f.write(".cyber/hooks.jsonc", "protected");
        f.write("replacement", "replacement");
        std::fs::hard_link(f.repo.join(".cyber/hooks.jsonc"), f.repo.join("alias")).unwrap();
        std::fs::create_dir(f.repo.join("destination")).unwrap();
        std::fs::hard_link(
            f.repo.join(".cyber/hooks.jsonc"),
            f.repo.join("destination/replacement"),
        )
        .unwrap();
        std::os::unix::fs::symlink(f.repo.join(".cyber/mcp.json"), f.repo.join("dangling"))
            .unwrap();
        let out = failed(
            f.call("accept-edits", "bash", json!({"command":command}))
                .await,
        );
        assert!(out.contains("no interactive approver"), "{command}: {out}");
        assert_eq!(f.read(".cyber/hooks.jsonc"), "protected");
        assert!(!f.repo.join(".cyber/mcp.json").exists());
    }
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

/// `sandbox` → Named sandbox profiles: a profile that extends `workspace` with
/// `deny_read` hides `key.pem` from a sandboxed `cat` (the OS refuses it) and from `read`,
/// `glob` and `grep`.
#[cfg(any(target_os = "macos", target_os = "linux"))]
#[tokio::test]
async fn profile_deny_read_blocks_cat_and_hides_glob_match() {
    let f = Fixture::new();
    f.set_config(json!({
        "permissions": {"bash": "allow"},
        "sandbox": {"profile": "ci", "profiles": {
            "ci": {"extends": "workspace", "deny_read": ["**/*.pem", "~/.aws/**"]}
        }}
    }));
    f.write("key.pem", "PRIVATE-KEY-MATERIAL\n");
    f.write("notes.txt", "public notes\n");
    let catted = ok(f
        .call(
            "default",
            "bash",
            json!({"command": "cat key.pem; cat notes.txt"}),
        )
        .await);
    assert!(!catted.contains("PRIVATE-KEY-MATERIAL"), "{catted}");
    assert!(catted.contains("public notes"), "{catted}");
    #[cfg(target_os = "macos")]
    assert!(
        catted.contains(&format!(
            "Sandbox denied: read {}",
            f.repo.join("key.pem").display()
        )),
        "{catted}"
    );
    let globbed = ok(f.call("default", "glob", json!({"pattern": "*"})).await);
    assert!(
        globbed.contains("notes.txt") && !globbed.contains("key.pem"),
        "{globbed}"
    );
    let grepped = ok(f
        .call("default", "grep", json!({"pattern": "PRIVATE"}))
        .await);
    assert!(!grepped.contains("key.pem"), "{grepped}");
    let read = failed(f.call("default", "read", json!({"path": "key.pem"})).await);
    assert!(read.starts_with("Sandbox denied: read"), "{read}");
    assert!(!read.contains("PRIVATE"), "{read}");
}

/// Regression (`sandbox` → Named sandbox profiles): `grep` and `glob` whose `path` names
/// a `deny_read` file directly are refused as `read` is; the walk never filters its root.
#[tokio::test]
async fn grep_and_glob_refuse_a_hidden_base_path() {
    let f = Fixture::new();
    f.set_config(json!({
        "sandbox": {"profile": "ci", "profiles": {
            "ci": {"extends": "workspace", "deny_read": ["**/*.pem"]}
        }}
    }));
    f.write("key.pem", "PRIVATE-KEY-MATERIAL\n");
    let grepped = failed(
        f.call(
            "default",
            "grep",
            json!({"pattern": "PRIVATE", "path": "key.pem"}),
        )
        .await,
    );
    assert!(grepped.starts_with("Sandbox denied: read"), "{grepped}");
    assert!(!grepped.contains("PRIVATE-KEY"), "{grepped}");
    let globbed = failed(
        f.call(
            "default",
            "glob",
            json!({"pattern": "*", "path": "key.pem"}),
        )
        .await,
    );
    assert!(globbed.starts_with("Sandbox denied: read"), "{globbed}");
}

/// `sandbox` → Environment policy: `mode: none` keeps only the base variables, `set`,
/// the `CYBER_*` markers and (with the network off) nothing else.
#[cfg(any(target_os = "macos", target_os = "linux"))]
#[tokio::test]
async fn sandbox_env_none_exposes_only_base_vars_set_and_markers() {
    let f = Fixture::new();
    f.set_config(json!({
        "permissions": {"bash": "allow"},
        "sandbox": {"network": "off", "env": {"mode": "none", "set": {"CI": "1"}}}
    }));
    let out = ok(f.call("default", "bash", json!({"command": "env"})).await);
    let shell_set = ["PWD", "SHLVL", "_", "OLDPWD"];
    let names: Vec<&str> = out
        .lines()
        .filter_map(|line| line.split_once('=').map(|(name, _)| name))
        .filter(|name| !shell_set.contains(name))
        .collect();
    assert!(out.lines().any(|line| line == "CI=1"), "{out}");
    for name in &names {
        assert!(
            cyber_sandbox::BASE_VARS.contains(name) || *name == "CI" || name.starts_with("CYBER"),
            "unexpected {name} in {out}"
        );
    }
    assert!(
        names.contains(&"PATH") && names.contains(&"TMPDIR"),
        "{out}"
    );
}

/// `sandbox` → Escalation requests, Sandbox events and audit: a write outside the
/// writable roots says `Sandbox denied: write <path>` and is stored as `sandbox.denied.1`.
#[cfg(any(target_os = "macos", target_os = "linux"))]
#[tokio::test]
async fn sandbox_write_denial_is_explicit_and_recorded_as_event() {
    use support::flow::{Flow, call, text};
    let target = format!("/usr/cyber-sandbox-denied-{}", std::process::id());
    let flow = Flow::new(
        vec![
            call(
                "c1",
                "bash",
                json!({"command": format!("echo x > {target}")}),
            ),
            text("done"),
        ],
        false,
    );
    flow.f
        .set_config(json!({"permissions": {"bash": "allow", "external_directory": "allow"}}));
    let id = flow.session("default").await;
    flow.prompt(&id, "write").await;
    flow.settle(&id).await;
    let output = flow.output(&id, "c1").await;
    assert!(
        output.contains(&format!("Sandbox denied: write {target}")),
        "{output}"
    );
    assert!(output.contains("sandbox: \"escalate\""), "{output}");
    assert!(!std::path::Path::new(&target).exists());
    let events = flow.f.store.read_events(&id, -1, 500).unwrap().events;
    let denied: Vec<_> = events
        .iter()
        .filter(|e| e.kind == "sandbox.denied.1")
        .collect();
    assert_eq!(denied.len(), 1, "{events:?}");
    assert_eq!(denied[0].data["operation"], "write");
    assert_eq!(denied[0].data["target"], target.as_str());
    assert_eq!(denied[0].data["call_id"], "c1");
    assert_eq!(denied[0].data["session_id"], id.as_str());
}

/// `sandbox` → Excluded commands: a matching command runs outside the sandbox, and an
/// allow rule cannot lift its `ask` floor outside `bypass`.
#[cfg(any(target_os = "macos", target_os = "linux"))]
#[tokio::test]
async fn excluded_command_runs_unsandboxed_and_asks_even_with_allow_rule() {
    let f = Fixture::new();
    let secret = f.dir.path().join("home/.ssh/id_fixture");
    std::fs::create_dir_all(secret.parent().unwrap()).unwrap();
    std::fs::write(&secret, "SECRET-IDENTITY\n").unwrap();
    let command = format!("cat {}", secret.display());
    f.set_config(json!({"permissions": {"bash": "allow"}}));
    let sandboxed = ok(f.call("bypass", "bash", json!({"command": command})).await);
    assert!(!sandboxed.contains("SECRET-IDENTITY"), "{sandboxed}");
    f.set_config(json!({
        "permissions": {"bash": "allow"},
        "sandbox": {"excluded_commands": ["cat *"]}
    }));
    for mode in ["default", "accept-edits", "dont-ask"] {
        let out = failed(f.call(mode, "bash", json!({"command": command})).await);
        assert!(
            out.contains("no interactive approver") || out.contains("Not pre-approved"),
            "{mode}: {out}"
        );
    }
    let unsandboxed = ok(f.call("bypass", "bash", json!({"command": command})).await);
    assert!(unsandboxed.contains("SECRET-IDENTITY"), "{unsandboxed}");
    let listed = ok(f
        .call(
            "bypass",
            "bash",
            json!({"command": format!("{command} | head -1")}),
        )
        .await);
    assert!(
        !listed.contains("SECRET-IDENTITY"),
        "a pipeline never qualifies: {listed}"
    );
}

/// `permissions-modes` → Critical-path removal guard: the scenario command is refused in
/// `bypass` before anything runs.
#[tokio::test]
async fn bypass_refuses_rm_rf_of_git_toplevel_substitution_without_execution() {
    let f = Fixture::new();
    f.set_config(json!({"permissions": {"bash": "allow", "external_directory": "allow"}}));
    f.write("keep.txt", "preserve me");
    for command in [
        "touch ran-before; rm -rf \"$(git rev-parse --show-toplevel)\"",
        "touch ran-before; nice -n 5 rm -rf \"$(git rev-parse --show-toplevel)\"",
        "touch ran-before; find \"$(git rev-parse --show-toplevel)\" -exec rm -rf {} +",
        "touch ran-before; git rev-parse --show-toplevel | xargs rm -rf",
    ] {
        let out = failed(f.call("bypass", "bash", json!({"command": command})).await);
        assert!(out.starts_with("Refused: removal"), "{command}: {out}");
        assert!(
            out.ends_with("Rewrite the command to target specific files."),
            "{out}"
        );
        assert_eq!(f.read("keep.txt"), "preserve me");
        assert!(
            !f.repo.join("ran-before").exists(),
            "nothing ran: {command}"
        );
    }
}

/// `permissions-modes` → accept-edits mode: `write` inside the Location runs; `curl`,
/// `webfetch` and a recursive `rm` still ask (and fail without an approver).
#[tokio::test]
async fn accept_edits_allows_write_but_keeps_non_edit_and_recursive_rm_asks() {
    let f = Fixture::new();
    ok(f.call(
        "accept-edits",
        "write",
        json!({"path": "src/new.rs", "content": "fn a() {}\n"}),
    )
    .await);
    assert_eq!(f.read("src/new.rs"), "fn a() {}\n");
    f.write("build/out.o", "object");
    for (tool, input) in [
        ("bash", json!({"command": "curl https://example.org"})),
        ("webfetch", json!({"url": "https://example.org"})),
        ("bash", json!({"command": "rm -r build"})),
    ] {
        let out = failed(f.call("accept-edits", tool, input.clone()).await);
        assert!(
            out.contains("no interactive approver"),
            "{tool} {input}: {out}"
        );
    }
    assert_eq!(f.read("build/out.o"), "object");
}

#[tokio::test]
async fn private_computer_capture_root_refuses_other_session_builtin_reads() {
    use cyber_computer::{
        Error,
        backend::{Backend, Capture, Health, NativeTarget, Selector},
        control::InputPermit,
    };
    use std::sync::Arc;
    struct CaptureFixture;
    impl Backend for CaptureFixture {
        fn health(&self, _: bool) -> Result<Health, Error> {
            Ok(Health {
                supported: true,
                platform: "fixture".into(),
                minimum_version: "fixture".into(),
                screen_capture: true,
                accessibility: true,
                binary: "fixture".into(),
            })
        }
        fn resolve(&self, _: &Selector) -> Result<NativeTarget, Error> {
            Ok(NativeTarget {
                identity: "app:fixture:42:1:7".into(),
                app: Some("fixture.app".into()),
                pid: 42,
                window: 7,
                display: 1,
                launch_ms: 1,
                x: 0.,
                y: 0.,
                width: 1.,
                height: 1.,
                image_width: 1,
                image_height: 1,
                rotation: 0,
            })
        }
        fn capture(&self, target: &NativeTarget) -> Result<Capture, Error> {
            let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
            png.extend(1u32.to_be_bytes());
            png.extend(1u32.to_be_bytes());
            Ok(Capture {
                target: target.clone(),
                png,
            })
        }
        fn input(&self, _: &NativeTarget, _: &InputPermit) -> Result<(), Error> {
            panic!("privacy fixture must not inject input")
        }
    }
    let f = Fixture::new();
    let home = f.dir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let paths = cyber_core::paths::Paths::resolve(
        &std::collections::HashMap::from([(
            "CYBER_HOME".into(),
            f.dir.path().join("capture-install").display().to_string(),
        )]),
        &home,
    );
    paths.ensure().unwrap();
    cyber_tools::plugin_host::hide_computer_artifacts(&paths, &home).unwrap();
    let root = paths.data.join("computer");
    let computer = cyber_server::runtime::computer::Computer::new(
        f.store.clone(),
        root.clone(),
        Arc::new(|| cyber_server::runtime::computer::ComputerPolicy {
            enabled: true,
            retention_days: 1,
        }),
        Arc::new(|_| true),
        Arc::new(CaptureFixture),
    )
    .unwrap();
    let preview = computer
        .preview(
            "ses_capture_owner",
            Selector {
                app: Some("fixture.app".into()),
                display: None,
            },
        )
        .unwrap();
    computer
        .allow("ses_capture_owner", &preview.id, None, false)
        .unwrap();
    let frame = computer
        .screenshot("ses_capture_owner", "fixture-owned-capture")
        .unwrap()
        .frame
        .unwrap();
    assert!(
        !computer
            .frame_bytes("ses_capture_owner", &frame.id)
            .unwrap()
            .is_empty()
    );
    assert!(computer.frame_bytes("ses_test", &frame.id).is_err());
    let directory = std::fs::read_dir(&root)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let canary = directory.join("private-canary.txt");
    std::fs::write(&canary, "CROSS_SESSION_CAPTURE_CANARY").unwrap();
    f.set_config(json!({"permissions":{"read":"allow","external_directory":"allow"},"sandbox":{"readable_paths":[root]}}));
    let denied = failed(f.call("default", "read", json!({"path":canary})).await);
    assert!(denied.starts_with("Sandbox denied: read"), "{denied}");
    assert!(!denied.contains("CROSS_SESSION_CAPTURE_CANARY"));
    f.write("public.txt", "PUBLIC_CONTROL");
    assert!(
        ok(f.call("default", "read", json!({"path":"public.txt"}))
            .await)
        .contains("PUBLIC_CONTROL")
    );
    assert!(
        !computer
            .frame_bytes("ses_capture_owner", &frame.id)
            .unwrap()
            .is_empty(),
        "owner API remains readable after sandbox registration"
    );
}

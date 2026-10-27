use super::*;
use cyber_core::autofix::{Config, Stage, Subscription, Trigger, TriggerKind};

fn subscription(directory: &Path) -> Subscription {
    Subscription::new(
        "afx_test".into(),
        directory.to_string_lossy().into_owned(),
        Config {
            repository: "owner/repo".into(),
            pr: 1,
            session_id: "ses_owner".into(),
            model: "test/model".into(),
            checks: vec![vec!["true".into()]],
            required_ci: vec!["check:1:test".into()],
            verification_paths: vec!["tests".into()],
            max_attempts: 3,
            max_tokens: 10000,
            max_seconds: 60,
        },
        1,
        "fix".into(),
        chrono::Utc::now().timestamp_millis(),
    )
    .unwrap()
}
#[cfg(unix)]
fn fixture() -> (tempfile::TempDir, Execution, PathBuf, String) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("attempts");
    let work = root.join("one");
    std::fs::create_dir_all(&work).unwrap();
    command(&work, &["init", "-q", "--initial-branch=fix"]);
    std::fs::create_dir(work.join("tests")).unwrap();
    std::fs::write(work.join("tests/check.txt"), "pinned\n").unwrap();
    std::fs::write(work.join("code.txt"), "old\n").unwrap();
    command(&work, &["add", "."]);
    command(&work, &["commit", "-qm", "base"]);
    let head = command(&work, &["rev-parse", "HEAD"]);
    let home = temp.path().join("home");
    std::fs::create_dir(&home).unwrap();
    let root = root.canonicalize().unwrap();
    let home = home.canonicalize().unwrap();
    let work = work.canonicalize().unwrap();
    (
        temp,
        Execution {
            root,
            home,
            source_override: None,
            private_roots: vec![],
        },
        work,
        head,
    )
}
#[cfg(unix)]
fn command(root: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "init.templateDir=",
            "-c",
            "commit.gpgSign=false",
        ])
        .args(args)
        .current_dir(root)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "Fixture")
        .env("GIT_AUTHOR_EMAIL", "fixture@localhost")
        .env("GIT_COMMITTER_NAME", "Fixture")
        .env("GIT_COMMITTER_EMAIL", "fixture@localhost")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "fixture Git failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().into()
}
fn claimed(mut s: Subscription, head: &str) -> Subscription {
    s.enqueue(
        vec![Trigger {
            key: "check:1".into(),
            head: head.into(),
            kind: TriggerKind::CheckFailure,
            text: "failure".into(),
        }],
        head,
    )
    .unwrap();
    s.claim("one".into(), head, chrono::Utc::now().timestamp_millis())
        .unwrap();
    s
}
#[cfg(unix)]
fn intent(mut s: Subscription, head: &str, commit: &str) -> Subscription {
    s = claimed(s, head);
    s.running("one", "ses_one".into()).unwrap();
    s.verifying("one", Some(100), None).unwrap();
    s.push_intent(
        "one",
        commit.into(),
        vec![CheckEvidence {
            argv: s.config.checks[0].clone(),
            passed: true,
            output_digest: "a".repeat(64),
        }],
    )
    .unwrap();
    s
}
#[test]
fn policy_protects_case_aliases_and_verification_sources() {
    for path in [
        ".Git/config",
        ".github/workflows/ci.yml",
        "dir/AGENTS.md",
        ".gitattributes",
        ".gitmodules",
        "../outside",
        "a\\b",
        "bad\nfile",
    ] {
        assert!(protected(path), "{path}");
    }
    assert!(!protected("src/lib.rs"));
    let s = subscription(Path::new("/fixture"));
    assert!(validate_patch_paths(&s, &["tests/check.txt".into()]).is_err());
    assert!(validate_patch_paths(&s, &["Tests/check.txt".into()]).is_err());
    assert!(validate_patch_paths(&s, &["src/lib.rs".into()]).is_ok());
}
#[test]
fn git_configuration_and_environment_do_not_inherit_credentials_or_hooks() {
    let args = git_arguments(&["status"]);
    assert!(
        args.windows(2)
            .any(|p| p == ["-c", "http.followRedirects=false"])
    );
    assert!(
        args.windows(2)
            .any(|p| p == ["-c", "protocol.ext.allow=never"])
    );
    assert!(
        args.windows(2)
            .any(|p| p == ["-c", "core.hooksPath=/dev/null"])
    );
    let env = owner_environment(Path::new("/fixture"));
    assert!(!env.contains_key("GIT_CONFIG_COUNT"));
    let env = verification_environment(Path::new("/fixture/tmp"));
    assert!(!env.contains_key("GH_CONFIG_DIR"));
    assert!(!env.contains_key("GITHUB_TOKEN"));
}
#[test]
fn verification_time_uses_the_original_attempt_clock() {
    let mut s = claimed(subscription(Path::new("/fixture")), &"1".repeat(40));
    assert_eq!(s.active().unwrap().stage, Stage::Claimed);
    s.attempts[0].started_ms = chrono::Utc::now().timestamp_millis() - 61_000;
    assert!(verification_deadline(&s).is_err());
}
#[tokio::test]
#[cfg(unix)]
async fn commit_rejects_changed_pinned_verification_source() {
    let (_temp, executor, work, head) = fixture();
    let s = subscription(&work);
    std::fs::write(work.join("tests/check.txt"), "bypass\n").unwrap();
    assert!(
        executor
            .commit(&s, "one", &head)
            .await
            .unwrap_err()
            .contains("protected")
    );
    assert_eq!(command(&work, &["rev-parse", "HEAD"]), head);
}
#[tokio::test]
#[cfg(unix)]
async fn commit_has_exact_parent_and_refuses_mutated_head() {
    let (_temp, executor, work, head) = fixture();
    let s = subscription(&work);
    std::fs::write(work.join("code.txt"), "fix\n").unwrap();
    let commit = executor.commit(&s, "one", &head).await.unwrap();
    assert_eq!(
        command(&work, &["show", "--no-patch", "--format=%P", &commit]),
        head
    );
    assert!(
        executor
            .commit(&s, "one", &head)
            .await
            .unwrap_err()
            .contains("base changed")
    );
}
#[tokio::test]
#[cfg(unix)]
async fn owner_git_refuses_project_local_filter_execution() {
    let (temp, executor, work, head) = fixture();
    let s = subscription(&work);
    let marker = temp.path().join("filter-ran");
    command(
        &work,
        &[
            "config",
            "filter.sneaky.clean",
            &format!("touch {}", marker.display()),
        ],
    );
    std::fs::write(work.join(".gitattributes"), "code.txt filter=sneaky\n").unwrap();
    std::fs::write(work.join("code.txt"), "fix\n").unwrap();
    assert!(
        executor
            .commit(&s, "one", &head)
            .await
            .unwrap_err()
            .contains("drivers")
    );
    assert!(!marker.exists());
}
#[tokio::test]
#[cfg(unix)]
async fn push_is_non_force_and_refuses_a_human_head_update() {
    let (temp, executor, work, head) = fixture();
    let remote = temp.path().join("remote.git");
    command(temp.path(), &["init", "--bare", remote.to_str().unwrap()]);
    command(
        &work,
        &["push", remote.to_str().unwrap(), "HEAD:refs/heads/fix"],
    );
    std::fs::write(work.join("code.txt"), "fix\n").unwrap();
    let commit = executor
        .commit(&subscription(&work), "one", &head)
        .await
        .unwrap();
    let s = intent(subscription(&work), &head, &commit);
    assert_eq!(s.active().unwrap().stage, Stage::PushIntent);
    // A separate actor advances the remote to an unrelated child of the old head.
    let human = temp.path().join("human");
    command(
        temp.path(),
        &["clone", remote.to_str().unwrap(), human.to_str().unwrap()],
    );
    command(&human, &["checkout", "fix"]);
    std::fs::write(human.join("human.txt"), "human\n").unwrap();
    command(&human, &["add", "."]);
    command(&human, &["commit", "-qm", "human"]);
    command(&human, &["push", "origin", "fix"]);
    let human_head = command(&human, &["rev-parse", "HEAD"]);
    let result = executor
        .push_to(&s, &work, &commit, remote.to_str().unwrap(), true)
        .await;
    assert!(result.unwrap_err().contains("remote head changed"));
    assert_eq!(
        command(&remote, &["rev-parse", "refs/heads/fix"]),
        human_head
    );
}
#[tokio::test]
#[cfg(unix)]
async fn push_confirms_exact_intended_commit_and_clean_source() {
    let (temp, executor, work, head) = fixture();
    let remote = temp.path().join("remote.git");
    command(temp.path(), &["init", "--bare", remote.to_str().unwrap()]);
    command(
        &work,
        &["push", remote.to_str().unwrap(), "HEAD:refs/heads/fix"],
    );
    std::fs::write(work.join("code.txt"), "fix\n").unwrap();
    let commit = executor
        .commit(&subscription(&work), "one", &head)
        .await
        .unwrap();
    let s = intent(subscription(&work), &head, &commit);
    std::fs::write(work.join("code.txt"), "changed after test\n").unwrap();
    assert!(
        executor
            .push_to(&s, &work, &commit, remote.to_str().unwrap(), true)
            .await
            .unwrap_err()
            .contains("modified")
    );
    command(&work, &["restore", "code.txt"]);
    executor
        .push_to(&s, &work, &commit, remote.to_str().unwrap(), true)
        .await
        .unwrap();
    assert_eq!(command(&remote, &["rev-parse", "refs/heads/fix"]), commit);
}
#[test]
#[cfg(unix)]
fn isolation_hides_registered_credentials_and_sibling_attempts() {
    let (_temp, executor, work, _head) = fixture();
    let secret = executor.home.join("custom-secret");
    std::fs::write(&secret, "test credential").unwrap();
    cyber_sandbox::hide_credential_file(&executor.home, secret.clone());
    let sibling = executor.root.join("other");
    std::fs::create_dir(&sibling).unwrap();
    let launch = executor
        .launch(&std::fs::canonicalize(&work).unwrap())
        .unwrap();
    assert!(launch.unreadable.contains(&secret));
    assert!(launch.unreadable.contains(&sibling));
    assert_eq!(launch.config.network, cyber_sandbox::NetworkMode::Off);
    assert_eq!(launch.read_only, vec![work.join(".git")]);
}
#[test]
#[cfg(unix)]
fn verification_sources_must_exist_and_cannot_escape_via_links() {
    use std::os::unix::fs::symlink;
    let (temp, _executor, work, _head) = fixture();
    let s = subscription(&work);
    assert!(validate_verification_paths(&s, &work).is_ok());
    std::fs::remove_dir_all(work.join("tests")).unwrap();
    assert!(validate_verification_paths(&s, &work).is_err());
    symlink(temp.path(), work.join("tests")).unwrap();
    assert!(validate_verification_paths(&s, &work).is_err());
}

#[test]
#[cfg(unix)]
fn verification_files_and_renameable_ancestors_are_read_only() {
    let (_temp, executor, work, _head) = fixture();
    let mut subscription = subscription(&work);
    subscription.config.verification_paths = vec!["tests/check.txt".into()];
    let mut launch = executor.launch(&work).unwrap();
    executor
        .pin_verification_sources(&mut launch, &subscription, &work)
        .unwrap();
    assert!(launch.read_only.contains(&work.join("tests/check.txt")));
    assert!(launch.read_only.contains(&work.join("tests")));
    assert!(!launch.read_only.contains(&work));
}

#[tokio::test]
#[cfg(unix)]
#[ignore = "requires native OS sandbox acceptance; run explicitly on a sandbox-capable host"]
async fn native_verifier_cannot_temporarily_rewrite_or_rename_pinned_sources() {
    let (_temp, executor, work, head) = fixture();
    executor.supported().unwrap();
    let mut subscription = claimed(subscription(&work), &head);
    // Pin a file rather than its directory, so the ancestor-rename protection
    // is exercised independently of a caller explicitly pinning the directory.
    subscription.config.verification_paths = vec!["tests/check.txt".into()];
    subscription.config.checks = vec![vec![
        "/bin/sh".into(),
        "-c".into(),
        "if printf weakened > tests/check.txt; then printf 'pinned\\n' > tests/check.txt; exit 1; fi; if mv tests moved-tests; then printf weakened > moved-tests/check.txt; printf 'pinned\\n' > moved-tests/check.txt; mv moved-tests tests; exit 1; fi; test \"$(cat tests/check.txt)\" = pinned; printf build-output > artifact; rm artifact".into(),
    ]];
    let evidence = executor.checks(&subscription, "one").await.unwrap();
    assert_eq!(evidence.len(), 1);
    assert!(
        evidence[0].passed,
        "pinned verification sources were writable"
    );
    assert_eq!(
        std::fs::read_to_string(work.join("tests/check.txt")).unwrap(),
        "pinned\n"
    );
    assert!(!work.join("moved-tests").exists());
}

#[tokio::test]
#[cfg(unix)]
async fn ignored_model_source_cannot_make_an_uncommitted_patch_pass_checks() {
    let (_temp, executor, work, head) = fixture();
    std::fs::write(work.join(".gitignore"), "fake_source.rs\n").unwrap();
    std::fs::write(work.join("fake_source.rs"), "pretend fixed code\n").unwrap();
    std::fs::write(work.join("code.txt"), "fix\n").unwrap();
    assert!(
        executor
            .commit(&subscription(&work), "one", &head)
            .await
            .unwrap_err()
            .contains("ignored files")
    );
}

#[tokio::test]
#[cfg(unix)]
async fn bounded_capture_terminates_background_descendants_on_timeout() {
    let temp = tempfile::tempdir().unwrap();
    let marker = temp.path().join("escaped");
    let script = format!("(sleep 1; printf escaped > '{}') & wait", marker.display());
    let process = Process::spawn("/bin/sh", &["-c".into(), script], None, |command| {
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
    })
    .await
    .unwrap();
    let result = capture(
        process,
        tokio::time::Instant::now() + Duration::from_millis(50),
    )
    .await;
    assert!(result.unwrap_err().contains("time limit"));
    tokio::time::sleep(Duration::from_millis(1200)).await;
    assert!(
        !marker.exists(),
        "a timed-out verifier leaked its background process"
    );
}

#[tokio::test]
#[cfg(unix)]
#[ignore = "requires native OS sandbox acceptance; run explicitly on a sandbox-capable host"]
async fn native_verifier_cannot_read_credentials_or_write_git_or_outside_checkout() {
    let (temp, executor, work, head) = fixture();
    executor.supported().unwrap();
    let secret = executor.home.join("private-token");
    std::fs::write(&secret, "fixture-only-credential").unwrap();
    cyber_sandbox::hide_credential_file(&executor.home, secret.clone());
    let outside = temp.path().join("outside");
    let script = format!(
        "test ! -r '{}' && ! printf forbidden >> .git/config && ! printf forbidden > '{}'",
        secret.display(),
        outside.display()
    );
    let mut s = subscription(&work);
    s.config.checks = vec![vec!["/bin/sh".into(), "-c".into(), script]];
    std::fs::write(work.join("code.txt"), "fixed\n").unwrap();
    executor.commit(&s, "one", &head).await.unwrap();
    s = claimed(s, &head);
    let evidence = executor.checks(&s, "one").await.unwrap();
    assert_eq!(evidence.len(), 1);
    assert!(evidence[0].passed);
    assert!(!outside.exists());
    assert_eq!(
        std::fs::read_to_string(secret).unwrap(),
        "fixture-only-credential"
    );
}

#[test]
#[cfg(target_os = "macos")]
fn mac_verification_explicitly_denies_owner_keychain_brokers() {
    let wrapped = cyber_sandbox::Wrapped {
        program: "/usr/bin/sandbox-exec".into(),
        args: vec![
            "-p".into(),
            "(version 1)\n(allow mach-lookup (global-name \"com.apple.SecurityServer\"))".into(),
            "--".into(),
            "/bin/true".into(),
        ],
        env: Default::default(),
    };
    let wrapped = tighten_credentials(wrapped).unwrap();
    let profile = &wrapped.args[1];
    assert!(profile.contains("(deny mach-lookup (global-name \"com.apple.SecurityServer\")"));
    assert!(profile.contains("com.apple.securityd.xpc"));
    assert!(
        profile.rfind("deny mach-lookup").unwrap() > profile.find("allow mach-lookup").unwrap()
    );
    assert!(
        tighten_credentials(cyber_sandbox::Wrapped {
            program: "/bin/sh".into(),
            args: vec![],
            env: Default::default()
        })
        .is_err()
    );
}

#[tokio::test]
async fn oversized_process_output_is_rejected_instead_of_truncated_to_success() {
    let output = vec![b'x'; LIMIT as usize + 1];
    assert!(
        limited(output.as_slice())
            .await
            .unwrap_err()
            .contains("exceeded")
    );
}

#[tokio::test]
#[cfg(unix)]
#[ignore = "requires native sandbox and installed Rust toolchain; run explicitly without network"]
async fn native_rust_verification_runs_offline_with_pinned_tests_and_scoped_scratch() {
    let (_temp, mut executor, work, _head) = fixture();
    executor.home = PathBuf::from(std::env::var_os("HOME").expect("test host home"));
    executor.supported().unwrap();
    std::fs::create_dir(work.join("src")).unwrap();
    std::fs::write(
        work.join("Cargo.toml"),
        "[package]\nname = \"autofix-fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::write(work.join("Cargo.lock"),"# This file is automatically @generated by Cargo.\n# It is not intended for manual editing.\nversion = 4\n\n[[package]]\nname = \"autofix-fixture\"\nversion = \"0.1.0\"\n").unwrap();
    std::fs::write(work.join(".gitignore"), "/target/\n").unwrap();
    std::fs::write(work.join("src/lib.rs"), "pub fn answer() -> u32 { 41 }\n").unwrap();
    std::fs::write(
        work.join("tests/answer.rs"),
        "#[test]\nfn expected_answer() { assert_eq!(autofix_fixture::answer(), 42); }\n",
    )
    .unwrap();
    command(&work, &["add", "."]);
    command(&work, &["commit", "-qm", "Rust fixture"]);
    let head = command(&work, &["rev-parse", "HEAD"]);
    let mut s = subscription(&work);
    s.config.checks = vec![vec![
        "cargo".into(),
        "test".into(),
        "--offline".into(),
        "--locked".into(),
    ]];
    s.config.verification_paths = vec!["Cargo.toml".into(), "Cargo.lock".into(), "tests".into()];
    s.config.max_seconds = 120;
    s = claimed(s, &head);
    let baseline = executor.diagnose(&s, "one").await.unwrap();
    assert_eq!(baseline.len(), 1);
    assert!(baseline[0].contains("expected_answer"), "{}", baseline[0]);
    assert!(baseline[0].len() <= 8192);
    assert!(
        !work.join("target").exists(),
        "diagnosis contaminated the model checkout"
    );
    std::fs::write(work.join("src/lib.rs"), "pub fn answer() -> u32 { 42 }\n").unwrap();
    executor.commit(&s, "one", &head).await.unwrap();
    let evidence = executor.checks(&s, "one").await.unwrap();
    assert_eq!(evidence.len(), 1);
    assert!(
        evidence[0].passed,
        "offline Rust verification failed; inspect controlled local fixture without exposing credential data"
    );
    assert!(!work.read_dir().unwrap().any(|e| {
        e.unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".autofix-verification-")
    }));
}

#[tokio::test]
#[cfg(unix)]
async fn publication_refuses_an_empty_commit_even_with_a_forged_check_receipt() {
    let (temp, executor, work, head) = fixture();
    let remote = temp.path().join("remote.git");
    command(temp.path(), &["init", "--bare", remote.to_str().unwrap()]);
    command(
        &work,
        &["push", remote.to_str().unwrap(), "HEAD:refs/heads/fix"],
    );
    command(&work, &["commit", "--allow-empty", "-qm", "empty"]);
    let commit = command(&work, &["rev-parse", "HEAD"]);
    let s = intent(subscription(&work), &head, &commit);
    assert!(
        executor
            .push_to(&s, &work, &commit, remote.to_str().unwrap(), true)
            .await
            .unwrap_err()
            .contains("nonempty")
    );
    assert_eq!(command(&remote, &["rev-parse", "refs/heads/fix"]), head);
}

#[test]
fn diagnostic_feedback_redacts_credentials_controls_and_private_checkout_paths() {
    let output = format!(
        "{}\nGITHUB_TOKEN=ghp_abcdefghijklmnopqrstuvxyz1234567890\n/fixture/private/path\x1b[31m\n",
        "x".repeat(9000)
    );
    let feedback = failure_feedback(
        0,
        output.as_bytes(),
        b"test failed",
        Path::new("/fixture/private"),
    );
    assert!(feedback.len() <= 8192);
    assert!(!feedback.contains("ghp_abcdefghijklmnopqrstuvxyz1234567890"));
    assert!(!feedback.contains('\x1b'));
    assert!(!feedback.contains("/fixture/private"));
    assert!(feedback.contains("test failed"));
}

#[test]
#[cfg(unix)]
fn verification_program_preserves_proxy_argv_zero_after_checking_canonical_target() {
    use std::os::unix::fs::symlink;
    let temp = tempfile::tempdir().unwrap();
    let proxy = temp.path().join("cargo");
    symlink("/bin/sh", &proxy).unwrap();
    let resolved =
        verification_program(proxy.to_str().unwrap(), Path::new("/unrelated-checkout")).unwrap();
    assert_eq!(resolved, proxy.to_string_lossy());
    assert!(resolved.ends_with("cargo"));
}

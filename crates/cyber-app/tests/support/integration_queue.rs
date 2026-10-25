//! Real App/server, local Git and host checks. No account, live model or GitHub mutation.
use super::*;

const CHECK_ARGV: &[&str] = &["awk", "{sum += $1} END {exit sum>1}", "a.txt", "b.txt"];
fn git_text(root: &std::path::Path, args: &[&str]) -> String {
    let result = std::process::Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .unwrap();
    assert!(result.status.success(), "git {args:?}");
    String::from_utf8(result.stdout).unwrap().trim().into()
}
fn repository() -> (tempfile::TempDir, String, String) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    git(&root, &["init", "--quiet", "-b", "main"]);
    git(&root, &["config", "user.name", "Integration fixture"]);
    git(&root, &["config", "user.email", "fixture@example.invalid"]);
    std::fs::write(
        root.join(".gitignore"),
        "/data/\n/config/\n/state/\n/cache/\n/tmp/\n/home/\n/out/\n",
    )
    .unwrap();
    for name in ["a.txt", "b.txt"] {
        std::fs::write(root.join(name), "0\n").unwrap();
    }
    git(&root, &["add", "."]);
    git(&root, &["commit", "--quiet", "-m", "base"]);
    git(&root, &["checkout", "--quiet", "-b", "api"]);
    std::fs::write(root.join("a.txt"), "1\n").unwrap();
    git(&root, &["commit", "--quiet", "-am", "api contribution"]);
    let api = git_text(&root, &["rev-parse", "HEAD"]);
    assert!(
        std::process::Command::new(CHECK_ARGV[0])
            .args(&CHECK_ARGV[1..])
            .current_dir(&root)
            .status()
            .unwrap()
            .success()
    );
    git(&root, &["checkout", "--quiet", "main"]);
    git(&root, &["checkout", "--quiet", "-b", "ui"]);
    std::fs::write(root.join("b.txt"), "1\n").unwrap();
    git(&root, &["commit", "--quiet", "-am", "ui contribution"]);
    let ui = git_text(&root, &["rev-parse", "HEAD"]);
    assert!(
        std::process::Command::new(CHECK_ARGV[0])
            .args(&CHECK_ARGV[1..])
            .current_dir(&root)
            .status()
            .unwrap()
            .success()
    );
    git(&root, &["checkout", "--quiet", "main"]);
    (temp, api, ui)
}
fn reply(body: &Value) -> Value {
    let schema = body.to_string();
    if schema.contains("return_result") {
        return json!({"tool_calls":[{"index":0,"id":"integration_result","type":"function","function":{"name":"return_result","arguments":json!({"recommended":true,"rationale":"Combined host checks passed; review completed in a separate read-only Session.","limitations":[]}).to_string()}}]});
    }
    json!({"content":"review unavailable"})
}
async fn open(temp: tempfile::TempDir, model: &ScriptedModel) -> Server {
    let p = paths(&temp.path().canonicalize().unwrap());
    p.ensure().unwrap();
    let mut config = model.config();
    config["permissions"] = json!({"bash":"allow","edit":"allow","external_directory":"allow","read":"allow","worktree":"allow"});
    std::fs::write(p.config.join("cyber.json"), config.to_string()).unwrap();
    Server::open(temp).await
}
fn manifest(commits: &[(&str, &str)]) -> Value {
    json!({"target_branch":"main","contributions":commits.iter().map(|(label,commit)|json!({"label":label,"commit":commit})).collect::<Vec<_>>(),"checks":[{"argv":CHECK_ARGV,"timeout_seconds":30}],"reviewer_model":"test/main"})
}
async fn submit(server: &Server, owner: &str, manifest: Value) -> Value {
    server
        .ok(
            Method::POST,
            "/workflows/integrations",
            Some(json!({"session_id":owner,"manifest":manifest})),
        )
        .await["data"]
        .clone()
}
async fn run(server: &Server, batch: &Value) -> Value {
    server
        .ok(
            Method::POST,
            &format!(
                "/workflows/integrations/{}/run",
                batch["id"].as_str().unwrap()
            ),
            Some(json!({"expected_revision":batch["revision"]})),
        )
        .await["data"]
        .clone()
}
async fn settled(server: &Server, batch: &Value) -> Value {
    let id = batch["id"].as_str().unwrap();
    for _ in 0..300 {
        let batch = server
            .ok(Method::GET, &format!("/workflows/integrations/{id}"), None)
            .await["data"]
            .clone();
        if !matches!(
            batch["status"].as_str(),
            Some("queued" | "preparing" | "checking" | "reviewing")
        ) {
            return batch;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let current = server
        .ok(Method::GET, &format!("/workflows/integrations/{id}"), None)
        .await;
    let run = current["data"]["run_id"].as_str().unwrap();
    let detail = server
        .ok(Method::GET, &format!("/workflows/runs/{run}"), None)
        .await;
    panic!("integration {id} did not settle: {current} run {detail}")
}

#[tokio::test]
async fn contributions_passing_alone_fail_when_the_combination_breaks_a_host_check() {
    let model = ScriptedModel::start(reply);
    let (temp, api, ui) = repository();
    let root = temp.path().canonicalize().unwrap();
    let server = open(temp, &model).await;
    let owner = server.session().await;
    let batch = submit(&server, &owner, manifest(&[("api", &api), ("ui", &ui)])).await;
    assert_eq!(batch["status"], "queued");
    assert!(model.requests().is_empty());
    run(&server, &batch).await;
    let report = settled(&server, &batch).await;
    assert_eq!(report["status"], "failed", "{report}");
    let host = &report["report"]["host"];
    assert_eq!(host["eligible"], false, "{report}");
    assert_eq!(host["checks"][0]["outcome"], "failed", "{report}");
    assert_eq!(host["checks"][0]["settled"], true);
    assert!(
        model.requests().is_empty(),
        "failed source was not sent to a reviewer"
    );
    assert_eq!(std::fs::read_to_string(root.join("a.txt")).unwrap(), "0\n");
    assert_eq!(std::fs::read_to_string(root.join("b.txt")).unwrap(), "0\n");
    server.close().await;
}

#[tokio::test]
async fn a_verified_combination_has_host_evidence_and_an_independent_plan_reviewer() {
    let model = ScriptedModel::start(reply);
    let (temp, api, _) = repository();
    let server = open(temp, &model).await;
    let owner = server.session().await;
    let batch = submit(&server, &owner, manifest(&[("api", &api)])).await;
    run(&server, &batch).await;
    let report = settled(&server, &batch).await;
    assert_eq!(report["status"], "ready", "{report}");
    assert_eq!(
        report["report"]["host"]["checks"][0]["outcome"], "passed",
        "{report}"
    );
    assert_eq!(
        report["report"]["workflow"]["review_status"], "reviewed",
        "{report}"
    );
    for evidence in [&report["report"]["host"], &report["report"]["workflow"]] {
        assert!(
            evidence["cost"]["actual_priced_usd"].is_number(),
            "{evidence}"
        );
        assert!(evidence["cost"]["estimated_usd"].is_null(), "{evidence}");
        assert!(evidence["cost"]["cost_complete"].is_boolean(), "{evidence}");
    }
    let run = report["run_id"].as_str().unwrap();
    let details = server
        .ok(Method::GET, &format!("/workflows/runs/{run}"), None)
        .await;
    let reviewer = details["data"]["agents_detail"][0]["session_id"]
        .as_str()
        .unwrap();
    let reviewer = server
        .ok(Method::GET, &format!("/sessions/{reviewer}"), None)
        .await;
    assert_eq!(reviewer["data"]["mode"], "plan");
    assert_ne!(reviewer["data"]["id"], owner);
    assert!(!model.requests().is_empty());
    server.close().await;
}

#[tokio::test]
async fn self_modifying_checks_never_certify_the_new_source() {
    let model = ScriptedModel::start(reply);
    let (temp, api, _) = repository();
    let server = open(temp, &model).await;
    let owner = server.session().await;
    let mut input = manifest(&[("api", &api)]);
    input["checks"] = json!([{"argv":["sh","-c","printf '2\\n' > a.txt"]}]);
    let batch = submit(&server, &owner, input).await;
    run(&server, &batch).await;
    let report = settled(&server, &batch).await;
    assert_ne!(report["status"], "ready", "{report}");
    assert_eq!(report["report"]["host"]["eligible"], false, "{report}");
    assert_eq!(
        report["report"]["host"]["checks"][0]["source_unchanged"], false,
        "{report}"
    );
    assert_eq!(
        report["report"]["host"]["checks"][0]["outcome"], "failed",
        "{report}"
    );
    assert!(model.requests().is_empty());
    server.close().await;
}

#[tokio::test]
async fn two_concurrent_runs_cannot_claim_one_revision_and_cancel_fences_stale_actions() {
    let model = ScriptedModel::start(reply);
    let (temp, api, _) = repository();
    let server = open(temp, &model).await;
    let owner = server.session().await;
    let batch = submit(&server, &owner, manifest(&[("api", &api)])).await;
    let path = format!(
        "/workflows/integrations/{}/run",
        batch["id"].as_str().unwrap()
    );
    let body = Some(json!({"expected_revision":batch["revision"]}));
    let (a, b) = tokio::join!(
        server.call(Method::POST, &path, body.clone()),
        server.call(Method::POST, &path, body)
    );
    assert_eq!(
        [a.0, b.0]
            .into_iter()
            .filter(|status| status.is_success())
            .count(),
        1,
        "{a:?} {b:?}"
    );
    let report = settled(&server, &batch).await;
    let path = format!(
        "/workflows/integrations/{}/cancel",
        batch["id"].as_str().unwrap()
    );
    let stale = server
        .call(Method::POST, &path, Some(json!({"expected_revision":1})))
        .await;
    assert_eq!(stale.0, StatusCode::CONFLICT, "{stale:?}");
    let cancelled = server
        .ok(
            Method::POST,
            &path,
            Some(json!({"expected_revision":report["revision"]})),
        )
        .await;
    assert_eq!(cancelled["data"]["status"], "cancelled");
    server.close().await;
}

#[tokio::test]
async fn protected_config_contributions_and_cycles_are_rejected_without_native_runs() {
    let model = ScriptedModel::start(reply);
    let (temp, _, _) = repository();
    let root = temp.path().canonicalize().unwrap();
    git(&root, &["checkout", "--quiet", "-b", "policy-change"]);
    std::fs::write(
        root.join("cyber.json"),
        "{\"permissions\":{\"bash\":\"allow\"}}",
    )
    .unwrap();
    git(&root, &["add", "cyber.json"]);
    git(
        &root,
        &["commit", "--quiet", "-m", "untrusted configuration"],
    );
    let commit = git_text(&root, &["rev-parse", "HEAD"]);
    git(&root, &["checkout", "--quiet", "main"]);
    let server = open(temp, &model).await;
    let owner = server.session().await;
    let (status, body) = server
        .call(
            Method::POST,
            "/workflows/integrations",
            Some(json!({"session_id":owner,"manifest":manifest(&[("policy",&commit)])})),
        )
        .await;
    assert!(!status.is_success(), "{body}");
    let runs = server.ok(Method::GET, "/workflows/runs", None).await;
    assert_eq!(runs["data"], json!([]));
    assert!(model.requests().is_empty());
    server.close().await;
}

#[tokio::test]
async fn an_advanced_target_cannot_silently_replace_the_submitted_base() {
    let model = ScriptedModel::start(reply);
    let (temp, api, _) = repository();
    let root = temp.path().canonicalize().unwrap();
    let server = open(temp, &model).await;
    let owner = server.session().await;
    let batch = submit(&server, &owner, manifest(&[("api", &api)])).await;
    std::fs::write(root.join("new.txt"), "target advanced").unwrap();
    git(&root, &["add", "new.txt"]);
    git(&root, &["commit", "--quiet", "-m", "advance target"]);
    let (status, _) = server
        .call(
            Method::POST,
            &format!(
                "/workflows/integrations/{}/run",
                batch["id"].as_str().unwrap()
            ),
            Some(json!({"expected_revision":batch["revision"]})),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(model.requests().is_empty());
    server.close().await;
}

#[tokio::test]
async fn textual_conflicts_are_settled_without_running_checks_or_changing_the_owner() {
    let model = ScriptedModel::start(reply);
    let (temp, api, _) = repository();
    let root = temp.path().canonicalize().unwrap();
    git(&root, &["checkout", "--quiet", "-b", "conflicting"]);
    std::fs::write(root.join("a.txt"), "2\n").unwrap();
    git(
        &root,
        &["commit", "--quiet", "-am", "conflicting contribution"],
    );
    let conflict = git_text(&root, &["rev-parse", "HEAD"]);
    git(&root, &["checkout", "--quiet", "main"]);
    let server = open(temp, &model).await;
    let owner = server.session().await;
    let batch = submit(
        &server,
        &owner,
        manifest(&[("api", &api), ("conflict", &conflict)]),
    )
    .await;
    run(&server, &batch).await;
    let report = settled(&server, &batch).await;
    assert_eq!(report["status"], "conflicted", "{report}");
    assert!(model.requests().is_empty());
    assert_eq!(std::fs::read_to_string(root.join("a.txt")).unwrap(), "0\n");
    server.close().await;
}

#[tokio::test]
async fn hub_references_fail_closed_without_an_account_or_issuer() {
    let model = ScriptedModel::start(reply);
    let (temp, api, _) = repository();
    let server = open(temp, &model).await;
    let owner = server.session().await;
    let mut manifest = manifest(&[("api", &api)]);
    manifest["contributions"][0]["hub"] =
        json!({"project":"prj_unavailable","agent":"agt_unavailable"});
    let (status, _) = server
        .call(
            Method::POST,
            "/workflows/integrations",
            Some(json!({"session_id":owner,"manifest":manifest})),
        )
        .await;
    assert!(!status.is_success());
    assert!(model.requests().is_empty());
    server.close().await;
}

#[tokio::test]
async fn unavailable_reviewer_is_disclosed_and_never_silently_replaced() {
    let model = ScriptedModel::start(reply);
    let (temp, api, _) = repository();
    let server = open(temp, &model).await;
    let owner = server.session().await;
    let mut manifest = manifest(&[("api", &api)]);
    manifest["reviewer_model"] = "missing/model".into();
    let batch = submit(&server, &owner, manifest).await;
    run(&server, &batch).await;
    let report = settled(&server, &batch).await;
    assert_eq!(report["status"], "ready", "{report}");
    assert_eq!(
        report["report"]["workflow"]["review_status"],
        "review_unavailable"
    );
    assert!(model.requests().is_empty());
    server.close().await;
}

#[tokio::test]
async fn completed_evidence_survives_restart_and_external_mutation_makes_it_stale() {
    let model = ScriptedModel::start(reply);
    let (temp, api, _) = repository();
    let server = open(temp, &model).await;
    let owner = server.session().await;
    let batch = submit(&server, &owner, manifest(&[("api", &api)])).await;
    run(&server, &batch).await;
    let report = settled(&server, &batch).await;
    assert_eq!(report["status"], "ready", "{report}");
    let count = model.requests().len();
    let server = server.restart().await;
    let report = settled(&server, &batch).await;
    assert_eq!(report["status"], "ready", "{report}");
    assert_eq!(model.requests().len(), count);
    let checkout = report["report"]["host"]["worktree"].as_str().unwrap();
    std::fs::write(std::path::Path::new(checkout).join("a.txt"), "2\n").unwrap();
    let report = settled(&server, &batch).await;
    assert_eq!(report["status"], "stale", "{report}");
    assert_eq!(report["report"]["host"]["eligible"], false);
    server.close().await;
}

#[tokio::test]
async fn repository_hooks_never_run_and_external_filter_configuration_is_refused() {
    use std::os::unix::fs::PermissionsExt;
    let model = ScriptedModel::start(reply);
    let (temp, api, _) = repository();
    let root = temp.path().canonicalize().unwrap();
    let hook = root.join(".git/hooks/post-checkout");
    std::fs::write(
        &hook,
        format!("#!/bin/sh\ntouch '{}'\n", root.join("hook-ran").display()),
    )
    .unwrap();
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o700)).unwrap();
    let server = open(temp, &model).await;
    let owner = server.session().await;
    let batch = submit(&server, &owner, manifest(&[("api", &api)])).await;
    run(&server, &batch).await;
    let report = settled(&server, &batch).await;
    assert_eq!(report["status"], "ready", "{report}");
    assert!(!root.join("hook-ran").exists());
    git(&root, &["config", "filter.evil.smudge", "touch filter-ran"]);
    let (status, _) = server
        .call(
            Method::POST,
            "/workflows/integrations",
            Some(json!({"session_id":owner,"manifest":manifest(&[("api",&api)])})),
        )
        .await;
    assert!(!status.is_success());
    assert!(!root.join("filter-ran").exists());
    git(&root, &["config", "--unset", "filter.evil.smudge"]);
    git(&root, &["config", "remote.origin.promisor", "true"]);
    let (status, _) = server
        .call(
            Method::POST,
            "/workflows/integrations",
            Some(json!({"session_id":owner,"manifest":manifest(&[("api",&api)])})),
        )
        .await;
    assert!(
        !status.is_success(),
        "partial clones require an explicit object fetch before admission"
    );
    server.close().await;
}

#[tokio::test]
async fn submission_replay_survives_a_lost_http_receipt_without_duplicate_batches() {
    let model = ScriptedModel::start(reply);
    let (temp, api, _) = repository();
    let server = open(temp, &model).await;
    let owner = server.session().await;
    let body = json!({"session_id":owner,"manifest":manifest(&[("api",&api)])});
    let send = |body: Value| {
        server
            .client
            .post(format!("{}/workflows/integrations", server.url))
            .basic_auth("cyber", Some(PASSWORD))
            .header("idempotency-key", "integration-lost-receipt")
            .json(&body)
            .send()
    };
    let first = send(body.clone()).await.unwrap();
    assert_eq!(first.status(), StatusCode::CREATED);
    let first: Value = first.json().await.unwrap();
    // Crash/retention can remove the HTTP response cache after the batch committed.
    server
        .app
        .store
        .transaction(|db| {
            db.execute(
                "DELETE FROM idempotency_key WHERE key='integration-lost-receipt'",
                [],
            )?;
            Ok(())
        })
        .unwrap();
    let second = send(body.clone()).await.unwrap();
    assert_eq!(second.status(), StatusCode::CREATED);
    let second: Value = second.json().await.unwrap();
    assert_eq!(first["data"]["id"], second["data"]["id"]);
    server
        .app
        .store
        .transaction(|db| {
            db.execute(
                "DELETE FROM idempotency_key WHERE key='integration-lost-receipt'",
                [],
            )?;
            Ok(())
        })
        .unwrap();
    let mut changed = body;
    changed["manifest"]["checks"][0]["argv"] = json!(["false"]);
    assert_eq!(send(changed).await.unwrap().status(), StatusCode::CONFLICT);
    assert!(
        !server.app.store.has_failed(),
        "rejected replay cannot stop the writer"
    );
    let batches = server
        .ok(Method::GET, "/workflows/integrations", None)
        .await;
    assert_eq!(batches["data"].as_array().unwrap().len(), 1);
    server.close().await;
}

#[tokio::test]
async fn exhausted_wall_budget_never_becomes_a_ready_integration() {
    let model = ScriptedModel::start(reply);
    let (temp, api, _) = repository();
    let server = open(temp, &model).await;
    let owner = server.session().await;
    let batch=server.ok(Method::POST,"/workflows/integrations",Some(json!({"session_id":owner,"manifest":manifest(&[("api",&api)]),"budget":{"max_wall_seconds":0.001}}))).await["data"].clone();
    run(&server, &batch).await;
    let report = settled(&server, &batch).await;
    assert_ne!(report["status"], "ready", "{report}");
    let id = report["run_id"].as_str().unwrap();
    let details = server
        .ok(Method::GET, &format!("/workflows/runs/{id}"), None)
        .await;
    assert_eq!(details["data"]["status"], "budget_exceeded", "{details}");
    assert!(model.requests().is_empty());
    assert!(!server.app.store.has_failed());
    server.close().await;
}

#[tokio::test]
async fn worktree_deny_and_plan_mode_block_integration_git_effects() {
    for plan in [false, true] {
        let model = ScriptedModel::start(reply);
        let (temp, api, _) = repository();
        let root = temp.path().canonicalize().unwrap();
        let p = paths(&root);
        p.ensure().unwrap();
        let mut config = model.config();
        config["permissions"] = if plan {
            json!({"bash":"allow","edit":"allow"})
        } else {
            json!({"bash":"allow","worktree":"deny"})
        };
        std::fs::write(p.config.join("cyber.json"), config.to_string()).unwrap();
        let server = Server::open(temp).await;
        let owner = server
            .ok(
                Method::POST,
                "/sessions",
                Some(json!({"title":"Denied integration","mode":if plan{"plan"}else{"default"}})),
            )
            .await["data"]["id"]
            .as_str()
            .unwrap()
            .to_string();
        let batch = submit(&server, &owner, manifest(&[("api", &api)])).await;
        run(&server, &batch).await;
        let report = settled(&server, &batch).await;
        assert_ne!(report["status"], "ready", "{report}");
        assert!(
            !git_text(&root, &["branch", "--list", "cyber/integrations/*"])
                .contains("integration-")
        );
        assert!(model.requests().is_empty());
        assert!(!server.app.store.has_failed());
        server.close().await;
    }
}

#[tokio::test]
async fn unrelated_history_and_claimed_verification_are_rejected_before_native_effects() {
    let model = ScriptedModel::start(reply);
    let (temp, api, _) = repository();
    let root = temp.path().canonicalize().unwrap();
    git(&root, &["checkout", "--quiet", "--orphan", "unrelated"]);
    git(&root, &["rm", "--quiet", "-r", "--cached", "."]);
    std::fs::write(root.join("unrelated.txt"), "unrelated").unwrap();
    git(&root, &["add", "unrelated.txt"]);
    git(&root, &["commit", "--quiet", "-m", "unrelated history"]);
    let unrelated = git_text(&root, &["rev-parse", "HEAD"]);
    for path in [".gitignore", "a.txt", "b.txt"] {
        std::fs::remove_file(root.join(path)).unwrap();
    }
    git(&root, &["checkout", "--quiet", "main"]);
    let server = open(temp, &model).await;
    let owner = server.session().await;
    let (status, _) = server
        .call(
            Method::POST,
            "/workflows/integrations",
            Some(json!({"session_id":owner,"manifest":manifest(&[("unrelated",&unrelated)])})),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let mut claimed = manifest(&[("api", &api)]);
    claimed["verified"] = true.into();
    let (status, _) = server
        .call(
            Method::POST,
            "/workflows/integrations",
            Some(json!({"session_id":owner,"manifest":claimed})),
        )
        .await;
    assert!(status.is_client_error());
    assert!(
        server
            .ok(Method::GET, "/workflows/integrations", None)
            .await["data"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(
        !git_text(&root, &["branch", "--list", "cyber/integrations/*"]).contains("integration-")
    );
    assert!(model.requests().is_empty());
    server.close().await;
}

#[tokio::test]
async fn a_replaced_native_temporary_root_cannot_redirect_host_artifact_writes() {
    use std::os::unix::fs::symlink;
    let model = ScriptedModel::start(reply);
    let (temp, api, _) = repository();
    let root = temp.path().canonicalize().unwrap();
    let server = open(temp, &model).await;
    let owner = server.session().await;
    let batch = submit(&server, &owner, manifest(&[("api", &api)])).await;
    run(&server, &batch).await;
    let report = settled(&server, &batch).await;
    assert_eq!(report["status"], "ready", "{report}");
    let run = report["run_id"].as_str().unwrap();
    let temporary = root.join("tmp/sessions").join(format!("integration-{run}"));
    let retained = temporary.with_extension("retained");
    let escape = root.join("tmp/escape");
    std::fs::create_dir_all(&escape).unwrap();
    std::fs::rename(&temporary, &retained).unwrap();
    symlink(&escape, &temporary).unwrap();
    let report = settled(&server, &batch).await;
    assert_eq!(report["status"], "unknown", "{report}");
    assert_eq!(report["report"]["host"]["eligible"], false);
    assert_eq!(std::fs::read_dir(&escape).unwrap().count(), 0);
    std::fs::remove_file(&temporary).unwrap();
    std::fs::rename(&retained, &temporary).unwrap();
    assert_eq!(settled(&server, &batch).await["status"], "ready");
    server.close().await;
}

#[tokio::test]
async fn project_configured_path_cannot_replace_native_git_authority() {
    use std::os::unix::fs::PermissionsExt;
    let model = ScriptedModel::start(reply);
    let (temp, api, _) = repository();
    let root = temp.path().canonicalize().unwrap();
    let server = open(temp, &model).await;
    let bin = root.join("tmp/contributed-bin");
    std::fs::create_dir_all(&bin).unwrap();
    let replacement = bin.join("git");
    let marker = bin.join("native-git-replaced");
    std::fs::write(
        &replacement,
        format!(
            "#!/bin/sh\nprintf replaced > '{}'\nexit 73\n",
            marker.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&replacement, std::fs::Permissions::from_mode(0o700)).unwrap();
    let config = root.join("config/cyber.json");
    let mut settings: Value = serde_json::from_slice(&std::fs::read(&config).unwrap()).unwrap();
    settings["sandbox"] = json!({"env":{"set":{"PATH":format!("{}:/usr/bin:/bin",bin.display())}}});
    std::fs::write(config, settings.to_string()).unwrap();
    let owner = server.session().await;
    let batch = submit(&server, &owner, manifest(&[("api", &api)])).await;
    run(&server, &batch).await;
    let report = settled(&server, &batch).await;
    assert_eq!(report["status"], "ready", "{report}");
    assert!(
        !marker.exists(),
        "native Git resolved a project-configured executable"
    );
    server.close().await;
}

#[tokio::test]
async fn a_late_publication_receipt_cannot_attach_to_a_replaced_attempt() {
    let model = ScriptedModel::start(reply);
    let (temp, api, _) = repository();
    let server = open(temp, &model).await;
    let owner = server.session().await;
    let batch = submit(&server, &owner, manifest(&[("api", &api)])).await;
    run(&server, &batch).await;
    let ready = settled(&server, &batch).await;
    assert_eq!(ready["status"], "ready");
    run(&server, &ready).await;
    let result = server.app.runtime.integration_publication(
        ready["id"].as_str().unwrap(),
        ready["revision"].as_u64().unwrap(),
        ready["run_id"].as_str().unwrap(),
        json!({"phase":"published","old_attempt":true}),
    );
    assert!(result.is_err());
    let current = settled(&server, &ready).await;
    assert_eq!(current["status"], "ready", "{current}");
    assert!(current["publication"].is_null());
    assert_ne!(ready["run_id"], current["run_id"]);
    server.close().await;
}

#[tokio::test]
async fn a_failed_second_start_cannot_recover_the_previous_successful_run() {
    let model = ScriptedModel::start(reply);
    let (temp, api, _) = repository();
    let server = open(temp, &model).await;
    let owner = server.session().await;
    let batch = submit(&server, &owner, manifest(&[("api", &api)])).await;
    run(&server, &batch).await;
    let ready = settled(&server, &batch).await;
    assert_eq!(ready["status"], "ready");
    let id = ready["id"].as_str().unwrap();
    let old_run = ready["run_id"].as_str().unwrap();
    let claimed = server
        .app
        .runtime
        .integration_claim(
            id,
            ready["revision"].as_u64().unwrap(),
            ready["args"].clone(),
        )
        .unwrap();
    assert_eq!(claimed.attempt, 2);
    assert!(server.app.runtime.integration_bind(id, old_run).is_err());
    let failed = server
        .app
        .runtime
        .integration_start_failed(id, "injected start failure".into())
        .unwrap();
    assert_eq!(failed.status, "unknown");
    assert!(failed.run_id.is_none());
    assert_eq!(failed.previous_runs, vec![old_run.to_string()]);
    assert_eq!(failed.report.as_ref().unwrap()["eligible"], false);
    server.close().await;
}

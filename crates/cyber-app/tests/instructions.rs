//! Instruction files through the production App (`system-context` → Instruction file
//! discovery, Nested rule files on read, Reconciliation at Safe Boundaries): the real
//! AppHost on a TCP listener and a local scripted model whose captured request bodies
//! show exactly what reached the provider.

#[path = "support/scripted_model.rs"]
mod scripted_model;

use std::path::{Path, PathBuf};
use std::time::Duration;

use cyber_app::{App, AppOptions};
use cyber_core::paths::{DatabaseLocation, Paths};
use reqwest::{Client, Method};
use scripted_model::ScriptedModel;
use serde_json::{Value, json};

const PASSWORD: &str = "instructions-test";

/// Reads `web/src/app.ts` when asked, answers a tool result with `done`, else `ok`.
fn reply(body: &Value) -> Value {
    let last = body["messages"]
        .as_array()
        .and_then(|messages| messages.last())
        .cloned()
        .unwrap_or_default();
    if last["role"] == "tool" {
        return json!({"content": "done"});
    }
    let text = last.to_string();
    for (prompt, id) in [
        ("read the app first", "call_first"),
        ("read the app again", "call_again"),
    ] {
        if text.contains(prompt) {
            let arguments = json!({"path": "web/src/app.ts"}).to_string();
            let id = format!("{id}_{}", body["messages"].as_array().unwrap().len());
            return json!({"tool_calls":[{"index":0,"id":id,"type":"function",
                "function":{"name":"read","arguments":arguments}}]});
        }
    }
    json!({"content": "ok"})
}

#[tokio::test]
async fn configured_instruction_limits_bound_baseline_and_nested_provider_text() {
    let f = Fixture::start_with(
        json!({"instruction_limits":{"max_file_bytes":3000,"max_total_bytes":4096}}),
    )
    .await;
    f.write("AGENTS.md", &"λ".repeat(5000));
    f.write("web/AGENTS.md", &"Ψ".repeat(5000));
    f.write("web/CYBER.md", &"Ω".repeat(5000));
    f.write("web/src/app.ts", "export const app = true;");
    let session = f.session(&f.repo).await;
    f.prompt(&session, &f.repo, "read the app first", 2).await;
    let body = f.last_request();
    let state = f.app.runtime.state(&session).await.unwrap();
    let epoch = state.epoch.unwrap();
    let instructions = &epoch.snapshot["core/instructions"];
    assert!(instructions.len() <= 4096);
    assert!(instructions.contains(&"λ".repeat(1500)));
    assert!(!instructions.contains(&"λ".repeat(1501)));
    assert!(system_prompt(&body).contains(instructions));
    let nested = last_tool_result(&body);
    let start = nested.find("\n\n<system-reminder>").unwrap();
    let reminder = &nested[start..];
    assert!(reminder.len() <= 4096, "{} bytes", reminder.len());
    assert!(reminder.ends_with("</system-reminder>"));
    assert!(reminder.contains("Ψ"));
    assert!(!reminder.contains(&"Ψ".repeat(1501)));
    assert!(!reminder.contains(&"Ω".repeat(1501)));
    assert!(reminder.contains("total budget") || reminder.contains("instruction budget"));

    f.app.runtime.shutdown().await;
}

#[tokio::test]
async fn bounded_lists_record_only_labelled_files_and_retry_unlisted_imports() {
    let f = Fixture::start_with(
        json!({"instruction_limits":{"max_file_bytes":512,"max_total_bytes":512}}),
    )
    .await;
    let refs: Vec<_> = (0..20).map(|i| format!("@r/{i:02}.md")).collect();
    f.write("web/CYBER.md", &refs.join("\n"));
    for i in 0..20 {
        f.write(&format!("web/r/{i:02}.md"), "x");
    }
    f.write("web/src/app.ts", "export const app = true;");
    let session = f.session(&f.repo).await;
    f.prompt(&session, &f.repo, "read the app first", 2).await;
    let first = f
        .app
        .runtime
        .state(&session)
        .await
        .unwrap()
        .epoch
        .unwrap()
        .reminded_instructions;
    let output = last_tool_result(&f.last_request());
    let reminder = &output[output.find("\n\n<system-reminder>").unwrap()..];
    assert!(reminder.len() <= 512);
    assert!(!first.is_empty() && first.len() < 21);
    for id in &first {
        assert!(
            reminder.contains(id),
            "unlabelled identity {id}: {reminder}"
        );
    }
    f.prompt(&session, &f.repo, "read the app again", 2).await;
    let second = f
        .app
        .runtime
        .state(&session)
        .await
        .unwrap()
        .epoch
        .unwrap()
        .reminded_instructions;
    let output = last_tool_result(&f.last_request());
    let reminder = &output[output.find("\n\n<system-reminder>").unwrap()..];
    assert!(reminder.len() <= 512);
    assert!(second.len() > first.len());
    for id in second.difference(&first) {
        assert!(
            reminder.contains(id),
            "unlabelled identity {id}: {reminder}"
        );
    }
    for id in &first {
        assert!(
            !reminder.contains(id),
            "duplicate identity {id}: {reminder}"
        );
    }
    f.app.runtime.shutdown().await;
}

#[tokio::test]
async fn project_rules_require_review_then_separate_baseline_from_matching_reads() {
    let f = Fixture::start().await;
    f.write(".cyber/rules/base.md", "ROOT_RULE_GUIDANCE");
    f.write(
        ".cyber/rules/web.md",
        "---\npaths: ['web/**']\n---\nWEB_RULE_GUIDANCE",
    );
    f.write("web/src/app.ts", "export const app = true;");
    let held = f.session(&f.repo).await;
    f.prompt(&held, &f.repo, "read the app first", 2).await;
    assert!(!every_request(&f).contains("ROOT_RULE_GUIDANCE"));
    assert!(!every_request(&f).contains("WEB_RULE_GUIDANCE"));
    f.approve(&f.repo);
    let session = f.session(&f.repo).await;
    f.prompt(&session, &f.repo, "read the app first", 2).await;
    let request = f.last_request();
    assert!(system_prompt(&request).contains("ROOT_RULE_GUIDANCE"));
    assert!(!system_prompt(&request).contains("WEB_RULE_GUIDANCE"));
    let result = last_tool_result(&request);
    assert!(result.contains("WEB_RULE_GUIDANCE"));
    assert!(!result.contains("paths:"));
    let state = f.app.runtime.state(&session).await.unwrap();
    let ids = state.epoch.unwrap().reminded_rules;
    assert_eq!(ids.len(), 1, "{ids:?}");
    assert!(ids.iter().all(|id| id.starts_with("rule:sha256:")));
    let events = f.app.store.read_events(&session, -1, 500).unwrap().events;
    let settled = events
        .iter()
        .find(|event| {
            event.kind == "session.tool.settled.1"
                && event.data["rule_reminders"]
                    .as_array()
                    .is_some_and(|rules| !rules.is_empty())
        })
        .unwrap();
    assert!(
        settled.data.get("instruction_reminders").is_none(),
        "legacy path field is preserved"
    );
    let binding = &settled.data["rule_reminders"][0];
    assert_eq!(binding["root"], f.repo.to_str().unwrap());
    assert_eq!(
        binding["path"],
        f.repo.join(".cyber/rules/web.md").to_str().unwrap()
    );
    assert!(binding["digest"].as_str().unwrap().starts_with("sha256:"));
    assert!(
        binding["generation"]
            .as_str()
            .unwrap()
            .starts_with("sha256:")
    );
    let proof = &settled.data["rule_read"];
    assert_eq!(
        proof["evidence"]["path"],
        f.repo.join("web/src/app.ts").to_str().unwrap()
    );
    assert_eq!(proof["evidence"]["root_identity"], binding["native_root"]);
    assert!(
        proof["evidence"]["file_identity"]
            .as_str()
            .unwrap()
            .starts_with("sha256:")
    );
    assert_eq!(
        proof["operation_key"],
        format!("{session}:{}", settled.data["call_id"].as_str().unwrap())
    );
    assert_eq!(proof["attempt"], 1);
    assert_eq!(proof["location"], f.repo.to_str().unwrap());
    for field in ["epoch", "attempt", "operation_key", "location"] {
        let mut changed = events.clone();
        let event = changed
            .iter_mut()
            .find(|event| event.id == settled.id)
            .unwrap();
        event.data["rule_read"][field] = if matches!(field, "operation_key" | "location") {
            json!("other-call")
        } else {
            json!(999)
        };
        let replayed = cyber_server::runtime::SessionState::replay(&changed).unwrap();
        assert!(replayed.epoch.unwrap().reminded_rules.is_empty(), "{field}");
    }
    let replayed = cyber_server::runtime::SessionState::replay(&events).unwrap();
    assert_eq!(replayed.epoch.unwrap().reminded_rules, ids);
    f.prompt(&session, &f.repo, "read the app again", 2).await;
    assert!(!last_tool_result(&f.last_request()).contains("WEB_RULE_GUIDANCE"));
    f.app.runtime.shutdown().await;
}

#[tokio::test]
async fn changed_rule_bytes_stay_held_until_the_new_definition_is_reviewed() {
    let f = Fixture::start().await;
    let path = ".cyber/rules/web.md";
    f.write(path, "---\npaths: ['web/**']\n---\nRULE_VERSION_A");
    f.write("web/src/app.ts", "export const app = true;");
    f.approve(&f.repo);
    let session = f.session(&f.repo).await;
    f.prompt(&session, &f.repo, "read the app first", 2).await;
    assert!(last_tool_result(&f.last_request()).contains("RULE_VERSION_A"));
    f.write(path, "---\npaths: ['web/**']\n---\nRULE_VERSION_B");
    f.prompt(&session, &f.repo, "read the app again", 2).await;
    assert!(!last_tool_result(&f.last_request()).contains("RULE_VERSION_B"));
    f.approve(&f.repo);
    f.prompt(&session, &f.repo, "read the app again", 2).await;
    assert!(last_tool_result(&f.last_request()).contains("RULE_VERSION_B"));
    assert_eq!(
        f.app
            .runtime
            .state(&session)
            .await
            .unwrap()
            .epoch
            .unwrap()
            .reminded_rules
            .len(),
        2
    );
    f.app.runtime.shutdown().await;
}

#[tokio::test]
async fn rule_imports_keep_read_and_scope_gates_and_claude_compatibility() {
    let f = Fixture::start_with(
        json!({"compat":{"claude":false},"permissions":{"read":{".cyber/rules/secret.md":"deny"}}}),
    )
    .await;
    f.write(
        ".cyber/rules/a.md",
        "CYBER_GUIDANCE\n@../../docs/extra.md\n@../../../private.md\n@db.md",
    );
    f.write(
        ".cyber/rules/db.md",
        "---\npaths: ['db/**']\n---\nDB_GUIDANCE",
    );
    f.write(".cyber/rules/secret.md", "SECRET_GUIDANCE");
    f.write(".claude/rules/a.md", "CLAUDE_GUIDANCE");
    f.write("docs/extra.md", "EXTRA_GUIDANCE");
    std::fs::write(
        f.repo.parent().unwrap().join("private.md"),
        "OUTSIDE_GUIDANCE",
    )
    .unwrap();
    std::fs::create_dir_all(f.config.join("rules")).unwrap();
    std::fs::write(f.config.join("rules/global.md"), "GLOBAL_GUIDANCE").unwrap();
    f.approve(&f.repo);
    let session = f.session(&f.repo).await;
    f.prompt(&session, &f.repo, "hello", 1).await;
    let system = system_prompt(&f.last_request());
    for present in ["GLOBAL_GUIDANCE", "CYBER_GUIDANCE", "EXTRA_GUIDANCE"] {
        assert!(system.contains(present), "{present}: {system}");
    }
    for absent in [
        "DB_GUIDANCE",
        "SECRET_GUIDANCE",
        "CLAUDE_GUIDANCE",
        "OUTSIDE_GUIDANCE",
    ] {
        assert!(!system.contains(absent), "{absent}: {system}");
    }
    f.app.runtime.shutdown().await;
}

#[tokio::test]
async fn nested_rules_use_the_reviewed_operation_directory_in_the_same_checkout() {
    let f = Fixture::start().await;
    f.write(
        "web/.cyber/rules/local.md",
        "---\npaths: ['web/src/**']\n---\nLOCAL_NESTED_GUIDANCE",
    );
    f.write("web/src/app.ts", "export const app = true;");
    f.approve(&f.repo.join("web"));
    let session = f.session(&f.repo).await;
    f.prompt(&session, &f.repo, "read the app first", 2).await;
    assert!(!system_prompt(&f.last_request()).contains("LOCAL_NESTED_GUIDANCE"));
    assert!(last_tool_result(&f.last_request()).contains("LOCAL_NESTED_GUIDANCE"));
    f.app.runtime.shutdown().await;
}

#[tokio::test]
async fn a_nested_checkout_cannot_supply_rules_for_its_parent_session() {
    let f = Fixture::start().await;
    f.write("web/.git/marker", "");
    f.write(
        "web/.cyber/rules/local.md",
        "---\npaths: ['web/src/**']\n---\nFOREIGN_RULE_GUIDANCE",
    );
    f.write("web/src/app.ts", "export const app = true;");
    f.approve(&f.repo.join("web"));
    let session = f.session(&f.repo).await;
    f.prompt(&session, &f.repo, "read the app first", 2).await;
    assert!(last_tool_result(&f.last_request()).contains("export const app"));
    assert!(!every_request(&f).contains("FOREIGN_RULE_GUIDANCE"));
    assert!(
        f.app
            .runtime
            .state(&session)
            .await
            .unwrap()
            .epoch
            .unwrap()
            .reminded_rules
            .is_empty()
    );
    f.app.runtime.shutdown().await;
}

struct Fixture {
    _temp: tempfile::TempDir,
    repo: PathBuf,
    config: PathBuf,
    model: ScriptedModel,
    app: App,
    url: String,
    client: Client,
}

impl Fixture {
    fn approve(&self, directory: &Path) {
        let env = std::collections::HashMap::<String, String>::new();
        let home = self._temp.path().join("home");
        let report = cyber_core::config::trust_report(&cyber_core::config::LoadRequest {
            location: directory,
            paths: &self.app.paths,
            env: &env,
            home: &home,
            profile: None,
            overrides: &[],
            flags: json!({}),
        })
        .unwrap();
        cyber_core::trust::TrustStore::new(self.app.paths.trust_file())
            .approve(&report.checkout_root, report.digest.as_deref().unwrap())
            .unwrap();
    }
    async fn start() -> Self {
        Self::start_with(json!({})).await
    }

    /// Start with `extra` top-level keys added to the global configuration.
    async fn start_with(extra: Value) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let paths = Paths {
            data: root.join("data"),
            config: root.join("config"),
            state: root.join("state"),
            cache: root.join("cache"),
            tmp: root.join("tmp"),
        };
        paths.ensure().unwrap();
        let model = ScriptedModel::start(reply);
        let mut global = model.config();
        for (key, value) in extra.as_object().unwrap() {
            global[key] = value.clone();
        }
        std::fs::write(paths.config.join("cyber.json"), global.to_string()).unwrap();
        let config = paths.config.clone();
        let app = App::build(AppOptions {
            paths,
            home: root.join("home"),
            database: DatabaseLocation::Memory,
            default_directory: root.clone(),
            sandbox_policy: None,
            snapshots: false,
            interactive: true,
            password: Some(PASSWORD.into()),
            mcp_credentials: None,
            remote_skill_fetcher: None,
            workflow_cores: None,
            isolation: None,
        })
        .await
        .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/api/v1", listener.local_addr().unwrap());
        tokio::spawn(cyber_server::http::serve_tcp(
            app.router(),
            listener,
            std::future::pending::<()>(),
        ));
        let repo = root.join("repo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        Self {
            _temp: temp,
            repo,
            config,
            model,
            app,
            url,
            client: Client::builder().no_proxy().build().unwrap(),
        }
    }

    fn write(&self, relative: &str, text: &str) -> PathBuf {
        let path = self.repo.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
        path
    }

    async fn call(
        &self,
        method: Method,
        path: &str,
        directory: &Path,
        body: Option<Value>,
    ) -> Value {
        let mut request = self
            .client
            .request(method, format!("{}{path}", self.url))
            .header("x-cyber-directory", directory.display().to_string())
            .basic_auth("cyber", Some(PASSWORD));
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.unwrap();
        let status = response.status();
        let text = response.text().await.unwrap();
        assert!(status.is_success(), "{path}: {status} {text}");
        serde_json::from_str(&text).unwrap_or(Value::Null)
    }

    async fn session(&self, directory: &Path) -> String {
        let created = self
            .call(
                Method::POST,
                "/sessions",
                directory,
                Some(json!({"model": "test/main", "mode": "bypass"})),
            )
            .await;
        created["data"]["id"].as_str().unwrap().to_owned()
    }

    /// Prompt and wait until the model has answered `requests` more times and the
    /// Session is idle again.
    async fn prompt(&self, session: &str, directory: &Path, text: &str, requests: usize) {
        let before = self.model.requests().len();
        self.call(
            Method::POST,
            &format!("/sessions/{session}/prompt"),
            directory,
            Some(json!({"parts": [{"type": "text", "text": text}]})),
        )
        .await;
        for _ in 0..1000 {
            if self.model.requests().len() >= before + requests
                && !self.app.runtime.is_running(session)
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("{session} did not answer {text:?}");
    }

    /// The latest Turn request. Session titles are generated by a concurrent request
    /// that can land after the Turn's, so it is skipped.
    fn last_request(&self) -> Value {
        self.model
            .requests()
            .into_iter()
            .rev()
            .find(|body| !is_title_request(body))
            .unwrap()
    }
}

fn is_title_request(body: &Value) -> bool {
    all_messages(body)
        .iter()
        .any(|message| text(message).starts_with("Write a short title"))
}

fn messages<'a>(body: &'a Value, role: &str) -> Vec<&'a Value> {
    body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| message["role"] == role)
        .collect()
}

fn all_messages(body: &Value) -> Vec<&Value> {
    body["messages"].as_array().unwrap().iter().collect()
}

/// A message's text (string content, or each text part).
fn text(message: &Value) -> String {
    match &message["content"] {
        Value::String(text) => text.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|part| part["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        other => other.to_string(),
    }
}

/// The system messages before the first user message: the cached prefix.
fn system_prompt(body: &Value) -> String {
    body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .take_while(|message| message["role"] == "system")
        .map(text)
        .collect::<Vec<_>>()
        .join("\n")
}

fn assert_in_order(text: &str, needles: &[&str]) {
    let mut from = 0;
    for needle in needles {
        let found = text[from..]
            .find(needle)
            .unwrap_or_else(|| panic!("{needle:?} missing after byte {from} in {text}"));
        from += found + needle.len();
    }
}

#[tokio::test]
async fn cyber_md_reaches_the_model_after_agents_md_nested_and_after_edits() {
    let f = Fixture::start().await;
    std::fs::write(f.config.join("AGENTS.md"), "Global agents rule.").unwrap();
    std::fs::write(f.config.join("CYBER.md"), "Global cyber rule.").unwrap();
    f.write("CYBER.md", "Root cyber rule.");
    f.write("AGENTS.md", "Root agents rule.");
    f.write("web/AGENTS.md", "Web agents rule.");
    let web_cyber = f.write("web/CYBER.md", "Web cyber rule.");
    f.write("web/src/app.ts", "export const app = 1;\n");
    let repo = f.repo.clone();

    // Root Location: global AGENTS.md, global CYBER.md, then the project's AGENTS.md and
    // CYBER.md; the web/ files are not in the baseline.
    let session = f.session(&repo).await;
    f.prompt(&session, &repo, "hello", 1).await;
    let first = system_prompt(&f.last_request());
    assert_in_order(
        &first,
        &[
            "Global agents rule.",
            "Global cyber rule.",
            "Root agents rule.",
            "Root cyber rule.",
        ],
    );
    assert!(!first.contains("Web cyber rule."), "{first}");
    let context = f
        .call(
            Method::GET,
            &format!("/sessions/{session}/context"),
            &repo,
            None,
        )
        .await;
    let baseline = context["data"]["baseline"].as_str().unwrap();
    let source = format!("Instructions from: {}", repo.join("CYBER.md").display());
    assert!(baseline.contains(&source), "{baseline}");

    // A read under web/ attaches web/AGENTS.md and web/CYBER.md once per Context Epoch.
    f.prompt(&session, &repo, "read the app first", 2).await;
    let tools = f.last_request();
    let result = text(messages(&tools, "tool").last().unwrap());
    assert!(result.contains("<system-reminder>"), "{result}");
    assert!(result.contains("Web cyber rule."), "{result}");
    let label = format!("Instructions from: {}", web_cyber.display());
    assert!(result.contains(&label), "{result}");
    assert_in_order(&result, &["Web agents rule.", "Web cyber rule."]);
    f.prompt(&session, &repo, "read the app again", 2).await;
    let again = f.last_request();
    let result = text(messages(&again, "tool").last().unwrap());
    assert!(result.contains("export const app"), "{result}");
    assert!(!result.contains("Web cyber rule."), "{result}");

    // A mid-session edit arrives as one durable system message (this adapter sends it as
    // a `<system-reminder>` turn); the cached prefix is unchanged.
    f.write("CYBER.md", "Root cyber rule v2.");
    f.prompt(&session, &repo, "hello again", 1).await;
    let edited = f.last_request();
    assert_eq!(system_prompt(&edited), first);
    let update = all_messages(&edited)
        .into_iter()
        .map(text)
        .find(|text| text.contains("core/instructions context changed"))
        .unwrap_or_else(|| panic!("a context update in {edited:#}"));
    assert!(update.contains("Root cyber rule v2."), "{update}");
    f.prompt(&session, &repo, "and once more", 1).await;
    let later = f.last_request();
    let updates = all_messages(&later)
        .into_iter()
        .filter(|m| text(m).contains("core/instructions context changed"))
        .count();
    assert_eq!(updates, 1, "the update is durable history, sent once");

    // A nested Location loads the web/ files in its baseline, after the root's.
    let web = repo.join("web");
    let nested = f.session(&web).await;
    f.prompt(&nested, &web, "hello", 1).await;
    assert_in_order(
        &system_prompt(&f.last_request()),
        &[
            "Global agents rule.",
            "Global cyber rule.",
            "Root agents rule.",
            "Root cyber rule v2.",
            "Web agents rule.",
            "Web cyber rule.",
        ],
    );
    f.app.runtime.shutdown().await;
}

/// The text of the last tool result the model was sent.
fn last_tool_result(body: &Value) -> String {
    text(messages(body, "tool").last().unwrap())
}

/// Regression: two nested files near `instructions.max_bytes` made the reminder larger
/// than the default tool output budget, so the read settled as `Tool crashed` and every
/// later read under `web/` crashed the same way for the rest of the Context Epoch.
#[tokio::test]
async fn two_full_size_nested_files_still_give_a_successful_read() {
    let f = Fixture::start().await;
    f.write("web/AGENTS.md", &"a".repeat(32 * 1024));
    let cyber = f.write("web/CYBER.md", &"c".repeat(32 * 1024));
    f.write("web/src/app.ts", "export const app = 1;\n");
    let repo = f.repo.clone();
    let session = f.session(&repo).await;
    f.prompt(&session, &repo, "read the app first", 2).await;
    let result = last_tool_result(&f.last_request());
    assert!(
        !result.contains("crashed"),
        "{}",
        &result[..result.len().min(400)]
    );
    assert!(result.contains("export const app"));
    assert!(result.contains(&"a".repeat(1024)));
    assert!(!result.contains(&"a".repeat(32 * 1024)));
    let reminder = &result[result.find("\n\n<system-reminder>").unwrap()..];
    assert!(reminder.len() <= 32 * 1024);
    assert!(result.contains(&format!("- {}", cyber.display())));
    assert!(!result.contains("cccc"));
    f.prompt(&session, &repo, "read the app again", 2).await;
    let again = last_tool_result(&f.last_request());
    assert!(again.contains("export const app"), "{again}");
    assert!(!again.contains("<system-reminder>"), "{again}");
    f.app.runtime.shutdown().await;
}

/// Regression: files attached by a nested read never fired `InstructionsLoaded`, so audit
/// and policy hooks did not see them enter the context.
#[tokio::test]
async fn nested_instruction_files_fire_instructions_loaded() {
    let recorder = ScriptedModel::start(|_| json!({}));
    let hook =
        json!([{"hooks":[{"type":"http","url":format!("{}/hook", recorder.url),"id":"recorder"}]}]);
    let f = Fixture::start_with(json!({"hooks": {"InstructionsLoaded": hook}})).await;
    f.write("CYBER.md", "Root cyber rule.");
    let web_cyber = f.write("web/CYBER.md", "Web cyber rule.");
    f.write("web/src/app.ts", "export const app = 1;\n");
    let repo = f.repo.clone();
    let session = f.session(&repo).await;
    f.prompt(&session, &repo, "read the app first", 2).await;
    let loaded: Vec<Value> = recorder
        .requests()
        .into_iter()
        .filter(|body| body["event"] == "InstructionsLoaded")
        .collect();
    assert_eq!(loaded.len(), 2, "{loaded:#?}");
    assert_eq!(loaded[0]["load_reason"], "session_start");
    let nested = &loaded[1];
    assert_eq!(nested["load_reason"], "nested_read", "{nested}");
    assert_eq!(nested["source"], "core/instructions");
    assert_eq!(nested["session_id"], session.as_str());
    assert_eq!(
        nested["file_paths"],
        json!([web_cyber.display().to_string()])
    );
    assert!(nested["digest"].as_str().unwrap().starts_with("sha256:"));
    f.app.runtime.shutdown().await;
}

/// Regression: the runtime read `compat.claude` once, from the server's default
/// directory, so a project that turned it off still had its `CLAUDE.md` injected (while
/// `cyber doctor`, reading the Location's config, said it was not loaded).
#[tokio::test]
async fn a_project_compat_claude_false_keeps_its_claude_md_out_of_the_request() {
    let f = Fixture::start().await;
    f.write("CLAUDE.md", "Project claude rule.");
    f.write("CYBER.md", "Project cyber rule.");
    f.write(
        "cyber.json",
        &json!({"compat": {"claude": false}}).to_string(),
    );
    let other = f.repo.parent().unwrap().join("other");
    std::fs::create_dir_all(other.join(".git")).unwrap();
    std::fs::write(other.join("CLAUDE.md"), "Other claude rule.").unwrap();
    let repo = f.repo.clone();
    let session = f.session(&repo).await;
    f.prompt(&session, &repo, "hello", 1).await;
    let prompt = system_prompt(&f.last_request());
    assert!(prompt.contains("Project cyber rule."), "{prompt}");
    assert!(!prompt.contains("Project claude rule."), "{prompt}");
    // A Location without the setting still loads its CLAUDE.md.
    let session = f.session(&other).await;
    f.prompt(&session, &other, "hello", 1).await;
    let prompt = system_prompt(&f.last_request());
    assert!(prompt.contains("Other claude rule."), "{prompt}");
    f.app.runtime.shutdown().await;
}

/// A directory holding `AGENTS.md`, `CLAUDE.md` and `CYBER.md` sends `AGENTS.md` then
/// `CYBER.md`: `CLAUDE.md` is the fallback for a directory without `AGENTS.md`
/// (compat-import "Both files in one directory"), and without `AGENTS.md` it takes that
/// slot, still before `CYBER.md`.
#[tokio::test]
async fn agents_claude_and_cyber_md_in_one_directory_send_agents_then_cyber() {
    let f = Fixture::start().await;
    f.write("CYBER.md", "Root cyber rule.");
    f.write("CLAUDE.md", "Root claude rule.");
    f.write("AGENTS.md", "Root agents rule.");
    let repo = f.repo.clone();
    let session = f.session(&repo).await;
    f.prompt(&session, &repo, "hello", 1).await;
    let prompt = system_prompt(&f.last_request());
    assert_in_order(&prompt, &["Root agents rule.", "Root cyber rule."]);
    assert!(!prompt.contains("Root claude rule."), "{prompt}");

    std::fs::remove_file(repo.join("AGENTS.md")).unwrap();
    let session = f.session(&repo).await;
    f.prompt(&session, &repo, "hello", 1).await;
    let prompt = system_prompt(&f.last_request());
    assert_in_order(&prompt, &["Root claude rule.", "Root cyber rule."]);
    assert!(!prompt.contains("Root agents rule."), "{prompt}");
    f.app.runtime.shutdown().await;
}

/// Regression: a project `CYBER.md` symlinked outside the checkout was injected into the
/// system context; a nested one leaked on the first read below it.
#[cfg(unix)]
#[tokio::test]
async fn instruction_files_symlinked_outside_the_project_never_reach_the_model() {
    let f = Fixture::start().await;
    let secret = f.repo.parent().unwrap().join("secret.txt");
    std::fs::write(&secret, "TOP SECRET KEY").unwrap();
    std::os::unix::fs::symlink("../secret.txt", f.repo.join("CYBER.md")).unwrap();
    std::fs::create_dir_all(f.repo.join("web")).unwrap();
    std::os::unix::fs::symlink("../../secret.txt", f.repo.join("web/CYBER.md")).unwrap();
    f.write("web/src/app.ts", "export const app = 1;\n");
    let repo = f.repo.clone();
    let session = f.session(&repo).await;
    f.prompt(&session, &repo, "read the app first", 2).await;
    let body = f.last_request().to_string();
    assert!(!body.contains("TOP SECRET KEY"), "{body}");
    assert!(last_tool_result(&f.last_request()).contains("export const app"));
    f.app.runtime.shutdown().await;
}

/// Every request the model was sent, as one string.
fn every_request(f: &Fixture) -> String {
    f.model
        .requests()
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n")
}

/// A checkout that imports or symlinks `secret` (project-relative) from its instruction
/// files, read at the root and through a nested read under `web/`.
async fn assert_secret_stays_out(f: &Fixture, secret: &str) {
    f.write(secret, "TOKEN=hunter2");
    f.write("CYBER.md", &format!("Root cyber rule.\n@{secret}"));
    std::fs::create_dir_all(f.repo.join("web")).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(format!("../{secret}"), f.repo.join("web/CYBER.md")).unwrap();
    #[cfg(not(unix))]
    f.write("web/CYBER.md", &format!("@../{secret}"));
    f.write("web/src/app.ts", "export const app = 1;\n");
    let repo = f.repo.clone();
    let session = f.session(&repo).await;
    f.prompt(&session, &repo, "read the app first", 2).await;
    let prompt = system_prompt(&f.last_request());
    assert!(prompt.contains("Root cyber rule."), "{prompt}");
    assert!(last_tool_result(&f.last_request()).contains("export const app"));
    let all = every_request(f);
    assert!(!all.contains("hunter2"), "{all}");
}

/// Regression: project instruction files and their imports skipped the `read` rules, so
/// `@.env` (or a `CYBER.md` symlinked to it) put a local secret into privileged context
/// without the prompt the default `*.env` Ask rule raises for a `read`.
#[tokio::test]
async fn instruction_files_never_import_a_file_the_default_read_rules_ask_about() {
    let f = Fixture::start().await;
    assert_secret_stays_out(&f, ".env").await;
    f.app.runtime.shutdown().await;
}

/// Regression: a configured `read` deny (as `Read(./config/secrets.json)` converts) did
/// not apply to instruction imports or symlinked instruction files.
#[tokio::test]
async fn instruction_files_never_import_a_file_a_read_deny_rule_covers() {
    let deny = json!({"rules": [
        {"action": "read", "resource": "config/secrets.json", "effect": "deny"}
    ]});
    let f = Fixture::start_with(json!({"permissions": deny})).await;
    assert_secret_stays_out(&f, "config/secrets.json").await;
    f.app.runtime.shutdown().await;
}

/// The provider's input is not operation evidence: PreToolUse can rewrite it.
#[cfg(unix)]
#[tokio::test]
async fn rewritten_reads_deliver_only_rules_for_the_file_actually_opened() {
    let rewrite = json!({"updated_input":{"path":"db/schema.sql"}});
    let command = format!("cat >/dev/null; printf '%s' '{rewrite}'");
    let f = Fixture::start_with(json!({"hooks":{"PreToolUse":[{"matcher":"read","hooks":[{
        "type":"command","id":"redirect-read","command":command
    }]}]}}))
    .await;
    f.write("web/src/app.ts", "ORIGINAL_WEB_BYTES");
    f.write("db/schema.sql", "ACTUAL_DATABASE_BYTES");
    f.write(
        ".cyber/rules/web.md",
        "---\npaths: ['web/**']\n---\nWRONG_WEB_RULE",
    );
    f.write(
        ".cyber/rules/db.md",
        "---\npaths: ['db/**']\n---\nACTUAL_DB_RULE",
    );
    f.approve(&f.repo);
    let id = f.session(&f.repo).await;
    f.prompt(&id, &f.repo, "read the app first", 2).await;
    let result = last_tool_result(&f.last_request());
    assert!(result.contains("ACTUAL_DATABASE_BYTES"), "{result}");
    assert!(result.contains("ACTUAL_DB_RULE"), "{result}");
    assert!(!result.contains("ORIGINAL_WEB_BYTES"), "{result}");
    assert!(!result.contains("WRONG_WEB_RULE"), "{result}");
    let events = f.app.store.read_events(&id, -1, 500).unwrap().events;
    let settled = events
        .iter()
        .find(|event| event.data["rule_read"].is_object())
        .unwrap();
    assert_eq!(
        settled.data["rule_read"]["evidence"]["path"],
        f.repo.join("db/schema.sql").to_str().unwrap()
    );
    let state = f.app.runtime.state(&id).await.unwrap();
    let call = &state.calls[settled.data["call_id"].as_str().unwrap()];
    assert_eq!(call.input.as_ref().unwrap()["path"], "web/src/app.ts");
    assert_eq!(state.epoch.unwrap().reminded_rules.len(), 1);
    f.app.runtime.shutdown().await;
}

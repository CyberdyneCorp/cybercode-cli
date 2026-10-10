mod support;
use cyber_server::runtime::{JobStatus, NoSnapshots};
use serde_json::json;
use std::sync::Arc;
use support::flow::{Flow, call, text};

#[tokio::test]
async fn model_forked_skill_uses_a_background_child_and_returns_only_its_summary() {
    let mut child_calls = call(
        "write",
        "write",
        json!({"path":"audit.txt","content":"audit evidence"}),
    );
    let ending = child_calls.split_off(1);
    child_calls.push(
        call(
            "notes",
            "read",
            json!({"path":".cyber/skills/audit/notes.md"}),
        )
        .remove(0),
    );
    child_calls.extend(ending);
    let flow = Flow::with_models(
        support::Fixture::new(),
        vec![
            call("skill", "skill", json!({"name":"audit"})),
            text("parent continues"),
            text("summary received"),
        ],
        false,
        Arc::new(NoSnapshots),
        vec![("other/skill", vec![child_calls, text("audit summary")])],
    );
    flow.f
        .set_config(json!({"permissions":{"skill":"allow","agent":"allow","edit":"ask"}}));
    flow.f.write(".cyber/skills/audit/SKILL.md", "---\nname: audit\ndescription: Audit changes\ncontext: fork\nmodel: other/skill\nallowed-tools: ['write:*']\n---\nPRIVATE AUDIT INSTRUCTIONS\n");
    flow.f.write(
        ".cyber/skills/audit/notes.md",
        "private supporting evidence",
    );
    let parent = flow.session("default").await;
    flow.prompt(&parent, "audit changes").await;
    flow.settle(&parent).await;
    let output = flow.output(&parent, "skill").await;
    let launched: serde_json::Value =
        serde_json::from_str(&output).expect("forked skill must acknowledge a background Job");
    let job_id = launched["job_id"].as_str().unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            if flow.runtime.job(job_id).unwrap().status != JobStatus::Running {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    flow.settle(&parent).await;
    let job = flow.runtime.job(job_id).unwrap();
    assert_eq!(job.status, JobStatus::Completed);
    assert!(job.description.contains("audit"));
    assert_eq!(job.result.unwrap()["text"], "audit summary");
    let child = flow
        .runtime
        .state(launched["id"].as_str().unwrap())
        .await
        .unwrap();
    assert_eq!(child.info.parent_id.as_deref(), Some(parent.as_str()));
    assert_eq!(child.info.model, "other/skill");
    assert_eq!(flow.f.read("audit.txt"), "audit evidence");
    assert_eq!(
        child.calls["notes"].status,
        cyber_server::runtime::CallStatus::Ok
    );
    let requests = flow.requests("other/skill");
    let texts: Vec<_> = requests[0]
        .messages
        .iter()
        .flat_map(|message| &message.content)
        .filter_map(|part| match part {
            cyber_llm::Content::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert!(texts.iter().any(|text| {
        text.contains(
            &flow
                .f
                .repo
                .join(".cyber/skills/audit")
                .display()
                .to_string(),
        ) && text.contains("notes.md")
    }));

    let parent_state = flow.runtime.state(&parent).await.unwrap();
    assert!(parent_state.calls["skill"].skill_activation.is_none());
    assert!(!format!("{:?}", parent_state.entries).contains("PRIVATE AUDIT INSTRUCTIONS"));
    assert!(format!("{:?}", parent_state.entries).contains("audit summary"));
    assert!(!format!("{:?}", parent_state.entries).contains("private supporting evidence"));
    flow.runtime.shutdown().await;
}

struct Services(Arc<cyber_tools::BuiltinHost>);
impl cyber_server::http::Services for Services {
    fn models(
        &self,
        _: &std::path::Path,
    ) -> futures::future::BoxFuture<'_, Result<Vec<cyber_server::http::ModelInfo>, String>> {
        Box::pin(async { Ok(vec![]) })
    }
    fn default_model(&self, _: &std::path::Path) -> Option<String> {
        Some("test/main".into())
    }
    fn agents(&self, _: &std::path::Path) -> Vec<cyber_server::http::AgentInfo> {
        vec![]
    }
    fn tools(
        &self,
        turn: &cyber_server::runtime::TurnContext,
    ) -> Vec<cyber_server::runtime::ToolDef> {
        cyber_server::runtime::ToolHost::definitions(self.0.as_ref(), turn)
    }
    fn commands(&self, _: &std::path::Path) -> Vec<cyber_server::http::CommandInfo> {
        vec![]
    }
    fn find_files(&self, _: &std::path::Path, _: &str, _: usize) -> Vec<String> {
        vec![]
    }
    fn expand_command(&self, _: &std::path::Path, _: &str, _: &str) -> Option<String> {
        None
    }
    fn command_plan<'a>(
        &'a self,
        turn: &'a cyber_server::runtime::TurnContext,
        name: &'a str,
        arguments: &'a str,
    ) -> futures::future::BoxFuture<'a, Result<Option<cyber_server::runtime::CommandPlan>, String>>
    {
        Box::pin(self.0.command_plan(turn, name, arguments))
    }
}

async fn terminal(flow: &Flow, job: &str) -> cyber_server::runtime::Job {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let job = flow.runtime.job(job).unwrap();
            if job.status != JobStatus::Running {
                return job;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn authenticated_user_fork_command_has_durable_retry_identity_and_queued_summary() {
    let mut flow = Flow::with_models(
        support::Fixture::new(),
        vec![text("parent received summary"), text("parent resumed")],
        false,
        Arc::new(NoSnapshots),
        vec![(
            "other/skill",
            vec![
                call(
                    "write",
                    "write",
                    json!({"path":"audit.txt","content":"audit"}),
                ),
                text("user audit summary"),
            ],
        )],
    );
    flow.f.set_config(json!({"permissions":{"edit":"ask"}}));
    let skill = "---\nname: audit\ndescription: Audit changes\ncontext: fork\nmodel: other/skill\ndisable-model-invocation: true\nallowed-tools: ['write:*']\n---\nPRIVATE USER AUDIT $ARGUMENTS\n";
    flow.f.write(".cyber/skills/audit/SKILL.md", skill);
    let parent = flow.session("default").await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!(
        "http://{}/api/v1/sessions/{parent}/command",
        listener.local_addr().unwrap()
    );
    let router = cyber_server::http::router(cyber_server::http::AppState {
        service: None,
        runtime: flow.runtime.clone(),
        remote_tools: Arc::default(),
        store: flow.f.store.clone(),
        services: Arc::new(Services(flow.f.host.clone())),
        options: Arc::new(cyber_server::http::HttpOptions {
            version: "test".into(),
            password: Some("password".into()),
            cors_origins: vec![],
            default_directory: flow.f.repo.clone(),
            features: vec![],
        }),
    });
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(cyber_server::http::serve_tcp(router, listener, async {
        let _ = stopped.await;
    }));
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let body = json!({"id":"msg_user_fork","name":"audit","arguments":"now","delivery":"hold","skill_command":{"model":"test/main"}});
    assert_eq!(
        client
            .post(&base)
            .json(&body)
            .send()
            .await
            .unwrap()
            .status(),
        reqwest::StatusCode::UNAUTHORIZED
    );
    let response = client
        .post(&base)
        .basic_auth("cyber", Some("password"))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::ACCEPTED);
    let admitted: serde_json::Value = response.json().await.unwrap();
    assert_eq!(admitted["data"]["message_id"], "msg_user_fork");
    assert_eq!(admitted["data"]["status"], "held");
    flow.settle(&parent).await;
    assert!(flow.runtime.jobs(Some(&parent)).unwrap().is_empty());
    assert!(
        flow.runtime
            .state(&parent)
            .await
            .unwrap()
            .entries
            .is_empty()
    );
    let release = client
        .post(format!(
            "{}/inbox/msg_user_fork/release",
            base.strip_suffix("/command").unwrap()
        ))
        .basic_auth("cyber", Some("password"))
        .json(&json!({"delivery":"queue"}))
        .send()
        .await
        .unwrap();
    assert_eq!(release.status(), reqwest::StatusCode::NO_CONTENT);
    let request =
        cyber_server::runtime::Runtime::skill_command_delegation_id(&parent, "msg_user_fork");
    let delegation = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            if let Some(d) = flow.runtime.delegation(&parent, &request).unwrap()
                && d.status != cyber_server::runtime::DelegationStatus::Pending
            {
                break d;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        delegation.status,
        cyber_server::runtime::DelegationStatus::Admitted
    );
    let job = terminal(&flow, delegation.job_id.as_deref().unwrap()).await;
    assert_eq!(job.status, JobStatus::Completed);
    assert_eq!(job.result.as_ref().unwrap()["text"], "user audit summary");
    let child = flow.runtime.state(&job.child_id).await.unwrap();
    assert_eq!(child.info.model, "other/skill");
    assert_eq!(
        child
            .inbox
            .iter()
            .find_map(|row| row.skill_command.as_ref())
            .unwrap()
            .activation
            .allowed_tools,
        ["write:*"]
    );
    flow.settle(&parent).await;
    let parent_state = flow.runtime.state(&parent).await.unwrap();
    assert!(!format!("{:?}", parent_state.entries).contains("PRIVATE USER AUDIT"));
    assert!(format!("{:?}", parent_state.entries).contains("user audit summary"));
    assert_eq!(flow.f.read("audit.txt"), "audit");
    let retry = client
        .post(&base)
        .basic_auth("cyber", Some("password"))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(retry.status(), reqwest::StatusCode::ACCEPTED);
    let retry: serde_json::Value = retry.json().await.unwrap();
    assert_eq!(
        retry["data"]["admitted_seq"],
        admitted["data"]["admitted_seq"]
    );
    assert_eq!(flow.runtime.jobs(Some(&parent)).unwrap().len(), 1);
    flow.f.write(
        ".cyber/skills/audit/SKILL.md",
        &skill.replace("write:*", "read:*"),
    );
    assert_eq!(
        client
            .post(&base)
            .basic_auth("cyber", Some("password"))
            .json(&body)
            .send()
            .await
            .unwrap()
            .status(),
        reqwest::StatusCode::CONFLICT
    );
    flow.f.write(
        ".cyber/skills/audit/SKILL.md",
        &skill.replace("context: fork", "context: inline"),
    );
    assert_eq!(
        client
            .post(&base)
            .basic_auth("cyber", Some("password"))
            .json(&body)
            .send()
            .await
            .unwrap()
            .status(),
        reqwest::StatusCode::CONFLICT
    );
    stop.send(()).unwrap();
    server.await.unwrap().unwrap();
    let child_requests = flow.requests("other/skill").len();
    flow.restart_default_runtime().await;
    assert!(
        flow.runtime
            .state(&parent)
            .await
            .unwrap()
            .input("msg_user_fork")
            .unwrap()
            .forwarded_skill
    );
    flow.runtime.resume(&parent).await.unwrap();
    flow.settle(&parent).await;
    assert_eq!(flow.runtime.jobs(Some(&parent)).unwrap().len(), 1);
    assert_eq!(flow.requests("other/skill").len(), child_requests);
    flow.runtime.shutdown().await;
}

#[tokio::test]
async fn forked_child_denials_override_grants_and_bypass() {
    let mut batch = call(
        "good",
        "write",
        json!({"path":"good.txt","content":"allowed"}),
    );
    let ending = batch.split_off(1);
    batch.push(
        call(
            "bad",
            "write",
            json!({"path":"blocked.txt","content":"denied"}),
        )
        .remove(0),
    );
    batch.push(call("stop", "task_stop", json!({"job_id":"missing"})).remove(0));
    batch.extend(ending);
    let flow = Flow::with_models(
        support::Fixture::new(),
        vec![
            call("skill", "skill", json!({"name":"audit"})),
            text("parent continues"),
            text("parent summary"),
        ],
        false,
        Arc::new(NoSnapshots),
        vec![("other/skill", vec![batch, text("limited audit summary")])],
    );
    flow.f.write(".cyber/skills/audit/SKILL.md","---\nname: audit\ndescription: Audit changes\ncontext: fork\nmodel: other/skill\nallowed-tools: ['write:*']\ndisallowed-tools: ['write:blocked*', task_stop]\n---\nAudit with limitations\n");
    let parent = flow.session("bypass").await;
    flow.prompt(&parent, "audit").await;
    flow.settle(&parent).await;
    let output = flow.output(&parent, "skill").await;
    let ack: serde_json::Value =
        serde_json::from_str(&output).unwrap_or_else(|error| panic!("{error}: {output}"));
    let job = terminal(&flow, ack["job_id"].as_str().unwrap()).await;
    assert_eq!(job.status, JobStatus::Completed);
    let child = flow.runtime.state(&job.child_id).await.unwrap();
    assert_eq!(
        child.calls["good"].status,
        cyber_server::runtime::CallStatus::Ok
    );
    assert_eq!(
        child.calls["bad"].status,
        cyber_server::runtime::CallStatus::Error
    );
    assert_eq!(
        child.calls["stop"].output.as_deref(),
        Some("Skill disallows tool: task_stop")
    );
    assert!(!flow.f.repo.join("blocked.txt").exists());
    assert_eq!(flow.f.read("good.txt"), "allowed");
    flow.runtime.shutdown().await;
}

#[tokio::test]
async fn queued_fork_waits_for_parent_completion_and_survives_parent_interrupt() {
    use cyber_server::runtime::{Delivery, PermissionReply};
    let flow = Flow::with_models(
        support::Fixture::new(),
        vec![
            call("pause", "read", json!({"path":"pause.txt"})),
            text("parent finished"),
            text("parent summary"),
        ],
        true,
        Arc::new(NoSnapshots),
        vec![(
            "other/skill",
            vec![
                call("secret", "read", json!({"path":".env"})),
                text("child summary"),
            ],
        )],
    );
    flow.f.write("pause.txt", "pause");
    flow.f.write(".env", "private child evidence");
    flow.f
        .set_config(json!({"permissions":{"read":{"pause.txt":"ask"}}}));
    flow.f.write(".cyber/skills/audit/SKILL.md","---\nname: audit\ndescription: Audit changes\ncontext: fork\nmodel: other/skill\n---\nAudit independently\n");
    let parent = flow.session("default").await;
    flow.prompt(&parent, "parent work").await;
    let parent_request = flow.pending(&parent).await;
    let info = flow.runtime.state(&parent).await.unwrap().info;
    let turn = cyber_server::runtime::TurnContext {
        session_id: parent.clone(),
        directory: info.directory,
        agent: info.agent,
        mode: info.mode,
        prefers_apply_patch: false,
        rules: info.rules,
    };
    let plan = flow
        .f
        .host
        .command_plan(&turn, "audit", "")
        .await
        .unwrap()
        .unwrap();
    flow.runtime
        .admit_user(
            &parent,
            plan.admission(Some("msg_queued_fork".into()), Delivery::Queue),
        )
        .await
        .unwrap();
    assert!(flow.runtime.jobs(Some(&parent)).unwrap().is_empty());
    assert_eq!(
        flow.runtime
            .state(&parent)
            .await
            .unwrap()
            .pending(Delivery::Queue)
            .count(),
        1
    );
    flow.runtime
        .reply_permission(&parent_request.id, PermissionReply::Once)
        .await
        .unwrap();
    let child_request = flow.pending(&parent).await;
    assert_ne!(child_request.session_id, parent);
    let job = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            if let Some(job) = flow.runtime.jobs(Some(&parent)).unwrap().into_iter().next() {
                break job;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(job.status, JobStatus::Running);
    flow.runtime.interrupt(&parent).await.unwrap();
    assert!(flow.runtime.is_running(&job.child_id));
    flow.runtime
        .reply_permission(&child_request.id, PermissionReply::Once)
        .await
        .unwrap();
    let completed = terminal(&flow, &job.id).await;
    assert_eq!(completed.status, JobStatus::Completed);
    flow.settle(&parent).await;
    let entries = format!("{:?}", flow.runtime.state(&parent).await.unwrap().entries);
    assert!(entries.contains("child summary"));
    assert!(!entries.contains("private child evidence"));
    assert!(!entries.contains("Audit independently"));
    flow.runtime.shutdown().await;
}

#[tokio::test]
async fn unavailable_fork_model_preserves_pending_input_without_creating_a_child() {
    use cyber_server::runtime::{Delivery, InputStatus};
    let flow = Flow::new(vec![text("base must not run")], false);
    flow.f.write(".cyber/skills/audit/SKILL.md","---\nname: audit\ndescription: Audit changes\ncontext: fork\nmodel: test/unavailable\n---\nAudit independently\n");
    let parent = flow.session("default").await;
    let info = flow.runtime.state(&parent).await.unwrap().info;
    let turn = cyber_server::runtime::TurnContext {
        session_id: parent.clone(),
        directory: info.directory,
        agent: info.agent,
        mode: info.mode,
        prefers_apply_patch: false,
        rules: info.rules,
    };
    let plan = flow
        .f
        .host
        .command_plan(&turn, "audit", "")
        .await
        .unwrap()
        .unwrap();
    flow.runtime
        .admit_user(
            &parent,
            plan.admission(Some("msg_unavailable_fork".into()), Delivery::Queue),
        )
        .await
        .unwrap();
    flow.settle(&parent).await;
    let state = flow.runtime.state(&parent).await.unwrap();
    assert_eq!(
        state.input("msg_unavailable_fork").unwrap().status,
        InputStatus::Pending
    );
    assert!(state.entries.is_empty());
    assert!(state.epoch.is_none());
    assert!(flow.runtime.jobs(Some(&parent)).unwrap().is_empty());
    assert!(flow.main.requests().is_empty());
    flow.runtime.shutdown().await;
}

#[tokio::test]
async fn replay_after_forwarding_before_delegation_uses_captured_body_scope_and_model() {
    use cyber_server::runtime::Delivery;
    let mut flow = Flow::with_models(
        support::Fixture::new(),
        vec![text("parent received captured summary")],
        false,
        Arc::new(NoSnapshots),
        vec![(
            "other/skill",
            vec![
                call(
                    "write",
                    "write",
                    json!({"path":"captured.txt","content":"captured evidence"}),
                ),
                text("captured summary"),
            ],
        )],
    );
    flow.f.set_config(json!({"permissions":{"edit":"ask"}}));
    let source = "---\nname: audit\ndescription: Audit changes\ncontext: fork\nmodel: other/skill\nallowed-tools: ['write:*']\n---\nCAPTURED BODY\n";
    flow.f.write(".cyber/skills/audit/SKILL.md", source);
    let parent = flow.session("default").await;
    let info = flow.runtime.state(&parent).await.unwrap().info;
    let turn = cyber_server::runtime::TurnContext {
        session_id: parent.clone(),
        directory: info.directory,
        agent: info.agent,
        mode: info.mode,
        prefers_apply_patch: false,
        rules: info.rules,
    };
    let plan = flow
        .f
        .host
        .command_plan(&turn, "audit", "")
        .await
        .unwrap()
        .unwrap();
    let mut admission = plan.admission(Some("msg_cut_fork".into()), Delivery::Queue);
    admission.resume = false;
    flow.runtime.admit_user(&parent, admission).await.unwrap();
    let seq = flow.runtime.state(&parent).await.unwrap().last_seq;
    // Restore the committed crash cut immediately before its independently owned delegation.
    flow.f
        .store
        .append(
            &parent,
            cyber_store::Expected::Seq(seq),
            vec![cyber_store::NewEvent::new(
                "session.prompt.skill_forwarded.1",
                json!({"message_id":"msg_cut_fork"}),
            )],
        )
        .unwrap();
    flow.f.write(
        ".cyber/skills/audit/SKILL.md",
        &source
            .replace("write:*", "read:*")
            .replace("CAPTURED BODY", "CHANGED BODY")
            .replace("other/skill", "test/main"),
    );
    flow.restart_default_runtime().await;
    flow.runtime.resume(&parent).await.unwrap();
    let job = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            if let Some(job) = flow.runtime.jobs(Some(&parent)).unwrap().into_iter().next() {
                break job;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(terminal(&flow, &job.id).await.status, JobStatus::Completed);
    flow.settle(&parent).await;
    assert_eq!(flow.f.read("captured.txt"), "captured evidence");
    let child = flow.runtime.state(&job.child_id).await.unwrap();
    assert_eq!(child.info.model, "other/skill");
    let entries = format!("{:?}", child.entries);
    assert!(entries.contains("CAPTURED BODY"));
    assert!(!entries.contains("CHANGED BODY"));
    assert_eq!(flow.main.requests().len(), 1);
    assert_eq!(flow.runtime.jobs(Some(&parent)).unwrap().len(), 1);
    flow.runtime.shutdown().await;
}

#[tokio::test]
async fn forked_steer_inherits_the_new_mode_at_the_safe_boundary() {
    use cyber_server::runtime::{Delivery, PermissionReply};
    let flow = Flow::with_models(
        support::Fixture::new(),
        vec![
            call("pause", "read", json!({"path":"pause.txt"})),
            text("parent finished"),
            text("parent summary"),
        ],
        true,
        Arc::new(NoSnapshots),
        vec![(
            "other/skill",
            vec![
                call(
                    "write",
                    "write",
                    json!({"path":"planned.txt","content":"must be denied"}),
                ),
                text("plan summary"),
            ],
        )],
    );
    flow.f.write("pause.txt", "pause");
    flow.f
        .set_config(json!({"permissions":{"read":{"pause.txt":"ask"}}}));
    flow.f.write(".cyber/skills/audit/SKILL.md","---\nname: audit\ndescription: Audit changes\ncontext: fork\nmodel: other/skill\nallowed-tools: ['write:*']\n---\nAudit the plan\n");
    let parent = flow.session("default").await;
    flow.prompt(&parent, "parent work").await;
    let question = flow.pending(&parent).await;
    flow.runtime.switch_mode(&parent, "plan").await.unwrap();
    let info = flow.runtime.state(&parent).await.unwrap().info;
    let turn = cyber_server::runtime::TurnContext {
        session_id: parent.clone(),
        directory: info.directory,
        agent: info.agent,
        mode: info.mode,
        prefers_apply_patch: false,
        rules: info.rules,
    };
    let plan = flow
        .f
        .host
        .command_plan(&turn, "audit", "")
        .await
        .unwrap()
        .unwrap();
    flow.runtime
        .admit_user(&parent, plan.admission(None, Delivery::Steer))
        .await
        .unwrap();
    flow.runtime
        .reply_permission(&question.id, PermissionReply::Once)
        .await
        .unwrap();
    let job = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            if let Some(job) = flow.runtime.jobs(Some(&parent)).unwrap().into_iter().next() {
                break job;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(terminal(&flow, &job.id).await.status, JobStatus::Completed);
    let child = flow.runtime.state(&job.child_id).await.unwrap();
    assert_eq!(child.info.mode, "plan");
    assert_eq!(
        child.calls["write"].status,
        cyber_server::runtime::CallStatus::Error
    );
    assert!(!flow.f.repo.join("planned.txt").exists());
    flow.runtime.shutdown().await;
}

#[tokio::test]
async fn plan_keeps_inline_skills_and_forked_read_only_delegation_available() {
    let fixture = support::Fixture::new();
    fixture.write(
        ".cyber/skills/inline/SKILL.md",
        "---\nname: inline\ndescription: Inline planning\n---\nInline plan instructions\n",
    );
    assert!(fixture.tool_names("plan", false).contains(&"skill".into()));
    assert!(
        support::ok(
            fixture
                .call("plan", "skill", json!({"name":"inline"}))
                .await
        )
        .contains("Inline plan instructions")
    );
    let mut calls = call(
        "read",
        "read",
        json!({"path":".cyber/skills/audit/notes.md"}),
    );
    let ending = calls.split_off(1);
    calls.push(
        call(
            "write",
            "write",
            json!({"path":"blocked.txt","content":"blocked"}),
        )
        .remove(0),
    );
    calls.extend(ending);
    let flow = Flow::with_models(
        support::Fixture::new(),
        vec![
            call("skill", "skill", json!({"name":"audit"})),
            text("parent planning"),
            text("parent summary"),
        ],
        false,
        Arc::new(NoSnapshots),
        vec![("other/skill", vec![calls, text("read-only findings")])],
    );
    flow.f
        .set_config(json!({"permissions":{"skill":"allow","agent":"allow","edit":"ask"}}));
    flow.f.write(".cyber/skills/audit/SKILL.md","---\nname: audit\ndescription: Audit changes\ncontext: fork\nmodel: other/skill\nallowed-tools: ['write:*']\n---\nAudit the plan\n");
    flow.f
        .write(".cyber/skills/audit/notes.md", "planning evidence");
    let parent = flow.session("plan").await;
    flow.prompt(&parent, "audit").await;
    flow.settle(&parent).await;
    let output = flow.output(&parent, "skill").await;
    let ack: serde_json::Value =
        serde_json::from_str(&output).unwrap_or_else(|error| panic!("{error}: {output}"));
    let job = terminal(&flow, ack["job_id"].as_str().unwrap()).await;
    assert_eq!(job.status, JobStatus::Completed);
    let child = flow.runtime.state(&job.child_id).await.unwrap();
    assert_eq!(child.info.mode, "plan");
    assert_eq!(
        child.calls["read"].status,
        cyber_server::runtime::CallStatus::Ok
    );
    assert_eq!(
        child.calls["write"].status,
        cyber_server::runtime::CallStatus::Error
    );
    assert!(!flow.f.repo.join("blocked.txt").exists());
    flow.runtime.shutdown().await;
}

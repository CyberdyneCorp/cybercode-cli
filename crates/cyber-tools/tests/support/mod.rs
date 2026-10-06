//! Shared fixtures: a temp repo, a file-backed store and a host with fixed config.

#![allow(dead_code)]

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use cyber_core::paths::DatabaseLocation;
use cyber_server::runtime::{Asker, Invocation, Runtime, ToolHost, ToolOutcome, TurnContext};
use cyber_store::{Store, StoreOptions};
use cyber_tools::{BuiltinHost, HostOptions};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

pub struct Fixture {
    pub dir: tempfile::TempDir,
    pub repo: PathBuf,
    pub store: Arc<Store>,
    pub host: Arc<BuiltinHost>,
    pub config: Arc<Mutex<Value>>,
    pub env: Arc<TestEnv>,
}

#[derive(Default)]
pub struct TestEnv(Mutex<HashMap<String, String>>);

impl TestEnv {
    pub fn set(&self, key: &str, value: &str) {
        self.0.lock().unwrap().insert(key.into(), value.into());
    }
}

impl cyber_core::env::EnvSource for TestEnv {
    fn get(&self, key: &str) -> Option<String> {
        self.0.lock().unwrap().get(key).cloned()
    }
}

impl Fixture {
    pub fn new() -> Self {
        Self::with_shell("bash", cyber_sandbox::find_helper())
    }

    pub fn with_shell(shell: &str, helper: Option<PathBuf>) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let repo = root.join("repo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        let store = Arc::new(
            Store::open(StoreOptions::new(
                DatabaseLocation::File(root.join("cyber.db")),
                Runtime::registry(),
            ))
            .unwrap(),
        );
        let config = Arc::new(Mutex::new(Value::Object(Default::default())));
        let shared = Arc::clone(&config);
        let env = Arc::new(TestEnv::default());
        let host = BuiltinHost::new(HostOptions {
            store: Arc::clone(&store),
            tool_output_dir: root.join("tool-output"),
            allowed_dirs: Vec::new(),
            home: root.join("home"),
            shell: shell.into(),
            config: Arc::new(move |_| Ok((shared.lock().unwrap().clone(), BTreeMap::new()))),
            global_config_dir: root.join("home/.config/cyber"),
            env: Arc::clone(&env) as Arc<dyn cyber_core::env::EnvSource + Send + Sync>,
            models: None,
            temp_dir: root.join("tmp"),
            sandbox_policy: None,
            sandbox_helper: helper,
        });
        Self {
            dir,
            repo,
            store,
            host,
            config,
            env,
        }
    }

    pub fn set_config(&self, value: Value) {
        *self.config.lock().unwrap() = value;
    }

    pub fn write(&self, rel: &str, content: &str) -> PathBuf {
        let path = self.repo.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, content).unwrap();
        path
    }

    pub fn read(&self, rel: &str) -> String {
        std::fs::read_to_string(self.repo.join(rel)).unwrap()
    }

    pub fn invocation(&self, mode: &str, name: &str, input: Value) -> Invocation {
        Invocation {
            session_id: "ses_test".into(),
            directory: self.repo.display().to_string(),
            agent: "build".into(),
            mode: mode.into(),
            message_id: "msg_1".into(),
            call_id: format!("call_{name}"),
            name: name.into(),
            input,
            attempt: 1,
            operation_key: "op".into(),
            asker: Asker::detached(),
            rules: Value::Null,
        }
    }

    /// Run one call with no interactive approver attached.
    pub async fn call(&self, mode: &str, name: &str, input: Value) -> ToolOutcome {
        self.host
            .execute(self.invocation(mode, name, input), CancellationToken::new())
            .await
    }

    pub fn tool_names(&self, mode: &str, prefers_patch: bool) -> Vec<String> {
        let turn = TurnContext {
            session_id: "ses_test".into(),
            directory: self.repo.display().to_string(),
            agent: "build".into(),
            mode: mode.into(),
            prefers_apply_patch: prefers_patch,
            rules: Value::Null,
        };
        self.host
            .definitions(&turn)
            .into_iter()
            .map(|d| d.spec.name)
            .collect()
    }
}

pub fn ok(outcome: ToolOutcome) -> String {
    match outcome {
        ToolOutcome::Ok(text) => text,
        other => panic!("expected Ok, got {other:?}"),
    }
}

pub fn failed(outcome: ToolOutcome) -> String {
    match outcome {
        ToolOutcome::Failed(text) => text,
        other => panic!("expected Failed, got {other:?}"),
    }
}

/// A minimal HTTP/1.1 server answering every request through `respond(path, head)`.
pub async fn serve<F>(respond: F) -> String
where
    F: Fn(&str, &str) -> (u16, Vec<(String, String)>, Vec<u8>) + Send + Sync + 'static,
{
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let respond = Arc::new(respond);
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            let respond = Arc::clone(&respond);
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    match socket.read(&mut chunk).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => buf.extend_from_slice(&chunk[..n]),
                    }
                }
                let head = String::from_utf8_lossy(&buf).to_string();
                let path = head.split_whitespace().nth(1).unwrap_or("/").to_string();
                let (status, headers, body) = respond(&path, &head.to_ascii_lowercase());
                let mut out = format!(
                    "HTTP/1.1 {status} X\r\ncontent-length: {}\r\nconnection: close\r\n",
                    body.len()
                );
                for (k, v) in headers {
                    out.push_str(&format!("{k}: {v}\r\n"));
                }
                out.push_str("\r\n");
                let _ = socket.write_all(out.as_bytes()).await;
                let _ = socket.write_all(&body).await;
            });
        }
    });
    format!("http://{addr}")
}

/// Normalize only the fixture's absolute checkout path; retain all output formatting.
pub fn golden(f: &Fixture, name: &str, output: &str) {
    let actual = output.replace(&f.repo.display().to_string(), "<repo>");
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/goldens")
        .join(format!("{name}.txt"));
    let expected = std::fs::read_to_string(path).unwrap();
    assert_eq!(
        actual,
        expected.strip_suffix('\n').unwrap_or(&expected),
        "{name}"
    );
}

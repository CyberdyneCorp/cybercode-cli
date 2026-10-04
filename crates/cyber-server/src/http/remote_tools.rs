//! Application-registered tools (`client-sdk` → Application-registered tools): tools that
//! run in a connected client. The server calls them over the client's JSON-RPC channel
//! and settles results through the standard tool boundary.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};

use cyber_llm::ToolSpec;
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use crate::runtime::{Invocation, RetrySafety, ToolDef, ToolOutcome};

struct Registration {
    def: ToolDef,
    owner: u64,
    out: mpsc::Sender<Value>,
}

#[derive(Default)]
pub struct RemoteTools {
    tools: Mutex<HashMap<String, Registration>>,
    pending: Mutex<HashMap<String, oneshot::Sender<Result<String, String>>>>,
    next_owner: AtomicU64,
}

/// `^[A-Za-z][A-Za-z0-9_-]{0,63}$`.
fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && name.len() <= 64
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

impl RemoteTools {
    /// A new channel owner; its registrations end with [`RemoteTools::release`].
    pub fn owner(&self) -> u64 {
        self.next_owner.fetch_add(1, Ordering::Relaxed) + 1
    }

    pub fn register(
        &self,
        owner: u64,
        out: mpsc::Sender<Value>,
        params: &Value,
        reserved: &[String],
    ) -> Result<Value, String> {
        let name = params["name"].as_str().unwrap_or_default();
        if !valid_name(name) {
            return Err(format!(
                "invalid tool name {name:?}: use ^[A-Za-z][A-Za-z0-9_-]{{0,63}}$"
            ));
        }
        if reserved.iter().any(|r| r == name) {
            return Err(format!("{name} is a built-in tool"));
        }
        let mut tools = self.tools.lock().unwrap_or_else(PoisonError::into_inner);
        if tools.get(name).is_some_and(|r| r.owner != owner) {
            return Err(format!("{name} is already registered by another client"));
        }
        let spec = ToolSpec {
            name: name.into(),
            description: params["description"].as_str().unwrap_or_default().into(),
            input_schema: params
                .get("input")
                .cloned()
                .filter(Value::is_object)
                .unwrap_or_else(|| json!({ "type": "object" })),
        };
        let def = ToolDef {
            spec,
            retry_safety: RetrySafety::Never,
            concurrency_safe: false,
        };
        tools.insert(name.into(), Registration { def, owner, out });
        Ok(json!({ "registered": name }))
    }

    /// Drop every tool a disconnected client registered; its calls in flight fail.
    pub fn release(&self, owner: u64) {
        self.tools
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|_, r| r.owner != owner);
    }

    pub fn release_one(&self, owner: u64, name: &str) {
        self.tools
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|n, r| !(r.owner == owner && n == name));
    }

    pub fn definitions(&self) -> Vec<ToolDef> {
        let tools = self.tools.lock().unwrap_or_else(PoisonError::into_inner);
        let mut defs: Vec<ToolDef> = tools.values().map(|r| r.def.clone()).collect();
        defs.sort_by(|a, b| a.spec.name.cmp(&b.spec.name));
        defs
    }

    pub fn has(&self, name: &str) -> bool {
        self.tools
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains_key(name)
    }

    /// Run a call in the registering client.
    pub async fn execute(&self, inv: Invocation, cancel: CancellationToken) -> ToolOutcome {
        let Some(out) = self
            .tools
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&inv.name)
            .map(|r| r.out.clone())
        else {
            return ToolOutcome::Failed(format!("Unknown tool: {}", inv.name));
        };
        let request = cyber_core::ids::new_id("rpc");
        let (tx, rx) = oneshot::channel();
        self.pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(request.clone(), tx);
        let call = json!({
            "jsonrpc": "2.0", "id": request, "method": "tool.execute",
            "params": { "name": inv.name, "input": inv.input, "session_id": inv.session_id, "call_id": inv.call_id },
        });
        if out.send(call).await.is_err() {
            self.pending
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .remove(&request);
            return ToolOutcome::Failed("the client that registered this tool disconnected".into());
        }
        let outcome = tokio::select! {
            _ = cancel.cancelled() => ToolOutcome::Aborted,
            r = rx => match r {
                Ok(Ok(text)) => ToolOutcome::Ok(text),
                Ok(Err(message)) => ToolOutcome::Failed(message),
                Err(_) => ToolOutcome::Failed("the client that registered this tool disconnected".into()),
            },
        };
        self.pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&request);
        outcome
    }

    /// A client's JSON-RPC response to `tool.execute`.
    pub fn resolve(&self, frame: &Value) {
        let Some(id) = frame["id"].as_str() else {
            return;
        };
        let Some(tx) = self
            .pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(id)
        else {
            return;
        };
        let result = match (&frame["result"], &frame["error"]) {
            (_, Value::Object(e)) => Err(e
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("tool failed")
                .to_string()),
            (Value::String(s), _) => Ok(s.clone()),
            (Value::Null, _) => Ok(String::new()),
            (other, _) => Ok(other.to_string()),
        };
        let _ = tx.send(result);
    }
}

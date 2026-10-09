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
        if name == "return_result" {
            return Err("return_result is a runtime-owned tool".into());
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
            scope: crate::runtime::ToolScope::Session,
            registration: Some(cyber_core::ids::new_id("reg")),
            spec,
            retry_safety: RetrySafety::Never,
            concurrency_safe: false,
        };
        let identity = def.registration.clone();
        tools.insert(name.into(), Registration { def, owner, out });
        Ok(json!({ "registered": name, "registration_id": identity }))
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
            .filter(|r| r.def.registration == inv.registration)
            .map(|r| r.out.clone())
        else {
            return ToolOutcome::Failed(format!("Stale tool call: {}", inv.name));
        };
        let permit = tokio::select! {
            _ = cancel.cancelled() => return ToolOutcome::Aborted,
            permit = out.reserve_owned() => match permit {
                Ok(permit) => permit,
                Err(_) => return ToolOutcome::Failed("the client that registered this tool disconnected".into()),
            },
        };
        let request = cyber_core::ids::new_id("rpc");
        let (tx, rx) = oneshot::channel();
        let call = json!({
            "jsonrpc": "2.0", "id": request, "method": "tool.execute",
            "params": { "name": inv.name, "registration_id": inv.registration, "input": inv.input, "session_id": inv.session_id, "call_id": inv.call_id },
        });
        {
            let tools = self.tools.lock().unwrap_or_else(PoisonError::into_inner);
            if cancel.is_cancelled() {
                return ToolOutcome::Aborted;
            }
            if !tools
                .get(&inv.name)
                .is_some_and(|registration| registration.def.registration == inv.registration)
            {
                return ToolOutcome::Failed(format!("Stale tool call: {}", inv.name));
            }
            self.pending
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(request.clone(), tx);
            permit.send(call);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::Asker;

    fn invocation(registration: Option<String>) -> Invocation {
        Invocation {
            registration,
            session_id: "ses_test".into(),
            directory: "/tmp".into(),
            agent: "build".into(),
            mode: "default".into(),
            message_id: "msg_test".into(),
            call_id: "call_test".into(),
            name: "mcp__shared__read".into(),
            input: json!({}),
            attempt: 1,
            operation_key: "op_test".into(),
            asker: Asker::detached(),
            rules: Value::Null,
        }
    }

    #[tokio::test]
    async fn replacement_client_registration_cannot_receive_an_old_native_binding() {
        let tools = RemoteTools::default();
        let (out, mut frames) = mpsc::channel(1);
        let owner = tools.owner();
        tools
            .register(
                owner,
                out.clone(),
                &json!({"name":"mcp__shared__read"}),
                &[],
            )
            .unwrap();
        let old = tools.definitions()[0].registration.clone();
        tools
            .register(owner, out, &json!({"name":"mcp__shared__read"}), &[])
            .unwrap();
        assert_ne!(tools.definitions()[0].registration, old);
        let invocation = invocation(old);
        assert_eq!(
            tools.execute(invocation, CancellationToken::new()).await,
            ToolOutcome::Failed("Stale tool call: mcp__shared__read".into())
        );
        assert_eq!(frames.try_recv(), Err(mpsc::error::TryRecvError::Empty));
    }
    #[tokio::test]
    async fn queued_client_dispatch_rechecks_its_registration_before_channel_effects() {
        let tools = RemoteTools::default();
        let (out, mut frames) = mpsc::channel(1);
        let owner = tools.owner();
        tools
            .register(
                owner,
                out.clone(),
                &json!({"name":"mcp__shared__read"}),
                &[],
            )
            .unwrap();
        out.send(json!({"occupied":true})).await.unwrap();
        let running = tools.execute(
            invocation(tools.definitions()[0].registration.clone()),
            CancellationToken::new(),
        );
        tokio::pin!(running);
        assert!(futures::poll!(&mut running).is_pending());
        tools
            .register(owner, out, &json!({"name":"mcp__shared__read"}), &[])
            .unwrap();
        frames.recv().await.unwrap();
        assert_eq!(
            running.await,
            ToolOutcome::Failed("Stale tool call: mcp__shared__read".into())
        );
        assert_eq!(frames.try_recv(), Err(mpsc::error::TryRecvError::Empty));
        assert!(tools.pending.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn queued_client_dispatch_cancellation_does_not_enqueue_or_leave_pending_ownership() {
        let tools = RemoteTools::default();
        let (out, mut frames) = mpsc::channel(1);
        tools
            .register(
                tools.owner(),
                out.clone(),
                &json!({"name":"mcp__shared__read"}),
                &[],
            )
            .unwrap();
        out.send(json!({"occupied":true})).await.unwrap();
        let cancel = CancellationToken::new();
        let running = tools.execute(
            invocation(tools.definitions()[0].registration.clone()),
            cancel.clone(),
        );
        tokio::pin!(running);
        assert!(futures::poll!(&mut running).is_pending());
        cancel.cancel();
        assert_eq!(running.await, ToolOutcome::Aborted);
        assert!(tools.pending.lock().unwrap().is_empty());
        frames.recv().await.unwrap();
        assert_eq!(frames.try_recv(), Err(mpsc::error::TryRecvError::Empty));
    }
    #[test]
    fn client_registration_scope_comes_from_authority_not_name_or_payload() {
        let tools = RemoteTools::default();
        let (out, _frames) = mpsc::channel(1);
        tools
            .register(
                tools.owner(),
                out,
                &json!({"name":"mcp__shared__read","scope":"mcp","registration_id":"mcs_forged"}),
                &[],
            )
            .unwrap();
        let definition = tools.definitions().pop().unwrap();
        assert_eq!(definition.scope, crate::runtime::ToolScope::Session);
        assert!(!definition.scope.deferrable());
        assert_ne!(definition.registration.as_deref(), Some("mcs_forged"));
    }
}

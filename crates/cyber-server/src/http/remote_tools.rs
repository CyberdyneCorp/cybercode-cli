//! Application-registered tools (`client-sdk` → Application-registered tools): tools that
//! run in a connected client. The server calls them over the client's JSON-RPC channel
//! and settles results through the standard tool boundary.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};

use cyber_llm::ToolSpec;
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use crate::runtime::{ClientHook, ClientHooks, Invocation, RetrySafety, ToolDef, ToolOutcome};

struct Registration {
    def: ToolDef,
    owner: u64,
    out: mpsc::Sender<Value>,
}

/// A client's hook handler (`client-sdk` → Hook callbacks).
struct HookRegistration {
    hook: ClientHook,
    owner: u64,
    out: mpsc::Sender<Value>,
}

/// Where a client's answer to one request goes.
type Answer = oneshot::Sender<Result<String, String>>;

#[derive(Default)]
pub struct RemoteTools {
    tools: Mutex<HashMap<String, Registration>>,
    /// Hook handlers in registration order; they end with their client's channel.
    hooks: Mutex<Vec<HookRegistration>>,
    /// The Sessions each client owns for its hook handlers (`v1.hook.attach`, or created
    /// over its channel).
    sessions: Mutex<HashMap<u64, HashSet<String>>>,
    /// Requests awaiting a client's answer, with the client that must answer them.
    pending: Mutex<HashMap<String, (u64, Answer)>>,
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
            deferred: false,
            registration: Some(cyber_core::ids::new_id("reg")),
            spec,
            retry_safety: RetrySafety::Never,
            concurrency_safe: false,
        };
        let identity = def.registration.clone();
        tools.insert(name.into(), Registration { def, owner, out });
        Ok(json!({ "registered": name, "registration_id": identity }))
    }

    /// Drop every tool, hook and Session a disconnected client registered; its calls and
    /// hook executions in flight fail as disconnected at once.
    pub fn release(&self, owner: u64) {
        self.tools
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|_, r| r.owner != owner);
        self.hooks
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|r| r.owner != owner);
        self.sessions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&owner);
        // Dropping the senders resolves each waiting receiver with an error.
        self.pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|_, (waiting_on, _)| *waiting_on != owner);
    }

    /// `v1.hook.attach` `{ session_id }`: this client's hook handlers apply to the Session
    /// and its descendants.
    pub fn attach_session(&self, owner: u64, params: &Value) -> Result<Value, String> {
        let session = params["session_id"].as_str().unwrap_or_default();
        if session.is_empty() {
            return Err("session_id: expected a Session ID".into());
        }
        self.sessions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(owner)
            .or_default()
            .insert(session.to_owned());
        Ok(json!({ "attached": session }))
    }

    /// `v1.hook.register` `{ event, matcher?, timeout? }`: a hook handler served by this
    /// client until it unregisters or disconnects.
    pub fn register_hook(
        &self,
        owner: u64,
        out: mpsc::Sender<Value>,
        params: &Value,
    ) -> Result<Value, String> {
        let event = params["event"].as_str().unwrap_or_default();
        let matcher = match &params["matcher"] {
            Value::Null => None,
            Value::String(matcher) => Some(matcher.clone()),
            _ => return Err("matcher: expected a string".into()),
        };
        let timeout = match &params["timeout"] {
            Value::Null => 60,
            value => value
                .as_u64()
                .and_then(|timeout| u32::try_from(timeout).ok())
                .ok_or("timeout: expected 1 through 600 seconds")?,
        };
        let id = cyber_core::ids::new_id("hkr");
        // Validates the event, matcher and timeout exactly as dispatch will use them.
        cyber_core::hooks::HookDefinition::client(event, &id, matcher.clone(), timeout)?;
        let hook = ClientHook {
            id: id.clone(),
            event: event.into(),
            matcher,
            timeout,
        };
        self.hooks
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(HookRegistration { hook, owner, out });
        Ok(json!({ "registration_id": id, "event": event }))
    }

    /// `v1.hook.unregister` `{ registration_id }` for one of this client's handlers.
    pub fn unregister_hook(&self, owner: u64, params: &Value) -> Value {
        let id = params["registration_id"].as_str().unwrap_or_default();
        let mut hooks = self.hooks.lock().unwrap_or_else(PoisonError::into_inner);
        let before = hooks.len();
        hooks.retain(|r| !(r.owner == owner && r.hook.id == id));
        json!({ "unregistered": hooks.len() != before })
    }

    async fn run_client_hook(
        &self,
        registration: &str,
        envelope: Value,
        timeout: std::time::Duration,
        cancel: CancellationToken,
    ) -> Result<Value, String> {
        let request = cyber_core::ids::new_id("rpc");
        let (tx, rx) = oneshot::channel();
        let (out, event) = {
            // Under the hooks lock, so a concurrent release cannot miss this request.
            let hooks = self.hooks.lock().unwrap_or_else(PoisonError::into_inner);
            let found = hooks
                .iter()
                .find(|r| r.hook.id == registration)
                .ok_or("the client that registered this hook disconnected")?;
            self.pending
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(request.clone(), (found.owner, tx));
            (found.out.clone(), found.hook.event.clone())
        };
        let call = json!({
            "jsonrpc": "2.0", "id": request, "method": "hook/execute",
            "params": { "registration_id": registration, "event": event, "input": envelope },
        });
        let answer = async {
            out.send(call)
                .await
                .map_err(|_| "the client that registered this hook disconnected".to_string())?;
            match rx.await {
                Ok(Ok(text)) if text.is_empty() => Ok(json!({})),
                Ok(Ok(text)) => serde_json::from_str(&text)
                    .map_err(|_| "client hook returned invalid JSON".to_string()),
                Ok(Err(message)) => Err(format!("client hook failed: {message}")),
                Err(_) => Err("the client that registered this hook disconnected".into()),
            }
        };
        let result = tokio::select! {
            _ = cancel.cancelled() => Err("client hook cancelled".into()),
            _ = tokio::time::sleep(timeout) => Err("client hook timed out".into()),
            result = answer => result,
        };
        self.pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&request);
        result
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
        let Some((owner, out)) = self
            .tools
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&inv.name)
            .filter(|r| r.def.registration == inv.registration)
            .map(|r| (r.owner, r.out.clone()))
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
                .insert(request.clone(), (owner, tx));
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
        let Some((_, tx)) = self
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

impl ClientHooks for RemoteTools {
    fn client_hooks(&self, sessions: &[String]) -> Vec<ClientHook> {
        let owners: HashSet<u64> = self
            .sessions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .filter(|(_, owned)| sessions.iter().any(|id| owned.contains(id)))
            .map(|(owner, _)| *owner)
            .collect();
        self.hooks
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .filter(|r| owners.contains(&r.owner))
            .map(|r| r.hook.clone())
            .collect()
    }

    fn execute_client_hook<'a>(
        &'a self,
        registration: &'a str,
        envelope: Value,
        timeout: std::time::Duration,
        cancel: CancellationToken,
    ) -> futures::future::BoxFuture<'a, Result<Value, String>> {
        Box::pin(self.run_client_hook(registration, envelope, timeout, cancel))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::Asker;

    fn invocation(registration: Option<String>) -> Invocation {
        Invocation {
            tool_output: Default::default(),
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

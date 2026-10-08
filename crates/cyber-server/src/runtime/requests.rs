//! Permission and question requests (`permissions-modes` → Replies and approvals,
//! Permission requests, routes and events, Non-interactive behavior).
//!
//! A tool asks through its [`Asker`]; the request is recorded durably and waits until an
//! attached client replies. Without an interactive client the reply is `Unattended` at once,
//! so non-interactive Sessions never block.

use std::sync::{Arc, PoisonError, Weak};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::oneshot;

use super::events::*;
use super::{Inner, Runtime, RuntimeError};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct PermissionAsk {
    pub action: String,
    pub resources: Vec<String>,
    /// Patterns saved by an `always` reply.
    pub always_patterns: Vec<String>,
    /// Display data such as a diff or the command.
    #[serde(default)]
    pub metadata: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "reply", rename_all = "snake_case")]
pub enum PermissionReply {
    Once,
    Always,
    /// Without a message the current Drain halts; with one, the model receives it.
    Reject {
        message: Option<String>,
    },
    /// No interactive client is attached; the caller resolves by Mode.
    Unattended,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct QuestionOption {
    pub label: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Question {
    pub question: String,
    pub header: String,
    pub options: Vec<QuestionOption>,
    #[serde(default)]
    pub multi_select: bool,
    #[serde(default = "yes")]
    pub allow_custom: bool,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "reply", rename_all = "snake_case")]
pub enum QuestionReply {
    /// One list of selected labels (or custom text) per question.
    Answers {
        answers: Vec<Vec<String>>,
    },
    /// Dismissal halts the Drain.
    Dismissed,
    Unattended,
}

/// Server-derived child identity for approval prompts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RequestOrigin {
    pub title: String,
    pub agent: String,
    pub directory: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RequestRoute {
    pub session_id: String,
    pub directory: String,
}

/// A request waiting for a client. Its Session remains the owner of replies.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct PendingRequest {
    pub id: String,
    pub session_id: String,
    pub call_id: String,
    pub message_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<RequestOrigin>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub routed_to: Vec<RequestRoute>,
    #[serde(flatten)]
    pub kind: PendingKind,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PendingKind {
    Permission(PermissionAsk),
    Question { questions: Vec<Question> },
}

enum Reply {
    Permission(oneshot::Sender<PermissionReply>),
    Question(oneshot::Sender<QuestionReply>),
}

pub(crate) struct Waiter {
    request: PendingRequest,
    reply: Reply,
}

/// The per-call handle tools use to ask the user.
#[derive(Clone)]
pub struct Asker {
    pub(super) inner: Weak<Inner>,
    pub session_id: String,
    pub call_id: String,
    pub message_id: String,
    agent: String,
    pub(super) auto_override: Option<super::auto_override::Grant>,
}

impl std::fmt::Debug for Asker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Asker")
            .field("session_id", &self.session_id)
            .field("call_id", &self.call_id)
            .finish()
    }
}

impl Asker {
    pub(crate) fn new(
        inner: &Arc<Inner>,
        session_id: &str,
        call_id: &str,
        message_id: &str,
        agent: &str,
    ) -> Self {
        Self {
            inner: Arc::downgrade(inner),
            session_id: session_id.into(),
            call_id: call_id.into(),
            message_id: message_id.into(),
            agent: agent.into(),
            auto_override: None,
        }
    }

    /// An asker with no runtime behind it; every request is unattended.
    pub fn detached() -> Self {
        Self {
            inner: Weak::new(),
            session_id: String::new(),
            call_id: String::new(),
            message_id: String::new(),
            agent: String::new(),
            auto_override: None,
        }
    }

    /// Whether a client can answer requests; when false every request resolves `Unattended`.
    pub fn attended(&self) -> bool {
        self.inner
            .upgrade()
            .is_some_and(|inner| inner.options.interactive)
    }

    pub async fn permission(&self, ask: PermissionAsk) -> PermissionReply {
        let Some(inner) = self.inner.upgrade() else {
            return PermissionReply::Unattended;
        };
        let (tx, rx) = oneshot::channel();
        if !inner
            .open_request(self, PendingKind::Permission(ask), Reply::Permission(tx))
            .await
        {
            return PermissionReply::Unattended;
        }
        rx.await.unwrap_or(PermissionReply::Reject {
            message: Some("the request was abandoned".into()),
        })
    }

    pub async fn question(&self, questions: Vec<Question>) -> QuestionReply {
        let Some(inner) = self.inner.upgrade() else {
            return QuestionReply::Unattended;
        };
        let (tx, rx) = oneshot::channel();
        if !inner
            .open_request(
                self,
                PendingKind::Question { questions },
                Reply::Question(tx),
            )
            .await
        {
            return QuestionReply::Unattended;
        }
        rx.await.unwrap_or(QuestionReply::Dismissed)
    }
}

impl Inner {
    pub(super) async fn abandon_call_requests(
        &self,
        session: &str,
        call: &str,
    ) -> Result<(), RuntimeError> {
        for request in self
            .owned_pending(session)
            .into_iter()
            .filter(|r| r.call_id == call)
        {
            if let Some(waiter) = self.take_waiter(&request.id)
                && let Reply::Permission(sender) = waiter.reply
            {
                let reply = PermissionReply::Reject {
                    message: Some("The operation was cancelled".into()),
                };
                self.record_reply(&request, &reply).await?;
                let _ = sender.send(reply);
            }
        }
        Ok(())
    }
    /// Record and register a request. Returns false when nobody can answer it.
    async fn open_request(
        self: &Arc<Self>,
        asker: &Asker,
        kind: PendingKind,
        reply: Reply,
    ) -> bool {
        if !self.options.interactive {
            return false;
        }
        let mut request = PendingRequest {
            id: cyber_core::ids::new_id(if matches!(kind, PendingKind::Permission(_)) {
                "per"
            } else {
                "que"
            }),
            session_id: asker.session_id.clone(),
            call_id: asker.call_id.clone(),
            message_id: asker.message_id.clone(),
            origin: None,
            routed_to: Vec::new(),
            kind,
        };
        let Ok(handle) = self.handle(&asker.session_id).await else {
            return false;
        };
        let info = handle.state.lock().await.info.clone();
        let runtime = super::Runtime {
            inner: self.clone(),
        };
        request.routed_to = match runtime.ancestors(&info).await {
            Ok(ancestors) => ancestors
                .into_iter()
                .map(|info| RequestRoute {
                    session_id: info.id,
                    directory: request_directory(&info.directory),
                })
                .collect(),
            Err(e) => {
                self.bus.publish(super::LiveEvent::Error {
                    session_id: asker.session_id.clone(),
                    kind: "ancestry".into(),
                    message: format!("could not route a child request: {e}"),
                });
                return false;
            }
        };
        if !request.routed_to.is_empty() {
            request.origin = Some(RequestOrigin {
                title: match info.subagent_name {
                    Some(name) => format!("{name}: {}", info.title),
                    None => info.title,
                },
                agent: asker.agent.clone(),
                directory: request_directory(&info.directory),
            });
        }
        let kind_name = if matches!(request.kind, PendingKind::Permission(_)) {
            PERMISSION_ASKED
        } else {
            QUESTION_ASKED
        };
        let mut state = handle.state.lock().await;
        let stored = match self.record_locked(&mut state, vec![event(kind_name, &request)]) {
            Ok(stored) => stored,
            Err(e) => {
                // A request that cannot be recorded is never shown; say why instead of failing silently.
                self.bus.publish(super::LiveEvent::Error {
                    session_id: asker.session_id.clone(),
                    kind: "storage".into(),
                    message: format!("could not record a {kind_name} request: {e}"),
                });
                return false;
            }
        };
        self.waiters
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(Waiter {
                request: request.clone(),
                reply,
            });
        // Only publish after durable storage and reply registration both succeeded.
        self.publish(&stored);
        self.route_request_event(&request, kind_name, serde_json::to_value(&request).unwrap());
        true
    }

    pub(crate) async fn abandon_requests(&self, session: &str) -> Result<(), RuntimeError> {
        for request in self.owned_pending(session) {
            if let Some(waiter) = self.take_waiter(&request.id) {
                match waiter.reply {
                    Reply::Permission(sender) => {
                        let reply = PermissionReply::Reject { message: None };
                        self.record_reply(&request, &reply).await?;
                        let _ = sender.send(reply);
                    }
                    Reply::Question(sender) => {
                        self.record_question_reply(&request, &QuestionReply::Dismissed)
                            .await?;
                        let _ = sender.send(QuestionReply::Dismissed);
                    }
                }
            }
        }
        Ok(())
    }

    pub(crate) fn pending(&self, session_id: Option<&str>) -> Vec<PendingRequest> {
        self.waiters
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .filter(|w| {
                session_id.is_none_or(|s| {
                    w.request.session_id == s
                        || w.request
                            .routed_to
                            .iter()
                            .any(|route| route.session_id == s)
                })
            })
            .map(|w| w.request.clone())
            .collect()
    }

    /// Cascades apply to the request owner, independently of ancestor visibility.
    fn owned_pending(&self, session_id: &str) -> Vec<PendingRequest> {
        self.waiters
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .filter(|w| w.request.session_id == session_id)
            .map(|w| w.request.clone())
            .collect()
    }

    fn take_waiter(&self, id: &str) -> Option<Waiter> {
        let mut waiters = self.waiters.lock().unwrap_or_else(PoisonError::into_inner);
        let index = waiters.iter().position(|w| w.request.id == id)?;
        Some(waiters.remove(index))
    }

    /// Apply a permission reply and its cascades.
    pub(crate) async fn reply_permission(
        &self,
        id: &str,
        reply: PermissionReply,
    ) -> Result<(), RuntimeError> {
        let waiter = self
            .take_waiter(id)
            .ok_or_else(|| RuntimeError::Invalid(format!("no pending request {id}")))?;
        let PendingKind::Permission(ask) = &waiter.request.kind else {
            return Err(RuntimeError::Invalid(format!("{id} is a question")));
        };
        let session_id = waiter.request.session_id.clone();
        let ask = ask.clone();
        self.record_reply(&waiter.request, &reply).await?;
        if let Reply::Permission(tx) = waiter.reply {
            let _ = tx.send(reply.clone());
        }
        match &reply {
            PermissionReply::Reject { message } => {
                if message.is_none() {
                    self.halt(&session_id).await;
                }
                self.decline_all(&session_id).await?;
            }
            PermissionReply::Always => self.approve_covered(&session_id, &ask).await?,
            _ => {}
        }
        Ok(())
    }

    pub(crate) async fn answer_question(
        &self,
        id: &str,
        reply: QuestionReply,
    ) -> Result<(), RuntimeError> {
        let waiter = self
            .take_waiter(id)
            .ok_or_else(|| RuntimeError::Invalid(format!("no pending request {id}")))?;
        if matches!(reply, QuestionReply::Dismissed) {
            self.halt(&waiter.request.session_id).await;
        }
        self.record_question_reply(&waiter.request, &reply).await?;
        match waiter.reply {
            Reply::Question(tx) => {
                let _ = tx.send(reply);
                Ok(())
            }
            Reply::Permission(_) => Err(RuntimeError::Invalid(format!(
                "{id} is a permission request"
            ))),
        }
    }

    async fn record_reply(
        &self,
        request: &PendingRequest,
        reply: &PermissionReply,
    ) -> Result<(), RuntimeError> {
        let handle = self.handle(&request.session_id).await?;
        let payload = serde_json::json!({ "request_id": request.id, "call_id": request.call_id, "reply": reply });
        self.commit(
            &handle,
            vec![cyber_store::NewEvent::new(
                PERMISSION_REPLIED,
                payload.clone(),
            )],
        )
        .await?;
        self.route_request_event(request, PERMISSION_REPLIED, payload);
        Ok(())
    }

    async fn record_question_reply(
        &self,
        request: &PendingRequest,
        reply: &QuestionReply,
    ) -> Result<(), RuntimeError> {
        let handle = self.handle(&request.session_id).await?;
        let payload = serde_json::json!({ "request_id": request.id, "call_id": request.call_id, "reply": reply });
        self.commit(
            &handle,
            vec![cyber_store::NewEvent::new(
                QUESTION_REPLIED,
                payload.clone(),
            )],
        )
        .await?;
        self.route_request_event(request, QUESTION_REPLIED, payload);
        Ok(())
    }

    fn route_request_event(&self, request: &PendingRequest, kind: &str, data: Value) {
        let mut data = data;
        if let Some(object) = data.as_object_mut() {
            object.insert(
                "session_id".into(),
                Value::String(request.session_id.clone()),
            );
            object.insert(
                "routed_to".into(),
                serde_json::to_value(&request.routed_to).unwrap(),
            );
        }
        for route in &request.routed_to {
            self.bus.publish(super::LiveEvent::RequestRouted {
                session_id: route.session_id.clone(),
                kind: kind.into(),
                data: data.clone(),
            });
        }
    }

    /// Any reject declines every other pending request of the Session.
    async fn decline_all(&self, session_id: &str) -> Result<(), RuntimeError> {
        let ids: Vec<String> = self
            .owned_pending(session_id)
            .into_iter()
            .map(|r| r.id)
            .collect();
        for id in ids {
            let Some(waiter) = self.take_waiter(&id) else {
                continue;
            };
            match waiter.reply {
                Reply::Permission(tx) => {
                    let reply = PermissionReply::Reject {
                        message: Some("Declined because another request was rejected".into()),
                    };
                    self.record_reply(&waiter.request, &reply).await?;
                    let _ = tx.send(reply);
                }
                Reply::Question(tx) => {
                    self.record_question_reply(&waiter.request, &QuestionReply::Dismissed)
                        .await?;
                    let _ = tx.send(QuestionReply::Dismissed);
                }
            }
        }
        Ok(())
    }

    /// An `always` reply approves pending requests its patterns now fully allow.
    async fn approve_covered(
        &self,
        session_id: &str,
        approved: &PermissionAsk,
    ) -> Result<(), RuntimeError> {
        let covered: Vec<String> = self
            .owned_pending(session_id)
            .into_iter()
            .filter(|r| match &r.kind {
                PendingKind::Permission(ask) => {
                    !ask.metadata["requires_confirmation"]
                        .as_bool()
                        .unwrap_or(false)
                        && ask.action == approved.action
                        && ask.resources.iter().all(|res| {
                            approved
                                .always_patterns
                                .iter()
                                .any(|p| cyber_core::wildcard::matches(p, res))
                        })
                }
                PendingKind::Question { .. } => false,
            })
            .map(|r| r.id)
            .collect();
        for id in covered {
            let Some(waiter) = self.take_waiter(&id) else {
                continue;
            };
            self.record_reply(&waiter.request, &PermissionReply::Once)
                .await?;
            if let Reply::Permission(tx) = waiter.reply {
                let _ = tx.send(PermissionReply::Once);
            }
        }
        Ok(())
    }

    /// Ask the running Drain to stop after the current tool group.
    async fn halt(&self, session_id: &str) {
        if let Ok(handle) = self.handle(session_id).await {
            handle.halt.store(true, std::sync::atomic::Ordering::SeqCst);
        }
    }
}

fn request_directory(directory: &str) -> String {
    std::fs::canonicalize(directory)
        .map_or_else(|_| directory.into(), |path| path.display().to_string())
}

impl Runtime {
    /// A trusted host operation can ask after inference settles, retaining Session routing.
    pub async fn operation_asker(&self, session_id: &str) -> Result<Asker, RuntimeError> {
        if self.inner.closed.is_cancelled() {
            return Err(RuntimeError::ShuttingDown);
        }
        let handle = self.inner.handle(session_id).await?;
        let info = handle.state.lock().await.info.clone();
        Ok(Asker::new(
            &self.inner,
            session_id,
            &cyber_core::ids::new_id("call"),
            "",
            &info.agent,
        ))
    }
}

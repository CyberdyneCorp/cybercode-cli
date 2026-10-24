//! Agent operations compute project and author from the local immutable publication.
use super::*;
use cyber_collab::model::{Agent, Message, Task};
impl Collaboration {
    pub(super) async fn replay_agent(
        &self,
        publication: &Publication,
        binding: &Binding,
    ) -> Result<(), ApiError> {
        let identity = serde_json::to_string(&(
            &publication.issuer,
            &publication.subject,
            "agent",
            &publication.agent_id,
        ))
        .map_err(|_| error("invalid agent identity"))?;
        let pending = self.local.pending(identity).map_err(storage)?;
        if pending.is_empty() {
            return Ok(());
        }
        let agent = publication
            .agent_id
            .as_deref()
            .ok_or_else(|| error("agent publication unavailable"))?;
        let token = self
            .secrets
            .get(&format!(
                "{}:{}:{agent}",
                publication.hub_id, publication.project
            ))
            .await
            .map_err(|_| error("protected capability unavailable"))?
            .ok_or_else(|| error("agent capability unavailable"))?;
        let client = HubClient::new(&binding.hub_url, token).map_err(hub_error)?;
        for (operation_id, hub, project, body) in pending {
            if hub != publication.hub_id
                || project != publication.project
                || self.suspended.load(Ordering::Acquire)
            {
                continue;
            }
            let method = Method::from_bytes(field(&body, "method")?.as_bytes())
                .map_err(|_| error("invalid pending operation"))?;
            let path = field(&body, "path")?;
            let mutation: Mutation<Value> = serde_json::from_value(
                body.get("mutation")
                    .cloned()
                    .ok_or_else(|| error("invalid pending operation"))?,
            )
            .map_err(|_| error("invalid pending operation"))?;
            let result: Result<MutationResult<Value>, _> =
                client.mutate(method, path, &mutation).await;
            match result {
                Ok(_) => self.local.complete(operation_id).map_err(storage)?,
                Err(e) if retryable_hub(&e) => return Err(hub_error(e)),
                Err(e) => {
                    self.local
                        .reject(operation_id, e.to_string())
                        .map_err(storage)?;
                    if let Ok(mut error) = self.last_error.lock() {
                        *error = Some(e.to_string());
                    }
                }
            }
        }
        Ok(())
    }

    pub(super) async fn agent_request(
        &self,
        session: String,
        operation: String,
        mut input: Value,
    ) -> Result<Value, ApiError> {
        if !self.enabled() || self.suspended.load(Ordering::Acquire) {
            return Err(ApiError::forbidden(
                "collaboration is disabled or logged out",
            ));
        }
        let publication = self
            .local
            .publication(&session)
            .map_err(storage)?
            .filter(|p| p.active)
            .ok_or_else(|| ApiError::forbidden("Session is not published"))?;
        let binding = self
            .binding(&json!({"session_id":session,"project":publication.project}))
            .await?;
        if binding.hub_id != publication.hub_id {
            return Err(error("published Session binding changed"));
        }
        let account = self
            .accounts
            .current()
            .await?
            .account
            .ok_or_else(ApiError::unauthorized)?;
        if account.issuer != publication.issuer || account.sub != publication.subject {
            return Err(ApiError::forbidden(
                "published Session belongs to another account",
            ));
        }
        let capability = self
            .secrets
            .get(&format!(
                "{}:{}:{}",
                publication.hub_id,
                publication.project,
                publication.agent_id.as_deref().unwrap_or_default()
            ))
            .await
            .map_err(|_| error("protected capability unavailable"))?
            .ok_or_else(|| error("AgentCapabilityExpiredError: wait for runtime renewal"))?;
        let client = HubClient::new(&binding.hub_url, capability).map_err(hub_error)?;
        let base = format!("/projects/{}", publication.project);
        match operation.as_str() {
            "participants" => {
                let members: Vec<Member> = client
                    .get(&format!("{base}/members"))
                    .await
                    .map_err(hub_error)?;
                let agents: Vec<Agent> = client
                    .get(&format!("{base}/agents"))
                    .await
                    .map_err(hub_error)?;
                return Ok(json!({"members":members,"agents":agents}));
            }
            "messages.list" => {
                let messages: Vec<Message> = client
                    .get(&format!("{base}/messages"))
                    .await
                    .map_err(hub_error)?;
                return Ok(json!(messages));
            }
            "tasks.list" => {
                let tasks: Vec<Task> = client
                    .get(&format!("{base}/tasks"))
                    .await
                    .map_err(hub_error)?;
                return Ok(json!(tasks));
            }
            _ => {}
        }
        let operation_id = operation_id(&input)?;
        // Typed DTOs reject hidden administrative fields and author/project forgery.
        let object = input
            .as_object_mut()
            .ok_or_else(|| error("expected object"))?;
        object.remove("operation_id");
        object.remove("action");
        let task = object.remove("task");
        let (method, path, payload) = match operation.as_str() {
            "messages.send" => {
                let value: SendMessage =
                    serde_json::from_value(input).map_err(|_| error("invalid message fields"))?;
                (Method::POST, format!("{base}/messages"), {
                    let runtime = self.runtime()?;
                    let mut payload = json!(value);
                    payload["depth"] = json!(runtime.effective_mcp_depth(&session));
                    payload["hops"] =
                        json!(runtime.effective_message_hops(&session).saturating_add(1));
                    payload
                })
            }
            "tasks.create" => {
                let value: CreateTask = serde_json::from_value(input)
                    .map_err(|_| error("invalid create task fields"))?;
                (Method::POST, format!("{base}/tasks"), json!(value))
            }
            "tasks.claim" => {
                let value: ClaimTask = serde_json::from_value(input)
                    .map_err(|_| error("invalid claim task fields"))?;
                let task = identifier(&json!({"task":task}), "task")?;
                (
                    Method::POST,
                    format!("{base}/tasks/{task}/claim"),
                    json!(value),
                )
            }
            "tasks.update" => {
                let value: UpdateTask = serde_json::from_value(input)
                    .map_err(|_| error("invalid update task fields"))?;
                let task = identifier(&json!({"task":task}), "task")?;
                (Method::PATCH, format!("{base}/tasks/{task}"), json!(value))
            }
            _ => return Err(ApiError::forbidden("agent operation is unavailable")),
        };
        let identity = serde_json::to_string(&(
            publication.issuer,
            publication.subject,
            "agent",
            publication.agent_id,
        ))
        .map_err(|_| error("invalid agent identity"))?;
        let mutation = Mutation {
            operation_id: operation_id.clone(),
            payload,
        };
        self.local
            .enqueue(
                operation_id.clone(),
                identity,
                publication.hub_id,
                publication.project,
                json!({"method":method.as_str(),"path":path,"mutation":mutation}),
            )
            .map_err(storage)?;
        let result: MutationResult<Value> = match client.mutate(method, &path, &mutation).await {
            Ok(result) => result,
            Err(error) if retryable_hub(&error) => return Ok(queued(&operation_id)),
            Err(error) => {
                self.local
                    .reject(operation_id, error.to_string())
                    .map_err(storage)?;
                return Err(hub_error(error));
            }
        };
        self.local.complete(operation_id).map_err(storage)?;
        Ok(result.data)
    }
}

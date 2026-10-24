//! Runtime leases and short-lived agent credentials remain outside model-visible data.
use super::*;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn capability_key(publication: &Publication) -> String {
    format!(
        "{}:{}:{}",
        publication.hub_id,
        publication.project,
        publication.agent_id.as_deref().unwrap_or_default()
    )
}
impl Collaboration {
    pub(super) async fn revoke_runtimes(&self, token: Option<cyber_account::Secret>) {
        let Ok(publications) = self.local.publications() else {
            return;
        };
        let Ok(bindings) = self.local.bindings() else {
            return;
        };
        let mut revoked = std::collections::BTreeSet::new();
        for publication in publications {
            let _ = self.secrets.delete(&capability_key(&publication)).await;
            let Some(token) = token.clone() else { continue };
            let Some(runtime) = publication.runtime.as_deref() else {
                continue;
            };
            if !revoked.insert((
                publication.hub_id.clone(),
                publication.project.clone(),
                runtime.to_owned(),
            )) {
                continue;
            }
            let Some(binding) = bindings
                .iter()
                .find(|b| b.hub_id == publication.hub_id && b.project == publication.project)
            else {
                continue;
            };
            if let Ok(client) = HubClient::new(&binding.hub_url, token) {
                let _: Result<MutationResult<Value>, _> = client
                    .mutate(
                        Method::DELETE,
                        &format!("/projects/{}/runtimes/{runtime}", publication.project),
                        &Mutation::new(Empty {}),
                    )
                    .await;
            }
        }
    }
    pub(super) fn spawn(self: &Arc<Self>) {
        let weak = Arc::downgrade(self);
        tokio::spawn(async move {
            let mut delay = 20;
            let mut retry = 1;
            loop {
                tokio::time::sleep(Duration::from_secs(delay)).await;
                let Some(connector) = weak.upgrade() else {
                    return;
                };
                if !connector.enabled() || connector.suspended.load(Ordering::Acquire) {
                    delay = 20;
                    continue;
                }
                let cancel = match connector.worker_cancel.lock() {
                    Ok(cancel) => cancel.clone(),
                    Err(_) => return,
                };
                let result = tokio::select! {_ = cancel.cancelled()=>None,result=connector.heartbeat()=>Some(result)};
                match result {
                    Some(Ok(())) => {
                        connector.connected.store(true, Ordering::Release);
                        if let Ok(mut error) = connector.last_error.lock() {
                            *error = None;
                        }
                        delay = 20;
                        retry = 1;
                    }
                    Some(Err(error)) => {
                        connector.connected.store(false, Ordering::Release);
                        if let Ok(mut last) = connector.last_error.lock() {
                            *last = Some(error.body.message);
                        }
                        delay = retry;
                        retry = (retry * 2).min(60);
                    }
                    None => {
                        delay = 20;
                    }
                }
            }
        });
    }
    pub(super) async fn publish(&self, input: Value) -> Result<Value, ApiError> {
        let session = identifier(&input, "session_id")?;
        let binding = self.binding(&input).await?;
        let checkout = self.checkout(&json!({"session_id":session})).await?;
        if checkout.display().to_string() != binding.checkout {
            return Err(error("Session checkout differs from the confirmed binding"));
        }
        if repository(&checkout, &binding.remote).await? != binding.repository {
            return Err(error("RepositoryMismatchError: checkout remote changed"));
        }
        let account = self
            .accounts
            .current()
            .await?
            .account
            .ok_or_else(ApiError::unauthorized)?;
        let existing = self.local.publication(&session).map_err(storage)?;
        let mut publication = existing.unwrap_or_else(|| Publication {
            session_id: session.clone(),
            hub_id: binding.hub_id.clone(),
            project: binding.project.clone(),
            publication_key: format!("cpub_{}", ulid::Ulid::new()),
            agent_id: None,
            runtime: None,
            alias: String::new(),
            active: false,
            issuer: account.issuer.clone(),
            subject: account.sub.clone(),
            expires_at: 0,
        });
        if publication.hub_id != binding.hub_id || publication.project != binding.project {
            return Err(error(
                "PublishedSessionProjectError: create a new Session for a different project",
            ));
        }
        if publication.issuer != account.issuer || publication.subject != account.sub {
            return Err(error(
                "AccountMismatchError: Session publication belongs to another account",
            ));
        }
        publication.alias = field(&input, "name")?.into();
        // Commit the context boundary before sending any metadata to a hub.
        self.local.publish(publication.clone()).map_err(storage)?;
        let client = self.pinned(&binding).await?;
        let base = format!("/projects/{}", binding.project);
        // Each explicit publication starts a fresh runtime enrollment. A revoked or
        // expired runtime from a previous login must never be silently resurrected.
        let runtime_operation = format!(
            "crtreg_{:x}",
            <sha2::Sha256 as sha2::Digest>::digest(operation_id(&input)?.as_bytes())
        );
        let registered: MutationResult<RuntimeRegistration> = client
            .mutate(
                Method::POST,
                &format!("{base}/runtimes"),
                &Mutation {
                    operation_id: runtime_operation,
                    payload: RegisterRuntime { name: None },
                },
            )
            .await
            .map_err(hub_error)?;
        let runtime = registered.data.runtime;
        let mut published: MutationResult<Published> = client
            .mutate(
                Method::POST,
                &format!("{base}/agents"),
                &Mutation {
                    operation_id: operation_id(&input)?,
                    payload: Publish {
                        runtime: runtime.clone(),
                        publication_key: publication.publication_key.clone(),
                        name: publication.alias.clone(),
                        repository: binding.repository,
                        subscribe_project_messages: input
                            .get("subscribe_project_messages")
                            .and_then(Value::as_bool)
                            .unwrap_or(false),
                    },
                },
            )
            .await
            .map_err(hub_error)?;
        if published.data.capability.is_none() {
            published = client
                .mutate(
                    Method::POST,
                    &format!("{base}/agents/{}/renew", published.data.agent.id),
                    &Mutation::new(Empty {}),
                )
                .await
                .map_err(hub_error)?;
        }
        publication.runtime = Some(runtime.to_string());
        publication.agent_id = Some(published.data.agent.id.to_string());
        publication.active = true;
        publication.expires_at = published.data.expires_at;
        self.save_capability(&publication, published.data.capability)
            .await?;
        self.local.publish(publication.clone()).map_err(storage)?;
        if let Ok(mut cancel) = self.worker_cancel.lock()
            && cancel.is_cancelled()
        {
            *cancel = tokio_util::sync::CancellationToken::new();
        }
        self.suspended.store(false, Ordering::Release);
        self.connected.store(true, Ordering::Release);
        Ok(json!(publication))
    }
    async fn save_capability(
        &self,
        publication: &Publication,
        capability: Option<String>,
    ) -> Result<(), ApiError> {
        if let Some(capability) = capability {
            self.secrets
                .put(
                    &capability_key(publication),
                    cyber_account::Secret::new(capability),
                )
                .await
                .map_err(|_| {
                    ApiError::service_unavailable(
                        "collaboration",
                        "could not protect agent capability",
                    )
                })?;
        }
        Ok(())
    }
    pub(super) async fn unpublish(&self, input: Value) -> Result<Value, ApiError> {
        let agent = identifier(&input, "agent")?;
        let mut publication = self
            .local
            .publications()
            .map_err(storage)?
            .into_iter()
            .find(|p| p.agent_id.as_deref() == Some(&agent))
            .ok_or_else(|| error("unknown local agent publication"))?;
        // Stop renewals even when revocation transport fails. The hub expiry remains a
        // strict upper bound while a disconnected owner retries explicit revocation.
        publication.active = false;
        self.local.publish(publication.clone()).map_err(storage)?;
        self.secrets
            .delete(&capability_key(&publication))
            .await
            .map_err(|_| error("could not remove agent capability"))?;
        let binding = self
            .local
            .bindings()
            .map_err(storage)?
            .into_iter()
            .find(|b| b.hub_id == publication.hub_id && b.project == publication.project)
            .ok_or_else(|| error("publication binding unavailable"))?;
        let client = self.pinned(&binding).await?;
        let _: MutationResult<Value> = client
            .mutate(
                Method::DELETE,
                &format!("/projects/{}/agents/{agent}", publication.project),
                &Mutation {
                    operation_id: operation_id(&input)?,
                    payload: Empty {},
                },
            )
            .await
            .map_err(hub_error)?;
        Ok(json!({"unpublished":true,"agent":agent}))
    }
    async fn replay_human(
        &self,
        current: &cyber_account::Account,
        bindings: &[Binding],
    ) -> Result<(), ApiError> {
        let identity = serde_json::to_string(&(&current.issuer, &current.sub))
            .map_err(|_| error("invalid identity"))?;
        for (operation_id, hub, project, body) in self.local.pending(identity).map_err(storage)? {
            if self.suspended.load(Ordering::Acquire) {
                return Ok(());
            }
            let Some(binding) = bindings
                .iter()
                .find(|b| b.hub_id == hub && b.project == project)
            else {
                continue;
            };
            let client = self.pinned(binding).await?;
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
    async fn heartbeat(&self) -> Result<(), ApiError> {
        let current = self
            .accounts
            .current()
            .await?
            .account
            .ok_or_else(ApiError::unauthorized)?;
        let bindings = self.local.bindings().map_err(storage)?;
        let publications = self.local.publications().map_err(storage)?;
        self.prune_delivery_scans(&publications)?;
        self.sync_bindings(&current, &bindings).await?;
        self.replay_human(&current, &bindings).await?;
        let presence: Vec<_> = publications
            .iter()
            .filter(|p| p.active && p.issuer == current.issuer && p.subject == current.sub)
            .cloned()
            .collect();
        let mut renewed = std::collections::BTreeSet::new();
        for publication in presence.iter().cloned() {
            self.refresh_publication(publication, &bindings, &presence, &mut renewed)
                .await?;
        }
        Ok(())
    }
    async fn refresh_publication(
        &self,
        mut publication: Publication,
        bindings: &[Binding],
        presence: &[Publication],
        renewed: &mut std::collections::BTreeSet<(String, String, String)>,
    ) -> Result<(), ApiError> {
        if self.suspended.load(Ordering::Acquire) {
            return Ok(());
        }
        let Some(binding) = bindings
            .iter()
            .find(|b| b.hub_id == publication.hub_id && b.project == publication.project)
        else {
            return Ok(());
        };
        let keep = match self.runtime()?.state(&publication.session_id).await {
            Ok(state) => !state.info.archived,
            Err(cyber_server::runtime::RuntimeError::SessionNotFound(_)) => false,
            Err(error) => return Err(super::error(error.to_string())),
        };
        if !keep {
            self.unpublish(json!({"agent":publication.agent_id,"operation_id":format!("cop_{}",ulid::Ulid::new())})).await?;
            return Ok(());
        }
        let Some(runtime) = publication.runtime.as_deref() else {
            return Ok(());
        };
        let Some(agent) = publication.agent_id.as_deref() else {
            return Ok(());
        };
        let client = self.pinned(binding).await?;
        let base = format!("/projects/{}", publication.project);
        if renewed.insert((
            publication.hub_id.clone(),
            publication.project.clone(),
            runtime.to_owned(),
        )) {
            let _: MutationResult<RuntimeRegistration> = client
                .mutate(
                    Method::POST,
                    &format!("{base}/runtimes/{runtime}/heartbeat"),
                    &Mutation::new(Heartbeat {
                        agents: presence
                            .iter()
                            .filter(|p| {
                                p.hub_id == publication.hub_id
                                    && p.project == publication.project
                                    && p.runtime.as_deref() == Some(runtime)
                            })
                            .filter_map(|p| {
                                Some(AgentPresence {
                                    agent: p.agent_id.as_deref()?.parse().ok()?,
                                    state: if self.runtime().ok()?.is_running(&p.session_id) {
                                        "running"
                                    } else {
                                        "idle"
                                    }
                                    .into(),
                                })
                            })
                            .collect(),
                    }),
                )
                .await
                .map_err(hub_error)?;
        }
        if publication.expires_at <= now() + 60
            || self
                .secrets
                .get(&capability_key(&publication))
                .await
                .map_err(|_| error("agent credential unavailable"))?
                .is_none()
        {
            let result: MutationResult<Published> = client
                .mutate(
                    Method::POST,
                    &format!("{base}/agents/{agent}/renew"),
                    &Mutation::new(Empty {}),
                )
                .await
                .map_err(hub_error)?;
            publication.expires_at = result.data.expires_at;
            self.save_capability(&publication, result.data.capability)
                .await?;
            self.local.publish(publication.clone()).map_err(storage)?;
        }
        self.replay_agent(&publication, binding).await?;
        self.poll_deliveries(&publication, binding).await?;
        Ok(())
    }
    pub(super) async fn unbound(&self, operation: &str, input: Value) -> Result<Value, ApiError> {
        let client = self.client(field(&input, "hub_url")?).await?;
        let _hello: HubHello = client.get("/hello").await.map_err(hub_error)?;
        if operation == "applications.status" {
            return client
                .get(&format!(
                    "/applications/{}",
                    identifier(&input, "application")?
                ))
                .await
                .map_err(hub_error);
        }
        let (path, payload) = match operation {
            "project.create" => (
                "/projects".into(),
                json!({"name":field(&input,"name")?,"repository":field(&input,"repository")?,"mode":input.get("mode").ok_or_else(||error("missing project mode"))?}),
            ),
            "join" => (
                "/join".into(),
                json!({"invitation_token":input.get("invitation_token"),"project":input.get("project"),"name":input.get("name")}),
            ),
            "applications.redeem" => (
                format!(
                    "/applications/{}/redeem",
                    identifier(&input, "application")?
                ),
                json!({"repository":Repository::parse(field(&input,"repository")?).map_err(|_|error("invalid repository"))?,"code":input.get("code")}),
            ),
            _ => return Err(error("unknown unbound operation")),
        };
        // Invitation/code payloads never enter the durable outbox or diagnostics.
        let response: MutationResult<Value> = client
            .mutate(
                Method::POST,
                &path,
                &Mutation {
                    operation_id: operation_id(&input)?,
                    payload,
                },
            )
            .await
            .map_err(hub_error)?;
        Ok(response.data)
    }
}

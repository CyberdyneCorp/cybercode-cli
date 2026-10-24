//! Durable, targeted ingress. Project broadcasts reach only explicit subscriptions.
use super::*;
use cyber_collab::model::{DeliveryState, MembershipKind, Participant};
use cyber_server::runtime::messaging::{CollaborationOrigin, Outcome};
impl Collaboration {
    pub(super) async fn authorize_delivery(
        &self,
        session: &str,
        origin: &CollaborationOrigin,
    ) -> Result<(), ApiError> {
        if !self.enabled() || self.suspended.load(Ordering::Acquire) {
            return Err(ApiError::forbidden(
                "collaboration is disabled or logged out; message remains held",
            ));
        }
        let publication = self
            .local
            .publication(session)
            .map_err(storage)?
            .filter(|p| p.active)
            .ok_or_else(|| ApiError::forbidden("Session publication was removed"))?;
        if publication.hub_id != origin.hub_id
            || publication.project != origin.project_id
            || publication.agent_id.as_deref() != Some(&origin.target_agent_id)
        {
            return Err(ApiError::forbidden(
                "message targets a different publication",
            ));
        }
        let binding = self
            .binding(&json!({"session_id":session,"project":publication.project}))
            .await?;
        if binding.hub_id != origin.hub_id {
            return Err(ApiError::forbidden("checkout binding changed"));
        }
        let account = self
            .accounts
            .current()
            .await?
            .account
            .ok_or_else(ApiError::unauthorized)?;
        if account.issuer != publication.issuer || account.sub != publication.subject {
            return Err(ApiError::forbidden(
                "publication belongs to another account",
            ));
        }
        let client = self.pinned(&binding).await?;
        let members: Vec<Member> = client
            .get(&format!("/projects/{}/members", publication.project))
            .await
            .map_err(hub_error)?;
        let sender = members
            .iter()
            .find(|member| {
                member.id.as_str() == origin.member_id
                    && member.active
                    && member.generation == origin.member_generation
            })
            .ok_or_else(|| {
                ApiError::forbidden("sender membership changed; message remains held")
            })?;
        let kind = match sender.kind {
            MembershipKind::Internal => "internal",
            MembershipKind::Guest => "guest",
            MembershipKind::Friend => "friend",
        };
        if kind != origin.membership_kind {
            return Err(ApiError::forbidden("sender membership kind changed"));
        }
        let authorization: DeliveryAuthorization = client
            .get(&format!(
                "/projects/{}/deliveries/{}/{}/authorization",
                publication.project, origin.remote_message_id, origin.target_agent_id
            ))
            .await
            .map_err(hub_error)?;
        if !authorization.allowed {
            return Err(ApiError::forbidden(
                authorization
                    .reason
                    .unwrap_or_else(|| "delivery is no longer authorized".into()),
            ));
        }
        Ok(())
    }
    pub(super) async fn poll_deliveries(
        &self,
        publication: &Publication,
        binding: &Binding,
    ) -> Result<(), ApiError> {
        let Some(agent) = publication.agent_id.as_deref() else {
            return Ok(());
        };
        let Some(token) = self
            .secrets
            .get(&format!(
                "{}:{}:{agent}",
                publication.hub_id, publication.project
            ))
            .await
            .map_err(|_| error("protected agent capability unavailable"))?
        else {
            return Ok(());
        };
        let client = HubClient::new(&binding.hub_url, token).map_err(hub_error)?;
        let base = format!("/projects/{}", publication.project);
        // Releases and drops commit in the normal runtime. Publish their durable
        // outcomes even if the hub no longer includes acknowledged held deliveries.
        for (message, hash, outcome, inbox) in self
            .local
            .delivery_outcomes(
                publication.hub_id.clone(),
                publication.project.clone(),
                agent.into(),
            )
            .map_err(storage)?
        {
            let state = match outcome.as_str() {
                "released" | "delivered" => DeliveryState::Released,
                "refused" => DeliveryState::Refused,
                _ => DeliveryState::Dropped,
            };
            let _: MutationResult<Delivery> = client
                .mutate(
                    Method::POST,
                    &format!("{base}/deliveries/{message}/{agent}/ack"),
                    &Mutation {
                        operation_id: format!("ack_{inbox}_{state:?}"),
                        payload: Acknowledge {
                            state,
                            content_hash: hash,
                        },
                    },
                )
                .await
                .map_err(hub_error)?;
        }
        let deliveries: Vec<Delivery> = client
            .get(&format!("{base}/deliveries?agent={agent}"))
            .await
            .map_err(hub_error)?;
        let members: Vec<Member> = client
            .get(&format!("{base}/members"))
            .await
            .map_err(hub_error)?;
        for delivery in deliveries {
            if self.suspended.load(Ordering::Acquire) || !self.enabled() {
                return Ok(());
            }
            if delivery.target.as_str() != agent
                || delivery.message.project.as_str() != publication.project
            {
                return Err(error("delivery targets another project or publication"));
            }
            let Some(member) = members
                .iter()
                .find(|member| member.id == delivery.message.owner && member.active)
            else {
                continue;
            };
            let message = &delivery.message;
            let origin = CollaborationOrigin {
                hub_id: publication.hub_id.clone(),
                project_id: publication.project.clone(),
                member_id: member.id.to_string(),
                agent_id: match &message.author {
                    Participant::Human(_) => None,
                    Participant::Agent(id) => Some(id.to_string()),
                },
                member_generation: message.member_generation,
                membership_kind: match message.membership_kind {
                    MembershipKind::Internal => "internal",
                    MembershipKind::Guest => "guest",
                    MembershipKind::Friend => "friend",
                }
                .into(),
                remote_message_id: message.id.to_string(),
                target_agent_id: agent.into(),
                depth: message.depth,
                hops: message.hops,
            };
            self.authorize_delivery(&publication.session_id, &origin)
                .await?;
            let stable = self
                .local
                .delivery(
                    &publication.hub_id,
                    &publication.project,
                    message.id.as_str(),
                    agent,
                    &delivery.content_hash,
                )
                .map_err(storage)?;
            let result = self
                .runtime()?
                .messaging()
                .receive_collaboration(
                    &publication.session_id,
                    &stable,
                    origin,
                    message.text.clone(),
                )
                .await
                .map_err(|e| error(e.to_string()))?;
            let state = match result.outcome {
                Outcome::Held => DeliveryState::Held,
                Outcome::Delivered => DeliveryState::Released,
                Outcome::Refused => DeliveryState::Refused,
            };
            let _: MutationResult<Delivery> = client
                .mutate(
                    Method::POST,
                    &format!("{base}/deliveries/{}/{agent}/ack", message.id),
                    &Mutation {
                        operation_id: format!("ack_{stable}_{state:?}"),
                        payload: Acknowledge {
                            state,
                            content_hash: delivery.content_hash,
                        },
                    },
                )
                .await
                .map_err(hub_error)?;
        }
        Ok(())
    }
}

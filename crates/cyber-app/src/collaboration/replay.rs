//! Replay checkpoints update observation metadata only. Targeted deliveries remain the
//! sole bridge into Runtime, including when replay retention requires a new snapshot.
use super::*;
use local::CursorScope;
impl Collaboration {
    pub(super) async fn sync_bindings(
        &self,
        current: &cyber_account::Account,
        bindings: &[Binding],
    ) -> Result<(), ApiError> {
        for binding in bindings {
            if self.suspended.load(Ordering::Acquire) || !self.enabled() {
                return Ok(());
            }
            self.sync_binding(current, binding).await?;
        }
        Ok(())
    }
    async fn sync_binding(
        &self,
        current: &cyber_account::Account,
        binding: &Binding,
    ) -> Result<(), ApiError> {
        let client = self.pinned(binding).await?;
        let base = format!("/projects/{}", binding.project);
        // These authorized reads run BEFORE looking up the saved cursor. A new member,
        // role generation, policy revision or account cannot inherit prior visibility.
        let project: Project = client.get(&base).await.map_err(hub_error)?;
        let members: Vec<Member> = client
            .get(&format!("{base}/members"))
            .await
            .map_err(hub_error)?;
        let member = members
            .into_iter()
            .find(|m| m.active && m.issuer == current.issuer && m.subject == current.sub)
            .ok_or_else(|| {
                ApiError::forbidden("current project membership is required for replay")
            })?;
        let scope = CursorScope {
            checkout: binding.checkout.clone(),
            identity: serde_json::to_string(&(&current.issuer, &current.sub))
                .map_err(|_| error("invalid identity"))?,
            hub: binding.hub_id.clone(),
            project: binding.project.clone(),
            member: member.id.to_string(),
            generation: member.generation,
            project_revision: project.revision,
        };
        self.sync_events(&client, &base, scope).await
    }
    async fn sync_events(
        &self,
        client: &HubClient,
        base: &str,
        scope: CursorScope,
    ) -> Result<(), ApiError> {
        let mut cursor = self.local.cursor(scope.clone()).map_err(storage)?;
        if cursor == 0 {
            return self.checkpoint_snapshot(client, base, scope).await;
        }
        // Bound each synchronization pass; the next heartbeat continues a large replay.
        for _ in 0..20 {
            let page: EventPage = match client.get(&format!("{base}/events?after={cursor}")).await {
                Ok(page) => page,
                Err(error) if error.tag == "ResyncRequiredError" => {
                    return self.checkpoint_snapshot(client, base, scope).await;
                }
                Err(error) => return Err(hub_error(error)),
            };
            if page.cursor < cursor {
                return Err(error("invalid replay cursor regression"));
            }
            self.local
                .checkpoint(scope.clone(), page.cursor)
                .map_err(storage)?;
            cursor = page.cursor;
            if !page.has_more {
                return Ok(());
            }
        }
        Ok(())
    }
    async fn checkpoint_snapshot(
        &self,
        client: &HubClient,
        base: &str,
        scope: CursorScope,
    ) -> Result<(), ApiError> {
        let snapshot: Snapshot = client
            .get(&format!("{base}/snapshot"))
            .await
            .map_err(hub_error)?;
        if snapshot.project.id.as_str() != scope.project {
            return Err(error("snapshot project mismatch"));
        }
        // Resource panels fetch their current authorized views. Snapshot content is
        // not input, tool work, or an instruction to replay tasks after any gap.
        self.local
            .checkpoint(scope, snapshot.cursor)
            .map_err(storage)
    }
}

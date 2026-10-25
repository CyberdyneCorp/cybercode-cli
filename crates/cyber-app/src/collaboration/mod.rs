//! Project hub connector. Only explicit owner bindings and publications cross this boundary.
mod agent;
mod delivery;
mod lifecycle;
pub mod local;
mod replay;
use crate::account::Accounts;
use cyber_collab::{
    api::*,
    client::HubClient,
    model::{Member, Project},
    repository::Repository,
};
use cyber_server::{
    http::{ApiError, account::AccountService, collaboration::CollaborationService},
    runtime::Runtime,
};
use futures::future::BoxFuture;
use local::{Binding, LocalStore, Publication};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

pub(super) struct HubConnection {
    client: HubClient,
    account: cyber_account::Account,
}
impl std::ops::Deref for HubConnection {
    type Target = HubClient;
    fn deref(&self) -> &Self::Target {
        &self.client
    }
}
pub struct Collaboration {
    pub local: LocalStore,
    accounts: Arc<Accounts>,
    runtime: cyber_server::runtime::WeakRuntime,
    default_directory: PathBuf,
    global: Arc<crate::services::GlobalConfigFn>,
    secrets: cyber_account::HostSecrets,
    suspended: AtomicBool,
    connected: AtomicBool,
    last_error: std::sync::Mutex<Option<String>>,
    worker_cancel: std::sync::Mutex<tokio_util::sync::CancellationToken>,
    delivery_scans: std::sync::Mutex<std::collections::BTreeMap<String, String>>,
}
fn error(text: impl Into<String>) -> ApiError {
    let text = text.into();
    if let Some((tag, message)) = text.split_once(": ")
        && tag.ends_with("Error")
        && tag.bytes().all(|b| b.is_ascii_alphanumeric())
    {
        return ApiError::new(StatusCode::BAD_REQUEST, tag, message);
    }
    ApiError::invalid(text)
}
fn storage(error: cyber_store::StoreError) -> ApiError {
    match error {
        cyber_store::StoreError::InvalidEvent { reason, .. } => self::error(reason),
        other => ApiError::service_unavailable("collaboration", other.to_string()),
    }
}
fn retryable_hub(error: &cyber_collab::error::Error) -> bool {
    matches!(
        error.tag.as_str(),
        "HubUnavailableError"
            | "LiveValidationUnavailableError"
            | "DiscoveryUnavailableError"
            | "TokenExchangeError"
            | "ServiceUnavailableError"
            | "StorageBusyError"
            | "StorageUnavailableError"
            | "RateLimitError"
    )
}
fn queued(operation: &str) -> Value {
    json!({"status":"queued","operation_id":operation,"message":"Durably queued; retry with this same operation ID. Do not resend as a new operation."})
}
fn hub_error(error: cyber_collab::error::Error) -> ApiError {
    let status = match error.tag.as_str() {
        "ForbiddenError" | "MissingScopeError" | "MembershipRevokedError" => StatusCode::FORBIDDEN,
        "NotLoggedInError"
        | "InactiveIdentityError"
        | "SessionExpiredError"
        | "InvalidTokenError" => StatusCode::UNAUTHORIZED,
        "RevisionConflictError" | "OperationConflictError" => StatusCode::CONFLICT,
        "ProjectNotFoundError" => StatusCode::NOT_FOUND,
        "HubUnavailableError" => StatusCode::BAD_GATEWAY,
        _ => StatusCode::BAD_REQUEST,
    };
    let mut result = ApiError::new(status, &error.tag, error.message);
    if let Some(revision) = error.current_revision {
        result = result.with_field("current_revision", revision);
    }
    if let Some(retry) = error.retry_after_ms {
        result = result.with_field("retry_after_ms", retry);
    }
    result
}
fn field<'a>(input: &'a Value, key: &str) -> Result<&'a str, ApiError> {
    input
        .get(key)
        .and_then(Value::as_str)
        .filter(|v| !v.is_empty())
        .ok_or_else(|| error(format!("missing {key}")))
}
fn identifier(input: &Value, key: &str) -> Result<String, ApiError> {
    let value = field(input, key)?;
    if !value
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'_')
    {
        return Err(error(format!("invalid {key}")));
    }
    Ok(value.into())
}
fn operation_id(input: &Value) -> Result<String, ApiError> {
    let value = field(input, "operation_id")?;
    if value.len() > 128 || value.chars().any(char::is_control) {
        return Err(error("invalid operation ID"));
    }
    Ok(value.into())
}
impl Collaboration {
    pub fn new(
        store: Arc<cyber_store::Store>,
        accounts: Arc<Accounts>,
        runtime: Runtime,
        default_directory: PathBuf,
        global: Arc<crate::services::GlobalConfigFn>,
        data: PathBuf,
    ) -> Result<Arc<Self>, String> {
        let secrets = cyber_account::HostSecrets::new(
            data,
            "collaboration-capabilities.json",
            "cyber-collaboration",
            cyber_account::credential_vault(),
        )
        .map_err(|e| e.to_string())?;
        Self::with_secrets(store, accounts, runtime, default_directory, global, secrets)
    }
    /// Embedders and acceptance fixtures can supply a protected credential adapter.
    /// Production construction always selects the platform's native credential vault.
    pub fn with_secrets(
        store: Arc<cyber_store::Store>,
        accounts: Arc<Accounts>,
        runtime: Runtime,
        default_directory: PathBuf,
        global: Arc<crate::services::GlobalConfigFn>,
        secrets: cyber_account::HostSecrets,
    ) -> Result<Arc<Self>, String> {
        let connector = Arc::new(Self {
            local: LocalStore::new(store).map_err(|e| e.to_string())?,
            accounts,
            runtime: runtime.downgrade(),
            default_directory,
            global,
            secrets,
            suspended: AtomicBool::new(false),
            connected: AtomicBool::new(false),
            last_error: std::sync::Mutex::new(None),
            worker_cancel: std::sync::Mutex::new(tokio_util::sync::CancellationToken::new()),
            delivery_scans: std::sync::Mutex::new(Default::default()),
        });
        let weak = Arc::downgrade(&connector);
        runtime.set_collaboration_authorizer(Arc::new(move |session, origin| {
            let weak = weak.clone();
            Box::pin(async move {
                let connector = weak
                    .upgrade()
                    .ok_or_else(|| "project connector unavailable".to_owned())?;
                connector
                    .authorize_delivery(&session, &origin)
                    .await
                    .map_err(|e| e.body.message)
            })
        }));
        let weak = Arc::downgrade(&connector);
        connector.accounts.on_logout(Arc::new(move |token, _all| {
            let connector = weak.upgrade();
            if let Some(connector) = &connector {
                connector.suspended.store(true, Ordering::Release);
                connector.connected.store(false, Ordering::Release);
                if let Ok(cancel) = connector.worker_cancel.lock() {
                    cancel.cancel();
                }
            }
            Box::pin(async move {
                if let Some(connector) = connector {
                    connector.revoke_runtimes(token).await;
                }
            })
        }));
        connector.spawn();
        Ok(connector)
    }
    fn runtime(&self) -> Result<Runtime, ApiError> {
        self.runtime
            .upgrade()
            .ok_or_else(|| error("local runtime unavailable"))
    }
    pub fn enabled(&self) -> bool {
        (self.global)().is_ok_and(|value| {
            cyber_core::features::Features::from_config(&value, &Default::default())
                .enabled("collaboration")
        })
    }
    async fn checkout(&self, input: &Value) -> Result<PathBuf, ApiError> {
        let directory = if let Some(session) = input.get("session_id").and_then(Value::as_str) {
            PathBuf::from(
                self.runtime()?
                    .state(session)
                    .await
                    .map_err(|e| error(e.to_string()))?
                    .info
                    .directory,
            )
        } else {
            input
                .get("directory")
                .and_then(Value::as_str)
                .map(PathBuf::from)
                .unwrap_or_else(|| self.default_directory.clone())
        };
        let directory = std::fs::canonicalize(directory)
            .map_err(|_| error("checkout directory is unavailable"))?;
        let output = tokio::process::Command::new("git")
            .arg("-C")
            .arg(&directory)
            .args(["rev-parse", "--show-toplevel"])
            .output()
            .await
            .map_err(|_| error("git is unavailable"))?;
        if !output.status.success() {
            return Err(error("Git checkout required for project collaboration"));
        }
        let root =
            String::from_utf8(output.stdout).map_err(|_| error("invalid checkout directory"))?;
        std::fs::canonicalize(root.trim()).map_err(|_| error("checkout directory is unavailable"))
    }
    async fn binding(&self, input: &Value) -> Result<Binding, ApiError> {
        let checkout = self.checkout(input).await?;
        let binding = self
            .local
            .binding(&checkout.display().to_string())
            .map_err(storage)?
            .ok_or_else(|| {
                error("ProjectBindingRequiredError: explicitly bind this checkout first")
            })?;
        if input
            .get("project")
            .and_then(Value::as_str)
            .is_some_and(|p| p != binding.project)
        {
            return Err(error(
                "ProjectBindingMismatchError: project differs from this checkout's binding",
            ));
        }
        Ok(binding)
    }
    async fn client(&self, url: &str) -> Result<HubConnection, ApiError> {
        let (token, account) = self.accounts.credential_snapshot().await?;
        Ok(HubConnection {
            client: HubClient::new(url, token).map_err(hub_error)?,
            account,
        })
    }
    async fn issuer_checked_client(&self, url: &str) -> Result<HubConnection, ApiError> {
        let client = self.client(url).await?;
        let hello: HubHello = client.get("/hello").await.map_err(hub_error)?;
        if client.account.issuer.trim_end_matches('/') != hello.issuer.trim_end_matches('/') {
            return Err(error(
                "IssuerMismatchError: hub and selected account use different issuers",
            ));
        }
        Ok(client)
    }
    pub(super) async fn pinned(&self, binding: &Binding) -> Result<HubConnection, ApiError> {
        let client = self.issuer_checked_client(&binding.hub_url).await?;
        let hello: HubHello = client.get("/hello").await.map_err(hub_error)?;
        if hello.hub_id.as_str() != binding.hub_id {
            return Err(error(
                "HubIdentityMismatchError: hub identity changed; explicitly inspect and bind again",
            ));
        }
        Ok(client)
    }
    async fn bind(&self, input: Value) -> Result<Value, ApiError> {
        if input.get("confirm").and_then(Value::as_bool) != Some(true) {
            return Err(error(
                "explicit confirmation is required to bind a checkout",
            ));
        }
        let checkout = self.checkout(&input).await?;
        let hub_url = field(&input, "hub_url")?;
        let project = identifier(&input, "project")?;
        let client = self.issuer_checked_client(hub_url).await?;
        let hello: HubHello = client.get("/hello").await.map_err(hub_error)?;
        let descriptor: Project = client
            .get(&format!("/projects/{project}"))
            .await
            .map_err(hub_error)?;
        let repository = repository(
            &checkout,
            input
                .get("remote")
                .and_then(Value::as_str)
                .unwrap_or("origin"),
        )
        .await?;
        if repository != descriptor.repository {
            return Err(error(
                "RepositoryMismatchError: checkout remote does not match the hub project",
            ));
        }
        let binding = Binding {
            remote: input
                .get("remote")
                .and_then(Value::as_str)
                .unwrap_or("origin")
                .into(),
            checkout: checkout.display().to_string(),
            hub_url: hub_url.into(),
            hub_id: hello.hub_id.to_string(),
            project,
            repository,
        };
        self.pinned(&binding).await?;
        self.local.bind(binding.clone()).map_err(storage)?;
        Ok(json!(binding))
    }
    async fn status(&self, input: Value) -> Result<Value, ApiError> {
        let publication = match input.get("session_id").and_then(Value::as_str) {
            Some(session) => self.local.publication(session).map_err(storage)?,
            None => None,
        };
        let mut published = json!(publication);
        if let Some(published) = published.as_object_mut() {
            published.insert(
                "connected".into(),
                json!(
                    self.connected.load(Ordering::Acquire)
                        && !self.suspended.load(Ordering::Acquire)
                ),
            );
            published.insert(
                "last_error".into(),
                json!(self.last_error.lock().ok().and_then(|error| error.clone())),
            );
        }
        if !self.enabled() {
            return Ok(
                json!({"enabled":false,"binding":null,"published":published,"account":null,"current_member":null,"error":null}),
            );
        }
        let binding = self
            .local
            .binding(&self.checkout(&input).await?.display().to_string())
            .map_err(storage)?;
        let account = self.accounts.current().await;
        let mut current_member = None;
        let mut failure = None;
        if let Some(binding) = &binding {
            match self.status_member(binding, &account).await {
                Ok(member) => current_member = member,
                Err(error) => failure = Some(error.body.message),
            }
        }
        let receipts = match &binding {
            Some(binding) => self
                .local
                .receipts(binding.hub_id.clone(), binding.project.clone())
                .map_err(storage)?,
            None => vec![],
        };
        Ok(
            json!({"enabled":self.enabled(),"binding":binding,"published":published,"account":account.ok(),"current_member":current_member,"error":failure,"receipts":receipts}),
        )
    }
    async fn status_member(
        &self,
        binding: &Binding,
        account: &Result<cyber_server::http::account::AccountView, ApiError>,
    ) -> Result<Option<Member>, ApiError> {
        let client = self.pinned(binding).await?;
        let members: Vec<Member> = client
            .get(&format!("/projects/{}/members", binding.project))
            .await
            .map_err(hub_error)?;
        let Some(account) = account.as_ref().ok().and_then(|view| view.account.as_ref()) else {
            return Ok(None);
        };
        Ok(members.into_iter().find(|member| {
            member.active && member.issuer == account.issuer && member.subject == account.sub
        }))
    }
    fn expected_actor(client: &HubConnection, input: &Value) -> Result<(), ApiError> {
        let Some(expected) = input.get("expected_actor") else {
            return Ok(());
        };
        if expected["issuer"] != client.account.issuer || expected["subject"] != client.account.sub
        {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                "AccountChangedError",
                "Selected account changed; refresh and review the action",
            ));
        }
        Ok(())
    }
    pub(super) async fn contribution_scope(
        client: &HubConnection,
        project: &str,
    ) -> Result<ContributionAuthorization, ApiError> {
        client
            .get(&format!("/projects/{project}/contributions/authorization"))
            .await
            .map_err(hub_error)
    }
    async fn authorize_contribution(
        &self,
        binding: &Binding,
        client: &HubClient,
        input: &Value,
    ) -> Result<Value, ApiError> {
        if repository(Path::new(&binding.checkout), &binding.remote).await? != binding.repository {
            return Err(error(
                "RepositoryMismatchError: pinned checkout remote changed",
            ));
        }
        let base = format!("/projects/{}", binding.project);
        let query = {
            let mut query = url::form_urlencoded::Serializer::new(String::new());
            for name in ["task", "agent"] {
                if let Some(value) = input.get(name).and_then(Value::as_str) {
                    query.append_pair(name, value);
                }
            }
            query.finish()
        };
        let authorization: cyber_collab::api::ContributionAuthorization = client
            .get(&format!("{base}/contributions/authorization?{query}"))
            .await
            .map_err(hub_error)?;
        if authorization.project.as_str() != binding.project
            || authorization.repository != binding.repository
        {
            return Err(error(
                "ProjectBindingMismatchError: contribution repository differs from the pinned checkout",
            ));
        }
        serde_json::to_value(authorization).map_err(|_| error("invalid contribution authorization"))
    }
    async fn project_call(&self, operation: &str, input: Value) -> Result<Value, ApiError> {
        let binding = self.binding(&input).await?;
        let client = self.pinned(&binding).await?;
        Self::expected_actor(&client, &input)?;
        let base = format!("/projects/{}", binding.project);
        if operation == "integration.authorize" {
            return self.authorize_contribution(&binding, &client, &input).await;
        }
        if operation == "participants" {
            let members: Value = client
                .get(&format!("{base}/members"))
                .await
                .map_err(hub_error)?;
            let agents: Value = client
                .get(&format!("{base}/agents"))
                .await
                .map_err(hub_error)?;
            return Ok(json!({"members":members,"agents":agents}));
        }
        if let Some(resource) = operation.strip_suffix(".list")
            && matches!(
                resource,
                "messages" | "tasks" | "members" | "invitations" | "applications"
            )
        {
            return client
                .get(&format!("{base}/{resource}"))
                .await
                .map_err(hub_error);
        }
        let (method, path) = match operation {
            "messages.send" => (Method::POST, format!("{base}/messages")),
            "tasks.create" => (Method::POST, format!("{base}/tasks")),
            "tasks.claim" => (
                Method::POST,
                format!("{base}/tasks/{}/claim", identifier(&input, "task")?),
            ),
            "tasks.update" => (
                Method::PATCH,
                format!("{base}/tasks/{}", identifier(&input, "task")?),
            ),
            "tasks.reconcile" => (
                Method::POST,
                format!("{base}/tasks/{}/reconcile", identifier(&input, "task")?),
            ),
            "project.policy" => (Method::PATCH, format!("{base}/policy")),
            "invite" => (Method::POST, format!("{base}/invitations")),
            "invitations.revoke" => (
                Method::DELETE,
                format!("{base}/invitations/{}", identifier(&input, "invitation")?),
            ),
            "applications.approve" | "applications.reject" => (
                Method::POST,
                format!(
                    "{base}/applications/{}/{}",
                    identifier(&input, "application")?,
                    operation.split('.').next_back().unwrap_or_default()
                ),
            ),
            "members.set-role" => (
                Method::PATCH,
                format!("{base}/members/{}", identifier(&input, "member")?),
            ),
            "members.remove" => (
                Method::DELETE,
                format!("{base}/members/{}", identifier(&input, "member")?),
            ),
            "leave" => (Method::POST, format!("{base}/leave")),
            _ => return Err(error("unknown collaboration operation")),
        };
        let operation_id = operation_id(&input)?;
        let mut payload = input.clone();
        let object = payload
            .as_object_mut()
            .ok_or_else(|| error("expected object"))?;
        for key in [
            "operation_id",
            "directory",
            "project",
            "session_id",
            "task",
            "member",
            "invitation",
            "application",
            "expected_actor",
        ] {
            object.remove(key);
        }
        if operation == "messages.send" {
            object.entry("depth").or_insert(json!(0));
            object.entry("hops").or_insert(json!(1));
        }
        let account = &client.account;
        let identity = serde_json::to_string(&(&account.issuer, &account.sub))
            .map_err(|_| error("invalid identity"))?;
        let mutation = Mutation {
            operation_id: operation_id.clone(),
            payload,
        };
        let durable = operation.starts_with("messages.") || operation.starts_with("tasks.");
        if durable {
            let authorization = Self::contribution_scope(&client, &binding.project).await?;
            self.local
                .enqueue(
                    operation_id.clone(),
                    identity,
                    binding.hub_id,
                    binding.project,
                    json!({"method":method.as_str(),"path":path,"mutation":mutation,"authorization":authorization}),
                )
                .map_err(storage)?;
        }
        let result: MutationResult<Value> = match client.mutate(method, &path, &mutation).await {
            Ok(result) => result,
            Err(error) if durable && retryable_hub(&error) => return Ok(queued(&operation_id)),
            Err(error) => {
                if durable {
                    self.local
                        .reject(operation_id, error.to_string())
                        .map_err(storage)?;
                }
                return Err(hub_error(error));
            }
        };
        if durable {
            self.local.complete(operation_id).map_err(storage)?;
        }
        Ok(result.data)
    }
}
impl CollaborationService for Collaboration {
    fn agent_scope(&self, session: &str) -> Option<String> {
        if !self.enabled() || self.suspended.load(Ordering::Acquire) {
            return None;
        }
        let publication = self.local.publication(session).ok()??;
        publication
            .active
            .then(|| format!("{}:{}", publication.hub_id, publication.project))
    }
    fn agent_call(
        &self,
        session: String,
        operation: String,
        input: Value,
    ) -> BoxFuture<'_, Result<Value, ApiError>> {
        Box::pin(self.agent_request(session, operation, input))
    }

    fn call(&self, operation: String, input: Value) -> BoxFuture<'_, Result<Value, ApiError>> {
        Box::pin(async move {
            if operation == "status" {
                return self.status(input).await;
            }
            if !self.enabled() {
                return Err(ApiError::service_unavailable(
                    "collaboration",
                    "enable features.collaboration in user configuration",
                ));
            }
            match operation.as_str() {
                "bind" => self.bind(input).await,
                "publish" => self.publish(input).await,
                "unpublish" => self.unpublish(input).await,
                "project.create"
                | "project.list"
                | "project.show"
                | "join"
                | "applications.redeem"
                | "applications.status" => self.unbound(&operation, input).await,
                _ => self.project_call(&operation, input).await,
            }
        })
    }
}
async fn repository(checkout: &Path, remote: &str) -> Result<Repository, ApiError> {
    if remote.is_empty() || remote.starts_with('-') || remote.chars().any(char::is_control) {
        return Err(error("invalid remote name"));
    }
    let output = tokio::process::Command::new("git")
        .arg("-C")
        .arg(checkout)
        .args(["remote", "get-url", "--", remote])
        .output()
        .await
        .map_err(|_| error("git is unavailable"))?;
    if !output.status.success() {
        return Err(error("checkout remote is unavailable"));
    }
    let url = String::from_utf8(output.stdout).map_err(|_| error("invalid remote URL"))?;
    Repository::parse(url.trim())
        .map_err(|_| error("InvalidRepositoryError: checkout needs a supported remote Git URL"))
}

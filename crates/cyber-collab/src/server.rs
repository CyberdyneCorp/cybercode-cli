//! Authenticated coordination API. This router deliberately exposes no Session,
//! shell, approval, provider credential or worktree execution endpoints.
use crate::{
    api::*, auth::Auth, error::Error, ids::*, model::*, repository::Repository, storage::*,
};
use axum::{
    Json, Router,
    body::Bytes,
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, Method, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::any,
};
use cyber_account::{IdentityLease, Secret};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex},
    time::Instant,
};

#[derive(Clone, Serialize, Deserialize)]
struct RuntimeRecord {
    registration: RuntimeRegistration,
    owner: MemberId,
    generation: u64,
    active: bool,
}
#[derive(Clone, Serialize, Deserialize)]
struct Publication {
    agent: Agent,
    key: String,
    subscribed: bool,
    generation: u64,
    digest: String,
    expires_at: u64,
}

pub struct Hub {
    pub db: Arc<Database>,
    pub auth: Auth,
    pub issuer: String,
    pub audience: String,
    pub bootstrap_sub: String,
    pub require_mfa: bool,
    pub allow_guests: bool,
    mac_key: Secret,
    leases: Mutex<HashMap<RuntimeId, Arc<IdentityLease>>>,
    rates: Mutex<HashMap<String, VecDeque<Instant>>>,
    sources: Mutex<HashMap<(String, String), Arc<IdentityLease>>>,
}
impl Hub {
    #[cfg(test)]
    pub(crate) fn leases_clear_for_test(&self) {
        self.leases.lock().unwrap().clear();
    }
    pub async fn new(config: &crate::serve::ServeConfig) -> Result<Arc<Self>, Error> {
        #[cfg(test)]
        let vault = None;
        #[cfg(not(test))]
        let vault = cyber_account::credential_vault();
        Self::with_vault(config, vault).await
    }
    /// Hosts can inject their protected secret store. Tests use an isolated fake
    /// vault or the private Unix fallback, never a developer's native keychain.
    pub async fn with_vault(
        config: &crate::serve::ServeConfig,
        vault: Option<Arc<dyn cyber_account::CredentialVault>>,
    ) -> Result<Arc<Self>, Error> {
        config.validate()?;
        let database = if config.database.is_absolute() {
            config.database.clone()
        } else {
            std::env::current_dir()
                .map_err(|_| Error::storage())?
                .join(&config.database)
        };
        let db = Arc::new(Database::open(&database)?);
        let mac_key = join_key(&db, &database, vault).await?;
        let auth = Auth::new(
            &config.issuer,
            &config.audience,
            config
                .introspection
                .clone()
                .ok_or_else(|| Error::invalid("backend credential is required"))?,
            config.allow_loopback_http,
        )
        .await?;
        Ok(Arc::new(Self {
            db,
            auth,
            issuer: config.issuer.clone(),
            audience: config.audience.clone(),
            bootstrap_sub: config.bootstrap_sub.clone(),
            require_mfa: config.require_mfa,
            allow_guests: config.allow_guests,
            mac_key,
            leases: Mutex::new(HashMap::new()),
            rates: Mutex::new(HashMap::new()),
            sources: Mutex::new(HashMap::new()),
        }))
    }
    async fn authenticate(&self, headers: &HeaderMap, fresh: bool) -> Result<Actor, Error> {
        let token = headers
            .get("authorization")
            .and_then(|h| h.to_str().ok())
            .and_then(|s| s.strip_prefix("Bearer "))
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                Error::new("UnauthenticatedError", "Bearer authentication is required")
            })?;
        if token.starts_with("ccap_") {
            let digest = hash(token.as_bytes());
            let cap=self.db.read(move|db|{
                let mut stmt=db.prepare("SELECT body FROM ch_objects WHERE kind='publication' AND json_extract(body,'$.digest')=?1 LIMIT 1")?;
                let body:String=stmt.query_row([digest],|r|r.get(0)).map_err(|_|Error::forbidden())?;decode::<Publication>(&body)
            })?;
            let proof = self
                .leases
                .lock()
                .map_err(|_| Error::storage())?
                .get(&cap.agent.runtime)
                .cloned()
                .ok_or_else(Error::forbidden)?;
            if !proof.is_current() || now() >= cap.expires_at || !cap.agent.active {
                return Err(Error::forbidden());
            }
            let actor = Actor {
                proof,
                member: None,
                agent: Some(cap.agent.clone()),
            };
            let copy = actor.clone();
            let project = cap.agent.project.to_string();
            return self.db.read(move |db| {
                let actor = copy.scope(db, &project, false)?;
                let member = actor.member.as_ref().ok_or_else(Error::forbidden)?;
                let runtime: RuntimeRecord = get(db, "runtime", cap.agent.runtime.as_str())?;
                if !runtime.active
                    || member.generation != cap.generation
                    || runtime.generation != member.generation
                {
                    return Err(Error::forbidden());
                }
                Ok(actor)
            });
        }
        let proof = self
            .auth
            .authenticate(Secret::new(token.to_owned()), fresh)
            .await?;
        self.remember(proof.clone())?;
        Ok(Actor {
            proof,
            member: None,
            agent: None,
        })
    }
    fn rate(&self, actor: &Actor, project: Option<&str>) -> Result<(), Error> {
        let a = actor.proof.account();
        let member = format!("member:{}:{}", a.issuer, a.sub);
        let mut rates = self.rates.lock().map_err(|_| Error::storage())?;
        let current = Instant::now();
        rates.retain(|_, v| {
            v.back()
                .is_some_and(|t| current.duration_since(*t).as_secs() < 60)
        });
        if rates.len() > 10000 {
            return Err(Error::new("RateLimitError", "hub admission is full"));
        }
        for (key, max) in [
            (member, 120),
            (format!("project:{}", project.unwrap_or("global")), 3000),
        ] {
            let q = rates.entry(key).or_default();
            while q
                .front()
                .is_some_and(|t| current.duration_since(*t).as_secs() >= 60)
            {
                q.pop_front();
            }
            if q.len() >= max {
                return Err(Error::new(
                    "RateLimitError",
                    "retry after the current minute",
                ));
            }
            q.push_back(current);
        }
        Ok(())
    }
}
pub fn router(hub: Arc<Hub>) -> Router {
    Router::new()
        .route("/api/v1/hub/{*path}", any(handle))
        .layer(DefaultBodyLimit::max(65536))
        .with_state(hub)
}
impl IntoResponse for Error {
    fn into_response(self) -> Response {
        let status = match self.tag.as_str() {
            "UnauthenticatedError" => StatusCode::UNAUTHORIZED,
            "ForbiddenError" | "InactiveIdentityError" | "StepUpRequiredError" => {
                StatusCode::FORBIDDEN
            }
            "ProjectNotFoundError" => StatusCode::NOT_FOUND,
            "RevisionConflictError"
            | "OperationConflictError"
            | "DeliveryTerminalConflictError" => StatusCode::CONFLICT,
            "RateLimitError" => StatusCode::TOO_MANY_REQUESTS,
            "StorageUnavailableError" | "StorageBusyError" | "LiveValidationUnavailableError" => {
                StatusCode::SERVICE_UNAVAILABLE
            }
            _ => StatusCode::BAD_REQUEST,
        };
        (status, [("cache-control", "no-store")], Json(self)).into_response()
    }
}
async fn handle(
    State(hub): State<Arc<Hub>>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, Error> {
    let path = uri
        .path()
        .strip_prefix("/api/v1/hub/")
        .ok_or_else(Error::missing)?;
    if method == Method::GET && path == "openapi.json" {
        return Ok(Json(crate::openapi::document()).into_response());
    }
    if method == Method::GET && path == "hello" {
        return Ok(Json(Data {
            data: HubHello {
                hub_id: hub.db.hub_id.clone(),
                issuer: hub.issuer.clone(),
                audience: hub.audience.clone(),
            },
        })
        .into_response());
    }
    let mut actor = hub.authenticate(&headers, method != Method::GET).await?;
    let parts: Vec<String> = path.split('/').map(str::to_owned).collect();
    let project = if parts.first().is_some_and(|s| s == "projects") {
        parts.get(1).map(String::as_str)
    } else {
        None
    };
    hub.rate(&actor, project)?;
    if parts.get(2).is_some_and(|p| p == "deliveries") {
        let project = parts[1].clone();
        let copy = actor.clone();
        hub.db.write(move|db|{copy.scope(db,&project,false)?;db.execute("UPDATE ch_objects SET body=json_set(body,'$.state','expired') WHERE kind='delivery' AND project=?1 AND json_extract(body,'$.message.expires_at')<=?2 AND json_extract(body,'$.state') IN ('queued','held')",params![project,now() as i64])?;Ok(())})?;
    }
    if parts.get(2).is_some_and(|p| p == "deliveries")
        && parts.last().is_some_and(|p| p == "authorization")
        && parts.len() >= 5
    {
        let recipient = actor.proof.account();
        actor.proof = hub
            .auth
            .subject(&recipient.issuer, &recipient.sub, true)
            .await?;
        let id = parts[3].clone();
        let copy = actor.clone();
        let project = parts[1].clone();
        let source = hub.db.read(move |db| {
            copy.scope(db, &project, false)?;
            let message: Message = get(db, "message", &id)?;
            if message.project.as_str() != project {
                return Err(Error::missing());
            }
            get::<Member>(db, "member", message.owner.as_str())
        })?;
        match hub
            .auth
            .subject(&source.issuer, &source.subject, true)
            .await
        {
            Ok(proof) => hub.remember(proof)?,
            Err(_) => {
                hub.sources
                    .lock()
                    .map_err(|_| Error::storage())?
                    .remove(&(source.issuer, source.subject));
            }
        }
    }
    let mut enrollment = hub.enrollment(None);
    if crate::enrollment::handles(&parts.iter().map(String::as_str).collect::<Vec<_>>()) {
        let paths = parts.clone();
        let copy = actor.clone();
        let config = hub.enrollment(None);
        let verb = method.to_string();
        let recipient = hub.db.read(move |db| {
            crate::enrollment::recipient(
                db,
                &copy,
                &paths.iter().map(String::as_str).collect::<Vec<_>>(),
                &verb,
                &config,
            )
        })?;
        if let Some((issuer, subject)) = recipient {
            enrollment.recipient_proof = Some(hub.auth.subject(&issuer, &subject, true).await?);
        }
    }
    if method == Method::GET {
        let query = uri.query().unwrap_or_default().to_owned();
        let hub_copy = hub.clone();
        let result = hub.db.read(move |db| {
            let path = parts.iter().map(String::as_str).collect::<Vec<_>>();
            if crate::enrollment::handles(&path) {
                crate::enrollment::read(db, &actor, &path, &enrollment)
            } else {
                read(db, &hub_copy, &actor, &parts, &query)
            }
        })?;
        return Ok(([("cache-control", "no-store")], Json(Data { data: result })).into_response());
    }
    let input: Mutation<Value> = serde_json::from_slice(&body)
        .map_err(|_| Error::invalid("expected mutation operation_id and payload"))?;
    // A heartbeat may replace a runtime's proof, but cannot extend the deadline
    // of an earlier proof. The lease map is intentionally empty after restart.
    let lease = actor.proof.clone();
    let hub_copy = hub.clone();
    let route = format!("{} {}", method, path);
    let path_owned = path.to_owned();
    let result = hub.db.write(move |db| {
        let path = parts.iter().map(String::as_str).collect::<Vec<_>>();
        let enroll = crate::enrollment::handles(&path);
        if enroll {
            crate::enrollment::authorize(db, &actor, &path, method.as_str(), &enrollment)?;
        } else {
            authorize(db, &hub_copy, &actor, &path, &method)?;
        }
        let fingerprint = if enroll {
            crate::enrollment::idempotency_payload(&path, &input.payload, &enrollment)?
        } else {
            input.payload.clone()
        };
        let result = mutation(
            db,
            &actor,
            &route,
            &input.operation_id,
            &fingerprint,
            || {
                if enroll {
                    crate::enrollment::mutate(
                        db,
                        &actor,
                        &path,
                        method.as_str(),
                        input.payload.clone(),
                        &enrollment,
                    )
                } else {
                    mutate(
                        db,
                        &hub_copy,
                        &actor,
                        &parts,
                        &method,
                        input.payload.clone(),
                    )
                }
            },
        )?;
        if let Ok(reg) = serde_json::from_value::<RuntimeRegistration>(result.data.clone()) {
            let runtime: RuntimeRecord = get(db, "runtime", reg.runtime.as_str())?;
            let member = actor
                .scope(db, path.get(1).ok_or_else(Error::missing)?, true)?
                .member
                .ok_or_else(Error::forbidden)?;
            if !runtime.active
                || runtime.owner != member.id
                || runtime.generation != member.generation
            {
                return Err(Error::forbidden());
            }
        }
        Ok(result)
    })?;
    if path_owned.contains("/runtimes")
        && !path_owned.ends_with("/agents")
        && let Ok(reg) = serde_json::from_value::<RuntimeRegistration>(result.data.clone())
    {
        let mut leases = hub.leases.lock().map_err(|_| Error::storage())?;
        leases.retain(|_, proof| proof.is_current());
        leases.insert(reg.runtime, lease);
    }
    Ok(([("cache-control", "no-store")], Json(result)).into_response())
}
fn parse<T: DeserializeOwned>(value: Value) -> Result<T, Error> {
    serde_json::from_value(value).map_err(|_| Error::invalid("invalid operation payload"))
}
fn value<T: Serialize>(value: T) -> Result<Value, Error> {
    serde_json::to_value(value).map_err(|_| Error::storage())
}
fn visible(db: &Connection, actor: &Actor, message: &Message) -> Result<bool, Error> {
    if message.target == MessageTarget::Project {
        return Ok(true);
    }
    let participant = actor.participant()?;
    if message.author == participant || message.target == MessageTarget::Direct(participant) {
        return Ok(true);
    }
    if actor.agent.is_some() {
        return Ok(false);
    }
    let own = actor.member.as_ref().ok_or_else(Error::forbidden)?;
    for p in [
        Some(&message.author),
        match &message.target {
            MessageTarget::Direct(p) => Some(p),
            _ => None,
        },
    ]
    .into_iter()
    .flatten()
    {
        if let Participant::Agent(id) = p {
            let agent: Agent = get(db, "agent", id.as_str())?;
            if agent.owner == own.id {
                return Ok(true);
            }
        }
    }
    Ok(false)
}
fn agent_owner(db: &Connection, actor: &Actor, id: &str) -> Result<Agent, Error> {
    let agent: Agent = get(db, "agent", id)?;
    if actor.agent.as_ref().is_some_and(|a| a.id != agent.id)
        || actor
            .member
            .as_ref()
            .is_none_or(|m| m.id != agent.owner || m.project != agent.project)
    {
        return Err(Error::forbidden());
    }
    Ok(agent)
}
fn delivery_allowed(
    db: &Connection,
    hub: &Hub,
    actor: &Actor,
    message: &Message,
    target: &str,
) -> Result<DeliveryAuthorization, Error> {
    let recipient = agent_owner(db, actor, target)?;
    let delivery: Delivery = get(db, "delivery", &format!("{}:{target}", message.id))?;
    let publication: Publication = get(db, "publication", recipient.id.as_str())?;
    let recipient_generation = actor
        .member
        .as_ref()
        .ok_or_else(Error::forbidden)?
        .generation;
    let receiving = publication.expires_at > now()
        && publication.generation == recipient_generation
        && recipient.active
        && hub.runtime_live(&recipient.runtime);
    let current_delivery = !matches!(
        delivery.state,
        DeliveryState::Refused | DeliveryState::Dropped | DeliveryState::Expired
    ) && (message.target != MessageTarget::Project
        || publication.subscribed)
        && now() < message.expires_at;
    let author: Member = get(db, "member", message.owner.as_str())?;
    let proof = hub.auth_cached_source(db, &author)?;
    let source_active = author.active && author.generation == message.member_generation && proof;
    let published = match &message.author {
        Participant::Agent(id) => {
            let agent: Agent = get(db, "agent", id.as_str())?;
            agent.active && hub.runtime_live(&agent.runtime)
        }
        _ => true,
    };
    let reason = if !receiving {
        Some("recipient_not_current")
    } else if !current_delivery {
        Some("delivery_not_current")
    } else if !source_active || !published {
        Some("source_not_current")
    } else {
        None
    };
    Ok(DeliveryAuthorization {
        allowed: reason.is_none(),
        reason: reason.map(str::to_owned),
    })
}
impl Hub {
    fn enrollment(
        &self,
        recipient_proof: Option<Arc<IdentityLease>>,
    ) -> crate::enrollment::EnrollmentConfig {
        crate::enrollment::EnrollmentConfig {
            bootstrap_sub: self.bootstrap_sub.clone(),
            allow_guests: self.allow_guests,
            require_mfa: self.require_mfa,
            mac_key: self.mac_key.clone(),
            recipient_proof,
        }
    }
    fn runtime_live(&self, id: &RuntimeId) -> bool {
        self.leases
            .lock()
            .ok()
            .and_then(|m| m.get(id).cloned())
            .is_some_and(|p| p.is_current())
    }
    fn remember(&self, proof: Arc<IdentityLease>) -> Result<(), Error> {
        let mut sources = self.sources.lock().map_err(|_| Error::storage())?;
        sources.retain(|_, p| p.is_current());
        let a = proof.account();
        let key = (a.issuer.clone(), a.sub.clone());
        if sources.len() >= 1024 && !sources.contains_key(&key) {
            return Err(Error::new("RateLimitError", "identity admission full"));
        }
        sources.insert(key, proof);
        Ok(())
    }
    fn auth_cached_source(&self, db: &Connection, member: &Member) -> Result<bool, Error> {
        let proof = self
            .sources
            .lock()
            .map_err(|_| Error::storage())?
            .get(&(member.issuer.clone(), member.subject.clone()))
            .cloned();
        let Some(proof) = proof.filter(|p| p.is_current()) else {
            return Ok(false);
        };
        let project: Project = get(db, "project", member.project.as_str())?;
        Ok(match (&project.mode, member.kind) {
            (ProjectMode::Organization { organization_id }, MembershipKind::Internal) => {
                proof.require_organization(organization_id).is_ok()
            }
            (ProjectMode::Organization { .. }, MembershipKind::Guest) => project.guests_enabled,
            (ProjectMode::Friends, MembershipKind::Friend) => true,
            _ => false,
        })
    }
}
fn read(
    db: &Connection,
    hub: &Hub,
    actor: &Actor,
    path: &[String],
    query: &str,
) -> Result<Value, Error> {
    let p: Vec<&str> = path.iter().map(String::as_str).collect();
    if p == ["projects"] {
        let a = actor.proof.account();
        let mut stmt =
            db.prepare("SELECT body FROM ch_objects WHERE kind='project' ORDER BY id LIMIT 1000")?;
        let projects = stmt
            .query_map([], |r| r.get::<_, String>(0))?
            .map(|r| decode::<Project>(&r?))
            .collect::<Result<Vec<_>, _>>()?;
        let mut result = Vec::new();
        for project in projects {
            if actor.scope(db, project.id.as_str(), false).is_ok() {
                result.push(project);
            }
        }
        let _ = a;
        return value(result);
    }
    let project = *p.get(1).ok_or_else(Error::missing)?;
    let actor = actor.scope(db, project, false)?;
    match p.as_slice() {
        ["projects", _] => value(get::<Project>(db, "project", project)?),
        ["projects", _, "members"] => value(list::<Member>(db, "member", project)?),
        ["projects", _, "agents"] => {
            let mut agents = list::<Agent>(db, "agent", project)?;
            for a in &mut agents {
                a.online = a.active && hub.runtime_live(&a.runtime);
            }
            value(agents)
        }
        ["projects", _, "messages"] => {
            let after = url::form_urlencoded::parse(query.as_bytes())
                .find(|(k, _)| k == "after")
                .map(|(_, v)| v.into_owned());
            value(message_page(db, &actor, project, after.as_deref())?)
        }
        ["projects", _, "tasks"] => value(task_views(db, hub, project)?),
        ["projects", _, "deliveries"] => {
            let target = url::form_urlencoded::parse(query.as_bytes())
                .find(|(k, _)| k == "agent")
                .map(|(_, v)| v.into_owned())
                .ok_or_else(|| Error::invalid("agent is required"))?;
            agent_owner(db, &actor, &target)?;
            let after = url::form_urlencoded::parse(query.as_bytes())
                .find(|(k, _)| k == "after")
                .map(|(_, v)| {
                    v.parse::<MessageId>()
                        .map_err(|_| Error::invalid("invalid delivery cursor"))
                })
                .transpose()?;
            let after = after.map(|id| format!("{id}:{target}")).unwrap_or_default();
            // Each indexed state range contributes at most100 candidates. Old
            // released/expired work cannot consume the receiver's page limit.
            let mut statement=db.prepare("SELECT body FROM (SELECT body,id FROM (SELECT body,id FROM ch_objects WHERE kind='delivery' AND project=?1 AND json_extract(body,'$.target')=?2 AND json_extract(body,'$.state')='queued' AND id>?3 AND json_extract(body,'$.message.expires_at')>?4 ORDER BY id LIMIT 100) UNION ALL SELECT body,id FROM (SELECT body,id FROM ch_objects WHERE kind='delivery' AND project=?1 AND json_extract(body,'$.target')=?2 AND json_extract(body,'$.state')='held' AND id>?3 AND json_extract(body,'$.message.expires_at')>?4 ORDER BY id LIMIT 100)) ORDER BY id LIMIT 100")?;
            let rows = statement
                .query_map(params![project, target, after, now() as i64], |r| {
                    r.get::<_, String>(0)
                })?
                .map(|row| decode::<Delivery>(&row?))
                .collect::<Result<Vec<_>, _>>()?;
            value(rows)
        }
        ["projects", _, "deliveries", msg, target, "authorization"] => {
            let message: Message = get(db, "message", msg)?;
            if message.project.as_str() != project {
                return Err(Error::missing());
            }
            value(delivery_allowed(db, hub, &actor, &message, target)?)
        }
        ["projects", _, "snapshot"] => {
            let messages = message_page(db, &actor, project, None)?;
            let cursor: i64 = db.query_row(
                "SELECT COALESCE(MAX(cursor),0) FROM ch_events WHERE project=?1",
                [project],
                |r| r.get(0),
            )?;
            value(Snapshot {
                project: get(db, "project", project)?,
                members: list(db, "member", project)?,
                agents: list(db, "agent", project)?,
                tasks: task_views(db, hub, project)?,
                messages,
                cursor: cursor as u64,
            })
        }
        ["projects", _, "events"] => events(db, &actor, project, query),
        _ => Err(Error::missing()),
    }
}
pub(crate) fn message_page(
    db: &Connection,
    actor: &Actor,
    project: &str,
    after: Option<&str>,
) -> Result<Vec<Message>, Error> {
    let cursor = after
        .map(|id| {
            id.parse::<MessageId>()
                .map(|id| id.to_string())
                .map_err(|_| Error::invalid("invalid message cursor"))
        })
        .transpose()?;
    let direction = if after.is_some() { "ASC" } else { "DESC" };
    // Each ordered primary-key lookup contributes at most 500 IDs. UNION and the
    // final sort therefore handle at most 1,000 authorized candidates, regardless
    // of other members' private traffic.
    let sql = format!(
        "WITH project_visible AS (SELECT message FROM ch_visibility WHERE project=?1 AND participant='project' AND message>?3 ORDER BY message {direction} LIMIT 500), own_visible AS (SELECT message FROM ch_visibility WHERE project=?1 AND participant=?2 AND message>?3 ORDER BY message {direction} LIMIT 500), candidates AS (SELECT message FROM project_visible UNION SELECT message FROM own_visible) SELECT m.body FROM candidates c JOIN ch_objects m ON m.kind='message' AND m.id=c.message AND m.project=?1 ORDER BY m.id {direction} LIMIT 500"
    );
    let participant = crate::storage::visibility::participant_key(&actor.participant()?);
    let mut statement = db.prepare(&sql)?;
    let rows = statement.query_map(
        params![project, participant, cursor.as_deref().unwrap_or("")],
        |r| r.get::<_, String>(0),
    )?;
    let mut messages = Vec::new();
    for row in rows {
        let message: Message = decode(&row?)?;
        if visible(db, actor, &message)? {
            messages.push(message);
        }
    }
    if after.is_none() {
        messages.reverse();
    }
    Ok(messages)
}

fn task_views(db: &Connection, hub: &Hub, project: &str) -> Result<Vec<Task>, Error> {
    let mut tasks = list::<Task>(db, "task", project)?;
    for task in &mut tasks {
        let state = match &task.assignee {
            Some(Participant::Human(id)) => {
                let member: Member = get(db, "member", id.as_str())?;
                if !member.active {
                    Some("revoked_unknown_work")
                } else if !hub.auth_cached_source(db, &member)? {
                    Some("offline_unknown_work")
                } else {
                    None
                }
            }
            Some(Participant::Agent(id)) => {
                let agent: Agent = get(db, "agent", id.as_str())?;
                let owner: Member = get(db, "member", agent.owner.as_str())?;
                if !owner.active || !agent.active {
                    Some("revoked_unknown_work")
                } else if !hub.runtime_live(&agent.runtime) {
                    Some("offline_unknown_work")
                } else {
                    None
                }
            }
            None => None,
        };
        if let Some(state) = state {
            task.assignee_state = Some(state.into());
        }
    }
    Ok(tasks)
}
fn events(db: &Connection, actor: &Actor, project: &str, query: &str) -> Result<Value, Error> {
    let after = url::form_urlencoded::parse(query.as_bytes())
        .find(|(k, _)| k == "after")
        .map(|(_, v)| v.parse::<u64>())
        .transpose()
        .map_err(|_| Error::invalid("invalid cursor"))?
        .unwrap_or(0);
    let first: i64 = db.query_row(
        "SELECT COALESCE(MIN(cursor),0) FROM ch_events WHERE project=?1",
        [project],
        |r| r.get(0),
    )?;
    let current: i64 = db.query_row(
        "SELECT COALESCE(MAX(cursor),0) FROM ch_events WHERE project=?1",
        [project],
        |r| r.get(0),
    )?;
    if after > current as u64 {
        return Err(Error::new(
            "ResyncRequiredError",
            "cursor is newer than restored state; fetch an authorized snapshot",
        ));
    }
    if after > 0 && first as u64 > after.saturating_add(1) {
        return Err(Error::new(
            "ResyncRequiredError",
            "cursor expired; fetch a snapshot without replaying execution",
        ));
    }
    let mut stmt=db.prepare("SELECT cursor,kind,body,created FROM ch_events WHERE project=?1 AND cursor>?2 ORDER BY cursor LIMIT 501")?;
    let rows = stmt
        .query_map(params![project, after.min(i64::MAX as u64) as i64], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let more = rows.len() > 500;
    let mut cursor = after;
    let mut events = Vec::new();
    for (c, k, b, t) in rows.into_iter().take(500) {
        cursor = c as u64;
        if (k.starts_with("invitation.") || k.starts_with("application."))
            && (actor.agent.is_some()
                || actor.member.as_ref().is_none_or(|m| m.role != Role::Owner))
        {
            continue;
        }
        let body: Value = decode(&b)?;
        if k.starts_with("message.") {
            let message: Message = parse(body.clone())?;
            if !visible(db, actor, &message)? {
                continue;
            }
        }
        events.push(ProjectEvent {
            cursor: c as u64,
            kind: k,
            data: body,
            created_at: t as u64,
        });
    }
    value(EventPage {
        events,
        cursor,
        has_more: more,
    })
}
fn mutate(
    db: &Connection,
    hub: &Hub,
    actor: &Actor,
    path: &[String],
    method: &Method,
    input: Value,
) -> Result<Value, Error> {
    let p: Vec<&str> = path.iter().map(String::as_str).collect();
    if p == ["projects"] && method == Method::POST {
        if actor.agent.is_some() || actor.proof.account().sub != hub.bootstrap_sub {
            return Err(Error::forbidden());
        }
        actor.proof.require_step_up(hub.require_mfa)?;
        let request: CreateProject = parse(input)?;
        if request.name.trim().is_empty() || request.name.len() > 200 {
            return Err(Error::invalid("project name must be 1–200 bytes"));
        }
        let kind = match &request.mode {
            ProjectMode::Organization { organization_id } => {
                actor.proof.require_organization(organization_id)?;
                MembershipKind::Internal
            }
            ProjectMode::Friends => MembershipKind::Friend,
        };
        let project = Project {
            id: ProjectId::new(),
            name: request.name,
            repository: Repository::parse(&request.repository)?,
            mode: request.mode,
            enrollment: Enrollment::InviteOnly,
            guests_enabled: false,
            revision: 1,
            created_at: now(),
        };
        let account = actor.proof.account();
        let member = Member {
            id: MemberId::new(),
            project: project.id.clone(),
            issuer: account.issuer.clone(),
            subject: account.sub.clone(),
            name: account.name.clone().unwrap_or_else(|| account.sub.clone()),
            role: Role::Owner,
            kind,
            generation: 1,
            active: true,
        };
        put(
            db,
            "project",
            project.id.as_str(),
            project.id.as_str(),
            &project,
        )?;
        put(
            db,
            "member",
            member.id.as_str(),
            project.id.as_str(),
            &member,
        )?;
        event(db, project.id.as_str(), "project.created", &project)?;
        return value(project);
    }
    let project = *p.get(1).ok_or_else(Error::missing)?;
    let actor = actor.scope(db, project, true)?;
    match p.as_slice() {
        ["projects", _, "runtimes"] if method == Method::POST => {
            if actor.agent.is_some() {
                return Err(Error::forbidden());
            }
            let _: RegisterRuntime = parse(input)?;
            let member = actor.member.as_ref().ok_or_else(Error::forbidden)?;
            if list::<RuntimeRecord>(db, "runtime", project)?
                .iter()
                .filter(|r| r.active && r.owner == member.id)
                .count()
                >= 128
            {
                return Err(Error::invalid(
                    "revoke an old runtime before registering more than 128",
                ));
            }
            let reg = RuntimeRegistration {
                runtime: RuntimeId::new(),
                lease_expires_at: now()
                    + actor
                        .proof
                        .deadline()
                        .saturating_duration_since(Instant::now())
                        .as_secs(),
            };
            let record = RuntimeRecord {
                registration: reg.clone(),
                owner: member.id.clone(),
                generation: member.generation,
                active: true,
            };
            put(db, "runtime", reg.runtime.as_str(), project, &record)?;
            value(reg)
        }
        ["projects", _, "runtimes", runtime, "heartbeat"] => {
            if actor.agent.is_some() {
                return Err(Error::forbidden());
            }
            let request: Heartbeat = parse(input)?;
            let mut record: RuntimeRecord = get(db, "runtime", runtime)?;
            let member = actor.member.as_ref().ok_or_else(Error::forbidden)?;
            if !record.active || record.owner != member.id || record.generation != member.generation
            {
                return Err(Error::forbidden());
            }
            if request.agents.len() > 1000 {
                return Err(Error::invalid("too many heartbeat entries"));
            }
            for presence in request.agents {
                let mut agent = agent_owner(db, &actor, presence.agent.as_str())?;
                if agent.runtime != record.registration.runtime {
                    return Err(Error::forbidden());
                }
                agent.online = true;
                put(db, "agent", agent.id.as_str(), project, &agent)?;
            }
            record.registration.lease_expires_at = now()
                + actor
                    .proof
                    .deadline()
                    .saturating_duration_since(Instant::now())
                    .as_secs();
            put(db, "runtime", runtime, project, &record)?;
            value(record.registration)
        }
        ["projects", _, "runtimes", runtime] if method == Method::DELETE => {
            if actor.agent.is_some() {
                return Err(Error::forbidden());
            }
            let mut record: RuntimeRecord = get(db, "runtime", runtime)?;
            if actor.member.as_ref().is_none_or(|m| m.id != record.owner) {
                return Err(Error::forbidden());
            }
            record.active = false;
            put(db, "runtime", runtime, project, &record)?;
            for mut agent in list::<Agent>(db, "agent", project)? {
                if agent.runtime.as_str() == *runtime {
                    agent.active = false;
                    agent.online = false;
                    put(db, "agent", agent.id.as_str(), project, &agent)?;
                }
            }
            Ok(Value::Null)
        }
        ["projects", _, "agents"] => publish(db, hub, &actor, project, parse(input)?),
        ["projects", _, "agents", id, "renew"] => {
            if actor.agent.is_some() {
                return Err(Error::forbidden());
            }
            let agent = agent_owner(db, &actor, id)?;
            let mut publication: Publication = get(db, "publication", id)?;
            if !agent.active || !hub.runtime_live(&agent.runtime) {
                return Err(Error::forbidden());
            }
            let token = issue_cap()?;
            publication.digest = hash(token.expose().as_bytes());
            publication.expires_at = now() + 900;
            put(db, "publication", id, project, &publication)?;
            value(Published {
                agent,
                capability: Some(token.expose().to_owned()),
                expires_at: publication.expires_at,
            })
        }
        ["projects", _, "agents", id] if method == Method::DELETE => {
            if actor.agent.is_some() {
                return Err(Error::forbidden());
            }
            let mut agent = agent_owner(db, &actor, id)?;
            agent.active = false;
            agent.online = false;
            put(db, "agent", id, project, &agent)?;
            event(db, project, "agent.unpublished", &agent)?;
            Ok(Value::Null)
        }
        ["projects", _, "messages"] => send(db, &actor, project, parse(input)?),
        ["projects", _, "deliveries", msg, target, "ack"] => {
            agent_owner(db, &actor, target)?;
            let id = format!("{msg}:{target}");
            let mut delivery: Delivery = get(db, "delivery", &id)?;
            let request: Acknowledge = parse(input)?;
            if delivery.message.project.as_str() != project
                || delivery.content_hash != request.content_hash
            {
                return Err(Error::forbidden());
            }
            // This is the receiver's observation of a durable local outcome,
            // not permission to admit or execute a prompt. Fresh source checks
            // belong to the separate authorization endpoint before local release.
            if matches!(
                delivery.state,
                DeliveryState::Released
                    | DeliveryState::Dropped
                    | DeliveryState::Refused
                    | DeliveryState::Expired
            ) && delivery.state != request.state
            {
                return Err(Error::new(
                    "DeliveryTerminalConflictError",
                    "delivery already has a different terminal observation",
                ));
            }
            delivery.state = request.state;
            put(db, "delivery", &id, project, &delivery)?;
            value(delivery)
        }
        ["projects", _, "tasks"] => create_task(db, &actor, project, parse(input)?),
        ["projects", _, "tasks", task, "claim"] => {
            claim_task(db, &actor, project, task, parse(input)?)
        }
        ["projects", _, "tasks", task] if method == Method::PATCH => {
            update_task(db, &actor, project, task, parse(input)?)
        }
        ["projects", _, "tasks", task, "reconcile"] => {
            reconcile_task(db, hub, &actor, project, task, parse(input)?)
        }
        _ => Err(Error::missing()),
    }
}
fn issue_cap() -> Result<Secret, Error> {
    use base64::Engine;
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|_| Error::storage())?;
    Ok(Secret::new(format!(
        "ccap_{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
    )))
}
fn publish(
    db: &Connection,
    hub: &Hub,
    actor: &Actor,
    project: &str,
    request: Publish,
) -> Result<Value, Error> {
    if actor.agent.is_some() {
        return Err(Error::forbidden());
    }
    let p: Project = get(db, "project", project)?;
    if p.repository != request.repository {
        return Err(Error::new(
            "RepositoryMismatchError",
            "confirm the pinned project repository",
        ));
    }
    if request.publication_key.is_empty()
        || request.publication_key.len() > 200
        || request.name.is_empty()
        || request.name.len() > 200
    {
        return Err(Error::invalid("publication name/key length is invalid"));
    }
    let record: RuntimeRecord = get(db, "runtime", request.runtime.as_str())?;
    let member = actor.member.as_ref().ok_or_else(Error::forbidden)?;
    if !record.active
        || record.owner != member.id
        || record.generation != member.generation
        || !hub.runtime_live(&request.runtime)
    {
        return Err(Error::forbidden());
    }
    // An explicit revocation permanently retires the public agent identity.
    // Reusing it could revive prompts queued before unpublish or membership removal.
    use rusqlite::OptionalExtension;
    let prior:Option<String>=db.query_row("SELECT p.body FROM ch_objects p JOIN ch_objects a ON a.kind='agent' AND a.id=p.id WHERE p.kind='publication' AND p.project=?1 AND json_extract(p.body,'$.key')=?2 AND json_extract(p.body,'$.generation')=?3 AND json_extract(a.body,'$.owner')=?4 AND json_extract(a.body,'$.active')=1 ORDER BY p.id DESC LIMIT 1",params![project,request.publication_key,member.generation as i64,member.id.as_str()],|row|row.get(0)).optional()?;
    let prior: Option<Publication> = prior.map(|body| decode(&body)).transpose()?;
    let id = prior.map_or_else(AgentId::new, |p| p.agent.id);
    let agent = Agent {
        id,
        project: p.id,
        owner: member.id.clone(),
        runtime: request.runtime,
        name: request.name,
        kind: member.kind,
        active: true,
        online: true,
    };
    let token = issue_cap()?;
    let publication = Publication {
        agent: agent.clone(),
        key: request.publication_key,
        subscribed: request.subscribe_project_messages,
        generation: member.generation,
        digest: hash(token.expose().as_bytes()),
        expires_at: now() + 900,
    };
    put(db, "agent", agent.id.as_str(), project, &agent)?;
    put(db, "publication", agent.id.as_str(), project, &publication)?;
    event(db, project, "agent.published", &agent)?;
    value(Published {
        agent,
        capability: Some(token.expose().to_owned()),
        expires_at: publication.expires_at,
    })
}
fn participant_valid(db: &Connection, project: &str, p: &Participant) -> Result<(), Error> {
    match p {
        Participant::Human(id) => {
            let m: Member = get(db, "member", id.as_str())?;
            if !m.active || m.project.as_str() != project {
                return Err(Error::forbidden());
            }
        }
        Participant::Agent(id) => {
            let a: Agent = get(db, "agent", id.as_str())?;
            if !a.active || a.project.as_str() != project {
                return Err(Error::forbidden());
            }
        }
    }
    Ok(())
}
fn send(
    db: &Connection,
    actor: &Actor,
    project: &str,
    mut request: SendMessage,
) -> Result<Value, Error> {
    if request.text.is_empty()
        || request.text.len() > 16384
        || request.depth > 8
        || request.hops == 0
        || request.hops > 32
    {
        return Err(Error::invalid("message size or chain bound exceeded"));
    }
    if let MessageTarget::Direct(p) = &request.target {
        participant_valid(db, project, p)?;
    }
    let author = actor.participant()?;
    if let Some(id) = &request.reply_to {
        let parent: Message = get(db, "message", id.as_str())?;
        if parent.project.as_str() != project || !visible(db, actor, &parent)? {
            return Err(Error::forbidden());
        }
        // A human reply is an explicit new user action; clients need not expose
        // internal counters. Agent replies must retain their runtime's chain.
        if actor.agent.is_none() {
            request.hops = parent.hops.saturating_add(1);
        }
        if request.hops <= parent.hops || request.hops > 32 {
            return Err(Error::invalid("message chain bound exceeded"));
        }
        let conversation = match (&parent.target, &request.target) {
            (MessageTarget::Project, MessageTarget::Project) => true,
            (MessageTarget::Direct(a), MessageTarget::Direct(b)) => {
                (parent.author == author && a == b) || (parent.author == *b && *a == author)
            }
            _ => false,
        };
        if !conversation {
            return Err(Error::forbidden());
        }
    }
    let member = actor.member.as_ref().ok_or_else(Error::forbidden)?;
    let message = Message {
        id: MessageId::new(),
        project: member.project.clone(),
        author,
        owner: member.id.clone(),
        membership_kind: member.kind,
        member_generation: member.generation,
        depth: request.depth,
        hops: request.hops,
        target: request.target,
        reply_to: request.reply_to,
        text: request.text,
        created_at: now(),
        expires_at: now() + 7 * 86400,
    };
    put(db, "message", message.id.as_str(), project, &message)?;
    for publication in list::<Publication>(db, "publication", project)? {
        let agent = publication.agent;
        let current: Agent = get(db, "agent", agent.id.as_str())?;
        if !current.active {
            continue;
        }
        let targeted = match &message.target {
            MessageTarget::Direct(Participant::Agent(id)) => *id == agent.id,
            MessageTarget::Project => publication.subscribed,
            _ => false,
        };
        if targeted {
            let delivery = Delivery {
                content_hash: hash(encode(&message)?.as_bytes()),
                message: message.clone(),
                target: agent.id.clone(),
                state: DeliveryState::Queued,
            };
            put(
                db,
                "delivery",
                &format!("{}:{}", message.id, agent.id),
                project,
                &delivery,
            )?;
        }
    }
    event(db, project, "message.created", &message)?;
    value(message)
}
fn dependencies(
    db: &Connection,
    project: &str,
    id: Option<&TaskId>,
    deps: &[TaskId],
) -> Result<(), Error> {
    if deps.len() > 100 {
        return Err(Error::invalid("too many dependencies"));
    }
    let mut seen = std::collections::HashSet::new();
    let mut pending = deps.to_vec();
    while let Some(next) = pending.pop() {
        if Some(&next) == id {
            return Err(Error::invalid("task dependencies must be acyclic"));
        }
        if !seen.insert(next.clone()) {
            continue;
        }
        if seen.len() > 10000 {
            return Err(Error::invalid("dependency graph exceeds bound"));
        }
        let task: Task = get(db, "task", next.as_str())?;
        if task.project.as_str() != project {
            return Err(Error::forbidden());
        }
        pending.extend(task.dependencies);
    }
    Ok(())
}
fn create_task(
    db: &Connection,
    actor: &Actor,
    project: &str,
    request: CreateTask,
) -> Result<Value, Error> {
    if request.title.trim().is_empty()
        || request.title.len() > 300
        || request.description.len() > 16384
    {
        return Err(Error::invalid("invalid task size"));
    }
    dependencies(db, project, None, &request.dependencies)?;
    let task = Task {
        id: TaskId::new(),
        project: project.parse().map_err(|_| Error::missing())?,
        creator: actor.participant()?,
        title: request.title,
        description: request.description,
        status: TaskStatus::Todo,
        assignee: None,
        assignee_state: None,
        dependencies: request.dependencies,
        revision: 1,
        claim_generation: 0,
        reports: Vec::new(),
        created_at: now(),
        updated_at: now(),
    };
    put(db, "task", task.id.as_str(), project, &task)?;
    event(db, project, "task.created", &task)?;
    value(task)
}
fn check_task(db: &Connection, project: &str, id: &str, revision: u64) -> Result<Task, Error> {
    let task: Task = get(db, "task", id)?;
    if task.project.as_str() != project {
        return Err(Error::missing());
    }
    if task.revision != revision {
        return Err(Error::conflict(task.revision));
    }
    Ok(task)
}
fn save_task(db: &Connection, project: &str, mut task: Task) -> Result<Value, Error> {
    task.revision += 1;
    task.updated_at = now();
    put(db, "task", task.id.as_str(), project, &task)?;
    event(db, project, "task.updated", &task)?;
    value(task)
}
fn claim_task(
    db: &Connection,
    actor: &Actor,
    project: &str,
    id: &str,
    request: ClaimTask,
) -> Result<Value, Error> {
    let mut task = check_task(db, project, id, request.expected_revision)?;
    if task.assignee.is_some() || matches!(task.status, TaskStatus::Done | TaskStatus::Cancelled) {
        return Err(Error::conflict(task.revision));
    }
    for dep in &task.dependencies {
        let dep: Task = get(db, "task", dep.as_str())?;
        if dep.status != TaskStatus::Done {
            return Err(Error::invalid("dependencies are not completed"));
        }
    }
    task.assignee = Some(actor.participant()?);
    task.assignee_state = Some("active".into());
    task.claim_generation += 1;
    task.status = TaskStatus::InProgress;
    save_task(db, project, task)
}
fn owns_claim(db: &Connection, actor: &Actor, task: &Task) -> Result<bool, Error> {
    let own = actor.participant()?;
    if task.assignee.as_ref() == Some(&own) {
        return Ok(true);
    }
    if actor.agent.is_none()
        && let Some(Participant::Agent(id)) = &task.assignee
    {
        let a: Agent = get(db, "agent", id.as_str())?;
        return Ok(actor.member.as_ref().is_some_and(|m| m.id == a.owner));
    }
    Ok(false)
}
fn update_task(
    db: &Connection,
    actor: &Actor,
    project: &str,
    id: &str,
    request: UpdateTask,
) -> Result<Value, Error> {
    let mut task = check_task(db, project, id, request.expected_revision)?;
    if request.claim_generation != task.claim_generation || !owns_claim(db, actor, &task)? {
        return Err(Error::conflict(task.revision));
    }
    if matches!(task.status, TaskStatus::Done | TaskStatus::Cancelled) {
        return Err(Error::invalid(
            "terminal task requires owner reconciliation",
        ));
    }
    if let Some(title) = request.title {
        if title.trim().is_empty() || title.len() > 300 {
            return Err(Error::invalid("invalid title"));
        }
        task.title = title;
    }
    if let Some(description) = request.description {
        if description.len() > 16384 {
            return Err(Error::invalid("description exceeds bound"));
        }
        task.description = description;
    }
    if let Some(deps) = request.dependencies {
        dependencies(db, project, Some(&task.id), &deps)?;
        task.dependencies = deps;
    }
    if let Some(status) = request.status {
        task.status = status;
    }
    if let Some(report) = request.report {
        if report.text.len() > 16384
            || report.references.len() > 30
            || report.references.iter().any(|r| r.value.len() > 2048)
        {
            return Err(Error::invalid("report exceeds bound"));
        }
        task.reports.push(Report {
            author: actor.participant()?,
            text: report.text,
            references: report.references,
            created_at: now(),
        });
        if task.reports.len() > 100 {
            task.reports.remove(0);
        }
    }
    save_task(db, project, task)
}
fn reconcile_task(
    db: &Connection,
    hub: &Hub,
    actor: &Actor,
    project: &str,
    id: &str,
    request: ReconcileTask,
) -> Result<Value, Error> {
    actor.owner(db, project, hub.require_mfa)?;
    let mut task = check_task(db, project, id, request.expected_revision)?;
    if request.claim_generation != task.claim_generation
        || !request.acknowledge_unknown_work
        || request.reason.trim().is_empty()
        || request.reason.len() > 2048
    {
        return Err(Error::invalid(
            "explicit unknown-work acknowledgement and reason required",
        ));
    }
    if let Some(p) = &request.assignee {
        participant_valid(db, project, p)?;
    }
    task.assignee = request.assignee;
    task.claim_generation += 1;
    task.assignee_state = Some("owner_reconciled".into());
    task.status = if task.assignee.is_some() {
        TaskStatus::InProgress
    } else {
        TaskStatus::Todo
    };
    task.reports.push(Report {
        author: actor.participant()?,
        text: request.reason,
        references: vec![],
        created_at: now(),
    });
    save_task(db, project, task)
}

fn authorize(
    db: &Connection,
    hub: &Hub,
    actor: &Actor,
    path: &[&str],
    method: &Method,
) -> Result<(), Error> {
    if path == ["projects"] {
        if method != Method::POST
            || actor.agent.is_some()
            || actor.proof.account().sub != hub.bootstrap_sub
        {
            return Err(Error::forbidden());
        }
        actor.proof.require_step_up(hub.require_mfa)?;
        return Ok(());
    }
    let project = path.get(1).ok_or_else(Error::missing)?;
    let actor = actor.scope(db, project, true)?;
    if path
        .get(2)
        .is_some_and(|p| matches!(*p, "runtimes" | "agents"))
        && actor.agent.is_some()
    {
        return Err(Error::forbidden());
    }
    if path.last() == Some(&"reconcile") {
        actor.owner(db, project, hub.require_mfa)?;
    }
    Ok(())
}
async fn join_key(
    db: &Database,
    path: &std::path::Path,
    vault: Option<Arc<dyn cyber_account::CredentialVault>>,
) -> Result<Secret, Error> {
    let expected: Option<String> = db.read(|db| {
        use rusqlite::OptionalExtension;
        Ok(db
            .query_row(
                "SELECT value FROM ch_meta WHERE key='join_key_digest'",
                [],
                |r| r.get(0),
            )
            .optional()?)
    })?;
    let directory = path
        .parent()
        .ok_or_else(|| Error::invalid("hub database needs a parent directory"))?
        .to_path_buf();
    let secrets = cyber_account::HostSecrets::new(
        directory,
        "hub-secrets.json",
        &db.hub_id.to_string(),
        vault,
    )?;
    if let Some(key) = secrets.get("join-mac").await? {
        if expected
            .as_ref()
            .is_some_and(|e| *e != hash(key.expose().as_bytes()))
        {
            return Err(Error::new(
                "HubKeyMismatchError",
                "restore the hub MAC key with its database",
            ));
        }
        if expected.is_none() {
            let digest = hash(key.expose().as_bytes());
            db.write(move |db| {
                db.execute("INSERT INTO ch_meta VALUES('join_key_digest',?1)", [digest])?;
                Ok(())
            })?;
        }
        return Ok(key);
    }
    if expected.is_some() {
        return Err(Error::new(
            "HubKeyMissingError",
            "restore the hub MAC key with its database",
        ));
    }
    let key = issue_cap()?;
    secrets.put("join-mac", key.clone()).await?;
    let digest = hash(key.expose().as_bytes());
    db.write(move |db| {
        db.execute("INSERT INTO ch_meta VALUES('join_key_digest',?1)", [digest])?;
        Ok(())
    })?;
    Ok(key)
}

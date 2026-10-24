//! Transactional project enrollment. Codes are owner-visible approval secrets;
//! personal account MFA always remains at the identity provider.
use crate::{api::*, error::Error, ids::*, model::*, storage::*};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use cyber_account::{IdentityLease, Secret};
use hmac::{Hmac, Mac};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use sha2::Sha256;
use std::sync::Arc;
use subtle::ConstantTimeEq;

#[derive(Clone)]
pub struct EnrollmentConfig {
    pub bootstrap_sub: String,
    pub allow_guests: bool,
    pub require_mfa: bool,
    pub mac_key: Secret,
    pub recipient_proof: Option<Arc<IdentityLease>>,
}
#[derive(Clone, Serialize, Deserialize)]
struct Invitation {
    view: InvitationView,
    issuer: String,
    digest: String,
}
#[derive(Clone, Serialize, Deserialize)]
struct Application {
    view: ApplicationView,
    issuer: String,
    name: String,
    invitation: Option<InvitationId>,
}
#[derive(Clone, Serialize, Deserialize)]
struct Challenge {
    id: ChallengeId,
    application: ApplicationId,
    project: ProjectId,
    issuer: String,
    subject: String,
    role: Role,
    invitation: Option<InvitationId>,
    mac: String,
    expires_at: u64,
    attempts: u32,
    active: bool,
    created_at: u64,
}

pub fn handles(path: &[&str]) -> bool {
    matches!(
        path,
        ["join"]
            | ["applications", _]
            | ["applications", _, "redeem"]
            | ["projects", _, "invitations"]
            | ["projects", _, "invitations", _]
            | ["projects", _, "applications"]
            | ["projects", _, "applications", _, "approve" | "reject"]
            | ["projects", _, "members", _]
            | ["projects", _, "policy"]
            | ["projects", _, "leave"]
    )
}
fn parse<T: DeserializeOwned>(value: Value) -> Result<T, Error> {
    serde_json::from_value(value).map_err(|_| Error::invalid("invalid enrollment payload"))
}
fn value<T: Serialize>(value: T) -> Result<Value, Error> {
    serde_json::to_value(value).map_err(|_| Error::storage())
}
fn human(actor: &Actor) -> Result<(), Error> {
    if actor.agent.is_some() || !actor.proof.is_current() {
        Err(Error::forbidden())
    } else {
        Ok(())
    }
}
fn app(db: &Connection, id: &str) -> Result<Application, Error> {
    get(db, "application", id)
}
fn app_project(application: &Application) -> Result<&ProjectId, Error> {
    application.view.project.as_ref().ok_or_else(Error::missing)
}
fn own_application(actor: &Actor, application: &Application) -> Result<(), Error> {
    human(actor)?;
    let a = actor.proof.account();
    if application.issuer != a.issuer || application.view.subject != a.sub {
        Err(Error::forbidden())
    } else {
        Ok(())
    }
}
fn project_application(db: &Connection, project: &str, id: &str) -> Result<Application, Error> {
    let application = app(db, id)?;
    if app_project(&application)?.as_str() != project {
        return Err(Error::missing());
    }
    Ok(application)
}

/// Must run in the transaction before returning a cached mutation result.
pub fn authorize(
    db: &Connection,
    actor: &Actor,
    path: &[&str],
    method: &str,
    config: &EnrollmentConfig,
) -> Result<(), Error> {
    human(actor)?;
    match (method, path) {
        ("POST", ["join"]) => Ok(()),
        ("GET", ["applications", id]) | ("POST", ["applications", id, "redeem"]) => {
            let application = app(db, id)?;
            own_application(actor, &application)?;
            if application.view.status == ApplicationStatus::Joined {
                actor.scope(db, app_project(&application)?.as_str(), false)?;
            }
            Ok(())
        }
        ("GET", ["projects", project, "invitations" | "applications"]) => {
            actor.owner(db, project, config.require_mfa)?;
            Ok(())
        }
        ("POST", ["projects", project, "leave"]) => {
            actor.scope(db, project, false)?;
            Ok(())
        }
        ("POST", ["projects", project, "invitations"])
        | ("DELETE", ["projects", project, "invitations", _])
        | ("POST", ["projects", project, "applications", _, "approve" | "reject"])
        | ("PATCH" | "DELETE", ["projects", project, "members", _])
        | ("PATCH", ["projects", project, "policy"]) => {
            actor.owner(db, project, config.require_mfa)?;
            Ok(())
        }
        _ => Err(Error::missing()),
    }
}
/// Authorize the owner before resolving a recipient credential outside the DB.
pub fn recipient(
    db: &Connection,
    actor: &Actor,
    path: &[&str],
    method: &str,
    config: &EnrollmentConfig,
) -> Result<Option<(String, String)>, Error> {
    authorize(db, actor, path, method, config)?;
    if let ["projects", project, "applications", id, "approve"] = path {
        let application = project_application(db, project, id)?;
        return Ok(Some((application.issuer, application.view.subject)));
    }
    Ok(None)
}
/// An ordinary SHA digest of a small numeric code is an offline guessing oracle.
/// Idempotency stores a keyed binding instead, with a separate MAC domain.
pub fn idempotency_payload(
    path: &[&str],
    payload: &Value,
    config: &EnrollmentConfig,
) -> Result<Value, Error> {
    let mut protected = payload.clone();
    if matches!(path, ["applications", _, "redeem"])
        && let Some(code) = payload.get("code").and_then(Value::as_str)
    {
        protected["code"] = json!(mac(config, "operation", &(path, code))?);
    }
    Ok(protected)
}
pub fn read(
    db: &Connection,
    actor: &Actor,
    path: &[&str],
    config: &EnrollmentConfig,
) -> Result<Value, Error> {
    authorize(db, actor, path, "GET", config)?;
    match path {
        ["applications", id] => application_status(db, actor, id, config),
        ["projects", project, "invitations"] => value(
            list::<Invitation>(db, "invitation", project)?
                .into_iter()
                .map(|i| i.view)
                .collect::<Vec<_>>(),
        ),
        ["projects", project, "applications"] => value(
            list::<Application>(db, "application", project)?
                .into_iter()
                .map(|a| a.view)
                .collect::<Vec<_>>(),
        ),
        _ => Err(Error::missing()),
    }
}
fn application_status(
    db: &Connection,
    actor: &Actor,
    id: &str,
    config: &EnrollmentConfig,
) -> Result<Value, Error> {
    let application = app(db, id)?;
    own_application(actor, &application)?;
    match application.view.status {
        ApplicationStatus::Joined => {
            let project = app_project(&application)?.as_str();
            let scoped = actor.scope(db, project, false)?;
            value(JoinOutcome {
                status: ApplicationStatus::Joined,
                application: Some(application.view.clone()),
                project: Some(get::<Project>(db, "project", project)?),
                member: scoped.member,
            })
        }
        ApplicationStatus::NeedsConfirmation | ApplicationStatus::NeedsCode => {
            let project: Project = get(db, "project", app_project(&application)?.as_str())?;
            admission_invitation(db, &application, &project)?;
            let kind = membership_kind(
                &project,
                application.view.kind == MembershipKind::Guest,
                &actor.proof,
                config,
            )?;
            if kind != application.view.kind {
                return Err(Error::forbidden());
            }
            value(JoinOutcome {
                status: application.view.status,
                application: Some(application.view),
                project: Some(project),
                member: None,
            })
        }
        _ => pending_outcome(application),
    }
}

pub fn mutate(
    db: &Connection,
    actor: &Actor,
    path: &[&str],
    method: &str,
    payload: Value,
    config: &EnrollmentConfig,
) -> Result<Value, Error> {
    authorize(db, actor, path, method, config)?;
    match (method, path) {
        ("POST", ["join"]) => join(db, actor, parse(payload)?, config),
        ("POST", ["applications", id, "redeem"]) => redeem(db, actor, id, parse(payload)?, config),
        ("POST", ["projects", project, "invitations"]) => {
            invite(db, actor, project, parse(payload)?, config)
        }
        ("DELETE", ["projects", project, "invitations", id]) => revoke_invitation(db, project, id),
        ("POST", ["projects", project, "applications", id, "approve"]) => {
            approve(db, actor, project, id, config)
        }
        ("POST", ["projects", project, "applications", id, "reject"]) => reject(db, project, id),
        ("PATCH", ["projects", project, "members", id]) => {
            set_role(db, project, id, parse(payload)?, config)
        }
        ("DELETE", ["projects", project, "members", id]) => remove_member(db, project, id),
        ("POST", ["projects", project, "leave"]) => {
            let scoped = actor.scope(db, project, false)?;
            let id = scoped
                .member
                .as_ref()
                .ok_or_else(Error::forbidden)?
                .id
                .clone();
            remove_member(db, project, id.as_str())
        }
        ("PATCH", ["projects", project, "policy"]) => policy(db, project, parse(payload)?, config),
        _ => Err(Error::missing()),
    }
}
fn random_token() -> Result<String, Error> {
    let mut bytes = [0; 32];
    getrandom::fill(&mut bytes).map_err(|_| Error::storage())?;
    Ok(format!("cinv_{}", URL_SAFE_NO_PAD.encode(bytes)))
}
fn bounded_name(name: Option<String>, actor: &Actor) -> Result<String, Error> {
    let name = name
        .or_else(|| actor.proof.account().name.clone())
        .unwrap_or_else(|| actor.proof.account().sub.clone());
    if name.trim().is_empty() || name.len() > 200 || name.chars().any(char::is_control) {
        return Err(Error::invalid(
            "member name must contain 1 to 200 printable bytes",
        ));
    }
    Ok(name)
}
fn invite(
    db: &Connection,
    actor: &Actor,
    project: &str,
    request: Invite,
    config: &EnrollmentConfig,
) -> Result<Value, Error> {
    let p: Project = get(db, "project", project)?;
    if request
        .subject
        .as_ref()
        .is_some_and(|s| s.trim().is_empty() || s.len() > 512)
    {
        return Err(Error::invalid("invalid recipient subject"));
    }
    match (&p.mode, request.guest, request.role) {
        (ProjectMode::Organization { .. }, true, Role::Owner) => return Err(Error::forbidden()),
        (ProjectMode::Organization { .. }, true, _)
            if !config.allow_guests || !p.guests_enabled =>
        {
            return Err(Error::forbidden());
        }
        (ProjectMode::Friends, true, _) => {
            return Err(Error::invalid("friends projects do not issue guest grants"));
        }
        _ => {}
    }
    let token = random_token()?;
    let view = InvitationView {
        id: InvitationId::new(),
        project: p.id.clone(),
        role: request.role,
        subject: request.subject,
        guest: request.guest,
        expires_at: now() + 86400,
        revoked: false,
        used: false,
    };
    let invitation = Invitation {
        view: view.clone(),
        issuer: actor.proof.account().issuer.clone(),
        digest: hash(token.as_bytes()),
    };
    put(db, "invitation", view.id.as_str(), project, &invitation)?;
    event(db, project, "invitation.issued", &view)?;
    value(IssuedInvitation {
        invitation: view,
        token: Some(token),
    })
}
fn invitation_for_token(db: &Connection, token: &str) -> Result<Invitation, Error> {
    if token.len() > 100 || !token.starts_with("cinv_") {
        return Err(Error::forbidden());
    }
    let digest = hash(token.as_bytes());
    let body:Option<String>=db.query_row("SELECT body FROM ch_objects WHERE kind='invitation' AND json_extract(body,'$.digest')=?1",[digest],|r|r.get(0)).optional()?;
    decode(&body.ok_or_else(Error::forbidden)?)
}
fn invitation_valid(
    invitation: &Invitation,
    actor: &Actor,
    project: &Project,
) -> Result<(), Error> {
    let account = actor.proof.account();
    if invitation.view.project != project.id
        || invitation.issuer != account.issuer
        || invitation.view.revoked
        || invitation.view.used
        || invitation.view.expires_at <= now()
        || invitation
            .view
            .subject
            .as_ref()
            .is_some_and(|sub| sub != &account.sub)
    {
        return Err(Error::forbidden());
    }
    Ok(())
}
fn membership_kind(
    project: &Project,
    guest: bool,
    proof: &IdentityLease,
    config: &EnrollmentConfig,
) -> Result<MembershipKind, Error> {
    match &project.mode {
        ProjectMode::Organization { organization_id } if !guest => {
            proof.require_organization(organization_id)?;
            Ok(MembershipKind::Internal)
        }
        ProjectMode::Organization { .. } if config.allow_guests && project.guests_enabled => {
            Ok(MembershipKind::Guest)
        }
        ProjectMode::Friends if !guest => Ok(MembershipKind::Friend),
        _ => Err(Error::forbidden()),
    }
}
fn join(
    db: &Connection,
    actor: &Actor,
    request: Join,
    config: &EnrollmentConfig,
) -> Result<Value, Error> {
    actor.proof.require_step_up(config.require_mfa)?;
    let (project, invitation, role, kind, status) = if let Some(token) = request.invitation_token {
        let invitation = invitation_for_token(db, &token)?;
        let project: Project = get(db, "project", invitation.view.project.as_str())?;
        invitation_valid(&invitation, actor, &project)?;
        if request.project.as_ref().is_some_and(|id| id != &project.id) {
            return Err(Error::forbidden());
        }
        let kind = membership_kind(&project, invitation.view.guest, &actor.proof, config)?;
        let status = if invitation.view.subject.is_some() && kind == MembershipKind::Internal {
            ApplicationStatus::NeedsConfirmation
        } else {
            ApplicationStatus::PendingApproval
        };
        (
            project,
            Some(invitation.clone()),
            invitation.view.role,
            kind,
            status,
        )
    } else {
        let id = request
            .project
            .ok_or_else(|| Error::invalid("an invitation or project ID is required"))?;
        let project: Project = get(db, "project", id.as_str())?;
        let role = match project.enrollment {
            Enrollment::OrganizationMembers { role: Role::Viewer } => Role::Viewer,
            Enrollment::OrganizationMembers { role: Role::Member } => Role::Member,
            _ => return Err(Error::forbidden()),
        };
        let kind = membership_kind(&project, false, &actor.proof, config)?;
        if kind != MembershipKind::Internal {
            return Err(Error::forbidden());
        }
        (
            project,
            None,
            role,
            kind,
            ApplicationStatus::NeedsConfirmation,
        )
    };
    let account = actor.proof.account();
    if list::<Member>(db, "member", project.id.as_str())?
        .iter()
        .any(|m| m.active && m.issuer == account.issuer && m.subject == account.sub)
    {
        let scoped = actor.scope(db, project.id.as_str(), false)?;
        return value(JoinOutcome {
            status: ApplicationStatus::Joined,
            application: None,
            project: Some(project),
            member: scoped.member,
        });
    }
    let same = list::<Application>(db, "application", project.id.as_str())?
        .into_iter()
        .find(|a| {
            a.issuer == account.issuer
                && a.view.subject == account.sub
                && a.invitation == invitation.as_ref().map(|i| i.view.id.clone())
                && !matches!(
                    a.view.status,
                    ApplicationStatus::Rejected | ApplicationStatus::Joined
                )
        });
    if let Some(application) = same {
        return pending_outcome(application);
    }
    let application_count: i64 = db.query_row(
        "SELECT COUNT(*) FROM ch_objects WHERE kind='application' AND project=?1",
        [project.id.as_str()],
        |row| row.get(0),
    )?;
    if application_count >= 10000 {
        return Err(Error::new(
            "ProjectLimitError",
            "project application limit reached",
        ));
    }
    let application = Application {
        view: ApplicationView {
            id: ApplicationId::new(),
            project: Some(project.id.clone()),
            subject: account.sub.clone(),
            role,
            kind,
            status,
            challenge: None,
        },
        issuer: account.issuer.clone(),
        name: bounded_name(request.name, actor)?,
        invitation: invitation.map(|i| i.view.id),
    };
    put(
        db,
        "application",
        application.view.id.as_str(),
        project.id.as_str(),
        &application,
    )?;
    event(
        db,
        project.id.as_str(),
        "application.created",
        &application.view,
    )?;
    pending_outcome(application)
}
fn pending_outcome(application: Application) -> Result<Value, Error> {
    let mut view = application.view;
    view.project = None;
    value(JoinOutcome {
        status: view.status,
        application: Some(view),
        project: None,
        member: None,
    })
}
fn recipient_proof<'a>(
    application: &Application,
    config: &'a EnrollmentConfig,
) -> Result<&'a IdentityLease, Error> {
    let proof = config.recipient_proof.as_deref().ok_or_else(|| {
        Error::new(
            "InactiveIdentityError",
            "fresh recipient validation is required",
        )
    })?;
    if !proof.is_current()
        || proof.account().issuer != application.issuer
        || proof.account().sub != application.view.subject
    {
        return Err(Error::forbidden());
    }
    proof.require_step_up(config.require_mfa)?;
    Ok(proof)
}
fn current_invitation(
    db: &Connection,
    application: &Application,
) -> Result<Option<Invitation>, Error> {
    let Some(id) = &application.invitation else {
        return Ok(None);
    };
    let invitation: Invitation = get(db, "invitation", id.as_str())?;
    if invitation.view.revoked
        || invitation.view.used
        || invitation.view.expires_at <= now()
        || invitation.issuer != application.issuer
        || invitation
            .view
            .subject
            .as_ref()
            .is_some_and(|sub| sub != &application.view.subject)
        || invitation.view.project != *app_project(application)?
        || invitation.view.role != application.view.role
    {
        return Err(Error::forbidden());
    }
    Ok(Some(invitation))
}
fn admission_invitation(
    db: &Connection,
    application: &Application,
    project: &Project,
) -> Result<Option<Invitation>, Error> {
    let invitation = current_invitation(db, application)?;
    if invitation.is_none()
        && !matches!(project.enrollment,Enrollment::OrganizationMembers{role} if role==application.view.role&&role!=Role::Owner)
    {
        return Err(Error::forbidden());
    }
    Ok(invitation)
}

fn approve(
    db: &Connection,
    _actor: &Actor,
    project: &str,
    id: &str,
    config: &EnrollmentConfig,
) -> Result<Value, Error> {
    let mut application = project_application(db, project, id)?;
    if matches!(
        application.view.status,
        ApplicationStatus::Joined | ApplicationStatus::Rejected
    ) {
        return Err(Error::forbidden());
    }
    let proof = recipient_proof(&application, config)?;
    let p: Project = get(db, "project", project)?;
    let invitation = admission_invitation(db, &application, &p)?;
    let kind = membership_kind(
        &p,
        application.view.kind == MembershipKind::Guest,
        proof,
        config,
    )?;
    if kind != application.view.kind
        || kind == MembershipKind::Guest && application.view.role == Role::Owner
    {
        return Err(Error::forbidden());
    }
    if kind == MembershipKind::Internal {
        application.view.status = ApplicationStatus::NeedsConfirmation;
        put(db, "application", id, project, &application)?;
        event(db, project, "application.approved", &application.view)?;
        return value(IssuedChallenge {
            application: application.view,
            code: None,
            expires_at: None,
        });
    }
    // Security counters must query the intended recipient, rather than truncate
    // a project-wide history before applying the one-hour/identity filters.
    let count: i64 = db.query_row("SELECT COUNT(*) FROM ch_objects WHERE kind='challenge' AND project=?1 AND json_extract(body,'$.issuer')=?2 AND json_extract(body,'$.subject')=?3 AND json_extract(body,'$.created_at')>?4", params![project,application.issuer,application.view.subject,now().saturating_sub(3600) as i64], |row|row.get(0))?;
    if count >= 3 {
        return Err(Error::new(
            "RateLimitError",
            "at most three join codes per recipient and project per hour",
        ));
    }
    let mut stmt = db.prepare("SELECT body FROM ch_objects WHERE kind='challenge' AND project=?1 AND json_extract(body,'$.issuer')=?2 AND json_extract(body,'$.subject')=?3 AND json_extract(body,'$.active')=1 AND json_extract(body,'$.expires_at')>?4")?;
    let challenges = stmt
        .query_map(
            params![
                project,
                application.issuer,
                application.view.subject,
                now() as i64
            ],
            |row| row.get::<_, String>(0),
        )?
        .map(|body| decode::<Challenge>(&body?))
        .collect::<Result<Vec<_>, _>>()?;
    for mut challenge in challenges {
        if challenge.invitation == application.invitation {
            challenge.active = false;
            put(db, "challenge", challenge.id.as_str(), project, &challenge)?;
        }
    }
    let code = random_code()?;
    let mut challenge = Challenge {
        id: ChallengeId::new(),
        application: application.view.id.clone(),
        project: p.id,
        issuer: application.issuer.clone(),
        subject: application.view.subject.clone(),
        role: application.view.role,
        invitation: invitation.map(|i| i.view.id),
        mac: String::new(),
        expires_at: now() + 600,
        attempts: 0,
        active: true,
        created_at: now(),
    };
    challenge.mac = challenge_mac(config, &challenge, &code)?;
    application.view.status = ApplicationStatus::NeedsCode;
    application.view.challenge = Some(challenge.id.clone());
    put(db, "challenge", challenge.id.as_str(), project, &challenge)?;
    put(db, "application", id, project, &application)?;
    event(db, project, "application.approved", &application.view)?;
    value(IssuedChallenge {
        application: application.view,
        code: Some(code),
        expires_at: Some(challenge.expires_at),
    })
}
fn mac<T: Serialize>(config: &EnrollmentConfig, domain: &str, value: &T) -> Result<String, Error> {
    if config.mac_key.expose().len() < 32 {
        return Err(Error::storage());
    }
    let mut mac = Hmac::<Sha256>::new_from_slice(config.mac_key.expose().as_bytes())
        .map_err(|_| Error::storage())?;
    mac.update(domain.as_bytes());
    mac.update(&[0]);
    mac.update(encode(value)?.as_bytes());
    Ok(mac
        .finalize()
        .into_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}
fn challenge_mac(config: &EnrollmentConfig, c: &Challenge, code: &str) -> Result<String, Error> {
    mac(
        config,
        "join-code",
        &(
            &c.id,
            &c.application,
            &c.project,
            &c.issuer,
            &c.subject,
            c.role,
            &c.invitation,
            code,
        ),
    )
}
fn random_code() -> Result<String, Error> {
    loop {
        let mut bytes = [0; 4];
        getrandom::fill(&mut bytes).map_err(|_| Error::storage())?;
        let n = u32::from_le_bytes(bytes);
        if n < 4_200_000_000 {
            return Ok(format!("{:08}", n % 100_000_000));
        }
    }
}
fn redeem(
    db: &Connection,
    actor: &Actor,
    id: &str,
    request: Redeem,
    config: &EnrollmentConfig,
) -> Result<Value, Error> {
    let mut application = app(db, id)?;
    own_application(actor, &application)?;
    let project_id = app_project(&application)?.clone();
    let project = project_id.as_str();
    let p: Project = get(db, "project", project)?;
    if request.repository != p.repository {
        return Err(Error::new(
            "RepositoryMismatchError",
            "repository confirmation does not match the invited project",
        ));
    }
    if application.view.status == ApplicationStatus::Joined {
        let scoped = actor.scope(db, project, false)?;
        return value(JoinOutcome {
            status: ApplicationStatus::Joined,
            application: None,
            project: Some(p),
            member: scoped.member,
        });
    }
    if !matches!(
        application.view.status,
        ApplicationStatus::NeedsCode | ApplicationStatus::NeedsConfirmation
    ) {
        return Err(Error::forbidden());
    }
    actor.proof.require_step_up(config.require_mfa)?;
    let kind = membership_kind(
        &p,
        application.view.kind == MembershipKind::Guest,
        &actor.proof,
        config,
    )?;
    if kind != application.view.kind
        || kind == MembershipKind::Guest && application.view.role == Role::Owner
    {
        return Err(Error::forbidden());
    }
    let invitation = admission_invitation(db, &application, &p)?;
    if application.view.status == ApplicationStatus::NeedsCode {
        let id = application
            .view
            .challenge
            .as_ref()
            .ok_or_else(Error::forbidden)?;
        let mut challenge: Challenge = get(db, "challenge", id.as_str())?;
        if !challenge.active
            || challenge.expires_at <= now()
            || challenge.attempts >= 5
            || challenge.application != application.view.id
            || challenge.project != project_id
            || challenge.issuer != application.issuer
            || challenge.subject != application.view.subject
            || challenge.role != application.view.role
            || challenge.invitation != application.invitation
        {
            return Err(Error::forbidden());
        }
        let code = request.code.as_deref().unwrap_or_default();
        let expected = challenge_mac(config, &challenge, code)?;
        if code.len() != 8
            || !code.bytes().all(|b| b.is_ascii_digit())
            || !bool::from(expected.as_bytes().ct_eq(challenge.mac.as_bytes()))
        {
            challenge.attempts += 1;
            if challenge.attempts >= 5 {
                challenge.active = false;
            }
            put(db, "challenge", challenge.id.as_str(), project, &challenge)?;
            return Err(Error::new(
                "InvalidJoinCodeError",
                "join code is invalid or exhausted",
            ));
        }
        challenge.active = false;
        put(db, "challenge", challenge.id.as_str(), project, &challenge)?;
    } else if kind != MembershipKind::Internal {
        return Err(Error::forbidden());
    }
    let members = list::<Member>(db, "member", project)?;
    if members.iter().filter(|m| m.active).count() >= 100
        && !members.iter().any(|m| {
            m.active && m.issuer == application.issuer && m.subject == application.view.subject
        })
    {
        return Err(Error::new(
            "ProjectLimitError",
            "project member limit reached",
        ));
    }
    let existing = members
        .into_iter()
        .find(|m| m.issuer == application.issuer && m.subject == application.view.subject);
    let member = Member {
        id: existing
            .as_ref()
            .map_or_else(MemberId::new, |m| m.id.clone()),
        project: project_id.clone(),
        issuer: application.issuer.clone(),
        subject: application.view.subject.clone(),
        name: application.name.clone(),
        role: application.view.role,
        kind: application.view.kind,
        generation: existing.map_or(1, |m| m.generation + 1),
        active: true,
    };
    if let Some(mut invitation) = invitation {
        invitation.view.used = true;
        put(
            db,
            "invitation",
            invitation.view.id.as_str(),
            project,
            &invitation,
        )?;
    }
    application.view.status = ApplicationStatus::Joined;
    put(db, "application", id, project, &application)?;
    put(db, "member", member.id.as_str(), project, &member)?;
    event(db, project, "member.joined", &member)?;
    value(JoinOutcome {
        status: ApplicationStatus::Joined,
        application: Some(application.view),
        project: Some(p),
        member: Some(member),
    })
}
fn reject(db: &Connection, project: &str, id: &str) -> Result<Value, Error> {
    let mut application = project_application(db, project, id)?;
    if application.view.status == ApplicationStatus::Joined {
        return Err(Error::forbidden());
    }
    application.view.status = ApplicationStatus::Rejected;
    invalidate_application_challenges(db, project, &application.view.id)?;
    put(db, "application", id, project, &application)?;
    event(db, project, "application.rejected", &application.view)?;
    Ok(Value::Null)
}
fn invalidate_application_challenges(
    db: &Connection,
    project: &str,
    id: &ApplicationId,
) -> Result<(), Error> {
    db.execute("UPDATE ch_objects SET body=json_set(body,'$.active',json('false')) WHERE kind='challenge' AND project=?1 AND json_extract(body,'$.application')=?2",params![project,id.as_str()])?;
    Ok(())
}

fn revoke_invitation(db: &Connection, project: &str, id: &str) -> Result<Value, Error> {
    let mut invitation: Invitation = get(db, "invitation", id)?;
    if invitation.view.project.as_str() != project {
        return Err(Error::missing());
    }
    invitation.view.revoked = true;
    put(db, "invitation", id, project, &invitation)?;
    for mut a in list::<Application>(db, "application", project)? {
        if a.invitation.as_ref() == Some(&invitation.view.id)
            && a.view.status != ApplicationStatus::Joined
        {
            a.view.status = ApplicationStatus::Rejected;
            invalidate_application_challenges(db, project, &a.view.id)?;
            put(db, "application", a.view.id.as_str(), project, &a)?;
        }
    }
    event(db, project, "invitation.revoked", &invitation.view)?;
    Ok(Value::Null)
}
fn last_owner(db: &Connection, project: &str, member: &Member) -> Result<(), Error> {
    if member.active
        && member.role == Role::Owner
        && list::<Member>(db, "member", project)?
            .iter()
            .filter(|m| m.active && m.role == Role::Owner)
            .count()
            <= 1
    {
        return Err(Error::new(
            "LastOwnerError",
            "the final owner cannot be removed or demoted",
        ));
    }
    Ok(())
}
fn set_role(
    db: &Connection,
    project: &str,
    id: &str,
    request: SetRole,
    _config: &EnrollmentConfig,
) -> Result<Value, Error> {
    let mut member: Member = get(db, "member", id)?;
    if member.project.as_str() != project {
        return Err(Error::missing());
    }
    if !member.active || member.kind == MembershipKind::Guest && request.role == Role::Owner {
        return Err(Error::forbidden());
    }
    if member.role == Role::Owner && request.role != Role::Owner {
        last_owner(db, project, &member)?;
    }
    if member.role != request.role {
        member.role = request.role;
        member.generation += 1;
        put(db, "member", id, project, &member)?;
        disable_agents(db, project, &member)?;
        event(db, project, "member.role_changed", &member)?;
    }
    value(member)
}
fn remove_member(db: &Connection, project: &str, id: &str) -> Result<Value, Error> {
    let mut member: Member = get(db, "member", id)?;
    if member.project.as_str() != project {
        return Err(Error::missing());
    }
    last_owner(db, project, &member)?;
    member.active = false;
    member.generation += 1;
    put(db, "member", id, project, &member)?;
    disable_agents(db, project, &member)?;
    for mut invitation in list::<Invitation>(db, "invitation", project)? {
        if invitation.issuer == member.issuer
            && invitation.view.subject.as_deref() == Some(member.subject.as_str())
            && !invitation.view.used
        {
            invitation.view.revoked = true;
            put(
                db,
                "invitation",
                invitation.view.id.as_str(),
                project,
                &invitation,
            )?;
        }
    }
    for mut a in list::<Application>(db, "application", project)? {
        if a.issuer == member.issuer
            && a.view.subject == member.subject
            && a.view.status != ApplicationStatus::Joined
        {
            a.view.status = ApplicationStatus::Rejected;
            invalidate_application_challenges(db, project, &a.view.id)?;
            put(db, "application", a.view.id.as_str(), project, &a)?;
        }
    }
    event(db, project, "member.removed", &member)?;
    Ok(Value::Null)
}
fn disable_agents(db: &Connection, project: &str, member: &Member) -> Result<(), Error> {
    for mut agent in list::<Agent>(db, "agent", project)? {
        if agent.owner == member.id {
            agent.active = false;
            agent.online = false;
            put(db, "agent", agent.id.as_str(), project, &agent)?;
        }
    }
    let mut stmt =
        db.prepare("SELECT id,body FROM ch_objects WHERE kind='runtime' AND project=?1")?;
    let rows = stmt
        .query_map([project], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    for (id, body) in rows {
        let mut record: Value = decode(&body)?;
        if record["owner"] == member.id.as_str() {
            record["active"] = json!(false);
            put(db, "runtime", &id, project, &record)?;
        }
    }
    Ok(())
}
fn policy(
    db: &Connection,
    project: &str,
    request: UpdatePolicy,
    config: &EnrollmentConfig,
) -> Result<Value, Error> {
    let mut p: Project = get(db, "project", project)?;
    if p.revision != request.expected_revision {
        return Err(Error::conflict(p.revision));
    }
    if request.guests_enabled && (!config.allow_guests || matches!(p.mode, ProjectMode::Friends)) {
        return Err(Error::forbidden());
    }
    if let Enrollment::OrganizationMembers { role } = request.enrollment
        && (role == Role::Owner || !matches!(p.mode, ProjectMode::Organization { .. }))
    {
        return Err(Error::forbidden());
    }
    if p.guests_enabled && !request.guests_enabled {
        for mut member in list::<Member>(db, "member", project)? {
            if member.kind == MembershipKind::Guest && member.active {
                member.active = false;
                member.generation += 1;
                put(db, "member", member.id.as_str(), project, &member)?;
                disable_agents(db, project, &member)?;
            }
        }
        for mut a in list::<Application>(db, "application", project)? {
            if a.view.kind == MembershipKind::Guest && a.view.status != ApplicationStatus::Joined {
                a.view.status = ApplicationStatus::Rejected;
                invalidate_application_challenges(db, project, &a.view.id)?;
                put(db, "application", a.view.id.as_str(), project, &a)?;
            }
        }
        for mut i in list::<Invitation>(db, "invitation", project)? {
            if i.view.guest && !i.view.used {
                i.view.revoked = true;
                put(db, "invitation", i.view.id.as_str(), project, &i)?;
            }
        }
    }
    p.enrollment = request.enrollment;
    p.guests_enabled = request.guests_enabled;
    p.revision += 1;
    put(db, "project", project, project, &p)?;
    event(db, project, "project.policy_changed", &p)?;
    value(p)
}

#[cfg(test)]
use crate::fixture;
#[cfg(test)]
mod tests {
    use super::*;
    use crate::repository::Repository;
    struct Setup {
        _dir: tempfile::TempDir,
        db: Database,
        project: Project,
        owner: Actor,
        bob: Actor,
        config: EnrollmentConfig,
        _issuer: fixture::Fixture,
    }
    impl Setup {
        async fn new(mode: ProjectMode) -> Self {
            let f = fixture::Fixture::start().await;
            let mut client = f.client().await;
            let owner_token = f.user("alice", json!([{"id":"org-uuid","short_name":"team"}]));
            let bob_token = f.user("bob", json!([{"id":"org-uuid","short_name":"team"}]));
            let owner = Actor {
                proof: Arc::new(
                    client
                        .authorize_human(&owner_token, &f.service(), "hub")
                        .await
                        .unwrap(),
                ),
                member: None,
                agent: None,
            };
            let bob = Actor {
                proof: Arc::new(
                    client
                        .authorize_human(&bob_token, &f.service(), "hub")
                        .await
                        .unwrap(),
                ),
                member: None,
                agent: None,
            };
            let dir = tempfile::tempdir().unwrap();
            let db = Database::open(&dir.path().join("hub.sqlite")).unwrap();
            let project = Project {
                id: ProjectId::new(),
                name: "project".into(),
                repository: Repository::parse("https://github.com/acme/repo.git").unwrap(),
                mode: mode.clone(),
                enrollment: Enrollment::InviteOnly,
                guests_enabled: false,
                revision: 1,
                created_at: now(),
            };
            let member = Member {
                id: MemberId::new(),
                project: project.id.clone(),
                issuer: f.state.issuer.clone(),
                subject: "alice".into(),
                name: "Alice".into(),
                role: Role::Owner,
                kind: if matches!(mode, ProjectMode::Friends) {
                    MembershipKind::Friend
                } else {
                    MembershipKind::Internal
                },
                generation: 1,
                active: true,
            };
            let p = project.clone();
            db.write(move |db| {
                put(db, "project", p.id.as_str(), p.id.as_str(), &p)?;
                put(db, "member", member.id.as_str(), p.id.as_str(), &member)
            })
            .unwrap();
            let config = EnrollmentConfig {
                bootstrap_sub: "alice".into(),
                allow_guests: true,
                require_mfa: true,
                mac_key: Secret::new("01234567890123456789012345678901".into()),
                recipient_proof: Some(bob.proof.clone()),
            };
            Self {
                _dir: dir,
                db,
                project,
                owner,
                bob,
                config,
                _issuer: f,
            }
        }
        fn call(
            &self,
            actor: &Actor,
            path: &[&str],
            method: &str,
            input: Value,
        ) -> Result<Value, Error> {
            let actor = actor.clone();
            let path: Vec<String> = path.iter().map(|s| s.to_string()).collect();
            let method = method.to_owned();
            let config = self.config.clone();
            self.db.write(move |db| {
                mutate(
                    db,
                    &actor,
                    &path.iter().map(String::as_str).collect::<Vec<_>>(),
                    &method,
                    input,
                    &config,
                )
            })
        }
        fn invite(&self, guest: bool) -> Value {
            self.call(
                &self.owner,
                &["projects", self.project.id.as_str(), "invitations"],
                "POST",
                json!({"role":"member","subject":"bob","guest":guest}),
            )
            .unwrap()
        }
        fn join(&self, invitation: &Value) -> Value {
            self.call(
                &self.bob,
                &["join"],
                "POST",
                json!({"invitation_token":invitation["token"],"name":"Bob"}),
            )
            .unwrap()
        }
        fn approve(&self, id: &str) -> Result<Value, Error> {
            self.call(
                &self.owner,
                &[
                    "projects",
                    self.project.id.as_str(),
                    "applications",
                    id,
                    "approve",
                ],
                "POST",
                json!({}),
            )
        }
        fn redeem(&self, id: &str, code: Option<&str>) -> Result<Value, Error> {
            self.call(
                &self.bob,
                &["applications", id, "redeem"],
                "POST",
                json!({"repository":self.project.repository,"code":code}),
            )
        }
    }
    #[tokio::test]
    async fn friends_codes_are_keyed_attempt_limited_reissued_and_consumed_atomically() {
        let s = Setup::new(ProjectMode::Friends).await;
        let invitation = s.invite(false);
        let joined = s.join(&invitation);
        assert_eq!(joined["status"], "pending_approval");
        assert!(joined["project"].is_null());
        assert!(joined["member"].is_null());
        let id = joined["application"]["id"].as_str().unwrap();
        let issued = s.approve(id).unwrap();
        let first = issued["code"].as_str().unwrap();
        assert_eq!(first.len(), 8);
        let wrong = if first == "00000000" {
            "00000001"
        } else {
            "00000000"
        };
        for _ in 0..5 {
            assert_eq!(
                s.redeem(id, Some(wrong)).unwrap_err().tag,
                "InvalidJoinCodeError"
            );
        }
        assert!(s.redeem(id, Some(first)).is_err());
        let reissued = s.approve(id).unwrap();
        let second = reissued["code"].as_str().unwrap();
        if first != second {
            assert_eq!(
                s.redeem(id, Some(first)).unwrap_err().tag,
                "InvalidJoinCodeError"
            );
        }
        let admitted = s.redeem(id, Some(second)).unwrap();
        assert_eq!(admitted["status"], "joined");
        assert_eq!(admitted["member"]["subject"], "bob");
        let member = admitted["member"]["id"].as_str().unwrap();
        s.call(
            &s.owner,
            &["projects", s.project.id.as_str(), "members", member],
            "DELETE",
            json!({}),
        )
        .unwrap();
        assert!(s.redeem(id, Some(second)).is_err());
        let project = s.project.id.to_string();
        let stored =
            s.db.read(move |db| list::<Challenge>(db, "challenge", &project))
                .unwrap();
        assert!(stored.iter().all(|c| !c.active && c.mac.len() == 64));
        assert_ne!(stored[0].mac, hash(first.as_bytes()));
        let protected = idempotency_payload(
            &["applications", id, "redeem"],
            &json!({"code":second}),
            &s.config,
        )
        .unwrap();
        assert_ne!(protected["code"], second);
        assert_ne!(protected["code"], hash(second.as_bytes()));
    }
    #[tokio::test]
    async fn internal_invites_still_require_repository_confirmation_but_no_project_code() {
        let s = Setup::new(ProjectMode::Organization {
            organization_id: "org-uuid".into(),
        })
        .await;
        let invitation = s.invite(false);
        let pending = s.join(&invitation);
        assert_eq!(pending["status"], "needs_confirmation");
        assert!(pending["project"].is_null());
        let id = pending["application"]["id"].as_str().unwrap();
        assert!(
            s.call(
                &s.bob,
                &["applications", id, "redeem"],
                "POST",
                json!({"repository":Repository::parse("https://github.com/acme/other").unwrap()})
            )
            .is_err()
        );
        assert_eq!(s.redeem(id, None).unwrap()["status"], "joined");
    }
    #[tokio::test]
    async fn approval_requires_recipient_proof_and_respects_issuance_limits() {
        let mut s = Setup::new(ProjectMode::Friends).await;
        let invitation = s.invite(false);
        let pending = s.join(&invitation);
        let id = pending["application"]["id"].as_str().unwrap();
        s.config.recipient_proof = Some(s.owner.proof.clone());
        assert!(s.approve(id).is_err());
        s.config.recipient_proof = Some(s.bob.proof.clone());
        for _ in 0..3 {
            s.approve(id).unwrap();
        }
        assert_eq!(s.approve(id).unwrap_err().tag, "RateLimitError");
        let project = s.project.id.to_string();
        let active =
            s.db.read(move |db| {
                Ok(list::<Challenge>(db, "challenge", &project)?
                    .into_iter()
                    .filter(|c| c.active)
                    .count())
            })
            .unwrap();
        assert_eq!(active, 1);
    }
    #[tokio::test]
    async fn last_owner_guest_policy_and_cross_project_guards_fail_closed() {
        let s = Setup::new(ProjectMode::Organization {
            organization_id: "org-uuid".into(),
        })
        .await;
        let project = s.project.id.to_string();
        let owner =
            s.db.read(move |db| Ok(list::<Member>(db, "member", &project)?.remove(0)))
                .unwrap();
        assert_eq!(
            s.call(
                &s.owner,
                &[
                    "projects",
                    s.project.id.as_str(),
                    "members",
                    owner.id.as_str()
                ],
                "DELETE",
                json!({})
            )
            .unwrap_err()
            .tag,
            "LastOwnerError"
        );
        assert!(s.call(&s.owner,&["projects",s.project.id.as_str(),"policy"],"PATCH",json!({"expected_revision":1,"enrollment":{"kind":"organization_members","role":"owner"},"guests_enabled":false})).is_err());
        assert!(
            s.call(
                &s.owner,
                &["projects", s.project.id.as_str(), "invitations"],
                "POST",
                json!({"role":"viewer","subject":"bob","guest":true})
            )
            .is_err()
        );
        let other = ProjectId::new();
        assert!(
            s.call(
                &s.owner,
                &["projects", other.as_str(), "members", owner.id.as_str()],
                "DELETE",
                json!({})
            )
            .is_err()
        );
    }
    #[tokio::test]
    async fn challenge_limits_filter_recipient_before_bounded_history_and_expiry_refuses_admission()
    {
        let s = Setup::new(ProjectMode::Friends).await;
        let project = s.project.id.clone();
        s.db.write(move |db| {
            for _ in 0..10001 {
                let c = Challenge {
                    id: ChallengeId::new(),
                    application: ApplicationId::new(),
                    project: project.clone(),
                    issuer: "other-issuer".into(),
                    subject: "other-user".into(),
                    role: Role::Viewer,
                    invitation: None,
                    mac: "a".repeat(64),
                    expires_at: now().saturating_sub(3600),
                    attempts: 0,
                    active: false,
                    created_at: now().saturating_sub(7200),
                };
                put(db, "challenge", c.id.as_str(), project.as_str(), &c)?;
            }
            Ok(())
        })
        .unwrap();
        let invitation = s.invite(false);
        let pending = s.join(&invitation);
        let id = pending["application"]["id"].as_str().unwrap();
        let issued = s.approve(id).unwrap();
        let code = issued["code"].as_str().unwrap();
        let challenge_id = issued["application"]["challenge"]
            .as_str()
            .unwrap()
            .to_owned();
        let project = s.project.id.to_string();
        s.db.write(move |db| {
            let mut c: Challenge = get(db, "challenge", &challenge_id)?;
            c.expires_at = now().saturating_sub(1);
            put(db, "challenge", &challenge_id, &project, &c)
        })
        .unwrap();
        assert!(s.redeem(id, Some(code)).is_err());
        s.approve(id).unwrap();
        s.approve(id).unwrap();
        assert_eq!(s.approve(id).unwrap_err().tag, "RateLimitError");
    }
    #[tokio::test]
    async fn valid_cross_org_guest_uses_code_never_becomes_owner_and_policy_off_revokes_it() {
        let mut s = Setup::new(ProjectMode::Organization {
            organization_id: "org-uuid".into(),
        })
        .await;
        let token = s._issuer.user("bob", json!([]));
        let mut client = s._issuer.client().await;
        s.bob.proof = Arc::new(
            client
                .authorize_human(&token, &s._issuer.service(), "hub")
                .await
                .unwrap(),
        );
        s.config.recipient_proof = Some(s.bob.proof.clone());
        s.call(&s.owner,&["projects",s.project.id.as_str(),"policy"],"PATCH",json!({"expected_revision":1,"enrollment":{"kind":"invite_only"},"guests_enabled":true})).unwrap();
        assert!(
            s.call(
                &s.owner,
                &["projects", s.project.id.as_str(), "invitations"],
                "POST",
                json!({"role":"owner","subject":"bob","guest":true})
            )
            .is_err()
        );
        let invitation = s.invite(true);
        let pending = s.join(&invitation);
        assert_eq!(pending["application"]["kind"], "guest");
        assert!(pending["project"].is_null());
        let id = pending["application"]["id"].as_str().unwrap();
        let issued = s.approve(id).unwrap();
        assert_eq!(issued["application"]["status"], "needs_code");
        assert!(s.redeem(id, None).is_err());
        let joined = s.redeem(id, issued["code"].as_str()).unwrap();
        assert_eq!(joined["member"]["kind"], "guest");
        assert_eq!(joined["member"]["role"], "member");
        let member = joined["member"]["id"].as_str().unwrap();
        assert!(
            s.call(
                &s.owner,
                &["projects", s.project.id.as_str(), "members", member],
                "PATCH",
                json!({"role":"owner"})
            )
            .is_err()
        );
        let scoped = s.bob.clone();
        let project = s.project.id.to_string();
        assert!(
            s.db.read(move |db| scoped.scope(db, &project, true))
                .is_ok()
        );
        s.call(&s.owner,&["projects",s.project.id.as_str(),"policy"],"PATCH",json!({"expected_revision":2,"enrollment":{"kind":"invite_only"},"guests_enabled":false})).unwrap();
        let scoped = s.bob.clone();
        let project = s.project.id.to_string();
        assert!(
            s.db.read(move |db| scoped.scope(db, &project, false))
                .is_err()
        );
        assert!(s.redeem(id, issued["code"].as_str()).is_err());
    }
    #[tokio::test]
    async fn missing_personal_mfa_cannot_administer_or_skip_internal_admission_step_up() {
        let mut s = Setup::new(ProjectMode::Organization {
            organization_id: "org-uuid".into(),
        })
        .await;
        let mut client = s._issuer.client().await;
        let token = s._issuer.user("alice", json!([{"id":"org-uuid"}]));
        s._issuer
            .state
            .users
            .lock()
            .unwrap()
            .get_mut("alice")
            .unwrap()["amr"] = json!(["pwd"]);
        s.owner.proof = Arc::new(
            client
                .authorize_human(&token, &s._issuer.service(), "hub")
                .await
                .unwrap(),
        );
        assert_eq!(
            s.call(
                &s.owner,
                &["projects", s.project.id.as_str(), "invitations"],
                "POST",
                json!({"role":"member","subject":"bob","guest":false})
            )
            .unwrap_err()
            .tag,
            "StepUpRequiredError"
        );
        // An organization claim removes the extra project code, not account MFA.
        let token = s._issuer.user("bob", json!([{"id":"org-uuid"}]));
        s._issuer
            .state
            .users
            .lock()
            .unwrap()
            .get_mut("bob")
            .unwrap()["amr"] = json!(["pwd"]);
        s.bob.proof = Arc::new(
            client
                .authorize_human(&token, &s._issuer.service(), "hub")
                .await
                .unwrap(),
        );
        assert_eq!(
            s.call(&s.bob, &["join"], "POST", json!({"project":s.project.id}))
                .unwrap_err()
                .tag,
            "StepUpRequiredError"
        );
    }
    #[tokio::test]
    async fn application_status_is_recipient_private_exposes_only_approved_descriptor_and_refuses_revoked_joined()
     {
        let s = Setup::new(ProjectMode::Friends).await;
        let invitation = s.invite(false);
        let pending = s.join(&invitation);
        let id = pending["application"]["id"].as_str().unwrap();
        let status = |actor: &Actor| {
            let actor = actor.clone();
            let id = id.to_owned();
            let config = s.config.clone();
            s.db.read(move |db| read(db, &actor, &["applications", &id], &config))
        };
        let waiting = status(&s.bob).unwrap();
        assert_eq!(waiting["status"], "pending_approval");
        assert!(waiting["project"].is_null());
        assert!(waiting["member"].is_null());
        assert!(status(&s.owner).is_err());
        let approved = s.approve(id).unwrap();
        let descriptor = status(&s.bob).unwrap();
        assert_eq!(descriptor["status"], "needs_code");
        assert_eq!(
            descriptor["project"]["repository"],
            json!(s.project.repository)
        );
        assert!(descriptor["member"].is_null());
        assert!(descriptor.get("code").is_none());
        assert!(descriptor.get("messages").is_none());
        assert!(status(&s.owner).is_err());
        let joined = s.redeem(id, approved["code"].as_str()).unwrap();
        assert_eq!(status(&s.bob).unwrap()["status"], "joined");
        let member = joined["member"]["id"].as_str().unwrap();
        s.call(
            &s.owner,
            &["projects", s.project.id.as_str(), "members", member],
            "DELETE",
            json!({}),
        )
        .unwrap();
        assert!(status(&s.bob).is_err());
    }
    #[tokio::test]
    async fn withdrawn_or_changed_auto_enrollment_hides_unconfirmed_descriptor() {
        let s = Setup::new(ProjectMode::Organization {
            organization_id: "org-uuid".into(),
        })
        .await;
        s.call(&s.owner,&["projects",s.project.id.as_str(),"policy"],"PATCH",json!({"expected_revision":1,"enrollment":{"kind":"organization_members","role":"viewer"},"guests_enabled":false})).unwrap();
        let pending = s
            .call(&s.bob, &["join"], "POST", json!({"project":s.project.id}))
            .unwrap();
        let id = pending["application"]["id"].as_str().unwrap();
        let status = || {
            let actor = s.bob.clone();
            let id = id.to_owned();
            let config = s.config.clone();
            s.db.read(move |db| read(db, &actor, &["applications", &id], &config))
        };
        assert!(status().unwrap()["project"].is_object());
        s.call(&s.owner,&["projects",s.project.id.as_str(),"policy"],"PATCH",json!({"expected_revision":2,"enrollment":{"kind":"invite_only"},"guests_enabled":false})).unwrap();
        assert!(status().is_err());
        assert!(s.redeem(id, None).is_err());
        s.call(&s.owner,&["projects",s.project.id.as_str(),"policy"],"PATCH",json!({"expected_revision":3,"enrollment":{"kind":"organization_members","role":"member"},"guests_enabled":false})).unwrap();
        assert!(status().is_err());
        assert!(s.redeem(id, None).is_err());
    }
}

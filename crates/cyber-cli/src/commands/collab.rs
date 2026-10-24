//! Owner-controlled project collaboration. Secrets are read from stdin, never argv.
use crate::{cli::GlobalArgs, context::Context, error::CliError, output};
use clap::{Args, Subcommand, ValueEnum};
use cyber_client::Client;
use serde_json::{Value, json};
use std::io::{IsTerminal, Read};

#[derive(Debug, Args)]
pub struct Scope {
    #[arg(long)]
    pub project: String,
}
/// Trusted hub listener and backend introspection settings. Cloned files cannot set these.
#[derive(Debug, Args)]
pub struct CollabServeArgs {
    #[arg(long)]
    issuer: String,
    #[arg(long)]
    audience: String,
    #[arg(long)]
    bootstrap_sub: String,
    #[arg(long, default_value = "127.0.0.1:4749")]
    listen: std::net::SocketAddr,
    #[arg(long)]
    database: Option<std::path::PathBuf>,
    #[arg(long,conflicts_with_all=["introspection_client_id","introspection_client_secret_env","introspection_scope"],required_unless_present="introspection_client_id")]
    introspection_token_env: Option<String>,
    #[arg(
        long,
        requires = "introspection_client_secret_env",
        required_unless_present = "introspection_token_env"
    )]
    introspection_client_id: Option<String>,
    #[arg(long, requires = "introspection_client_id")]
    introspection_client_secret_env: Option<String>,
    #[arg(long, requires = "introspection_client_id")]
    introspection_scope: Option<String>,
    #[arg(long, requires = "tls_key", conflicts_with = "trusted_proxy")]
    tls_cert: Option<std::path::PathBuf>,
    #[arg(long, requires = "tls_cert", conflicts_with = "trusted_proxy")]
    tls_key: Option<std::path::PathBuf>,
    #[arg(long,conflicts_with_all=["tls_cert","tls_key"])]
    trusted_proxy: Option<std::net::IpAddr>,
    #[arg(long)]
    allow_loopback_http: bool,
    #[arg(long)]
    allow_guests: bool,
    #[arg(long,default_value_t=true,action=clap::ArgAction::Set)]
    require_mfa: bool,
}
#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum Role {
    Owner,
    Member,
    Viewer,
}
impl Role {
    fn value(self) -> &'static str {
        match self {
            Self::Owner => "owner",
            Self::Member => "member",
            Self::Viewer => "viewer",
        }
    }
}
#[derive(Debug, Subcommand)]
pub enum CollabCmd {
    /// Read local binding and publication status.
    Status {
        #[arg(long)]
        session: Option<String>,
    },
    /// Run the independent project hub (TLS termination is required outside loopback).
    Serve(CollabServeArgs),
    /// Create a collaboration project.
    Project {
        #[command(subcommand)]
        cmd: ProjectCmd,
    },
    /// Explicitly pin this checkout to one hub and project after comparing its Git remote.
    Bind {
        #[arg(long)]
        hub: String,
        #[arg(long)]
        project: String,
        #[arg(long, default_value = "origin")]
        remote: String,
        #[arg(long)]
        confirm: bool,
    },
    /// Publish one local Session. Its context is permanently pinned to this project.
    Publish {
        #[arg(long)]
        project: String,
        #[arg(long)]
        session: String,
        #[arg(long)]
        name: String,
        #[arg(long)]
        subscribe_project_messages: bool,
    },
    /// Revoke an agent publication, retaining its Session's project tombstone.
    Unpublish { agent: String },
    /// Create a single-use invitation (the secret is displayed only on issuance).
    Invite {
        #[arg(long)]
        project: String,
        #[arg(long, value_enum)]
        role: Option<Role>,
        #[arg(long)]
        subject: Option<String>,
        #[arg(long)]
        guest: bool,
    },
    Invitations {
        #[command(subcommand)]
        cmd: InvitationsCmd,
    },
    /// Apply to join. Invitation tokens are accepted only on stdin.
    Join {
        #[arg(long)]
        hub: String,
        #[arg(long)]
        project: Option<String>,
        #[arg(long)]
        invite_stdin: bool,
        #[arg(long)]
        name: Option<String>,
    },
    Applications {
        #[command(subcommand)]
        cmd: ApplicationsCmd,
    },
    Members {
        #[command(subcommand)]
        cmd: MembersCmd,
    },
    Messages {
        #[command(subcommand)]
        cmd: MessagesCmd,
    },
    Tasks {
        #[command(subcommand)]
        cmd: TasksCmd,
    },
    Leave {
        #[arg(long)]
        project: String,
    },
}
#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum EnrollmentArg {
    InviteOnly,
    OrganizationMembers,
}
#[derive(Debug, Subcommand)]
pub enum ProjectCmd {
    /// Change explicit enrollment/guest policy with optimistic concurrency.
    Policy {
        #[arg(long)]
        project: String,
        #[arg(long)]
        expected_revision: u64,
        #[arg(long, value_enum, default_value = "invite-only")]
        enrollment: EnrollmentArg,
        #[arg(long, value_enum, default_value = "viewer")]
        role: Role,
        #[arg(long,default_value_t=false,action=clap::ArgAction::Set)]
        allow_guests: bool,
    },
    Create {
        #[arg(long)]
        hub: String,
        #[arg(long)]
        repo: String,
        #[arg(long)]
        name: String,
        #[arg(long, conflicts_with = "friends", required_unless_present = "friends")]
        organization: Option<String>,
        #[arg(long, conflicts_with = "organization")]
        friends: bool,
    },
}
#[derive(Debug, Subcommand)]
pub enum InvitationsCmd {
    List {
        #[arg(long)]
        project: String,
    },
    Revoke {
        #[arg(long)]
        project: String,
        invitation: String,
    },
}
#[derive(Debug, Subcommand)]
pub enum ApplicationsCmd {
    /// Read your own pending application and refresh identity for owner approval.
    Status {
        #[arg(long)]
        hub: String,
        application: String,
    },
    List {
        #[arg(long)]
        project: String,
    },
    Approve {
        #[arg(long)]
        project: String,
        application: String,
    },
    Reject {
        #[arg(long)]
        project: String,
        application: String,
    },
    /// Confirm the displayed repository and optionally redeem a join code from stdin.
    Redeem {
        #[arg(long)]
        hub: String,
        application: String,
        #[arg(long)]
        repo: String,
        #[arg(long)]
        code_stdin: bool,
    },
}
#[derive(Debug, Subcommand)]
pub enum MembersCmd {
    List {
        #[arg(long)]
        project: String,
    },
    SetRole {
        #[arg(long)]
        project: String,
        member: String,
        #[arg(long, value_enum)]
        role: Role,
    },
    Remove {
        #[arg(long)]
        project: String,
        member: String,
    },
}
#[derive(Debug, Subcommand)]
pub enum MessagesCmd {
    List {
        #[arg(long)]
        project: String,
    },
    Send {
        #[arg(long)]
        project: String,
        text: String,
        #[arg(long, conflicts_with = "agent")]
        member: Option<String>,
        #[arg(long, conflicts_with = "member")]
        agent: Option<String>,
        #[arg(long)]
        reply_to: Option<String>,
    },
}
#[derive(Debug, Subcommand)]
pub enum TasksCmd {
    List {
        #[arg(long)]
        project: String,
    },
    Create {
        #[arg(long)]
        project: String,
        title: String,
        #[arg(long, default_value = "")]
        description: String,
        #[arg(long)]
        dependency: Vec<String>,
    },
    Claim {
        #[arg(long)]
        project: String,
        task: String,
        #[arg(long)]
        expected_revision: u64,
    },
    Update {
        #[arg(long)]
        project: String,
        task: String,
        #[arg(long)]
        expected_revision: u64,
        #[arg(long)]
        claim_generation: u64,
        #[arg(long)]
        status: Option<String>,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        description: Option<String>,
        #[arg(long)]
        report: Option<String>,
    },
    Reconcile {
        #[arg(long)]
        project: String,
        task: String,
        #[arg(long)]
        expected_revision: u64,
        #[arg(long)]
        claim_generation: u64,
        #[arg(long)]
        acknowledge_unknown_work: bool,
        #[arg(long)]
        reason: String,
        #[arg(long, conflicts_with = "agent")]
        member: Option<String>,
        #[arg(long, conflicts_with = "member")]
        agent: Option<String>,
    },
}
fn secret_stdin(label: &str) -> Result<String, CliError> {
    if std::io::stdin().is_terminal() {
        return Err(CliError::usage(format!(
            "pipe the {label} to stdin; secrets must not appear in command arguments"
        )));
    }
    let mut input = String::new();
    std::io::stdin()
        .take(16 * 1024 + 1)
        .read_to_string(&mut input)
        .map_err(|_| CliError::runtime("could not read secret from stdin"))?;
    if input.len() > 16 * 1024 || input.trim().is_empty() {
        return Err(CliError::usage("empty or oversized secret"));
    }
    Ok(input.trim().into())
}
fn participant(member: Option<String>, agent: Option<String>) -> Value {
    match (member, agent) {
        (Some(id), _) => json!({"kind":"human","id":id}),
        (_, Some(id)) => json!({"kind":"agent","id":id}),
        _ => Value::Null,
    }
}
fn prepare(cmd: CollabCmd) -> Result<(String, Value), CliError> {
    Ok(match cmd {
        CollabCmd::Status { session } => ("status".into(), json!({"session_id":session})),
        CollabCmd::Bind {
            hub,
            project,
            remote,
            confirm,
        } => (
            "bind".into(),
            json!({"hub_url":hub,"project":project,"remote":remote,"confirm":confirm}),
        ),
        CollabCmd::Publish {
            project,
            session,
            name,
            subscribe_project_messages,
        } => (
            "publish".into(),
            json!({"project":project,"session_id":session,"name":name,"subscribe_project_messages":subscribe_project_messages}),
        ),
        CollabCmd::Unpublish { agent } => ("unpublish".into(), json!({"agent":agent})),
        CollabCmd::Project {
            cmd:
                ProjectCmd::Create {
                    hub,
                    repo,
                    name,
                    organization,
                    friends: _,
                },
        } => (
            "project.create".into(),
            json!({"hub_url":hub,"repository":repo,"name":name,"mode":organization.map(|id|json!({"kind":"organization","organization_id":id})).unwrap_or(json!({"kind":"friends"}))}),
        ),
        CollabCmd::Project {
            cmd:
                ProjectCmd::Policy {
                    project,
                    expected_revision,
                    enrollment,
                    role,
                    allow_guests,
                },
        } => {
            let enrollment = match enrollment {
                EnrollmentArg::InviteOnly => json!({"kind":"invite_only"}),
                EnrollmentArg::OrganizationMembers => {
                    if matches!(role, Role::Owner) {
                        return Err(CliError::usage(
                            "organization enrollment cannot grant owner",
                        ));
                    }
                    json!({"kind":"organization_members","role":role.value()})
                }
            };
            (
                "project.policy".into(),
                json!({"project":project,"expected_revision":expected_revision,"enrollment":enrollment,"guests_enabled":allow_guests}),
            )
        }
        CollabCmd::Invite {
            project,
            role,
            subject,
            guest,
        } => (
            "invite".into(),
            json!({"project":project,"role":role.unwrap_or(if guest{Role::Viewer}else{Role::Member}).value(),"subject":subject,"guest":guest}),
        ),
        CollabCmd::Invitations {
            cmd: InvitationsCmd::List { project },
        } => ("invitations.list".into(), json!({"project":project})),
        CollabCmd::Invitations {
            cmd:
                InvitationsCmd::Revoke {
                    project,
                    invitation,
                },
        } => (
            "invitations.revoke".into(),
            json!({"project":project,"invitation":invitation}),
        ),
        CollabCmd::Join {
            hub,
            project,
            invite_stdin,
            name,
        } => (
            "join".into(),
            json!({"hub_url":hub,"project":project,"invitation_token":if invite_stdin{Some(secret_stdin("invitation token")?)}else{None},"name":name}),
        ),
        CollabCmd::Applications {
            cmd: ApplicationsCmd::Status { hub, application },
        } => (
            "applications.status".into(),
            json!({"hub_url":hub,"application":application}),
        ),
        CollabCmd::Applications {
            cmd: ApplicationsCmd::List { project },
        } => ("applications.list".into(), json!({"project":project})),
        CollabCmd::Applications {
            cmd:
                ApplicationsCmd::Approve {
                    project,
                    application,
                },
        } => (
            "applications.approve".into(),
            json!({"project":project,"application":application}),
        ),
        CollabCmd::Applications {
            cmd:
                ApplicationsCmd::Reject {
                    project,
                    application,
                },
        } => (
            "applications.reject".into(),
            json!({"project":project,"application":application}),
        ),
        CollabCmd::Applications {
            cmd:
                ApplicationsCmd::Redeem {
                    hub,
                    application,
                    repo,
                    code_stdin,
                },
        } => (
            "applications.redeem".into(),
            json!({"hub_url":hub,"application":application,"repository":repo,"code":if code_stdin{Some(secret_stdin("join code")?)}else{None}}),
        ),
        CollabCmd::Members {
            cmd: MembersCmd::List { project },
        } => ("members.list".into(), json!({"project":project})),
        CollabCmd::Members {
            cmd:
                MembersCmd::SetRole {
                    project,
                    member,
                    role,
                },
        } => (
            "members.set-role".into(),
            json!({"project":project,"member":member,"role":role.value()}),
        ),
        CollabCmd::Members {
            cmd: MembersCmd::Remove { project, member },
        } => (
            "members.remove".into(),
            json!({"project":project,"member":member}),
        ),
        CollabCmd::Messages {
            cmd: MessagesCmd::List { project },
        } => ("messages.list".into(), json!({"project":project})),
        CollabCmd::Messages {
            cmd:
                MessagesCmd::Send {
                    project,
                    text,
                    member,
                    agent,
                    reply_to,
                },
        } => {
            let text = if text == "-" {
                secret_stdin("message text")?
            } else {
                text
            };
            let direct = participant(member, agent);
            let target = if direct.is_null() {
                json!({"kind":"project"})
            } else {
                json!({"kind":"direct","participant":direct})
            };
            (
                "messages.send".into(),
                json!({"project":project,"text":text,"target":target,"reply_to":reply_to}),
            )
        }
        CollabCmd::Tasks {
            cmd: TasksCmd::List { project },
        } => ("tasks.list".into(), json!({"project":project})),
        CollabCmd::Tasks {
            cmd:
                TasksCmd::Create {
                    project,
                    title,
                    description,
                    dependency,
                },
        } => (
            "tasks.create".into(),
            json!({"project":project,"title":title,"description":description,"dependencies":dependency}),
        ),
        CollabCmd::Tasks {
            cmd:
                TasksCmd::Claim {
                    project,
                    task,
                    expected_revision,
                },
        } => (
            "tasks.claim".into(),
            json!({"project":project,"task":task,"expected_revision":expected_revision}),
        ),
        CollabCmd::Tasks {
            cmd:
                TasksCmd::Update {
                    project,
                    task,
                    expected_revision,
                    claim_generation,
                    status,
                    title,
                    description,
                    report,
                },
        } => (
            "tasks.update".into(),
            json!({"project":project,"task":task,"expected_revision":expected_revision,"claim_generation":claim_generation,"status":status,"title":title,"description":description,"report":report.map(|text|json!({"text":text,"references":[]}))}),
        ),
        CollabCmd::Tasks {
            cmd:
                TasksCmd::Reconcile {
                    project,
                    task,
                    expected_revision,
                    claim_generation,
                    acknowledge_unknown_work,
                    reason,
                    member,
                    agent,
                },
        } => (
            "tasks.reconcile".into(),
            json!({"project":project,"task":task,"expected_revision":expected_revision,"claim_generation":claim_generation,"acknowledge_unknown_work":acknowledge_unknown_work,"reason":reason,"assignee":participant(member,agent)}),
        ),
        CollabCmd::Leave { project } => ("leave".into(), json!({"project":project})),
        CollabCmd::Serve(_) => return Err(CliError::runtime("hub server must run in foreground")),
    })
}
pub fn run(cmd: CollabCmd, ctx: &Context, _global: &GlobalArgs) -> Result<(), CliError> {
    if let CollabCmd::Serve(args) = cmd {
        return serve(args, ctx);
    }
    let (operation, mut input) = prepare(cmd)?;
    input["directory"] = json!(ctx.location.display().to_string());
    input["operation_id"] = json!(format!("cop_{}", ulid::Ulid::new()));
    super::serve::runtime()?.block_on(async {
        let info = super::serve::start(ctx).await?;
        let client = Client::http(&info.registration.url, Some(info.password))
            .at(&ctx.location.display().to_string());
        let value = if operation == "status" {
            client
                .get(&format!(
                    "/collaboration/status?{}",
                    url::form_urlencoded::Serializer::new(String::new())
                        .append_pair("directory", &ctx.location.display().to_string())
                        .extend_pairs(
                            input
                                .get("session_id")
                                .and_then(Value::as_str)
                                .map(|session| ("session_id", session))
                        )
                        .finish()
                ))
                .await
        } else {
            client
                .post(&format!("/collaboration/owner/{operation}"), input)
                .await
        }
        .map_err(client_error)?;
        output::json(value.get("data").unwrap_or(&value))
    })
}
fn client_error(error: cyber_client::ClientError) -> CliError {
    match &error {
        cyber_client::ClientError::Api { tag, .. }
            if matches!(
                tag.as_str(),
                "NotLoggedInError"
                    | "MissingScopeError"
                    | "SessionExpiredError"
                    | "InactiveIdentityError"
                    | "InvalidTokenError"
            ) =>
        {
            CliError {
                code: 6,
                message: error.to_string(),
                hint: Some(
                    "run cyber login (or cyber login --reauth for a fresh owner action)".into(),
                ),
            }
        }
        cyber_client::ClientError::Api { status: 400, .. } => CliError::usage(error.to_string()),
        _ => CliError::runtime(error.to_string()),
    }
}
fn serve(args: CollabServeArgs, ctx: &Context) -> Result<(), CliError> {
    use cyber_core::env::EnvSource;
    if !cyber_core::features::Features::from_config(&ctx.global_config()?, &Default::default())
        .enabled("collaboration")
    {
        return Err(CliError::usage(
            "enable features.collaboration in user configuration or pass --features collaboration",
        ));
    }
    let read_secret = |name: &str| -> Result<cyber_account::Secret, CliError> {
        if name.is_empty() || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
            return Err(CliError::usage(
                "invalid credential environment variable name",
            ));
        }
        let value = ctx.env.get(name).filter(|v| !v.is_empty()).ok_or_else(|| {
            CliError::usage(
                "backend introspection credential environment variable is missing or empty",
            )
        })?;
        Ok(cyber_account::Secret::new(value))
    };
    let credentials = if let Some(name) = args.introspection_token_env {
        cyber_collab::BackendCredentials::Token(read_secret(&name)?)
    } else {
        cyber_collab::BackendCredentials::Client {
            client_id: args
                .introspection_client_id
                .ok_or_else(|| CliError::usage("backend introspection client ID required"))?,
            client_secret: read_secret(
                args.introspection_client_secret_env
                    .as_deref()
                    .ok_or_else(|| {
                        CliError::usage("backend credential environment variable required")
                    })?,
            )?,
            scope: args.introspection_scope,
        }
    };
    let mut config = cyber_collab::ServeConfig::new(
        args.issuer,
        args.audience,
        args.bootstrap_sub,
        args.database
            .unwrap_or_else(|| ctx.paths.data.join("collaboration-hub.db")),
    );
    config.listen = args.listen;
    config.introspection = Some(credentials);
    config.allow_guests = args.allow_guests;
    config.require_mfa = args.require_mfa;
    config.enabled = true;
    config.allow_loopback_http = args.allow_loopback_http;
    config.trusted_proxy = args.trusted_proxy;
    config.tls = args
        .tls_cert
        .zip(args.tls_key)
        .map(|(certificate, private_key)| cyber_collab::TlsOptions {
            certificate,
            private_key,
        });
    super::serve::runtime()?
        .block_on(cyber_collab::serve(config, async {
            let _ = tokio::signal::ctrl_c().await;
        }))
        .map_err(|e| CliError::runtime(e.to_string()))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn guest_invitations_default_to_viewer_unless_explicitly_elevated() {
        let invitation = |role| {
            prepare(CollabCmd::Invite {
                project: "cprj_one".into(),
                role,
                subject: None,
                guest: true,
            })
            .unwrap()
            .1
        };
        assert_eq!(invitation(None)["role"], "viewer");
        assert_eq!(invitation(Some(Role::Member))["role"], "member");
    }
    #[test]
    fn direct_targets_preserve_participant_kind() {
        assert_eq!(
            participant(Some("cmem_one".into()), None),
            json!({"kind":"human","id":"cmem_one"})
        );
        assert_eq!(
            participant(None, Some("cagt_one".into())),
            json!({"kind":"agent","id":"cagt_one"})
        );
    }
    #[test]
    fn publish_has_no_local_session_in_public_payload() {
        let (op, input) = prepare(CollabCmd::Publish {
            project: "cprj_one".into(),
            session: "ses_local".into(),
            name: "reviewer".into(),
            subscribe_project_messages: false,
        })
        .unwrap();
        assert_eq!(op, "publish");
        assert_eq!(input["session_id"], "ses_local");
        assert!(input.get("capability").is_none());
    }
}

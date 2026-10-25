//! Public coordination DTOs. Mutation authors and roles always come from credentials.
use crate::{ids::*, model::*, repository::Repository};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Fresh human/project authorization for an owner-selected integration reference.
/// This attests identity and binding, never a patch's verification result.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ContributionAuthorization {
    pub project: crate::ids::ProjectId,
    pub repository: crate::repository::Repository,
    pub member: crate::ids::MemberId,
    pub member_generation: u64,
    pub project_revision: u64,
    pub task: Option<String>,
    pub agent: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct Data<T> {
    pub data: T,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Mutation<T> {
    pub operation_id: String,
    pub payload: T,
}
impl<T> Mutation<T> {
    pub fn new(payload: T) -> Self {
        Self {
            operation_id: format!("cop_{}", ulid::Ulid::new()),
            payload,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct MutationResult<T> {
    pub data: T,
    pub replayed: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct HubHello {
    pub hub_id: HubId,
    pub issuer: String,
    pub audience: String,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Empty {}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateProject {
    pub name: String,
    pub repository: String,
    pub mode: ProjectMode,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdatePolicy {
    pub expected_revision: u64,
    pub enrollment: Enrollment,
    pub guests_enabled: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetRole {
    pub role: Role,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Invite {
    pub role: Role,
    pub subject: Option<String>,
    #[serde(default)]
    pub guest: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct InvitationView {
    pub id: InvitationId,
    pub project: ProjectId,
    pub role: Role,
    pub subject: Option<String>,
    pub guest: bool,
    pub expires_at: u64,
    pub revoked: bool,
    pub used: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct IssuedInvitation {
    pub invitation: InvitationView,
    pub token: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Join {
    pub invitation_token: Option<String>,
    pub project: Option<ProjectId>,
    pub name: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct ApplicationView {
    pub id: ApplicationId,
    pub project: Option<ProjectId>,
    pub subject: String,
    pub role: Role,
    pub kind: MembershipKind,
    pub status: ApplicationStatus,
    pub challenge: Option<ChallengeId>,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ApplicationStatus {
    PendingApproval,
    NeedsConfirmation,
    NeedsCode,
    Joined,
    Rejected,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct JoinOutcome {
    pub status: ApplicationStatus,
    pub application: Option<ApplicationView>,
    pub project: Option<Project>,
    pub member: Option<Member>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Redeem {
    pub repository: Repository,
    pub code: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct IssuedChallenge {
    pub application: ApplicationView,
    pub code: Option<String>,
    pub expires_at: Option<u64>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RegisterRuntime {
    pub name: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct RuntimeRegistration {
    pub runtime: RuntimeId,
    pub lease_expires_at: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AgentPresence {
    pub agent: AgentId,
    pub state: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Heartbeat {
    pub agents: Vec<AgentPresence>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Publish {
    pub runtime: RuntimeId,
    pub publication_key: String,
    pub name: String,
    pub repository: Repository,
    #[serde(default)]
    pub subscribe_project_messages: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct Published {
    pub agent: Agent,
    pub capability: Option<String>,
    pub expires_at: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SendMessage {
    pub target: MessageTarget,
    pub text: String,
    pub reply_to: Option<MessageId>,
    #[serde(default)]
    pub depth: u32,
    #[serde(default = "crate::model::default_hops")]
    pub hops: u32,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct Delivery {
    pub message: Message,
    pub target: AgentId,
    pub state: DeliveryState,
    pub content_hash: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Acknowledge {
    pub state: DeliveryState,
    pub content_hash: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct DeliveryAuthorization {
    pub allowed: bool,
    pub reason: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateTask {
    pub title: String,
    pub description: String,
    #[serde(default)]
    pub dependencies: Vec<TaskId>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ClaimTask {
    pub expected_revision: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReportInput {
    pub text: String,
    #[serde(default)]
    pub references: Vec<GitReference>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateTask {
    pub expected_revision: u64,
    pub claim_generation: u64,
    pub status: Option<TaskStatus>,
    pub title: Option<String>,
    pub description: Option<String>,
    pub dependencies: Option<Vec<TaskId>>,
    pub report: Option<ReportInput>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReconcileTask {
    pub expected_revision: u64,
    pub claim_generation: u64,
    pub assignee: Option<Participant>,
    pub acknowledge_unknown_work: bool,
    pub reason: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct ProjectEvent {
    pub cursor: u64,
    pub kind: String,
    pub data: serde_json::Value,
    pub created_at: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct EventPage {
    pub events: Vec<ProjectEvent>,
    pub cursor: u64,
    pub has_more: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct Snapshot {
    pub project: Project,
    pub members: Vec<Member>,
    pub agents: Vec<Agent>,
    pub messages: Vec<Message>,
    pub tasks: Vec<Task>,
    pub cursor: u64,
}

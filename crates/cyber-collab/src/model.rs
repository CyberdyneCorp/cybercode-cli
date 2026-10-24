use crate::{ids::*, repository::Repository};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Owner,
    Member,
    Viewer,
}
impl Role {
    pub fn can_write(self) -> bool {
        matches!(self, Self::Owner | Self::Member)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MembershipKind {
    Internal,
    Guest,
    Friend,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProjectMode {
    Organization { organization_id: String },
    Friends,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Enrollment {
    InviteOnly,
    OrganizationMembers { role: Role },
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct Project {
    pub id: ProjectId,
    pub name: String,
    pub repository: Repository,
    pub mode: ProjectMode,
    pub enrollment: Enrollment,
    pub guests_enabled: bool,
    pub revision: u64,
    pub created_at: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct Member {
    pub id: MemberId,
    pub project: ProjectId,
    pub issuer: String,
    pub subject: String,
    pub name: String,
    pub role: Role,
    pub kind: MembershipKind,
    pub generation: u64,
    pub active: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum Participant {
    Human(MemberId),
    Agent(AgentId),
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct Agent {
    pub id: AgentId,
    pub project: ProjectId,
    pub owner: MemberId,
    pub runtime: RuntimeId,
    pub name: String,
    pub kind: MembershipKind,
    pub active: bool,
    pub online: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", content = "participant", rename_all = "snake_case")]
pub enum MessageTarget {
    Project,
    Direct(Participant),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryState {
    Queued,
    Held,
    Released,
    Refused,
    Dropped,
    Expired,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct Message {
    pub id: MessageId,
    pub project: ProjectId,
    pub author: Participant,
    pub owner: MemberId,
    pub membership_kind: MembershipKind,
    pub member_generation: u64,
    #[serde(default)]
    pub depth: u32,
    #[serde(default = "default_hops")]
    pub hops: u32,
    pub target: MessageTarget,
    pub reply_to: Option<MessageId>,
    pub text: String,
    pub created_at: u64,
    pub expires_at: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Todo,
    InProgress,
    Blocked,
    Done,
    Cancelled,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct GitReference {
    pub kind: GitReferenceKind,
    pub value: String,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GitReferenceKind {
    Commit,
    Branch,
    Issue,
    PullRequest,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct Report {
    pub author: Participant,
    pub text: String,
    pub references: Vec<GitReference>,
    pub created_at: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct Task {
    pub id: TaskId,
    pub project: ProjectId,
    pub creator: Participant,
    pub title: String,
    pub description: String,
    pub status: TaskStatus,
    pub assignee: Option<Participant>,
    pub assignee_state: Option<String>,
    pub dependencies: Vec<TaskId>,
    pub revision: u64,
    pub claim_generation: u64,
    pub reports: Vec<Report>,
    pub created_at: u64,
    pub updated_at: u64,
}

pub fn default_hops() -> u32 {
    1
}

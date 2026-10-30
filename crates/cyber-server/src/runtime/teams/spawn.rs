//! Team creation and teammate spawning (`agent-teams` → Team composition, Team
//! definitions; design section 4). Teammates are root Sessions with the `team` tag and
//! the title `<team>/<member>`, so they outlive the lead. The size check and the spawns it
//! admits run under the runtime's spawn lock: the spawn that would exceed
//! `teams.max_teammates` fails with `Team size limit reached (<n>)` and creates nothing.

use cyber_core::teams::{Isolation, RESERVED_NAMES, TeamDefinition, TeamSettings};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use super::records::{self, Created, MemberSpawned, Role, TeamTag};
use super::{INSTRUCTIONS_REASON, Member, Team};
use crate::runtime::events::{SYSTEM_ADDED, SystemAdded, event};
use crate::runtime::{
    Admission, CreateSession, Delivery, Runtime, RuntimeError, SessionInfo, TeamWorktree,
};

/// The agent a teammate runs when none is named.
const DEFAULT_AGENT: &str = "build";

/// One teammate to spawn (`team_spawn`).
#[derive(Debug, Clone, Default, PartialEq, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TeamSpawn {
    pub name: String,
    /// Default `build`.
    #[serde(default)]
    pub agent: Option<String>,
    /// Default: the lead's model.
    #[serde(default)]
    pub model: Option<String>,
    /// Default: `worktree` when two or more teammates can edit, else `shared`.
    #[serde(default)]
    pub isolation: Option<Isolation>,
    /// Globs, relative to the team directory, only this teammate may edit.
    #[serde(default)]
    pub owns: Vec<String>,
    /// Admitted as the teammate's first input, with `source: "team"`.
    #[serde(default)]
    pub prompt: Option<String>,
}

/// How `POST /teams` starts a team: a definition, teammates, or both, and an optional
/// budget over the definition's.
#[derive(Debug, Clone, Default)]
pub struct TeamStart {
    pub definition: Option<TeamDefinition>,
    pub teammates: Vec<TeamSpawn>,
    pub budget: Option<cyber_core::budget::Budget>,
}

/// A spawned teammate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, schemars::JsonSchema)]
pub struct SpawnedMember {
    pub name: String,
    pub session_id: String,
    pub agent: String,
    pub isolation: Isolation,
    pub directory: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
}

/// A validated spawn, with its agent and isolation chosen.
struct Planned {
    spec: TeamSpawn,
    agent: String,
    isolation: Isolation,
}

impl Runtime {
    /// Start a team from `definition` with `lead_id` as its lead: record it, admit the
    /// definition body to the lead as a `team_instructions` system message, then spawn
    /// the defined teammates.
    pub async fn create_team(
        &self,
        lead_id: &str,
        definition: &TeamDefinition,
    ) -> Result<(Team, Vec<SpawnedMember>), RuntimeError> {
        let start = TeamStart {
            definition: Some(definition.clone()),
            ..TeamStart::default()
        };
        self.start_team(lead_id, start).await
    }

    /// Start a team led by `lead_id` (`POST /teams`): from a definition, from teammates,
    /// or both (the definition's first). The budget is `start.budget`, else the
    /// definition's, else `budgets.team`. Everything is checked before the team is
    /// recorded, so a refused start leaves nothing.
    pub async fn start_team(
        &self,
        lead_id: &str,
        start: TeamStart,
    ) -> Result<(Team, Vec<SpawnedMember>), RuntimeError> {
        let lead = self.state(lead_id).await?.info;
        let settings = self.team_settings(&lead)?;
        let definition = start.definition.as_ref();
        if definition.is_none() && start.teammates.is_empty() {
            return Err(RuntimeError::Invalid(
                "A team needs a definition or at least one teammate".into(),
            ));
        }
        let _spawning = self.inner.team_spawn.lock().await;
        if let Some((team, _)) = self.team_membership(lead_id)? {
            return Err(RuntimeError::Conflict(format!(
                "Session {lead_id} is already a member of team {}",
                team.id
            )));
        }
        let specs = definition
            .into_iter()
            .flat_map(|d| &d.teammates)
            .map(|teammate| TeamSpawn {
                name: teammate.name.clone(),
                agent: Some(teammate.agent.clone()),
                model: teammate.model.clone(),
                isolation: teammate.isolation,
                owns: teammate.owns.clone(),
                prompt: None,
            })
            .chain(start.teammates.iter().cloned())
            .collect();
        let planned = self.plan(&lead, &[], specs, &settings)?;
        let budget = start
            .budget
            .clone()
            .or_else(|| definition.and_then(|d| d.budget.clone()))
            .or(settings.budget.clone());
        let team = self.record_team(&lead, definition, budget).await?;
        let spawned = self.spawn_planned(&lead, &team, planned).await?;
        Ok((team, spawned))
    }

    /// `team_spawn`: spawn teammates of the team `lead_id` leads, creating an ad-hoc
    /// team (`adhoc`) first when it leads none.
    pub async fn team_spawn(
        &self,
        lead_id: &str,
        specs: Vec<TeamSpawn>,
    ) -> Result<Vec<SpawnedMember>, RuntimeError> {
        let lead = self.state(lead_id).await?.info;
        let settings = self.team_settings(&lead)?;
        if specs.is_empty() {
            return Err(RuntimeError::Invalid(
                "team_spawn needs at least one teammate".into(),
            ));
        }
        let _spawning = self.inner.team_spawn.lock().await;
        let (team, members) = match self.team_membership(lead_id)? {
            Some((team, _)) if team.status != "active" => {
                return Err(RuntimeError::Conflict(format!(
                    "Team {} is {}",
                    team.id, team.status
                )));
            }
            Some((team, _)) => {
                let members = self.team_members(&team.id)?;
                (Some(team), members)
            }
            None => (None, Vec::new()),
        };
        let planned = self.plan(&lead, &members, specs, &settings)?;
        let team = match team {
            Some(team) => team,
            None => {
                self.record_team(&lead, None, settings.budget.clone())
                    .await?
            }
        };
        self.spawn_planned(&lead, &team, planned).await
    }

    /// The Location's team settings, refused while `teams` is off or for a lead that
    /// cannot lead: a teammate (no nested teams) or a child Session, whose own ceilings
    /// its teammates would not inherit.
    fn team_settings(&self, lead: &SessionInfo) -> Result<TeamSettings, RuntimeError> {
        if lead
            .team
            .as_ref()
            .is_some_and(|tag| tag.role == Role::Teammate)
        {
            return Err(RuntimeError::Invalid("Teammates cannot lead a team".into()));
        }
        if lead.parent_id.is_some() {
            return Err(RuntimeError::Invalid(
                "Only a top-level Session can lead a team".into(),
            ));
        }
        self.inner
            .tools
            .team_settings(&lead.directory)
            .map_err(RuntimeError::Invalid)?
            .ok_or(RuntimeError::FeatureDisabled(cyber_core::teams::DISABLED))
    }

    /// Check a spawn against the team as it is: the size limit first, then each name
    /// and agent; choose each teammate's agent and isolation.
    fn plan(
        &self,
        lead: &SessionInfo,
        members: &[Member],
        specs: Vec<TeamSpawn>,
        settings: &TeamSettings,
    ) -> Result<Vec<Planned>, RuntimeError> {
        let teammates: Vec<&Member> = members
            .iter()
            .filter(|member| member.role == Role::Teammate)
            .collect();
        if teammates.len() + specs.len() > settings.max_teammates as usize {
            return Err(RuntimeError::TeamLimit(settings.max_teammates));
        }
        let mut taken: Vec<&str> = teammates.iter().map(|m| m.name.as_str()).collect();
        for spec in &specs {
            check_name(&spec.name, &taken)?;
            check_owns(spec)?;
            taken.push(&spec.name);
        }
        let agents = specs
            .iter()
            .map(|spec| {
                let agent = spec.agent.clone().unwrap_or_else(|| DEFAULT_AGENT.into());
                // An unknown or hidden agent fails here, before anything is created.
                self.agent_permission_mode(&lead.directory, &agent)?;
                Ok(agent)
            })
            .collect::<Result<Vec<_>, RuntimeError>>()?;
        let editors = teammates
            .iter()
            .map(|member| member.agent.as_str())
            .chain(agents.iter().map(String::as_str))
            .filter(|agent| self.inner.tools.team_editor(&lead.directory, agent))
            .count();
        let isolated = editors >= 2 && in_git_checkout(&lead.directory);
        Ok(specs
            .into_iter()
            .zip(agents)
            .map(|(spec, agent)| Planned {
                isolation: spec.isolation.unwrap_or(if isolated {
                    Isolation::Worktree
                } else {
                    Isolation::Shared
                }),
                spec,
                agent,
            })
            .collect())
    }

    /// Record `team.created.1` and the lead's `team.member.spawned.1` on the lead's
    /// stream, with the definition body as a `team_instructions` system message, under
    /// `budget`; reserved enforcement requires request-level atomic admission.
    async fn record_team(
        &self,
        lead: &SessionInfo,
        definition: Option<&TeamDefinition>,
        budget: Option<cyber_core::budget::Budget>,
    ) -> Result<Team, RuntimeError> {
        crate::runtime::budget::validate(budget.as_ref())?;
        let team_id = cyber_core::ids::new_id("team");
        let instructions = definition
            .map(|d| d.instructions.trim().to_owned())
            .filter(|text| !text.is_empty());
        let created = Created {
            team_id: team_id.clone(),
            name: definition.map_or_else(|| "adhoc".into(), |d| d.name.clone()),
            directory: lead.directory.clone(),
            budget,
            definition: definition.map(|d| d.name.clone()),
            instructions: instructions.clone(),
        };
        let member = MemberSpawned {
            team_id: team_id.clone(),
            name: "lead".into(),
            session_id: lead.id.clone(),
            role: Role::Lead,
            agent: lead.agent.clone(),
            model: Some(lead.model.clone()),
            isolation: Isolation::Shared,
            worktree_id: None,
            branch: None,
            owns: Vec::new(),
        };
        let mut events = vec![
            event(records::CREATED, &created),
            event(records::MEMBER_SPAWNED, &member),
        ];
        if let Some(text) = instructions {
            events.push(event(
                SYSTEM_ADDED,
                &SystemAdded {
                    message_id: cyber_core::ids::new_id("msg"),
                    text: format!("<team_instructions>\n{text}\n</team_instructions>"),
                    reason: INSTRUCTIONS_REASON.into(),
                },
            ));
        }
        let handle = self.inner.handle(&lead.id).await?;
        self.inner.commit(&handle, events).await?;
        self.team(&team_id)?
            .ok_or_else(|| RuntimeError::Corrupt(format!("team {team_id} was not projected")))
    }

    /// Spawn each planned teammate in order. A failure after some were spawned names
    /// them, since they stay members.
    async fn spawn_planned(
        &self,
        lead: &SessionInfo,
        team: &Team,
        planned: Vec<Planned>,
    ) -> Result<Vec<SpawnedMember>, RuntimeError> {
        let mut spawned: Vec<SpawnedMember> = Vec::new();
        for plan in planned {
            match self.spawn_member(lead, team, plan).await {
                Ok(member) => spawned.push(member),
                Err(error) if spawned.is_empty() => return Err(error),
                Err(error) => {
                    let names: Vec<&str> = spawned.iter().map(|m| m.name.as_str()).collect();
                    let error = match error {
                        RuntimeError::Invalid(message) => message,
                        other => other.to_string(),
                    };
                    return Err(RuntimeError::Invalid(format!(
                        "{error} (already spawned: {})",
                        names.join(", ")
                    )));
                }
            }
        }
        Ok(spawned)
    }

    async fn spawn_member(
        &self,
        lead: &SessionInfo,
        team: &Team,
        plan: Planned,
    ) -> Result<SpawnedMember, RuntimeError> {
        let Planned {
            spec,
            agent,
            isolation,
        } = plan;
        let model = spec.model.clone().unwrap_or_else(|| lead.model.clone());
        let request = CreateSession {
            id: Some(cyber_core::ids::new_id("ses")),
            directory: lead.directory.clone(),
            model: model.clone(),
            agent: Some(agent.clone()),
            title: Some(format!("{}/{}", team.name, spec.name)),
            team: Some(TeamTag {
                team_id: team.id.clone(),
                role: Role::Teammate,
                name: spec.name.clone(),
            }),
            ..CreateSession::default()
        };
        let (info, branch) = match isolation {
            Isolation::Shared => (self.create_session(request).await?, None),
            Isolation::Worktree => {
                let worktree = TeamWorktree::new(&team.id, &spec.name);
                let info = self
                    .inner
                    .tools
                    .create_team_session(lead, request, &worktree, CancellationToken::new())
                    .await
                    .map_err(RuntimeError::Invalid)?;
                (info, Some(worktree.branch))
            }
        };
        let spawned = MemberSpawned {
            team_id: team.id.clone(),
            name: spec.name.clone(),
            session_id: info.id.clone(),
            role: Role::Teammate,
            agent: agent.clone(),
            model: Some(model),
            isolation,
            worktree_id: info.worktree_id.clone(),
            branch: branch.clone(),
            owns: spec.owns.clone(),
        };
        let handle = self.inner.handle(&lead.id).await?;
        self.inner
            .commit(&handle, vec![event(records::MEMBER_SPAWNED, &spawned)])
            .await?;
        if let Some(prompt) = spec.prompt.filter(|text| !text.trim().is_empty()) {
            let admission = Admission {
                source: "team".into(),
                ..Admission::text(prompt, Delivery::Queue)
            };
            // A teammate spawned by a tainted lead starts tainted.
            let untrusted = self.handed_taint(&lead.id).await;
            self.admit_recorded(
                &info.id,
                admission,
                None,
                crate::runtime::PromptCheck::Checked(None),
                Vec::new(),
                untrusted,
            )
            .await?;
        }
        Ok(SpawnedMember {
            name: spec.name,
            session_id: info.id,
            agent,
            isolation,
            directory: info.directory,
            branch,
        })
    }
}

/// A teammate name: valid, not reserved, unused in the team, and short enough for its
/// worktree name.
fn check_name(name: &str, taken: &[&str]) -> Result<(), RuntimeError> {
    if !cyber_core::teams::valid_name(name) || RESERVED_NAMES.contains(&name) {
        return Err(RuntimeError::Invalid(format!(
            "Teammate name {name:?} is not valid: use [a-z][a-z0-9-]{{0,31}}, not lead or all"
        )));
    }
    if taken.contains(&name) {
        return Err(RuntimeError::Invalid(format!(
            "Teammate name {name} is already in the team"
        )));
    }
    let worktree = TeamWorktree::new(&cyber_core::ids::new_id("team"), name);
    cyber_core::worktrees::Name::parse(&worktree.name)
        .map(drop)
        .map_err(|_| RuntimeError::Invalid(format!("Teammate name {name} is too long")))
}

/// Every `owns` glob of a spawn is relative to the team directory and valid, so the
/// ownership guard never meets one it cannot apply.
fn check_owns(spec: &TeamSpawn) -> Result<(), RuntimeError> {
    spec.owns.iter().try_for_each(|glob| {
        cyber_core::teams::check_owned_glob(glob)
            .map_err(|error| RuntimeError::Invalid(format!("Teammate {} owns: {error}", spec.name)))
    })
}

/// A git checkout, where a worktree can isolate a teammate.
fn in_git_checkout(directory: &str) -> bool {
    cyber_core::project::identify(std::path::Path::new(directory)).id != "global"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn teammate_names_are_checked_before_anything_is_created() {
        assert!(check_name("api", &[]).is_ok());
        for name in ["lead", "all", "API", "", "1api"] {
            assert!(check_name(name, &[]).is_err(), "{name}");
        }
        assert!(
            check_name("api", &["api"])
                .unwrap_err()
                .to_string()
                .contains("already in the team")
        );
        // `team-<ulid>-` takes 32 of the 63 worktree name characters.
        assert!(check_name(&"a".repeat(31), &[]).is_ok());
        assert!(
            check_name(&"a".repeat(32), &[])
                .unwrap_err()
                .to_string()
                .contains("too long")
        );
    }

    #[test]
    fn owns_globs_are_checked_before_anything_is_created() {
        let spec = |owns: &[&str]| TeamSpawn {
            name: "api".into(),
            owns: owns.iter().map(|glob| (*glob).to_owned()).collect(),
            ..TeamSpawn::default()
        };
        assert!(check_owns(&spec(&["server/**", "docs/*.md"])).is_ok());
        for bad in ["/etc/**", "../outside/**", "["] {
            let error = check_owns(&spec(&[bad])).unwrap_err().to_string();
            assert!(error.contains("Teammate api owns:"), "{error}");
        }
    }

    #[test]
    fn team_worktrees_are_named_after_the_team_and_member() {
        let worktree = TeamWorktree::new("team_01J0ABCDEF", "api");
        assert_eq!(worktree.name, "team-01j0abcdef-api");
        assert_eq!(worktree.branch, "cyber/team-01j0abcdef/api");
    }
}

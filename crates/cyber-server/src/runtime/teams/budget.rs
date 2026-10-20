//! Team budgets (`agent-teams` → Team budgets; design section 11): one more soft scope
//! over `runtime/budget.rs`. A team's budget is its definition's `budget`, else
//! `budgets.team`, else none. Its spend is every member's `usage_step` rows since the
//! team was created (a member's subagents count through their root; the lead's share
//! includes earlier leads'), plus wall time since creation.
//!
//! `check_team` runs at each Turn start of a teammate or of one of its descendants
//! (subagents, background children, Workflow agents), before input is promoted: a team
//! that is not `active` starts no such Turn (its input stays queued), and the first
//! teammate to find the budget exceeded records `team.budget.exceeded.1` with the
//! per-member breakdown, which moves the team to `stopping` (reason `budget`), and
//! admits the breakdown to the lead in the same commit. Teammates stop after their
//! current Turn; the lead's own Turns are never blocked, so it can report.

use cyber_core::budget::Budget;
use rusqlite::{OptionalExtension, params};
use serde_json::json;

use super::records::{self, BudgetExceeded, MemberSpend, Role, TeamTag};
use crate::runtime::budget::{Limit, RunSpend, limits};
use crate::runtime::events::event;
use crate::runtime::{Admission, Delivery, Handle, Inner, Runtime, RuntimeError, SessionInfo};

/// What the budget check reads of a team.
struct Budgeted {
    status: String,
    budget: Option<Budget>,
    created_ms: i64,
}

/// A team's spend since creation: the totals and each member's share, the lead first.
#[derive(Debug, Clone, Default)]
pub(in crate::runtime) struct TeamSpend {
    pub(in crate::runtime) total: RunSpend,
    pub(in crate::runtime) members: Vec<MemberSpend>,
}

/// `Team budget exceeded (max_cost_usd 20.00, spent 20.10): api $8.40, ui $6.20, tests
/// $5.50`: every teammate's cost, and the lead's when it spent anything.
pub fn exceeded_line(limit: &str, spent: f64, maximum: f64, members: &[MemberSpend]) -> String {
    let amount = |value: f64| {
        if limit == "max_cost_usd" {
            format!("{value:.2}")
        } else {
            format!("{value:.0}")
        }
    };
    let shares: Vec<String> = members
        .iter()
        .filter(|m| m.name != "lead" || m.cost_usd > 0.0)
        .map(|m| format!("{} ${:.2}", m.name, m.cost_usd))
        .collect();
    format!(
        "Team budget exceeded ({limit} {}, spent {}): {}",
        amount(maximum),
        amount(spent),
        shares.join(", ")
    )
}

fn budgeted(db: &rusqlite::Connection, team_id: &str) -> Result<Option<Budgeted>, RuntimeError> {
    let row = db
        .query_row(
            "SELECT status, budget, created_ms FROM team WHERE id = ?1",
            params![team_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            },
        )
        .optional()
        .map_err(store)?;
    let Some((status, budget, created_ms)) = row else {
        return Ok(None);
    };
    // A budget that no longer reads fails closed: the team is treated as exceeded.
    let budget = budget
        .map(|text| serde_json::from_str::<Budget>(&text))
        .transpose()
        .map_err(|error| RuntimeError::Corrupt(format!("team {team_id} budget: {error}")))?;
    Ok(Some(Budgeted {
        status,
        budget,
        created_ms,
    }))
}

/// Each member's turns, tokens and cost since `since`, the lead first. The lead's share
/// includes every earlier lead's, so deleting a lead never resets the team's spend.
fn member_spend(
    db: &rusqlite::Connection,
    team_id: &str,
    since: i64,
) -> rusqlite::Result<Vec<MemberSpend>> {
    let mut statement = db.prepare(
        "SELECT m.name,
             COALESCE(SUM(u.purpose = 'turn' AND u.failed = 0), 0),
             COALESCE(SUM(u.input_tokens + u.output_tokens + u.reasoning_tokens
                 + u.cache_read_tokens + u.cache_write_tokens), 0),
             COALESCE(SUM(COALESCE(u.cost, 0.0)), 0.0)
         FROM team_member m
         JOIN team t ON t.id = m.team_id
         LEFT JOIN usage_step u ON u.ended_ms >= ?2 AND (u.root_id = m.session_id
             OR (m.role = 'lead' AND u.root_id IN (SELECT value FROM json_each(t.former_leads))))
         WHERE m.team_id = ?1
         GROUP BY m.rowid
         ORDER BY m.role = 'teammate', m.rowid",
    )?;
    let rows = statement.query_map(params![team_id, since], |row| {
        Ok(MemberSpend {
            name: row.get(0)?,
            turns: row.get::<_, i64>(1)?.max(0) as u64,
            tokens: row.get::<_, i64>(2)?.max(0) as u64,
            cost_usd: row.get(3)?,
        })
    })?;
    rows.collect()
}

fn store(error: rusqlite::Error) -> RuntimeError {
    RuntimeError::Store(cyber_store::StoreError::Sqlite(error))
}

impl Runtime {
    /// A team's spend since it was created; `None` for an unknown team.
    pub(in crate::runtime) fn team_spend(
        &self,
        team_id: &str,
    ) -> Result<Option<TeamSpend>, RuntimeError> {
        let team_id = team_id.to_owned();
        let now = chrono::Utc::now().timestamp_millis();
        self.inner
            .store
            .read(move |db| Ok(spend_of(db, &team_id, now)))?
    }

    /// `check_team`: whether a teammate of `team_id` may start a Turn. A team that is
    /// not `active` starts none; an exceeded budget records the stop once and starts
    /// none. An unknown team fails closed.
    async fn check_team(&self, team_id: &str) -> Result<bool, RuntimeError> {
        let id = team_id.to_owned();
        let team = self
            .inner
            .store
            .read(move |db| Ok(budgeted(db, &id)))??
            .ok_or_else(|| RuntimeError::Invalid(format!("No team {team_id}")))?;
        if team.status != "active" {
            return Ok(false);
        }
        let Some(budget) = team.budget else {
            return Ok(true);
        };
        let spend = self.team_spend(team_id)?.unwrap_or_default();
        let Some(limit) = limits(&budget, &spend.total)
            .into_iter()
            .find(|limit| limit.exceeded)
        else {
            return Ok(true);
        };
        self.team_budget_exceeded(team_id, budget, spend, &limit)
            .await?;
        Ok(false)
    }

    /// Record `team.budget.exceeded.1` and admit the breakdown to the lead in one
    /// commit, unless another teammate's check already stopped the team.
    async fn team_budget_exceeded(
        &self,
        team_id: &str,
        budget: Budget,
        spend: TeamSpend,
        limit: &Limit,
    ) -> Result<(), RuntimeError> {
        let _status = self.inner.team_status.lock().await;
        let Some(team) = self.team(team_id)?.filter(|team| team.status == "active") else {
            return Ok(());
        };
        let line = exceeded_line(limit.name, limit.spent, limit.maximum, &spend.members);
        let exceeded = BudgetExceeded {
            team_id: team.id.clone(),
            budget,
            spent: json!({
                "limit": limit.name,
                "spent": limit.spent,
                "maximum": limit.maximum,
                "cost_usd": spend.total.cost,
                "tokens": spend.total.tokens,
                "turns": spend.total.turns,
                "wall_seconds": spend.total.elapsed_seconds,
            }),
            members: spend.members,
        };
        let admission = Admission {
            source: super::NOTICE_SOURCE.into(),
            resume: true,
            ..Admission::text(line, Delivery::Steer)
        };
        self.admit_message(
            &team.lead_id,
            admission,
            vec![event(records::BUDGET_EXCEEDED, &exceeded)],
        )
        .await
        .map(drop)
    }
}

/// The spend of a team at `now`; `None` for an unknown team.
fn spend_of(
    db: &rusqlite::Connection,
    team_id: &str,
    now: i64,
) -> Result<Option<TeamSpend>, RuntimeError> {
    let Some(team) = budgeted(db, team_id)? else {
        return Ok(None);
    };
    let members = member_spend(db, team_id, team.created_ms).map_err(store)?;
    let total = RunSpend {
        cost: members.iter().map(|m| m.cost_usd).sum(),
        tokens: members.iter().map(|m| m.tokens).sum(),
        turns: members.iter().map(|m| m.turns).sum(),
        elapsed_seconds: now.saturating_sub(team.created_ms).max(0) as f64 / 1000.0,
    };
    Ok(Some(TeamSpend { total, members }))
}

impl Runtime {
    /// The teammate tag that binds `info`: its own, or for a child Session (a subagent,
    /// background child or Workflow agent) its root's, so a teammate's descendants are
    /// gated and stopped like the teammate. `None` outside a team and for the lead's
    /// tree. An ancestry that cannot be read fails the caller (fail closed).
    pub(crate) async fn binding_teammate(
        &self,
        info: &SessionInfo,
    ) -> Result<Option<TeamTag>, RuntimeError> {
        let tag = if info.parent_id.is_some() {
            self.ancestors(info).await?.pop().and_then(|root| root.team)
        } else {
            info.team.clone()
        };
        Ok(tag.filter(|tag| tag.role == Role::Teammate))
    }
}

impl Inner {
    /// The team gate at a Turn start (`drain`) of a teammate or one of its descendants:
    /// `false` ends the Drain with its input still queued. A Session outside a teammate's
    /// tree passes; a child Session reads its ancestry to find out.
    pub(crate) async fn team_turn_allowed(&self, handle: &Handle) -> Result<bool, RuntimeError> {
        let info = handle.state.lock().await.info.clone();
        if info.team.is_none() && info.parent_id.is_none() {
            return Ok(true);
        }
        let inner = self.me.upgrade().ok_or(RuntimeError::ShuttingDown)?;
        let runtime = Runtime { inner };
        match runtime.binding_teammate(&info).await? {
            Some(tag) => runtime.check_team(&tag.team_id).await,
            None => Ok(true),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn share(name: &str, cost_usd: f64) -> MemberSpend {
        MemberSpend {
            name: name.into(),
            turns: 1,
            tokens: 10,
            cost_usd,
        }
    }

    #[test]
    fn the_breakdown_names_every_teammate_and_a_spending_lead() {
        let members = [
            share("lead", 0.0),
            share("api", 8.4),
            share("ui", 6.2),
            share("tests", 5.5),
        ];
        assert_eq!(
            exceeded_line("max_cost_usd", 20.1, 20.0, &members),
            "Team budget exceeded (max_cost_usd 20.00, spent 20.10): api $8.40, ui $6.20, tests $5.50"
        );
        let members = [share("lead", 1.0), share("api", 0.0)];
        assert_eq!(
            exceeded_line("max_turns", 12.0, 12.0, &members),
            "Team budget exceeded (max_turns 12, spent 12): lead $1.00, api $0.00"
        );
    }
}

//! One SQL snapshot supplies own and retained descendant billing.

use cyber_llm::Usage;
use rusqlite::OptionalExtension;
use serde::Serialize;

use super::{Runtime, RuntimeError};

#[derive(Debug, Clone, PartialEq, Serialize, schemars::JsonSchema)]
pub struct UsageAmount {
    pub tokens: Usage,
    /// Combined known tokens, including receipts whose class evidence was purged.
    pub total_tokens: u64,
    /// Known priced spending; unpriced calls and incomplete history make it a lower bound.
    pub cost: f64,
    pub unpriced_steps: u64,
    pub usage_complete: bool,
    pub token_classes_complete: bool,
}

/// The `scope` of a Session usage report; it discriminates [`ScopeUsage`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SessionScope {
    Session,
}

/// The `scope` of a Workflow Run usage report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RunScope {
    Run,
}

#[derive(Debug, Clone, PartialEq, Serialize, schemars::JsonSchema)]
pub struct UsageReport {
    pub scope: SessionScope,
    pub id: String,
    pub own: UsageAmount,
    pub descendants: UsageAmount,
    pub total: UsageAmount,
}

fn invalid(message: &str) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        0,
        rusqlite::types::Type::Text,
        Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            message.to_owned(),
        )),
    )
}

fn add(a: u64, b: u64) -> rusqlite::Result<u64> {
    a.checked_add(b)
        .ok_or_else(|| invalid("usage counter overflow"))
}

fn token_total(usage: &Usage) -> rusqlite::Result<u64> {
    [
        usage.input,
        usage.output,
        usage.reasoning,
        usage.cache_read,
        usage.cache_write,
    ]
    .into_iter()
    .try_fold(0, add)
}

fn combine(own: &UsageAmount, descendants: &UsageAmount) -> rusqlite::Result<UsageAmount> {
    let a = &own.tokens;
    let b = &descendants.tokens;
    Ok(UsageAmount {
        tokens: Usage {
            input: add(a.input, b.input)?,
            output: add(a.output, b.output)?,
            reasoning: add(a.reasoning, b.reasoning)?,
            cache_read: add(a.cache_read, b.cache_read)?,
            cache_write: add(a.cache_write, b.cache_write)?,
        },
        total_tokens: add(own.total_tokens, descendants.total_tokens)?,
        cost: valid_cost(own.cost + descendants.cost)?,
        unpriced_steps: add(own.unpriced_steps, descendants.unpriced_steps)?,
        usage_complete: own.usage_complete && descendants.usage_complete,
        token_classes_complete: own.token_classes_complete && descendants.token_classes_complete,
    })
}

fn valid_cost(cost: f64) -> rusqlite::Result<f64> {
    if cost.is_finite() && cost >= 0.0 {
        Ok(cost)
    } else {
        Err(invalid("invalid usage cost"))
    }
}

fn counter(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<u64> {
    row.get::<_, i64>(index)?
        .try_into()
        .map_err(|_| invalid("negative usage counter"))
}

fn read(row: &rusqlite::Row<'_>, id: &str) -> rusqlite::Result<UsageReport> {
    let tokens = Usage {
        input: counter(row, 1)?,
        output: counter(row, 2)?,
        reasoning: counter(row, 3)?,
        cache_read: counter(row, 4)?,
        cache_write: counter(row, 5)?,
    };
    let own = UsageAmount {
        total_tokens: token_total(&tokens)?,
        tokens,
        cost: valid_cost(row.get(0)?)?,
        unpriced_steps: counter(row, 6)?,
        usage_complete: !row.get::<_, bool>(13)?,
        token_classes_complete: true,
    };
    let tokens: Usage = serde_json::from_str(&row.get::<_, String>(11)?)
        .map_err(|_| invalid("invalid descendant token classes"))?;
    let total_tokens = counter(row, 8)?;
    if token_total(&tokens)? > total_tokens {
        return Err(invalid("descendant token classes exceed combined tokens"));
    }
    let descendants = UsageAmount {
        tokens,
        total_tokens,
        cost: valid_cost(row.get(7)?)?,
        unpriced_steps: counter(row, 9)?,
        usage_complete: row.get::<_, bool>(10)? && !row.get::<_, bool>(14)?,
        token_classes_complete: row.get::<_, bool>(12)?,
    };
    if descendants.token_classes_complete
        && token_total(&descendants.tokens)? != descendants.total_tokens
    {
        return Err(invalid(
            "complete descendant token classes disagree with combined tokens",
        ));
    }
    let total = combine(&own, &descendants)?;
    Ok(UsageReport {
        scope: SessionScope::Session,
        id: id.into(),
        own,
        descendants,
        total,
    })
}

/// Usage of one Workflow agent: its child Session with descendants.
#[derive(Debug, Clone, PartialEq, Serialize, schemars::JsonSchema)]
pub struct RunAgentUsage {
    /// `agt_` ID.
    pub agent_id: String,
    pub session_id: String,
    /// The model reference the child ran with.
    pub model: String,
    /// The tier the call named, when it named one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tier: Option<String>,
    pub cost: f64,
    pub total_tokens: u64,
    pub unpriced_steps: u64,
}

/// A rollup of a run's agents by model or by tier.
#[derive(Debug, Clone, PartialEq, Serialize, schemars::JsonSchema)]
pub struct RunUsageGroup {
    /// The model reference or tier name.
    pub key: String,
    pub agents: u64,
    pub cost: f64,
    pub total_tokens: u64,
    pub unpriced_steps: u64,
}

/// `GET /usage?scope=run`: the run's agents summed, per agent, per model and per tier
/// (`observability-costs` → per workflow run, agent and model). The orchestrator script
/// makes no model calls of its own.
#[derive(Debug, Clone, PartialEq, Serialize, schemars::JsonSchema)]
pub struct RunUsageReport {
    pub scope: RunScope,
    pub id: String,
    pub cost: f64,
    pub total_tokens: u64,
    pub unpriced_steps: u64,
    /// False when a child's descendant usage history is incomplete.
    pub usage_complete: bool,
    pub agents: Vec<RunAgentUsage>,
    pub models: Vec<RunUsageGroup>,
    pub tiers: Vec<RunUsageGroup>,
}

/// One of the usage reports a scope returns, discriminated by `scope`.
#[derive(Debug, Clone, PartialEq, Serialize, schemars::JsonSchema)]
#[serde(untagged)]
pub enum ScopeUsage {
    Session(Box<super::SessionUsage>),
    Run(RunUsageReport),
}

fn group(
    agents: &[RunAgentUsage],
    key: impl Fn(&RunAgentUsage) -> Option<String>,
) -> Vec<RunUsageGroup> {
    let mut groups: std::collections::BTreeMap<String, RunUsageGroup> = Default::default();
    for agent in agents {
        let Some(name) = key(agent) else {
            continue;
        };
        let entry = groups.entry(name.clone()).or_insert(RunUsageGroup {
            key: name,
            agents: 0,
            cost: 0.0,
            total_tokens: 0,
            unpriced_steps: 0,
        });
        entry.agents += 1;
        entry.cost += agent.cost;
        entry.total_tokens = entry.total_tokens.saturating_add(agent.total_tokens);
        entry.unpriced_steps = entry.unpriced_steps.saturating_add(agent.unpriced_steps);
    }
    groups.into_values().collect()
}

impl Runtime {
    /// Usage of a Workflow Run from the Sessions its agents created.
    pub fn run_usage(&self, id: &str) -> Result<RunUsageReport, RuntimeError> {
        if self.workflow_run(id)?.is_none() {
            return Err(RuntimeError::Invalid(format!("No Workflow Run {id}")));
        }
        let run = id.to_owned();
        let (agents, complete) = self.inner.store.read(move |db| {
            let mut statement = db.prepare(
                "SELECT COALESCE(a.id, s.subagent_name, s.id), s.id, s.model, json_extract(a.data, '$.tier'),
                    s.cost + s.children_cost,
                    s.input_tokens+s.output_tokens+s.reasoning_tokens+s.cache_read_tokens+s.cache_write_tokens+s.children_tokens,
                    s.unpriced_steps + s.children_unpriced_steps, s.children_usage_complete
                 FROM session s LEFT JOIN workflow_agent a ON a.run_id = s.workflow_run_id AND a.id = s.subagent_name
                 WHERE s.workflow_run_id = ?1 ORDER BY s.created_at, s.id",
            )?;
            let mut complete = true;
            let agents = statement
                .query_map([&run], |row| {
                    Ok((
                        RunAgentUsage {
                            agent_id: row.get(0)?,
                            session_id: row.get(1)?,
                            model: row.get(2)?,
                            tier: row.get(3)?,
                            cost: valid_cost(row.get(4)?)?,
                            total_tokens: counter(row, 5)?,
                            unpriced_steps: counter(row, 6)?,
                        },
                        row.get::<_, bool>(7)?,
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .map(|(agent, own)| {
                    complete &= own;
                    agent
                })
                .collect::<Vec<_>>();
            Ok((agents, complete))
        })?;
        Ok(RunUsageReport {
            scope: RunScope::Run,
            id: id.into(),
            cost: agents.iter().map(|a| a.cost).sum(),
            total_tokens: agents.iter().map(|a| a.total_tokens).sum(),
            unpriced_steps: agents.iter().map(|a| a.unpriced_steps).sum(),
            usage_complete: complete,
            models: group(&agents, |a| Some(a.model.clone())),
            tiers: group(&agents, |a| a.tier.clone()),
            agents,
        })
    }

    pub fn session_usage(&self, id: &str) -> Result<UsageReport, RuntimeError> {
        let source = id.to_owned();
        self.inner.store.read(move |db| {
            Ok(db.query_row("SELECT cost,input_tokens,output_tokens,reasoning_tokens,cache_read_tokens,cache_write_tokens,unpriced_steps,
                children_cost,children_tokens,children_unpriced_steps,children_usage_complete,children_token_classes,children_token_classes_complete,
                EXISTS(SELECT 1 FROM budget_reservation b WHERE b.session_id=session.id AND b.state='uncertain' AND b.receipt_event_id IS NOT NULL),
                EXISTS(WITH RECURSIVE tree(id) AS (SELECT id FROM session WHERE parent_id=?1 UNION ALL SELECT s.id FROM session s JOIN tree t ON s.parent_id=t.id) SELECT 1 FROM budget_reservation b WHERE b.session_id IN (SELECT id FROM tree) AND b.state='uncertain' AND b.receipt_event_id IS NOT NULL)
                FROM session WHERE id=?1", [&source], |row| read(row,&source)).optional()?)
        })?.ok_or_else(|| RuntimeError::SessionNotFound(id.into()))
    }
}

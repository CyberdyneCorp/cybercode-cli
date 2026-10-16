//! `GET /usage/insights` (`observability-costs` → Usage insights): which characteristics
//! of recent usage account for its total. Each factor is a share of the window total and
//! factors overlap, so they are characteristics, not a breakdown. The thresholds live here
//! and nowhere else.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use rusqlite::Connection;
use serde::Serialize;

use super::events::CREATED;
use super::usage_stats::{in_project, now_ms};
use super::{Runtime, RuntimeError};

/// A tree is subagent-heavy when its descendants used more than this share of it.
pub const SUBAGENT_HEAVY_SHARE: f64 = 0.5;
/// A Session is long-running when its first and last step in the window are at least
/// this far apart.
pub const LONG_RUNNING_MS: i64 = 8 * 3_600_000;
/// A step has a large context when its prompt tokens exceed this.
pub const LARGE_CONTEXT_TOKENS: u64 = 150_000;
/// The scope note every response carries.
pub const SCOPE_NOTE: &str = "Covers only Sessions stored by this server";

/// The look-back window of an insights request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsightWindow {
    Day,
    Week,
}

impl InsightWindow {
    /// `24h` or `7d`; anything else is refused.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "24h" => Some(Self::Day),
            "7d" => Some(Self::Week),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Day => "24h",
            Self::Week => "7d",
        }
    }

    pub fn duration_ms(self) -> i64 {
        match self {
            Self::Day => 86_400_000,
            Self::Week => 7 * 86_400_000,
        }
    }
}

/// What `GET /usage/insights` asks for.
#[derive(Debug, Clone)]
pub struct InsightsRequest {
    pub window: InsightWindow,
    /// Only Sessions whose directory is inside this project root.
    pub project: Option<PathBuf>,
}

/// What the shares are shares of.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum InsightBasis {
    /// Every step in the window was priced.
    Cost,
    /// Some step was unpriced, so shares are of tokens.
    Tokens,
}

/// A contributing factor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum InsightFactorKind {
    /// Root Session trees whose descendants used more than half of the tree.
    SubagentHeavy,
    /// Sessions whose first and last step in the window are 8 or more hours apart.
    LongRunning,
    /// Steps whose prompt exceeded 150,000 tokens.
    LargeContext,
    /// (P2, reserved) Sessions that belong to a Workflow Run; never reported yet.
    Workflow,
}

#[derive(Debug, Clone, PartialEq, Serialize, schemars::JsonSchema)]
pub struct InsightFactor {
    pub factor: InsightFactorKind,
    /// Share (0–1) of the window total.
    pub share: f64,
    /// The usage behind the share, in the basis unit (USD or tokens).
    pub amount: f64,
}

/// How a subagent group is named.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SubagentGroupKind {
    /// Child Sessions running this agent.
    Agent,
    /// Child Sessions of a Workflow Run.
    Workflow,
    /// Child Sessions forked from their parent's history.
    Fork,
}

#[derive(Debug, Clone, PartialEq, Serialize, schemars::JsonSchema)]
pub struct SubagentShare {
    /// The agent name, `workflow` or `fork`.
    pub group: String,
    pub kind: SubagentGroupKind,
    /// Share (0–1) of the window total.
    pub share: f64,
    pub amount: f64,
}

/// The `GET /usage/insights` response.
#[derive(Debug, Clone, PartialEq, Serialize, schemars::JsonSchema)]
pub struct UsageInsights {
    /// `24h` or `7d`.
    pub window: String,
    pub since_ms: i64,
    pub until_ms: i64,
    pub basis: InsightBasis,
    /// The window total in the basis unit (USD or tokens).
    pub total: f64,
    /// Known priced cost of the window; null when nothing priced ran.
    pub total_cost: Option<f64>,
    pub total_tokens: u64,
    /// Finished calls in the window.
    pub steps: u64,
    /// `subagent_heavy`, `long_running` and `large_context`, in that order; they overlap.
    pub factors: Vec<InsightFactor>,
    /// Descendant usage per group, highest first.
    pub subagents: Vec<SubagentShare>,
    /// Always `server`: only Sessions stored by this server are counted.
    pub scope: String,
    pub note: String,
    /// True while stored history is still being indexed.
    pub backfilling: bool,
}

/// One finished ledger call in the window.
#[derive(Debug, Clone)]
pub(super) struct Call {
    pub session: String,
    pub root: String,
    pub run: Option<String>,
    pub agent: String,
    pub started_ms: i64,
    pub ended_ms: i64,
    pub prompt_tokens: u64,
    pub tokens: u64,
    pub cost: Option<f64>,
    pub directory: String,
}

impl Call {
    fn amount(&self, basis: InsightBasis) -> f64 {
        match basis {
            InsightBasis::Cost => self.cost.unwrap_or(0.0),
            InsightBasis::Tokens => self.tokens as f64,
        }
    }

    fn descendant(&self) -> bool {
        self.session != self.root
    }
}

fn share(amount: f64, total: f64) -> f64 {
    if total > 0.0 {
        (amount / total).clamp(0.0, 1.0)
    } else {
        0.0
    }
}

fn factor(factor: InsightFactorKind, amount: f64, total: f64) -> InsightFactor {
    InsightFactor {
        factor,
        share: share(amount, total),
        amount,
    }
}

/// Usage of trees whose descendants used more than [`SUBAGENT_HEAVY_SHARE`] of them.
fn subagent_heavy(calls: &[Call], basis: InsightBasis) -> f64 {
    let mut trees: HashMap<&str, (f64, f64)> = HashMap::new();
    for call in calls {
        let tree = trees.entry(&call.root).or_default();
        tree.0 += call.amount(basis);
        if call.descendant() {
            tree.1 += call.amount(basis);
        }
    }
    trees
        .values()
        .filter(|(all, descendants)| *all > 0.0 && descendants / all > SUBAGENT_HEAVY_SHARE)
        .map(|(all, _)| all)
        .sum()
}

/// Usage of Sessions whose calls span at least [`LONG_RUNNING_MS`].
fn long_running(calls: &[Call], basis: InsightBasis) -> f64 {
    let mut sessions: HashMap<&str, (i64, i64, f64)> = HashMap::new();
    for call in calls {
        let span = sessions
            .entry(&call.session)
            .or_insert((i64::MAX, i64::MIN, 0.0));
        span.0 = span.0.min(call.started_ms);
        span.1 = span.1.max(call.ended_ms);
        span.2 += call.amount(basis);
    }
    sessions
        .values()
        .filter(|(first, last, _)| last.saturating_sub(*first) >= LONG_RUNNING_MS)
        .map(|(_, _, amount)| amount)
        .sum()
}

/// The group of a descendant call: `workflow`, `fork` or its agent.
fn group(call: &Call, forks: &HashSet<String>) -> (String, SubagentGroupKind) {
    if call.run.is_some() {
        ("workflow".into(), SubagentGroupKind::Workflow)
    } else if forks.contains(&call.session) {
        ("fork".into(), SubagentGroupKind::Fork)
    } else {
        (call.agent.clone(), SubagentGroupKind::Agent)
    }
}

fn subagents(
    calls: &[Call],
    forks: &HashSet<String>,
    basis: InsightBasis,
    total: f64,
) -> Vec<SubagentShare> {
    let mut groups: BTreeMap<(String, &'static str), (SubagentGroupKind, f64)> = BTreeMap::new();
    for call in calls.iter().filter(|c| c.descendant()) {
        let (name, kind) = group(call, forks);
        let key = (name, kind_key(kind));
        groups.entry(key).or_insert((kind, 0.0)).1 += call.amount(basis);
    }
    let mut out: Vec<SubagentShare> = groups
        .into_iter()
        .map(|((group, _), (kind, amount))| SubagentShare {
            group,
            kind,
            share: share(amount, total),
            amount,
        })
        .collect();
    out.sort_by(|a, b| b.amount.total_cmp(&a.amount).then(a.group.cmp(&b.group)));
    out
}

fn kind_key(kind: SubagentGroupKind) -> &'static str {
    match kind {
        SubagentGroupKind::Agent => "agent",
        SubagentGroupKind::Workflow => "workflow",
        SubagentGroupKind::Fork => "fork",
    }
}

/// The pure part: calls of the window in, the response out.
pub(super) fn insights(
    calls: &[Call],
    forks: &HashSet<String>,
    window: InsightWindow,
    until_ms: i64,
) -> UsageInsights {
    let basis = if calls.iter().all(|c| c.cost.is_some()) {
        InsightBasis::Cost
    } else {
        InsightBasis::Tokens
    };
    let total: f64 = calls.iter().map(|c| c.amount(basis)).sum();
    let priced: Vec<f64> = calls.iter().filter_map(|c| c.cost).collect();
    let large: f64 = calls
        .iter()
        .filter(|c| c.prompt_tokens > LARGE_CONTEXT_TOKENS)
        .map(|c| c.amount(basis))
        .sum();
    UsageInsights {
        window: window.label().into(),
        since_ms: until_ms.saturating_sub(window.duration_ms()),
        until_ms,
        basis,
        total,
        total_cost: (!priced.is_empty()).then(|| priced.iter().sum()),
        total_tokens: calls.iter().map(|c| c.tokens).sum(),
        steps: calls.len() as u64,
        factors: vec![
            factor(
                InsightFactorKind::SubagentHeavy,
                subagent_heavy(calls, basis),
                total,
            ),
            factor(
                InsightFactorKind::LongRunning,
                long_running(calls, basis),
                total,
            ),
            factor(InsightFactorKind::LargeContext, large, total),
        ],
        subagents: subagents(calls, forks, basis, total),
        scope: "server".into(),
        note: SCOPE_NOTE.into(),
        backfilling: false,
    }
}

fn calls(db: &Connection, since_ms: i64) -> rusqlite::Result<Vec<Call>> {
    let mut statement = db.prepare(
        "SELECT session_id, root_id, workflow_run_id, agent, started_ms, ended_ms, prompt_tokens,
            input_tokens + output_tokens + reasoning_tokens + cache_read_tokens + cache_write_tokens,
            cost, directory
         FROM usage_step WHERE started_ms >= ?1 AND failed = 0",
    )?;
    let n = |r: &rusqlite::Row<'_>, i: usize| r.get::<_, i64>(i).map(|v| v.max(0) as u64);
    statement
        .query_map([since_ms], |r| {
            Ok(Call {
                session: r.get(0)?,
                root: r.get(1)?,
                run: r.get(2)?,
                agent: r.get(3)?,
                started_ms: r.get(4)?,
                ended_ms: r.get(5)?,
                prompt_tokens: n(r, 6)?,
                tokens: n(r, 7)?,
                cost: r.get(8)?,
                directory: r.get(9)?,
            })
        })?
        .collect()
}

/// Descendant Sessions with usage in the window that were forked from their parent.
fn forks(db: &Connection, since_ms: i64) -> rusqlite::Result<HashSet<String>> {
    let mut statement = db.prepare(
        "SELECT DISTINCT u.session_id FROM usage_step u
         JOIN event e ON e.aggregate_id = u.session_id AND e.type = ?2
         WHERE u.started_ms >= ?1 AND u.session_id != u.root_id
            AND json_extract(e.data, '$.forked_from') IS NOT NULL",
    )?;
    statement
        .query_map(rusqlite::params![since_ms, CREATED], |r| r.get(0))?
        .collect()
}

type Read = (Vec<Call>, HashSet<String>, bool);

fn read(db: &Connection, since_ms: i64) -> rusqlite::Result<Read> {
    let backfilling = db.query_row("SELECT EXISTS(SELECT 1 FROM usage_backfill)", [], |r| {
        r.get(0)
    })?;
    Ok((calls(db, since_ms)?, forks(db, since_ms)?, backfilling))
}

impl Runtime {
    /// Contributing factors of the window's usage.
    pub fn usage_insights(&self, request: &InsightsRequest) -> Result<UsageInsights, RuntimeError> {
        self.usage_insights_at(request, now_ms())
    }

    /// [`Runtime::usage_insights`] as of `now_ms`.
    pub fn usage_insights_at(
        &self,
        request: &InsightsRequest,
        now_ms: i64,
    ) -> Result<UsageInsights, RuntimeError> {
        let since_ms = now_ms.saturating_sub(request.window.duration_ms());
        let (mut calls, forks, backfilling) =
            self.inner.store.read(move |db| Ok(read(db, since_ms)?))?;
        let project: Option<&Path> = request.project.as_deref();
        calls.retain(|c| in_project(&c.directory, project));
        let mut out = insights(&calls, &forks, request.window, now_ms);
        out.backfilling = backfilling;
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOUR: i64 = 3_600_000;

    fn call(session: &str, root: &str, prompt: u64, cost: Option<f64>) -> Call {
        Call {
            session: session.into(),
            root: root.into(),
            run: None,
            agent: "build".into(),
            started_ms: 0,
            ended_ms: 1_000,
            prompt_tokens: prompt,
            tokens: prompt + 100,
            cost,
            directory: "/repo".into(),
        }
    }

    fn shares(out: &UsageInsights) -> Vec<f64> {
        out.factors.iter().map(|f| f.share).collect()
    }

    #[test]
    fn thresholds_are_the_specified_constants() {
        assert_eq!(SUBAGENT_HEAVY_SHARE, 0.5);
        assert_eq!(LONG_RUNNING_MS, 28_800_000);
        assert_eq!(LARGE_CONTEXT_TOKENS, 150_000);
        assert_eq!(InsightWindow::parse("24h"), Some(InsightWindow::Day));
        assert_eq!(InsightWindow::parse("7d"), Some(InsightWindow::Week));
        for bad in ["1d", "24", "week", "", "7D"] {
            assert_eq!(InsightWindow::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn large_context_share_of_cost() {
        // 84% of the cost came from steps above 150,000 prompt tokens.
        let calls = [
            call("a", "a", 150_001, Some(0.84)),
            call("a", "a", 150_000, Some(0.16)),
        ];
        let out = insights(&calls, &HashSet::new(), InsightWindow::Day, 10 * HOUR);
        assert_eq!(out.basis, InsightBasis::Cost);
        assert!((out.factors[2].share - 0.84).abs() < 1e-9, "{out:?}");
        assert_eq!(out.factors[2].factor, InsightFactorKind::LargeContext);
        assert_eq!(out.since_ms, 10 * HOUR - 24 * HOUR);
        assert_eq!(out.note, SCOPE_NOTE);
    }

    #[test]
    fn an_unpriced_step_makes_every_share_a_share_of_tokens() {
        let calls = [
            call("a", "a", 200_000, Some(10.0)),
            call("b", "b", 1_000, None),
        ];
        let out = insights(&calls, &HashSet::new(), InsightWindow::Week, 0);
        assert_eq!(out.basis, InsightBasis::Tokens);
        assert_eq!(out.total, 201_200.0);
        assert!((out.factors[2].share - 200_100.0 / 201_200.0).abs() < 1e-9);
        assert_eq!(out.total_cost, Some(10.0));
        assert_eq!(out.window, "7d");
    }

    #[test]
    fn subagent_heavy_needs_more_than_half_of_the_tree() {
        let mut calls = vec![
            // Tree `a`: the child used 60%, so the whole tree counts.
            call("a", "a", 0, Some(0.4)),
            call("a1", "a", 0, Some(0.6)),
            // Tree `b`: exactly half is not more than half.
            call("b", "b", 0, Some(0.5)),
            call("b1", "b", 0, Some(0.5)),
        ];
        calls[3].agent = "explore".into();
        let out = insights(&calls, &HashSet::new(), InsightWindow::Day, 0);
        assert!((out.factors[0].share - 0.5).abs() < 1e-9, "{out:?}");
        let groups: Vec<(&str, f64)> = out
            .subagents
            .iter()
            .map(|s| (s.group.as_str(), s.share))
            .collect();
        assert_eq!(groups, [("build", 0.3), ("explore", 0.25)]);
    }

    #[test]
    fn long_running_sessions_span_eight_hours() {
        let mut early = call("a", "a", 0, Some(1.0));
        let mut late = call("a", "a", 0, Some(1.0));
        early.started_ms = 0;
        late.ended_ms = 8 * HOUR;
        let mut short = call("b", "b", 0, Some(2.0));
        short.ended_ms = 8 * HOUR - 1;
        let out = insights(
            &[early, late, short],
            &HashSet::new(),
            InsightWindow::Day,
            0,
        );
        assert_eq!(shares(&out), [0.0, 0.5, 0.0]);
    }

    #[test]
    fn workflow_and_fork_children_have_their_own_groups() {
        let mut run = call("w1", "a", 0, Some(1.0));
        run.run = Some("wfr_1".into());
        let fork = call("f1", "a", 0, Some(1.0));
        let agent = call("e1", "a", 0, Some(2.0));
        let root = call("a", "a", 0, Some(4.0));
        let forks = HashSet::from(["f1".to_owned()]);
        let out = insights(&[run, fork, agent, root], &forks, InsightWindow::Day, 0);
        let groups: Vec<(&str, SubagentGroupKind)> = out
            .subagents
            .iter()
            .map(|s| (s.group.as_str(), s.kind))
            .collect();
        assert_eq!(
            groups,
            [
                ("build", SubagentGroupKind::Agent),
                ("fork", SubagentGroupKind::Fork),
                ("workflow", SubagentGroupKind::Workflow),
            ]
        );
        // The workflow factor is reserved for P2 and never reported.
        assert!(
            out.factors
                .iter()
                .all(|f| f.factor != InsightFactorKind::Workflow)
        );
    }

    #[test]
    fn an_empty_window_has_zero_shares_and_no_cost() {
        let out = insights(&[], &HashSet::new(), InsightWindow::Day, 0);
        assert_eq!(out.total, 0.0);
        assert_eq!(out.total_cost, None);
        assert_eq!(shares(&out), [0.0, 0.0, 0.0]);
        assert!(out.subagents.is_empty());
    }
}

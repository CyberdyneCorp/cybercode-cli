//! Durable PR auto-fix decisions. Network and process adapters cannot bypass these gates.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub repository: String,
    pub pr: u64,
    pub session_id: String,
    pub model: String,
    pub checks: Vec<Vec<String>>,
    /// Exact CI producer identity: check:<app_id>:<name> or status:<creator_id>:<context>.
    pub required_ci: Vec<String>,
    pub max_attempts: u32,
    pub max_tokens: u64,
    pub max_seconds: u32,
}
impl Config {
    pub fn validate(&self) -> Result<(), String> {
        let parts = self.repository.split('/').collect::<Vec<_>>();
        if parts.len() != 2
            || parts.iter().any(|p| {
                p.is_empty()
                    || *p == "."
                    || *p == ".."
                    || !p
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
            })
        {
            return Err("expected GitHub owner/repository".into());
        }
        if self.pr == 0 || self.session_id.is_empty() || self.model.is_empty() {
            return Err("PR, linked Session and model are required".into());
        }
        if !(1..=10).contains(&self.max_attempts)
            || self.max_tokens == 0
            || !(1..=7200).contains(&self.max_seconds)
        {
            return Err("expected 1..10 attempts, positive tokens and 1..7200 seconds".into());
        }
        if self.checks.is_empty()
            || self.checks.len() > 16
            || self.checks.iter().any(|a| {
                a.is_empty()
                    || a.len() > 128
                    || a.iter()
                        .any(|s| s.is_empty() || s.len() > 4096 || s.contains('\0'))
            })
        {
            return Err("one to sixteen bounded verification argv arrays are required".into());
        }
        if self.required_ci.is_empty()
            || self.required_ci.len() > 100
            || self
                .required_ci
                .iter()
                .any(|s| s.len() > 512 || !(s.starts_with("check:") || s.starts_with("status:")))
        {
            return Err("one to one hundred exact CI producer IDs are required".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Enabled,
    Paused,
    Exhausted,
    Closed,
    Disabled,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    Claimed,
    Running,
    Verifying,
    PushIntent,
    AwaitingCi,
    Succeeded,
    Failed,
    Unknown,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TriggerKind {
    CheckFailure,
    Review,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Trigger {
    pub key: String,
    pub head: String,
    pub kind: TriggerKind,
    /// Untrusted, bounded feedback; never instructions granting authority.
    pub text: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CheckEvidence {
    pub argv: Vec<String>,
    pub passed: bool,
    pub output_digest: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Attempt {
    pub id: String,
    pub head: String,
    pub triggers: Vec<Trigger>,
    pub stage: Stage,
    pub session_id: Option<String>,
    pub commit: Option<String>,
    pub checks: Vec<CheckEvidence>,
    pub tokens: Option<u64>,
    /// Absent for unpriced or incompletely accounted usage.
    pub cost_usd: Option<f64>,
    pub started_ms: i64,
    pub ended_ms: Option<i64>,
    pub reason: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Subscription {
    pub id: String,
    pub directory: String,
    pub config: Config,
    pub repository_id: u64,
    pub branch: String,
    pub revision: u64,
    pub status: Status,
    pub failures: u32,
    pub seen: BTreeSet<String>,
    pub queued: Vec<Trigger>,
    pub attempts: Vec<Attempt>,
    pub next_poll_ms: i64,
    pub created_ms: i64,
    pub updated_ms: i64,
    pub reason: Option<String>,
    pub exhaustion_notice: Option<String>,
}

pub fn valid_sha(s: &str) -> bool {
    s.len() == 40 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

impl Subscription {
    pub fn new(
        id: String,
        directory: String,
        config: Config,
        repository_id: u64,
        branch: String,
        now: i64,
    ) -> Result<Self, String> {
        config.validate()?;
        if repository_id == 0
            || branch.is_empty()
            || branch.starts_with('-')
            || branch.contains(['\0', '\n', '\r'])
        {
            return Err("invalid repository identity or branch".into());
        }
        Ok(Self {
            id,
            directory,
            config,
            repository_id,
            branch,
            revision: 1,
            status: Status::Enabled,
            failures: 0,
            seen: BTreeSet::new(),
            queued: vec![],
            attempts: vec![],
            next_poll_ms: now,
            created_ms: now,
            updated_ms: now,
            reason: None,
            exhaustion_notice: None,
        })
    }
    pub fn active(&self) -> Option<&Attempt> {
        self.attempts
            .last()
            .filter(|a| !matches!(a.stage, Stage::Succeeded | Stage::Failed))
    }
    fn active_mut(&mut self, id: &str) -> Result<&mut Attempt, String> {
        self.attempts
            .last_mut()
            .filter(|a| a.id == id && !matches!(a.stage, Stage::Succeeded | Stage::Failed))
            .ok_or_else(|| "attempt changed or already settled".into())
    }
    pub fn enqueue(&mut self, events: Vec<Trigger>, head: &str) -> Result<(), String> {
        if self.status != Status::Enabled {
            return Ok(());
        }
        if !valid_sha(head) {
            return Err("invalid head SHA".into());
        }
        for event in &events {
            if !valid_sha(&event.head)
                || event.key.is_empty()
                || event.key.len() > 256
                || event.text.len() > 16384
            {
                return Err("invalid or oversized trigger".into());
            }
        }
        let fresh = events
            .into_iter()
            .filter(|e| e.head == head && !self.seen.contains(&e.key))
            .collect::<Vec<_>>();
        if self.seen.len() + fresh.len() > 4096 || self.queued.len() + fresh.len() > 100 {
            return Err("trigger history capacity reached; owner must replace subscription".into());
        }
        self.queued.retain(|e| e.head == head);
        for event in fresh {
            if self.seen.insert(event.key.clone()) {
                self.queued.push(event)
            }
        }
        Ok(())
    }
    pub fn claim(&mut self, id: String, head: &str, now: i64) -> Result<bool, String> {
        if self.status != Status::Enabled || self.active().is_some() {
            return Ok(false);
        }
        self.queued.retain(|e| e.head == head);
        if self.queued.is_empty() {
            return Ok(false);
        }
        if self.attempts.len() >= 100 {
            self.pause("attempt history capacity reached");
            return Ok(false);
        }
        let triggers = std::mem::take(&mut self.queued);
        self.attempts.push(Attempt {
            id,
            head: head.into(),
            triggers,
            stage: Stage::Claimed,
            session_id: None,
            commit: None,
            checks: vec![],
            tokens: None,
            cost_usd: None,
            started_ms: now,
            ended_ms: None,
            reason: None,
        });
        Ok(true)
    }
    pub fn running(&mut self, id: &str, session: String) -> Result<(), String> {
        if self.status != Status::Enabled {
            return Err("subscription is not enabled".into());
        }
        let a = self.active_mut(id)?;
        if a.stage != Stage::Claimed {
            return Err("attempt already dispatched".into());
        }
        a.session_id = Some(session);
        a.stage = Stage::Running;
        Ok(())
    }
    pub fn verifying(
        &mut self,
        id: &str,
        tokens: Option<u64>,
        cost: Option<f64>,
    ) -> Result<(), String> {
        let a = self.active_mut(id)?;
        if a.stage != Stage::Running {
            return Err("attempt is not running".into());
        }
        if cost.is_some_and(|v| !v.is_finite() || v < 0.0) {
            return Err("invalid usage".into());
        }
        a.tokens = tokens;
        a.cost_usd = cost;
        a.stage = Stage::Verifying;
        Ok(())
    }
    pub fn push_intent(
        &mut self,
        id: &str,
        commit: String,
        checks: Vec<CheckEvidence>,
    ) -> Result<(), String> {
        if self.status != Status::Enabled {
            return Err("subscription is not enabled".into());
        }
        if !valid_sha(&commit) {
            return Err("invalid commit SHA".into());
        }
        let required = self.config.checks.clone();
        let a = self.active_mut(id)?;
        if a.stage != Stage::Verifying
            || commit == a.head
            || checks.len() != required.len()
            || checks
                .iter()
                .zip(&required)
                .any(|(e, argv)| !e.passed || &e.argv != argv || e.output_digest.len() != 64)
        {
            return Err("verified nonempty patch and exact pinned checks required".into());
        }
        a.commit = Some(commit);
        a.checks = checks;
        a.stage = Stage::PushIntent;
        Ok(())
    }
    pub fn pushed(&mut self, id: &str) -> Result<(), String> {
        let a = self.active_mut(id)?;
        if a.stage != Stage::PushIntent && a.stage != Stage::Unknown {
            return Err("no pending push intent".into());
        }
        if a.commit.is_none() {
            return Err("unknown attempt has no push intent".into());
        }
        a.stage = Stage::AwaitingCi;
        Ok(())
    }
    pub fn ci(&mut self, id: &str, head: &str, success: bool, now: i64) -> Result<(), String> {
        let a = self.active_mut(id)?;
        if a.stage != Stage::AwaitingCi || a.commit.as_deref() != Some(head) {
            return Err("CI does not match pending fix commit".into());
        }
        if success {
            a.stage = Stage::Succeeded;
            a.ended_ms = Some(now);
            self.failures = 0;
            Ok(())
        } else {
            self.failed(id, "fix commit failed CI".into(), now)
        }
    }
    pub fn failed(&mut self, id: &str, reason: String, now: i64) -> Result<(), String> {
        let a = self.active_mut(id)?;
        if matches!(a.stage, Stage::PushIntent | Stage::Unknown) {
            return Err("uncertain external outcome must be reconciled".into());
        }
        a.stage = Stage::Failed;
        a.ended_ms = Some(now);
        a.reason = Some(reason);
        self.failures += 1;
        if self.failures >= self.config.max_attempts {
            self.status = Status::Exhausted;
            self.reason = Some("consecutive attempt limit reached".into())
        }
        Ok(())
    }
    pub fn unknown(&mut self, id: &str, reason: &str) -> Result<(), String> {
        self.active_mut(id)?.stage = Stage::Unknown;
        self.pause(reason);
        Ok(())
    }
    pub fn pause(&mut self, reason: &str) {
        self.status = Status::Paused;
        self.reason = Some(reason.chars().take(500).collect())
    }
    pub fn control(&mut self, action: &str) -> Result<(), String> {
        match action {
            "pause" => self.pause("paused by owner"),
            "disable" => {
                self.status = Status::Disabled;
                self.reason = Some("disabled by owner".into())
            }
            "resume" => {
                if self.status != Status::Paused
                    || self.active().is_some()
                    || self.failures >= self.config.max_attempts
                {
                    return Err("cannot resume unresolved, exhausted or closed subscription".into());
                }
                self.status = Status::Enabled;
                self.reason = None;
            }
            _ => return Err("expected pause, resume or disable".into()),
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;

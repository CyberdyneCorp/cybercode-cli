//! `workflows`: Workflow runtime settings (`workflows` → Sandboxed QuickJS runtime,
//! Concurrency limit, Model tiers and mixing, Automatic orchestration).

use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::{Map, Value};

const KEY: &str = "workflows";
/// Model roles a tier may name (`provider-catalog` → Model roles).
const ROLES: &[&str] = &[
    "default",
    "small",
    "advisor",
    "evaluator",
    "compaction",
    "title",
];

/// What a tier name resolves to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", content = "target", rename_all = "snake_case")]
pub enum TierTarget {
    /// A `provider/model[#variant]` reference.
    Model(String),
    /// A model role, resolved through `model_roles` when the call is made.
    Role(String),
}

/// The resolved `workflows` key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WorkflowSettings {
    /// Workflow agents running at once across all runs of a server.
    pub max_concurrent: u32,
    /// The default share of one run (`Fair agent scheduling`); unset for the whole limit.
    pub max_concurrent_per_run: Option<u32>,
    /// Runs holding an execution attempt at once on a server (`Run queue`); later starts
    /// and resumes wait `queued`.
    pub max_running_runs: u32,
    /// Resume the runs a restart interrupted once recovery ends (`Recovery of several
    /// runs`); otherwise they stay `paused` until resumed.
    pub resume_after_restart: bool,
    /// QuickJS heap limit per run.
    pub max_memory_mb: u32,
    /// Synchronous script CPU per run, excluding time awaiting host calls.
    pub script_cpu_seconds: u32,
    /// Tier name → target; `fast`, `smart` and `verify` always exist.
    pub tiers: BTreeMap<String, TierTarget>,
    /// `off`, `suggest` or `on` (`Automatic orchestration`, a later milestone).
    pub auto: String,
    pub auto_threshold_files: u32,
    pub size_guideline: u32,
}

impl Default for WorkflowSettings {
    fn default() -> Self {
        let tiers = [
            ("fast", "small"),
            ("smart", "default"),
            ("verify", "advisor"),
        ]
        .into_iter()
        .map(|(tier, role)| (tier.to_string(), TierTarget::Role(role.into())))
        .collect();
        Self {
            max_concurrent: 16,
            max_concurrent_per_run: None,
            max_running_runs: 8,
            resume_after_restart: false,
            max_memory_mb: 256,
            script_cpu_seconds: 60,
            tiers,
            auto: "suggest".into(),
            auto_threshold_files: 10,
            size_guideline: 10,
        }
    }
}

/// `(key, minimum, maximum)` of every integer setting.
const RANGES: [(&str, u32, u32); 7] = [
    ("max_concurrent", 1, 256),
    ("max_concurrent_per_run", 1, 256),
    ("max_running_runs", 1, 64),
    ("max_memory_mb", 16, 4096),
    ("script_cpu_seconds", 1, 3600),
    ("auto_threshold_files", 1, 100_000),
    ("size_guideline", 1, 1000),
];

impl WorkflowSettings {
    /// Validate `workflows` in a configuration value and apply its defaults.
    pub fn from_config(config: &Value) -> Result<Self, String> {
        let mut settings = Self::default();
        let Some(value) = config.get(KEY) else {
            return Ok(settings);
        };
        let map = value
            .as_object()
            .ok_or_else(|| format!("{KEY}: expected an object, got {value}"))?;
        for (key, value) in map {
            match key.as_str() {
                "tiers" => tiers(value, &mut settings.tiers)?,
                "auto" => settings.auto = auto(value)?,
                "resume_after_restart" => {
                    settings.resume_after_restart = value.as_bool().ok_or_else(|| {
                        format!("{KEY}.resume_after_restart: expected true or false, got {value}")
                    })?;
                }
                "max_concurrent_per_run" => {
                    settings.max_concurrent_per_run = (!value.is_null())
                        .then(|| integer(key, value))
                        .transpose()?;
                }
                _ => *settings.slot(key)? = integer(key, value)?,
            }
        }
        Ok(settings)
    }

    /// `max_concurrent` reduced to the number of CPU cores when lower.
    pub fn effective_max_concurrent(&self, cores: usize) -> u32 {
        let cores = u32::try_from(cores.max(1)).unwrap_or(u32::MAX);
        self.max_concurrent.min(cores)
    }

    fn slot(&mut self, key: &str) -> Result<&mut u32, String> {
        Ok(match key {
            "max_concurrent" => &mut self.max_concurrent,
            "max_running_runs" => &mut self.max_running_runs,
            "max_memory_mb" => &mut self.max_memory_mb,
            "script_cpu_seconds" => &mut self.script_cpu_seconds,
            "auto_threshold_files" => &mut self.auto_threshold_files,
            "size_guideline" => &mut self.size_guideline,
            other => return Err(format!("{KEY}.{other}: unknown key")),
        })
    }
}

fn integer(key: &str, value: &Value) -> Result<u32, String> {
    let (_, min, max) = RANGES
        .iter()
        .find(|(name, _, _)| *name == key)
        .copied()
        .unwrap_or((key, 0, 0));
    value
        .as_u64()
        .and_then(|n| u32::try_from(n).ok())
        .filter(|n| (min..=max).contains(n))
        .ok_or_else(|| format!("{KEY}.{key}: expected an integer from {min} to {max}, got {value}"))
}

fn auto(value: &Value) -> Result<String, String> {
    match value.as_str() {
        Some(mode @ ("off" | "suggest" | "on")) => Ok(mode.into()),
        _ => Err(format!(
            "{KEY}.auto: expected one of off, suggest, on, got {value}"
        )),
    }
}

/// Tier names are `^[a-z][a-z0-9_-]{0,31}$`, so they never look like a model reference.
fn valid_tier_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    bytes.next().is_some_and(|b| b.is_ascii_lowercase())
        && name.len() <= 32
        && bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
}

fn tiers(value: &Value, tiers: &mut BTreeMap<String, TierTarget>) -> Result<(), String> {
    let map: &Map<String, Value> = value
        .as_object()
        .ok_or_else(|| format!("{KEY}.tiers: expected an object, got {value}"))?;
    for (name, target) in map {
        if !valid_tier_name(name) {
            return Err(format!(
                "{KEY}.tiers.{name}: tier names must match ^[a-z][a-z0-9_-]{{0,31}}$"
            ));
        }
        tiers.insert(name.clone(), tier_target(name, target)?);
    }
    Ok(())
}

fn tier_target(name: &str, value: &Value) -> Result<TierTarget, String> {
    let invalid = || {
        format!(
            "{KEY}.tiers.{name}: expected provider/model[#variant] or role:<{}>, got {value}",
            ROLES.join("|")
        )
    };
    let text = value.as_str().ok_or_else(invalid)?;
    if let Some(role) = text.strip_prefix("role:") {
        return ROLES
            .contains(&role)
            .then(|| TierTarget::Role(role.into()))
            .ok_or_else(invalid);
    }
    text.split_once('/')
        .filter(|(provider, model)| !provider.is_empty() && !model.is_empty())
        .map(|_| TierTarget::Model(text.into()))
        .ok_or_else(invalid)
}

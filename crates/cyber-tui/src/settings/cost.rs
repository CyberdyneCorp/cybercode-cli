//! The Usage tab's data (`tui` → Settings Usage tab), moved from the `/cost` overlay with
//! its validation intact: a snapshot retains missing attribution instead of converting it
//! to zero, and a report for another Session or with inconsistent totals is refused.

use serde::Deserialize;
use serde_json::Value;

const LABELS: [&str; 5] = ["Input", "Output", "Reasoning", "Cache read", "Cache write"];

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Cost {
    own: Option<[u64; 5]>,
    children: Option<[u64; 5]>,
    own_cost: Option<f64>,
    children_cost: Option<f64>,
    unpriced: Option<u128>,
    classes_complete: bool,
    cost_complete: bool,
    /// The server's breakdown; `None` from a server without one.
    pub breakdown: Option<super::usage::Breakdown>,
}

#[derive(Debug, Deserialize)]
struct Amount {
    tokens: Tokens,
    total_tokens: u64,
    cost: f64,
    unpriced_steps: u64,
    usage_complete: bool,
    token_classes_complete: bool,
}

#[derive(Debug, Deserialize)]
struct Tokens {
    input: u64,
    output: u64,
    reasoning: u64,
    cache_read: u64,
    cache_write: u64,
}
impl Tokens {
    fn values(&self) -> [u64; 5] {
        [
            self.input,
            self.output,
            self.reasoning,
            self.cache_read,
            self.cache_write,
        ]
    }
}
impl Amount {
    fn validate(&self) -> Result<(), String> {
        let known: u128 = self.tokens.values().into_iter().map(u128::from).sum();
        if !self.cost.is_finite()
            || self.cost < 0.0
            || known > u128::from(self.total_tokens)
            || (self.token_classes_complete && known != u128::from(self.total_tokens))
        {
            return Err("Invalid usage amount".into());
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
struct Report {
    scope: String,
    id: String,
    own: Amount,
    descendants: Amount,
    total: Amount,
}
fn same_cost(a: f64, b: f64) -> bool {
    a.is_finite()
        && b.is_finite()
        && (a - b).abs() <= 4.0 * f64::EPSILON * a.abs().max(b.abs()).max(f64::MIN_POSITIVE)
}

impl Report {
    fn validate(&self, id: &str) -> Result<(), String> {
        if self.scope != "session" || self.id != id {
            return Err("Usage snapshot belongs to a different Session".into());
        }
        self.own.validate()?;
        self.descendants.validate()?;
        self.total.validate()?;
        let a = &self.own;
        let b = &self.descendants;
        let sum_matches = a
            .tokens
            .values()
            .into_iter()
            .zip(b.tokens.values())
            .zip(self.total.tokens.values())
            .all(|((a, b), total)| u128::from(a) + u128::from(b) == u128::from(total));
        if !sum_matches
            || u128::from(a.total_tokens) + u128::from(b.total_tokens)
                != u128::from(self.total.total_tokens)
            || u128::from(a.unpriced_steps) + u128::from(b.unpriced_steps)
                != u128::from(self.total.unpriced_steps)
            || !same_cost(a.cost + b.cost, self.total.cost)
            || (a.usage_complete && b.usage_complete) != self.total.usage_complete
            || (a.token_classes_complete && b.token_classes_complete)
                != self.total.token_classes_complete
        {
            return Err("Inconsistent usage totals".into());
        }
        Ok(())
    }
}

impl Cost {
    pub fn parse_report(value: &Value, id: &str) -> Result<Self, String> {
        let report: Report = serde_json::from_value(value.clone())
            .map_err(|_| "Malformed usage snapshot".to_string())?;
        report.validate(id)?;
        let breakdown = value
            .get("by_model")
            .map(|_| super::usage::Breakdown::parse(value))
            .transpose()?;
        Ok(Self {
            breakdown,
            own: Some(report.own.tokens.values()),
            children: Some(report.descendants.tokens.values()),
            own_cost: Some(report.own.cost),
            children_cost: Some(report.descendants.cost),
            unpriced: Some(u128::from(report.total.unpriced_steps)),
            classes_complete: report.total.token_classes_complete,
            cost_complete: report.total.usage_complete,
        })
    }

    /// The token table and cache hit rate.
    pub fn body(&self) -> Vec<String> {
        let mut lines = vec![format!(
            "{:<14}{:>14}{:>14}{:>16}",
            "Tokens", "Own", "Descendants", "Total (known)"
        )];
        for (index, label) in LABELS.iter().enumerate() {
            let own = self.own.map(|v| v[index]);
            let children = self.children.map(|v| v[index]);
            let total = own
                .zip(children)
                .map(|(a, b)| u128::from(a) + u128::from(b));
            lines.push(format!(
                "{label:<14}{:>14}{:>14}{:>16}",
                own.map_or("unknown".into(), |v| v.to_string()),
                children.map_or("unknown".into(), |v| v.to_string()),
                total.map_or("unknown".into(), |v| v.to_string())
            ));
        }
        lines.push(String::new());
        lines.push(format!("Cache hit rate: {}", self.cache_rate()));
        if !self.classes_complete || !self.cost_complete {
            lines.push("Attribution incomplete; known amounts are lower bounds.".into());
        }
        lines
    }

    /// `$1.23`, `—` when every call was unpriced, or `≥ $1.23 · N unpriced calls` when
    /// some were or attribution is incomplete.
    pub fn total_cost(&self) -> String {
        let Some((own, children)) = self.own_cost.zip(self.children_cost) else {
            return "unknown (missing attribution)".into();
        };
        let total = own + children;
        let unpriced = self.unpriced.unwrap_or(0);
        let priced_models = self
            .breakdown
            .as_ref()
            .map(|b| b.models.iter().any(|m| m.cost.is_some()));
        if unpriced > 0
            && (priced_models == Some(false) || (priced_models.is_none() && total == 0.0))
        {
            return "—".into();
        }
        if self.cost_complete && unpriced == 0 {
            return super::usage::money(total);
        }
        let mut text = format!("≥ {}", super::usage::money(total));
        if unpriced > 0 {
            text.push_str(&format!(" · {unpriced} unpriced calls"));
        }
        text
    }

    fn cache_rate(&self) -> String {
        if !self.classes_complete {
            return "unknown (incomplete token classes)".into();
        }
        let (Some(own), Some(children)) = (self.own, self.children) else {
            return "unknown".into();
        };
        let input = u128::from(own[0]) + u128::from(children[0]);
        let read = u128::from(own[3]) + u128::from(children[3]);
        let write = u128::from(own[4]) + u128::from(children[4]);
        let prompt = input + read + write;
        if prompt == 0 {
            return "— (no prompt tokens)".into();
        }
        format!(
            "{:.1}% (cache read / input + cache read + cache write)",
            100.0 * read as f64 / prompt as f64
        )
    }
}

#[derive(Debug, Default)]
pub struct CostView {
    pub generation: u64,
    session_id: String,
    snapshot: Cost,
    loading: bool,
    error: Option<String>,
}
impl CostView {
    pub fn invalidate(&mut self) {
        self.session_id.clear();
        self.snapshot = Cost::default();
        self.loading = false;
        self.error = None;
    }
    pub fn start(&mut self, id: &str) -> u64 {
        if self.session_id != id {
            self.snapshot = Cost::default();
        }
        self.session_id = id.into();
        self.generation = self
            .generation
            .checked_add(1)
            .expect("cost request generation exhausted");
        self.loading = true;
        self.error = None;
        self.generation
    }
    pub fn apply(&mut self, id: &str, generation: u64, result: Result<Cost, String>) {
        if self.session_id != id || self.generation != generation {
            return;
        }
        self.loading = false;
        match result {
            Ok(snapshot) => {
                self.snapshot = snapshot;
                self.error = None;
            }
            Err(error) => self.error = Some(error),
        }
    }
    /// The last snapshot, or `None` before the first one arrives.
    pub fn snapshot(&self) -> Option<&Cost> {
        (self.snapshot != Cost::default()).then_some(&self.snapshot)
    }

    /// What differs between test runs (durations, reset times) is fixed through this.
    #[cfg(test)]
    pub fn snapshot_mut(&mut self) -> Option<&mut Cost> {
        (self.snapshot != Cost::default()).then_some(&mut self.snapshot)
    }

    /// The last refresh's error, shown below the snapshot it left in place.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }
}

use super::invalid;
use serde::{Deserialize, Serialize};

/// Provider-neutral token classes; input/output exclude cache/reasoning respectively.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TokenAmounts {
    pub input: u64,
    pub output: u64,
    pub reasoning: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

impl TokenAmounts {
    pub fn total(&self) -> Result<u64, LedgerError> {
        self.classes().into_iter().try_fold(0u64, |sum, n| {
            sum.checked_add(n)
                .ok_or_else(|| invalid("token total overflow"))
        })
    }

    pub(super) fn classes(&self) -> [u64; 5] {
        [
            self.input,
            self.output,
            self.reasoning,
            self.cache_read,
            self.cache_write,
        ]
    }
}

/// Class maxima may overlap: `total_tokens` bounds their true combined consumption.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Estimate {
    pub tokens: TokenAmounts,
    pub total_tokens: u64,
    #[serde(default)]
    pub turns: u64,
    /// Conservative integer micro-USD; `None` is unpriced, never free.
    pub cost_microusd: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub request_id: String,
    pub session_id: String,
    pub provider: String,
    pub model: String,
    pub purpose: String,
    #[serde(default)]
    pub call_id: Option<String>,
    #[serde(default)]
    pub bound_source: Option<String>,
    #[serde(default)]
    pub normalized_sha256: Option<String>,
    /// Absolute local wall deadline captured atomically with admission.
    #[serde(default)]
    pub deadline_ms: Option<i64>,
    pub estimate: Estimate,
}

/// Resolve recorded spend and these limits in the same transaction as `reserve`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scope {
    pub id: String,
    pub max_tokens: Option<u64>,
    #[serde(default)]
    pub max_turns: Option<u64>,
    pub max_cost_microusd: Option<u64>,
    pub spent_tokens: u64,
    #[serde(default)]
    pub spent_turns: u64,
    pub spent_cost_microusd: u64,
    /// The caller could not establish recorded spending relevant to these active caps.
    pub uncertain: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    pub tokens: Option<TokenAmounts>,
    #[serde(default)]
    pub turns: u64,
    pub cost_microusd: Option<u64>,
    pub complete: bool,
}

impl Receipt {
    /// New confirmed charges since the prior cumulative observation. Confirmed amounts
    /// never go backwards; an incomplete observation may omit previously known amounts.
    pub fn delta_since(&self, previous: Option<&Self>) -> Result<Self, LedgerError> {
        let old = previous.cloned().unwrap_or_default();
        let tokens = match (self.tokens, old.tokens) {
            (Some(now), Some(before)) => Some(TokenAmounts {
                input: difference(now.input, before.input)?,
                output: difference(now.output, before.output)?,
                reasoning: difference(now.reasoning, before.reasoning)?,
                cache_read: difference(now.cache_read, before.cache_read)?,
                cache_write: difference(now.cache_write, before.cache_write)?,
            }),
            (Some(now), None) => Some(now),
            (None, _) => None,
        };
        let cost_microusd = match (self.cost_microusd, old.cost_microusd) {
            (Some(now), Some(before)) => Some(difference(now, before)?),
            (Some(now), None) => Some(now),
            (None, _) => None,
        };
        Ok(Self {
            tokens,
            turns: difference(self.turns, old.turns)?,
            cost_microusd,
            complete: self.complete,
        })
    }
}

fn difference(now: u64, before: u64) -> Result<u64, LedgerError> {
    now.checked_sub(before)
        .ok_or_else(|| invalid("confirmed receipt amount decreased"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Reserved,
    Uncertain,
    Settled,
    Released,
}

impl State {
    pub(super) fn parse(value: &str) -> Result<Self, LedgerError> {
        match value {
            "reserved" => Ok(Self::Reserved),
            "uncertain" => Ok(Self::Uncertain),
            "settled" => Ok(Self::Settled),
            "released" => Ok(Self::Released),
            _ => Err(invalid("unknown reservation state")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    pub request: Request,
    pub state: State,
    pub revision: u64,
    pub created_ms: i64,
    pub dispatched_ms: Option<i64>,
    pub updated_ms: i64,
    pub settled_ms: Option<i64>,
    pub released_ms: Option<i64>,
    pub actual: Option<Receipt>,
    pub receipt_event_id: Option<String>,
    /// A provider can exceed the estimate; actual usage is retained without truncation.
    pub overrun_tokens: u64,
    pub overrun_cost_microusd: Option<u64>,
    pub scopes: Vec<Scope>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Admission {
    /// Only a newly admitted request can acquire dispatch authority.
    pub new: bool,
    pub record: Record,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settlement {
    pub record: Record,
    pub delta: Receipt,
    pub applied: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reconciled {
    pub record: Record,
    /// Only settlement has a financial delta; release does not erase confirmed usage.
    pub delta: Option<Receipt>,
    pub applied: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ReconcileAction {
    Release,
    Settle {
        receipt_event_id: String,
        actual: Receipt,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reconciliation {
    pub operation_id: String,
    pub expected_revision: u64,
    pub owner: String,
    pub reason: String,
    pub action: ReconcileAction,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Audit {
    pub request_id: String,
    #[serde(flatten)]
    pub reconciliation: Reconciliation,
    pub reconciled_ms: i64,
}

#[derive(Debug, thiserror::Error)]
pub enum LedgerError {
    #[error("reservation database error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("invalid reservation: {0}")]
    Invalid(String),
    #[error("reservation {0} not found")]
    NotFound(String),
    #[error("reservation {0} identity was reused with different data")]
    Conflict(String),
    #[error("scope {scope} exceeds {limit}")]
    Exceeded { scope: String, limit: String },
    #[error("BudgetUnpricedError: USD reservation for scope {0} has no price bound")]
    Unpriced(String),
    #[error("scope {0} has incomplete recorded spending")]
    Uncertain(String),
    #[error("reservation {0} cannot make this transition")]
    Transition(String),
    #[error("reservation {request_id} revision changed: expected {expected}, actual {actual}")]
    Stale {
        request_id: String,
        expected: u64,
        actual: u64,
    },
}

impl From<LedgerError> for crate::StoreError {
    fn from(error: LedgerError) -> Self {
        if let LedgerError::Sqlite(error) = error {
            return error.into();
        }
        let (code, mut detail) = match &error {
            LedgerError::Exceeded { scope, limit } => {
                ("exceeded", serde_json::json!({"scope":scope,"limit":limit}))
            }
            LedgerError::Unpriced(scope) => ("unpriced", serde_json::json!({"scope":scope})),
            LedgerError::Uncertain(scope) => ("uncertain", serde_json::json!({"scope":scope})),
            LedgerError::Stale {
                request_id,
                expected,
                actual,
            } => (
                "stale",
                serde_json::json!({"request_id":request_id,"expected":expected,"actual":actual}),
            ),
            LedgerError::NotFound(request_id) => {
                ("not_found", serde_json::json!({"request_id":request_id}))
            }
            LedgerError::Transition(request_id) => {
                ("transition", serde_json::json!({"request_id":request_id}))
            }
            LedgerError::Conflict(identity) => {
                ("conflict", serde_json::json!({"identity":identity}))
            }
            LedgerError::Invalid(_) | LedgerError::Sqlite(_) => ("invalid", serde_json::json!({})),
        };
        detail["message"] = error.to_string().into();
        Self::Projector {
            kind: format!("budget.reservation.{code}"),
            reason: detail.to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cursor {
    pub created_ms: i64,
    pub request_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Filter {
    pub session_id: Option<String>,
    pub scope_id: Option<String>,
    #[serde(default)]
    pub open_only: bool,
    pub after: Option<Cursor>,
    pub limit: u32,
}

impl Default for Filter {
    fn default() -> Self {
        Self {
            session_id: None,
            scope_id: None,
            open_only: true,
            after: None,
            limit: 100,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Page {
    pub records: Vec<Record>,
    pub next: Option<Cursor>,
}

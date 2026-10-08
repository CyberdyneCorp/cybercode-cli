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

#[derive(Debug, Clone, PartialEq, Serialize, schemars::JsonSchema)]
pub struct UsageReport {
    pub scope: String,
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
        usage_complete: true,
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
        usage_complete: row.get::<_, bool>(10)?,
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
        scope: "session".into(),
        id: id.into(),
        own,
        descendants,
        total,
    })
}

impl Runtime {
    pub fn session_usage(&self, id: &str) -> Result<UsageReport, RuntimeError> {
        let source = id.to_owned();
        self.inner.store.read(move |db| {
            Ok(db.query_row("SELECT cost,input_tokens,output_tokens,reasoning_tokens,cache_read_tokens,cache_write_tokens,unpriced_steps,
                children_cost,children_tokens,children_unpriced_steps,children_usage_complete,children_token_classes,children_token_classes_complete
                FROM session WHERE id=?1", [&source], |row| read(row,&source)).optional()?)
        })?.ok_or_else(|| RuntimeError::SessionNotFound(id.into()))
    }
}

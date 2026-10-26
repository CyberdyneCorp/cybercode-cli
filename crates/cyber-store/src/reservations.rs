//! Durable provider-request admission. Call these functions inside the store's writer
//! transaction; settlement must share the transaction that records native usage.

mod model;
mod query;
mod write;

use serde::Serialize;

pub use model::*;
pub use query::{audit, get, list};
pub use write::{mark_dispatched, reconcile, release_before_dispatch, reserve, settle};

fn invalid(message: impl Into<String>) -> LedgerError {
    LedgerError::Invalid(message.into())
}

fn sql(value: u64) -> Result<i64, LedgerError> {
    i64::try_from(value).map_err(|_| invalid("integer exceeds durable ledger range"))
}

fn text(value: &str, field: &str, max: usize) -> Result<(), LedgerError> {
    if value.is_empty() || value.len() > max || value.chars().any(char::is_control) {
        return Err(invalid(format!(
            "{field} must be bounded, nonempty printable text"
        )));
    }
    Ok(())
}

fn encode<T: Serialize>(value: &T) -> Result<String, LedgerError> {
    serde_json::to_string(value).map_err(|error| invalid(error.to_string()))
}

fn decode<T: serde::de::DeserializeOwned>(value: &str) -> Result<T, LedgerError> {
    serde_json::from_str(value).map_err(|error| invalid(error.to_string()))
}

fn integer(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<u64> {
    let value: i64 = row.get(index)?;
    value.try_into().map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            index,
            rusqlite::types::Type::Integer,
            Box::new(error),
        )
    })
}

fn optional_integer(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<Option<u64>> {
    let value: Option<i64> = row.get(index)?;
    value
        .map(|n| {
            n.try_into().map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    index,
                    rusqlite::types::Type::Integer,
                    Box::new(error),
                )
            })
        })
        .transpose()
}

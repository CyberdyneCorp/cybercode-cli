//! Rendering helpers.

use serde::Serialize;

use crate::cli::Format;
use crate::error::CliError;

pub fn json<T: Serialize>(value: &T) -> Result<(), CliError> {
    let text = serde_json::to_string_pretty(value).map_err(|e| CliError::runtime(e.to_string()))?;
    println!("{text}");
    Ok(())
}

/// Print `key  value` rows aligned on the key column.
pub fn rows(rows: &[(&str, String)]) {
    let width = rows.iter().map(|(k, _)| k.len()).max().unwrap_or(0);
    for (key, value) in rows {
        println!("{key:width$}  {value}");
    }
}

pub fn is_json(format: Option<Format>) -> bool {
    format == Some(Format::Json)
}

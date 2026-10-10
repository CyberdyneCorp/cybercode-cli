//! Owned blocking discovery from fresh resolved configuration.
use cyber_core::{
    config::FormatterSettings,
    env::ProcessEnv,
    intelligence::{ExecutableSearch, detect_formatters},
};
use cyber_server::http::{ApiError, FormatterStatus};
use cyber_tools::ConfigFn;
use std::{path::PathBuf, sync::Arc};

pub(super) async fn formatters(
    config: Arc<ConfigFn>,
    cache: PathBuf,
    directory: PathBuf,
) -> Result<Vec<FormatterStatus>, ApiError> {
    tokio::task::spawn_blocking(move || {
        let (value, _) = config(&directory)
            .map_err(|_| ApiError::invalid("Formatter status configuration is unavailable"))?;
        let settings = FormatterSettings::from_config(&value)
            .map_err(|_| ApiError::invalid("Formatter status configuration is unavailable"))?;
        let search = ExecutableSearch::new(&directory, &cache, &ProcessEnv);
        Ok(detect_formatters(&settings, &search)
            .into_iter()
            .map(FormatterStatus::from)
            .collect())
    })
    .await
    .map_err(|_| {
        ApiError::new(
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            "InternalError",
            "Formatter status discovery could not complete",
        )
    })?
}

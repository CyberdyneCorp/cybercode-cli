//! Plugin review for a Location (`plugins-marketplace` → Plugin CLI, `/plugins`).
use super::{ApiError, AppState};
use axum::{Json, Router, extract::State, routing::get};

pub fn routes() -> Router<AppState> {
    Router::new().route("/plugins", get(list))
}

async fn list(
    State(state): State<AppState>,
    parts: axum::http::request::Parts,
) -> Result<Json<super::envelope::Located<cyber_core::plugins::PluginList>>, ApiError> {
    let directory = super::envelope::location(&parts, &state.options.default_directory)?;
    Ok(Json(super::envelope::Located {
        data: state.services.plugins(&directory)?,
        location: super::envelope::LocationInfo::of(&directory),
    }))
}

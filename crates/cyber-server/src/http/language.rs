//! Language routes (`localization` → Language selection, `server-api` → Language
//! routes): the Location's `language`, and setting the user's global default.

use axum::extract::State;
use axum::http::request::Parts;
use axum::{Json, Router, routing::get};
use cyber_core::locale::LanguageSetting;
use schemars::JsonSchema;
use serde::Deserialize;

use super::{
    ApiError, AppState,
    envelope::{Located, LocationInfo, location},
};

/// `{ language: "auto" | "en" | "pt-BR" | "es" | "fr" }`, matched ignoring case.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LanguageBody {
    pub language: String,
}

pub(super) fn routes() -> Router<AppState> {
    Router::new().route("/config/language", get(show).put(set_default))
}

async fn show(
    State(state): State<AppState>,
    parts: Parts,
) -> Result<Json<Located<LanguageSetting>>, ApiError> {
    let directory = location(&parts, &state.options.default_directory)?;
    Ok(Json(Located {
        location: LocationInfo::of(&directory),
        data: state.services.language(&directory)?,
    }))
}

/// Write `language` to the user's global configuration; a project layer that sets its
/// own still wins at its Location, which the returned setting shows.
async fn set_default(
    State(state): State<AppState>,
    parts: Parts,
    Json(body): Json<LanguageBody>,
) -> Result<Json<Located<LanguageSetting>>, ApiError> {
    let directory = location(&parts, &state.options.default_directory)?;
    let language = cyber_core::locale::canonical(&body.language).ok_or_else(|| {
        ApiError::invalid(format!(
            "language: expected auto, en, pt-BR, es or fr, got \"{}\"",
            body.language
        ))
    })?;
    Ok(Json(Located {
        location: LocationInfo::of(&directory),
        data: state.services.set_language(&directory, &language)?,
    }))
}

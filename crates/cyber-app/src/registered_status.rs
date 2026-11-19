//! Read-only observation of an existing registered server, without startup or database access.
use cyber_core::paths::Paths;
use cyber_server::http::{LspStatus, RepoMapIndex, RepoMapRequest, RepoMapView};
use serde_json::Value;
use std::{
    path::{Component, Path},
    time::Duration,
};

const MAX_RESPONSE: usize = 1024 * 1024;
/// A map waits up to 10 s for a running build.
const MAP_TIMEOUT: Duration = Duration::from_secs(30);

/// One authenticated call to the registered server's `path` for `location`: its
/// response body and the registration it was made against, or `None` without a server.
async fn registered_call(
    paths: &Paths,
    location: &Path,
    method: reqwest::Method,
    path: &str,
    query: &[(&str, String)],
    timeout: Duration,
) -> Result<Option<(Value, crate::Registration)>, String> {
    let Some(registration) = crate::read_registration(paths) else {
        if crate::registration_path(paths).exists() {
            return Err("Existing server registration is invalid".into());
        }
        return Ok(None);
    };
    let location = location
        .canonicalize()
        .map_err(|_| "Status Location is unavailable")?;
    let password = std::fs::read_to_string(paths.state.join("password"))
        .map_err(|_| "Cannot read existing server credentials")?;
    if password.trim().is_empty() {
        return Err("Existing server credentials are empty".into());
    }
    let builder = reqwest::Client::builder()
        .timeout(timeout)
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none());
    let (builder, url) = if registration.url.is_empty() {
        #[cfg(unix)]
        {
            let socket = registration
                .socket
                .as_ref()
                .ok_or("Existing server has no status listener")?;
            (
                builder.unix_socket(socket.as_str()),
                "http://localhost".to_owned(),
            )
        }
        #[cfg(not(unix))]
        return Err("Existing server has no TCP status listener".into());
    } else {
        (builder, registration.url.clone())
    };
    let mut url = reqwest::Url::parse(&url).map_err(|_| "Existing server listener is invalid")?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
        || url.host_str().is_none()
    {
        return Err("Existing server listener is invalid".into());
    }
    url.set_path(path);
    let mut pairs = vec![(
        "location[directory]",
        location.to_string_lossy().into_owned(),
    )];
    pairs.extend(query.iter().cloned());
    let mut response = builder
        .build()
        .map_err(|_| "Cannot create status transport")?
        .request(method, url)
        .basic_auth("cyber", Some(password.trim()))
        .query(&pairs)
        .send()
        .await
        .map_err(|_| "Existing server status is unavailable")?;
    if !response.status().is_success() {
        return Err(format!(
            "Existing server status refused ({})",
            response.status().as_u16()
        ));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "Existing server status transport failed")?
    {
        if bytes.len().saturating_add(chunk.len()) > MAX_RESPONSE {
            return Err("Existing server status exceeds the response budget".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    let body: Value =
        serde_json::from_slice(&bytes).map_err(|_| "Existing server status is malformed")?;
    if body["location"]["directory"].as_str() != location.to_str() {
        return Err("Existing server status returned another Location".into());
    }
    Ok(Some((body, registration)))
}

pub async fn registered_lsp_status(
    paths: &Paths,
    location: &Path,
) -> Result<Option<Vec<LspStatus>>, String> {
    let Some((body, registration)) = registered_call(
        paths,
        location,
        reqwest::Method::GET,
        "/api/v1/lsp",
        &[],
        Duration::from_secs(5),
    )
    .await?
    else {
        return Ok(None);
    };
    let location = location
        .canonicalize()
        .map_err(|_| "Status Location is unavailable")?;
    let statuses: Vec<LspStatus> = serde_json::from_value(body["data"].clone())
        .map_err(|_| "Existing server status entries are invalid")?;
    if statuses.iter().any(|status| {
        status.id.is_empty()
            || !status.root.is_absolute()
            || !status.root.starts_with(&location)
            || status
                .root
                .components()
                .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
    }) {
        return Err("Existing server status roots are invalid".into());
    }
    if crate::read_registration(paths).as_ref() != Some(&registration) {
        return Err("Existing server registration changed during status observation".into());
    }
    Ok(Some(statuses))
}

/// `GET /api/v1/lsp/map` on the registered server, or `None` without one.
pub async fn registered_repo_map(
    paths: &Paths,
    location: &Path,
    request: &RepoMapRequest,
) -> Result<Option<RepoMapView>, String> {
    let mut query: Vec<(&str, String)> = Vec::new();
    if let Some(path) = &request.path {
        query.push(("path", path.clone()));
    }
    query.extend(request.focus.iter().map(|f| ("focus", f.clone())));
    query.extend(request.symbol.iter().map(|s| ("symbol", s.clone())));
    if let Some(tokens) = request.tokens {
        query.push(("tokens", tokens.to_string()));
    }
    let Some((body, _)) = registered_call(
        paths,
        location,
        reqwest::Method::GET,
        "/api/v1/lsp/map",
        &query,
        MAP_TIMEOUT,
    )
    .await?
    else {
        return Ok(None);
    };
    serde_json::from_value(body["data"].clone())
        .map(Some)
        .map_err(|_| "Existing server map is malformed".into())
}

/// `POST /api/v1/lsp/map/rebuild` on the registered server, or `None` without one.
pub async fn registered_repo_map_rebuild(
    paths: &Paths,
    location: &Path,
) -> Result<Option<RepoMapIndex>, String> {
    let Some((body, _)) = registered_call(
        paths,
        location,
        reqwest::Method::POST,
        "/api/v1/lsp/map/rebuild",
        &[],
        MAP_TIMEOUT,
    )
    .await?
    else {
        return Ok(None);
    };
    serde_json::from_value(body["data"].clone())
        .map(Some)
        .map_err(|_| "Existing server map status is malformed".into())
}

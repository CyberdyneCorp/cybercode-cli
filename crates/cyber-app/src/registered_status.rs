//! Read-only observation of an existing registered server, without startup or database access.
use cyber_core::paths::Paths;
use cyber_server::http::LspStatus;
use serde_json::Value;
use std::{
    path::{Component, Path},
    time::Duration,
};

const MAX_RESPONSE: usize = 1024 * 1024;

pub async fn registered_lsp_status(
    paths: &Paths,
    location: &Path,
) -> Result<Option<Vec<LspStatus>>, String> {
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
    let mut builder = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none());
    let mut url = registration.url.clone();
    if url.is_empty() {
        #[cfg(unix)]
        {
            let socket = registration
                .socket
                .as_ref()
                .ok_or("Existing server has no status listener")?;
            builder = builder.unix_socket(socket.as_str());
            url = "http://localhost".into();
        }
        #[cfg(not(unix))]
        return Err("Existing server has no TCP status listener".into());
    }
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
    url.set_path("/api/v1/lsp");
    let mut response = builder
        .build()
        .map_err(|_| "Cannot create status transport")?
        .get(url)
        .basic_auth("cyber", Some(password.trim()))
        .query(&[("location[directory]", location.to_string_lossy().as_ref())])
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

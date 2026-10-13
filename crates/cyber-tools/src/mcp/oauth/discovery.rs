//! RFC 9728 protected-resource and RFC 8414 / OpenID Connect authorization-server
//! discovery. Every document and endpoint must be HTTPS (plain HTTP only on loopback), and
//! only a local MCP server may name a local `resource_metadata` URL or issuer, and only a
//! local issuer a local endpoint;
//! the protected resource must name exactly the configured server URL and the metadata
//! exactly its issuer, and PKCE S256 must be offered, or discovery fails closed.
use reqwest::Url;
use serde_json::Value;

use super::{AuthChallenge, OAuthError, get_json, resource_indicator, secure, secure_from};

/// Validated authorization-server metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorizationServer {
    pub issuer: Url,
    pub authorization_endpoint: Url,
    pub token_endpoint: Url,
    pub registration_endpoint: Option<Url>,
    pub scopes_supported: Vec<String>,
}

/// The protected resource and the authorization server it names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Discovery {
    /// The RFC 8707 resource indicator: the canonical MCP server URL without its query.
    pub resource: Url,
    /// Scopes the protected resource advertises.
    pub scopes_supported: Vec<String>,
    pub server: AuthorizationServer,
}

/// Fetch the protected-resource metadata (the challenge's `resource_metadata`, then the
/// path-suffixed well-known URL, then the root one) and its first authorization server's
/// metadata (RFC 8414, then OpenID Connect discovery). The server URL itself must be HTTPS
/// (plain HTTP only on loopback), so no metadata document is fetched in cleartext.
pub async fn discover(
    client: &reqwest::Client,
    server: &Url,
    challenge: Option<&AuthChallenge>,
) -> Result<Discovery, OAuthError> {
    secure(server)?;
    let (resource, document) = protected_resource(client, server, challenge).await?;
    let issuer = first_issuer(&document, server)?;
    let metadata = authorization_server(client, &issuer).await?;
    Ok(Discovery {
        resource,
        scopes_supported: strings(&document["scopes_supported"]),
        server: metadata,
    })
}

async fn protected_resource(
    client: &reqwest::Client,
    server: &Url,
    challenge: Option<&AuthChallenge>,
) -> Result<(Url, Value), OAuthError> {
    let expected = resource_indicator(server);
    for candidate in resource_candidates(server, challenge)? {
        let Some(document) = get_json(client, &candidate).await? else {
            continue;
        };
        let resource = document["resource"]
            .as_str()
            .and_then(|resource| Url::parse(resource).ok())
            .ok_or(OAuthError::Metadata(
                "protected resource metadata names no resource",
            ))?;
        if resource != expected {
            return Err(OAuthError::ResourceMismatch);
        }
        return Ok((resource, document));
    }
    Err(OAuthError::Metadata(
        "protected resource metadata not found",
    ))
}

fn resource_candidates(
    server: &Url,
    challenge: Option<&AuthChallenge>,
) -> Result<Vec<Url>, OAuthError> {
    let mut candidates = Vec::new();
    if let Some(hint) = challenge.and_then(|challenge| challenge.resource_metadata.as_deref()) {
        let hint =
            Url::parse(hint).map_err(|_| OAuthError::Metadata("invalid resource_metadata URL"))?;
        secure_from(&hint, server)?;
        candidates.push(hint);
    }
    let path = server.path().trim_end_matches('/');
    if !path.is_empty() {
        candidates.push(well_known(server, "oauth-protected-resource", path));
    }
    candidates.push(well_known(server, "oauth-protected-resource", ""));
    for candidate in &candidates {
        secure(candidate)?;
    }
    Ok(candidates)
}

/// `<origin>/.well-known/<name><suffix>` (RFC 8615 with RFC 8414/9728 path insertion).
fn well_known(base: &Url, name: &str, suffix: &str) -> Url {
    let mut url = base.clone();
    url.set_path(&format!("/.well-known/{name}{suffix}"));
    url.set_query(None);
    url.set_fragment(None);
    url
}

fn first_issuer(document: &Value, server: &Url) -> Result<Url, OAuthError> {
    let issuer = document["authorization_servers"]
        .as_array()
        .and_then(|servers| servers.first())
        .and_then(Value::as_str)
        .and_then(|issuer| Url::parse(issuer).ok())
        .ok_or(OAuthError::Metadata(
            "protected resource metadata names no authorization server",
        ))?;
    secure_from(&issuer, server)?;
    if issuer.query().is_some() {
        return Err(OAuthError::Metadata("issuer must not carry a query"));
    }
    Ok(issuer)
}

async fn authorization_server(
    client: &reqwest::Client,
    issuer: &Url,
) -> Result<AuthorizationServer, OAuthError> {
    for candidate in metadata_candidates(issuer) {
        if let Some(document) = get_json(client, &candidate).await? {
            return parse_metadata(issuer, &document);
        }
    }
    Err(OAuthError::Metadata(
        "authorization server metadata not found",
    ))
}

fn metadata_candidates(issuer: &Url) -> Vec<Url> {
    let path = issuer.path().trim_end_matches('/');
    let mut candidates = vec![
        well_known(issuer, "oauth-authorization-server", path),
        well_known(issuer, "openid-configuration", path),
    ];
    if !path.is_empty() {
        let mut appended = issuer.clone();
        appended.set_path(&format!("{path}/.well-known/openid-configuration"));
        candidates.push(appended);
    }
    candidates
}

fn parse_metadata(issuer: &Url, document: &Value) -> Result<AuthorizationServer, OAuthError> {
    let named = document["issuer"]
        .as_str()
        .and_then(|value| Url::parse(value).ok());
    if named.as_ref() != Some(issuer) {
        return Err(OAuthError::IssuerMismatch);
    }
    let methods = strings(&document["code_challenge_methods_supported"]);
    if !methods.iter().any(|method| method == "S256") {
        return Err(OAuthError::NoPkce);
    }
    let registration_endpoint = match document.get("registration_endpoint") {
        None | Some(Value::Null) => None,
        Some(_) => Some(endpoint(issuer, document, "registration_endpoint")?),
    };
    Ok(AuthorizationServer {
        issuer: issuer.clone(),
        authorization_endpoint: endpoint(issuer, document, "authorization_endpoint")?,
        token_endpoint: endpoint(issuer, document, "token_endpoint")?,
        registration_endpoint,
        scopes_supported: strings(&document["scopes_supported"]),
    })
}

fn endpoint(issuer: &Url, document: &Value, field: &'static str) -> Result<Url, OAuthError> {
    let url = document[field]
        .as_str()
        .and_then(|value| Url::parse(value).ok())
        .ok_or(OAuthError::Metadata(
            "authorization server metadata lacks a required endpoint",
        ))?;
    secure_from(&url, issuer)?;
    Ok(url)
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn parse(url: &str) -> Url {
        Url::parse(url).unwrap()
    }

    #[test]
    fn a_remote_server_cannot_name_a_local_resource_metadata_url_or_issuer() {
        let server = parse("https://mcp.example.test/mcp");
        let challenge = AuthChallenge {
            resource_metadata: Some("http://127.0.0.1:8080/admin".into()),
            ..AuthChallenge::default()
        };
        assert!(matches!(
            resource_candidates(&server, Some(&challenge)),
            Err(OAuthError::InsecureEndpoint)
        ));
        let document = json!({"authorization_servers":["http://127.0.0.1:8080"]});
        assert!(matches!(
            first_issuer(&document, &server),
            Err(OAuthError::InsecureEndpoint)
        ));
        // A local server may use local metadata and a local issuer.
        let local = parse("http://127.0.0.1:7000/mcp");
        assert!(resource_candidates(&local, Some(&challenge)).is_ok());
        assert!(first_issuer(&document, &local).is_ok());
    }

    #[test]
    fn a_cleartext_non_loopback_server_gets_no_metadata_candidates() {
        let server = parse("http://mcp.corp.example.test/mcp");
        assert!(matches!(
            resource_candidates(&server, None),
            Err(OAuthError::InsecureEndpoint)
        ));
        assert!(resource_candidates(&parse("https://mcp.example.test/mcp"), None).is_ok());
    }

    #[test]
    fn the_resource_indicator_drops_the_url_query() {
        let server = parse("https://mcp.example.test/mcp?key=secret");
        assert_eq!(
            resource_indicator(&server).as_str(),
            "https://mcp.example.test/mcp"
        );
        for candidate in resource_candidates(&server, None).unwrap() {
            assert!(candidate.query().is_none(), "{candidate}");
        }
    }

    #[test]
    fn an_https_issuer_cannot_name_local_endpoints() {
        let issuer = parse("https://as.example.test");
        let metadata = |field: &str, value: &str| {
            let mut document = json!({
                "issuer": issuer.as_str(),
                "authorization_endpoint": "https://as.example.test/authorize",
                "token_endpoint": "https://as.example.test/token",
                "registration_endpoint": "https://as.example.test/register",
                "code_challenge_methods_supported": ["S256"],
            });
            document[field] = json!(value);
            document
        };
        assert!(parse_metadata(&issuer, &metadata("issuer", issuer.as_str())).is_ok());
        for field in [
            "authorization_endpoint",
            "token_endpoint",
            "registration_endpoint",
        ] {
            for value in [
                "http://127.0.0.1:8080/admin",
                "https://localhost:8443/admin",
            ] {
                assert!(
                    matches!(
                        parse_metadata(&issuer, &metadata(field, value)),
                        Err(OAuthError::InsecureEndpoint)
                    ),
                    "{field} {value}"
                );
            }
        }
        // A loopback issuer may name loopback endpoints.
        let local = parse("http://127.0.0.1:9000/");
        let mut document = metadata("issuer", local.as_str());
        document["token_endpoint"] = json!("http://127.0.0.1:9000/token");
        document["authorization_endpoint"] = json!("http://127.0.0.1:9000/authorize");
        document["registration_endpoint"] = json!("http://127.0.0.1:9000/register");
        assert!(parse_metadata(&local, &document).is_ok());
    }
}

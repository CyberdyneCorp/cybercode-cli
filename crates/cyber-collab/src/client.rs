//! Explicit hub transport. No server configuration or cloned project can create it
//! automatically; the caller must confirm/pin the hub and project locally.
use crate::{
    api::{Data, Mutation, MutationResult},
    error::Error,
};
use cyber_account::Secret;
use reqwest::{Client, Method, redirect::Policy};
use serde::{Serialize, de::DeserializeOwned};
use std::time::Duration;
use url::{Host, Url};
#[derive(Clone)]
pub struct HubClient {
    base: Url,
    token: Secret,
    http: Client,
}
impl HubClient {
    pub fn new(base: &str, token: Secret) -> Result<Self, Error> {
        let mut base = Url::parse(base).map_err(|_| Error::invalid("invalid hub URL"))?;
        let loopback = matches!(base.host(),Some(Host::Ipv4(ip)) if ip.is_loopback())
            || matches!(base.host(),Some(Host::Ipv6(ip)) if ip.is_loopback());
        if !(base.scheme() == "https" || (base.scheme() == "http" && loopback))
            || !base.username().is_empty()
            || base.password().is_some()
            || base.query().is_some()
            || base.fragment().is_some()
        {
            return Err(Error::invalid("HTTPS is required outside literal loopback"));
        }
        base.set_path(&format!("{}/", base.path().trim_end_matches('/')));
        let http = Client::builder()
            .redirect(Policy::none())
            .timeout(Duration::from_secs(15))
            .build()
            .map_err(|_| Error::new("HubUnavailableError", "hub transport unavailable"))?;
        Ok(Self { base, token, http })
    }
    fn url(&self, path: &str) -> Result<Url, Error> {
        if !path.starts_with('/')
            || path.starts_with("//")
            || path.contains("..")
            || path.contains('#')
        {
            return Err(Error::invalid("invalid hub operation path"));
        }
        // Keep an explicit deployment prefix while preventing an absolute URL or
        // authority override from sending a credential to a different origin.
        let suffix = path
            .strip_prefix("/api/v1/hub")
            .unwrap_or(path)
            .trim_start_matches('/');
        let base_path = self.base.path().trim_end_matches('/');
        let prefix = if base_path.ends_with("/api/v1/hub") {
            base_path.to_owned()
        } else {
            format!("{base_path}/api/v1/hub")
        };
        let mut url = self.base.clone();
        let (path, query) = suffix
            .split_once('?')
            .map_or((suffix, None), |(p, q)| (p, Some(q)));
        url.set_path(&format!("{prefix}/{path}"));
        url.set_query(query);
        Ok(url)
    }
    pub async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T, Error> {
        let response = self
            .http
            .get(self.url(path)?)
            .bearer_auth(self.token.expose())
            .send()
            .await
            .map_err(|_| Error::new("HubUnavailableError", "hub is unreachable"))?;
        let data: Data<T> = response_json(response).await?;
        Ok(data.data)
    }
    pub async fn mutate<I: Serialize, O: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        input: &Mutation<I>,
    ) -> Result<MutationResult<O>, Error> {
        let response = self
            .http
            .request(method, self.url(path)?)
            .bearer_auth(self.token.expose())
            .json(input)
            .send()
            .await
            .map_err(|_| {
                Error::new(
                    "HubUnavailableError",
                    "result unknown; retry with the same operation ID",
                )
            })?;
        response_json(response).await
    }
}
async fn response_json<T: DeserializeOwned>(mut response: reqwest::Response) -> Result<T, Error> {
    let status = response.status();
    let mut bytes = zeroize::Zeroizing::new(Vec::new());
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| Error::new("HubUnavailableError", "hub response was interrupted"))?
    {
        if chunk.len() > (16 * 1024 * 1024usize).saturating_sub(bytes.len()) {
            return Err(Error::invalid("hub response exceeds the client bound"));
        }
        bytes.extend_from_slice(&chunk);
    }
    if !status.is_success() {
        return Err(serde_json::from_slice::<Error>(&bytes)
            .unwrap_or_else(|_| Error::new("HubUnavailableError", "hub refused the request")));
    }
    serde_json::from_slice(&bytes).map_err(|_| Error::invalid("invalid hub response"))
}

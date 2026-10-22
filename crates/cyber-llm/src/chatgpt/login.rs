//! The end of a sign-in, shared by `cyber providers login chatgpt` and the server's provider
//! login API: exchange the authorization code, store the login and cache the plan's models.

use super::{AuthFile, ChatGptError, Issuer, Login, ModelsFile, exchange_code, fetch_models};

/// A stored login and the plan's model slugs, or why they could not be listed. A failed
/// listing keeps the login: signing in again retries it.
#[derive(Debug)]
pub struct SignedIn {
    pub login: Login,
    pub models: Result<Vec<String>, ChatGptError>,
}

/// Exchange `code` with the PKCE `verifier`, save the login to `file` and cache the models
/// the backend lists for it. `backend` is [`super::backend_url`]'s result.
pub async fn complete_login(
    http: &reqwest::Client,
    issuer: &Issuer,
    backend: Result<String, ChatGptError>,
    file: &AuthFile,
    code: &str,
    verifier: &str,
) -> Result<SignedIn, ChatGptError> {
    let tokens = exchange_code(http, issuer, code, verifier).await?;
    let login = Login::from_exchange(tokens)?;
    file.save(&login)?;
    let models = match backend {
        Ok(backend) => cache_models(http, &backend, file, &login).await,
        Err(error) => Err(error),
    };
    Ok(SignedIn { login, models })
}

async fn cache_models(
    http: &reqwest::Client,
    backend: &str,
    file: &AuthFile,
    login: &Login,
) -> Result<Vec<String>, ChatGptError> {
    let models = fetch_models(http, backend, login).await?;
    let slugs = models.iter().map(|model| model.slug.clone()).collect();
    ModelsFile::beside(file.path()).save(models)?;
    Ok(slugs)
}

//! Provider sign-in for API clients (`provider-credentials` → Provider login API): the
//! ChatGPT OAuth flow run by the server, so the login lands in the data directory of the
//! server that uses it and the catalog is rebuilt without a restart.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use cyber_core::env::ProcessEnv;
use cyber_llm::chatgpt::{self, AuthFile, Issuer, ModelsFile, SignedIn};
use cyber_server::http::ApiError;
use cyber_server::http::providers::{LoginAttempt, LoginStatus, ProviderLogins};
use cyber_server::runtime::CatalogResolver;
use cyber_tools::mcp::oauth::{self, CallbackListener, Pkce};
use futures::future::BoxFuture;
use tokio::task::JoinHandle;

type Result<T> = std::result::Result<T, ApiError>;

/// How long a pending attempt waits for the browser.
const ATTEMPT_TIMEOUT: Duration = Duration::from_secs(5 * 60);
/// How long a finished attempt stays readable.
const FINISHED_RETENTION: Duration = Duration::from_secs(10 * 60);

pub struct ProviderLoginService {
    inner: Arc<Inner>,
}

struct Inner {
    file: AuthFile,
    resolver: Arc<CatalogResolver>,
    http: reqwest::Client,
    attempts: Mutex<HashMap<String, Attempt>>,
}

struct Attempt {
    view: LoginAttempt,
    /// Taken by whichever completion runs first, so an attempt completes once.
    secret: Option<Secret>,
    listener: Option<JoinHandle<()>>,
    deadline: Instant,
    finished_at: Option<Instant>,
}

struct Secret {
    issuer: Issuer,
    verifier: String,
    state: String,
}

impl ProviderLoginService {
    pub fn new(data: &std::path::Path, resolver: Arc<CatalogResolver>) -> Self {
        Self {
            inner: Arc::new(Inner {
                file: AuthFile::new(chatgpt::auth_path(data)),
                resolver,
                http: cyber_llm::Endpoint::client_builder()
                    .build()
                    .unwrap_or_default(),
                attempts: Mutex::default(),
            }),
        }
    }
}

impl ProviderLogins for ProviderLoginService {
    fn start<'a>(&'a self, provider: &'a str) -> BoxFuture<'a, Result<LoginAttempt>> {
        Box::pin(async move {
            self.inner.supported(provider)?;
            if cfg!(windows) {
                return Err(ApiError::invalid(
                    "ChatGPT login is not supported on Windows yet; use the openai provider with OPENAI_API_KEY",
                ));
            }
            Inner::start(&self.inner).await
        })
    }

    fn attempt(&self, provider: &str, attempt_id: &str) -> Result<LoginAttempt> {
        self.inner.supported(provider)?;
        let attempts = self.inner.attempts();
        attempts
            .get(attempt_id)
            .map(|attempt| attempt.view.clone())
            .ok_or_else(|| not_found(attempt_id))
    }

    fn submit_code<'a>(
        &'a self,
        provider: &'a str,
        attempt_id: &'a str,
        redirect_url: &'a str,
    ) -> BoxFuture<'a, Result<LoginAttempt>> {
        Box::pin(async move {
            self.inner.supported(provider)?;
            let state = self.inner.pending_state(attempt_id)?;
            let code = oauth::callback_code(redirect_url, &state)
                .map_err(|error| ApiError::invalid(error.to_string()))?;
            let secret = self.inner.take_secret(attempt_id, true)?;
            Ok(self.inner.complete(attempt_id, secret, &code).await)
        })
    }

    fn cancel(&self, provider: &str, attempt_id: &str) -> Result<()> {
        self.inner.supported(provider)?;
        let mut attempts = self.inner.attempts();
        let attempt = attempts
            .get_mut(attempt_id)
            .ok_or_else(|| not_found(attempt_id))?;
        attempt.fail("login cancelled");
        Ok(())
    }

    fn logout(&self, provider: &str) -> Result<bool> {
        self.inner.supported(provider)?;
        let removed = self.inner.file.remove().map_err(ApiError::unknown)?;
        ModelsFile::beside(self.inner.file.path()).remove();
        self.inner.resolver.reload(&ProcessEnv);
        Ok(removed)
    }
}

impl Inner {
    /// The attempts, with expired ones failed and old finished ones forgotten.
    fn attempts(&self) -> MutexGuard<'_, HashMap<String, Attempt>> {
        let mut attempts = self.attempts.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        for attempt in attempts.values_mut() {
            if attempt.view.status == LoginStatus::Pending && now >= attempt.deadline {
                attempt.fail("login timed out");
            }
        }
        attempts.retain(|_, attempt| {
            attempt
                .finished_at
                .is_none_or(|at| now.duration_since(at) < FINISHED_RETENTION)
        });
        attempts
    }

    /// Only `chatgpt` signs in; any other provider is told which variables carry its key.
    fn supported(&self, provider: &str) -> Result<()> {
        if provider == chatgpt::PROVIDER_ID {
            return Ok(());
        }
        let env = self
            .resolver
            .catalog()
            .providers
            .get(provider)
            .map(|provider| provider.env.clone())
            .unwrap_or_default();
        let hint = if env.is_empty() {
            "configure its key under providers in cyber.jsonc".to_string()
        } else {
            format!("set {}", env.join(" or "))
        };
        Err(ApiError::new(
            axum::http::StatusCode::BAD_REQUEST,
            "ProviderLoginUnsupportedError",
            format!("Provider login is not available for {provider}; {hint}"),
        )
        .with_field("env", env))
    }

    async fn start(inner: &Arc<Self>) -> Result<LoginAttempt> {
        let issuer = Issuer::from_env(&ProcessEnv).map_err(|e| ApiError::invalid(e.to_string()))?;
        let pkce = Pkce::generate().map_err(ApiError::unknown)?;
        let state = oauth::random_state().map_err(ApiError::unknown)?;
        let url = chatgpt::authorize_url(&issuer, &pkce.challenge, &state);
        inner.supersede_pending().await;
        let listener =
            CallbackListener::bind_path(Some(chatgpt::CALLBACK_PORT), chatgpt::CALLBACK_PATH)
                .await
                .ok();
        let id = cyber_core::ids::new_id("pla");
        let view = LoginAttempt {
            attempt_id: id.clone(),
            provider: chatgpt::PROVIDER_ID.into(),
            status: LoginStatus::Pending,
            authorization_url: url.to_string(),
            callback: listener.is_some(),
            expires_at: unix_now() + ATTEMPT_TIMEOUT.as_secs(),
            email: None,
            plan: None,
            models: Vec::new(),
            warning: None,
            error: None,
        };
        let listener = listener.map(|listener| {
            let (inner, id, state) = (Arc::clone(inner), id.clone(), state.clone());
            tokio::spawn(async move { inner.await_callback(&id, listener, &state).await })
        });
        let secret = Secret {
            issuer,
            verifier: pkce.verifier,
            state,
        };
        inner.attempts().insert(
            id,
            Attempt {
                view: view.clone(),
                secret: Some(secret),
                listener,
                deadline: Instant::now() + ATTEMPT_TIMEOUT,
                finished_at: None,
            },
        );
        Ok(view)
    }

    /// Cancel every pending attempt and wait for its listener to release the port.
    async fn supersede_pending(&self) {
        let listeners: Vec<JoinHandle<()>> = self
            .attempts()
            .values_mut()
            .filter(|attempt| attempt.view.status == LoginStatus::Pending)
            .filter_map(|attempt| {
                let listener = attempt.listener.take();
                attempt.fail("superseded by a new login");
                listener
            })
            .collect();
        for listener in listeners {
            listener.abort();
            let _ = listener.await;
        }
    }

    async fn await_callback(&self, id: &str, listener: CallbackListener, state: &str) {
        let error = match listener.wait(state, ATTEMPT_TIMEOUT).await {
            Ok(code) => {
                // A pasted code may have claimed the attempt first.
                if let Ok(secret) = self.take_secret(id, false) {
                    self.complete(id, secret, &code).await;
                }
                return;
            }
            Err(error) => error.to_string(),
        };
        if let Some(attempt) = self.attempts().get_mut(id) {
            attempt.fail(&error);
        }
    }

    fn pending_state(&self, id: &str) -> Result<String> {
        let attempts = self.attempts();
        let attempt = attempts.get(id).ok_or_else(|| not_found(id))?;
        attempt
            .secret
            .as_ref()
            .map(|secret| secret.state.clone())
            .ok_or_else(|| ApiError::conflict("the login attempt is no longer pending"))
    }

    /// Claim the attempt's completion. A pasted code stops the listener; the listener's own
    /// completion leaves its task running.
    fn take_secret(&self, id: &str, stop_listener: bool) -> Result<Secret> {
        let mut attempts = self.attempts();
        let attempt = attempts.get_mut(id).ok_or_else(|| not_found(id))?;
        let secret = attempt
            .secret
            .take()
            .ok_or_else(|| ApiError::conflict("the login attempt is no longer pending"))?;
        let listener = attempt.listener.take();
        if stop_listener && let Some(listener) = listener {
            listener.abort();
        }
        Ok(secret)
    }

    async fn complete(&self, id: &str, secret: Secret, code: &str) -> LoginAttempt {
        let result = chatgpt::complete_login(
            &self.http,
            &secret.issuer,
            chatgpt::backend_url(&ProcessEnv),
            &self.file,
            code,
            &secret.verifier,
        )
        .await;
        if result.is_ok() {
            self.resolver.reload(&ProcessEnv);
        }
        let mut attempts = self.attempts();
        let Some(attempt) = attempts.get_mut(id) else {
            return failed_view(id, "login attempt expired");
        };
        match result {
            Ok(signed_in) => attempt.succeed(signed_in),
            Err(error) => attempt.fail(&error.to_string()),
        }
        attempt.view.clone()
    }
}

impl Attempt {
    fn fail(&mut self, error: &str) {
        if self.view.status != LoginStatus::Pending {
            return;
        }
        if let Some(listener) = self.listener.take() {
            listener.abort();
        }
        self.secret = None;
        self.view.status = LoginStatus::Failed;
        self.view.error = Some(error.to_string());
        self.finished_at = Some(Instant::now());
    }

    fn succeed(&mut self, signed_in: SignedIn) {
        self.view.status = LoginStatus::Complete;
        self.view.error = None;
        self.view.email = signed_in.login.email;
        self.view.plan = signed_in.login.plan;
        match signed_in.models {
            Ok(models) => self.view.models = models,
            Err(error) => {
                self.view.warning = Some(format!(
                    "could not list the plan's models ({error}); sign in again to retry"
                ))
            }
        }
        self.finished_at = Some(Instant::now());
    }
}

fn not_found(id: &str) -> ApiError {
    ApiError::not_found(
        "ProviderLoginNotFoundError",
        format!("Login attempt not found: {id}"),
    )
}

fn failed_view(id: &str, error: &str) -> LoginAttempt {
    LoginAttempt {
        attempt_id: id.into(),
        provider: chatgpt::PROVIDER_ID.into(),
        status: LoginStatus::Failed,
        authorization_url: String::new(),
        callback: false,
        expires_at: 0,
        email: None,
        plan: None,
        models: Vec::new(),
        warning: None,
        error: Some(error.into()),
    }
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

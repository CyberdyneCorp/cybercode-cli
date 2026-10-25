//! Account orchestration for the local-owner API. Settings come from the application's
//! trusted global loader; protocol and storage adapters live in `cyber-account`.
use axum::http::StatusCode;
use cyber_account::{
    AccountSettings, AccountStore, CallbackHandle, Error, LoopbackLogin, StorageKind, TokenSet,
};
use cyber_server::http::{
    ApiError,
    account::{AccountService, AccountView, LoginAttempt, LoginOptions, LoginStatus},
};
use futures::future::BoxFuture;
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

pub type LogoutHook = dyn Fn(Option<(cyber_account::Secret, cyber_account::Account)>, bool) -> BoxFuture<'static, ()>
    + Send
    + Sync;
pub type SettingsFn = dyn Fn() -> Result<AccountSettings, Error> + Send + Sync;
struct Attempt {
    view: LoginAttempt,
    callback: Option<CallbackHandle>,
    started: Instant,
    cancel: tokio_util::sync::CancellationToken,
}
struct Inner {
    store: AccountStore,
    cache: PathBuf,
    settings: Arc<SettingsFn>,
    attempts: Mutex<BTreeMap<String, Attempt>>,
    logout_hooks: Mutex<Vec<Arc<LogoutHook>>>,
}
#[derive(Clone)]
pub struct Accounts {
    inner: Arc<Inner>,
}
impl Accounts {
    pub fn new(store: AccountStore, cache: PathBuf, settings: Arc<SettingsFn>) -> Self {
        Self {
            inner: Arc::new(Inner {
                store,
                cache,
                settings,
                attempts: Mutex::new(BTreeMap::new()),
                logout_hooks: Mutex::new(Vec::new()),
            }),
        }
    }
    pub fn on_logout(&self, hook: Arc<LogoutHook>) {
        if let Ok(mut hooks) = self.inner.logout_hooks.lock() {
            hooks.push(hook);
        }
    }
    async fn tokens(&self) -> Result<Option<TokenSet>, ApiError> {
        let Some(tokens) = self.inner.store.load().await.map_err(api_error)? else {
            return Ok(None);
        };
        let settings = (self.inner.settings)().map_err(api_error)?;
        if tokens.account().issuer.trim_end_matches('/') != settings.issuer.trim_end_matches('/')
            || tokens.client_id() != settings.client_id
        {
            return Err(api_error(Error::IssuerMismatch));
        }
        if tokens.account().expires_at > now().saturating_add(60) {
            return Ok(Some(tokens));
        }
        let client = settings
            .client(&self.inner.cache)
            .await
            .map_err(api_error)?;
        self.inner
            .store
            .refresh(client, settings.audience)
            .await
            .map(Some)
            .map_err(api_error)
    }
    /// Trusted runtime adapters only. HTTP handlers expose `AccountView`, never this.
    pub async fn access_token(&self) -> Result<cyber_account::Secret, ApiError> {
        self.tokens()
            .await?
            .map(|t| t.access_token().clone())
            .ok_or_else(|| api_error(Error::NotLoggedIn))
    }
    /// One verified credential record; never combine a token with a later account selection.
    pub(crate) async fn credential_snapshot(
        &self,
    ) -> Result<(cyber_account::Secret, cyber_account::Account), ApiError> {
        let tokens = self
            .tokens()
            .await?
            .ok_or_else(|| api_error(Error::NotLoggedIn))?;
        Ok((tokens.access_token().clone(), tokens.account().clone()))
    }
    fn finish(&self, id: &str, result: Result<(), Error>) {
        let Ok(mut attempts) = self.inner.attempts.lock() else {
            return;
        };
        if let Some(attempt) = attempts.get_mut(id) {
            attempt.view.authorization_url = None;
            attempt.view.user_code = None;
            attempt.callback = None;
            match result {
                Ok(()) => attempt.view.status = LoginStatus::Complete,
                Err(error) => {
                    attempt.view.status = LoginStatus::Failed;
                    attempt.view.error = Some(error.to_string());
                }
            }
        }
    }
}
impl AccountService for Accounts {
    fn current(&self) -> BoxFuture<'_, Result<AccountView, ApiError>> {
        Box::pin(async move {
            let tokens = self.tokens().await?;
            let storage = self
                .inner
                .store
                .storage_kind()
                .await
                .map_err(api_error)?
                .map(|s| {
                    match s {
                        StorageKind::File => "file",
                        StorageKind::Keyring => "keyring",
                    }
                    .to_owned()
                });
            Ok(AccountView {
                logged_in: tokens.is_some(),
                account: tokens.map(|t| t.account().clone()),
                storage,
            })
        })
    }
    fn start(&self, options: LoginOptions) -> BoxFuture<'_, Result<LoginAttempt, ApiError>> {
        Box::pin(async move {
            let epoch = self.inner.store.login_epoch().await.map_err(api_error)?;
            let settings = (self.inner.settings)().map_err(api_error)?;
            let mut client = settings
                .client(&self.inner.cache)
                .await
                .map_err(api_error)?;
            let warnings: Vec<_> = client
                .provider()
                .login_scopes()
                .map_err(api_error)?
                .omitted
                .into_iter()
                .map(|s| format!("issuer does not advertise scope {s}"))
                .collect();
            let id = format!("cal_{}", ulid::Ulid::new());
            let (view, callback, completion): (
                LoginAttempt,
                Option<CallbackHandle>,
                BoxFuture<'static, Result<TokenSet, Error>>,
            ) = if options.no_browser
                && !options.reauthenticate
                && client.provider().device_authorization_endpoint().is_some()
            {
                let device = client
                    .start_device_authorization()
                    .await
                    .map_err(api_error)?;
                let view = LoginAttempt {
                    attempt_id: id.clone(),
                    status: LoginStatus::Pending,
                    authorization_url: Some(device.verification_uri().to_string()),
                    user_code: Some(device.user_code().to_owned()),
                    error: None,
                    warnings: warnings.clone(),
                };
                let completion = Box::pin(async move {
                    client
                        .poll_device_authorization(device, &settings.audience)
                        .await
                });
                (view, None, completion)
            } else {
                let flow = LoopbackLogin::start_with_reauthentication(
                    client.provider(),
                    &settings.client_id,
                    options.reauthenticate,
                )
                .await
                .map_err(api_error)?;
                let view = LoginAttempt {
                    attempt_id: id.clone(),
                    status: LoginStatus::Pending,
                    authorization_url: Some(flow.authorization_url().to_string()),
                    user_code: None,
                    error: None,
                    warnings: warnings.clone(),
                };
                let callback = Some(flow.callback_handle());
                let completion = Box::pin(async move {
                    client
                        .exchange(flow.finish().await?, &settings.audience)
                        .await
                });
                (view, callback, completion)
            };
            let cancel = tokio_util::sync::CancellationToken::new();
            {
                let mut attempts = self
                    .inner
                    .attempts
                    .lock()
                    .map_err(|_| ApiError::unknown("account state unavailable"))?;
                attempts.retain(|_, a| a.started.elapsed() < Duration::from_secs(900));
                if attempts.len() >= 32 {
                    return Err(ApiError::new(
                        StatusCode::TOO_MANY_REQUESTS,
                        "AccountBusyError",
                        "too many login attempts",
                    ));
                }
                attempts.insert(
                    id.clone(),
                    Attempt {
                        view: view.clone(),
                        callback,
                        started: Instant::now(),
                        cancel: cancel.clone(),
                    },
                );
            }
            let service = self.clone();
            tokio::spawn(async move {
                let completed = tokio::select! {result=completion=>result,_=cancel.cancelled()=>Err(Error::LoginExpired)};
                let result = match completed {
                    Ok(tokens) => service
                        .inner
                        .store
                        .save_login(tokens, epoch)
                        .await
                        .map(|_| ()),
                    Err(error) => Err(error),
                };
                service.finish(&id, result);
            });
            Ok(view)
        })
    }
    fn attempt(&self, id: &str) -> Result<LoginAttempt, ApiError> {
        self.inner
            .attempts
            .lock()
            .map_err(|_| ApiError::unknown("account state unavailable"))?
            .get(id)
            .map(|a| a.view.clone())
            .ok_or_else(|| {
                ApiError::new(
                    StatusCode::NOT_FOUND,
                    "AccountLoginNotFoundError",
                    "login attempt not found",
                )
            })
    }
    fn submit(&self, id: &str, redirect: &str) -> Result<LoginAttempt, ApiError> {
        if redirect.len() > 8192 {
            return Err(api_error(Error::InvalidCallback));
        }
        let callback = self
            .inner
            .attempts
            .lock()
            .map_err(|_| ApiError::unknown("account state unavailable"))?
            .get(id)
            .and_then(|a| a.callback.clone())
            .ok_or_else(|| {
                ApiError::new(
                    StatusCode::CONFLICT,
                    "AccountLoginUnavailableError",
                    "this attempt does not accept a pasted redirect",
                )
            })?;
        callback.submit(redirect).map_err(api_error)?;
        self.attempt(id)
    }
    fn logout(&self, all: bool) -> BoxFuture<'_, Result<(), ApiError>> {
        Box::pin(async move {
            let tokens = self.inner.store.load().await.ok().flatten();
            self.inner.store.logout(all).await.map_err(api_error)?;
            if let Ok(mut attempts) = self.inner.attempts.lock() {
                for attempt in attempts.values_mut() {
                    if attempt.view.status == LoginStatus::Pending {
                        attempt.cancel.cancel();
                        attempt.view.status = LoginStatus::Failed;
                        attempt.view.error = Some(Error::LoginExpired.to_string());
                        attempt.callback = None;
                        attempt.view.authorization_url = None;
                        attempt.view.user_code = None;
                    }
                }
            }
            let hooks = self
                .inner
                .logout_hooks
                .lock()
                .map(|hooks| hooks.clone())
                .unwrap_or_default();
            for hook in hooks {
                let cleanup = hook(
                    tokens
                        .as_ref()
                        .map(|t| (t.access_token().clone(), t.account().clone())),
                    all,
                );
                let _ = tokio::time::timeout(Duration::from_secs(5), cleanup).await;
            }
            if let Some(tokens) = tokens
                && let Ok(settings) = (self.inner.settings)()
                && let Ok(client) = settings.client(&self.inner.cache).await
            {
                client.revoke_logout(&tokens).await;
            }
            Ok(())
        })
    }
}
fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}
fn api_error(error: Error) -> ApiError {
    let status = match error {
        Error::NotLoggedIn | Error::SessionExpired => StatusCode::UNAUTHORIZED,
        Error::Busy => StatusCode::CONFLICT,
        Error::Storage(_) | Error::DiscoveryUnavailable | Error::LiveValidationUnavailable => {
            StatusCode::SERVICE_UNAVAILABLE
        }
        _ => StatusCode::BAD_REQUEST,
    };
    let message = error.to_string();
    let kind = message
        .split(':')
        .next()
        .unwrap_or("AccountError")
        .to_owned();
    ApiError::new(status, &kind, message)
}

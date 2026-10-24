//! Temporary loopback callback adapter. The PKCE state machine remains independent
//! of HTTP and credentials never appear in the browser response.
use crate::{AuthorizationAttempt, CodeGrant, Error, Provider};
use axum::{
    Router,
    extract::{OriginalUri, State},
    http::StatusCode,
    routing::get,
};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{sync::oneshot, task::JoinHandle};
use url::Url;

struct Pending {
    attempt: Option<AuthorizationAttempt>,
    sender: Option<oneshot::Sender<Result<CodeGrant, Error>>>,
}

#[derive(Clone)]
pub struct CallbackHandle {
    pending: Arc<Mutex<Pending>>,
    redirect: Url,
}
impl CallbackHandle {
    /// Only the user's pasted full redirect URL belongs here. The same single-use
    /// state validation applies whether the browser or headless client submits it.
    pub fn submit(&self, callback: &str) -> Result<(), Error> {
        let mut pending = self.pending.lock().map_err(|_| Error::InvalidCallback)?;
        let attempt = pending.attempt.take().ok_or(Error::LoginExpired)?;
        let result = attempt.finish(callback);
        let error = result.as_ref().err().cloned();
        let sender = pending.sender.take().ok_or(Error::LoginExpired)?;
        sender.send(result).map_err(|_| Error::LoginExpired)?;
        error.map_or(Ok(()), Err)
    }
}

/// No Debug/Serialize: the authorization URL contains live state and nonce.
pub struct LoopbackLogin {
    authorization: Url,
    callback: CallbackHandle,
    receive: oneshot::Receiver<Result<CodeGrant, Error>>,
    task: Option<JoinHandle<()>>,
    stop: tokio_util::sync::CancellationToken,
}
impl LoopbackLogin {
    pub async fn start(provider: &Provider, client_id: &str) -> Result<Self, Error> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|_| Error::InvalidCallback)?;
        let redirect: Url = format!(
            "http://{}/callback",
            listener.local_addr().map_err(|_| Error::InvalidCallback)?
        )
        .parse()
        .map_err(|_| Error::InvalidCallback)?;
        let attempt = AuthorizationAttempt::new(provider, client_id, redirect.clone())?;
        let authorization = attempt.authorization_url().clone();
        let (sender, receive) = oneshot::channel();
        let callback = CallbackHandle {
            pending: Arc::new(Mutex::new(Pending {
                attempt: Some(attempt),
                sender: Some(sender),
            })),
            redirect,
        };
        let router = Router::new()
            .route("/callback", get(callback_request))
            .with_state(callback.clone());
        let stop = tokio_util::sync::CancellationToken::new();
        let shutdown = stop.clone();
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, router)
                .with_graceful_shutdown(shutdown.cancelled_owned())
                .await;
        });
        Ok(Self {
            authorization,
            callback,
            receive,
            task: Some(task),
            stop,
        })
    }
    pub fn authorization_url(&self) -> &Url {
        &self.authorization
    }
    pub fn callback_handle(&self) -> CallbackHandle {
        self.callback.clone()
    }
    pub async fn finish(mut self) -> Result<CodeGrant, Error> {
        let result = match tokio::time::timeout(Duration::from_secs(300), &mut self.receive).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(Error::InvalidCallback),
            Err(_) => Err(Error::LoginExpired),
        };
        self.stop.cancel();
        if let Some(mut task) = self.task.take()
            && tokio::time::timeout(Duration::from_millis(500), &mut task)
                .await
                .is_err()
        {
            task.abort();
        }
        result
    }
}
impl Drop for LoopbackLogin {
    fn drop(&mut self) {
        self.stop.cancel();
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}

async fn callback_request(
    State(handle): State<CallbackHandle>,
    OriginalUri(uri): OriginalUri,
) -> (StatusCode, &'static str) {
    let mut callback = handle.redirect.clone();
    callback.set_query(uri.query());
    match handle.submit(callback.as_str()) {
        Ok(()) => (
            StatusCode::OK,
            "Authorization received. Return to Cyber Code to check login status.",
        ),
        Err(_) => (
            StatusCode::BAD_REQUEST,
            "Authorization could not be verified. Start login again.",
        ),
    }
}

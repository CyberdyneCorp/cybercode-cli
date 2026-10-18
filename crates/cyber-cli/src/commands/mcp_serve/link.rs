//! The bridge's link to the registered server: the Unix socket with peer credentials
//! when the registration has one, else `127.0.0.1` with the local password. After a
//! transport failure it reconnects with backoff (0.5 s to 5 s) for up to 30 s; requests
//! wait meanwhile, and the link is gone for good when that window passes.

use std::sync::Arc;
use std::time::Duration;

use cyber_app::Registration;
use cyber_client::Client;
use cyber_core::paths::Paths;
use serde_json::json;
use tokio::sync::watch;
use tokio::time::Instant;

use super::Log;

const FIRST_BACKOFF: Duration = Duration::from_millis(500);
const MAX_BACKOFF: Duration = Duration::from_secs(5);
const RECONNECT_WINDOW: Duration = Duration::from_secs(30);
#[cfg(unix)]
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Clone)]
enum State {
    /// Connected; `epoch` counts reconnections so a stale failure is ignored.
    Up {
        client: Client,
        epoch: u64,
    },
    Reconnecting,
    Gone,
}

pub(super) struct Link {
    paths: Paths,
    state: watch::Sender<State>,
    log: Arc<Log>,
}

impl Link {
    pub async fn new(paths: Paths, info: &cyber_app::ServerClientInfo, log: Arc<Log>) -> Arc<Self> {
        let client = client_for(&info.registration, info.password.clone()).await;
        Arc::new(Self {
            paths,
            state: watch::Sender::new(State::Up { client, epoch: 0 }),
            log,
        })
    }

    /// The client to use, waiting while reconnecting; `None` once the link is gone.
    pub async fn client(&self) -> Option<(Client, u64)> {
        let mut state = self.state.subscribe();
        loop {
            match &*state.borrow_and_update() {
                State::Up { client, epoch } => return Some((client.clone(), *epoch)),
                State::Gone => return None,
                State::Reconnecting => {}
            }
            state.changed().await.ok()?;
        }
    }

    /// The client when connected right now, without waiting.
    pub fn current(&self) -> Option<Client> {
        match &*self.state.borrow() {
            State::Up { client, .. } => Some(client.clone()),
            _ => None,
        }
    }

    /// A request sent on `epoch` failed in transport: start reconnecting, once.
    pub fn failed(self: &Arc<Self>, epoch: u64, error: &str) {
        let start = self.state.send_if_modified(|state| {
            let current = matches!(state, State::Up { epoch: e, .. } if *e == epoch);
            if current {
                *state = State::Reconnecting;
            }
            current
        });
        if start {
            self.log.warn(
                "server unreachable; reconnecting",
                json!({ "error": error }),
            );
            tokio::spawn(Arc::clone(self).reconnect(epoch + 1));
        }
    }

    /// Resolves when the link is gone for good.
    pub async fn gone(&self) {
        let mut state = self.state.subscribe();
        let _ = state.wait_for(|s| matches!(s, State::Gone)).await;
    }

    async fn reconnect(self: Arc<Self>, epoch: u64) {
        let deadline = Instant::now() + RECONNECT_WINDOW;
        let mut delay = FIRST_BACKOFF;
        loop {
            tokio::time::sleep_until((Instant::now() + delay).min(deadline)).await;
            if let Some(client) = connect(&self.paths).await {
                self.log.info("reconnected", json!({}));
                self.state.send_replace(State::Up { client, epoch });
                return;
            }
            if Instant::now() >= deadline {
                self.log
                    .warn("server unreachable for 30 s; giving up", json!({}));
                self.state.send_replace(State::Gone);
                return;
            }
            delay = (delay * 2).min(MAX_BACKOFF);
        }
    }
}

/// A client for the registered server when it answers its health check; never starts one.
async fn connect(paths: &Paths) -> Option<Client> {
    let registration = cyber_app::read_registration(paths)?;
    if !cyber_app::registered_health(&registration, cyber_app::version()).await {
        return None;
    }
    let password = cyber_app::password(paths).ok()?;
    Some(client_for(&registration, password).await)
}

/// The registered Unix socket when it answers (peer credentials, no password), else TCP
/// with the local password.
async fn client_for(registration: &Registration, password: String) -> Client {
    #[cfg(unix)]
    if let Some(socket) = &registration.socket {
        let client = Client::unix(socket);
        let probe = tokio::time::timeout(PROBE_TIMEOUT, client.get("/health")).await;
        if matches!(probe, Ok(Ok(_))) {
            return client;
        }
    }
    Client::http(&registration.url, Some(password))
}

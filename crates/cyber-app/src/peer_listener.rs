//! The optional second listener for other `cyber` servers (`server-api` → Peer listener).
//!
//! It opens only when user config sets `peers.listen`. Anything that keeps it from opening
//! safely (invalid user config, a plain listener off loopback, an unreadable TLS key, a
//! port in use) leaves it closed and is logged; the main API starts either way.

use std::sync::Arc;
use std::time::Duration;

use cyber_core::config::PeersSettings;
use cyber_server::http;
use cyber_server::http::peer::PeerHandle;
use cyber_server::http::peer::tls::{self, CertSlot, PeerIdentity, SubjectNames};
use serde_json::json;

use crate::App;

/// A bound peer listener, ready to serve.
pub(crate) struct PeerListener {
    listener: tokio::net::TcpListener,
    tls: Option<Arc<tls::ServerConfig>>,
    /// Where it listens and the identity it presents; the main API's peer routes rotate it.
    pub handle: Arc<PeerHandle>,
}

impl PeerListener {
    /// Serve the peer router until `shutdown` resolves.
    pub fn spawn(
        self,
        app: &App,
        shutdown: impl Future<Output = ()> + Send + 'static,
    ) -> tokio::task::JoinHandle<std::io::Result<()>> {
        let router = http::peer::handle_router(app.state.clone(), &self.handle);
        tokio::spawn(http::serve_peer(router, self.listener, self.tls, shutdown))
    }
}

/// Bind the configured peer listener; `None` when none is configured or it cannot open.
pub(crate) async fn bind(app: &App, attach_wait: Duration) -> Option<PeerListener> {
    match try_bind(app, attach_wait).await {
        Ok(listener) => listener,
        Err(reason) => {
            cyber_core::log::warn(
                "server",
                "peer listener not started",
                json!({ "reason": reason }),
            );
            None
        }
    }
}

async fn try_bind(app: &App, attach_wait: Duration) -> Result<Option<PeerListener>, String> {
    let config = (app.global_config)()?;
    let settings = PeersSettings::from_config(&config)?;
    let Some(listen) = settings.listen else {
        return Ok(None);
    };
    // Validation refuses this already; the listener never relies on that alone.
    if !listen.tls && !listen.address.ip().is_loopback() {
        return Err(format!(
            "peers.listen.tls: false is allowed only on a loopback address, got {}",
            listen.address
        ));
    }
    let machine = http::peer::machine_name(settings.machine.as_deref());
    let names = SubjectNames {
        machine: machine.clone(),
        address: listen.address,
    };
    let dir = app.paths.state.join(tls::DIR);
    let identity = PeerIdentity::load_or_create(&dir, &names)?;
    let slot = if listen.tls {
        Some(CertSlot::new(&identity)?)
    } else {
        None
    };
    let server_config = slot.as_ref().map(CertSlot::server_config).transpose()?;
    let listener = tokio::net::TcpListener::bind(listen.address)
        .await
        .map_err(|error| format!("cannot listen on {}: {error}", listen.address))?;
    let address = listener.local_addr().map_err(|error| error.to_string())?;
    let handle = PeerHandle::new(address, &identity, slot, dir, names)
        .share_paths(settings.share_paths)
        .attach_wait(attach_wait);
    Ok(Some(PeerListener {
        listener,
        tls: server_config,
        handle: Arc::new(handle),
    }))
}

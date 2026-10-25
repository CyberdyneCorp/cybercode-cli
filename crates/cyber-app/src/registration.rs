//! `server.json` registration and background service management
//! (`server-api` → Background service management).

use std::future::Future;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use cyber_core::paths::Paths;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Registration {
    pub id: String,
    pub version: String,
    pub url: String,
    pub socket: Option<String>,
    pub pid: u32,
    /// Configured owner bind address, distinct from the reachable URL for wildcard binds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bind_host: Option<String>,
    /// Public owner certificate chain used for verified same-user local service management.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tls_cert: Option<String>,
    /// The server binary's build identity; absent in registrations of older builds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build: Option<String>,
    /// Canonical path of the server executable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binary: Option<String>,
    /// Length and modification time of that executable when the server started.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binary_stamp: Option<String>,
    /// The peer listener (`server-api` → Peer listener), when `peers.listen` started one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peer_url: Option<String>,
    /// `sha256:<hex>` of the peer certificate's SubjectPublicKeyInfo, with `peer_url`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peer_fingerprint: Option<String>,
}

impl Registration {
    /// This process as a registered server.
    pub fn current(id: String, url: String, socket: Option<String>) -> Self {
        let binary = cyber_core::version::binary();
        Self {
            id,
            version: crate::version().into(),
            url,
            socket,
            pid: std::process::id(),
            bind_host: None,
            tls_cert: None,
            build: Some(cyber_core::version::build_info().build.into()),
            binary: binary.path.clone(),
            binary_stamp: binary.stamp.clone(),
            peer_url: None,
            peer_fingerprint: None,
        }
    }
}

/// What a client needs to talk to the registered server.
#[derive(Debug, Clone)]
pub struct ServerClientInfo {
    pub registration: Registration,
    pub password: String,
    /// What starting did about a registered server of another build, for the client to
    /// tell the user.
    pub notice: Option<ServiceNotice>,
}

/// A registered server of another build, found while starting the service.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceNotice {
    /// It was idle and ran this install's executable, so it was replaced.
    Restarted,
    /// It runs work, belongs to another install, or cannot report its work: it is kept.
    StaleKept,
}

impl ServiceNotice {
    pub fn message(self) -> &'static str {
        match self {
            Self::Restarted => "restarted the Cyber server: it was running an older build",
            Self::StaleKept => {
                "The Cyber server runs an older build; restart it with `cyber service restart` when idle"
            }
        }
    }
}

/// This process's build, as compared with a registered server's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientBuild {
    pub version: String,
    pub build: String,
    pub binary: Option<String>,
    pub binary_stamp: Option<String>,
}

impl ClientBuild {
    pub fn current(version: &str) -> Self {
        let binary = cyber_core::version::binary();
        Self {
            version: version.into(),
            build: cyber_core::version::build_info().build.into(),
            binary: binary.path.clone(),
            binary_stamp: binary.stamp.clone(),
        }
    }

    /// Whether `reg`, answering `/health` with `health_version` and `health_build`, runs
    /// another build than this client: another version or build identity, or this same
    /// executable path rebuilt since the server started.
    pub fn is_stale(
        &self,
        reg: &Registration,
        health_version: &str,
        health_build: Option<&str>,
    ) -> bool {
        let rebuilt = reg.binary.is_some()
            && reg.binary == self.binary
            && reg.binary_stamp.is_some()
            && reg.binary_stamp != self.binary_stamp;
        health_version != self.version || health_build != Some(self.build.as_str()) || rebuilt
    }

    /// Whether `reg` runs this install's executable, so restarting it changes nothing
    /// another install relies on.
    pub fn same_install(&self, reg: &Registration) -> bool {
        reg.binary.is_some() && reg.binary == self.binary
    }
}

/// What `/health` reported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Probe {
    pub version: String,
    pub build: Option<String>,
    /// The answering server's instance id; absent from builds that predate it.
    pub id: Option<String>,
}

impl Probe {
    /// Whether this answer comes from the server `reg` names. A registration left by a
    /// killed server can point at a port another server (another user or install) took
    /// over; its password would not work there, so that server is never reused or stopped.
    pub fn serves(&self, reg: &Registration) -> bool {
        self.id.as_deref().is_none_or(|id| id == reg.id)
    }
}

pub fn registration_path(paths: &Paths) -> PathBuf {
    paths.state.join("server.json")
}

pub fn read_registration(paths: &Paths) -> Option<Registration> {
    let text = std::fs::read_to_string(registration_path(paths)).ok()?;
    let registration: Registration = serde_json::from_str(&text).ok()?;
    #[cfg(unix)]
    if let Some(socket) = &registration.socket {
        crate::server::local_socket(
            &crate::server::ServeOptions {
                socket: Some(PathBuf::from(socket)),
                ..Default::default()
            },
            paths,
        )
        .ok()?;
    }
    Some(registration)
}

fn checked_registration(paths: &Paths) -> Result<Option<Registration>, String> {
    let registration = read_registration(paths);
    if registration.is_none() && registration_path(paths).exists() {
        return Err("Existing server registration is invalid or uses an unsafe Unix socket. Stop the old server manually, remove its stale registration and restart with a socket inside the private state directory (or authenticated TCP).".into());
    }
    Ok(registration)
}

/// Remove `server.json` only when it still holds this server's ID.
pub(crate) fn remove_registration(paths: &Paths, id: &str) {
    if read_registration(paths).is_some_and(|r| r.id == id) {
        let _ = std::fs::remove_file(registration_path(paths));
    }
}

/// Write a file readable only by the owner (mode 0600), atomically.
pub(crate) fn write_private(path: &Path, text: &str) -> Result<(), String> {
    cyber_core::trust::write_private_atomic(path, text.as_bytes())
        .map_err(|e| format!("cannot write {}: {e}", path.display()))
}

/// Whether the server at `url` answers `/health` within 2 seconds with `version`.
pub async fn health(url: &str, version: &str) -> bool {
    probe(url).await.is_some_and(|p| p.version == version)
}

/// Whether the server `reg` names is the one answering at its URL, healthy and of
/// `version`.
pub async fn registered_health(reg: &Registration, version: &str) -> bool {
    probe_registration(reg)
        .await
        .is_some_and(|p| p.version == version && p.serves(reg))
}

pub async fn probe_registration(reg: &Registration) -> Option<Probe> {
    let (client, url) = server_client(reg).ok()?;
    let body = client
        .get(format!("{url}/api/v1/health"))
        .send()
        .await
        .ok()?
        .json::<serde_json::Value>()
        .await
        .ok()?;
    if body["healthy"] != true {
        return None;
    }
    Some(Probe {
        version: body["version"].as_str()?.into(),
        build: body["build"].as_str().map(str::to_string),
        id: body["id"].as_str().map(str::to_string),
    })
}

/// The version and build a healthy server at `url` reports within 2 seconds.
pub async fn probe(url: &str) -> Option<Probe> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
        .ok()?;
    let res = client
        .get(format!("{url}/api/v1/health"))
        .send()
        .await
        .ok()?;
    let body = res.json::<serde_json::Value>().await.ok()?;
    if body["healthy"] != true {
        return None;
    }
    Some(Probe {
        version: body["version"].as_str()?.to_string(),
        build: body["build"].as_str().map(str::to_string),
        id: body["id"].as_str().map(str::to_string),
    })
}

/// Reuse a healthy registered server of this build, or spawn `<exe> serve --register`
/// detached and wait for its registration, checking promptly within a five-second
/// deadline. A registered server of another build is replaced when it is idle and runs
/// this install's executable; otherwise it is kept and the notice says so.
pub async fn start_service(
    paths: &Paths,
    exe: &Path,
    version: &str,
) -> Result<ServerClientInfo, String> {
    start_service_with(paths, exe, version, None).await
}

/// Start or reuse the dashboard service without silently changing a healthy listener.
pub async fn start_web_service(
    paths: &Paths,
    exe: &Path,
    version: &str,
    hostname: Option<&str>,
    port: Option<u16>,
    tls: bool,
) -> Result<ServerClientInfo, String> {
    start_service_with(paths, exe, version, Some((hostname, port, tls))).await
}

async fn start_service_with(
    paths: &Paths,
    exe: &Path,
    version: &str,
    web: Option<(Option<&str>, Option<u16>, bool)>,
) -> Result<ServerClientInfo, String> {
    let existing = checked_registration(paths)?;
    let password = crate::password(paths)?;
    let me = ClientBuild::current(version);
    let mut notice = None;
    let mut launch_bind = web.map(|(host, port, _)| (host.map(str::to_owned), port));
    if let Some(reg) = existing
        && let Some(probe) = probe_registration(&reg).await
        && probe.serves(&reg)
    {
        if let Some((host, port, tls)) = web {
            check_web_listener(&reg, host, port, tls)?;
        }
        if !me.is_stale(&reg, &probe.version, probe.build.as_deref()) {
            return Ok(ServerClientInfo {
                registration: reg,
                password,
                notice: None,
            });
        }
        if !replaceable(&me, &reg, &password).await {
            if web.is_some() {
                return Err("the registered server runs an older build and cannot be replaced safely; finish its work, run `cyber service stop`, then `cyber web`".into());
            }
            return Ok(ServerClientInfo {
                registration: reg,
                password,
                notice: Some(ServiceNotice::StaleKept),
            });
        }
        if let Some(bind) = &mut launch_bind {
            preserve_web_bind(bind, &reg);
        }
        stop_service(paths).await?;
        notice = Some(ServiceNotice::Restarted);
    }
    let log_dir = paths.state.join("log");
    let _ = std::fs::create_dir_all(&log_dir);
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_dir.join("server.log"))
        .map_err(|e| e.to_string())?;
    let mut cmd = std::process::Command::new(exe);
    cmd.args(["serve", "--register"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(log));
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP: no console and no console signals,
        // like the Unix process group above.
        cmd.creation_flags(0x0000_0008 | 0x0000_0200);
        windows_std::disinherit_std_handles();
    }
    if let Some((hostname, port)) = launch_bind {
        if let Some(hostname) = hostname {
            cmd.args(["--hostname", &hostname]);
        }
        if let Some(port) = port {
            cmd.args(["--port", &port.to_string()]);
        }
    }
    cmd.spawn()
        .map_err(|e| format!("cannot start the server: {e}"))?;
    if let Some(registration) = poll_ready(|| async {
        let registration = read_registration(paths)?;
        registered_health(&registration, version)
            .await
            .then_some(registration)
    })
    .await
    {
        return Ok(ServerClientInfo {
            registration,
            password,
            notice,
        });
    }
    Err(format!(
        "the server did not become healthy; see {}",
        log_dir.join("server.log").display()
    ))
}

fn preserve_web_bind(bind: &mut (Option<String>, Option<u16>), reg: &Registration) {
    let Ok(url) = url::Url::parse(&reg.url) else {
        return;
    };
    if bind.0.is_none() {
        bind.0 = reg.bind_host.clone().or_else(|| {
            url.host_str()
                .map(|h| h.trim_matches(['[', ']']).to_owned())
        });
    }
    if bind.1.is_none() {
        bind.1 = url.port_or_known_default();
    }
}

fn check_web_listener(
    reg: &Registration,
    hostname: Option<&str>,
    port: Option<u16>,
    tls: bool,
) -> Result<(), String> {
    let url = url::Url::parse(&reg.url).map_err(|_| "the registered server has no dashboard TCP URL; stop it with `cyber service stop` before starting `cyber web`")?;
    let actual_host = url.host_str().unwrap_or("").trim_matches(['[', ']']);
    let bind_host = reg.bind_host.as_deref().unwrap_or(actual_host);
    let host_matches =
        hostname.is_none_or(|h| h.trim_matches(['[', ']']) == bind_host.trim_matches(['[', ']']));
    if !host_matches
        || port.is_some_and(|p| Some(p) != url.port_or_known_default())
        || (url.scheme() == "https") != tls
    {
        return Err("a healthy server already uses different listener settings; stop it with `cyber service stop`, then run `cyber web` with the requested settings".into());
    }
    let local = actual_host == "localhost"
        || actual_host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback());
    if url.scheme() != "https" && !local {
        return Err("the registered listener is not safe for browser credentials: use HTTPS or a loopback SSH forward".into());
    }
    Ok(())
}

/// Whether a stale registered server may be stopped: it runs this install's executable
/// and `GET /service` reports its identity and no running Turns or workflow runs. A
/// server that cannot answer (an older build without the route) is kept.
async fn replaceable(me: &ClientBuild, reg: &Registration, password: &str) -> bool {
    if !me.same_install(reg) {
        return false;
    }
    let Ok((client, url)) = server_client(reg) else {
        return false;
    };
    let Ok(res) = client
        .get(format!("{url}/api/v1/service"))
        .basic_auth("cyber", Some(password.trim()))
        .send()
        .await
    else {
        return false;
    };
    if !res.status().is_success() {
        return false;
    }
    let Ok(info) = res.json::<serde_json::Value>().await else {
        return false;
    };
    info["id"] == reg.id.as_str()
        && info["running_sessions"] == 0
        && info["active_workflow_runs"] == 0
}

/// A client for the registered server's TCP listener, or its Unix socket when it has no
/// TCP URL, with the URL to address it by.
fn server_client(reg: &Registration) -> Result<(reqwest::Client, String), String> {
    #[allow(unused_mut)]
    let mut builder = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy();
    #[allow(unused_mut)]
    let mut url = reg.url.clone();
    #[cfg(unix)]
    if reg
        .socket
        .as_ref()
        .is_some_and(|path| std::path::Path::new(path).exists())
        || url.is_empty()
    {
        let socket = reg
            .socket
            .as_ref()
            .ok_or("registration has no server listener")?;
        builder = builder.unix_socket(socket.as_str());
        url = "http://localhost".into();
    }
    if url.starts_with("https://")
        && let Some(cert) = &reg.tls_cert
    {
        let pem = std::fs::read(cert)
            .map_err(|e| format!("cannot read registered owner certificate: {e}"))?;
        for cert in reqwest::Certificate::from_pem_bundle(&pem).map_err(|e| e.to_string())? {
            builder = builder.add_root_certificate(cert);
        }
    }
    Ok((builder.build().map_err(|e| e.to_string())?, url))
}

async fn poll_ready<T, F, Fut>(mut check: F) -> Option<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Option<T>>,
{
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(ready) = check().await {
                return ready;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .ok()
}

/// Ask the registered server to stop and wait for it to unregister.
pub async fn stop_service(paths: &Paths) -> Result<Option<Registration>, String> {
    let Some(reg) = checked_registration(paths)? else {
        return Ok(None);
    };
    let (client, url) = server_client(&reg)?;
    let password = std::fs::read_to_string(paths.state.join("password"))
        .map_err(|e| format!("cannot read server credentials: {e}"))?;
    let response = client
        .post(format!("{url}/api/v1/service/stop"))
        .basic_auth("cyber", Some(password.trim()))
        .json(&serde_json::json!({"id": reg.id}))
        .send()
        .await
        .map_err(|e| format!("cannot request server shutdown: {e}"))?;
    if !response.status().is_success() {
        return Err(format!("server shutdown refused ({})", response.status()));
    }
    let accepted: serde_json::Value = response
        .json()
        .await
        .map_err(|e| format!("invalid shutdown response: {e}"))?;
    if accepted["id"] != reg.id || accepted["stopping"] != true {
        return Err("server shutdown response did not match the registration".into());
    }
    for _ in 0..100 {
        if read_registration(paths).is_none_or(|r| r.id != reg.id) {
            return Ok(Some(reg));
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    Err(format!("server {} (pid {}) did not stop", reg.id, reg.pid))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::Instant;

    #[test]
    fn idle_web_upgrade_preserves_registered_hostname_and_port() {
        let mut reg = Registration::current(
            "fixture".into(),
            "https://dashboard.example:8443".into(),
            None,
        );
        reg.bind_host = Some("0.0.0.0".into());
        let mut bind = (None, None);
        preserve_web_bind(&mut bind, &reg);
        assert_eq!(bind, (Some("0.0.0.0".into()), Some(8443)));
        let mut explicit = (Some("dashboard.example".into()), Some(8443));
        preserve_web_bind(&mut explicit, &reg);
        assert_eq!(explicit, (Some("dashboard.example".into()), Some(8443)));
    }

    #[test]
    fn web_reuse_never_silently_changes_bind_scheme_or_port() {
        let mut reg = Registration::current("fixture".into(), "http://127.0.0.1:4747".into(), None);
        reg.bind_host = Some("127.0.0.1".into());
        assert!(check_web_listener(&reg, None, None, false).is_ok());
        assert!(check_web_listener(&reg, Some("0.0.0.0"), None, false).is_err());
        assert!(check_web_listener(&reg, None, Some(4748), false).is_err());
        assert!(check_web_listener(&reg, None, None, true).is_err());
        reg.url = "https://127.0.0.1:4747".into();
        reg.bind_host = Some("0.0.0.0".into());
        assert!(check_web_listener(&reg, Some("0.0.0.0"), Some(4747), true).is_ok());
        assert!(check_web_listener(&reg, Some("127.0.0.1"), None, true).is_err());
        reg.url = "http://192.0.2.1:4747".into();
        assert!(check_web_listener(&reg, None, None, false).is_err());
        reg.url.clear();
        assert!(check_web_listener(&reg, None, None, false).is_err());
    }

    #[tokio::test(start_paused = true)]
    async fn startup_observes_a_ready_service_without_a_fifty_ms_delay() {
        let start = Instant::now();
        let ready_at = start + Duration::from_millis(20);
        let result =
            poll_ready(|| async { (Instant::now() >= ready_at).then_some("healthy") }).await;
        assert_eq!(result, Some("healthy"));
        assert!(start.elapsed() < Duration::from_millis(50));
    }

    #[tokio::test(start_paused = true)]
    async fn startup_deadline_includes_a_stalled_health_check() {
        let start = Instant::now();
        let result: Option<()> = poll_ready(|| async {
            tokio::time::sleep(Duration::from_secs(20)).await;
            Some(())
        })
        .await;
        assert_eq!(result, None);
        assert_eq!(start.elapsed(), Duration::from_secs(5));
    }
}

/// Windows `CreateProcess` gives a child every inheritable handle of its parent, so a
/// long-lived server started from a CLI whose stdout is a pipe would keep that pipe open,
/// and the CLI's caller (`$(cyber ...)`, a test's `output()`) would never see EOF. The CLI's
/// own standard handles are made non-inheritable before spawning the server; a later
/// `Stdio::inherit` child still works, because std duplicates an inheritable copy for it.
#[cfg(windows)]
mod windows_std {
    use windows_sys::Win32::Foundation::{
        HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE, SetHandleInformation,
    };
    use windows_sys::Win32::System::Console::{
        GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
    };

    #[allow(unsafe_code)]
    pub(super) fn disinherit_std_handles() {
        for id in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
            // SAFETY: GetStdHandle has no preconditions; SetHandleInformation only changes
            // the inherit flag of a handle this process owns and fails harmlessly otherwise.
            unsafe {
                let handle = GetStdHandle(id);
                if !handle.is_null() && handle != INVALID_HANDLE_VALUE {
                    SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0);
                }
            }
        }
    }
}

#[cfg(all(test, unix))]
mod socket_registration_tests {
    use super::*;
    fn paths(root: &Path) -> Paths {
        Paths {
            state: root.join("state"),
            data: root.join("data"),
            config: root.join("config"),
            cache: root.join("cache"),
            tmp: root.join("tmp"),
        }
    }
    #[test]
    fn registered_unix_sockets_require_the_owned_state_boundary() {
        let temporary = tempfile::tempdir().unwrap();
        let paths = paths(temporary.path());
        paths.ensure().unwrap();
        std::fs::create_dir_all(paths.state.join("nested")).unwrap();
        let save = |socket: PathBuf| {
            let registration = Registration::current(
                "srv_fixture".into(),
                "http://127.0.0.1:4747".into(),
                Some(socket.display().to_string()),
            );
            write_private(
                &registration_path(&paths),
                &serde_json::to_string(&registration).unwrap(),
            )
            .unwrap();
        };
        save(paths.state.join("nested/owner.sock"));
        assert!(read_registration(&paths).is_some());
        save(temporary.path().join("outside.sock"));
        assert!(read_registration(&paths).is_none());
        std::os::unix::fs::symlink(temporary.path(), paths.state.join("escape")).unwrap();
        save(paths.state.join("escape/owner.sock"));
        assert!(read_registration(&paths).is_none());
    }
    #[tokio::test]
    async fn unsafe_legacy_socket_registration_never_connects_or_sends_owner_credentials() {
        let temporary = tempfile::tempdir().unwrap();
        let paths = paths(temporary.path());
        paths.ensure().unwrap();
        let workspace = temporary.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let socket = workspace.join("legacy-owner.sock");
        let trap = tokio::net::UnixListener::bind(&socket).unwrap();
        let registration = Registration::current(
            "srv_legacy".into(),
            String::new(),
            Some(socket.display().to_string()),
        );
        write_private(
            &registration_path(&paths),
            &serde_json::to_string(&registration).unwrap(),
        )
        .unwrap();
        write_private(
            &paths.state.join("password"),
            "synthetic-owner-credential-never-send",
        )
        .unwrap();
        assert!(read_registration(&paths).is_none());
        let stopped = stop_service(&paths).await.unwrap_err();
        assert!(stopped.contains("unsafe Unix socket"));
        assert!(
            crate::registered_lsp_status(&paths, &workspace)
                .await
                .is_err()
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(100), trap.accept())
                .await
                .is_err(),
            "unsafe legacy socket received an automatic client connection"
        );
        assert!(
            registration_path(&paths).is_file(),
            "preserve the rejected receipt for explicit owner cleanup"
        );
    }
}

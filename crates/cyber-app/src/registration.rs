//! `server.json` registration and background service management
//! (`server-api` → Background service management).

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
}

/// What a client needs to talk to the registered server.
#[derive(Debug, Clone)]
pub struct ServerClientInfo {
    pub registration: Registration,
    pub password: String,
}

pub fn registration_path(paths: &Paths) -> PathBuf {
    paths.state.join("server.json")
}

pub fn read_registration(paths: &Paths) -> Option<Registration> {
    let text = std::fs::read_to_string(registration_path(paths)).ok()?;
    serde_json::from_str(&text).ok()
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
    let Ok(client) = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
    else {
        return false;
    };
    let Ok(res) = client.get(format!("{url}/api/v1/health")).send().await else {
        return false;
    };
    let Ok(body) = res.json::<serde_json::Value>().await else {
        return false;
    };
    body["healthy"] == true && body["version"] == version
}

/// Reuse a healthy registered server, or spawn `<exe> serve --register` detached and wait
/// (every 50 ms, up to 100 times) for its registration.
pub async fn start_service(
    paths: &Paths,
    exe: &Path,
    version: &str,
) -> Result<ServerClientInfo, String> {
    let password = crate::password(paths)?;
    if let Some(reg) = read_registration(paths)
        && health(&reg.url, version).await
    {
        return Ok(ServerClientInfo {
            registration: reg,
            password,
        });
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
    cmd.spawn()
        .map_err(|e| format!("cannot start the server: {e}"))?;
    for _ in 0..100 {
        tokio::time::sleep(Duration::from_millis(50)).await;
        if let Some(reg) = read_registration(paths)
            && health(&reg.url, version).await
        {
            return Ok(ServerClientInfo {
                registration: reg,
                password,
            });
        }
    }
    Err(format!(
        "the server did not become healthy; see {}",
        log_dir.join("server.log").display()
    ))
}

/// Ask the registered server to stop and wait for it to unregister.
pub async fn stop_service(paths: &Paths) -> Result<Option<Registration>, String> {
    let Some(reg) = read_registration(paths) else {
        return Ok(None);
    };
    let status = std::process::Command::new("kill")
        .arg(reg.pid.to_string())
        .status()
        .map_err(|e| e.to_string())?;
    if !status.success() {
        // The process is gone; clear the stale registration.
        let _ = std::fs::remove_file(registration_path(paths));
        return Ok(Some(reg));
    }
    for _ in 0..100 {
        if read_registration(paths).is_none_or(|r| r.id != reg.id) {
            return Ok(Some(reg));
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    Err(format!("server {} (pid {}) did not stop", reg.id, reg.pid))
}

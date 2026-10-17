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
/// for its registration, checking promptly within a five-second deadline.
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
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP: no console and no console signals,
        // like the Unix process group above.
        cmd.creation_flags(0x0000_0008 | 0x0000_0200);
        windows_std::disinherit_std_handles();
    }
    cmd.spawn()
        .map_err(|e| format!("cannot start the server: {e}"))?;
    if let Some(registration) = poll_ready(|| async {
        let registration = read_registration(paths)?;
        health(&registration.url, version)
            .await
            .then_some(registration)
    })
    .await
    {
        return Ok(ServerClientInfo {
            registration,
            password,
        });
    }
    Err(format!(
        "the server did not become healthy; see {}",
        log_dir.join("server.log").display()
    ))
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
    let Some(reg) = read_registration(paths) else {
        return Ok(None);
    };
    #[allow(unused_mut)]
    let mut builder = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy();
    #[allow(unused_mut)]
    let mut url = reg.url.clone();
    #[cfg(unix)]
    if url.is_empty() {
        let socket = reg
            .socket
            .as_ref()
            .ok_or("registration has no server listener")?;
        builder = builder.unix_socket(socket.as_str());
        url = "http://localhost".into();
    }
    let password = std::fs::read_to_string(paths.state.join("password"))
        .map_err(|e| format!("cannot read server credentials: {e}"))?;
    let response = builder
        .build()
        .map_err(|e| e.to_string())?
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

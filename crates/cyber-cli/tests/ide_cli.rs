//! `cyber ide token create|list|revoke` through the real binary and background server
//! (`server-api` → Local IDE token issuance, Local IDE token revocation and expiry;
//! `cli-commands` → Resource command groups, "IDE token subcommands").

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde_json::{Value, json};

struct Home {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

impl Home {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join("cyber-home/config")).unwrap();
        std::fs::create_dir_all(root.join("repo/.git")).unwrap();
        // Sessions need a model; these tests never call it.
        let local = json!({ "api": { "type": "openai-compatible", "url": "http://127.0.0.1:9/v1" },
            "models": { "main": {} } });
        std::fs::write(
            root.join("cyber-home/config/cyber.jsonc"),
            json!({ "providers": { "local": local }, "model": "local/main" }).to_string(),
        )
        .unwrap();
        Self { _dir: dir, root }
    }

    fn cyber(&self, args: &[&str]) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_cyber"));
        command
            .args(args)
            .env_clear()
            .env("HOME", self.root.join("home"))
            .env("CYBER_HOME", self.root.join("cyber-home"))
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .current_dir(self.root.join("repo"))
            .stdin(Stdio::null());
        #[cfg(windows)]
        if let Some(system) = std::env::var_os("SystemRoot") {
            command.env("SystemRoot", system);
        }
        command.output().unwrap()
    }

    fn state(&self) -> PathBuf {
        self.root.join("cyber-home/state")
    }

    fn token_file(&self) -> PathBuf {
        self.state().join("ide/vscode.token")
    }

    /// The token line of the file; the second line is `expires_at=<RFC 3339>`.
    fn token(&self) -> String {
        let text = std::fs::read_to_string(self.token_file()).unwrap();
        let mut lines = text.lines();
        let token = lines.next().unwrap().to_string();
        assert!(lines.next().unwrap().starts_with("expires_at=20"), "{text}");
        token
    }

    /// The registered server's base URL.
    fn url(&self) -> String {
        let registration: Value =
            serde_json::from_slice(&std::fs::read(self.state().join("server.json")).unwrap())
                .unwrap();
        registration["url"].as_str().unwrap().to_string()
    }

    /// The status of `GET /api/v1/sessions` with `token` as the bearer credential.
    fn status_with(&self, token: &str) -> u16 {
        let url = format!("{}/api/v1/sessions", self.url());
        let token = token.to_string();
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(async move {
                reqwest::Client::builder()
                    .no_proxy()
                    .build()
                    .unwrap()
                    .get(url)
                    .bearer_auth(token)
                    .send()
                    .await
                    .unwrap()
                    .status()
                    .as_u16()
            })
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = self.cyber(&["service", "stop"]);
    }
}

fn exits(output: &Output, code: i32) -> (String, String) {
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert_eq!(output.status.code(), Some(code), "{stdout}{stderr}");
    (stdout, stderr)
}

fn list(home: &Home) -> Vec<Value> {
    let (stdout, _) = exits(
        &home.cyber(&["ide", "token", "list", "--format", "json"]),
        0,
    );
    serde_json::from_str::<Value>(&stdout)
        .unwrap()
        .as_array()
        .unwrap()
        .clone()
}

/// Every file under `dir`, recursively, except `skip`.
fn files(dir: &Path, skip: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path == skip {
            continue;
        }
        if path.is_dir() {
            files(&path, skip, out);
        } else {
            out.push(path);
        }
    }
}

/// "Issue a token" (CLI), "Reissue rotates", "IDE token subcommands", revocation removing
/// the file, and "Password replacement revokes IDE tokens".
#[test]
fn tokens_are_created_listed_rotated_and_revoked() {
    let home = Home::new();
    exits(&home.cyber(&["service", "start"]), 0);
    let token = create_writes_only_the_file(&home);
    let records = listed_without_secrets(&home, &token);
    let rotated = reissue_rotates(&home, &token);
    revoke_removes_the_current_file(&home, records[0]["id"].as_str().unwrap(), &rotated);
    password_replacement_revokes_all(&home);
}

/// `create` prints the ID and expiry only; the file is private and the only place the
/// value exists (the database, its WAL and the logs hold none).
fn create_writes_only_the_file(home: &Home) -> String {
    let created = home.cyber(&["ide", "token", "create", "--client", "vscode"]);
    let (stdout, stderr) = exits(&created, 0);
    let token = home.token();
    assert!(token.starts_with("cyi_") && token.len() == 47, "{token}");
    assert!(
        stdout.contains("itk_") && stdout.contains("expires"),
        "{stdout}"
    );
    assert!(!stdout.contains(&token) && !stderr.contains(&token));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&home.token_file()), 0o600);
        assert_eq!(mode(home.token_file().parent().unwrap()), 0o700);
    }
    let mut all = Vec::new();
    files(&home.root.join("cyber-home"), &home.token_file(), &mut all);
    for file in all {
        let bytes = std::fs::read(&file).unwrap_or_default();
        assert!(
            !String::from_utf8_lossy(&bytes).contains(&token),
            "{} holds the token",
            file.display()
        );
    }
    token
}

/// `list --format json`: the record fields, a 30-day expiry and no token value.
fn listed_without_secrets(home: &Home, token: &str) -> Vec<Value> {
    let records = list(home);
    assert_eq!(records.len(), 1);
    for field in [
        "id",
        "client",
        "name",
        "created_at",
        "expires_at",
        "last_used_at",
        "revoked_at",
    ] {
        assert!(records[0].get(field).is_some(), "{field}");
    }
    assert!(!records[0].to_string().contains(token));
    let expires =
        chrono::DateTime::parse_from_rfc3339(records[0]["expires_at"].as_str().unwrap()).unwrap();
    let days = (expires.with_timezone(&chrono::Utc) - chrono::Utc::now()).num_days();
    assert!((29..=30).contains(&days), "{days}");
    assert_eq!(home.status_with(token), 200);
    records
}

/// Reissue rotates: the old token is refused, the file holds the new one.
fn reissue_rotates(home: &Home, token: &str) -> String {
    let args = [
        "ide",
        "token",
        "create",
        "--client",
        "vscode",
        "--expires",
        "7",
    ];
    exits(&home.cyber(&args), 0);
    let rotated = home.token();
    assert_ne!(rotated, token);
    assert_eq!(home.status_with(token), 401);
    assert_eq!(home.status_with(&rotated), 200);
    rotated
}

/// Revoking an old token keeps the file; revoking the current one removes it; revoking
/// again succeeds and an unknown ID is a usage error.
fn revoke_removes_the_current_file(home: &Home, old: &str, rotated: &str) {
    exits(&home.cyber(&["ide", "token", "revoke", old]), 0);
    assert!(home.token_file().exists());
    let current = list(home)
        .into_iter()
        .find(|record| record["status"] == "active")
        .unwrap();
    let current = current["id"].as_str().unwrap().to_string();
    let (stdout, _) = exits(&home.cyber(&["ide", "token", "revoke", &current]), 0);
    assert!(stdout.contains("Removed"), "{stdout}");
    assert!(!home.token_file().exists());
    assert_eq!(home.status_with(rotated), 401);
    exits(&home.cyber(&["ide", "token", "revoke", &current]), 0);
    exits(&home.cyber(&["ide", "token", "revoke", "itk_missing"]), 2);
}

/// Replacing the owner password revokes every token and restarts the server.
fn password_replacement_revokes_all(home: &Home) {
    exits(
        &home.cyber(&["ide", "token", "create", "--client", "vscode"]),
        0,
    );
    let last = home.token();
    assert_eq!(home.status_with(&last), 200);
    let replace = ["service", "password", "replacement-owner-password-1"];
    exits(&home.cyber(&replace), 0);
    assert!(!home.token_file().exists());
    assert_eq!(home.status_with(&last), 401);
    assert!(
        list(home)
            .iter()
            .all(|record| record["status"] == "revoked")
    );
}

#[test]
fn invalid_arguments_are_refused_before_any_request() {
    let home = Home::new();
    for args in [
        vec!["ide", "token", "create", "--client", "jetbrains"],
        vec![
            "ide",
            "token",
            "create",
            "--client",
            "vscode",
            "--expires",
            "0",
        ],
        vec![
            "ide",
            "token",
            "create",
            "--client",
            "vscode",
            "--expires",
            "91",
        ],
        vec!["ide", "token", "create"],
    ] {
        exits(&home.cyber(&args), 2);
    }
    assert!(!home.state().join("server.json").exists());
}

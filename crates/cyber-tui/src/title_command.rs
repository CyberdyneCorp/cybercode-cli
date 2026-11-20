//! `tui.title.command` (`tui` → Rendering modes and terminal integration): a command from
//! the user's global `tui.jsonc` that reads Session state as JSON on stdin and prints the
//! terminal window title. It runs in the background, at most once every 300 ms and only
//! when the state changed, for at most one second; a failure, a timeout or empty output
//! keeps the built-in title.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::model::Session;

/// The least time between two starts of the command.
pub const INTERVAL: Duration = Duration::from_millis(300);
/// How long one run may take before it is killed.
pub const TIMEOUT: Duration = Duration::from_secs(1);
/// Stdout read from one run; the rest is ignored.
const MAX_OUTPUT: u64 = 4096;
/// The longest title shown, in characters.
const MAX_TITLE: usize = 200;

/// The configured command and the title its last successful run printed.
#[derive(Debug, Default, Clone)]
pub struct TitleCommand {
    command: Option<String>,
    input: Option<String>,
    started: Option<Instant>,
    running: Arc<Mutex<bool>>,
    title: Arc<Mutex<Option<String>>>,
}

impl TitleCommand {
    pub fn new(command: Option<String>) -> Self {
        Self {
            command,
            ..Self::default()
        }
    }

    /// The title the command printed last; `None` keeps the built-in title.
    pub fn title(&self) -> Option<String> {
        self.title
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Start the command for `session` when one is configured, none is running, the
    /// state changed since the last start and [`INTERVAL`] has passed.
    pub fn poll(&mut self, session: &Session, now: Instant) {
        let Some(command) = self.command.clone() else {
            return;
        };
        let input = state(session).to_string();
        let due = self
            .started
            .is_none_or(|started| now.duration_since(started) >= INTERVAL);
        if !due || self.input.as_ref() == Some(&input) || *self.lock_running() {
            return;
        }
        *self.lock_running() = true;
        self.input = Some(input.clone());
        self.started = Some(now);
        let (running, title) = (Arc::clone(&self.running), Arc::clone(&self.title));
        let directory = session.directory.clone();
        tokio::spawn(async move {
            if let Some(printed) = run(&command, &directory, &input).await {
                *title.lock().unwrap_or_else(PoisonError::into_inner) = printed;
            }
            *running.lock().unwrap_or_else(PoisonError::into_inner) = false;
        });
    }

    fn lock_running(&self) -> std::sync::MutexGuard<'_, bool> {
        self.running.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The JSON the command reads on stdin.
pub fn state(session: &Session) -> Value {
    json!({
        "session_id": session.id,
        "title": session.title,
        "status": if session.running { "running" } else { "idle" },
        "directory": session.directory,
        "model": session.model,
        "agent": session.agent,
        "mode": session.mode,
        "cost_usd": session.cost,
        "input_tokens": session.input_tokens,
        "output_tokens": session.output_tokens,
    })
}

/// Run `command` once in `directory` with `input` on stdin. `None` when it could not
/// start, failed or timed out; `Some(None)` when it printed nothing usable.
pub async fn run(command: &str, directory: &str, input: &str) -> Option<Option<String>> {
    let mut child = shell(command)
        .current_dir(directory)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .ok()?;
    let mut stdin = child.stdin.take()?;
    let mut stdout = child.stdout.take()?;
    let work = async {
        // A command that ignores its input closes the pipe; that is not a failure.
        let _ = stdin.write_all(input.as_bytes()).await;
        drop(stdin);
        let mut bytes = Vec::new();
        (&mut stdout)
            .take(MAX_OUTPUT)
            .read_to_end(&mut bytes)
            .await
            .ok()?;
        let status = child.wait().await.ok()?;
        status.success().then_some(bytes)
    };
    let bytes = tokio::time::timeout(TIMEOUT, work).await.ok()??;
    Some(clean(&String::from_utf8_lossy(&bytes)))
}

/// The first non-empty line of `output`, without control characters (the title goes into
/// an OSC sequence a BEL or ESC would end), cut to [`MAX_TITLE`] characters.
pub fn clean(output: &str) -> Option<String> {
    let line: String = crate::sidebar::clean(output)
        .chars()
        .take(MAX_TITLE)
        .collect();
    let line = line.trim();
    (!line.is_empty()).then(|| line.to_owned())
}

#[cfg(unix)]
fn shell(command: &str) -> tokio::process::Command {
    let mut shell = tokio::process::Command::new("/bin/sh");
    shell.arg("-c").arg(command);
    shell
}

#[cfg(windows)]
fn shell(command: &str) -> tokio::process::Command {
    let mut shell = tokio::process::Command::new("cmd");
    shell.arg("/C").arg(command);
    shell
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn session() -> Session {
        Session {
            id: "ses_1".into(),
            title: "Fix login".into(),
            directory: std::env::temp_dir().display().to_string(),
            model: "test/main".into(),
            ..Session::default()
        }
    }

    #[tokio::test]
    async fn the_command_reads_state_on_stdin_and_its_first_line_is_the_title() {
        let input = state(&session()).to_string();
        let command = r#"read -r state; case "$state" in *'"title":"Fix login"'*) printf '\n  login fix \033]52;c;eA==\007 · ok\nignored\n';; esac"#;
        let title = run(command, &session().directory, &input).await;
        assert_eq!(title, Some(Some("login fix ]52;c;eA== · ok".into())));
    }

    #[tokio::test]
    async fn failure_timeout_and_empty_output_keep_the_built_in_title() {
        let input = state(&session()).to_string();
        let directory = session().directory;
        assert_eq!(run("echo partial; exit 3", &directory, &input).await, None);
        let started = Instant::now();
        assert_eq!(run("sleep 5; echo late", &directory, &input).await, None);
        assert!(started.elapsed() < Duration::from_secs(3));
        assert_eq!(run("true", &directory, &input).await, Some(None));
    }

    #[tokio::test]
    async fn poll_runs_only_when_the_state_changed_and_not_more_often_than_the_interval() {
        let mut command = TitleCommand::new(Some("cat >/dev/null; echo custom".into()));
        let mut session = session();
        let start = Instant::now();
        command.poll(&session, start);
        for _ in 0..200 {
            if command.title().is_some() && !*command.lock_running() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(command.title().as_deref(), Some("custom"));
        let first = command.started;
        command.poll(&session, start + INTERVAL * 2);
        assert_eq!(command.started, first, "unchanged state does not rerun");
        session.running = true;
        command.poll(&session, start + Duration::from_millis(10));
        assert_eq!(command.started, first, "too soon after the last start");
        command.poll(&session, start + INTERVAL);
        assert_eq!(command.started, Some(start + INTERVAL));
    }

    #[test]
    fn no_command_never_runs() {
        let mut command = TitleCommand::new(None);
        command.poll(&session(), Instant::now());
        assert!(command.started.is_none() && command.title().is_none());
    }
}

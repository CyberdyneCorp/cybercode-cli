//! `/login` and `/logout` (`tui` → Provider login commands): sign in to a ChatGPT
//! subscription through the server's provider login API. The dialog shows the
//! authorization URL, polls the attempt once a second and takes a pasted redirect URL
//! when the server cannot receive the browser's redirect.

use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde_json::{Value, json};

use cyber_client::Client;

use crate::app::{Action, App, Overlay, PickerKind};
use crate::model::Choice;
use crate::perform::{Msg, err};

/// The provider the API signs in to; `/login openai` and `/login chatgpt` name it.
const PROVIDER: &str = "chatgpt";
const POLL_EVERY: Duration = Duration::from_secs(1);

/// What `/login` and `/logout` ask the server.
#[derive(Debug, Clone, PartialEq)]
pub enum LoginRequest {
    Start,
    Poll(String),
    Code {
        attempt_id: String,
        redirect_url: String,
    },
    Cancel(String),
    Logout,
}

/// The server's answer.
#[derive(Debug, Clone, PartialEq)]
pub enum LoginReply {
    /// The attempt as the API returned it.
    Attempt(Value),
    LoggedOut(bool),
    Failed(String),
}

/// The open login dialog.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LoginDialog {
    pub attempt_id: Option<String>,
    pub url: String,
    /// The server waits for the browser's redirect.
    pub callback: bool,
    /// The pasted redirect URL.
    pub input: String,
    pub error: Option<String>,
    /// A request is in flight; polling waits for it.
    busy: bool,
    poll_due: Option<Instant>,
}

/// How a provider named after `/login` signs in.
enum Target {
    ChatGpt,
    Google,
    Other,
}

fn target(name: &str) -> Target {
    match name.to_ascii_lowercase().as_str() {
        "" | "openai" | "chatgpt" => Target::ChatGpt,
        "google" | "gemini" => Target::Google,
        _ => Target::Other,
    }
}

impl App {
    /// `/login` lists the subscriptions; `/login openai` starts signing in.
    pub(crate) fn login_command(&mut self, args: &str) -> Vec<Action> {
        if args.is_empty() {
            let choices = vec![Choice {
                key: "openai".into(),
                label: "openai".into(),
                detail: "ChatGPT Plus or Pro subscription".into(),
            }];
            self.open_picker(PickerKind::Login, "Sign in · Enter starts", choices);
            return Vec::new();
        }
        match target(args) {
            Target::ChatGpt => self.start_login(),
            target => {
                self.toast(refusal(target, args));
                Vec::new()
            }
        }
    }

    /// `/logout [openai]`.
    pub(crate) fn logout_command(&mut self, args: &str) -> Vec<Action> {
        match target(args) {
            Target::ChatGpt => vec![Action::Login(LoginRequest::Logout)],
            _ => {
                self.toast(format!("No subscription login for {args}"));
                Vec::new()
            }
        }
    }

    pub(crate) fn start_login(&mut self) -> Vec<Action> {
        self.login = LoginDialog {
            busy: true,
            ..LoginDialog::default()
        };
        self.overlay = Overlay::Login;
        vec![Action::Login(LoginRequest::Start)]
    }

    /// A poll of the open attempt, at most once a second and one at a time.
    pub(crate) fn login_poll(&mut self) -> Vec<Action> {
        let now = self.now();
        let dialog = &mut self.login;
        let Some(id) = dialog.attempt_id.clone() else {
            return Vec::new();
        };
        let due = dialog.poll_due.is_none_or(|due| now >= due);
        if self.overlay != Overlay::Login || dialog.busy || !due {
            return Vec::new();
        }
        dialog.busy = true;
        dialog.poll_due = Some(now + POLL_EVERY);
        vec![Action::Login(LoginRequest::Poll(id))]
    }

    /// Keys of the dialog: type or paste the redirect URL, Enter submits it, Ctrl+Y copies
    /// the authorization URL, Esc cancels the attempt.
    pub(crate) fn login_key(&mut self, key: KeyEvent) -> Vec<Action> {
        let dialog = &mut self.login;
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => return self.close_login(),
            KeyCode::Enter => return self.submit_login_code(),
            KeyCode::Char('y') if control && !dialog.url.is_empty() => {
                return vec![Action::Copy(dialog.url.clone())];
            }
            KeyCode::Char('u') if control => dialog.input.clear(),
            KeyCode::Backspace => {
                dialog.input.pop();
            }
            KeyCode::Char(c) if !control => dialog.input.push(c),
            _ => {}
        }
        Vec::new()
    }

    /// A paste goes into an open dialog's input, without whitespace.
    pub(crate) fn login_paste(&mut self, text: &str) -> bool {
        if self.overlay != Overlay::Login {
            return false;
        }
        self.login
            .input
            .extend(text.chars().filter(|c| !c.is_whitespace()));
        true
    }

    fn submit_login_code(&mut self) -> Vec<Action> {
        let dialog = &mut self.login;
        let (Some(attempt_id), false) = (dialog.attempt_id.clone(), dialog.input.is_empty()) else {
            return Vec::new();
        };
        dialog.busy = true;
        dialog.error = None;
        vec![Action::Login(LoginRequest::Code {
            attempt_id,
            redirect_url: std::mem::take(&mut dialog.input),
        })]
    }

    fn close_login(&mut self) -> Vec<Action> {
        self.overlay = Overlay::None;
        let dialog = std::mem::take(&mut self.login);
        dialog
            .attempt_id
            .map(|id| vec![Action::Login(LoginRequest::Cancel(id))])
            .unwrap_or_default()
    }

    /// The server answered; a finished attempt closes the dialog.
    pub(crate) fn login_reply(&mut self, reply: LoginReply) -> Vec<Action> {
        let open = self.overlay == Overlay::Login;
        self.login.busy = false;
        match reply {
            LoginReply::LoggedOut(true) => self.toast("Signed out of ChatGPT"),
            LoginReply::LoggedOut(false) => self.toast("Not signed in to ChatGPT"),
            LoginReply::Failed(error) if open => self.login.error = Some(error),
            LoginReply::Failed(error) => self.toast(error),
            LoginReply::Attempt(attempt) if open => return self.login_attempt(&attempt),
            LoginReply::Attempt(_) => {}
        }
        Vec::new()
    }

    fn login_attempt(&mut self, attempt: &Value) -> Vec<Action> {
        let id = attempt["attempt_id"].as_str().map(str::to_owned);
        if self.login.attempt_id.is_some() && self.login.attempt_id != id {
            return Vec::new();
        }
        match attempt["status"].as_str() {
            Some("complete") => {
                self.overlay = Overlay::None;
                self.login = LoginDialog::default();
                self.toast(signed_in(attempt));
                vec![Action::LoadModels]
            }
            Some("failed") => {
                self.overlay = Overlay::None;
                self.login = LoginDialog::default();
                let error = attempt["error"].as_str().unwrap_or("unknown error");
                self.toast(format!("ChatGPT sign-in failed: {error}"));
                Vec::new()
            }
            _ => {
                let dialog = &mut self.login;
                dialog.attempt_id = id;
                dialog.url = attempt["authorization_url"]
                    .as_str()
                    .unwrap_or_default()
                    .into();
                dialog.callback = attempt["callback"].as_bool().unwrap_or(false);
                Vec::new()
            }
        }
    }
}

fn refusal(target: Target, name: &str) -> String {
    match target {
        Target::Google => {
            "Google subscription login is not supported, and Gemini models await a native Gemini adapter"
                .into()
        }
        _ => format!(
            "No subscription login for {name}; set its API key in the environment or under providers in cyber.jsonc"
        ),
    }
}

fn signed_in(attempt: &Value) -> String {
    let email = attempt["email"].as_str().unwrap_or("unknown account");
    let plan = attempt["plan"].as_str().unwrap_or("unknown plan");
    let mut text = format!("Signed in to ChatGPT as {email} ({plan})");
    if let Some(warning) = attempt["warning"].as_str() {
        text.push_str(&format!(" · {warning}"));
    }
    text
}

/// Run a login request. Failures come back as [`LoginReply::Failed`], so an open dialog
/// shows them in place.
pub async fn perform(client: &Client, request: LoginRequest) -> Result<Msg, String> {
    let base = format!("/providers/{PROVIDER}/login");
    let answer = match request {
        LoginRequest::Start => client.post(&base, json!({})).await,
        LoginRequest::Poll(id) => client.get(&format!("{base}/{id}")).await,
        LoginRequest::Code {
            attempt_id,
            redirect_url,
        } => {
            let path = format!("{base}/{attempt_id}/code");
            client
                .post(&path, json!({ "redirect_url": redirect_url }))
                .await
        }
        LoginRequest::Cancel(id) => {
            let _ = client.delete(&format!("{base}/{id}")).await;
            return Ok(Msg::Done);
        }
        LoginRequest::Logout => {
            return Ok(Msg::Login(match client.delete(&base).await {
                Ok(body) => LoginReply::LoggedOut(body["data"]["removed"] == true),
                Err(error) => LoginReply::Failed(message(error)),
            }));
        }
    };
    Ok(Msg::Login(match answer {
        Ok(body) => LoginReply::Attempt(body["data"].clone()),
        Err(error) => LoginReply::Failed(message(error)),
    }))
}

/// The server's wording of a refusal, without its error tag.
fn message(error: cyber_client::ClientError) -> String {
    match error {
        cyber_client::ClientError::Api { message, .. } if !error.route_missing() => message,
        other => err(other),
    }
}

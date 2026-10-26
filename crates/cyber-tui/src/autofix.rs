//! `/autofix` controls use owner API operations, never prompt the model to publish.
use crate::{
    app::{Action, App},
    model::Session,
    perform::{Msg, err},
};
use cyber_client::Client;
use cyber_core::autofix::Config;
use serde_json::{Value, json};

#[derive(Debug, Clone, PartialEq)]
pub enum Request {
    Enable(Config),
    Control { session_id: String, action: String },
    Status { session_id: String },
}
fn enable(session: &Session, input: &str) -> Result<Config, String> {
    let mut value:Value=serde_json::from_str(input).map_err(|_|"Use /autofix on {JSON settings}; include repository, pr, checks, required_ci, max_tokens and max_seconds".to_owned())?;
    let map = value
        .as_object_mut()
        .ok_or("auto-fix settings must be a JSON object")?;
    if map
        .get("session_id")
        .is_some_and(|id| id.as_str() != Some(&session.id))
    {
        return Err("auto-fix must use the current Session".into());
    }
    map.insert("session_id".into(), session.id.clone().into());
    map.entry("model")
        .or_insert_with(|| session.model.clone().into());
    map.entry("max_attempts").or_insert(3.into());
    let config: Config = serde_json::from_value(value).map_err(|_| {
        "invalid auto-fix settings; explicit verification and resource limits are required"
            .to_owned()
    })?;
    config.validate()?;
    Ok(config)
}
pub fn parse(session: &Session, input: &str) -> Result<Request, String> {
    let (action, settings) = input.trim().split_once(' ').unwrap_or((input.trim(), ""));
    match action {
        "on" => Ok(Request::Enable(enable(session, settings.trim())?)),
        "" | "status" if settings.is_empty() => Ok(Request::Status {
            session_id: session.id.clone(),
        }),
        "off" | "pause" | "resume" if settings.is_empty() => Ok(Request::Control {
            session_id: session.id.clone(),
            action: if action == "off" { "disable" } else { action }.into(),
        }),
        _ => Err("Use /autofix on {JSON settings}, status, pause, resume or off".into()),
    }
}
impl App {
    pub(crate) fn autofix_command(&mut self, args: &str) -> Vec<Action> {
        match parse(&self.session, args) {
            Ok(request) => vec![Action::Autofix(request)],
            Err(message) => {
                self.toast(message);
                vec![]
            }
        }
    }
}
async fn selected(client: &Client, session_id: &str) -> Result<Value, String> {
    let response = client.get("/autofix/subscriptions").await.map_err(err)?;
    response["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|s| s["config"]["session_id"].as_str() == Some(session_id))
        .max_by_key(|s| {
            (
                !matches!(s["status"].as_str(), Some("disabled" | "closed")),
                s["created_ms"].as_i64().unwrap_or_default(),
            )
        })
        .cloned()
        .ok_or_else(|| {
            "This Session has no auto-fix subscription; enable one with /autofix on {JSON settings}"
                .into()
        })
}
fn summary(s: &Value) -> String {
    let attempt = s["attempts"]
        .as_array()
        .and_then(|a| a.last())
        .and_then(|a| a["stage"].as_str())
        .unwrap_or("no attempts");
    let reason = s["reason"]
        .as_str()
        .map(|s| format!(" · {s}"))
        .unwrap_or_default();
    format!(
        "Auto-fix {} PR #{} · {} · {attempt} · {} failures · cap {} tokens / {}s{reason}. Self-hosted server must remain available.",
        s["config"]["repository"].as_str().unwrap_or(""),
        s["config"]["pr"],
        s["status"].as_str().unwrap_or("unknown"),
        s["failures"],
        s["config"]["max_tokens"],
        s["config"]["max_seconds"]
    )
}
pub async fn perform(client: &Client, request: Request) -> Result<Msg, String> {
    let data = match request {
        Request::Enable(config) => client
            .post(
                "/autofix/subscriptions",
                serde_json::to_value(config).map_err(|e| e.to_string())?,
            )
            .await
            .map_err(err)?["data"]
            .clone(),
        Request::Status { session_id } => selected(client, &session_id).await?,
        Request::Control { session_id, action } => {
            let current = selected(client, &session_id).await?;
            let id = current["id"].as_str().ok_or("subscription has no ID")?;
            client
                .post(
                    &format!("/autofix/subscriptions/{id}/control"),
                    json!({"expected_revision":current["revision"],"action":action}),
                )
                .await
                .map_err(err)?["data"]
                .clone()
        }
    };
    Ok(Msg::Toast(summary(&data)))
}
#[cfg(test)]
mod tests {
    use super::*;
    fn session() -> Session {
        Session {
            id: "ses_current".into(),
            model: "test/main".into(),
            ..Session::default()
        }
    }
    #[test]
    fn enable_binds_current_session_and_preserves_explicit_argv() {
        let input = r#"on {"repository":"team/repo","pr":3,"checks":[["cargo","test","with spaces"]],"required_ci":["check:9:test"],"max_tokens":1000,"max_seconds":60}"#;
        let Request::Enable(config) = parse(&session(), input).unwrap() else {
            panic!("enable expected")
        };
        assert_eq!(config.session_id, "ses_current");
        assert_eq!(config.model, "test/main");
        assert_eq!(config.max_attempts, 3);
        assert_eq!(config.checks[0][2], "with spaces");
    }
    #[test]
    fn missing_limits_or_different_session_are_refused() {
        assert!(parse(&session(), "on {}").is_err());
        assert!(parse(&session(), r#"on {"session_id":"ses_other"}"#).is_err());
    }
    #[test]
    fn lifecycle_controls_do_not_become_model_prompts() {
        for action in ["off", "pause", "resume"] {
            assert!(matches!(
                parse(&session(), action),
                Ok(Request::Control { .. })
            ))
        }
        assert!(matches!(
            parse(&session(), "status"),
            Ok(Request::Status { .. })
        ));
        assert!(parse(&session(), "off unexpected").is_err());
    }
}

//! Native authority is read from the server; this state never creates a grant.
use crate::{app::Action, model::Session};
use serde_json::Value;
use std::time::{Duration, Instant};
#[derive(Debug, Default)]
pub struct State {
    pub session: String,
    pub grant: Option<(String, i64, String)>,
    pub uncertain: bool,
    pub supported: bool,
    /// Capability mode, reason codes and evidence level from the health report.
    pub capability: Option<String>,
    pending: bool,
    next: Option<Instant>,
}
impl State {
    pub fn poll(&mut self, session: &Session, now: Instant) -> Vec<Action> {
        if self.session != session.id {
            *self = Self {
                session: session.id.clone(),
                ..Self::default()
            };
        }
        if self.pending || self.session.is_empty() || self.next.is_some_and(|next| next > now) {
            return Vec::new();
        }
        self.pending = true;
        self.next = Some(now + Duration::from_secs(2));
        vec![Action::ComputerStatus]
    }
    pub fn apply(&mut self, session: &str, result: Result<Value, String>) {
        if self.session != session {
            return;
        }
        self.pending = false;
        match result {
            Ok(value) => {
                self.supported = value["data"]["enabled"] == true
                    && value["data"]["health"]["supported"] == true;
                self.uncertain = false;
                self.capability = capability(&value["data"]["health"]);
                self.grant = value["data"]["grant"]["target"]["identity"]
                    .as_str()
                    .zip(value["data"]["grant"]["expires_at"].as_i64())
                    .map(|(id, expires)| {
                        (
                            id.into(),
                            expires,
                            value["data"]["grant"]["session_id"]
                                .as_str()
                                .unwrap_or("unknown Session")
                                .into(),
                        )
                    });
            }
            Err(_) => {
                self.uncertain = self.grant.is_some();
                self.next = Some(Instant::now() + Duration::from_secs(10));
            }
        }
    }
    pub fn banner(&self, session: &str) -> Option<String> {
        if session != self.session {
            return None;
        }
        let (target, expiry, owner) = self.grant.as_ref()?;
        if *expiry <= crate::loops::now_ms() && !self.uncertain {
            return None;
        }
        let capability = self
            .capability
            .as_deref()
            .map(|text| format!(" · {}", crate::sidebar::clean(text)))
            .unwrap_or_default();
        Some(format!(
            "Computer {} · Ctrl+Alt+Esc STOP · {} · {}s · {}{capability}",
            if self.uncertain {
                "status unknown"
            } else {
                "ARMED"
            },
            crate::sidebar::clean(owner),
            expiry.saturating_sub(crate::loops::now_ms()).max(0) / 1000,
            crate::sidebar::clean(target),
        ))
    }
}
/// `mode · reasons · evidence` from a version 2 health report; `None` for version 1.
fn capability(health: &Value) -> Option<String> {
    let mode = health["mode"].as_str()?;
    let mut parts = vec![mode.to_owned()];
    let reasons: Vec<&str> = ["capture", "input"]
        .iter()
        .flat_map(|kind| health[kind]["reasons"].as_array().into_iter().flatten())
        .filter_map(Value::as_str)
        .collect();
    let mut unique = reasons.clone();
    unique.dedup();
    if !unique.is_empty() {
        parts.push(unique.join(","));
    }
    if let Some(evidence) = health["evidence"].as_str() {
        parts.push(format!("evidence {evidence}"));
    }
    Some(parts.join(" · "))
}

/// A short note for a computer receipt that carries a reason, cleanup state or event
/// counts. Receipts never contain typed text or window titles.
pub fn receipt_note(output: &str) -> Option<String> {
    let receipt: Value = serde_json::from_str(output).ok()?;
    let outcome = receipt["outcome"].as_str()?;
    let mut parts = Vec::new();
    if let Some(reason) = receipt["reason"].as_str() {
        parts.push(format!("reason {reason}"));
    }
    if let Some(cleanup) = receipt["cleanup"].as_str().filter(|c| *c != "not_needed") {
        parts.push(format!("cleanup {cleanup}"));
    }
    if let (Some(posted), Some(requested)) = (
        receipt["events"]["posted"].as_u64(),
        receipt["events"]["requested"].as_u64(),
    ) {
        parts.push(format!("{posted}/{requested} events posted"));
    }
    (!parts.is_empty()).then(|| format!("{outcome} · {}", parts.join(" · ")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn health_capability_and_receipt_reasons_are_summarized() {
        let mut state = State {
            session: "one".into(),
            ..State::default()
        };
        state.apply("one",Ok(json!({"data":{"enabled":true,"health":{"supported":true,"mode":"capture_only","capture":{"available":true,"reasons":[]},"input":{"available":false,"reasons":["evidence_pending"]},"evidence":"fixture_tested"},"grant":{"target":{"identity":"pick:output"},"expires_at":i64::MAX,"session_id":"one"}}})));
        let banner = state.banner("one").unwrap();
        assert!(
            banner.contains("capture_only · evidence_pending · evidence fixture_tested"),
            "{banner}"
        );
        state.apply("one",Ok(json!({"data":{"health":{"supported":true},"grant":{"target":{"identity":"app:x"},"expires_at":i64::MAX}}})));
        assert!(
            state.capability.is_none(),
            "version 1 reports render as before"
        );
        assert_eq!(
            receipt_note(r#"{"outcome":"unknown","reason":"input_unavailable","cleanup":"uncertain","events":{"requested":2,"posted":1}}"#).as_deref(),
            Some("unknown · reason input_unavailable · cleanup uncertain · 1/2 events posted")
        );
        assert_eq!(
            receipt_note(r#"{"outcome":"posted","cleanup":"not_needed"}"#),
            None
        );
        assert_eq!(receipt_note("not json"), None);
    }
    #[test]
    fn authority_is_bound_to_session_and_uncertain_control_keeps_stop_visible() {
        let mut state = State::default();
        let session = Session {
            id: "one".into(),
            ..Session::default()
        };
        assert_eq!(
            state.poll(&session, Instant::now()),
            vec![Action::ComputerStatus]
        );
        state.apply("one",Ok(json!({"data":{"grant":{"target":{"identity":"app:fixture"},"expires_at":i64::MAX}}})));
        assert!(state.banner("one").unwrap().contains("STOP"));
        assert!(state.banner("two").is_none());
        state.apply("one", Err("offline".into()));
        assert!(state.banner("one").unwrap().contains("unknown"));
    }
}

/// Own the exact Session stream. A Location-wide TUI stream must not silently
/// count as attendance for every Session on the host.
#[derive(Default)]
pub struct Presence {
    session: String,
    task: Option<tokio::task::JoinHandle<()>>,
}
impl Presence {
    pub fn sync(&mut self, client: &cyber_client::Client, session: &Session, enabled: bool) {
        let id = if enabled { session.id.as_str() } else { "" };
        if self.session == id {
            return;
        }
        self.session = id.into();
        if let Some(task) = self.task.take() {
            task.abort();
        }
        if id.is_empty() {
            return;
        }
        let client = client.at(&session.directory);
        let id = id.to_owned();
        let after = session.seq;
        self.task = Some(tokio::spawn(async move {
            use futures::StreamExt;
            loop {
                if let Ok(stream) = client.session_event_stream(&id, after).await {
                    let mut stream = Box::pin(stream);
                    while stream.next().await.is_some() {}
                }
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        }));
    }
}
impl Drop for Presence {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

pub async fn perform(
    client: &cyber_client::Client,
    session: &Session,
    action: Action,
) -> Result<crate::perform::Msg, String> {
    use crate::perform::Msg;
    match action {
        Action::ComputerStatus => Ok(Msg::Computer {
            session_id: session.id.clone(),
            result: client
                .get("/tools/computer/health")
                .await
                .map_err(|error| error.to_string()),
        }),
        Action::ComputerStop => {
            client
                .post("/tools/computer/stop", serde_json::json!({}))
                .await
                .map_err(|error| error.to_string())?;
            Ok(Msg::Toast("Native computer control stopped".into()))
        }
        _ => Err("Unsupported native computer action".into()),
    }
}

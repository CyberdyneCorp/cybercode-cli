//! Native authority is read from the server; this state never creates a grant.
use crate::{app::Action, model::Session};
use serde_json::Value;
use std::time::{Duration, Instant};
#[derive(Debug, Default)]
pub struct State {
    pub session: String,
    pub grant: Option<(String, i64)>,
    pub uncertain: bool,
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
                self.uncertain = false;
                self.grant = value["data"]["grant"]["target"]["identity"]
                    .as_str()
                    .zip(value["data"]["grant"]["expires_at"].as_i64())
                    .map(|(id, expires)| (id.into(), expires));
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
        let (target, expiry) = self.grant.as_ref()?;
        if *expiry <= crate::loops::now_ms() && !self.uncertain {
            return None;
        }
        Some(format!(
            "Computer {} · {} · until {} · Ctrl+Alt+Esc STOP",
            if self.uncertain {
                "status unknown"
            } else {
                "ARMED"
            },
            crate::sidebar::clean(target),
            expiry
        ))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
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

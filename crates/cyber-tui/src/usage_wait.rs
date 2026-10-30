//! Usage limit waits in the TUI (`tui` → Resume at usage limit reset): the hint on a
//! usage-limit failure, the toasts of a wait and the footer badge.

use serde_json::Value;

/// The hint a usage-limit failure with a known reset gets, when the server did not wait.
pub const RESUME_HINT: &str = "/resume-at-reset to continue then";

/// The toast for `session.error`: a usage limit that resets at a known time, and that
/// the server did not wait for, offers `/resume-at-reset`.
pub fn error_notice(data: &Value, text: String) -> String {
    let message = data["message"].as_str().unwrap_or_default();
    let resumable = data["kind"] == "quota_exceeded"
        && message.contains("usage limit reached; resets at ")
        && !message.contains("not waiting");
    if resumable {
        format!("{text} · {RESUME_HINT}")
    } else {
        text
    }
}

/// The toast for a `session.usage_limit.*` event.
pub fn wait_notice(kind: &str, data: &Value) -> String {
    match kind.trim_end_matches(".1") {
        "session.usage_limit.waiting" => format!(
            "usage limit reached; resuming at {} · Esc cancels",
            at(&data["resume_at_ms"])
        ),
        "session.usage_limit.resumed" => "usage limit reset; continuing".into(),
        _ => format!(
            "usage limit wait ended: {}",
            data["reason"].as_str().unwrap_or("unknown")
        ),
    }
}

/// `/resume-at-reset`: schedule the wait, then say when the Session resumes.
pub async fn perform(
    client: &cyber_client::Client,
    session_id: &str,
) -> Result<crate::perform::Msg, String> {
    let path = format!("/sessions/{session_id}/usage-limit/resume");
    let wait = client
        .post(&path, serde_json::json!({}))
        .await
        .map_err(crate::perform::err)?;
    Ok(crate::perform::Msg::Toast(scheduled(&wait["data"])))
}

/// The toast after `/resume-at-reset` scheduled the wait.
pub fn scheduled(wait: &Value) -> String {
    format!(
        "resuming at {} after the usage limit resets",
        at(&wait["resume_at_ms"])
    )
}

/// The footer badge while a wait is pending.
pub fn footer(resume_at_ms: i64) -> String {
    format!("⏸ usage limit · resumes {}", clock(resume_at_ms))
}

fn at(value: &Value) -> String {
    value.as_i64().map(clock).unwrap_or_else(|| "?".into())
}

/// `14:31` in local time, with the date on another day.
fn clock(ms: i64) -> String {
    let Ok(at) = jiff::Timestamp::from_millisecond(ms) else {
        return "?".into();
    };
    let zone = jiff::tz::TimeZone::system();
    let (at, today) = (
        at.to_zoned(zone.clone()),
        jiff::Timestamp::now().to_zoned(zone),
    );
    let format = if at.date() == today.date() {
        "%H:%M"
    } else {
        "%Y-%m-%d %H:%M"
    };
    at.strftime(format).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn only_a_usage_limit_the_server_did_not_wait_for_offers_the_command() {
        let notice = |kind: &str, message: &str| {
            error_notice(
                &json!({"kind": kind, "message": message}),
                format!("error: {message}"),
            )
        };
        assert_eq!(
            notice(
                "quota_exceeded",
                "QuotaExceeded: usage limit reached; resets at 14:20"
            ),
            "error: QuotaExceeded: usage limit reached; resets at 14:20 · /resume-at-reset to continue then"
        );
        let far = "QuotaExceeded: usage limit reached; resets at 14:20 (not waiting: the reset is more than usage_limit.max_wait (6h) away)";
        assert_eq!(notice("quota_exceeded", far), format!("error: {far}"));
        assert_eq!(
            notice("quota_exceeded", "QuotaExceeded: usage limit reached"),
            "error: QuotaExceeded: usage limit reached"
        );
        assert_eq!(notice("model", "boom"), "error: boom");
    }

    #[test]
    fn wait_events_and_the_footer_read_naturally() {
        let now = jiff::Timestamp::now().as_millisecond();
        let waiting = wait_notice(
            "session.usage_limit.waiting.1",
            &json!({"resume_at_ms": now}),
        );
        assert!(
            waiting.starts_with("usage limit reached; resuming at ")
                && waiting.ends_with(" · Esc cancels"),
            "{waiting}"
        );
        assert_eq!(
            wait_notice("session.usage_limit.resumed.1", &json!({})),
            "usage limit reset; continuing"
        );
        assert_eq!(
            wait_notice(
                "session.usage_limit.abandoned.1",
                &json!({"reason": "interrupted"})
            ),
            "usage limit wait ended: interrupted"
        );
        let badge = footer(now);
        assert!(badge.starts_with("⏸ usage limit · resumes "), "{badge}");
        assert_eq!(badge.chars().filter(|c| *c == ':').count(), 1, "{badge}");
    }
}

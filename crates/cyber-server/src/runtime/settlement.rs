//! The owned-work settlement rule (`exec-mode`): a run is not done while a background
//! Job it started is running, or has ended without its handback admitted to the parent
//! (the handback Turn is then still to come). `cyber exec` and MCP serve both apply it to
//! the Jobs of the Session (`GET /api/v1/jobs` rows) that belong to their run.

use std::time::Duration;

use serde_json::Value;

/// How long ended work may stay without its handback admitted before the handback is
/// presumed lost (its admission failed and its live error may have been dropped).
pub const HANDBACK_GRACE: Duration = Duration::from_secs(30);

/// One owned Job under the settlement rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobSettlement {
    Running,
    /// Ended, but its handback is not admitted to the parent yet.
    AwaitingHandback,
    Settled,
}

/// Classify one Job row; a stopping run does not await handbacks (they are cancelled
/// work).
pub fn job_settlement(job: &Value, stopping: bool) -> JobSettlement {
    if job["status"] == "running" {
        JobSettlement::Running
    } else if stopping || job["notified"] != false {
        JobSettlement::Settled
    } else {
        JobSettlement::AwaitingHandback
    }
}

/// Whether every Job started at or after `since_ms` has settled at `now_ms`. A handback
/// still missing [`HANDBACK_GRACE`] after the Job ended is presumed lost and no longer
/// awaited.
pub fn owned_work_settled(jobs: &[Value], since_ms: i64, now_ms: i64) -> bool {
    let grace = i64::try_from(HANDBACK_GRACE.as_millis()).unwrap_or(i64::MAX);
    jobs.iter()
        .filter(|job| job["started_ms"].as_i64().is_some_and(|t| t >= since_ms))
        .all(|job| match job_settlement(job, false) {
            JobSettlement::Running => false,
            JobSettlement::Settled => true,
            JobSettlement::AwaitingHandback => job["ended_ms"]
                .as_i64()
                .is_some_and(|ended| now_ms.saturating_sub(ended) >= grace),
        })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn jobs_settle_once_ended_and_handed_back() {
        let running = json!({"status":"running","notified":false});
        let ended = json!({"status":"completed","notified":false});
        let handed = json!({"status":"completed","notified":true});
        assert_eq!(job_settlement(&running, false), JobSettlement::Running);
        assert_eq!(job_settlement(&running, true), JobSettlement::Running);
        assert_eq!(
            job_settlement(&ended, false),
            JobSettlement::AwaitingHandback
        );
        assert_eq!(job_settlement(&ended, true), JobSettlement::Settled);
        assert_eq!(job_settlement(&handed, false), JobSettlement::Settled);
    }

    #[test]
    fn only_jobs_started_by_the_run_are_awaited() {
        let old = json!({"status":"running","notified":false,"started_ms":5});
        assert!(owned_work_settled(std::slice::from_ref(&old), 10, 20));
        let mine = json!({"status":"running","notified":false,"started_ms":10});
        assert!(!owned_work_settled(&[old, mine], 10, 20));
        let ended = json!({"status":"failed","notified":false,"started_ms":12,"ended_ms":15});
        assert!(!owned_work_settled(std::slice::from_ref(&ended), 10, 20));
        assert!(
            owned_work_settled(&[ended], 10, 15 + 30_000),
            "presumed lost"
        );
        let handed = json!({"status":"completed","notified":true,"started_ms":12});
        assert!(owned_work_settled(&[handed], 10, 20));
    }
}

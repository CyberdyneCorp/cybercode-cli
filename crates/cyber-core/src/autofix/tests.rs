use super::*;
const BASE: &str = "1111111111111111111111111111111111111111";
const FIX: &str = "2222222222222222222222222222222222222222";
fn subscription() -> Subscription {
    Subscription::new(
        "afx_test".into(),
        "/project".into(),
        Config {
            repository: "team/repo".into(),
            pr: 1,
            session_id: "ses_owner".into(),
            model: "provider/model".into(),
            checks: vec![vec!["cargo".into(), "test".into()]],
            verification_paths: vec!["tests".into()],
            required_ci: vec!["check:1:test".into()],
            max_attempts: 3,
            max_tokens: 10000,
            max_seconds: 60,
        },
        7,
        "fix".into(),
        0,
    )
    .unwrap()
}
fn event(key: &str) -> Trigger {
    Trigger {
        key: key.into(),
        head: BASE.into(),
        kind: TriggerKind::CheckFailure,
        text: "untrusted failure".into(),
    }
}
fn claim(s: &mut Subscription, id: &str) {
    s.enqueue(vec![event(id)], BASE).unwrap();
    assert!(s.claim(id.into(), BASE, 0).unwrap())
}
fn intent(s: &mut Subscription, id: &str) {
    s.running(id, "ses_fix".into()).unwrap();
    s.verifying(id, Some(10), None).unwrap();
    s.push_intent(
        id,
        FIX.into(),
        vec![CheckEvidence {
            argv: s.config.checks[0].clone(),
            passed: true,
            output_digest: "a".repeat(64),
        }],
    )
    .unwrap()
}
#[test]
fn duplicates_survive_serialization_and_concurrent_claims() {
    let mut s = subscription();
    claim(&mut s, "check:1");
    let mut recovered: Subscription =
        serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
    recovered.enqueue(vec![event("check:1")], BASE).unwrap();
    assert!(recovered.queued.is_empty());
    assert!(!recovered.claim("other".into(), BASE, 1).unwrap());
}
#[test]
fn stale_events_cannot_trigger_on_another_head() {
    let mut s = subscription();
    s.enqueue(vec![event("check:1")], FIX).unwrap();
    assert!(!s.claim("one".into(), FIX, 0).unwrap());
    assert!(s.seen.is_empty());
}
#[test]
fn push_is_not_success_and_wrong_commit_cannot_reset_failures() {
    let mut s = subscription();
    s.failures = 2;
    claim(&mut s, "a");
    intent(&mut s, "a");
    s.pushed("a").unwrap();
    assert_eq!(s.failures, 2);
    assert!(s.ci("a", BASE, true, 2).is_err());
    s.ci("a", FIX, true, 2).unwrap();
    assert_eq!(s.failures, 0);
}
#[test]
fn three_failures_exhaust_without_duplicate_settlement() {
    let mut s = subscription();
    for i in 0..3 {
        let id = i.to_string();
        claim(&mut s, &id);
        s.failed(&id, "failed".into(), 1).unwrap();
        assert!(s.failed(&id, "again".into(), 1).is_err())
    }
    assert_eq!(s.status, Status::Exhausted);
    assert!(s.control("resume").is_err());
}
#[test]
fn uncertain_push_never_repeats_dispatch() {
    let mut s = subscription();
    claim(&mut s, "a");
    intent(&mut s, "a");
    s.unknown("a", "lost acknowledgement").unwrap();
    assert!(s.control("resume").is_err());
    assert!(s.failed("a", "oops".into(), 2).is_err());
    assert!(!s.claim("new".into(), BASE, 3).unwrap());
    s.pushed("a").unwrap();
    assert_eq!(s.active().unwrap().stage, Stage::AwaitingCi);
    assert_eq!(s.status, Status::Paused);
}
#[test]
fn failed_checks_and_changed_argv_cannot_authorize_push() {
    let mut s = subscription();
    claim(&mut s, "a");
    s.running("a", "ses".into()).unwrap();
    s.verifying("a", None, None).unwrap();
    let mut check = CheckEvidence {
        argv: vec!["true".into()],
        passed: true,
        output_digest: "a".repeat(64),
    };
    assert!(s.push_intent("a", FIX.into(), vec![check.clone()]).is_err());
    check.argv = s.config.checks[0].clone();
    check.passed = false;
    assert!(s.push_intent("a", FIX.into(), vec![check]).is_err());
}
#[test]
fn pause_fences_later_push() {
    let mut s = subscription();
    claim(&mut s, "a");
    s.running("a", "ses".into()).unwrap();
    s.verifying("a", None, None).unwrap();
    s.control("pause").unwrap();
    assert!(s.push_intent("a", FIX.into(), vec![]).is_err());
}
#[test]
fn reject_unsafe_configuration_and_oversized_feedback() {
    let mut s = subscription();
    s.config.repository = "../repo".into();
    assert!(s.config.validate().is_err());
    s.config.repository = "team/repo".into();
    s.config.max_tokens = 0;
    assert!(s.config.validate().is_err());
    let mut e = event("a");
    e.text = "x".repeat(16385);
    assert!(s.enqueue(vec![e], BASE).is_err());
    assert!(s.queued.is_empty());
}

#[test]
fn disabling_inflight_work_survives_unknown_settlement() {
    let mut s = subscription();
    claim(&mut s, "a");
    intent(&mut s, "a");
    s.control("disable").unwrap();
    s.unknown("a", "lost reply").unwrap();
    assert_eq!(s.status, Status::Disabled);
    assert!(s.control("pause").is_err());
    assert!(s.control("resume").is_err());
    s.pushed("a").unwrap();
    assert_eq!(s.status, Status::Disabled);
}
#[test]
fn closed_subscription_cannot_be_reopened_by_pause_resume() {
    let mut s = subscription();
    s.status = Status::Closed;
    assert!(s.control("pause").is_err());
    assert!(s.control("resume").is_err());
}

#[test]
fn review_arriving_during_attempt_is_revalidated_after_push_instead_of_lost() {
    let mut s = subscription();
    claim(&mut s, "first_failure");
    let mut review = event("review:42:revision1");
    review.kind = TriggerKind::Review;
    s.enqueue(vec![review.clone()], BASE).unwrap();
    intent(&mut s, "first_failure");
    s.pushed("first_failure").unwrap();
    s.ci("first_failure", FIX, true, 1).unwrap();
    let mut s: Subscription = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
    s.enqueue(vec![], FIX).unwrap();
    assert!(!s.seen.contains(&review.key));
    assert!(s.queued.is_empty());
    // GitHub revalidates the current writer and immutable review revision before re-emission.
    review.head = FIX.into();
    s.enqueue(vec![review.clone()], FIX).unwrap();
    assert!(s.claim("next".into(), FIX, 2).unwrap());
    assert_eq!(s.active().unwrap().triggers, vec![review.clone()]);
    let mut s: Subscription = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
    s.enqueue(vec![review], FIX).unwrap();
    assert!(s.queued.is_empty());
}

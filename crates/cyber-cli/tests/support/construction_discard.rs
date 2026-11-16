//! Owner-only discard of checkout construction evidence through the real binary
//! (`snapshots-checkpoints` → Owner-only discard of construction evidence): a real
//! SIGKILL leaves a flushed, unpublished claim stage; the discard refuses while the
//! preparation is unresolved and for a stale fingerprint, and worktree removal is
//! admitted only after the stage is discarded. A second table kills the service
//! between the durable intent, the quarantine move and the unlink.
use super::*;

const CUTS: [(&str, Place); 3] = [
    ("construction-discard-intent-committed", Place::Source),
    ("construction-discard-quarantined", Place::Quarantined),
    ("construction-discard-unlinked", Place::Gone),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Place {
    Source,
    Quarantined,
    Gone,
}

struct Fixture {
    _model: scripted_model::ScriptedModel,
    env: Env,
    session: String,
    activity: String,
    stage: PathBuf,
    name: String,
}

impl Fixture {
    /// A managed checkout whose claim stage survived a real kill of its constructor.
    fn new() -> Self {
        let model = scripted_model::ScriptedModel::start(reply);
        let (env, session) = managed_git_fixture(&model);
        std::fs::write(env.directory.join("tracked"), "before").unwrap();
        env.exec(Some(&session), "before snapshot");
        let first = env.user_ids(&session)[0].clone();
        std::fs::write(env.directory.join("tracked"), "after").unwrap();
        env.exec(Some(&session), "after snapshot");
        kill_at_claim_stage(&env, &session, &first);
        let (stage, activity) = retained_claim_stage(&env);
        let name = stage.file_name().unwrap().to_str().unwrap().to_owned();
        Self {
            _model: model,
            env,
            session,
            activity,
            stage,
            name,
        }
    }

    fn cancel_preparation(&self) {
        let review = json(&self.env.cyber(&[
            "sessions",
            "recovery-preparation",
            &self.session,
            &self.activity,
            "--format",
            "json",
        ]));
        json(&self.env.cyber(&[
            "sessions",
            "recovery-preparation",
            &self.session,
            &self.activity,
            "--confirm",
            review["confirmation"].as_str().unwrap(),
            "--request-id",
            "cancel-before-discard",
            "--format",
            "json",
        ]));
    }

    fn review(&self) -> Value {
        json(&self.env.cyber(&[
            "sessions",
            "recovery-discard",
            &self.session,
            &self.name,
            "--format",
            "json",
        ]))
    }

    fn confirm(&self, fingerprint: &str, request: &str) -> Output {
        self.env.cyber(&[
            "sessions",
            "recovery-discard",
            &self.session,
            &self.name,
            "--confirm",
            fingerprint,
            "--request-id",
            request,
            "--format",
            "json",
        ])
    }

    fn lookup(&self, request: &str) -> Value {
        json(&self.env.cyber(&[
            "sessions",
            "recovery-discard-request",
            &self.session,
            request,
            "--format",
            "json",
        ]))
    }

    fn listing(&self) -> Value {
        json(&self.env.cyber(&[
            "sessions",
            "recovery-discard",
            &self.session,
            "--format",
            "json",
        ]))
    }

    fn quarantine(&self) -> PathBuf {
        self.stage
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("discarded")
    }

    /// Where the stage is: its activity directory, the quarantine, or nowhere.
    fn place(&self) -> Place {
        let quarantined = std::fs::read_dir(self.quarantine())
            .into_iter()
            .flatten()
            .flat_map(|slot| std::fs::read_dir(slot.unwrap().path()).unwrap())
            .any(|entry| entry.unwrap().file_name().to_str() == Some(self.name.as_str()));
        match (self.stage.exists(), quarantined) {
            (true, false) => Place::Source,
            (false, true) => Place::Quarantined,
            (false, false) => Place::Gone,
            (true, true) => panic!("stage both in place and quarantined"),
        }
    }

    fn assert_completed(&self, receipt: &Value, request: &str) {
        assert_eq!(receipt["status"], "completed", "{receipt}");
        assert_eq!(receipt["request_id"], request);
        assert_eq!(receipt["actor"], "local_owner");
        assert_eq!(receipt["stage"]["name"], self.name.as_str());
        assert_eq!(receipt["stage"]["activity_id"], self.activity.as_str());
        assert_eq!(self.place(), Place::Gone);
        assert!(
            std::fs::read_dir(self.quarantine())
                .map(|mut entries| entries.next().is_none())
                .unwrap_or(true),
            "quarantine slot retained"
        );
        assert_eq!(self.listing()["pending"], json!([]));
    }

    /// Even a forced removal (the fixture's checkout has local edits) refuses
    /// while the stage remains.
    fn assert_removal_fenced(&self) {
        let refused = self
            .env
            .cyber(&["api", "DELETE", "/worktrees/managed?force=true"]);
        let text = exits(&refused, 1);
        assert!(text.contains("same-owner unknown constructor"), "{text}");
        assert!(text.contains(&self.name), "{text}");
        assert!(self.env.directory.exists());
    }

    fn assert_removal_admitted(&mut self) {
        exits(
            &self
                .env
                .cyber(&["api", "DELETE", "/worktrees/managed?force=true"]),
            0,
        );
        assert!(!self.env.directory.exists());
        // Later commands (and the fixture's service shutdown) run from the repository.
        self.env.directory = self.env.root.join("repo");
    }
}

fn sha256(bytes: &[u8]) -> String {
    use sha2::Digest;
    format!("{:x}", sha2::Sha256::digest(bytes))
}

/// The stage is listed with its command and reviewed read-only; an unresolved
/// preparation refuses the confirmation. Returns the (soon stale) review.
fn listed_reviewed_and_refused_while_unresolved(fixture: &Fixture, bytes: &[u8]) -> Value {
    let listed = fixture.listing();
    assert_eq!(listed["stages"][0]["name"], fixture.name.as_str());
    assert_eq!(listed["stages"][0]["kind"], "checkout_claim");
    assert_eq!(
        listed["stages"][0]["activity_id"],
        fixture.activity.as_str()
    );
    let text = exits(
        &fixture
            .env
            .cyber(&["sessions", "recovery", &fixture.session]),
        0,
    );
    assert!(text.contains("cyber sessions recovery-discard"), "{text}");
    let early = fixture.review();
    assert_eq!(early["stage"]["kind"], "checkout_claim");
    assert_eq!(early["stage"]["location"], "present");
    assert_eq!(early["stage"]["evidence"]["sha256"], sha256(bytes));
    assert_eq!(early["stage"]["evidence"]["size"], bytes.len());
    assert_eq!(std::fs::read(&fixture.stage).unwrap(), bytes);
    let refused = fixture.confirm(early["confirmation"].as_str().unwrap(), "early");
    let text = exits(&refused, 1);
    assert!(text.contains("unresolved snapshot preparation"), "{text}");
    assert_eq!(std::fs::read(&fixture.stage).unwrap(), bytes);
    fixture.assert_removal_fenced();
    early
}

/// A later Turn changes the Session revision: the early review is stale and
/// nothing is recorded or changed.
fn stale_review_refuses(fixture: &Fixture, early: &Value, bytes: &[u8]) {
    fixture
        .env
        .exec(Some(&fixture.session), "after cancellation");
    assert_eq!(std::fs::read(&fixture.stage).unwrap(), bytes);
    let stale = fixture.confirm(early["confirmation"].as_str().unwrap(), "stale");
    let text = exits(&stale, 1);
    assert!(text.contains("stale"), "{text}");
    assert_eq!(std::fs::read(&fixture.stage).unwrap(), bytes);
    let missing = fixture.env.cyber(&[
        "sessions",
        "recovery-discard-request",
        &fixture.session,
        "stale",
    ]);
    exits(&missing, 1);
    fixture.assert_removal_fenced();
}

#[test]
fn discarding_a_killed_claim_stage_is_fenced_receipted_and_admits_worktree_removal() {
    let mut fixture = Fixture::new();
    let bytes = std::fs::read(&fixture.stage).unwrap();
    let early = listed_reviewed_and_refused_while_unresolved(&fixture, &bytes);
    fixture.cancel_preparation();
    stale_review_refuses(&fixture, &early, &bytes);
    let review = fixture.review();
    let fingerprint = review["confirmation"].as_str().unwrap();
    let receipt = json(&fixture.confirm(fingerprint, "discard-claim"));
    fixture.assert_completed(&receipt, "discard-claim");
    // A retry and a lookup return the receipt and touch nothing else.
    assert_eq!(
        json(&fixture.confirm(fingerprint, "discard-claim")),
        receipt
    );
    assert_eq!(fixture.lookup("discard-claim"), receipt);
    let history = json(&fixture.env.cyber(&[
        "api",
        "GET",
        &format!("/sessions/{}/history?limit=500", fixture.session),
    ]));
    assert!(
        history
            .to_string()
            .contains("session.snapshot.construction_discarded.1"),
        "{history}"
    );
    fixture.assert_removal_admitted();
}

/// Kill the service at `point` inside a confirmed discard.
fn kill_at_discard(fixture: &Fixture, fingerprint: &str, request: &str, point: &str) {
    exits(&fixture.env.cyber(&["service", "stop"]), 0);
    let marker = fixture.env.root.join(format!("{point}.marker"));
    let log = fixture.env.root.join(format!("{point}.log"));
    let output = std::fs::File::create(&log).unwrap();
    let mut confirm = fixture
        .env
        .command(&[
            "sessions",
            "recovery-discard",
            &fixture.session,
            &fixture.name,
            "--confirm",
            fingerprint,
            "--request-id",
            request,
            "--format",
            "json",
        ])
        .env("CYBER_TEST_PROCESS_CUT", point)
        .env("CYBER_TEST_PROCESS_CUT_MARKER", &marker)
        .stdout(output.try_clone().unwrap())
        .stderr(output)
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(120);
    while !marker.exists() {
        if confirm.try_wait().unwrap().is_some() || std::time::Instant::now() >= deadline {
            let _ = confirm.kill();
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                kill_registered_service(&fixture.env)
            }));
            panic!(
                "discard cut {point} was never reached: {}",
                std::fs::read_to_string(&log).unwrap_or_default()
            );
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    kill_registered_service(&fixture.env);
    assert!(!confirm.wait().unwrap().success());
}

#[test]
fn discard_resumes_after_real_process_death_between_intent_quarantine_and_unlink() {
    for (point, expected) in CUTS {
        let mut fixture = Fixture::new();
        fixture.cancel_preparation();
        let review = fixture.review();
        let fingerprint = review["confirmation"].as_str().unwrap();
        let request = format!("cut-{point}");
        kill_at_discard(&fixture, fingerprint, &request, point);
        assert_eq!(fixture.place(), expected, "{point}");
        // The intent was durable before any filesystem change.
        let intent = fixture.lookup(&request);
        assert_eq!(intent["status"], "intent", "{point}: {intent}");
        assert_eq!(intent["stage"], review["stage"], "{point}");
        let pending = fixture.listing()["pending"].clone();
        assert_eq!(pending[0]["request_id"], request.as_str(), "{point}");
        let doctor = fixture.env.cyber(&["doctor", "--format", "json"]);
        let doctor: Value = serde_json::from_slice(&doctor.stdout).unwrap();
        let check = doctor["checks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|check| check["check"] == "construction discards")
            .unwrap();
        assert!(
            check["detail"]
                .as_str()
                .unwrap()
                .contains("discard pending"),
            "{point}: {check}"
        );
        if expected == Place::Source {
            // Nothing moved yet: removal stays fenced until the discard completes.
            fixture.assert_removal_fenced();
        }
        if expected == Place::Quarantined {
            // Maintenance finishes a moved stage on its own, with no owner action.
            exits(&fixture.env.cyber(&["service", "stop"]), 0);
            let started = fixture
                .env
                .command(&["sessions", "recovery-discard", &fixture.session])
                .env("CYBER_TEST_DISCARD_RETRY_DELAY_MS", "50")
                .output()
                .unwrap();
            exits(&started, 0);
            let deadline = std::time::Instant::now() + Duration::from_secs(60);
            while fixture.lookup(&request)["status"] != "completed" {
                assert!(
                    std::time::Instant::now() < deadline,
                    "maintenance never retried"
                );
                std::thread::sleep(Duration::from_millis(50));
            }
        }
        // The same request resumes its frozen plan; a new review is not needed.
        let receipt = json(&fixture.confirm(fingerprint, &request));
        fixture.assert_completed(&receipt, &request);
        assert_eq!(fixture.lookup(&request), receipt, "{point}");
        fixture.assert_removal_admitted();
    }
}

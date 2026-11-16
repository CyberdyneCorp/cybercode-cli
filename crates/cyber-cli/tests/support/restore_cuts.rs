//! Real process-death cuts on the file-copy restore and recovery paths that the M8
//! matrix listed as gaps: directly after a progress record, after filesystem
//! completion with displaced payload images, after the paired recovery writer
//! commit, and after a native journal acknowledgement whose reply was never sent.
//! Each cut is a debug-only `CYBER_TEST_PROCESS_CUT` point in the service.
use super::*;
#[path = "cleanup_cuts.rs"]
mod cleanup;
const FILES: usize = 4;

struct Fixture {
    model: scripted_model::ScriptedModel,
    env: Env,
    session: String,
    first: String,
}
impl Fixture {
    /// Non-Git Location with FILES modified tracked files; every restore change
    /// displaces a payload image into its journal.
    fn new() -> Self {
        let model = scripted_model::ScriptedModel::start(reply);
        let env = Env::new(&model);
        std::fs::remove_dir_all(env.directory.join(".git")).unwrap();
        Self::capture(model, env, None)
    }
    /// The same files tracked in a managed Git worktree (`native-git-acceptance`).
    #[cfg(feature = "native-git-acceptance")]
    fn managed() -> Self {
        let model = scripted_model::ScriptedModel::start(reply);
        let (env, session) = managed_git_fixture(&model);
        write_files(&env, "before");
        git_fixture(&env.directory, &["add", "--all"]);
        Self::capture(model, env, Some(session))
    }
    fn capture(model: scripted_model::ScriptedModel, env: Env, session: Option<String>) -> Self {
        if session.is_none() {
            write_files(&env, "before");
        }
        let session = env.exec(session.as_deref(), "before snapshot");
        let first = env.user_ids(&session)[0].clone();
        write_files(&env, "after");
        env.exec(Some(&session), "after snapshot");
        Self {
            model,
            env,
            session,
            first,
        }
    }
    fn namespace(&self) -> PathBuf {
        let namespaces: Vec<_> =
            std::fs::read_dir(self.env.root.join("cyber-home/data/snapshot/global"))
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .collect();
        assert_eq!(namespaces.len(), 1);
        namespaces[0].clone()
    }
    fn journals(&self) -> Vec<String> {
        let mut journals: Vec<String> = std::fs::read_dir(self.namespace().join("journals"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        journals.sort();
        journals
    }
    fn intent(&self, journal: &str) -> Value {
        let path = self
            .namespace()
            .join("journals")
            .join(journal)
            .join("intent.json");
        serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
    }
    /// The only journal, which the killed restore left behind.
    fn only_journal(&self) -> (String, Value) {
        let journals = self.journals();
        assert_eq!(journals.len(), 1, "{journals:?}");
        let intent = self.intent(&journals[0]);
        (journals[0].clone(), intent)
    }
    fn payloads(&self, journal: &str) -> usize {
        std::fs::read_dir(self.namespace().join("journals").join(journal))
            .unwrap()
            .filter(|entry| {
                entry
                    .as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with("stage_")
            })
            .count()
    }
    fn contents(&self) -> Vec<Option<String>> {
        (0..FILES)
            .map(|index| self.env.file(&name(index)))
            .collect()
    }
    /// Run the reviewed `both` rewind on a fresh service that stops at `point`, then
    /// SIGKILL the registered service.
    fn kill_rewind_at(&self, point: &str) {
        let preview = rewind_preview(&self.env, &self.session, &self.first, "both");
        let confirmation = preview["confirmation"].as_str().unwrap().to_owned();
        self.kill_at(
            point,
            &[
                "sessions",
                "rewind",
                &self.session,
                "--to",
                &self.first,
                "--target",
                "both",
                "--confirm",
                &confirmation,
                "--format",
                "json",
            ],
        );
    }
    /// Stop the running service, run `args` on a fresh service started with the
    /// fixture point, wait for its marker and SIGKILL the registered service.
    fn kill_at(&self, point: &str, args: &[&str]) {
        exits(&self.env.cyber(&["service", "stop"]), 0);
        let marker = self.env.root.join(format!("{point}.marker"));
        // A point may be reached once per process; only this run's marker counts.
        let _ = std::fs::remove_file(&marker);
        let log = self.env.root.join(format!("{point}.log"));
        let output = std::fs::File::create(&log).unwrap();
        let mut child = Child(
            self.env
                .command(args)
                .env("CYBER_TEST_PROCESS_CUT", point)
                .env("CYBER_TEST_PROCESS_CUT_MARKER", &marker)
                .stdout(output.try_clone().unwrap())
                .stderr(output)
                .spawn()
                .unwrap(),
        );
        wait_for(&marker, &mut child, &log);
        kill_registered_service(&self.env);
        assert!(!child.0.wait().unwrap().success());
    }
    /// The latest durable record of the journal's native owner.
    fn activity(&self, intent: &Value) -> Value {
        latest_record(&self.env, intent["origin"]["activity_id"].as_str().unwrap())
    }
    fn review(&self, journal: &str, action: &str) -> Value {
        let review = json(&self.env.cyber(&[
            "sessions",
            "recovery",
            &self.session,
            journal,
            "--action",
            action,
            "--format",
            "json",
        ]));
        assert!(
            review["conflicts"].as_array().unwrap().is_empty(),
            "{review}"
        );
        review
    }
    fn catalog_complete(&self, journal: &str) -> Value {
        let catalog =
            json(
                &self
                    .env
                    .cyber(&["sessions", "recovery", &self.session, "--format", "json"]),
            );
        let entries = catalog.as_array().unwrap();
        assert_eq!(entries.len(), 1, "{catalog}");
        assert_eq!(entries[0]["id"], journal);
        entries[0]["filesystem_complete"].clone()
    }
    fn request(&self, journal: &str, action: &str, request_id: &str) -> Value {
        let review = self.review(journal, action);
        json!({"journal_id":journal,"action":action,"confirmation":review["confirmation"],"request_id":request_id})
    }
    fn lookup(&self, request_id: &str, acknowledge: bool) -> Value {
        let mut args = vec![
            "sessions",
            "recovery-request",
            &self.session,
            request_id,
            "--format",
            "json",
        ];
        if acknowledge {
            args.push("--acknowledge");
        }
        json(&self.env.cyber(&args))
    }
    /// Later owner edits made while no service runs: a file replaced by a symlink and
    /// an unrelated new file.
    fn edit_as_owner(&self) -> PathBuf {
        let target = self.env.root.join("later-owner-target");
        std::fs::write(&target, "later owner bytes").unwrap();
        let link = self.env.directory.join(name(0));
        std::fs::remove_file(&link).unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();
        std::fs::write(self.env.directory.join("unrelated-owner"), "owner data").unwrap();
        target
    }
    fn assert_owner_edits(&self, target: &std::path::Path) {
        assert_eq!(
            std::fs::read_link(self.env.directory.join(name(0))).unwrap(),
            target
        );
        assert_eq!(
            std::fs::read_to_string(target).unwrap(),
            "later owner bytes"
        );
        assert_eq!(
            self.env.file("unrelated-owner").as_deref(),
            Some("owner data")
        );
    }
}
impl Fixture {
    /// Confirm a reviewed recovery request and require runtime and native
    /// acknowledgement.
    fn confirm_acknowledged(&self, request: &Value) -> Value {
        let result = json(&recovery_confirm_cli(&self.env, &self.session, request));
        assert_eq!(result["state"], "acknowledged");
        assert_eq!(result["journal_acknowledged"], true);
        result
    }
    /// After keep only the applied path holds restored bytes; rollback leaves none.
    fn assert_resolved(&self, action: &str, applied: &str) {
        let expected: Vec<_> = (0..FILES)
            .map(|index| {
                let kept = action == "keep" && name(index) == applied;
                Some(if kept { "before" } else { "after" }.to_owned())
            })
            .collect();
        assert_eq!(self.contents(), expected);
    }
    fn assert_uniform(&self, contents: &str) {
        assert_eq!(self.contents(), vec![Some(contents.to_owned()); FILES]);
    }
    /// No transcript movement and no inference since the captured baseline.
    fn assert_quiet(&self, messages: &[Value], requests: usize) {
        assert_eq!(self.env.messages(&self.session), messages);
        assert_eq!(self.model.requests().len(), requests);
    }
}
/// CLI arguments that confirm `request` (journal, action, fingerprint, request ID).
fn confirm_args<'a>(f: &'a Fixture, request: &'a Value) -> Vec<&'a str> {
    let field = |key: &str| request[key].as_str().unwrap();
    vec![
        "sessions",
        "recovery",
        &f.session,
        field("journal_id"),
        "--action",
        field("action"),
        "--confirm",
        field("confirmation"),
        "--request-id",
        field("request_id"),
        "--format",
        "json",
    ]
}
fn name(index: usize) -> String {
    format!("cut_{index:04}")
}
fn write_files(env: &Env, contents: &str) {
    for index in 0..FILES {
        std::fs::write(env.directory.join(name(index)), contents).unwrap();
    }
}
fn unacknowledged(intent: &Value) -> bool {
    intent
        .get("acknowledged")
        .is_none_or(|value| value == false)
}
fn latest_record(env: &Env, aggregate: &str) -> Value {
    let database = std::fs::read_dir(env.root.join("cyber-home/data"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.extension().is_some_and(|extension| extension == "db"))
        .unwrap();
    let db =
        rusqlite::Connection::open_with_flags(database, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    let data: String = db
        .query_row(
            "SELECT data FROM event WHERE aggregate_id=?1 ORDER BY seq DESC LIMIT 1",
            [aggregate],
            |row| row.get(0),
        )
        .unwrap();
    serde_json::from_str(&data).unwrap()
}
struct Child(std::process::Child);
impl Drop for Child {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn wait_for(marker: &std::path::Path, child: &mut Child, log: &std::path::Path) {
    let deadline = std::time::Instant::now() + Duration::from_secs(120);
    while !marker.exists() {
        assert!(
            child.0.try_wait().unwrap().is_none() && std::time::Instant::now() < deadline,
            "cut was never reached: {}",
            std::fs::read_to_string(log).unwrap_or_default()
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Kill directly after the first exchange's progress record: exactly the recorded
/// path holds its restored bytes, the writer never committed and the transcript did
/// not move. Returns the journal and the applied path.
fn kill_after_first_progress(f: &Fixture) -> (String, String) {
    f.kill_rewind_at("copy-restore-progress-recorded");
    first_progress(f)
}

/// The only journal holds exactly one applied change and its restored bytes.
fn first_progress(f: &Fixture) -> (String, String) {
    let (journal, intent) = f.only_journal();
    let applied: Vec<String> = serde_json::from_value(intent["applied"].clone()).unwrap();
    assert_eq!(applied.len(), 1, "{intent}");
    assert_eq!(intent["completed"], false);
    assert!(unacknowledged(&intent));
    for index in 0..FILES {
        let expected = if name(index) == applied[0] {
            "before"
        } else {
            "after"
        };
        assert_eq!(f.env.file(&name(index)).as_deref(), Some(expected));
    }
    assert_ne!(f.activity(&intent)["phase"], "snapshot_restore_committed");
    (journal, applied[0].clone())
}

fn progress_cut_resolution(f: Fixture, action: &str) {
    let messages = f.env.messages(&f.session);
    let (journal, applied) = kill_after_first_progress(&f);
    // A fresh service: the failed restore did not move the transcript.
    assert_eq!(f.env.messages(&f.session), messages);
    assert_eq!(f.catalog_complete(&journal), false);
    let requests = f.model.requests().len();
    std::fs::write(f.env.directory.join("unrelated-owner"), "owner data").unwrap();
    let request_id = format!("progress-cut-{action}");
    let request = f.request(&journal, action, &request_id);
    let result = f.confirm_acknowledged(&request);
    f.assert_resolved(action, &applied);
    assert_eq!(f.env.file("unrelated-owner").as_deref(), Some("owner data"));
    // Repeating the request is lookup only, even over a later owner edit.
    std::fs::write(f.env.directory.join(&applied), "edit after recovery").unwrap();
    let retry = json(&recovery_confirm_cli(&f.env, &f.session, &request));
    assert_eq!(retry["receipt"], result["receipt"]);
    assert!(retry["journal_acknowledged"].is_null());
    assert_eq!(f.env.file(&applied).as_deref(), Some("edit after recovery"));
    f.assert_quiet(&messages, requests);
}

#[test]
fn restore_killed_after_a_progress_record_is_kept_without_replay() {
    progress_cut_resolution(Fixture::new(), "keep");
}

#[test]
fn restore_killed_after_a_progress_record_is_rolled_back_without_replay() {
    progress_cut_resolution(Fixture::new(), "rollback");
}

fn completion_cut_resolution(f: Fixture, action: &str) {
    let messages = f.env.messages(&f.session);
    f.kill_rewind_at("copy-restore-filesystem-completed");
    let journal = assert_completed_without_writer(&f, &messages);
    let requests = f.model.requests().len();
    std::fs::write(f.env.directory.join("unrelated-owner"), "owner data").unwrap();
    let request_id = format!("completion-cut-{action}");
    let result = f.confirm_acknowledged(&f.request(&journal, action, &request_id));
    let expected = if action == "keep" { "before" } else { "after" };
    f.assert_uniform(expected);
    assert_eq!(f.env.file("unrelated-owner").as_deref(), Some("owner data"));
    // Resolution never commits the interrupted rewind or starts inference.
    f.assert_quiet(&messages, requests);
    assert_eq!(f.lookup(&request_id, false)["receipt"], result["receipt"]);
    f.assert_uniform(expected);
}

/// Completed filesystem with every displaced image retained and no writer commit.
fn assert_completed_without_writer(f: &Fixture, messages: &[Value]) -> String {
    let (journal, intent) = f.only_journal();
    assert_eq!(intent["completed"], true);
    assert!(unacknowledged(&intent));
    assert_eq!(f.payloads(&journal), FILES);
    f.assert_uniform("before");
    assert_ne!(f.activity(&intent)["phase"], "snapshot_restore_committed");
    assert_eq!(f.env.messages(&f.session), messages);
    assert_eq!(f.catalog_complete(&journal), true);
    journal
}

#[test]
fn restore_killed_after_filesystem_completion_with_payloads_is_kept() {
    completion_cut_resolution(Fixture::new(), "keep");
}

#[test]
fn restore_killed_after_filesystem_completion_with_payloads_is_rolled_back() {
    completion_cut_resolution(Fixture::new(), "rollback");
}

/// The paired recovery commit is durable when the service dies; neither journal is
/// acknowledged. Restart: lookup reports the receipt without native effects and the
/// explicit acknowledgement publishes only journal metadata.
#[test]
fn paired_recovery_killed_after_writer_commit_reconciles_without_replay() {
    paired_case(Fixture::new());
}

fn paired_case(f: Fixture) {
    let messages = f.env.messages(&f.session);
    let (journal, _) = kill_after_first_progress(&f);
    let request_id = "paired-writer-cut";
    let request = f.request(&journal, "rollback", request_id);
    let requests = f.model.requests().len();
    f.kill_at("recovery-pair-writer-commit", &confirm_args(&f, &request));
    let (receipt, resolution) = assert_pair_committed_unacknowledged(&f, &journal, request_id);
    let target = f.edit_as_owner();
    let lookup = f.lookup(request_id, false);
    assert_eq!(lookup["receipt"], receipt);
    assert!(lookup["journal_acknowledged"].is_null());
    assert!(unacknowledged(&f.intent(&journal)), "lookup acknowledged");
    let reconciled = f.lookup(request_id, true);
    assert_eq!(reconciled["receipt"], receipt);
    assert_eq!(reconciled["journal_acknowledged"], true);
    let original = f.intent(&journal);
    assert_eq!(original["acknowledged"], true);
    assert_eq!(original["settled_by"], resolution.as_str());
    assert_eq!(f.intent(&resolution)["acknowledged"], true);
    // The confirm repeated after restart is lookup only.
    let retry = json(&recovery_confirm_cli(&f.env, &f.session, &request));
    assert_eq!(retry["receipt"], receipt);
    f.assert_owner_edits(&target);
    for index in 1..FILES {
        assert_eq!(f.env.file(&name(index)).as_deref(), Some("after"));
    }
    f.assert_quiet(&messages, requests);
}

/// Both owners are committed at the writer and both journals are unacknowledged.
/// Returns the committed receipt and the resolution journal.
fn assert_pair_committed_unacknowledged(
    f: &Fixture,
    journal: &str,
    request_id: &str,
) -> (Value, String) {
    let original = f.intent(journal);
    assert!(unacknowledged(&original));
    assert!(original.get("settled_by").is_none());
    let activity = f.activity(&original);
    assert_eq!(activity["status"], "settled");
    assert_eq!(activity["phase"], "snapshot_recovery_committed");
    let receipt = activity["snapshot_receipt"].clone();
    assert_eq!(receipt["request_id"], request_id);
    let resolution = receipt["resolution_id"].as_str().unwrap().to_owned();
    assert!(unacknowledged(&f.intent(&resolution)));
    f.assert_uniform("after");
    (receipt, resolution)
}

/// The service dies after acknowledging the paired recovery journals and before the
/// reply; the repeated confirmation returns the committed receipt without replay.
#[test]
fn paired_recovery_killed_after_journal_acknowledgement_returns_the_receipt() {
    let f = Fixture::new();
    let messages = f.env.messages(&f.session);
    let (journal, _) = kill_after_first_progress(&f);
    let request = f.request(&journal, "keep", "paired-ack-cut");
    let requests = f.model.requests().len();
    let args = confirm_args(&f, &request);
    f.kill_at("recovery-journal-acknowledged", &args);
    let original = f.intent(&journal);
    assert_eq!(original["acknowledged"], true);
    let receipt = f.activity(&original)["snapshot_receipt"].clone();
    assert_eq!(original["settled_by"], receipt["resolution_id"]);
    let target = f.edit_as_owner();
    let retry = json(&recovery_confirm_cli(&f.env, &f.session, &request));
    assert_eq!(retry["state"], "acknowledged");
    assert_eq!(retry["receipt"], receipt);
    assert!(retry["journal_acknowledged"].is_null());
    f.assert_owner_edits(&target);
    f.assert_quiet(&messages, requests);
}

/// The service dies after the ordinary restore's journal acknowledgement and before
/// the reply: the rewind is committed once, lookup returns the receipt and the same
/// confirmation never restores again.
#[test]
fn ordinary_restore_killed_after_journal_acknowledgement_never_replays() {
    let f = Fixture::new();
    let preview = rewind_preview(&f.env, &f.session, &f.first, "both");
    let requests = f.model.requests().len();
    let confirmation = preview["confirmation"].as_str().unwrap().to_owned();
    let args = [
        "sessions",
        "rewind",
        f.session.as_str(),
        "--to",
        f.first.as_str(),
        "--target",
        "both",
        "--confirm",
        confirmation.as_str(),
        "--format",
        "json",
    ];
    f.kill_at("ordinary-restore-journal-acknowledged", &args);
    let (journal, intent) = f.only_journal();
    assert_eq!(intent["completed"], true);
    assert_eq!(intent["acknowledged"], true);
    f.assert_uniform("before");
    let target = f.edit_as_owner();
    // A fresh service shows the committed rewind and the exact receipt.
    assert!(f.env.user_ids(&f.session).is_empty());
    assert_owned_snapshot_receipt(&f.env, &f.session, &journal);
    let route = format!(
        "/sessions/{}/rewind/restores/{}",
        f.session,
        intent["origin"]["activity_id"].as_str().unwrap()
    );
    let receipt = json(&f.env.cyber(&["api", "GET", &route]))["data"].clone();
    assert_eq!(receipt["origin"], intent["origin"]);
    let messages = f.env.messages(&f.session);
    // The retained confirmation is refused and changes nothing.
    assert!(!f.env.command(&args).output().unwrap().status.success());
    f.assert_owner_edits(&target);
    for index in 1..FILES {
        assert_eq!(f.env.file(&name(index)).as_deref(), Some("before"));
    }
    assert_eq!(f.journals(), vec![journal.clone()]);
    assert_eq!(f.intent(&journal), intent);
    assert_eq!(f.env.messages(&f.session), messages);
    assert_eq!(f.model.requests().len(), requests);
}

/// Review and cancel the only exited preparation; returns its activity and receipt.
fn cancel_preparation(f: &Fixture, request_id: &str) -> (String, Value) {
    let list = json(&f.env.cyber(&[
        "sessions",
        "recovery-preparation",
        &f.session,
        "--format",
        "json",
    ]));
    let activity = list[0]["activity_id"].as_str().unwrap().to_owned();
    let review = json(&f.env.cyber(&[
        "sessions",
        "recovery-preparation",
        &f.session,
        &activity,
        "--format",
        "json",
    ]));
    let body = json!({"confirmation":review["confirmation"],"request_id":request_id});
    let receipt = json(&preparation_confirm_cli(
        &f.env, &f.session, &activity, &body,
    ));
    assert_eq!(receipt["activity_id"], activity.as_str());
    (activity, receipt)
}

/// Kill a restore at a pre-effect journal construction point, then cancel the exited
/// preparation through the real CLI. Returns whether the journal held an intent.
fn construction_cut_cancellation(point: &str) -> bool {
    let f = Fixture::new();
    let messages = f.env.messages(&f.session);
    f.kill_rewind_at(point);
    let journals = f.journals();
    assert_eq!(journals.len(), 1, "{journals:?}");
    let journal = f.namespace().join("journals").join(&journals[0]);
    let has_intent = journal.join("intent.json").exists();
    assert_eq!(f.payloads(&journals[0]), 0, "no image may be staged yet");
    f.assert_uniform("after");
    // New input is fenced until the exited preparation is resolved.
    let refused = f.env.cyber(&["exec", "--session", &f.session, "fenced"]);
    assert!(!refused.status.success());
    let requests = f.model.requests().len();
    let request_id = format!("{point}-cancel");
    let (activity, receipt) = cancel_preparation(&f, &request_id);
    let body = json!({"confirmation":receipt["confirmation"],"request_id":request_id});
    // A repeated confirmation is lookup only.
    assert_eq!(
        json(&preparation_confirm_cli(
            &f.env, &f.session, &activity, &body
        )),
        receipt
    );
    f.assert_uniform("after");
    assert_eq!(f.env.messages(&f.session), messages);
    assert_eq!(f.model.requests().len(), requests);
    f.env.exec(Some(&f.session), "after cancellation");
    has_intent
}

#[test]
fn restore_killed_after_origin_before_intent_is_cancelled_without_source_effects() {
    assert!(!construction_cut_cancellation(
        "copy-restore-origin-published"
    ));
}

#[test]
fn restore_killed_after_intent_before_staging_is_cancelled_without_source_effects() {
    assert!(construction_cut_cancellation(
        "copy-restore-intent-published"
    ));
}

/// Kill a rollback resolution right after its first exchange's progress record. The
/// interrupted resolution is retained as independent evidence: its own request stays
/// pending without replay, the resolution journal is resolved on its own, and only
/// then can the original journal be settled by a new reviewed request.
#[test]
fn interrupted_resolution_is_retained_and_resolved_as_independent_evidence() {
    interrupted_case(Fixture::new());
}

fn interrupted_case(f: Fixture) {
    let messages = f.env.messages(&f.session);
    let (journal, applied) = kill_after_first_progress(&f);
    let request = f.request(&journal, "rollback", "interrupted-resolution");
    let requests = f.model.requests().len();
    f.kill_at(
        "copy-restore-progress-recorded",
        &confirm_args(&f, &request),
    );
    let child = interrupted_child(&f, &journal, &applied);
    assert_not_redispatched(&f, &request, &applied);
    // The resolution journal is resolved on its own.
    let kept = f.confirm_acknowledged(&f.request(&child, "keep", "child-keep"));
    let resolution = f.intent(&child);
    assert_eq!(resolution["acknowledged"], true);
    assert_eq!(resolution["settled_by"], kept["receipt"]["resolution_id"]);
    assert_eq!(
        f.lookup("interrupted-resolution", false)["state"],
        "resolved_by_another_request"
    );
    assert!(unacknowledged(&f.intent(&journal)));
    let fenced = f.env.cyber(&["exec", "--session", &f.session, "fenced"]);
    assert!(!fenced.status.success());
    // The original journal still needs its own reviewed settlement.
    f.confirm_acknowledged(&f.request(&journal, "keep", "parent-keep"));
    f.assert_uniform("after");
    f.assert_quiet(&messages, requests);
    f.env.exec(Some(&f.session), "after recovery");
}

/// The killed resolution left its own incomplete journal beside the original.
fn interrupted_child(f: &Fixture, journal: &str, applied: &str) -> String {
    let journals = f.journals();
    assert_eq!(journals.len(), 2, "{journals:?}");
    let child = journals
        .iter()
        .find(|id| id.starts_with("restore_resolution_"))
        .unwrap()
        .clone();
    let resolution = f.intent(&child);
    assert_eq!(resolution["applied"], json!([applied]));
    assert_eq!(resolution["completed"], false);
    assert!(unacknowledged(&resolution));
    assert!(unacknowledged(&f.intent(journal)));
    f.assert_uniform("after");
    child
}

/// Repeating the interrupted request reports it pending and never touches files.
fn assert_not_redispatched(f: &Fixture, request: &Value, applied: &str) {
    std::fs::write(f.env.directory.join(applied), "owner edit").unwrap();
    let retry = recovery_confirm_cli(&f.env, &f.session, request);
    assert_eq!(retry.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&retry.stderr);
    assert!(stderr.contains("Recovery is not acknowledged"), "{stderr}");
    let retry: Value = serde_json::from_slice(&retry.stdout).unwrap();
    assert_eq!(retry["state"], "pending");
    assert_eq!(
        f.lookup("interrupted-resolution", false)["state"],
        "pending"
    );
    assert_eq!(f.env.file(applied).as_deref(), Some("owner edit"));
    std::fs::write(f.env.directory.join(applied), "after").unwrap();
}

/// An interrupted staged code undo in a Session that also has an inactive branch:
/// keep or rollback settles the native journal without staging, committing or
/// discarding conversation state, and the branch stays exportable.
fn staged_undo_partial(f: Fixture, action: &str) {
    let branch = inactive_branch(&f);
    let messages = f.env.messages(&f.session);
    let selection = reviewed_selection(&f.env, &f.first, "code", "stage");
    let mut body = selection.clone();
    body["confirmation"] = reviewed_preview(&f.env, &f.session, &selection)["confirmation"].clone();
    let path = format!("/sessions/{}/rewind/confirm", f.session);
    let body = body.to_string();
    f.kill_at(
        "copy-restore-progress-recorded",
        &["api", "POST", &path, "-d", &body],
    );
    let (journal, applied) = first_progress(&f);
    assert!(session_revert(&f.env, &f.session).is_null());
    assert_eq!(f.env.messages(&f.session), messages);
    let requests = f.model.requests().len();
    f.confirm_acknowledged(&f.request(&journal, action, &format!("staged-{action}")));
    f.assert_resolved(action, &applied);
    // Recovery neither stages nor commits the interrupted undo and keeps the branch.
    assert!(session_revert(&f.env, &f.session).is_null());
    assert_branch_kept(&f, &branch);
    f.assert_quiet(&messages, requests);
    // A new staged undo is admitted and cleared normally afterwards.
    assert!(!reviewed_apply(&f.env, &f.session, &selection).is_null());
    f.assert_uniform("before");
    let clear = reviewed_selection(&f.env, &f.first, "code", "clear");
    reviewed_apply(&f.env, &f.session, &clear);
    assert!(session_revert(&f.env, &f.session).is_null());
}

/// Add a third turn and rewind the conversation before it, leaving it inactive.
fn inactive_branch(f: &Fixture) -> String {
    f.env.exec(Some(&f.session), "third prompt");
    let users = f.env.user_ids(&f.session);
    assert_eq!(users.len(), 3);
    let preview = rewind_preview(&f.env, &f.session, &users[2], "conversation");
    json(&confirm_rewind(
        &f.env,
        &f.session,
        &users[2],
        "conversation",
        &preview,
    ));
    assert_branch_kept(f, &users[2]);
    users[2].clone()
}

/// The branch message is inactive in the tree and still exportable.
fn assert_branch_kept(f: &Fixture, message: &str) {
    assert!(
        f.env
            .tree(&f.session)
            .iter()
            .any(|node| node["id"] == message && node["active"] == false)
    );
    let exported = json(
        &f.env
            .cyber(&["sessions", "export", &f.session, "--all-branches"]),
    );
    assert!(exported.to_string().contains(message));
}

#[test]
fn interrupted_staged_undo_with_an_inactive_branch_is_kept() {
    staged_undo_partial(Fixture::new(), "keep");
}

#[test]
fn interrupted_staged_undo_with_an_inactive_branch_is_rolled_back() {
    staged_undo_partial(Fixture::new(), "rollback");
}

/// The private journal namespace is replaced (by an equal-content copy, then by a
/// symlink alias) between review and confirmation: confirmation refuses before any
/// source effect, and the reviewed recovery succeeds once the original is back.
#[test]
fn replaced_private_journal_namespace_refuses_recovery_without_effects() {
    namespace_swap(Fixture::new());
}

fn namespace_swap(f: Fixture) {
    let messages = f.env.messages(&f.session);
    let (journal, applied) = kill_after_first_progress(&f);
    let request = f.request(&journal, "rollback", "namespace-swap");
    let journals = f.namespace().join("journals");
    let retained = f.namespace().join("journals.retained");
    std::fs::rename(&journals, &retained).unwrap();
    let copied = Command::new("cp")
        .arg("-Rp")
        .arg(&retained)
        .arg(&journals)
        .status()
        .unwrap();
    assert!(copied.success());
    let refusals = |label: &str, reason: &str| {
        let refused = recovery_confirm_cli(&f.env, &f.session, &request);
        assert!(!refused.status.success(), "{label} namespace was admitted");
        let stderr = String::from_utf8_lossy(&refused.stderr);
        assert!(stderr.contains(reason), "{label}: {stderr}");
        assert_eq!(f.env.file(&applied).as_deref(), Some("before"), "{label}");
        let mut names: Vec<String> = std::fs::read_dir(&retained)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, vec![journal.clone()], "{label}");
    };
    refusals("copied", "Intent slot identity changed");
    std::fs::remove_dir_all(&journals).unwrap();
    std::os::unix::fs::symlink(&retained, &journals).unwrap();
    // The alias is never followed.
    refusals("aliased", "Not a directory");
    std::fs::remove_file(&journals).unwrap();
    std::fs::rename(&retained, &journals).unwrap();
    f.confirm_acknowledged(&request);
    f.assert_uniform("after");
    assert_eq!(f.env.messages(&f.session), messages);
}

/// A corrupt, then missing, retained `intent.json` refuses listing, review and an
/// already reviewed confirmation without source effects; with the exact record back
/// in place the reviewed request settles normally.
fn damaged_intent(f: Fixture) {
    let messages = f.env.messages(&f.session);
    let (journal, applied) = kill_after_first_progress(&f);
    let request = f.request(&journal, "rollback", "damaged-intent");
    let intent = f
        .namespace()
        .join("journals")
        .join(&journal)
        .join("intent.json");
    let bytes = std::fs::read(&intent).unwrap();
    let moved = intent.with_file_name("moved-intent.json");
    let refused = |reason: &str| {
        for args in [
            vec!["sessions", "recovery", &f.session, "--format", "json"],
            vec![
                "sessions", "recovery", &f.session, &journal, "--action", "rollback", "--format",
                "json",
            ],
        ] {
            let stderr = exits(&f.env.cyber(&args), 2);
            assert!(stderr.contains(reason), "{stderr}");
        }
        assert!(
            !recovery_confirm_cli(&f.env, &f.session, &request)
                .status
                .success()
        );
        assert_eq!(f.env.file(&applied).as_deref(), Some("before"));
        assert_eq!(f.journals(), vec![journal.clone()]);
    };
    std::fs::write(&intent, b"{not json").unwrap();
    refused("key must be a string");
    std::fs::write(&intent, &bytes).unwrap();
    std::fs::rename(&intent, &moved).unwrap();
    refused("No such file or directory");
    std::fs::rename(&moved, &intent).unwrap();
    f.confirm_acknowledged(&request);
    f.assert_uniform("after");
    assert_eq!(f.env.messages(&f.session), messages);
}

#[test]
fn damaged_retained_intent_refuses_recovery_without_effects() {
    damaged_intent(Fixture::new());
}

/// Managed Git (`native-git-acceptance`) equivalents of the file-copy cuts above.
#[cfg(feature = "native-git-acceptance")]
mod managed {
    use super::*;

    #[test]
    fn git_restore_killed_after_a_progress_record_is_kept() {
        progress_cut_resolution(Fixture::managed(), "keep");
    }
    #[test]
    fn git_restore_killed_after_a_progress_record_is_rolled_back() {
        progress_cut_resolution(Fixture::managed(), "rollback");
    }
    #[test]
    fn git_interrupted_staged_undo_with_an_inactive_branch_is_kept() {
        staged_undo_partial(Fixture::managed(), "keep");
    }
    #[test]
    fn git_interrupted_staged_undo_with_an_inactive_branch_is_rolled_back() {
        staged_undo_partial(Fixture::managed(), "rollback");
    }
    #[test]
    fn git_replaced_private_journal_namespace_refuses_recovery() {
        namespace_swap(Fixture::managed());
    }
    #[test]
    fn git_restore_killed_after_filesystem_completion_is_rolled_back() {
        completion_cut_resolution(Fixture::managed(), "rollback");
    }
    #[test]
    fn git_paired_recovery_killed_after_writer_commit_reconciles() {
        paired_case(Fixture::managed());
    }
    #[test]
    fn git_interrupted_resolution_is_retained_as_independent_evidence() {
        interrupted_case(Fixture::managed());
    }
    #[test]
    fn git_damaged_retained_intent_refuses_recovery() {
        damaged_intent(Fixture::managed());
    }
}

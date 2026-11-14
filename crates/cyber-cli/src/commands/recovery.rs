//! Owner recovery through the existing Session-bound server protocol.
use clap::{Args, ValueEnum};
use cyber_client::Client;
use cyber_server::runtime::{
    SnapshotRecovery, SnapshotRecoveryAction, SnapshotRecoveryConfirmation, SnapshotRecoveryImage,
    SnapshotRecoveryResult, SnapshotRecoveryReview, SnapshotRecoverySelection,
    SnapshotRecoveryState,
};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use super::sessions::{api, clean, encode, session_id};
use crate::{cli::GlobalArgs, error::CliError, output};

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum Action {
    Keep,
    Rollback,
}
impl From<Action> for SnapshotRecoveryAction {
    fn from(action: Action) -> Self {
        match action {
            Action::Keep => Self::Keep,
            Action::Rollback => Self::Rollback,
        }
    }
}
#[derive(Debug, Args)]
pub struct RecoveryArgs {
    /// Session ID, exact title or unique ID prefix.
    pub session: String,
    /// Exact retained restore journal ID; omit to list journals.
    #[arg(value_parser = journal_id, requires = "action")]
    pub journal: Option<String>,
    /// Keep reviewed live files or roll back only proven restore effects.
    #[arg(long, value_enum, requires = "journal")]
    pub action: Option<Action>,
    /// Apply the exact previously reviewed recovery fingerprint.
    #[arg(long, value_parser = fingerprint, requires_all = ["journal", "request_id"])]
    pub confirm: Option<String>,
    /// Stable request identity; retain it for lookup after interruption or a lost reply.
    #[arg(long, value_parser = request_id, requires = "confirm")]
    pub request_id: Option<String>,
}
#[derive(Debug, Args)]
pub struct RequestArgs {
    /// Session ID, exact title or unique ID prefix.
    pub session: String,
    /// Exact retained request ID from recovery confirmation.
    #[arg(value_parser = request_id)]
    pub request_id: String,
    /// Reconcile native acknowledgement of the committed request; never replay source work.
    #[arg(long)]
    pub acknowledge: bool,
}

fn journal_id(value: &str) -> Result<String, String> {
    if value.starts_with("restore_")
        && value.len() <= 256
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        Ok(value.into())
    } else {
        Err("expected an exact restore journal ID from sessions recovery".into())
    }
}
pub(super) fn fingerprint(value: &str) -> Result<String, String> {
    if value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(value.into())
    } else {
        Err("expected the 64-character lowercase fingerprint from recovery review".into())
    }
}
pub(super) fn request_id(value: &str) -> Result<String, String> {
    if matches!(value, "." | "..") {
        return Err("a dot path segment cannot be addressed as a recovery request; choose a stable named ID".into());
    }
    if !value.is_empty() && value.len() <= 256 && !value.contains('\0') {
        Ok(value.into())
    } else {
        Err("recovery request ID must contain 1-256 bytes without NUL".into())
    }
}
pub(super) fn data<T: DeserializeOwned>(reply: Value) -> Result<T, CliError> {
    serde_json::from_value(reply["data"].clone())
        .map_err(|error| CliError::runtime(format!("Invalid recovery response: {error}")))
}
fn review_response(
    reply: Value,
    session: &str,
    selection: &SnapshotRecoverySelection,
) -> Result<SnapshotRecoveryReview, CliError> {
    let review: SnapshotRecoveryReview = data(reply)?;
    if review.journal_id != selection.journal_id
        || review.action != selection.action
        || (review.confirmation.is_some() && review.origin.is_none())
        || review.origin.as_ref().is_some_and(|origin| {
            origin.session_id != session || origin.binding.journal_id != selection.journal_id
        })
    {
        return Err(CliError::runtime(
            "Recovery review does not match the selected Session and journal",
        ));
    }
    Ok(review)
}
fn result_response(
    reply: Value,
    session: &str,
    request: &str,
) -> Result<SnapshotRecoveryResult, CliError> {
    let result: SnapshotRecoveryResult = data(reply)?;
    if result.request.request_id != request
        || result.original.session_id != session
        || result.original.binding.journal_id != result.request.journal_id
    {
        return Err(CliError::runtime(
            "Recovery reply does not match the selected Session and request",
        ));
    }
    Ok(result)
}
fn lookup_hint(session: &str, request_id: &str) -> String {
    if session
        .chars()
        .chain(request_id.chars())
        .any(char::is_control)
    {
        return format!(
            "Inspect the retained request with cyber sessions recovery-request using the original IDs (JSON): Session {}, request {}; do not choose a new request ID to retry",
            json!(session),
            json!(request_id)
        );
    }
    // Shell quoting preserves unusual IDs exactly when copied from an error.
    format!(
        "Inspect the retained request with cyber sessions recovery-request -- {} {}; do not choose a new request ID to retry",
        shell_words::quote(session),
        shell_words::quote(request_id),
    )
}

pub async fn run(client: &Client, args: RecoveryArgs, global: &GlobalArgs) -> Result<(), CliError> {
    let id = session_id(client, &args.session).await?;
    let base = format!("/sessions/{}/rewind/recovery", encode(&id));
    let Some(journal_id) = args.journal else {
        let journals: Vec<SnapshotRecovery> = data(client.get(&base).await.map_err(api)?)?;
        return list(&journals, global);
    };
    let action = args
        .action
        .ok_or_else(|| CliError::usage("Recovery requires --action"))?
        .into();
    if let Some(confirmation) = args.confirm {
        let request_id = args
            .request_id
            .ok_or_else(|| CliError::usage("Recovery confirmation requires --request-id"))?;
        let hint = lookup_hint(&id, &request_id);
        // The caller-provided identity stays unchanged on every transport attempt.
        let body = SnapshotRecoveryConfirmation {
            journal_id,
            action,
            confirmation,
            request_id,
        };
        let reply = client
            .post(&format!("{base}/confirm"), json!(&body))
            .await
            .map_err(|error| api(error).with_hint(&hint))?;
        let result = result_response(reply, &id, &body.request_id)
            .map_err(|error| error.with_hint(&hint))?;
        if result.request != body {
            return Err(
                CliError::runtime("Recovery reply does not match the confirmed request")
                    .with_hint(hint),
            );
        }
        return result_output(&result, global, ResultOperation::Confirm);
    }
    let selection = SnapshotRecoverySelection { journal_id, action };
    let reply = client
        .post(&format!("{base}/review"), json!(&selection))
        .await
        .map_err(api)?;
    let review = review_response(reply, &id, &selection)?;
    review_output(&review, global)
}

pub async fn request(
    client: &Client,
    args: RequestArgs,
    global: &GlobalArgs,
) -> Result<(), CliError> {
    let id = session_id(client, &args.session).await?;
    let path = format!(
        "/sessions/{}/rewind/recovery/requests/{}",
        encode(&id),
        encode(&args.request_id)
    );
    let reply = if args.acknowledge {
        client.post(&format!("{path}/acknowledge"), json!({})).await
    } else {
        client.get(&path).await
    }
    .map_err(|error| api(error).with_hint(lookup_hint(&id, &args.request_id)))?;
    let result = result_response(reply, &id, &args.request_id)
        .map_err(|error| error.with_hint(lookup_hint(&id, &args.request_id)))?;
    let operation = if args.acknowledge {
        ResultOperation::Acknowledge
    } else {
        ResultOperation::Lookup
    };
    result_output(&result, global, operation)
}

fn list(journals: &[SnapshotRecovery], global: &GlobalArgs) -> Result<(), CliError> {
    if output::is_json(global.format) {
        return output::json(&journals);
    }
    if journals.is_empty() {
        println!("No retained restore journals.");
    }
    for journal in journals {
        println!(
            "{}  filesystem_complete={}  observed_applied={}/{}",
            clean(&journal.id),
            journal.filesystem_complete,
            journal.applied.len(),
            journal.paths.len()
        );
    }
    if !journals.is_empty() {
        println!(
            "Filesystem completion does not establish acknowledgement. Review an exact journal with --action keep or --action rollback."
        );
    }
    Ok(())
}
fn action_name(action: SnapshotRecoveryAction) -> &'static str {
    match action {
        SnapshotRecoveryAction::Keep => "keep",
        SnapshotRecoveryAction::Rollback => "rollback",
    }
}
fn image(image: Option<&SnapshotRecoveryImage>) -> String {
    match image {
        None => "absent".into(),
        Some(image) => format!(
            "{:?} {} ({} bytes, mode {:o}, uid {}, gid {}, native {})",
            image.kind,
            clean(&image.hash),
            image.bytes,
            image.mode,
            image.uid,
            image.gid,
            image
                .native_identity
                .as_deref()
                .map(clean)
                .unwrap_or_else(|| "unavailable".into())
        ),
    }
}
fn literal(value: &str) -> String {
    format!("{value:?}")
}
fn review_output(review: &SnapshotRecoveryReview, global: &GlobalArgs) -> Result<(), CliError> {
    if output::is_json(global.format) {
        return output::json(review);
    }
    println!(
        "Recovery review: {} ({})",
        clean(&review.journal_id),
        action_name(review.action)
    );
    println!(
        "Filesystem complete: {}; acknowledged: {}",
        review.filesystem_complete, review.acknowledged
    );
    if let Some(origin) = &review.origin {
        println!(
            "Original Session: {}; worker: {}",
            clean(&origin.session_id),
            clean(&origin.activity_id)
        );
        println!(
            "Location: {}",
            literal(&origin.binding.directory.to_string_lossy())
        );
    } else {
        println!("Original worker identity: unavailable");
    }
    for path in &review.paths {
        println!(
            "{} [{}] changes={}",
            literal(&path.path),
            clean(&path.state),
            path.changes
        );
        for (name, contents) in [
            ("current", &path.current),
            ("before", &path.recorded_before),
            ("after", &path.recorded_after),
            ("retained", &path.retained),
            ("proposed", &path.proposed),
        ] {
            println!("  {name}: {}", image(contents.as_ref()));
        }
    }
    retained_paths("Historical exclusions", review.exclusions.as_deref());
    retained_paths(
        "Protected created parents",
        review.recorded_created_parents.as_deref(),
    );
    for conflict in &review.conflicts {
        println!("Conflict: {}", literal(conflict));
    }
    println!("No files or conversation changed. External effects remain retained.");
    match &review.confirmation {
        Some(hash) => println!(
            "To apply this review, repeat with --confirm {} --request-id <stable-request-id>",
            clean(hash)
        ),
        None => println!("Confirmation unavailable; retain the recovery evidence."),
    }
    Ok(())
}
fn retained_paths(label: &str, paths: Option<&[String]>) {
    match paths {
        None => println!("{label}: unavailable historical evidence"),
        Some([]) => println!("{label}: none"),
        Some(paths) => {
            for path in paths {
                println!("{label}: {}", literal(path));
            }
        }
    }
}
enum ResultOperation {
    Confirm,
    Lookup,
    Acknowledge,
}
fn result_output(
    result: &SnapshotRecoveryResult,
    global: &GlobalArgs,
    operation: ResultOperation,
) -> Result<(), CliError> {
    if output::is_json(global.format) {
        output::json(result)?;
    } else {
        println!("Recovery request: {}", literal(&result.request.request_id));
        println!("Runtime state: {:?}", result.state);
        println!(
            "Native journal acknowledgement: {}",
            result
                .journal_acknowledged
                .map_or("not observed by this response", |value| if value {
                    "observed"
                } else {
                    "not acknowledged"
                })
        );
        println!("The conversation is unchanged. External effects remain retained.");
    }
    let incomplete = match operation {
        ResultOperation::Lookup => false,
        ResultOperation::Confirm => result.state != SnapshotRecoveryState::Acknowledged,
        ResultOperation::Acknowledge => {
            result.state != SnapshotRecoveryState::Acknowledged
                || result.journal_acknowledged != Some(true)
        }
    };
    if incomplete {
        return Err(
            CliError::runtime("Recovery is not acknowledged").with_hint(lookup_hint(
                &result.original.session_id,
                &result.request.request_id,
            )),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn invalid_or_unreviewed_recovery_arguments_fail_before_startup() {
        let hash = "a".repeat(64);
        let oversized = "x".repeat(257);
        for tail in [
            vec!["restore_one"],
            vec!["--action", "keep"],
            vec!["restore_one", "--action", "delete"],
            vec!["restore_one", "--action", "keep", "--confirm", &hash],
            vec!["restore_one", "--action", "keep", "--request-id", "one"],
            vec![
                "restore_one",
                "--action",
                "keep",
                "--confirm",
                "yes",
                "--request-id",
                "one",
            ],
            vec!["../restore_one", "--action", "keep"],
            vec![
                "restore_one",
                "--action",
                "keep",
                "--confirm",
                &hash,
                "--request-id",
                "..",
            ],
            vec![
                "restore_one",
                "--action",
                "keep",
                "--confirm",
                &hash,
                "--request-id",
                &oversized,
            ],
        ] {
            let mut args = vec!["cyber", "sessions", "recovery", "ses_one"];
            args.extend(tail);
            assert!(crate::cli::Cli::try_parse_from(&args).is_err(), "{args:?}");
        }
        for tail in [
            vec![],
            vec!["restore_one", "--action", "keep"],
            vec![
                "restore_one",
                "--action",
                "rollback",
                "--confirm",
                &hash,
                "--request-id",
                "one / #?%",
            ],
        ] {
            let mut args = vec!["cyber", "sessions", "recovery", "ses_one"];
            args.extend(tail);
            crate::cli::Cli::try_parse_from(args).unwrap();
        }
    }

    #[test]
    fn malformed_and_foreign_replies_cannot_be_presented_as_the_requested_recovery() {
        let selection = SnapshotRecoverySelection {
            journal_id: "restore_one".into(),
            action: SnapshotRecoveryAction::Keep,
        };
        let origin = json!({"version":1,"session_id":"ses_one","session_created_id":"created","activity_id":"act_one","worktree_id":null,
            "binding":{"directory":"/repo","root_identity":"root","namespace_identity":"namespace","journal_id":"restore_one","journal_identity":"journal"}});
        let mut review = json!({"data":{"journal_id":"restore_one","action":"keep","origin":null,"filesystem_complete":false,"acknowledged":false,"confirmation":null,"paths":[],"exclusions":null,"recorded_created_parents":null,"conflicts":["legacy proof unavailable"]}});
        review_response(review.clone(), "ses_one", &selection).unwrap();
        review["data"]["confirmation"] = "a".repeat(64).into();
        assert!(review_response(review.clone(), "ses_one", &selection).is_err());
        review["data"]["confirmation"] = Value::Null;
        review["data"]["journal_id"] = "restore_foreign".into();
        assert!(review_response(review.clone(), "ses_one", &selection).is_err());
        review["data"]["journal_id"] = "restore_one".into();
        review["data"]["origin"] = origin.clone();
        assert!(review_response(review, "ses_foreign", &selection).is_err());
        let mut result = json!({"data":{"request":{"journal_id":"restore_one","action":"keep","confirmation":"a".repeat(64),"request_id":"one / #?%"},"original":origin,"resolution":null,"state":"pending","receipt":null,"journal_acknowledged":null}});
        result_response(result.clone(), "ses_one", "one / #?%").unwrap();
        assert!(result_response(result.clone(), "ses_one", "other").is_err());
        assert!(result_response(result.clone(), "ses_foreign", "one / #?%").is_err());
        result["data"]["original"]["binding"]["journal_id"] = "restore_foreign".into();
        assert!(result_response(result, "ses_one", "one / #?%").is_err());
    }

    #[test]
    fn review_labels_preserve_distinct_paths_without_terminal_control_effects() {
        assert_ne!(literal("one\ntwo"), literal("one two"));
        let displayed = literal("one\n\x1b[31mtwo");
        assert!(!displayed.contains(['\n', '\x1b']));
        assert!(displayed.contains("\\n"));
    }

    #[test]
    fn request_lookup_never_accepts_confirmation_options_and_diagnostics_escape_controls() {
        for option in ["--action", "--confirm", "--request-id"] {
            assert!(
                crate::cli::Cli::try_parse_from([
                    "cyber",
                    "sessions",
                    "recovery-request",
                    "ses_one",
                    "request",
                    option,
                    "value"
                ])
                .is_err()
            );
        }
        for tail in [vec![], vec!["--acknowledge"]] {
            let mut args = vec![
                "cyber",
                "sessions",
                "recovery-request",
                "ses_one",
                "one / #?%",
            ];
            args.extend(tail);
            crate::cli::Cli::try_parse_from(args).unwrap();
        }
        crate::cli::Cli::try_parse_from([
            "cyber",
            "sessions",
            "recovery-request",
            "ses_one",
            "--",
            "-request",
        ])
        .unwrap();
        assert!(
            lookup_hint("ses_one", "-request").contains("recovery-request -- ses_one -request")
        );
        let hint = lookup_hint("ses_one", "line\n\x1b[31m");
        assert!(!hint.contains(['\n', '\x1b']));
        assert!(hint.contains("\\n\\u001b"));
        assert!(data::<SnapshotRecoveryReview>(json!({"data": {}})).is_err());
    }
}

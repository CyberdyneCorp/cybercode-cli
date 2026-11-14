//! Cancellation uses exact exited-worker proof; missing intents never license clearing fences.
use super::{
    recovery::{data, fingerprint, request_id},
    sessions::{api, clean, encode, session_id},
};
use crate::{cli::GlobalArgs, error::CliError, output};
use clap::Args;
use cyber_client::Client;
use cyber_server::runtime::{
    SnapshotPreparation, SnapshotPreparationConfirmation, SnapshotPreparationReceipt,
    SnapshotPreparationReview,
};
use serde_json::{Value, json};

#[derive(Debug, Args)]
pub struct PreparationArgs {
    /// Session ID, exact title or unique ID prefix.
    session: String,
    /// Exact snapshot activity ID; omit to list retained preparation candidates.
    #[arg(value_parser = activity_id)]
    activity: Option<String>,
    /// Cancel only the exact reviewed preparation before admitted source effects.
    #[arg(long, value_parser = fingerprint, requires_all = ["activity", "request_id"])]
    confirm: Option<String>,
    /// Stable cancellation identity to retain for lookup after interruption or reply loss.
    #[arg(long, value_parser = request_id, requires = "confirm")]
    request_id: Option<String>,
}
#[derive(Debug, Args)]
pub struct RequestArgs {
    /// Session ID, exact title or unique ID prefix.
    session: String,
    /// Exact snapshot activity ID from preparation review.
    #[arg(value_parser = activity_id)]
    activity: String,
    /// Exact retained cancellation request ID.
    #[arg(value_parser = request_id)]
    request_id: String,
    /// Reconcile the committed receipt's native metadata without source-file access or replay.
    #[arg(long)]
    acknowledge: bool,
}
fn activity_id(value: &str) -> Result<String, String> {
    if value.starts_with("op_")
        && value.len() <= 256
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        Ok(value.into())
    } else {
        Err("expected an exact snapshot activity ID from preparation recovery".into())
    }
}
fn hint(session: &str, activity: &str, request: &str) -> String {
    let ids = [session, activity, request];
    if ids.iter().flat_map(|id| id.chars()).any(char::is_control) {
        return format!(
            "Look up the retained cancellation with sessions recovery-preparation-request using the original IDs (JSON): {}; do not choose a new request ID to retry",
            json!(ids)
        );
    }
    format!(
        "Inspect the retained cancellation with cyber sessions recovery-preparation-request -- {} {} {}; do not choose a new request ID to retry",
        shell_words::quote(session),
        shell_words::quote(activity),
        shell_words::quote(request)
    )
}
fn origin_matches_lifetime(
    origin: Option<&cyber_server::runtime::SnapshotOrigin>,
    lifetime: &cyber_server::runtime::SnapshotLifetimeBinding,
) -> bool {
    origin.is_none_or(|origin| {
        origin.version == 1
            && lifetime.version == 2
            && origin.session_id == lifetime.owner.session_id
            && origin.activity_id == lifetime.owner.activity_id
            && origin.session_created_id == lifetime.owner.session_created_id
            && origin.worktree_id == lifetime.owner.worktree_id
            && origin.binding.directory == lifetime.owner.directory
            && origin.binding.root_identity == lifetime.root_identity
    })
}
fn review_response(
    reply: Value,
    session: &str,
    activity: &str,
) -> Result<SnapshotPreparationReview, CliError> {
    let review: SnapshotPreparationReview = data(reply)?;
    if review.lifetime.version != 2
        || review.lifetime.source_phase.is_none()
        || review.activity_id != activity
        || review.lifetime.owner.activity_id != activity
        || review.lifetime.owner.session_id != session
        || !origin_matches_lifetime(review.origin.as_ref(), &review.lifetime)
    {
        return Err(CliError::runtime(
            "Preparation review lacks matching current pre-effect ownership evidence",
        ));
    }
    Ok(review)
}
fn receipt_response(
    reply: Value,
    session: &str,
    activity: &str,
    request: &str,
) -> Result<SnapshotPreparationReceipt, CliError> {
    let receipt: SnapshotPreparationReceipt = data(reply)?;
    if receipt.activity_id != activity
        || receipt.request_id != request
        || receipt.lifetime.owner.activity_id != activity
        || receipt.lifetime.owner.session_id != session
        || receipt.source_admitted == Some(true)
        || (receipt.origin.is_some() && receipt.source_admitted != Some(false))
        || !origin_matches_lifetime(receipt.origin.as_ref(), &receipt.lifetime)
    {
        return Err(CliError::runtime(
            "Cancellation receipt does not match the selected pre-effect owner and request",
        ));
    }
    Ok(receipt)
}
pub async fn run(
    client: &Client,
    args: PreparationArgs,
    global: &GlobalArgs,
) -> Result<(), CliError> {
    let session = session_id(client, &args.session).await?;
    let base = format!("/sessions/{}/rewind/preparations", encode(&session));
    let Some(activity) = args.activity else {
        let list: Vec<SnapshotPreparation> = data(client.get(&base).await.map_err(api)?)?;
        return list_output(&list, global);
    };
    let base = format!("{base}/{}", encode(&activity));
    if let Some(confirmation) = args.confirm {
        let request_id = args
            .request_id
            .ok_or_else(|| CliError::usage("Preparation confirmation requires --request-id"))?;
        let lookup = hint(&session, &activity, &request_id);
        let body = SnapshotPreparationConfirmation {
            confirmation,
            request_id,
        };
        let reply = client
            .post(&format!("{base}/confirm"), json!(&body))
            .await
            .map_err(|error| api(error).with_hint(&lookup))?;
        let receipt = receipt_response(reply, &session, &activity, &body.request_id)
            .map_err(|error| error.with_hint(&lookup))?;
        if receipt.confirmation != body.confirmation {
            return Err(CliError::runtime(
                "Cancellation reply differs from the confirmed fingerprint",
            )
            .with_hint(lookup));
        }
        return receipt_output(&receipt, global, false);
    }
    let review = review_response(
        client
            .post(&format!("{base}/review"), json!({}))
            .await
            .map_err(api)?,
        &session,
        &activity,
    )?;
    review_output(&review, global)
}
pub async fn request(
    client: &Client,
    args: RequestArgs,
    global: &GlobalArgs,
) -> Result<(), CliError> {
    let session = session_id(client, &args.session).await?;
    let path = format!(
        "/sessions/{}/rewind/preparations/{}/requests/{}",
        encode(&session),
        encode(&args.activity),
        encode(&args.request_id)
    );
    let lookup = hint(&session, &args.activity, &args.request_id);
    let reply = if args.acknowledge {
        client.post(&format!("{path}/acknowledge"), json!({})).await
    } else {
        client.get(&path).await
    }
    .map_err(|error| api(error).with_hint(&lookup))?;
    let receipt = receipt_response(reply, &session, &args.activity, &args.request_id)
        .map_err(|error| error.with_hint(&lookup))?;
    receipt_output(&receipt, global, args.acknowledge)
}
fn list_output(list: &[SnapshotPreparation], global: &GlobalArgs) -> Result<(), CliError> {
    if output::is_json(global.format) {
        return output::json(&list);
    }
    if list.is_empty() {
        println!("No retained pre-effect preparation candidates.");
    }
    for item in list {
        println!(
            "{} · status={} · lifetime evidence={}",
            clean(&item.activity_id),
            clean(&item.status),
            item.lifetime_available
        );
    }
    println!(
        "Listing is not cancellation authority. Review an exact activity; live workers and unavailable historical proof refuse."
    );
    Ok(())
}
fn review_output(review: &SnapshotPreparationReview, global: &GlobalArgs) -> Result<(), CliError> {
    if output::is_json(global.format) {
        return output::json(review);
    }
    println!(
        "Preparation cancellation review: {}",
        clean(&review.activity_id)
    );
    println!(
        "Session: {}; Location: {:?}; lifetime version: {}",
        clean(&review.lifetime.owner.session_id),
        review.lifetime.owner.directory,
        review.lifetime.version
    );
    println!(
        "Native sequence: {}; Session sequence: {}",
        review.native_sequence, review.session_sequence
    );
    println!(
        "Published origin: {}; acquired checkout digest: {}",
        review.origin.is_some(),
        review
            .checkout_resources_digest
            .as_deref()
            .map(clean)
            .unwrap_or_else(|| "unavailable historical proof".into())
    );
    println!("{}", clean(&review.warning));
    println!(
        "No source files or transcript changed. To cancel this reviewed preparation, repeat with --confirm {} --request-id <stable-cancellation-id>",
        clean(&review.confirmation)
    );
    Ok(())
}
fn receipt_output(
    receipt: &SnapshotPreparationReceipt,
    global: &GlobalArgs,
    reconciled: bool,
) -> Result<(), CliError> {
    if output::is_json(global.format) {
        return output::json(receipt);
    }
    println!(
        "Committed preparation cancellation: {} · request {:?}",
        clean(&receipt.activity_id),
        receipt.request_id
    );
    println!(
        "Session: {}; Location: {:?}",
        clean(&receipt.lifetime.owner.session_id),
        receipt.lifetime.owner.directory
    );
    println!(
        "Lifetime version: {}; acquired checkout digest: {}",
        receipt.lifetime.version,
        receipt
            .checkout_resources_digest
            .as_deref()
            .map(clean)
            .unwrap_or_else(|| "unavailable historical proof".into())
    );
    if reconciled {
        println!("Acknowledgement reconciliation completed for this receipt.");
    } else {
        println!(
            "This receipt establishes durable cancellation; this response alone does not observe native acknowledgement. Use recovery-preparation-request --acknowledge to reconcile explicitly."
        );
    }
    println!(
        "Files and transcript are preserved. Independent process/checkout uncertainty and recovery evidence remain protected."
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn unreviewed_or_malformed_cancellations_fail_before_startup() {
        let hash = "a".repeat(64);
        for tail in [
            vec!["--confirm", &hash, "--request-id", "one"],
            vec!["op_one", "--confirm", &hash],
            vec!["op_one", "--request-id", "one"],
            vec!["op_one", "--confirm", "yes", "--request-id", "one"],
            vec!["../op_one"],
            vec!["op_one", "--force"],
            vec!["op_one", "--confirm", &hash, "--request-id", ".."],
        ] {
            let mut args = vec!["cyber", "sessions", "recovery-preparation", "ses_one"];
            args.extend(tail);
            assert!(crate::cli::Cli::try_parse_from(&args).is_err(), "{args:?}");
        }
        for tail in [
            vec![],
            vec!["op_one"],
            vec!["op_one", "--confirm", &hash, "--request-id", "one / #?%"],
        ] {
            let mut args = vec!["cyber", "sessions", "recovery-preparation", "ses_one"];
            args.extend(tail);
            crate::cli::Cli::try_parse_from(args).unwrap();
        }
        crate::cli::Cli::try_parse_from([
            "cyber",
            "sessions",
            "recovery-preparation-request",
            "--acknowledge",
            "--",
            "ses_one",
            "op_one",
            "-request",
        ])
        .unwrap();
        assert!(!hint("ses_one", "op_one", "line\n\x1b[31m").contains(['\n', '\x1b']));
    }
    fn lifetime() -> Value {
        json!({"version":2,"owner":{"activity_id":"op_one","session_id":"ses_one","session_created_id":"created","directory":"/project","worktree_id":null},"root_identity":"root","namespace_identity":"namespace","directory_identity":"directory","file_identity":"file","source_phase":"phase"})
    }
    #[test]
    fn receipts_and_reviews_must_match_the_selected_pre_effect_owner() {
        let review = json!({"data":{"activity_id":"op_one","lifetime":lifetime(),"native_sequence":0,"session_sequence":1,"confirmation":"a".repeat(64),"warning":"independent uncertainty remains","origin":null,"checkout_resources_digest":"d".repeat(64)}});
        review_response(review.clone(), "ses_one", "op_one").unwrap();
        let mut unsupported = review.clone();
        unsupported["data"]["lifetime"]["version"] = 1.into();
        assert!(review_response(unsupported, "ses_one", "op_one").is_err());
        let mut missing_phase = review.clone();
        missing_phase["data"]["lifetime"]["source_phase"] = Value::Null;
        assert!(review_response(missing_phase, "ses_one", "op_one").is_err());
        assert!(review_response(review.clone(), "ses_foreign", "op_one").is_err());
        assert!(review_response(review, "ses_one", "op_foreign").is_err());
        let mut receipt = json!({"data":{"activity_id":"op_one","request_id":"request / #?%","confirmation":"a".repeat(64),"lifetime":lifetime(),"native_sequence":0,"session_sequence":1,"origin":null,"source_admitted":false,"checkout_resources_digest":"d".repeat(64)}});
        receipt_response(receipt.clone(), "ses_one", "op_one", "request / #?%").unwrap();
        assert!(
            receipt_response(receipt.clone(), "ses_foreign", "op_one", "request / #?%").is_err()
        );
        assert!(
            receipt_response(receipt.clone(), "ses_one", "op_foreign", "request / #?%").is_err()
        );
        assert!(receipt_response(receipt.clone(), "ses_one", "op_one", "foreign").is_err());
        receipt["data"]["origin"] = json!({"version":1,"session_id":"ses_foreign","session_created_id":"created","activity_id":"op_one","worktree_id":null,"binding":{"directory":"/project","root_identity":"root","namespace_identity":"journal_namespace","journal_id":"restore_one","journal_identity":"journal"}});
        assert!(receipt_response(receipt.clone(), "ses_one", "op_one", "request / #?%").is_err());
        receipt["data"]["origin"] = Value::Null;
        receipt["data"]["source_admitted"] = true.into();
        assert!(receipt_response(receipt, "ses_one", "op_one", "request / #?%").is_err());
    }
}

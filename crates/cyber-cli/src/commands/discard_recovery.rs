//! `cyber sessions recovery-discard`: the local owner's two-step discard of one
//! unpublished construction stage. Listing and review never change anything; a
//! confirmation names the reviewed stage, its fingerprint and a stable request ID.
use super::{
    recovery::{data, fingerprint, request_id},
    sessions::{api, clean, encode, session_id},
};
use crate::{cli::GlobalArgs, error::CliError, output};
use clap::Args;
use cyber_client::Client;
use cyber_server::runtime::{
    ConstructionDiscardConfirmation, ConstructionDiscardReceipt, ConstructionDiscardReview,
    ConstructionDiscardSelection, ConstructionDiscardStatus, ConstructionInventory,
    ConstructionLocation, ConstructionStageKind,
};
use serde_json::json;

#[derive(Debug, Args)]
pub struct DiscardArgs {
    /// Session ID, exact title or unique ID prefix.
    session: String,
    /// Exact construction stage name from the listing; omit to list stages and pending discards.
    #[arg(value_parser = stage_name, conflicts_with = "activity")]
    stage: Option<String>,
    /// Review the only construction stage of this snapshot activity instead of naming it.
    #[arg(long, value_parser = activity_id)]
    activity: Option<String>,
    /// Discard only the reviewed stage; requires the stage name and --request-id.
    #[arg(long, value_parser = fingerprint, requires_all = ["stage", "request_id"])]
    confirm: Option<String>,
    /// Stable request identity; repeat it to resume after an interruption or a lost reply.
    #[arg(long, value_parser = request_id, requires = "confirm")]
    request_id: Option<String>,
}

#[derive(Debug, Args)]
pub struct DiscardRequestArgs {
    /// Session ID, exact title or unique ID prefix.
    session: String,
    /// Exact request ID used to confirm the discard.
    #[arg(value_parser = request_id)]
    request_id: String,
}

fn stage_name(value: &str) -> Result<String, String> {
    if cyber_core::construction_discard::valid_component(value)
        && ["preparing_", "ack_preparing_", "tmp_"]
            .iter()
            .any(|prefix| value.starts_with(prefix))
    {
        Ok(value.into())
    } else {
        Err("expected an exact construction stage name from the recovery-discard listing".into())
    }
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
        Err("expected an exact snapshot activity ID".into())
    }
}

/// The exact command that reviews `stage`, quoted for a shell.
pub(super) fn command(session: &str, stage: &str) -> String {
    format!(
        "cyber sessions recovery-discard {} {}",
        shell_words::quote(session),
        shell_words::quote(stage)
    )
}

fn hint(session: &str, request: &str) -> String {
    format!(
        "Look up or resume it with cyber sessions recovery-discard-request {} {}, or repeat the same confirmation; do not choose a new request ID",
        shell_words::quote(session),
        shell_words::quote(request)
    )
}

pub async fn run(client: &Client, args: DiscardArgs, global: &GlobalArgs) -> Result<(), CliError> {
    let session = session_id(client, &args.session).await?;
    let base = format!("/sessions/{}/rewind/constructions", encode(&session));
    if args.stage.is_none() && args.activity.is_none() {
        let inventory: ConstructionInventory = data(client.get(&base).await.map_err(api)?)?;
        return list_output(&inventory, &session, global);
    }
    if let (Some(confirmation), Some(stage)) = (args.confirm, &args.stage) {
        let request_id = args
            .request_id
            .ok_or_else(|| CliError::usage("Discard confirmation requires --request-id"))?;
        let lookup = hint(&session, &request_id);
        let body = ConstructionDiscardConfirmation {
            stage: stage.clone(),
            confirmation,
            request_id,
        };
        let reply = client
            .post(&format!("{base}/discard/confirm"), json!(&body))
            .await
            .map_err(|error| api(error).with_hint(&lookup))?;
        let receipt = receipt_response(reply, &session, &body)?;
        return receipt_output(&receipt, global);
    }
    let selection = ConstructionDiscardSelection {
        stage: args.stage,
        activity_id: args.activity,
    };
    let review: ConstructionDiscardReview = data(
        client
            .post(&format!("{base}/discard/review"), json!(&selection))
            .await
            .map_err(api)?,
    )?;
    if review.session_id != session
        || selection
            .stage
            .as_ref()
            .is_some_and(|stage| *stage != review.stage.name)
    {
        return Err(CliError::runtime(
            "Discard review does not match the selected Session and stage",
        ));
    }
    review_output(&review, global)
}

pub async fn request(
    client: &Client,
    args: DiscardRequestArgs,
    global: &GlobalArgs,
) -> Result<(), CliError> {
    let session = session_id(client, &args.session).await?;
    let path = format!(
        "/sessions/{}/rewind/constructions/discard/requests/{}",
        encode(&session),
        encode(&args.request_id)
    );
    let receipt: ConstructionDiscardReceipt = data(client.get(&path).await.map_err(api)?)?;
    if receipt.session_id != session || receipt.request_id != args.request_id {
        return Err(CliError::runtime(
            "Discard receipt does not match the selected Session and request",
        ));
    }
    receipt_output(&receipt, global)
}

fn receipt_response(
    reply: serde_json::Value,
    session: &str,
    body: &ConstructionDiscardConfirmation,
) -> Result<ConstructionDiscardReceipt, CliError> {
    let receipt: ConstructionDiscardReceipt = data(reply)?;
    if receipt.session_id != session
        || receipt.request_id != body.request_id
        || receipt.confirmation != body.confirmation
        || receipt.stage.name != body.stage
    {
        return Err(CliError::runtime(
            "Discard receipt differs from the confirmed stage, fingerprint or request",
        )
        .with_hint(hint(session, &body.request_id)));
    }
    Ok(receipt)
}

fn kind_name(kind: ConstructionStageKind) -> &'static str {
    match kind {
        ConstructionStageKind::CheckoutClaim => "checkout claim stage",
        ConstructionStageKind::CheckoutAcknowledgement => "checkout acknowledgement stage",
        ConstructionStageKind::CancellationRecord => "cancellation-record stage",
    }
}

fn location_note(location: ConstructionLocation) -> &'static str {
    match location {
        ConstructionLocation::Present => "",
        ConstructionLocation::Removed => " · Location removed",
    }
}

fn list_output(
    inventory: &ConstructionInventory,
    session: &str,
    global: &GlobalArgs,
) -> Result<(), CliError> {
    if output::is_json(global.format) {
        return output::json(inventory);
    }
    if inventory.stages.is_empty() && inventory.pending.is_empty() {
        println!("No construction evidence.");
    }
    for stage in &inventory.stages {
        println!(
            "{} · {} · owner {}{}",
            clean(&stage.name),
            kind_name(stage.kind),
            stage
                .activity_id
                .as_deref()
                .map(clean)
                .unwrap_or_else(|| "unknown (refused)".into()),
            location_note(stage.location)
        );
        println!("  review: {}", command(session, &stage.name));
    }
    for receipt in &inventory.pending {
        println!(
            "discard pending: {} · request {:?} · {}",
            clean(&receipt.stage.name),
            receipt.request_id,
            pending_note(receipt)
        );
    }
    println!(
        "Listing is not discard authority. Review one stage; a live constructor, a published record or a busy Session refuses."
    );
    Ok(())
}

fn pending_note(receipt: &ConstructionDiscardReceipt) -> String {
    match (&receipt.status, &receipt.detail) {
        (ConstructionDiscardStatus::Intent, _) => {
            "interrupted before completion; repeat the confirmation with the same request ID".into()
        }
        (_, Some(detail)) => format!("unlink retried by maintenance: {}", clean(detail)),
        _ => "unlink retried by maintenance".into(),
    }
}

fn review_output(review: &ConstructionDiscardReview, global: &GlobalArgs) -> Result<(), CliError> {
    if output::is_json(global.format) {
        return output::json(review);
    }
    let stage = &review.stage;
    println!("Construction discard review: {}", clean(&stage.name));
    println!(
        "Kind: {} · owner activity {} · Session {} (sequence {}){}",
        kind_name(stage.kind),
        clean(&stage.activity_id),
        clean(&review.session_id),
        review.session_sequence,
        location_note(stage.location)
    );
    println!(
        "Identity: device {} inode {} · {} bytes · modified {} ns · sha256 {}",
        stage.evidence.device,
        stage.evidence.inode,
        stage.evidence.size,
        stage.evidence.modified_ns,
        clean(&stage.evidence.sha256)
    );
    println!("Fences: {}", clean(&stage.fences.join("; ")));
    println!("{}", clean(&review.warning));
    println!(
        "Nothing changed. To discard this stage, repeat with --confirm {} --request-id <stable-request-id>",
        clean(&review.confirmation)
    );
    Ok(())
}

fn receipt_output(
    receipt: &ConstructionDiscardReceipt,
    global: &GlobalArgs,
) -> Result<(), CliError> {
    if output::is_json(global.format) {
        return output::json(receipt);
    }
    let status = match receipt.status {
        ConstructionDiscardStatus::Completed => "discarded",
        ConstructionDiscardStatus::Pending => "discard pending (unlink retried by maintenance)",
        ConstructionDiscardStatus::Intent => {
            "discard pending (interrupted; repeat the confirmation with the same request ID)"
        }
        ConstructionDiscardStatus::Changed => "not discarded: the stage changed",
    };
    println!(
        "{}: {} · request {:?} · by {}",
        status,
        clean(&receipt.stage.name),
        receipt.request_id,
        clean(&receipt.actor)
    );
    if let Some(detail) = &receipt.detail {
        println!("{}", clean(detail));
    }
    println!(
        "Restore journals, payload images and published records are untouched; the managed worktree was not removed."
    );
    Ok(())
}

/// Best-effort text pointer from `sessions recovery` to construction evidence and
/// pending discards. The journal listing already succeeded, so a failed lookup is
/// reported on stderr and never fails the command.
pub(super) async fn construction_hint(client: &Client, session: &str) {
    let path = format!("/sessions/{}/rewind/constructions", encode(session));
    let inventory: Result<ConstructionInventory, CliError> = match client.get(&path).await {
        Ok(reply) => data(reply),
        Err(error) => Err(api(error)),
    };
    match inventory {
        Ok(inventory) => {
            for stage in &inventory.stages {
                println!(
                    "Construction evidence {} ({}): discard it with {}",
                    clean(&stage.name),
                    kind_name(stage.kind),
                    command(session, &stage.name)
                );
            }
            for receipt in &inventory.pending {
                println!(
                    "discard pending: {} · request {:?} · {}",
                    clean(&receipt.stage.name),
                    receipt.request_id,
                    pending_note(receipt)
                );
            }
        }
        Err(error) => eprintln!(
            "Construction evidence listing unavailable: {}",
            clean(&error.message)
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn unreviewed_or_malformed_discards_fail_before_startup() {
        let hash = "a".repeat(64);
        let stage = "preparing_10_op_fixture_01JABCDEFGHJKMNPQRSTVWXYZ0";
        for tail in [
            vec!["--confirm", &hash, "--request-id", "one"],
            vec![stage, "--confirm", &hash],
            vec![stage, "--request-id", "one"],
            vec![stage, "--confirm", "yes", "--request-id", "one"],
            vec!["../preparing_x"],
            vec!["intent.json"],
            vec![stage, "--activity", "op_one"],
            vec![
                "--activity",
                "op_one",
                "--confirm",
                &hash,
                "--request-id",
                "one",
            ],
        ] {
            let mut args = vec!["cyber", "sessions", "recovery-discard", "ses_one"];
            args.extend(tail);
            assert!(crate::cli::Cli::try_parse_from(&args).is_err(), "{args:?}");
        }
        for tail in [
            vec![],
            vec![stage],
            vec!["--activity", "op_one"],
            vec![stage, "--confirm", &hash, "--request-id", "one / #?%"],
        ] {
            let mut args = vec!["cyber", "sessions", "recovery-discard", "ses_one"];
            args.extend(tail);
            crate::cli::Cli::try_parse_from(args).unwrap();
        }
        crate::cli::Cli::try_parse_from([
            "cyber",
            "sessions",
            "recovery-discard-request",
            "ses_one",
            "one",
        ])
        .unwrap();
        assert_eq!(
            command("ses_one", stage),
            format!("cyber sessions recovery-discard ses_one {stage}")
        );
    }
}

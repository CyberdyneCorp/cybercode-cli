//! Explicit reconciliation through an existing server; no local database or worker startup.
use super::*;
use serde_json::Value;

type Request = (&'static str, String, Option<Value>, Vec<(String, String)>);

pub(super) fn run(args: &MemoryArgs, ctx: &Context, global: &GlobalArgs) -> Result<(), CliError> {
    let (method, path, body, headers) = request(args)?;
    let registered = cyber_app::read_registration(&ctx.paths)
        .ok_or_else(|| CliError::runtime("No registered server; start cyber service first"))?;
    if registered.url.is_empty() {
        return Err(CliError::runtime(
            "Memory server recovery requires a TCP listener",
        ));
    }
    let password = std::fs::read_to_string(ctx.paths.state.join("password"))
        .map_err(|_| CliError::runtime("Cannot read existing server credentials"))?;
    if password.trim().is_empty() {
        return Err(CliError::runtime("Existing server credentials are empty"));
    }
    let client = cyber_client::Client::http(&registered.url, Some(password.trim().into()))
        .at(&ctx.location.display().to_string());
    let (status, value) = super::super::serve::runtime()?.block_on(async {
        tokio::time::timeout(
            std::time::Duration::from_secs(30),
            client.raw(method, &path, body, &headers),
        )
        .await
        .map_err(|_| {
            CliError::runtime(
                "Recovery response timed out; inspect the retained request key before retrying",
            )
        })?
        .map_err(|_| {
            CliError::runtime(
                "Recovery transport failed; inspect the retained request key before retrying",
            )
        })
    })?;
    if !(200..300).contains(&status) {
        return Err(CliError::runtime(format!(
            "Memory server request refused ({status}): {}",
            value["message"].as_str().unwrap_or("request failed")
        )));
    }
    if value["location"]["directory"].as_str() != ctx.location.to_str()
        || value.get("data").is_none()
    {
        return Err(CliError::runtime(
            "Memory server returned an invalid Location envelope",
        ));
    }
    if output::is_json(global.format) {
        return output::json(&value);
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&value).map_err(|e| CliError::runtime(e.to_string()))?
    );
    Ok(())
}

fn request(args: &MemoryArgs) -> Result<Request, CliError> {
    let scope = if args.global { "global" } else { "project" };
    let path = format!("/memory/recovery/{scope}");
    match &args.command {
        MemoryCmd::Recovery => Ok(("GET", path, None, Vec::new())),
        MemoryCmd::Recover {
            review,
            admission_review,
            key,
        } => {
            recovery::validate_review(review)?;
            let admission = admission_review.as_ref().ok_or_else(|| {
                CliError::usage(
                    "Server recovery requires --admission-review from recovery --server",
                )
            })?;
            cyber_server::http::RecoverMemory {
                storage_fingerprint: review.clone(),
                admission_fingerprint: admission.clone(),
            }
            .validate()
            .map_err(|_| {
                CliError::usage(
                    "Expected storage and admission fingerprints from recovery --server",
                )
            })?;
            let key = key
                .as_ref()
                .ok_or_else(|| CliError::usage("Server recovery requires a retained --key"))?;
            validate_key(key)?;
            Ok((
                "POST",
                path,
                Some(json!({"storage_fingerprint":review,"admission_fingerprint":admission})),
                vec![("idempotency-key".into(), key.clone())],
            ))
        }
        MemoryCmd::RecoveryRequest { key } => {
            validate_key(key)?;
            Ok((
                "GET",
                format!(
                    "/memory/recovery/requests?scope={scope}&key={}",
                    encode(key)
                ),
                None,
                Vec::new(),
            ))
        }
        _ => Err(CliError::usage(
            "--server is supported only for recovery, recover and recovery-request",
        )),
    }
}

fn validate_key(key: &str) -> Result<(), CliError> {
    if !(1..=128).contains(&key.len()) || !key.bytes().all(|b| (0x21..=0x7e).contains(&b)) {
        return Err(CliError::usage(
            "Recovery key must be 1-128 printable characters",
        ));
    }
    Ok(())
}

fn encode(key: &str) -> String {
    key.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
                char::from(b).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}
